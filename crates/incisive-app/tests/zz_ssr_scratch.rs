//! TEMPORARY scratch repro (not to be committed).
use axum::{Router, body::Body, http::Request};
use http_body_util::BodyExt;
use incisive_app::db::lazy_pool;
use incisive_app::gate::OfflineClassifier;
use incisive_app::identity::{IapVerifier, JwkSource};
use incisive_app::runs::RunsConfig;
use incisive_app::runs::worker::WorkerHandle;
use incisive_app::server::AppState;
use incisive_app::spend::{CapsConfig, Prices};
use leptos::config::LeptosOptions;
use secrecy::SecretString;
use std::sync::Arc;
use std::time::Duration;
use tower::ServiceExt;

struct AlwaysUp;
#[async_trait::async_trait]
impl incisive_app::health::Readiness for AlwaysUp {
    async fn db_ok(&self) -> bool {
        true
    }
}

fn router() -> Router {
    let pool = lazy_pool(&SecretString::from(
        "postgres://u:p@127.0.0.1:1/none".to_string(),
    ))
    .unwrap();
    let st = AppState {
        pool,
        verifier: Arc::new(IapVerifier::new(
            "aud".into(),
            JwkSource::Static(serde_json::from_str(r#"{"keys":[]}"#).unwrap()),
        )),
        gate: Arc::new(OfflineClassifier),
        prices: Prices::from_json(r#"{"m-test": {"input": 1.0, "output": 5.0}}"#).unwrap(),
        usd_inr: 88.0,
        caps: CapsConfig {
            global_monthly_paise: 1,
        },
        gate_model: "m-test".into(),
        dev_mode: true,
        runs: RunsConfig::default(),
        worker: WorkerHandle::default(),
    };
    let opts = LeptosOptions::builder().output_name("incisive-app").build();
    incisive_app::server::build_router(Arc::new(AlwaysUp), st, opts)
}

async fn hit(path: &str) {
    let req = Request::builder()
        .uri(path)
        .header("x-dev-user-email", "a@example.com")
        .body(Body::empty())
        .unwrap();
    let res = router().oneshot(req).await.unwrap();
    let _ = res.into_body().collect().await.unwrap();
}

/// Many threads, each with its own current-thread runtime (like #[tokio::test]).
#[test]
fn ssr_many_runtimes() {
    let handles: Vec<_> = (0..8)
        .map(|t| {
            std::thread::spawn(move || {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap();
                rt.block_on(async {
                    for i in 0..50 {
                        let p = if (t + i) % 2 == 0 { "/history" } else { "/api/runs" };
                        tokio::time::timeout(Duration::from_secs(5), hit(p))
                            .await
                            .unwrap_or_else(|_| panic!("hang t{t} i{i} {p}"));
                    }
                })
            })
        })
        .collect();
    for h in handles {
        h.join().unwrap();
    }
}

/// One multi-thread runtime, many concurrent requests (like production).
#[test]
fn ssr_one_runtime() {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async {
        let mut set = tokio::task::JoinSet::new();
        for t in 0..400usize {
            set.spawn(async move {
                let p = if t % 2 == 0 { "/history" } else { "/api/runs" };
                tokio::time::timeout(Duration::from_secs(5), hit(p))
                    .await
                    .unwrap_or_else(|_| panic!("hang t{t} {p}"));
            });
        }
        while let Some(r) = set.join_next().await {
            r.unwrap();
        }
    });
}
