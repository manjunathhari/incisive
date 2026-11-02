//! Migration 0013 (memory tables) with the database (`TEST_DATABASE_URL`):
//! the eight tables exist, records outlive their run, jobs cascade with the
//! run, row-level security is on, and the verdict CHECK holds.

use incisive_app::db::{lazy_pool, migrate};
use secrecy::SecretString;
use sqlx::PgPool;
use uuid::Uuid;

const TABLES: [&str; 8] = [
    "topics",
    "evidence_records",
    "ledger_studies",
    "ledger_claims",
    "topic_verdicts",
    "owner_revisions",
    "topic_changes",
    "memory_jobs",
];

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

async fn seed_topic(pool: &PgPool) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO incisive.topics (label, pairs) VALUES ($1, '[]'::jsonb) RETURNING id",
    )
    .bind(format!("topic {}", Uuid::new_v4()))
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn seed_record(pool: &PgPool, topic: i64, run: Uuid) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO incisive.evidence_records \
         (topic_id, version, source_run_id, question, answer, answer_version, board, \
          era_table, evidence_through, topic_label) \
         VALUES ($1, 1, $2, 'robotic hernia?', '{}'::jsonb, 3, '{}'::jsonb, '{}'::jsonb, \
                 CURRENT_DATE, 'Hernia') RETURNING id",
    )
    .bind(topic)
    .bind(run)
    .fetch_one(pool)
    .await
    .unwrap()
}

#[tokio::test]
async fn memory_tables_exist() {
    let Some(pool) = connect().await else { return };
    for t in TABLES {
        let n: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM information_schema.tables \
             WHERE table_schema = 'incisive' AND table_name = $1",
        )
        .bind(t)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(n, 1, "missing table {t}");
    }
}

#[tokio::test]
async fn record_survives_run_delete() {
    let Some(pool) = connect().await else { return };
    let (_, run) = seed_run(&pool).await;
    let topic = seed_topic(&pool).await;
    let rec = seed_record(&pool, topic, run).await;
    sqlx::query("DELETE FROM incisive.runs WHERE id = $1")
        .bind(run)
        .execute(&pool)
        .await
        .unwrap();
    let src: Option<Uuid> =
        sqlx::query_scalar("SELECT source_run_id FROM incisive.evidence_records WHERE id = $1")
            .bind(rec)
            .fetch_one(&pool)
            .await
            .expect("record still exists");
    assert!(src.is_none());
}

#[tokio::test]
async fn memory_job_cascades_with_run() {
    let Some(pool) = connect().await else { return };
    let (_, run) = seed_run(&pool).await;
    sqlx::query("INSERT INTO incisive.memory_jobs (run_id) VALUES ($1)")
        .bind(run)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM incisive.runs WHERE id = $1")
        .bind(run)
        .execute(&pool)
        .await
        .unwrap();
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM incisive.memory_jobs WHERE run_id = $1")
        .bind(run)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(n, 0);
}

#[tokio::test]
async fn rls_enabled() {
    let Some(pool) = connect().await else { return };
    for t in TABLES {
        let on: bool = sqlx::query_scalar(
            "SELECT c.relrowsecurity FROM pg_class c \
             JOIN pg_namespace n ON n.oid = c.relnamespace \
             WHERE n.nspname = 'incisive' AND c.relname = $1",
        )
        .bind(t)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(on, "RLS off on {t}");
    }
}

#[tokio::test]
async fn verdict_check_rejects_unknown() {
    let Some(pool) = connect().await else { return };
    let (user, _) = seed_run(&pool).await;
    let topic = seed_topic(&pool).await;
    let insert = |v: &'static str| {
        let pool = pool.clone();
        async move {
            sqlx::query(
                "INSERT INTO incisive.topic_verdicts (topic_id, verdict, note, author_user_id) \
                 VALUES ($1, $2, '', $3)",
            )
            .bind(topic)
            .bind(v)
            .bind(user)
            .execute(&pool)
            .await
        }
    };
    assert!(insert("maybe").await.is_err());
    insert("selective").await.expect("valid verdict accepted");
}
