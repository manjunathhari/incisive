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

/// Move a run to `status`. `Running` stamps `started_at` (once); terminal
/// statuses stamp `finished_at`. `error_kind` is stored as given.
pub async fn set_status(
    pool: &PgPool,
    run_id: Uuid,
    status: RunStatus,
    error_kind: Option<&str>,
) -> sqlx::Result<()> {
    sqlx::query(
        "UPDATE incisive.runs SET status = $2, error_kind = $3, \
           started_at = CASE WHEN $2 = 'running' THEN COALESCE(started_at, now()) ELSE started_at END, \
           finished_at = CASE WHEN $2 IN ('queued','running') THEN NULL ELSE now() END \
         WHERE id = $1",
    )
    .bind(run_id)
    .bind(status.as_str())
    .bind(error_kind)
    .execute(pool)
    .await?;
    Ok(())
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

/// Store (or replace) the structured answer.
pub async fn save_answer(
    pool: &PgPool,
    run_id: Uuid,
    body: &serde_json::Value,
) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO incisive.run_answers (run_id, body) VALUES ($1, $2) \
         ON CONFLICT (run_id) DO UPDATE SET body = EXCLUDED.body",
    )
    .bind(run_id)
    .bind(body)
    .execute(pool)
    .await?;
    Ok(())
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

/// Startup recovery: every queued or running run (its worker died with the
/// process) becomes `interrupted`. Returns `(run_id, user_id)` of each so the
/// caller can release their reservations.
pub async fn mark_stale_runs_interrupted(pool: &PgPool) -> sqlx::Result<Vec<(Uuid, i64)>> {
    sqlx::query_as(
        "UPDATE incisive.runs SET status = 'interrupted', finished_at = now() \
         WHERE status IN ('queued','running') RETURNING id, user_id",
    )
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
