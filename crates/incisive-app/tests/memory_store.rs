//! `memory::store` against the database (`TEST_DATABASE_URL`).

use chrono::NaiveDate;
use incisive_app::db::{lazy_pool, migrate};
use incisive_app::memory::extract::{LedgerClaim, LedgerStudy};
use incisive_app::memory::store::{self, NewRecord};
use secrecy::SecretString;
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

async fn connect() -> Option<PgPool> {
    let Ok(url) = std::env::var("TEST_DATABASE_URL") else {
        eprintln!("SKIP: TEST_DATABASE_URL not set");
        return None;
    };
    let pool = lazy_pool(&SecretString::from(url)).expect("lazy pool");
    migrate(&pool).await.expect("migrate");
    Some(pool)
}

async fn seed_run(pool: &PgPool) -> (i64, Uuid) {
    let email = format!("mem-{}@example.test", Uuid::new_v4());
    let user: i64 = sqlx::query_scalar(
        "INSERT INTO incisive.users (email, display_name, role, daily_cap_paise) \
         VALUES ($1, 'Mem', 'surgeon', 100000) RETURNING id",
    )
    .bind(&email)
    .fetch_one(pool)
    .await
    .unwrap();
    let run = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO incisive.runs (id, user_id, question, mode, status, limit_paise) \
         VALUES ($1, $2, 'q', 'quick', 'done', 1000)",
    )
    .bind(run)
    .bind(user)
    .execute(pool)
    .await
    .unwrap();
    (user, run)
}

fn pairs() -> Vec<(String, String)> {
    vec![("robotic".into(), "laparoscopic".into())]
}

async fn new_topic(pool: &PgPool) -> i64 {
    let mut c = pool.acquire().await.unwrap();
    store::create_topic(&mut c, &format!("topic {}", Uuid::new_v4()), &pairs())
        .await
        .unwrap()
}

async fn rec(pool: &PgPool, topic: i64, run: Uuid, label: &str) -> (i64, i32) {
    let v = json!({});
    let mut tx = pool.begin().await.unwrap();
    let r = store::insert_record(
        &mut tx,
        &NewRecord {
            topic_id: topic,
            source_run_id: run,
            question: "q",
            answer: &v,
            answer_version: 3,
            board: &v,
            era_table: &v,
            evidence_through: NaiveDate::from_ymd_opt(2026, 1, 1).unwrap(),
            topic_label: label,
            fts_extra: "",
        },
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    r
}

fn study(pmid: &str) -> LedgerStudy {
    LedgerStudy {
        pmid: pmid.into(),
        pub_year: Some(2020),
        treatment_start: Some(2015),
        treatment_end: Some(2018),
        years_assumed: false,
        era_id: Some("e1".into()),
    }
}

#[tokio::test]
async fn insert_versions_increment() {
    let Some(pool) = connect().await else { return };
    let (_, run) = seed_run(&pool).await;
    let t = new_topic(&pool).await;
    let (_, v1) = rec(&pool, t, run, "L").await;
    let (_, v2) = rec(&pool, t, run, "L").await;
    assert_eq!((v1, v2), (1, 2));
}

#[tokio::test]
async fn ledger_dedups_pmids() {
    let Some(pool) = connect().await else { return };
    let (_, run) = seed_run(&pool).await;
    let t = new_topic(&pool).await;
    let (r1, _) = rec(&pool, t, run, "L").await;
    let (r2, _) = rec(&pool, t, run, "L").await;
    let claim = LedgerClaim {
        outcome: "o".into(),
        intervention: "robotic".into(),
        comparator: "lap".into(),
        era_id: "e1".into(),
        era_start: 2010,
        era_end: 2020,
        estimate: Some(0.8),
        lo: Some(0.5),
        hi: None,
        effect_type: "OR".into(),
        n_studies: 3,
        label: "stable".into(),
    };
    let mut tx = pool.begin().await.unwrap();
    // duplicate pmid inside one call must not error
    store::upsert_ledger(
        &mut tx,
        t,
        r1,
        &[study("123"), study("123")],
        std::slice::from_ref(&claim),
    )
    .await
    .unwrap();
    store::upsert_ledger(
        &mut tx,
        t,
        r2,
        &[study("123")],
        std::slice::from_ref(&claim),
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    let rows: Vec<(i64, i64)> = sqlx::query_as(
        "SELECT first_record_id, last_record_id FROM incisive.ledger_studies WHERE topic_id=$1",
    )
    .bind(t)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(rows, vec![(r1, r2)]);
    let n: i64 =
        sqlx::query_scalar("SELECT count(*) FROM incisive.ledger_claims WHERE topic_id=$1")
            .bind(t)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(n, 2);
}

#[tokio::test]
async fn resolve_follows_chain() {
    let Some(pool) = connect().await else { return };
    let (a, b, c) = (
        new_topic(&pool).await,
        new_topic(&pool).await,
        new_topic(&pool).await,
    );
    for (from, to) in [(a, b), (b, c)] {
        sqlx::query("UPDATE incisive.topics SET merged_into=$2 WHERE id=$1")
            .bind(from)
            .bind(to)
            .execute(&pool)
            .await
            .unwrap();
    }
    assert_eq!(store::resolve_topic(&pool, a).await.unwrap(), c);
    assert_eq!(store::resolve_topic(&pool, c).await.unwrap(), c);
    // cycle c -> a (a->b->c->a)
    sqlx::query("UPDATE incisive.topics SET merged_into=$2 WHERE id=$1")
        .bind(c)
        .bind(a)
        .execute(&pool)
        .await
        .unwrap();
    let r = store::resolve_topic(&pool, a).await.unwrap();
    assert!([a, b, c].contains(&r));
}

#[tokio::test]
async fn candidates_exclude_merged() {
    let Some(pool) = connect().await else { return };
    let (_, run) = seed_run(&pool).await;
    let word = format!("zyx{}", Uuid::new_v4().simple());
    let live = new_topic(&pool).await;
    let dead = new_topic(&pool).await;
    rec(&pool, live, run, &format!("{word} live")).await;
    rec(&pool, dead, run, &format!("{word} dead")).await;
    sqlx::query("UPDATE incisive.topics SET merged_into=$2 WHERE id=$1")
        .bind(dead)
        .bind(live)
        .execute(&pool)
        .await
        .unwrap();
    let got = store::topic_candidates(&pool, &word, None, 10)
        .await
        .unwrap();
    let ids: Vec<i64> = got.iter().map(|t| t.topic_id).collect();
    assert_eq!(ids, vec![live]);
    assert!((got[0].fts_rank - 1.0).abs() < 1e-6);
    assert_eq!(got[0].pairs, pairs());
    assert!(
        store::topic_candidates(&pool, "  ", None, 10)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn claim_is_exclusive() {
    let Some(pool) = connect().await else { return };
    let (_, run) = seed_run(&pool).await;
    assert!(store::enqueue_job(&pool, run).await.unwrap());
    assert!(!store::enqueue_job(&pool, run).await.unwrap());
    let (a, b) = tokio::join!(store::claim_jobs(&pool, 5), store::claim_jobs(&pool, 5));
    let total = a
        .unwrap()
        .into_iter()
        .chain(b.unwrap())
        .filter(|r| *r == run)
        .count();
    assert_eq!(total, 1);
    store::finish_job(&pool, run, "failed", Some("boom"), None)
        .await
        .unwrap();
    let (s, e, att): (String, Option<String>, i32) = sqlx::query_as(
        "SELECT status, last_error_kind, attempts FROM incisive.memory_jobs WHERE run_id=$1",
    )
    .bind(run)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!((s.as_str(), e.as_deref(), att), ("failed", Some("boom"), 1));
}

#[tokio::test]
async fn vec_literal_round_trip() {
    let Some(pool) = connect().await else { return };
    let (_, run) = seed_run(&pool).await;
    let t = new_topic(&pool).await;
    let (r, _) = rec(&pool, t, run, "emb label").await;
    let v: Vec<f32> = (0..768).map(|i| (i as f32) * 0.001 - 0.3).collect();
    store::set_record_embedding(&pool, r, &v, "m")
        .await
        .unwrap();
    store::set_topic_embedding(&pool, t, &v, "m").await.unwrap();
    let txt: String =
        sqlx::query_scalar("SELECT embedding::text FROM incisive.evidence_records WHERE id=$1")
            .bind(r)
            .fetch_one(&pool)
            .await
            .unwrap();
    let back: Vec<f32> = txt
        .trim_matches(|c| c == '[' || c == ']')
        .split(',')
        .map(|x| x.parse().unwrap())
        .collect();
    assert_eq!(back.len(), 768);
    for (a, b) in v.iter().zip(&back) {
        assert!((a - b).abs() < 1e-6);
    }
    assert!(store::vec_literal(&[0.5, 1.0]).starts_with("[0.5,1"));
    let c = store::topic_candidates(&pool, "", Some(&v), 5)
        .await
        .unwrap();
    assert!(c.iter().any(|f| f.topic_id == t && f.embedding.is_some()));
    let missing = store::records_missing_embedding(&pool, 1000).await.unwrap();
    assert!(!missing.iter().any(|(id, _)| *id == r));
}
