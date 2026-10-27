//! Ask pipeline tests: identity -> identifier filter -> spend reservation -> gate
//! -> settle -> accepted, every step audited under one request id.
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
use claude_client::Usage;
use http_body_util::BodyExt;
use incisive_app::ask::{AskFailure, AskOutcome, run_ask};
use incisive_app::db::{lazy_pool, migrate};
use incisive_app::gate::{Classifier, GateDecision, GateError, Verdict};
use incisive_app::identity::{IapVerifier, JwkSource, Role, User};
use incisive_app::server::{AppState, api_routes, build_router};
use incisive_app::spend::{CapsConfig, ModelPrice, Prices};
use leptos::config::LeptosOptions;
use secrecy::SecretString;
use serde_json::{Value, json};
use sqlx::{PgPool, Row};
use tower::ServiceExt;

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
         your question. Please retype the question without copy-pasted special characters."
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
    for (tag, err, kind) in [
        ("gatefail-u", GateError::Unavailable, "unavailable"),
        ("gatefail-b", GateError::BadResponse, "bad_response"),
    ] {
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
    assert_eq!(
        b,
        json!({"status": "accepted",
               "message": "Accepted. The evidence agent arrives in Phase 2.",
               "alert": null})
    );
    let rows = audit_for(&pool, u.id).await;
    assert_eq!(events(&rows), ["gate_decided", "charged", "accepted"]);
    one_request_id(&rows);
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
    // Estimate is 16 paise; a 20-paise cap puts the reservation past 80%.
    let u = make_user(&pool, "rule-alert", Role::Surgeon, 20).await;
    let app = api_routes(state_with(pool.clone(), Arc::new(MustNotCall), true));
    let (_, b) = call(app, ask_req(&u.email, RULE_QUESTION)).await;
    assert_eq!(b["status"], "accepted", "{b}");
    assert!(b["alert"].as_str().unwrap().contains("80%"), "{b}");
    let rows = audit_for(&pool, u.id).await;
    assert_eq!(events(&rows), ["gate_decided", "accepted"]);
    one_request_id(&rows);
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
            Fixed::error(GateError::BadResponse),
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
        let (s, _) = call(app, ask_req(&u.email, &q)).await;
        assert_eq!(s, StatusCode::OK);
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
    // accepted by model 3, refused by model 2, gate error 1, accepted by rule 2, blocked 1.
    assert_eq!(audit_for(&pool, u.id).await.len(), 3 + 2 + 1 + 2 + 1);
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
            run_ask(&st, &user, q).await,
            Ok(AskOutcome::Unclear {
                message: "Please type your clinical question.".into()
            }),
            "{q:?}"
        );
    }
    // Over 4,000 characters (counted as characters, not bytes), even with an identifier.
    let long = format!("Mrs. Lakshmi ph 9845012345 {}", "é".repeat(4_000));
    assert_eq!(
        run_ask(&st, &user, &long).await,
        Ok(AskOutcome::NotMedical {
            message:
                "That question is too long for Incisive (4,000 characters max). Please shorten it."
                    .into()
        })
    );
    // Exactly 4,000 characters is allowed through to the next step (here: the
    // unreachable audit log, so a record failure rather than a length refusal).
    let exact = format!("Mrs. Lakshmi ph 9845012345 {}", "a".repeat(4_000 - 27));
    assert_eq!(exact.chars().count(), 4_000);
    assert_eq!(run_ask(&st, &user, &exact).await, Err(AskFailure::Record));
}

#[tokio::test]
async fn empty_question_is_not_audited_or_charged() {
    let Some(pool) = test_pool().await else {
        return;
    };
    let u = make_user(&pool, "empty", Role::Surgeon, 30_000).await;
    let app = || api_routes(state_with(pool.clone(), Arc::new(MustNotCall), true));
    let (_, b) = call(app(), ask_req(&u.email, "  ")).await;
    assert_eq!(b["status"], "unclear");
    let (_, b) = call(app(), ask_req(&u.email, &"a".repeat(4_001))).await;
    assert_eq!(b["status"], "not_medical");
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
    let r = run_ask(&st, &user, "Mrs. Lakshmi ph 9845012345").await;
    assert!(matches!(r, Err(AskFailure::Record)), "{r:?}");
    let r = run_ask(&st, &user, RULE_QUESTION).await;
    assert!(r.is_err(), "{r:?}");
    assert!(!matches!(r, Ok(AskOutcome::Accepted { .. })));

    // The routes turn this into a 500 carrying this plain advice.
    assert_eq!(
        AskFailure::Record.message(),
        "Incisive couldn't record this request. Please try again."
    );
}

// ---------- identity, /api/me, activity ----------

#[tokio::test]
async fn prod_mode_ignores_dev_header() {
    let Some(pool) = test_pool().await else {
        return;
    };
    let u = make_user(&pool, "prod", Role::Owner, 30_000).await;
    let count = || async {
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM incisive.audit WHERE event = 'identity_rejected'",
        )
        .fetch_one(&pool)
        .await
        .unwrap()
    };
    let before = count().await;
    let app = api_routes(state_with(pool.clone(), Arc::new(MustNotCall), false));
    let (s, b) = call(app, ask_req(&u.email, RULE_QUESTION)).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    assert_eq!(b["error"], "not_signed_in");
    assert!(audit_for(&pool, u.id).await.is_empty());
    assert!(count().await > before, "identity rejection is audited");
    let latest: Value = sqlx::query_scalar(
        "SELECT detail FROM incisive.audit WHERE event = 'identity_rejected' ORDER BY seq DESC LIMIT 1",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(latest, json!({"reason": "not_signed_in"}));

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

#[tokio::test]
async fn me_reports_name_role_and_spend() {
    let Some(pool) = test_pool().await else {
        return;
    };
    let u = make_user(&pool, "me", Role::Surgeon, 30_000).await;
    let gate = Fixed::verdict(Verdict::Medical, "ok");
    call(
        api_routes(state_with(pool.clone(), gate, true)),
        ask_req(&u.email, MODEL_QUESTION),
    )
    .await;
    let req = Request::get("/api/me")
        .header("x-dev-user-email", &u.email)
        .body(Body::empty())
        .unwrap();
    let (s, b) = call(
        api_routes(state_with(pool, Arc::new(MustNotCall), true)),
        req,
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(
        b,
        json!({"display_name": "Dr me", "role": "surgeon",
               "today_paise": 9, "daily_cap_paise": 30_000})
    );
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
    let mine = entries
        .iter()
        .find(|e| e["user_email"] == owner.email.as_str())
        .expect("owner's own entry is listed");
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
