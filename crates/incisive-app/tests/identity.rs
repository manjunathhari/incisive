//! Identity tests. JWT tests use test-only ES256 keys from `tests/fixtures/`
//! (never used anywhere real). DB tests are gated on `TEST_DATABASE_URL`.

use std::time::{SystemTime, UNIX_EPOCH};

use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode},
};
use base64::Engine;
use http_body_util::BodyExt;
use incisive_app::config::AppEnv;
use incisive_app::db::{lazy_pool, migrate};
use incisive_app::gate::OfflineClassifier;
use incisive_app::identity::{
    IapVerifier, IdentityError, JwkFetch, JwkSource, Role, ensure_owner, find_user,
};
use incisive_app::server::{AppState, api_routes};
use incisive_app::spend::{CapsConfig, Prices};
use jsonwebtoken::{Algorithm, EncodingKey, Header, jwk::JwkSet};
use secrecy::SecretString;
use serde_json::json;
use tower::ServiceExt;

const AUD: &str = "/projects/123/apps/456";
const ISS: &str = "https://cloud.google.com/iap";
const PRIV_A: &str = include_str!("fixtures/test-only-es256-a-private.pem");
const PUB_A: &str = include_str!("fixtures/test-only-es256-a-public.pem");
const PRIV_B: &str = include_str!("fixtures/test-only-es256-b-private.pem");

fn jwk_set() -> JwkSet {
    // Public half of test-only key A, published under kid "test-kid-a".
    serde_json::from_value(json!({"keys": [{
        "kty": "EC", "crv": "P-256", "alg": "ES256", "use": "sig", "kid": "test-kid-a",
        "x": "ypYhjPDz0LD5Fn0cQ5vh0kkTlaDJs-nnNt0GUBFsIAc",
        "y": "aRxTxOygykCAIPtTPNsG9U24GycfiiL7pdX0N-ZFoOs"
    }]}))
    .unwrap()
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

fn claims(email: &str, aud: &str, iss: &str, exp: u64) -> serde_json::Value {
    json!({"email": email, "aud": aud, "iss": iss, "iat": now(), "exp": exp, "sub": "accounts.google.com:1"})
}

fn sign(kid: &str, key: &str, c: &serde_json::Value) -> String {
    let mut h = Header::new(Algorithm::ES256);
    h.kid = Some(kid.to_string());
    jsonwebtoken::encode(&h, c, &EncodingKey::from_ec_pem(key.as_bytes()).unwrap()).unwrap()
}

fn good_token(email: &str) -> String {
    sign("test-kid-a", PRIV_A, &claims(email, AUD, ISS, now() + 3600))
}

fn verifier() -> IapVerifier {
    IapVerifier::new(AUD.to_string(), JwkSource::Static(jwk_set()))
}

fn invalid(r: Result<incisive_app::identity::VerifiedIdentity, IdentityError>) {
    match r {
        Err(IdentityError::Invalid(_)) => {}
        other => panic!("expected Invalid, got {other:?}"),
    }
}

#[tokio::test]
async fn valid_token_yields_email() {
    let id = verifier()
        .verify(&good_token("Manj@Example.com"))
        .await
        .unwrap();
    assert_eq!(id.email, "manj@example.com");
}

#[tokio::test]
async fn wrong_audience_rejected() {
    let t = sign(
        "test-kid-a",
        PRIV_A,
        &claims("a@b.com", "/projects/9/apps/9", ISS, now() + 3600),
    );
    invalid(verifier().verify(&t).await);
}

#[tokio::test]
async fn expired_rejected() {
    let t = sign(
        "test-kid-a",
        PRIV_A,
        &claims("a@b.com", AUD, ISS, now() - 3600),
    );
    invalid(verifier().verify(&t).await);
}

#[tokio::test]
async fn wrong_issuer_rejected() {
    let t = sign(
        "test-kid-a",
        PRIV_A,
        &claims("a@b.com", AUD, "https://evil.example", now() + 3600),
    );
    invalid(verifier().verify(&t).await);
}

#[tokio::test]
async fn unknown_kid_rejected() {
    // Signed by a different key under a kid the key set does not contain.
    let t = sign(
        "test-kid-b",
        PRIV_B,
        &claims("a@b.com", AUD, ISS, now() + 3600),
    );
    invalid(verifier().verify(&t).await);
}

#[tokio::test]
async fn known_kid_but_wrong_key_rejected() {
    let t = sign(
        "test-kid-a",
        PRIV_B,
        &claims("a@b.com", AUD, ISS, now() + 3600),
    );
    invalid(verifier().verify(&t).await);
}

#[tokio::test]
async fn alg_none_rejected() {
    let b64 = |v: serde_json::Value| {
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(v.to_string())
    };
    let t = format!(
        "{}.{}.",
        b64(json!({"alg": "none", "typ": "JWT", "kid": "test-kid-a"})),
        b64(claims("a@b.com", AUD, ISS, now() + 3600))
    );
    invalid(verifier().verify(&t).await);
}

#[tokio::test]
async fn hs256_with_public_key_as_secret_rejected() {
    let mut h = Header::new(Algorithm::HS256);
    h.kid = Some("test-kid-a".to_string());
    let t = jsonwebtoken::encode(
        &h,
        &claims("a@b.com", AUD, ISS, now() + 3600),
        &EncodingKey::from_secret(PUB_A.as_bytes()),
    )
    .unwrap();
    invalid(verifier().verify(&t).await);
}

#[tokio::test]
async fn unreachable_remote_keys_are_keys_unavailable() {
    // Loopback port 1: refused immediately, no external network.
    let v = IapVerifier::new(
        AUD.to_string(),
        JwkSource::Remote("http://127.0.0.1:1/keys".parse().unwrap()),
    );
    let r = v.verify(&good_token("a@b.com")).await;
    assert_eq!(r, Err(IdentityError::KeysUnavailable));
}

#[tokio::test]
async fn garbage_rejected() {
    invalid(verifier().verify("not-a-jwt").await);
}

// ---- router level ----

fn state_with(env: AppEnv, pool: sqlx::PgPool, verifier: IapVerifier) -> AppState {
    AppState {
        pool,
        verifier: std::sync::Arc::new(verifier),
        gate: std::sync::Arc::new(OfflineClassifier),
        prices: Prices::default(),
        usd_inr: 88.0,
        caps: CapsConfig {
            global_monthly_paise: 1_000_000,
        },
        gate_model: "unused".into(),
        dev_mode: env == AppEnv::Dev,
        runs: incisive_app::runs::RunsConfig::default(),
        // Phase 2 tests: Quick keeps the classic pipeline unless a test says otherwise.
        quick_engine: incisive_app::runs::QuickEngine::Classic,
        full_engine: incisive_app::runs::Engine::Classic,
        worker: incisive_app::runs::worker::WorkerHandle::default(),
        llm: None,
        lit: incisive_app::agent::testing::offline_literature(),
    }
}

fn state(env: AppEnv, pool: sqlx::PgPool) -> AppState {
    state_with(env, pool, verifier())
}

fn unused_pool() -> sqlx::PgPool {
    // Lazy and never touched: requests in these tests are rejected before any query.
    lazy_pool(&SecretString::from(
        "postgres://u:p@127.0.0.1:1/none".to_string(),
    ))
    .unwrap()
}

async fn call(app: Router, req: Request<Body>) -> (StatusCode, serde_json::Value) {
    let res = app.oneshot(req).await.unwrap();
    let status = res.status();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null),
    )
}

#[tokio::test]
async fn missing_header_is_401() {
    let app = api_routes(state(AppEnv::Prod, unused_pool()));
    let (s, b) = call(app, Request::get("/api/me").body(Body::empty()).unwrap()).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    assert_eq!(b, json!({"error": "not_signed_in"}));
}

#[tokio::test]
async fn invalid_jwt_is_401() {
    let app = api_routes(state(AppEnv::Prod, unused_pool()));
    let req = Request::get("/api/me")
        .header("x-goog-iap-jwt-assertion", "garbage")
        .body(Body::empty())
        .unwrap();
    let (s, b) = call(app, req).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    assert_eq!(b, json!({"error": "not_signed_in"}));
}

#[tokio::test]
async fn unverified_email_header_ignored() {
    let app = api_routes(state(AppEnv::Prod, unused_pool()));
    let req = Request::get("/api/me")
        .header(
            "x-goog-authenticated-user-email",
            "accounts.google.com:owner@example.com",
        )
        .body(Body::empty())
        .unwrap();
    let (s, _) = call(app, req).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn dev_header_works_only_in_dev() {
    let req = || {
        Request::get("/api/me")
            .header("x-dev-user-email", "someone@example.com")
            .body(Body::empty())
            .unwrap()
    };
    // Prod: header ignored, no database touched.
    let (s, _) = call(api_routes(state(AppEnv::Prod, unused_pool())), req()).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);

    // Dev: header is honoured (needs a database to find the user).
    let Some(pool) = test_pool().await else {
        return;
    };
    let owner = ensure_owner(&pool, "dev-header@example.com", "Dev")
        .await
        .unwrap();
    let req = Request::get("/api/me")
        .header("x-dev-user-email", "Dev-Header@Example.com")
        .body(Body::empty())
        .unwrap();
    let (s, b) = call(api_routes(state(AppEnv::Dev, pool)), req).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(b["display_name"], owner.display_name);
    assert_eq!(b["role"], "owner");
}

// ---- database gated ----

async fn test_pool() -> Option<sqlx::PgPool> {
    let Ok(url) = std::env::var("TEST_DATABASE_URL") else {
        eprintln!("SKIP: TEST_DATABASE_URL not set");
        return None;
    };
    let pool = lazy_pool(&SecretString::from(url)).expect("lazy pool");
    migrate(&pool).await.expect("migrate");
    Some(pool)
}

async fn insert_user(pool: &sqlx::PgPool, email: &str, active: bool) {
    sqlx::query(
        "INSERT INTO incisive.users (email, display_name, role, daily_cap_paise, active) \
         VALUES ($1, 'T', 'surgeon', 30000, $2) \
         ON CONFLICT (email) DO UPDATE SET active = EXCLUDED.active",
    )
    .bind(email)
    .bind(active)
    .execute(pool)
    .await
    .unwrap();
}

fn iap_req(token: &str) -> Request<Body> {
    Request::get("/api/me")
        .header("x-goog-iap-jwt-assertion", token)
        .body(Body::empty())
        .unwrap()
}

#[tokio::test]
async fn verified_but_unknown_user_is_403_not_set_up() {
    let Some(pool) = test_pool().await else {
        return;
    };
    let app = api_routes(state(AppEnv::Prod, pool));
    let (s, b) = call(app, iap_req(&good_token("nobody-known@example.com"))).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    assert_eq!(b["error"], "not_set_up");
    assert_eq!(
        b["message"],
        "Your Google account is allowed in, but you haven't been set up in Incisive yet. Ask the owner to add you."
    );
}

#[tokio::test]
async fn inactive_user_is_403() {
    let Some(pool) = test_pool().await else {
        return;
    };
    insert_user(&pool, "inactive-user@example.com", false).await;
    let app = api_routes(state(AppEnv::Prod, pool));
    let (s, b) = call(app, iap_req(&good_token("Inactive-User@example.com"))).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    assert_eq!(b["error"], "not_set_up");
}

#[tokio::test]
async fn active_user_gets_through() {
    let Some(pool) = test_pool().await else {
        return;
    };
    insert_user(&pool, "active-user@example.com", true).await;
    let app = api_routes(state(AppEnv::Prod, pool.clone()));
    let (s, b) = call(app, iap_req(&good_token("active-user@example.com"))).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(b["display_name"], "T");
    assert_eq!(b["role"], "surgeon");
    assert_eq!(b["daily_cap_paise"], 30000);
    assert!(b["today_paise"].is_i64());
    let u = find_user(&pool, "ACTIVE-user@example.com")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(u.role, Role::Surgeon);
}

#[tokio::test]
async fn ensure_owner_is_idempotent() {
    let Some(pool) = test_pool().await else {
        return;
    };
    let email = "idempotent-owner@example.com";
    let a = ensure_owner(&pool, email, "Owner").await.unwrap();
    let b = ensure_owner(&pool, "Idempotent-Owner@Example.com", "Owner")
        .await
        .unwrap();
    assert_eq!(a.id, b.id);
    assert_eq!(b.role, Role::Owner);
    assert_eq!(b.daily_cap_paise, 150_000);
    assert_eq!(b.monthly_cap_paise, None);
    assert!(b.active);
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM incisive.users WHERE email = $1")
        .bind(email)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(n, 1);
}

// ---- cold-start key fetch ----

async fn counting_listener() -> (
    std::net::SocketAddr,
    std::sync::Arc<std::sync::atomic::AtomicUsize>,
) {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    let n = Arc::new(AtomicUsize::new(0));
    let n2 = n.clone();
    tokio::spawn(async move {
        loop {
            if let Ok((sock, _)) = l.accept().await {
                n2.fetch_add(1, Ordering::SeqCst);
                drop(sock); // close without answering: fetch fails
            }
        }
    });
    (addr, n)
}

/// Fails every fetch and counts the attempts. No sockets: with Tokio's paused
/// clock, real I/O lets the runtime auto-advance time (past reqwest's 5 s
/// timeout and the 10 s retry interval), which made this test flaky.
struct FailingFetch(std::sync::atomic::AtomicUsize);

#[async_trait::async_trait]
impl JwkFetch for FailingFetch {
    async fn fetch(&self) -> Result<JwkSet, ()> {
        self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Err(())
    }
}

#[tokio::test(start_paused = true)]
async fn empty_cache_retries_after_short_interval() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let fetch = std::sync::Arc::new(FailingFetch(AtomicUsize::new(0)));
    let v = IapVerifier::new(AUD.to_string(), JwkSource::Fetch(fetch.clone()));
    let hits = || fetch.0.load(Ordering::SeqCst);
    let t = good_token("a@b.com");
    assert_eq!(v.verify(&t).await, Err(IdentityError::KeysUnavailable));
    assert_eq!(hits(), 1);
    // Within the short interval: no new fetch.
    tokio::time::advance(std::time::Duration::from_secs(5)).await;
    assert_eq!(v.verify(&t).await, Err(IdentityError::KeysUnavailable));
    assert_eq!(hits(), 1);
    // After it: one more attempt, long before the 5-minute throttle.
    tokio::time::advance(std::time::Duration::from_secs(6)).await;
    assert_eq!(v.verify(&t).await, Err(IdentityError::KeysUnavailable));
    assert_eq!(hits(), 2);
    // And again only after another full interval.
    tokio::time::advance(std::time::Duration::from_secs(9)).await;
    assert_eq!(v.verify(&t).await, Err(IdentityError::KeysUnavailable));
    assert_eq!(hits(), 2);
}

#[tokio::test]
async fn remote_fetch_failure_is_keys_unavailable() {
    // The real HTTP path (real clock): a server that closes the connection.
    let (addr, hits) = counting_listener().await;
    let v = IapVerifier::new(
        AUD.to_string(),
        JwkSource::Remote(format!("http://{addr}/keys").parse().unwrap()),
    );
    let t = good_token("a@b.com");
    assert_eq!(v.verify(&t).await, Err(IdentityError::KeysUnavailable));
    assert_eq!(hits.load(std::sync::atomic::Ordering::SeqCst), 1);
}

#[tokio::test]
async fn keys_unavailable_is_503() {
    let verifier = IapVerifier::new(
        AUD.to_string(),
        JwkSource::Remote("http://127.0.0.1:1/keys".parse().unwrap()),
    );
    let app = api_routes(state_with(AppEnv::Prod, unused_pool(), verifier));
    let (s, b) = call(app, iap_req(&good_token("a@b.com"))).await;
    assert_eq!(s, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        b,
        json!({"error": "sign_in_check_unavailable",
               "message": "Sign-in could not be checked just now. Please try again in a moment."})
    );
}
