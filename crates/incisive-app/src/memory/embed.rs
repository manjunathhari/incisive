//! Gemma embeddings from the owner's Ollama server (`POST {url}/api/embed`,
//! HTTP basic auth). Errors carry fixed codes only; the password and the
//! text are never logged or put in a URL.

use std::time::Duration;

use secrecy::{ExposeSecret, SecretString};
use serde_json::Value;

use crate::memory::settings::MemorySettings;

/// `embeddinggemma` returns 768 floats.
pub const EMBED_DIM: usize = 768;

#[derive(Debug, Clone, PartialEq)]
pub enum EmbedError {
    Timeout,
    Unauthorized,
    Http(u16),
    BadBody,
    WrongDim(usize),
}

impl EmbedError {
    /// The server itself is unavailable or refusing us, so more calls in the
    /// same sweep would fail too. Anything else (a bad body, a wrong size, a
    /// 4xx for this text) is about one record only.
    pub fn is_server_unavailable(&self) -> bool {
        match self {
            EmbedError::Timeout | EmbedError::Unauthorized => true,
            EmbedError::Http(c) => *c == 0 || *c >= 500,
            EmbedError::BadBody | EmbedError::WrongDim(_) => false,
        }
    }

    pub fn code(&self) -> &'static str {
        match self {
            EmbedError::Timeout => "timeout",
            EmbedError::Unauthorized => "unauthorized",
            EmbedError::Http(_) => "http",
            EmbedError::BadBody => "bad_body",
            EmbedError::WrongDim(_) => "wrong_dim",
        }
    }
}

/// `GEMMA_AUTH` is `user:password`, split on the first `:`. `None` (with a
/// fixed-code warning, never the value) when there is no colon. Shared with
/// the chat client.
pub(crate) fn parse_auth(raw: &SecretString) -> Option<(String, SecretString)> {
    let Some((user, password)) = raw.expose_secret().split_once(':') else {
        tracing::warn!(code = "bad_auth", "GEMMA_AUTH is not user:password");
        return None;
    };
    Some((user.to_string(), SecretString::from(password.to_string())))
}

pub struct Embedder {
    client: reqwest::Client,
    url: String,
    user: String,
    password: SecretString,
    model: String,
}

impl std::fmt::Debug for Embedder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Embedder")
            .field("url", &self.url)
            .field("model", &self.model)
            .finish_non_exhaustive()
    }
}

impl Embedder {
    /// `None` unless both `GEMMA_URL` and `GEMMA_AUTH` are set.
    /// `GEMMA_AUTH` is `user:password`, split on the first `:`.
    pub fn from_settings(s: &MemorySettings) -> Option<Embedder> {
        if !s.embeddings_enabled() {
            return None;
        }
        let url = s.gemma_url.as_ref()?.trim_end_matches('/').to_string();
        let (user, password) = parse_auth(s.gemma_auth.as_ref()?)?;
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .ok()?;
        Some(Embedder {
            client,
            url,
            user,
            password,
            model: s.embed_model.clone(),
        })
    }

    pub fn model(&self) -> &str {
        &self.model
    }

    pub async fn embed(&self, text: &str, timeout: Duration) -> Result<Vec<f32>, EmbedError> {
        let resp = self
            .client
            .post(format!("{}/api/embed", self.url))
            .basic_auth(&self.user, Some(self.password.expose_secret()))
            .timeout(timeout)
            .json(&serde_json::json!({
                "model": self.model,
                "input": text,
                "keep_alive": "30m",
            }))
            .send()
            .await
            .map_err(|e| {
                if e.is_timeout() {
                    EmbedError::Timeout
                } else {
                    EmbedError::Http(0)
                }
            })?;
        let status = resp.status();
        if status.as_u16() == 401 {
            return Err(EmbedError::Unauthorized);
        }
        if !status.is_success() {
            return Err(EmbedError::Http(status.as_u16()));
        }
        let body: Value = resp.json().await.map_err(|e| {
            if e.is_timeout() {
                EmbedError::Timeout
            } else {
                EmbedError::BadBody
            }
        })?;
        let first = body
            .get("embeddings")
            .and_then(|e| e.get(0))
            .and_then(Value::as_array)
            .ok_or(EmbedError::BadBody)?;
        if first.len() != EMBED_DIM {
            return Err(EmbedError::WrongDim(first.len()));
        }
        first
            .iter()
            .map(|v| v.as_f64().map(|f| f as f32).ok_or(EmbedError::BadBody))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;
    use base64::engine::general_purpose::STANDARD;
    use wiremock::matchers::{body_partial_json, header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn embedder(server: &MockServer) -> Embedder {
        let s = MemorySettings {
            on: true,
            gemma_url: Some(format!("{}/", server.uri())),
            gemma_auth: Some(SecretString::from("incisive:s3cret-pw")),
            ..MemorySettings::default()
        };
        Embedder::from_settings(&s).expect("embedder")
    }

    fn vec_body(n: usize) -> Value {
        serde_json::json!({ "embeddings": [vec![0.5f32; n]] })
    }

    const T: Duration = Duration::from_secs(5);

    #[tokio::test]
    async fn embed_ok_768() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/embed"))
            .and(header(
                "authorization",
                format!("Basic {}", STANDARD.encode("incisive:s3cret-pw")).as_str(),
            ))
            .and(body_partial_json(serde_json::json!({
                "model": "embeddinggemma", "input": "hello", "keep_alive": "30m"
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(vec_body(768)))
            .expect(1)
            .mount(&server)
            .await;
        let v = embedder(&server).embed("hello", T).await.unwrap();
        assert_eq!(v.len(), 768);
    }

    #[tokio::test]
    async fn embed_401() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(401))
            .mount(&server)
            .await;
        let e = embedder(&server).embed("x", T).await.unwrap_err();
        assert_eq!(e, EmbedError::Unauthorized);
        assert_eq!(e.code(), "unauthorized");
    }

    #[tokio::test]
    async fn embed_timeout() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(vec_body(768))
                    .set_delay(Duration::from_millis(600)),
            )
            .mount(&server)
            .await;
        let e = embedder(&server)
            .embed("x", Duration::from_millis(100))
            .await
            .unwrap_err();
        assert_eq!(e, EmbedError::Timeout);
        assert_eq!(e.code(), "timeout");
    }

    #[tokio::test]
    async fn embed_html_404() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(404).set_body_string("<html>nope</html>"))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        let e = embedder(&server).embed("x", T).await.unwrap_err();
        assert_eq!(e, EmbedError::Http(404));
        assert_eq!(e.code(), "http");

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({"error": "model not found"})),
            )
            .mount(&server)
            .await;
        let e = embedder(&server).embed("x", T).await.unwrap_err();
        assert_eq!(e, EmbedError::BadBody);
        assert_eq!(e.code(), "bad_body");

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(vec_body(384)))
            .mount(&server)
            .await;
        let e = embedder(&server).embed("x", T).await.unwrap_err();
        assert_eq!(e, EmbedError::WrongDim(384));
        assert_eq!(e.code(), "wrong_dim");
    }

    #[tokio::test]
    async fn auth_not_in_debug() {
        let server = MockServer::start().await;
        let dbg = format!("{:?}", embedder(&server));
        assert!(!dbg.contains("s3cret-pw"));
        assert!(!dbg.contains("incisive:"));
        assert!(dbg.contains("embeddinggemma"));
    }

    #[test]
    fn parse_auth_splits_on_first_colon() {
        let (u, p) = parse_auth(&SecretString::from("incisive:pw:with:colons")).unwrap();
        assert_eq!(u, "incisive");
        assert_eq!(p.expose_secret(), "pw:with:colons");
        assert!(parse_auth(&SecretString::from("nocolon")).is_none());
    }

    #[test]
    fn none_without_settings() {
        assert!(Embedder::from_settings(&MemorySettings::default()).is_none());
    }
}
