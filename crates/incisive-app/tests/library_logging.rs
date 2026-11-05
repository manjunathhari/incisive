//! Ivory redesign: the new read-only server functions (the Library cards,
//! the Ask page's latest answers and Today counts) and the Ask and Library
//! pages never put question or answer text into `tracing` output.
//!
//! Own test binary with ONE test, for the reason given in `ask_logging.rs`:
//! the capture subscriber is the global default before any callsite
//! registers. To prove the capture sees this binary's request path, the
//! test first makes a request that logs (an unpriced gate model) and checks
//! that line is captured.
//!
//! Gated on `TEST_DATABASE_URL`.

use std::collections::HashMap;
use std::io::Write;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode},
};
use claude_client::Usage;
use incisive_app::db::{lazy_pool, migrate};
use incisive_app::gate::{Classifier, GateDecision, GateError, Verdict};
use incisive_app::identity::{IapVerifier, JwkSource};
use incisive_app::runs::store::{create_run, save_answer, set_status};
use incisive_app::runs::{RunMode, RunStatus};
use incisive_app::server::{AppState, build_router};
use incisive_app::spend::{CapsConfig, ModelPrice, Prices};
use leptos::config::LeptosOptions;
use secrecy::SecretString;
use serde_json::json;
use sqlx::PgPool;
use tower::ServiceExt;

const PRICED: &str = "fake-gate-model";

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

struct Fake;

#[async_trait]
impl Classifier for Fake {
    async fn classify(&self, _q: &str) -> Result<GateDecision, GateError> {
        Ok(GateDecision {
            verdict: Verdict::Medical,
            reason: "fixed".into(),
            by_rule: false,
            usage: Some(Usage {
                input_tokens: 500,
                output_tokens: 20,
                ..Usage::default()
            }),
            model: Some(PRICED.to_string()),
        })
    }
}

struct AlwaysUp;

#[async_trait]
impl incisive_app::health::Readiness for AlwaysUp {
    async fn db_ok(&self) -> bool {
        true
    }
}

/// `gate_model` without a price makes the Ask path log a known line.
fn state(pool: PgPool, gate_model: &str) -> AppState {
    let mut prices = HashMap::new();
    prices.insert(
        PRICED.to_string(),
        ModelPrice {
            input_usd_per_mtok: 1.0,
            output_usd_per_mtok: 5.0,
            cache_write_usd_per_mtok: 1.25,
            cache_read_usd_per_mtok: 0.1,
        },
    );
    AppState {
        pool,
        verifier: Arc::new(IapVerifier::new(
            "aud".into(),
            JwkSource::Static(serde_json::from_str(r#"{"keys":[]}"#).unwrap()),
        )),
        gate: Arc::new(Fake),
        prices: Prices::new(prices),
        usd_inr: 88.0,
        caps: CapsConfig {
            global_monthly_paise: 1_000_000_000_000,
        },
        gate_model: gate_model.to_string(),
        dev_mode: true,
        runs: incisive_app::runs::RunsConfig::default(),
        quick_engine: incisive_app::runs::QuickEngine::Classic,
        full_engine: incisive_app::runs::Engine::Classic,
        completeness: false,
        temporal: Default::default(),
        temporal_client: None,
        stream: false,
        intro: Default::default(),
        memory: incisive_app::memory::settings::MemorySettings::default(),
        chat: incisive_app::chat::settings::ChatSettings::default(),
        embedder: None,
        gemma: None,
        worker: incisive_app::runs::worker::WorkerHandle::default(),
        llm: None,
        lit: incisive_app::agent::testing::offline_literature(),
    }
}

fn router(st: AppState) -> Router {
    let opts = LeptosOptions::builder().output_name("incisive-app").build();
    build_router(Arc::new(AlwaysUp), st, opts)
}

async fn send(app: Router, req: Request<Body>) -> (StatusCode, String) {
    let res = app.oneshot(req).await.unwrap();
    let status = res.status();
    let bytes = http_body_util::BodyExt::collect(res.into_body())
        .await
        .unwrap()
        .to_bytes();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

#[tokio::test]
async fn library_question_and_answer_text_never_reach_tracing() {
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

    let email = format!("lib-log-{}@example.com", uuid::Uuid::new_v4().simple());
    let uid: i64 = sqlx::query_scalar(
        "INSERT INTO incisive.users (email, display_name, role, daily_cap_paise) \
         VALUES ($1, 'Dr Log', 'owner', 30000) RETURNING id",
    )
    .bind(&email)
    .fetch_one(&pool)
    .await
    .unwrap();

    let marker = format!("zqxmarker{}", uuid::Uuid::new_v4().simple());
    let verdict_marker = format!("vrdmarker{}", uuid::Uuid::new_v4().simple());
    // A run with an answer, and one that failed with none.
    for (status, answer) in [
        (
            RunStatus::Done,
            Some(json!({
                "version": 3,
                "answer": {
                    "at_a_glance": {"verdict": format!("Verdict {verdict_marker}.")},
                    "bottom_line": {"text": format!("Bottom {verdict_marker}."), "certainty": "Low"}
                },
                "references": []
            })),
        ),
        (RunStatus::Failed, None),
    ] {
        let id = create_run(
            &pool,
            uid,
            &format!("Robotic hernia repair {marker}?"),
            RunMode::Focused,
            4000,
        )
        .await
        .unwrap()
        .unwrap();
        assert!(
            set_status(&pool, id, RunStatus::Running, None)
                .await
                .unwrap()
        );
        if let Some(a) = answer {
            assert!(save_answer(&pool, id, &a).await.unwrap());
        }
        assert!(set_status(&pool, id, status, None).await.unwrap());
    }

    // A request that logs, so the capture is shown to work here.
    let req = Request::post("/api/ask")
        .header("content-type", "application/json")
        .header("x-dev-user-email", &email)
        .body(Body::from(
            json!({ "question": "What does the surgical literature say about this approach?" })
                .to_string(),
        ))
        .unwrap();
    let (s, _) = send(router(state(pool.clone(), "no-price")), req).await;
    assert_eq!(s, StatusCode::OK);

    let st = state(pool.clone(), PRICED);
    for path in ["/api/fn/my_library", "/api/fn/my_ask_overview"] {
        let req = Request::post(path)
            .header("content-type", "application/x-www-form-urlencoded")
            .header("x-dev-user-email", &email)
            .body(Body::empty())
            .unwrap();
        let (s, body) = send(router(st.clone()), req).await;
        assert_eq!(s, StatusCode::OK, "{path}: {body}");
        // The user's own text is returned to them...
        assert!(body.contains(&marker), "{path}");
        assert!(body.contains(&verdict_marker), "{path}");
    }
    for path in ["/", "/history"] {
        let req = Request::get(path)
            .header("x-dev-user-email", &email)
            .body(Body::empty())
            .unwrap();
        let (s, body) = send(router(st.clone()), req).await;
        assert_eq!(s, StatusCode::OK, "{path}");
        assert!(body.contains(&verdict_marker), "{path}");
    }

    let logs = String::from_utf8(captured.0.lock().unwrap().clone()).unwrap();
    assert!(
        logs.contains("gate estimate could not be priced"),
        "log line missing from capture:\n{logs}"
    );
    // ...and never logged.
    assert!(
        !logs.contains(&marker),
        "question text reached tracing output"
    );
    assert!(
        !logs.contains(&verdict_marker),
        "answer text reached tracing output"
    );
}
