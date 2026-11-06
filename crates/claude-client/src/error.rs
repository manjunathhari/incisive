use std::time::Duration;

use thiserror::Error;

use crate::stream::StreamBreak;

#[derive(Debug, Error)]
pub enum ClaudeError {
    /// The API rejected the request with a structured error body.
    #[error("API error {status} ({error_type}): {message}")]
    Api {
        status: u16,
        error_type: String,
        message: String,
    },
    #[error("rate limited (retry after {retry_after:?})")]
    RateLimited { retry_after: Option<Duration> },
    #[error("API overloaded")]
    Overloaded,
    #[error("transport error: {0}")]
    Transport(#[from] reqwest::Error),
    /// The response body was not the JSON we expected. The snippet comes from
    /// the response only, never from request data.
    #[error("could not decode response (status {status}): {body_snippet}")]
    Decode { status: u16, body_snippet: String },
    /// A streamed reply broke off. Counts only, never reply text.
    #[error("stream interrupted ({0})")]
    StreamInterrupted(StreamBreak),
}
