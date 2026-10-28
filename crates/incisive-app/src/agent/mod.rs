//! The evidence agent's pipeline entry point. Server only.
//!
//! [`run_pipeline`] executes one run's steps (§2: Frame, Search, Screen,
//! [Retrieve, Focused only], Extract, Write, Check, Present) and returns what
//! the worker should store. The worker (`runs::worker`) owns the run's status,
//! the answer row, the reservation release and the `run_finished` audit; the
//! pipeline only adds progress lines and charges its Claude calls through
//! `ctx.budget`.
//!
//! This is the placeholder (Phase 2, Task 3): it writes one line per step and
//! returns a fixed body. Task 13 replaces the body of [`run_pipeline`]; the
//! [`RunContext`] shape is fixed for it.

pub mod frame;
pub mod llm;
pub mod search;

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{Value, json};
use uuid::Uuid;

use crate::identity::User;
use crate::lit::europepmc::EuropePmc;
use crate::lit::pubmed::PubMed;
use crate::runs::budget::RunBudget;
use crate::runs::worker::{self, CancelFlag};
use crate::runs::{DepthSettings, RunMode, StepKind, store};
use crate::server::AppState;
use llm::{AgentModels, LlmCall, LlmClient};

/// Everything one run's pipeline needs. Built by the worker.
pub struct RunContext {
    /// Pool, prices, run limits and config. Later tasks reach the agent's
    /// shared Claude client (an `Arc<dyn …>`) through it.
    pub state: AppState,
    pub run_id: Uuid,
    pub user: User,
    /// The question as stored (post identifier filter). Never logged or audited.
    pub question: String,
    pub mode: RunMode,
    pub settings: DepthSettings,
    pub budget: Arc<RunBudget>,
    /// Set when the user deletes the run; checked between steps.
    pub cancel: CancelFlag,
}

/// The agent's Claude client and its model per step.
///
/// Task 13 threads this through [`RunContext`] (an `Arc<AgentLlm>` field built
/// once at startup); until then the step functions take it as a parameter.
#[derive(Clone)]
pub struct AgentLlm {
    pub client: Arc<dyn LlmClient>,
    pub models: AgentModels,
}

impl AgentLlm {
    /// A budgeted call handle for one step of `ctx`'s run.
    pub fn call(&self, ctx: &RunContext, model: &str, step: &'static str) -> LlmCall {
        LlmCall::new(
            self.client.clone(),
            ctx.budget.clone(),
            ctx.state.pool.clone(),
            ctx.cancel.clone(),
            model,
            step,
        )
    }
}

/// The literature clients the steps use. The paper cache is reached through
/// `ctx.state.pool`.
///
/// Task 13/14 build one at startup and thread it through [`RunContext`] as an
/// `Arc<Literature>` field; until then the step functions take it as a
/// parameter. Retrieve (Task 10) adds the Unpaywall client here.
pub struct Literature {
    pub pubmed: PubMed,
    pub epmc: EuropePmc,
}

/// Longest step line stored.
pub const MAX_STEP_LINE: usize = 160;

/// `s` with control characters turned into spaces, runs of whitespace
/// collapsed, trimmed, and cut to at most `max` characters (an ellipsis marks
/// a cut). Character-safe.
pub fn clip(s: &str, max: usize) -> String {
    let clean = s
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect::<String>();
    let clean = clean.split_whitespace().collect::<Vec<_>>().join(" ");
    if clean.chars().count() <= max {
        return clean;
    }
    let mut out: String = clean.chars().take(max.saturating_sub(1)).collect();
    out = out.trim_end().to_string();
    out.push('\u{2026}');
    out
}

/// Consecutive question words that count as an echo.
pub const ECHO_WORDS: usize = 8;

/// Lower-cased words with everything but letters and digits removed.
fn echo_words(s: &str) -> Vec<String> {
    s.split_whitespace()
        .map(|w| {
            w.chars()
                .filter(|c| c.is_alphanumeric())
                .flat_map(char::to_lowercase)
                .collect::<String>()
        })
        .filter(|w| !w.is_empty())
        .collect()
}

/// True when `text` contains a run of [`ECHO_WORDS`] (8) or more consecutive
/// words of `question`, ignoring case, whitespace and punctuation. Used to
/// keep the question out of step lines and search queries. A question of
/// fewer than eight words never matches.
pub fn contains_question(text: &str, question: &str) -> bool {
    let q = echo_words(question);
    if q.len() < ECHO_WORDS {
        return false;
    }
    let t = format!(" {} ", echo_words(text).join(" "));
    q.windows(ECHO_WORDS)
        .any(|w| t.contains(&format!(" {} ", w.join(" "))))
}

/// Append a progress line (cut to [`MAX_STEP_LINE`]). A failed write is
/// logged by kind only and otherwise ignored: the worker's heartbeat notices a
/// deleted or lost run between steps.
pub(crate) async fn add_line(ctx: &RunContext, kind: StepKind, line: &str) {
    let line = clip(line, MAX_STEP_LINE);
    if store::add_step(&ctx.state.pool, ctx.run_id, kind, &line)
        .await
        .is_err()
    {
        tracing::warn!(run_id = %ctx.run_id, step = kind.as_str(), "step line not stored");
    }
}

/// What the pipeline produced.
#[derive(Debug, Clone, PartialEq)]
pub enum PipelineOutcome {
    /// A complete answer body.
    Done(Value),
    /// An answer, but a step degraded.
    Partial(Value),
    /// No answer; a fixed error kind (stored as the run's `error_kind`).
    Failed(&'static str),
}

/// A boxed pipeline future borrowing its context.
pub type PipelineFuture<'a> = Pin<Box<dyn Future<Output = PipelineOutcome> + Send + 'a>>;

/// The pipeline steps for `mode`, in order, with their progress lines.
fn steps(mode: RunMode) -> Vec<(StepKind, &'static str)> {
    let mut v = vec![
        (StepKind::Frame, "Framing the question"),
        (StepKind::Search, "Searching PubMed and Europe PMC"),
        (StepKind::Screen, "Screening titles and abstracts"),
    ];
    if mode == RunMode::Focused {
        v.push((StepKind::Retrieve, "Fetching free full texts"));
    }
    v.extend([
        (StepKind::Extract, "Extracting findings"),
        (StepKind::Write, "Writing the answer"),
        (StepKind::Check, "Checking every claim against its sources"),
        (StepKind::Present, "Preparing the answer"),
    ]);
    v
}

/// Run the pipeline for `ctx`. Placeholder: one progress line per step, no
/// Claude calls (so nothing is charged), and a fixed answer body.
pub async fn run_pipeline(ctx: &RunContext) -> PipelineOutcome {
    for (kind, line) in steps(ctx.mode) {
        if ctx.cancel.is_cancelled() {
            return PipelineOutcome::Failed("cancelled");
        }
        // A failed write usually means the run was deleted; the worker checks.
        if store::add_step(&ctx.state.pool, ctx.run_id, kind, line)
            .await
            .is_err()
        {
            return PipelineOutcome::Failed("store");
        }
        // Still ours? (Interrupted or deleted elsewhere: stop.)
        if !worker::heartbeat(ctx).await {
            return PipelineOutcome::Failed("interrupted");
        }
        tokio::time::sleep(Duration::ZERO).await;
    }
    PipelineOutcome::Done(json!({ "placeholder": true }))
}
