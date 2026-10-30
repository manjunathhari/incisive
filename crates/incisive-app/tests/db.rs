//! Database tests. Gated on `TEST_DATABASE_URL`; skipped with a note when unset.
//! The target server must have pgvector installed (CI uses pgvector/pgvector:pg16).

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};

use incisive_app::db::{PgReadiness, lazy_pool, migrate, migrate_until_done};
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

    let ready = PgReadiness::new(pool.clone(), Arc::new(AtomicBool::new(true)));
    assert!(ready.db_ok().await);

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
    // flag true so the query against the dead port is actually exercised
    let ok = PgReadiness::new(pool, Arc::new(AtomicBool::new(true)))
        .db_ok()
        .await;
    assert!(!ok);
    assert!(
        start.elapsed() < Duration::from_secs(6),
        "took {:?}",
        start.elapsed()
    );
}

#[tokio::test]
async fn not_ready_until_migrated() {
    let Ok(url) = std::env::var("TEST_DATABASE_URL") else {
        eprintln!("SKIP not_ready_until_migrated: TEST_DATABASE_URL not set");
        return;
    };
    let pool = lazy_pool(&SecretString::from(url)).expect("lazy pool");
    let migrated = Arc::new(AtomicBool::new(false));
    let ready = PgReadiness::new(pool.clone(), migrated.clone());

    assert!(!ready.db_ok().await, "must not be ready before migration");

    migrate_until_done(
        pool,
        migrated.clone(),
        Duration::from_millis(1),
        Duration::from_millis(5),
    )
    .await;

    assert!(migrated.load(Ordering::Acquire));
    assert!(ready.db_ok().await);
}

#[tokio::test]
async fn migrate_retry_keeps_trying_when_db_unreachable() {
    let url = SecretString::from("postgres://u:p@127.0.0.1:1/x".to_string());
    let pool = lazy_pool(&url).expect("lazy pool");
    let migrated = Arc::new(AtomicBool::new(false));

    let res = tokio::time::timeout(
        Duration::from_secs(2),
        migrate_until_done(
            pool,
            migrated.clone(),
            Duration::from_millis(1),
            Duration::from_millis(5),
        ),
    )
    .await;

    assert!(res.is_err(), "should still be retrying");
    assert!(!migrated.load(Ordering::Acquire));
}

/// Stage 3 review 8: an image whose migrations are older than the
/// database's (an image rollback after an additive migration) still
/// migrates and starts: applied migrations it does not know are ignored.
#[tokio::test]
async fn an_older_image_runs_against_a_newer_additive_schema() {
    let Ok(url) = std::env::var("TEST_DATABASE_URL") else {
        eprintln!("SKIP: TEST_DATABASE_URL not set");
        return;
    };
    let pool = lazy_pool(&SecretString::from(url)).expect("lazy pool");
    migrate(&pool).await.expect("migrate");
    // A migration from a newer image, as sqlx records it.
    let version: i64 = 99_990_000 + i64::from(std::process::id() % 9_999);
    sqlx::query(
        "INSERT INTO _sqlx_migrations (version, description, success, checksum, execution_time) \
         VALUES ($1, 'from a newer image', true, '\\x00'::bytea, 0) ON CONFLICT DO NOTHING",
    )
    .bind(version)
    .execute(&pool)
    .await
    .expect("record a newer migration");
    let r = migrate(&pool).await;
    sqlx::query("DELETE FROM _sqlx_migrations WHERE version = $1")
        .bind(version)
        .execute(&pool)
        .await
        .expect("clean up");
    r.expect("an unknown applied migration is ignored");
}
