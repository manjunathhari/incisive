//! Gemma chat from the owner's Ollama server (`POST {url}/api/chat`, HTTP
//! basic auth). Errors carry fixed codes only; the password, question, history
//! and passages are never logged or put in a URL.

use std::time::Duration;

use futures_util::StreamExt;
use secrecy::{ExposeSecret, SecretString};
use serde_json::Value;

use super::settings::ChatSettings;
use crate::memory::embed::parse_auth;
use crate::memory::settings::MemorySettings;

/// The JSON body of one chat call, minus the model (the client adds it).
/// Built by `prompt::grounded` (and, in a later task, a prior builder).
#[derive(Debug, Clone)]
pub struct GemmaRequest {
    body: Value,
}

impl GemmaRequest {
    pub(crate) fn new(body: Value) -> GemmaRequest {
        GemmaRequest { body }
    }

    pub fn body(&self) -> &Value {
        &self.body
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum GemmaError {
    Timeout,
    Unauthorized,
    Http(u16),
    /// The body or a stream line was not what Ollama sends, or the stream
    /// ended without its `done` line.
    BadBody,
    /// Ollama sent an `{"error": ...}` line.
    Upstream,
}

impl GemmaError {
    /// The server itself is unavailable or refusing us. Same rule as
    /// `EmbedError`.
    pub fn is_server_unavailable(&self) -> bool {
        match self {
            GemmaError::Timeout | GemmaError::Unauthorized => true,
            GemmaError::Http(c) => *c == 0 || *c >= 500,
            GemmaError::BadBody | GemmaError::Upstream => false,
        }
    }

    pub fn code(&self) -> &'static str {
        match self {
            GemmaError::Timeout => "timeout",
            GemmaError::Unauthorized => "unauthorized",
            GemmaError::Http(_) => "http",
            GemmaError::BadBody => "bad_body",
            GemmaError::Upstream => "upstream",
        }
    }
}

/// How a stream finished without an error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamEnd {
    /// Ollama's `done` line arrived.
    Complete,
    /// The time limit hit after some text had arrived.
    CutAtLimit,
}

pub struct GemmaChat {
    client: reqwest::Client,
    url: String,
    user: String,
    password: SecretString,
    model: String,
}

impl std::fmt::Debug for GemmaChat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GemmaChat")
            .field("url", &self.url)
            .field("model", &self.model)
            .finish_non_exhaustive()
    }
}

impl GemmaChat {
    /// `None` unless chat is on and both `GEMMA_URL` and `GEMMA_AUTH` are set
    /// (`GEMMA_AUTH` as `user:password`).
    pub fn from_settings(memory: &MemorySettings, chat: &ChatSettings) -> Option<GemmaChat> {
        if !chat.on {
            return None;
        }
        let url = memory.gemma_url.as_ref()?.trim_end_matches('/').to_string();
        let (user, password) = parse_auth(memory.gemma_auth.as_ref()?)?;
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .ok()?;
        Some(GemmaChat {
            client,
            url,
            user,
            password,
            model: chat.model.clone(),
        })
    }

    /// The request with the model and stream flag set. The caller's request
    /// is not changed.
    fn send_body(&self, req: &GemmaRequest, stream: bool) -> Value {
        let mut body = req.body().clone();
        if let Some(o) = body.as_object_mut() {
            o.insert("model".into(), Value::String(self.model.clone()));
            o.insert("stream".into(), Value::Bool(stream));
        }
        body
    }

    /// Sends the request and checks the status. `deadline` bounds the wait
    /// for the response head.
    async fn post(
        &self,
        body: &Value,
        deadline: tokio::time::Instant,
    ) -> Result<reqwest::Response, GemmaError> {
        let send = self
            .client
            .post(format!("{}/api/chat", self.url))
            .basic_auth(&self.user, Some(self.password.expose_secret()))
            .json(body)
            .send();
        let resp = match tokio::time::timeout_at(deadline, send).await {
            Err(_) => return Err(GemmaError::Timeout),
            Ok(Err(e)) if e.is_timeout() => return Err(GemmaError::Timeout),
            Ok(Err(_)) => return Err(GemmaError::Http(0)),
            Ok(Ok(r)) => r,
        };
        let status = resp.status();
        if status.as_u16() == 401 {
            return Err(GemmaError::Unauthorized);
        }
        if !status.is_success() {
            return Err(GemmaError::Http(status.as_u16()));
        }
        Ok(resp)
    }

    /// Streams the reply, calling `on_text` with each piece of text as it
    /// arrives. `limit` is one deadline over the whole call. At the limit:
    /// `Ok(CutAtLimit)` if some text arrived, `Err(Timeout)` if none did.
    /// A stream that ends without Ollama's `done` line is `BadBody`.
    pub async fn stream(
        &self,
        req: &GemmaRequest,
        limit: Duration,
        mut on_text: impl FnMut(&str),
    ) -> Result<StreamEnd, GemmaError> {
        let deadline = tokio::time::Instant::now() + limit;
        let resp = self.post(&self.send_body(req, true), deadline).await?;
        let mut body = resp.bytes_stream();
        // Bytes, not strings: a line or a character can split across chunks.
        let mut buf: Vec<u8> = Vec::new();
        let mut got_text = false;
        loop {
            let next = match tokio::time::timeout_at(deadline, body.next()).await {
                Err(_) => {
                    return if got_text {
                        Ok(StreamEnd::CutAtLimit)
                    } else {
                        Err(GemmaError::Timeout)
                    };
                }
                Ok(n) => n,
            };
            match next {
                Some(Ok(chunk)) => buf.extend_from_slice(&chunk),
                Some(Err(e)) if e.is_timeout() => return Err(GemmaError::Timeout),
                Some(Err(_)) => return Err(GemmaError::Http(0)),
                None => {
                    // A last line with no newline.
                    if buf.iter().any(|b| !b.is_ascii_whitespace()) {
                        let line = std::mem::take(&mut buf);
                        if handle_line(&line, &mut on_text, &mut got_text)? {
                            return Ok(StreamEnd::Complete);
                        }
                    }
                    return Err(GemmaError::BadBody);
                }
            }
            while let Some(i) = buf.iter().position(|&b| b == b'\n') {
                let line: Vec<u8> = buf.drain(..=i).collect();
                if handle_line(&line, &mut on_text, &mut got_text)? {
                    return Ok(StreamEnd::Complete);
                }
            }
        }
    }

    /// One non-streaming call whose `message.content` is JSON; returns that
    /// JSON. `limit` bounds the whole call.
    pub async fn json(&self, req: &GemmaRequest, limit: Duration) -> Result<Value, GemmaError> {
        let deadline = tokio::time::Instant::now() + limit;
        let resp = self.post(&self.send_body(req, false), deadline).await?;
        let bytes = match tokio::time::timeout_at(deadline, resp.bytes()).await {
            Err(_) => return Err(GemmaError::Timeout),
            Ok(Err(e)) if e.is_timeout() => return Err(GemmaError::Timeout),
            Ok(Err(_)) => return Err(GemmaError::Http(0)),
            Ok(Ok(b)) => b,
        };
        let outer: Value = serde_json::from_slice(&bytes).map_err(|_| GemmaError::BadBody)?;
        let content = outer
            .get("message")
            .and_then(|m| m.get("content"))
            .and_then(Value::as_str)
            .ok_or(GemmaError::BadBody)?;
        serde_json::from_str(content).map_err(|_| GemmaError::BadBody)
    }
}

/// One NDJSON line. `Ok(true)` on the `done` line.
fn handle_line(
    line: &[u8],
    on_text: &mut impl FnMut(&str),
    got_text: &mut bool,
) -> Result<bool, GemmaError> {
    if line.iter().all(|b| b.is_ascii_whitespace()) {
        return Ok(false);
    }
    let v: Value = serde_json::from_slice(line).map_err(|_| GemmaError::BadBody)?;
    if v.get("error").is_some() {
        return Err(GemmaError::Upstream);
    }
    if let Some(t) = v
        .get("message")
        .and_then(|m| m.get("content"))
        .and_then(Value::as_str)
        && !t.is_empty()
    {
        *got_text = true;
        on_text(t);
    }
    Ok(v.get("done").and_then(Value::as_bool) == Some(true))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chat::ChatMode;
    use crate::chat::passages::PassageKind;
    use crate::chat::prompt::{GroundedPassages, grounded};
    use crate::chat::retrieve::Hit;
    use base64::Engine;
    use base64::engine::general_purpose::STANDARD;
    use tokio::io::AsyncWriteExt;
    use tokio::net::TcpListener;
    use wiremock::matchers::{body_partial_json, header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const T: Duration = Duration::from_secs(5);

    fn settings(url: String) -> (MemorySettings, ChatSettings) {
        (
            MemorySettings {
                on: true,
                gemma_url: Some(url),
                gemma_auth: Some(SecretString::from("incisive:s3cret-pw")),
                ..MemorySettings::default()
            },
            ChatSettings {
                on: true,
                ..ChatSettings::default()
            },
        )
    }

    fn client(url: String) -> GemmaChat {
        let (m, c) = settings(url);
        GemmaChat::from_settings(&m, &c).expect("client")
    }

    fn req() -> GemmaRequest {
        let hit = Hit {
            passage_id: Some(1),
            qa_id: None,
            kind: PassageKind::Evidence,
            text: "text".into(),
            ref_ns: vec![],
            score: 1.0,
        };
        let p = GroundedPassages::new(vec![hit]).unwrap();
        grounded(&p, "question?", &[], ChatMode::Clinical)
    }

    fn line(content: &str) -> String {
        format!(
            "{}\n",
            serde_json::json!({"message": {"role": "assistant", "content": content}, "done": false})
        )
    }

    const DONE: &str = "{\"done\":true,\"done_reason\":\"stop\"}\n";

    /// A raw HTTP server: after the request is read it sends a chunked 200
    /// and each `(delay, bytes)` piece as its own HTTP chunk, then (if `hold`)
    /// keeps the connection open without finishing.
    async fn raw_server(pieces: Vec<(Duration, Vec<u8>)>, hold: bool) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            use tokio::io::AsyncReadExt;
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = vec![0u8; 65536];
            let _ = sock.read(&mut buf).await;
            sock.write_all(
                b"HTTP/1.1 200 OK\r\ncontent-type: application/x-ndjson\r\ntransfer-encoding: chunked\r\n\r\n",
            )
            .await
            .unwrap();
            for (delay, bytes) in pieces {
                tokio::time::sleep(delay).await;
                sock.write_all(format!("{:x}\r\n", bytes.len()).as_bytes())
                    .await
                    .unwrap();
                sock.write_all(&bytes).await.unwrap();
                sock.write_all(b"\r\n").await.unwrap();
                sock.flush().await.unwrap();
            }
            if hold {
                tokio::time::sleep(Duration::from_secs(30)).await;
            } else {
                sock.write_all(b"0\r\n\r\n").await.unwrap();
            }
        });
        format!("http://{addr}")
    }

    const MS: fn(u64) -> Duration = Duration::from_millis;

    #[tokio::test]
    async fn stream_sends_model_auth_and_body() {
        let server = MockServer::start().await;
        let body = format!("{}{}{}", line("Hel"), line("lo"), DONE);
        Mock::given(method("POST"))
            .and(path("/api/chat"))
            .and(header(
                "authorization",
                format!("Basic {}", STANDARD.encode("incisive:s3cret-pw")).as_str(),
            ))
            .and(body_partial_json(serde_json::json!({
                "model": "gemma3:4b", "stream": true, "keep_alive": "30m",
                "options": {"num_predict": 300}
            })))
            .respond_with(ResponseTemplate::new(200).set_body_string(body))
            .expect(1)
            .mount(&server)
            .await;
        let mut got = String::new();
        let end = client(format!("{}/", server.uri()))
            .stream(&req(), T, |t| got.push_str(t))
            .await
            .unwrap();
        assert_eq!(end, StreamEnd::Complete);
        assert_eq!(got, "Hello");
    }

    #[tokio::test]
    async fn stream_joins_chunks() {
        // One JSON line split across two HTTP chunks, and a two-byte
        // character split across two more.
        let l1 = line("one ");
        let (a, b) = l1.as_bytes().split_at(10);
        let l2 = line("caf\u{e9} ok");
        let at = l2.as_bytes().iter().position(|&x| x == 0xc3).unwrap() + 1;
        let (c, d) = l2.as_bytes().split_at(at);
        let url = raw_server(
            vec![
                (MS(0), a.to_vec()),
                (MS(30), b.to_vec()),
                (MS(30), c.to_vec()),
                (MS(30), d.to_vec()),
                (MS(0), DONE.as_bytes().to_vec()),
            ],
            false,
        )
        .await;
        let mut got = String::new();
        let mut calls = 0;
        let end = client(url)
            .stream(&req(), T, |t| {
                calls += 1;
                got.push_str(t)
            })
            .await
            .unwrap();
        assert_eq!(end, StreamEnd::Complete);
        assert_eq!(got, "one caf\u{e9} ok");
        assert_eq!(calls, 2);
    }

    #[tokio::test]
    async fn error_line_in_stream_is_an_error() {
        let server = MockServer::start().await;
        let body = format!("{}{{\"error\":\"model runner crashed\"}}\n", line("par"));
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_string(body))
            .mount(&server)
            .await;
        let mut got = String::new();
        let e = client(server.uri())
            .stream(&req(), T, |t| got.push_str(t))
            .await
            .unwrap_err();
        assert_eq!(e, GemmaError::Upstream);
        assert_eq!(e.code(), "upstream");
        assert!(!e.is_server_unavailable());
    }

    #[tokio::test]
    async fn stream_without_done_is_bad_body() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_string(line("cut off")))
            .mount(&server)
            .await;
        let e = client(server.uri())
            .stream(&req(), T, |_| {})
            .await
            .unwrap_err();
        assert_eq!(e, GemmaError::BadBody);
    }

    #[tokio::test]
    async fn limit_keeps_partial_text() {
        let url = raw_server(vec![(MS(0), line("partial ").into_bytes())], true).await;
        let mut got = String::new();
        let started = std::time::Instant::now();
        let end = client(url)
            .stream(&req(), MS(300), |t| got.push_str(t))
            .await
            .unwrap();
        assert_eq!(end, StreamEnd::CutAtLimit);
        assert_eq!(got, "partial ");
        assert!(started.elapsed() < Duration::from_secs(3));
    }

    #[tokio::test]
    async fn limit_with_no_text_is_a_timeout() {
        let url = raw_server(vec![], true).await;
        let e = client(url)
            .stream(&req(), MS(300), |_| panic!("no text expected"))
            .await
            .unwrap_err();
        assert_eq!(e, GemmaError::Timeout);
        assert_eq!(e.code(), "timeout");
        assert!(e.is_server_unavailable());

        // Slow response head, not just a slow body.
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string(DONE)
                    .set_delay(Duration::from_millis(800)),
            )
            .mount(&server)
            .await;
        let e = client(server.uri())
            .stream(&req(), MS(200), |_| {})
            .await
            .unwrap_err();
        assert_eq!(e, GemmaError::Timeout);
    }

    #[tokio::test]
    async fn html_404_and_401_map_to_codes() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(404).set_body_string("<html>nope</html>"))
            .mount(&server)
            .await;
        let e = client(server.uri())
            .stream(&req(), T, |_| {})
            .await
            .unwrap_err();
        assert_eq!(e, GemmaError::Http(404));
        assert_eq!(e.code(), "http");
        assert!(!e.is_server_unavailable());

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(401))
            .mount(&server)
            .await;
        let e = client(server.uri())
            .stream(&req(), T, |_| {})
            .await
            .unwrap_err();
        assert_eq!(e, GemmaError::Unauthorized);
        assert_eq!(e.code(), "unauthorized");
        assert!(e.is_server_unavailable());

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(503))
            .mount(&server)
            .await;
        let e = client(server.uri()).json(&req(), T).await.unwrap_err();
        assert_eq!(e, GemmaError::Http(503));
        assert!(e.is_server_unavailable());
    }

    #[tokio::test]
    async fn redirect_is_not_followed() {
        let target = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_string(DONE))
            .expect(0)
            .mount(&target)
            .await;
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(
                ResponseTemplate::new(302)
                    .insert_header("location", format!("{}/api/chat", target.uri()).as_str()),
            )
            .mount(&server)
            .await;
        let e = client(server.uri())
            .stream(&req(), T, |_| {})
            .await
            .unwrap_err();
        assert_eq!(e, GemmaError::Http(302));
    }

    #[tokio::test]
    async fn json_sends_stream_false_and_parses_content() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/chat"))
            .and(body_partial_json(serde_json::json!({
                "model": "gemma3:4b", "stream": false
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "message": {"role": "assistant", "content": "{\"a\": [1, 2]}"},
                "done": true
            })))
            .expect(1)
            .mount(&server)
            .await;
        let v = client(server.uri()).json(&req(), T).await.unwrap();
        assert_eq!(v, serde_json::json!({"a": [1, 2]}));
        // The request itself is untouched.
        assert_eq!(req().body()["stream"], true);

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "message": {"role": "assistant", "content": "not json"}, "done": true
            })))
            .mount(&server)
            .await;
        let e = client(server.uri()).json(&req(), T).await.unwrap_err();
        assert_eq!(e, GemmaError::BadBody);
    }

    #[tokio::test]
    async fn auth_not_in_debug() {
        let dbg = format!("{:?}", client("http://127.0.0.1:9".to_string()));
        assert!(!dbg.contains("s3cret-pw"));
        assert!(!dbg.contains("incisive:"));
        assert!(dbg.contains("gemma3:4b"));
    }

    #[test]
    fn none_unless_chat_on_and_configured() {
        let (m, c) = settings("http://127.0.0.1:9".into());
        assert!(GemmaChat::from_settings(&m, &c).is_some());
        let off = ChatSettings::default();
        assert!(GemmaChat::from_settings(&m, &off).is_none());
        assert!(GemmaChat::from_settings(&MemorySettings::default(), &c).is_none());
        let bad = MemorySettings {
            gemma_auth: Some(SecretString::from("nocolon")),
            ..m
        };
        assert!(GemmaChat::from_settings(&bad, &c).is_none());
    }
}
