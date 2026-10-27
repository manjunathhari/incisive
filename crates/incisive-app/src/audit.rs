//! Tamper-evident audit log: a hash chain in `incisive.audit`.
//!
//! Each entry's `hash` is SHA-256 over its own fields and the previous entry's
//! hash, so editing a row, or deleting a row from the middle or the start, is
//! found by [`verify_chain`].
//!
//! # What this protects, and what it does not
//!
//! This is tamper DETECTION, not prevention. Supabase connects the app as the
//! database owner, so the migration's `REVOKE UPDATE, DELETE ... FROM PUBLIC`
//! does not restrict the app (an owner is not affected by it). The REVOKE stays
//! because it is harmless and documents intent. Prevention would need a separate
//! restricted role for the app; that is a later follow-up.
//!
//! Removing the NEWEST rows (truncating the tail) is NOT detectable by the chain
//! alone: what remains is still a valid chain. A periodic external checkpoint of
//! the latest `(seq, hash)`, stored somewhere the database owner cannot edit,
//! would cover it. That is a follow-up, not built yet.
//!
//! # Privacy
//!
//! The log never holds question text, patient identifiers, or model-generated text
//! about the question. Use the typed detail helpers ([`deid_blocked_detail`], [`gate_detail`],
//! [`accepted_detail`], [`charged_detail`]); they carry only finding kinds, byte
//! offsets, lengths, a SHA-256 of the question, verdicts, models, tokens and paise.
//! The gate's free-text reason is stored only for rule decisions (static text), never
//! for model decisions.
//!
//! # Canonical hashing
//!
//! The hashed byte string is these fields joined by `\x1f`:
//! `prev_hash, seq, at, request_id, user_id, event, detail`.
//! - `at`: RFC 3339 UTC with exactly 6 fractional digits (microseconds). Postgres
//!   stores microseconds, so `append` truncates to microseconds BEFORE hashing and
//!   inserting, and `entry_hash` truncates too; the stored value round-trips exactly.
//! - `request_id`, `user_id`: `None` hashes as the EMPTY STRING; `Some(x)` as its
//!   canonical text (hyphenated lowercase UUID, decimal integer). So `None` and
//!   `Some(0)` differ ("" vs "0").
//! - `event`: its snake_case name.
//! - `detail`: compact JSON with keys sorted at every nesting level. This is done
//!   explicitly here and does not depend on serde_json's map type (JSONB also
//!   reorders keys). Array order is significant. Floats are NOT allowed anywhere
//!   in `detail` (enforced): JSONB stores numbers as numeric, and a float such as
//!   `1e16` can come back as a different number, which would show up as a false
//!   `Broken`. `append` rejects any `f64` number at any depth with
//!   `sqlx::Error::Encode`, before taking the lock. Use integers and strings.
//!
//! Appends serialise on a transaction-scoped advisory lock
//! `pg_advisory_xact_lock(LOCK_NS_AUDIT, 0)` (two-int form, a namespace distinct
//! from the spend locks), so the chain never forks.

use chrono::{DateTime, SecondsFormat, Utc};
use claude_client::Usage;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Row, postgres::PgRow};
use uuid::Uuid;

use crate::deid::Finding;
use crate::gate::GateDecision;
use crate::runs::RunMode;

/// Advisory-lock namespace for audit appends (key 0). Distinct from
/// `spend::LOCK_NS_SPEND_GLOBAL` / `LOCK_NS_SPEND_USER`.
pub const LOCK_NS_AUDIT: i32 = 0x4155_0001;
const LOCK_KEY_AUDIT: i32 = 0;

/// `prev_hash` of the first entry: 64 zeros.
pub const GENESIS_HASH: &str = "0000000000000000000000000000000000000000000000000000000000000000";

const AUDIT_TABLE: &str = "incisive.audit";
const SEP: char = '\x1f';
const VERIFY_BATCH: i64 = 1000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Event {
    IdentityRejected,
    DeidBlocked,
    GateDecided,
    GateFailed,
    CapBlocked,
    Accepted,
    Charged,
    RunStarted,
    RunFinished,
    RunDeleted,
}

impl Event {
    pub fn as_str(self) -> &'static str {
        match self {
            Event::IdentityRejected => "identity_rejected",
            Event::DeidBlocked => "deid_blocked",
            Event::GateDecided => "gate_decided",
            Event::GateFailed => "gate_failed",
            Event::CapBlocked => "cap_blocked",
            Event::Accepted => "accepted",
            Event::Charged => "charged",
            Event::RunStarted => "run_started",
            Event::RunFinished => "run_finished",
            Event::RunDeleted => "run_deleted",
        }
    }

    fn parse(s: &str) -> Option<Event> {
        Some(match s {
            "identity_rejected" => Event::IdentityRejected,
            "deid_blocked" => Event::DeidBlocked,
            "gate_decided" => Event::GateDecided,
            "gate_failed" => Event::GateFailed,
            "cap_blocked" => Event::CapBlocked,
            "accepted" => Event::Accepted,
            "charged" => Event::Charged,
            "run_started" => Event::RunStarted,
            "run_finished" => Event::RunFinished,
            "run_deleted" => Event::RunDeleted,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub seq: i64,
    pub at: DateTime<Utc>,
    pub request_id: Option<Uuid>,
    pub user_id: Option<i64>,
    pub event: Event,
    pub detail: Value,
    pub prev_hash: String,
    pub hash: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChainStatus {
    Valid { entries: i64 },
    Broken { at_seq: i64 },
}

// ---------- canonical hashing ----------

fn truncate_micros(at: DateTime<Utc>) -> DateTime<Utc> {
    DateTime::from_timestamp_micros(at.timestamp_micros()).unwrap_or(at)
}

/// Compact JSON with object keys sorted at every level.
fn write_canonical(v: &Value, out: &mut String) {
    match v {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            out.push('{');
            for (i, k) in keys.into_iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&Value::String(k.clone()).to_string());
                out.push(':');
                write_canonical(&map[k], out);
            }
            out.push('}');
        }
        Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_canonical(item, out);
            }
            out.push(']');
        }
        scalar => out.push_str(&scalar.to_string()),
    }
}

pub fn canonical_detail(detail: &Value) -> String {
    let mut s = String::new();
    write_canonical(detail, &mut s);
    s
}

/// Lowercase hex SHA-256 over the canonical byte string (see module docs).
pub fn entry_hash(
    prev_hash: &str,
    seq: i64,
    at: DateTime<Utc>,
    request_id: Option<Uuid>,
    user_id: Option<i64>,
    event: Event,
    detail: &Value,
) -> String {
    let mut s = String::new();
    s.push_str(prev_hash);
    s.push(SEP);
    s.push_str(&seq.to_string());
    s.push(SEP);
    s.push_str(&truncate_micros(at).to_rfc3339_opts(SecondsFormat::Micros, true));
    s.push(SEP);
    if let Some(r) = request_id {
        s.push_str(&r.hyphenated().to_string());
    }
    s.push(SEP);
    if let Some(u) = user_id {
        s.push_str(&u.to_string());
    }
    s.push(SEP);
    s.push_str(event.as_str());
    s.push(SEP);
    write_canonical(detail, &mut s);
    hex::encode(Sha256::digest(s.as_bytes()))
}

// ---------- typed detail helpers (no question text, ever) ----------

/// Finding kinds and byte offsets only, plus the question's byte length.
pub fn deid_blocked_detail(findings: &[Finding], question_len: usize) -> Value {
    let items: Vec<Value> = findings
        .iter()
        .map(|f| {
            json!({
                "kind": f.kind,
                "start_byte": f.start,
                "end_byte": f.end,
            })
        })
        .collect();
    json!({ "findings": items, "question_len": question_len })
}

/// Verdict, by_rule, model and tokens. The reason is included ONLY when `by_rule`
/// (static rule text); a model-written reason could echo the question, so it is omitted.
pub fn gate_detail(d: &GateDecision) -> Value {
    let mut v = json!({
        "verdict": d.verdict,
        "by_rule": d.by_rule,
        "model": d.model,
        "input_tokens": d.usage.as_ref().map(|u| u.input_tokens),
        "output_tokens": d.usage.as_ref().map(|u| u.output_tokens),
    });
    if d.by_rule {
        v["reason"] = Value::String(d.reason.clone());
    }
    v
}

/// Question byte length and SHA-256 hex. Never the text.
pub fn accepted_detail(question: &str) -> Value {
    json!({
        "question_len": question.len(),
        "question_sha256": hex::encode(Sha256::digest(question.as_bytes())),
    })
}

pub fn charged_detail(model: Option<&str>, usage: Option<&Usage>, paise: i64) -> Value {
    json!({
        "model": model,
        "input_tokens": usage.map(|u| u.input_tokens),
        "output_tokens": usage.map(|u| u.output_tokens),
        "paise": paise,
    })
}

/// The run id and its depth. Never the question.
pub fn run_started_detail(run_id: Uuid, mode: RunMode) -> Value {
    json!({ "run_id": run_id.to_string(), "mode": mode.as_str() })
}

/// How a run ended: a fixed status word (`done`, `partial`, `failed`,
/// `interrupted`, `deleted`), what it cost and how many progress lines it wrote.
/// Never the question or the answer.
pub fn run_finished_detail(
    run_id: Uuid,
    status: &'static str,
    cost_paise: i64,
    steps: i64,
) -> Value {
    json!({
        "run_id": run_id.to_string(),
        "status": status,
        "cost_paise": cost_paise,
        "steps": steps,
    })
}

/// The deleted run's id only.
pub fn run_deleted_detail(run_id: Uuid) -> Value {
    json!({ "run_id": run_id.to_string() })
}

/// A fixed reason kind only (e.g. `not_signed_in`): never headers, tokens or email.
pub fn identity_rejected_detail(reason: &'static str) -> Value {
    json!({ "reason": reason })
}

/// The estimate that did not fit. The cap itself is on the user row.
pub fn cap_blocked_detail(estimate_paise: i64) -> Value {
    json!({ "estimate_paise": estimate_paise })
}

/// A fixed error kind only: `unavailable`, `bad_response` or `unpriced_model`.
pub fn gate_failed_detail(error_kind: &'static str) -> Value {
    json!({ "error_kind": error_kind })
}

// ---------- storage ----------

/// Table names are interpolated into SQL (so tests can use a scratch copy);
/// accept only `[A-Za-z0-9_.]`.
#[cfg(feature = "test-util")]
fn check_table(table: &str) -> sqlx::Result<()> {
    let ok = !table.is_empty()
        && table
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.');
    if ok {
        Ok(())
    } else {
        Err(sqlx::Error::Configuration(
            "invalid audit table name".into(),
        ))
    }
}

fn row_to_entry(row: &PgRow) -> Result<Entry, sqlx::Error> {
    let event: String = row.try_get("event")?;
    let event = Event::parse(&event)
        .ok_or_else(|| sqlx::Error::Decode(format!("unknown audit event {event:?}").into()))?;
    Ok(Entry {
        seq: row.try_get("seq")?,
        at: row.try_get("at")?,
        request_id: row.try_get("request_id")?,
        user_id: row.try_get("user_id")?,
        event,
        detail: row.try_get("detail")?,
        prev_hash: row.try_get("prev_hash")?,
        hash: row.try_get("hash")?,
    })
}

const COLUMNS: &str = "seq, at, request_id, user_id, event, detail, prev_hash, hash";

/// Append one entry to the audit log.
pub async fn append(
    pool: &PgPool,
    request_id: Option<Uuid>,
    user_id: Option<i64>,
    event: Event,
    detail: Value,
) -> sqlx::Result<Entry> {
    append_impl(pool, AUDIT_TABLE, request_id, user_id, event, detail).await
}

/// Tests only: [`append`] against a named table with the audit schema.
#[cfg(feature = "test-util")]
pub async fn append_in(
    pool: &PgPool,
    table: &str,
    request_id: Option<Uuid>,
    user_id: Option<i64>,
    event: Event,
    detail: Value,
) -> sqlx::Result<Entry> {
    check_table(table)?;
    append_impl(pool, table, request_id, user_id, event, detail).await
}

/// First float found at any depth, as a path-free description.
fn contains_float(v: &Value) -> bool {
    match v {
        Value::Number(n) => n.is_f64(),
        Value::Array(a) => a.iter().any(contains_float),
        Value::Object(m) => m.values().any(contains_float),
        _ => false,
    }
}

async fn append_impl(
    pool: &PgPool,
    table: &str,
    request_id: Option<Uuid>,
    user_id: Option<i64>,
    event: Event,
    detail: Value,
) -> sqlx::Result<Entry> {
    // Checked before taking the lock. `Encode` is the sqlx variant for "this value
    // cannot be encoded for storage", which is exactly what a float in detail is.
    if contains_float(&detail) {
        return Err(sqlx::Error::Encode(
            "audit detail must not contain floating-point numbers; use integers or strings".into(),
        ));
    }
    let mut tx = pool.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock($1, $2)")
        .bind(LOCK_NS_AUDIT)
        .bind(LOCK_KEY_AUDIT)
        .execute(&mut *tx)
        .await?;
    let last = sqlx::query(&format!(
        "SELECT seq, hash FROM {table} ORDER BY seq DESC LIMIT 1"
    ))
    .fetch_optional(&mut *tx)
    .await?;
    let (seq, prev_hash) = match last {
        Some(r) => (
            r.try_get::<i64, _>("seq")? + 1,
            r.try_get::<String, _>("hash")?,
        ),
        None => (1, GENESIS_HASH.to_string()),
    };
    let at = truncate_micros(Utc::now());
    let hash = entry_hash(&prev_hash, seq, at, request_id, user_id, event, &detail);
    sqlx::query(&format!(
        "INSERT INTO {table} ({COLUMNS}) VALUES ($1, $2, $3, $4, $5, $6, $7, $8)"
    ))
    .bind(seq)
    .bind(at)
    .bind(request_id)
    .bind(user_id)
    .bind(event.as_str())
    .bind(&detail)
    .bind(&prev_hash)
    .bind(&hash)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Entry {
        seq,
        at,
        request_id,
        user_id,
        event,
        detail,
        prev_hash,
        hash,
    })
}

/// Newest-first.
pub async fn recent(pool: &PgPool, limit: i64) -> sqlx::Result<Vec<Entry>> {
    recent_impl(pool, AUDIT_TABLE, limit).await
}

/// Tests only.
#[cfg(feature = "test-util")]
pub async fn recent_in(pool: &PgPool, table: &str, limit: i64) -> sqlx::Result<Vec<Entry>> {
    check_table(table)?;
    recent_impl(pool, table, limit).await
}

async fn recent_impl(pool: &PgPool, table: &str, limit: i64) -> sqlx::Result<Vec<Entry>> {
    let rows = sqlx::query(&format!(
        "SELECT {COLUMNS} FROM {table} ORDER BY seq DESC LIMIT $1"
    ))
    .bind(limit)
    .fetch_all(pool)
    .await?;
    rows.iter().map(row_to_entry).collect()
}

/// Walk the log in `seq` order and recompute every hash and link. `seq` must be
/// contiguous from 1; a gap is `Broken` at the first missing seq. Reads in
/// keyset-paged batches so memory stays bounded.
pub async fn verify_chain(pool: &PgPool) -> sqlx::Result<ChainStatus> {
    verify_chain_impl(pool, AUDIT_TABLE).await
}

/// Tests only.
#[cfg(feature = "test-util")]
pub async fn verify_chain_in(pool: &PgPool, table: &str) -> sqlx::Result<ChainStatus> {
    check_table(table)?;
    verify_chain_impl(pool, table).await
}

async fn verify_chain_impl(pool: &PgPool, table: &str) -> sqlx::Result<ChainStatus> {
    let sql = format!(
        "SELECT seq, at, request_id, user_id, event, detail, prev_hash, hash \
         FROM {table} WHERE seq > $1 ORDER BY seq LIMIT $2"
    );
    let mut expected_seq: i64 = 1;
    let mut prev_hash = GENESIS_HASH.to_string();
    let mut after: i64 = 0;
    loop {
        let rows = sqlx::query(&sql)
            .bind(after)
            .bind(VERIFY_BATCH)
            .fetch_all(pool)
            .await?;
        if rows.is_empty() {
            return Ok(ChainStatus::Valid {
                entries: expected_seq - 1,
            });
        }
        for row in &rows {
            let seq: i64 = row.try_get("seq")?;
            if seq != expected_seq {
                return Ok(ChainStatus::Broken {
                    at_seq: expected_seq,
                });
            }
            let Ok(e) = row_to_entry(row) else {
                return Ok(ChainStatus::Broken { at_seq: seq });
            };
            let want = entry_hash(
                &prev_hash,
                e.seq,
                e.at,
                e.request_id,
                e.user_id,
                e.event,
                &e.detail,
            );
            if e.prev_hash != prev_hash || e.hash != want {
                return Ok(ChainStatus::Broken { at_seq: seq });
            }
            prev_hash = e.hash;
            expected_seq += 1;
            after = seq;
        }
    }
}
