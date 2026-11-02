//! Memory layer settings (`MEMORY`, `GEMMA_URL`, `GEMMA_AUTH`, `EMBED_MODEL`,
//! `MEMORY_TOPIC_MATCH`, `MEMORY_GATE_MATCH`).

use secrecy::SecretString;

/// Default embedding model name.
pub const DEFAULT_EMBED_MODEL: &str = "embeddinggemma";
/// Default topic-match similarity threshold.
pub const DEFAULT_TOPIC_MATCH: f32 = 0.80;
/// Default gate-match similarity threshold.
pub const DEFAULT_GATE_MATCH: f32 = 0.85;

/// `Debug` is derived: `SecretString` redacts itself, so `gemma_auth` never
/// shows its value.
#[derive(Clone, Debug)]
pub struct MemorySettings {
    pub on: bool,
    /// `GEMMA_URL`, trailing `/` trimmed.
    pub gemma_url: Option<String>,
    /// `GEMMA_AUTH`.
    pub gemma_auth: Option<SecretString>,
    pub embed_model: String,
    pub topic_match: f32,
    pub gate_match: f32,
}

impl Default for MemorySettings {
    fn default() -> Self {
        MemorySettings {
            on: false,
            gemma_url: None,
            gemma_auth: None,
            embed_model: DEFAULT_EMBED_MODEL.to_string(),
            topic_match: DEFAULT_TOPIC_MATCH,
            gate_match: DEFAULT_GATE_MATCH,
        }
    }
}

impl MemorySettings {
    /// Embeddings need both the URL and the auth.
    pub fn embeddings_enabled(&self) -> bool {
        self.gemma_url.is_some() && self.gemma_auth.is_some()
    }
}
