//! Migration 0014 against the database (`TEST_DATABASE_URL`).

use incisive_app::db::{lazy_pool, migrate};
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

async fn seed_user(pool: &PgPool) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO incisive.users (email, display_name, role, daily_cap_paise) \
         VALUES ($1, 'Chat', 'surgeon', 100000) RETURNING id",
    )
    .bind(format!("chat-{}@example.test", Uuid::new_v4()))
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn seed_run(pool: &PgPool, user: i64) -> Uuid {
    let run = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO incisive.runs (id, user_id, question, mode, status, limit_paise) \
         VALUES ($1, $2, 'q', 'focused', 'done', 1000)",
    )
    .bind(run)
    .bind(user)
    .execute(pool)
    .await
    .unwrap();
    run
}

async fn seed_record(pool: &PgPool, run: Uuid) -> i64 {
    let topic: i64 = sqlx::query_scalar(
        "INSERT INTO incisive.topics (label, pairs) VALUES ($1, '[]'::jsonb) RETURNING id",
    )
    .bind(format!("topic {}", Uuid::new_v4()))
    .fetch_one(pool)
    .await
    .unwrap();
    sqlx::query_scalar(
        "INSERT INTO incisive.evidence_records \
         (topic_id, version, source_run_id, question, answer, answer_version, board, era_table, \
          evidence_through, topic_label) \
         VALUES ($1, 1, $2, 'q', $3, 3, $3, $3, '2026-01-01', 'L') RETURNING id",
    )
    .bind(topic)
    .bind(run)
    .bind(json!({}))
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn seed_conversation(pool: &PgPool, run: Uuid, user: i64) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO incisive.chat_conversations (run_id, user_id, mode) \
         VALUES ($1, $2, 'clinical') RETURNING id",
    )
    .bind(run)
    .bind(user)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn add_turn(
    pool: &PgPool,
    conv: i64,
    user: i64,
    seq: i32,
    status: &str,
) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar(
        "INSERT INTO incisive.chat_turns (conversation_id, user_id, seq, question, mode, status) \
         VALUES ($1, $2, $3, 'q', 'clinical', $4) RETURNING id",
    )
    .bind(conv)
    .bind(user)
    .bind(seq)
    .bind(status)
    .fetch_one(pool)
    .await
}

#[tokio::test]
async fn migration_applies_and_tables_exist() {
    let Some(pool) = connect().await else { return };
    for t in [
        "chat_passages",
        "chat_prep",
        "topic_qas",
        "chat_conversations",
        "chat_turns",
        "chat_priors",
    ] {
        let rls: bool = sqlx::query_scalar(
            "SELECT c.relrowsecurity FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace \
             WHERE n.nspname = 'incisive' AND c.relname = $1",
        )
        .bind(t)
        .fetch_one(&pool)
        .await
        .unwrap_or_else(|e| panic!("{t} missing: {e}"));
        assert!(rls, "{t} has row-level security on");
    }
    for (t, c) in [
        ("evidence_records", "embed_attempts"),
        ("chat_passages", "embed_attempts"),
        ("topic_qas", "embed_attempts"),
    ] {
        let found: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM information_schema.columns \
             WHERE table_schema = 'incisive' AND table_name = $1 AND column_name = $2)",
        )
        .bind(t)
        .bind(c)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(found, "{t}.{c}");
    }
    let def: String = sqlx::query_scalar(
        "SELECT indexdef FROM pg_indexes \
         WHERE schemaname = 'incisive' AND indexname = 'chat_turns_user_created_idx'",
    )
    .fetch_one(&pool)
    .await
    .expect("daily-limit index");
    assert!(def.contains("(user_id, created_at)"), "{def}");
}

#[tokio::test]
async fn second_pending_turn_for_user_is_refused() {
    let Some(pool) = connect().await else { return };
    let user = seed_user(&pool).await;
    let run = seed_run(&pool, user).await;
    let conv = seed_conversation(&pool, run, user).await;
    add_turn(&pool, conv, user, 1, "pending").await.unwrap();
    let err = add_turn(&pool, conv, user, 2, "pending").await.unwrap_err();
    let code = err
        .as_database_error()
        .and_then(|d| d.code().map(|c| c.to_string()));
    assert_eq!(code.as_deref(), Some("23505"));
    // Once the first is resolved, another may start.
    sqlx::query("UPDATE incisive.chat_turns SET status = 'done' WHERE conversation_id = $1")
        .bind(conv)
        .execute(&pool)
        .await
        .unwrap();
    add_turn(&pool, conv, user, 2, "pending").await.unwrap();
}

#[tokio::test]
async fn run_delete_cascades_conversations_and_turns_but_keeps_passages() {
    let Some(pool) = connect().await else { return };
    let user = seed_user(&pool).await;
    let run = seed_run(&pool, user).await;
    let record = seed_record(&pool, run).await;
    let passage: i64 = sqlx::query_scalar(
        "INSERT INTO incisive.chat_passages (record_id, kind, seq, text) \
         VALUES ($1, 'outcome', 0, 'text') RETURNING id",
    )
    .bind(record)
    .fetch_one(&pool)
    .await
    .unwrap();
    let conv = seed_conversation(&pool, run, user).await;
    let turn = add_turn(&pool, conv, user, 1, "done").await.unwrap();

    sqlx::query("DELETE FROM incisive.runs WHERE id = $1")
        .bind(run)
        .execute(&pool)
        .await
        .unwrap();

    let convs: i64 =
        sqlx::query_scalar("SELECT count(*) FROM incisive.chat_conversations WHERE id = $1")
            .bind(conv)
            .fetch_one(&pool)
            .await
            .unwrap();
    let turns: i64 = sqlx::query_scalar("SELECT count(*) FROM incisive.chat_turns WHERE id = $1")
        .bind(turn)
        .fetch_one(&pool)
        .await
        .unwrap();
    let passages: i64 =
        sqlx::query_scalar("SELECT count(*) FROM incisive.chat_passages WHERE id = $1")
            .bind(passage)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!((convs, turns, passages), (0, 0, 1));
}

#[tokio::test]
async fn run_delete_nulls_prior_and_qa_links() {
    let Some(pool) = connect().await else { return };
    let user = seed_user(&pool).await;
    let run = seed_run(&pool, user).await;
    let record = seed_record(&pool, run).await;
    let topic: i64 =
        sqlx::query_scalar("SELECT topic_id FROM incisive.evidence_records WHERE id = $1")
            .bind(record)
            .fetch_one(&pool)
            .await
            .unwrap();
    let conv = seed_conversation(&pool, run, user).await;
    let turn = add_turn(&pool, conv, user, 1, "done").await.unwrap();
    let qa: i64 = sqlx::query_scalar(
        "INSERT INTO incisive.topic_qas (topic_id, source_turn_id, question, gemma_reply, \
         author_user_id) VALUES ($1, $2, 'q', 'r', $3) RETURNING id",
    )
    .bind(topic)
    .bind(turn)
    .bind(user)
    .fetch_one(&pool)
    .await
    .unwrap();
    let prior: i64 = sqlx::query_scalar(
        "INSERT INTO incisive.chat_priors (run_id, record_id, status) \
         VALUES ($1, $2, 'written') RETURNING id",
    )
    .bind(run)
    .bind(record)
    .fetch_one(&pool)
    .await
    .unwrap();

    sqlx::query("DELETE FROM incisive.runs WHERE id = $1")
        .bind(run)
        .execute(&pool)
        .await
        .unwrap();

    let qa_turn: Option<i64> =
        sqlx::query_scalar("SELECT source_turn_id FROM incisive.topic_qas WHERE id = $1")
            .bind(qa)
            .fetch_one(&pool)
            .await
            .unwrap();
    let prior_run: Option<Uuid> =
        sqlx::query_scalar("SELECT run_id FROM incisive.chat_priors WHERE id = $1")
            .bind(prior)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!((qa_turn, prior_run), (None, None));
}
