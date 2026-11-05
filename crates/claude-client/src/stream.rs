//! Reading a streamed Messages reply: server-sent events rebuilt into the same
//! [`MessagesResponse`] a plain call returns. Nothing here logs reply text, and
//! [`StreamBreak`] carries counts only.

use serde::Deserialize;
use serde_json::Value;

use crate::{ClaudeError, ContentBlock, MessagesResponse, StopReason, Usage};

/// What ended a stream early.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BreakKind {
    /// The connection failed while reading.
    Transport,
    /// No event within the idle limit.
    Idle,
    /// The caller's overall deadline passed.
    Deadline,
    /// The body ended with no `message_stop`.
    EndedEarly,
    /// An `error` event.
    ApiError,
    /// An event that could not be read.
    Malformed,
}

impl BreakKind {
    /// Fixed kind for logs.
    pub fn as_str(self) -> &'static str {
        match self {
            BreakKind::Transport => "transport",
            BreakKind::Idle => "idle",
            BreakKind::Deadline => "deadline",
            BreakKind::EndedEarly => "ended_early",
            BreakKind::ApiError => "api_error",
            BreakKind::Malformed => "malformed",
        }
    }
}

/// What was known when a stream broke off. No reply text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamBreak {
    pub kind: BreakKind,
    /// At least one event had arrived.
    pub started: bool,
    /// The usage reported so far: `message_start`'s input tokens, and the
    /// output tokens of the latest `message_delta`. `None` before
    /// `message_start`, when the input is not known.
    pub usage: Option<Usage>,
    /// Bytes of model output received: text, thinking and tool input.
    pub output_bytes: usize,
}

impl std::fmt::Display for StreamBreak {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}, {} output bytes",
            self.kind.as_str(),
            self.output_bytes
        )
    }
}

/// Bytes off the wire, split into the `data:` payload of each event.
#[derive(Default)]
pub(crate) struct SseBuffer {
    buf: Vec<u8>,
}

impl SseBuffer {
    pub(crate) fn push(&mut self, chunk: &[u8]) {
        self.buf.extend_from_slice(chunk);
    }

    /// The next complete event's data. An event with no data (a comment or a
    /// keep-alive) is skipped; a piece may end in the middle of a character,
    /// so nothing is decoded until its event is whole.
    pub(crate) fn next_data(&mut self) -> Option<Vec<u8>> {
        loop {
            let (end, skip) = event_end(&self.buf)?;
            let event: Vec<u8> = self.buf.drain(..end + skip).take(end).collect();
            let mut data = Vec::new();
            for line in event.split(|b| *b == b'\n') {
                let line = line.strip_suffix(b"\r").unwrap_or(line);
                if let Some(rest) = line.strip_prefix(b"data:") {
                    if !data.is_empty() {
                        data.push(b'\n');
                    }
                    data.extend_from_slice(rest.strip_prefix(b" ").unwrap_or(rest));
                }
            }
            if !data.is_empty() {
                return Some(data);
            }
        }
    }
}

/// Where the first event in `buf` ends, and the length of its blank line.
fn event_end(buf: &[u8]) -> Option<(usize, usize)> {
    let mut i = 0;
    while i < buf.len() {
        if buf[i] == b'\n' {
            match buf.get(i + 1) {
                Some(b'\n') => return Some((i, 2)),
                Some(b'\r') if buf.get(i + 2) == Some(&b'\n') => return Some((i, 3)),
                _ => {}
            }
        }
        i += 1;
    }
    None
}

#[derive(Deserialize)]
struct MessageStart {
    message: StartedMessage,
}

#[derive(Deserialize)]
struct StartedMessage {
    #[serde(default)]
    id: String,
    usage: Usage,
}

#[derive(Deserialize)]
struct BlockStart {
    index: usize,
    content_block: ContentBlock,
}

#[derive(Deserialize)]
struct BlockDelta {
    index: usize,
    delta: Value,
}

#[derive(Deserialize)]
struct MessageDelta {
    #[serde(default)]
    delta: DeltaBody,
    #[serde(default)]
    usage: Option<DeltaUsage>,
}

#[derive(Deserialize, Default)]
struct DeltaBody {
    #[serde(default)]
    stop_reason: Option<StopReason>,
}

/// Cumulative counts; any of them may be absent.
#[derive(Deserialize)]
struct DeltaUsage {
    #[serde(default)]
    input_tokens: Option<u32>,
    #[serde(default)]
    output_tokens: Option<u32>,
    #[serde(default)]
    cache_creation_input_tokens: Option<u32>,
    #[serde(default)]
    cache_read_input_tokens: Option<u32>,
}

#[derive(Deserialize)]
struct ErrorEvent {
    error: ErrorBody,
}

#[derive(Deserialize)]
struct ErrorBody {
    #[serde(rename = "type")]
    error_type: String,
    #[serde(default)]
    message: String,
}

/// One content block as it is rebuilt.
struct Building {
    block: ContentBlock,
    /// A tool's input arrives as pieces of JSON.
    tool_json: String,
}

/// The reply so far.
#[derive(Default)]
pub(crate) struct StreamState {
    started: bool,
    stopped: bool,
    id: String,
    usage: Option<Usage>,
    stop_reason: Option<StopReason>,
    blocks: Vec<Option<Building>>,
    text: String,
    output_bytes: usize,
}

impl StreamState {
    pub(crate) fn stopped(&self) -> bool {
        self.stopped
    }

    pub(crate) fn interrupted(&self, kind: BreakKind) -> ClaudeError {
        ClaudeError::StreamInterrupted(StreamBreak {
            kind,
            started: self.started,
            usage: self.usage,
            output_bytes: self.output_bytes,
        })
    }

    /// One event's data. An unknown event is ignored; one that cannot be read
    /// breaks the stream, because a lost piece would corrupt the reply.
    pub(crate) fn apply(
        &mut self,
        data: &[u8],
        on_text: &(dyn Fn(&str) + Send + Sync),
    ) -> Result<(), ClaudeError> {
        let Ok(event) = serde_json::from_slice::<Value>(data) else {
            return Err(self.interrupted(BreakKind::Malformed));
        };
        let kind = event.get("type").and_then(Value::as_str).unwrap_or("");
        match kind {
            "message_start" => {
                let m: MessageStart = self.read(event)?;
                self.id = m.message.id;
                self.usage = Some(m.message.usage);
            }
            "content_block_start" => {
                let b: BlockStart = self.read(event)?;
                self.start_block(b);
            }
            "content_block_delta" => {
                let d: BlockDelta = self.read(event)?;
                self.delta(d, on_text);
            }
            "message_delta" => {
                let d: MessageDelta = self.read(event)?;
                if d.delta.stop_reason.is_some() {
                    self.stop_reason = d.delta.stop_reason;
                }
                if let Some(u) = d.usage {
                    let mut now = self.usage.unwrap_or_default();
                    now.input_tokens = u.input_tokens.unwrap_or(now.input_tokens);
                    now.output_tokens = u.output_tokens.unwrap_or(now.output_tokens);
                    now.cache_creation_input_tokens = u
                        .cache_creation_input_tokens
                        .unwrap_or(now.cache_creation_input_tokens);
                    now.cache_read_input_tokens = u
                        .cache_read_input_tokens
                        .unwrap_or(now.cache_read_input_tokens);
                    self.usage = Some(now);
                }
            }
            "message_stop" => self.stopped = true,
            "error" => {
                let e: ErrorEvent = self.read(event)?;
                return Err(if self.started {
                    self.interrupted(BreakKind::ApiError)
                } else {
                    error_before_start(e.error)
                });
            }
            // `ping`, `content_block_stop` and anything new.
            _ => {}
        }
        self.started = true;
        Ok(())
    }

    fn read<T: serde::de::DeserializeOwned>(&mut self, event: Value) -> Result<T, ClaudeError> {
        serde_json::from_value(event).map_err(|_| self.interrupted(BreakKind::Malformed))
    }

    fn start_block(&mut self, b: BlockStart) {
        if self.blocks.len() <= b.index {
            self.blocks.resize_with(b.index + 1, || None);
        }
        self.output_bytes += match &b.content_block {
            ContentBlock::Text { text } => text.len(),
            ContentBlock::Thinking { thinking, .. } => thinking.len(),
            ContentBlock::RedactedThinking { data } => data.len(),
            _ => 0,
        };
        self.blocks[b.index] = Some(Building {
            block: b.content_block,
            tool_json: String::new(),
        });
    }

    fn delta(&mut self, d: BlockDelta, on_text: &(dyn Fn(&str) + Send + Sync)) {
        let Some(Some(b)) = self.blocks.get_mut(d.index) else {
            return;
        };
        let piece = |k: &str| d.delta.get(k).and_then(Value::as_str).unwrap_or("");
        match d.delta.get("type").and_then(Value::as_str).unwrap_or("") {
            "text_delta" => {
                let t = piece("text");
                if let ContentBlock::Text { text } = &mut b.block {
                    text.push_str(t);
                    self.output_bytes += t.len();
                    self.text.push_str(t);
                    on_text(&self.text);
                }
            }
            "thinking_delta" => {
                let t = piece("thinking");
                if let ContentBlock::Thinking { thinking, .. } = &mut b.block {
                    thinking.push_str(t);
                    self.output_bytes += t.len();
                }
            }
            "signature_delta" => {
                if let ContentBlock::Thinking { signature, .. } = &mut b.block {
                    signature.push_str(piece("signature"));
                }
            }
            "input_json_delta" => {
                let t = piece("partial_json");
                b.tool_json.push_str(t);
                self.output_bytes += t.len();
            }
            _ => {}
        }
    }

    /// The whole reply, as [`crate::Client::create_message`] returns it.
    pub(crate) fn into_response(self) -> MessagesResponse {
        let content = self
            .blocks
            .into_iter()
            .flatten()
            .map(|b| match b.block {
                ContentBlock::ToolUse { id, name, input } if !b.tool_json.is_empty() => {
                    ContentBlock::ToolUse {
                        id,
                        name,
                        input: serde_json::from_str(&b.tool_json).unwrap_or(input),
                    }
                }
                other => other,
            })
            .collect();
        MessagesResponse {
            id: self.id,
            content,
            stop_reason: self.stop_reason,
            usage: self.usage.unwrap_or_default(),
        }
    }
}

/// An `error` event before any other: the same errors as a status code, so
/// the client's retry policy treats it the same.
fn error_before_start(e: ErrorBody) -> ClaudeError {
    match e.error_type.as_str() {
        "overloaded_error" => ClaudeError::Overloaded,
        "rate_limit_error" => ClaudeError::RateLimited { retry_after: None },
        _ => ClaudeError::Api {
            status: match e.error_type.as_str() {
                "invalid_request_error" => 400,
                "authentication_error" => 401,
                "permission_error" => 403,
                "not_found_error" => 404,
                "request_too_large" => 413,
                "timeout_error" => 504,
                _ => 500,
            },
            error_type: e.error_type,
            message: e.message,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_split_at_any_byte_come_out_whole() {
        let wire =
            "event: a\ndata: {\"x\":\"é\"}\n\n: comment\n\nevent: b\r\ndata: {\"y\":1}\r\n\r\n";
        for cut in 0..wire.len() {
            let mut b = SseBuffer::default();
            let mut out = Vec::new();
            for part in [&wire.as_bytes()[..cut], &wire.as_bytes()[cut..]] {
                b.push(part);
                while let Some(d) = b.next_data() {
                    out.push(String::from_utf8(d).unwrap());
                }
            }
            assert_eq!(out, ["{\"x\":\"é\"}", "{\"y\":1}"], "cut at {cut}");
        }
    }

    #[test]
    fn several_data_lines_join() {
        let mut b = SseBuffer::default();
        b.push(b"data: {\"a\":\ndata: 1}\n\n");
        assert_eq!(b.next_data().unwrap(), b"{\"a\":\n1}");
        assert!(b.next_data().is_none());
    }
}
