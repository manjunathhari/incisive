//! Postgres store for runs, their progress lines and answers. Server only.
//!
//! Every read or delete that takes a `user_id` is scoped to that user, so
//! another user's run id behaves exactly like a missing one.

use sqlx::{PgPool, Row, postgres::PgRow};
use uuid::Uuid;

use super::{RunMode, RunStatus, RunSummary, RunView, StepKind, StepLine};

/// Name of the partial unique index that allows one queued/running run per user.
const ACTIVE_INDEX: &str = "runs_one_active_per_user";

/// The user already has a queued or running run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ActiveRunExists;

fn decode_err(what: &str, v: &str) -> sqlx::Error {
    sqlx::Error::Decode(format!("unexpected {what} in database: {v}").into())
}

fn mode_of(s: &str) -> sqlx::Result<RunMode> {
    RunMode::parse(s).ok_or_else(|| decode_err("run mode", s))
}

fn summary_of(r: &PgRow) -> sqlx::Result<RunSummary> {
    let mode: String = r.try_get("mode")?;
    let status: String = r.try_get("status")?;
    Ok(RunSummary {
        id: r.try_get("id")?,
        mode: mode_of(&mode)?,
        status: RunStatus::parse(&status).ok_or_else(|| decode_err("run status", &status))?,
        created_at: r.try_get("created_at")?,
        cost_paise: r.try_get("cost_paise")?,
        title: r.try_get("title")?,
    })
}

// `left()` counts characters, not bytes.
const SUMMARY_COLS: &str = "id, mode, status, created_at, cost_paise, left(question, 80) AS title";

/// Insert a queued run. A second active run for the same user returns
/// `Ok(Err(ActiveRunExists))`; any other failure is a real error.
pub async fn create_run(
    pool: &PgPool,
    user_id: i64,
    question: &str,
    mode: RunMode,
    limit_paise: i64,
) -> sqlx::Result<Result<Uuid, ActiveRunExists>> {
    let id = Uuid::new_v4();
    let res = sqlx::query(
        "INSERT INTO incisive.runs (id, user_id, question, mode, limit_paise) \
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(id)
    .bind(user_id)
    .bind(question)
    .bind(mode.as_str())
    .bind(limit_paise)
    .execute(pool)
    .await;
    match res {
        Ok(_) => Ok(Ok(id)),
        Err(sqlx::Error::Database(e))
            if e.code().as_deref() == Some("23505") && e.constraint() == Some(ACTIVE_INDEX) =>
        {
            Ok(Err(ActiveRunExists))
        }
        Err(e) => Err(e),
    }
}

/// Move a run to `status`. `Running` stamps `started_at` (once) and the first
/// heartbeat; terminal statuses stamp `finished_at`. `error_kind` is stored as given.
///
/// A run already in a terminal status (done, partial, failed, interrupted) is
/// never changed. Returns whether a row was updated.
#[must_use = "false means the run was already finished and was not changed"]
pub async fn set_status(
    pool: &PgPool,
    run_id: Uuid,
    status: RunStatus,
    error_kind: Option<&str>,
) -> sqlx::Result<bool> {
    let r = sqlx::query(
        "UPDATE incisive.runs SET status = $2, error_kind = $3, \
           started_at = CASE WHEN $2 = 'running' THEN COALESCE(started_at, now()) ELSE started_at END, \
           heartbeat_at = CASE WHEN $2 = 'running' THEN now() ELSE heartbeat_at END, \
           finished_at = CASE WHEN $2 IN ('queued','running') THEN NULL ELSE now() END \
         WHERE id = $1 AND status NOT IN ('done','partial','failed','interrupted')",
    )
    .bind(run_id)
    .bind(status.as_str())
    .bind(error_kind)
    .execute(pool)
    .await?;
    Ok(r.rows_affected() > 0)
}

/// Append a progress line and return its sequence number (1, 2, 3, ...).
/// A run has a single writer (its worker); the primary key rejects a duplicate.
pub async fn add_step(
    pool: &PgPool,
    run_id: Uuid,
    kind: StepKind,
    line: &str,
) -> sqlx::Result<i32> {
    sqlx::query_scalar(
        "INSERT INTO incisive.run_steps (run_id, seq, kind, line) \
         SELECT $1, COALESCE(MAX(seq), 0) + 1, $2, $3 FROM incisive.run_steps WHERE run_id = $1 \
         RETURNING seq",
    )
    .bind(run_id)
    .bind(kind.as_str())
    .bind(line)
    .fetch_one(pool)
    .await
}

/// Add real spend to the run's running cost.
pub async fn add_cost(pool: &PgPool, run_id: Uuid, paise: i64) -> sqlx::Result<()> {
    sqlx::query("UPDATE incisive.runs SET cost_paise = cost_paise + $2 WHERE id = $1")
        .bind(run_id)
        .bind(paise)
        .execute(pool)
        .await?;
    Ok(())
}

/// Store (or replace) the structured answer, only while the run is `running`.
/// Returns false (nothing stored) if the run is gone or no longer running,
/// e.g. interrupted by recovery or deleted by the user.
pub async fn save_answer(
    pool: &PgPool,
    run_id: Uuid,
    body: &serde_json::Value,
) -> sqlx::Result<bool> {
    let r = sqlx::query(
        "INSERT INTO incisive.run_answers (run_id, body) \
         SELECT id, $2 FROM incisive.runs WHERE id = $1 AND status = 'running' \
         ON CONFLICT (run_id) DO UPDATE SET body = EXCLUDED.body",
    )
    .bind(run_id)
    .bind(body)
    .execute(pool)
    .await?;
    Ok(r.rows_affected() > 0)
}

/// Refresh the run's heartbeat (database clock). False if the run is gone or
/// no longer running: its worker must stop.
pub async fn heartbeat(pool: &PgPool, run_id: Uuid) -> sqlx::Result<bool> {
    let r = sqlx::query(
        "UPDATE incisive.runs SET heartbeat_at = now() WHERE id = $1 AND status = 'running'",
    )
    .bind(run_id)
    .execute(pool)
    .await?;
    Ok(r.rows_affected() > 0)
}

/// The run with its steps and answer, only if it belongs to `user_id`.
pub async fn get_run_for_user(
    pool: &PgPool,
    user_id: i64,
    run_id: Uuid,
) -> sqlx::Result<Option<RunView>> {
    let row = sqlx::query(&format!(
        "SELECT {SUMMARY_COLS} FROM incisive.runs WHERE id = $1 AND user_id = $2"
    ))
    .bind(run_id)
    .bind(user_id)
    .fetch_optional(pool)
    .await?;
    let Some(row) = row else { return Ok(None) };
    let summary = summary_of(&row)?;

    let steps = sqlx::query(
        "SELECT seq, at, kind, line FROM incisive.run_steps WHERE run_id = $1 ORDER BY seq",
    )
    .bind(run_id)
    .fetch_all(pool)
    .await?
    .iter()
    .map(|r| {
        let kind: String = r.try_get("kind")?;
        Ok(StepLine {
            seq: r.try_get("seq")?,
            at: r.try_get("at")?,
            kind: StepKind::parse(&kind).ok_or_else(|| decode_err("step kind", &kind))?,
            line: r.try_get("line")?,
        })
    })
    .collect::<sqlx::Result<Vec<_>>>()?;

    let answer = sqlx::query_scalar("SELECT body FROM incisive.run_answers WHERE run_id = $1")
        .bind(run_id)
        .fetch_optional(pool)
        .await?;
    Ok(Some(RunView {
        summary,
        steps,
        answer,
    }))
}

/// The user's most recent runs, newest first.
pub async fn list_runs(pool: &PgPool, user_id: i64, limit: i64) -> sqlx::Result<Vec<RunSummary>> {
    sqlx::query(&format!(
        "SELECT {SUMMARY_COLS} FROM incisive.runs WHERE user_id = $1 \
         ORDER BY created_at DESC, id LIMIT $2"
    ))
    .bind(user_id)
    .bind(limit)
    .fetch_all(pool)
    .await?
    .iter()
    .map(summary_of)
    .collect()
}

/// Delete one of the user's runs (steps and answer cascade). False if it is
/// not theirs or does not exist.
pub async fn delete_run(pool: &PgPool, user_id: i64, run_id: Uuid) -> sqlx::Result<bool> {
    let r = sqlx::query("DELETE FROM incisive.runs WHERE id = $1 AND user_id = $2")
        .bind(run_id)
        .bind(user_id)
        .execute(pool)
        .await?;
    Ok(r.rows_affected() > 0)
}

/// Delete all of the user's runs; returns how many.
pub async fn delete_all_runs(pool: &PgPool, user_id: i64) -> sqlx::Result<u64> {
    let r = sqlx::query("DELETE FROM incisive.runs WHERE user_id = $1")
        .bind(user_id)
        .execute(pool)
        .await?;
    Ok(r.rows_affected())
}

/// What the worker needs to execute a run: `(user_id, question, mode, limit_paise)`.
pub async fn load_question(
    pool: &PgPool,
    run_id: Uuid,
) -> sqlx::Result<Option<(i64, String, RunMode, i64)>> {
    let row =
        sqlx::query("SELECT user_id, question, mode, limit_paise FROM incisive.runs WHERE id = $1")
            .bind(run_id)
            .fetch_optional(pool)
            .await?;
    row.map(|r| {
        let mode: String = r.try_get("mode")?;
        Ok((
            r.try_get("user_id")?,
            r.try_get("question")?,
            mode_of(&mode)?,
            r.try_get("limit_paise")?,
        ))
    })
    .transpose()
}

/// A run stops being live after this long without a heartbeat (or, if
/// queued, without being picked up).
pub const STALE_AFTER_SECS: i32 = 90;

/// Mark every abandoned run `interrupted` (`error_kind = 'interrupted'`), by
/// the database clock: `running` with a heartbeat older than 90 s (or none),
/// or `queued` for over 90 s with no heartbeat. Live runs, including another
/// instance's, keep fresh heartbeats and are left alone. Returns
/// `(run_id, user_id)` of each so the caller can release and audit them.
/// Used by startup recovery and by the self-heal in `run_ask`.
pub async fn interrupt_stale_runs(pool: &PgPool) -> sqlx::Result<Vec<(Uuid, i64)>> {
    sqlx::query_as(
        "UPDATE incisive.runs SET status = 'interrupted', error_kind = 'interrupted', \
           finished_at = now() \
         WHERE (status = 'running' \
                AND COALESCE(heartbeat_at, started_at, created_at) \
                    < now() - make_interval(secs => $1)) \
            OR (status = 'queued' AND heartbeat_at IS NULL \
                AND created_at < now() - make_interval(secs => $1)) \
         RETURNING id, user_id",
    )
    .bind(f64::from(STALE_AFTER_SECS))
    .fetch_all(pool)
    .await
}

/// Whether the run row still exists (the user may delete it while it runs).
pub async fn run_exists(pool: &PgPool, run_id: Uuid) -> sqlx::Result<bool> {
    sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM incisive.runs WHERE id = $1)")
        .bind(run_id)
        .fetch_one(pool)
        .await
}

/// How many progress lines the run has (0 once it is deleted).
pub async fn count_steps(pool: &PgPool, run_id: Uuid) -> sqlx::Result<i64> {
    sqlx::query_scalar("SELECT count(*) FROM incisive.run_steps WHERE run_id = $1")
        .bind(run_id)
        .fetch_one(pool)
        .await
}

/// Minutes after a run finishes before recovery may release a reservation it
/// still holds (a Claude call can take up to a few minutes with retries).
pub const LATE_CHARGE_GRACE_MIN: i32 = 10;

/// The run's recorded cost in paise, if the run exists.
pub async fn run_cost(pool: &PgPool, run_id: Uuid) -> sqlx::Result<Option<i64>> {
    sqlx::query_scalar("SELECT cost_paise FROM incisive.runs WHERE id = $1")
        .bind(run_id)
        .fetch_optional(pool)
        .await
}

/// Finished runs (any terminal status) that still hold a spend reservation,
/// as `(run_id, user_id)`: a release that never happened. Startup recovery
/// releases them.
///
/// A run that finished less than [`LATE_CHARGE_GRACE_MIN`] minutes ago is
/// left alone: its worker may have kept part of the reservation for a Claude
/// call still in flight, which releases it once its late charge is booked
/// (Ruling R-8d).
pub async fn finished_runs_holding_reservation(pool: &PgPool) -> sqlx::Result<Vec<(Uuid, i64)>> {
    sqlx::query_as(
        "SELECT r.id, r.user_id FROM incisive.runs r \
         JOIN incisive.spend s ON s.request_id = r.id AND s.user_id = r.user_id \
           AND s.kind = 'reservation' \
         WHERE r.status IN ('done','partial','failed','interrupted') \
           AND (r.finished_at IS NULL \
                OR r.finished_at < now() - make_interval(mins => $1))",
    )
    .bind(LATE_CHARGE_GRACE_MIN)
    .fetch_all(pool)
    .await
}

/// Run reservations whose run row no longer exists, made more than
/// [`LATE_CHARGE_GRACE_MIN`] minutes ago, as `(run_id, user_id)` (final
/// review M-2). A run's reservation is made only after its row exists, so
/// such a row was deleted (by the user, or by `undo_run`) and its release
/// failed, or a late release after deletion failed. The grace leaves a
/// delete that is still releasing, or a late charge still booking, alone.
pub async fn orphaned_run_reservations(pool: &PgPool) -> sqlx::Result<Vec<(Uuid, i64)>> {
    sqlx::query_as(
        "SELECT s.request_id, s.user_id FROM incisive.spend s \
         WHERE s.kind = 'reservation' AND s.purpose = 'run' \
           AND s.at < now() - make_interval(mins => $1) \
           AND NOT EXISTS (SELECT 1 FROM incisive.runs r WHERE r.id = s.request_id)",
    )
    .bind(LATE_CHARGE_GRACE_MIN)
    .fetch_all(pool)
    .await
}

/// Retention: delete runs created more than `days` days ago (steps and answers cascade).
pub async fn delete_runs_older_than(pool: &PgPool, days: i32) -> sqlx::Result<u64> {
    let r = sqlx::query(
        "DELETE FROM incisive.runs WHERE created_at < now() - make_interval(days => $1)",
    )
    .bind(days)
    .execute(pool)
    .await?;
    Ok(r.rows_affected())
}

/// Ids of the user's queued or running runs (at most one in practice).
pub async fn active_run_ids(pool: &PgPool, user_id: i64) -> sqlx::Result<Vec<Uuid>> {
    sqlx::query_scalar(
        "SELECT id FROM incisive.runs WHERE user_id = $1 AND status IN ('queued','running')",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await
}

/// Delete all of the user's runs and return `(id, was_active)` for each, where
/// `was_active` means queued or running at the moment of deletion (such a run
/// may still hold a spend reservation).
pub async fn delete_all_runs_returning(
    pool: &PgPool,
    user_id: i64,
) -> sqlx::Result<Vec<(Uuid, bool)>> {
    sqlx::query_as(
        "DELETE FROM incisive.runs WHERE user_id = $1 \
         RETURNING id, status IN ('queued','running')",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await
}
