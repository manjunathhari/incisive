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
    assert_eq!(limit, 500);
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
    // Room for the gate estimate and a Quick run, not a Focused one (4000).
    let user = new_user_with_cap(&pool, 1_000).await;
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

    // A Quick run (500) still fits.
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
    let mut tx = pool.begin().await.unwrap();
    sqlx::query("SET LOCAL lock_timeout = '5s'")
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query(&format!(
        "CREATE FUNCTION {func}() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN \
           IF {when} THEN {act} END IF; RETURN NEW; END $$"
    ))
    .execute(&mut *tx)
    .await
    .unwrap();
    sqlx::query(&format!(
        "CREATE TRIGGER {trig} BEFORE INSERT OR UPDATE ON {table} \
         FOR EACH ROW EXECUTE FUNCTION {func}()"
    ))
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();

    // In its own task, so a panic is caught and the trigger still dropped.
    let res = tokio::spawn(body).await;

    let mut tx = pool.begin().await.unwrap();
    sqlx::query("SET LOCAL lock_timeout = '5s'")
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query(&format!("DROP TRIGGER {trig} ON {table}"))
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query(&format!("DROP FUNCTION {func}()"))
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    match res {
        Ok(v) => v,
        Err(e) => std::panic::resume_unwind(e.into_panic()),
    }
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
            excluded_top: vec![("wrong population".into(), 7)],
            refinements: vec![],
            stopped_early: None,
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
            "/?q=ICG%20%3Cb%3Eleak%3C%2Fb%3E%20evidence%20%26%20more%3F&amp;mode=focused"
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

    // The home page prefills from ?q= (rendered for the browser to hydrate).
    let (s, body) = send(
        router(st),
        api("GET", "/?q=ICG%20%3Cb%3Eleak&mode=quick", &email),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    // The box is filled in the browser (`prop:value`): the server never puts
    // the query text into the page as markup.
    assert!(body.contains("<textarea"));
    assert!(!body.contains("<b>leak"), "{body}");
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
