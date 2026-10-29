use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize)]
pub struct MessagesRequest {
    pub model: String,
    pub max_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system: Option<String>,
    pub messages: Vec<Message>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<ToolDef>,
    /// How the model must use `tools`. Unset: omitted (the API default).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<ToolChoice>,
    /// Structured outputs: the reply's text is JSON matching the schema.
    /// Unset: omitted.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_config: Option<OutputConfig>,
}

/// The Messages API `output_config`.
///
/// A structured call sets `format`; a tool-use turn sets only `effort`
/// (Phase 3: tools and a format are never combined in one request).
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct OutputConfig {
    /// Structured outputs. Unset: omitted (free text or tool use).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format: Option<OutputFormat>,
    /// How much the model thinks (thinking counts toward `max_tokens`).
    /// Unset: omitted (the model's default).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effort: Option<Effort>,
}

impl OutputConfig {
    /// `{"format": {"type": "json_schema", "schema": …}}`, default effort.
    pub fn format(schema: serde_json::Value) -> OutputConfig {
        OutputConfig {
            format: Some(OutputFormat::JsonSchema { schema }),
            effort: None,
        }
    }
}

/// `output_config.effort`.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Effort {
    Low,
    Medium,
    High,
    Xhigh,
    Max,
}

/// `output_config.format`: `{"type":"json_schema","schema":{…}}`.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum OutputFormat {
    JsonSchema { schema: serde_json::Value },
}

/// The Messages API `tool_choice`. `Tool { name }` forces that one tool
/// (`{"type":"tool","name":"…"}`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToolChoice {
    Auto,
    Any,
    Tool { name: String },
    None,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Message {
    pub role: Role,
    pub content: Vec<ContentBlock>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    Text {
        text: String,
    },
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    ToolResult {
        tool_use_id: String,
        content: String,
        #[serde(default)]
        is_error: bool,
    },
    /// The model's thinking. Sent back unchanged (with its signature) in the
    /// assistant turn of a tool-use conversation; never logged or shown.
    Thinking {
        #[serde(default)]
        thinking: String,
        #[serde(default)]
        signature: String,
    },
    /// Encrypted thinking. Sent back unchanged, like [`ContentBlock::Thinking`].
    RedactedThinking {
        data: String,
    },
    /// Any other block type. Never sent back: serialising it would produce a
    /// block the API rejects, so callers drop it from echoed turns.
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolDef {
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MessagesResponse {
    pub id: String,
    pub content: Vec<ContentBlock>,
    pub stop_reason: Option<StopReason>,
    pub usage: Usage,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    EndTurn,
    MaxTokens,
    StopSequence,
    ToolUse,
    /// The model declined. Billed; the output may not match a schema.
    Refusal,
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct Usage {
    pub input_tokens: u32,
    pub output_tokens: u32,
}
