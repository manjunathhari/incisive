//! Database tests. Gated on `TEST_DATABASE_URL`; skipped with a note when unset.
//! The target server must have pgvector installed (CI uses pgvector/pgvector:pg16).

use std::time::{Duration, Instant};

use incisive_app::db::{PgReadiness, lazy_pool, migrate};
use incisive_app::health::Readiness;
use secrecy::SecretString;

#[tokio::test]
async fn migrations_apply_and_ready_is_true() {
    let Ok(url) = std::env::var("TEST_DATABASE_URL") else {
        eprintln!("SKIP migrations_apply_and_ready_is_true: TEST_DATABASE_URL not set");
        return;
    };
    let pool = lazy_pool(&SecretString::from(url)).expect("lazy pool");

    migrate(&pool).await.expect("first migrate");
    migrate(&pool).await.expect("second migrate (idempotent)");

    assert!(PgReadiness(pool.clone()).db_ok().await);

    let rows = sqlx::query("SELECT 1 FROM pg_extension WHERE extname = 'vector'")
        .fetch_all(&pool)
        .await
        .expect("query pg_extension");
    assert_eq!(rows.len(), 1, "vector extension should be installed");

    let schema =
        sqlx::query("SELECT 1 FROM information_schema.schemata WHERE schema_name = 'incisive'")
            .fetch_all(&pool)
            .await
            .expect("query schemata");
    assert_eq!(schema.len(), 1, "incisive schema should exist");
}

#[tokio::test]
async fn unreachable_db_is_not_ready_and_does_not_panic() {
    let url = SecretString::from("postgres://u:p@127.0.0.1:1/x".to_string());
    let pool = lazy_pool(&url).expect("lazy pool never connects at construction");
    let start = Instant::now();
    let ok = PgReadiness(pool).db_ok().await;
    assert!(!ok);
    assert!(
        start.elapsed() < Duration::from_secs(4),
        "took {:?}",
        start.elapsed()
    );
}
