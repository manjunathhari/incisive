use std::time::{Duration, Instant};

use reqwest::Url;
use secrecy::{ExposeSecret, SecretString};

use serde::Deserialize;

use crate::retry::{is_retryable, status_of};
use crate::stream::{self, BreakKind, StreamState};
use crate::{ClaudeError, MessagesRequest, MessagesResponse, RetryPolicy};

const DEFAULT_BASE_URL: &str = "https://api.anthropic.com";
const ANTHROPIC_VERSION: &str = "2023-06-01";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(120);
const SNIPPET_CHARS: usize = 200;
/// A streamed call is given up after this long with no event, pings included.
pub const STREAM_IDLE_TIMEOUT: Duration = Duration::from_secs(60);
/// Opening a connection for a streamed call (there is no whole-request limit).
const STREAM_CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

// No `Debug` derive: it would print the API key. See the manual impl below.
pub struct Client {
    http: reqwest::Client,
    /// For [`Client::stream_message`]: no whole-request timeout, which would
    /// cut a long reply off; the idle limit and the caller's deadline bound it.
    stream_http: reqwest::Client,
    stream_idle: Duration,
    api_key: SecretString,
    base_url: Url,
    retry: RetryPolicy,
}

impl std::fmt::Debug for Client {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Client {{ base_url: {}, api_key: \"[redacted]\" }}",
            self.base_url
        )
    }
}

#[derive(Deserialize)]
struct ApiErrorEnvelope {
    error: ApiErrorBody,
}

#[derive(Deserialize)]
struct ApiErrorBody {
    #[serde(rename = "type")]
    error_type: String,
    message: String,
}

impl Client {
    pub fn new(api_key: SecretString) -> Client {
        let base_url = DEFAULT_BASE_URL.parse().expect("default base URL is valid");
        Client::with_base_url(api_key, base_url)
    }

    pub fn with_base_url(api_key: SecretString, base_url: Url) -> Client {
        let http = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .build()
            .expect("reqwest client builds with static configuration");
        let stream_http = reqwest::Client::builder()
            .connect_timeout(STREAM_CONNECT_TIMEOUT)
            .build()
            .expect("reqwest client builds with static configuration");
        Client {
            http,
            stream_http,
            stream_idle: STREAM_IDLE_TIMEOUT,
            api_key,
            base_url,
            retry: RetryPolicy::default(),
        }
    }

    /// Per-attempt timeout (default 120 s). Mainly for tests and short calls.
    pub fn with_request_timeout(mut self, timeout: Duration) -> Client {
        self.http = reqwest::Client::builder()
            .timeout(timeout)
            .build()
            .expect("reqwest client builds with static configuration");
        self
    }

    pub fn with_retry_policy(mut self, retry: RetryPolicy) -> Client {
        self.retry = retry;
        self
    }

    /// The idle limit of a streamed call (default 60 s). Mainly for tests.
    pub fn with_stream_idle_timeout(mut self, idle: Duration) -> Client {
        self.stream_idle = idle;
        self
    }

    pub async fn create_message(
        &self,
        req: &MessagesRequest,
    ) -> Result<MessagesResponse, ClaudeError> {
        let max_attempts = self.retry.max_attempts.max(1);
        let mut attempt = 1;
        loop {
            let err = match self.send_once(req).await {
                Ok(resp) => return Ok(resp),
                Err(e) => e,
            };
            if attempt >= max_attempts || !is_retryable(&err) {
                return Err(err);
            }
            let retry_after = match &err {
                ClaudeError::RateLimited { retry_after } => *retry_after,
                _ => None,
            };
            let delay = self.retry.delay_for(attempt, retry_after);
            // Attempt number and status only: never headers or bodies.
            tracing::warn!(
                attempt,
                max_attempts,
                status = ?status_of(&err),
                delay_ms = delay.as_millis() as u64,
                "retrying Claude API request"
            );
            tokio::time::sleep(delay).await;
            attempt += 1;
        }
    }

    /// The same request as [`Client::create_message`] with `"stream": true`,
    /// rebuilt into the same [`MessagesResponse`]: same blocks, stop reason and
    /// usage. `on_text` gets the reply's text so far after every piece of it;
    /// thinking is kept in the response and never passed on.
    ///
    /// Limits: no whole-request limit. Each wait is at most the idle limit
    /// (no event, pings included) and the call ends at `deadline`, if any.
    ///
    /// Retries: before the first event, the client's retry policy, as for
    /// [`Client::create_message`]. Once events have started, a break is
    /// [`ClaudeError::StreamInterrupted`], with what was received, and is not
    /// retried here: the caller decides, and books the charge.
    pub async fn stream_message(
        &self,
        req: &MessagesRequest,
        on_text: &(dyn Fn(&str) + Send + Sync),
        deadline: Option<Instant>,
    ) -> Result<MessagesResponse, ClaudeError> {
        let max_attempts = self.retry.max_attempts.max(1);
        let mut attempt = 1;
        let mut earlier = None;
        loop {
            let err = match self.stream_once(req, on_text, deadline).await {
                Ok(resp) => return Ok(resp),
                Err(e) => e,
            };
            // An attempt that sent nothing says nothing: an earlier one was
            // sent, and may have been billed, so its error is the one to book.
            if matches!(
                &err,
                ClaudeError::StreamInterrupted(b) if b.kind == BreakKind::NotSent
            ) {
                return Err(earlier.unwrap_or(err));
            }
            if attempt >= max_attempts || !is_retryable(&err) {
                return Err(err);
            }
            let retry_after = match &err {
                ClaudeError::RateLimited { retry_after } => *retry_after,
                _ => None,
            };
            let delay = self.retry.delay_for(attempt, retry_after);
            // Attempt number and status only: never headers or bodies.
            tracing::warn!(
                attempt,
                max_attempts,
                status = ?status_of(&err),
                delay_ms = delay.as_millis() as u64,
                "retrying Claude API request"
            );
            tokio::time::sleep(delay).await;
            attempt += 1;
            earlier = Some(err);
        }
    }

    /// One streamed attempt.
    async fn stream_once(
        &self,
        req: &MessagesRequest,
        on_text: &(dyn Fn(&str) + Send + Sync),
        deadline: Option<Instant>,
    ) -> Result<MessagesResponse, ClaudeError> {
        let mut st = StreamState::default();
        if deadline.is_some_and(|d| Instant::now() >= d) {
            return Err(st.interrupted(BreakKind::NotSent));
        }
        let url = self
            .base_url
            .join("v1/messages")
            .expect("static path joins onto a base URL");
        let mut body = req.body();
        if let Some(o) = body.as_object_mut() {
            o.insert("stream".to_string(), serde_json::Value::Bool(true));
        }
        let body = serde_json::to_vec(&body).expect("request types serialize to JSON");
        let sent = self
            .stream_http
            .post(url)
            .header("x-api-key", self.api_key.expose_secret())
            .header("anthropic-version", ANTHROPIC_VERSION)
            .header("content-type", "application/json")
            .header("accept", "text/event-stream")
            .body(body)
            .send();
        let (wait, kind) = self.stream_wait(Instant::now(), deadline);
        let mut resp = match tokio::time::timeout(wait, sent).await {
            Ok(r) => r?,
            Err(_) => return Err(st.interrupted(kind)),
        };
        let status = resp.status().as_u16();
        if !(200..300).contains(&status) {
            let retry_after = retry_after_of(&resp);
            let (wait, kind) = self.stream_wait(Instant::now(), deadline);
            return match tokio::time::timeout(wait, resp.bytes()).await {
                Ok(bytes) => Err(error_of(status, retry_after, &bytes?)),
                Err(_) => Err(st.interrupted(kind)),
            };
        }
        let mut sse = stream::SseBuffer::default();
        let mut last_event = Instant::now();
        loop {
            let (wait, kind) = self.stream_wait(last_event, deadline);
            let chunk = match tokio::time::timeout(wait, resp.chunk()).await {
                Err(_) => return Err(st.interrupted(kind)),
                Ok(Err(_)) => return Err(st.interrupted(BreakKind::Transport)),
                Ok(Ok(None)) => return Err(st.interrupted(BreakKind::EndedEarly)),
                Ok(Ok(Some(chunk))) => chunk,
            };
            sse.push(&chunk);
            while let Some(data) = sse.next_data() {
                last_event = Instant::now();
                st.apply(&data, on_text)?;
                if st.stopped() {
                    return Ok(st.into_response());
                }
            }
        }
    }

    /// How long the next wait may last, counted from `since`, and which limit
    /// ends it.
    fn stream_wait(&self, since: Instant, deadline: Option<Instant>) -> (Duration, BreakKind) {
        let now = Instant::now();
        let idle = self
            .stream_idle
            .saturating_sub(now.saturating_duration_since(since));
        match deadline.map(|d| d.saturating_duration_since(now)) {
            Some(left) if left <= idle => (left, BreakKind::Deadline),
            _ => (idle, BreakKind::Idle),
        }
    }

    /// One HTTP attempt, classified into a typed result.
    async fn send_once(&self, req: &MessagesRequest) -> Result<MessagesResponse, ClaudeError> {
        let url = self
            .base_url
            .join("v1/messages")
            .expect("static path joins onto a base URL");
        let body = serde_json::to_vec(&req.body()).expect("request types serialize to JSON");
        let resp = self
            .http
            .post(url)
            .header("x-api-key", self.api_key.expose_secret())
            .header("anthropic-version", ANTHROPIC_VERSION)
            .header("content-type", "application/json")
            .body(body)
            .send()
            .await?;
        let status = resp.status().as_u16();
        let retry_after = retry_after_of(&resp);
        let bytes = resp.bytes().await?;
        if (200..300).contains(&status) {
            return serde_json::from_slice(&bytes).map_err(|_| decode_error(status, &bytes));
        }
        Err(error_of(status, retry_after, &bytes))
    }
}

fn retry_after_of(resp: &reqwest::Response) -> Option<Duration> {
    resp.headers()
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse::<u64>().ok())
        .map(Duration::from_secs)
}

fn decode_error(status: u16, bytes: &[u8]) -> ClaudeError {
    ClaudeError::Decode {
        status,
        body_snippet: String::from_utf8_lossy(bytes)
            .chars()
            .take(SNIPPET_CHARS)
            .collect(),
    }
}

/// A non-success response, classified.
fn error_of(status: u16, retry_after: Option<Duration>, bytes: &[u8]) -> ClaudeError {
    match status {
        429 => ClaudeError::RateLimited { retry_after },
        529 => ClaudeError::Overloaded,
        _ => match serde_json::from_slice::<ApiErrorEnvelope>(bytes) {
            Ok(env) => ClaudeError::Api {
                status,
                error_type: env.error.error_type,
                message: env.error.message,
            },
            Err(_) => decode_error(status, bytes),
        },
    }
}
