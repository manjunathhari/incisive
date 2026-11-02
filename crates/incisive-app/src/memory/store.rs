//! All SQL for the memory tables. Plain `sqlx::query`; JSON and vectors travel
//! as text and are cast in SQL. Nothing here logs question or answer text.

use std::collections::HashSet;

use chrono::NaiveDate;
use serde_json::Value;
use sqlx::{PgConnection, PgPool, Row};
use uuid::Uuid;

use super::extract::{LedgerClaim, LedgerStudy};
use super::matcher::TopicFeatures;

/// pgvector text form: `[0.1,0.2,...]`.
pub fn vec_literal(v: &[f32]) -> String {
    let mut s = String::with_capacity(v.len() * 10 + 2);
    s.push('[');
    for (i, x) in v.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        s.push_str(&x.to_string());
    }
    s.push(']');
    s
}

fn parse_vec(s: &str) -> Option<Vec<f32>> {
    s.trim()
        .trim_start_matches('[')
        .trim_end_matches(']')
        .split(',')
        .map(|x| x.trim().parse::<f32>().ok())
        .collect()
}

fn json_err(e: serde_json::Error) -> sqlx::Error {
    sqlx::Error::Decode(Box::new(e))
}

/// Follow `merged_into` to the live topic. Stops after 16 hops (cycle-safe).
pub async fn resolve_topic(pool: &PgPool, topic_id: i64) -> sqlx::Result<i64> {
    let mut cur = topic_id;
    for _ in 0..16 {
        let next: Option<Option<i64>> =
            sqlx::query_scalar("SELECT merged_into FROM incisive.topics WHERE id = $1")
                .bind(cur)
                .fetch_optional(pool)
                .await?;
        match next {
            Some(Some(n)) => cur = n,
            _ => break,
        }
    }
    Ok(cur)
}

/// Candidate live topics by full-text rank over their records and, when an
/// embedding is given, by vector nearness. `fts_rank` is normalised by the max
/// in the result (all 0 if the max is 0); vector-only topics get 0.
pub async fn topic_candidates(
    pool: &PgPool,
    fts_query: &str,
    embedding: Option<&[f32]>,
    limit: i64,
) -> sqlx::Result<Vec<TopicFeatures>> {
    let mut ranks: Vec<(i64, f32)> = Vec::new();
    if !fts_query.trim().is_empty() {
        let rows = sqlx::query(
            "WITH q AS (SELECT websearch_to_tsquery('english', $1) AS q) \
             SELECT r.topic_id, max(ts_rank_cd(r.fts, q.q))::float4 AS rank \
             FROM incisive.evidence_records r \
             JOIN incisive.topics t ON t.id = r.topic_id AND t.merged_into IS NULL, q \
             WHERE r.fts @@ q.q \
             GROUP BY r.topic_id ORDER BY rank DESC, r.topic_id LIMIT $2",
        )
        .bind(fts_query)
        .bind(limit)
        .fetch_all(pool)
        .await?;
        for r in rows {
            ranks.push((r.try_get("topic_id")?, r.try_get("rank")?));
        }
    }
    let max = ranks.iter().map(|(_, r)| *r).fold(0.0_f32, f32::max);
    for (_, r) in ranks.iter_mut() {
        *r = if max > 0.0 { *r / max } else { 0.0 };
    }
    if let Some(e) = embedding {
        let ids: Vec<i64> = sqlx::query_scalar(
            "SELECT id FROM incisive.topics \
             WHERE merged_into IS NULL AND embedding IS NOT NULL \
             ORDER BY embedding <=> $1::vector, id LIMIT $2",
        )
        .bind(vec_literal(e))
        .bind(limit)
        .fetch_all(pool)
        .await?;
        let seen: HashSet<i64> = ranks.iter().map(|(i, _)| *i).collect();
        for id in ids {
            if !seen.contains(&id) {
                ranks.push((id, 0.0));
            }
        }
    }
    ranks.truncate(limit.max(0) as usize);
    if ranks.is_empty() {
        return Ok(Vec::new());
    }
    let ids: Vec<i64> = ranks.iter().map(|(i, _)| *i).collect();
    let rows = sqlx::query(
        "SELECT id, pairs::text AS pairs, embedding::text AS embedding \
         FROM incisive.topics WHERE id = ANY($1) AND merged_into IS NULL",
    )
    .bind(&ids)
    .fetch_all(pool)
    .await?;
    let mut out = Vec::with_capacity(rows.len());
    for (id, rank) in ranks {
        let Some(row) = rows.iter().find(|r| r.get::<i64, _>("id") == id) else {
            continue;
        };
        let pairs: Vec<(String, String)> =
            serde_json::from_str(&row.get::<String, _>("pairs")).map_err(json_err)?;
        let embedding = row
            .get::<Option<String>, _>("embedding")
            .and_then(|s| parse_vec(&s));
        out.push(TopicFeatures {
            topic_id: id,
            pairs,
            embedding,
            fts_rank: rank,
        });
    }
    Ok(out)
}

pub async fn create_topic(
    tx: &mut PgConnection,
    label: &str,
    pairs: &[(String, String)],
) -> sqlx::Result<i64> {
    let json = serde_json::to_string(pairs).map_err(json_err)?;
    sqlx::query_scalar(
        "INSERT INTO incisive.topics (label, pairs) VALUES ($1, $2::text::jsonb) RETURNING id",
    )
    .bind(label)
    .bind(json)
    .fetch_one(tx)
    .await
}

pub struct NewRecord<'a> {
    pub topic_id: i64,
    pub source_run_id: Uuid,
    pub question: &'a str,
    pub answer: &'a Value,
    pub answer_version: i16,
    pub board: &'a Value,
    pub era_table: &'a Value,
    pub evidence_through: NaiveDate,
    pub topic_label: &'a str,
    pub fts_extra: &'a str,
}

/// Insert a record at version max+1 for its topic; returns (record id, version).
/// The topic row is locked first so concurrent inserts serialise.
pub async fn insert_record(tx: &mut PgConnection, r: &NewRecord<'_>) -> sqlx::Result<(i64, i32)> {
    sqlx::query("SELECT id FROM incisive.topics WHERE id = $1 FOR UPDATE")
        .bind(r.topic_id)
        .fetch_optional(&mut *tx)
        .await?;
    let row = sqlx::query(
        "INSERT INTO incisive.evidence_records \
         (topic_id, version, source_run_id, question, answer, answer_version, board, era_table, \
          evidence_through, topic_label, fts_extra) \
         VALUES ($1, (SELECT COALESCE(MAX(version), 0) + 1 FROM incisive.evidence_records \
                      WHERE topic_id = $1), \
                 $2, $3, $4::text::jsonb, $5, $6::text::jsonb, $7::text::jsonb, $8, $9, $10) \
         RETURNING id, version",
    )
    .bind(r.topic_id)
    .bind(r.source_run_id)
    .bind(r.question)
    .bind(r.answer.to_string())
    .bind(r.answer_version)
    .bind(r.board.to_string())
    .bind(r.era_table.to_string())
    .bind(r.evidence_through)
    .bind(r.topic_label)
    .bind(r.fts_extra)
    .fetch_one(tx)
    .await?;
    Ok((row.try_get("id")?, row.try_get("version")?))
}

/// Upsert studies on (topic, pmid) keeping `first_record_id`; insert claims
/// for this record. Repeated pmids in `studies` keep the first occurrence;
/// every study is stored, with a missing year or era left NULL.
pub async fn upsert_ledger(
    tx: &mut PgConnection,
    topic_id: i64,
    record_id: i64,
    studies: &[LedgerStudy],
    claims: &[LedgerClaim],
) -> sqlx::Result<()> {
    let mut seen = HashSet::new();
    for s in studies {
        if !seen.insert(s.pmid.as_str()) {
            continue;
        }
        sqlx::query(
            "INSERT INTO incisive.ledger_studies \
             (topic_id, pmid, pub_year, treatment_start, treatment_end, years_assumed, era_id, \
              first_record_id, last_record_id) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $8) \
             ON CONFLICT (topic_id, pmid) DO UPDATE SET \
               last_record_id = EXCLUDED.last_record_id, pub_year = EXCLUDED.pub_year, \
               treatment_start = EXCLUDED.treatment_start, \
               treatment_end = EXCLUDED.treatment_end, \
               years_assumed = EXCLUDED.years_assumed, era_id = EXCLUDED.era_id",
        )
        .bind(topic_id)
        .bind(&s.pmid)
        .bind(s.pub_year)
        .bind(s.treatment_start)
        .bind(s.treatment_end)
        .bind(s.years_assumed)
        .bind(s.era_id.as_deref())
        .bind(record_id)
        .execute(&mut *tx)
        .await?;
    }
    for c in claims {
        sqlx::query(
            "INSERT INTO incisive.ledger_claims \
             (record_id, topic_id, outcome, intervention, comparator, era_id, era_start, era_end, \
              estimate, lo, hi, effect_type, n_studies, label) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9::float8::numeric, $10::float8::numeric, \
                     $11::float8::numeric, $12, $13, $14) \
             ON CONFLICT DO NOTHING",
        )
        .bind(record_id)
        .bind(topic_id)
        .bind(&c.outcome)
        .bind(&c.intervention)
        .bind(&c.comparator)
        .bind(&c.era_id)
        .bind(c.era_start)
        .bind(c.era_end)
        .bind(c.estimate)
        .bind(c.lo)
        .bind(c.hi)
        .bind(&c.effect_type)
        .bind(c.n_studies)
        .bind(&c.label)
        .execute(&mut *tx)
        .await?;
    }
    Ok(())
}

pub async fn set_record_embedding(
    pool: &PgPool,
    record_id: i64,
    v: &[f32],
    model: &str,
) -> sqlx::Result<()> {
    sqlx::query(
        "UPDATE incisive.evidence_records \
         SET embedding = $2::vector, embed_model = $3, embedded_at = now() WHERE id = $1",
    )
    .bind(record_id)
    .bind(vec_literal(v))
    .bind(model)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn set_topic_embedding(
    pool: &PgPool,
    topic_id: i64,
    v: &[f32],
    model: &str,
) -> sqlx::Result<()> {
    sqlx::query(
        "UPDATE incisive.topics \
         SET embedding = $2::vector, embed_model = $3, updated_at = now() WHERE id = $1",
    )
    .bind(topic_id)
    .bind(vec_literal(v))
    .bind(model)
    .execute(pool)
    .await?;
    Ok(())
}

/// True if a new job was queued; false if the run already had one.
pub async fn enqueue_job(pool: &PgPool, run_id: Uuid) -> sqlx::Result<bool> {
    let r =
        sqlx::query("INSERT INTO incisive.memory_jobs (run_id) VALUES ($1) ON CONFLICT DO NOTHING")
            .bind(run_id)
            .execute(pool)
            .await?;
    Ok(r.rows_affected() == 1)
}

/// Claim up to `n` jobs (pending, or running and stale for 10 minutes) that
/// have fewer than 3 attempts. Concurrent callers never get the same job.
pub async fn claim_jobs(pool: &PgPool, n: i64) -> sqlx::Result<Vec<Uuid>> {
    // A job stale in `running` with no attempts left is given up, not left stuck.
    sqlx::query(
        "UPDATE incisive.memory_jobs SET status = 'failed', last_error_kind = 'gave_up', \
                updated_at = now() \
         WHERE status = 'running' AND claimed_at < now() - interval '10 minutes' \
           AND attempts >= 3",
    )
    .execute(pool)
    .await?;
    sqlx::query_scalar(
        "UPDATE incisive.memory_jobs SET status = 'running', claimed_at = now(), \
                attempts = attempts + 1, updated_at = now() \
         WHERE run_id IN ( \
             SELECT run_id FROM incisive.memory_jobs \
             WHERE (status = 'pending' \
                    OR (status = 'running' AND claimed_at < now() - interval '10 minutes')) \
               AND attempts < 3 \
             ORDER BY created_at LIMIT $1 FOR UPDATE SKIP LOCKED) \
         RETURNING run_id",
    )
    .bind(n)
    .fetch_all(pool)
    .await
}

pub async fn finish_job(
    pool: &PgPool,
    run_id: Uuid,
    status: &'static str,
    code: Option<&'static str>,
    record_id: Option<i64>,
) -> sqlx::Result<()> {
    sqlx::query(
        "UPDATE incisive.memory_jobs \
         SET status = $2, last_error_kind = $3, record_id = $4, updated_at = now() \
         WHERE run_id = $1",
    )
    .bind(run_id)
    .bind(status)
    .bind(code)
    .bind(record_id)
    .execute(pool)
    .await?;
    Ok(())
}

/// Records with no embedding yet, with the text to embed.
pub async fn records_missing_embedding(pool: &PgPool, n: i64) -> sqlx::Result<Vec<(i64, String)>> {
    sqlx::query_as(
        "SELECT id, topic_label || E'\\n' || fts_extra FROM incisive.evidence_records \
         WHERE embedding IS NULL ORDER BY id LIMIT $1",
    )
    .bind(n)
    .fetch_all(pool)
    .await
}
