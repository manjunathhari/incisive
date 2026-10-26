use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use axum::{Json, Router, extract::State, http::StatusCode, routing::get};
use serde_json::{Value, json};

/// How long `/readyz` waits for the database before reporting it down.
const READY_TIMEOUT: Duration = Duration::from_secs(2);

#[async_trait]
pub trait Readiness: Send + Sync + 'static {
    async fn db_ok(&self) -> bool;
}

/// Placeholder readiness used until the database is wired in; always down.
pub struct NoDb;

#[async_trait]
impl Readiness for NoDb {
    async fn db_ok(&self) -> bool {
        false
    }
}

async fn healthz() -> &'static str {
    "ok"
}

async fn readyz(State(r): State<Arc<dyn Readiness>>) -> (StatusCode, Json<Value>) {
    let up = tokio::time::timeout(READY_TIMEOUT, r.db_ok())
        .await
        .unwrap_or(false);
    if up {
        (StatusCode::OK, Json(json!({"db": "up"})))
    } else {
        (StatusCode::SERVICE_UNAVAILABLE, Json(json!({"db": "down"})))
    }
}

pub fn health_routes(r: Arc<dyn Readiness>) -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .with_state(r)
}
