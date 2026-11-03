//! `chat::retrieve::top` against the database (`TEST_DATABASE_URL`).

use incisive_app::chat::passages::PassageKind;
use incisive_app::chat::retrieve::{self, top};
use incisive_app::db::{lazy_pool, migrate};
use incisive_app::memory::store;
use secrecy::SecretString;
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

async fn user(pool: &PgPool) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO incisive.users (email, display_name, role, daily_cap_paise) \
         VALUES ($1, 'R', 'owner', 100000) RETURNING id",
    )
    .bind(format!("ret-{}@example.test", Uuid::new_v4()))
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn topic(pool: &PgPool) -> i64 {
    let mut c = pool.acquire().await.unwrap();
    store::create_topic(
        &mut c,
        &format!("topic {}", Uuid::new_v4()),
        &[("robotic".to_string(), "laparoscopic".to_string())],
    )
    .await
    .unwrap()
}

async fn record(pool: &PgPool, topic: i64) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO incisive.evidence_records \
         (topic_id, version, question, answer, answer_version, board, era_table, \
          evidence_through, topic_label) \
         VALUES ($1, 1, 'q', '{}', 3, '{}', '{}', '2026-01-01', 'l') RETURNING id",
    )
    .bind(topic)
    .fetch_one(pool)
    .await
    .unwrap()
}

fn axis(i: usize) -> Vec<f32> {
    let mut v = vec![0.0f32; 768];
    v[i] = 1.0;
    v
}

async fn passage(
    pool: &PgPool,
    rec: i64,
    seq: i32,
    text: &str,
    comps: &[&str],
    emb: Option<Vec<f32>>,
) -> i64 {
    let comps: Vec<String> = comps.iter().map(|s| s.to_string()).collect();
    sqlx::query_scalar(
        "INSERT INTO incisive.chat_passages (record_id, kind, seq, ref_ns, comparisons, text, embedding) \
         VALUES ($1, 'evidence', $2, '{1}', $3, $4, $5::vector) RETURNING id",
    )
    .bind(rec)
    .bind(seq)
    .bind(&comps)
    .bind(text)
    .bind(emb.as_deref().map(store::vec_literal))
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn qa(pool: &PgPool, topic: i64, author: i64, status: &str, text: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO incisive.topic_qas (topic_id, question, gemma_reply, owner_text, status, author_user_id) \
         VALUES ($1, 'asked', 'gemma said', $2, $3, $4) RETURNING id",
    )
    .bind(topic)
    .bind(text)
    .bind(status)
    .bind(author)
    .fetch_one(pool)
    .await
    .unwrap()
}

const RL: &str = "laparoscopic|robotic";
const RO: &str = "open|robotic";

#[tokio::test]
async fn other_comparison_excluded() {
    let Some(pool) = connect().await else { return };
    let t = topic(&pool).await;
    let r = record(&pool, t).await;
    let lap = passage(&pool, r, 0, "zebra recurrence laparoscopic", &[RL], None).await;
    let open = passage(&pool, r, 1, "zebra recurrence open", &[RO], None).await;
    let hits = top(&pool, r, t, Some(RO), "zebra recurrence", None, 0.1)
        .await
        .unwrap();
    let ids: Vec<_> = hits.iter().filter_map(|h| h.passage_id).collect();
    assert_eq!(ids, vec![open]);
    assert!(!ids.contains(&lap));
}

#[tokio::test]
async fn untagged_passages_stay() {
    let Some(pool) = connect().await else { return };
    let t = topic(&pool).await;
    let r = record(&pool, t).await;
    let p = passage(&pool, r, 0, "zebra recurrence general", &[], None).await;
    let hits = top(&pool, r, t, Some(RO), "zebra recurrence", None, 0.1)
        .await
        .unwrap();
    assert_eq!(hits[0].passage_id, Some(p));
}

#[tokio::test]
async fn passage_with_both_tags_stays() {
    let Some(pool) = connect().await else { return };
    let t = topic(&pool).await;
    let r = record(&pool, t).await;
    let p = passage(&pool, r, 0, "zebra recurrence both", &[RL, RO], None).await;
    let hits = top(&pool, r, t, Some(RO), "zebra recurrence", None, 0.1)
        .await
        .unwrap();
    assert_eq!(hits[0].passage_id, Some(p));
}

#[tokio::test]
async fn below_threshold_returns_empty() {
    let Some(pool) = connect().await else { return };
    let t = topic(&pool).await;
    let r = record(&pool, t).await;
    passage(&pool, r, 0, "zebra recurrence", &[], None).await;
    let hits = top(&pool, r, t, None, "unrelated giraffe", None, 0.2)
        .await
        .unwrap();
    assert!(hits.is_empty());
    // a partial match under a high bar is also out
    let hits = top(&pool, r, t, None, "zebra giraffe", None, 0.9)
        .await
        .unwrap();
    assert!(hits.is_empty());
}

#[tokio::test]
async fn keeps_top_three() {
    let Some(pool) = connect().await else { return };
    let t = topic(&pool).await;
    let r = record(&pool, t).await;
    for i in 0..5 {
        passage(&pool, r, i, "zebra recurrence", &[], None).await;
    }
    let hits = top(&pool, r, t, None, "zebra recurrence", None, 0.1)
        .await
        .unwrap();
    assert_eq!(hits.len(), 3);
    // ties break by lower id, deterministically
    let ids: Vec<_> = hits.iter().filter_map(|h| h.passage_id).collect();
    let mut sorted = ids.clone();
    sorted.sort();
    assert_eq!(ids, sorted);
}

#[tokio::test]
async fn null_vector_passage_can_match_on_words() {
    let Some(pool) = connect().await else { return };
    let t = topic(&pool).await;
    let r = record(&pool, t).await;
    let nov = passage(&pool, r, 0, "zebra recurrence", &[], None).await;
    let vec = passage(&pool, r, 1, "other text", &[], Some(axis(1))).await;
    let hits = top(&pool, r, t, None, "zebra recurrence", Some(&axis(1)), 0.5)
        .await
        .unwrap();
    let ids: Vec<_> = hits.iter().filter_map(|h| h.passage_id).collect();
    assert!(ids.contains(&nov), "word-only match kept");
    assert!(ids.contains(&vec), "vector match kept");
    let v = hits.iter().find(|h| h.passage_id == Some(vec)).unwrap();
    assert!(
        (v.score - 0.7).abs() < 1e-3,
        "0.7*cos(1)+0.3*0, got {}",
        v.score
    );
}

#[tokio::test]
async fn any_word_matches_not_all() {
    let Some(pool) = connect().await else { return };
    let t = topic(&pool).await;
    let r = record(&pool, t).await;
    let p = passage(&pool, r, 0, "hernia repair outcomes", &[], None).await;
    let hits = top(&pool, r, t, None, "hernia mesh complications", None, 0.2)
        .await
        .unwrap();
    assert_eq!(hits[0].passage_id, Some(p));
}

#[tokio::test]
async fn approved_qa_for_topic_is_included_and_other_statuses_are_not() {
    let Some(pool) = connect().await else { return };
    let t = topic(&pool).await;
    let r = record(&pool, t).await;
    let u = user(&pool).await;
    let ok = qa(&pool, t, u, "approved", "zebra recurrence owner answer").await;
    qa(&pool, t, u, "suggested", "zebra recurrence suggested").await;
    qa(&pool, t, u, "rejected", "zebra recurrence rejected").await;
    let other = topic(&pool).await;
    qa(&pool, other, u, "approved", "zebra recurrence elsewhere").await;
    let hits = top(&pool, r, t, None, "zebra recurrence", None, 0.1)
        .await
        .unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].qa_id, Some(ok));
    assert_eq!(hits[0].kind, PassageKind::OwnerQa);
    assert_eq!(hits[0].text, "zebra recurrence owner answer");
    assert!(hits[0].passage_id.is_none() && hits[0].ref_ns.is_empty());
}

#[tokio::test]
async fn qa_of_merged_topic_is_found() {
    let Some(pool) = connect().await else { return };
    let (a, b, c) = (topic(&pool).await, topic(&pool).await, topic(&pool).await);
    sqlx::query("UPDATE incisive.topics SET merged_into = $2 WHERE id = $1")
        .bind(a)
        .bind(b)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE incisive.topics SET merged_into = $2 WHERE id = $1")
        .bind(b)
        .bind(c)
        .execute(&pool)
        .await
        .unwrap();
    let r = record(&pool, a).await;
    let u = user(&pool).await;
    let qb = qa(&pool, b, u, "approved", "zebra recurrence on B").await;
    let qc = qa(&pool, c, u, "approved", "zebra recurrence on C").await;
    let hits = top(&pool, r, a, None, "zebra recurrence", None, 0.1)
        .await
        .unwrap();
    let ids: Vec<_> = hits.iter().filter_map(|h| h.qa_id).collect();
    assert!(ids.contains(&qb) && ids.contains(&qc));
}

#[tokio::test]
async fn other_records_passages_are_never_returned() {
    let Some(pool) = connect().await else { return };
    let t = topic(&pool).await;
    let r1 = record(&pool, t).await;
    let t2 = topic(&pool).await;
    let r2 = record(&pool, t2).await;
    passage(&pool, r2, 0, "zebra recurrence", &[], None).await;
    let hits = top(&pool, r1, t, None, "zebra recurrence", None, 0.0)
        .await
        .unwrap();
    assert!(hits.is_empty());
    let _ = retrieve::TOP_N;
}

/// An arm the record does not compare: only passages tagged with that arm, and
/// untagged ones, come through.
#[tokio::test]
async fn single_arm_keeps_only_passages_tagged_with_it() {
    let Some(pool) = connect().await else {
        return;
    };
    let t = topic(&pool).await;
    let r = record(&pool, t).await;
    let rl = passage(&pool, r, 0, "zebra stay", &[RL], None).await;
    let open = passage(&pool, r, 1, "zebra stay", &[RO], None).await;
    let plain = passage(&pool, r, 2, "zebra stay", &[], None).await;
    let hits = top(&pool, r, t, Some("open"), "zebra stay", None, 0.1)
        .await
        .unwrap();
    let ids: Vec<i64> = hits.iter().filter_map(|h| h.passage_id).collect();
    assert_eq!(ids, [open, plain], "not {rl}");
    // A key still needs the whole key.
    let hits = top(&pool, r, t, Some(RL), "zebra stay", None, 0.1)
        .await
        .unwrap();
    let ids: Vec<i64> = hits.iter().filter_map(|h| h.passage_id).collect();
    assert_eq!(ids, [rl, plain]);
}
