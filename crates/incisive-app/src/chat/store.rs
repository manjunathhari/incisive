//! Database access for the chat loop: records waiting for passages, the
//! abstracts to copy, the passage insert, vector fills and stale turns.
//! Nothing here logs or returns stored text in an error.

use serde_json::Value;
use sqlx::{PgPool, Row};

use super::passages::NewPassage;
use crate::memory::store::vec_literal;

/// A turn left `pending` this long without an update belongs to a dead
/// instance (a live reply is bounded by Gemma's 60 s limit).
pub const STALE_TURN_SECS: i32 = 90;
/// A passage is left alone after this many failures of its own.
pub const MAX_EMBED_ATTEMPTS: i32 = 5;

/// A stored record that has no `chat_prep` row yet.
#[derive(Debug, Clone)]
pub struct RecordForPrep {
    pub id: i64,
    pub answer: Value,
    pub board: Value,
    pub era_table: Value,
}

/// The oldest `n` records with no prep row (done or failed).
pub async fn records_needing_prep(pool: &PgPool, n: i64) -> sqlx::Result<Vec<RecordForPrep>> {
    let rows = sqlx::query(
        "SELECT r.id, r.answer, r.board, r.era_table FROM incisive.evidence_records r \
         WHERE NOT EXISTS (SELECT 1 FROM incisive.chat_prep p WHERE p.record_id = r.id) \
         ORDER BY r.id LIMIT $1",
    )
    .bind(n)
    .fetch_all(pool)
    .await?;
    rows.into_iter()
        .map(|r| {
            Ok(RecordForPrep {
                id: r.try_get("id")?,
                answer: r.try_get("answer")?,
                board: r.try_get("board")?,
                era_table: r.try_get("era_table")?,
            })
        })
        .collect()
}

/// `(pmid, abstract)` for each PMID that has one, read straight from
/// `paper_cache`: `lit::cache::get_many` hides anything over 30 days old,
/// and the cache is pruned at 90, but a passage must outlive both.
pub async fn read_abstracts(
    pool: &PgPool,
    pmids: &[String],
) -> sqlx::Result<Vec<(String, String)>> {
    if pmids.is_empty() {
        return Ok(Vec::new());
    }
    sqlx::query_as(
        "SELECT pmid, abstract FROM incisive.paper_cache \
         WHERE pmid = ANY($1) AND abstract IS NOT NULL AND abstract <> ''",
    )
    .bind(pmids)
    .fetch_all(pool)
    .await
}

/// Stores a record's passages and its `ok` prep row in one transaction.
/// `Ok(false)` when another tick prepared the record first (the prep row's
/// primary key settles it; nothing is written twice).
pub async fn prep_record(
    pool: &PgPool,
    record_id: i64,
    passages: &[NewPassage],
) -> sqlx::Result<bool> {
    let mut tx = pool.begin().await?;
    let claimed = sqlx::query(
        "INSERT INTO incisive.chat_prep (record_id, status, passages) VALUES ($1, 'ok', $2) \
         ON CONFLICT (record_id) DO NOTHING",
    )
    .bind(record_id)
    .bind(passages.len() as i32)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    if claimed == 0 {
        tx.rollback().await?;
        return Ok(false);
    }
    for p in passages {
        sqlx::query(
            "INSERT INTO incisive.chat_passages \
             (record_id, kind, seq, ref_ns, pmid, comparisons, text) \
             VALUES ($1, $2, $3, $4, $5, $6, $7) \
             ON CONFLICT (record_id, kind, seq) DO NOTHING",
        )
        .bind(record_id)
        .bind(p.kind.as_str())
        .bind(p.seq)
        .bind(&p.ref_ns)
        .bind(&p.pmid)
        .bind(&p.comparisons)
        .bind(&p.text)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(true)
}

/// Marks a record that cannot be prepared. `code` is a fixed code.
pub async fn fail_record(pool: &PgPool, record_id: i64, code: &'static str) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO incisive.chat_prep (record_id, status, passages, error_kind) \
         VALUES ($1, 'failed', 0, $2) ON CONFLICT (record_id) DO NOTHING",
    )
    .bind(record_id)
    .bind(code)
    .execute(pool)
    .await?;
    Ok(())
}

/// Passages with no vector (fewer than five failed attempts): `(id, text)`.
pub async fn passages_missing_vector(pool: &PgPool, n: i64) -> sqlx::Result<Vec<(i64, String)>> {
    sqlx::query_as(
        "SELECT id, text FROM incisive.chat_passages \
         WHERE embedding IS NULL AND embed_attempts < $2 ORDER BY id LIMIT $1",
    )
    .bind(n)
    .bind(MAX_EMBED_ATTEMPTS)
    .fetch_all(pool)
    .await
}

pub async fn set_passage_vector(
    pool: &PgPool,
    id: i64,
    v: &[f32],
    model: &str,
) -> sqlx::Result<()> {
    sqlx::query(
        "UPDATE incisive.chat_passages SET embedding = $2::vector, embed_model = $3 WHERE id = $1",
    )
    .bind(id)
    .bind(vec_literal(v))
    .bind(model)
    .execute(pool)
    .await?;
    Ok(())
}

/// Counts one failure that was the passage's own (bad body, wrong size, 4xx).
pub async fn note_passage_attempt(pool: &PgPool, id: i64) -> sqlx::Result<()> {
    sqlx::query(
        "UPDATE incisive.chat_passages SET embed_attempts = embed_attempts + 1 WHERE id = $1",
    )
    .bind(id)
    .execute(pool)
    .await?;
    Ok(())
}

/// Resolves turns left `pending` past [`STALE_TURN_SECS`] (their instance
/// died): text so far becomes `partial`, none becomes `unavailable`. This
/// frees the user's one-pending slot. Returns how many were resolved.
pub async fn resolve_stale_turns(pool: &PgPool) -> sqlx::Result<u64> {
    let r = sqlx::query(
        "UPDATE incisive.chat_turns \
         SET status = CASE WHEN reply <> '' THEN 'partial' ELSE 'unavailable' END, \
             updated_at = now() \
         WHERE status = 'pending' AND updated_at < now() - make_interval(secs => $1)",
    )
    .bind(f64::from(STALE_TURN_SECS))
    .execute(pool)
    .await?;
    Ok(r.rows_affected())
}

/// Replies in flight across all instances.
pub async fn pending_turns(pool: &PgPool) -> sqlx::Result<i64> {
    sqlx::query_scalar("SELECT count(*) FROM incisive.chat_turns WHERE status = 'pending'")
        .fetch_one(pool)
        .await
}
