//! The question text never reaches `tracing` output.
//!
//! Own test binary with ONE test: the capture subscriber is installed as the
//! global default before any callsite registers. A per-test `set_default` is
//! unreliable when other tests run in parallel (tracing-core caches callsite
//! interest, so a parallel test can leave the pipeline's callsites disabled and
//! the capture empty). To prove the capture really sees the pipeline, the test
//! asserts that the pipeline's own log lines are present.
//!
//! Gated on `TEST_DATABASE_URL`.

use std::collections::HashMap;
use std::io::Write;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use claude_client::Usage;
use incisive_app::db::{lazy_pool, migrate};
use incisive_app::gate::{Classifier, GateDecision, GateError, Verdict};
use incisive_app::identity::{IapVerifier, JwkSource};
use incisive_app::server::{AppState, api_routes};
use incisive_app::spend::{CapsConfig, ModelPrice, Prices};
use secrecy::SecretString;
use serde_json::json;
use sqlx::PgPool;
use tower::ServiceExt;

const PRICED: &str = "fake-gate-model";
const MODEL_QUESTION: &str = "What does the surgical literature say about this clinical approach?";

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

/// Answers with a fixed result; `model` may be one with no configured price.
struct Fake(Result<GateDecision, GateError>);

impl Fake {
    fn verdict(v: Verdict, model: &str) -> Arc<Fake> {
        Arc::new(Fake(Ok(GateDecision {
            verdict: v,
            reason: "fixed".into(),
            by_rule: false,
            usage: Some(Usage {
                input_tokens: 500,
                output_tokens: 20,
            }),
            model: Some(model.to_string()),
        })))
    }
}

#[async_trait]
impl Classifier for Fake {
    async fn classify(&self, _q: &str) -> Result<GateDecision, GateError> {
        self.0.clone()
    }
}

fn state(pool: PgPool, gate: Arc<dyn Classifier>, gate_model: &str) -> AppState {
    let mut prices = HashMap::new();
    prices.insert(
        PRICED.to_string(),
        ModelPrice {
            input_usd_per_mtok: 1.0,
            output_usd_per_mtok: 5.0,
        },
    );
    AppState {
        pool,
        verifier: Arc::new(IapVerifier::new(
            "aud".into(),
            JwkSource::Static(serde_json::from_str(r#"{"keys":[]}"#).unwrap()),
        )),
        gate,
        prices: Prices::new(prices),
        usd_inr: 88.0,
        caps: CapsConfig {
            global_monthly_paise: 1_000_000_000_000,
        },
        gate_model: gate_model.to_string(),
        dev_mode: true,
        runs: incisive_app::runs::RunsConfig::default(),
        worker: incisive_app::runs::worker::WorkerHandle::default(),
    }
}

#[tokio::test]
async fn question_text_never_reaches_tracing() {
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

    let email = format!("ask-log-{}@example.com", uuid::Uuid::new_v4().simple());
    sqlx::query(
        "INSERT INTO incisive.users (email, display_name, role, daily_cap_paise) \
         VALUES ($1, 'Dr Log', 'surgeon', 30000)",
    )
    .bind(&email)
    .execute(&pool)
    .await
    .unwrap();

    let marker = format!("zqxmarker{}", uuid::Uuid::new_v4().simple());
    let q = |text: &str| format!("{text} {marker}");
    let cases: Vec<(AppState, String, &str)> = vec![
        // Unpriced gate model: "gate estimate could not be priced".
        (
            state(
                pool.clone(),
                Fake::verdict(Verdict::Medical, PRICED),
                "no-price",
            ),
            q(MODEL_QUESTION),
            "gate_unavailable",
        ),
        // Decision names an unpriced model: "gate usage could not be priced".
        (
            state(
                pool.clone(),
                Fake::verdict(Verdict::Medical, "no-price"),
                PRICED,
            ),
            q(MODEL_QUESTION),
            "run_started",
        ),
        // Identifiers blocked.
        (
            state(
                pool.clone(),
                Fake::verdict(Verdict::Medical, PRICED),
                PRICED,
            ),
            q("Mrs. Lakshmi ph 9845012345"),
            "blocked_identifiers",
        ),
        // Refused by the model and by rule.
        (
            state(
                pool.clone(),
                Fake::verdict(Verdict::NotMedical, PRICED),
                PRICED,
            ),
            q(MODEL_QUESTION),
            "not_medical",
        ),
        (
            state(
                pool.clone(),
                Fake::verdict(Verdict::Medical, PRICED),
                PRICED,
            ),
            q("bitcoin price tomorrow"),
            "not_medical",
        ),
        // Gate error.
        (
            state(
                pool.clone(),
                Arc::new(Fake(Err(GateError::BadResponse {
                    model: PRICED.to_string(),
                    usage: claude_client::Usage {
                        input_tokens: 10,
                        output_tokens: 150,
                    },
                }))),
                PRICED,
            ),
            q(MODEL_QUESTION),
            "gate_unavailable",
        ),
    ];
    for (st, question, want) in cases {
        let req = Request::post("/api/ask")
            .header("content-type", "application/json")
            .header("x-dev-user-email", &email)
            .body(Body::from(json!({ "question": question }).to_string()))
            .unwrap();
        let res = api_routes(st).oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let bytes = http_body_util::BodyExt::collect(res.into_body())
            .await
            .unwrap()
            .to_bytes();
        let b: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(b["status"], want, "{b}");
    }

    let logs = String::from_utf8(captured.0.lock().unwrap().clone()).unwrap();
    // The capture really sees the pipeline's own lines.
    assert!(
        logs.contains("gate estimate could not be priced"),
        "pipeline log line missing from capture:\n{logs}"
    );
    assert!(
        logs.contains("gate usage could not be priced"),
        "pipeline log line missing from capture:\n{logs}"
    );
    // ...and never the question.
    assert!(
        !logs.contains(&marker),
        "question text reached tracing output"
    );
    assert!(!logs.contains("9845012345"));
    assert!(!logs.contains("Lakshmi"));
}
