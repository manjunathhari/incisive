//! The evidence agent's pipeline entry point. Server only.
//!
//! [`run_pipeline`] executes one run's steps (§2: Frame, Search, Screen,
//! [Retrieve, Focused only], Extract, Write, Check, Present) and returns what
//! the worker should store. The worker (`runs::worker`) owns the run's status,
//! the answer row, the reservation release and the `run_finished` audit; the
//! pipeline only adds progress lines and charges its Claude calls through
//! `ctx.budget`.
//!
//! The orchestration itself lives in [`pipeline`]; the stored body is an
//! [`answer::Answer`].

pub mod agent_loop;
pub mod answer;
pub mod answer_v3;
mod bounded;
pub mod check;
pub mod extract;
pub mod frame;
pub mod llm;
mod pipeline;
pub mod researcher;
pub mod retrieve;
pub mod screen;
pub mod search;
pub mod team;
#[cfg(feature = "test-util")]
pub mod testing;
pub mod tools;
pub mod write;
pub mod writer_v3;

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use secrecy::SecretString;
use serde_json::Value;
use uuid::Uuid;

use crate::identity::User;
use crate::lit::europepmc::EuropePmc;
use crate::lit::http::{AllowlistedClient, OaPolicy};
use crate::lit::pubmed::PubMed;
use crate::runs::budget::RunBudget;
use crate::runs::worker::CancelFlag;
use crate::runs::{DepthSettings, RunMode, StepKind, store};
use crate::server::AppState;
use llm::{AgentModels, LlmCall, LlmClient};

pub use pipeline::run_pipeline;

/// The pipeline the worker runs for `ctx`: the Phase 3 evidence team for
/// Quick runs when `QUICK_ENGINE` is `team` (the default), the Phase 2
/// pipeline otherwise (Focused, or `classic`).
pub fn run_engine(ctx: &RunContext) -> PipelineFuture<'_> {
    if ctx.state.quick_engine.uses_team(ctx.mode) {
        Box::pin(team::run_team(ctx))
    } else {
        Box::pin(run_pipeline(ctx))
    }
}

/// Everything one run's pipeline needs. Built by the worker.
pub struct RunContext {
    /// Pool, prices, run limits and config.
    pub state: AppState,
    /// The agent's Claude client and its model per step (built at startup).
    pub llm: Arc<AgentLlm>,
    /// The literature clients (built at startup).
    pub lit: Arc<Literature>,
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

/// The agent's Claude client and its model per step. Built once at startup
/// and reached through [`RunContext::llm`]; the step functions take it as a
/// parameter (the pipeline passes `&ctx.llm`), so tests can drive one step
/// with its own client.
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
/// Built once at startup ([`Literature::new`]) and reached through
/// [`RunContext::lit`]; the step functions take it as a parameter. Retrieve
/// (Task 10) needs only Europe PMC's `full_text` (Ruling R-10a), so there is
/// no Unpaywall client here in Phase 2.
pub struct Literature {
    pub pubmed: PubMed,
    pub epmc: EuropePmc,
}

impl Literature {
    /// PubMed (with the NCBI key when there is one, and `email` as NCBI's
    /// contact address) and Europe PMC over one allowlisted client and one
    /// shared open-access policy.
    pub fn new(
        client: AllowlistedClient,
        oa: OaPolicy,
        ncbi_key: Option<SecretString>,
        email: Option<String>,
    ) -> Literature {
        Literature {
            pubmed: PubMed::new(client.clone(), ncbi_key).with_email(email),
            epmc: EuropePmc::new(client, oa),
        }
    }
}

/// Paise to keep free for the finishing steps given what has been extracted:
/// Write's hold plus [`write::MAX_CLAIMS`] Check holds, each from the real
/// prompt builders and the same estimator the holds use (Ruling R-13c).
/// Gathering calls may not hold into it ([`RunBudget::try_hold_gathering`]).
///
/// It is computed for the findings extracted so far, so reading one more
/// paper can raise it by that paper's findings (a few hundred input tokens
/// across the calls). Holds reserve each call's full `max_tokens`, so the
/// real charges usually leave that much slack; if they do not, Write is
/// refused and the run ends `partial` (see `pipeline`). A JSON retry
/// (one per call at most) is not reserved for. An unpriced model counts 0:
/// its call fails anyway.
pub fn finish_reserve(
    llm: &AgentLlm,
    budget: &RunBudget,
    pico: &frame::Pico,
    extracted: &[extract::Extracted],
) -> i64 {
    let write = write::hold_for(llm, budget, pico, extracted).unwrap_or(0);
    let check = check::hold_per_claim_for(llm, budget, extracted).unwrap_or(0);
    let claims = i64::try_from(write::MAX_CLAIMS).unwrap_or(i64::MAX);
    write.saturating_add(check.saturating_mul(claims))
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
    /// An answer, degraded for this reason, stored as the run's
    /// `error_kind` (Phase 3: `writer_truncated`, `writer_time`, …).
    PartialKind(Value, &'static str),
    /// No answer; a fixed error kind (stored as the run's `error_kind`).
    Failed(&'static str),
}

/// A boxed pipeline future borrowing its context.
pub type PipelineFuture<'a> = Pin<Box<dyn Future<Output = PipelineOutcome> + Send + 'a>>;
