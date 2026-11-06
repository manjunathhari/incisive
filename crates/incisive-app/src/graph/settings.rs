//! The graph's switch, model and start date (`GRAPH`, `GRAPH_MODEL`,
//! `GRAPH_FROM`), and what a running app holds for it.

use std::sync::Arc;
use std::time::Duration;

use chrono::NaiveDate;

use super::job::TIME_LIMIT;
use crate::agent::llm::LlmClient;

/// The default `GRAPH_MODEL`.
pub const DEFAULT_MODEL: &str = "claude-sonnet-5-5";

/// The switch, the model and the first day it applies from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphSettings {
    /// `GRAPH=on`. Default off: no job, no row, no call.
    pub on: bool,
    pub model: String,
    /// `GRAPH_FROM` (required when on); `None` while off.
    pub from: Option<NaiveDate>,
    /// How long a job waits for its call: [`TIME_LIMIT`], shorter only in
    /// tests.
    pub time_limit: Duration,
}

impl Default for GraphSettings {
    fn default() -> Self {
        GraphSettings {
            on: false,
            model: DEFAULT_MODEL.to_string(),
            from: None,
            time_limit: TIME_LIMIT,
        }
    }
}

/// What a running app holds for the graph.
#[derive(Clone, Default)]
pub struct GraphRuntime {
    pub settings: GraphSettings,
    /// The graph's own client ([`super::job::client`]). `None` (dev without a
    /// key): every job fails `no_client`; the agent's client is never used.
    pub client: Option<Arc<dyn LlmClient>>,
}
