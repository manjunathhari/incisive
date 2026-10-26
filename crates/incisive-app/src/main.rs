mod config;

use std::sync::Arc;

use incisive_app::{
    db::{PgReadiness, lazy_pool, migrate},
    server::build_router,
};
use leptos::config::get_configuration;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .json()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let config = match config::Config::from_env() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    };

    let mut leptos_options = get_configuration(None)
        .expect("leptos configuration")
        .leptos_options;
    leptos_options.site_addr = std::net::SocketAddr::from(([0, 0, 0, 0], config.port));

    let pool = match lazy_pool(&config.database_url) {
        Ok(p) => p,
        Err(e) => {
            let _ = e; // may echo URL parts; keep credentials out of logs
            eprintln!("invalid DATABASE_URL");
            std::process::exit(1);
        }
    };
    let migrate_pool = pool.clone();
    tokio::spawn(async move {
        match migrate(&migrate_pool).await {
            Ok(()) => tracing::info!("migrations applied"),
            Err(e) => tracing::error!(error = %e, "migrations failed"),
        }
    });

    let app = build_router(Arc::new(PgReadiness(pool)), leptos_options.clone());
    let listener = tokio::net::TcpListener::bind(leptos_options.site_addr)
        .await
        .expect("bind listener");
    tracing::info!(addr = %leptos_options.site_addr, env = ?config.app_env, "listening");
    axum::serve(listener, app).await.expect("server error");
}
