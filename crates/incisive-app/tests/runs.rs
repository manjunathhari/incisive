//! Run-store tests, gated on `TEST_DATABASE_URL`. Every test creates its own
//! user (unique email), so tests running in parallel on one DB never share rows.
//! The two global operations (`mark_stale_runs_interrupted`,
//! `delete_runs_older_than`) are asserted only on rows each test created.

use claude_client::Usage;
use incisive_app::db::{lazy_pool, migrate};
use incisive_app::identity::User;
use incisive_app::runs::budget::{BudgetError, RunBudget};
use incisive_app::runs::store::{
    ActiveRunExists, add_cost, add_step, create_run, delete_all_runs, delete_run,
    delete_runs_older_than, get_run_for_user, list_runs, load_question,
    mark_stale_runs_interrupted, save_answer, set_status,
};
use incisive_app::runs::{RunMode, RunStatus, StepKind};
use incisive_app::spend::{CapCheck, CapsConfig, Prices, charge_call, release, reserve, totals};
use secrecy::SecretString;
use serde_json::json;
use sqlx::PgPool;
use std::time::{Duration, Instant};
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

fn budget(run: Uuid, user: i64, limit: i64, started: Instant) -> RunBudget {
    RunBudget::new(run, user, limit, started, prices(), 88.0)
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

/// A run with `limit` reserved up front, as `run_ask` will do.
async fn reserved_run(pool: &PgPool, limit: i64) -> (i64, Uuid) {
    let uid = new_user(pool).await;
    let run = create_run(pool, uid, "q", RunMode::Focused, limit)
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
    charge_call(&pool, uid, run, call, "m-test", &usage(10, 10), 300)
        .await
        .unwrap();
    assert_eq!(reservation(&pool, run).await, Some(3700));
    assert_eq!(charges(&pool, uid).await, [(300, format!("run:{run}"))]);
    let t = totals(&pool, uid).await.unwrap();
    assert_eq!((t.today_paise, t.month_paise), (4000, 4000));
    // Replaying the same call id neither double-charges nor double-decrements.
    charge_call(&pool, uid, run, call, "m-test", &usage(10, 10), 300)
        .await
        .unwrap();
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

#[tokio::test]
async fn budget_charge_books_audits_and_adds_run_cost() {
    let Some((pool, _guard)) = test_pool().await else {
        return;
    };
    let (uid, run) = reserved_run(&pool, 4000).await;
    let b = budget(run, uid, 4000, Instant::now());
    // 1,000 in + 150 out at $1/$5 per Mtok, 88 INR/USD = 15.4 paise -> 16.
    assert_eq!(
        b.charge(&pool, "m-test", &usage(1000, 150)).await.unwrap(),
        16
    );
    assert_eq!(b.spent(), 16);
    assert_eq!(reservation(&pool, run).await, Some(4000 - 16));
    let v = get_run_for_user(&pool, uid, run).await.unwrap().unwrap();
    assert_eq!(v.summary.cost_paise, 16);
    let rows: Vec<(String, serde_json::Value)> =
        sqlx::query_as("SELECT event, detail FROM incisive.audit WHERE user_id = $1 ORDER BY seq")
            .bind(uid)
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].0, "charged");
    assert_eq!(rows[0].1["paise"], 16);
    assert_eq!(rows[0].1["model"], "m-test");
    assert!(!rows[0].1.to_string().contains("\"q\""));
}

#[tokio::test]
async fn unknown_model_is_an_error_not_free() {
    let Some((pool, _guard)) = test_pool().await else {
        return;
    };
    let (uid, run) = reserved_run(&pool, 4000).await;
    let b = budget(run, uid, 4000, Instant::now());
    let err = b.charge(&pool, "mystery", &usage(5, 5)).await.unwrap_err();
    assert!(matches!(err, BudgetError::UnknownModel));
    assert_eq!(b.spent(), 0);
    assert!(charges(&pool, uid).await.is_empty());
    assert_eq!(reservation(&pool, run).await, Some(4000));
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
