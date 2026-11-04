//! The introduction agent's storage and job. Server only.
//!
//! **Additive and detached.** Once a run has passed every check and holds its
//! own reservation, `ask::start_run` calls [`start`], which only spawns a
//! task and returns: nothing on the request path waits for the model. With
//! `INTRO=on` the task inserts a `pending` row in `incisive.run_intros` and
//! runs [`run_job`]. The job never touches the run's row, steps, answer, cost
//! or reservation, and nothing in the engine reads its text. Every failure
//! ends the row `failed`, `skipped` or `cap_blocked`, which the page shows as
//! nothing at all.
//!
//! **The call.** One attempt, about 30 s, [`MAX_TOKENS`] of output, through
//! the budgeted path (`LlmCall`). The question goes in with `wrap_source`, so
//! the `SOURCE_RULE` every `LlmCall` adds marks it as not to be obeyed.
//! Thinking counts toward `max_tokens`, and at the model's default effort it
//! could use all of this small allowance before any text (`extractor.rs`,
//! Ruling R-FA2). The request type has no switch for thinking, so the call
//! sets [`EFFORT`] (low), through `start_turn`, the one path that sends an
//! effort without a schema. A thinking block in the reply is never read as
//! text; a reply cut off at `max_tokens` or empty counts as failed.
//!
//! **Money.** The job reserves the call's hold against the **user's** caps
//! under its own request id (purpose `intro`), after the run's reservation is
//! in place, so it can never block the run: at a cap the row ends
//! `cap_blocked` and the run goes on. The charge goes under that same id; as
//! for the temporal job the charge row's purpose reads `run:<request_id>`.
//!
//! **Recovery** ([`recover`], from the run recovery): a `pending` row older
//! than [`STALE_AFTER_MIN`] minutes becomes `failed` and its reservation is
//! released; an `intro` reservation whose row is gone or finished is released
//! after the same time.
//!
//! Logs and audit entries carry ids, fixed reason kinds and paise only.

use std::panic::AssertUnwindSafe;
use std::sync::Arc;
use std::time::{Duration, Instant};

use claude_client::{ContentBlock, Message, Role, StopReason};
use futures_util::FutureExt;
use sqlx::{PgPool, Row};
use uuid::Uuid;

use super::budget::RunBudget;
use super::worker::CancelFlag;
use super::{IntroView, store};
use crate::agent::AgentLlm;
use crate::agent::llm::{Effort, LlmCall, LlmClient, StepError, wrap_source};
use crate::audit::{self, Event};
use crate::identity;
use crate::intro_page::PROMPT;
use crate::server::AppState;
use crate::spend::{self, CapCheck};

/// The spend purpose of the job's reservation.
pub const PURPOSE: &str = "intro";
/// The step name on its Claude call (logs only; not a finishing step).
pub const STEP: &str = "intro";
/// `max_tokens` of the call.
pub const MAX_TOKENS: u32 = 800;
/// The call's effort (see the module docs).
pub const EFFORT: Effort = Effort::Low;
/// The job gives the call up after this long, and its own client's one
/// attempt times out at the same moment (its charge is still booked).
pub const TIME_LIMIT: Duration = Duration::from_secs(30);
/// A pending row older than this is failed by recovery (the job takes at most
/// its 30 s limit plus its charge).
pub const STALE_AFTER_MIN: i32 = 5;
/// The default `INTRO_MODEL`.
pub const DEFAULT_MODEL: &str = "claude-sonnet-5-5";

/// The switch and the model (`INTRO`, `INTRO_MODEL`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IntroSettings {
    /// `INTRO=on`. Default off: no call, no row, no card.
    pub on: bool,
    pub model: String,
    /// [`TIME_LIMIT`], shorter only in tests.
    pub time_limit: Duration,
}

impl Default for IntroSettings {
    fn default() -> Self {
        IntroSettings {
            on: false,
            model: DEFAULT_MODEL.to_string(),
            time_limit: TIME_LIMIT,
        }
    }
}

/// What a running app holds for the introduction.
#[derive(Clone, Default)]
pub struct IntroRuntime {
    pub settings: IntroSettings,
    /// The job's own client ([`client`]). `None` (tests, dev without a key):
    /// the agent's client is used.
    pub client: Option<Arc<dyn LlmClient>>,
}

/// The job's own Claude client: one attempt (no retry) with a [`TIME_LIMIT`]
/// per-attempt timeout. Built apart from the agent's shared client.
pub fn client(key: secrecy::SecretString) -> claude_client::Client {
    claude_client::Client::new(key)
        .with_request_timeout(TIME_LIMIT)
        .with_retry_policy(claude_client::RetryPolicy {
            max_attempts: 1,
            ..claude_client::RetryPolicy::default()
        })
}

/// How a job ended (the row's `status`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobStatus {
    Done,
    Failed,
    Skipped,
    CapBlocked,
}

impl JobStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            JobStatus::Done => "done",
            JobStatus::Failed => "failed",
            JobStatus::Skipped => "skipped",
            JobStatus::CapBlocked => "cap_blocked",
        }
    }

    fn event(self) -> Event {
        match self {
            JobStatus::Done => Event::IntroDone,
            JobStatus::Failed => Event::IntroFailed,
            JobStatus::Skipped => Event::IntroSkipped,
            JobStatus::CapBlocked => Event::IntroCapBlocked,
        }
    }
}

/// Fixed reason kinds (stored in `reason`; never model text).
pub mod reason {
    pub const DAILY_CAP: &str = "limit reached";
    pub const UNPRICED: &str = "unpriced model";
    pub const COST: &str = "cost limit";
    pub const TIME: &str = "time";
    pub const API: &str = "api error";
    pub const UNUSABLE: &str = "unusable reply";
    pub const EMPTY: &str = "empty";
    pub const TRUNCATED: &str = "truncated";
    pub const UNRECORDED: &str = "unrecorded charge";
    pub const STORE: &str = "store";
    pub const PANIC: &str = "panic";
    pub const STALE: &str = "stale";
    pub const GONE: &str = "run deleted";
}

/// What the call sends as its one user message: the question as data.
pub fn user_message(question: &str) -> String {
    wrap_source("question", question)
}

// ---------- store ----------

/// Insert the run's `pending` row with the job's request id. False when a row
/// already exists or the run is gone.
pub async fn insert_pending(pool: &PgPool, run_id: Uuid, request_id: Uuid) -> sqlx::Result<bool> {
    let r = sqlx::query(
        "INSERT INTO incisive.run_intros (run_id, status, request_id) \
         SELECT id, 'pending', $2 FROM incisive.runs WHERE id = $1 \
         ON CONFLICT (run_id) DO NOTHING",
    )
    .bind(run_id)
    .bind(request_id)
    .execute(pool)
    .await?;
    Ok(r.rows_affected() > 0)
}

/// What a finished job stores.
#[derive(Debug, Clone, Default)]
pub struct Finished {
    pub reason: Option<&'static str>,
    pub text: Option<String>,
    pub cost_paise: i64,
}

/// Finish the run's row, only while it is `pending`. False when it is gone
/// (the run was deleted) or already finished (recovery failed it).
pub async fn finish(
    pool: &PgPool,
    run_id: Uuid,
    status: JobStatus,
    f: &Finished,
) -> sqlx::Result<bool> {
    let r = sqlx::query(
        "UPDATE incisive.run_intros SET status = $2, reason = $3, text = $4, cost_paise = $5, \
           finished_at = now() \
         WHERE run_id = $1 AND status = 'pending'",
    )
    .bind(run_id)
    .bind(status.as_str())
    .bind(f.reason)
    .bind(f.text.as_deref())
    .bind(f.cost_paise.max(0))
    .execute(pool)
    .await?;
    Ok(r.rows_affected() > 0)
}

/// Raise the row's cost to `paise` (a late charge has settled). Never lowers it.
async fn set_cost(pool: &PgPool, run_id: Uuid, paise: i64) -> sqlx::Result<()> {
    sqlx::query(
        "UPDATE incisive.run_intros SET cost_paise = GREATEST(cost_paise, $2) WHERE run_id = $1",
    )
    .bind(run_id)
    .bind(paise.max(0))
    .execute(pool)
    .await?;
    Ok(())
}

/// The run's introduction, only if the run belongs to `user_id`: `Ok(None)`
/// when the run is not theirs or does not exist; `Ok(Some(None))` when it is
/// theirs and has nothing to show (no row, failed, skipped or cap blocked).
pub async fn get_for_user(
    pool: &PgPool,
    user_id: i64,
    run_id: Uuid,
) -> sqlx::Result<Option<Option<IntroView>>> {
    let row = sqlx::query(
        "SELECT i.status, i.text \
         FROM incisive.runs r \
         LEFT JOIN incisive.run_intros i ON i.run_id = r.id \
         WHERE r.id = $1 AND r.user_id = $2",
    )
    .bind(run_id)
    .bind(user_id)
    .fetch_optional(pool)
    .await?;
    let Some(row) = row else { return Ok(None) };
    let status: Option<String> = row.try_get("status")?;
    let text: Option<String> = row.try_get("text")?;
    Ok(Some(match (status.as_deref(), text) {
        (Some("pending"), _) => Some(IntroView::Pending),
        (Some("done"), Some(text)) => Some(IntroView::Done { text }),
        _ => None,
    }))
}

/// [`recover_stale`] for the run recovery: never fails it; an error is logged
/// and retried at the next sweep.
pub async fn recover(pool: &PgPool) {
    if recover_stale(pool).await.is_err() {
        tracing::warn!("intro recovery failed; will retry");
    }
}

/// Recovery: fail every `pending` row older than [`STALE_AFTER_MIN`] minutes
/// ("stale"), release each one's reservation and audit it; then release
/// `intro` reservations older than that whose row is gone or no longer
/// pending. Returns the rows failed.
pub async fn recover_stale(pool: &PgPool) -> sqlx::Result<usize> {
    let stale: Vec<(Uuid, i64, Option<Uuid>, i64)> = sqlx::query_as(
        "UPDATE incisive.run_intros i SET status = 'failed', reason = $2, finished_at = now() \
         FROM incisive.runs r \
         WHERE r.id = i.run_id AND i.status = 'pending' \
           AND i.created_at < now() - make_interval(mins => $1) \
         RETURNING i.run_id, r.user_id, i.request_id, i.cost_paise",
    )
    .bind(STALE_AFTER_MIN)
    .bind(reason::STALE)
    .fetch_all(pool)
    .await?;
    for &(run_id, user_id, request_id, cost) in &stale {
        if let Some(req) = request_id
            && spend::release(pool, user_id, req).await.is_err()
        {
            tracing::error!(%run_id, "stale intro reservation could not be released");
        }
        append_audit(
            pool,
            run_id,
            user_id,
            JobStatus::Failed,
            Some(reason::STALE),
            cost,
        )
        .await;
    }
    let leaked: Vec<(Uuid, i64)> = sqlx::query_as(
        "SELECT s.request_id, s.user_id FROM incisive.spend s \
         WHERE s.kind = 'reservation' AND s.purpose = $1 \
           AND s.at < now() - make_interval(mins => $2) \
           AND NOT EXISTS (SELECT 1 FROM incisive.run_intros i \
                           WHERE i.request_id = s.request_id AND i.status = 'pending')",
    )
    .bind(PURPOSE)
    .bind(STALE_AFTER_MIN)
    .fetch_all(pool)
    .await?;
    for (req, user_id) in leaked {
        tracing::info!(request_id = %req, "leaked intro reservation released");
        spend::release(pool, user_id, req).await?;
    }
    Ok(stale.len())
}

async fn append_audit(
    pool: &PgPool,
    run_id: Uuid,
    user_id: i64,
    status: JobStatus,
    reason: Option<&'static str>,
    cost: i64,
) {
    if audit::append(
        pool,
        Some(run_id),
        Some(user_id),
        status.event(),
        audit::intro_detail(run_id, reason, cost),
    )
    .await
    .is_err()
    {
        tracing::error!(%run_id, "intro audit append failed");
    }
}

// ---------- the trigger ----------

/// Called by `ask::start_run` after every check has passed and the run holds
/// its reservation. Spawns the job and returns at once; every error is logged
/// by id and kind only and leaves the run alone. With `INTRO=off`, nothing.
pub fn start(state: &AppState, run_id: Uuid, user_id: i64, question: &str) {
    if !state.intro.settings.on {
        return;
    }
    let Some(llm) = state.llm.clone() else { return };
    let state = state.clone();
    let question = question.to_string();
    tokio::spawn(async move {
        let request_id = Uuid::new_v4();
        match insert_pending(&state.pool, run_id, request_id).await {
            Ok(true) => {}
            Ok(false) => return,
            Err(_) => {
                tracing::error!(%run_id, "intro pending row could not be stored");
                return;
            }
        }
        let job = AssertUnwindSafe(run_job(
            &state, &llm, run_id, user_id, request_id, &question,
        ));
        // The payload is dropped unread: it could quote the question.
        if job.catch_unwind().await.is_err() {
            tracing::error!(%run_id, "intro job panicked");
            let f = Finished {
                reason: Some(reason::PANIC),
                ..Finished::default()
            };
            let _ = finish(&state.pool, run_id, JobStatus::Failed, &f).await;
            let _ = spend::release(&state.pool, user_id, request_id).await;
            append_audit(&state.pool, run_id, user_id, JobStatus::Failed, f.reason, 0).await;
        }
    });
}

// ---------- the job ----------

/// The job (see the module docs). Ends the row and audits; returns how it
/// ended (for tests).
pub async fn run_job(
    state: &AppState,
    llm: &Arc<AgentLlm>,
    run_id: Uuid,
    user_id: i64,
    request_id: Uuid,
    question: &str,
) -> JobStatus {
    let (status, f) = job_inner(state, llm, run_id, user_id, request_id, question).await;
    match finish(&state.pool, run_id, status, &f).await {
        Ok(true) => {}
        Ok(false) => tracing::info!(%run_id, "intro row gone or already finished"),
        Err(_) => {
            tracing::error!(%run_id, "intro result could not be stored");
            let e = Finished {
                reason: Some(reason::STORE),
                cost_paise: f.cost_paise,
                ..Finished::default()
            };
            let _ = finish(&state.pool, run_id, JobStatus::Failed, &e).await;
        }
    }
    tracing::info!(%run_id, status = status.as_str(), reason = f.reason, cost_paise = f.cost_paise, "intro job finished");
    append_audit(&state.pool, run_id, user_id, status, f.reason, f.cost_paise).await;
    status
}

async fn job_inner(
    state: &AppState,
    llm: &Arc<AgentLlm>,
    run_id: Uuid,
    user_id: i64,
    request_id: Uuid,
    question: &str,
) -> (JobStatus, Finished) {
    let mut f = Finished::default();
    let fail = |mut f: Finished, s: JobStatus, r: &'static str| {
        f.reason = Some(r);
        (s, f)
    };
    let messages = [Message {
        role: Role::User,
        content: vec![ContentBlock::Text {
            text: user_message(question),
        }],
    }];
    let settings = &state.intro.settings;
    // Its own client (one attempt, no retry), else the agent's.
    let client = state
        .intro
        .client
        .clone()
        .unwrap_or_else(|| llm.client.clone());
    let call = |budget: Arc<RunBudget>| {
        LlmCall::new(
            client.clone(),
            budget,
            state.pool.clone(),
            CancelFlag::default(),
            settings.model.clone(),
            STEP,
        )
    };

    // The hold estimate, then the same amount reserved against the user's
    // caps under the job's own request id.
    let probe = Arc::new(RunBudget::new(
        request_id,
        user_id,
        0,
        Instant::now(),
        state.prices.clone(),
        state.usd_inr,
    ));
    let Some(estimate) =
        call(probe).turn_estimate(PROMPT, &messages, &[], Some(EFFORT), MAX_TOKENS)
    else {
        return fail(f, JobStatus::Failed, reason::UNPRICED);
    };
    // The run may have been deleted already: reserve nothing then.
    match store::run_exists(&state.pool, run_id).await {
        Ok(true) => {}
        Ok(false) => return fail(f, JobStatus::Skipped, reason::GONE),
        Err(_) => return fail(f, JobStatus::Failed, reason::STORE),
    }
    let user = match identity::find_user_by_id(&state.pool, user_id).await {
        Ok(Some(u)) => u,
        _ => return fail(f, JobStatus::Failed, reason::STORE),
    };
    match spend::reserve(
        &state.pool,
        &user,
        &state.caps,
        estimate,
        request_id,
        PURPOSE,
    )
    .await
    {
        Ok(CapCheck::Ok { .. }) => {}
        Ok(CapCheck::Blocked { .. }) => return fail(f, JobStatus::CapBlocked, reason::DAILY_CAP),
        Err(_) => return fail(f, JobStatus::Failed, reason::STORE),
    }
    let budget = Arc::new(RunBudget::new(
        request_id,
        user_id,
        estimate,
        Instant::now(),
        state.prices.clone(),
        state.usd_inr,
    ));
    // The call and its charge run in their own task (booked whatever happens
    // here); the job waits for it at most the time limit.
    let started_call =
        call(budget.clone()).start_turn(PROMPT, &messages, &[], Some(EFFORT), MAX_TOKENS);
    let mut handle = match started_call {
        Ok(c) => tokio::spawn(c.wait()),
        Err(e) => {
            let _ = spend::release(&state.pool, user_id, request_id).await;
            let r = if e == StepError::OverBudget {
                reason::COST
            } else {
                reason::API
            };
            return fail(f, JobStatus::Failed, r);
        }
    };
    let reply = match tokio::time::timeout(settings.time_limit, &mut handle).await {
        Ok(Ok(r)) => Ok(r),
        Ok(Err(_)) => Ok(Err(StepError::Unrecorded)),
        Err(_) => Err(()),
    };
    // The job ends here either way: release the reservation except what a
    // call still in flight holds (its late charge releases the rest).
    let keep = budget.finish();
    if spend::release_keeping(&state.pool, user_id, request_id, keep)
        .await
        .is_err()
    {
        tracing::error!(%run_id, "intro reservation could not be released");
    }
    f.cost_paise = budget.spent();
    let reply = match reply {
        Err(()) => {
            // Timed out: the call still finishes and books its charge in its
            // own task; the row's cost is then brought up to date.
            let (pool, budget) = (state.pool.clone(), budget.clone());
            tokio::spawn(async move {
                let _ = handle.await;
                if set_cost(&pool, run_id, budget.spent()).await.is_err() {
                    tracing::error!(%run_id, "intro late cost could not be stored");
                }
            });
            return fail(f, JobStatus::Failed, reason::TIME);
        }
        Ok(Err(StepError::OverBudget)) => return fail(f, JobStatus::Failed, reason::COST),
        Ok(Err(StepError::Unrecorded)) => return fail(f, JobStatus::Failed, reason::UNRECORDED),
        Ok(Err(e)) => {
            // A refusal arrives here too (the budgeted path reports it as
            // `Llm`, charged).
            tracing::warn!(%run_id, error_kind = e.kind(), "intro call failed");
            return fail(f, JobStatus::Failed, reason::API);
        }
        Ok(Ok(r)) => r,
    };
    tracing::info!(
        %run_id, input_tokens = reply.input_tokens, output_tokens = reply.output_tokens,
        stop_reason = ?reply.stop_reason, "intro call done"
    );
    if reply.truncated() {
        return fail(f, JobStatus::Failed, reason::TRUNCATED);
    }
    if reply.stop_reason != Some(StopReason::EndTurn) {
        return fail(f, JobStatus::Failed, reason::UNUSABLE);
    }
    let text = reply.text.trim();
    if text.is_empty() {
        return fail(f, JobStatus::Failed, reason::EMPTY);
    }
    f.text = Some(text.to_string());
    (JobStatus::Done, f)
}
