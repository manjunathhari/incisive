//! The chat background loop: prepares passages from stored evidence records,
//! fills their vectors and resolves stale pending turns. Errors carry fixed
//! codes only; nothing from an answer, question or passage is ever logged.

use std::panic::AssertUnwindSafe;
use std::time::Duration;

use futures_util::FutureExt;
use serde_json::Value;

use super::{passages, store};
use crate::memory::extract;
use crate::memory::job::SweepLock;
use crate::server::AppState;

/// Fixed key of the chat sweep's advisory lock (ASCII "INCCHT01").
pub const CHAT_SWEEP_LOCK_KEY: i64 = 0x494E_4343_4854_3031;
const EMBED_TIMEOUT: Duration = Duration::from_secs(5);
/// Total time one vector sweep may take.
const SWEEP_BUDGET: Duration = Duration::from_secs(25);
const SWEEP_BATCH: i64 = 40;
const PREP_BATCH: i64 = 5;

/// PMIDs of the answer's references.
fn reference_pmids(answer: &Value) -> Vec<String> {
    answer
        .get("references")
        .and_then(Value::as_array)
        .map(|rs| {
            rs.iter()
                .filter_map(|r| r.get("pmid").and_then(Value::as_str))
                .filter(|p| !p.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// A record's comparison pairs: from its own era table, exactly as the memory
/// layer extracted them (the topic's merged pairs could tag a one-pair record
/// as a many-pair one). `Err` is a fixed code.
pub(crate) fn record_pairs(
    rec: &store::RecordForPrep,
) -> Result<Vec<(String, String)>, &'static str> {
    let ex = extract::extract(&rec.answer, &rec.era_table, &rec.board).map_err(|e| e.code())?;
    Ok(ex.pairs)
}

/// Builds and stores one record's passages. `Err` is a fixed code for a
/// `failed` prep row.
async fn prep_one(state: &AppState, rec: &store::RecordForPrep) -> Result<(), &'static str> {
    let pairs = record_pairs(rec)?;
    let pmids = reference_pmids(&rec.answer);
    let abstracts = store::read_abstracts(&state.pool, &pmids)
        .await
        .map_err(|_| "store")?;
    let built =
        passages::build(&rec.answer, &rec.board, &pairs, &abstracts).map_err(|e| e.code())?;
    // Ok(false): a concurrent tick got there first, which is fine.
    store::prep_record(&state.pool, rec.id, &built)
        .await
        .map_err(|_| "store")?;
    Ok(())
}

/// One pass: resolve stale turns, then prepare up to five records that have
/// no prep row. A bad record gets a `failed` row and never stops the rest.
/// Returns how many records this call prepared or failed.
pub async fn tick(state: &AppState) -> usize {
    if store::resolve_stale_turns(&state.pool).await.is_err() {
        tracing::warn!(code = "stale_turns", "stale turns not resolved");
    }
    let Ok(records) = store::records_needing_prep(&state.pool, PREP_BATCH).await else {
        tracing::warn!(code = "prep_query", "chat prep query failed");
        return 0;
    };
    let mut handled = 0;
    for rec in records {
        // The panic payload is dropped unread: it could quote stored text.
        let result = match AssertUnwindSafe(prep_one(state, &rec)).catch_unwind().await {
            Ok(r) => r,
            Err(_) => {
                tracing::error!(record = rec.id, "chat prep panicked");
                Err("panic")
            }
        };
        match result {
            Ok(()) => handled += 1,
            // A store error is transient: leave the record for the next tick.
            Err("store") => tracing::warn!(code = "store", "chat prep store error"),
            Err(code) => {
                tracing::warn!(code, record = rec.id, "chat record not prepared");
                match store::fail_record(&state.pool, rec.id, code).await {
                    Ok(()) => handled += 1,
                    Err(_) => tracing::warn!(code = "fail_row", "failed prep row not stored"),
                }
            }
        }
    }
    handled
}

/// Fills passage vectors, stopping at the 25 s budget. Does nothing while a
/// user's reply is running (checked before every embed call) or without an
/// embedder. Stops when the server is unavailable; a passage whose own
/// failure it was is counted and skipped. Returns how many were filled.
pub async fn sweep_passage_vectors(state: &AppState) -> usize {
    let Some(e) = state.embedder.as_ref() else {
        return 0;
    };
    let Ok(rows) = store::passages_missing_vector(&state.pool, SWEEP_BATCH).await else {
        tracing::warn!(code = "sweep_query", "passage sweep query failed");
        return 0;
    };
    let started = std::time::Instant::now();
    let mut filled = 0;
    for (id, text) in rows {
        if started.elapsed() > SWEEP_BUDGET {
            break;
        }
        match store::pending_turns(&state.pool).await {
            Ok(0) => {}
            Ok(_) => break,
            Err(_) => {
                tracing::warn!(code = "pending_query", "pending turns not counted");
                break;
            }
        }
        match e.embed(&text, EMBED_TIMEOUT).await {
            Ok(v) => {
                if store::set_passage_vector(&state.pool, id, &v, e.model())
                    .await
                    .is_err()
                {
                    tracing::warn!(code = "embed_store", "passage vector not stored");
                    break;
                }
                filled += 1;
            }
            Err(err) if err.is_server_unavailable() => {
                tracing::warn!(code = err.code(), "passage sweep stopped");
                break;
            }
            Err(err) => {
                tracing::warn!(code = err.code(), "passage skipped by sweep");
                if store::note_passage_attempt(&state.pool, id).await.is_err() {
                    tracing::warn!(code = "embed_attempt", "passage attempt not noted");
                }
            }
        }
    }
    filled
}

/// The vector sweep under its own advisory lock; false when another
/// instance holds it (this tick's sweep is skipped).
pub async fn sweep_tick(state: &AppState) -> bool {
    let lock = match SweepLock::try_acquire(&state.pool, CHAT_SWEEP_LOCK_KEY).await {
        Ok(Some(l)) => l,
        Ok(None) => return false,
        Err(code) => {
            tracing::warn!(code, "chat sweep lock not taken");
            return false;
        }
    };
    sweep_passage_vectors(state).await;
    lock.release().await;
    true
}

/// Every 30 s: `tick`, then the vector sweep. Never exits; a panic in one
/// pass is logged by code and the next pass runs.
pub async fn chat_loop(state: AppState) {
    let mut every = tokio::time::interval(Duration::from_secs(30));
    every.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        every.tick().await;
        let pass = async {
            tick(&state).await;
            sweep_tick(&state).await;
        };
        if AssertUnwindSafe(pass).catch_unwind().await.is_err() {
            tracing::error!("chat loop pass panicked");
        }
    }
}
