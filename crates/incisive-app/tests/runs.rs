//! Run-store tests, gated on `TEST_DATABASE_URL`. Every test creates its own
//! user (unique email), so tests running in parallel on one DB never share rows.
//! The two global operations (`mark_stale_runs_interrupted`,
//! `delete_runs_older_than`) are asserted only on rows each test created.

use incisive_app::db::{lazy_pool, migrate};
use incisive_app::runs::store::{
    ActiveRunExists, add_cost, add_step, create_run, delete_all_runs, delete_run,
    delete_runs_older_than, get_run_for_user, list_runs, load_question,
    mark_stale_runs_interrupted, save_answer, set_status,
};
use incisive_app::runs::{RunMode, RunStatus, StepKind};
use secrecy::SecretString;
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

/// `mark_stale_runs_interrupted` is global: it would interrupt the queued/running
/// runs of every concurrently running test. So every test holds the read side of
/// this lock, and the stale-runs test alone takes the write side.
static ACTIVE_RUNS: tokio::sync::RwLock<()> = tokio::sync::RwLock::const_new(());

async fn test_pool() -> Option<(PgPool, tokio::sync::RwLockReadGuard<'static, ()>)> {
    let guard = ACTIVE_RUNS.read().await;
    Some((connect().await?, guard))
}

async fn test_pool_exclusive() -> Option<(PgPool, tokio::sync::RwLockWriteGuard<'static, ()>)> {
    let guard = ACTIVE_RUNS.write().await;
    Some((connect().await?, guard))
}

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
    let email = format!("runs-{}@example.com", Uuid::new_v4());
    sqlx::query_scalar(
        "INSERT INTO incisive.users (email, display_name, role, daily_cap_paise) \
         VALUES ($1, 'T', 'surgeon', 100000) RETURNING id",
    )
    .bind(email)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn count(pool: &PgPool, table: &str, run: Uuid) -> i64 {
    sqlx::query_scalar(&format!(
        "SELECT count(*) FROM incisive.{table} WHERE run_id = $1"
    ))
    .bind(run)
    .fetch_one(pool)
    .await
    .unwrap()
}

#[tokio::test]
async fn create_and_read_back() {
    let Some((pool, _guard)) = test_pool().await else {
        return;
    };
    let u = new_user(&pool).await;
    let q = "é".repeat(100); // multi-byte: title must cut at 80 chars, not bytes
    let id = create_run(&pool, u, &q, RunMode::Quick, 500)
        .await
        .unwrap()
        .unwrap();

    let v = get_run_for_user(&pool, u, id).await.unwrap().unwrap();
    assert_eq!(v.summary.id, id);
    assert_eq!(v.summary.mode, RunMode::Quick);
    assert_eq!(v.summary.status, RunStatus::Queued);
    assert_eq!(v.summary.cost_paise, 0);
    assert_eq!(v.summary.title.chars().count(), 80);
    assert_eq!(v.summary.title, "é".repeat(80));
    assert!(v.steps.is_empty());
    assert!(v.answer.is_none());

    assert_eq!(
        load_question(&pool, id).await.unwrap(),
        Some((u, q.clone(), RunMode::Quick, 500))
    );
    assert_eq!(load_question(&pool, Uuid::new_v4()).await.unwrap(), None);

    add_cost(&pool, id, 12).await.unwrap();
    add_cost(&pool, id, 30).await.unwrap();
    save_answer(&pool, id, &json!({"a": 1})).await.unwrap();
    let v = get_run_for_user(&pool, u, id).await.unwrap().unwrap();
    assert_eq!(v.summary.cost_paise, 42);
    assert_eq!(v.answer, Some(json!({"a": 1})));
    // saving again replaces
    save_answer(&pool, id, &json!({"a": 2})).await.unwrap();
    let v = get_run_for_user(&pool, u, id).await.unwrap().unwrap();
    assert_eq!(v.answer, Some(json!({"a": 2})));

    let list = list_runs(&pool, u, 10).await.unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].id, id);
}

#[tokio::test]
async fn second_active_run_is_refused() {
    let Some((pool, _guard)) = test_pool().await else {
        return;
    };
    let u = new_user(&pool).await;
    let first = create_run(&pool, u, "q1", RunMode::Quick, 500)
        .await
        .unwrap()
        .unwrap();
    let second = create_run(&pool, u, "q2", RunMode::Focused, 4000)
        .await
        .unwrap();
    assert_eq!(second, Err(ActiveRunExists));
    // still refused while running
    set_status(&pool, first, RunStatus::Running, None)
        .await
        .unwrap();
    assert_eq!(
        create_run(&pool, u, "q3", RunMode::Quick, 500)
            .await
            .unwrap(),
        Err(ActiveRunExists)
    );
    // another user is unaffected
    let other = new_user(&pool).await;
    assert!(
        create_run(&pool, other, "q", RunMode::Quick, 500)
            .await
            .unwrap()
            .is_ok()
    );
}

#[tokio::test]
async fn finished_run_allows_a_new_one() {
    let Some((pool, _guard)) = test_pool().await else {
        return;
    };
    let u = new_user(&pool).await;
    for terminal in [
        RunStatus::Done,
        RunStatus::Partial,
        RunStatus::Failed,
        RunStatus::Interrupted,
    ] {
        let id = create_run(&pool, u, "q", RunMode::Quick, 500)
            .await
            .unwrap()
            .unwrap();
        set_status(&pool, id, RunStatus::Running, None)
            .await
            .unwrap();
        let v = get_run_for_user(&pool, u, id).await.unwrap().unwrap();
        assert_eq!(v.summary.status, RunStatus::Running);
        let (started, finished): (bool, bool) = sqlx::query_as(
            "SELECT started_at IS NOT NULL, finished_at IS NOT NULL FROM incisive.runs WHERE id = $1",
        )
        .bind(id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(started && !finished);

        let kind = (terminal == RunStatus::Failed).then_some("timeout");
        set_status(&pool, id, terminal, kind).await.unwrap();
        let (finished, ek): (bool, Option<String>) = sqlx::query_as(
            "SELECT finished_at IS NOT NULL, error_kind FROM incisive.runs WHERE id = $1",
        )
        .bind(id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(finished);
        assert_eq!(ek.as_deref(), kind);
    }
    // four finished runs, and a fifth is still allowed
    assert!(
        create_run(&pool, u, "again", RunMode::Quick, 500)
            .await
            .unwrap()
            .is_ok()
    );
}

#[tokio::test]
async fn other_user_cannot_read_or_delete() {
    let Some((pool, _guard)) = test_pool().await else {
        return;
    };
    let owner = new_user(&pool).await;
    let other = new_user(&pool).await;
    let id = create_run(&pool, owner, "private", RunMode::Quick, 500)
        .await
        .unwrap()
        .unwrap();
    assert!(get_run_for_user(&pool, other, id).await.unwrap().is_none());
    assert!(!delete_run(&pool, other, id).await.unwrap());
    assert!(list_runs(&pool, other, 10).await.unwrap().is_empty());
    // still there for the owner
    assert!(get_run_for_user(&pool, owner, id).await.unwrap().is_some());
    assert!(delete_run(&pool, owner, id).await.unwrap());
    assert!(get_run_for_user(&pool, owner, id).await.unwrap().is_none());
}

#[tokio::test]
async fn steps_are_ordered_and_capped_at_seq() {
    let Some((pool, _guard)) = test_pool().await else {
        return;
    };
    let u = new_user(&pool).await;
    let id = create_run(&pool, u, "q", RunMode::Focused, 4000)
        .await
        .unwrap()
        .unwrap();
    let kinds = [
        StepKind::Frame,
        StepKind::Search,
        StepKind::Screen,
        StepKind::Note,
    ];
    let mut seqs = vec![];
    for (i, k) in kinds.into_iter().enumerate() {
        seqs.push(add_step(&pool, id, k, &format!("line {i}")).await.unwrap());
    }
    assert_eq!(seqs, vec![1, 2, 3, 4]);

    let v = get_run_for_user(&pool, u, id).await.unwrap().unwrap();
    let got: Vec<(i32, StepKind, &str)> = v
        .steps
        .iter()
        .map(|s| (s.seq, s.kind, s.line.as_str()))
        .collect();
    assert_eq!(
        got,
        vec![
            (1, StepKind::Frame, "line 0"),
            (2, StepKind::Search, "line 1"),
            (3, StepKind::Screen, "line 2"),
            (4, StepKind::Note, "line 3"),
        ]
    );
    // a second run's steps start again at 1
    let u2 = new_user(&pool).await;
    let id2 = create_run(&pool, u2, "q", RunMode::Quick, 500)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(add_step(&pool, id2, StepKind::Frame, "x").await.unwrap(), 1);
}

#[tokio::test]
async fn delete_cascades_steps_and_answer() {
    let Some((pool, _guard)) = test_pool().await else {
        return;
    };
    let u = new_user(&pool).await;
    let id = create_run(&pool, u, "q", RunMode::Quick, 500)
        .await
        .unwrap()
        .unwrap();
    add_step(&pool, id, StepKind::Frame, "a").await.unwrap();
    save_answer(&pool, id, &json!({"x": 1})).await.unwrap();
    assert_eq!(count(&pool, "run_steps", id).await, 1);
    assert_eq!(count(&pool, "run_answers", id).await, 1);
    assert!(delete_run(&pool, u, id).await.unwrap());
    assert_eq!(count(&pool, "run_steps", id).await, 0);
    assert_eq!(count(&pool, "run_answers", id).await, 0);
    assert!(!delete_run(&pool, u, id).await.unwrap());
}

#[tokio::test]
async fn delete_all_only_touches_own_runs() {
    let Some((pool, _guard)) = test_pool().await else {
        return;
    };
    let a = new_user(&pool).await;
    let b = new_user(&pool).await;
    let mut a_ids = vec![];
    for s in [RunStatus::Done, RunStatus::Failed, RunStatus::Queued] {
        let id = create_run(&pool, a, "qa", RunMode::Quick, 500)
            .await
            .unwrap()
            .unwrap();
        add_step(&pool, id, StepKind::Frame, "s").await.unwrap();
        set_status(&pool, id, s, None).await.unwrap();
        a_ids.push(id);
    }
    let b_id = create_run(&pool, b, "qb", RunMode::Quick, 500)
        .await
        .unwrap()
        .unwrap();
    add_step(&pool, b_id, StepKind::Frame, "s").await.unwrap();

    assert_eq!(delete_all_runs(&pool, a).await.unwrap(), 3);
    assert!(list_runs(&pool, a, 10).await.unwrap().is_empty());
    for id in a_ids {
        assert_eq!(count(&pool, "run_steps", id).await, 0);
    }
    assert_eq!(list_runs(&pool, b, 10).await.unwrap().len(), 1);
    assert_eq!(count(&pool, "run_steps", b_id).await, 1);
    assert_eq!(delete_all_runs(&pool, a).await.unwrap(), 0);
}

#[tokio::test]
async fn stale_runs_marked_interrupted() {
    let Some((pool, _guard)) = test_pool_exclusive().await else {
        return;
    };
    let (u1, u2, u3) = (
        new_user(&pool).await,
        new_user(&pool).await,
        new_user(&pool).await,
    );
    let queued = create_run(&pool, u1, "q", RunMode::Quick, 500)
        .await
        .unwrap()
        .unwrap();
    let running = create_run(&pool, u2, "q", RunMode::Quick, 500)
        .await
        .unwrap()
        .unwrap();
    set_status(&pool, running, RunStatus::Running, None)
        .await
        .unwrap();
    let done = create_run(&pool, u3, "q", RunMode::Quick, 500)
        .await
        .unwrap()
        .unwrap();
    set_status(&pool, done, RunStatus::Done, None)
        .await
        .unwrap();

    let marked = mark_stale_runs_interrupted(&pool).await.unwrap();
    assert!(marked.contains(&(queued, u1)));
    assert!(marked.contains(&(running, u2)));
    assert!(!marked.iter().any(|(id, _)| *id == done));

    let status = |u, id| {
        let pool = pool.clone();
        async move {
            get_run_for_user(&pool, u, id)
                .await
                .unwrap()
                .unwrap()
                .summary
                .status
        }
    };
    assert_eq!(status(u1, queued).await, RunStatus::Interrupted);
    assert_eq!(status(u2, running).await, RunStatus::Interrupted);
    assert_eq!(status(u3, done).await, RunStatus::Done);
    // interrupted runs no longer block a new run
    assert!(
        create_run(&pool, u1, "q", RunMode::Quick, 500)
            .await
            .unwrap()
            .is_ok()
    );
    let finished: bool =
        sqlx::query_scalar("SELECT finished_at IS NOT NULL FROM incisive.runs WHERE id = $1")
            .bind(queued)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(finished);
}

#[tokio::test]
async fn retention_deletes_only_older_than_365_days() {
    let Some((pool, _guard)) = test_pool().await else {
        return;
    };
    let u = new_user(&pool).await;
    let mut ids = vec![];
    for days in [400, 366, 364, 1] {
        let id = create_run(&pool, u, "q", RunMode::Quick, 500)
            .await
            .unwrap()
            .unwrap();
        set_status(&pool, id, RunStatus::Done, None).await.unwrap();
        add_step(&pool, id, StepKind::Frame, "s").await.unwrap();
        save_answer(&pool, id, &json!({})).await.unwrap();
        sqlx::query(
            "UPDATE incisive.runs SET created_at = now() - make_interval(days => $2) WHERE id = $1",
        )
        .bind(id)
        .bind(days)
        .execute(&pool)
        .await
        .unwrap();
        ids.push(id);
    }
    // Other tests' old rows may also be removed; only ours are asserted.
    let n = delete_runs_older_than(&pool, 365).await.unwrap();
    assert!(n >= 2);
    let left: Vec<Uuid> = list_runs(&pool, u, 10)
        .await
        .unwrap()
        .into_iter()
        .map(|r| r.id)
        .collect();
    assert_eq!(left.len(), 2);
    assert!(left.contains(&ids[2]) && left.contains(&ids[3]));
    assert_eq!(count(&pool, "run_steps", ids[0]).await, 0);
    assert_eq!(count(&pool, "run_answers", ids[1]).await, 0);
    assert_eq!(count(&pool, "run_steps", ids[2]).await, 1);
}
