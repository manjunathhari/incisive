//! Postgres (Supabase) connection pool, migrations and readiness probe.

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

use async_trait::async_trait;
use secrecy::{ExposeSecret, SecretString};
use sqlx::{PgPool, postgres::PgPoolOptions};

use crate::health::Readiness;

/// Build a pool without connecting, so boot never blocks on the database.
pub fn lazy_pool(url: &SecretString) -> Result<PgPool, sqlx::Error> {
    PgPoolOptions::new()
        .max_connections(5)
        .acquire_timeout(Duration::from_secs(3))
        .connect_lazy(url.expose_secret())
}

/// Apply the migrations in the repo-root `migrations/` directory.
pub async fn migrate(pool: &PgPool) -> Result<(), sqlx::migrate::MigrateError> {
    sqlx::migrate!("../../migrations").run(pool).await
}

/// Retry `migrate` with exponential backoff (`base`, doubling, capped at `cap`)
/// until it succeeds, then set `migrated`. Never logs the database URL.
pub async fn migrate_until_done(
    pool: PgPool,
    migrated: Arc<AtomicBool>,
    base: Duration,
    cap: Duration,
) {
    let mut delay = base;
    let mut attempt: u32 = 1;
    loop {
        match migrate(&pool).await {
            Ok(()) => {
                migrated.store(true, Ordering::Release);
                tracing::info!(attempt, "migrations applied");
                return;
            }
            Err(e) => {
                tracing::warn!(attempt, error = %e, "migration attempt failed; will retry");
                tokio::time::sleep(delay).await;
                delay = (delay * 2).min(cap);
                attempt = attempt.saturating_add(1);
            }
        }
    }
}

/// Ready only when the database answers `SELECT 1` and migrations have completed.
pub struct PgReadiness {
    pub pool: PgPool,
    pub migrated: Arc<AtomicBool>,
}

impl PgReadiness {
    pub fn new(pool: PgPool, migrated: Arc<AtomicBool>) -> Self {
        Self { pool, migrated }
    }
}

#[async_trait]
impl Readiness for PgReadiness {
    async fn db_ok(&self) -> bool {
        self.migrated.load(Ordering::Acquire)
            && sqlx::query("SELECT 1").execute(&self.pool).await.is_ok()
    }
}
