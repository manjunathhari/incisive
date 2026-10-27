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
use tower_http::limit::RequestBodyLimitLayer;

use crate::app::{App, shell};
use crate::ask::{ASK_BODY_LIMIT, activity_handler, ask_handler, me_handler};
use crate::config::AppEnv;
use crate::gate::Classifier;
use crate::health::{Readiness, health_routes};
use crate::identity::{IapVerifier, IdentityState};
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

/// `GET /api/me`, `POST /api/ask`, `GET /api/activity`.
pub fn api_routes(state: AppState) -> Router {
    Router::new()
        .route("/api/me", get(me_handler))
        .route(
            "/api/ask",
            post(ask_handler).layer(DefaultBodyLimit::max(ASK_BODY_LIMIT)),
        )
        .route("/api/activity", get(activity_handler))
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
        .with_state(leptos_options);
    health_routes(r).merge(api_routes(state)).merge(app)
}
