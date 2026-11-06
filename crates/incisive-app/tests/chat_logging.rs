//! `question_and_reply_never_logged`: the ask flow never puts question, reply
//! or passage text into `tracing` output. Own test binary with ONE test, for
//! the reason given in `ask_logging.rs`: the capture subscriber must be the
//! global default before any callsite registers. Gated on `TEST_DATABASE_URL`.

use std::io::Write;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use incisive_app::agent::temporal_writer::TemporalSettings;
use incisive_app::agent::testing::{offline_literature, placeholder_pipeline};
use incisive_app::chat::gemma::GemmaChat;
use incisive_app::chat::service::{self, ChatError};
use incisive_app::chat::settings::ChatSettings;
use incisive_app::chat::{ChatMode, job};
use incisive_app::db::{lazy_pool, migrate};
use incisive_app::gate::OfflineClassifier;
use incisive_app::identity::{IapVerifier, JwkSource, Role, User};
use incisive_app::memory::job as memory_job;
use incisive_app::memory::settings::MemorySettings;
use incisive_app::runs::RunsConfig;
use incisive_app::runs::worker::WorkerHandle;
use incisive_app::server::AppState;
use incisive_app::spend::{CapsConfig, Prices};
use secrecy::SecretString;
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[derive(Clone, Default)]
struct Captured(Arc<Mutex<Vec<u8>>>);

impl Write for Captured {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn state(pool: PgPool, uri: &str) -> AppState {
    let memory = MemorySettings {
        on: true,
        gemma_url: Some(uri.to_string()),
        gemma_auth: Some(SecretString::from("incisive:pw")),
        ..MemorySettings::default()
    };
    let chat = ChatSettings {
        on: true,
        ..ChatSettings::default()
    };
    let gemma = GemmaChat::from_settings(&memory, &chat).expect("client");
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
        stream: false,
        intro: Default::default(),
        graph: Default::default(),
        chat,
        memory,
        embedder: None,
        gemma: Some(Arc::new(gemma)),
        worker: WorkerHandle::with_pipeline(placeholder_pipeline),
        llm: None,
        lit: offline_literature(),
    }
}

async fn wait_turn(pool: &PgPool, id: i64) -> String {
    for _ in 0..600 {
        let s: String = sqlx::query_scalar("SELECT status FROM incisive.chat_turns WHERE id = $1")
            .bind(id)
            .fetch_one(pool)
            .await
            .unwrap();
        if s != "pending" {
            return s;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("turn never finished");
}

#[tokio::test]
async fn question_and_reply_never_logged() {
    let captured = Captured::default();
    let writer = captured.clone();
    tracing::subscriber::set_global_default(
        tracing_subscriber::fmt()
            .with_max_level(tracing::Level::TRACE)
            .with_writer(move || writer.clone())
            .finish(),
    )
    .expect("first and only global subscriber in this binary");

    let Ok(url) = std::env::var("TEST_DATABASE_URL") else {
        eprintln!("SKIP: TEST_DATABASE_URL not set");
        return;
    };
    let pool = lazy_pool(&SecretString::from(url)).expect("lazy pool");
    migrate(&pool).await.expect("migrate");
    // Other test binaries can leave `pending` turns behind, which would read
    // as a busy queue here.
    sqlx::query("TRUNCATE incisive.chat_turns, incisive.chat_conversations CASCADE")
        .execute(&pool)
        .await
        .unwrap();

    let email = format!("clog-{}@example.test", Uuid::new_v4());
    let uid: i64 = sqlx::query_scalar(
        "INSERT INTO incisive.users (email, display_name, role, daily_cap_paise) \
         VALUES ($1, 'L', 'surgeon', 100000) RETURNING id",
    )
    .bind(&email)
    .fetch_one(&pool)
    .await
    .unwrap();
    let user = User {
        id: uid,
        email,
        display_name: "L".into(),
        role: Role::Surgeon,
        daily_cap_paise: 100_000,
        monthly_cap_paise: None,
        active: true,
    };
    let run = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO incisive.runs (id, user_id, question, mode, status, limit_paise) \
         VALUES ($1, $2, 'Is robotic better?', 'focused', 'done', 1000)",
    )
    .bind(run)
    .bind(uid)
    .execute(&pool)
    .await
    .unwrap();
    let body: Value =
        serde_json::from_str(include_str!("fixtures/temporal/full/hernia.json")).unwrap();
    sqlx::query(
        "INSERT INTO incisive.run_answers (run_id, body, answer_version) VALUES ($1, $2, 3)",
    )
    .bind(run)
    .bind(&body)
    .execute(&pool)
    .await
    .unwrap();
    let input = incisive_app::temporal::TemporalInput::from_json(include_str!(
        "fixtures/temporal/hernia.json"
    ))
    .unwrap();
    let table = serde_json::to_value(incisive_app::temporal::build(&input)).unwrap();
    let wrapped: Value =
        serde_json::from_str(include_str!("fixtures/temporal/boards/hernia-board.json")).unwrap();
    sqlx::query(
        "INSERT INTO incisive.run_temporal (run_id, status, table_json, board_json) \
         VALUES ($1, 'done', $2, $3)",
    )
    .bind(run)
    .bind(&table)
    .bind(&wrapped["board"])
    .execute(&pool)
    .await
    .unwrap();

    let marker = format!("zqxmarker{}", Uuid::new_v4().simple());
    let reply_marker = format!("rplmarker{}", Uuid::new_v4().simple());
    let passage_marker = format!("psgmarker{}", Uuid::new_v4().simple());
    let idn_marker = "Sharma";

    let server = MockServer::start().await;
    let ok = format!(
        "{}\n{{\"done\":true}}\n",
        json!({"message": {"content": format!("{reply_marker} [1]")}, "done": false})
    );
    Mock::given(method("POST"))
        .and(path("/api/chat"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(ok, "application/x-ndjson"))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    // Next: text, then a stream that breaks. Then the server is down.
    let broken = format!(
        "{}\n",
        json!({"message": {"content": format!("{reply_marker} broken")}, "done": false})
    );
    Mock::given(method("POST"))
        .and(path("/api/chat"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(broken, "application/x-ndjson"))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/chat"))
        .respond_with(ResponseTemplate::new(503))
        .mount(&server)
        .await;

    let st = state(pool.clone(), &server.uri());
    let memory_job::JobOutcome::Done { record_id, .. } = memory_job::process(&st, run).await else {
        panic!("memory job did not record the run");
    };
    while job::tick(&st).await > 0 {}
    sqlx::query(
        "INSERT INTO incisive.chat_passages (record_id, kind, seq, ref_ns, comparisons, text) \
         VALUES ($1, 'evidence', 9000, '{1}', '{}', $2)",
    )
    .bind(record_id)
    .bind(format!("{marker} first second third {passage_marker}"))
    .execute(&pool)
    .await
    .unwrap();

    // Done, partial, unavailable, not covered: every way a turn can finish.
    let mut statuses = Vec::new();
    for q in [
        format!("{marker} first?"),
        format!("{marker} second?"),
        format!("{marker} third?"),
        "giraffe xylophone quantum".to_string(),
    ] {
        let a = service::ask(&st, &user, run, &q, ChatMode::Clinical)
            .await
            .unwrap();
        statuses.push(wait_turn(&pool, a.turn_id).await);
    }
    assert_eq!(statuses, ["done", "partial", "unavailable", "not_covered"]);

    // Blocked by the identifier filter.
    let err = service::ask(
        &st,
        &user,
        run,
        &format!("Mr {idn_marker}, 12 March, {marker}?"),
        ChatMode::Clinical,
    )
    .await
    .unwrap_err();
    assert!(matches!(err, ChatError::Identifiers { .. }));

    // A panic: its payload is dropped unread.
    let panic_marker = format!("pncmarker{}", Uuid::new_v4().simple());
    let t: i64 = sqlx::query_scalar(
        "INSERT INTO incisive.chat_turns (conversation_id, user_id, seq, question, mode, status) \
         SELECT id, user_id, 100, 'q', 'clinical', 'pending' \
         FROM incisive.chat_conversations WHERE run_id = $1 RETURNING id",
    )
    .bind(run)
    .fetch_one(&pool)
    .await
    .unwrap();
    let m = panic_marker.clone();
    service::guard_turn(pool.clone(), t, async move { panic!("{m}") }).await;

    let logs = String::from_utf8(captured.0.lock().unwrap().clone()).unwrap();
    assert!(
        logs.contains("chat reply failed"),
        "log line missing from capture:\n{logs}"
    );
    assert!(logs.contains("chat turn panicked"), "{logs}");
    for (what, m) in [
        ("question", marker.as_str()),
        ("reply", reply_marker.as_str()),
        ("passage", passage_marker.as_str()),
        ("identifier", idn_marker),
        ("panic payload", panic_marker.as_str()),
    ] {
        assert!(!logs.contains(m), "{what} text reached tracing output");
    }
}
