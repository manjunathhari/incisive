//! Run-store tests, gated on `TEST_DATABASE_URL`. Every test creates its own
//! user (unique email), so tests running in parallel on one DB never share rows.
//! The global operations (`interrupt_stale_runs`, recovery, self-heal,
//! `delete_runs_older_than`) are asserted only on rows each test created.

use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode},
};
use claude_client::Usage;
use http_body_util::BodyExt;
use incisive_app::db::{lazy_pool, migrate};
use incisive_app::identity::User;
use incisive_app::runs::budget::{BudgetError, RunBudget};
use incisive_app::runs::store::{
    ActiveRunExists, add_cost, add_step, create_run, delete_all_runs, delete_run,
    delete_runs_older_than, get_run_for_user, heartbeat, interrupt_stale_runs, list_runs,
    load_question, save_answer, set_status,
};
use incisive_app::runs::{RunMode, RunStatus, StepKind};
use incisive_app::spend::{CapCheck, CapsConfig, Prices, charge_call, release, reserve, totals};
use leptos::config::LeptosOptions;
use secrecy::SecretString;
use serde_json::json;
use sqlx::PgPool;
use std::time::{Duration, Instant};
use tower::ServiceExt;
use uuid::Uuid;

/// `interrupt_stale_runs` is global: it would interrupt the queued/running
/// runs of every concurrently running test. So every test holds the read side of
/// this lock, and the stale-runs test alone takes the write side.
static ACTIVE_RUNS: tokio::sync::RwLock<()> = tokio::sync::RwLock::const_new(());

/// Longest a database test may hold [`ACTIVE_RUNS`] (they take ~2 s at most).
const TEST_DEADLINE: Duration = Duration::from_secs(60);

/// Safety net: a test still holding its guard after [`TEST_DEADLINE`] is hung.
/// A hung holder would block every test queued behind the lock, so the whole
/// binary is stopped with the test's name rather than left to stall CI.
/// Dropping it (the sender) wakes the watchdog thread, which then just exits.
struct Watchdog(#[allow(dead_code)] std::sync::mpsc::Sender<()>);

impl Watchdog {
    fn start() -> Self {
        let name = std::thread::current()
            .name()
            .unwrap_or("unnamed test")
            .to_string();
        let (tx, rx) = std::sync::mpsc::channel::<()>();
        std::thread::spawn(move || {
            if let Err(std::sync::mpsc::RecvTimeoutError::Timeout) = rx.recv_timeout(TEST_DEADLINE)
            {
                eprintln!("test {name} exceeded {TEST_DEADLINE:?}; aborting the runs binary");
                std::process::exit(1);
            }
        });
        Watchdog(tx)
    }
}

/// Held for the whole test: the lock side, plus its watchdog.
struct TestGuard<G> {
    _lock: G,
    _watchdog: Watchdog,
}

async fn test_pool() -> Option<(PgPool, TestGuard<tokio::sync::RwLockReadGuard<'static, ()>>)> {
    let lock = ACTIVE_RUNS.read().await;
    let guard = TestGuard {
        _lock: lock,
        _watchdog: Watchdog::start(),
    };
    Some((connect().await?, guard))
}

async fn test_pool_exclusive() -> Option<(
    PgPool,
    TestGuard<tokio::sync::RwLockWriteGuard<'static, ()>>,
)> {
    let lock = ACTIVE_RUNS.write().await;
    let guard = TestGuard {
        _lock: lock,
        _watchdog: Watchdog::start(),
    };
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

/// Move a run timestamp column `secs` seconds into the past (DB clock).
async fn backdate(pool: &PgPool, run: Uuid, column: &str, secs: i64) {
    sqlx::query(&format!(
        "UPDATE incisive.runs SET {column} = now() - make_interval(secs => $2) WHERE id = $1"
    ))
    .bind(run)
    .bind(secs as f64)
    .execute(pool)
    .await
    .unwrap();
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
    // An answer is stored only while the run is running.
    assert!(
        set_status(&pool, id, RunStatus::Running, None)
            .await
            .unwrap()
    );
    assert!(save_answer(&pool, id, &json!({"a": 1})).await.unwrap());
    let v = get_run_for_user(&pool, u, id).await.unwrap().unwrap();
    assert_eq!(v.summary.cost_paise, 42);
    assert_eq!(v.answer, Some(json!({"a": 1})));
    // saving again replaces
    assert!(save_answer(&pool, id, &json!({"a": 2})).await.unwrap());
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
    assert!(
        set_status(&pool, id, RunStatus::Running, None)
            .await
            .unwrap()
    );
    assert!(save_answer(&pool, id, &json!({"x": 1})).await.unwrap());
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
    // Make the active two stale: queued for 2 minutes, last heartbeat 2 minutes ago.
    backdate(&pool, queued, "created_at", 120).await;
    backdate(&pool, running, "heartbeat_at", 120).await;
    backdate(&pool, done, "created_at", 120).await;

    let marked = interrupt_stale_runs(&pool).await.unwrap();
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
    let kinds: Vec<Option<String>> =
        sqlx::query_scalar("SELECT error_kind FROM incisive.runs WHERE id = ANY($1) ORDER BY id")
            .bind(vec![queued, running])
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(kinds, vec![Some("interrupted".to_string()); 2]);
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
        set_status(&pool, id, RunStatus::Running, None)
            .await
            .unwrap();
        add_step(&pool, id, StepKind::Frame, "s").await.unwrap();
        assert!(save_answer(&pool, id, &json!({})).await.unwrap());
        set_status(&pool, id, RunStatus::Done, None).await.unwrap();
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

// ---------- terminal status ----------

#[tokio::test]
async fn terminal_status_is_not_overwritten() {
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
        assert!(
            set_status(&pool, id, RunStatus::Running, None)
                .await
                .unwrap()
        );
        assert!(set_status(&pool, id, terminal, None).await.unwrap());
        let finished = |pool: &PgPool| {
            let pool = pool.clone();
            async move {
                sqlx::query_scalar::<_, Option<chrono::DateTime<chrono::Utc>>>(
                    "SELECT finished_at FROM incisive.runs WHERE id = $1",
                )
                .bind(id)
                .fetch_one(&pool)
                .await
                .unwrap()
            }
        };
        let before = finished(&pool).await;
        for later in [
            RunStatus::Running,
            RunStatus::Queued,
            RunStatus::Done,
            RunStatus::Failed,
        ] {
            assert!(!set_status(&pool, id, later, Some("x")).await.unwrap());
        }
        let v = get_run_for_user(&pool, u, id).await.unwrap().unwrap();
        assert_eq!(v.summary.status, terminal);
        assert_eq!(finished(&pool).await, before);
    }
}

// ---------- run budget ----------

fn prices() -> Prices {
    Prices::from_json(r#"{"m-test": {"input": 1.0, "output": 5.0}}"#).unwrap()
}

fn budget(run: Uuid, user: i64, limit: i64, started: Instant) -> std::sync::Arc<RunBudget> {
    std::sync::Arc::new(RunBudget::new(run, user, limit, started, prices(), 88.0))
}

fn big_global() -> CapsConfig {
    CapsConfig {
        global_monthly_paise: i64::MAX / 4,
    }
}

async fn full_user(pool: &PgPool, id: i64) -> User {
    let email: String = sqlx::query_scalar("SELECT email FROM incisive.users WHERE id = $1")
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap();
    incisive_app::identity::find_user(pool, &email)
        .await
        .unwrap()
        .unwrap()
}

const MARKER: &str = "UNIQUE-QUESTION-MARKER-7f3a";

/// A run with `limit` reserved up front, as `run_ask` will do.
async fn reserved_run(pool: &PgPool, limit: i64) -> (i64, Uuid) {
    let uid = new_user(pool).await;
    let run = create_run(pool, uid, MARKER, RunMode::Focused, limit)
        .await
        .unwrap()
        .unwrap();
    let user = full_user(pool, uid).await;
    let c = reserve(pool, &user, &big_global(), limit, run, "run")
        .await
        .unwrap();
    assert!(matches!(c, CapCheck::Ok { .. }));
    (uid, run)
}

async fn reservation(pool: &PgPool, run: Uuid) -> Option<i64> {
    sqlx::query_scalar(
        "SELECT paise FROM incisive.spend WHERE request_id = $1 AND kind = 'reservation'",
    )
    .bind(run)
    .fetch_optional(pool)
    .await
    .unwrap()
}

async fn charges(pool: &PgPool, uid: i64) -> Vec<(i64, String)> {
    sqlx::query_as(
        "SELECT paise, purpose FROM incisive.spend WHERE user_id = $1 AND kind = 'charge' ORDER BY id",
    )
    .bind(uid)
    .fetch_all(pool)
    .await
    .unwrap()
}

fn usage(i: u32, o: u32) -> Usage {
    Usage {
        input_tokens: i,
        output_tokens: o,
        ..Usage::default()
    }
}

#[tokio::test]
async fn charge_call_reduces_reservation_and_records_charge() {
    let Some((pool, _guard)) = test_pool().await else {
        return;
    };
    let (uid, run) = reserved_run(&pool, 4000).await;
    let call = Uuid::new_v4();
    assert!(
        charge_call(&pool, uid, run, call, "m-test", &usage(10, 10), 300)
            .await
            .unwrap()
    );
    assert_eq!(reservation(&pool, run).await, Some(3700));
    assert_eq!(charges(&pool, uid).await, [(300, format!("run:{run}"))]);
    let t = totals(&pool, uid).await.unwrap();
    assert_eq!((t.today_paise, t.month_paise), (4000, 4000));
    // Replaying the same call id neither double-charges nor double-decrements.
    assert!(
        !charge_call(&pool, uid, run, call, "m-test", &usage(10, 10), 300)
            .await
            .unwrap()
    );
    assert_eq!(reservation(&pool, run).await, Some(3700));
    assert_eq!(charges(&pool, uid).await.len(), 1);
}

#[tokio::test]
async fn charge_beyond_reservation_floors_at_zero() {
    let Some((pool, _guard)) = test_pool().await else {
        return;
    };
    let (uid, run) = reserved_run(&pool, 100).await;
    charge_call(&pool, uid, run, Uuid::new_v4(), "m-test", &usage(1, 1), 250)
        .await
        .unwrap();
    assert_eq!(reservation(&pool, run).await, Some(0));
    assert_eq!(charges(&pool, uid).await[0].0, 250);
    // Real cost counts in full even past the reservation.
    assert_eq!(totals(&pool, uid).await.unwrap().today_paise, 250);
}

#[tokio::test]
async fn release_returns_remaining_and_is_idempotent() {
    let Some((pool, _guard)) = test_pool().await else {
        return;
    };
    let (uid, run) = reserved_run(&pool, 4000).await;
    charge_call(&pool, uid, run, Uuid::new_v4(), "m-test", &usage(1, 1), 300)
        .await
        .unwrap();
    assert_eq!(release(&pool, uid, run).await.unwrap(), 3700);
    assert_eq!(reservation(&pool, run).await, None);
    assert_eq!(totals(&pool, uid).await.unwrap().today_paise, 300);
    assert_eq!(release(&pool, uid, run).await.unwrap(), 0);
}

#[tokio::test]
async fn charge_and_release_survive_a_missing_reservation() {
    let Some((pool, _guard)) = test_pool().await else {
        return;
    };
    let uid = new_user(&pool).await;
    let run = Uuid::new_v4();
    assert_eq!(release(&pool, uid, run).await.unwrap(), 0);
    charge_call(&pool, uid, run, Uuid::new_v4(), "m-test", &usage(1, 1), 40)
        .await
        .unwrap();
    assert_eq!(charges(&pool, uid).await[0].0, 40);
    assert_eq!(reservation(&pool, run).await, None);
}

fn hold(b: &std::sync::Arc<RunBudget>, n: i64) -> incisive_app::runs::budget::Hold {
    b.try_hold(n).expect("hold fits")
}

async fn audit_rows(pool: &PgPool, uid: i64) -> Vec<(String, serde_json::Value)> {
    sqlx::query_as("SELECT event, detail FROM incisive.audit WHERE user_id = $1 ORDER BY seq")
        .bind(uid)
        .fetch_all(pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn budget_charge_books_audits_and_adds_run_cost() {
    let Some((pool, _guard)) = test_pool().await else {
        return;
    };
    let (uid, run) = reserved_run(&pool, 4000).await;
    let b = budget(run, uid, 4000, Instant::now());
    // 1,000 in + 150 out at $1/$5 per Mtok, 88 INR/USD = 15.4 paise -> 16.
    let h = hold(&b, 100);
    assert_eq!(
        b.charge(&pool, h, Uuid::new_v4(), "m-test", &usage(1000, 150))
            .await
            .unwrap(),
        16
    );
    assert_eq!(b.spent(), 16);
    assert_eq!(reservation(&pool, run).await, Some(4000 - 16));
    let v = get_run_for_user(&pool, uid, run).await.unwrap().unwrap();
    assert_eq!(v.summary.cost_paise, 16);
    let rows = audit_rows(&pool, uid).await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].0, "charged");
    assert_eq!(rows[0].1["paise"], 16);
    assert_eq!(rows[0].1["model"], "m-test");
    // The question text never reaches the audit detail.
    let q: String = sqlx::query_scalar("SELECT question FROM incisive.runs WHERE id = $1")
        .bind(run)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(q, MARKER);
    assert!(!rows[0].1.to_string().contains(MARKER));
    // The hold was converted: all 4000 can be held again less the 16 spent.
    assert!(b.try_hold(3984).is_some());
}

#[tokio::test]
async fn unknown_model_is_an_error_not_free() {
    let Some((pool, _guard)) = test_pool().await else {
        return;
    };
    let (uid, run) = reserved_run(&pool, 4000).await;
    let b = budget(run, uid, 4000, Instant::now());
    let h = hold(&b, 100);
    let err = b
        .charge(&pool, h, Uuid::new_v4(), "mystery", &usage(5, 5))
        .await
        .unwrap_err();
    assert!(matches!(err, BudgetError::UnknownModel));
    assert_eq!(b.spent(), 0);
    assert!(charges(&pool, uid).await.is_empty());
    assert_eq!(reservation(&pool, run).await, Some(4000));
    // The failed call's hold was freed.
    assert!(b.try_hold(4000).is_some());
}

#[tokio::test]
async fn retry_with_same_call_id_books_once() {
    let Some((pool, _guard)) = test_pool().await else {
        return;
    };
    let (uid, run) = reserved_run(&pool, 4000).await;
    let b = budget(run, uid, 4000, Instant::now());
    let call = Uuid::new_v4();
    assert_eq!(
        b.charge(&pool, hold(&b, 50), call, "m-test", &usage(1000, 150))
            .await
            .unwrap(),
        16
    );
    assert_eq!(
        b.charge(&pool, hold(&b, 50), call, "m-test", &usage(1000, 150))
            .await
            .unwrap(),
        16
    );
    assert_eq!(b.spent(), 16);
    assert_eq!(charges(&pool, uid).await.len(), 1);
    assert_eq!(reservation(&pool, run).await, Some(4000 - 16));
    let v = get_run_for_user(&pool, uid, run).await.unwrap().unwrap();
    assert_eq!(v.summary.cost_paise, 16);
    assert_eq!(audit_rows(&pool, uid).await.len(), 1);
}

#[tokio::test]
async fn booked_but_not_recorded_then_retry_completes_without_double_booking() {
    let Some((pool, _guard)) = test_pool().await else {
        return;
    };
    let (uid, run) = reserved_run(&pool, 4000).await;
    let b = budget(run, uid, 4000, Instant::now());
    let call = Uuid::new_v4();

    // Make add_cost fail for this run only: a trigger that rejects its updates.
    let fname = format!("fail_{}", run.simple());
    sqlx::query(&format!(
        "CREATE FUNCTION incisive.{fname}() RETURNS trigger LANGUAGE plpgsql AS \
         $$ BEGIN RAISE EXCEPTION 'injected'; END $$"
    ))
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(&format!(
        "CREATE TRIGGER {fname} BEFORE UPDATE ON incisive.runs FOR EACH ROW \
         WHEN (NEW.id = '{run}') EXECUTE FUNCTION incisive.{fname}()"
    ))
    .execute(&pool)
    .await
    .unwrap();

    let err = b
        .charge(&pool, hold(&b, 50), call, "m-test", &usage(1000, 150))
        .await
        .unwrap_err();
    assert!(
        matches!(err, BudgetError::BookedButNotRecorded(_)),
        "{err:?}"
    );
    // Booked: ledger and spent reflect it; the run cost and audit do not yet.
    assert_eq!(b.spent(), 16);
    assert_eq!(charges(&pool, uid).await.len(), 1);
    let v = get_run_for_user(&pool, uid, run).await.unwrap().unwrap();
    assert_eq!(v.summary.cost_paise, 0);
    assert!(audit_rows(&pool, uid).await.is_empty());

    sqlx::query(&format!("DROP TRIGGER {fname} ON incisive.runs"))
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(&format!("DROP FUNCTION incisive.{fname}()"))
        .execute(&pool)
        .await
        .unwrap();

    // Retry with the same call id: completes the records, books nothing twice.
    assert_eq!(
        b.charge(&pool, hold(&b, 50), call, "m-test", &usage(1000, 150))
            .await
            .unwrap(),
        16
    );
    assert_eq!(b.spent(), 16);
    assert_eq!(charges(&pool, uid).await.len(), 1);
    assert_eq!(reservation(&pool, run).await, Some(4000 - 16));
    let v = get_run_for_user(&pool, uid, run).await.unwrap().unwrap();
    assert_eq!(v.summary.cost_paise, 16);
    assert_eq!(audit_rows(&pool, uid).await.len(), 1);
}

#[test]
fn concurrent_holds_admit_only_what_fits() {
    let b = budget(Uuid::new_v4(), 1, 1000, Instant::now());
    let holds: Vec<_> = std::thread::scope(|s| {
        let handles: Vec<_> = (0..32).map(|_| s.spawn(|| b.try_hold(100))).collect();
        handles
            .into_iter()
            .filter_map(|h| h.join().unwrap())
            .collect()
    });
    assert_eq!(holds.len(), 10);
    assert!(b.try_hold(1).is_none());
    assert!(!b.can_afford(1));
    drop(holds);
    assert!(b.can_afford(1000));
}

#[test]
fn dropped_hold_frees_its_amount() {
    let b = budget(Uuid::new_v4(), 1, 1000, Instant::now());
    b.record_spent_for_test(300);
    let h = b.try_hold(700).unwrap();
    assert!(b.try_hold(1).is_none());
    drop(h);
    assert!(b.try_hold(700).is_some());
    assert!(b.try_hold(-1).is_none());
}

#[test]
fn wrap_up_at_70_percent_or_3_minutes() {
    let t0 = Instant::now();
    let b = budget(Uuid::new_v4(), 1, 1000, t0);
    assert!(!b.should_wrap_up(t0));
    assert!(!b.should_wrap_up(t0 + Duration::from_secs(179)));
    assert!(b.should_wrap_up(t0 + Duration::from_secs(180)));

    let spent = |n: i64| {
        let b = budget(Uuid::new_v4(), 1, 1000, t0);
        b.record_spent_for_test(n);
        b
    };
    assert!(!spent(699).should_wrap_up(t0));
    assert!(spent(700).should_wrap_up(t0));
}

#[test]
fn hard_stop_at_6_minutes() {
    let t0 = Instant::now();
    let b = budget(Uuid::new_v4(), 1, 1000, t0);
    assert!(!b.past_hard_stop(t0 + Duration::from_secs(359)));
    assert!(b.past_hard_stop(t0 + Duration::from_secs(360)));
}

#[test]
fn can_afford_checks_the_limit() {
    let b = budget(Uuid::new_v4(), 1, 1000, Instant::now());
    b.record_spent_for_test(900);
    assert!(b.can_afford(100));
    assert!(!b.can_afford(101));
    assert!(b.can_afford(0));
    assert!(!b.can_afford(-1));
}

// ---------- worker: accepted questions become background runs ----------

use incisive_app::agent::{PipelineFuture, PipelineOutcome, RunContext};
use incisive_app::ask::{AskOutcome, run_ask};
use incisive_app::gate::OfflineClassifier;
use incisive_app::identity::{IapVerifier, JwkSource};
use incisive_app::runs::RunsConfig;
use incisive_app::runs::worker::{WorkerHandle, recover_stale_runs};
use incisive_app::server::AppState;
use std::sync::Arc;

/// Decided Medical by rule, so the offline gate (no model) accepts it.
const RULE_QUESTION: &str =
    "52 y/o M, robotic LAR, ICG for leak? Evidence on anastomotic resection";
const RUN_STARTED: &str =
    "Working on it \u{2014} you can leave this page; the answer will be in My questions.";
const RUN_IN_PROGRESS: &str = "You already have a question running.";

fn app_state(pool: PgPool) -> AppState {
    AppState {
        pool,
        verifier: Arc::new(IapVerifier::new(
            "aud".into(),
            JwkSource::Static(serde_json::from_str(r#"{"keys":[]}"#).unwrap()),
        )),
        gate: Arc::new(OfflineClassifier),
        prices: prices(),
        usd_inr: 88.0,
        caps: big_global(),
        gate_model: "m-test".into(),
        dev_mode: true,
        runs: RunsConfig::default(),
        // Phase 2 tests: Quick keeps the classic pipeline unless a test says otherwise.
        quick_engine: incisive_app::runs::QuickEngine::Classic,
        full_engine: incisive_app::runs::Engine::Classic,
        // Runs started by `run_ask` use the deterministic placeholder.
        worker: WorkerHandle::with_pipeline(incisive_app::agent::testing::placeholder_pipeline),
        llm: Some(incisive_app::agent::testing::idle_llm("m-test")),
        lit: incisive_app::agent::testing::offline_literature(),
    }
}

async fn new_user_with_cap(pool: &PgPool, daily_cap: i64) -> User {
    let email = format!("runs-{}@example.com", Uuid::new_v4());
    sqlx::query(
        "INSERT INTO incisive.users (email, display_name, role, daily_cap_paise) \
         VALUES ($1, 'T', 'surgeon', $2)",
    )
    .bind(&email)
    .bind(daily_cap)
    .execute(pool)
    .await
    .unwrap();
    incisive_app::identity::find_user(pool, &email)
        .await
        .unwrap()
        .unwrap()
}

/// Wait (up to 5 s) until the worker has completely finished with `run`.
async fn wait_idle(st: &AppState, run: Uuid) {
    for _ in 0..500 {
        if !st.worker.is_running(run) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("run {run} did not finish within 5 s");
}

/// Poll the store (up to 5 s) until the run reaches `want`.
async fn poll_status(pool: &PgPool, uid: i64, run: Uuid, want: RunStatus) {
    for _ in 0..500 {
        let v = get_run_for_user(pool, uid, run).await.unwrap().unwrap();
        if v.summary.status == want {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("run {run} never reached {want:?}");
}

async fn run_events(pool: &PgPool, run: Uuid) -> Vec<(String, serde_json::Value)> {
    sqlx::query_as("SELECT event, detail FROM incisive.audit WHERE request_id = $1 ORDER BY seq")
        .bind(run)
        .fetch_all(pool)
        .await
        .unwrap()
}

async fn error_kind(pool: &PgPool, run: Uuid) -> Option<String> {
    sqlx::query_scalar("SELECT error_kind FROM incisive.runs WHERE id = $1")
        .bind(run)
        .fetch_one(pool)
        .await
        .unwrap()
}

fn started(outcome: Result<AskOutcome, incisive_app::ask::AskFailure>) -> Uuid {
    match outcome {
        Ok(AskOutcome::RunStarted {
            run_id, message, ..
        }) => {
            assert_eq!(message, RUN_STARTED);
            run_id
        }
        other => panic!("expected RunStarted, got {other:?}"),
    }
}

#[tokio::test]
async fn accepted_question_starts_a_run_and_finishes() {
    let Some((pool, _guard)) = test_pool().await else {
        return;
    };
    let st = app_state(pool.clone());
    let user = new_user_with_cap(&pool, 100_000).await;

    let outcome = run_ask(&st, &user, RULE_QUESTION, RunMode::Focused).await;
    let wire = serde_json::to_value(outcome.as_ref().unwrap()).unwrap();
    let run = started(outcome);
    assert_eq!(
        wire,
        json!({"status": "run_started", "run_id": run, "message": RUN_STARTED, "alert": null})
    );
    poll_status(&pool, user.id, run, RunStatus::Done).await;
    wait_idle(&st, run).await;

    let v = get_run_for_user(&pool, user.id, run)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(v.summary.mode, RunMode::Focused);
    let kinds: Vec<StepKind> = v.steps.iter().map(|s| s.kind).collect();
    assert_eq!(
        kinds,
        [
            StepKind::Frame,
            StepKind::Search,
            StepKind::Screen,
            StepKind::Retrieve,
            StepKind::Extract,
            StepKind::Write,
            StepKind::Check,
            StepKind::Present
        ]
    );
    assert_eq!(v.answer, Some(json!({"placeholder": true})));
    assert_eq!(error_kind(&pool, run).await, None);

    // The ask's own events, then the worker's under the run id.
    let events: Vec<String> = audit_rows(&pool, user.id)
        .await
        .into_iter()
        .map(|(e, _)| e)
        .collect();
    assert_eq!(
        events,
        ["gate_decided", "accepted", "run_started", "run_finished"]
    );
    let rows = audit_rows(&pool, user.id).await;
    assert_eq!(
        rows[2].1,
        json!({"run_id": run.to_string(), "mode": "focused"})
    );
    let finished = run_events(&pool, run).await;
    assert_eq!(finished.len(), 1);
    assert_eq!(
        finished[0].1,
        json!({"run_id": run.to_string(), "status": "done", "cost_paise": 0, "steps": 8})
    );

    // Quick mode skips Retrieve.
    let quick = started(run_ask(&st, &user, RULE_QUESTION, RunMode::Quick).await);
    poll_status(&pool, user.id, quick, RunStatus::Done).await;
    wait_idle(&st, quick).await;
    let v = get_run_for_user(&pool, user.id, quick)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(v.summary.mode, RunMode::Quick);
    assert_eq!(v.steps.len(), 7);
    assert!(v.steps.iter().all(|s| s.kind != StepKind::Retrieve));
    let limit: i64 = sqlx::query_scalar("SELECT limit_paise FROM incisive.runs WHERE id = $1")
        .bind(quick)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(limit, 1500);
}

#[tokio::test]
async fn second_question_while_running_gets_run_in_progress() {
    let Some((pool, _guard)) = test_pool().await else {
        return;
    };
    let st = app_state(pool.clone());
    let user = new_user_with_cap(&pool, 100_000).await;
    // An active run already holds the user's one slot.
    let first = create_run(&pool, user.id, "q", RunMode::Quick, 500)
        .await
        .unwrap()
        .unwrap();

    let outcome = run_ask(&st, &user, RULE_QUESTION, RunMode::Focused)
        .await
        .unwrap();
    assert_eq!(
        outcome,
        AskOutcome::RunInProgress {
            message: RUN_IN_PROGRESS.into()
        }
    );
    assert_eq!(
        serde_json::to_value(&outcome).unwrap(),
        json!({"status": "run_in_progress", "message": RUN_IN_PROGRESS})
    );
    let runs = list_runs(&pool, user.id, 10).await.unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].id, first);
    // No run reservation was taken.
    let reservations: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM incisive.spend WHERE user_id = $1 AND kind = 'reservation'",
    )
    .bind(user.id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(reservations, 0);
    let events: Vec<String> = audit_rows(&pool, user.id)
        .await
        .into_iter()
        .map(|(e, _)| e)
        .collect();
    assert!(!events.contains(&"run_started".to_string()), "{events:?}");
    // Leave nothing active behind.
    assert!(
        set_status(&pool, first, RunStatus::Failed, None)
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn run_reservation_released_at_end() {
    let Some((pool, _guard)) = test_pool().await else {
        return;
    };
    let st = app_state(pool.clone());
    let user = new_user_with_cap(&pool, 100_000).await;
    let run = started(run_ask(&st, &user, RULE_QUESTION, RunMode::Focused).await);
    wait_idle(&st, run).await;
    poll_status(&pool, user.id, run, RunStatus::Done).await;

    assert_eq!(reservation(&pool, run).await, None);
    let charged: i64 = sqlx::query_scalar(
        "SELECT COALESCE(SUM(paise), 0)::BIGINT FROM incisive.spend WHERE user_id = $1 AND kind = 'charge'",
    )
    .bind(user.id)
    .fetch_one(&pool)
    .await
    .unwrap();
    let t = totals(&pool, user.id).await.unwrap();
    assert_eq!(t.today_paise, charged);
}

#[tokio::test]
async fn over_cap_for_run_limit_creates_no_run() {
    let Some((pool, _guard)) = test_pool().await else {
        return;
    };
    let st = app_state(pool.clone());
    // Room for the gate estimate and a Quick run (1500), not a Focused one
    // (4000).
    let user = new_user_with_cap(&pool, 2_000).await;
    let outcome = run_ask(&st, &user, RULE_QUESTION, RunMode::Focused)
        .await
        .unwrap();
    assert!(matches!(outcome, AskOutcome::OverCap { .. }), "{outcome:?}");
    assert!(list_runs(&pool, user.id, 10).await.unwrap().is_empty());
    let reservations: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM incisive.spend WHERE user_id = $1 AND kind = 'reservation'",
    )
    .bind(user.id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(reservations, 0);
    let rows = audit_rows(&pool, user.id).await;
    let events: Vec<&str> = rows.iter().map(|(e, _)| e.as_str()).collect();
    assert_eq!(events, ["gate_decided", "accepted", "cap_blocked"]);
    assert_eq!(rows[2].1, json!({"estimate_paise": 4000}));

    // A Quick run (1500) still fits.
    let run = started(run_ask(&st, &user, RULE_QUESTION, RunMode::Quick).await);
    wait_idle(&st, run).await;
}

#[tokio::test]
async fn restart_recovery_interrupts_and_releases() {
    let Some((pool, _guard)) = test_pool_exclusive().await else {
        return;
    };
    // A running run whose heartbeat stopped, and a queued run nobody picked up.
    let (u1, running) = reserved_run(&pool, 4000).await;
    assert!(
        set_status(&pool, running, RunStatus::Running, None)
            .await
            .unwrap()
    );
    add_step(&pool, running, StepKind::Frame, "framing")
        .await
        .unwrap();
    backdate(&pool, running, "heartbeat_at", 120).await;
    let (u2, queued) = reserved_run(&pool, 4000).await;
    backdate(&pool, queued, "created_at", 120).await;
    // A finished run whose worker never released (e.g. a failed release),
    // finished long enough ago that no late call can still be settling.
    let (u3, leaked) = reserved_run(&pool, 4000).await;
    assert!(
        set_status(&pool, leaked, RunStatus::Done, None)
            .await
            .unwrap()
    );
    backdate(&pool, leaked, "finished_at", 15 * 60).await;
    // A live run (another instance's): fresh heartbeat, left alone.
    let (u4, live) = reserved_run(&pool, 4000).await;
    assert!(
        set_status(&pool, live, RunStatus::Running, None)
            .await
            .unwrap()
    );
    backdate(&pool, live, "created_at", 600).await;

    let n = recover_stale_runs(&pool).await.unwrap();
    assert!(n >= 2, "{n}");

    for (uid, run) in [(u1, running), (u2, queued)] {
        poll_status(&pool, uid, run, RunStatus::Interrupted).await;
        assert_eq!(error_kind(&pool, run).await.as_deref(), Some("interrupted"));
        assert_eq!(reservation(&pool, run).await, None);
        let ev = run_events(&pool, run).await;
        let finished: Vec<_> = ev.iter().filter(|(e, _)| e == "run_finished").collect();
        assert_eq!(finished.len(), 1, "{ev:?}");
        assert_eq!(finished[0].1["status"], "interrupted");
        assert_eq!(finished[0].1["cost_paise"], 0);
    }
    assert_eq!(run_events(&pool, running).await[0].1["steps"], 1);
    assert_eq!(reservation(&pool, leaked).await, None);
    let v = get_run_for_user(&pool, u3, leaked).await.unwrap().unwrap();
    assert_eq!(v.summary.status, RunStatus::Done);
    let v = get_run_for_user(&pool, u4, live).await.unwrap().unwrap();
    assert_eq!(v.summary.status, RunStatus::Running);
    assert_eq!(reservation(&pool, live).await, Some(4000));
    // Leave nothing active behind.
    assert!(
        set_status(&pool, live, RunStatus::Failed, None)
            .await
            .unwrap()
    );
    release(&pool, u4, live).await.unwrap();
}

#[tokio::test]
async fn fresh_running_run_with_live_heartbeat_is_not_interrupted() {
    let Some((pool, _guard)) = test_pool_exclusive().await else {
        return;
    };
    let (uid, run) = reserved_run(&pool, 4000).await;
    assert!(
        set_status(&pool, run, RunStatus::Running, None)
            .await
            .unwrap()
    );
    // Old run, but its worker is alive (heartbeat 10 s ago).
    backdate(&pool, run, "created_at", 3600).await;
    backdate(&pool, run, "heartbeat_at", 10).await;
    let marked = interrupt_stale_runs(&pool).await.unwrap();
    assert!(!marked.iter().any(|(id, _)| *id == run));
    // A heartbeat keeps it fresh.
    assert!(heartbeat(&pool, run).await.unwrap());
    let v = get_run_for_user(&pool, uid, run).await.unwrap().unwrap();
    assert_eq!(v.summary.status, RunStatus::Running);
    assert_eq!(reservation(&pool, run).await, Some(4000));
    assert!(
        set_status(&pool, run, RunStatus::Failed, None)
            .await
            .unwrap()
    );
    release(&pool, uid, run).await.unwrap();
}

#[tokio::test]
async fn stale_heartbeat_run_is_interrupted_and_released() {
    let Some((pool, _guard)) = test_pool_exclusive().await else {
        return;
    };
    let (uid, run) = reserved_run(&pool, 4000).await;
    assert!(
        set_status(&pool, run, RunStatus::Running, None)
            .await
            .unwrap()
    );
    backdate(&pool, run, "heartbeat_at", 91).await;
    recover_stale_runs(&pool).await.unwrap();
    let v = get_run_for_user(&pool, uid, run).await.unwrap().unwrap();
    assert_eq!(v.summary.status, RunStatus::Interrupted);
    assert_eq!(reservation(&pool, run).await, None);
    let ev = run_events(&pool, run).await;
    assert_eq!(ev.len(), 1, "{ev:?}");
    assert_eq!(ev[0].1["status"], "interrupted");
    // A heartbeat for an interrupted run changes nothing.
    assert!(!heartbeat(&pool, run).await.unwrap());
    // A second sweep finds nothing more to do for it.
    let again = interrupt_stale_runs(&pool).await.unwrap();
    assert!(!again.iter().any(|(id, _)| *id == run));
}

#[tokio::test]
async fn queued_without_heartbeat_older_than_90s_is_interrupted() {
    let Some((pool, _guard)) = test_pool_exclusive().await else {
        return;
    };
    let (u_old, old) = reserved_run(&pool, 4000).await;
    backdate(&pool, old, "created_at", 91).await;
    let (u_new, new) = reserved_run(&pool, 4000).await;
    backdate(&pool, new, "created_at", 60).await;
    let marked = interrupt_stale_runs(&pool).await.unwrap();
    assert!(marked.contains(&(old, u_old)));
    assert!(!marked.iter().any(|(id, _)| *id == new));
    let v = get_run_for_user(&pool, u_new, new).await.unwrap().unwrap();
    assert_eq!(v.summary.status, RunStatus::Queued);
    // Tidy: the caller of interrupt_stale_runs releases; do it here for both.
    release(&pool, u_old, old).await.unwrap();
    assert!(
        set_status(&pool, new, RunStatus::Failed, None)
            .await
            .unwrap()
    );
    release(&pool, u_new, new).await.unwrap();
}

#[tokio::test]
async fn self_heal_replaces_stale_active_run_on_next_send() {
    // Self-heal sweeps globally, like recovery.
    let Some((pool, _guard)) = test_pool_exclusive().await else {
        return;
    };
    let st = app_state(pool.clone());
    let user = new_user_with_cap(&pool, 100_000).await;
    // A run stuck queued (its request died) for 2 minutes, holding a reservation.
    let stuck = create_run(&pool, user.id, "q", RunMode::Focused, 4000)
        .await
        .unwrap()
        .unwrap();
    let c = reserve(&pool, &user, &big_global(), 4000, stuck, "run")
        .await
        .unwrap();
    assert!(matches!(c, CapCheck::Ok { .. }));
    backdate(&pool, stuck, "created_at", 120).await;

    let run = started(run_ask(&st, &user, RULE_QUESTION, RunMode::Focused).await);
    assert_ne!(run, stuck);
    wait_idle(&st, run).await;
    poll_status(&pool, user.id, run, RunStatus::Done).await;

    let v = get_run_for_user(&pool, user.id, stuck)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(v.summary.status, RunStatus::Interrupted);
    assert_eq!(reservation(&pool, stuck).await, None);
    let ev = run_events(&pool, stuck).await;
    assert_eq!(ev.len(), 1, "{ev:?}");
    assert_eq!(ev[0].1["status"], "interrupted");
}

/// Run `body` with a per-test trigger on `table` (dropped afterwards, even if
/// `body` panics). `when` is the trigger's condition, `act` its plpgsql action.
async fn with_trigger<F, T>(pool: &PgPool, table: &str, when: &str, act: &str, body: F) -> T
where
    F: std::future::Future<Output = T> + Send + 'static,
    T: Send + 'static,
{
    let tag = Uuid::new_v4().simple().to_string();
    let func = format!("incisive.test_trig_{tag}");
    let trig = format!("test_trig_{tag}");
    ddl(
        pool,
        &format!(
            "CREATE FUNCTION {func}() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN \
               IF {when} THEN {act} END IF; RETURN NEW; END $$"
        ),
    )
    .await;
    ddl(
        pool,
        &format!(
            "CREATE TRIGGER {trig} BEFORE INSERT OR UPDATE ON {table} \
             FOR EACH ROW EXECUTE FUNCTION {func}()"
        ),
    )
    .await;

    // In its own task, so a panic is caught and the trigger still dropped.
    let res = tokio::spawn(body).await;

    ddl(pool, &format!("DROP TRIGGER {trig} ON {table}")).await;
    ddl(pool, &format!("DROP FUNCTION {func}()")).await;
    match res {
        Ok(v) => v,
        Err(e) => std::panic::resume_unwind(e.into_panic()),
    }
}

/// Run one DDL statement in its own transaction, never waiting long for a
/// table lock. A lock timeout (SQLSTATE 55P03) means a parallel test is using
/// the table right now: back off and retry instead of failing the test.
async fn ddl(pool: &PgPool, sql: &str) {
    for attempt in 1..=40u64 {
        let mut tx = pool.begin().await.unwrap();
        sqlx::query("SET LOCAL lock_timeout = '2s'")
            .execute(&mut *tx)
            .await
            .unwrap();
        match sqlx::query(sql).execute(&mut *tx).await {
            Ok(_) => {
                tx.commit().await.unwrap();
                return;
            }
            Err(sqlx::Error::Database(e)) if e.code().as_deref() == Some("55P03") => {
                drop(tx);
                tokio::time::sleep(std::time::Duration::from_millis(100 * attempt.min(10))).await;
            }
            Err(e) => panic!("ddl failed: {sql}: {e}"),
        }
    }
    panic!("ddl still blocked by a lock after 40 attempts: {sql}");
}

#[tokio::test]
async fn start_run_completes_even_if_request_future_is_dropped() {
    let Some((pool, _guard)) = test_pool().await else {
        return;
    };
    let st = app_state(pool.clone());
    let user = new_user_with_cap(&pool, 100_000).await;
    // Slow down this user's run insert, so the request can be dropped mid-start.
    let when = format!("TG_OP = 'INSERT' AND NEW.user_id = {}", user.id);
    let (st2, u2) = (st.clone(), user.clone());
    with_trigger(
        &pool,
        "incisive.runs",
        &when,
        "PERFORM pg_sleep(0.5);",
        async move {
            let request = tokio::spawn(async move {
                let _ = run_ask(&st2, &u2, RULE_QUESTION, RunMode::Focused).await;
            });
            tokio::time::sleep(Duration::from_millis(200)).await;
            // The client goes away: axum drops the request future.
            request.abort();
            let _ = request.await;
            // Let the start finish (or undo itself) after the drop.
            tokio::time::sleep(Duration::from_millis(600)).await;
        },
    )
    .await;

    // Either the run started and finished, or nothing was left behind.
    let runs = list_runs(&pool, user.id, 10).await.unwrap();
    assert!(runs.len() <= 1, "{runs:?}");
    if let Some(r) = runs.first() {
        wait_idle(&st, r.id).await;
        poll_status(&pool, user.id, r.id, RunStatus::Done).await;
        assert_eq!(reservation(&pool, r.id).await, None);
        let ev = run_events(&pool, r.id).await;
        assert_eq!(ev.last().unwrap().1["status"], "done");
    }
    let reservations: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM incisive.spend WHERE user_id = $1 AND kind = 'reservation'",
    )
    .bind(user.id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(reservations, 0);
    assert_eq!(runs.len(), 1, "with the start detached, the run goes ahead");
}

#[tokio::test]
async fn save_answer_refused_when_not_running() {
    let Some((pool, _guard)) = test_pool().await else {
        return;
    };
    let u = new_user(&pool).await;
    let id = create_run(&pool, u, "q", RunMode::Quick, 500)
        .await
        .unwrap()
        .unwrap();
    assert!(!save_answer(&pool, id, &json!({"a": 1})).await.unwrap());
    assert!(
        set_status(&pool, id, RunStatus::Running, None)
            .await
            .unwrap()
    );
    assert!(save_answer(&pool, id, &json!({"a": 2})).await.unwrap());
    assert!(
        set_status(&pool, id, RunStatus::Interrupted, Some("interrupted"))
            .await
            .unwrap()
    );
    assert!(!save_answer(&pool, id, &json!({"a": 3})).await.unwrap());
    let v = get_run_for_user(&pool, u, id).await.unwrap().unwrap();
    assert_eq!(v.answer, Some(json!({"a": 2})));
    assert!(delete_run(&pool, u, id).await.unwrap());
    assert!(!save_answer(&pool, id, &json!({"a": 4})).await.unwrap());
    assert!(
        !save_answer(&pool, Uuid::new_v4(), &json!({}))
            .await
            .unwrap()
    );
}

/// Writes a step, then the run is interrupted (as another instance's recovery
/// would); the pipeline's next heartbeat finds it gone and stops.
fn interrupted_mid_run(ctx: &RunContext) -> PipelineFuture<'_> {
    Box::pin(async move {
        let pool = &ctx.state.pool;
        add_step(pool, ctx.run_id, StepKind::Frame, "framing")
            .await
            .unwrap();
        let h = ctx.budget.try_hold(100).unwrap();
        ctx.budget
            .charge(pool, h, Uuid::new_v4(), "m-test", &usage(1000, 100))
            .await
            .unwrap();
        assert!(
            set_status(
                pool,
                ctx.run_id,
                RunStatus::Interrupted,
                Some("interrupted")
            )
            .await
            .unwrap()
        );
        if !incisive_app::runs::worker::heartbeat(ctx).await {
            return PipelineOutcome::Failed("interrupted");
        }
        PipelineOutcome::Done(json!({"should": "not happen"}))
    })
}

#[tokio::test]
async fn interrupted_run_stops_without_a_second_audit() {
    let Some((pool, _guard)) = test_pool().await else {
        return;
    };
    let st = app_state(pool.clone());
    let (uid, run) = reserved_run(&pool, 4000).await;
    st.worker.spawn_with(st.clone(), run, interrupted_mid_run);
    wait_idle(&st, run).await;
    let v = get_run_for_user(&pool, uid, run).await.unwrap().unwrap();
    assert_eq!(v.summary.status, RunStatus::Interrupted);
    assert!(v.answer.is_none());
    assert_eq!(reservation(&pool, run).await, None);
    assert_eq!(charges(&pool, uid).await.len(), 1);
    // Only the charge; whoever interrupted it audits run_finished, not the worker.
    let ev: Vec<String> = run_events(&pool, run)
        .await
        .into_iter()
        .map(|(e, _)| e)
        .collect();
    assert_eq!(ev, ["charged"]);
}

#[tokio::test]
async fn final_status_write_failure_audits_store_error() {
    let Some((pool, _guard)) = test_pool().await else {
        return;
    };
    let st = app_state(pool.clone());
    let (uid, run) = reserved_run(&pool, 4000).await;
    // Every attempt to mark this run done fails.
    let when = format!("TG_OP = 'UPDATE' AND NEW.id = '{run}' AND NEW.status = 'done'");
    let st2 = st.clone();
    with_trigger(
        &pool,
        "incisive.runs",
        &when,
        "RAISE EXCEPTION 'injected status failure';",
        async move {
            st2.worker.spawn(st2.clone(), run);
            wait_idle(&st2, run).await;
        },
    )
    .await;
    assert_eq!(reservation(&pool, run).await, None);
    let ev = run_events(&pool, run).await;
    let last = ev.last().unwrap();
    assert_eq!(last.0, "run_finished");
    assert_eq!(last.1["status"], "store_error");
    // The row is left running; self-heal / recovery clear it once its heartbeat is stale.
    let v = get_run_for_user(&pool, uid, run).await.unwrap().unwrap();
    assert_eq!(v.summary.status, RunStatus::Running);
    assert!(
        set_status(&pool, run, RunStatus::Failed, None)
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn worker_finishes_with_no_client_connected() {
    let Some((pool, _guard)) = test_pool().await else {
        return;
    };
    let user = new_user_with_cap(&pool, 100_000).await;
    let worker = {
        // Everything request-scoped is dropped at the end of this block.
        let st = app_state(pool.clone());
        let run = started(run_ask(&st, &user, RULE_QUESTION, RunMode::Focused).await);
        (st.worker.clone(), run)
    };
    let (handle, run) = worker;
    poll_status(&pool, user.id, run, RunStatus::Done).await;
    for _ in 0..500 {
        if !handle.is_running(run) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(!handle.is_running(run));
    assert_eq!(reservation(&pool, run).await, None);
}

fn panicking(_ctx: &RunContext) -> PipelineFuture<'_> {
    Box::pin(async { panic!("{}", MARKER) })
}

#[tokio::test]
async fn worker_panic_marks_run_failed_and_releases() {
    let Some((pool, _guard)) = test_pool().await else {
        return;
    };
    let st = app_state(pool.clone());
    let (uid, run) = reserved_run(&pool, 4000).await;
    st.worker.spawn_with(st.clone(), run, panicking);
    wait_idle(&st, run).await;
    poll_status(&pool, uid, run, RunStatus::Failed).await;
    assert_eq!(error_kind(&pool, run).await.as_deref(), Some("panic"));
    assert_eq!(reservation(&pool, run).await, None);
    let ev = run_events(&pool, run).await;
    assert_eq!(ev.last().unwrap().0, "run_finished");
    assert_eq!(ev.last().unwrap().1["status"], "failed");
    // The worker survives: the next run on the same handle completes.
    let user = full_user(&pool, uid).await;
    let next = started(run_ask(&st, &user, RULE_QUESTION, RunMode::Quick).await);
    wait_idle(&st, next).await;
    poll_status(&pool, uid, next, RunStatus::Done).await;
}

/// Charges one call, deletes its own run (as the user would), then tries to write.
fn deletes_itself(ctx: &RunContext) -> PipelineFuture<'_> {
    Box::pin(async move {
        let pool = &ctx.state.pool;
        add_step(pool, ctx.run_id, StepKind::Frame, "framing")
            .await
            .unwrap();
        let h = ctx.budget.try_hold(100).unwrap();
        ctx.budget
            .charge(pool, h, Uuid::new_v4(), "m-test", &usage(1000, 100))
            .await
            .unwrap();
        assert!(delete_run(pool, ctx.user.id, ctx.run_id).await.unwrap());
        match add_step(pool, ctx.run_id, StepKind::Search, "searching").await {
            Ok(_) => PipelineOutcome::Done(json!({"should": "not happen"})),
            Err(_) => PipelineOutcome::Failed("store"),
        }
    })
}

#[tokio::test]
async fn run_deleted_mid_run_counts_as_cancelled() {
    let Some((pool, _guard)) = test_pool().await else {
        return;
    };
    let st = app_state(pool.clone());
    let (uid, run) = reserved_run(&pool, 4000).await;
    st.worker.spawn_with(st.clone(), run, deletes_itself);
    wait_idle(&st, run).await;

    assert!(get_run_for_user(&pool, uid, run).await.unwrap().is_none());
    assert_eq!(count(&pool, "run_answers", run).await, 0);
    // The charge stands; the rest of the reservation is released.
    let c = charges(&pool, uid).await;
    assert_eq!(c.len(), 1);
    assert!(c[0].0 > 0);
    assert_eq!(reservation(&pool, run).await, None);
    let ev = run_events(&pool, run).await;
    let last = ev.last().unwrap();
    assert_eq!(last.0, "run_finished");
    assert_eq!(last.1["status"], "deleted");
    assert_eq!(last.1["cost_paise"], c[0].0);
}

/// Writes one step, then waits (up to 5 s) to be cancelled.
fn waits_for_cancel(ctx: &RunContext) -> PipelineFuture<'_> {
    Box::pin(async move {
        add_step(&ctx.state.pool, ctx.run_id, StepKind::Frame, "framing")
            .await
            .unwrap();
        for _ in 0..500 {
            if ctx.cancel.is_cancelled() {
                return PipelineOutcome::Failed("cancelled");
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        PipelineOutcome::Done(json!({"not": "cancelled"}))
    })
}

#[tokio::test]
async fn cancel_stops_the_worker_and_releases() {
    let Some((pool, _guard)) = test_pool().await else {
        return;
    };
    let st = app_state(pool.clone());
    let (uid, run) = reserved_run(&pool, 4000).await;
    assert!(!st.worker.cancel(run), "nothing to cancel yet");
    st.worker.spawn_with(st.clone(), run, waits_for_cancel);
    for _ in 0..500 {
        if count(&pool, "run_steps", run).await == 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(st.worker.cancel(run));
    wait_idle(&st, run).await;

    assert_eq!(count(&pool, "run_answers", run).await, 0);
    assert_eq!(reservation(&pool, run).await, None);
    let v = get_run_for_user(&pool, uid, run).await.unwrap().unwrap();
    assert_eq!(v.summary.status, RunStatus::Failed);
    assert_eq!(error_kind(&pool, run).await.as_deref(), Some("cancelled"));
    let ev = run_events(&pool, run).await;
    assert_eq!(ev.last().unwrap().1["status"], "deleted");
}

#[tokio::test]
async fn already_finished_run_is_left_alone() {
    let Some((pool, _guard)) = test_pool().await else {
        return;
    };
    let st = app_state(pool.clone());
    let (uid, run) = reserved_run(&pool, 4000).await;
    // Interrupted by recovery before the worker got to it.
    assert!(
        set_status(&pool, run, RunStatus::Interrupted, Some("interrupted"))
            .await
            .unwrap()
    );
    st.worker.spawn(st.clone(), run);
    wait_idle(&st, run).await;
    let v = get_run_for_user(&pool, uid, run).await.unwrap().unwrap();
    assert_eq!(v.summary.status, RunStatus::Interrupted);
    assert!(v.steps.is_empty());
    assert_eq!(reservation(&pool, run).await, None);
    assert!(run_events(&pool, run).await.is_empty());
}

// ---------- run API, My questions and deletion ----------

struct AlwaysUp;

#[async_trait::async_trait]
impl incisive_app::health::Readiness for AlwaysUp {
    async fn db_ok(&self) -> bool {
        true
    }
}

fn router(st: AppState) -> Router {
    let opts = LeptosOptions::builder().output_name("incisive-app").build();
    incisive_app::server::build_router(Arc::new(AlwaysUp), st, opts)
}

async fn email_of(pool: &PgPool, uid: i64) -> String {
    sqlx::query_scalar("SELECT email FROM incisive.users WHERE id = $1")
        .bind(uid)
        .fetch_one(pool)
        .await
        .unwrap()
}

/// Send `req` and return the status and the raw body.
async fn send(app: Router, req: Request<Body>) -> (StatusCode, String) {
    let res = app.oneshot(req).await.unwrap();
    let status = res.status();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

fn as_json(body: &str) -> serde_json::Value {
    serde_json::from_str(body).unwrap_or(serde_json::Value::Null)
}

fn api(method: &str, path: &str, email: &str) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(path)
        .header("x-dev-user-email", email)
        .body(Body::empty())
        .unwrap()
}

/// A finished run for `uid` (so the user can start another), created `age_s`
/// seconds ago.
async fn finished_run(pool: &PgPool, uid: i64, question: &str, age_s: i64) -> Uuid {
    let id = create_run(pool, uid, question, RunMode::Quick, 500)
        .await
        .unwrap()
        .unwrap();
    assert!(
        set_status(pool, id, RunStatus::Done, None).await.unwrap(),
        "finish"
    );
    backdate(pool, id, "created_at", age_s).await;
    id
}

#[tokio::test]
async fn run_endpoints_are_owner_scoped() {
    let Some((pool, _guard)) = test_pool().await else {
        return;
    };
    let st = app_state(pool.clone());
    let (a, run) = reserved_run(&pool, 4000).await;
    let (b, _other) = reserved_run(&pool, 4000).await;
    let (ea, eb) = (email_of(&pool, a).await, email_of(&pool, b).await);
    let path = format!("/api/runs/{run}");

    // B sees A's run exactly like a missing one, on read and on delete.
    let (s, body) = send(router(st.clone()), api("GET", &path, &eb)).await;
    assert_eq!(s, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(as_json(&body)["error"], "not_found");
    assert!(!body.contains(MARKER));
    let (s, _) = send(router(st.clone()), api("DELETE", &path, &eb)).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    assert!(get_run_for_user(&pool, a, run).await.unwrap().is_some());
    assert!(reservation(&pool, run).await.is_some());
    // A malformed id is the same 404.
    let (s, _) = send(router(st.clone()), api("GET", "/api/runs/not-a-uuid", &ea)).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    // B's list never contains A's run.
    let (s, body) = send(router(st.clone()), api("GET", "/api/runs", &eb)).await;
    assert_eq!(s, StatusCode::OK);
    assert!(!body.contains(&run.to_string()));

    // The owner reads, then deletes, then it is gone for the owner too.
    let (s, body) = send(router(st.clone()), api("GET", &path, &ea)).await;
    assert_eq!(s, StatusCode::OK, "{body}");
    let v = as_json(&body);
    assert_eq!(v["summary"]["id"], run.to_string());
    assert_eq!(v["summary"]["status"], "queued");
    let (s, body) = send(router(st.clone()), api("DELETE", &path, &ea)).await;
    assert_eq!(s, StatusCode::NO_CONTENT, "{body}");
    assert!(body.is_empty());
    let (s, _) = send(router(st.clone()), api("GET", &path, &ea)).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    let (s, _) = send(router(st.clone()), api("DELETE", &path, &ea)).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    // Unauthenticated callers (no dev header in a prod-style state) get nothing.
    let mut prod = app_state(pool.clone());
    prod.dev_mode = false;
    let (s, _) = send(router(prod), api("GET", &path, &ea)).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);

    // Audited once, with the id only.
    let ev = run_events(&pool, run).await;
    let deleted: Vec<_> = ev.iter().filter(|e| e.0 == "run_deleted").collect();
    assert_eq!(deleted.len(), 1, "{ev:?}");
    assert_eq!(deleted[0].1, json!({"run_id": run.to_string()}));
}

/// Writes one step, charges one call, then runs (up to 5 s) until cancelled.
fn slow_charging(ctx: &RunContext) -> PipelineFuture<'_> {
    Box::pin(async move {
        let pool = &ctx.state.pool;
        add_step(pool, ctx.run_id, StepKind::Frame, "framing")
            .await
            .unwrap();
        let h = ctx.budget.try_hold(100).unwrap();
        ctx.budget
            .charge(pool, h, Uuid::new_v4(), "m-test", &usage(1000, 100))
            .await
            .unwrap();
        for _ in 0..500 {
            if ctx.cancel.is_cancelled() {
                return PipelineOutcome::Failed("cancelled");
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        PipelineOutcome::Done(json!({"not": "cancelled"}))
    })
}

#[tokio::test]
async fn delete_while_running_cancels_and_releases() {
    let Some((pool, _guard)) = test_pool().await else {
        return;
    };
    let st = app_state(pool.clone());
    let (uid, run) = reserved_run(&pool, 4000).await;
    let email = email_of(&pool, uid).await;
    st.worker.spawn_with(st.clone(), run, slow_charging);
    // Genuinely running: a step and a charge exist, the worker is live.
    for _ in 0..500 {
        if count(&pool, "run_steps", run).await == 1 && charges(&pool, uid).await.len() == 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(st.worker.is_running(run));
    assert_eq!(charges(&pool, uid).await.len(), 1);
    let charge_before = charges(&pool, uid).await;

    let (s, body) = send(
        router(st.clone()),
        api("DELETE", &format!("/api/runs/{run}"), &email),
    )
    .await;
    assert_eq!(s, StatusCode::NO_CONTENT, "{body}");

    // The worker stops (is_running turns false within 5 s).
    wait_idle(&st, run).await;
    assert!(!st.worker.is_running(run));
    // No answer row, and the run is gone.
    assert_eq!(count(&pool, "run_answers", run).await, 0);
    assert_eq!(count(&pool, "run_steps", run).await, 0);
    assert!(get_run_for_user(&pool, uid, run).await.unwrap().is_none());
    // Charges stand; the reservation is released.
    assert_eq!(charges(&pool, uid).await, charge_before);
    assert!(charge_before[0].0 > 0);
    assert_eq!(reservation(&pool, run).await, None);
    // Audited as deleted (ids only).
    let ev = run_events(&pool, run).await;
    assert!(
        ev.iter()
            .any(|e| e.0 == "run_deleted" && e.1 == json!({"run_id": run.to_string()})),
        "{ev:?}"
    );
}

#[tokio::test]
async fn delete_all_returns_count() {
    let Some((pool, _guard)) = test_pool().await else {
        return;
    };
    let st = app_state(pool.clone());
    let uid = new_user(&pool).await;
    let other = new_user(&pool).await;
    let email = email_of(&pool, uid).await;
    for i in 0..3 {
        finished_run(&pool, uid, &format!("q{i}"), 10 + i).await;
    }
    let theirs = finished_run(&pool, other, "theirs", 5).await;

    let (s, body) = send(router(st.clone()), api("DELETE", "/api/runs", &email)).await;
    assert_eq!(s, StatusCode::OK, "{body}");
    assert_eq!(as_json(&body), json!({"deleted": 3}));
    assert!(list_runs(&pool, uid, 10).await.unwrap().is_empty());
    assert!(
        get_run_for_user(&pool, other, theirs)
            .await
            .unwrap()
            .is_some()
    );
    // Nothing left: zero, not an error.
    let (s, body) = send(router(st.clone()), api("DELETE", "/api/runs", &email)).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(as_json(&body), json!({"deleted": 0}));
}

#[tokio::test]
async fn delete_all_cancels_and_releases_an_active_run() {
    let Some((pool, _guard)) = test_pool().await else {
        return;
    };
    let st = app_state(pool.clone());
    let (uid, run) = reserved_run(&pool, 4000).await;
    let email = email_of(&pool, uid).await;
    st.worker.spawn_with(st.clone(), run, slow_charging);
    for _ in 0..500 {
        if charges(&pool, uid).await.len() == 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let (s, body) = send(router(st.clone()), api("DELETE", "/api/runs", &email)).await;
    assert_eq!(s, StatusCode::OK, "{body}");
    assert_eq!(as_json(&body), json!({"deleted": 1}));
    wait_idle(&st, run).await;
    assert_eq!(count(&pool, "run_answers", run).await, 0);
    assert_eq!(charges(&pool, uid).await.len(), 1);
    assert_eq!(reservation(&pool, run).await, None);
}

#[tokio::test]
async fn list_is_newest_first_and_clamped() {
    let Some((pool, _guard)) = test_pool().await else {
        return;
    };
    let st = app_state(pool.clone());
    let uid = new_user(&pool).await;
    let email = email_of(&pool, uid).await;
    let oldest = finished_run(&pool, uid, "oldest", 300).await;
    let middle = finished_run(&pool, uid, "middle", 200).await;
    let newest = finished_run(&pool, uid, "newest", 100).await;

    let ids = |body: &str| -> Vec<String> {
        as_json(body)
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["id"].as_str().unwrap().to_string())
            .collect()
    };
    let (s, body) = send(router(st.clone()), api("GET", "/api/runs", &email)).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(
        ids(&body),
        [newest.to_string(), middle.to_string(), oldest.to_string()]
    );
    let first = &as_json(&body)[0];
    assert_eq!(first["title"], "newest");
    assert_eq!(first["status"], "done");
    assert_eq!(first["mode"], "quick");

    let (_, body) = send(router(st.clone()), api("GET", "/api/runs?limit=2", &email)).await;
    assert_eq!(ids(&body), [newest.to_string(), middle.to_string()]);
    // Below 1 is raised to 1; above 100 is lowered (3 rows here either way).
    let (_, body) = send(router(st.clone()), api("GET", "/api/runs?limit=0", &email)).await;
    assert_eq!(ids(&body), [newest.to_string()]);
    let (_, body) = send(router(st.clone()), api("GET", "/api/runs?limit=-5", &email)).await;
    assert_eq!(ids(&body), [newest.to_string()]);
    let (_, body) = send(
        router(st.clone()),
        api("GET", "/api/runs?limit=1000", &email),
    )
    .await;
    assert_eq!(ids(&body).len(), 3);

    use incisive_app::runs::api::clamp_limit;
    assert_eq!(clamp_limit(None), 20);
    assert_eq!(clamp_limit(Some(0)), 1);
    assert_eq!(clamp_limit(Some(-3)), 1);
    assert_eq!(clamp_limit(Some(100)), 100);
    assert_eq!(clamp_limit(Some(101)), 100);
    assert_eq!(clamp_limit(Some(i64::MAX)), 100);
}

#[tokio::test]
async fn poll_shows_steps_in_order() {
    let Some((pool, _guard)) = test_pool().await else {
        return;
    };
    let st = app_state(pool.clone());
    let (uid, run) = reserved_run(&pool, 4000).await;
    let email = email_of(&pool, uid).await;
    let path = format!("/api/runs/{run}");
    assert!(
        set_status(&pool, run, RunStatus::Running, None)
            .await
            .unwrap()
    );
    for (kind, line) in [
        (StepKind::Frame, "one"),
        (StepKind::Search, "two"),
        (StepKind::Screen, "three"),
    ] {
        add_step(&pool, run, kind, line).await.unwrap();
    }
    let (s, body) = send(router(st.clone()), api("GET", &path, &email)).await;
    assert_eq!(s, StatusCode::OK);
    let v = as_json(&body);
    assert_eq!(v["summary"]["status"], "running");
    assert_eq!(v["answer"], serde_json::Value::Null);
    let lines: Vec<_> = v["steps"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| (x["seq"].as_i64().unwrap(), x["line"].as_str().unwrap()))
        .collect();
    assert_eq!(lines, [(1, "one"), (2, "two"), (3, "three")]);

    // A later poll sees the new line after the old ones, and the answer.
    add_step(&pool, run, StepKind::Write, "four").await.unwrap();
    assert!(save_answer(&pool, run, &json!({"a": 1})).await.unwrap());
    let (_, body) = send(router(st.clone()), api("GET", &path, &email)).await;
    let v = as_json(&body);
    assert_eq!(v["steps"].as_array().unwrap().len(), 4);
    assert_eq!(v["steps"][3]["line"], "four");
    assert_eq!(v["steps"][3]["kind"], "write");
    assert_eq!(v["answer"], json!({"a": 1}));
}

fn cross_site(method: &str, path: &str, origin: &str) -> Request<Body> {
    let mut b = Request::builder()
        .method(method)
        .uri(path)
        .header("host", "incisive.test:8080")
        .header("origin", origin)
        .header("x-dev-user-email", "x@example.com");
    if method == "POST" {
        b = b.header("content-type", "application/x-www-form-urlencoded");
    }
    b.body(Body::empty()).unwrap()
}

#[tokio::test]
async fn fn_routes_reject_cross_site_origin() {
    // Unreachable database: a 403 proves nothing past the origin check ran.
    let unreachable = || {
        let pool = lazy_pool(&SecretString::from(
            "postgres://u:p@127.0.0.1:1/none".to_string(),
        ))
        .unwrap();
        app_state(pool)
    };
    let run = Uuid::new_v4();
    let cases: Vec<(&str, String)> = vec![
        ("POST", "/api/fn/my_runs".into()),
        ("POST", "/api/fn/my_run".into()),
        ("POST", "/api/fn/delete_my_run".into()),
        ("POST", "/api/fn/delete_my_runs".into()),
        ("DELETE", format!("/api/runs/{run}")),
        ("DELETE", "/api/runs".into()),
    ];
    for (method, path) in &cases {
        for origin in [
            "https://evil.example",
            "http://incisive.test:9999",
            "http://incisive.test",
            "null",
            "not a url",
        ] {
            let (s, body) = send(router(unreachable()), cross_site(method, path, origin)).await;
            assert_eq!(s, StatusCode::FORBIDDEN, "{method} {path} {origin}");
            assert_eq!(
                as_json(&body),
                json!({"error": "cross_site", "message": "Request blocked: cross-site request."}),
                "{method} {path} {origin}"
            );
        }
    }
}

#[tokio::test]
async fn server_functions_list_get_and_delete() {
    let Some((pool, _guard)) = test_pool().await else {
        return;
    };
    let st = app_state(pool.clone());
    let uid = new_user(&pool).await;
    let other = new_user(&pool).await;
    let email = email_of(&pool, uid).await;
    let mine = finished_run(&pool, uid, "mine", 10).await;
    let theirs = finished_run(&pool, other, "theirs", 10).await;
    let post = |path: &str, body: String| {
        Request::post(path)
            .header("content-type", "application/x-www-form-urlencoded")
            .header("x-dev-user-email", &email)
            .body(Body::from(body))
            .unwrap()
    };

    let (s, body) = send(router(st.clone()), post("/api/fn/my_runs", String::new())).await;
    assert_eq!(s, StatusCode::OK, "{body}");
    assert_eq!(as_json(&body).as_array().unwrap().len(), 1);
    let (s, body) = send(
        router(st.clone()),
        post("/api/fn/my_run", format!("run_id={mine}")),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{body}");
    assert_eq!(as_json(&body)["summary"]["id"], mine.to_string());
    // Someone else's run is a 404 here too.
    let (s, _) = send(
        router(st.clone()),
        post("/api/fn/my_run", format!("run_id={theirs}")),
    )
    .await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    let (s, _) = send(
        router(st.clone()),
        post("/api/fn/delete_my_run", format!("run_id={theirs}")),
    )
    .await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    assert!(
        get_run_for_user(&pool, other, theirs)
            .await
            .unwrap()
            .is_some()
    );

    let (s, body) = send(
        router(st.clone()),
        post("/api/fn/delete_my_runs", String::new()),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{body}");
    assert_eq!(as_json(&body), json!(1));
    assert!(list_runs(&pool, uid, 10).await.unwrap().is_empty());
}

/// Building a router while another request renders must not stall that
/// render. (Leptos route generation sets a process-global "suppress resource
/// loads" flag; a resource first polled while it was set never loaded, which
/// hung this binary when tests built routers concurrently.)
#[tokio::test]
async fn ssr_render_survives_concurrent_router_builds() {
    let Some((pool, _guard)) = test_pool().await else {
        return;
    };
    let st = app_state(pool.clone());
    let uid = new_user(&pool).await;
    let email = email_of(&pool, uid).await;
    finished_run(&pool, uid, "concurrent build", 10).await;
    let app = router(st.clone());

    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let builder = {
        let (stop, st) = (stop.clone(), st.clone());
        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            let _rt = rt.enter();
            while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                drop(router(st.clone()));
            }
        })
    };
    let mut outcome = Ok(());
    for i in 0..20 {
        let req = send(app.clone(), api("GET", "/history", &email));
        match tokio::time::timeout(Duration::from_secs(10), req).await {
            Ok((s, body)) if s == StatusCode::OK && body.contains("concurrent build") => {}
            Ok((s, _)) => {
                outcome = Err(format!("render {i}: status {s}"));
                break;
            }
            Err(_) => {
                outcome = Err(format!("render {i} stalled while routers were being built"));
                break;
            }
        }
    }
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    builder.join().unwrap();
    outcome.unwrap();
}

#[tokio::test]
async fn ssr_history_page_renders() {
    let Some((pool, _guard)) = test_pool().await else {
        return;
    };
    let st = app_state(pool.clone());
    let uid = new_user(&pool).await;
    let email = email_of(&pool, uid).await;
    // Text with markup: it must be escaped, never rendered as HTML.
    finished_run(&pool, uid, "ICG <b>leak</b> evidence?", 10).await;
    let (s, body) = send(router(st.clone()), api("GET", "/history", &email)).await;
    assert_eq!(s, StatusCode::OK);
    assert!(body.contains("My questions"), "{body}");
    assert!(body.contains("Delete all my history"), "{body}");
    assert!(
        body.contains("ICG &lt;b&gt;leak&lt;/b&gt; evidence?"),
        "{body}"
    );
    assert!(!body.contains("<b>leak</b>"));

    // The run page renders too (nothing sensitive is in the shell for others).
    let run = list_runs(&pool, uid, 1).await.unwrap()[0].id;
    let (s, body) = send(
        router(st.clone()),
        api("GET", &format!("/runs/{run}"), &email),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert!(
        body.contains("ICG &lt;b&gt;leak&lt;/b&gt; evidence?"),
        "{body}"
    );
}

// ---------- Task 13: agent wiring, late charges, answer page ----------

#[tokio::test]
async fn no_api_key_fails_closed() {
    let Some((pool, _guard)) = test_pool().await else {
        return;
    };
    // The real pipeline, and no Claude client (dev without ANTHROPIC_API_KEY).
    let mut st = app_state(pool.clone());
    st.worker = WorkerHandle::default();
    st.llm = None;
    let user = new_user_with_cap(&pool, 100_000).await;
    let run = started(run_ask(&st, &user, RULE_QUESTION, RunMode::Focused).await);
    poll_status(&pool, user.id, run, RunStatus::Failed).await;
    wait_idle(&st, run).await;
    assert_eq!(error_kind(&pool, run).await.as_deref(), Some("no_api_key"));
    assert_eq!(reservation(&pool, run).await, None);
    let v = get_run_for_user(&pool, user.id, run)
        .await
        .unwrap()
        .unwrap();
    assert!(v.answer.is_none() && v.steps.is_empty());
}

/// Starts one Claude call, abandons it after 50 ms (as the hard stop drops
/// a call mid-flight), and finishes the run.
fn abandons_a_call(ctx: &RunContext) -> PipelineFuture<'_> {
    Box::pin(async move {
        let call = ctx.llm.call(ctx, "m-test", "test");
        let r = tokio::time::timeout(Duration::from_millis(50), call.text("s", "u", 1000)).await;
        assert!(r.is_err(), "the call must still be in flight");
        PipelineOutcome::Done(json!({"abandoned": true}))
    })
}

/// Answers only once the test opens the gate, so "still in flight" does not
/// depend on timing.
struct GatedLlm {
    inner: incisive_app::agent::llm::ScriptedLlm,
    gate: Arc<tokio::sync::Notify>,
}

#[async_trait::async_trait]
impl incisive_app::agent::llm::LlmClient for GatedLlm {
    async fn call(
        &self,
        req: &claude_client::MessagesRequest,
    ) -> Result<claude_client::MessagesResponse, claude_client::ClaudeError> {
        self.gate.notified().await;
        self.inner.call(req).await
    }
}

/// Ruling R-8d: the worker keeps the reservation share held by calls still
/// in flight; the late charge books against it and the last call releases
/// the rest. Recovery's sweep leaves a just-finished run alone.
#[tokio::test]
async fn finish_keeps_headroom_for_calls_in_flight() {
    let Some((pool, _guard)) = test_pool_exclusive().await else {
        return;
    };
    let gate = Arc::new(tokio::sync::Notify::new());
    let llm = Arc::new(GatedLlm {
        inner: incisive_app::agent::llm::ScriptedLlm::new().reply_text(
            "late",
            Usage {
                input_tokens: 100,
                output_tokens: 20,
                ..Usage::default()
            },
        ),
        gate: gate.clone(),
    });
    let mut st = app_state(pool.clone());
    st.llm = Some(incisive_app::agent::testing::agent_llm(llm, "m-test"));
    let (uid, run) = reserved_run(&pool, 4000).await;
    st.worker.spawn_with(st.clone(), run, abandons_a_call);

    poll_status(&pool, uid, run, RunStatus::Done).await;
    wait_idle(&st, run).await;
    // Finished, but the call is still in flight: its hold stays reserved.
    let kept = reservation(&pool, run).await.expect("hold kept");
    assert!(kept > 0 && kept < 4000, "{kept}");
    assert!(charges(&pool, uid).await.is_empty());
    // The periodic sweep must not release it either.
    recover_stale_runs(&pool).await.unwrap();
    assert_eq!(reservation(&pool, run).await, Some(kept));

    // The late charge lands, then the call releases the rest.
    gate.notify_one();
    for _ in 0..300 {
        if reservation(&pool, run).await.is_none() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(reservation(&pool, run).await, None);
    let c = charges(&pool, uid).await;
    assert_eq!(c.len(), 1);
    let cost: i64 = sqlx::query_scalar("SELECT cost_paise FROM incisive.runs WHERE id = $1")
        .bind(run)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(cost, c[0].0, "runs.cost_paise is authoritative");
    // The run_finished audit was written before the late charge.
    let ev = run_events(&pool, run).await;
    let finished: Vec<_> = ev.iter().filter(|(e, _)| e == "run_finished").collect();
    assert_eq!(finished[0].1["cost_paise"], 0);
}

fn answer_json() -> serde_json::Value {
    use incisive_app::agent::answer::*;
    let a = Answer {
        question: "ICG <b>leak</b> evidence & more?".into(),
        mode: RunMode::Focused,
        created_at: chrono::Utc::now(),
        pico: Pico {
            population: "adults having colorectal resection".into(),
            intervention: "ICG angiography".into(),
            comparison: Some("white light".into()),
            outcomes: vec!["anastomotic leak".into()],
            limits: vec![],
        },
        final_query: "icg[tiab] AND leak[tiab]".into(),
        bottom_line: vec![
            AnswerClaim {
                text: "ICG <i>reduced</i> leak".into(),
                refs: vec![1, 2],
                confidence: "strong".into(),
                partial: false,
            },
            AnswerClaim {
                text: "Benefit was clearest in rectal resections".into(),
                refs: vec![1],
                confidence: "moderate".into(),
                partial: true,
            },
        ],
        insufficient: false,
        no_verified_claims: false,
        key_studies: vec![KeyStudy {
            ref_id: 1,
            citation: "Smith J et al., Lancet, 2025".into(),
            design: "meta-analysis".into(),
            n: Some(4754),
            main_finding: "anastomotic leak: RR 0.66".into(),
            source: "abstract only".into(),
        }],
        references: vec![Reference {
            ref_id: 1,
            title: "ICG meta-analysis".into(),
            journal: Some("Lancet".into()),
            year: Some(2025),
            pmid: Some("41871586".into()),
            doi: Some("10.1016/S2468-1253(25)00373-5".into()),
            study_type: "meta-analysis".into(),
        }],
        check: CheckSummary {
            checked: 2,
            supported: 1,
            partial: 1,
            removed: 0,
            refs_verified: 1,
            refs_total: 1,
        },
        trail: Trail {
            pubmed_count: 126,
            epmc_count: 789,
            screened: 15,
            included: 2,
            not_assessed: 0,
            excluded_top: vec![("wrong population".into(), 7)],
            refinements: vec![],
            stopped_early: None,
            write_refused: false,
            service_failed: false,
            unavailable: vec![],
        },
        cost_paise: 1840,
        elapsed_s: 161,
    };
    serde_json::to_value(a).unwrap()
}

#[tokio::test]
async fn ssr_run_page_renders_answer() {
    let Some((pool, _guard)) = test_pool().await else {
        return;
    };
    let st = app_state(pool.clone());
    let uid = new_user(&pool).await;
    let email = email_of(&pool, uid).await;
    let run = create_run(
        &pool,
        uid,
        "ICG <b>leak</b> evidence & more?",
        RunMode::Focused,
        4000,
    )
    .await
    .unwrap()
    .unwrap();
    set_status(&pool, run, RunStatus::Running, None)
        .await
        .unwrap();
    assert!(save_answer(&pool, run, &answer_json()).await.unwrap());
    set_status(&pool, run, RunStatus::Done, None).await.unwrap();

    let (s, body) = send(
        router(st.clone()),
        api("GET", &format!("/runs/{run}"), &email),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let at = |needle: &str| {
        body.find(needle)
            .unwrap_or_else(|| panic!("missing {needle:?} in {body}"))
    };
    // Decision support first, before the bottom line.
    assert!(at("Decision support \u{2014} the clinician decides.") < at("Bottom line"));
    assert!(at("Bottom line") < at("Key studies"));
    assert!(at("Key studies") < at("References"));
    // Escaped text, never markup.
    assert!(body.contains("ICG &lt;i&gt;reduced&lt;/i&gt; leak"));
    assert!(!body.contains("<i>reduced</i>"));
    assert!(!body.contains("<b>leak</b>"));
    // Partial claims marked; check, trail, cost and time.
    assert!(body.contains("\u{25d0}"));
    assert!(body.contains("2 claims checked: 1 supported, 1 partly supported"));
    assert!(body.contains("Screened 15, included 2, excluded 13"));
    assert!(body.contains("\u{20b9}18.40"));
    assert!(body.contains("2 min 41 s"));
    assert!(body.contains("https://pubmed.ncbi.nlm.nih.gov/41871586/"));
    // Edit & re-run prefills the question box.
    assert!(
        body.contains(
            "/#q=ICG%20%3Cb%3Eleak%3C%2Fb%3E%20evidence%20%26%20more%3F&amp;mode=focused"
        ),
        "{body}"
    );
    assert!(body.contains("Edit &amp; re-run"));

    // A stored body that is not an Answer is not shown raw.
    let run2 = create_run(&pool, uid, "second", RunMode::Quick, 500)
        .await
        .unwrap()
        .unwrap();
    set_status(&pool, run2, RunStatus::Running, None)
        .await
        .unwrap();
    assert!(
        save_answer(&pool, run2, &json!({"odd": "<script>x</script>"}))
            .await
            .unwrap()
    );
    set_status(&pool, run2, RunStatus::Done, None)
        .await
        .unwrap();
    let (s, body) = send(
        router(st.clone()),
        api("GET", &format!("/runs/{run2}"), &email),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert!(!body.contains("<script>x</script>"));
    assert!(body.contains("This answer can"), "{body}");

    // The server never reads a question from the query string: the box is
    // filled in the browser from the fragment, which is never sent here.
    let (s, body) = send(router(st), api("GET", "/?q=zebrafinch&mode=quick", &email)).await;
    assert_eq!(s, StatusCode::OK);
    assert!(body.contains("<textarea"));
    assert!(!body.contains("zebrafinch"), "{body}");
}

#[tokio::test]
async fn history_page_lists_up_to_100() {
    let Some((pool, _guard)) = test_pool().await else {
        return;
    };
    let st = app_state(pool.clone());
    let uid = new_user(&pool).await;
    let email = email_of(&pool, uid).await;
    for i in 0..25 {
        finished_run(&pool, uid, &format!("history item {i:02}"), 100 - i).await;
    }
    let (s, body) = send(router(st), api("GET", "/history", &email)).await;
    assert_eq!(s, StatusCode::OK);
    for i in 0..25 {
        assert!(body.contains(&format!("history item {i:02}")), "item {i}");
    }
}

// ---------- Task 14: Quick/Focused toggle and production settings ----------

use incisive_app::agent::answer::{initial_mode, mode_hint};
use incisive_app::config::Config;

/// `application/x-www-form-urlencoded` value (the browser's form encoding).
fn form_encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'*' => {
                out.push(char::from(b))
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Records the mode and depth settings the worker received, then waits to be
/// cancelled (so the reservation can be read while the run is active).
fn reports_settings(ctx: &RunContext) -> PipelineFuture<'_> {
    Box::pin(async move {
        let s = ctx.settings;
        let line = format!(
            "{} limit={} screen={} use={} full_text={} refinements={}",
            ctx.mode.as_str(),
            s.limit_paise,
            s.screen_n,
            s.use_n,
            s.full_text,
            s.refinements
        );
        add_step(&ctx.state.pool, ctx.run_id, StepKind::Frame, &line)
            .await
            .unwrap();
        for _ in 0..500 {
            if ctx.cancel.is_cancelled() {
                return PipelineOutcome::Failed("cancelled");
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        PipelineOutcome::Done(json!({"not": "cancelled"}))
    })
}

/// Send the question box's form (as the browser does) and return the run's
/// first step line and its reservation while it is still running.
async fn ask_through_form(
    pool: &PgPool,
    st: &AppState,
    email: &str,
    body: String,
) -> (Uuid, String, Option<i64>, i64) {
    let req = Request::post("/api/fn/ask")
        .header("content-type", "application/x-www-form-urlencoded")
        .header("x-dev-user-email", email)
        .body(Body::from(body))
        .unwrap();
    let (s, body) = send(router(st.clone()), req).await;
    assert_eq!(s, StatusCode::OK, "{body}");
    let v = as_json(&body);
    assert_eq!(v["status"], "run_started", "{body}");
    let run = Uuid::parse_str(v["run_id"].as_str().unwrap()).unwrap();
    let mut line = None;
    for _ in 0..500 {
        let rv = sqlx::query_scalar::<_, String>(
            "SELECT line FROM incisive.run_steps WHERE run_id = $1 ORDER BY seq LIMIT 1",
        )
        .bind(run)
        .fetch_optional(pool)
        .await
        .unwrap();
        if rv.is_some() {
            line = rv;
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let line = line.expect("the worker wrote its settings line");
    let held = reservation(pool, run).await;
    let limit: i64 = sqlx::query_scalar("SELECT limit_paise FROM incisive.runs WHERE id = $1")
        .bind(run)
        .fetch_one(pool)
        .await
        .unwrap();
    assert!(st.worker.cancel(run));
    wait_idle(st, run).await;
    (run, line, held, limit)
}

#[tokio::test]
async fn quick_request_uses_quick_limits_and_reservation() {
    let Some((pool, _guard)) = test_pool().await else {
        return;
    };
    let mut st = app_state(pool.clone());
    st.worker = WorkerHandle::with_pipeline(reports_settings);
    let uid = new_user(&pool).await;
    let email = email_of(&pool, uid).await;
    let (run, line, held, limit) = ask_through_form(
        &pool,
        &st,
        &email,
        format!("question={}&mode=quick", form_encode(RULE_QUESTION)),
    )
    .await;
    assert_eq!(
        line,
        "quick limit=1500 screen=20 use=5 full_text=false refinements=1"
    );
    // The full Quick limit (owner ruling: 1500 paise) is reserved up front.
    assert_eq!(held, Some(1500));
    assert_eq!(limit, 1500);
    let v = get_run_for_user(&pool, uid, run).await.unwrap().unwrap();
    assert_eq!(v.summary.mode, RunMode::Quick);
    // Ended: nothing left reserved.
    assert_eq!(reservation(&pool, run).await, None);
}

#[tokio::test]
async fn unknown_mode_is_rejected_before_any_work() {
    let Some((pool, _guard)) = test_pool().await else {
        return;
    };
    let mut st = app_state(pool.clone());
    st.worker = WorkerHandle::with_pipeline(reports_settings);
    let uid = new_user(&pool).await;
    let email = email_of(&pool, uid).await;
    let req = Request::post("/api/fn/ask")
        .header("content-type", "application/x-www-form-urlencoded")
        .header("x-dev-user-email", &email)
        .body(Body::from(format!(
            "question={}&mode=turbo",
            form_encode(RULE_QUESTION)
        )))
        .unwrap();
    let (s, body) = send(router(st), req).await;
    // Leptos answers a form it cannot decode with 500 (before the function
    // body runs, so the app cannot choose the status). Pinned so a change shows.
    assert_eq!(s, StatusCode::INTERNAL_SERVER_ERROR, "{body}");
    assert!(body.contains("unknown variant `turbo`"), "{body}");
    assert!(!body.contains("run_started"), "{body}");
    // Rejected while decoding the form, before identity, the identifier filter
    // and the gate: no run, no money held, no audit row.
    assert!(list_runs(&pool, uid, 10).await.unwrap().is_empty());
    let spend: i64 = sqlx::query_scalar("SELECT count(*) FROM incisive.spend WHERE user_id = $1")
        .bind(uid)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(spend, 0);
    assert!(audit_rows(&pool, uid).await.is_empty());
}

#[tokio::test]
async fn mode_defaults_to_focused() {
    // The type, the toggle's choice of starting mode, and the wire default.
    assert_eq!(RunMode::default(), RunMode::Focused);
    assert_eq!(initial_mode(None, None), RunMode::Focused);
    assert_eq!(initial_mode(None, Some("nonsense")), RunMode::Focused);
    assert_eq!(initial_mode(None, Some("focused")), RunMode::Focused);
    // The browser's remembered choice applies when nothing else says otherwise.
    assert_eq!(initial_mode(None, Some("quick")), RunMode::Quick);
    // Edit & re-run's fragment beats the remembered choice for that page load.
    assert_eq!(
        initial_mode(Some(RunMode::Quick), Some("focused")),
        RunMode::Quick
    );
    assert_eq!(
        initial_mode(Some(RunMode::Focused), Some("quick")),
        RunMode::Focused
    );

    let Some((pool, _guard)) = test_pool().await else {
        return;
    };
    let mut st = app_state(pool.clone());
    st.worker = WorkerHandle::with_pipeline(reports_settings);
    let uid = new_user(&pool).await;
    let email = email_of(&pool, uid).await;
    // No `mode` field at all: Focused.
    let (run, line, held, limit) = ask_through_form(
        &pool,
        &st,
        &email,
        format!("question={}", form_encode(RULE_QUESTION)),
    )
    .await;
    assert_eq!(
        line,
        "focused limit=4000 screen=100 use=10 full_text=true refinements=3"
    );
    assert_eq!(held, Some(4000));
    assert_eq!(limit, 4000);
    let v = get_run_for_user(&pool, uid, run).await.unwrap().unwrap();
    assert_eq!(v.summary.mode, RunMode::Focused);
}

#[test]
fn mode_hint_text() {
    assert_eq!(
        mode_hint(RunMode::Quick, 1500),
        "Quick \u{b7} up to \u{20b9}15"
    );
    assert_eq!(
        mode_hint(RunMode::Focused, 4000),
        "Full \u{b7} up to \u{20b9}40"
    );
    // Not whole rupees: shown with paise.
    assert_eq!(
        mode_hint(RunMode::Quick, 1250),
        "Quick \u{b7} up to \u{20b9}12.50"
    );
}

#[tokio::test]
async fn ssr_ask_page_shows_mode_toggle_with_configured_limits() {
    let Some((pool, _guard)) = test_pool().await else {
        return;
    };
    let mut st = app_state(pool.clone());
    // The hints come from config (`RUN_LIMITS`), not from constants.
    st.runs.quick.limit_paise = 1200;
    let uid = new_user(&pool).await;
    let email = email_of(&pool, uid).await;
    let (s, body) = send(router(st.clone()), api("GET", "/", &email)).await;
    assert_eq!(s, StatusCode::OK);
    assert!(body.contains(r#"name="mode""#), "{body}");
    assert!(body.contains(r#"value="quick""#), "{body}");
    assert!(body.contains(r#"value="focused""#), "{body}");
    assert!(body.contains("Quick \u{b7} up to \u{20b9}12"), "{body}");
    // Stage 3: shown as Full; the stored value is still `focused`.
    assert!(body.contains("Full \u{b7} up to \u{20b9}40"), "{body}");
    // Focused is the one checked before the browser applies a saved choice.
    let focused = body.find(r#"value="focused""#).unwrap();
    let tag_start = body[..focused].rfind('<').unwrap();
    let tag_end = focused + body[focused..].find('>').unwrap();
    assert!(
        body[tag_start..tag_end].contains("checked"),
        "{}",
        &body[tag_start..=tag_end]
    );
    let quick = body.find(r#"value="quick""#).unwrap();
    let q_start = body[..quick].rfind('<').unwrap();
    let q_end = quick + body[quick..].find('>').unwrap();
    assert!(!body[q_start..q_end].contains("checked"));
    // The explanatory hint is tied to the radio group for screen readers.
    let fs = body.find("<fieldset").expect("fieldset");
    let fs_tag = &body[fs..fs + body[fs..].find('>').unwrap()];
    assert!(
        fs_tag.contains(r#"aria-describedby="depth-hint""#),
        "{fs_tag}"
    );
    assert!(
        body.contains(r#"<p class="hint" id="depth-hint">"#),
        "{body}"
    );

    // `/api/fn/me` carries the figures the hints use.
    let req = Request::post("/api/fn/me")
        .header("content-type", "application/x-www-form-urlencoded")
        .header("x-dev-user-email", &email)
        .body(Body::empty())
        .unwrap();
    let (s, body) = send(router(st), req).await;
    assert_eq!(s, StatusCode::OK, "{body}");
    let me = as_json(&body);
    assert_eq!(me["quick_limit_paise"], 1200);
    assert_eq!(me["focused_limit_paise"], 4000);
}

#[tokio::test]
async fn ssr_run_page_shows_mode_badge() {
    let Some((pool, _guard)) = test_pool().await else {
        return;
    };
    let st = app_state(pool.clone());
    let uid = new_user(&pool).await;
    let email = email_of(&pool, uid).await;
    let run = finished_run(&pool, uid, "badge question", 10).await;
    let (s, body) = send(
        router(st.clone()),
        api("GET", &format!("/runs/{run}"), &email),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert!(
        body.contains(r#"<span class="badge mode">Quick</span>"#),
        "{body}"
    );
    let focused = create_run(&pool, uid, "focused badge", RunMode::Focused, 4000)
        .await
        .unwrap()
        .unwrap();
    assert!(
        set_status(&pool, focused, RunStatus::Done, None)
            .await
            .unwrap()
    );
    let (s, body) = send(router(st), api("GET", &format!("/runs/{focused}"), &email)).await;
    assert_eq!(s, StatusCode::OK);
    assert!(
        body.contains(r#"<span class="badge mode">Full</span>"#),
        "{body}"
    );
    assert!(!body.contains(r#"<span class="badge mode">Quick</span>"#));
}

#[test]
fn ncbi_key_optional_and_redacted_in_debug() {
    use secrecy::ExposeSecret;
    let base = [
        ("CLAUDE_MODEL", "claude-sonnet-5-5"),
        ("DATABASE_URL", "postgres://u:p@127.0.0.1:1/x"),
        ("APP_ENV", "dev"),
        ("OWNER_EMAIL", "owner@example.com"),
    ];
    let cfg = |extra: &[(&'static str, &'static str)]| {
        let pairs: Vec<(&str, &str)> = base.iter().chain(extra).copied().collect();
        Config::from_lookup(move |k| {
            pairs
                .iter()
                .find(|(n, _)| *n == k)
                .map(|(_, v)| v.to_string())
        })
        .unwrap()
    };
    // Optional: absent or blank means no key (PubMed then paces at 3/s).
    assert!(cfg(&[]).ncbi_api_key.is_none());
    assert!(cfg(&[("NCBI_API_KEY", "   ")]).ncbi_api_key.is_none());
    // Present: kept as a secret, trimmed, and never shown by Debug.
    let c = cfg(&[("NCBI_API_KEY", " ncbi-FAKE-key-0123456789 ")]);
    assert_eq!(
        c.ncbi_api_key.as_ref().unwrap().expose_secret(),
        "ncbi-FAKE-key-0123456789"
    );
    let dbg = format!("{c:?}");
    assert!(!dbg.contains("ncbi-FAKE-key-0123456789"), "{dbg}");
    assert!(dbg.contains("ncbi_api_key"), "{dbg}");
}

// ---------- Task 14: workflow files ----------

fn workflow(name: &str) -> String {
    let path = format!(
        "{}/../../.github/workflows/{name}",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path}: {e}"))
}

/// The `run: |` scripts of a workflow, by step name (blank when unnamed).
fn run_scripts(yaml: &str) -> Vec<(String, String)> {
    let lines: Vec<&str> = yaml.lines().collect();
    let indent = |l: &str| l.len() - l.trim_start().len();
    let mut out = Vec::new();
    let mut step = String::new();
    let mut i = 0;
    while i < lines.len() {
        let t = lines[i].trim_start();
        if let Some(n) = t.strip_prefix("- name:") {
            step = n.trim().to_string();
        }
        if t == "run: |" {
            let key = indent(lines[i]);
            let mut script = String::new();
            i += 1;
            while i < lines.len() && (lines[i].trim().is_empty() || indent(lines[i]) > key) {
                script.push_str(lines[i]);
                script.push('\n');
                i += 1;
            }
            out.push((step.clone(), script));
            continue;
        }
        i += 1;
    }
    out
}

#[test]
fn unpaywall_email_in_smoke_env() {
    let ci = workflow("ci.yml");
    let scripts = run_scripts(&ci);
    let smoke = scripts
        .iter()
        .find(|(n, _)| n == "smoke test image")
        .map(|(_, s)| s)
        .expect("ci.yml has the smoke test step");
    // Prod mode requires it; a dummy `.invalid` address can never be contacted.
    assert!(smoke.contains("-e APP_ENV=prod"), "{smoke}");
    assert!(smoke.contains("-e UNPAYWALL_EMAIL="), "{smoke}");
}

#[test]
fn deploy_workflow_wires_phase2_settings() {
    let deploy = workflow("deploy.yml");
    let scripts = run_scripts(&deploy);
    // No inline expressions in any script: values arrive through `env:`.
    for (name, s) in &scripts {
        assert!(
            !s.contains("${{"),
            "inline expression in run script {name:?}"
        );
    }
    let check = &scripts
        .iter()
        .find(|(n, _)| n == "Check required repository variables")
        .expect("check step")
        .1;
    for v in ["AGENT_MODELS", "RUN_LIMITS", "UNPAYWALL_EMAIL"] {
        assert!(check.contains(v), "fail-fast check misses {v}");
    }
    // The key names the check accepts are the ones the app accepts.
    let quoted = |after: &str, prefix: &str| -> Vec<String> {
        let from = check
            .find(after)
            .unwrap_or_else(|| panic!("check misses {after}"));
        let at = from
            + check[from..]
                .find(prefix)
                .unwrap_or_else(|| panic!("check misses {prefix}"));
        let rest = &check[at + prefix.len()..];
        let mut v: Vec<String> = rest[..rest.find('"').unwrap()]
            .split_whitespace()
            .map(str::to_string)
            .collect();
        v.sort();
        v
    };
    let mut steps: Vec<String> = incisive_app::agent::llm::AgentModels::default()
        .entries()
        .iter()
        .map(|(s, _)| s.to_string())
        .collect();
    steps.sort();
    assert_eq!(quoted("AGENT_MODELS)", r#"allowed=""#), steps);
    assert_eq!(quoted("RUN_LIMITS)", r#"allowed=""#), ["focused", "quick"]);
    let mut fields: Vec<String> =
        match serde_json::to_value(incisive_app::runs::DepthSettings::QUICK).unwrap() {
            serde_json::Value::Object(m) => m.keys().cloned().collect(),
            _ => unreachable!(),
        };
    fields.sort();
    assert_eq!(quoted("RUN_LIMITS)", r#"fields=""#), fields);
    let run = &scripts
        .iter()
        .find(|(n, _)| n == "Deploy to Cloud Run")
        .expect("deploy step")
        .1;
    for v in ["\"AGENT_MODELS\"", "\"RUN_LIMITS\"", "\"UNPAYWALL_EMAIL\""] {
        assert!(run.contains(v), "env-vars file misses {v}");
    }
    assert!(run.contains("NCBI_API_KEY=ncbi-api-key:latest"));
    assert!(run.contains(r#""$HAS_NCBI_KEY" = "true""#));
    assert!(run.contains(r#""$CPU_ALWAYS" = "true""#));
    // Each branch pinned exactly (a bare `--cpu-throttling` would also match
    // inside `--no-cpu-throttling`); the throttled default is the cost-safe one.
    assert!(run.contains(r#"cpu_flag="--no-cpu-throttling""#));
    assert!(run.contains(r#"cpu_flag="--cpu-throttling""#));
    assert!(run.contains(r#""$cpu_flag""#));
    let else_branch = run
        .split("if [ \"$CPU_ALWAYS\" = \"true\" ]; then")
        .nth(1)
        .and_then(|rest| rest.split("else").nth(1))
        .and_then(|rest| rest.split("fi").next())
        .expect("CPU_ALWAYS if/else");
    assert_eq!(else_branch.trim(), r#"cpu_flag="--cpu-throttling""#);
    // The Deploy step's `env:` passes the guards in from repository variables.
    for v in [
        "HAS_NCBI_KEY: ${{ vars.HAS_NCBI_KEY }}",
        "CPU_ALWAYS: ${{ vars.CPU_ALWAYS }}",
        "AGENT_MODELS: ${{ vars.AGENT_MODELS }}",
        "RUN_LIMITS: ${{ vars.RUN_LIMITS }}",
        "UNPAYWALL_EMAIL: ${{ vars.UNPAYWALL_EMAIL }}",
    ] {
        assert!(deploy.contains(v), "deploy.yml misses `{v}`");
    }
}

/// The deploy check's QUICK_ENGINE lines (from the trim to the `esac`).
fn quick_engine_check(check: &str) -> String {
    let start = check.find("qe=").expect("QUICK_ENGINE trim");
    let end = start + check[start..].find("esac").expect("esac") + "esac".len();
    check[start..end].to_string()
}

/// Stage 2 §0: the Phase 3 roles and the Quick engine switch reach the
/// deploy. The price pre-check covers every role's **default** model (not
/// only keys set in AGENT_MODELS), from the app's own defaults.
#[test]
fn deploy_workflow_wires_phase3_settings() {
    let deploy = workflow("deploy.yml");
    let scripts = run_scripts(&deploy);
    let check = &scripts
        .iter()
        .find(|(n, _)| n == "Check required repository variables")
        .expect("check step")
        .1;
    // Every role and its default model appear in the pre-check's defaults.
    for (role, model) in incisive_app::agent::llm::AgentModels::default().entries() {
        let pair = format!(r#""{role}": "{model}""#);
        assert!(
            check.contains(&pair),
            "price pre-check misses default {pair}"
        );
    }
    // QUICK_ENGINE: optional, team or classic, checked before the build.
    assert!(check.contains("QUICK_ENGINE"), "check misses QUICK_ENGINE");
    assert!(
        check.contains(r#"""|team|classic) ;;"#),
        "QUICK_ENGINE must be checked against team|classic"
    );
    // Review finding 9: only leading and trailing blanks are stripped (as
    // the app trims), so "tea m" is refused, not read as "team".
    assert!(
        !check.contains("${QUICK_ENGINE//[[:space:]]/}"),
        "inner spaces must not be removed"
    );
    for (value, ok) in [
        ("team", true),
        ("  classic ", true),
        ("", true),
        ("tea m", false),
        ("Team", false),
    ] {
        let status = std::process::Command::new("bash")
            .arg("-c")
            .arg(format!("{}\nexit 0", quick_engine_check(check)))
            .env("QUICK_ENGINE", value)
            .status()
            .expect("bash");
        assert_eq!(status.success(), ok, "QUICK_ENGINE={value:?}");
    }
    let run = &scripts
        .iter()
        .find(|(n, _)| n == "Deploy to Cloud Run")
        .expect("deploy step")
        .1;
    assert!(
        run.contains(r#"optional = ["AGENT_MODELS", "RUN_LIMITS", "QUICK_ENGINE", "FULL_ENGINE"]"#),
        "QUICK_ENGINE must be an optional env var"
    );
    assert_eq!(
        deploy
            .matches("QUICK_ENGINE: ${{ vars.QUICK_ENGINE }}")
            .count(),
        2,
        "QUICK_ENGINE must reach both the check and the deploy step"
    );
}

/// Reports the hard stop the worker gave the run's budget.
fn reports_hard_stop(ctx: &RunContext) -> PipelineFuture<'_> {
    Box::pin(async move {
        PipelineOutcome::Done(json!({"hard_stop_s": ctx.budget.hard_stop().as_secs()}))
    })
}

/// M-1: the worker builds each run's budget from the mode's `time_limit_s`
/// (RUN_LIMITS): hard stop = time_limit_s + 60 s.
#[tokio::test]
async fn worker_uses_the_modes_time_limit() {
    let Some((pool, _guard)) = test_pool().await else {
        return;
    };
    let mut st = app_state(pool.clone());
    st.runs.focused.time_limit_s = 120;
    let (uid, run) = reserved_run(&pool, 4000).await;
    st.worker.spawn_with(st.clone(), run, reports_hard_stop);
    poll_status(&pool, uid, run, RunStatus::Done).await;
    wait_idle(&st, run).await;
    let v = get_run_for_user(&pool, uid, run).await.unwrap().unwrap();
    assert_eq!(v.answer, Some(json!({"hard_stop_s": 180})));
}

async fn backdate_reservation(pool: &PgPool, request: Uuid, secs: i64) {
    sqlx::query(
        "UPDATE incisive.spend SET at = now() - make_interval(secs => $2) \
         WHERE request_id = $1 AND kind = 'reservation'",
    )
    .bind(request)
    .bind(secs as f64)
    .execute(pool)
    .await
    .unwrap();
}

async fn delete_run_row(pool: &PgPool, run: Uuid) {
    sqlx::query("DELETE FROM incisive.runs WHERE id = $1")
        .bind(run)
        .execute(pool)
        .await
        .unwrap();
}

/// M-2: a run reservation whose run row is gone (its release failed) is
/// released by the recovery sweep once it is older than the grace period.
#[tokio::test]
async fn recovery_releases_reservations_whose_run_is_gone() {
    let Some((pool, _guard)) = test_pool_exclusive().await else {
        return;
    };
    // Orphaned 15 minutes ago: released.
    let (u1, old) = reserved_run(&pool, 4000).await;
    delete_run_row(&pool, old).await;
    backdate_reservation(&pool, old, 15 * 60).await;
    // Orphaned just now (a delete may still be releasing): kept.
    let (_u2, fresh) = reserved_run(&pool, 4000).await;
    delete_run_row(&pool, fresh).await;
    // Old reservation whose run still exists (an active run): kept.
    let (_u3, live) = reserved_run(&pool, 4000).await;
    backdate_reservation(&pool, live, 15 * 60).await;
    // An old gate reservation (no run by design): not this sweep's.
    let gate_req = Uuid::new_v4();
    let user = full_user(&pool, u1).await;
    reserve(&pool, &user, &big_global(), 30, gate_req, "gate")
        .await
        .unwrap();
    backdate_reservation(&pool, gate_req, 15 * 60).await;

    recover_stale_runs(&pool).await.unwrap();
    assert_eq!(reservation(&pool, old).await, None);
    assert_eq!(reservation(&pool, fresh).await, Some(4000));
    assert_eq!(reservation(&pool, live).await, Some(4000));
    assert_eq!(reservation(&pool, gate_req).await, Some(30));
    // The caps no longer count it.
    assert_eq!(
        totals(&pool, u1).await.unwrap().today_paise,
        30,
        "only the gate reservation remains"
    );
}

/// M-7: the run page header is the full question (not the 80-character
/// title), as escaped text.
#[tokio::test]
async fn ssr_run_page_header_is_the_full_question() {
    let Some((pool, _guard)) = test_pool().await else {
        return;
    };
    let st = app_state(pool.clone());
    let uid = new_user(&pool).await;
    let email = email_of(&pool, uid).await;
    let question = format!(
        "In adults having robotic low anterior resection, does <i>ICG</i> angiography reduce leak? {}",
        "Please include RCTs and meta-analyses from the last ten years."
    );
    assert!(question.chars().count() > 80);
    let run = create_run(&pool, uid, &question, RunMode::Focused, 4000)
        .await
        .unwrap()
        .unwrap();
    set_status(&pool, run, RunStatus::Running, None)
        .await
        .unwrap();
    let mut body = answer_json();
    body["question"] = json!(question);
    assert!(save_answer(&pool, run, &body).await.unwrap());
    set_status(&pool, run, RunStatus::Done, None).await.unwrap();

    let (s, html) = send(router(st), api("GET", &format!("/runs/{run}"), &email)).await;
    assert_eq!(s, StatusCode::OK);
    let escaped = question.replace('<', "&lt;").replace('>', "&gt;");
    let h2 = html.find("<h2 class=\"run-question\">").expect("header");
    let end = h2 + html[h2..].find("</h2>").unwrap();
    assert!(html[h2..end].contains(&escaped), "{}", &html[h2..end]);
    assert!(!html.contains("<i>ICG</i>"));
}

// ---------- Phase 3 T6: the AnswerV3 page ----------

fn answer_v3_json(retracted: bool) -> serde_json::Value {
    use incisive_app::agent::answer_v3::*;
    let a = AnswerV3 {
        version: ANSWER_VERSION,
        question: "Robotic <i>vs</i> lap hernia?".into(),
        mode: RunMode::Quick,
        created_at: chrono::Utc::now(),
        answer: Some(WrittenAnswer {
            bottom_line: BottomLine {
                text: "No benefit <script>alert(1)</script> [41060645; 39979619].".into(),
                certainty: Certainty::Low,
                certainty_reason: "Small trials <b>only</b> [39979619]".into(),
            },
            outcomes: vec![Outcome {
                outcome: "Chronic pain".into(),
                rct_evidence: "Two RCTs".into(),
                effect_text: "RR 0.53 (0.20 to 1.39) [41060645]".into(),
                numbers: vec![Number {
                    label: "RR".into(),
                    value: 0.53,
                    unit: None,
                    arm: None,
                    ci_low: Some(0.2),
                    ci_high: Some(1.39),
                    p: None,
                    pmid: "41060645".into(),
                }],
                certainty: Certainty::VeryLow,
                why: "wide CI".into(),
                pmids: vec!["41060645".into()],
            }],
            studies: vec![Study {
                pmid: "39979619".into(),
                label: "Self-fixated <img src=x onerror=alert(2)> (2025)".into(),
                design: "RCT".into(),
                n: Some(120),
                setting: "Single centre".into(),
                comparison: "self-fixating vs tacked".into(),
                key_results: "no difference [39979619]".into(),
                appraisal: "short follow-up".into(),
            }],
            conflicts: vec![Conflict {
                text: "One trial differs [39979619]".into(),
                pmids: vec!["39979619".into()],
            }],
            applicability: Applicability {
                case_mix: "Primary hernias".into(),
                india_cost_context: "Indian costs are not known.".into(),
                training: "A low-risk index case.".into(),
            },
            would_change: vec!["A multicentre blinded RCT".into()],
        }),
        empty_reason: None,
        references: vec![
            ReferenceV3 {
                n: 1,
                pmid: "41060645".into(),
                title: "Biological vs Synthetic Mesh".into(),
                journal: Some("JAMA Surg".into()),
                year: Some(2025),
                doi: Some("10.1001/jamasurg.2025.4071".into()),
                retracted: false,
                found_by_skeptic: false,
            },
            ReferenceV3 {
                n: 2,
                pmid: "39979619".into(),
                title: "Self-fixated meshes".into(),
                journal: Some("Surg Endosc".into()),
                year: Some(2025),
                doi: None,
                retracted,
                found_by_skeptic: false,
            },
        ],
        searches: vec![SearchEntry {
            query: "hernia[tiab] AND <mesh>".into(),
            source: "PubMed".into(),
            hits: 327,
        }],
        integrity: IntegrityV3 {
            dropped_unseen: 1,
            ..IntegrityV3::default()
        },
        stopped_early: None,
        cost_paise: 2345,
        elapsed_s: 125,
        skeptic: None,
        evidence: vec![],
        notes: vec![],
    };
    serde_json::to_value(a).unwrap()
}

/// The page without its `<script>` blocks (which carry the hydration data,
/// including the raw stored answer): what is drawn.
fn markup_of(html: &str) -> String {
    let mut markup = String::new();
    let mut rest = html;
    while let Some(i) = rest.find("<script") {
        markup.push_str(&rest[..i]);
        rest = rest[i..]
            .find("</script>")
            .map_or("", |j| &rest[i + j + "</script>".len()..]);
    }
    markup.push_str(rest);
    markup
}

async fn v3_page(pool: &PgPool, body: serde_json::Value) -> (StatusCode, String) {
    let st = app_state(pool.clone());
    let uid = new_user(pool).await;
    let email = email_of(pool, uid).await;
    let run = create_run(pool, uid, "Robotic vs lap hernia?", RunMode::Quick, 6000)
        .await
        .unwrap()
        .unwrap();
    set_status(pool, run, RunStatus::Running, None)
        .await
        .unwrap();
    assert!(save_answer(pool, run, &body).await.unwrap());
    set_status(pool, run, RunStatus::Done, None).await.unwrap();
    send(
        router(st),
        api("GET", &format!("/runs/{run}"), email.as_str()),
    )
    .await
}

/// T6: a stored AnswerV3 renders in the spec order, model text escaped,
/// `[PMID]` markers as numbered reference links, tables in a horizontal
/// scroller, a retraction warning and the collapsed search trail.
#[tokio::test]
async fn ssr_answer_v3_page_escapes_text_and_links_citations() {
    let Some((pool, _guard)) = test_pool().await else {
        return;
    };
    let (s, html) = v3_page(&pool, answer_v3_json(true)).await;
    assert_eq!(s, StatusCode::OK);
    // Escaped, never markup.
    assert!(
        html.contains("&lt;script&gt;alert(1)&lt;/script&gt;"),
        "{html}"
    );
    assert!(!html.contains("<script>alert(1)"));
    assert!(!html.contains("<img src=x"));
    assert!(!html.contains("<b>only</b>"));
    assert!(
        html.contains("&lt;i&gt;vs&lt;/i&gt;"),
        "the question header is escaped"
    );
    assert!(html.contains("hernia[tiab] AND &lt;mesh&gt;"));
    // Citations: numbered links to the references, no raw PMID markers.
    assert!(html.contains(r##"href="#ref-1""##));
    assert!(html.contains(r##"href="#ref-2""##));
    assert!(html.contains(r#"id="ref-1""#) && html.contains(r#"id="ref-2""#));
    // The markup (scripts, which carry the hydration data, removed) has no
    // raw markers.
    let markup = markup_of(&html);
    assert!(!markup.contains("[41060645"), "raw markers are replaced");
    assert!(markup.contains(
        r##"No benefit &lt;script&gt;alert(1)&lt;/script&gt; <span class="cite">[<a href="#ref-1" rel="external">1</a>, <a href="#ref-2" rel="external">2</a>"##
    ));
    assert!(html.contains("https://pubmed.ncbi.nlm.nih.gov/41060645/"));
    assert!(html.contains("https://doi.org/10.1001/jamasurg.2025.4071"));
    // Retraction warning on the retracted reference only.
    assert_eq!(html.matches("RETRACTED").count(), 1);
    // Sections in order.
    let mut at = 0;
    for h in [
        "Bottom line",
        "Certainty: Low",
        "Outcome by outcome",
        "The trials",
        "Conflicting evidence",
        "Applicability",
        "India and cost",
        "What would change this answer",
        "References",
        "<details",
        "Search trail (1 searches)",
        "Cost and time",
        "₹23.45",
    ] {
        let i = html[at..]
            .find(h)
            .unwrap_or_else(|| panic!("{h} missing or out of order"));
        at += i + h.len();
    }
    // Tables scroll sideways on a phone.
    assert_eq!(
        html.matches(r#"<div class="scroll"><table class="wide">"#)
            .count(),
        2
    );
    // The trail notes what the checks dropped.
    assert!(html.contains("not returned by this run"));
    // The Phase 2 sections are not used for a v3 answer.
    assert!(!html.contains("How I read your question"));
}

/// T6: a v3 answer with nothing written shows its reason; a Phase 2 answer
/// still renders with its own view.
#[tokio::test]
async fn ssr_empty_v3_answer_and_phase2_answers_still_render() {
    let Some((pool, _guard)) = test_pool().await else {
        return;
    };
    let mut body = answer_v3_json(false);
    body["answer"] = serde_json::Value::Null;
    body["empty_reason"] = json!("No relevant evidence found.");
    let (s, html) = v3_page(&pool, body).await;
    assert_eq!(s, StatusCode::OK);
    let html = markup_of(&html);
    assert!(html.contains("No relevant evidence found."));
    assert!(html.contains(r#"id="ref-1""#), "references still listed");
    assert!(!html.contains("Outcome by outcome"));
    assert!(!html.contains("RETRACTED"));
    assert!(!html.contains("can't be shown"));

    let (s, html) = v3_page(&pool, answer_json()).await;
    assert_eq!(s, StatusCode::OK);
    let html = markup_of(&html);
    assert!(html.contains("How I read your question"), "Phase 2 view");
    assert!(!html.contains("Outcome by outcome"));
}

/// The deploy check's FULL_ENGINE lines (from the trim to its `esac`).
fn full_engine_check(check: &str) -> String {
    let start = check.find("fe=").expect("FULL_ENGINE trim");
    let end = start + check[start..].find("esac").expect("esac") + "esac".len();
    check[start..end].to_string()
}

/// Stage 3 §0: the Full engine switch reaches the deploy like QUICK_ENGINE:
/// optional, checked against team|classic with only the ends trimmed, and
/// written to the env-vars file only when set.
#[test]
fn deploy_workflow_wires_the_full_engine() {
    let deploy = workflow("deploy.yml");
    let scripts = run_scripts(&deploy);
    let check = &scripts
        .iter()
        .find(|(n, _)| n == "Check required repository variables")
        .expect("check step")
        .1;
    assert!(check.contains("FULL_ENGINE"), "check misses FULL_ENGINE");
    assert!(
        !check.contains("${FULL_ENGINE//[[:space:]]/}"),
        "inner spaces must not be removed"
    );
    for (value, ok) in [
        ("team", true),
        ("  classic ", true),
        ("", true),
        ("tea m", false),
        ("Classic", false),
        ("focused", false),
    ] {
        let status = std::process::Command::new("bash")
            .arg("-c")
            .arg(format!("{}\nexit 0", full_engine_check(check)))
            .env("FULL_ENGINE", value)
            .status()
            .expect("bash");
        assert_eq!(status.success(), ok, "FULL_ENGINE={value:?}");
    }
    assert_eq!(
        deploy
            .matches("FULL_ENGINE: ${{ vars.FULL_ENGINE }}")
            .count(),
        2,
        "FULL_ENGINE must reach both the check and the deploy step"
    );
}

// ---------- Phase 3 Stage 3: the Full additions to the AnswerV3 page ----------

/// A v3 answer stored before Stage 3 (no `skeptic`, `evidence`, `notes` or
/// `found_by_skeptic`) still loads and renders.
#[tokio::test]
async fn ssr_v3_answers_stored_before_stage3_still_render() {
    let Some((pool, _guard)) = test_pool().await else {
        return;
    };
    let mut body = answer_v3_json(false);
    let o = body.as_object_mut().unwrap();
    for k in ["skeptic", "evidence", "notes"] {
        assert!(o.remove(k).is_some(), "{k}");
    }
    for r in o["references"].as_array_mut().unwrap() {
        r.as_object_mut().unwrap().remove("found_by_skeptic");
    }
    let a: incisive_app::agent::answer_v3::AnswerV3 = serde_json::from_value(body.clone()).unwrap();
    assert!(a.skeptic.is_none() && a.evidence.is_empty() && a.notes.is_empty());
    let (s, html) = v3_page(&pool, body).await;
    assert_eq!(s, StatusCode::OK);
    let m = markup_of(&html);
    assert!(m.contains("The trials"));
    assert!(!m.contains("Challenged by the skeptic"));
    assert!(!m.contains("Evidence checked"));
}

/// Full: the skeptic box (verdict, caveats, overlaps, missed trials) and the
/// trials table's evidence column, escaped, with citations linked.
#[tokio::test]
async fn ssr_full_answer_shows_the_skeptic_box_and_evidence_checked() {
    use incisive_app::agent::answer_v3::*;
    let Some((pool, _guard)) = test_pool().await else {
        return;
    };
    let mut a: AnswerV3 = serde_json::from_value(answer_v3_json(false)).unwrap();
    a.mode = RunMode::Focused;
    a.skeptic = Some(SkepticV3 {
        verdict: "holds with caveats".into(),
        must_mention: vec![
            "Two trials share <script>alert(3)</script> one group [41060645].".into(),
        ],
        contradicting: vec![SkepticItem {
            pmid: "41060645".into(),
            text: "faster with experience".into(),
            quote: Some("Operative time fell to 38 <b>min</b>.".into()),
        }],
        missed_rcts: vec![SkepticItem {
            pmid: "39979619".into(),
            text: "a newer trial".into(),
            quote: None,
        }],
        guidelines: vec![],
        overlaps: vec![SkepticOverlap {
            // 12345678 has no reference (its record could not be fetched).
            pmids: vec!["41060645".into(), "39979619".into(), "12345678".into()],
            note: "same group".into(),
        }],
    });
    a.evidence = vec![EvidenceV3 {
        pmid: "39979619".into(),
        text: "full text".into(),
        verified_quotes: 3,
        numbers_dropped: 1,
        unverified_outcomes: vec![],
    }];
    a.notes = vec![
        "Extraction stopped at the Full limit; 1 study uses the abstract only, labelled as such."
            .into(),
    ];
    let (s, html) = v3_page(&pool, serde_json::to_value(&a).unwrap()).await;
    assert_eq!(s, StatusCode::OK);
    let m = markup_of(&html);
    assert!(m.contains("Challenged by the skeptic"), "{m}");
    assert!(m.contains("Verdict: the conclusion holds with caveats."));
    assert!(!m.contains("<script>alert(3)"), "escaped");
    assert!(m.contains("&lt;script&gt;alert(3)"));
    assert!(m.contains("Not independent"));
    assert!(m.contains("Trials the search missed"));
    // Re-review 3: a named study without a reference is linked to PubMed.
    assert!(
        m.contains(r#"href="https://pubmed.ncbi.nlm.nih.gov/12345678/""#),
        "{m}"
    );
    // Review 2: the verified quote is shown, escaped.
    assert!(
        m.contains("faster with experience \u{2014} \u{201c}Operative time fell to 38 &lt;b&gt;min&lt;/b&gt;.\u{201d}"),
        "{m}"
    );
    assert!(m.contains("#ref-1"), "citations link to references");
    assert!(m.contains("Evidence checked"));
    assert!(m.contains("full text \u{b7} 3 verified quotes"));
    assert!(m.contains("Extraction stopped at the Full limit"));
    // The box comes before the references.
    assert!(m.find("Challenged by the skeptic").unwrap() < m.find(">References<").unwrap());
}
