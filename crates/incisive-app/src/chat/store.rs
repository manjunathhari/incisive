//! Database access for the chat loop: records waiting for passages, the
//! abstracts to copy, the passage insert, vector fills and stale turns.
//! Nothing here logs or returns stored text in an error.

use serde_json::Value;
use sqlx::{PgPool, Row};
use uuid::Uuid;

use super::passages::NewPassage;
use crate::memory::store::vec_literal;

/// A turn left `pending` this long without an update belongs to a dead
/// instance (a live reply is bounded by Gemma's 60 s limit).
pub const STALE_TURN_SECS: i32 = 90;
/// A passage is left alone after this many failures of its own.
pub const MAX_EMBED_ATTEMPTS: i32 = 5;
/// The partial unique index behind "one reply in flight per user".
const ONE_PENDING_INDEX: &str = "chat_turns_one_pending_idx";

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

// ---------- the ask flow ----------

/// What decides a run's chat state, for its own user only.
#[derive(Debug, Clone)]
pub struct RunFacts {
    pub mode: String,
    pub opinion: Option<String>,
    pub memory_job: Option<String>,
    pub record: Option<RecordRef>,
    pub prep: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecordRef {
    pub id: i64,
    pub topic_id: i64,
}

/// `None` when the run does not exist or is someone else's.
pub async fn run_facts(
    pool: &PgPool,
    user_id: i64,
    run_id: Uuid,
) -> sqlx::Result<Option<RunFacts>> {
    let row = sqlx::query(
        "SELECT r.mode, \
                (SELECT t.status FROM incisive.run_temporal t WHERE t.run_id = r.id) AS opinion, \
                (SELECT j.status FROM incisive.memory_jobs j WHERE j.run_id = r.id) AS memory_job, \
                e.id AS record_id, e.topic_id, \
                (SELECT p.status FROM incisive.chat_prep p WHERE p.record_id = e.id) AS prep \
         FROM incisive.runs r \
         LEFT JOIN incisive.evidence_records e ON e.source_run_id = r.id \
         WHERE r.id = $1 AND r.user_id = $2",
    )
    .bind(run_id)
    .bind(user_id)
    .fetch_optional(pool)
    .await?;
    let Some(r) = row else { return Ok(None) };
    let record_id: Option<i64> = r.try_get("record_id")?;
    let topic_id: Option<i64> = r.try_get("topic_id")?;
    Ok(Some(RunFacts {
        mode: r.try_get("mode")?,
        opinion: r.try_get("opinion")?,
        memory_job: r.try_get("memory_job")?,
        record: record_id
            .zip(topic_id)
            .map(|(id, topic_id)| RecordRef { id, topic_id }),
        prep: r.try_get("prep")?,
    }))
}

/// One stored record's bodies.
pub async fn record_bodies(pool: &PgPool, record_id: i64) -> sqlx::Result<RecordForPrep> {
    let r = sqlx::query(
        "SELECT id, answer, board, era_table FROM incisive.evidence_records WHERE id = $1",
    )
    .bind(record_id)
    .fetch_one(pool)
    .await?;
    Ok(RecordForPrep {
        id: r.try_get("id")?,
        answer: r.try_get("answer")?,
        board: r.try_get("board")?,
        era_table: r.try_get("era_table")?,
    })
}

/// Turns this user has started since midnight IST today.
pub async fn turns_today(pool: &PgPool, user_id: i64) -> sqlx::Result<i64> {
    sqlx::query_scalar(
        "SELECT count(*) FROM incisive.chat_turns \
         WHERE user_id = $1 AND created_at >= \
           (date_trunc('day', now() AT TIME ZONE 'Asia/Kolkata') AT TIME ZONE 'Asia/Kolkata')",
    )
    .bind(user_id)
    .fetch_one(pool)
    .await
}

/// A turn just stored as `pending`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NewTurn {
    pub id: i64,
    pub conversation_id: i64,
    pub seq: i32,
}

/// The fields of a new `pending` turn.
pub struct PendingTurn<'a> {
    pub run_id: Uuid,
    pub user_id: i64,
    pub question: &'a str,
    pub mode: &'static str,
    pub safety_line: bool,
    pub personal: bool,
}

/// Stores a `pending` turn in the user's conversation on the run, making the
/// conversation if this is the first turn. One conversation per run and user:
/// the Clinical / Plain switch changes the turn's mode, not the conversation.
/// `Ok(None)`: the user already has a reply in flight.
pub async fn insert_pending_turn(
    pool: &PgPool,
    t: &PendingTurn<'_>,
) -> sqlx::Result<Option<NewTurn>> {
    let mut tx = pool.begin().await?;
    // Holds two first asks on one run apart, and keeps `seq` from colliding.
    // `NO KEY`: other rows' foreign keys to this run are not blocked.
    sqlx::query("SELECT 1 FROM incisive.runs WHERE id = $1 FOR NO KEY UPDATE")
        .bind(t.run_id)
        .fetch_one(&mut *tx)
        .await?;
    let existing: Option<i64> = sqlx::query_scalar(
        "SELECT id FROM incisive.chat_conversations \
         WHERE run_id = $1 AND user_id = $2 AND NOT is_eval ORDER BY id LIMIT 1",
    )
    .bind(t.run_id)
    .bind(t.user_id)
    .fetch_optional(&mut *tx)
    .await?;
    let conversation_id = match existing {
        Some(id) => id,
        None => {
            sqlx::query_scalar(
                "INSERT INTO incisive.chat_conversations (run_id, user_id, mode) \
                 VALUES ($1, $2, $3) RETURNING id",
            )
            .bind(t.run_id)
            .bind(t.user_id)
            .bind(t.mode)
            .fetch_one(&mut *tx)
            .await?
        }
    };
    let inserted: sqlx::Result<(i64, i32)> = sqlx::query_as(
        "INSERT INTO incisive.chat_turns \
         (conversation_id, user_id, seq, question, mode, status, safety_line, personal) \
         SELECT $1, $2, COALESCE(MAX(seq), 0) + 1, $3, $4, 'pending', $5, $6 \
         FROM incisive.chat_turns WHERE conversation_id = $1 \
         RETURNING id, seq",
    )
    .bind(conversation_id)
    .bind(t.user_id)
    .bind(t.question)
    .bind(t.mode)
    .bind(t.safety_line)
    .bind(t.personal)
    .fetch_one(&mut *tx)
    .await;
    match inserted {
        Ok((id, seq)) => {
            tx.commit().await?;
            Ok(Some(NewTurn {
                id,
                conversation_id,
                seq,
            }))
        }
        Err(sqlx::Error::Database(e)) if e.constraint() == Some(ONE_PENDING_INDEX) => {
            tx.rollback().await?;
            Ok(None)
        }
        Err(e) => Err(e),
    }
}

/// The question of the turn before `seq`, whatever became of it: "that" in a
/// follow-up means the last thing asked. A turn still `pending` is skipped.
pub async fn previous_question(
    pool: &PgPool,
    conversation_id: i64,
    seq: i32,
) -> sqlx::Result<Option<String>> {
    sqlx::query_scalar(
        "SELECT question FROM incisive.chat_turns \
         WHERE conversation_id = $1 AND seq < $2 AND status <> 'pending' \
         ORDER BY seq DESC LIMIT 1",
    )
    .bind(conversation_id)
    .bind(seq)
    .fetch_optional(pool)
    .await
}

/// `(question, reply)` of the last `n` `done` or `partial` turns before `seq`,
/// oldest first.
pub async fn history_turns(
    pool: &PgPool,
    conversation_id: i64,
    seq: i32,
    n: i64,
) -> sqlx::Result<Vec<(String, String)>> {
    let mut rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT question, reply FROM incisive.chat_turns \
         WHERE conversation_id = $1 AND seq < $2 AND status IN ('done', 'partial') \
         ORDER BY seq DESC LIMIT $3",
    )
    .bind(conversation_id)
    .bind(seq)
    .bind(n)
    .fetch_all(pool)
    .await?;
    rows.reverse();
    Ok(rows)
}

/// Every write below is guarded by `status = 'pending'`: the stale sweep may
/// already have finished the turn, and its verdict stands. `Ok(false)` means
/// nothing was written (finished, or the run has gone).
async fn pending_write(
    q: sqlx::query::Query<'_, sqlx::Postgres, sqlx::postgres::PgArguments>,
    pool: &PgPool,
) -> sqlx::Result<bool> {
    Ok(q.execute(pool).await?.rows_affected() > 0)
}

/// Records what the reply may cite, in citation order: position `i` is `[i+1]`.
pub async fn set_turn_sources(pool: &PgPool, id: i64, sources: &[i64]) -> sqlx::Result<bool> {
    pending_write(
        sqlx::query(
            "UPDATE incisive.chat_turns SET passage_ids = $2, updated_at = now() \
             WHERE id = $1 AND status = 'pending'",
        )
        .bind(id)
        .bind(sources),
        pool,
    )
    .await
}

/// The reply so far; keeps `updated_at` current for the stale sweep.
pub async fn set_turn_reply(pool: &PgPool, id: i64, reply: &str) -> sqlx::Result<bool> {
    pending_write(
        sqlx::query(
            "UPDATE incisive.chat_turns SET reply = $2, updated_at = now() \
             WHERE id = $1 AND status = 'pending'",
        )
        .bind(id)
        .bind(reply),
        pool,
    )
    .await
}

/// Finishes a turn with its final text.
pub async fn finish_turn(
    pool: &PgPool,
    id: i64,
    status: &'static str,
    reply: &str,
    checks: Option<&Value>,
    latency_ms: i32,
) -> sqlx::Result<bool> {
    pending_write(
        sqlx::query(
            "UPDATE incisive.chat_turns \
             SET status = $2, reply = $3, checks = $4, latency_ms = $5, updated_at = now() \
             WHERE id = $1 AND status = 'pending'",
        )
        .bind(id)
        .bind(status)
        .bind(reply)
        .bind(checks)
        .bind(latency_ms),
        pool,
    )
    .await
}

/// Finishes a turn `unavailable`, leaving what was stored in place.
pub async fn fail_turn(pool: &PgPool, id: i64) -> sqlx::Result<bool> {
    pending_write(
        sqlx::query(
            "UPDATE incisive.chat_turns SET status = 'unavailable', updated_at = now() \
             WHERE id = $1 AND status = 'pending'",
        )
        .bind(id),
        pool,
    )
    .await
}

/// A stored turn as read for its own user.
#[derive(Debug, Clone)]
pub struct TurnRow {
    pub id: i64,
    pub seq: i32,
    pub question: String,
    pub reply: String,
    pub status: String,
    pub mode: String,
    pub passage_ids: Vec<i64>,
    pub checks: Option<Value>,
    pub safety_line: bool,
    pub personal: bool,
    pub feedback: Option<i16>,
    pub suggested: bool,
    pub latency_ms: Option<i32>,
}

const TURN_COLS: &str = "t.id, t.seq, t.question, t.reply, t.status, t.mode, t.passage_ids, \
                         t.checks, t.safety_line, t.personal, t.feedback, t.suggested, t.latency_ms";

fn turn_row(r: &sqlx::postgres::PgRow) -> sqlx::Result<TurnRow> {
    Ok(TurnRow {
        id: r.try_get("id")?,
        seq: r.try_get("seq")?,
        question: r.try_get("question")?,
        reply: r.try_get("reply")?,
        status: r.try_get("status")?,
        mode: r.try_get("mode")?,
        passage_ids: r.try_get("passage_ids")?,
        checks: r.try_get("checks")?,
        safety_line: r.try_get("safety_line")?,
        personal: r.try_get("personal")?,
        feedback: r.try_get("feedback")?,
        suggested: r.try_get("suggested")?,
        latency_ms: r.try_get("latency_ms")?,
    })
}

/// The turn, only if it is this user's.
pub async fn turn_for_user(pool: &PgPool, user_id: i64, id: i64) -> sqlx::Result<Option<TurnRow>> {
    let row = sqlx::query(&format!(
        "SELECT {TURN_COLS} FROM incisive.chat_turns t WHERE t.id = $1 AND t.user_id = $2"
    ))
    .bind(id)
    .bind(user_id)
    .fetch_optional(pool)
    .await?;
    row.as_ref().map(turn_row).transpose()
}

/// The user's conversation on the run and its turns in order.
pub async fn conversation_for_run(
    pool: &PgPool,
    user_id: i64,
    run_id: Uuid,
) -> sqlx::Result<(Option<i64>, Vec<TurnRow>)> {
    let conv: Option<i64> = sqlx::query_scalar(
        "SELECT id FROM incisive.chat_conversations \
         WHERE run_id = $1 AND user_id = $2 AND NOT is_eval ORDER BY id LIMIT 1",
    )
    .bind(run_id)
    .bind(user_id)
    .fetch_optional(pool)
    .await?;
    let Some(c) = conv else {
        return Ok((None, Vec::new()));
    };
    let rows = sqlx::query(&format!(
        "SELECT {TURN_COLS} FROM incisive.chat_turns t \
         WHERE t.conversation_id = $1 AND t.user_id = $2 ORDER BY t.seq"
    ))
    .bind(c)
    .bind(user_id)
    .fetch_all(pool)
    .await?;
    let turns = rows.iter().map(turn_row).collect::<sqlx::Result<_>>()?;
    Ok((Some(c), turns))
}
