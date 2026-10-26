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
    let (status, body) = get(build_router(Arc::new(AlwaysUp), opts), "/").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("Incisive"));
    assert!(body.contains("Cut to the evidence."));
}
