//! Temporal T2/T3 with the database (`TEST_DATABASE_URL`): migration 0012,
//! the store round trip and scoping, stale-pending recovery, the job's
//! outcomes with the scripted LLM (success, schema refusal, refusal, API
//! error, timeout, money short, cap blocked), and the trigger in the worker,
//! with the run's own row byte-identical with the job on and off.

use std::sync::Arc;
use std::time::Duration;

use claude_client::{ClaudeError, StopReason, Usage};
use incisive_app::agent::llm::{AgentModels, ScriptedLlm};
use incisive_app::agent::temporal_writer::{self as writer, BoardFlags, TemporalSettings};
use incisive_app::agent::testing::{agent_llm, offline_literature, placeholder_pipeline};
use incisive_app::agent::{AgentLlm, PipelineFuture, PipelineOutcome, RunContext};
use incisive_app::db::{lazy_pool, migrate};
use incisive_app::gate::OfflineClassifier;
use incisive_app::identity::{IapVerifier, JwkSource};
use incisive_app::runs::temporal::{
    self as job, Finished, JobStatus, finish, get_for_user, insert_pending, recover_stale, run_job,
    should_run,
};
use incisive_app::runs::worker::WorkerHandle;
use incisive_app::runs::{RunMode, RunStatus, RunsConfig, store};
use incisive_app::server::AppState;
use incisive_app::spend::{CapCheck, CapsConfig, Prices, reserve};
use secrecy::SecretString;
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

const MODEL: &str = "m-test";

fn hernia_body() -> Value {
    serde_json::from_str(include_str!("fixtures/temporal/full/hernia.json")).unwrap()
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

fn prices() -> Prices {
    Prices::from_json(r#"{"m-test": {"input": 1.0, "output": 5.0}}"#).unwrap()
}

fn state(pool: PgPool, llm: Arc<AgentLlm>, temporal: TemporalSettings, global: i64) -> AppState {
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
            global_monthly_paise: global,
        },
        gate_model: MODEL.into(),
        dev_mode: true,
        runs: RunsConfig::default(),
        quick_engine: incisive_app::runs::QuickEngine::Classic,
        full_engine: incisive_app::runs::Engine::Classic,
        completeness: false,
        temporal,
        temporal_client: None,
        memory: incisive_app::memory::settings::MemorySettings::default(),
        worker: WorkerHandle::with_pipeline(placeholder_pipeline),
        llm: Some(llm),
        lit: offline_literature(),
    }
}

const BIG: i64 = i64::MAX / 4;

async fn new_user(pool: &PgPool, daily_cap: i64) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO incisive.users (email, display_name, role, daily_cap_paise) \
         VALUES ($1, 'T', 'surgeon', $2) RETURNING id",
    )
    .bind(format!("temporal-{}@example.com", Uuid::new_v4()))
    .bind(daily_cap)
    .fetch_one(pool)
    .await
    .unwrap()
}

/// A finished Full run with the hernia answer saved (as the worker leaves
/// it), and no reservation.
async fn finished_run(pool: &PgPool, uid: i64) -> Uuid {
    let run = store::create_run(pool, uid, "Q", RunMode::Focused, 30_000)
        .await
        .unwrap()
        .unwrap();
    assert!(
        store::set_status(pool, run, RunStatus::Running, None)
            .await
            .unwrap()
    );
    assert!(store::save_answer(pool, run, &hernia_body()).await.unwrap());
    assert!(
        store::set_status(pool, run, RunStatus::Done, None)
            .await
            .unwrap()
    );
    run
}

/// The run's whole row and its answer, as text (for byte comparisons).
async fn run_snapshot(pool: &PgPool, run: Uuid) -> (String, String) {
    let row: String =
        sqlx::query_scalar("SELECT row_to_json(r)::text FROM incisive.runs r WHERE r.id = $1")
            .bind(run)
            .fetch_one(pool)
            .await
            .unwrap();
    let answer: String = sqlx::query_scalar(
        "SELECT row_to_json(a)::text FROM incisive.run_answers a WHERE a.run_id = $1",
    )
    .bind(run)
    .fetch_one(pool)
    .await
    .unwrap();
    (row, answer)
}

/// A board that reads back and passes the shape check.
fn board_json() -> Value {
    let round =
        |n: i64, sp: &str| json!({"round": n, "speaker": sp, "title": "t", "points": ["p"]});
    json!({
        "topic": "Robotic vs laparoscopic inguinal hernia repair",
        "at_a_glance": {"original_quote": "q", "expert": "Equal for the patient.", "today": "Laparoscopic for routine."},
        "eras": [
            {"era_id": "e-2016-2020", "going_in": "g", "showed": "Operative time 75.5 vs 40.5 [1].", "knowledge_after": "k"},
            {"era_id": "e-2021-2026", "going_in": "g", "showed": "s", "knowledge_after": "k"}
        ],
        "linked_note": "RIVAL 2020 and 2023 are one trial [1, 2].",
        "debate": [round(1, "researcher"), round(2, "researcher"), round(3, "skeptic"), round(4, "researcher"), round(5, "skeptic")],
        "scoreboard": [{"code": "H1", "name": "Experience", "for": "f", "against": "a", "status": "leading"}],
        "ledger": {"label_mode": "treatment", "rows": [{"question": "Slower?", "cells": ["Unknown", "Yes", "Less"]}]},
        "opinion": {"verdict": "v", "why": "w", "curve_reading": "c", "did_not_improve": "Pain."},
        "decisions": [{"scenario": "Routine", "choice": "Laparoscopic", "reason": "Equal outcomes [3]."}],
        "triggers": [{"if": "i", "then": "t"}],
        "settle": ["A blinded multicentre RCT."]
    })
}

fn usage() -> Usage {
    Usage {
        input_tokens: 30_000,
        output_tokens: 6_000,
        ..Usage::default()
    }
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

async fn reservations(pool: &PgPool, uid: i64) -> i64 {
    sqlx::query_scalar(
        "SELECT count(*) FROM incisive.spend WHERE user_id = $1 AND kind = 'reservation'",
    )
    .bind(uid)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn last_event(pool: &PgPool, run: Uuid) -> (String, Value) {
    sqlx::query_as(
        "SELECT event, detail FROM incisive.audit WHERE request_id = $1 \
         AND event LIKE 'temporal_%' ORDER BY seq DESC LIMIT 1",
    )
    .bind(run)
    .fetch_one(pool)
    .await
    .unwrap()
}

/// Run the job on a finished run with `llm` and return (status, view).
async fn job_with(
    pool: &PgPool,
    uid: i64,
    llm: ScriptedLlm,
    settings: TemporalSettings,
    global: i64,
) -> (
    Uuid,
    JobStatus,
    incisive_app::runs::TemporalView,
    Arc<ScriptedLlm>,
) {
    let run = finished_run(pool, uid).await;
    let before = run_snapshot(pool, run).await;
    let llm = Arc::new(llm);
    let st = state(
        pool.clone(),
        agent_llm(llm.clone(), MODEL),
        settings,
        global,
    );
    let req = Uuid::new_v4();
    assert!(insert_pending(pool, run, req).await.unwrap());
    let status = run_job(&st, st.llm.as_ref().unwrap(), run, uid, req, &hernia_body()).await;
    let view = get_for_user(pool, uid, run)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(view.status, status.as_str());
    // The run is never touched.
    assert_eq!(run_snapshot(pool, run).await, before, "run row changed");
    // No reservation is left behind (a timed-out call keeps its hold until
    // its late charge; the caller waits for that).
    (run, status, view, llm)
}

// ---------- migration and store ----------

#[tokio::test]
async fn migration_store_round_trip_and_scoping() {
    let Some(pool) = connect().await else { return };
    let uid = new_user(&pool, 100_000).await;
    let other = new_user(&pool, 100_000).await;
    let run = finished_run(&pool, uid).await;
    // No row yet: theirs, not run.
    assert_eq!(get_for_user(&pool, uid, run).await.unwrap(), Some(None));
    assert!(insert_pending(&pool, run, Uuid::new_v4()).await.unwrap());
    assert!(
        !insert_pending(&pool, run, Uuid::new_v4()).await.unwrap(),
        "never twice"
    );
    let v = get_for_user(&pool, uid, run)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!((v.status.as_str(), v.board.is_none()), ("pending", true));
    // Someone else's run reads as missing.
    assert_eq!(get_for_user(&pool, other, run).await.unwrap(), None);
    let f = Finished {
        reason: None,
        topic: Some("Topic A".into()),
        evidence_year: Some(2026),
        table: Some(json!({"t": 1})),
        board: Some(json!({"b": 2})),
        flags: Some(json!({"f": 3})),
        cost_paise: 1234,
    };
    assert!(finish(&pool, run, JobStatus::Done, &f).await.unwrap());
    assert!(
        !finish(&pool, run, JobStatus::Failed, &f).await.unwrap(),
        "only while pending"
    );
    let v = get_for_user(&pool, uid, run)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(v.status, "done");
    assert_eq!(v.topic.as_deref(), Some("Topic A"));
    assert_eq!(v.evidence_year, Some(2026));
    assert_eq!(
        (v.table, v.board, v.flags),
        (
            Some(json!({"t": 1})),
            Some(json!({"b": 2})),
            Some(json!({"f": 3}))
        )
    );
    // Findable by topic and year (the indexes exist).
    let idx: Vec<String> = sqlx::query_scalar(
        "SELECT indexname::text FROM pg_indexes WHERE tablename = 'run_temporal' ORDER BY 1",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert!(idx.contains(&"run_temporal_topic_idx".into()), "{idx:?}");
    assert!(
        idx.contains(&"run_temporal_evidence_year_idx".into()),
        "{idx:?}"
    );
    // The status check refuses an unknown status.
    assert!(
        sqlx::query("UPDATE incisive.run_temporal SET status = 'odd' WHERE run_id = $1")
            .bind(run)
            .execute(&pool)
            .await
            .is_err()
    );
    // Delete cascades.
    assert!(store::delete_run(&pool, uid, run).await.unwrap());
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM incisive.run_temporal WHERE run_id = $1")
        .bind(run)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(n, 0);
}

#[tokio::test]
async fn stale_pending_is_failed_and_its_reservation_released() {
    let Some(pool) = connect().await else { return };
    let uid = new_user(&pool, 100_000).await;
    let user = incisive_app::identity::find_user_by_id(&pool, uid)
        .await
        .unwrap()
        .unwrap();
    let (old, fresh) = (
        finished_run(&pool, uid).await,
        finished_run(&pool, uid).await,
    );
    let (req_old, req_fresh) = (Uuid::new_v4(), Uuid::new_v4());
    for (run, req) in [(old, req_old), (fresh, req_fresh)] {
        assert!(insert_pending(&pool, run, req).await.unwrap());
        let c = reserve(
            &pool,
            &user,
            &CapsConfig {
                global_monthly_paise: BIG,
            },
            500,
            req,
            job::PURPOSE,
        )
        .await
        .unwrap();
        assert!(matches!(c, CapCheck::Ok { .. }));
    }
    // A leaked reservation with no row at all.
    let leaked = Uuid::new_v4();
    reserve(
        &pool,
        &user,
        &CapsConfig {
            global_monthly_paise: BIG,
        },
        300,
        leaked,
        job::PURPOSE,
    )
    .await
    .unwrap();
    for (sql, id) in [
        (
            "UPDATE incisive.run_temporal SET created_at = now() - interval '11 minutes' WHERE run_id = $1",
            old,
        ),
        (
            "UPDATE incisive.spend SET at = now() - interval '11 minutes' WHERE request_id = $1",
            req_old,
        ),
        (
            "UPDATE incisive.spend SET at = now() - interval '11 minutes' WHERE request_id = $1",
            leaked,
        ),
    ] {
        sqlx::query(sql).bind(id).execute(&pool).await.unwrap();
    }
    let before_old = run_snapshot(&pool, old).await;
    recover_stale(&pool).await.unwrap();
    let v = get_for_user(&pool, uid, old)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(
        (v.status.as_str(), v.reason.as_deref()),
        ("failed", Some("stale"))
    );
    let v = get_for_user(&pool, uid, fresh)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(v.status, "pending", "a fresh job is left alone");
    assert_eq!(
        reservations(&pool, uid).await,
        1,
        "only the fresh job's reservation is kept"
    );
    assert_eq!(run_snapshot(&pool, old).await, before_old);
    assert_eq!(last_event(&pool, old).await.0, "temporal_failed");
}

// ---------- the job, with the scripted LLM ----------

#[tokio::test]
async fn job_success_stores_board_table_flags_and_charges_the_user() {
    let Some(pool) = connect().await else { return };
    let uid = new_user(&pool, 100_000).await;
    let llm = ScriptedLlm::new().reply_json(board_json(), usage());
    let (run, status, v, llm) = job_with(&pool, uid, llm, TemporalSettings::on(), BIG).await;
    assert_eq!(status, JobStatus::Done);
    assert_eq!(v.reason, None);
    assert_eq!(
        v.topic.as_deref(),
        Some("Robotic vs laparoscopic inguinal hernia repair")
    );
    assert_eq!(v.evidence_year, Some(2026));
    assert!(v.table.as_ref().unwrap()["eras"].is_array());
    assert_eq!(
        v.board.as_ref().unwrap()["eras"][0]["era_id"],
        "e-2016-2020"
    );
    let flags: BoardFlags = serde_json::from_value(v.flags.clone().unwrap()).unwrap();
    assert!(
        flags.shape.is_empty() && flags.unknown_ids.is_empty(),
        "{flags:?}"
    );
    // One call: Opus-config model, the board schema, max_tokens and effort.
    let rq = llm.requests();
    assert_eq!(rq.len(), 1);
    assert_eq!(rq[0].max_tokens, writer::DEFAULT_MAX_TOKENS);
    assert_eq!(rq[0].model, MODEL);
    let oc = rq[0].output_config.as_ref().unwrap();
    assert_eq!(oc.effort, Some(writer::EFFORT));
    assert!(rq[0].system.as_ref().unwrap().contains("EXPERT WRITER"));
    // Charged to the user (30k in at $1/M + 6k out at $5/M = $0.06 = ₹5.28),
    // under its own request, the reservation released.
    let ch = charges(&pool, uid).await;
    assert_eq!(ch.len(), 1);
    assert_eq!(ch[0].0, 528);
    assert!(ch[0].1.starts_with("run:"), "booked by the run budget path");
    assert!(
        !ch[0].1.contains(&run.to_string()),
        "never under the run's id"
    );
    let cost: i64 =
        sqlx::query_scalar("SELECT cost_paise FROM incisive.run_temporal WHERE run_id = $1")
            .bind(run)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(cost, 528);
    assert_eq!(reservations(&pool, uid).await, 0);
    let (ev, detail) = last_event(&pool, run).await;
    assert_eq!(ev, "temporal_done");
    assert_eq!(detail["cost_paise"], 528);
    assert!(
        !detail.to_string().contains("hernia"),
        "no text in the audit"
    );
}

#[tokio::test]
async fn job_schema_refused_by_the_api_fails_with_nothing_charged() {
    let Some(pool) = connect().await else { return };
    let uid = new_user(&pool, 100_000).await;
    let llm = ScriptedLlm::new().fail(ClaudeError::Api {
        status: 400,
        error_type: "invalid_request_error".into(),
        message: "schema too complex".into(),
    });
    let (run, status, v, _) = job_with(&pool, uid, llm, TemporalSettings::on(), BIG).await;
    assert_eq!(
        (status, v.reason.as_deref()),
        (JobStatus::Failed, Some("api error"))
    );
    assert!(v.board.is_none() && v.table.is_some());
    assert!(
        charges(&pool, uid).await.is_empty(),
        "an HTTP error is not billed"
    );
    assert_eq!(reservations(&pool, uid).await, 0);
    assert_eq!(last_event(&pool, run).await.0, "temporal_failed");
}

#[tokio::test]
async fn job_refusal_and_unusable_replies_fail_and_are_charged() {
    let Some(pool) = connect().await else { return };
    let uid = new_user(&pool, 100_000).await;
    let llm = ScriptedLlm::new().reply_stop("", StopReason::Refusal, usage());
    let (_, status, v, _) = job_with(&pool, uid, llm, TemporalSettings::on(), BIG).await;
    assert_eq!(
        (status, v.reason.as_deref()),
        (JobStatus::Failed, Some("api error"))
    );
    let llm = ScriptedLlm::new().reply_stop("{\"topic\": \"cut", StopReason::MaxTokens, usage());
    let (_, status, v, _) = job_with(&pool, uid, llm, TemporalSettings::on(), BIG).await;
    assert_eq!(
        (status, v.reason.as_deref()),
        (JobStatus::Failed, Some("truncated"))
    );
    let llm = ScriptedLlm::new().reply_text("not json", usage());
    let (_, status, v, llm) = job_with(&pool, uid, llm, TemporalSettings::on(), BIG).await;
    assert_eq!(
        (status, v.reason.as_deref()),
        (JobStatus::Failed, Some("unusable reply"))
    );
    assert_eq!(llm.requests().len(), 1, "no retry");
    assert_eq!(charges(&pool, uid).await.len(), 3, "every reply is charged");
    assert_eq!(reservations(&pool, uid).await, 0);
}

#[tokio::test]
async fn job_api_error_fails() {
    let Some(pool) = connect().await else { return };
    let uid = new_user(&pool, 100_000).await;
    let llm = ScriptedLlm::new().fail(ClaudeError::Overloaded);
    let (run, status, v, _) = job_with(&pool, uid, llm, TemporalSettings::on(), BIG).await;
    assert_eq!(
        (status, v.reason.as_deref()),
        (JobStatus::Failed, Some("api error"))
    );
    assert_eq!(last_event(&pool, run).await.1["reason"], "api error");
}

#[tokio::test]
async fn job_timeout_fails_and_the_late_charge_is_still_booked() {
    let Some(pool) = connect().await else { return };
    let uid = new_user(&pool, 100_000).await;
    let llm = ScriptedLlm::new()
        .delay(Duration::from_millis(600))
        .reply_json(board_json(), usage());
    let settings = TemporalSettings {
        time_limit: Duration::from_millis(100),
        ..TemporalSettings::on()
    };
    let (run, status, v, _) = job_with(&pool, uid, llm, settings, BIG).await;
    assert_eq!(
        (status, v.reason.as_deref()),
        (JobStatus::Failed, Some("time"))
    );
    assert!(v.board.is_none());
    let cost_now: i64 =
        sqlx::query_scalar("SELECT cost_paise FROM incisive.run_temporal WHERE run_id = $1")
            .bind(run)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(cost_now, 0, "nothing booked yet when the job gives up");
    // The call finishes in its own task: charged, the kept hold released,
    // and the row's cost brought up to date (review fix 2).
    for _ in 0..300 {
        let cost: i64 =
            sqlx::query_scalar("SELECT cost_paise FROM incisive.run_temporal WHERE run_id = $1")
                .bind(run)
                .fetch_one(&pool)
                .await
                .unwrap();
        if charges(&pool, uid).await.len() == 1
            && reservations(&pool, uid).await == 0
            && cost == 528
        {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("late charge not booked, reservation not released or cost not updated");
}

#[tokio::test]
async fn job_money_short_unpriced_model_fails_before_any_call() {
    let Some(pool) = connect().await else { return };
    let uid = new_user(&pool, 100_000).await;
    let run = finished_run(&pool, uid).await;
    let llm = Arc::new(ScriptedLlm::new().reply_json(board_json(), usage()));
    let models = AgentModels {
        temporal: "unpriced".into(),
        ..AgentModels::default()
    };
    let agent = Arc::new(AgentLlm {
        client: llm.clone(),
        models,
    });
    let st = state(pool.clone(), agent.clone(), TemporalSettings::on(), BIG);
    let req = Uuid::new_v4();
    insert_pending(&pool, run, req).await.unwrap();
    let s = run_job(&st, &agent, run, uid, req, &hernia_body()).await;
    assert_eq!(s, JobStatus::Failed);
    let v = get_for_user(&pool, uid, run)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(v.reason.as_deref(), Some("unpriced model"));
    assert!(llm.requests().is_empty());
    assert_eq!(reservations(&pool, uid).await, 0);
}

#[tokio::test]
async fn job_cap_blocked_by_the_users_daily_cap_or_the_global_cap() {
    let Some(pool) = connect().await else { return };
    // Daily cap ₹1: the writer's hold (several rupees) does not fit (D3).
    let uid = new_user(&pool, 100).await;
    let llm = ScriptedLlm::new().reply_json(board_json(), usage());
    let (run, status, v, llm) = job_with(&pool, uid, llm, TemporalSettings::on(), BIG).await;
    assert_eq!(
        (status, v.reason.as_deref()),
        (JobStatus::CapBlocked, Some("limit reached"))
    );
    assert!(llm.requests().is_empty(), "no call");
    assert_eq!(v.status, "cap_blocked");
    assert_eq!(last_event(&pool, run).await.0, "temporal_cap_blocked");
    // Money short globally: the overall monthly budget is spent.
    let uid = new_user(&pool, 100_000).await;
    let llm = ScriptedLlm::new().reply_json(board_json(), usage());
    let (_, status, _, llm) = job_with(&pool, uid, llm, TemporalSettings::on(), 1).await;
    assert_eq!(status, JobStatus::CapBlocked);
    assert!(llm.requests().is_empty());
    assert_eq!(reservations(&pool, uid).await, 0);
}

#[tokio::test]
async fn job_skips_with_fewer_than_two_primary_studies() {
    let Some(pool) = connect().await else { return };
    let uid = new_user(&pool, 100_000).await;
    let run = finished_run(&pool, uid).await;
    let mut body = hernia_body();
    let studies = body["answer"]["studies"].as_array_mut().unwrap();
    studies.truncate(1);
    let llm = Arc::new(ScriptedLlm::new());
    let st = state(
        pool.clone(),
        agent_llm(llm.clone(), MODEL),
        TemporalSettings::on(),
        BIG,
    );
    let req = Uuid::new_v4();
    insert_pending(&pool, run, req).await.unwrap();
    let s = run_job(&st, st.llm.as_ref().unwrap(), run, uid, req, &body).await;
    assert_eq!(s, JobStatus::Skipped);
    let v = get_for_user(&pool, uid, run)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(v.reason.as_deref(), Some("not enough primary studies"));
    assert!(llm.requests().is_empty());
    assert_eq!(reservations(&pool, uid).await, 0);
    assert_eq!(last_event(&pool, run).await.0, "temporal_skipped");
}

// ---------- the trigger ----------

#[test]
fn the_trigger_fires_only_for_full_v3_with_an_answer_and_temporal_on() {
    let body = hernia_body();
    let done = Some(RunStatus::Done);
    assert!(should_run(true, RunMode::Focused, done, &body));
    assert!(should_run(
        true,
        RunMode::Focused,
        Some(RunStatus::Partial),
        &body
    ));
    assert!(
        !should_run(false, RunMode::Focused, done, &body),
        "TEMPORAL=off"
    );
    assert!(!should_run(true, RunMode::Quick, done, &body), "Quick");
    assert!(
        !should_run(true, RunMode::Focused, Some(RunStatus::Failed), &body),
        "failed"
    );
    assert!(
        !should_run(true, RunMode::Focused, None, &body),
        "not saved"
    );
    let mut v2 = body.clone();
    v2["version"] = json!(2);
    assert!(
        !should_run(true, RunMode::Focused, done, &v2),
        "not version 3"
    );
    let mut empty = body.clone();
    empty["answer"] = Value::Null;
    assert!(
        !should_run(true, RunMode::Focused, done, &empty),
        "no written answer"
    );
}

fn full_done(_ctx: &RunContext) -> PipelineFuture<'_> {
    Box::pin(async { PipelineOutcome::Done(hernia_body()) })
}

fn full_failed(_ctx: &RunContext) -> PipelineFuture<'_> {
    Box::pin(async { PipelineOutcome::Failed("llm") })
}

/// A worker run with `pipeline`; returns the run, its row at the moment the
/// worker is done, and the job's row once it has finished (if any).
async fn worker_run(
    pool: &PgPool,
    on: bool,
    mode: RunMode,
    pipeline: incisive_app::runs::worker::PipelineFn,
) -> (
    i64,
    Uuid,
    (String, Option<String>),
    Option<incisive_app::runs::TemporalView>,
    Arc<ScriptedLlm>,
) {
    let uid = new_user(pool, 100_000).await;
    let user = incisive_app::identity::find_user_by_id(pool, uid)
        .await
        .unwrap()
        .unwrap();
    let llm = Arc::new(ScriptedLlm::new().reply_json(board_json(), usage()));
    let settings = if on {
        TemporalSettings::on()
    } else {
        TemporalSettings::default()
    };
    let st = state(pool.clone(), agent_llm(llm.clone(), MODEL), settings, BIG);
    let run = store::create_run(pool, uid, "Q", mode, 30_000)
        .await
        .unwrap()
        .unwrap();
    reserve(pool, &user, &st.caps, 30_000, run, "run")
        .await
        .unwrap();
    st.worker.spawn_with(st.clone(), run, pipeline);
    for _ in 0..500 {
        if !st.worker.is_running(run) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(!st.worker.is_running(run));
    let row: String =
        sqlx::query_scalar("SELECT row_to_json(r)::text FROM incisive.runs r WHERE r.id = $1")
            .bind(run)
            .fetch_one(pool)
            .await
            .unwrap();
    let answer: Option<String> =
        sqlx::query_scalar("SELECT body::text FROM incisive.run_answers WHERE run_id = $1")
            .bind(run)
            .fetch_optional(pool)
            .await
            .unwrap();
    let mut view = None;
    for _ in 0..500 {
        view = get_for_user(pool, uid, run).await.unwrap().unwrap();
        if view.as_ref().is_none_or(|v| v.status != "pending") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    (uid, run, (row, answer), view, llm)
}

/// The run's row without its id and clock values (the fields two runs can
/// differ in), for comparing a run with the job on and off.
fn comparable(row: &str) -> Value {
    let mut v: Value = serde_json::from_str(row).unwrap();
    let o = v.as_object_mut().unwrap();
    for k in [
        "id",
        "user_id",
        "created_at",
        "started_at",
        "finished_at",
        "heartbeat_at",
    ] {
        o.remove(k);
    }
    v
}

#[tokio::test]
async fn the_worker_starts_the_job_after_the_run_and_never_touches_the_run() {
    let Some(pool) = connect().await else { return };
    let (uid_on, on, (row_on, ans_on), view, llm) =
        worker_run(&pool, true, RunMode::Focused, full_done).await;
    let view = view.expect("job ran");
    assert_eq!(view.status, "done");
    assert_eq!(llm.requests().len(), 1);
    // After the job, the run row and answer are byte-identical to what the
    // worker left.
    let after: String =
        sqlx::query_scalar("SELECT row_to_json(r)::text FROM incisive.runs r WHERE r.id = $1")
            .bind(on)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(after, row_on, "run row changed by the job");
    let (uid_off, off, (row_off, ans_off), view_off, llm_off) =
        worker_run(&pool, false, RunMode::Focused, full_done).await;
    assert!(view_off.is_none(), "TEMPORAL=off: no row");
    assert!(llm_off.requests().is_empty());
    // With the job on and off, the run's row (less id and clocks) and its
    // answer bytes are identical; the run's cost holds only the run's spend.
    assert_eq!(comparable(&row_on), comparable(&row_off));
    assert_eq!(ans_on, ans_off);
    assert_eq!(comparable(&row_on)["status"], "done");
    assert_eq!(comparable(&row_on)["cost_paise"], 0);
    // The run's reservation is released either way; the job's charge is the
    // user's, not the run's.
    assert_eq!(reservations(&pool, uid_on).await, 0);
    assert_eq!(reservations(&pool, uid_off).await, 0);
    assert_eq!(charges(&pool, uid_on).await.len(), 1);
    assert!(charges(&pool, uid_off).await.is_empty());
    let _ = (on, off);
}

#[tokio::test]
async fn the_worker_never_starts_the_job_for_quick_or_a_failed_run() {
    let Some(pool) = connect().await else { return };
    let (_, _, _, view, llm) = worker_run(&pool, true, RunMode::Quick, full_done).await;
    assert!(view.is_none() && llm.requests().is_empty(), "Quick");
    let (_, _, (row, ans), view, llm) =
        worker_run(&pool, true, RunMode::Focused, full_failed).await;
    assert!(view.is_none() && llm.requests().is_empty(), "failed");
    assert!(ans.is_none());
    assert_eq!(comparable(&row)["status"], "failed");
}

/// The money reserve on the three stored answers at Opus 5.5 prices
/// ($4 / $20 per million tokens, ₹88): the worst case the job reserves
/// against the user's caps. Printed for the report; bounded so a prompt
/// change that doubles it is noticed.
#[test]
fn the_reserve_on_the_three_fixtures_is_bounded() {
    use incisive_app::runs::budget::RunBudget;
    use incisive_app::temporal::{TemporalInput, build};
    let prices =
        Prices::from_json(r#"{"claude-opus-5-5": {"input": 4.0, "output": 20.0}}"#).unwrap();
    let b = RunBudget::new(
        Uuid::new_v4(),
        1,
        0,
        std::time::Instant::now(),
        prices,
        88.0,
    );
    for name in ["cholecystitis", "hernia", "achalasia"] {
        let p = format!(
            "{}/tests/fixtures/temporal/full/{name}.json",
            env!("CARGO_MANIFEST_DIR")
        );
        let j = std::fs::read_to_string(p).unwrap();
        let body: Value = serde_json::from_str(&j).unwrap();
        let input = TemporalInput::from_json(&j).unwrap();
        let t = build(&input);
        let user = writer::user(&writer::answer_text(&body, &input), &t, &input);
        let hold =
            writer::reserve(&b, "claude-opus-5-5", &user, writer::DEFAULT_MAX_TOKENS).unwrap();
        println!(
            "{name}: prompt {} bytes, primary {}, reserve {hold} paise",
            user.len(),
            job::primary_count(&t)
        );
        assert!(job::primary_count(&t) >= 2, "{name} must not skip");
        assert!((3_000..=6_500).contains(&hold), "{name}: {hold}");
    }
    // The ceiling: both parts at their caps.
    let worst = writer::SYSTEM.len() + writer::MAX_ANSWER_BYTES + writer::MAX_TABLE_BYTES + 6_000;
    let cap = b
        .cost_of(
            "claude-opus-5-5",
            &incisive_app::agent::llm::estimate_usage(worst, writer::DEFAULT_MAX_TOKENS),
        )
        .unwrap();
    println!("ceiling: about {worst} bytes, {cap} paise");
    assert!(cap <= 8_000, "{cap}");
}

/// The smoke's request (`temporal_writer::request`, emitted offline for the
/// owner's curl script) is exactly what the job sends: the scripted LLM
/// records the job's request, and both serialise to the same wire body.
#[tokio::test]
async fn the_smoke_request_is_the_jobs_request_byte_for_byte() {
    let Some(pool) = connect().await else { return };
    let uid = new_user(&pool, 100_000).await;
    let llm = ScriptedLlm::new().reply_json(board_json(), usage());
    let (_, status, _, llm) = job_with(&pool, uid, llm, TemporalSettings::on(), BIG).await;
    assert_eq!(status, JobStatus::Done);
    let sent = llm.requests().pop().unwrap();
    let body = hernia_body();
    let input = incisive_app::temporal::TemporalInput::from_json(&body.to_string()).unwrap();
    let table = incisive_app::temporal::build(&input);
    let user = writer::user(&writer::answer_text(&body, &input), &table, &input);
    let smoke = writer::request(MODEL, &user, writer::DEFAULT_MAX_TOKENS);
    assert_eq!(
        serde_json::to_vec(&smoke.body()).unwrap(),
        serde_json::to_vec(&sent.body()).unwrap()
    );
}

/// Review nit: the run is checked before any money is reserved; a run
/// deleted since it ended makes no call and reserves nothing.
#[tokio::test]
async fn job_reserves_nothing_for_a_run_deleted_in_the_meantime() {
    let Some(pool) = connect().await else { return };
    let uid = new_user(&pool, 100_000).await;
    let run = finished_run(&pool, uid).await;
    let req = Uuid::new_v4();
    insert_pending(&pool, run, req).await.unwrap();
    assert!(store::delete_run(&pool, uid, run).await.unwrap());
    let llm = Arc::new(ScriptedLlm::new().reply_json(board_json(), usage()));
    let st = state(
        pool.clone(),
        agent_llm(llm.clone(), MODEL),
        TemporalSettings::on(),
        BIG,
    );
    let s = run_job(&st, st.llm.as_ref().unwrap(), run, uid, req, &hernia_body()).await;
    assert_eq!(s, JobStatus::Skipped);
    assert!(llm.requests().is_empty());
    assert_eq!(reservations(&pool, uid).await, 0);
    assert!(charges(&pool, uid).await.is_empty());
}

/// Review nits: the job uses the writer's own client when one is set (the
/// agent's client is never called), and migration 0012 carries the
/// partial index recovery uses.
#[tokio::test]
async fn job_uses_the_writers_own_client_and_the_pending_index_exists() {
    let Some(pool) = connect().await else { return };
    let uid = new_user(&pool, 100_000).await;
    let run = finished_run(&pool, uid).await;
    let agent = Arc::new(ScriptedLlm::new());
    let own = Arc::new(ScriptedLlm::new().reply_json(board_json(), usage()));
    let mut st = state(
        pool.clone(),
        agent_llm(agent.clone(), MODEL),
        TemporalSettings::on(),
        BIG,
    );
    st.temporal_client = Some(own.clone());
    let req = Uuid::new_v4();
    insert_pending(&pool, run, req).await.unwrap();
    let s = run_job(&st, st.llm.as_ref().unwrap(), run, uid, req, &hernia_body()).await;
    assert_eq!(s, JobStatus::Done);
    assert_eq!(own.requests().len(), 1);
    assert!(agent.requests().is_empty());
    let def: String = sqlx::query_scalar(
        "SELECT indexdef FROM pg_indexes WHERE indexname = 'run_temporal_pending_idx'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(
        def.contains("(created_at)") && def.contains("'pending'"),
        "{def}"
    );
}
