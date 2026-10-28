//! Test helpers (feature `test-util` only): the old placeholder pipeline, an
//! offline literature client and a test `AgentLlm`.

use std::sync::Arc;

use serde_json::json;

use super::llm::{AgentModels, LlmClient, ScriptedLlm};
use super::{AgentLlm, Literature, PipelineFuture, PipelineOutcome, RunContext};
use crate::lit::http::{AllowlistedClient, OaPolicy};
use crate::runs::worker;
use crate::runs::{RunMode, StepKind, store};

/// The Phase 2 Task 3 placeholder: one progress line per step, no Claude
/// calls (nothing is charged) and the body `{"placeholder": true}`. For tests
/// of the worker that need a deterministic pipeline.
pub fn placeholder_pipeline(ctx: &RunContext) -> PipelineFuture<'_> {
    Box::pin(async move {
        let mut steps = vec![
            (StepKind::Frame, "Framing the question"),
            (StepKind::Search, "Searching PubMed and Europe PMC"),
            (StepKind::Screen, "Screening titles and abstracts"),
        ];
        if ctx.mode == RunMode::Focused {
            steps.push((StepKind::Retrieve, "Fetching free full texts"));
        }
        steps.extend([
            (StepKind::Extract, "Extracting findings"),
            (StepKind::Write, "Writing the answer"),
            (StepKind::Check, "Checking every claim against its sources"),
            (StepKind::Present, "Preparing the answer"),
        ]);
        for (kind, line) in steps {
            if ctx.cancel.is_cancelled() {
                return PipelineOutcome::Failed("cancelled");
            }
            if store::add_step(&ctx.state.pool, ctx.run_id, kind, line)
                .await
                .is_err()
            {
                return PipelineOutcome::Failed("store");
            }
            if !worker::heartbeat(ctx).await {
                return PipelineOutcome::Failed("interrupted");
            }
            tokio::task::yield_now().await;
        }
        PipelineOutcome::Done(json!({ "placeholder": true }))
    })
}

/// Literature clients that point at a closed local port: any call fails at
/// once, nothing leaves the machine.
pub fn offline_literature() -> Arc<Literature> {
    let oa = OaPolicy::default();
    let client = AllowlistedClient::new("Incisive/test", oa.clone())
        .with_base_override("http://127.0.0.1:9")
        .with_retry_backoff(std::time::Duration::from_millis(1));
    Arc::new(Literature::new(client, oa, None, None))
}

/// An `AgentLlm` using `client` for every step, all on `model`.
pub fn agent_llm(client: Arc<dyn LlmClient>, model: &str) -> Arc<AgentLlm> {
    let m = model.to_string();
    Arc::new(AgentLlm {
        client,
        models: AgentModels {
            frame: m.clone(),
            search: m.clone(),
            screen: m.clone(),
            extract: m.clone(),
            write: m.clone(),
            check: m,
        },
    })
}

/// An `AgentLlm` with no scripted replies (every call fails, unbilled).
pub fn idle_llm(model: &str) -> Arc<AgentLlm> {
    agent_llm(Arc::new(ScriptedLlm::new()), model)
}
