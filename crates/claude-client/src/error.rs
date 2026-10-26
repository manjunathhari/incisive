use thiserror::Error;

#[derive(Debug, Error)]
pub enum ClaudeError {
    #[error("transport error: {0}")]
    Transport(#[from] reqwest::Error),
    #[error("could not decode response (status {status}): {body_snippet}")]
    Decode { status: u16, body_snippet: String },
}
