//! The graph job and its loop. Server only.
//!
//! **Additive and detached.** [`graph_loop`] (spawned by `main` only with
//! `GRAPH=on`) queues every finished run created from `GRAPH_FROM` on
//! ([`enqueue_new`]) and works the queue one job at a time ([`process`]).
//! A job reads the run's stored answer and writes only `graph_*` rows: the
//! run's row, answer, steps, cost, reservation, expert opinion and memory job
//! are never touched. Runs that are not `done` or `partial`, or whose answer
//! is not a version 3 body with an answer, end `skipped` with no call.
//!
//! **The call.** One structured call per attempt (`LlmCall::start` with the
//! picks schema), [`MAX_TOKENS`] of output at [`EFFORT`] (low: thinking
//! counts toward `max_tokens`), through the graph's own client (one attempt,
//! [`TIME_LIMIT`]; see [`client`]). The code checks (`extract::check`) clean
//! the reply; a reply with no organ, or with no outcome while the answer has
//! outcome rows, is `empty_picks` and writes nothing.
//!
//! **Money.** Each attempt reserves the call's hold against the **user's**
//! caps under its own request id (`graph_jobs.request_id`, purpose
//! [`PURPOSE`]); the budget limit is that same hold, so an attempt makes at
//! most one call. The charge goes under that id (its purpose reads
//! `run:<request_id>`), and the job's `cost_paise` adds up every attempt. At
//! a cap the job becomes `cap_blocked` until the next IST midnight and the
//! attempt is not counted.
//!
//! **Retries.** An API error, a timeout, a cut-off or unreadable reply,
//! `empty_picks` or a store error puts the job back to `pending` (after
//! [`RETRY_AFTER_MIN`] minutes) until [`MAX_ATTEMPTS`]; then it is `failed`.
//! An unpriced model, a cost refusal, an unrecorded charge or a panic fails
//! it at once. A run deleted at any point leaves no rows: its job row goes
//! with it, and `store::write_run` refuses a run that is gone.
//!
//! **Recovery** ([`recover_stale`], every tick): a claimed job holds a lease
//! of [`STALE_AFTER_MIN`] minutes (`next_try_at`); a `running` job past it is
//! returned to `pending` (or `failed` with no attempts left) and its
//! reservation released; a `graph` reservation older than that whose job is
//! not running is released.
//!
//! Logs carry ids, fixed codes, counts and paise only: never the question,
//! the reply or a name.

use std::panic::AssertUnwindSafe;
use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::NaiveDate;
use futures_util::FutureExt;
use serde_json::Value;
use sqlx::{PgPool, Row};
use uuid::Uuid;

use super::extract::{self, RawPicks};
use super::store;
use crate::agent::answer_v3::AnswerV3;
use crate::agent::llm::{Effort, LlmCall, OutputSchema, StepError, hold_estimate, read_structured};
use crate::identity;
use crate::runs::budget::RunBudget;
use crate::runs::worker::CancelFlag;
use crate::server::AppState;
use crate::spend::{self, CapCheck};

/// The spend purpose of the job's reservation.
pub const PURPOSE: &str = "graph";
/// The step name on its Claude call (logs only; not a finishing step).
pub const STEP: &str = "graph";
/// `max_tokens` of the call.
pub const MAX_TOKENS: u32 = 1200;
/// The call's effort (see the module docs).
pub const EFFORT: Effort = Effort::Low;
/// Attempts a job gets before it is `failed`.
pub const MAX_ATTEMPTS: i32 = 3;
/// The job gives the call up after this long, and its own client's one
/// attempt times out at the same moment (its charge is still booked).
pub const TIME_LIMIT: Duration = Duration::from_secs(30);
/// A claimed job's lease: past it, recovery takes the job back.
pub const STALE_AFTER_MIN: i32 = 5;
/// A retried job waits this long.
pub const RETRY_AFTER_MIN: i32 = 2;
/// How often the loop wakes.
const EVERY: Duration = Duration::from_secs(30);
/// Most jobs worked per tick, one after another.
const PER_TICK: usize = 5;

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

/// Where a job stands after [`process`] (its row's `status`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobStatus {
    Done,
    /// Back in the queue for another attempt.
    Pending,
    Failed,
    Skipped,
    CapBlocked,
    /// Nothing was done: no due job for the run (none, not yet due, finished,
    /// claimed elsewhere) or `GRAPH=off`.
    NotDue,
}

impl JobStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            JobStatus::Done => "done",
            JobStatus::Pending => "pending",
            JobStatus::Failed => "failed",
            JobStatus::Skipped => "skipped",
            JobStatus::CapBlocked => "cap_blocked",
            JobStatus::NotDue => "not_due",
        }
    }
}

/// Fixed codes (stored in `last_error_kind`; never model text).
pub mod reason {
    pub const DAILY_CAP: &str = "daily_cap";
    pub const UNPRICED: &str = "unpriced";
    pub const COST: &str = "cost";
    pub const TIME: &str = "time";
    pub const API: &str = "api";
    pub const UNUSABLE: &str = "unusable";
    pub const TRUNCATED: &str = "truncated";
    pub const EMPTY_PICKS: &str = "empty_picks";
    pub const UNRECORDED: &str = "unrecorded";
    pub const STORE: &str = "store";
    pub const PANIC: &str = "panic";
    pub const STALE: &str = "stale";
    pub const GONE: &str = "run_gone";
    pub const NOT_FINISHED: &str = "not_finished";
    pub const NOT_V3: &str = "not_v3";
    pub const NO_ANSWER: &str = "no_answer";
    pub const NO_CLIENT: &str = "no_client";
}

/// How one attempt ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Outcome {
    Done {
        dropped: u32,
    },
    Skip(&'static str),
    Cap,
    /// Worth another attempt, while attempts remain.
    Retry(&'static str),
    /// Final at once.
    Fail(&'static str),
}

impl Outcome {
    fn code(self) -> Option<&'static str> {
        match self {
            Outcome::Done { .. } => None,
            Outcome::Cap => Some(reason::DAILY_CAP),
            Outcome::Skip(r) | Outcome::Retry(r) | Outcome::Fail(r) => Some(r),
        }
    }

    /// The `finish` target.
    fn target(self) -> &'static str {
        match self {
            Outcome::Done { .. } => "done",
            Outcome::Skip(_) => "skipped",
            Outcome::Cap => "cap",
            Outcome::Retry(_) => "retry",
            Outcome::Fail(_) => "failed",
        }
    }

    /// The status this outcome gives when the row could not be read back.
    fn status(self) -> JobStatus {
        match self {
            Outcome::Done { .. } => JobStatus::Done,
            Outcome::Skip(_) => JobStatus::Skipped,
            Outcome::Cap => JobStatus::CapBlocked,
            Outcome::Retry(_) => JobStatus::Pending,
            Outcome::Fail(_) => JobStatus::Failed,
        }
    }
}

// ---------- the queue ----------

/// Queue every finished run (done, partial, failed, interrupted) created on
/// or after `from` (an IST date) that has no job yet. Returns how many.
pub async fn enqueue_new(pool: &PgPool, from: NaiveDate) -> sqlx::Result<u64> {
    let r = sqlx::query(
        "INSERT INTO incisive.graph_jobs (run_id) \
         SELECT r.id FROM incisive.runs r \
         WHERE r.status IN ('done','partial','failed','interrupted') \
           AND r.created_at >= ($1::date::timestamp AT TIME ZONE 'Asia/Kolkata') \
           AND NOT EXISTS (SELECT 1 FROM incisive.graph_jobs j WHERE j.run_id = r.id) \
         ON CONFLICT DO NOTHING",
    )
    .bind(from)
    .execute(pool)
    .await?;
    Ok(r.rows_affected())
}

/// The next due job, if any.
async fn next_due(pool: &PgPool) -> sqlx::Result<Option<Uuid>> {
    sqlx::query_scalar(
        "SELECT run_id FROM incisive.graph_jobs \
         WHERE status IN ('pending','cap_blocked') AND next_try_at <= now() AND attempts < $1 \
         ORDER BY next_try_at LIMIT 1",
    )
    .bind(MAX_ATTEMPTS)
    .fetch_optional(pool)
    .await
}

/// Claim the run's job if it is due: `running`, one more attempt, a new
/// request id and a lease. Returns the run's user, or `None` when there is
/// nothing to claim (another instance may have it).
async fn claim(pool: &PgPool, run_id: Uuid, request_id: Uuid) -> sqlx::Result<Option<i64>> {
    sqlx::query_scalar(
        "UPDATE incisive.graph_jobs j SET status = 'running', attempts = j.attempts + 1, \
                request_id = $2, next_try_at = now() + make_interval(mins => $3) \
         FROM incisive.runs r \
         WHERE j.run_id = $1 AND r.id = j.run_id \
           AND j.status IN ('pending','cap_blocked') AND j.next_try_at <= now() \
           AND j.attempts < $4 \
         RETURNING r.user_id",
    )
    .bind(run_id)
    .bind(request_id)
    .bind(STALE_AFTER_MIN)
    .bind(MAX_ATTEMPTS)
    .fetch_optional(pool)
    .await
}

/// End the attempt, only while this claim still holds the row. `None` when
/// the row is gone (the run was deleted) or recovery took it back.
async fn finish(
    pool: &PgPool,
    run_id: Uuid,
    request_id: Uuid,
    outcome: Outcome,
    cost: i64,
) -> sqlx::Result<Option<JobStatus>> {
    let dropped = match outcome {
        Outcome::Done { dropped } => i32::try_from(dropped).unwrap_or(i32::MAX),
        _ => 0,
    };
    let status: Option<String> = sqlx::query_scalar(
        "UPDATE incisive.graph_jobs SET \
           status = CASE $3::text \
             WHEN 'retry' THEN CASE WHEN attempts < $6 THEN 'pending' ELSE 'failed' END \
             WHEN 'cap' THEN 'cap_blocked' ELSE $3::text END, \
           attempts = CASE WHEN $3::text = 'cap' THEN GREATEST(attempts - 1, 0) ELSE attempts END, \
           last_error_kind = $4, dropped = $5, cost_paise = cost_paise + $7, \
           next_try_at = CASE $3::text \
             WHEN 'retry' THEN now() + make_interval(mins => $8) \
             WHEN 'cap' THEN (date_trunc('day', now() AT TIME ZONE 'Asia/Kolkata') \
                              + interval '1 day') AT TIME ZONE 'Asia/Kolkata' \
             ELSE next_try_at END, \
           finished_at = CASE \
             WHEN $3::text IN ('done','skipped','failed') THEN now() \
             WHEN $3::text = 'retry' AND attempts >= $6 THEN now() \
             ELSE NULL END \
         WHERE run_id = $1 AND status = 'running' AND request_id = $2 \
         RETURNING status",
    )
    .bind(run_id)
    .bind(request_id)
    .bind(outcome.target())
    .bind(outcome.code())
    .bind(dropped)
    .bind(MAX_ATTEMPTS)
    .bind(cost.max(0))
    .bind(RETRY_AFTER_MIN)
    .fetch_optional(pool)
    .await?;
    Ok(status.map(|s| match s.as_str() {
        "done" => JobStatus::Done,
        "pending" => JobStatus::Pending,
        "skipped" => JobStatus::Skipped,
        "cap_blocked" => JobStatus::CapBlocked,
        _ => JobStatus::Failed,
    }))
}

/// Add a late charge (a call that outlived the time limit) to the job's cost.
async fn add_cost(pool: &PgPool, run_id: Uuid, paise: i64) -> sqlx::Result<()> {
    sqlx::query("UPDATE incisive.graph_jobs SET cost_paise = cost_paise + $2 WHERE run_id = $1")
        .bind(run_id)
        .bind(paise.max(0))
        .execute(pool)
        .await?;
    Ok(())
}

/// A job whose task panicked: `failed` with `panic`, its reservation
/// released.
async fn fail_panicked(pool: &PgPool, run_id: Uuid) {
    let row: sqlx::Result<Option<(i64, Option<Uuid>)>> = sqlx::query_as(
        "UPDATE incisive.graph_jobs j SET status = 'failed', last_error_kind = $2, \
                finished_at = now() \
         FROM incisive.runs r \
         WHERE j.run_id = $1 AND r.id = j.run_id AND j.status = 'running' \
         RETURNING r.user_id, j.request_id",
    )
    .bind(run_id)
    .bind(reason::PANIC)
    .fetch_optional(pool)
    .await;
    match row {
        Ok(Some((user_id, Some(req)))) => {
            if spend::release(pool, user_id, req).await.is_err() {
                tracing::error!(%run_id, "graph reservation could not be released");
            }
        }
        Ok(_) => {}
        Err(_) => tracing::error!(%run_id, "graph panicked job could not be stored"),
    }
}

/// Recovery: every `running` job past its lease goes back to `pending` (or
/// `failed` with no attempts left), code `stale`, and its reservation is
/// released; then `graph` reservations older than the lease whose job is not
/// running are released. Returns the jobs taken back.
pub async fn recover_stale(pool: &PgPool) -> sqlx::Result<usize> {
    let stale: Vec<(Uuid, i64, Option<Uuid>)> = sqlx::query_as(
        "UPDATE incisive.graph_jobs j SET \
           status = CASE WHEN j.attempts < $1 THEN 'pending' ELSE 'failed' END, \
           last_error_kind = $2, next_try_at = now(), \
           finished_at = CASE WHEN j.attempts < $1 THEN NULL ELSE now() END \
         FROM incisive.runs r \
         WHERE r.id = j.run_id AND j.status = 'running' AND j.next_try_at < now() \
         RETURNING j.run_id, r.user_id, j.request_id",
    )
    .bind(MAX_ATTEMPTS)
    .bind(reason::STALE)
    .fetch_all(pool)
    .await?;
    for &(run_id, user_id, request_id) in &stale {
        tracing::info!(%run_id, "stale graph job taken back");
        if let Some(req) = request_id
            && spend::release(pool, user_id, req).await.is_err()
        {
            tracing::error!(%run_id, "stale graph reservation could not be released");
        }
    }
    let leaked: Vec<(Uuid, i64)> = sqlx::query_as(
        "SELECT s.request_id, s.user_id FROM incisive.spend s \
         WHERE s.kind = 'reservation' AND s.purpose = $1 \
           AND s.at < now() - make_interval(mins => $2) \
           AND NOT EXISTS (SELECT 1 FROM incisive.graph_jobs j \
                           WHERE j.request_id = s.request_id AND j.status = 'running')",
    )
    .bind(PURPOSE)
    .bind(STALE_AFTER_MIN)
    .fetch_all(pool)
    .await?;
    for (req, user_id) in leaked {
        tracing::info!(request_id = %req, "leaked graph reservation released");
        spend::release(pool, user_id, req).await?;
    }
    Ok(stale.len())
}

// ---------- the loop ----------

/// Every 30 s: recovery, queue new runs, then work due jobs one at a time.
/// Returns at once with `GRAPH=off` (or no `GRAPH_FROM`). Never exits
/// otherwise; a panic in one job is that job's failure.
pub async fn graph_loop(state: AppState) {
    if !state.graph.settings.on {
        return;
    }
    let Some(from) = state.graph.settings.from else {
        tracing::warn!(
            code = "no_from",
            "GRAPH=on without GRAPH_FROM; graph loop not started"
        );
        return;
    };
    let mut every = tokio::time::interval(EVERY);
    every.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        every.tick().await;
        tick(&state, from).await;
    }
}

async fn tick(state: &AppState, from: NaiveDate) {
    let pool = &state.pool;
    if recover_stale(pool).await.is_err() {
        tracing::warn!(code = "recover", "graph recovery failed; will retry");
    }
    if enqueue_new(pool, from).await.is_err() {
        tracing::warn!(code = "enqueue", "graph jobs not queued");
    }
    for _ in 0..PER_TICK {
        let run_id = match next_due(pool).await {
            Ok(Some(id)) => id,
            Ok(None) => break,
            Err(_) => {
                tracing::warn!(code = "claim", "graph claim failed");
                break;
            }
        };
        // The payload is dropped unread: it could quote stored text.
        if AssertUnwindSafe(process(state, run_id))
            .catch_unwind()
            .await
            .is_err()
        {
            tracing::error!(%run_id, "graph job panicked");
            fail_panicked(pool, run_id).await;
        }
    }
}

// ---------- the job ----------

/// Claim the run's job if it is due, make one attempt and store how it
/// ended. Returns the job's status after it ([`JobStatus::NotDue`] when
/// nothing was claimed, or with `GRAPH=off`).
pub async fn process(state: &AppState, run_id: Uuid) -> JobStatus {
    if !state.graph.settings.on {
        return JobStatus::NotDue;
    }
    let pool = &state.pool;
    let request_id = Uuid::new_v4();
    let user_id = match claim(pool, run_id, request_id).await {
        Ok(Some(u)) => u,
        Ok(None) => return JobStatus::NotDue,
        Err(_) => {
            tracing::warn!(%run_id, code = "claim", "graph claim failed");
            return JobStatus::NotDue;
        }
    };
    let (outcome, cost) = attempt(state, run_id, user_id, request_id).await;
    let status = match finish(pool, run_id, request_id, outcome, cost).await {
        Ok(Some(s)) => s,
        Ok(None) => {
            tracing::info!(%run_id, "graph job gone or taken back");
            outcome.status()
        }
        Err(_) => {
            // Left `running`: recovery takes it back after the lease.
            tracing::error!(%run_id, "graph result could not be stored");
            outcome.status()
        }
    };
    let dropped = match outcome {
        Outcome::Done { dropped } => dropped,
        _ => 0,
    };
    tracing::info!(
        %run_id, status = status.as_str(), reason = outcome.code(), cost_paise = cost, dropped,
        "graph job finished"
    );
    status
}

/// The run's status, answer body and answer version (`None`: no run).
async fn load(
    pool: &PgPool,
    run_id: Uuid,
) -> sqlx::Result<Option<(String, Option<Value>, Option<i16>)>> {
    let r = sqlx::query(
        "SELECT r.status, a.body, a.answer_version FROM incisive.runs r \
         LEFT JOIN incisive.run_answers a ON a.run_id = r.id WHERE r.id = $1",
    )
    .bind(run_id)
    .fetch_optional(pool)
    .await?;
    r.map(|r| {
        Ok((
            r.try_get("status")?,
            r.try_get("body")?,
            r.try_get("answer_version")?,
        ))
    })
    .transpose()
}

/// One attempt: `(outcome, paise spent)`.
async fn attempt(state: &AppState, run_id: Uuid, user_id: i64, request_id: Uuid) -> (Outcome, i64) {
    let pool = &state.pool;
    let settings = &state.graph.settings;
    // Its own client (one attempt, no retry), else the agent's.
    let Some(client) = state
        .graph
        .client
        .clone()
        .or_else(|| state.llm.as_ref().map(|l| l.client.clone()))
    else {
        return (Outcome::Fail(reason::NO_CLIENT), 0);
    };

    // What the run holds; nothing is called for a run without a usable answer.
    let (status, body, version) = match load(pool, run_id).await {
        Ok(Some(r)) => r,
        Ok(None) => return (Outcome::Skip(reason::GONE), 0),
        Err(_) => return (Outcome::Retry(reason::STORE), 0),
    };
    if !matches!(status.as_str(), "done" | "partial") {
        return (Outcome::Skip(reason::NOT_FINISHED), 0);
    }
    let Some(body) = body else {
        return (Outcome::Skip(reason::NO_ANSWER), 0);
    };
    if version != Some(3) {
        return (Outcome::Skip(reason::NOT_V3), 0);
    }
    let a: AnswerV3 = match serde_json::from_value(body) {
        Ok(a) => a,
        Err(_) => return (Outcome::Skip(reason::NO_ANSWER), 0),
    };
    let known = match store::known_names(pool, user_id).await {
        Ok(k) => k,
        Err(_) => return (Outcome::Retry(reason::STORE), 0),
    };
    let Some(user_msg) = extract::user_message(&a, &known) else {
        return (Outcome::Skip(reason::NO_ANSWER), 0);
    };
    let pmids: Vec<String> = a
        .references
        .iter()
        .map(|r| r.pmid.trim().to_string())
        .filter(|p| !p.is_empty())
        .collect();
    let papers = match store::pub_types_for(pool, &pmids).await {
        Ok(types) => store::papers_from(&a, &types),
        Err(_) => return (Outcome::Retry(reason::STORE), 0),
    };
    let schema = OutputSchema::new(extract::schema()).with_effort(EFFORT);

    // The hold estimate, then the same amount reserved against the user's
    // caps under the attempt's own request id.
    let probe = RunBudget::new(
        request_id,
        user_id,
        0,
        Instant::now(),
        state.prices.clone(),
        state.usd_inr,
    );
    let Some(estimate) = hold_estimate(
        &probe,
        &settings.model,
        extract::SYSTEM,
        &user_msg,
        Some(&schema),
        MAX_TOKENS,
    ) else {
        return (Outcome::Fail(reason::UNPRICED), 0);
    };
    // The run may have been deleted already: reserve nothing then.
    match crate::runs::store::run_exists(pool, run_id).await {
        Ok(true) => {}
        Ok(false) => return (Outcome::Skip(reason::GONE), 0),
        Err(_) => return (Outcome::Retry(reason::STORE), 0),
    }
    let user = match identity::find_user_by_id(pool, user_id).await {
        Ok(Some(u)) => u,
        _ => return (Outcome::Retry(reason::STORE), 0),
    };
    match spend::reserve(pool, &user, &state.caps, estimate, request_id, PURPOSE).await {
        Ok(CapCheck::Ok { .. }) => {}
        Ok(CapCheck::Blocked { .. }) => return (Outcome::Cap, 0),
        Err(_) => return (Outcome::Retry(reason::STORE), 0),
    }
    let budget = Arc::new(RunBudget::new(
        request_id,
        user_id,
        estimate,
        Instant::now(),
        state.prices.clone(),
        state.usd_inr,
    ));
    let call = LlmCall::new(
        client,
        budget.clone(),
        pool.clone(),
        CancelFlag::default(),
        settings.model.clone(),
        STEP,
    );
    // The call and its charge run in their own task (booked whatever happens
    // here); the job waits for it at most the time limit.
    let mut handle = match call.start(extract::SYSTEM, &user_msg, Some(&schema), MAX_TOKENS) {
        Ok(c) => tokio::spawn(c.wait()),
        Err(e) => {
            let _ = spend::release(pool, user_id, request_id).await;
            let o = if e == StepError::OverBudget {
                Outcome::Fail(reason::COST)
            } else {
                Outcome::Retry(reason::API)
            };
            return (o, 0);
        }
    };
    let reply = match tokio::time::timeout(TIME_LIMIT, &mut handle).await {
        Ok(Ok(r)) => Ok(r),
        Ok(Err(_)) => Ok(Err(StepError::Unrecorded)),
        Err(_) => Err(()),
    };
    // The attempt ends here either way: release the reservation except what
    // a call still in flight holds (its late charge releases the rest).
    let keep = budget.finish();
    if spend::release_keeping(pool, user_id, request_id, keep)
        .await
        .is_err()
    {
        tracing::error!(%run_id, "graph reservation could not be released");
    }
    let cost = budget.spent();
    let reply = match reply {
        Err(()) => {
            // Timed out: the call still finishes and books its charge in its
            // own task; the job's cost is then brought up to date.
            let (pool, budget) = (pool.clone(), budget.clone());
            tokio::spawn(async move {
                let _ = handle.await;
                let late = budget.spent() - cost;
                if late > 0 && add_cost(&pool, run_id, late).await.is_err() {
                    tracing::error!(%run_id, "graph late cost could not be stored");
                }
            });
            return (Outcome::Retry(reason::TIME), cost);
        }
        Ok(Err(StepError::OverBudget)) => return (Outcome::Fail(reason::COST), cost),
        Ok(Err(StepError::Unrecorded)) => return (Outcome::Fail(reason::UNRECORDED), cost),
        Ok(Err(e)) => {
            // A refusal arrives here too (charged).
            tracing::warn!(%run_id, error_kind = e.kind(), "graph call failed");
            return (Outcome::Retry(reason::API), cost);
        }
        Ok(Ok(r)) => r,
    };
    tracing::info!(
        %run_id, input_tokens = reply.input_tokens, output_tokens = reply.output_tokens,
        stop_reason = ?reply.stop_reason, "graph call done"
    );
    if reply.truncated() {
        return (Outcome::Retry(reason::TRUNCATED), cost);
    }
    let raw: RawPicks = match read_structured(&reply) {
        Ok(r) => r,
        Err(fault) => {
            tracing::warn!(%run_id, fault = fault.category, "graph reply unusable");
            return (Outcome::Retry(reason::UNUSABLE), cost);
        }
    };
    let picks = extract::check(raw, &a);
    if !extract::is_usable(&picks, &a) {
        return (Outcome::Retry(reason::EMPTY_PICKS), cost);
    }
    match store::write_run(pool, run_id, &picks, &papers).await {
        Ok(()) => (
            Outcome::Done {
                dropped: picks.dropped,
            },
            cost,
        ),
        Err(sqlx::Error::RowNotFound) => (Outcome::Skip(reason::GONE), cost),
        Err(sqlx::Error::Database(d)) if d.code().as_deref() == Some("23503") => {
            (Outcome::Skip(reason::GONE), cost)
        }
        Err(_) => (Outcome::Retry(reason::STORE), cost),
    }
}
