use std::time::Duration;

use reqwest::Url;
use secrecy::{ExposeSecret, SecretString};

use serde::Deserialize;

use crate::retry::{is_retryable, status_of};
use crate::{ClaudeError, MessagesRequest, MessagesResponse, RetryPolicy};

const DEFAULT_BASE_URL: &str = "https://api.anthropic.com";
const ANTHROPIC_VERSION: &str = "2023-06-01";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(120);
const SNIPPET_CHARS: usize = 200;

// No `Debug` derive: it would print the API key. See the manual impl below.
pub struct Client {
    http: reqwest::Client,
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
        Client {
            http,
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

    /// One HTTP attempt, classified into a typed result.
    async fn send_once(&self, req: &MessagesRequest) -> Result<MessagesResponse, ClaudeError> {
        let url = self
            .base_url
            .join("v1/messages")
            .expect("static path joins onto a base URL");
        let body = serde_json::to_vec(req).expect("request types serialize to JSON");
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
        let retry_after = resp
            .headers()
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.trim().parse::<u64>().ok())
            .map(Duration::from_secs);
        let bytes = resp.bytes().await?;
        let decode_error = || ClaudeError::Decode {
            status,
            body_snippet: String::from_utf8_lossy(&bytes)
                .chars()
                .take(SNIPPET_CHARS)
                .collect(),
        };
        if (200..300).contains(&status) {
            return serde_json::from_slice(&bytes).map_err(|_| decode_error());
        }
        match status {
            429 => Err(ClaudeError::RateLimited { retry_after }),
            529 => Err(ClaudeError::Overloaded),
            _ => match serde_json::from_slice::<ApiErrorEnvelope>(&bytes) {
                Ok(env) => Err(ClaudeError::Api {
                    status,
                    error_type: env.error.error_type,
                    message: env.error.message,
                }),
                Err(_) => Err(decode_error()),
            },
        }
    }
}
