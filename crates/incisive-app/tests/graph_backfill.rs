//! The graph backfill with the database (`TEST_DATABASE_URL`): `--count`
//! counts exactly the runs `--queue` would queue, changes nothing, and
//! `--queue` is repeatable.

use incisive_app::db::{lazy_pool, migrate};
use incisive_app::graph::job::{backfill_count, backfill_line, backfill_queue};
use incisive_app::runs::store;
use secrecy::SecretString;
use serde_json::Value;
use sqlx::PgPool;
use uuid::Uuid;

/// The backfill touches every run in the shared database, so these tests
/// run one at a time.
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

async fn connect() -> Option<PgPool> {
    let Ok(url) = std::env::var("TEST_DATABASE_URL") else {
        eprintln!("SKIP: TEST_DATABASE_URL not set");
        return None;
    };
    let pool = lazy_pool(&SecretString::from(url)).expect("lazy pool");
    migrate(&pool).await.expect("migrate");
    Some(pool)
}

async fn new_user(pool: &PgPool) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO incisive.users (email, display_name, role, daily_cap_paise) \
         VALUES ($1, 'B', 'surgeon', 100000) RETURNING id",
    )
    .bind(format!("graph-backfill-{}@example.com", Uuid::new_v4()))
    .fetch_one(pool)
    .await
    .unwrap()
}

fn hernia() -> Value {
    serde_json::from_str(include_str!("fixtures/temporal/full/hernia.json")).unwrap()
}

async fn new_run(pool: &PgPool, uid: i64, status: &str, body: Option<Value>) -> Uuid {
    let run = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO incisive.runs (id, user_id, question, mode, status, limit_paise, cost_paise, \
           started_at, finished_at) \
         VALUES ($1, $2, 'q', 'focused', $3, 1500, 321, now(), now())",
    )
    .bind(run)
    .bind(uid)
    .bind(status)
    .execute(pool)
    .await
    .unwrap();
    if let Some(body) = body {
        sqlx::query(
            "INSERT INTO incisive.run_answers (run_id, body, answer_version) VALUES ($1, $2, $3)",
        )
        .bind(run)
        .bind(&body)
        .bind(store::answer_version_of(&body))
        .execute(pool)
        .await
        .unwrap();
    }
    run
}

async fn jobs_among(pool: &PgPool, runs: &[Uuid]) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM incisive.graph_jobs WHERE run_id = ANY($1)")
        .bind(runs)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn all_jobs(pool: &PgPool) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM incisive.graph_jobs")
        .fetch_one(pool)
        .await
        .unwrap()
}

#[test]
fn the_count_line_is_runs_and_paise_only() {
    assert_eq!(backfill_line(12, 40), "12 runs, about 480 paise");
    assert_eq!(backfill_line(0, 40), "0 runs, about 0 paise");
}

#[tokio::test]
async fn count_changes_nothing_and_queue_adds_only_eligible_runs_once() {
    let Some(pool) = connect().await else { return };
    let _g = SERIAL.lock().await;
    let uid = new_user(&pool).await;
    // Take the other tests' leftovers out of the way first.
    backfill_queue(&pool).await.unwrap();
    assert_eq!(backfill_count(&pool).await.unwrap(), 0);

    let mut v2 = hernia();
    v2.as_object_mut().unwrap().remove("version");
    let mut null = hernia();
    null["answer"] = Value::Null;

    let done = new_run(&pool, uid, "done", Some(hernia())).await;
    let partial = new_run(&pool, uid, "partial", Some(hernia())).await;
    let already = new_run(&pool, uid, "done", Some(hernia())).await;
    sqlx::query("INSERT INTO incisive.graph_jobs (run_id) VALUES ($1)")
        .bind(already)
        .execute(&pool)
        .await
        .unwrap();
    let skipped = [
        new_run(&pool, uid, "failed", Some(hernia())).await,
        new_run(&pool, uid, "interrupted", Some(hernia())).await,
        new_run(&pool, uid, "running", Some(hernia())).await,
        new_run(&pool, uid, "done", Some(v2)).await,
        new_run(&pool, uid, "done", Some(null)).await,
        new_run(&pool, uid, "done", None).await,
    ];

    // --count: exactly the two runs a queue would call; nothing changes.
    let jobs_before = all_jobs(&pool).await;
    assert_eq!(backfill_count(&pool).await.unwrap(), 2);
    assert_eq!(backfill_count(&pool).await.unwrap(), 2);
    assert_eq!(all_jobs(&pool).await, jobs_before, "count writes nothing");

    // --queue: those two, not the skipped ones, not the already queued one.
    assert_eq!(backfill_queue(&pool).await.unwrap(), 2);
    assert_eq!(jobs_among(&pool, &[done, partial, already]).await, 3);
    assert_eq!(jobs_among(&pool, &skipped).await, 0);

    // A second --queue adds nothing, and nothing is left to count.
    assert_eq!(backfill_queue(&pool).await.unwrap(), 0);
    assert_eq!(backfill_count(&pool).await.unwrap(), 0);
    assert_eq!(all_jobs(&pool).await, jobs_before + 2);

    // Leave the shared database as found for the other tests.
    sqlx::query("DELETE FROM incisive.graph_jobs j USING incisive.runs r WHERE r.id = j.run_id AND r.user_id = $1")
        .bind(uid)
        .execute(&pool)
        .await
        .unwrap();
}
