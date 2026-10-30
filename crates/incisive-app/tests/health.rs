use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode},
};
use http_body_util::BodyExt;
use incisive_app::health::{Readiness, health_routes};
use incisive_app::server::build_router;
use leptos::config::LeptosOptions;
use tower::ServiceExt;

fn identity_state() -> incisive_app::server::AppState {
    use incisive_app::identity::{IapVerifier, JwkSource};
    let pool = incisive_app::db::lazy_pool(&secrecy::SecretString::from(
        "postgres://u:p@127.0.0.1:1/none".to_string(),
    ))
    .unwrap();
    let verifier = IapVerifier::new(
        "aud".into(),
        JwkSource::Static(serde_json::from_str(r#"{"keys":[]}"#).unwrap()),
    );
    incisive_app::server::AppState {
        pool,
        verifier: Arc::new(verifier),
        gate: Arc::new(incisive_app::gate::OfflineClassifier),
        prices: incisive_app::spend::Prices::default(),
        usd_inr: 88.0,
        caps: incisive_app::spend::CapsConfig {
            global_monthly_paise: 1,
        },
        gate_model: "unused".into(),
        dev_mode: false,
        runs: incisive_app::runs::RunsConfig::default(),
        // Phase 2 tests: Quick keeps the classic pipeline unless a test says otherwise.
        quick_engine: incisive_app::runs::QuickEngine::Classic,
        full_engine: incisive_app::runs::Engine::Classic,
        worker: incisive_app::runs::worker::WorkerHandle::default(),
        llm: None,
        lit: incisive_app::agent::testing::offline_literature(),
    }
}

struct AlwaysUp;
struct AlwaysDown;
struct Hangs;

#[async_trait]
impl Readiness for AlwaysUp {
    async fn db_ok(&self) -> bool {
        true
    }
}
#[async_trait]
impl Readiness for AlwaysDown {
    async fn db_ok(&self) -> bool {
        false
    }
}
#[async_trait]
impl Readiness for Hangs {
    async fn db_ok(&self) -> bool {
        tokio::time::sleep(Duration::from_secs(10)).await;
        true
    }
}

async fn get(app: Router, path: &str) -> (StatusCode, String) {
    let res = app
        .oneshot(Request::get(path).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = res.status();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    (status, String::from_utf8(bytes.to_vec()).unwrap())
}

fn json(s: &str) -> serde_json::Value {
    serde_json::from_str(s).unwrap()
}

#[tokio::test]
async fn healthz_ok_even_when_db_down() {
    let (status, body) = get(health_routes(Arc::new(AlwaysDown)), "/healthz").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "ok");
}

#[tokio::test]
async fn readyz_up() {
    let (status, body) = get(health_routes(Arc::new(AlwaysUp)), "/readyz").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json(&body), serde_json::json!({"db": "up"}));
}

#[tokio::test]
async fn readyz_down() {
    let (status, body) = get(health_routes(Arc::new(AlwaysDown)), "/readyz").await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(json(&body), serde_json::json!({"db": "down"}));
}

#[tokio::test]
async fn health_alias_ok() {
    let (status, body) = get(health_routes(Arc::new(AlwaysDown)), "/health").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "ok");
}

#[tokio::test]
async fn ready_alias_matches_readyz() {
    let (status, body) = get(health_routes(Arc::new(AlwaysUp)), "/ready").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json(&body), serde_json::json!({"db": "up"}));
    let (status, body) = get(health_routes(Arc::new(AlwaysDown)), "/ready").await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(json(&body), serde_json::json!({"db": "down"}));
}

#[tokio::test(start_paused = true)]
async fn readyz_times_out_as_down() {
    let app = health_routes(Arc::new(Hangs));
    let handle = tokio::spawn(async move { get(app, "/readyz").await });
    tokio::time::advance(Duration::from_secs(3)).await;
    let (status, body) = handle.await.unwrap();
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(json(&body), serde_json::json!({"db": "down"}));
}

#[tokio::test]
async fn home_page_shows_name_and_tagline() {
    let opts = LeptosOptions::builder().output_name("incisive-app").build();
    let (status, body) = get(
        build_router(Arc::new(AlwaysUp), identity_state(), opts),
        "/",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("Incisive"));
    assert!(body.contains("Cut to the evidence."));
}

#[tokio::test]
async fn browser_code_is_revalidated_on_every_load() {
    // A deploy keeps the /pkg/ file names, so browsers must revalidate them
    // (otherwise a cached old build runs against the new page and nothing
    // on it responds).
    let dir = std::env::temp_dir().join(format!("incisive-site-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("pkg")).unwrap();
    std::fs::write(dir.join("pkg/incisive-app.js"), "export default 1;").unwrap();
    let opts = LeptosOptions::builder()
        .output_name("incisive-app")
        .site_root(dir.to_str().unwrap())
        .build();
    let res = build_router(Arc::new(AlwaysUp), identity_state(), opts)
        .oneshot(
            Request::builder()
                .uri("/pkg/incisive-app.js")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(
        res.headers()
            .get(axum::http::header::CACHE_CONTROL)
            .map(|v| v.to_str().unwrap()),
        Some("no-cache")
    );
    // Pages and look-alike paths are left alone.
    for path in ["/", "/pkgx"] {
        let res = build_router(
            Arc::new(AlwaysUp),
            identity_state(),
            LeptosOptions::builder()
                .output_name("incisive-app")
                .site_root(dir.to_str().unwrap())
                .build(),
        )
        .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
        .await
        .unwrap();
        assert!(
            res.headers()
                .get(axum::http::header::CACHE_CONTROL)
                .is_none(),
            "{path}"
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}
