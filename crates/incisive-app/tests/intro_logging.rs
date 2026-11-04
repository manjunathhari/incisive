//! The question and the introduction never reach `tracing` output. Own test
//! binary with ONE test, for the reason given in `ask_logging.rs`: the capture
//! subscriber must be the global default before any callsite registers. To
//! prove the capture really sees the job, the test asserts the job's own log
//! lines are present. Gated on `TEST_DATABASE_URL`.

use std::io::Write;
use std::sync::{Arc, Mutex};

use claude_client::{ClaudeError, StopReason, Usage};
use incisive_app::agent::llm::{LlmClient, ScriptedLlm};
use incisive_app::agent::testing::{agent_llm, offline_literature, placeholder_pipeline};
use incisive_app::ask::{AskOutcome, start_run};
use incisive_app::db::{lazy_pool, migrate};
use incisive_app::gate::OfflineClassifier;
use incisive_app::identity::{IapVerifier, JwkSource, find_user_by_id};
use incisive_app::runs::intro::{IntroRuntime, IntroSettings};
use incisive_app::runs::worker::WorkerHandle;
use incisive_app::runs::{RunMode, RunsConfig, store};
use incisive_app::server::AppState;
use incisive_app::spend::{CapsConfig, Prices};
use secrecy::SecretString;
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
        intro: IntroRuntime {
            settings: IntroSettings {
                on: true,
                model: "m-test".into(),
                ..IntroSettings::default()
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
async fn question_and_introduction_never_logged() {
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

    let id: i64 = sqlx::query_scalar(
        "INSERT INTO incisive.users (email, display_name, role, daily_cap_paise) \
         VALUES ($1, 'Dr Log', 'surgeon', 100000) RETURNING id",
    )
    .bind(format!("intro-log-{}@example.com", Uuid::new_v4().simple()))
    .fetch_one(&pool)
    .await
    .unwrap();
    let user = find_user_by_id(&pool, id).await.unwrap().unwrap();

    let q_marker = format!("zqxquestion{}", Uuid::new_v4().simple());
    let t_marker = format!("zqxreply{}", Uuid::new_v4().simple());
    let usage = Usage {
        input_tokens: 100,
        output_tokens: 200,
        ..Usage::default()
    };
    let reply = format!("About this {t_marker}.");
    let cases: Vec<ScriptedLlm> = vec![
        ScriptedLlm::new().reply_text(&reply, usage),
        ScriptedLlm::new().reply_stop(&reply, StopReason::MaxTokens, usage),
        ScriptedLlm::new().reply_stop(&reply, StopReason::Refusal, usage),
        ScriptedLlm::new().reply_text("   ", usage),
        ScriptedLlm::new().fail(ClaudeError::Api {
            status: 500,
            error_type: "api_error".into(),
            message: format!("{t_marker} {q_marker}"),
        }),
    ];
    for llm in cases {
        let st = state(pool.clone(), Arc::new(llm));
        let question = format!("Question about {q_marker}?");
        let out = start_run(&st, Uuid::new_v4(), &user, &question, RunMode::Quick, None)
            .await
            .unwrap();
        let AskOutcome::RunStarted { run_id, .. } = out else {
            panic!("not started");
        };
        for _ in 0..400 {
            let row: Option<String> =
                sqlx::query_scalar("SELECT status FROM incisive.run_intros WHERE run_id = $1")
                    .bind(run_id)
                    .fetch_optional(&pool)
                    .await
                    .unwrap();
            let view = store::get_run_for_user(&pool, id, run_id)
                .await
                .unwrap()
                .unwrap();
            if row.is_some_and(|s| s != "pending") && !view.summary.status.is_active() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
    }

    let logged = String::from_utf8_lossy(&cap.0.lock().unwrap()).into_owned();
    assert!(
        logged.contains("intro job finished"),
        "the capture sees the job"
    );
    assert!(logged.contains("intro call done"));
    assert!(!logged.contains(&q_marker), "the question was logged");
    assert!(!logged.contains(&t_marker), "the introduction was logged");
    assert!(!logged.contains("simple, brief introduction"), "the prompt");
}
