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
    let candidates = store::topic_candidates(pool, &fts, None, 10).await?;
    let cand = matcher::Candidate {
        pairs: &ex.pairs,
        embedding: None,
    };
    let matched = matcher::best(&cand, &candidates)
        .filter(|(_, score)| *score >= state.memory.topic_match)
        .map(|(id, _)| id);
    let matched = match matched {
        Some(id) => Some(store::resolve_topic(pool, id).await?),
        None => None,
    };

    match write_record(
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
    .await
    {
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
    }
}
