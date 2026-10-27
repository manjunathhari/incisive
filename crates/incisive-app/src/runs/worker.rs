//! Background execution of runs, cancellation and restart recovery. Server only.
//!
//! [`WorkerHandle::spawn`] starts a tokio task for one run; it is detached from
//! the request that created the run, so closing the page does not stop it.
//! The task:
//! 1. loads the run and its user and moves it to `running` (a run that is
//!    already finished, e.g. interrupted by recovery, is left alone);
//! 2. runs the pipeline, catching panics (`Failed("panic")`, payload never
//!    read) and enforcing the hard stop (`Failed("hard_stop")`);
//! 3. saves the answer and sets the final status;
//! 4. releases what is left of the reservation (charges stand);
//! 5. audits `run_finished` with `{run_id, status, cost_paise, steps}`.
//!
//! A run whose row disappears mid-run (the user deleted it), or whose
//! [`CancelFlag`] is set, is treated as cancelled: no answer is stored, the
//! charges stand, the reservation is released and the audit status is
//! `deleted`.
//!
//! Logs carry run ids and fixed kinds only, never the question or answer.

use std::collections::HashMap;
use std::panic::AssertUnwindSafe;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use futures_util::FutureExt;
use sqlx::PgPool;
use uuid::Uuid;

use super::budget::{HARD_STOP_AFTER, RunBudget};
use super::{RunStatus, store};
use crate::agent::{self, PipelineFuture, PipelineOutcome, RunContext};
use crate::audit::{self, Event};
use crate::identity;
use crate::server::AppState;
use crate::spend;

/// A pipeline implementation (the real one, or a test double).
pub type PipelineFn = for<'a> fn(&'a RunContext) -> PipelineFuture<'a>;

fn default_pipeline(ctx: &RunContext) -> PipelineFuture<'_> {
    Box::pin(agent::run_pipeline(ctx))
}

/// Set when a run should stop (the user deleted it). The pipeline checks it
/// between steps; the worker checks it when the pipeline returns.
#[derive(Debug, Clone, Default)]
pub struct CancelFlag(Arc<AtomicBool>);

impl CancelFlag {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}

/// Starts run tasks and tracks them, so a run can be cancelled. Cheap to clone;
/// clones share one registry.
#[derive(Clone, Default)]
pub struct WorkerHandle {
    active: Arc<Mutex<HashMap<Uuid, CancelFlag>>>,
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
        self.spawn_pipeline(state, run_id, default_pipeline);
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
            execute(&state, run_id, cancel, pipeline).await;
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
                Ok(Some(user_id)) => finish(pool, run_id, user_id, end, 0).await,
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
            finish(pool, run_id, user_id, end, 0).await;
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
            finish(pool, run_id, user_id, end, 0).await;
            return;
        }
        Err(_) => {
            tracing::error!(%run_id, "run could not be started");
            let end = End::Status(RunStatus::Failed, Some("store"));
            finish(pool, run_id, user_id, end, 0).await;
            return;
        }
    }

    let ctx = RunContext {
        state: state.clone(),
        run_id,
        user,
        question,
        mode,
        settings: state.runs.for_mode(mode),
        budget: RunBudget::new(
            run_id,
            user_id,
            limit_paise,
            Instant::now(),
            state.prices.clone(),
            state.usd_inr,
        ),
        cancel,
    };
    let guarded = AssertUnwindSafe(async { pipeline(&ctx).await }).catch_unwind();
    let outcome = match tokio::time::timeout(HARD_STOP_AFTER, guarded).await {
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
    finish(pool, run_id, user_id, end, ctx.budget.spent()).await;
}

/// Turn the pipeline's outcome into how the run ends, storing the answer.
async fn decide(pool: &PgPool, ctx: &RunContext, outcome: PipelineOutcome) -> End {
    let run_id = ctx.run_id;
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
        Ok(()) => End::Status(status, None),
        Err(_) if gone(pool, run_id).await => End::Deleted,
        Err(_) => {
            tracing::error!(%run_id, "run answer could not be stored");
            End::Status(RunStatus::Failed, Some("store"))
        }
    }
}

/// Store the final status, release the reservation's remainder and audit.
async fn finish(pool: &PgPool, run_id: Uuid, user_id: i64, end: End, cost_paise: i64) {
    let audited: Option<&'static str> = match end {
        End::Status(status, kind) => match store::set_status(pool, run_id, status, kind).await {
            Ok(true) => Some(status.as_str()),
            Ok(false) if gone(pool, run_id).await => Some("deleted"),
            // Finished by someone else (restart recovery), who audits it.
            Ok(false) => None,
            Err(_) => {
                tracing::error!(%run_id, "run status could not be stored");
                Some(RunStatus::Failed.as_str())
            }
        },
        End::Deleted => Some("deleted"),
        End::AlreadyFinished => None,
    };
    // The charges stand; only the unused remainder is released.
    if spend::release(pool, user_id, run_id).await.is_err() {
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

/// Startup recovery. Every run left queued or running by an earlier process
/// (created before `created_before`, this process's start) becomes
/// `interrupted`: its charges stand, its reservation is released and
/// `run_finished {status: interrupted}` is audited. Then any finished run that
/// still holds a reservation (a release that failed earlier) is released.
/// Returns how many runs were interrupted.
pub async fn recover_stale_runs(
    pool: &PgPool,
    created_before: DateTime<Utc>,
) -> sqlx::Result<usize> {
    let marked = store::mark_stale_runs_interrupted(pool, created_before).await?;
    for &(run_id, user_id) in &marked {
        if spend::release(pool, user_id, run_id).await.is_err() {
            // The sweep below (or the next attempt) picks it up.
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
    for (run_id, user_id) in store::finished_runs_holding_reservation(pool).await? {
        spend::release(pool, user_id, run_id).await?;
    }
    Ok(marked.len())
}

/// [`recover_stale_runs`] with exponential backoff (`base`, doubling, capped
/// at `cap`) until it succeeds.
pub async fn recover_until_done(
    pool: PgPool,
    created_before: DateTime<Utc>,
    base: Duration,
    cap: Duration,
) {
    let mut delay = base;
    let mut attempt: u32 = 1;
    loop {
        match recover_stale_runs(&pool, created_before).await {
            Ok(n) => {
                tracing::info!(attempt, interrupted = n, "run recovery done");
                return;
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
}
