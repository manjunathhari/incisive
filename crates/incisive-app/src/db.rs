//! Postgres (Supabase) connection pool, migrations and readiness probe.

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

/// Readiness backed by a `SELECT 1` round trip.
pub struct PgReadiness(pub PgPool);

#[async_trait]
impl Readiness for PgReadiness {
    async fn db_ok(&self) -> bool {
        sqlx::query("SELECT 1").execute(&self.0).await.is_ok()
    }
}
