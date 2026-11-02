//! The memory job (`memory::job`) against the database (`TEST_DATABASE_URL`).

use std::sync::Arc;

use incisive_app::agent::temporal_writer::TemporalSettings;
use incisive_app::agent::testing::{offline_literature, placeholder_pipeline};
use incisive_app::db::{lazy_pool, migrate};
use incisive_app::gate::OfflineClassifier;
use incisive_app::identity::{IapVerifier, JwkSource};
use incisive_app::memory::job::{self, JobOutcome};
use incisive_app::memory::settings::MemorySettings;
use incisive_app::memory::store;
use incisive_app::runs::RunsConfig;
use incisive_app::runs::worker::WorkerHandle;
use incisive_app::server::AppState;
use incisive_app::spend::{CapsConfig, Prices};
use secrecy::SecretString;
use serde_json::Value;
use sqlx::PgPool;
use uuid::Uuid;

static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// The tests share one database and topic matching is global, so they run
/// one at a time on an empty memory store.
async fn connect() -> Option<PgPool> {
    let Ok(url) = std::env::var("TEST_DATABASE_URL") else {
        eprintln!("SKIP: TEST_DATABASE_URL not set");
        return None;
    };
    let pool = lazy_pool(&SecretString::from(url)).expect("lazy pool");
    migrate(&pool).await.expect("migrate");
    Some(pool)
}

async fn begin() -> Option<(tokio::sync::MutexGuard<'static, ()>, PgPool)> {
    let guard = SERIAL.lock().await;
    let pool = connect().await?;
    sqlx::query(
        "TRUNCATE incisive.memory_jobs, incisive.ledger_claims, incisive.ledger_studies, \
         incisive.evidence_records, incisive.topics CASCADE",
    )
    .execute(&pool)
    .await
    .unwrap();
    Some((guard, pool))
}

fn state(pool: PgPool, on: bool) -> AppState {
    AppState {
        pool,
        verifier: Arc::new(IapVerifier::new(
            "aud".into(),
            JwkSource::Static(serde_json::from_str(r#"{"keys":[]}"#).unwrap()),
        )),
        gate: Arc::new(OfflineClassifier),
        prices: Prices::from_json(r#"{"m-test": {"input": 1.0, "output": 5.0}}"#).unwrap(),
        usd_inr: 88.0,
        caps: CapsConfig {
            global_monthly_paise: i64::MAX / 4,
        },
        gate_model: "m-test".into(),
        dev_mode: true,
        runs: RunsConfig::default(),
        quick_engine: incisive_app::runs::QuickEngine::Classic,
        full_engine: incisive_app::runs::Engine::Classic,
        completeness: false,
        temporal: TemporalSettings::default(),
        temporal_client: None,
        memory: MemorySettings {
            on,
            ..MemorySettings::default()
        },
        embedder: None,
        worker: WorkerHandle::with_pipeline(placeholder_pipeline),
        llm: None,
        lit: offline_literature(),
    }
}

struct Fx {
    body: &'static str,
    slim: &'static str,
    board: &'static str,
}

const HERNIA: Fx = Fx {
    body: include_str!("fixtures/temporal/full/hernia.json"),
    slim: include_str!("fixtures/temporal/hernia.json"),
    board: include_str!("fixtures/temporal/boards/hernia-board.json"),
};
const ACHALASIA: Fx = Fx {
    body: include_str!("fixtures/temporal/full/achalasia.json"),
    slim: include_str!("fixtures/temporal/achalasia.json"),
    board: include_str!("fixtures/temporal/boards/achalasia-board.json"),
};

/// Seeds a done run of `mode` with its answer and a `done` temporal row.
async fn seed(pool: &PgPool, fx: &Fx, mode: &str, temporal_status: Option<&str>) -> Uuid {
    let email = format!("mjob-{}@example.test", Uuid::new_v4());
    let user: i64 = sqlx::query_scalar(
        "INSERT INTO incisive.users (email, display_name, role, daily_cap_paise) \
         VALUES ($1, 'M', 'surgeon', 100000) RETURNING id",
    )
    .bind(&email)
    .fetch_one(pool)
    .await
    .unwrap();
    let run = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO incisive.runs (id, user_id, question, mode, status, limit_paise) \
         VALUES ($1, $2, 'Is robotic better?', $3, 'done', 1000)",
    )
    .bind(run)
    .bind(user)
    .bind(mode)
    .execute(pool)
    .await
    .unwrap();
    let body: Value = serde_json::from_str(fx.body).unwrap();
    sqlx::query(
        "INSERT INTO incisive.run_answers (run_id, body, answer_version) VALUES ($1, $2, 3)",
    )
    .bind(run)
    .bind(&body)
    .execute(pool)
    .await
    .unwrap();
    if let Some(st) = temporal_status {
        let input = incisive_app::temporal::TemporalInput::from_json(fx.slim).unwrap();
        let table = serde_json::to_value(incisive_app::temporal::build(&input)).unwrap();
        let wrapped: Value = serde_json::from_str(fx.board).unwrap();
        let board = wrapped["board"].clone();
        sqlx::query(
            "INSERT INTO incisive.run_temporal (run_id, status, table_json, board_json) \
             VALUES ($1, $2, $3, $4)",
        )
        .bind(run)
        .bind(st)
        .bind(&table)
        .bind(&board)
        .execute(pool)
        .await
        .unwrap();
    }
    run
}

async fn job_row_count(pool: &PgPool, run: Uuid) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM incisive.memory_jobs WHERE run_id = $1")
        .bind(run)
        .fetch_one(pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn hernia_run_becomes_record() {
    let Some((_g, pool)) = begin().await else {
        return;
    };
    let st = state(pool.clone(), true);
    let run = seed(&pool, &HERNIA, "focused", Some("done")).await;
    let JobOutcome::Done {
        record_id,
        new_topic,
        ..
    } = job::process(&st, run).await
    else {
        panic!("expected Done");
    };
    assert!(new_topic);
    let stored: String =
        sqlx::query_scalar("SELECT answer::text FROM incisive.evidence_records WHERE id = $1")
            .bind(record_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    let body: Value = sqlx::query_scalar("SELECT body FROM incisive.run_answers WHERE run_id = $1")
        .bind(run)
        .fetch_one(&pool)
        .await
        .unwrap();
    let stored: Value = serde_json::from_str(&stored).unwrap();
    assert_eq!(stored, body);
    let through: chrono::NaiveDate =
        sqlx::query_scalar("SELECT evidence_through FROM incisive.evidence_records WHERE id = $1")
            .bind(record_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    let created: chrono::DateTime<chrono::Utc> =
        sqlx::query_scalar("SELECT created_at FROM incisive.runs WHERE id = $1")
            .bind(run)
            .fetch_one(&pool)
            .await
            .unwrap();
    let ist = chrono::FixedOffset::east_opt(19800).unwrap();
    assert_eq!(through, created.with_timezone(&ist).date_naive());
    // Idempotent: a second call returns the same record and inserts nothing.
    let again = job::process(&st, run).await;
    assert_eq!(
        again,
        JobOutcome::Done {
            record_id,
            topic_id: sqlx::query_scalar(
                "SELECT topic_id FROM incisive.evidence_records WHERE id = $1"
            )
            .bind(record_id)
            .fetch_one(&pool)
            .await
            .unwrap(),
            new_topic: false
        }
    );
    let n: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM incisive.evidence_records WHERE source_run_id = $1",
    )
    .bind(run)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(n, 1);
}

#[tokio::test]
async fn same_topic_second_run_joins() {
    let Some((_g, pool)) = begin().await else {
        return;
    };
    let st = state(pool.clone(), true);
    let r1 = seed(&pool, &HERNIA, "focused", Some("done")).await;
    let JobOutcome::Done { topic_id: t1, .. } = job::process(&st, r1).await else {
        panic!("first");
    };
    let r2 = seed(&pool, &HERNIA, "focused", Some("done")).await;
    let JobOutcome::Done {
        record_id,
        topic_id: t2,
        new_topic,
    } = job::process(&st, r2).await
    else {
        panic!("second");
    };
    assert!(!new_topic);
    assert_eq!(t1, t2);
    let v: i32 = sqlx::query_scalar("SELECT version FROM incisive.evidence_records WHERE id = $1")
        .bind(record_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(v, 2);
}

#[tokio::test]
async fn different_topic_new() {
    let Some((_g, pool)) = begin().await else {
        return;
    };
    let st = state(pool.clone(), true);
    let r1 = seed(&pool, &HERNIA, "focused", Some("done")).await;
    let JobOutcome::Done { topic_id: t1, .. } = job::process(&st, r1).await else {
        panic!("first");
    };
    let r2 = seed(&pool, &ACHALASIA, "focused", Some("done")).await;
    let JobOutcome::Done {
        topic_id: t2,
        new_topic,
        ..
    } = job::process(&st, r2).await
    else {
        panic!("second");
    };
    assert_ne!(t1, t2);
    assert!(new_topic);
}

#[tokio::test]
async fn quick_run_skipped() {
    let Some((_g, pool)) = begin().await else {
        return;
    };
    let st = state(pool.clone(), true);
    let run = seed(&pool, &HERNIA, "quick", Some("done")).await;
    assert_eq!(
        job::process(&st, run).await,
        JobOutcome::Skipped("not_full")
    );
}

#[tokio::test]
async fn no_opinion_skipped() {
    let Some((_g, pool)) = begin().await else {
        return;
    };
    let st = state(pool.clone(), true);
    let none = seed(&pool, &HERNIA, "focused", None).await;
    assert_eq!(
        job::process(&st, none).await,
        JobOutcome::Skipped("no_opinion")
    );
    let failed = seed(&pool, &HERNIA, "focused", Some("failed")).await;
    assert_eq!(
        job::process(&st, failed).await,
        JobOutcome::Skipped("no_opinion")
    );
}

async fn snapshot(pool: &PgPool, run: Uuid) -> (String, String) {
    let row: String =
        sqlx::query_scalar("SELECT row_to_json(r)::text FROM incisive.runs r WHERE r.id = $1")
            .bind(run)
            .fetch_one(pool)
            .await
            .unwrap();
    let body: String =
        sqlx::query_scalar("SELECT body::text FROM incisive.run_answers WHERE run_id = $1")
            .bind(run)
            .fetch_one(pool)
            .await
            .unwrap();
    (row, body)
}

#[tokio::test]
async fn run_untouched() {
    let Some((_g, pool)) = begin().await else {
        return;
    };
    let st = state(pool.clone(), true);
    let run = seed(&pool, &HERNIA, "focused", Some("done")).await;
    let before = snapshot(&pool, run).await;
    assert!(matches!(
        job::process(&st, run).await,
        JobOutcome::Done { .. }
    ));
    assert_eq!(before, snapshot(&pool, run).await);
}

#[tokio::test]
async fn run_deleted_mid_flight() {
    let Some((_g, pool)) = begin().await else {
        return;
    };
    let st = state(pool.clone(), true);
    let run = seed(&pool, &HERNIA, "focused", Some("done")).await;
    assert!(store::enqueue_job(&pool, run).await.unwrap());
    let claimed = store::claim_jobs(&pool, 1000).await.unwrap();
    assert!(claimed.contains(&run));
    sqlx::query("DELETE FROM incisive.runs WHERE id = $1")
        .bind(run)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(job::process(&st, run).await, JobOutcome::Failed("run_gone"));
    let n: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM incisive.evidence_records WHERE source_run_id = $1",
    )
    .bind(run)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(n, 0);
}

#[tokio::test]
async fn trigger_only_when_on() {
    let Some((_g, pool)) = begin().await else {
        return;
    };
    let run = seed(&pool, &HERNIA, "focused", Some("done")).await;
    job::on_temporal_done(&state(pool.clone(), false), run).await;
    assert_eq!(job_row_count(&pool, run).await, 0);
    job::on_temporal_done(&state(pool.clone(), true), run).await;
    job::on_temporal_done(&state(pool.clone(), true), run).await;
    assert_eq!(job_row_count(&pool, run).await, 1);
}

#[tokio::test]
async fn concurrent_process_yields_one_record() {
    let Some((_g, pool)) = begin().await else {
        return;
    };
    let st = state(pool.clone(), true);
    let run = seed(&pool, &HERNIA, "focused", Some("done")).await;
    let (a, b) = tokio::join!(job::process(&st, run), job::process(&st, run));
    let (JobOutcome::Done { record_id: ra, .. }, JobOutcome::Done { record_id: rb, .. }) = (a, b)
    else {
        panic!("both must be Done");
    };
    assert_eq!(ra, rb);
    let n: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM incisive.evidence_records WHERE source_run_id = $1",
    )
    .bind(run)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(n, 1);
    let t: i64 = sqlx::query_scalar("SELECT count(*) FROM incisive.topics")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(t, 1, "the losing transaction's topic is rolled back");
}

#[tokio::test]
async fn store_failure_retries_until_attempts_spent() {
    let Some((_g, pool)) = begin().await else {
        return;
    };
    let run = seed(&pool, &HERNIA, "focused", Some("done")).await;
    store::enqueue_job(&pool, run).await.unwrap();
    for attempt in 1..=3 {
        let claimed = store::claim_jobs(&pool, 10).await.unwrap();
        assert!(claimed.contains(&run), "attempt {attempt} claimable");
        store::retry_or_fail_job(&pool, run, "store").await.unwrap();
        let (status, kind): (String, Option<String>) = sqlx::query_as(
            "SELECT status, last_error_kind FROM incisive.memory_jobs WHERE run_id = $1",
        )
        .bind(run)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(kind.as_deref(), Some("store"));
        assert_eq!(status, if attempt < 3 { "pending" } else { "failed" });
    }
    assert!(!store::claim_jobs(&pool, 10).await.unwrap().contains(&run));
}

#[tokio::test]
async fn backfill_enqueues_once() {
    let Some((_g, pool)) = begin().await else {
        return;
    };
    let eligible_a = seed(&pool, &HERNIA, "focused", Some("done")).await;
    let eligible_b = seed(&pool, &ACHALASIA, "focused", Some("done")).await;
    let quick = seed(&pool, &HERNIA, "quick", Some("done")).await;
    let no_opinion = seed(&pool, &HERNIA, "focused", None).await;
    let failed = seed(&pool, &HERNIA, "focused", Some("failed")).await;
    let queued = job::backfill(&pool).await.unwrap();
    assert!(queued >= 2, "queued {queued}");
    assert_eq!(job_row_count(&pool, eligible_a).await, 1);
    assert_eq!(job_row_count(&pool, eligible_b).await, 1);
    assert_eq!(job_row_count(&pool, quick).await, 0);
    assert_eq!(job_row_count(&pool, no_opinion).await, 0);
    assert_eq!(job_row_count(&pool, failed).await, 0);
    // The first call queued every eligible run, so a second call adds none.
    assert_eq!(job::backfill(&pool).await.unwrap(), 0);
    assert_eq!(job_row_count(&pool, eligible_a).await, 1);
    assert_eq!(job_row_count(&pool, eligible_b).await, 1);
}

fn embed_state(pool: PgPool, uri: &str) -> AppState {
    let settings = MemorySettings {
        on: true,
        gemma_url: Some(uri.to_string()),
        gemma_auth: Some(SecretString::from("incisive:pw")),
        ..MemorySettings::default()
    };
    let mut st = state(pool, true);
    st.embedder = Some(Arc::new(
        incisive_app::memory::embed::Embedder::from_settings(&settings).unwrap(),
    ));
    st.memory = settings;
    st
}

#[tokio::test]
async fn record_embedded_after_job() {
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};
    let Some((_g, pool)) = begin().await else {
        return;
    };
    let server = MockServer::start().await;
    let ok = Mock::given(method("POST"))
        .and(path("/api/embed"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({ "embeddings": [vec![0.25f32; 768]] })),
        )
        .mount_as_scoped(&server)
        .await;
    let st = embed_state(pool.clone(), &server.uri());
    let run = seed(&pool, &HERNIA, "focused", Some("done")).await;
    let JobOutcome::Done {
        record_id,
        topic_id,
        new_topic,
    } = job::process(&st, run).await
    else {
        panic!("expected Done");
    };
    assert!(new_topic);
    let rec: bool = sqlx::query_scalar(
        "SELECT embedding IS NOT NULL FROM incisive.evidence_records WHERE id = $1",
    )
    .bind(record_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    let top: bool =
        sqlx::query_scalar("SELECT embedding IS NOT NULL FROM incisive.topics WHERE id = $1")
            .bind(topic_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(rec && top);
    drop(ok);

    // Server down (500): the job still succeeds, the embedding stays NULL,
    // and the sweep fills it once the server recovers.
    server.reset().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;
    let run2 = seed(&pool, &ACHALASIA, "focused", Some("done")).await;
    let JobOutcome::Done { record_id: r2, .. } = job::process(&st, run2).await else {
        panic!("expected Done");
    };
    let null: bool =
        sqlx::query_scalar("SELECT embedding IS NULL FROM incisive.evidence_records WHERE id = $1")
            .bind(r2)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(null);
    assert_eq!(job::sweep_embeddings(&st).await, 0);
    server.reset().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({ "embeddings": [vec![0.25f32; 768]] })),
        )
        .mount(&server)
        .await;
    assert_eq!(job::sweep_embeddings(&st).await, 1);
    let filled: bool = sqlx::query_scalar(
        "SELECT embedding IS NOT NULL FROM incisive.evidence_records WHERE id = $1",
    )
    .bind(r2)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(filled);
}

#[tokio::test]
async fn inline_and_sweep_send_identical_input() {
    use wiremock::matchers::{body_partial_json, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};
    let Some((_g, pool)) = begin().await else {
        return;
    };
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/embed"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;
    let st = embed_state(pool.clone(), &server.uri());
    let run = seed(&pool, &HERNIA, "focused", Some("done")).await;
    let JobOutcome::Done { record_id, .. } = job::process(&st, run).await else {
        panic!("expected Done");
    };
    let text: String =
        sqlx::query_scalar("SELECT embed_text FROM incisive.evidence_records WHERE id = $1")
            .bind(record_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(!text.is_empty());
    // The inline attempt (500) already sent the same text the sweep will.
    let first = server.received_requests().await.unwrap();
    let sent: Value = serde_json::from_slice(&first[0].body).unwrap();
    assert_eq!(sent["input"], text);
    server.reset().await;
    let m = Mock::given(method("POST"))
        .and(path("/api/embed"))
        .and(body_partial_json(serde_json::json!({ "input": text })))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({ "embeddings": [vec![0.25f32; 768]] })),
        )
        .expect(1)
        .mount_as_scoped(&server)
        .await;
    assert_eq!(job::sweep_embeddings(&st).await, 1);
    drop(m);
}
