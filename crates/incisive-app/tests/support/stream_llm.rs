//! A scripted Claude whose streamed calls deliver their reply in pieces, as the
//! real client does, or break off part way. Plain calls go straight through.
#![allow(dead_code)]

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use async_trait::async_trait;
use claude_client::{
    BreakKind, ClaudeError, ContentBlock, MessagesRequest, MessagesResponse, StreamBreak, Usage,
};
use incisive_app::agent::llm::LlmClient;

/// One broken stream: its text so far and what it reports.
#[derive(Clone)]
pub struct Broken {
    pub text: String,
    pub usage: Option<Usage>,
    pub output_bytes: usize,
}

/// A stream that breaks after `text`, the input known.
pub fn broken(text: &str, input_tokens: u32) -> Broken {
    Broken {
        text: text.to_string(),
        usage: Some(Usage {
            input_tokens,
            output_tokens: 1,
            ..Usage::default()
        }),
        output_bytes: text.len(),
    }
}

pub struct StreamLlm {
    inner: Arc<dyn LlmClient>,
    breaks: Mutex<VecDeque<Broken>>,
    /// Requests that came by the streamed path.
    pub streamed: Mutex<Vec<MessagesRequest>>,
    /// Requests that came by the plain path.
    pub plain: Mutex<Vec<MessagesRequest>>,
    pub deadlines: Mutex<Vec<Option<Instant>>>,
}

impl StreamLlm {
    pub fn new(inner: Arc<dyn LlmClient>) -> StreamLlm {
        StreamLlm {
            inner,
            breaks: Mutex::default(),
            streamed: Mutex::default(),
            plain: Mutex::default(),
            deadlines: Mutex::default(),
        }
    }

    /// The next streamed calls break off like this, in order.
    pub fn breaking(self, b: Vec<Broken>) -> StreamLlm {
        *self.breaks.lock().unwrap() = b.into();
        self
    }
}

fn pieces(text: &str, on_text: &(dyn for<'t> Fn(&'t str) + Send + Sync)) {
    let mut end = 0;
    for (i, _) in text.char_indices() {
        if i - end >= 7 {
            on_text(&text[..i]);
            end = i;
        }
    }
    if !text.is_empty() {
        on_text(text);
    }
}

#[async_trait]
impl LlmClient for StreamLlm {
    async fn call(&self, req: &MessagesRequest) -> Result<MessagesResponse, ClaudeError> {
        self.plain.lock().unwrap().push(req.clone());
        self.inner.call(req).await
    }

    async fn stream(
        &self,
        req: &MessagesRequest,
        on_text: &(dyn for<'t> Fn(&'t str) + Send + Sync),
        deadline: Option<Instant>,
    ) -> Result<MessagesResponse, ClaudeError> {
        self.streamed.lock().unwrap().push(req.clone());
        self.deadlines.lock().unwrap().push(deadline);
        let plan = self.breaks.lock().unwrap().pop_front();
        if let Some(b) = plan {
            pieces(&b.text, on_text);
            return Err(ClaudeError::StreamInterrupted(StreamBreak {
                kind: BreakKind::Transport,
                started: true,
                usage: b.usage,
                output_bytes: b.output_bytes,
            }));
        }
        let resp = self.inner.call(req).await?;
        let text: String = resp
            .content
            .iter()
            .filter_map(|b| match b {
                ContentBlock::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        pieces(&text, on_text);
        Ok(resp)
    }
}
