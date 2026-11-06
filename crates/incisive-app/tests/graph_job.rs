//! The graph job with the database (`TEST_DATABASE_URL`): which runs are
//! queued and processed, the exact request, its money under its own request
//! id and against the user's caps, retries, empty replies, deleted runs,
//! recovery, the engine's rows unchanged and the switch.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use chrono::{FixedOffset, Utc};
use claude_client::{ClaudeError, ContentBlock, Effort, MessagesRequest, MessagesResponse, Usage};
use incisive_app::agent::answer_v3::AnswerV3;
use incisive_app::agent::llm::{LlmClient, SOURCE_RULE, ScriptedLlm};
use incisive_app::agent::testing::{agent_llm, offline_literature, placeholder_pipeline};
use incisive_app::db::{lazy_pool, migrate};
use incisive_app::gate::OfflineClassifier;
use incisive_app::graph::extract::{self, KnownNames};
use incisive_app::graph::job::{
    JobStatus, MAX_ATTEMPTS, MAX_TOKENS, PURPOSE, TIME_LIMIT, enqueue_new, fail_panicked,
    graph_loop, process, reason, recover_stale,
};
use incisive_app::graph::settings::{GraphRuntime, GraphSettings};
use incisive_app::identity::{IapVerifier, JwkSource};
use incisive_app::runs::worker::WorkerHandle;
use incisive_app::runs::{RunsConfig, store};
use incisive_app::server::AppState;
use incisive_app::spend::{CapsConfig, Prices};
use secrecy::SecretString;
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

const MODEL: &str = "m-graph";

/// `enqueue_new` and the jobs touch every run in the shared database, so the
/// database tests run one at a time.
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

fn today_ist() -> chrono::NaiveDate {
    Utc::now()
        .with_timezone(&FixedOffset::east_opt(19800).unwrap())
        .date_naive()
}

fn state(pool: PgPool, llm: Arc<dyn LlmClient>, on: bool) -> AppState {
    AppState {
        pool,
        verifier: Arc::new(IapVerifier::new(
            "aud".into(),
            JwkSource::Static(serde_json::from_str(r#"{"keys":[]}"#).unwrap()),
        )),
        gate: Arc::new(OfflineClassifier),
        prices: Prices::from_json(
            r#"{"m-test": {"input": 1.0, "output": 5.0}, "m-graph": {"input": 2.0, "output": 10.0}}"#,
        )
        .unwrap(),
        usd_inr: 88.0,
        caps: CapsConfig {
            global_monthly_paise: i64::MAX / 4,
        },
        gate_model: "m-test".into(),
        dev_mode: true,
        runs: RunsConfig::default(),
        quick_engine: incisive_app::runs::QuickEngine::Classic,
        full_engine: incisive_app::runs::Engine::Classic,
        completeness: false,
        temporal: Default::default(),
        temporal_client: None,
        stream: false,
        intro: Default::default(),
        graph: GraphRuntime {
            settings: GraphSettings {
                on,
                model: MODEL.into(),
                from: on.then(today_ist),
                ..GraphSettings::default()
            },
            client: Some(llm),
        },
        memory: incisive_app::memory::settings::MemorySettings::default(),
        chat: incisive_app::chat::settings::ChatSettings::default(),
        embedder: None,
        gemma: None,
        worker: WorkerHandle::with_pipeline(placeholder_pipeline),
        llm: Some(agent_llm(Arc::new(ScriptedLlm::new()), "m-test")),
        lit: offline_literature(),
    }
}

async fn new_user(pool: &PgPool, daily_cap: i64) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO incisive.users (email, display_name, role, daily_cap_paise) \
         VALUES ($1, 'G', 'surgeon', $2) RETURNING id",
    )
    .bind(format!("graph-job-{}@example.com", Uuid::new_v4()))
    .bind(daily_cap)
    .fetch_one(pool)
    .await
    .unwrap()
}

fn hernia() -> Value {
    serde_json::from_str(include_str!("fixtures/temporal/full/hernia.json")).unwrap()
}

/// A run in `status` of `mode` (`quick` or `focused`), with `body` stored as
/// its answer (version from its `version` field, else 2).
async fn new_run(pool: &PgPool, uid: i64, mode: &str, status: &str, body: Option<Value>) -> Uuid {
    let run = Uuid::new_v4();
    let question = body
        .as_ref()
        .and_then(|b| b["question"].as_str())
        .unwrap_or("q")
        .to_string();
    sqlx::query(
        "INSERT INTO incisive.runs (id, user_id, question, mode, status, limit_paise, cost_paise, \
           started_at, finished_at) \
         VALUES ($1, $2, $3, $4, $5, 1500, 321, now(), now())",
    )
    .bind(run)
    .bind(uid)
    .bind(question)
    .bind(mode)
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

async fn queue(pool: &PgPool, run: Uuid) {
    sqlx::query("INSERT INTO incisive.graph_jobs (run_id) VALUES ($1) ON CONFLICT DO NOTHING")
        .bind(run)
        .execute(pool)
        .await
        .unwrap();
}

/// Make the run's job due now.
async fn due(pool: &PgPool, run: Uuid) {
    sqlx::query("UPDATE incisive.graph_jobs SET next_try_at = now() WHERE run_id = $1")
        .bind(run)
        .execute(pool)
        .await
        .unwrap();
}

#[derive(Debug)]
#[allow(dead_code)]
struct Job {
    status: String,
    attempts: i32,
    kind: Option<String>,
    dropped: i32,
    cost: i64,
    request_id: Option<Uuid>,
    finished: bool,
}

type JobTuple = (String, i32, Option<String>, i32, i64, Option<Uuid>, bool);

async fn job_row(pool: &PgPool, run: Uuid) -> Option<Job> {
    let r: Option<JobTuple> = sqlx::query_as(
        "SELECT status, attempts, last_error_kind, dropped, cost_paise, request_id, \
                finished_at IS NOT NULL FROM incisive.graph_jobs WHERE run_id = $1",
    )
    .bind(run)
    .fetch_optional(pool)
    .await
    .unwrap();
    r.map(
        |(status, attempts, kind, dropped, cost, request_id, finished)| Job {
            status,
            attempts,
            kind,
            dropped,
            cost,
            request_id,
            finished,
        },
    )
}

/// (concept links, paper links) of the run.
async fn links(pool: &PgPool, run: Uuid) -> (i64, i64) {
    sqlx::query_as(
        "SELECT (SELECT count(*) FROM incisive.graph_run_concepts WHERE run_id = $1), \
                (SELECT count(*) FROM incisive.graph_run_papers WHERE run_id = $1)",
    )
    .bind(run)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn reservations(pool: &PgPool, uid: i64) -> Vec<(String, i64, Uuid)> {
    sqlx::query_as(
        "SELECT purpose, paise, request_id FROM incisive.spend \
         WHERE user_id = $1 AND kind = 'reservation' ORDER BY id",
    )
    .bind(uid)
    .fetch_all(pool)
    .await
    .unwrap()
}

async fn charges(pool: &PgPool, uid: i64) -> Vec<(String, i64)> {
    sqlx::query_as(
        "SELECT purpose, paise FROM incisive.spend WHERE user_id = $1 AND kind = 'charge' ORDER BY id",
    )
    .bind(uid)
    .fetch_all(pool)
    .await
    .unwrap()
}

fn usage() -> Usage {
    Usage {
        input_tokens: 900,
        output_tokens: 150,
        ..Usage::default()
    }
}

/// A good reply with names unique to `tag`.
fn good(tag: &str) -> Value {
    json!({
        "diseases": [format!("Inguinal hernia {tag}")],
        "treatments": [format!("Robotic surgery {tag}"), format!("Open repair {tag}")],
        "organs": ["Abdominal wall"],
        "outcomes": [{"row": 1, "name": format!("Pain {tag}")}, {"row": 99, "name": "Far"}]
    })
}

fn tag() -> String {
    format!("t{}", Uuid::new_v4().simple())
}

fn api_error() -> ClaudeError {
    ClaudeError::Api {
        status: 500,
        error_type: "api_error".into(),
        message: "no".into(),
    }
}

// ---------- which runs ----------

#[tokio::test]
async fn done_full_and_quick_runs_get_links_with_the_exact_request() {
    let Some(pool) = connect().await else { return };
    let _g = SERIAL.lock().await;
    for mode in ["focused", "quick"] {
        let uid = new_user(&pool, 100_000).await;
        let run = new_run(&pool, uid, mode, "done", Some(hernia())).await;
        queue(&pool, run).await;
        let t = tag();
        let llm = Arc::new(ScriptedLlm::new().reply_json(good(&t), usage()));
        let st = state(pool.clone(), llm.clone(), true);
        assert_eq!(process(&st, run).await, JobStatus::Done, "{mode}");

        let reqs = llm.requests();
        assert_eq!(reqs.len(), 1, "{mode}: one call");
        let req = &reqs[0];
        assert_eq!(req.model, MODEL);
        assert_eq!(req.max_tokens, 1200);
        assert_eq!(MAX_TOKENS, 1200);
        assert_eq!(
            req.system.as_deref().unwrap(),
            format!("{}\n\n{SOURCE_RULE}", extract::SYSTEM)
        );
        let a: AnswerV3 = serde_json::from_value(hernia()).unwrap();
        let want = extract::user_message(&a, &KnownNames::default()).unwrap();
        match &req.messages[..] {
            [m] => match &m.content[..] {
                [ContentBlock::Text { text }] => assert_eq!(text, &want),
                other => panic!("{other:?}"),
            },
            other => panic!("{other:?}"),
        }
        let cfg = req.output_config.as_ref().unwrap();
        assert_eq!(
            cfg.effort,
            Some(Effort::Low),
            "thinking counts toward max_tokens"
        );
        assert!(cfg.format.is_some(), "structured");

        let j = job_row(&pool, run).await.unwrap();
        assert_eq!(j.status, "done");
        assert_eq!(j.attempts, 1);
        assert_eq!(j.kind, None);
        assert_eq!(j.dropped, 1, "row 99");
        assert!(j.cost > 0);
        assert!(j.finished);
        // disease + 2 treatments + organ + outcome; every reference a paper.
        let a_refs = a.references.len() as i64;
        assert_eq!(links(&pool, run).await, (5, a_refs), "{mode}");
        let certainty: String = sqlx::query_scalar(
            "SELECT rc.certainty FROM incisive.graph_run_concepts rc \
             JOIN incisive.graph_concepts c ON c.id = rc.concept_id \
             WHERE rc.run_id = $1 AND c.kind = 'outcome'",
        )
        .bind(run)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(
            certainty,
            a.answer.as_ref().unwrap().outcomes[0].certainty.as_str()
        );
        // Done is final: not processed again.
        due(&pool, run).await;
        assert_eq!(process(&st, run).await, JobStatus::NotDue);
        assert_eq!(llm.requests().len(), 1);
    }
}

#[tokio::test]
async fn a_partial_run_with_an_answer_is_processed() {
    let Some(pool) = connect().await else { return };
    let _g = SERIAL.lock().await;
    let uid = new_user(&pool, 100_000).await;
    let run = new_run(&pool, uid, "focused", "partial", Some(hernia())).await;
    queue(&pool, run).await;
    let llm = Arc::new(ScriptedLlm::new().reply_json(good(&tag()), usage()));
    let st = state(pool.clone(), llm, true);
    assert_eq!(process(&st, run).await, JobStatus::Done);
    assert!(links(&pool, run).await.0 > 0);
}

#[tokio::test]
async fn failed_version_2_and_null_answers_are_skipped_without_a_call() {
    let Some(pool) = connect().await else { return };
    let _g = SERIAL.lock().await;
    let uid = new_user(&pool, 100_000).await;
    let mut v2 = hernia();
    v2.as_object_mut().unwrap().remove("version");
    let mut null = hernia();
    null["answer"] = Value::Null;
    let cases = [
        ("failed", "failed", Some(hernia()), reason::NOT_FINISHED),
        (
            "interrupted",
            "interrupted",
            Some(hernia()),
            reason::NOT_FINISHED,
        ),
        ("version 2", "done", Some(v2), reason::NOT_V3),
        ("answer null", "done", Some(null), reason::NO_ANSWER),
        ("no answer row", "done", None, reason::NO_ANSWER),
    ];
    for (name, status, body, want) in cases {
        let run = new_run(&pool, uid, "focused", status, body).await;
        queue(&pool, run).await;
        let llm = Arc::new(ScriptedLlm::new().reply_json(good(&tag()), usage()));
        let st = state(pool.clone(), llm.clone(), true);
        assert_eq!(process(&st, run).await, JobStatus::Skipped, "{name}");
        assert!(llm.requests().is_empty(), "{name}: no call");
        let j = job_row(&pool, run).await.unwrap();
        assert_eq!(j.status, "skipped", "{name}");
        assert_eq!(j.kind.as_deref(), Some(want), "{name}");
        assert_eq!(links(&pool, run).await, (0, 0), "{name}");
    }
    assert!(reservations(&pool, uid).await.is_empty());
    assert!(charges(&pool, uid).await.is_empty());
}

#[tokio::test]
async fn only_finished_runs_from_graph_from_are_queued_once() {
    let Some(pool) = connect().await else { return };
    let _g = SERIAL.lock().await;
    let uid = new_user(&pool, 100_000).await;
    let old = new_run(&pool, uid, "focused", "done", Some(hernia())).await;
    sqlx::query("UPDATE incisive.runs SET created_at = now() - interval '3 days' WHERE id = $1")
        .bind(old)
        .execute(&pool)
        .await
        .unwrap();
    let new = new_run(&pool, uid, "quick", "done", Some(hernia())).await;
    let running = new_run(&pool, uid, "quick", "running", None).await;
    let from = today_ist() - chrono::Days::new(1);
    assert!(enqueue_new(&pool, from).await.unwrap() >= 1);
    assert!(job_row(&pool, new).await.is_some(), "queued");
    assert!(job_row(&pool, old).await.is_none(), "before GRAPH_FROM");
    assert!(job_row(&pool, running).await.is_none(), "not finished yet");
    // A second pass adds nothing for these runs.
    enqueue_new(&pool, from).await.unwrap();
    let n: i64 =
        sqlx::query_scalar("SELECT count(*) FROM incisive.graph_jobs WHERE run_id = ANY($1)")
            .bind(vec![old, new, running])
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(n, 1);
    // Clear what this test queued for other users' runs, which other tests
    // drive by hand.
    sqlx::query(
        "DELETE FROM incisive.graph_jobs j USING incisive.runs r \
         WHERE r.id = j.run_id AND r.user_id <> $1 AND j.status = 'pending' AND j.attempts = 0",
    )
    .bind(uid)
    .execute(&pool)
    .await
    .unwrap();
}

// ---------- money ----------

#[tokio::test]
async fn money_is_reserved_and_charged_under_the_jobs_own_request_id() {
    let Some(pool) = connect().await else { return };
    let _g = SERIAL.lock().await;
    let uid = new_user(&pool, 100_000).await;
    let run = new_run(&pool, uid, "focused", "done", Some(hernia())).await;
    queue(&pool, run).await;
    let llm = Arc::new(
        ScriptedLlm::new()
            .delay(Duration::from_millis(300))
            .reply_json(good(&tag()), usage()),
    );
    let st = state(pool.clone(), llm, true);
    let h = {
        let st = st.clone();
        tokio::spawn(async move { process(&st, run).await })
    };
    let mut held = Vec::new();
    for _ in 0..200 {
        held = reservations(&pool, uid).await;
        if !held.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let req = job_row(&pool, run).await.unwrap().request_id.unwrap();
    assert_eq!(held.len(), 1, "held while the call runs");
    assert_eq!(held[0].0, PURPOSE);
    assert_eq!(PURPOSE, "graph");
    assert_eq!(held[0].2, req);
    assert_ne!(req, run, "not the run's own id");
    assert_eq!(h.await.unwrap(), JobStatus::Done);
    assert!(reservations(&pool, uid).await.is_empty(), "released");
    let c = charges(&pool, uid).await;
    assert_eq!(c.len(), 1);
    assert_eq!(c[0].0, format!("run:{req}"));
    assert_eq!(job_row(&pool, run).await.unwrap().cost, c[0].1);
    // The run's own cost is untouched.
    assert_eq!(store::run_cost(&pool, run).await.unwrap(), Some(321));
}

#[tokio::test]
async fn at_the_daily_cap_it_waits_for_the_next_ist_day_without_using_an_attempt() {
    let Some(pool) = connect().await else { return };
    let _g = SERIAL.lock().await;
    let uid = new_user(&pool, 1).await;
    let run = new_run(&pool, uid, "focused", "done", Some(hernia())).await;
    queue(&pool, run).await;
    let llm = Arc::new(ScriptedLlm::new().reply_json(good(&tag()), usage()));
    let st = state(pool.clone(), llm.clone(), true);
    assert_eq!(process(&st, run).await, JobStatus::CapBlocked);
    assert!(llm.requests().is_empty());
    assert!(reservations(&pool, uid).await.is_empty());
    let j = job_row(&pool, run).await.unwrap();
    assert_eq!(j.status, "cap_blocked");
    assert_eq!(j.attempts, 0, "no attempt used");
    assert_eq!(j.kind.as_deref(), Some(reason::DAILY_CAP));
    let next_day: bool = sqlx::query_scalar(
        "SELECT next_try_at = (date_trunc('day', now() AT TIME ZONE 'Asia/Kolkata') \
                               + interval '1 day') AT TIME ZONE 'Asia/Kolkata' \
         FROM incisive.graph_jobs WHERE run_id = $1",
    )
    .bind(run)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(next_day, "next_try_at is the next IST midnight");
    // Not due before then.
    assert_eq!(process(&st, run).await, JobStatus::NotDue);

    sqlx::query("UPDATE incisive.users SET daily_cap_paise = 100000 WHERE id = $1")
        .bind(uid)
        .execute(&pool)
        .await
        .unwrap();
    due(&pool, run).await;
    assert_eq!(process(&st, run).await, JobStatus::Done);
    let j = job_row(&pool, run).await.unwrap();
    assert_eq!((j.status.as_str(), j.attempts), ("done", 1));
}

// ---------- retries ----------

#[tokio::test]
async fn a_client_error_retries_up_to_three_then_fails_with_a_fixed_code() {
    let Some(pool) = connect().await else { return };
    let _g = SERIAL.lock().await;
    let uid = new_user(&pool, 100_000).await;
    let run = new_run(&pool, uid, "focused", "done", Some(hernia())).await;
    queue(&pool, run).await;
    let llm = Arc::new(
        ScriptedLlm::new()
            .fail(api_error())
            .fail(api_error())
            .fail(api_error()),
    );
    let st = state(pool.clone(), llm.clone(), true);
    assert_eq!(MAX_ATTEMPTS, 3);
    for attempt in 1..=3 {
        let s = process(&st, run).await;
        let j = job_row(&pool, run).await.unwrap();
        assert_eq!(j.attempts, attempt);
        assert_eq!(j.kind.as_deref(), Some(reason::API));
        assert!(reservations(&pool, uid).await.is_empty(), "released");
        if attempt < 3 {
            assert_eq!(s, JobStatus::Pending);
            assert_eq!(j.status, "pending");
            // A retry waits a little.
            assert_eq!(process(&st, run).await, JobStatus::NotDue);
            due(&pool, run).await;
        } else {
            assert_eq!(s, JobStatus::Failed);
            assert_eq!(j.status, "failed");
            assert!(j.finished);
        }
    }
    assert_eq!(llm.requests().len(), 3);
    due(&pool, run).await;
    assert_eq!(
        process(&st, run).await,
        JobStatus::NotDue,
        "failed is final"
    );
    assert_eq!(links(&pool, run).await, (0, 0));
}

#[tokio::test]
async fn an_empty_reply_writes_nothing_retries_and_then_succeeds() {
    let Some(pool) = connect().await else { return };
    let _g = SERIAL.lock().await;
    let uid = new_user(&pool, 100_000).await;
    let run = new_run(&pool, uid, "focused", "done", Some(hernia())).await;
    queue(&pool, run).await;
    let t = tag();
    let mut no_organ = good(&t);
    no_organ["organs"] = json!([]);
    let mut no_outcomes = good(&t);
    no_outcomes["outcomes"] = json!([{"row": 99, "name": "Out of range"}]);
    let llm = Arc::new(
        ScriptedLlm::new()
            .reply_json(no_organ, usage())
            .reply_json(no_outcomes, usage())
            .reply_json(good(&t), usage()),
    );
    let st = state(pool.clone(), llm.clone(), true);
    for _ in 0..2 {
        assert_eq!(process(&st, run).await, JobStatus::Pending);
        let j = job_row(&pool, run).await.unwrap();
        assert_eq!(j.kind.as_deref(), Some(reason::EMPTY_PICKS));
        assert_eq!(links(&pool, run).await, (0, 0), "nothing written");
        due(&pool, run).await;
    }
    assert_eq!(process(&st, run).await, JobStatus::Done);
    let j = job_row(&pool, run).await.unwrap();
    assert_eq!((j.status.as_str(), j.attempts), ("done", 3));
    assert_eq!(j.kind, None);
    assert!(j.cost > 0, "every attempt is counted");
    assert_eq!(charges(&pool, uid).await.len(), 3);
    assert_eq!(
        j.cost,
        charges(&pool, uid).await.iter().map(|c| c.1).sum::<i64>()
    );
    assert!(links(&pool, run).await.0 > 0);
}

#[tokio::test]
async fn an_empty_reply_three_times_ends_failed() {
    let Some(pool) = connect().await else { return };
    let _g = SERIAL.lock().await;
    let uid = new_user(&pool, 100_000).await;
    let run = new_run(&pool, uid, "quick", "done", Some(hernia())).await;
    queue(&pool, run).await;
    let mut empty = good(&tag());
    empty["organs"] = json!([]);
    let llm = Arc::new(
        ScriptedLlm::new()
            .reply_json(empty.clone(), usage())
            .reply_json(empty.clone(), usage())
            .reply_json(empty, usage()),
    );
    let st = state(pool.clone(), llm, true);
    let mut last = JobStatus::Pending;
    for _ in 0..3 {
        last = process(&st, run).await;
        due(&pool, run).await;
    }
    assert_eq!(last, JobStatus::Failed);
    let j = job_row(&pool, run).await.unwrap();
    assert_eq!(j.status, "failed");
    assert_eq!(j.kind.as_deref(), Some(reason::EMPTY_PICKS));
    assert_eq!(links(&pool, run).await, (0, 0), "off the map");
}

#[tokio::test]
async fn a_reply_with_no_treatments_is_accepted() {
    let Some(pool) = connect().await else { return };
    let _g = SERIAL.lock().await;
    let uid = new_user(&pool, 100_000).await;
    let run = new_run(&pool, uid, "focused", "done", Some(hernia())).await;
    queue(&pool, run).await;
    let mut r = good(&tag());
    r["treatments"] = json!([]);
    let st = state(
        pool.clone(),
        Arc::new(ScriptedLlm::new().reply_json(r, usage())),
        true,
    );
    assert_eq!(process(&st, run).await, JobStatus::Done);
    assert_eq!(links(&pool, run).await.0, 3, "disease, organ, outcome");
}

#[tokio::test]
async fn names_offered_are_the_users_own() {
    let Some(pool) = connect().await else { return };
    let _g = SERIAL.lock().await;
    let (a, b) = (
        new_user(&pool, 100_000).await,
        new_user(&pool, 100_000).await,
    );
    let t = tag();
    let first = new_run(&pool, a, "focused", "done", Some(hernia())).await;
    queue(&pool, first).await;
    let st = state(
        pool.clone(),
        Arc::new(ScriptedLlm::new().reply_json(good(&t), usage())),
        true,
    );
    assert_eq!(process(&st, first).await, JobStatus::Done);
    for (uid, seen) in [(a, true), (b, false)] {
        let run = new_run(&pool, uid, "focused", "done", Some(hernia())).await;
        queue(&pool, run).await;
        let llm = Arc::new(ScriptedLlm::new().reply_json(good(&tag()), usage()));
        let st = state(pool.clone(), llm.clone(), true);
        assert_eq!(process(&st, run).await, JobStatus::Done);
        let req = &llm.requests()[0];
        let ContentBlock::Text { text } = &req.messages[0].content[0] else {
            panic!("text");
        };
        assert_eq!(text.contains(&format!("Inguinal hernia {t}")), seen);
    }
}

// ---------- deleted runs ----------

#[tokio::test]
async fn a_run_deleted_before_the_call_makes_no_call() {
    let Some(pool) = connect().await else { return };
    let _g = SERIAL.lock().await;
    let uid = new_user(&pool, 100_000).await;
    let run = new_run(&pool, uid, "focused", "done", Some(hernia())).await;
    queue(&pool, run).await;
    assert!(store::delete_run(&pool, uid, run).await.unwrap());
    let llm = Arc::new(ScriptedLlm::new().reply_json(good(&tag()), usage()));
    let st = state(pool.clone(), llm.clone(), true);
    assert_eq!(
        process(&st, run).await,
        JobStatus::NotDue,
        "its job is gone"
    );
    assert!(llm.requests().is_empty());
    assert!(job_row(&pool, run).await.is_none());
    assert!(reservations(&pool, uid).await.is_empty());
}

#[tokio::test]
async fn a_run_deleted_during_the_call_leaves_no_rows_and_no_reservation() {
    let Some(pool) = connect().await else { return };
    let _g = SERIAL.lock().await;
    let uid = new_user(&pool, 100_000).await;
    let run = new_run(&pool, uid, "focused", "done", Some(hernia())).await;
    queue(&pool, run).await;
    let t = tag();
    let llm = Arc::new(
        ScriptedLlm::new()
            .delay(Duration::from_millis(300))
            .reply_json(good(&t), usage()),
    );
    let st = state(pool.clone(), llm, true);
    let h = {
        let st = st.clone();
        tokio::spawn(async move { process(&st, run).await })
    };
    for _ in 0..200 {
        if !reservations(&pool, uid).await.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert!(store::delete_run(&pool, uid, run).await.unwrap());
    let s = h.await.unwrap();
    assert_ne!(s, JobStatus::Done, "a deleted run never ends done");
    assert_eq!(s, JobStatus::Skipped);
    assert!(job_row(&pool, run).await.is_none(), "gone with the run");
    assert_eq!(links(&pool, run).await, (0, 0));
    let names: i64 =
        sqlx::query_scalar("SELECT count(*) FROM incisive.graph_concepts WHERE label LIKE $1")
            .bind(format!("% {t}"))
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(names, 0, "no concept written for it");
    assert!(reservations(&pool, uid).await.is_empty());
    assert_eq!(
        charges(&pool, uid).await.len(),
        1,
        "the call is still charged"
    );
}

// ---------- recovery ----------

async fn hold(pool: &PgPool, uid: i64, req: Uuid, age: &str) {
    sqlx::query(
        "INSERT INTO incisive.spend (user_id, at, kind, purpose, paise, request_id) \
         VALUES ($1, now() - $2::interval, 'reservation', 'graph', 60, $3)",
    )
    .bind(uid)
    .bind(age)
    .bind(req)
    .execute(pool)
    .await
    .unwrap();
}

#[tokio::test]
async fn recovery_returns_a_stale_running_job_and_releases_leaked_holds() {
    let Some(pool) = connect().await else { return };
    let _g = SERIAL.lock().await;
    let uid = new_user(&pool, 100_000).await;
    let stale = new_run(&pool, uid, "focused", "done", Some(hernia())).await;
    let spent = new_run(&pool, uid, "focused", "done", Some(hernia())).await;
    let live = new_run(&pool, uid, "focused", "done", Some(hernia())).await;
    let (r1, r2, r3) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
    for (run, req, attempts, lease) in [
        (stale, r1, 1, "-1 minute"),
        (spent, r2, 3, "-1 minute"),
        (live, r3, 1, "4 minutes"),
    ] {
        sqlx::query(
            "INSERT INTO incisive.graph_jobs (run_id, status, attempts, request_id, next_try_at) \
             VALUES ($1, 'running', $2, $3, now() + $4::interval)",
        )
        .bind(run)
        .bind(attempts)
        .bind(req)
        .bind(lease)
        .execute(&pool)
        .await
        .unwrap();
    }
    hold(&pool, uid, r1, "6 minutes").await;
    hold(&pool, uid, r2, "6 minutes").await;
    hold(&pool, uid, r3, "1 minute").await;
    let leaked = Uuid::new_v4();
    hold(&pool, uid, leaked, "6 minutes").await;
    let young = Uuid::new_v4();
    hold(&pool, uid, young, "1 minute").await;

    // Recovery is global: rows other runs of this test left may count too.
    assert!(recover_stale(&pool).await.unwrap() >= 2);
    let j = job_row(&pool, stale).await.unwrap();
    assert_eq!(
        (j.status.as_str(), j.kind.as_deref()),
        ("pending", Some(reason::STALE))
    );
    let j = job_row(&pool, spent).await.unwrap();
    assert_eq!(
        (j.status.as_str(), j.kind.as_deref()),
        ("failed", Some(reason::STALE))
    );
    assert!(j.finished);
    assert_eq!(
        job_row(&pool, live).await.unwrap().status,
        "running",
        "lease not over"
    );
    let left: Vec<Uuid> = reservations(&pool, uid)
        .await
        .into_iter()
        .map(|r| r.2)
        .collect();
    assert_eq!(left, vec![r3, young], "stale and leaked holds released");

    // A recovered job runs again.
    due(&pool, stale).await;
    let st = state(
        pool.clone(),
        Arc::new(ScriptedLlm::new().reply_json(good(&tag()), usage())),
        true,
    );
    assert_eq!(process(&st, stale).await, JobStatus::Done);
    // Leave nothing running for the next run of this test.
    assert!(store::delete_run(&pool, uid, live).await.unwrap());
}

/// A client that panics (in the call's own task: reported as unrecorded).
struct Panicking;

#[async_trait]
impl LlmClient for Panicking {
    async fn call(&self, _req: &MessagesRequest) -> Result<MessagesResponse, ClaudeError> {
        panic!("a panic that could quote the question");
    }
}

#[tokio::test]
async fn a_panicking_call_fails_the_job_and_releases_its_hold() {
    let Some(pool) = connect().await else { return };
    let _g = SERIAL.lock().await;
    let uid = new_user(&pool, 100_000).await;
    let run = new_run(&pool, uid, "focused", "done", Some(hernia())).await;
    queue(&pool, run).await;
    let st = state(pool.clone(), Arc::new(Panicking), true);
    assert_eq!(process(&st, run).await, JobStatus::Failed);
    let j = job_row(&pool, run).await.unwrap();
    assert_eq!(j.kind.as_deref(), Some(reason::UNRECORDED));
    for _ in 0..200 {
        if reservations(&pool, uid).await.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(reservations(&pool, uid).await.is_empty());
}

// ---------- the engine's rows and the switch ----------

async fn engine_snapshot(pool: &PgPool, run: Uuid) -> Vec<Option<String>> {
    let mut out = Vec::new();
    for sql in [
        "SELECT row_to_json(r)::text FROM incisive.runs r WHERE id = $1",
        "SELECT row_to_json(a)::text FROM incisive.run_answers a WHERE run_id = $1",
        "SELECT json_agg(s ORDER BY seq)::text FROM incisive.run_steps s WHERE run_id = $1",
        "SELECT row_to_json(t)::text FROM incisive.run_temporal t WHERE run_id = $1",
        "SELECT row_to_json(m)::text FROM incisive.memory_jobs m WHERE run_id = $1",
    ] {
        let v: Option<Option<String>> = sqlx::query_scalar(sql)
            .bind(run)
            .fetch_optional(pool)
            .await
            .unwrap();
        out.push(v.flatten());
    }
    out
}

#[tokio::test]
async fn the_runs_rows_are_byte_identical_before_and_after() {
    let Some(pool) = connect().await else { return };
    let _g = SERIAL.lock().await;
    let uid = new_user(&pool, 100_000).await;
    let run = new_run(&pool, uid, "focused", "done", Some(hernia())).await;
    for line in ["Searching", "Writing"] {
        store::add_step(&pool, run, incisive_app::runs::StepKind::Search, line)
            .await
            .unwrap();
    }
    sqlx::query(
        "INSERT INTO incisive.run_temporal (run_id, status, reason) VALUES ($1, 'skipped', 'x')",
    )
    .bind(run)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO incisive.memory_jobs (run_id) VALUES ($1)")
        .bind(run)
        .execute(&pool)
        .await
        .unwrap();
    queue(&pool, run).await;
    let before = engine_snapshot(&pool, run).await;
    assert!(before.iter().all(Option::is_some), "{before:?}");
    let st = state(
        pool.clone(),
        Arc::new(ScriptedLlm::new().reply_json(good(&tag()), usage())),
        true,
    );
    assert_eq!(process(&st, run).await, JobStatus::Done);
    assert_eq!(engine_snapshot(&pool, run).await, before);
}

#[tokio::test]
async fn graph_off_starts_no_loop_and_makes_no_call() {
    let Some(pool) = connect().await else { return };
    let _g = SERIAL.lock().await;
    let uid = new_user(&pool, 100_000).await;
    let run = new_run(&pool, uid, "focused", "done", Some(hernia())).await;
    let llm = Arc::new(ScriptedLlm::new().reply_json(good(&tag()), usage()));
    let st = state(pool.clone(), llm.clone(), false);
    tokio::time::timeout(Duration::from_secs(2), graph_loop(st.clone()))
        .await
        .expect("graph_loop returns at once when off");
    assert!(job_row(&pool, run).await.is_none(), "nothing queued");
    queue(&pool, run).await;
    assert_eq!(
        process(&st, run).await,
        JobStatus::NotDue,
        "off: no job runs"
    );
    assert!(llm.requests().is_empty());
}

// ---------- no client, store faults, panics, time ----------

#[tokio::test]
async fn without_its_own_client_the_job_fails_and_never_uses_the_agents() {
    let Some(pool) = connect().await else { return };
    let _g = SERIAL.lock().await;
    let uid = new_user(&pool, 100_000).await;
    let run = new_run(&pool, uid, "focused", "done", Some(hernia())).await;
    queue(&pool, run).await;
    let agents = Arc::new(ScriptedLlm::new().reply_json(good(&tag()), usage()));
    let mut st = state(pool.clone(), Arc::new(ScriptedLlm::new()), true);
    st.graph.client = None;
    st.llm = Some(agent_llm(agents.clone(), MODEL));
    assert_eq!(process(&st, run).await, JobStatus::Failed);
    let j = job_row(&pool, run).await.unwrap();
    assert_eq!(
        (j.status.as_str(), j.kind.as_deref()),
        ("failed", Some(reason::NO_CLIENT))
    );
    assert!(j.finished);
    assert_eq!(j.cost, 0);
    assert!(
        agents.requests().is_empty(),
        "the agent's client is not used"
    );
    assert!(reservations(&pool, uid).await.is_empty());
    assert!(charges(&pool, uid).await.is_empty());
    assert_eq!(links(&pool, run).await, (0, 0));
}

/// Make every paper link insert for `run` fail with a foreign-key violation
/// (23503) while the run itself stays. Returns the trigger's name.
async fn fk_fault(pool: &PgPool, run: Uuid) -> String {
    let name = format!("graph_fk_fault_{}", run.simple());
    sqlx::query(&format!(
        "CREATE FUNCTION incisive.{name}() RETURNS trigger LANGUAGE plpgsql AS $$ \
         BEGIN IF NEW.run_id = '{run}'::uuid THEN \
           RAISE EXCEPTION 'fault' USING ERRCODE = '23503'; END IF; RETURN NEW; END $$"
    ))
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(&format!(
        "CREATE TRIGGER {name} BEFORE INSERT ON incisive.graph_run_papers \
         FOR EACH ROW EXECUTE FUNCTION incisive.{name}()"
    ))
    .execute(pool)
    .await
    .unwrap();
    name
}

async fn drop_fk_fault(pool: &PgPool, name: &str) {
    sqlx::query(&format!("DROP TRIGGER {name} ON incisive.graph_run_papers"))
        .execute(pool)
        .await
        .unwrap();
    sqlx::query(&format!("DROP FUNCTION incisive.{name}()"))
        .execute(pool)
        .await
        .unwrap();
}

#[tokio::test]
async fn a_foreign_key_fault_while_the_run_exists_is_retried_not_skipped() {
    let Some(pool) = connect().await else { return };
    let _g = SERIAL.lock().await;
    let uid = new_user(&pool, 100_000).await;
    let run = new_run(&pool, uid, "focused", "done", Some(hernia())).await;
    queue(&pool, run).await;
    let llm = Arc::new(
        ScriptedLlm::new()
            .reply_json(good(&tag()), usage())
            .reply_json(good(&tag()), usage()),
    );
    let st = state(pool.clone(), llm.clone(), true);
    let fault = fk_fault(&pool, run).await;
    let s = process(&st, run).await;
    drop_fk_fault(&pool, &fault).await;
    assert_eq!(s, JobStatus::Pending);
    let j = job_row(&pool, run).await.unwrap();
    assert_eq!(
        (j.status.as_str(), j.attempts, j.kind.as_deref()),
        ("pending", 1, Some(reason::STORE))
    );
    assert!(!j.finished);
    assert!(j.cost > 0, "the call was charged");
    assert_eq!(links(&pool, run).await, (0, 0), "the write rolled back");
    assert!(reservations(&pool, uid).await.is_empty());

    // The next attempt writes the run.
    due(&pool, run).await;
    assert_eq!(process(&st, run).await, JobStatus::Done);
    assert!(links(&pool, run).await.1 > 0);
}

#[tokio::test]
async fn a_panicked_job_is_failed_and_its_reservation_released() {
    let Some(pool) = connect().await else { return };
    let _g = SERIAL.lock().await;
    let uid = new_user(&pool, 100_000).await;
    let run = new_run(&pool, uid, "focused", "done", Some(hernia())).await;
    let other = new_run(&pool, uid, "focused", "done", Some(hernia())).await;
    let (req, other_req) = (Uuid::new_v4(), Uuid::new_v4());
    for (r, q) in [(run, req), (other, other_req)] {
        sqlx::query(
            "INSERT INTO incisive.graph_jobs (run_id, status, attempts, request_id, next_try_at) \
             VALUES ($1, 'running', 1, $2, now() + interval '5 minutes')",
        )
        .bind(r)
        .bind(q)
        .execute(&pool)
        .await
        .unwrap();
        hold(&pool, uid, q, "0 minutes").await;
    }

    fail_panicked(&pool, run).await;
    let j = job_row(&pool, run).await.unwrap();
    assert_eq!(
        (j.status.as_str(), j.kind.as_deref()),
        ("failed", Some(reason::PANIC))
    );
    assert!(j.finished);
    let left: Vec<Uuid> = reservations(&pool, uid)
        .await
        .into_iter()
        .map(|r| r.2)
        .collect();
    assert_eq!(
        left,
        vec![other_req],
        "only the panicked job's hold released"
    );
    assert_eq!(job_row(&pool, other).await.unwrap().status, "running");

    // A job that is no longer running is left alone.
    fail_panicked(&pool, run).await;
    assert_eq!(
        job_row(&pool, run).await.unwrap().kind.as_deref(),
        Some(reason::PANIC)
    );
    assert!(store::delete_run(&pool, uid, other).await.unwrap());
    sqlx::query("DELETE FROM incisive.spend WHERE request_id = $1")
        .bind(other_req)
        .execute(&pool)
        .await
        .unwrap();
}

#[test]
fn the_time_limit_defaults_to_thirty_seconds() {
    assert_eq!(TIME_LIMIT, Duration::from_secs(30));
    assert_eq!(GraphSettings::default().time_limit, TIME_LIMIT);
}

#[tokio::test]
async fn a_timed_out_call_is_retried_and_its_late_charge_is_booked() {
    let Some(pool) = connect().await else { return };
    let _g = SERIAL.lock().await;
    let uid = new_user(&pool, 100_000).await;
    let run = new_run(&pool, uid, "focused", "done", Some(hernia())).await;
    queue(&pool, run).await;
    let llm = Arc::new(
        ScriptedLlm::new()
            .delay(Duration::from_millis(400))
            .reply_json(good(&tag()), usage()),
    );
    let mut st = state(pool.clone(), llm, true);
    st.graph.settings.time_limit = Duration::from_millis(30);
    let started = std::time::Instant::now();
    assert_eq!(process(&st, run).await, JobStatus::Pending);
    assert!(
        started.elapsed() < Duration::from_millis(400),
        "did not wait"
    );
    let j = job_row(&pool, run).await.unwrap();
    assert_eq!(
        (j.status.as_str(), j.attempts, j.kind.as_deref()),
        ("pending", 1, Some(reason::TIME))
    );
    assert_eq!(
        links(&pool, run).await,
        (0, 0),
        "a late reply is not written"
    );
    // The call still books its charge, frees what the job kept for it, and
    // the job's cost is brought up to date.
    for _ in 0..200 {
        let cost = job_row(&pool, run).await.unwrap().cost;
        if cost > 0 && reservations(&pool, uid).await.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert!(reservations(&pool, uid).await.is_empty());
    let c = charges(&pool, uid).await;
    assert_eq!(c.len(), 1);
    assert_eq!(
        job_row(&pool, run).await.unwrap().cost,
        c[0].1,
        "late cost on the job"
    );
}
