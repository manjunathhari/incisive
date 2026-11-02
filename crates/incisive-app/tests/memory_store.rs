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

async fn rec(pool: &PgPool, topic: i64, _run: Uuid, label: &str) -> (i64, i32) {
    // One record per run (unique index): each record gets its own run.
    let (_, run) = seed_run(pool).await;
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
            embed_text: &format!("{label}\nembed"),
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
        group_id: "g-o0-robotic-lap-ratio--higher_worse".into(),
        unit_key: "".into(),
        polarity: "higher_worse".into(),
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
async fn ledger_claims_keyed_by_group() {
    let Some(pool) = connect().await else { return };
    let (_, run) = seed_run(&pool).await;
    let t = new_topic(&pool).await;
    let (r, _) = rec(&pool, t, run, "L").await;
    let claim = |g: &str| LedgerClaim {
        group_id: g.into(),
        unit_key: "u".into(),
        polarity: "higher_worse".into(),
        outcome: "o".into(),
        intervention: "robotic".into(),
        comparator: "lap".into(),
        era_id: "e1".into(),
        era_start: 2010,
        era_end: 2020,
        estimate: Some(1.0),
        lo: None,
        hi: None,
        effect_type: "diff".into(),
        n_studies: 1,
        label: "stable".into(),
    };
    // Same outcome, arms and era in two groups: both are kept.
    let mut tx = pool.begin().await.unwrap();
    store::upsert_ledger(&mut tx, t, r, &[], &[claim("g-a"), claim("g-b")])
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let n: i64 =
        sqlx::query_scalar("SELECT count(*) FROM incisive.ledger_claims WHERE record_id=$1")
            .bind(r)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(n, 2);
    // The same group and era twice is an error, not a silent drop.
    let mut tx = pool.begin().await.unwrap();
    assert!(
        store::upsert_ledger(&mut tx, t, r, &[], &[claim("g-a")])
            .await
            .is_err()
    );
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
    // Absolute rank, rank/(rank+1): a lone hit is never a perfect 1.0.
    assert!(
        got[0].fts_rank > 0.0 && got[0].fts_rank < 1.0,
        "{}",
        got[0].fts_rank
    );
    // Any query word matches (OR), and punctuation cannot break the query.
    let any = store::topic_candidates(&pool, &format!("nomatchword, \"{word}\"; ' --"), None, 10)
        .await
        .unwrap();
    assert_eq!(
        any.iter().map(|t| t.topic_id).collect::<Vec<_>>(),
        vec![live]
    );
    assert_eq!(got[0].pairs, pairs());
    assert!(
        store::topic_candidates(&pool, "  ", None, 10)
            .await
            .unwrap()
            .is_empty()
    );
}

static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Tests that call `claim_jobs` claim across the whole database, so they run
/// one at a time on an empty job queue.
async fn begin_jobs() -> Option<(tokio::sync::MutexGuard<'static, ()>, PgPool)> {
    let guard = SERIAL.lock().await;
    let pool = connect().await?;
    sqlx::query("TRUNCATE incisive.memory_jobs")
        .execute(&pool)
        .await
        .unwrap();
    Some((guard, pool))
}

#[test]
fn fts_words_tokenise() {
    assert_eq!(
        store::fts_words("Robotic vs. Lap-Chole (2021) a  or ROBOTIC; it's \"x\" ' --"),
        vec!["robotic", "vs", "lap", "chole", "2021", "it"]
    );
    assert!(store::fts_words(" ,; a - ").is_empty());
    let long: String = (0..100).map(|i| format!("word{i} ")).collect();
    assert_eq!(store::fts_words(&long).len(), 40);
}

#[tokio::test]
async fn claim_is_exclusive() {
    let Some((_g, pool)) = begin_jobs().await else {
        return;
    };
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
    let mut v = v;
    v[0] = (Uuid::new_v4().as_u128() % 10_000) as f32 / 10_000.0;
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
}

#[tokio::test]
async fn candidates_by_vector_and_missing_embedding() {
    let Some(pool) = connect().await else { return };
    let (_, run) = seed_run(&pool).await;
    let t = new_topic(&pool).await;
    let label = format!("miss {}", Uuid::new_v4());
    let (r, _) = rec(&pool, t, run, &label).await;
    let missing = store::records_missing_embedding(&pool, 100_000)
        .await
        .unwrap();
    let hit = missing.iter().find(|(id, _)| *id == r).expect("returned");
    assert_eq!(hit.1, format!("{label}\nembed"));
    store::set_record_embedding(&pool, r, &[0.1; 768], "m")
        .await
        .unwrap();
    let missing = store::records_missing_embedding(&pool, 100_000)
        .await
        .unwrap();
    assert!(!missing.iter().any(|(id, _)| *id == r));
    // vector-only candidate: an exact-match unique vector is nearest
    let mut v = vec![0.0_f32; 768];
    v[1] = 1.0;
    v[2] = (Uuid::new_v4().as_u128() % 10_000) as f32 / 10_000.0 + 0.5;
    store::set_topic_embedding(&pool, t, &v, "m").await.unwrap();
    let c = store::topic_candidates(&pool, "", Some(&v), 1)
        .await
        .unwrap();
    assert_eq!(c.len(), 1);
    assert_eq!(c[0].topic_id, t);
    assert_eq!(c[0].fts_rank, 0.0);
    assert!(c[0].embedding.is_some());
}

#[tokio::test]
async fn ledger_keeps_study_without_year_or_era() {
    let Some(pool) = connect().await else { return };
    let (_, run) = seed_run(&pool).await;
    let t = new_topic(&pool).await;
    let (r, _) = rec(&pool, t, run, "L").await;
    let s = LedgerStudy {
        pub_year: None,
        era_id: None,
        ..study("999")
    };
    let mut tx = pool.begin().await.unwrap();
    store::upsert_ledger(&mut tx, t, r, &[s], &[])
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let row: (Option<i32>, Option<String>) = sqlx::query_as(
        "SELECT pub_year, era_id FROM incisive.ledger_studies WHERE topic_id=$1 AND pmid='999'",
    )
    .bind(t)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(row, (None, None));
}

#[tokio::test]
async fn stale_exhausted_running_job_is_given_up() {
    let Some((_g, pool)) = begin_jobs().await else {
        return;
    };
    let (_, run) = seed_run(&pool).await;
    store::enqueue_job(&pool, run).await.unwrap();
    sqlx::query(
        "UPDATE incisive.memory_jobs SET status='running', attempts=3, \
         claimed_at = now() - interval '20 minutes' WHERE run_id=$1",
    )
    .bind(run)
    .execute(&pool)
    .await
    .unwrap();
    let got = store::claim_jobs(&pool, 100).await.unwrap();
    assert!(!got.contains(&run));
    let (s, e): (String, Option<String>) =
        sqlx::query_as("SELECT status, last_error_kind FROM incisive.memory_jobs WHERE run_id=$1")
            .bind(run)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!((s.as_str(), e.as_deref()), ("failed", Some("gave_up")));
}

#[tokio::test]
async fn backfill_topic_embeddings_copies_latest_record_vector() {
    let Some(pool) = connect().await else { return };
    let (_, run) = seed_run(&pool).await;
    let t = new_topic(&pool).await;
    let (r1, _) = rec(&pool, t, run, "old").await;
    let (r2, _) = rec(&pool, t, run, "new").await;
    store::set_record_embedding(&pool, r1, &[0.1; 768], "m1")
        .await
        .unwrap();
    store::set_record_embedding(&pool, r2, &[0.2; 768], "m2")
        .await
        .unwrap();
    let t2 = new_topic(&pool).await; // no embedded record: stays NULL
    rec(&pool, t2, run, "none").await;
    assert!(store::backfill_topic_embeddings(&pool).await.unwrap() >= 1);
    let (has, model): (bool, Option<String>) = sqlx::query_as(
        "SELECT embedding IS NOT NULL, embed_model FROM incisive.topics WHERE id = $1",
    )
    .bind(t)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(has);
    assert_eq!(model.as_deref(), Some("m2"));
    let none: bool =
        sqlx::query_scalar("SELECT embedding IS NULL FROM incisive.topics WHERE id = $1")
            .bind(t2)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(none);
}

#[tokio::test]
async fn note_embed_attempt_caps_the_missing_list() {
    let Some(pool) = connect().await else { return };
    let (_, run) = seed_run(&pool).await;
    let t = new_topic(&pool).await;
    let (r, _) = rec(&pool, t, run, &format!("cap {}", Uuid::new_v4())).await;
    let listed = || async {
        store::records_missing_embedding(&pool, 100_000)
            .await
            .unwrap()
            .iter()
            .any(|(id, _)| *id == r)
    };
    for _ in 0..4 {
        store::note_embed_attempt(&pool, r).await.unwrap();
    }
    assert!(listed().await);
    store::note_embed_attempt(&pool, r).await.unwrap();
    assert!(!listed().await);
}
