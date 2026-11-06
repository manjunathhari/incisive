//! Migration 0017 (graph tables) with the database (`TEST_DATABASE_URL`):
//! the tables exist, a run's delete removes its links and job but not the
//! shared concepts and papers, and the CHECKs and the unique key hold.

use incisive_app::db::{lazy_pool, migrate};
use secrecy::SecretString;
use sqlx::PgPool;
use uuid::Uuid;

const TABLES: [&str; 5] = [
    "graph_concepts",
    "graph_papers",
    "graph_run_concepts",
    "graph_run_papers",
    "graph_jobs",
];

async fn connect() -> Option<PgPool> {
    let Ok(url) = std::env::var("TEST_DATABASE_URL") else {
        eprintln!("SKIP: TEST_DATABASE_URL not set");
        return None;
    };
    let pool = lazy_pool(&SecretString::from(url)).expect("lazy pool");
    migrate(&pool).await.expect("migrate");
    Some(pool)
}

async fn seed_run(pool: &PgPool) -> (i64, Uuid) {
    let email = format!("graph-{}@example.test", Uuid::new_v4());
    let user: i64 = sqlx::query_scalar(
        "INSERT INTO incisive.users (email, display_name, role, daily_cap_paise) \
         VALUES ($1, 'Graph', 'surgeon', 100000) RETURNING id",
    )
    .bind(&email)
    .fetch_one(pool)
    .await
    .unwrap();
    let run = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO incisive.runs (id, user_id, question, mode, status, limit_paise) \
         VALUES ($1, $2, 'q', 'quick', 'done', 1000)",
    )
    .bind(run)
    .bind(user)
    .execute(pool)
    .await
    .unwrap();
    (user, run)
}

async fn seed_concept(pool: &PgPool, kind: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO incisive.graph_concepts (kind, label, norm_label) \
         VALUES ($1, 'Label', $2) RETURNING id",
    )
    .bind(kind)
    .bind(Uuid::new_v4().to_string())
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn seed_paper(pool: &PgPool) -> String {
    let pmid = Uuid::new_v4().simple().to_string();
    sqlx::query("INSERT INTO incisive.graph_papers (pmid, title) VALUES ($1, 't')")
        .bind(&pmid)
        .execute(pool)
        .await
        .unwrap();
    pmid
}

async fn link_paper(pool: &PgPool, run: Uuid, user: i64, pmid: &str, rank: &str) -> bool {
    sqlx::query(
        "INSERT INTO incisive.graph_run_papers (run_id, user_id, pmid, rank, rank_label) \
         VALUES ($1, $2, $3, $4, 'x')",
    )
    .bind(run)
    .bind(user)
    .bind(pmid)
    .bind(rank)
    .execute(pool)
    .await
    .is_ok()
}

async fn count(pool: &PgPool, table: &str, col: &str, v: &str) -> i64 {
    sqlx::query_scalar(&format!(
        "SELECT count(*) FROM incisive.{table} WHERE {col}::text = $1"
    ))
    .bind(v)
    .fetch_one(pool)
    .await
    .unwrap()
}

#[tokio::test]
async fn tables_exist_with_rls_on() {
    let Some(pool) = connect().await else { return };
    for t in TABLES {
        let rls: bool = sqlx::query_scalar(
            "SELECT c.relrowsecurity FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace \
             WHERE n.nspname = 'incisive' AND c.relname = $1",
        )
        .bind(t)
        .fetch_one(&pool)
        .await
        .unwrap_or_else(|e| panic!("{t}: {e}"));
        assert!(rls, "{t} has row-level security on");
    }
}

#[tokio::test]
async fn run_delete_cascades_links_and_job_not_shared_rows() {
    let Some(pool) = connect().await else { return };
    let (user, run) = seed_run(&pool).await;
    let concept = seed_concept(&pool, "disease").await;
    let pmid = seed_paper(&pool).await;
    sqlx::query(
        "INSERT INTO incisive.graph_run_concepts (run_id, user_id, concept_id) VALUES ($1, $2, $3)",
    )
    .bind(run)
    .bind(user)
    .bind(concept)
    .execute(&pool)
    .await
    .unwrap();
    assert!(link_paper(&pool, run, user, &pmid, "rct").await);
    sqlx::query("INSERT INTO incisive.graph_jobs (run_id) VALUES ($1)")
        .bind(run)
        .execute(&pool)
        .await
        .unwrap();
    let r = run.to_string();
    for t in ["graph_run_concepts", "graph_run_papers", "graph_jobs"] {
        assert_eq!(count(&pool, t, "run_id", &r).await, 1, "{t} before");
    }
    sqlx::query("DELETE FROM incisive.runs WHERE id = $1")
        .bind(run)
        .execute(&pool)
        .await
        .unwrap();
    for t in ["graph_run_concepts", "graph_run_papers", "graph_jobs"] {
        assert_eq!(count(&pool, t, "run_id", &r).await, 0, "{t} after");
    }
    assert_eq!(
        count(&pool, "graph_concepts", "id", &concept.to_string()).await,
        1
    );
    assert_eq!(count(&pool, "graph_papers", "pmid", &pmid).await, 1);
}

#[tokio::test]
async fn unique_kind_and_label_and_checks_hold() {
    let Some(pool) = connect().await else { return };
    let norm = Uuid::new_v4().to_string();
    let ins = |kind: &'static str, norm: String| {
        let pool = pool.clone();
        async move {
            sqlx::query(
                "INSERT INTO incisive.graph_concepts (kind, label, norm_label) VALUES ($1, 'L', $2)",
            )
            .bind(kind)
            .bind(norm)
            .execute(&pool)
            .await
            .is_ok()
        }
    };
    assert!(ins("disease", norm.clone()).await);
    assert!(
        !ins("disease", norm.clone()).await,
        "duplicate (kind, norm_label)"
    );
    assert!(ins("organ", norm.clone()).await, "same label, other kind");
    assert!(!ins("bogus", Uuid::new_v4().to_string()).await, "bad kind");

    let (user, run) = seed_run(&pool).await;
    let pmid = seed_paper(&pool).await;
    assert!(
        !link_paper(&pool, run, user, &pmid, "bogus").await,
        "bad rank"
    );
    assert!(link_paper(&pool, run, user, &pmid, "unclassified").await);

    let bad_status =
        sqlx::query("INSERT INTO incisive.graph_jobs (run_id, status) VALUES ($1, 'bogus')")
            .bind(run)
            .execute(&pool)
            .await;
    assert!(bad_status.is_err(), "bad status");
    let id = seed_concept(&pool, "outcome").await;
    let self_merge =
        sqlx::query("UPDATE incisive.graph_concepts SET merged_into = id WHERE id = $1")
            .bind(id)
            .execute(&pool)
            .await;
    assert!(self_merge.is_err(), "no self merge");
}
