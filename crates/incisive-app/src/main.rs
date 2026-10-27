use std::sync::{Arc, atomic::AtomicBool};
use std::time::Duration;

use incisive_app::{
    config::Config,
    db::{PgReadiness, lazy_pool, migrate_until_done},
    identity::{IAP_JWK_URL, IapVerifier, IdentityState, JwkSource, ensure_owner_until_done},
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

    let config = match Config::from_env() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    };

    if config.app_env == incisive_app::config::AppEnv::Dev {
        tracing::warn!(
            "APP_ENV=dev: the x-dev-user-email identity header is enabled; never use in production"
        );
    }

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
    let migrated = Arc::new(AtomicBool::new(false));
    let (base, cap) = (Duration::from_secs(1), Duration::from_secs(30));
    // Migrations first, then the owner row (which needs the users table).
    tokio::spawn({
        let (pool, migrated) = (pool.clone(), migrated.clone());
        let (owner_email,) = (config.owner_email.clone(),);
        async move {
            migrate_until_done(pool.clone(), migrated, base, cap).await;
            ensure_owner_until_done(pool, owner_email, "Owner".to_string(), base, cap).await;
        }
    });

    let verifier = IapVerifier::new(
        config.iap_audience.clone(),
        JwkSource::Remote(IAP_JWK_URL.parse().expect("IAP JWK url")),
    );
    let identity = IdentityState::new(verifier, pool.clone(), config.app_env);

    let app = build_router(
        Arc::new(PgReadiness::new(pool, migrated)),
        identity,
        leptos_options.clone(),
    );
    let listener = tokio::net::TcpListener::bind(leptos_options.site_addr)
        .await
        .expect("bind listener");
    tracing::info!(addr = %leptos_options.site_addr, env = ?config.app_env, "listening");
    axum::serve(listener, app).await.expect("server error");
}
