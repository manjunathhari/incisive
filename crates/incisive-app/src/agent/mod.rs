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

pub mod llm;

use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use serde_json::{Value, json};
use uuid::Uuid;

use crate::identity::User;
use crate::runs::budget::RunBudget;
use crate::runs::worker::{self, CancelFlag};
use crate::runs::{DepthSettings, RunMode, StepKind, store};
use crate::server::AppState;

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
    pub budget: RunBudget,
    /// Set when the user deletes the run; checked between steps.
    pub cancel: CancelFlag,
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
