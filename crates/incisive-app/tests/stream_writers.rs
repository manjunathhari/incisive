//! Stage S1: the writer's streamed path and its money, and the drafts. A
//! scripted Claude only (no network). Needs `TEST_DATABASE_URL` (skipped when
//! unset).

use std::sync::Arc;
use std::time::{Duration, Instant};

use claude_client::Usage;
use incisive_app::agent::RunContext;
use incisive_app::agent::llm::{LlmCall, LlmClient, ScriptedLlm, StepError, Streaming};
use incisive_app::agent::testing::{agent_llm, idle_llm, offline_literature};
use incisive_app::agent::writer_v3::{self, WriteFailure};
use incisive_app::db::{lazy_pool, migrate};
use incisive_app::gate::OfflineClassifier;
use incisive_app::identity::{IapVerifier, JwkSource, User};
use incisive_app::runs::budget::RunBudget;
use incisive_app::runs::drafts::{self, Part};
use incisive_app::runs::store::{create_run, delete_run, save_answer, set_status};
use incisive_app::runs::temporal::{self, JobStatus, insert_pending};
use incisive_app::runs::worker::{CancelFlag, WorkerHandle, recover_stale_runs};
use incisive_app::runs::{DepthSettings, RunMode, RunStatus, RunsConfig, api};
use incisive_app::server::AppState;
use incisive_app::spend::{CapCheck, CapsConfig, Prices, reserve};
use secrecy::SecretString;
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

#[path = "support/stream_llm.rs"]
mod stream_llm;
use stream_llm::{Broken, StreamLlm, broken};

const MODEL: &str = "m-test";

async fn pool() -> Option<PgPool> {
    let Ok(url) = std::env::var("TEST_DATABASE_URL") else {
        eprintln!("SKIP: TEST_DATABASE_URL not set");
        return None;
    };
    let pool = lazy_pool(&SecretString::from(url)).expect("lazy pool");
    migrate(&pool).await.expect("migrate");
    Some(pool)
}

fn prices() -> Prices {
    Prices::from_json(r#"{"m-test": {"input": 1.0, "output": 5.0}}"#).unwrap()
}

fn state(pool: PgPool, stream: bool) -> AppState {
    AppState {
        pool,
        verifier: Arc::new(IapVerifier::new(
            "aud".into(),
            JwkSource::Static(serde_json::from_str(r#"{"keys":[]}"#).unwrap()),
        )),
        gate: Arc::new(OfflineClassifier),
        prices: prices(),
        usd_inr: 88.0,
        caps: CapsConfig {
            global_monthly_paise: i64::MAX / 4,
        },
        gate_model: MODEL.into(),
        dev_mode: true,
        runs: RunsConfig::default(),
        quick_engine: incisive_app::runs::QuickEngine::Team,
        full_engine: incisive_app::runs::Engine::Team,
        completeness: false,
        temporal: Default::default(),
        temporal_client: None,
        stream,
        intro: Default::default(),
        graph: Default::default(),
        memory: Default::default(),
        chat: Default::default(),
        embedder: None,
        gemma: None,
        worker: WorkerHandle::default(),
        llm: Some(idle_llm(MODEL)),
        lit: offline_literature(),
    }
}

async fn new_user(pool: &PgPool) -> (i64, User) {
    let email = format!("draft-{}@example.com", Uuid::new_v4());
    let uid: i64 = sqlx::query_scalar(
        "INSERT INTO incisive.users (email, display_name, role, daily_cap_paise) \
         VALUES ($1, 'T', 'surgeon', 100000) RETURNING id",
    )
    .bind(&email)
    .fetch_one(pool)
    .await
    .unwrap();
    let user = incisive_app::identity::find_user(pool, &email)
        .await
        .unwrap()
        .unwrap();
    (uid, user)
}

/// A running Quick run of a new user, `limit` reserved.
async fn running(pool: &PgPool, limit: i64) -> (i64, User, Uuid) {
    let (uid, user) = new_user(pool).await;
    let run = create_run(pool, uid, "q", RunMode::Quick, limit)
        .await
        .unwrap()
        .unwrap();
    let caps = CapsConfig {
        global_monthly_paise: i64::MAX / 4,
    };
    let c = reserve(pool, &user, &caps, limit, run, "run")
        .await
        .unwrap();
    assert!(matches!(c, CapCheck::Ok { .. }));
    assert!(
        set_status(pool, run, RunStatus::Running, None)
            .await
            .unwrap()
    );
    (uid, user, run)
}

fn budget(run: Uuid, uid: i64, limit: i64) -> Arc<RunBudget> {
    Arc::new(RunBudget::new(
        run,
        uid,
        limit,
        Instant::now(),
        prices(),
        88.0,
    ))
}

fn no_hooks() -> Streaming {
    Streaming {
        on_attempt: Arc::new(|| {}),
        on_text: Arc::new(|_| {}),
        deadline: None,
    }
}

/// The charge rows of a run: (input, output, paise).
async fn charged(pool: &PgPool, uid: i64) -> Vec<(i32, i32, i64)> {
    sqlx::query_as(
        "SELECT input_tokens, output_tokens, paise FROM incisive.spend \
         WHERE user_id = $1 AND kind = 'charge' ORDER BY id",
    )
    .bind(uid)
    .fetch_all(pool)
    .await
    .unwrap()
}

/// One streamed call that breaks like `b`, `max` tokens of output, limit
/// `limit`: its result, and what the hold was.
async fn broken_call(
    pool: &PgPool,
    uid: i64,
    run: Uuid,
    b: Broken,
    max: u32,
    limit: i64,
) -> (Result<(), StepError>, i64) {
    let llm: Arc<dyn LlmClient> =
        Arc::new(StreamLlm::new(Arc::new(ScriptedLlm::new())).breaking(vec![b]));
    let budget = budget(run, uid, limit);
    let call = LlmCall::new(
        llm,
        budget.clone(),
        pool.clone(),
        CancelFlag::default(),
        MODEL,
        "write",
    )
    .streaming(no_hooks());
    let hold =
        incisive_app::agent::llm::hold_estimate(&budget, MODEL, "s", "u", None, max).unwrap();
    let r = call.text("s", "u", max).await.map(|_| ());
    (r, hold)
}

#[tokio::test]
async fn a_broken_stream_is_charged_the_real_input_and_the_bytes_received() {
    let Some(pool) = pool().await else { return };
    let (uid, _, run) = running(&pool, 100_000).await;
    let b = Broken {
        text: String::new(),
        usage: Some(Usage {
            input_tokens: 1000,
            output_tokens: 3,
            ..Usage::default()
        }),
        output_bytes: 2001,
    };
    let (r, hold) = broken_call(&pool, uid, run, b, 20_000, 100_000).await;
    assert_eq!(r, Err(StepError::StreamBroken));
    let c = charged(&pool, uid).await;
    assert_eq!(c.len(), 1);
    // Rounded up, at two bytes a token.
    assert_eq!((c[0].0, c[0].1), (1000, 1001));
    assert!(c[0].2 <= hold);
    assert!(c[0].2 > 0);
}

/// What the API reported stands, even past the estimate from the bytes.
#[tokio::test]
async fn a_broken_stream_is_never_charged_below_what_the_api_reported() {
    let Some(pool) = pool().await else { return };
    let (uid, _, run) = running(&pool, 100_000).await;
    let b = Broken {
        text: String::new(),
        usage: Some(Usage {
            input_tokens: 1000,
            output_tokens: 700,
            ..Usage::default()
        }),
        output_bytes: 10,
    };
    let (r, _) = broken_call(&pool, uid, run, b, 20_000, 100_000).await;
    assert_eq!(r, Err(StepError::StreamBroken));
    assert_eq!(charged(&pool, uid).await[0].1, 700);
}

/// Bytes that come to more than the hold are lowered to it.
#[tokio::test]
async fn a_broken_stream_is_never_charged_above_the_hold() {
    let Some(pool) = pool().await else { return };
    let (uid, _, run) = running(&pool, 100_000).await;
    let b = Broken {
        text: String::new(),
        // Far more bytes than `max_tokens`: the estimate has to come down.
        usage: Some(Usage {
            input_tokens: 200,
            output_tokens: 1,
            ..Usage::default()
        }),
        output_bytes: 2_000_000,
    };
    let (r, hold) = broken_call(&pool, uid, run, b, 20_000, 100_000).await;
    assert_eq!(r, Err(StepError::StreamBroken));
    let c = charged(&pool, uid).await;
    assert!(c[0].2 <= hold, "{} > hold {hold}", c[0].2);
    assert!(c[0].1 < 20_000, "lowered below max_tokens: {}", c[0].1);
    assert!(c[0].1 > 1000, "as near the hold as fits: {}", c[0].1);
}

/// The real input and reported output always stand, as on every call, even
/// past the hold (only the estimate is lowered).
#[tokio::test]
async fn real_usage_past_the_hold_is_booked_in_full() {
    let Some(pool) = pool().await else { return };
    let (uid, _, run) = running(&pool, 100_000).await;
    let b = Broken {
        text: String::new(),
        usage: Some(Usage {
            input_tokens: 5_000_000,
            output_tokens: 42,
            ..Usage::default()
        }),
        output_bytes: 2_000_000,
    };
    let (r, hold) = broken_call(&pool, uid, run, b, 20_000, 100_000).await;
    assert_eq!(r, Err(StepError::StreamBroken));
    let c = charged(&pool, uid).await;
    assert_eq!((c[0].0, c[0].1), (5_000_000, 42));
    assert!(c[0].2 > hold);
}

/// Before `message_start` nothing is known: the estimate, as a timeout today.
#[tokio::test]
async fn a_stream_broken_before_its_first_usage_is_charged_the_estimate() {
    let Some(pool) = pool().await else { return };
    let (uid, _, run) = running(&pool, 100_000).await;
    let b = Broken {
        text: String::new(),
        usage: None,
        output_bytes: 0,
    };
    let (r, hold) = broken_call(&pool, uid, run, b, 20_000, 100_000).await;
    assert_eq!(r, Err(StepError::StreamBroken));
    assert_eq!(charged(&pool, uid).await[0].2, hold);
}

fn ctx(
    st: &AppState,
    llm: Arc<dyn LlmClient>,
    user: User,
    run: Uuid,
    budget: Arc<RunBudget>,
) -> RunContext {
    RunContext {
        state: st.clone(),
        llm: agent_llm(llm, MODEL),
        lit: st.lit.clone(),
        run_id: run,
        user,
        question: "q".into(),
        mode: RunMode::Quick,
        settings: DepthSettings::QUICK,
        budget,
        cancel: CancelFlag::default(),
    }
}

#[tokio::test]
async fn no_time_for_a_retry_is_the_failure_path_with_one_call() {
    let Some(pool) = pool().await else { return };
    let (uid, user, run) = running(&pool, 100_000).await;
    let llm =
        Arc::new(StreamLlm::new(Arc::new(ScriptedLlm::new())).breaking(vec![broken("{", 100)]));
    // A time limit shorter than the writer's minimum.
    let budget = Arc::new(
        RunBudget::new(run, uid, 100_000, Instant::now(), prices(), 88.0).with_time_limit(60),
    );
    let st = state(pool.clone(), true);
    let c = ctx(&st, llm.clone(), user, run, budget);
    let r = writer_v3::write_counted(&c, &c.llm, "s", "u").await;
    assert!(
        matches!(r, Err(WriteFailure::Step(StepError::Llm))),
        "{:?}",
        r.err()
    );
    assert_eq!(llm.streamed.lock().unwrap().len(), 1);
    assert_eq!(charged(&pool, uid).await.len(), 1);
}

#[tokio::test]
async fn no_money_for_a_retry_is_the_failure_path_with_one_call() {
    let Some(pool) = pool().await else { return };
    let (uid, user, run) = running(&pool, 100_000).await;
    let llm =
        Arc::new(StreamLlm::new(Arc::new(ScriptedLlm::new())).breaking(vec![broken("{", 100)]));
    // Exactly one hold's room: after the first charge the second does not fit.
    let probe = budget(run, uid, 0);
    let hold = incisive_app::agent::llm::hold_estimate(
        &probe,
        MODEL,
        "s",
        "u",
        Some(&incisive_app::agent::llm::OutputSchema::new(
            writer_v3::writer_schema(),
        )),
        DepthSettings::QUICK.writer_max_tokens,
    )
    .unwrap();
    let b = Arc::new(
        RunBudget::new(run, uid, hold, Instant::now(), prices(), 88.0).with_time_limit(600),
    );
    let st = state(pool.clone(), true);
    let c = ctx(&st, llm.clone(), user, run, b);
    let r = writer_v3::write_counted(&c, &c.llm, "s", "u").await;
    assert!(
        matches!(r, Err(WriteFailure::Step(StepError::Llm))),
        "{:?}",
        r.err()
    );
    assert_eq!(llm.streamed.lock().unwrap().len(), 1);
}

// ---------- the drafts ----------

async fn draft(pool: &PgPool, run: Uuid, part: Part) -> Option<String> {
    drafts::get(pool, run, part).await.unwrap()
}

#[tokio::test]
async fn a_draft_is_stored_only_while_its_writer_is_live() {
    let Some(pool) = pool().await else { return };
    let (_, _, run) = running(&pool, 1000).await;
    assert!(
        drafts::upsert(&pool, run, Part::Answer, "{\"a\"")
            .await
            .unwrap()
    );
    assert!(
        drafts::upsert(&pool, run, Part::Answer, "{\"a\":1")
            .await
            .unwrap()
    );
    assert_eq!(
        draft(&pool, run, Part::Answer).await.as_deref(),
        Some("{\"a\":1")
    );
    let chars: i32 = sqlx::query_scalar(
        "SELECT chars FROM incisive.run_drafts WHERE run_id = $1 AND part = 'answer'",
    )
    .bind(run)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(chars, 6);
    // No pending opinion row: nothing stored.
    assert!(
        !drafts::upsert(&pool, run, Part::Opinion, "{")
            .await
            .unwrap()
    );
    // After the run ended a late write stores nothing.
    assert!(set_status(&pool, run, RunStatus::Done, None).await.unwrap());
    drafts::clear(&pool, run, Part::Answer).await;
    assert!(
        !drafts::upsert(&pool, run, Part::Answer, "late")
            .await
            .unwrap()
    );
    assert_eq!(draft(&pool, run, Part::Answer).await, None);
}

/// A write racing the run's end: it waits for the status to commit, then finds
/// the run no longer running, so the clear that follows leaves nothing behind.
#[tokio::test]
async fn a_late_write_racing_the_final_status_stores_nothing() {
    let Some(pool) = pool().await else { return };
    let (_, _, run) = running(&pool, 1000).await;
    let mut tx = pool.begin().await.unwrap();
    sqlx::query("UPDATE incisive.runs SET status = 'done' WHERE id = $1")
        .bind(run)
        .execute(&mut *tx)
        .await
        .unwrap();
    let late = {
        let pool = pool.clone();
        tokio::spawn(async move { drafts::upsert(&pool, run, Part::Answer, "late").await })
    };
    tokio::time::sleep(Duration::from_millis(300)).await;
    tx.commit().await.unwrap();
    drafts::clear(&pool, run, Part::Answer).await;
    assert!(
        !late.await.unwrap().unwrap(),
        "it must see the final status"
    );
    assert_eq!(draft(&pool, run, Part::Answer).await, None);
}

/// The same for the opinion.
#[tokio::test]
async fn a_late_opinion_write_racing_its_row_finishing_stores_nothing() {
    let Some(pool) = pool().await else { return };
    let (_, _, run) = running(&pool, 1000).await;
    assert!(set_status(&pool, run, RunStatus::Done, None).await.unwrap());
    assert!(insert_pending(&pool, run, Uuid::new_v4()).await.unwrap());
    let mut tx = pool.begin().await.unwrap();
    sqlx::query("UPDATE incisive.run_temporal SET status = 'failed' WHERE run_id = $1")
        .bind(run)
        .execute(&mut *tx)
        .await
        .unwrap();
    let late = {
        let pool = pool.clone();
        tokio::spawn(async move { drafts::upsert(&pool, run, Part::Opinion, "late").await })
    };
    tokio::time::sleep(Duration::from_millis(300)).await;
    tx.commit().await.unwrap();
    drafts::clear(&pool, run, Part::Opinion).await;
    assert!(
        !late.await.unwrap().unwrap(),
        "it must see the final status"
    );
    assert_eq!(draft(&pool, run, Part::Opinion).await, None);
}

#[tokio::test]
async fn deleting_the_run_cascades_to_its_draft() {
    let Some(pool) = pool().await else { return };
    let (uid, _, run) = running(&pool, 1000).await;
    assert!(drafts::upsert(&pool, run, Part::Answer, "{").await.unwrap());
    assert!(delete_run(&pool, uid, run).await.unwrap());
    assert_eq!(draft(&pool, run, Part::Answer).await, None);
}

#[tokio::test]
async fn recovery_sweeps_drafts_whose_writer_has_ended() {
    let Some(pool) = pool().await else { return };
    // Ended run: swept. Live run: kept. Finished opinion: swept.
    let (_, _, ended) = running(&pool, 1000).await;
    assert!(
        drafts::upsert(&pool, ended, Part::Answer, "{")
            .await
            .unwrap()
    );
    assert!(
        set_status(&pool, ended, RunStatus::Failed, Some("x"))
            .await
            .unwrap()
    );
    let (_, _, live) = running(&pool, 1000).await;
    assert!(
        drafts::upsert(&pool, live, Part::Answer, "{")
            .await
            .unwrap()
    );
    sqlx::query("UPDATE incisive.runs SET heartbeat_at = now() WHERE id = $1")
        .bind(live)
        .execute(&pool)
        .await
        .unwrap();
    let (_, _, op) = running(&pool, 1000).await;
    assert!(set_status(&pool, op, RunStatus::Done, None).await.unwrap());
    assert!(insert_pending(&pool, op, Uuid::new_v4()).await.unwrap());
    assert!(drafts::upsert(&pool, op, Part::Opinion, "{").await.unwrap());
    sqlx::query("UPDATE incisive.run_temporal SET status = 'failed' WHERE run_id = $1")
        .bind(op)
        .execute(&pool)
        .await
        .unwrap();
    drafts::sweep(&pool).await.unwrap();
    assert_eq!(draft(&pool, ended, Part::Answer).await, None);
    assert_eq!(draft(&pool, live, Part::Answer).await.as_deref(), Some("{"));
    assert_eq!(draft(&pool, op, Part::Opinion).await, None);
    // Stale by age, however its row reads.
    sqlx::query(
        "UPDATE incisive.run_drafts SET updated_at = now() - interval '2 hours' WHERE run_id = $1",
    )
    .bind(live)
    .execute(&pool)
    .await
    .unwrap();
    drafts::sweep(&pool).await.unwrap();
    assert_eq!(draft(&pool, live, Part::Answer).await, None);
}

/// Recovery interrupting an abandoned run removes its draft too.
#[tokio::test]
async fn an_interrupted_runs_draft_goes_at_recovery() {
    let Some(pool) = pool().await else { return };
    let (_, _, run) = running(&pool, 1000).await;
    assert!(drafts::upsert(&pool, run, Part::Answer, "{").await.unwrap());
    sqlx::query("UPDATE incisive.runs SET heartbeat_at = now() - interval '1 hour' WHERE id = $1")
        .bind(run)
        .execute(&pool)
        .await
        .unwrap();
    recover_stale_runs(&pool).await.unwrap();
    assert_eq!(draft(&pool, run, Part::Answer).await, None);
}

/// A finished opinion takes its draft with it, whatever the status.
#[tokio::test]
async fn the_opinions_draft_goes_when_its_row_finishes() {
    let Some(pool) = pool().await else { return };
    for status in [JobStatus::Done, JobStatus::Failed] {
        let (_, _, run) = running(&pool, 1000).await;
        assert!(set_status(&pool, run, RunStatus::Done, None).await.unwrap());
        assert!(insert_pending(&pool, run, Uuid::new_v4()).await.unwrap());
        assert!(
            drafts::upsert(&pool, run, Part::Opinion, "{")
                .await
                .unwrap()
        );
        let f = temporal::Finished::default();
        assert!(temporal::finish(&pool, run, status, &f).await.unwrap());
        assert_eq!(draft(&pool, run, Part::Opinion).await, None);
    }
}

/// Run through a real streamed call: writes land, no faster than the interval
/// allows, and the last text is there when the call ends.
#[tokio::test]
async fn the_hooks_keep_the_draft_and_write_the_last_text_at_the_end() {
    let Some(pool) = pool().await else { return };
    let (_, _, run) = running(&pool, 100_000).await;
    let s = drafts::streaming_every(
        pool.clone(),
        run,
        Part::Answer,
        Instant::now() + Duration::from_secs(60),
        Duration::from_millis(40),
    );
    (s.on_attempt)();
    let mut text = String::new();
    for i in 0..30 {
        text.push_str(&format!("{i},"));
        (s.on_text)(&text);
    }
    // Throttled: far fewer rows' worth of writes than pieces; the first went
    // out at once, and the rest only on the interval.
    let mut seen = None;
    for _ in 0..200 {
        seen = draft(&pool, run, Part::Answer).await;
        if seen.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert!(seen.is_some(), "the first write");
    drop(s);
    let mut last = None;
    for _ in 0..200 {
        last = draft(&pool, run, Part::Answer).await;
        if last.as_deref() == Some(text.as_str()) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert_eq!(last.as_deref(), Some(text.as_str()), "the last write");
}

/// A retry starts again: the old text goes, and shorter text can replace it.
#[tokio::test]
async fn a_retry_clears_the_draft_and_starts_again() {
    let Some(pool) = pool().await else { return };
    let (_, _, run) = running(&pool, 100_000).await;
    let s = drafts::streaming_every(
        pool.clone(),
        run,
        Part::Answer,
        Instant::now() + Duration::from_secs(60),
        Duration::from_millis(1),
    );
    (s.on_attempt)();
    (s.on_text)("first attempt, quite long");
    let wait_for = async |want: Option<&str>| {
        for _ in 0..400 {
            if draft(&pool, run, Part::Answer).await.as_deref() == want {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        false
    };
    assert!(wait_for(Some("first attempt, quite long")).await);
    (s.on_attempt)();
    assert!(wait_for(None).await, "cleared");
    (s.on_text)("second");
    assert!(wait_for(Some("second")).await);
}

/// A database that cannot be reached costs the draft only.
#[tokio::test]
async fn a_failed_draft_write_leaves_the_call_and_its_charge_alone() {
    let Some(pool) = pool().await else { return };
    let (uid, _, run) = running(&pool, 100_000).await;
    let dead = lazy_pool(&SecretString::from(
        "postgres://nobody:x@127.0.0.1:1/none".to_string(),
    ))
    .unwrap();
    let hooks = drafts::streaming_every(
        dead,
        run,
        Part::Answer,
        Instant::now() + Duration::from_secs(60),
        Duration::from_millis(1),
    );
    let llm: Arc<dyn LlmClient> =
        Arc::new(StreamLlm::new(Arc::new(ScriptedLlm::new().reply_text(
            "whole reply",
            Usage {
                input_tokens: 10,
                output_tokens: 10,
                ..Usage::default()
            },
        ))));
    let call = LlmCall::new(
        llm,
        budget(run, uid, 100_000),
        pool.clone(),
        CancelFlag::default(),
        MODEL,
        "write",
    )
    .streaming(hooks);
    assert_eq!(call.text("s", "u", 100).await.unwrap(), "whole reply");
    assert_eq!(charged(&pool, uid).await.len(), 1);
    assert_eq!(draft(&pool, run, Part::Answer).await, None);
}

// ---------- what the API gives ----------

#[tokio::test]
async fn the_run_api_gives_the_draft_to_its_owner_only_while_writing() {
    let Some(pool) = pool().await else { return };
    let (uid, user, run) = running(&pool, 1000).await;
    let (_, other) = new_user(&pool).await;
    assert!(
        drafts::upsert(&pool, run, Part::Answer, "{\"bottom")
            .await
            .unwrap()
    );
    let st = state(pool.clone(), true);
    let v = api::get_view(&st, &user, run).await.unwrap();
    assert_eq!(v.draft.as_deref(), Some("{\"bottom"));
    assert!(matches!(
        api::get_view(&st, &other, run).await,
        Err(api::RunApiError::NotFound)
    ));
    // With `STREAM=off`: never.
    let off = state(pool.clone(), false);
    assert_eq!(api::get_view(&off, &user, run).await.unwrap().draft, None);
    let wire = serde_json::to_value(api::get_view(&off, &user, run).await.unwrap()).unwrap();
    assert!(wire.get("draft").is_none(), "absent from the wire");
    // Once the answer is saved the draft is no longer shown.
    assert!(
        save_answer(&pool, run, &json!({"version":3}))
            .await
            .unwrap()
    );
    assert_eq!(api::get_view(&st, &user, run).await.unwrap().draft, None);
    let _ = uid;
}

#[tokio::test]
async fn the_opinion_api_gives_the_draft_to_its_owner_only_while_pending() {
    let Some(pool) = pool().await else { return };
    let (_, user, run) = running(&pool, 1000).await;
    let (_, other) = new_user(&pool).await;
    assert!(set_status(&pool, run, RunStatus::Done, None).await.unwrap());
    assert!(insert_pending(&pool, run, Uuid::new_v4()).await.unwrap());
    assert!(
        drafts::upsert(&pool, run, Part::Opinion, "{\"at")
            .await
            .unwrap()
    );
    let st = state(pool.clone(), true);
    let v = api::temporal_view(&st, &user, run).await.unwrap().unwrap();
    assert_eq!(v.draft.as_deref(), Some("{\"at"));
    assert!(v.streaming);
    assert!(matches!(
        api::temporal_view(&st, &other, run).await,
        Err(api::RunApiError::NotFound)
    ));
    let off = state(pool.clone(), false);
    let v = api::temporal_view(&off, &user, run).await.unwrap().unwrap();
    assert!(!v.streaming);
    assert_eq!(v.draft, None, "with STREAM=off: never");
    // Finished: gone, and the page need not hurry.
    let f = temporal::Finished {
        reason: Some("x"),
        ..Default::default()
    };
    assert!(
        temporal::finish(&pool, run, JobStatus::Failed, &f)
            .await
            .unwrap()
    );
    let v = api::temporal_view(&st, &user, run).await.unwrap().unwrap();
    assert_eq!(v.draft, None);
    assert!(!v.streaming);
    let wire = serde_json::to_value(&v).unwrap();
    assert!(wire.get("draft").is_none() && wire.get("streaming").is_none());
}
