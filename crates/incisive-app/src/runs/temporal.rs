//! The temporal expert opinion's storage and after-run job (Temporal T3).
//! Server only.
//!
//! **Additive and detached.** The worker calls [`maybe_start`] once a run
//! has ended and its answer is saved: for a Full run with a written
//! AnswerV3 and `TEMPORAL=on` it inserts a `pending` row in
//! `incisive.run_temporal` and spawns [`run_job`] in its own task. The
//! run's row, status, timing, cost and reservation are never touched: the
//! job books its money under its **own** request id (`run_temporal.
//! request_id`), not the run's, and writes only its own row.
//!
//! **The job** ([`run_job`]): build the T1 table; skip with "not enough
//! primary studies" under two independent primary studies; reserve the
//! writer's worst-case hold against the **user's** caps (owner D3;
//! `spend::reserve`, purpose `temporal`); make one structured call through
//! the agent's budgeted path (`LlmCall`: hold, call, charge the real cost,
//! release the rest); run the code checks; store the board, the table and
//! the flags. Every failure ends the row `failed`, `skipped` or
//! `cap_blocked` with a fixed reason, audited as `temporal_*` with ids and
//! kinds only. Nothing here logs question, answer or reply text.
//!
//! **Money.** The reservation is the call's hold estimate
//! (`temporal_writer::reserve`: the prompt at about three bytes a token
//! plus `max_tokens` of output), and the job's budget limit is that same
//! amount, so at most one call is made (no retry). A reply cut short, a
//! refusal, an API error, a timeout or an unusable reply is charged as the
//! run's calls are and fails the job. As for runs, a call can cost more
//! than its hold only by its input excess; the real cost is always booked.
//!
//! **Recovery** ([`recover_stale`], from the run recovery): a `pending`
//! row older than [`STALE_AFTER_MIN`] minutes becomes `failed` ("stale")
//! and its reservation is released; a `temporal` reservation whose job row
//! is gone (the run was deleted) or finished is released after the same
//! time.

use std::panic::AssertUnwindSafe;
use std::sync::Arc;
use std::time::Instant;

use futures_util::FutureExt;
use serde_json::Value;
use sqlx::{PgPool, Row};
use uuid::Uuid;

use super::budget::RunBudget;
use super::worker::CancelFlag;
use super::{RunMode, RunStatus, TemporalView, store};
use crate::agent::AgentLlm;
use crate::agent::llm::{LlmCall, StepError, read_structured};
use crate::agent::temporal_writer::{
    self as writer, BoardFlags, CheckContext, ExpertBoardV1, TemporalSettings,
};
use crate::audit::{self, Event};
use crate::identity;
use crate::server::AppState;
use crate::spend::{self, CapCheck};
use crate::temporal::{StudyKind, TemporalInput, TemporalTable, build};

/// The spend purpose of the job's reservation.
pub const PURPOSE: &str = "temporal";
/// A pending row older than this is failed by recovery (the job takes at
/// most its 300 s time limit plus its charge).
pub const STALE_AFTER_MIN: i32 = 10;
/// The job skips under this many independent primary studies.
pub const MIN_PRIMARY: usize = 2;

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
            JobStatus::Done => Event::TemporalDone,
            JobStatus::Failed => Event::TemporalFailed,
            JobStatus::Skipped => Event::TemporalSkipped,
            JobStatus::CapBlocked => Event::TemporalCapBlocked,
        }
    }
}

/// Fixed reason kinds (stored in `reason`; never model text).
pub mod reason {
    pub const NO_ANSWER: &str = "no answer";
    pub const NOT_ENOUGH: &str = "not enough primary studies";
    pub const DAILY_CAP: &str = "limit reached";
    pub const UNPRICED: &str = "unpriced model";
    pub const COST: &str = "cost limit";
    pub const TIME: &str = "time";
    pub const API: &str = "api error";
    pub const UNUSABLE: &str = "unusable reply";
    pub const TRUNCATED: &str = "truncated";
    pub const UNRECORDED: &str = "unrecorded charge";
    pub const STORE: &str = "store";
    pub const PANIC: &str = "panic";
    pub const STALE: &str = "stale";
    pub const GONE: &str = "run deleted";
}

// ---------- store ----------

/// Insert the run's `pending` row with the job's request id. False when a
/// row already exists (the job is not started twice) or the run is gone.
pub async fn insert_pending(pool: &PgPool, run_id: Uuid, request_id: Uuid) -> sqlx::Result<bool> {
    let r = sqlx::query(
        "INSERT INTO incisive.run_temporal (run_id, status, request_id) \
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
    pub topic: Option<String>,
    pub evidence_year: Option<i32>,
    pub table: Option<Value>,
    pub board: Option<Value>,
    pub flags: Option<Value>,
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
        "UPDATE incisive.run_temporal SET status = $2, reason = $3, topic = $4, \
           evidence_year = $5, table_json = $6, board_json = $7, flags = $8, \
           cost_paise = $9, finished_at = now() \
         WHERE run_id = $1 AND status = 'pending'",
    )
    .bind(run_id)
    .bind(status.as_str())
    .bind(f.reason)
    .bind(f.topic.as_deref())
    .bind(f.evidence_year)
    .bind(&f.table)
    .bind(&f.board)
    .bind(&f.flags)
    .bind(f.cost_paise.max(0))
    .execute(pool)
    .await?;
    Ok(r.rows_affected() > 0)
}

/// Raise the row's cost to `paise` (a late charge, booked after the job
/// gave its call up, has settled). Never lowers it.
pub async fn set_cost(pool: &PgPool, run_id: Uuid, paise: i64) -> sqlx::Result<()> {
    sqlx::query(
        "UPDATE incisive.run_temporal SET cost_paise = GREATEST(cost_paise, $2) WHERE run_id = $1",
    )
    .bind(run_id)
    .bind(paise.max(0))
    .execute(pool)
    .await?;
    Ok(())
}

/// The run's expert opinion, only if the run belongs to `user_id`:
/// `Ok(None)` when the run is not theirs or does not exist; `Ok(Some(None))`
/// when it is theirs and has no opinion row (not run).
pub async fn get_for_user(
    pool: &PgPool,
    user_id: i64,
    run_id: Uuid,
) -> sqlx::Result<Option<Option<TemporalView>>> {
    let row = sqlx::query(
        "SELECT t.run_id AS t_run, t.status, t.reason, t.topic, t.evidence_year, \
                t.table_json, t.board_json, t.flags \
         FROM incisive.runs r \
         LEFT JOIN incisive.run_temporal t ON t.run_id = r.id \
         WHERE r.id = $1 AND r.user_id = $2",
    )
    .bind(run_id)
    .bind(user_id)
    .fetch_optional(pool)
    .await?;
    let Some(row) = row else { return Ok(None) };
    let t_run: Option<Uuid> = row.try_get("t_run")?;
    if t_run.is_none() {
        return Ok(Some(None));
    }
    Ok(Some(Some(TemporalView {
        status: row.try_get("status")?,
        reason: row.try_get("reason")?,
        topic: row.try_get("topic")?,
        evidence_year: row.try_get("evidence_year")?,
        board: row.try_get("board_json")?,
        table: row.try_get("table_json")?,
        flags: row.try_get("flags")?,
    })))
}

/// Recovery: fail every `pending` row older than [`STALE_AFTER_MIN`]
/// minutes ("stale"), release each one's reservation and audit it; then
/// release `temporal` reservations older than that whose row is gone or no
/// longer pending. Returns the rows failed.
pub async fn recover_stale(pool: &PgPool) -> sqlx::Result<usize> {
    let stale: Vec<(Uuid, i64, Option<Uuid>, i64)> = sqlx::query_as(
        "UPDATE incisive.run_temporal t SET status = 'failed', reason = $2, finished_at = now() \
         FROM incisive.runs r \
         WHERE r.id = t.run_id AND t.status = 'pending' \
           AND t.created_at < now() - make_interval(mins => $1) \
         RETURNING t.run_id, r.user_id, t.request_id, t.cost_paise",
    )
    .bind(STALE_AFTER_MIN)
    .bind(reason::STALE)
    .fetch_all(pool)
    .await?;
    for &(run_id, user_id, request_id, cost) in &stale {
        if let Some(req) = request_id
            && spend::release(pool, user_id, req).await.is_err()
        {
            tracing::error!(%run_id, "stale temporal reservation could not be released");
        }
        append_audit(
            pool,
            run_id,
            user_id,
            JobStatus::Failed,
            Some(reason::STALE),
            cost,
            0,
        )
        .await;
    }
    let leaked: Vec<(Uuid, i64)> = sqlx::query_as(
        "SELECT s.request_id, s.user_id FROM incisive.spend s \
         WHERE s.kind = 'reservation' AND s.purpose = $1 \
           AND s.at < now() - make_interval(mins => $2) \
           AND NOT EXISTS (SELECT 1 FROM incisive.run_temporal t \
                           WHERE t.request_id = s.request_id AND t.status = 'pending')",
    )
    .bind(PURPOSE)
    .bind(STALE_AFTER_MIN)
    .fetch_all(pool)
    .await?;
    for (req, user_id) in leaked {
        tracing::info!(request_id = %req, "leaked temporal reservation released");
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
    flags: usize,
) {
    if audit::append(
        pool,
        Some(run_id),
        Some(user_id),
        status.event(),
        audit::temporal_detail(run_id, reason, cost, flags),
    )
    .await
    .is_err()
    {
        tracing::error!(%run_id, "temporal audit append failed");
    }
}

// ---------- the trigger ----------

/// Whether a run that ended with `status` (its answer saved: only `done`
/// and `partial` runs have one) gets the expert opinion: Full, a version 3
/// body with a written answer, and `TEMPORAL=on`. Pure, so it is tested
/// apart from the worker.
pub fn should_run(on: bool, mode: RunMode, status: Option<RunStatus>, body: &Value) -> bool {
    on && mode == RunMode::Focused
        && matches!(status, Some(RunStatus::Done | RunStatus::Partial))
        && body.get("version").and_then(Value::as_i64) == Some(3)
        && body.get("answer").is_some_and(|a| a.is_object())
}

/// Called by the worker once the run has ended (its status stored, its
/// reservation released): when [`should_run`] holds, insert the pending row
/// and spawn [`run_job`], detached. Never fails the run: every error is
/// logged by id and kind only.
pub async fn maybe_start(
    state: &AppState,
    run_id: Uuid,
    user_id: i64,
    mode: RunMode,
    status: Option<RunStatus>,
    body: Option<Value>,
) {
    let Some(body) = body else { return };
    if !should_run(state.temporal.on, mode, status, &body) {
        return;
    }
    let Some(llm) = state.llm.clone() else { return };
    let request_id = Uuid::new_v4();
    match insert_pending(&state.pool, run_id, request_id).await {
        Ok(true) => {}
        Ok(false) => return,
        Err(_) => {
            tracing::error!(%run_id, "temporal pending row could not be stored");
            return;
        }
    }
    let state = state.clone();
    tokio::spawn(async move {
        let job = AssertUnwindSafe(run_job(&state, &llm, run_id, user_id, request_id, &body));
        // The payload is dropped unread: it could quote stored text.
        if job.catch_unwind().await.is_err() {
            tracing::error!(%run_id, "temporal job panicked");
            let f = Finished {
                reason: Some(reason::PANIC),
                ..Finished::default()
            };
            let _ = finish(&state.pool, run_id, JobStatus::Failed, &f).await;
            let _ = spend::release(&state.pool, user_id, request_id).await;
            append_audit(
                &state.pool,
                run_id,
                user_id,
                JobStatus::Failed,
                f.reason,
                0,
                0,
            )
            .await;
        }
    });
}

// ---------- the job ----------

/// Independent primary studies of the table (linked follow-up reports
/// count once).
pub fn primary_count(t: &TemporalTable) -> usize {
    t.studies
        .iter()
        .filter(|s| s.kind == StudyKind::Primary && s.parent_id.is_none())
        .count()
}

/// The job (see the module docs). Ends the row and audits; returns how it
/// ended (for tests).
pub async fn run_job(
    state: &AppState,
    llm: &Arc<AgentLlm>,
    run_id: Uuid,
    user_id: i64,
    request_id: Uuid,
    body: &Value,
) -> JobStatus {
    let (status, f) = job_inner(state, llm, run_id, user_id, request_id, body).await;
    let flags = f
        .flags
        .as_ref()
        .and_then(|v| serde_json::from_value::<BoardFlags>(v.clone()).ok())
        .map_or(0, |b| b.total());
    match finish(&state.pool, run_id, status, &f).await {
        Ok(true) => {}
        Ok(false) => tracing::info!(%run_id, "temporal row gone or already finished"),
        Err(_) => {
            tracing::error!(%run_id, "temporal result could not be stored");
            let e = Finished {
                reason: Some(reason::STORE),
                cost_paise: f.cost_paise,
                ..Finished::default()
            };
            let _ = finish(&state.pool, run_id, JobStatus::Failed, &e).await;
        }
    }
    tracing::info!(%run_id, status = status.as_str(), reason = f.reason, cost_paise = f.cost_paise, flags, "temporal job finished");
    append_audit(
        &state.pool,
        run_id,
        user_id,
        status,
        f.reason,
        f.cost_paise,
        flags,
    )
    .await;
    status
}

async fn job_inner(
    state: &AppState,
    llm: &Arc<AgentLlm>,
    run_id: Uuid,
    user_id: i64,
    request_id: Uuid,
    body: &Value,
) -> (JobStatus, Finished) {
    let mut f = Finished::default();
    let fail = |mut f: Finished, s: JobStatus, r: &'static str| {
        f.reason = Some(r);
        (s, f)
    };
    let input: TemporalInput = match serde_json::from_value(body.clone()) {
        Ok(i) => i,
        Err(_) => return fail(f, JobStatus::Skipped, reason::NO_ANSWER),
    };
    let table = build(&input);
    f.evidence_year = table.run_year;
    f.table = serde_json::to_value(&table).ok();
    if primary_count(&table) < MIN_PRIMARY {
        return fail(f, JobStatus::Skipped, reason::NOT_ENOUGH);
    }
    let answer_text = writer::answer_text(body, &input);
    let user_msg = writer::user(&answer_text, &table, &input);
    // The checks compare the board with the text the writer was shown.
    let shown = writer::shown_answer(&answer_text);
    let settings: TemporalSettings = state.temporal;
    let model = llm.models.temporal.clone();

    // The hold estimate, then the same amount reserved against the user's
    // caps (D3) under the job's own request id.
    let probe = RunBudget::new(
        request_id,
        user_id,
        0,
        Instant::now(),
        state.prices.clone(),
        state.usd_inr,
    );
    let Some(estimate) = writer::reserve(&probe, &model, &user_msg, settings.max_tokens) else {
        return fail(f, JobStatus::Failed, reason::UNPRICED);
    };
    // The run may have been deleted since it ended: reserve nothing then.
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
    // The writer's own client (one attempt, no retry), else the agent's.
    let client = state
        .temporal_client
        .clone()
        .unwrap_or_else(|| llm.client.clone());
    let call = LlmCall::new(
        client,
        budget.clone(),
        state.pool.clone(),
        CancelFlag::default(),
        model,
        writer::STEP,
    );
    let schema = writer::output_schema();
    let started = Instant::now();
    // The call and its charge run in their own task (booked whatever happens
    // here); the job waits for it at most the time limit.
    let started_call = call.start(
        writer::SYSTEM,
        &user_msg,
        Some(&schema),
        settings.max_tokens,
    );
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
        tracing::error!(%run_id, "temporal reservation could not be released");
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
                    tracing::error!(%run_id, "temporal late cost could not be stored");
                }
            });
            return fail(f, JobStatus::Failed, reason::TIME);
        }
        Ok(Err(StepError::OverBudget)) => return fail(f, JobStatus::Failed, reason::COST),
        Ok(Err(StepError::Unrecorded)) => return fail(f, JobStatus::Failed, reason::UNRECORDED),
        Ok(Err(e)) => {
            tracing::warn!(%run_id, error_kind = e.kind(), "temporal writer call failed");
            return fail(f, JobStatus::Failed, reason::API);
        }
        Ok(Ok(r)) => r,
    };
    let secs = started.elapsed().as_secs();
    let board: ExpertBoardV1 = match read_structured(&reply) {
        Ok(b) => b,
        Err(fault) => {
            tracing::warn!(%run_id, fault = fault.category, output_tokens = reply.output_tokens, "temporal reply unusable");
            let r = if reply.truncated() {
                reason::TRUNCATED
            } else {
                reason::UNUSABLE
            };
            return fail(f, JobStatus::Failed, r);
        }
    };
    let refs = writer::reference_numbers(&input);
    let markers = writer::marker_reference_numbers(&input, &table);
    let flags = writer::check(
        &board,
        &CheckContext {
            answer_text: &shown,
            table: &table,
            references: &refs,
            marker_references: &markers,
        },
    );
    tracing::info!(
        %run_id, input_tokens = reply.input_tokens, output_tokens = reply.output_tokens, secs,
        flags = flags.total(), "temporal writer done"
    );
    let topic = crate::agent::clip(&board.topic, 200);
    f.topic = (!topic.is_empty()).then_some(topic);
    f.board = serde_json::to_value(&board).ok();
    f.flags = serde_json::to_value(&flags).ok();
    (JobStatus::Done, f)
}
