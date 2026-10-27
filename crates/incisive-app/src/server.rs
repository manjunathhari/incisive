use std::sync::Arc;

use axum::Router;
use leptos::config::LeptosOptions;
use leptos_axum::{LeptosRoutes, generate_route_list};

use crate::app::{App, shell};
use crate::health::{Readiness, health_routes};
use crate::identity::{IdentityState, identity_routes};

pub fn build_router(
    r: Arc<dyn Readiness>,
    identity: IdentityState,
    leptos_options: LeptosOptions,
) -> Router {
    let routes = generate_route_list(App);
    let app: Router = Router::new()
        .leptos_routes(&leptos_options, routes, {
            let opts = leptos_options.clone();
            move || shell(opts.clone())
        })
        .fallback(leptos_axum::file_and_error_handler(shell))
        .with_state(leptos_options);
    health_routes(r).merge(identity_routes(identity)).merge(app)
}
