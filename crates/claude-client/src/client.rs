use std::time::Duration;

use reqwest::Url;
use secrecy::{ExposeSecret, SecretString};

use crate::{ClaudeError, MessagesRequest, MessagesResponse};

const DEFAULT_BASE_URL: &str = "https://api.anthropic.com";
const ANTHROPIC_VERSION: &str = "2023-06-01";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(120);
const SNIPPET_CHARS: usize = 200;

// Deliberately no `Debug` derive: it would print the API key.
pub struct Client {
    http: reqwest::Client,
    api_key: SecretString,
    base_url: Url,
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
        }
    }

    pub async fn create_message(
        &self,
        req: &MessagesRequest,
    ) -> Result<MessagesResponse, ClaudeError> {
        self.send_once(req).await
    }

    /// One HTTP attempt; a retry loop can wrap this.
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
        let bytes = resp.bytes().await?;
        serde_json::from_slice(&bytes).map_err(|_| ClaudeError::Decode {
            status,
            body_snippet: String::from_utf8_lossy(&bytes)
                .chars()
                .take(SNIPPET_CHARS)
                .collect(),
        })
    }
}
