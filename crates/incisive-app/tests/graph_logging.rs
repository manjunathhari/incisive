//! The question and the concept names never reach `tracing` output. Own test
//! binary with ONE test, for the reason given in `ask_logging.rs`: the capture
//! subscriber must be the global default before any callsite registers. To
//! prove the capture really sees the job, the test asserts the job's own log
//! lines are present. Gated on `TEST_DATABASE_URL`.

use std::io::Write;
use std::sync::{Arc, Mutex};

use claude_client::{ClaudeError, StopReason, Usage};
use incisive_app::agent::llm::{LlmClient, ScriptedLlm};
use incisive_app::agent::testing::{agent_llm, offline_literature, placeholder_pipeline};
use incisive_app::db::{lazy_pool, migrate};
use incisive_app::gate::OfflineClassifier;
use incisive_app::graph::job::process;
use incisive_app::graph::settings::{GraphRuntime, GraphSettings};
use incisive_app::identity::{IapVerifier, JwkSource};
use incisive_app::runs::RunsConfig;
use incisive_app::runs::worker::WorkerHandle;
use incisive_app::server::AppState;
use incisive_app::spend::{CapsConfig, Prices};
use secrecy::SecretString;
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

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

fn state(pool: PgPool, llm: Arc<dyn LlmClient>) -> AppState {
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
        temporal: Default::default(),
        temporal_client: None,
        stream: false,
        intro: Default::default(),
        graph: GraphRuntime {
            settings: GraphSettings {
                on: true,
                model: "m-test".into(),
                from: Some(chrono::NaiveDate::from_ymd_opt(2026, 1, 1).unwrap()),
                ..GraphSettings::default()
            },
            client: Some(llm),
        },
        memory: incisive_app::memory::settings::MemorySettings::default(),
        chat: incisive_app::chat::settings::ChatSettings::default(),
        embedder: None,
        gemma: None,
        worker: WorkerHandle::with_pipeline(placeholder_pipeline),
        llm: Some(agent_llm(Arc::new(ScriptedLlm::new()), "m-test")),
        lit: offline_literature(),
    }
}

#[tokio::test]
async fn question_and_names_never_logged() {
    let cap = Captured::default();
    let writer = cap.clone();
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

    let uid: i64 = sqlx::query_scalar(
        "INSERT INTO incisive.users (email, display_name, role, daily_cap_paise) \
         VALUES ($1, 'Dr Log', 'surgeon', 100000) RETURNING id",
    )
    .bind(format!("graph-log-{}@example.com", Uuid::new_v4().simple()))
    .fetch_one(&pool)
    .await
    .unwrap();

    let q_marker = format!("zqxquestion{}", Uuid::new_v4().simple());
    let n_marker = format!("zqxname{}", Uuid::new_v4().simple());
    let usage = Usage {
        input_tokens: 100,
        output_tokens: 200,
        ..Usage::default()
    };
    let good = json!({
        "diseases": [format!("Hernia {n_marker}")],
        "treatments": [format!("Mesh {n_marker}")],
        "organs": ["Abdominal wall", format!("Organ {n_marker}")],
        "outcomes": [{"row": 1, "name": format!("Pain {n_marker}")},
                     {"row": 99, "name": format!("Far {n_marker}")}]
    });
    let mut empty = good.clone();
    empty["organs"] = json!([]);
    let reply = good.to_string();
    let cases: Vec<ScriptedLlm> = vec![
        ScriptedLlm::new().reply_json(good, usage),
        ScriptedLlm::new().reply_json(empty, usage),
        ScriptedLlm::new().reply_stop(&reply, StopReason::MaxTokens, usage),
        ScriptedLlm::new().reply_stop(&reply, StopReason::Refusal, usage),
        ScriptedLlm::new().reply_text(&format!("not json {n_marker}"), usage),
        ScriptedLlm::new().fail(ClaudeError::Api {
            status: 500,
            error_type: "api_error".into(),
            message: format!("{n_marker} {q_marker}"),
        }),
    ];
    let mut body: Value =
        serde_json::from_str(include_str!("fixtures/temporal/full/hernia.json")).unwrap();
    let question = format!("Question about {q_marker}?");
    body["question"] = json!(question);
    for llm in cases {
        let run = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO incisive.runs (id, user_id, question, mode, status, limit_paise) \
             VALUES ($1, $2, $3, 'quick', 'done', 1500)",
        )
        .bind(run)
        .bind(uid)
        .bind(&question)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO incisive.run_answers (run_id, body, answer_version) VALUES ($1, $2, 3)",
        )
        .bind(run)
        .bind(&body)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("INSERT INTO incisive.graph_jobs (run_id) VALUES ($1)")
            .bind(run)
            .execute(&pool)
            .await
            .unwrap();
        process(&state(pool.clone(), Arc::new(llm)), run).await;
    }

    let logged = String::from_utf8_lossy(&cap.0.lock().unwrap()).into_owned();
    assert!(
        logged.contains("graph job finished"),
        "the capture sees the job"
    );
    assert!(logged.contains("graph call done"));
    assert!(!logged.contains(&q_marker), "the question was logged");
    assert!(!logged.contains(&n_marker), "a name was logged");
    assert!(!logged.contains("Abdominal wall"), "an organ was logged");
}
