//! Tools an agent loop ([`super::agent_loop`]) may call. Server only.
//!
//! A [`Tool`] has a name, a description and a strict-compatible JSON input
//! schema (every object `additionalProperties: false` with a full
//! `required` list, no `minimum`/`maximum`: ranges are checked in code), and
//! an executor. Executors run on our server and reach the network only
//! through the allowlisted literature clients ([`lit`]).
//!
//! **Errors.** A bad input or a failed lookup is the model's problem, not
//! the run's: the executor returns [`ToolError::Input`] or
//! [`ToolError::Failed`] and the loop sends it back as an `is_error` tool
//! result. Only [`ToolError::Fatal`] (cancellation, unrecorded money) ends
//! the loop. Error messages are fixed text written here (never the model's
//! input, never a URL), so they are safe to log by kind and to show the
//! model.
//!
//! **Output.** A tool returns plain JSON text. The loop caps it and wraps it
//! whole with `wrap_source` before the model sees it: every tool result,
//! paper text or not, is data.

pub mod lit;

use std::sync::Arc;

use async_trait::async_trait;
use claude_client::ToolDef;
use serde_json::Value;

use super::llm::StepError;

/// Why a tool produced no result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolError {
    /// The input was empty, oversized or malformed. Sent to the model.
    Input(String),
    /// The lookup failed (database down, not found, …). Sent to the model.
    Failed(String),
    /// The run must stop: `Cancelled`, `Unrecorded`, … Propagated.
    Fatal(StepError),
}

impl ToolError {
    /// Fixed kind for logs.
    pub fn kind(&self) -> &'static str {
        match self {
            ToolError::Input(_) => "input",
            ToolError::Failed(_) => "failed",
            ToolError::Fatal(e) => e.kind(),
        }
    }
}

/// One tool.
#[async_trait]
pub trait Tool: Send + Sync {
    /// The name the model calls it by (`[a-z_]+`).
    fn name(&self) -> &'static str;
    fn description(&self) -> &'static str;
    /// Strict-compatible JSON Schema of the input.
    fn input_schema(&self) -> Value;
    /// Run it. `Ok` is the result text (JSON) the model gets, before caps
    /// and wrapping.
    async fn run(&self, input: &Value) -> Result<String, ToolError>;
}

/// The tools one loop offers, by name.
#[derive(Clone, Default)]
pub struct ToolRegistry {
    tools: Vec<Arc<dyn Tool>>,
}

impl ToolRegistry {
    pub fn new() -> ToolRegistry {
        ToolRegistry::default()
    }

    /// Add `tool`. A second tool with the same name is a programming error.
    pub fn with(mut self, tool: Arc<dyn Tool>) -> ToolRegistry {
        assert!(
            self.get(tool.name()).is_none(),
            "duplicate tool {}",
            tool.name()
        );
        self.tools.push(tool);
        self
    }

    /// The tool called `name`, if registered.
    pub fn get(&self, name: &str) -> Option<&Arc<dyn Tool>> {
        self.tools.iter().find(|t| t.name() == name)
    }

    /// The registered name equal to `name`, for logs: a name the model
    /// made up is logged as `unknown`, never echoed.
    pub fn log_name(&self, name: &str) -> &'static str {
        self.get(name).map_or("unknown", |t| t.name())
    }

    /// The `tools` a request sends.
    pub fn defs(&self) -> Vec<ToolDef> {
        self.tools
            .iter()
            .map(|t| ToolDef {
                name: t.name().to_string(),
                description: t.description().to_string(),
                input_schema: t.input_schema(),
            })
            .collect()
    }

    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }
}
