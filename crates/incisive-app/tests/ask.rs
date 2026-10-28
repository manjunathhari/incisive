//! Ask pipeline tests: identity -> identifier filter -> spend reservation -> gate
//! -> settle -> accepted -> run started, every step audited under one request id
//! (the run's own events are audited under the run id).
//!
//! DB tests are gated on `TEST_DATABASE_URL`. Each test uses its own user (a
//! random email), and reads back only that user's audit and spend rows, because
//! `incisive.audit` and `incisive.spend` are shared by every test.

use std::collections::HashMap;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use async_trait::async_trait;
use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode},
};
use claude_client::{Client, RetryPolicy, Usage};
use http_body_util::BodyExt;
use incisive_app::ask::{AskFailure, AskOutcome, gate_estimate, run_ask};
use incisive_app::db::{lazy_pool, migrate};
use incisive_app::gate::{Classifier, ClaudeClassifier, GateDecision, GateError, Verdict};
use incisive_app::identity::{IapVerifier, JwkSource, Role, User};
use incisive_app::runs::worker::WorkerHandle;
use incisive_app::runs::{RunMode, RunsConfig};
use incisive_app::server::{AppState, api_routes, build_router};
use incisive_app::spend::{CapsConfig, ModelPrice, Prices, cost_paise};
use leptos::config::LeptosOptions;
use secrecy::SecretString;
use serde_json::{Value, json};
use sqlx::{PgPool, Row};
use std::time::Duration;
use tower::ServiceExt;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const GATE_MODEL: &str = "fake-gate-model";
/// Needs the model: two generic words only, no specific terms, no rule match.
const MODEL_QUESTION: &str = "What does the surgical literature say about this clinical approach?";
/// Decided Medical by rule (two specific terms), so no key or model is needed.
const RULE_QUESTION: &str =
    "52 y/o M, robotic LAR, ICG for leak? Evidence on anastomotic resection";

// ---------- fakes ----------

/// Fails the test if the pipeline ever calls the model.
struct MustNotCall;

#[async_trait]
impl Classifier for MustNotCall {
    async fn classify(&self, _q: &str) -> Result<GateDecision, GateError> {
        panic!("the gate classifier must not be called for this request");
    }
}

/// Returns a fixed answer and counts calls.
struct Fixed {
    answer: Result<GateDecision, GateError>,
    calls: AtomicUsize,
}

impl Fixed {
    fn verdict(v: Verdict, reason: &str) -> Arc<Fixed> {
        Arc::new(Fixed {
            answer: Ok(GateDecision {
                verdict: v,
                reason: reason.to_string(),
                by_rule: false,
                usage: Some(Usage {
                    input_tokens: 812,
                    output_tokens: 37,
                }),
                model: Some(GATE_MODEL.to_string()),
            }),
            calls: AtomicUsize::new(0),
        })
    }

    fn error(e: GateError) -> Arc<Fixed> {
        Arc::new(Fixed {
            answer: Err(e),
            calls: AtomicUsize::new(0),
        })
    }
}

#[async_trait]
impl Classifier for Fixed {
    async fn classify(&self, _q: &str) -> Result<GateDecision, GateError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.answer.clone()
    }
}

// ---------- state and helpers ----------

fn prices() -> Prices {
    let mut m = HashMap::new();
    m.insert(
        GATE_MODEL.to_string(),
        ModelPrice {
            input_usd_per_mtok: 1.0,
            output_usd_per_mtok: 5.0,
        },
    );
    Prices::new(m)
}

fn state_with(pool: PgPool, gate: Arc<dyn Classifier>, dev_mode: bool) -> AppState {
    AppState {
        pool,
        verifier: Arc::new(IapVerifier::new(
            "aud".into(),
            JwkSource::Static(serde_json::from_str(r#"{"keys":[]}"#).unwrap()),
        )),
        gate,
        prices: prices(),
        usd_inr: 88.0,
        // Large: the global month is shared with every other test's rows.
        caps: CapsConfig {
            global_monthly_paise: 1_000_000_000_000,
        },
        gate_model: GATE_MODEL.to_string(),
        dev_mode,
        runs: RunsConfig::default(),
        // Runs started by `run_ask` use the deterministic placeholder.
        worker: WorkerHandle::with_pipeline(incisive_app::agent::testing::placeholder_pipeline),
        llm: Some(incisive_app::agent::testing::idle_llm(GATE_MODEL)),
        lit: incisive_app::agent::testing::offline_literature(),
    }
}

fn unreachable_pool() -> PgPool {
    lazy_pool(&SecretString::from(
        "postgres://u:p@127.0.0.1:1/none".to_string(),
    ))
    .unwrap()
}

async fn test_pool() -> Option<PgPool> {
    let Ok(url) = std::env::var("TEST_DATABASE_URL") else {
        eprintln!("SKIP: TEST_DATABASE_URL not set");
        return None;
    };
    let pool = lazy_pool(&SecretString::from(url)).expect("lazy pool");
    migrate(&pool).await.expect("migrate");
    Some(pool)
}

fn unique_email(tag: &str) -> String {
    format!("ask-{tag}-{}@example.com", uuid::Uuid::new_v4().simple())
}

async fn make_user(pool: &PgPool, tag: &str, role: Role, daily_cap: i64) -> User {
    let email = unique_email(tag);
    let row = sqlx::query(
        "INSERT INTO incisive.users (email, display_name, role, daily_cap_paise, monthly_cap_paise) \
         VALUES ($1, $2, $3, $4, NULL) RETURNING id",
    )
    .bind(&email)
    .bind(format!("Dr {tag}"))
    .bind(role.as_str())
    .bind(daily_cap)
    .fetch_one(pool)
    .await
    .unwrap();
    User {
        id: row.get("id"),
        email,
        display_name: format!("Dr {tag}"),
        role,
        daily_cap_paise: daily_cap,
        monthly_cap_paise: None,
        active: true,
    }
}

async fn call(app: Router, req: Request<Body>) -> (StatusCode, Value) {
    let res = app.oneshot(req).await.unwrap();
    let status = res.status();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

fn ask_req(email: &str, question: &str) -> Request<Body> {
    Request::post("/api/ask")
        .header("content-type", "application/json")
        .header("x-dev-user-email", email)
        .body(Body::from(json!({ "question": question }).to_string()))
        .unwrap()
}

/// This user's audit entries, oldest first: (event, request_id, detail).
async fn audit_for(pool: &PgPool, user_id: i64) -> Vec<(String, Option<uuid::Uuid>, Value)> {
    sqlx::query(
        "SELECT event, request_id, detail FROM incisive.audit WHERE user_id = $1 ORDER BY seq",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await
    .unwrap()
    .iter()
    .map(|r| (r.get("event"), r.get("request_id"), r.get("detail")))
    .collect()
}

fn events(rows: &[(String, Option<uuid::Uuid>, Value)]) -> Vec<&str> {
    rows.iter().map(|r| r.0.as_str()).collect()
}

/// Wait (up to 5 s) for the worker's `run_finished` entry for `run_id`.
async fn wait_run_finished(pool: &PgPool, run_id: uuid::Uuid) -> Value {
    for _ in 0..500 {
        let d: Option<Value> = sqlx::query_scalar(
            "SELECT detail FROM incisive.audit WHERE request_id = $1 AND event = 'run_finished'",
        )
        .bind(run_id)
        .fetch_optional(pool)
        .await
        .unwrap();
        if let Some(d) = d {
            return d;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("run {run_id} did not finish within 5 s");
}

fn run_id_of(b: &Value) -> uuid::Uuid {
    b["run_id"].as_str().expect("run_id").parse().unwrap()
}

const RUN_STARTED: &str =
    "Working on it \u{2014} you can leave this page; the answer will be in My questions.";

fn one_request_id(rows: &[(String, Option<uuid::Uuid>, Value)]) {
    let first = rows[0].1.expect("request id");
    assert!(
        rows.iter().all(|r| r.1 == Some(first)),
        "all entries share one request id: {rows:?}"
    );
}

/// This user's spend rows: (kind, model, input_tokens, output_tokens, paise).
async fn spend_for(
    pool: &PgPool,
    user_id: i64,
) -> Vec<(String, Option<String>, Option<i32>, Option<i32>, i64)> {
    sqlx::query(
        "SELECT kind, model, input_tokens, output_tokens, paise FROM incisive.spend \
         WHERE user_id = $1 ORDER BY id",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await
    .unwrap()
    .iter()
    .map(|r| {
        (
            r.get("kind"),
            r.get("model"),
            r.get("input_tokens"),
            r.get("output_tokens"),
            r.get("paise"),
        )
    })
    .collect()
}

// ---------- pipeline ----------

#[tokio::test]
async fn identifiers_blocked_before_gate() {
    let Some(pool) = test_pool().await else {
        return;
    };
    let u = make_user(&pool, "deid", Role::Surgeon, 30_000).await;
    let app = api_routes(state_with(pool.clone(), Arc::new(MustNotCall), true));
    let q = "Mrs. Lakshmi ph 9845012345, robotic LAR, leak?";
    let (s, b) = call(app, ask_req(&u.email, q)).await;
    assert_eq!(s, StatusCode::OK, "{b}");
    assert_eq!(b["status"], "blocked_identifiers");
    let kinds: Vec<&str> = b["findings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["kind"].as_str().unwrap())
        .collect();
    assert!(kinds.contains(&"phone"), "{b}");
    assert!(kinds.contains(&"person_name"), "{b}");
    for f in b["findings"].as_array().unwrap() {
        // kind, label and offsets only: never the matched text.
        let keys: Vec<&String> = f.as_object().unwrap().keys().collect();
        assert_eq!(keys.len(), 4, "{f}");
        assert!(f["label"].is_string() && f["start"].is_u64() && f["end"].is_u64());
    }
    assert!(!b.to_string().contains("9845012345"));
    assert!(!b.to_string().contains("Lakshmi"));
    assert!(b["message"].as_str().unwrap().contains("remove"), "{b}");

    let rows = audit_for(&pool, u.id).await;
    assert_eq!(events(&rows), ["deid_blocked"]);
    let detail = rows[0].2.to_string();
    assert!(!detail.contains("9845012345") && !detail.contains("Lakshmi"));
    assert!(spend_for(&pool, u.id).await.is_empty(), "no reservation");
}

#[tokio::test]
async fn hidden_characters_blocked_before_gate() {
    let Some(pool) = test_pool().await else {
        return;
    };
    let u = make_user(&pool, "zwsp", Role::Surgeon, 30_000).await;
    let gate = Fixed::verdict(Verdict::Medical, "would accept");
    let app = api_routes(state_with(pool.clone(), gate.clone(), true));
    let q = "robotic LAR leak? ph 98450\u{200B}12345";
    let (s, b) = call(app, ask_req(&u.email, q)).await;
    assert_eq!(s, StatusCode::OK, "{b}");
    assert_eq!(b["status"], "blocked_identifiers", "{b}");
    assert_eq!(
        gate.calls.load(Ordering::SeqCst),
        0,
        "classifier never called"
    );
    let f = &b["findings"][0];
    assert_eq!(f["kind"], "unusual_characters");
    assert_eq!(f["label"], "hidden or unusual characters");
    let start = q.find('\u{200B}').unwrap();
    assert_eq!(
        (f["start"].as_u64(), f["end"].as_u64()),
        (Some(start as u64), Some(start as u64 + 3))
    );
    assert_eq!(
        b["message"],
        "Incisive found details that could identify a patient. Please remove them and resend \
         your question. Please retype the question without copy-pasted special characters \
         (use plain hyphens, colons and spaces)."
    );
    let rows = audit_for(&pool, u.id).await;
    assert_eq!(events(&rows), ["deid_blocked"]);
    assert_eq!(
        rows[0].2,
        json!({"findings": [{"kind": "unusual_characters", "start_byte": start, "end_byte": start + 3}],
               "question_len": q.len()})
    );
    assert!(spend_for(&pool, u.id).await.is_empty(), "no reservation");
}

#[tokio::test]
async fn non_medical_refused_and_audited() {
    let Some(pool) = test_pool().await else {
        return;
    };
    let u = make_user(&pool, "nonmed", Role::Surgeon, 30_000).await;
    let gate = Fixed::verdict(Verdict::NotMedical, "That is about investing.");
    let app = api_routes(state_with(pool.clone(), gate.clone(), true));
    let (s, b) = call(app, ask_req(&u.email, MODEL_QUESTION)).await;
    assert_eq!(s, StatusCode::OK, "{b}");
    assert_eq!(b["status"], "not_medical");
    assert_eq!(
        b["message"],
        "Incisive only answers medical and surgical questions. That is about investing."
    );
    assert_eq!(gate.calls.load(Ordering::SeqCst), 1);
    let rows = audit_for(&pool, u.id).await;
    assert_eq!(events(&rows), ["gate_decided", "charged"]);
    one_request_id(&rows);
    assert_eq!(rows[0].2["verdict"], "not_medical");
    // A model-written reason is never stored.
    assert!(rows[0].2.get("reason").is_none());
}

#[tokio::test]
async fn non_medical_by_rule_is_not_charged() {
    let Some(pool) = test_pool().await else {
        return;
    };
    let u = make_user(&pool, "nonmedrule", Role::Surgeon, 30_000).await;
    let app = api_routes(state_with(pool.clone(), Arc::new(MustNotCall), true));
    let (_, b) = call(app, ask_req(&u.email, "bitcoin price tomorrow")).await;
    assert_eq!(b["status"], "not_medical");
    let rows = audit_for(&pool, u.id).await;
    assert_eq!(events(&rows), ["gate_decided"]);
    assert_eq!(rows[0].2["by_rule"], true);
    let spend = spend_for(&pool, u.id).await;
    assert_eq!(spend.len(), 1);
    assert_eq!((spend[0].0.as_str(), spend[0].4), ("charge", 0));
}

#[tokio::test]
async fn unclear_asks_for_more() {
    let Some(pool) = test_pool().await else {
        return;
    };
    let u = make_user(&pool, "unclear", Role::Surgeon, 30_000).await;
    let gate = Fixed::verdict(Verdict::Unclear, "It is not clear what is being asked.");
    let app = api_routes(state_with(pool.clone(), gate, true));
    let (_, b) = call(app, ask_req(&u.email, MODEL_QUESTION)).await;
    assert_eq!(b["status"], "unclear");
    assert!(
        b["message"]
            .as_str()
            .unwrap()
            .starts_with("Could you say a little more about the clinical question?")
    );
    let rows = audit_for(&pool, u.id).await;
    assert_eq!(events(&rows), ["gate_decided", "charged"]);
}

#[tokio::test]
async fn gate_failure_fails_closed() {
    let Some(pool) = test_pool().await else {
        return;
    };
    // Not billed (an HTTP error status): settled to zero, no `charged` entry.
    for (tag, err, kind) in [(
        "gatefail-u",
        GateError::Unavailable {
            maybe_billed: false,
        },
        "unavailable",
    )] {
        let u = make_user(&pool, tag, Role::Surgeon, 30_000).await;
        let app = api_routes(state_with(pool.clone(), Fixed::error(err), true));
        let (s, b) = call(app, ask_req(&u.email, MODEL_QUESTION)).await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(b["status"], "gate_unavailable");
        assert_eq!(
            b["message"],
            "I couldn't check this question just now. Please try again in a moment."
        );
        let rows = audit_for(&pool, u.id).await;
        assert_eq!(events(&rows), ["gate_failed"]);
        assert_eq!(rows[0].2, json!({"error_kind": kind}));
        // Reservation settled to a zero charge.
        let spend = spend_for(&pool, u.id).await;
        assert_eq!(spend.len(), 1, "{spend:?}");
        assert_eq!((spend[0].0.as_str(), spend[0].4), ("charge", 0));
    }
}

fn bad_response(model: &str, input_tokens: u32, output_tokens: u32) -> GateError {
    GateError::BadResponse {
        model: model.to_string(),
        usage: Usage {
            input_tokens,
            output_tokens,
        },
    }
}

/// The priced gate estimate for `question`, as `run_ask` reserves it.
fn estimate_paise(question: &str) -> i64 {
    cost_paise(&prices(), 88.0, GATE_MODEL, &gate_estimate(question)).unwrap()
}

#[test]
fn gate_estimate_grows_with_question_bytes() {
    let u = |q: &str| {
        let e = gate_estimate(q);
        (e.input_tokens, e.output_tokens)
    };
    // 700 for the prompt and policy, ceil(bytes / 2) for the question, 150 out.
    assert_eq!(u(""), (700, 150));
    assert_eq!(u("a"), (701, 150));
    assert_eq!(u("abcd"), (702, 150));
    assert_eq!(u("abcde"), (703, 150));
    // Indic scripts are 3 bytes per character in UTF-8: estimated conservatively.
    let kannada = "\u{0CB0}\u{0CCB}\u{0C97}"; // 3 chars, 9 bytes
    assert_eq!(u(kannada), (705, 150));
    let long = "\u{0930}".repeat(4_000); // the longest question: 12,000 bytes
    assert_eq!(u(&long), (6_700, 150));
    assert_eq!(
        u(MODEL_QUESTION),
        (700 + MODEL_QUESTION.len().div_ceil(2) as u32, 150)
    );
}

#[tokio::test]
async fn bad_gate_answer_is_charged_at_real_cost() {
    let Some(pool) = test_pool().await else {
        return;
    };
    let u = make_user(&pool, "badresp", Role::Surgeon, 30_000).await;
    let gate = Fixed::error(bad_response(GATE_MODEL, 812, 150));
    let app = api_routes(state_with(pool.clone(), gate, true));
    let (s, b) = call(app, ask_req(&u.email, MODEL_QUESTION)).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(b["status"], "gate_unavailable", "{b}");
    assert_eq!(
        b["message"],
        "I couldn't check this question just now. Please try again in a moment."
    );
    let rows = audit_for(&pool, u.id).await;
    assert_eq!(events(&rows), ["charged", "gate_failed"]);
    one_request_id(&rows);
    // (812 * 1 + 150 * 5) / 1e6 USD * 88 * 100 = 13.7456 paise -> 14.
    assert_eq!(
        rows[0].2,
        json!({"model": GATE_MODEL, "input_tokens": 812, "output_tokens": 150, "paise": 14})
    );
    assert_eq!(rows[1].2, json!({"error_kind": "bad_response"}));
    assert_eq!(
        spend_for(&pool, u.id).await,
        vec![(
            "charge".to_string(),
            Some(GATE_MODEL.to_string()),
            Some(812),
            Some(150),
            14
        )]
    );
}

#[tokio::test]
async fn bad_gate_answer_with_unpriced_model_charges_the_estimate() {
    let Some(pool) = test_pool().await else {
        return;
    };
    let u = make_user(&pool, "badresp-unpriced", Role::Surgeon, 30_000).await;
    let gate = Fixed::error(bad_response("no-price-model", 812, 150));
    let app = api_routes(state_with(pool.clone(), gate, true));
    let (_, b) = call(app, ask_req(&u.email, MODEL_QUESTION)).await;
    assert_eq!(b["status"], "gate_unavailable", "{b}");
    let rows = audit_for(&pool, u.id).await;
    assert_eq!(events(&rows), ["charged", "gate_failed"]);
    let est = estimate_paise(MODEL_QUESTION);
    assert!(est > 0);
    assert_eq!(rows[0].2["paise"], est);
    let spend = spend_for(&pool, u.id).await;
    assert_eq!(spend.len(), 1, "{spend:?}");
    assert_eq!((spend[0].0.as_str(), spend[0].4), ("charge", est));
}

fn wiremock_gate(server: &MockServer, timeout: Duration) -> Arc<dyn Classifier> {
    let client = Client::with_base_url(
        SecretString::from("sk-test".to_string()),
        server.uri().parse().unwrap(),
    )
    .with_request_timeout(timeout)
    .with_retry_policy(RetryPolicy {
        max_attempts: 3,
        base_delay: Duration::from_millis(1),
        max_delay: Duration::from_millis(5),
    });
    Arc::new(ClaudeClassifier::new(Arc::new(client), GATE_MODEL))
}

#[tokio::test]
async fn gate_timeout_settles_the_estimate() {
    let Some(pool) = test_pool().await else {
        return;
    };
    let u = make_user(&pool, "timeout", Role::Surgeon, 30_000).await;
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(10)))
        .mount(&server)
        .await;
    let gate = wiremock_gate(&server, Duration::from_millis(100));
    let app = api_routes(state_with(pool.clone(), gate, true));
    let (_, b) = call(app, ask_req(&u.email, MODEL_QUESTION)).await;
    assert_eq!(b["status"], "gate_unavailable", "{b}");
    let rows = audit_for(&pool, u.id).await;
    assert_eq!(events(&rows), ["charged", "gate_failed"]);
    let est = estimate_paise(MODEL_QUESTION);
    assert_eq!(
        rows[0].2,
        json!({"model": GATE_MODEL, "input_tokens": null, "output_tokens": null, "paise": est})
    );
    assert_eq!(rows[1].2, json!({"error_kind": "unavailable"}));
    assert_eq!(
        spend_for(&pool, u.id).await,
        vec![(
            "charge".to_string(),
            Some(GATE_MODEL.to_string()),
            None,
            None,
            est
        )]
    );
}

#[tokio::test]
async fn gate_overloaded_settles_zero() {
    let Some(pool) = test_pool().await else {
        return;
    };
    let u = make_user(&pool, "overloaded", Role::Surgeon, 30_000).await;
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(529).set_body_json(json!({
            "type": "error", "error": {"type": "overloaded_error", "message": "Overloaded"}
        })))
        .expect(3)
        .mount(&server)
        .await;
    let gate = wiremock_gate(&server, Duration::from_secs(30));
    let app = api_routes(state_with(pool.clone(), gate, true));
    let (_, b) = call(app, ask_req(&u.email, MODEL_QUESTION)).await;
    assert_eq!(b["status"], "gate_unavailable", "{b}");
    let rows = audit_for(&pool, u.id).await;
    assert_eq!(events(&rows), ["gate_failed"]);
    assert_eq!(rows[0].2, json!({"error_kind": "unavailable"}));
    assert_eq!(
        spend_for(&pool, u.id).await,
        vec![("charge".to_string(), None, None, None, 0)]
    );
}

#[tokio::test]
async fn unpriced_gate_model_fails_closed() {
    let Some(pool) = test_pool().await else {
        return;
    };
    let u = make_user(&pool, "unpriced", Role::Surgeon, 30_000).await;
    let mut st = state_with(pool.clone(), Arc::new(MustNotCall), true);
    st.gate_model = "no-price-for-this-model".into();
    let (_, b) = call(api_routes(st), ask_req(&u.email, RULE_QUESTION)).await;
    assert_eq!(b["status"], "gate_unavailable");
    let rows = audit_for(&pool, u.id).await;
    assert_eq!(events(&rows), ["gate_failed"]);
    assert_eq!(rows[0].2, json!({"error_kind": "unpriced_model"}));
    assert!(spend_for(&pool, u.id).await.is_empty(), "nothing reserved");
}

#[tokio::test]
async fn over_cap_blocked_before_gate() {
    let Some(pool) = test_pool().await else {
        return;
    };
    let u = make_user(&pool, "overcap", Role::Fellow, 0).await;
    let app = api_routes(state_with(pool.clone(), Arc::new(MustNotCall), true));
    let (s, b) = call(app, ask_req(&u.email, MODEL_QUESTION)).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(b["status"], "over_cap");
    assert_eq!(
        b["message"],
        "You've reached today's Incisive limit (\u{20b9}0). It resets at midnight IST."
    );
    let rows = audit_for(&pool, u.id).await;
    assert_eq!(events(&rows), ["cap_blocked"]);
    assert!(rows[0].2["estimate_paise"].as_i64().unwrap() > 0);
    assert!(spend_for(&pool, u.id).await.is_empty());
}

#[tokio::test]
async fn accepted_path_audits_gate_charge_accepted_in_order() {
    let Some(pool) = test_pool().await else {
        return;
    };
    let u = make_user(&pool, "accepted", Role::Surgeon, 30_000).await;
    let gate = Fixed::verdict(Verdict::Medical, "A clinical question.");
    let app = api_routes(state_with(pool.clone(), gate, true));
    let (s, b) = call(app, ask_req(&u.email, MODEL_QUESTION)).await;
    assert_eq!(s, StatusCode::OK, "{b}");
    let run_id = run_id_of(&b);
    assert_eq!(
        b,
        json!({"status": "run_started",
               "run_id": run_id,
               "message": RUN_STARTED,
               "alert": null})
    );
    assert_eq!(wait_run_finished(&pool, run_id).await["status"], "done");
    let all = audit_for(&pool, u.id).await;
    // The worker's entry is filed under the run id; the ask's under one request id.
    assert_eq!(
        events(&all),
        [
            "gate_decided",
            "charged",
            "accepted",
            "run_started",
            "run_finished"
        ]
    );
    assert_eq!(all[4].1, Some(run_id));
    let rows = all[..4].to_vec();
    one_request_id(&rows);
    assert_eq!(rows[3].2["run_id"], run_id.to_string());
    assert_eq!(rows[3].2["mode"], "focused");
    // (812 * 1 + 37 * 5) / 1e6 USD * 88 * 100 = 8.7736 paise -> 9.
    assert_eq!(rows[1].2["paise"], 9);
    assert_eq!(rows[1].2["input_tokens"], 812);
    let spend = spend_for(&pool, u.id).await;
    assert_eq!(
        spend,
        vec![(
            "charge".to_string(),
            Some(GATE_MODEL.to_string()),
            Some(812),
            Some(37),
            9
        )]
    );
}

#[tokio::test]
async fn accepted_by_rule_needs_no_model_and_shows_alert() {
    let Some(pool) = test_pool().await else {
        return;
    };
    // A cap just above the Focused run's limit (4000) puts its reservation past 80%;
    // the gate estimate alone stays far below it.
    let est = estimate_paise(RULE_QUESTION);
    let cap = 4000 * 100 / 85;
    assert!(est * 100 < cap * 80, "{est} / {cap}");
    let u = make_user(&pool, "rule-alert", Role::Surgeon, cap).await;
    let app = api_routes(state_with(pool.clone(), Arc::new(MustNotCall), true));
    let (_, b) = call(app, ask_req(&u.email, RULE_QUESTION)).await;
    assert_eq!(b["status"], "run_started", "{b}");
    assert!(b["alert"].as_str().unwrap().contains("80%"), "{b}");
    let run_id = run_id_of(&b);
    wait_run_finished(&pool, run_id).await;
    let rows = audit_for(&pool, u.id).await;
    assert_eq!(
        events(&rows),
        ["gate_decided", "accepted", "run_started", "run_finished"]
    );
    one_request_id(&rows[..3]);
    let spend = spend_for(&pool, u.id).await;
    assert_eq!(spend.len(), 1);
    assert_eq!((spend[0].0.as_str(), spend[0].4), ("charge", 0));
}

#[tokio::test]
async fn question_text_never_persisted() {
    let Some(pool) = test_pool().await else {
        return;
    };
    // Database half only; tracing output is checked in tests/ask_logging.rs.
    let marker = format!("zqxmarker{}", uuid::Uuid::new_v4().simple());
    let u = make_user(&pool, "persist", Role::Surgeon, 30_000).await;
    let questions = [
        (
            Fixed::verdict(Verdict::Medical, "ok"),
            format!("{MODEL_QUESTION} {marker}"),
        ),
        (
            Fixed::verdict(Verdict::NotMedical, "no"),
            format!("{MODEL_QUESTION} {marker}"),
        ),
        (
            Fixed::error(bad_response(GATE_MODEL, 812, 150)),
            format!("{MODEL_QUESTION} {marker}"),
        ),
        (
            Fixed::verdict(Verdict::Medical, "ok"),
            format!("{RULE_QUESTION} {marker}"),
        ),
        (
            Fixed::verdict(Verdict::Medical, "ok"),
            format!("Mrs. Lakshmi ph 9845012345 {marker}"),
        ),
    ];
    for (gate, q) in questions {
        let app = api_routes(state_with(pool.clone(), gate, true));
        let (s, b) = call(app, ask_req(&u.email, &q)).await;
        assert_eq!(s, StatusCode::OK);
        // One active run per user: let each run finish before the next question.
        if b["status"] == "run_started" {
            wait_run_finished(&pool, run_id_of(&b)).await;
        }
    }

    let audit_hits: i64 =
        sqlx::query_scalar("SELECT count(*) FROM incisive.audit a WHERE a::text LIKE $1")
            .bind(format!("%{marker}%"))
            .fetch_one(&pool)
            .await
            .unwrap();
    let spend_hits: i64 =
        sqlx::query_scalar("SELECT count(*) FROM incisive.spend s WHERE s::text LIKE $1")
            .bind(format!("%{marker}%"))
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!((audit_hits, spend_hits), (0, 0));
    // accepted by model 5 (+ run_started, run_finished), refused by model 2,
    // bad gate answer 2 (charged + gate_failed), accepted by rule 4, blocked 1.
    assert_eq!(audit_for(&pool, u.id).await.len(), 5 + 2 + 2 + 4 + 1);
}

// ---------- input limits ----------

#[tokio::test]
async fn empty_and_oversized_questions_rejected_before_anything_else() {
    // No database: if the pipeline touched the audit log, spend or the gate,
    // these would fail (unreachable pool) or panic (MustNotCall).
    let st = state_with(unreachable_pool(), Arc::new(MustNotCall), true);
    let user = User {
        id: 1,
        email: "x@example.com".into(),
        display_name: "X".into(),
        role: Role::Surgeon,
        daily_cap_paise: 30_000,
        monthly_cap_paise: None,
        active: true,
    };
    for q in ["", "   \n\t  "] {
        assert_eq!(
            run_ask(&st, &user, q, RunMode::Focused).await,
            Ok(AskOutcome::InvalidInput {
                message: "Please type your clinical question.".into()
            }),
            "{q:?}"
        );
    }
    // Over 4,000 characters (counted as characters, not bytes), even with an identifier.
    let long = format!("Mrs. Lakshmi ph 9845012345 {}", "é".repeat(4_000));
    assert_eq!(
        run_ask(&st, &user, &long, RunMode::Focused).await,
        Ok(AskOutcome::InvalidInput {
            message:
                "That question is too long for Incisive (4,000 characters max). Please shorten it."
                    .into()
        })
    );
    // Exactly 4,000 characters is allowed through to the next step (here: the
    // unreachable audit log, so a record failure rather than a length refusal).
    let exact = format!("Mrs. Lakshmi ph 9845012345 {}", "a".repeat(4_000 - 27));
    assert_eq!(exact.chars().count(), 4_000);
    assert_eq!(
        run_ask(&st, &user, &exact, RunMode::Focused).await,
        Err(AskFailure::Record)
    );
}

#[tokio::test]
async fn empty_question_is_not_audited_or_charged() {
    let Some(pool) = test_pool().await else {
        return;
    };
    let u = make_user(&pool, "empty", Role::Surgeon, 30_000).await;
    let app = || api_routes(state_with(pool.clone(), Arc::new(MustNotCall), true));
    let (_, b) = call(app(), ask_req(&u.email, "  ")).await;
    assert_eq!(
        b,
        json!({"status": "invalid_input", "message": "Please type your clinical question."})
    );
    let (_, b) = call(app(), ask_req(&u.email, &"a".repeat(4_001))).await;
    assert_eq!(b["status"], "invalid_input");
    assert!(audit_for(&pool, u.id).await.is_empty());
    assert!(spend_for(&pool, u.id).await.is_empty());
}

#[tokio::test]
async fn server_function_body_limit_is_64_kib() {
    let st = state_with(unreachable_pool(), Arc::new(MustNotCall), true);
    let body = format!("question={}", "a".repeat(70 * 1024));
    let req = Request::post("/api/fn/ask")
        .header("content-type", "application/x-www-form-urlencoded")
        // Browsers and curl send it; the limit rejects on it before any work.
        .header("content-length", body.len())
        .header("x-dev-user-email", "x@example.com")
        .body(Body::from(body))
        .unwrap();
    let (s, _) = call(router(st), req).await;
    assert_eq!(s, StatusCode::PAYLOAD_TOO_LARGE);
}

#[tokio::test]
async fn audit_failure_fails_closed() {
    // No database at all: the first audit append fails, so nothing may be answered.
    let st = state_with(unreachable_pool(), Arc::new(MustNotCall), true);
    let user = User {
        id: 1,
        email: "x@example.com".into(),
        display_name: "X".into(),
        role: Role::Surgeon,
        daily_cap_paise: 30_000,
        monthly_cap_paise: None,
        active: true,
    };
    let r = run_ask(&st, &user, "Mrs. Lakshmi ph 9845012345", RunMode::Focused).await;
    assert!(matches!(r, Err(AskFailure::Record)), "{r:?}");
    let r = run_ask(&st, &user, RULE_QUESTION, RunMode::Focused).await;
    assert!(r.is_err(), "{r:?}");
    assert!(!matches!(r, Ok(AskOutcome::RunStarted { .. })));

    // The routes turn this into a 500 carrying this plain advice.
    assert_eq!(
        AskFailure::Record.message(),
        "Incisive couldn't record this request. Please try again."
    );
}

#[tokio::test]
async fn run_started_audit_failure_undoes_the_run() {
    let Some(pool) = test_pool().await else {
        return;
    };
    let u = make_user(&pool, "runstart-fail", Role::Surgeon, 30_000).await;
    // Make the `run_started` append fail for this user only.
    let tag = uuid::Uuid::new_v4().simple().to_string();
    let func = format!("incisive.fail_run_started_{tag}");
    let ddl = |sql: String| {
        let pool = pool.clone();
        async move {
            // Never wait long for the audit table's lock.
            let mut tx = pool.begin().await.unwrap();
            sqlx::query("SET LOCAL lock_timeout = '5s'")
                .execute(&mut *tx)
                .await
                .unwrap();
            sqlx::query(&sql).execute(&mut *tx).await.unwrap();
            tx.commit().await.unwrap();
        }
    };
    ddl(format!(
        "CREATE FUNCTION {func}() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN \
           IF NEW.user_id = {uid} AND NEW.event = 'run_started' THEN \
             RAISE EXCEPTION 'injected audit failure'; END IF; RETURN NEW; END $$",
        uid = u.id
    ))
    .await;
    ddl(format!(
        "CREATE TRIGGER fail_run_started_{tag} BEFORE INSERT ON incisive.audit \
         FOR EACH ROW EXECUTE FUNCTION {func}()"
    ))
    .await;

    // In its own task: a panic there still lets the trigger be dropped below.
    let st = state_with(pool.clone(), Arc::new(MustNotCall), true);
    let user = u.clone();
    let r = tokio::spawn(async move { run_ask(&st, &user, RULE_QUESTION, RunMode::Focused).await })
        .await;

    ddl(format!(
        "DROP TRIGGER fail_run_started_{tag} ON incisive.audit"
    ))
    .await;
    ddl(format!("DROP FUNCTION {func}()")).await;
    let r = match r {
        Ok(r) => r,
        Err(e) => std::panic::resume_unwind(e.into_panic()),
    };

    assert_eq!(r, Err(AskFailure::Record));
    let runs: i64 = sqlx::query_scalar("SELECT count(*) FROM incisive.runs WHERE user_id = $1")
        .bind(u.id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(runs, 0, "the run is deleted");
    let spend = spend_for(&pool, u.id).await;
    assert!(
        spend.iter().all(|r| r.0 != "reservation"),
        "the reservation is released: {spend:?}"
    );
    assert_eq!(
        events(&audit_for(&pool, u.id).await),
        ["gate_decided", "accepted"]
    );
}

// ---------- identity, /api/me, activity ----------

#[tokio::test]
async fn prod_mode_ignores_dev_header() {
    let Some(pool) = test_pool().await else {
        return;
    };
    let u = make_user(&pool, "prod", Role::Owner, 30_000).await;
    // A rejection has no user and a fresh request id, so scope the check to
    // rows appended after this point (the log is shared with other tests).
    let before: i64 = sqlx::query_scalar("SELECT COALESCE(max(seq), 0) FROM incisive.audit")
        .fetch_one(&pool)
        .await
        .unwrap();
    let app = api_routes(state_with(pool.clone(), Arc::new(MustNotCall), false));
    let (s, b) = call(app, ask_req(&u.email, RULE_QUESTION)).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    assert_eq!(b["error"], "not_signed_in");
    assert!(audit_for(&pool, u.id).await.is_empty());
    let mine: Vec<(Option<i64>, Value)> = sqlx::query_as(
        "SELECT user_id, detail FROM incisive.audit \
         WHERE event = 'identity_rejected' AND seq > $1 ORDER BY seq",
    )
    .bind(before)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert!(
        mine.contains(&(None, json!({"reason": "not_signed_in"}))),
        "identity rejection is audited: {mine:?}"
    );

    // /api/me and /api/activity ignore it too.
    for path in ["/api/me", "/api/activity"] {
        let app = api_routes(state_with(pool.clone(), Arc::new(MustNotCall), false));
        let req = Request::get(path)
            .header("x-dev-user-email", &u.email)
            .body(Body::empty())
            .unwrap();
        assert_eq!(call(app, req).await.0, StatusCode::UNAUTHORIZED, "{path}");
    }
}

/// The IST calendar day containing now.
fn ist_today() -> chrono::NaiveDate {
    (chrono::Utc::now() + chrono::Duration::minutes(5 * 60 + 30)).date_naive()
}

#[tokio::test]
async fn me_reports_name_role_and_spend() {
    let Some(pool) = test_pool().await else {
        return;
    };
    // `today_paise` is for the current IST day, read from the clock. If the run
    // straddles IST midnight (18:30 UTC), retry once with a fresh user.
    for attempt in 0.. {
        let day = ist_today();
        let u = make_user(&pool, "me", Role::Surgeon, 30_000).await;
        let gate = Fixed::verdict(Verdict::Medical, "ok");
        let (_, started) = call(
            api_routes(state_with(pool.clone(), gate, true)),
            ask_req(&u.email, MODEL_QUESTION),
        )
        .await;
        // Until the run ends its reservation counts too; afterwards only charges.
        wait_run_finished(&pool, run_id_of(&started)).await;
        let req = Request::get("/api/me")
            .header("x-dev-user-email", &u.email)
            .body(Body::empty())
            .unwrap();
        let (s, b) = call(
            api_routes(state_with(pool.clone(), Arc::new(MustNotCall), true)),
            req,
        )
        .await;
        if ist_today() != day {
            assert_eq!(attempt, 0, "IST day changed twice during one test");
            continue;
        }
        assert_eq!(s, StatusCode::OK);
        assert_eq!(
            b,
            json!({"display_name": "Dr me", "role": "surgeon",
                   "today_paise": 9, "daily_cap_paise": 30_000})
        );
        break;
    }
}

#[tokio::test]
async fn activity_is_owner_only() {
    let Some(pool) = test_pool().await else {
        return;
    };
    let surgeon = make_user(&pool, "act-s", Role::Surgeon, 30_000).await;
    let owner = make_user(&pool, "act-o", Role::Owner, 150_000).await;
    // Something of the owner's own to find in the list.
    call(
        api_routes(state_with(pool.clone(), Arc::new(MustNotCall), true)),
        ask_req(&owner.email, "bitcoin price tomorrow"),
    )
    .await;
    let get = |email: &str| {
        Request::get("/api/activity?limit=50")
            .header("x-dev-user-email", email)
            .body(Body::empty())
            .unwrap()
    };
    let app = || api_routes(state_with(pool.clone(), Arc::new(MustNotCall), true));

    let (s, b) = call(app(), get(&surgeon.email)).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    assert_eq!(b["error"], "owner_only");
    assert!(b.get("entries").is_none());

    let (s, b) = call(app(), get(&owner.email)).await;
    assert_eq!(s, StatusCode::OK, "{b}");
    let entries = b["entries"].as_array().unwrap();
    assert!(entries.len() <= 50 && !entries.is_empty());
    // Newest first, consecutive sequence numbers.
    let seqs: Vec<i64> = entries.iter().map(|e| e["seq"].as_i64().unwrap()).collect();
    assert!(seqs.windows(2).all(|w| w[0] > w[1]), "{seqs:?}");

    // Other tests append to the shared log concurrently, so the owner's entry
    // may already have dropped out of the newest 50. Find it by its seq with
    // the largest window that still reaches back to it.
    let my_seq: i64 = sqlx::query_scalar(
        "SELECT seq FROM incisive.audit WHERE user_id = $1 ORDER BY seq DESC LIMIT 1",
    )
    .bind(owner.id)
    .fetch_one(&pool)
    .await
    .unwrap();
    let (_, b) = call(
        app(),
        Request::get("/api/activity?limit=500")
            .header("x-dev-user-email", &owner.email)
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    let entries = b["entries"].as_array().unwrap();
    assert!(entries.len() <= 500);
    let mine = entries
        .iter()
        .find(|e| e["seq"] == my_seq)
        .unwrap_or_else(|| panic!("seq {my_seq} not in the newest 500 entries"));
    assert_eq!(mine["user_email"], owner.email.as_str());
    assert_eq!(mine["event"], "gate_decided");
    assert!(
        mine["time_ist"].as_str().unwrap().ends_with("IST"),
        "{mine}"
    );
    assert!(mine["detail"].is_string());
    assert!(
        matches!(b["chain"]["status"].as_str(), Some("valid" | "broken")),
        "{b}"
    );
    assert!(b["chain"]["label"].as_str().unwrap().starts_with("Chain "));
}

// ---------- server function and page ----------

fn router(st: AppState) -> Router {
    let opts = LeptosOptions::builder().output_name("incisive-app").build();
    build_router(Arc::new(AlwaysUp), st, opts)
}

struct AlwaysUp;

#[async_trait]
impl incisive_app::health::Readiness for AlwaysUp {
    async fn db_ok(&self) -> bool {
        true
    }
}

#[tokio::test]
async fn server_function_uses_the_same_pipeline() {
    let Some(pool) = test_pool().await else {
        return;
    };
    let u = make_user(&pool, "sfn", Role::Surgeon, 30_000).await;
    let req = Request::post("/api/fn/ask")
        .header("content-type", "application/x-www-form-urlencoded")
        .header("x-dev-user-email", &u.email)
        .body(Body::from("question=Mrs.+Lakshmi+ph+9845012345"))
        .unwrap();
    let (s, b) = call(
        router(state_with(pool.clone(), Arc::new(MustNotCall), true)),
        req,
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{b}");
    assert_eq!(b["status"], "blocked_identifiers", "{b}");
    assert_eq!(events(&audit_for(&pool, u.id).await), ["deid_blocked"]);
}

#[tokio::test]
async fn server_function_ignores_dev_header_in_prod() {
    // Prod: the dev header is not an identity. Rejected before any user lookup.
    let st = state_with(unreachable_pool(), Arc::new(MustNotCall), false);
    let req = Request::post("/api/fn/ask")
        .header("content-type", "application/x-www-form-urlencoded")
        .header("x-dev-user-email", "owner@example.com")
        .body(Body::from("question=hello"))
        .unwrap();
    let res = router(st).oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    let body = res.into_body().collect().await.unwrap().to_bytes();
    let text = String::from_utf8_lossy(&body);
    assert!(text.contains("You're not signed in to Incisive"), "{text}");
}

#[tokio::test]
async fn server_function_activity_is_403_for_non_owner_page_stays_200() {
    let Some(pool) = test_pool().await else {
        return;
    };
    let surgeon = make_user(&pool, "fn-act", Role::Surgeon, 30_000).await;
    let app = || router(state_with(pool.clone(), Arc::new(MustNotCall), true));
    let req = Request::post("/api/fn/activity")
        .header("x-dev-user-email", &surgeon.email)
        .body(Body::empty())
        .unwrap();
    let res = app().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN);

    // The same check while rendering the page must not change the page status.
    let req = Request::get("/activity")
        .header("x-dev-user-email", &surgeon.email)
        .body(Body::empty())
        .unwrap();
    let res = app().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = res.into_body().collect().await.unwrap().to_bytes();
    assert!(String::from_utf8_lossy(&body).contains("Only the Incisive owner"));
}

// ---------- same-origin check ----------

fn post_with(path: &str, origin: Option<&str>, host: &str, email: &str) -> Request<Body> {
    let mut b = Request::post(path)
        .header("host", host)
        .header("x-dev-user-email", email);
    if let Some(o) = origin {
        b = b.header("origin", o);
    }
    if path == "/api/ask" {
        b.header("content-type", "application/json")
            .body(Body::from(json!({ "question": " " }).to_string()))
            .unwrap()
    } else {
        b.header("content-type", "application/x-www-form-urlencoded")
            .body(Body::from("question=+"))
            .unwrap()
    }
}

#[tokio::test]
async fn cross_site_posts_are_refused_before_anything_else() {
    // Unreachable database and a gate that panics: a 403 here proves nothing
    // past the check ran (identity would need the database).
    for path in ["/api/ask", "/api/fn/ask", "/api/fn/activity"] {
        for origin in [
            "https://evil.example",
            "http://incisive.test:9999",
            "http://incisive.test",
            "http://evil.example:8080",
            "null",
            "not a url",
        ] {
            let st = state_with(unreachable_pool(), Arc::new(MustNotCall), true);
            let req = post_with(path, Some(origin), "incisive.test:8080", "x@example.com");
            let (s, b) = call(router(st), req).await;
            assert_eq!(s, StatusCode::FORBIDDEN, "{path} {origin}");
            assert_eq!(
                b,
                json!({"error": "cross_site", "message": "Request blocked: cross-site request."}),
                "{path} {origin}"
            );
        }
    }
    // api_routes on its own is protected too.
    let st = state_with(unreachable_pool(), Arc::new(MustNotCall), true);
    let req = post_with(
        "/api/ask",
        Some("https://evil.example"),
        "incisive.test",
        "x@example.com",
    );
    assert_eq!(call(api_routes(st), req).await.0, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn same_origin_and_missing_origin_are_allowed() {
    let Some(pool) = test_pool().await else {
        return;
    };
    let u = make_user(&pool, "origin", Role::Surgeon, 30_000).await;
    for (origin, host) in [
        (Some("http://incisive.test:8080"), "incisive.test:8080"),
        (Some("https://Incisive.Test"), "incisive.test"),
        (Some("https://incisive.test"), "incisive.test:443"),
        (Some("http://incisive.test"), "incisive.test:80"),
        // curl and server-side clients send no Origin.
        (None, "incisive.test"),
    ] {
        for path in ["/api/ask", "/api/fn/ask"] {
            let st = state_with(pool.clone(), Arc::new(MustNotCall), true);
            let (s, b) = call(router(st), post_with(path, origin, host, &u.email)).await;
            assert_eq!(s, StatusCode::OK, "{path} {origin:?} {host}: {b}");
            assert_eq!(b["status"], "invalid_input", "{path} {origin:?}: {b}");
        }
    }
}

#[tokio::test]
async fn home_page_renders_question_box() {
    let st = state_with(unreachable_pool(), Arc::new(MustNotCall), false);
    let res = router(st)
        .oneshot(Request::get("/").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body =
        String::from_utf8(res.into_body().collect().await.unwrap().to_bytes().to_vec()).unwrap();
    assert!(body.contains("Incisive"));
    assert!(body.contains("Cut to the evidence."));
    assert!(body.contains("<textarea"), "{body}");
    assert!(body.contains("Send"));
}
