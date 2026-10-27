//! Audit-log tests. Pure tests run anywhere; DB tests are gated on
//! `TEST_DATABASE_URL`.
//!
//! Isolation: the real `incisive.audit` table is global and shared by every
//! test binary, and the tamper tests deliberately corrupt rows. So every DB test
//! that appends or tampers works in its OWN throw-away table
//! (`incisive.audit_t_<random>`, a `LIKE incisive.audit INCLUDING ALL` copy) via
//! the `*_in` functions, and drops it afterwards. Nothing tampers with the real
//! table; one smoke test uses the public `append`/`verify_chain` on it and only
//! checks entries it created itself.

use chrono::{DateTime, TimeZone, Utc};
use claude_client::Usage;
use incisive_app::audit::{
    ChainStatus, Event, GENESIS_HASH, LOCK_NS_AUDIT, accepted_detail, append, append_in,
    charged_detail, deid_blocked_detail, entry_hash, gate_detail, recent, recent_in, verify_chain,
    verify_chain_in,
};
use incisive_app::db::{lazy_pool, migrate};
use incisive_app::deid;
use incisive_app::gate::{GateDecision, Verdict};
use secrecy::SecretString;
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

fn t0() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 10, 27, 12, 0, 0).unwrap()
}

const RID: Uuid = Uuid::from_u128(0x1234_5678_9abc_def0_1234_5678_9abc_def0);

// ---------- pure ----------

#[test]
fn hash_is_stable_and_order_sensitive() {
    let d = json!({"a": 1});
    let h1 = entry_hash(
        GENESIS_HASH,
        1,
        t0(),
        Some(RID),
        Some(7),
        Event::Accepted,
        &d,
    );
    let h2 = entry_hash(
        GENESIS_HASH,
        1,
        t0(),
        Some(RID),
        Some(7),
        Event::Accepted,
        &d,
    );
    assert_eq!(h1, h2);
    assert_eq!(h1.len(), 64);
    assert!(
        h1.chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_uppercase())
    );
    assert_eq!(GENESIS_HASH, "0".repeat(64));

    // Every input matters.
    let base = |prev: &str, seq, at, rid, uid, ev, d: &serde_json::Value| {
        entry_hash(prev, seq, at, rid, uid, ev, d)
    };
    let others = [
        base(&h1, 1, t0(), Some(RID), Some(7), Event::Accepted, &d),
        base(
            GENESIS_HASH,
            2,
            t0(),
            Some(RID),
            Some(7),
            Event::Accepted,
            &d,
        ),
        base(
            GENESIS_HASH,
            1,
            t0() + chrono::Duration::microseconds(1),
            Some(RID),
            Some(7),
            Event::Accepted,
            &d,
        ),
        base(GENESIS_HASH, 1, t0(), None, Some(7), Event::Accepted, &d),
        base(
            GENESIS_HASH,
            1,
            t0(),
            Some(RID),
            Some(8),
            Event::Accepted,
            &d,
        ),
        base(
            GENESIS_HASH,
            1,
            t0(),
            Some(RID),
            Some(7),
            Event::Charged,
            &d,
        ),
        base(
            GENESIS_HASH,
            1,
            t0(),
            Some(RID),
            Some(7),
            Event::Accepted,
            &json!({"a": 2}),
        ),
    ];
    for o in &others {
        assert_ne!(&h1, o);
    }
    // Order sensitive: swapping prev_hash and a detail-borne value is not a collision.
    let x = entry_hash("aa", 1, t0(), None, None, Event::Accepted, &json!({}));
    let y = entry_hash("a", 1, t0(), None, None, Event::Accepted, &json!({}));
    assert_ne!(x, y);
}

#[test]
fn detail_key_order_does_not_change_hash() {
    // Nested objects, inside arrays too, in different insertion orders.
    let a: serde_json::Value =
        serde_json::from_str(r#"{"z":1,"a":{"y":[{"q":1,"b":2}],"c":3},"m":null}"#).unwrap();
    let b: serde_json::Value =
        serde_json::from_str(r#"{"m":null,"a":{"c":3,"y":[{"b":2,"q":1}]},"z":1}"#).unwrap();
    assert_eq!(
        entry_hash(GENESIS_HASH, 1, t0(), None, None, Event::GateDecided, &a),
        entry_hash(GENESIS_HASH, 1, t0(), None, None, Event::GateDecided, &b)
    );
    // Array order is significant.
    let c = json!({"k": [1, 2]});
    let d = json!({"k": [2, 1]});
    assert_ne!(
        entry_hash(GENESIS_HASH, 1, t0(), None, None, Event::GateDecided, &c),
        entry_hash(GENESIS_HASH, 1, t0(), None, None, Event::GateDecided, &d)
    );
}

#[test]
fn none_and_zero_hash_differently() {
    let d = json!({});
    let h = |rid, uid| entry_hash(GENESIS_HASH, 1, t0(), rid, uid, Event::Accepted, &d);
    assert_ne!(h(None, None), h(None, Some(0)));
    assert_ne!(h(None, None), h(Some(Uuid::nil()), None));
    assert_ne!(h(None, Some(0)), h(Some(Uuid::nil()), Some(0)));
}

#[test]
fn hash_ignores_sub_microsecond_but_not_microsecond() {
    let d = json!({});
    let micro = Utc.timestamp_micros(1_800_000_000_123_456).unwrap();
    let nano = micro + chrono::Duration::nanoseconds(789);
    // entry_hash truncates to microseconds itself, so the stored value round-trips.
    assert_eq!(
        entry_hash(GENESIS_HASH, 1, micro, None, None, Event::Accepted, &d),
        entry_hash(GENESIS_HASH, 1, nano, None, None, Event::Accepted, &d)
    );
}

const PROBE: &str = "Patient called 9876543210 about her hernia repair";

#[test]
fn deid_blocked_detail_never_contains_question_text() {
    let findings = deid::scan(PROBE);
    assert!(
        findings.iter().any(|f| f.kind == deid::FindingKind::Phone),
        "premise: probe has a phone number"
    );
    let detail = deid_blocked_detail(&findings, PROBE.len());
    let s = detail.to_string();
    assert!(!s.contains("9876543210"));
    assert!(!s.contains("hernia"));
    assert!(!s.contains("Patient"));
    assert_eq!(detail["question_len"], PROBE.len());
    let f = &detail["findings"][0];
    assert_eq!(f["kind"], "phone");
    assert!(f["start_byte"].is_u64() && f["end_byte"].is_u64());
}

#[test]
fn accepted_detail_never_contains_question_text() {
    let detail = accepted_detail(PROBE);
    let s = detail.to_string();
    assert!(!s.contains("9876543210"));
    assert!(!s.contains("hernia"));
    assert_eq!(detail["question_len"], PROBE.len());
    let sha = detail["question_sha256"].as_str().unwrap();
    assert_eq!(sha.len(), 64);
    // Known SHA-256 of "abc" pins the algorithm and encoding.
    assert_eq!(
        accepted_detail("abc")["question_sha256"],
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
}

#[test]
fn gate_and_charged_details_have_required_fields() {
    let d = GateDecision {
        verdict: Verdict::NotMedical,
        reason: "About billing.".into(),
        by_rule: false,
        usage: Some(Usage {
            input_tokens: 900,
            output_tokens: 40,
        }),
        model: Some("m-gate".into()),
    };
    let g = gate_detail(&d);
    assert_eq!(g["verdict"], "not_medical");
    assert_eq!(g["reason"], "About billing.");
    assert_eq!(g["by_rule"], false);
    assert_eq!(g["model"], "m-gate");
    assert_eq!(g["input_tokens"], 900);
    assert_eq!(g["output_tokens"], 40);

    let rule = GateDecision {
        verdict: Verdict::Medical,
        reason: "r".into(),
        by_rule: true,
        usage: None,
        model: None,
    };
    let g = gate_detail(&rule);
    assert!(g["model"].is_null() && g["input_tokens"].is_null());

    let c = charged_detail(
        Some("m-gate"),
        Some(&Usage {
            input_tokens: 1,
            output_tokens: 2,
        }),
        16,
    );
    assert_eq!(c["model"], "m-gate");
    assert_eq!(c["input_tokens"], 1);
    assert_eq!(c["output_tokens"], 2);
    assert_eq!(c["paise"], 16);
}

#[test]
fn lock_namespace_is_distinct_from_spend() {
    assert_ne!(LOCK_NS_AUDIT, incisive_app::spend::LOCK_NS_SPEND_GLOBAL);
    assert_ne!(LOCK_NS_AUDIT, incisive_app::spend::LOCK_NS_SPEND_USER);
}

// ---------- DB ----------

async fn test_pool() -> Option<PgPool> {
    let Ok(url) = std::env::var("TEST_DATABASE_URL") else {
        eprintln!("SKIP: TEST_DATABASE_URL not set");
        return None;
    };
    let pool = lazy_pool(&SecretString::from(url)).expect("lazy pool");
    migrate(&pool).await.expect("migrate");
    Some(pool)
}

/// A private copy of the audit table, dropped on `finish`.
struct Scratch {
    pool: PgPool,
    table: String,
}

impl Scratch {
    async fn new(pool: &PgPool) -> Scratch {
        let table = format!("incisive.audit_t_{}", Uuid::new_v4().simple());
        sqlx::query(&format!(
            "CREATE TABLE {table} (LIKE incisive.audit INCLUDING ALL)"
        ))
        .execute(pool)
        .await
        .unwrap();
        Scratch {
            pool: pool.clone(),
            table,
        }
    }
    async fn finish(self) {
        sqlx::query(&format!("DROP TABLE {}", self.table))
            .execute(&self.pool)
            .await
            .unwrap();
    }
}

async fn fill(s: &Scratch, n: i64) {
    for i in 0..n {
        append_in(
            &s.pool,
            &s.table,
            Some(RID),
            Some(i),
            Event::Accepted,
            json!({"i": i, "nested": {"b": 1, "a": [1, {"y": 2, "x": 3}]}}),
        )
        .await
        .unwrap();
    }
}

#[tokio::test]
async fn chain_links_and_verifies() {
    let Some(pool) = test_pool().await else {
        return;
    };
    let s = Scratch::new(&pool).await;
    assert_eq!(
        verify_chain_in(&pool, &s.table).await.unwrap(),
        ChainStatus::Valid { entries: 0 }
    );
    fill(&s, 5).await;
    let rows = recent_in(&pool, &s.table, 10).await.unwrap();
    assert_eq!(rows.len(), 5);
    // recent is newest first.
    assert_eq!(
        rows.iter().map(|e| e.seq).collect::<Vec<_>>(),
        vec![5, 4, 3, 2, 1]
    );
    assert_eq!(rows[4].prev_hash, GENESIS_HASH);
    for w in rows.windows(2) {
        assert_eq!(w[0].prev_hash, w[1].hash);
    }
    // The stored value round-trips exactly: recomputing from what we read matches.
    for e in &rows {
        assert_eq!(
            e.hash,
            entry_hash(
                &e.prev_hash,
                e.seq,
                e.at,
                e.request_id,
                e.user_id,
                e.event,
                &e.detail
            )
        );
    }
    assert_eq!(
        verify_chain_in(&pool, &s.table).await.unwrap(),
        ChainStatus::Valid { entries: 5 }
    );
    s.finish().await;
}

#[tokio::test]
async fn edited_detail_breaks_chain_at_that_seq() {
    let Some(pool) = test_pool().await else {
        return;
    };
    let s = Scratch::new(&pool).await;
    fill(&s, 6).await;
    sqlx::query(&format!(
        "UPDATE {} SET detail = '{{\"i\": 999}}' WHERE seq = 3",
        s.table
    ))
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(
        verify_chain_in(&pool, &s.table).await.unwrap(),
        ChainStatus::Broken { at_seq: 3 }
    );
    s.finish().await;
}

#[tokio::test]
async fn edited_hash_and_link_are_caught() {
    let Some(pool) = test_pool().await else {
        return;
    };
    let s = Scratch::new(&pool).await;
    fill(&s, 4).await;
    // Rewriting row 2's hash breaks row 2 (hash mismatch).
    sqlx::query(&format!(
        "UPDATE {} SET hash = repeat('a', 64) WHERE seq = 2",
        s.table
    ))
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(
        verify_chain_in(&pool, &s.table).await.unwrap(),
        ChainStatus::Broken { at_seq: 2 }
    );
    s.finish().await;
}

#[tokio::test]
async fn deleted_middle_row_breaks_chain() {
    let Some(pool) = test_pool().await else {
        return;
    };
    let s = Scratch::new(&pool).await;
    fill(&s, 6).await;
    sqlx::query(&format!("DELETE FROM {} WHERE seq = 4", s.table))
        .execute(&pool)
        .await
        .unwrap();
    // Gap: the first missing seq is reported.
    assert_eq!(
        verify_chain_in(&pool, &s.table).await.unwrap(),
        ChainStatus::Broken { at_seq: 4 }
    );
    s.finish().await;
}

#[tokio::test]
async fn deleted_first_row_breaks_chain() {
    let Some(pool) = test_pool().await else {
        return;
    };
    let s = Scratch::new(&pool).await;
    fill(&s, 3).await;
    sqlx::query(&format!("DELETE FROM {} WHERE seq = 1", s.table))
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        verify_chain_in(&pool, &s.table).await.unwrap(),
        ChainStatus::Broken { at_seq: 1 }
    );
    s.finish().await;
}

#[tokio::test]
async fn truncated_tail_is_not_detectable_by_chain_alone() {
    // Documents the known limit (R-P5): only an external checkpoint of the
    // latest hash would catch this.
    let Some(pool) = test_pool().await else {
        return;
    };
    let s = Scratch::new(&pool).await;
    fill(&s, 4).await;
    sqlx::query(&format!("DELETE FROM {} WHERE seq > 2", s.table))
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        verify_chain_in(&pool, &s.table).await.unwrap(),
        ChainStatus::Valid { entries: 2 }
    );
    s.finish().await;
}

#[tokio::test]
async fn unknown_event_text_is_reported_broken() {
    let Some(pool) = test_pool().await else {
        return;
    };
    let s = Scratch::new(&pool).await;
    fill(&s, 2).await;
    sqlx::query(&format!(
        "UPDATE {} SET event = 'bogus' WHERE seq = 2",
        s.table
    ))
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(
        verify_chain_in(&pool, &s.table).await.unwrap(),
        ChainStatus::Broken { at_seq: 2 }
    );
    s.finish().await;
}

#[tokio::test]
async fn concurrent_appends_never_fork() {
    let Some(pool) = test_pool().await else {
        return;
    };
    let s = Scratch::new(&pool).await;
    let mut handles = Vec::new();
    for i in 0..50i64 {
        let pool = pool.clone();
        let table = s.table.clone();
        handles.push(tokio::spawn(async move {
            append_in(
                &pool,
                &table,
                Some(Uuid::new_v4()),
                Some(i),
                Event::Charged,
                json!({"i": i}),
            )
            .await
            .unwrap()
        }));
    }
    for h in handles {
        h.await.unwrap();
    }
    let rows = recent_in(&pool, &s.table, 100).await.unwrap();
    assert_eq!(rows.len(), 50);
    let mut seqs: Vec<i64> = rows.iter().map(|e| e.seq).collect();
    seqs.sort();
    assert_eq!(seqs, (1..=50).collect::<Vec<_>>());
    for w in rows.windows(2) {
        assert_eq!(w[0].prev_hash, w[1].hash, "fork at seq {}", w[0].seq);
    }
    assert_eq!(
        verify_chain_in(&pool, &s.table).await.unwrap(),
        ChainStatus::Valid { entries: 50 }
    );
    s.finish().await;
}

#[tokio::test]
async fn public_api_appends_to_the_real_table() {
    let Some(pool) = test_pool().await else {
        return;
    };
    // The real table is shared and may hold other tests' rows, so check only
    // entries this test created, and the chain's validity (nothing tampers with it).
    let rid = Uuid::new_v4();
    let e1 = append(
        &pool,
        Some(rid),
        None,
        Event::IdentityRejected,
        json!({"why": "x"}),
    )
    .await
    .unwrap();
    let e2 = append(
        &pool,
        Some(rid),
        Some(1),
        Event::Accepted,
        accepted_detail("q"),
    )
    .await
    .unwrap();
    assert!(e2.seq > e1.seq);
    let mine: Vec<_> = recent(&pool, 200)
        .await
        .unwrap()
        .into_iter()
        .filter(|e| e.request_id == Some(rid))
        .collect();
    assert_eq!(mine.len(), 2);
    assert!(matches!(
        verify_chain(&pool).await.unwrap(),
        ChainStatus::Valid { .. }
    ));
}
