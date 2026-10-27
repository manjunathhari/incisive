use std::sync::Arc;

use axum::{
    Router,
    extract::{DefaultBodyLimit, FromRef},
    routing::{get, post},
};
use leptos::config::LeptosOptions;
use leptos::prelude::provide_context;
use leptos_axum::{LeptosRoutes, generate_route_list};
use sqlx::PgPool;
use tower_http::catch_panic::CatchPanicLayer;
use tower_http::limit::RequestBodyLimitLayer;

use crate::app::{App, shell};
use crate::ask::{ASK_BODY_LIMIT, activity_handler, ask_handler, me_handler};
use crate::config::AppEnv;
use crate::gate::Classifier;
use crate::health::{Readiness, health_routes};
use crate::identity::{IapVerifier, IdentityState};
use crate::runs::RunsConfig;
use crate::runs::worker::WorkerHandle;
use crate::spend::{CapsConfig, Prices};

/// Everything a request needs. Also provided as Leptos context to server functions.
#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
    pub verifier: Arc<IapVerifier>,
    pub gate: Arc<dyn Classifier>,
    pub prices: Prices,
    pub usd_inr: f64,
    pub caps: CapsConfig,
    pub gate_model: String,
    /// `APP_ENV=dev`: the `x-dev-user-email` header is trusted. Never in prod.
    pub dev_mode: bool,
    /// Per-depth run limits (`RUN_LIMITS`).
    pub runs: RunsConfig,
    /// Starts and cancels background runs.
    pub worker: WorkerHandle,
}

impl FromRef<AppState> for IdentityState {
    fn from_ref(s: &AppState) -> IdentityState {
        IdentityState {
            verifier: s.verifier.clone(),
            pool: s.pool.clone(),
            app_env: if s.dev_mode {
                AppEnv::Dev
            } else {
                AppEnv::Prod
            },
        }
    }
}

/// Body of the 500 returned when a request handler panics.
pub const PANIC_BODY: &str = "Incisive hit an unexpected error. Please try again.";

/// A fixed 500 for a panicking request. The payload is dropped unread: a
/// str-slicing panic message embeds part of the string, which could be
/// question text.
fn panic_response(_payload: Box<dyn std::any::Any + Send + 'static>) -> axum::response::Response {
    use axum::response::IntoResponse;
    (
        axum::http::StatusCode::INTERNAL_SERVER_ERROR,
        [(
            axum::http::header::CONTENT_TYPE,
            "text/plain; charset=utf-8",
        )],
        PANIC_BODY,
    )
        .into_response()
}

type PanicHandler = fn(Box<dyn std::any::Any + Send + 'static>) -> axum::response::Response;

/// Catches handler panics and answers with [`PANIC_BODY`].
pub fn catch_panic_layer() -> CatchPanicLayer<PanicHandler> {
    CatchPanicLayer::custom(panic_response as PanicHandler)
}

/// Process-wide panic hook: logs only where the panic happened (file:line) at
/// error level, never the payload.
pub fn install_panic_hook() {
    std::panic::set_hook(Box::new(|info| {
        let location = info
            .location()
            .map(|l| format!("{}:{}", l.file(), l.line()))
            .unwrap_or_else(|| "unknown".to_string());
        tracing::error!(%location, "panic");
    }));
}

pub const CROSS_SITE_MESSAGE: &str = "Request blocked: cross-site request.";

/// Whether `origin` (an `Origin` header value) names the same host and port as
/// `authority` (the request's `Host`). Ports default from the origin's scheme.
fn same_origin(origin: &str, authority: &str) -> bool {
    let Ok(o) = url::Url::parse(origin) else {
        return false;
    };
    let (Some(o_host), Some(o_port)) = (o.host_str(), o.port_or_known_default()) else {
        return false;
    };
    let Ok(a) = authority.parse::<axum::http::uri::Authority>() else {
        return false;
    };
    let a_port = a.port_u16().unwrap_or(o_port);
    // url lowercases the host; IPv6 hosts keep their brackets in both.
    o_host.eq_ignore_ascii_case(a.host()) && o_port == a_port
}

/// `POST /api/ask` and `POST /api/fn/*`: when an `Origin` header is present it
/// must match the request's host and port, or the request is refused with 403
/// before identity, audit or spend. A missing `Origin` is allowed (curl and
/// server-side clients omit it).
async fn same_origin_check(
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    use axum::http::{Method, StatusCode, header};
    use axum::response::IntoResponse;
    let path = req.uri().path();
    let guarded =
        req.method() == Method::POST && (path == "/api/ask" || path.starts_with("/api/fn/"));
    if guarded && let Some(origin) = req.headers().get(header::ORIGIN) {
        // HTTP/2 carries the host in the URI authority instead of a Host header.
        let authority = req
            .headers()
            .get(header::HOST)
            .and_then(|h| h.to_str().ok())
            .map(str::to_string)
            .or_else(|| req.uri().authority().map(|a| a.to_string()));
        let ok = match (origin.to_str(), authority) {
            (Ok(o), Some(a)) => same_origin(o, &a),
            _ => false,
        };
        if !ok {
            tracing::warn!("cross-site request refused");
            return (
                StatusCode::FORBIDDEN,
                axum::Json(serde_json::json!({
                    "error": "cross_site",
                    "message": CROSS_SITE_MESSAGE,
                })),
            )
                .into_response();
        }
    }
    next.run(req).await
}

/// `GET /api/me`, `POST /api/ask`, `GET /api/activity`.
pub fn api_routes(state: AppState) -> Router {
    Router::new()
        .route("/api/me", get(me_handler))
        .route(
            "/api/ask",
            post(ask_handler).layer(DefaultBodyLimit::max(ASK_BODY_LIMIT)),
        )
        .route("/api/activity", get(activity_handler))
        .layer(axum::middleware::from_fn(same_origin_check))
        .with_state(state)
}

pub fn build_router(
    r: Arc<dyn Readiness>,
    state: AppState,
    leptos_options: LeptosOptions,
) -> Router {
    let routes = generate_route_list(App);
    let context_state = state.clone();
    let app: Router = Router::new()
        .leptos_routes_with_context(
            &leptos_options,
            routes,
            move || provide_context(context_state.clone()),
            {
                let opts = leptos_options.clone();
                move || shell(opts.clone())
            },
        )
        .fallback(leptos_axum::file_and_error_handler(shell))
        // Server functions (`/api/fn/*`) get the same cap as `POST /api/ask`. They
        // read the raw body, which axum's `DefaultBodyLimit` does not cover, so the
        // limit is enforced on the body itself (413 when exceeded).
        .layer(RequestBodyLimitLayer::new(ASK_BODY_LIMIT))
        // Server functions read identity in the handler: refuse cross-site first.
        .layer(axum::middleware::from_fn(same_origin_check))
        .with_state(leptos_options);
    health_routes(r)
        .merge(api_routes(state))
        .merge(app)
        .layer(catch_panic_layer())
}
