use std::sync::{Arc, atomic::AtomicBool};
use std::time::Duration;

use claude_client::Client;
use incisive_app::{
    config::{AppEnv, Config},
    db::{PgReadiness, lazy_pool, migrate_until_done},
    gate::{Classifier, ClaudeClassifier, OfflineClassifier},
    identity::{IAP_JWK_URL, IapVerifier, JwkSource, ensure_owner_until_done},
    runs::worker::{WorkerHandle, recovery_loop},
    server::{AppState, build_router, install_panic_hook},
    spend::CapsConfig,
};
use leptos::config::get_configuration;

/// Retention: runs older than 12 months are deleted at startup (after
/// migrations) and then daily. Logs counts only.
async fn retention_loop(pool: sqlx::PgPool) {
    let mut every = tokio::time::interval(Duration::from_secs(24 * 60 * 60));
    loop {
        // The first tick completes at once: the startup pass.
        every.tick().await;
        match incisive_app::runs::store::delete_runs_older_than(&pool, 365).await {
            Ok(deleted) => tracing::info!(deleted, "run retention pass"),
            Err(_) => tracing::error!("run retention pass failed"),
        }
    }
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .json()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();
    // After the subscriber, so the hook's single error line is logged as JSON.
    install_panic_hook();

    let config = match Config::from_env() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    };

    if config.app_env == AppEnv::Dev {
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
    // Migrations first, then the owner row (which needs the users table) and
    // run recovery: abandoned runs (stale heartbeat, by the database clock)
    // become interrupted and release their reservations, at startup and then
    // every minute. Live runs of other instances are left alone.
    tokio::spawn({
        let (pool, migrated) = (pool.clone(), migrated.clone());
        let (owner_email,) = (config.owner_email.clone(),);
        async move {
            migrate_until_done(pool.clone(), migrated, base, cap).await;
            tokio::spawn(recovery_loop(pool.clone(), base, cap));
            tokio::spawn(retention_loop(pool.clone()));
            ensure_owner_until_done(pool, owner_email, "Owner".to_string(), base, cap).await;
        }
    });

    let verifier = IapVerifier::new(
        config.iap_audience.clone(),
        JwkSource::Remote(IAP_JWK_URL.parse().expect("IAP JWK url")),
    );

    // The gate. No network at startup; the key is never logged. Config only lets
    // the key be absent in dev, where every model call then fails closed.
    let gate: Arc<dyn Classifier> = match config.anthropic_api_key.clone() {
        Some(key) => Arc::new(ClaudeClassifier::new(
            Arc::new(Client::new(key)),
            config.gate_model.clone(),
        )),
        None => {
            tracing::warn!(
                "no ANTHROPIC_API_KEY (dev): questions the rules cannot decide will be refused as unavailable"
            );
            Arc::new(OfflineClassifier)
        }
    };
    if config.model_prices.get(&config.gate_model).is_none() {
        tracing::warn!(
            gate_model = %config.gate_model,
            "GATE_MODEL has no entry in MODEL_PRICES; every question will fail closed"
        );
    }

    let state = AppState {
        pool: pool.clone(),
        verifier: Arc::new(verifier),
        gate,
        prices: config.model_prices.clone(),
        usd_inr: config.usd_inr,
        caps: CapsConfig {
            global_monthly_paise: config.global_monthly_cap_paise,
        },
        gate_model: config.gate_model.clone(),
        dev_mode: config.app_env == AppEnv::Dev,
        runs: config.runs,
        worker: WorkerHandle::default(),
    };

    let app = build_router(
        Arc::new(PgReadiness::new(pool, migrated)),
        state,
        leptos_options.clone(),
    );
    let listener = tokio::net::TcpListener::bind(leptos_options.site_addr)
        .await
        .expect("bind listener");
    tracing::info!(addr = %leptos_options.site_addr, env = ?config.app_env, "listening");
    axum::serve(listener, app).await.expect("server error");
}
