//! Background execution of runs, cancellation and restart recovery. Server only.
//!
//! [`WorkerHandle::spawn`] starts a tokio task for one run; it is detached from
//! the request that created the run, so closing the page does not stop it.
//! The task:
//! 1. loads the run and its user and moves it to `running`, which stamps the
//!    first heartbeat (a run that is already finished, e.g. interrupted by
//!    recovery, is left alone);
//! 2. runs the pipeline while a ticker refreshes the heartbeat every
//!    [`HEARTBEAT_EVERY`] (the pipeline also beats between steps), enforcing
//!    the hard stop (`Failed("hard_stop")`);
//! 3. saves the answer (only while still `running`) and sets the final
//!    status, retrying that write once; if it still fails the audit status is
//!    `store_error` and self-heal / recovery clear the row later;
//! 4. releases what is left of the reservation (charges stand);
//! 5. audits `run_finished` with `{run_id, status, cost_paise, steps}`.
//!
//! A panic anywhere in the task ends the run as `failed`/`panic` with its
//! reservation released; the payload is never read.
//!
//! A run whose row disappears mid-run (the user deleted it), or whose
//! [`CancelFlag`] is cancelled, ends as `deleted`: no answer is stored, the
//! charges stand and the reservation is released. A run found interrupted by
//! someone else (a heartbeat or the answer write affects no row) stops
//! quietly: charges stand, the reservation is released, and no second
//! `run_finished` is audited.
//!
//! Logs carry run ids and fixed kinds only, never the question or answer.

use std::collections::HashMap;
use std::panic::AssertUnwindSafe;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use futures_util::FutureExt;
use sqlx::PgPool;
use uuid::Uuid;

use super::budget::RunBudget;
use super::{RunStatus, store};
use crate::agent::{self, PipelineFuture, PipelineOutcome, RunContext};
use crate::audit::{self, Event};
use crate::identity;
use crate::server::AppState;
use crate::spend;

/// A pipeline implementation (the real one, or a test double).
pub type PipelineFn = for<'a> fn(&'a RunContext) -> PipelineFuture<'a>;

fn default_pipeline(ctx: &RunContext) -> PipelineFuture<'_> {
    agent::run_engine(ctx)
}

/// Refresh a live run's heartbeat at least this often (recovery treats 90 s
/// without one as abandoned).
pub const HEARTBEAT_EVERY: Duration = Duration::from_secs(20);

const LIVE: u8 = 0;
const CANCELLED: u8 = 1;
const LOST: u8 = 2;

/// Set when a run should stop: the user deleted it ([`CancelFlag::cancel`]),
/// or a heartbeat found it no longer running (interrupted elsewhere). The
/// pipeline checks [`CancelFlag::is_cancelled`] between steps; the worker
/// checks why when the pipeline returns.
#[derive(Debug, Clone, Default)]
pub struct CancelFlag(Arc<AtomicU8>);

impl CancelFlag {
    /// The user is deleting the run.
    pub fn cancel(&self) {
        let _ = self
            .0
            .compare_exchange(LIVE, CANCELLED, Ordering::AcqRel, Ordering::Acquire);
    }
    /// True once the run should stop, for either reason.
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire) != LIVE
    }
    fn mark_lost(&self) {
        let _ = self
            .0
            .compare_exchange(LIVE, LOST, Ordering::AcqRel, Ordering::Acquire);
    }
    fn is_lost(&self) -> bool {
        self.0.load(Ordering::Acquire) == LOST
    }
}

/// Refresh the run's heartbeat. False when the run is gone or no longer
/// running (interrupted or deleted): the pipeline must stop, and the flag is
/// set so the worker knows why. A database error counts as alive (the ticker
/// retries); a truly dead run is caught by the next beat or the final writes.
pub async fn heartbeat(ctx: &RunContext) -> bool {
    beat(&ctx.state.pool, ctx.run_id, &ctx.cancel).await
}

async fn beat(pool: &PgPool, run_id: Uuid, cancel: &CancelFlag) -> bool {
    match store::heartbeat(pool, run_id).await {
        Ok(true) => true,
        Ok(false) => {
            cancel.mark_lost();
            false
        }
        Err(_) => {
            tracing::warn!(%run_id, "run heartbeat could not be stored");
            true
        }
    }
}

/// Aborts the heartbeat ticker when the run ends, however it ends.
struct Ticker(tokio::task::JoinHandle<()>);

impl Drop for Ticker {
    fn drop(&mut self) {
        self.0.abort();
    }
}

fn start_ticker(pool: PgPool, run_id: Uuid, cancel: CancelFlag) -> Ticker {
    Ticker(tokio::spawn(async move {
        loop {
            tokio::time::sleep(HEARTBEAT_EVERY).await;
            if !beat(&pool, run_id, &cancel).await {
                return;
            }
        }
    }))
}

/// Starts run tasks and tracks them, so a run can be cancelled. Cheap to clone;
/// clones share one registry.
#[derive(Clone)]
pub struct WorkerHandle {
    active: Arc<Mutex<HashMap<Uuid, CancelFlag>>>,
    /// The pipeline [`WorkerHandle::spawn`] runs: the real one, except in
    /// tests ([`WorkerHandle::with_pipeline`]).
    pipeline: PipelineFn,
}

impl Default for WorkerHandle {
    fn default() -> Self {
        WorkerHandle {
            active: Arc::default(),
            pipeline: default_pipeline,
        }
    }
}

/// Removes the run from the registry when its task ends, however it ends.
struct Tracked {
    handle: WorkerHandle,
    run_id: Uuid,
}

impl Drop for Tracked {
    fn drop(&mut self) {
        self.handle.active().remove(&self.run_id);
    }
}

impl WorkerHandle {
    fn active(&self) -> MutexGuard<'_, HashMap<Uuid, CancelFlag>> {
        self.active.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Execute `run_id` in a background tokio task.
    pub fn spawn(&self, state: AppState, run_id: Uuid) {
        self.spawn_pipeline(state, run_id, self.pipeline);
    }

    /// Tests only: a handle whose [`WorkerHandle::spawn`] (and so every run
    /// `run_ask` starts) uses `pipeline`.
    #[cfg(feature = "test-util")]
    pub fn with_pipeline(pipeline: PipelineFn) -> WorkerHandle {
        WorkerHandle {
            active: Arc::default(),
            pipeline,
        }
    }

    /// Tests only: execute `run_id` with a substitute pipeline.
    #[cfg(feature = "test-util")]
    pub fn spawn_with(&self, state: AppState, run_id: Uuid, pipeline: PipelineFn) {
        self.spawn_pipeline(state, run_id, pipeline);
    }

    fn spawn_pipeline(&self, state: AppState, run_id: Uuid, pipeline: PipelineFn) {
        let cancel = CancelFlag::default();
        self.active().insert(run_id, cancel.clone());
        let tracked = Tracked {
            handle: self.clone(),
            run_id,
        };
        tokio::spawn(async move {
            let _tracked = tracked;
            let run = AssertUnwindSafe(execute(&state, run_id, cancel, pipeline));
            // The payload is dropped unread: it could quote question text.
            if run.catch_unwind().await.is_err() {
                tracing::error!(%run_id, "run worker panicked");
                after_panic(&state.pool, run_id).await;
            }
        });
    }

    /// Ask the run's task to stop. Returns whether a task for it was running.
    pub fn cancel(&self, run_id: Uuid) -> bool {
        match self.active().get(&run_id) {
            Some(flag) => {
                flag.cancel();
                true
            }
            None => false,
        }
    }

    /// Whether a task for this run is still working (including its release
    /// and audit at the end).
    pub fn is_running(&self, run_id: Uuid) -> bool {
        self.active().contains_key(&run_id)
    }
}

/// How the worker ends a run.
enum End {
    /// Store this status (and error kind), then audit it.
    Status(RunStatus, Option<&'static str>),
    /// Deleted or cancelled by the user: nothing more is written to the run.
    Deleted,
    /// Already finished by someone else (restart recovery): release only.
    AlreadyFinished,
}

/// A panic outside the pipeline (or before its own boundary): fail the run,
/// release its reservation and audit, as far as the database allows.
///
/// The whole reservation is released (`keep = 0`): the run's budget is out
/// of reach here, so what in-flight Claude calls still hold is unknown. Their
/// late charges then book with no headroom left for them, a small cap
/// overshoot bounded by those calls' holds. This needs a panic outside
/// `catch_unwind` while calls are in flight (a double fault): rare, accepted.
async fn after_panic(pool: &PgPool, run_id: Uuid) {
    let user_id = match store::load_question(pool, run_id).await {
        Ok(Some((user_id, ..))) => Some(user_id),
        _ => spend::reservation_owner(pool, run_id).await.ok().flatten(),
    };
    let Some(user_id) = user_id else { return };
    let cost = store::run_cost(pool, run_id)
        .await
        .ok()
        .flatten()
        .unwrap_or(0);
    let end = End::Status(RunStatus::Failed, Some("panic"));
    // A second panic here is caught by tokio; the run is then left for
    // self-heal / recovery.
    finish(pool, run_id, user_id, end, cost, 0).await;
}

/// True only when the run row is known to be gone. A failed check counts as
/// "still there", so a database hiccup is never mistaken for a deletion.
async fn gone(pool: &PgPool, run_id: Uuid) -> bool {
    matches!(store::run_exists(pool, run_id).await, Ok(false))
}

async fn execute(state: &AppState, run_id: Uuid, cancel: CancelFlag, pipeline: PipelineFn) {
    let pool = &state.pool;
    let (user_id, question, mode, limit_paise) = match store::load_question(pool, run_id).await {
        Ok(Some(r)) => r,
        other => {
            // Deleted before it started, or unreadable: release whatever was
            // reserved for it (found through the reservation itself).
            let end = match other {
                Ok(_) => End::Deleted,
                Err(_) => {
                    tracing::error!(%run_id, "run could not be loaded");
                    End::Status(RunStatus::Failed, Some("store"))
                }
            };
            match spend::reservation_owner(pool, run_id).await {
                Ok(Some(user_id)) => finish(pool, run_id, user_id, end, 0, 0).await,
                Ok(None) => {}
                Err(_) => tracing::error!(%run_id, "run reservation owner could not be read"),
            }
            return;
        }
    };
    let user = match identity::find_user_by_id(pool, user_id).await {
        Ok(Some(u)) => u,
        _ => {
            tracing::error!(%run_id, "run user could not be loaded");
            let end = End::Status(RunStatus::Failed, Some("store"));
            finish(pool, run_id, user_id, end, 0, 0).await;
            return;
        }
    };
    match store::set_status(pool, run_id, RunStatus::Running, None).await {
        Ok(true) => {}
        Ok(false) => {
            // Already finished (e.g. interrupted by recovery) or deleted.
            let end = if gone(pool, run_id).await {
                End::Deleted
            } else {
                End::AlreadyFinished
            };
            finish(pool, run_id, user_id, end, 0, 0).await;
            return;
        }
        Err(_) => {
            tracing::error!(%run_id, "run could not be started");
            let end = End::Status(RunStatus::Failed, Some("store"));
            finish(pool, run_id, user_id, end, 0, 0).await;
            return;
        }
    }

    // No Claude client (dev without ANTHROPIC_API_KEY): fail closed.
    let Some(llm) = state.llm.clone() else {
        tracing::warn!(%run_id, "no Claude client configured; run failed");
        let end = End::Status(RunStatus::Failed, Some("no_api_key"));
        finish(pool, run_id, user_id, end, 0, 0).await;
        return;
    };

    let _ticker = start_ticker(pool.clone(), run_id, cancel.clone());
    let ctx = RunContext {
        state: state.clone(),
        llm,
        lit: state.lit.clone(),
        run_id,
        user,
        question,
        mode,
        settings: state.runs.for_mode(mode),
        budget: Arc::new(
            RunBudget::new(
                run_id,
                user_id,
                limit_paise,
                Instant::now(),
                state.prices.clone(),
                state.usd_inr,
            )
            // Wrap-up and hard stop from the mode's time limit (RUN_LIMITS):
            // `agent_time_limit_s` when the evidence team answers the run
            // (QUICK_ENGINE), `time_limit_s` otherwise.
            .with_time_limit(state.runs.limits(mode, state.quick_engine).1),
        ),
        cancel,
    };
    let guarded = AssertUnwindSafe(async { pipeline(&ctx).await }).catch_unwind();
    let outcome = match tokio::time::timeout(ctx.budget.hard_stop(), guarded).await {
        Ok(Ok(outcome)) => outcome,
        // The payload is dropped unread: it could quote question text.
        Ok(Err(_payload)) => {
            tracing::error!(%run_id, "run pipeline panicked");
            PipelineOutcome::Failed("panic")
        }
        Err(_) => {
            tracing::warn!(%run_id, "run reached the hard stop");
            PipelineOutcome::Failed("hard_stop")
        }
    };
    let end = decide(pool, &ctx, outcome).await;
    // Calls dropped mid-flight (hard stop) still charge from their own task.
    let keep = ctx.budget.finish();
    finish(pool, run_id, user_id, end, ctx.budget.spent(), keep).await;
}

/// Turn the pipeline's outcome into how the run ends, storing the answer.
async fn decide(pool: &PgPool, ctx: &RunContext, outcome: PipelineOutcome) -> End {
    let run_id = ctx.run_id;
    if ctx.cancel.is_lost() {
        // A heartbeat found the run no longer running.
        return if gone(pool, run_id).await {
            End::Deleted
        } else {
            End::AlreadyFinished
        };
    }
    if ctx.cancel.is_cancelled() {
        // The user is deleting this run. If the row is still there, close it
        // so it no longer holds the user's active slot. Changed or not, the
        // run ends as deleted.
        if store::set_status(pool, run_id, RunStatus::Failed, Some("cancelled"))
            .await
            .is_err()
        {
            tracing::warn!(%run_id, "cancelled run status could not be stored");
        }
        return End::Deleted;
    }
    let (status, body) = match outcome {
        PipelineOutcome::Done(body) => (RunStatus::Done, body),
        PipelineOutcome::Partial(body) => (RunStatus::Partial, body),
        PipelineOutcome::Failed(kind) => {
            return if gone(pool, run_id).await {
                End::Deleted
            } else {
                End::Status(RunStatus::Failed, Some(kind))
            };
        }
    };
    match store::save_answer(pool, run_id, &body).await {
        Ok(true) => End::Status(status, None),
        // Not running any more: deleted, or interrupted by someone else.
        Ok(false) if gone(pool, run_id).await => End::Deleted,
        Ok(false) => End::AlreadyFinished,
        Err(_) if gone(pool, run_id).await => End::Deleted,
        Err(_) => {
            tracing::error!(%run_id, "run answer could not be stored");
            End::Status(RunStatus::Failed, Some("store"))
        }
    }
}

/// Store the final status, release the reservation's remainder and audit.
///
/// `keep` is what calls still in flight hold ([`RunBudget::finish`]): that
/// much of the reservation stays until they settle, and the last of them
/// releases it (Ruling R-8d). The audited `cost_paise` is what was charged by
/// now, so it can lag a late charge; `runs.cost_paise` is authoritative.
async fn finish(pool: &PgPool, run_id: Uuid, user_id: i64, end: End, cost_paise: i64, keep: i64) {
    let audited: Option<&'static str> = match end {
        End::Status(status, kind) => {
            let mut stored = store::set_status(pool, run_id, status, kind).await;
            if stored.is_err() {
                tracing::warn!(%run_id, "run status could not be stored; retrying once");
                stored = store::set_status(pool, run_id, status, kind).await;
            }
            match stored {
                Ok(true) => Some(status.as_str()),
                Ok(false) if gone(pool, run_id).await => Some("deleted"),
                // Finished by someone else (restart recovery), who audits it.
                Ok(false) => None,
                Err(_) => {
                    // The row stays active; once its heartbeat is stale,
                    // self-heal or recovery interrupts it.
                    tracing::error!(%run_id, "run status could not be stored");
                    Some("store_error")
                }
            }
        }
        End::Deleted => Some("deleted"),
        End::AlreadyFinished => None,
    };
    // The charges stand; only the unused remainder is released, less what
    // calls in flight still hold.
    if spend::release_keeping(pool, user_id, run_id, keep)
        .await
        .is_err()
    {
        tracing::error!(%run_id, "run reservation could not be released");
    }
    let Some(status) = audited else { return };
    let steps = store::count_steps(pool, run_id).await.unwrap_or(0);
    if audit::append(
        pool,
        Some(run_id),
        Some(user_id),
        Event::RunFinished,
        audit::run_finished_detail(run_id, status, cost_paise, steps),
    )
    .await
    .is_err()
    {
        tracing::error!(%run_id, "run_finished audit append failed");
    }
}

/// Interrupt every abandoned run ([`store::interrupt_stale_runs`]): release
/// each one's reservation (its charges stand) and audit
/// `run_finished {status: interrupted}`. Returns the runs interrupted.
/// Shared by startup recovery and the self-heal in `run_ask`.
pub async fn interrupt_and_release_stale(pool: &PgPool) -> sqlx::Result<Vec<(Uuid, i64)>> {
    let marked = store::interrupt_stale_runs(pool).await?;
    for &(run_id, user_id) in &marked {
        if spend::release(pool, user_id, run_id).await.is_err() {
            // The leaked-reservation sweep in recovery picks it up.
            tracing::error!(%run_id, "interrupted run's reservation could not be released");
        }
        let cost = store::run_cost(pool, run_id)
            .await
            .ok()
            .flatten()
            .unwrap_or(0);
        let steps = store::count_steps(pool, run_id).await.unwrap_or(0);
        if audit::append(
            pool,
            Some(run_id),
            Some(user_id),
            Event::RunFinished,
            audit::run_finished_detail(run_id, RunStatus::Interrupted.as_str(), cost, steps),
        )
        .await
        .is_err()
        {
            tracing::error!(%run_id, "run_finished audit append failed");
        }
    }
    Ok(marked)
}

/// Recovery: interrupt abandoned runs (see [`interrupt_and_release_stale`]),
/// then release any finished run that still holds a reservation (a release
/// that failed earlier). Safe with several instances: live runs keep fresh
/// heartbeats. Returns how many runs were interrupted.
pub async fn recover_stale_runs(pool: &PgPool) -> sqlx::Result<usize> {
    let marked = interrupt_and_release_stale(pool).await?;
    for (run_id, user_id) in store::finished_runs_holding_reservation(pool).await? {
        spend::release(pool, user_id, run_id).await?;
    }
    // Reservations whose run row is gone (final review M-2): released under
    // the same locks, so they stop counting against the caps.
    for (run_id, user_id) in store::orphaned_run_reservations(pool).await? {
        tracing::info!(%run_id, "orphaned run reservation released");
        spend::release(pool, user_id, run_id).await?;
    }
    Ok(marked.len())
}

/// How often recovery runs after startup, so a run abandoned by a crashed
/// instance does not show as running until the next restart or send.
pub const RECOVERY_EVERY: Duration = Duration::from_secs(60);

/// [`recover_stale_runs`] at startup with exponential backoff (`base`,
/// doubling, capped at `cap`) until it succeeds, then every [`RECOVERY_EVERY`].
/// Never returns.
pub async fn recovery_loop(pool: PgPool, base: Duration, cap: Duration) {
    let mut delay = base;
    let mut attempt: u32 = 1;
    loop {
        match recover_stale_runs(&pool).await {
            Ok(n) => {
                tracing::info!(attempt, interrupted = n, "run recovery done");
                break;
            }
            Err(_) => {
                // The error could carry stored values; the attempt number only.
                tracing::warn!(attempt, "run recovery attempt failed; will retry");
                tokio::time::sleep(delay).await;
                delay = (delay * 2).min(cap);
                attempt = attempt.saturating_add(1);
            }
        }
    }
    loop {
        tokio::time::sleep(RECOVERY_EVERY).await;
        match recover_stale_runs(&pool).await {
            Ok(0) => {}
            Ok(n) => tracing::info!(interrupted = n, "abandoned runs interrupted"),
            Err(_) => tracing::warn!("periodic run recovery failed; will retry"),
        }
    }
}
