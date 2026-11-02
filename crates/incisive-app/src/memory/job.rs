//! The post-opinion memory job: turns a finished Full run (answer, era
//! table, expert board) into a permanent evidence record under a topic.
//! Errors carry fixed codes only; nothing from the answer or question is
//! ever logged or stored as an error.

use std::panic::AssertUnwindSafe;
use std::time::Duration;

use chrono::{DateTime, FixedOffset, NaiveDate, Utc};
use futures_util::FutureExt;
use serde_json::Value;
use sqlx::{PgPool, Row};
use uuid::Uuid;

use crate::memory::{extract, matcher, store};
use crate::runs::RunMode;
use crate::server::AppState;

#[derive(Debug, Clone, PartialEq)]
pub enum JobOutcome {
    Done {
        record_id: i64,
        topic_id: i64,
        new_topic: bool,
    },
    Skipped(&'static str),
    Failed(&'static str),
}

/// Called when a run's Expert Opinion finishes `done`: queue a memory job
/// when memory is on. Errors are logged by code only.
pub async fn on_temporal_done(state: &AppState, run_id: Uuid) {
    if !state.memory.on {
        return;
    }
    if store::enqueue_job(&state.pool, run_id).await.is_err() {
        tracing::warn!(code = "enqueue", "memory job not queued");
    }
}

/// A foreign-key violation means the run (or its row) is gone.
fn classify(e: &sqlx::Error) -> &'static str {
    match e {
        sqlx::Error::Database(d) if d.code().as_deref() == Some("23503") => "run_gone",
        _ => "store",
    }
}

/// IST is UTC+05:30.
fn ist_date(t: DateTime<Utc>) -> NaiveDate {
    let ist = FixedOffset::east_opt(19800).expect("valid offset");
    t.with_timezone(&ist).date_naive()
}

async fn existing_record(pool: &PgPool, run_id: Uuid) -> sqlx::Result<Option<(i64, i64)>> {
    let r = sqlx::query(
        "SELECT id, topic_id FROM incisive.evidence_records WHERE source_run_id = $1 \
         ORDER BY id LIMIT 1",
    )
    .bind(run_id)
    .fetch_optional(pool)
    .await?;
    r.map(|r| Ok((r.try_get("id")?, r.try_get("topic_id")?)))
        .transpose()
}

async fn run_created_at(pool: &PgPool, run_id: Uuid) -> sqlx::Result<Option<DateTime<Utc>>> {
    sqlx::query_scalar("SELECT created_at FROM incisive.runs WHERE id = $1")
        .bind(run_id)
        .fetch_optional(pool)
        .await
}

async fn answer_row(pool: &PgPool, run_id: Uuid) -> sqlx::Result<Option<(Value, i16)>> {
    let r = sqlx::query("SELECT body, answer_version FROM incisive.run_answers WHERE run_id = $1")
        .bind(run_id)
        .fetch_optional(pool)
        .await?;
    r.map(|r| Ok((r.try_get("body")?, r.try_get("answer_version")?)))
        .transpose()
}

/// (status, table_json, board_json) of the run's expert opinion.
async fn temporal_row(
    pool: &PgPool,
    run_id: Uuid,
) -> sqlx::Result<Option<(String, Option<Value>, Option<Value>)>> {
    let r = sqlx::query(
        "SELECT status, table_json, board_json FROM incisive.run_temporal WHERE run_id = $1",
    )
    .bind(run_id)
    .fetch_optional(pool)
    .await?;
    r.map(|r| {
        Ok((
            r.try_get("status")?,
            r.try_get("table_json")?,
            r.try_get("board_json")?,
        ))
    })
    .transpose()
}

pub async fn process(state: &AppState, run_id: Uuid) -> JobOutcome {
    match process_inner(state, run_id).await {
        Ok(o) => o,
        Err(e) => JobOutcome::Failed(classify(&e)),
    }
}

async fn process_inner(state: &AppState, run_id: Uuid) -> sqlx::Result<JobOutcome> {
    let pool = &state.pool;
    // Idempotent: a run already recorded is not recorded again.
    if let Some((record_id, topic_id)) = existing_record(pool, run_id).await? {
        return Ok(JobOutcome::Done {
            record_id,
            topic_id,
            new_topic: false,
        });
    }
    let Some((_, question, mode, _)) = crate::runs::store::load_question(pool, run_id).await?
    else {
        return Ok(JobOutcome::Failed("run_gone"));
    };
    if mode != RunMode::Focused {
        return Ok(JobOutcome::Skipped("not_full"));
    }
    let Some(created_at) = run_created_at(pool, run_id).await? else {
        return Ok(JobOutcome::Failed("run_gone"));
    };
    let Some((status, table, board)) = temporal_row(pool, run_id).await? else {
        return Ok(JobOutcome::Skipped("no_opinion"));
    };
    if status != "done" {
        return Ok(JobOutcome::Skipped("no_opinion"));
    }
    let Some((body, answer_version)) = answer_row(pool, run_id).await? else {
        return Ok(JobOutcome::Skipped(extract::ExtractError::NoAnswer.code()));
    };
    let (table, board) = (table.unwrap_or(Value::Null), board.unwrap_or(Value::Null));
    let ex = match extract::extract(&body, &table, &board) {
        Ok(x) => x,
        Err(e) => return Ok(JobOutcome::Skipped(e.code())),
    };

    let mut fts = ex.topic_label.clone();
    for (a, b) in &ex.pairs {
        fts.push(' ');
        fts.push_str(a);
        fts.push(' ');
        fts.push_str(b);
    }
    // Best effort: any failure means FTS-only matching and a NULL embedding
    // that the sweep fills later.
    let emb = embed_best_effort(state, &ex.embed_text).await;
    let candidates = store::topic_candidates(pool, &fts, emb.as_deref(), 10).await?;
    let cand = matcher::Candidate {
        pairs: &ex.pairs,
        embedding: emb.as_deref(),
    };
    let matched = matcher::best(&cand, &candidates)
        .filter(|(_, score)| *score >= state.memory.topic_match)
        .map(|(id, _)| id);
    let matched = match matched {
        Some(id) => Some(store::resolve_topic(pool, id).await?),
        None => None,
    };

    let written = write_record(
        pool,
        run_id,
        &ex,
        matched,
        &question,
        &body,
        answer_version,
        &board,
        &table,
        created_at,
    )
    .await;
    if let (
        Ok(JobOutcome::Done {
            record_id,
            topic_id,
            new_topic,
        }),
        Some(v),
        Some(e),
    ) = (&written, &emb, &state.embedder)
    {
        let model = e.model();
        if store::set_record_embedding(pool, *record_id, v, model)
            .await
            .is_err()
        {
            tracing::warn!(code = "embed_store", "record embedding not stored");
        }
        if *new_topic
            && store::set_topic_embedding(pool, *topic_id, v, model)
                .await
                .is_err()
        {
            tracing::warn!(code = "embed_store", "topic embedding not stored");
        }
    }
    match written {
        Err(sqlx::Error::Database(d)) if d.code().as_deref() == Some("23505") => {
            // Another job recorded this run first; join its record.
            match existing_record(pool, run_id).await? {
                Some((record_id, topic_id)) => Ok(JobOutcome::Done {
                    record_id,
                    topic_id,
                    new_topic: false,
                }),
                None => Ok(JobOutcome::Failed("store")),
            }
        }
        other => other,
    }
}

const EMBED_TIMEOUT: Duration = Duration::from_secs(5);
/// Total time one tick's record sweep may take.
const SWEEP_BUDGET: Duration = Duration::from_secs(30);

async fn embed_best_effort(state: &AppState, text: &str) -> Option<Vec<f32>> {
    let e = state.embedder.as_ref()?;
    match e.embed(text, EMBED_TIMEOUT).await {
        Ok(v) => Some(v),
        Err(err) => {
            tracing::warn!(code = err.code(), "embedding failed; matching without it");
            None
        }
    }
}

/// Fills up to 20 records that have no embedding. Stops when the server is
/// unavailable (timeout, 401, connection error, 5xx); skips a record whose own
/// failure is not that (bad body, wrong size, other 4xx) and goes on. Returns how many were filled.
pub async fn sweep_embeddings(state: &AppState) -> usize {
    let Some(e) = state.embedder.as_ref() else {
        return 0;
    };
    let Ok(rows) = store::records_missing_embedding(&state.pool, 20).await else {
        tracing::warn!(code = "sweep_query", "embedding sweep query failed");
        return 0;
    };
    let started = std::time::Instant::now();
    let mut filled = 0;
    for (id, text) in rows {
        if started.elapsed() > SWEEP_BUDGET {
            break;
        }
        match e.embed(&text, EMBED_TIMEOUT).await {
            Ok(v) => {
                if store::set_record_embedding(&state.pool, id, &v, e.model())
                    .await
                    .is_err()
                {
                    tracing::warn!(code = "embed_store", "record embedding not stored");
                    break;
                }
                filled += 1;
            }
            Err(err) if err.is_server_unavailable() => {
                tracing::warn!(code = err.code(), "embedding sweep stopped");
                break;
            }
            Err(err) => {
                // A poison record must not block the ones after it.
                tracing::warn!(code = err.code(), "record skipped by embedding sweep");
                if store::note_embed_attempt(&state.pool, id).await.is_err() {
                    tracing::warn!(code = "embed_attempt", "embedding attempt not noted");
                }
            }
        }
    }
    filled
}

/// `write_record` with its error classified as `process` does (a foreign-key
/// violation is `Failed("run_gone")`). Exposed so tests can reach the
/// in-transaction path directly, e.g. with a run deleted after the job
/// loaded its data.
#[doc(hidden)]
#[allow(clippy::too_many_arguments)]
pub async fn write_record_classified(
    pool: &PgPool,
    run_id: Uuid,
    ex: &extract::Extracted,
    matched: Option<i64>,
    question: &str,
    body: &Value,
    answer_version: i16,
    board: &Value,
    table: &Value,
    created_at: DateTime<Utc>,
) -> JobOutcome {
    match write_record(
        pool,
        run_id,
        ex,
        matched,
        question,
        body,
        answer_version,
        board,
        table,
        created_at,
    )
    .await
    {
        Ok(o) => o,
        Err(e) => JobOutcome::Failed(classify(&e)),
    }
}

#[allow(clippy::too_many_arguments)]
async fn write_record(
    pool: &PgPool,
    run_id: Uuid,
    ex: &extract::Extracted,
    matched: Option<i64>,
    question: &str,
    body: &Value,
    answer_version: i16,
    board: &Value,
    table: &Value,
    created_at: DateTime<Utc>,
) -> sqlx::Result<JobOutcome> {
    let mut tx = pool.begin().await?;
    let (topic_id, new_topic) = match matched {
        Some(id) => (id, false),
        None => (
            store::create_topic(&mut tx, &ex.topic_label, &ex.pairs).await?,
            true,
        ),
    };
    let (record_id, _version) = store::insert_record(
        &mut tx,
        &store::NewRecord {
            topic_id,
            source_run_id: run_id,
            question,
            answer: body,
            answer_version,
            board,
            era_table: table,
            evidence_through: ist_date(created_at),
            topic_label: &ex.topic_label,
            fts_extra: &ex.fts_extra,
            embed_text: &ex.embed_text,
        },
    )
    .await?;
    store::upsert_ledger(&mut tx, topic_id, record_id, &ex.studies, &ex.claims).await?;
    tx.commit().await?;
    Ok(JobOutcome::Done {
        record_id,
        topic_id,
        new_topic,
    })
}

/// Only a transient store error is retried; run_gone and panic are terminal.
fn outcome_is_store_failure(code: Option<&'static str>, status: &str) -> bool {
    status == "failed" && code == Some("store")
}

/// Every 30 s: claim up to five jobs, process each, record the outcome.
/// Never exits; a panic in one job is that job's failure.
pub async fn memory_loop(state: AppState) {
    let mut every = tokio::time::interval(Duration::from_secs(30));
    every.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        every.tick().await;
        let claimed = match store::claim_jobs(&state.pool, 5).await {
            Ok(ids) => ids,
            Err(_) => {
                tracing::warn!(code = "claim", "memory claim failed");
                continue;
            }
        };
        for run_id in claimed {
            // The panic payload is dropped unread: it could quote stored text.
            let outcome = match AssertUnwindSafe(process(&state, run_id))
                .catch_unwind()
                .await
            {
                Ok(o) => o,
                Err(_) => {
                    tracing::error!(%run_id, "memory job panicked");
                    JobOutcome::Failed("panic")
                }
            };
            let (status, code, record) = match outcome {
                JobOutcome::Done { record_id, .. } => ("done", None, Some(record_id)),
                JobOutcome::Skipped(c) => ("skipped", Some(c), None),
                JobOutcome::Failed(c) => ("failed", Some(c), None),
            };
            let finished = match outcome_is_store_failure(code, status) {
                true => store::retry_or_fail_job(&state.pool, run_id, "store").await,
                false => store::finish_job(&state.pool, run_id, status, code, record).await,
            };
            if finished.is_err() {
                tracing::warn!(code = "finish", "memory job not finished");
            }
        }
        sweep_tick(&state).await;
    }
}

/// Fixed key of the session advisory lock that lets one instance at a time
/// run the record sweep and topic backfill (ASCII "INCMEM01").
pub const SWEEP_LOCK_KEY: i64 = 0x494E_434D_454D_3031;

/// A session advisory lock held on one dedicated pooled connection. The guard
/// owns the connection for its whole life: unless `release` unlocks cleanly,
/// dropping it (a panic, a cancelled future, a failed unlock) detaches the
/// connection, so one that may still hold the lock never goes back to the pool.
pub(crate) struct SweepLock {
    conn: Option<sqlx::pool::PoolConnection<sqlx::Postgres>>,
    key: i64,
}

impl SweepLock {
    /// `Ok(Some)` when the lock was taken, `Ok(None)` when another session
    /// holds it, `Err(code)` when the connection or the lock query failed.
    pub(crate) async fn try_acquire(pool: &PgPool, key: i64) -> Result<Option<Self>, &'static str> {
        let conn = pool.acquire().await.map_err(|_| "sweep_conn")?;
        // The guard owns the connection before the query runs, so a future
        // cancelled mid-query detaches it instead of pooling it.
        let mut guard = SweepLock {
            conn: Some(conn),
            key,
        };
        let conn = guard.conn.as_mut().ok_or("sweep_conn")?;
        match sqlx::query_scalar::<_, bool>("SELECT pg_try_advisory_lock($1)")
            .bind(key)
            .fetch_one(&mut **conn)
            .await
        {
            Ok(true) => Ok(Some(guard)),
            Ok(false) => {
                // Not ours: the connection holds nothing and may be pooled.
                guard.conn.take();
                Ok(None)
            }
            Err(_) => Err("sweep_lock"),
        }
    }

    /// The connection, for work that must run on the lock's session.
    #[allow(dead_code)]
    pub(crate) fn conn(&mut self) -> Option<&mut sqlx::PgConnection> {
        self.conn.as_deref_mut()
    }

    /// Unlocks; the connection is pooled only when Postgres confirms.
    pub(crate) async fn release(mut self) {
        let Some(mut conn) = self.conn.take() else {
            return;
        };
        let unlocked = sqlx::query_scalar::<_, bool>("SELECT pg_advisory_unlock($1)")
            .bind(self.key)
            .fetch_one(&mut *conn)
            .await;
        if matches!(unlocked, Ok(true)) {
            return;
        }
        tracing::warn!(code = "sweep_unlock", "sweep unlock failed");
        let _ = conn.detach();
    }
}

impl Drop for SweepLock {
    fn drop(&mut self) {
        if let Some(conn) = self.conn.take() {
            let _ = conn.detach();
        }
    }
}

/// The record sweep and topic backfill, under a Postgres advisory lock held
/// on one dedicated pooled connection. Returns false when another instance
/// holds the lock (this tick's sweep is skipped).
pub async fn sweep_tick(state: &AppState) -> bool {
    let lock = match SweepLock::try_acquire(&state.pool, SWEEP_LOCK_KEY).await {
        Ok(Some(l)) => l,
        Ok(None) => return false,
        Err(code) => {
            tracing::warn!(code, "sweep lock not taken");
            return false;
        }
    };
    sweep_embeddings(state).await;
    if store::backfill_topic_embeddings(&state.pool).await.is_err() {
        tracing::warn!(code = "topic_embed", "topic embedding backfill failed");
    }
    lock.release().await;
    true
}

/// Queue every existing Full run with a `done` Expert Opinion that has no
/// memory job yet. Returns how many jobs were queued.
pub async fn backfill(pool: &PgPool) -> sqlx::Result<u64> {
    let r = sqlx::query(
        "INSERT INTO incisive.memory_jobs (run_id) \
         SELECT r.id FROM incisive.runs r \
         JOIN incisive.run_temporal t ON t.run_id = r.id \
         WHERE r.mode = 'focused' AND t.status = 'done' \
           AND NOT EXISTS (SELECT 1 FROM incisive.memory_jobs j WHERE j.run_id = r.id) \
         ON CONFLICT DO NOTHING",
    )
    .execute(pool)
    .await?;
    Ok(r.rows_affected())
}

#[cfg(test)]
mod sweep_lock_tests {
    use super::*;
    use sqlx::Connection;

    #[tokio::test]
    async fn sweep_lock_released_when_guard_dropped_unreleased() {
        let Ok(url) = std::env::var("TEST_DATABASE_URL") else {
            return;
        };
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect(&url)
            .await
            .unwrap();
        let key = 0x5445_5354_4C4B_0001_i64;
        let guard = SweepLock::try_acquire(&pool, key)
            .await
            .unwrap()
            .expect("lock free");
        // A second attempt sees it held.
        assert!(SweepLock::try_acquire(&pool, key).await.unwrap().is_none());
        // Dropped mid-way (as on panic or cancellation): never released cleanly.
        drop(guard);
        let mut other = sqlx::PgConnection::connect(&url).await.unwrap();
        let mut got = false;
        for _ in 0..20 {
            got = sqlx::query_scalar("SELECT pg_try_advisory_lock($1)")
                .bind(key)
                .fetch_one(&mut other)
                .await
                .unwrap();
            if got {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        assert!(got, "lock still held after guard drop");
        // The pool is not poisoned: a clean acquire/release cycle works.
        sqlx::query("SELECT pg_advisory_unlock($1)")
            .bind(key)
            .execute(&mut other)
            .await
            .unwrap();
        let g = SweepLock::try_acquire(&pool, key).await.unwrap().unwrap();
        g.release().await;
        assert!(SweepLock::try_acquire(&pool, key).await.unwrap().is_some());
    }
}
