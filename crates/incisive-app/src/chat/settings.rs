//! Chat settings (`CHAT`, `CHAT_MODEL`, `CHAT_DAILY_LIMIT`, `CHAT_PRIOR`,
//! `CHAT_MIN_SCORE`). The Gemma URL and auth are the memory layer's.

/// Default chat model on the Gemma server.
pub const DEFAULT_CHAT_MODEL: &str = "gemma3:4b";
/// Default questions per user per day.
pub const DEFAULT_DAILY_LIMIT: i64 = 30;
/// Default retrieval score floor; a placeholder until calibration.
pub const DEFAULT_MIN_SCORE: f32 = 0.45;

#[derive(Clone, Debug)]
pub struct ChatSettings {
    /// `CHAT=on` and `MEMORY=on`; otherwise off.
    pub on: bool,
    pub model: String,
    pub daily_limit: i64,
    /// Gemma's prior and its reconciliation; only when chat is on.
    pub prior: bool,
    pub min_score: f32,
}

impl Default for ChatSettings {
    fn default() -> Self {
        ChatSettings {
            on: false,
            model: DEFAULT_CHAT_MODEL.to_string(),
            daily_limit: DEFAULT_DAILY_LIMIT,
            prior: false,
            min_score: DEFAULT_MIN_SCORE,
        }
    }
}
