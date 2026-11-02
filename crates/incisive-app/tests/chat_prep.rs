//! The chat loop (`chat::job`) against the database (`TEST_DATABASE_URL`):
//! passage prep, the vector sweep and stale pending turns.

use std::sync::Arc;

use incisive_app::agent::temporal_writer::TemporalSettings;
use incisive_app::agent::testing::{offline_literature, placeholder_pipeline};
use incisive_app::chat::{job, store};
use incisive_app::db::{lazy_pool, migrate};
use incisive_app::gate::OfflineClassifier;
use incisive_app::identity::{IapVerifier, JwkSource};
use incisive_app::memory::job as memory_job;
use incisive_app::memory::settings::MemorySettings;
use incisive_app::runs::RunsConfig;
use incisive_app::runs::worker::WorkerHandle;
use incisive_app::server::AppState;
use incisive_app::spend::{CapsConfig, Prices};
use secrecy::SecretString;
use serde_json::Value;
use sqlx::PgPool;
use uuid::Uuid;

static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

async fn begin() -> Option<(tokio::sync::MutexGuard<'static, ()>, PgPool)> {
    let guard = SERIAL.lock().await;
    let Ok(url) = std::env::var("TEST_DATABASE_URL") else {
        eprintln!("SKIP: TEST_DATABASE_URL not set");
        return None;
    };
    let pool = lazy_pool(&SecretString::from(url)).expect("lazy pool");
    migrate(&pool).await.expect("migrate");
    sqlx::query(
        "TRUNCATE incisive.chat_passages, incisive.chat_prep, incisive.chat_turns, \
         incisive.chat_conversations, incisive.memory_jobs, incisive.ledger_claims, \
         incisive.ledger_studies, incisive.evidence_records, incisive.topics CASCADE",
    )
    .execute(&pool)
    .await
    .unwrap();
    Some((guard, pool))
}

fn state(pool: PgPool) -> AppState {
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
        chat: incisive_app::chat::settings::ChatSettings {
            on: true,
            ..Default::default()
        },
        memory: MemorySettings {
            on: true,
            ..MemorySettings::default()
        },
        embedder: None,
        worker: WorkerHandle::with_pipeline(placeholder_pipeline),
        llm: None,
        lit: offline_literature(),
    }
}

fn embed_state(pool: PgPool, uri: &str) -> AppState {
    let settings = MemorySettings {
        on: true,
        gemma_url: Some(uri.to_string()),
        gemma_auth: Some(SecretString::from("incisive:pw")),
        ..MemorySettings::default()
    };
    let mut st = state(pool);
    st.embedder = Some(Arc::new(
        incisive_app::memory::embed::Embedder::from_settings(&settings).unwrap(),
    ));
    st.memory = settings;
    st
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

async fn seed_user(pool: &PgPool) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO incisive.users (email, display_name, role, daily_cap_paise) \
         VALUES ($1, 'C', 'surgeon', 100000) RETURNING id",
    )
    .bind(format!("cprep-{}@example.test", Uuid::new_v4()))
    .fetch_one(pool)
    .await
    .unwrap()
}

/// A real evidence record, made by the memory job from a seeded run.
async fn seed_record(pool: &PgPool, fx: &Fx) -> (Uuid, i64) {
    let user = seed_user(pool).await;
    let run = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO incisive.runs (id, user_id, question, mode, status, limit_paise) \
         VALUES ($1, $2, 'Is robotic better?', 'focused', 'done', 1000)",
    )
    .bind(run)
    .bind(user)
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
    let input = incisive_app::temporal::TemporalInput::from_json(fx.slim).unwrap();
    let table = serde_json::to_value(incisive_app::temporal::build(&input)).unwrap();
    let wrapped: Value = serde_json::from_str(fx.board).unwrap();
    sqlx::query(
        "INSERT INTO incisive.run_temporal (run_id, status, table_json, board_json) \
         VALUES ($1, 'done', $2, $3)",
    )
    .bind(run)
    .bind(&table)
    .bind(&wrapped["board"])
    .execute(pool)
    .await
    .unwrap();
    let st = state(pool.clone());
    let memory_job::JobOutcome::Done { record_id, .. } = memory_job::process(&st, run).await else {
        panic!("memory job did not record the run");
    };
    (run, record_id)
}

async fn passage_count(pool: &PgPool, record: i64) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM incisive.chat_passages WHERE record_id = $1")
        .bind(record)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn prep_row(pool: &PgPool, record: i64) -> Option<(String, i32, Option<String>)> {
    sqlx::query_as(
        "SELECT status, passages, error_kind FROM incisive.chat_prep WHERE record_id = $1",
    )
    .bind(record)
    .fetch_optional(pool)
    .await
    .unwrap()
}

#[tokio::test]
async fn tick_prepares_existing_records_and_is_idempotent() {
    let Some((_g, pool)) = begin().await else {
        return;
    };
    let st = state(pool.clone());
    let (_, a) = seed_record(&pool, &HERNIA).await;
    let (_, b) = seed_record(&pool, &ACHALASIA).await;
    assert_eq!(
        store::records_needing_prep(&pool, 5).await.unwrap().len(),
        2
    );
    job::tick(&st).await;
    for r in [a, b] {
        let (status, n, err) = prep_row(&pool, r).await.expect("prep row");
        assert_eq!(status, "ok");
        assert!(err.is_none());
        assert!(n > 5, "passages {n}");
        assert_eq!(passage_count(&pool, r).await, i64::from(n));
    }
    assert!(
        store::records_needing_prep(&pool, 5)
            .await
            .unwrap()
            .is_empty()
    );
    let before = passage_count(&pool, a).await;
    job::tick(&st).await;
    job::tick(&st).await;
    assert_eq!(passage_count(&pool, a).await, before);
}

#[tokio::test]
async fn bad_record_gets_failed_prep_and_others_still_run() {
    let Some((_g, pool)) = begin().await else {
        return;
    };
    let st = state(pool.clone());
    let (_, bad) = seed_record(&pool, &HERNIA).await;
    sqlx::query("UPDATE incisive.evidence_records SET answer = '{\"x\": 1}'::jsonb WHERE id = $1")
        .bind(bad)
        .execute(&pool)
        .await
        .unwrap();
    let (_, good) = seed_record(&pool, &ACHALASIA).await;
    job::tick(&st).await;
    let (status, n, err) = prep_row(&pool, bad).await.expect("failed row");
    assert_eq!(status, "failed");
    assert_eq!(n, 0);
    let code = err.expect("a code");
    assert!(
        !code.is_empty() && code.len() < 32 && !code.contains(' '),
        "{code}"
    );
    assert_eq!(passage_count(&pool, bad).await, 0);
    assert_eq!(prep_row(&pool, good).await.unwrap().0, "ok");
    assert!(passage_count(&pool, good).await > 0);
    // A failed record is not retried every tick.
    assert!(
        store::records_needing_prep(&pool, 5)
            .await
            .unwrap()
            .is_empty()
    );
}

/// First reference pmid of the hernia answer and that paper's abstract rows.
fn hernia_pmid() -> String {
    let b: Value = serde_json::from_str(HERNIA.body).unwrap();
    b["references"][0]["pmid"].as_str().unwrap().to_string()
}

async fn cache_abstract(pool: &PgPool, pmid: &str, days_old: i32) {
    sqlx::query(
        "INSERT INTO incisive.paper_cache (pmid, title, authors, pub_types, abstract, fetched_at) \
         VALUES ($1, 't', '[]', '{}', $2, now() - make_interval(days => $3)) \
         ON CONFLICT (pmid) DO UPDATE SET abstract = EXCLUDED.abstract, fetched_at = EXCLUDED.fetched_at",
    )
    .bind(pmid)
    .bind("Background: a unique zebra-striped baseline. Methods: we compared meshes. Results: recurrence fell. Conclusion: mesh helps.")
    .bind(days_old)
    .execute(pool)
    .await
    .unwrap();
}

async fn abstract_passages(pool: &PgPool, record: i64, pmid: &str) -> i64 {
    sqlx::query_scalar(
        "SELECT count(*) FROM incisive.chat_passages \
         WHERE record_id = $1 AND kind = 'abstract' AND pmid = $2",
    )
    .bind(record)
    .bind(pmid)
    .fetch_one(pool)
    .await
    .unwrap()
}

#[tokio::test]
async fn abstract_older_than_30_days_is_still_copied() {
    let Some((_g, pool)) = begin().await else {
        return;
    };
    let st = state(pool.clone());
    let (_, rec) = seed_record(&pool, &HERNIA).await;
    let pmid = hernia_pmid();
    cache_abstract(&pool, &pmid, 60).await;
    // The shared cache read hides it, which is why prep reads the table itself.
    let hidden = incisive_app::lit::cache::get_many(&pool, std::slice::from_ref(&pmid))
        .await
        .unwrap();
    assert!(hidden.is_empty());
    let got = store::read_abstracts(&pool, std::slice::from_ref(&pmid))
        .await
        .unwrap();
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].0, pmid);
    job::tick(&st).await;
    assert!(abstract_passages(&pool, rec, &pmid).await >= 1);
}

#[tokio::test]
async fn passages_survive_paper_cache_prune() {
    let Some((_g, pool)) = begin().await else {
        return;
    };
    let st = state(pool.clone());
    let (_, rec) = seed_record(&pool, &HERNIA).await;
    let pmid = hernia_pmid();
    cache_abstract(&pool, &pmid, 80).await;
    job::tick(&st).await;
    let before = abstract_passages(&pool, rec, &pmid).await;
    assert!(before >= 1);
    cache_abstract(&pool, &pmid, 100).await;
    assert!(incisive_app::lit::cache::prune(&pool).await.unwrap() >= 1);
    assert!(
        store::read_abstracts(&pool, std::slice::from_ref(&pmid))
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(abstract_passages(&pool, rec, &pmid).await, before);
}

#[tokio::test]
async fn two_ticks_at_once_give_one_set_of_passages() {
    let Some((_g, pool)) = begin().await else {
        return;
    };
    let st = state(pool.clone());
    let (_, a) = seed_record(&pool, &HERNIA).await;
    let (_, b) = seed_record(&pool, &ACHALASIA).await;
    let (x, y) = tokio::join!(job::tick(&st), job::tick(&st));
    let _ = (x, y);
    for r in [a, b] {
        let (status, n, _) = prep_row(&pool, r).await.expect("prep row");
        assert_eq!(status, "ok");
        assert_eq!(passage_count(&pool, r).await, i64::from(n));
        let distinct: i64 = sqlx::query_scalar(
            "SELECT count(DISTINCT (kind, seq)) FROM incisive.chat_passages WHERE record_id = $1",
        )
        .bind(r)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(distinct, i64::from(n));
    }
}

fn embed_ok() -> wiremock::ResponseTemplate {
    wiremock::ResponseTemplate::new(200)
        .set_body_json(serde_json::json!({ "embeddings": [vec![0.25f32; 768]] }))
}

async fn embedded(pool: &PgPool) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM incisive.chat_passages WHERE embedding IS NOT NULL")
        .fetch_one(pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn sweep_fills_vectors_and_gives_up_after_five_bad_attempts() {
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};
    let Some((_g, pool)) = begin().await else {
        return;
    };
    let server = MockServer::start().await;
    let st = embed_state(pool.clone(), &server.uri());
    seed_record(&pool, &ACHALASIA).await;
    job::tick(&st).await;
    let total: i64 = sqlx::query_scalar("SELECT count(*) FROM incisive.chat_passages")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(total > 0);

    // Five bad answers per passage (a 400 is the passage's own fault).
    Mock::given(method("POST"))
        .and(path("/api/embed"))
        .respond_with(ResponseTemplate::new(400))
        .mount(&server)
        .await;
    let mut seen = 0;
    for _ in 0..5 {
        assert_eq!(job::sweep_passage_vectors(&st).await, 0);
        let now = server.received_requests().await.unwrap().len();
        assert!(now > seen, "each sweep tries the passages again");
        seen = now;
    }
    // The batch is 40 passages, so later ones are reached as earlier ones give up.
    for _ in 0..20 {
        assert_eq!(job::sweep_passage_vectors(&st).await, 0);
        let now = server.received_requests().await.unwrap().len();
        if now == seen {
            break;
        }
        seen = now;
    }
    let maxed: i64 =
        sqlx::query_scalar("SELECT count(*) FROM incisive.chat_passages WHERE embed_attempts >= 5")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(maxed, total);
    assert_eq!(job::sweep_passage_vectors(&st).await, 0);
    assert_eq!(server.received_requests().await.unwrap().len(), seen);

    // Reset the counters: a healthy server fills every vector.
    sqlx::query("UPDATE incisive.chat_passages SET embed_attempts = 0")
        .execute(&pool)
        .await
        .unwrap();
    server.reset().await;
    Mock::given(method("POST"))
        .and(path("/api/embed"))
        .respond_with(embed_ok())
        .mount(&server)
        .await;
    let mut filled = 0;
    for _ in 0..10 {
        let n = job::sweep_passage_vectors(&st).await;
        filled += n;
        if n == 0 {
            break;
        }
    }
    assert_eq!(filled as i64, total);
    assert_eq!(embedded(&pool).await, total);
    let model: String = sqlx::query_scalar(
        "SELECT embed_model FROM incisive.chat_passages WHERE embedding IS NOT NULL LIMIT 1",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(model, "embeddinggemma");
}

#[tokio::test]
async fn sweep_stops_when_server_unavailable() {
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};
    let Some((_g, pool)) = begin().await else {
        return;
    };
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/embed"))
        .respond_with(ResponseTemplate::new(503))
        .mount(&server)
        .await;
    let st = embed_state(pool.clone(), &server.uri());
    seed_record(&pool, &ACHALASIA).await;
    job::tick(&st).await;
    assert_eq!(job::sweep_passage_vectors(&st).await, 0);
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
    // An outage is not the passage's fault.
    let bumped: i64 =
        sqlx::query_scalar("SELECT count(*) FROM incisive.chat_passages WHERE embed_attempts > 0")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(bumped, 0);
}

async fn add_turn(pool: &PgPool, status: &str, reply: &str, age_secs: i32) -> i64 {
    let (run, _) = seed_record(pool, &HERNIA).await;
    let user: i64 = sqlx::query_scalar("SELECT user_id FROM incisive.runs WHERE id = $1")
        .bind(run)
        .fetch_one(pool)
        .await
        .unwrap();
    let conv: i64 = sqlx::query_scalar(
        "INSERT INTO incisive.chat_conversations (run_id, user_id, mode) \
         VALUES ($1, $2, 'clinical') RETURNING id",
    )
    .bind(run)
    .bind(user)
    .fetch_one(pool)
    .await
    .unwrap();
    sqlx::query_scalar(
        "INSERT INTO incisive.chat_turns \
         (conversation_id, user_id, seq, question, reply, mode, status, updated_at) \
         VALUES ($1, $2, 1, 'q', $3, 'clinical', $4, now() - make_interval(secs => $5)) \
         RETURNING id",
    )
    .bind(conv)
    .bind(user)
    .bind(reply)
    .bind(status)
    .bind(f64::from(age_secs))
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn turn_status(pool: &PgPool, id: i64) -> String {
    sqlx::query_scalar("SELECT status FROM incisive.chat_turns WHERE id = $1")
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn sweep_yields_when_a_turn_is_pending() {
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer};
    let Some((_g, pool)) = begin().await else {
        return;
    };
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/embed"))
        .respond_with(embed_ok())
        .mount(&server)
        .await;
    let st = embed_state(pool.clone(), &server.uri());
    let fresh = add_turn(&pool, "pending", "", 5).await;
    job::tick(&st).await;
    assert_eq!(store::pending_turns(&pool).await.unwrap(), 1);
    assert_eq!(job::sweep_passage_vectors(&st).await, 0);
    assert_eq!(server.received_requests().await.unwrap().len(), 0);
    // Once the reply is over the sweep runs.
    sqlx::query("UPDATE incisive.chat_turns SET status = 'done' WHERE id = $1")
        .bind(fresh)
        .execute(&pool)
        .await
        .unwrap();
    assert!(job::sweep_passage_vectors(&st).await > 0);
}

#[tokio::test]
async fn stale_pending_turn_becomes_partial_after_90s() {
    let Some((_g, pool)) = begin().await else {
        return;
    };
    let st = state(pool.clone());
    let with_text = add_turn(&pool, "pending", "The mesh group had", 120).await;
    // A second user's empty stale turn.
    let empty = add_turn(&pool, "pending", "", 120).await;
    let fresh = add_turn(&pool, "pending", "so far", 30).await;
    job::tick(&st).await;
    assert_eq!(turn_status(&pool, with_text).await, "partial");
    assert_eq!(turn_status(&pool, empty).await, "unavailable");
    assert_eq!(turn_status(&pool, fresh).await, "pending");
    // The user is no longer locked out: another reply may start.
    assert_eq!(store::pending_turns(&pool).await.unwrap(), 1);
    let text: String = sqlx::query_scalar("SELECT reply FROM incisive.chat_turns WHERE id = $1")
        .bind(with_text)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(text, "The mesh group had");
}
