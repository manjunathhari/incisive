//! The chat routes through `api_routes` against the database
//! (`TEST_DATABASE_URL`) with a fake Gemma.

use std::sync::Arc;
use std::time::Duration;

use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode},
};
use http_body_util::BodyExt;
use incisive_app::agent::temporal_writer::TemporalSettings;
use incisive_app::agent::testing::{offline_literature, placeholder_pipeline};
use incisive_app::ask::BLOCKED_MESSAGE;
use incisive_app::chat::gemma::GemmaChat;
use incisive_app::chat::settings::ChatSettings;
use incisive_app::chat::{job, lines, store};
use incisive_app::db::{lazy_pool, migrate};
use incisive_app::gate::OfflineClassifier;
use incisive_app::identity::{IapVerifier, JwkSource, Role, User};
use incisive_app::memory::job as memory_job;
use incisive_app::memory::settings::MemorySettings;
use incisive_app::runs::RunsConfig;
use incisive_app::runs::worker::WorkerHandle;
use incisive_app::server::{AppState, api_routes};
use incisive_app::spend::{CapsConfig, Prices};
use secrecy::SecretString;
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

const ZEBRA_Q: &str = "What is the zebra recurrence?";
const ZEBRA_TEXT: &str = "Zebra recurrence was 5 % in this series.";
const ZEBRA_REPLY: &str = "Zebra recurrence was 5% [1].";

async fn begin() -> Option<(tokio::sync::MutexGuard<'static, ()>, PgPool)> {
    let guard = SERIAL.lock().await;
    let Ok(url) = std::env::var("TEST_DATABASE_URL") else {
        eprintln!("SKIP: TEST_DATABASE_URL not set");
        return None;
    };
    let pool = lazy_pool(&SecretString::from(url)).expect("lazy pool");
    migrate(&pool).await.expect("migrate");
    sqlx::query(
        "TRUNCATE incisive.chat_passages, incisive.chat_prep, incisive.chat_turns, \
         incisive.chat_conversations, incisive.topic_qas, incisive.chat_priors, incisive.memory_jobs, \
         incisive.ledger_claims, incisive.ledger_studies, incisive.evidence_records, \
         incisive.topics CASCADE",
    )
    .execute(&pool)
    .await
    .unwrap();
    Some((guard, pool))
}

fn base_state(pool: PgPool, memory: MemorySettings, chat: ChatSettings) -> AppState {
    AppState {
        pool,
        verifier: Arc::new(IapVerifier::new(
            "aud".into(),
            JwkSource::Static(serde_json::from_str(r#"{"keys":[]}"#).unwrap()),
        )),
        gate: Arc::new(OfflineClassifier),
        prices: Prices::from_json(r#"{"m-test": {"input": 1.0, "output": 5.0}}"#).unwrap(),
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
        temporal: TemporalSettings::default(),
        temporal_client: None,
        intro: Default::default(),
        chat,
        memory,
        embedder: None,
        gemma: None,
        worker: WorkerHandle::with_pipeline(placeholder_pipeline),
        llm: None,
        lit: offline_literature(),
    }
}

/// A state with no Gemma client.
fn no_gemma(pool: PgPool) -> AppState {
    base_state(
        pool,
        MemorySettings {
            on: true,
            ..MemorySettings::default()
        },
        ChatSettings {
            on: true,
            ..ChatSettings::default()
        },
    )
}

/// A state whose Gemma is at `uri`. No embedder: retrieval is words only.
fn with_gemma(pool: PgPool, uri: &str) -> AppState {
    let memory = MemorySettings {
        on: true,
        gemma_url: Some(uri.to_string()),
        gemma_auth: Some(SecretString::from("incisive:pw")),
        ..MemorySettings::default()
    };
    let chat = ChatSettings {
        on: true,
        ..ChatSettings::default()
    };
    let gemma = GemmaChat::from_settings(&memory, &chat).expect("client");
    let mut st = base_state(pool, memory, chat);
    st.gemma = Some(Arc::new(gemma));
    st
}

struct Fx {
    body: &'static str,
    slim: &'static str,
    board: &'static str,
}

const HERNIA: Fx = Fx {
    body: include_str!("fixtures/temporal/full/hernia.json"),
    slim: include_str!("fixtures/temporal/hernia.json"),
    board: include_str!("fixtures/temporal/boards/hernia-board.json"),
};

async fn seed_user_role(pool: &PgPool, role: Role) -> User {
    let email = format!("csvc-{}@example.test", Uuid::new_v4());
    let role_text = match role {
        Role::Owner => "owner",
        Role::Surgeon => "surgeon",
        Role::Fellow => "fellow",
    };
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO incisive.users (email, display_name, role, daily_cap_paise) \
         VALUES ($1, 'C', $2, 100000) RETURNING id",
    )
    .bind(&email)
    .bind(role_text)
    .fetch_one(pool)
    .await
    .unwrap();
    User {
        id,
        email,
        display_name: "C".into(),
        role,
        daily_cap_paise: 100_000,
        monthly_cap_paise: None,
        active: true,
    }
}

async fn seed_run(pool: &PgPool, user: &User, mode: &str) -> Uuid {
    let run = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO incisive.runs (id, user_id, question, mode, status, limit_paise) \
         VALUES ($1, $2, 'Is robotic better?', $3, 'done', 1000)",
    )
    .bind(run)
    .bind(user.id)
    .bind(mode)
    .execute(pool)
    .await
    .unwrap();
    run
}

/// A run with a real evidence record, made by the memory job, but no prep.
async fn seed_record(pool: &PgPool, fx: &Fx) -> (User, Uuid, i64) {
    let user = seed_user_role(pool, Role::Surgeon).await;
    let run = seed_run(pool, &user, "focused").await;
    let body: Value = serde_json::from_str(fx.body).unwrap();
    sqlx::query(
        "INSERT INTO incisive.run_answers (run_id, body, answer_version) VALUES ($1, $2, 3)",
    )
    .bind(run)
    .bind(&body)
    .execute(pool)
    .await
    .unwrap();
    let input = incisive_app::temporal::TemporalInput::from_json(fx.slim).unwrap();
    let table = serde_json::to_value(incisive_app::temporal::build(&input)).unwrap();
    let wrapped: Value = serde_json::from_str(fx.board).unwrap();
    sqlx::query(
        "INSERT INTO incisive.run_temporal (run_id, status, table_json, board_json) \
         VALUES ($1, 'done', $2, $3)",
    )
    .bind(run)
    .bind(&table)
    .bind(&wrapped["board"])
    .execute(pool)
    .await
    .unwrap();
    let st = no_gemma(pool.clone());
    let memory_job::JobOutcome::Done { record_id, .. } = memory_job::process(&st, run).await else {
        panic!("memory job did not record the run");
    };
    (user, run, record_id)
}

/// A ready run: record and prep, plus the zebra passage.
async fn ready(pool: &PgPool, fx: &Fx) -> (User, Uuid, i64) {
    let (user, run, rec) = seed_record(pool, fx).await;
    job::tick(&no_gemma(pool.clone())).await;
    add_passage(pool, rec, 9000, ZEBRA_TEXT, &[]).await;
    (user, run, rec)
}

async fn add_passage(pool: &PgPool, rec: i64, seq: i32, text: &str, comps: &[&str]) -> i64 {
    let comps: Vec<String> = comps.iter().map(|s| s.to_string()).collect();
    sqlx::query_scalar(
        "INSERT INTO incisive.chat_passages (record_id, kind, seq, ref_ns, comparisons, text) \
         VALUES ($1, 'evidence', $2, '{1}', $3, $4) RETURNING id",
    )
    .bind(rec)
    .bind(seq)
    .bind(&comps)
    .bind(text)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn other_pending(pool: &PgPool, run: Uuid, age_secs: i32) -> i64 {
    let u = seed_user_role(pool, Role::Surgeon).await;
    add_turn(pool, run, u.id, 1, "pending", "", age_secs).await
}

/// A turn in `user`'s conversation on `run` (made if needed).
async fn add_turn(
    pool: &PgPool,
    run: Uuid,
    user: i64,
    seq: i32,
    status: &str,
    reply: &str,
    age_secs: i32,
) -> i64 {
    add_asked_turn(pool, run, user, seq, "q", status, reply, age_secs).await
}

#[allow(clippy::too_many_arguments)]
async fn add_asked_turn(
    pool: &PgPool,
    run: Uuid,
    user: i64,
    seq: i32,
    question: &str,
    status: &str,
    reply: &str,
    age_secs: i32,
) -> i64 {
    let existing: Option<i64> = sqlx::query_scalar(
        "SELECT id FROM incisive.chat_conversations WHERE run_id = $1 AND user_id = $2",
    )
    .bind(run)
    .bind(user)
    .fetch_optional(pool)
    .await
    .unwrap();
    let conv = match existing {
        Some(c) => c,
        None => sqlx::query_scalar(
            "INSERT INTO incisive.chat_conversations (run_id, user_id, mode) \
             VALUES ($1, $2, 'clinical') RETURNING id",
        )
        .bind(run)
        .bind(user)
        .fetch_one(pool)
        .await
        .unwrap(),
    };
    sqlx::query_scalar(
        "INSERT INTO incisive.chat_turns \
         (conversation_id, user_id, seq, question, reply, mode, status, updated_at) \
         VALUES ($1, $2, $3, $4, $5, 'clinical', $6, now() - make_interval(secs => $7)) \
         RETURNING id",
    )
    .bind(conv)
    .bind(user)
    .bind(seq)
    .bind(question)
    .bind(reply)
    .bind(status)
    .bind(f64::from(age_secs))
    .fetch_one(pool)
    .await
    .unwrap()
}

// ---------- HTTP helpers ----------

async fn call(app: Router, req: Request<Body>) -> (StatusCode, Value) {
    let res = app.oneshot(req).await.unwrap();
    let status = res.status();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

fn get(email: &str, uri: &str) -> Request<Body> {
    Request::get(uri)
        .header("x-dev-user-email", email)
        .body(Body::empty())
        .unwrap()
}

fn post(email: &str, uri: &str, body: Value) -> Request<Body> {
    post_raw(email, uri, body.to_string())
}

fn post_raw(email: &str, uri: &str, body: String) -> Request<Body> {
    Request::post(uri)
        .header("content-type", "application/json")
        .header("x-dev-user-email", email)
        .body(Body::from(body))
        .unwrap()
}

fn ask_body(q: &str) -> Value {
    json!({"question": q, "mode": "clinical"})
}

/// A `done` turn with the given citations.
async fn done_turn(pool: &PgPool, run: Uuid, user: &User, seq: i32, ids: &[i64]) -> i64 {
    let id = add_asked_turn(pool, run, user.id, seq, ZEBRA_Q, "done", ZEBRA_REPLY, 0).await;
    sqlx::query("UPDATE incisive.chat_turns SET passage_ids = $2 WHERE id = $1")
        .bind(id)
        .bind(ids)
        .execute(pool)
        .await
        .unwrap();
    id
}

async fn approved_qa(pool: &PgPool, rec: i64, user: &User) -> i64 {
    let topic: i64 =
        sqlx::query_scalar("SELECT topic_id FROM incisive.evidence_records WHERE id = $1")
            .bind(rec)
            .fetch_one(pool)
            .await
            .unwrap();
    sqlx::query_scalar(
        "INSERT INTO incisive.topic_qas \
         (topic_id, question, gemma_reply, owner_text, status, author_user_id) \
         VALUES ($1, 'Q?', 'g', 'The owner says so.', 'approved', $2) RETURNING id",
    )
    .bind(topic)
    .bind(user.id)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn count(pool: &PgPool, sql: &str) -> i64 {
    sqlx::query_scalar(sql).fetch_one(pool).await.unwrap()
}

// ---------- tests ----------

#[tokio::test]
async fn routes_404_when_chat_off() {
    let Some((_g, pool)) = begin().await else {
        return;
    };
    let (user, run, rec) = ready(&pool, &HERNIA).await;
    let pid = add_passage(&pool, rec, 9001, "Cited text.", &[]).await;
    let turn = done_turn(&pool, run, &user, 1, &[pid]).await;

    let mut off = no_gemma(pool.clone());
    off.chat.on = false;
    let app = || api_routes(off.clone());
    let e = user.email.as_str();
    let reqs = vec![
        get(e, &format!("/api/runs/{run}/chat")),
        post(e, &format!("/api/runs/{run}/chat"), ask_body(ZEBRA_Q)),
        get(e, &format!("/api/chat/turns/{turn}")),
        get(e, &format!("/api/chat/passages/{pid}")),
        post(
            e,
            &format!("/api/chat/turns/{turn}/feedback"),
            json!({"value": 1}),
        ),
        post(e, &format!("/api/chat/turns/{turn}/suggest"), json!({})),
    ];
    for r in reqs {
        let uri = r.uri().to_string();
        let (status, _) = call(app(), r).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{uri}");
    }
    // Nothing was written, and an existing route still works.
    assert_eq!(
        count(&pool, "SELECT count(*) FROM incisive.topic_qas").await,
        0
    );
    assert_eq!(
        count(
            &pool,
            "SELECT count(*) FROM incisive.chat_turns WHERE feedback IS NOT NULL"
        )
        .await,
        0
    );
    let (status, _) = call(app(), get(e, &format!("/api/runs/{run}"))).await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn cross_site_post_refused() {
    let Some((_g, pool)) = begin().await else {
        return;
    };
    let (user, run, _) = ready(&pool, &HERNIA).await;
    let turn = done_turn(&pool, run, &user, 1, &[]).await;
    // No Gemma client: the same-origin ask below is refused 503 past the check
    // and starts no task, so nothing outlives the test.
    let st = no_gemma(pool.clone());
    for (uri, body) in [
        (format!("/api/runs/{run}/chat"), ask_body(ZEBRA_Q)),
        (
            format!("/api/chat/turns/{turn}/feedback"),
            json!({"value": 1}),
        ),
        (format!("/api/chat/turns/{turn}/suggest"), json!({})),
    ] {
        let req = Request::post(&uri)
            .header("content-type", "application/json")
            .header("x-dev-user-email", &user.email)
            .header("host", "incisive.example")
            .header("origin", "https://evil.example")
            .body(Body::from(body.to_string()))
            .unwrap();
        let (status, b) = call(api_routes(st.clone()), req).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{uri}");
        assert_eq!(b["error"], "cross_site");
        // The same request from the same origin gets past the check.
        let req = Request::post(&uri)
            .header("content-type", "application/json")
            .header("x-dev-user-email", &user.email)
            .header("host", "incisive.example")
            .header("origin", "https://incisive.example")
            .body(Body::from(body.to_string()))
            .unwrap();
        let (status, _) = call(api_routes(st.clone()), req).await;
        assert_ne!(status, StatusCode::FORBIDDEN, "{uri}");
    }
}

#[tokio::test]
async fn other_users_turn_is_not_found_on_every_route() {
    let Some((_g, pool)) = begin().await else {
        return;
    };
    let (owner_of_run, run, rec) = ready(&pool, &HERNIA).await;
    let pid = add_passage(&pool, rec, 9001, "Cited text.", &[]).await;
    let turn = done_turn(&pool, run, &owner_of_run, 1, &[pid]).await;
    let intruder = seed_user_role(&pool, Role::Surgeon).await;
    let owner = seed_user_role(&pool, Role::Owner).await;

    let server = gemma_says(ZEBRA_REPLY, 0).await;
    let st = with_gemma(pool.clone(), &server.uri());
    let missing_run = Uuid::new_v4();
    let missing_turn = turn + 1000;

    for who in [&intruder, &owner] {
        let e = who.email.as_str();
        // (someone else's, missing) for each route: identical answers.
        let pairs = vec![
            (
                get(e, &format!("/api/runs/{run}/chat")),
                get(e, &format!("/api/runs/{missing_run}/chat")),
            ),
            (
                post(e, &format!("/api/runs/{run}/chat"), ask_body(ZEBRA_Q)),
                post(
                    e,
                    &format!("/api/runs/{missing_run}/chat"),
                    ask_body(ZEBRA_Q),
                ),
            ),
            (
                get(e, &format!("/api/chat/turns/{turn}")),
                get(e, &format!("/api/chat/turns/{missing_turn}")),
            ),
            (
                get(e, &format!("/api/chat/passages/{pid}")),
                get(e, &format!("/api/chat/passages/{}", pid + 1000)),
            ),
            (
                post(
                    e,
                    &format!("/api/chat/turns/{turn}/feedback"),
                    json!({"value": 1}),
                ),
                post(
                    e,
                    &format!("/api/chat/turns/{missing_turn}/feedback"),
                    json!({"value": 1}),
                ),
            ),
            (
                post(e, &format!("/api/chat/turns/{turn}/suggest"), json!({})),
                post(
                    e,
                    &format!("/api/chat/turns/{missing_turn}/suggest"),
                    json!({}),
                ),
            ),
            (
                get(e, "/api/chat/turns/not-a-number"),
                get(e, &format!("/api/chat/turns/{missing_turn}")),
            ),
        ];
        for (theirs, missing) in pairs {
            let uri = theirs.uri().to_string();
            let (s1, b1) = call(api_routes(st.clone()), theirs).await;
            let (s2, b2) = call(api_routes(st.clone()), missing).await;
            assert_eq!(s1, StatusCode::NOT_FOUND, "{uri}");
            assert_eq!((s1, b1), (s2, b2), "{uri}");
        }
    }
    // Nothing of the real owner's changed.
    assert_eq!(
        count(&pool, "SELECT count(*) FROM incisive.topic_qas").await,
        0
    );
    assert_eq!(
        count(
            &pool,
            "SELECT count(*) FROM incisive.chat_turns WHERE feedback IS NOT NULL OR suggested"
        )
        .await,
        0
    );
    assert_eq!(
        count(&pool, "SELECT count(*) FROM incisive.chat_turns").await,
        1
    );
}

#[tokio::test]
async fn passage_route_refuses_passage_not_in_users_turns() {
    let Some((_g, pool)) = begin().await else {
        return;
    };
    let (user, run, rec) = ready(&pool, &HERNIA).await;
    let cited = add_passage(&pool, rec, 9001, "Cited text.", &[]).await;
    let uncited = add_passage(&pool, rec, 9002, "Uncited text.", &[]).await;
    let qa = approved_qa(&pool, rec, &user).await;
    let other_qa = approved_qa(&pool, rec, &user).await;
    // Position 1 is a Q&A, stored negated, as the service writes it.
    done_turn(&pool, run, &user, 1, &[cited, -qa]).await;

    let st = no_gemma(pool.clone());
    let e = user.email.as_str();
    let (status, b) = call(
        api_routes(st.clone()),
        get(e, &format!("/api/chat/passages/{cited}")),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(b["source"], "passage");
    assert_eq!(b["text"], "Cited text.");
    assert_eq!(b["id"], cited);
    // The reference behind ref number 1 comes with it.
    assert_eq!(b["references"][0]["n"], 1);
    assert!(b["references"][0]["title"].is_string());

    let (status, b) = call(
        api_routes(st.clone()),
        get(e, &format!("/api/chat/passages/{}", -qa)),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(b["source"], "qa");
    assert_eq!(b["text"], "The owner says so.");

    for id in [uncited, -other_qa, 0, -cited, qa + 5000] {
        let (status, _) = call(
            api_routes(st.clone()),
            get(e, &format!("/api/chat/passages/{id}")),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{id}");
    }
    // The positive qa id is not the Q&A's stored form: it is a passage lookup.
    let (status, _) = call(
        api_routes(st.clone()),
        get(e, &format!("/api/chat/passages/{qa}")),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // A Q&A rejected after it was cited is no longer shown.
    sqlx::query("UPDATE incisive.topic_qas SET status = 'rejected' WHERE id = $1")
        .bind(qa)
        .execute(&pool)
        .await
        .unwrap();
    let (status, _) = call(
        api_routes(st),
        get(e, &format!("/api/chat/passages/{}", -qa)),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn sources_from_a_real_turn_view_open_in_the_passage_route() {
    let Some((_g, pool)) = begin().await else {
        return;
    };
    let (user, run, rec) = ready(&pool, &HERNIA).await;
    let qa = approved_qa(&pool, rec, &user).await;
    let p = add_passage(&pool, rec, 9003, "Another.", &[]).await;
    let turn = done_turn(&pool, run, &user, 1, &[p, -qa]).await;
    let st = no_gemma(pool.clone());
    let e = user.email.as_str();
    let (_, v) = call(
        api_routes(st.clone()),
        get(e, &format!("/api/chat/turns/{turn}")),
    )
    .await;
    assert_eq!(v["sources"], json!([{"passage": p}, {"qa": qa}]));
    // The client turns `{"qa": n}` into `-n`, `{"passage": n}` into `n`.
    for (src, stored) in [("passage", p), ("qa", -qa)] {
        let (status, b) = call(
            api_routes(st.clone()),
            get(e, &format!("/api/chat/passages/{stored}")),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{src}");
        assert_eq!(b["source"], src);
    }
}

/// A pool of one connection: asking for a second inside the transaction would
/// wait until the pool gave up.
#[tokio::test]
async fn suggest_holds_one_connection() {
    let Some((_g, pool)) = begin().await else {
        return;
    };
    let (user, run, _) = ready(&pool, &HERNIA).await;
    let turn = done_turn(&pool, run, &user, 1, &[]).await;
    let one = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .acquire_timeout(std::time::Duration::from_secs(2))
        .connect(&std::env::var("TEST_DATABASE_URL").unwrap())
        .await
        .unwrap();
    let out = incisive_app::chat::store::suggest_turn(&one, user.id, turn)
        .await
        .expect("no second connection");
    assert!(matches!(
        out,
        incisive_app::chat::store::SuggestOutcome::Filed { created: true, .. }
    ));
}

#[tokio::test]
async fn suggest_is_idempotent_and_only_for_done_turns() {
    let Some((_g, pool)) = begin().await else {
        return;
    };
    let (user, run, _) = ready(&pool, &HERNIA).await;
    let st = no_gemma(pool.clone());
    let e = user.email.as_str();
    let suggest = |t: i64| post(e, &format!("/api/chat/turns/{t}/suggest"), json!({}));

    // Not done: pending, partial, not covered, unavailable.
    for (seq, status) in [
        (1, "pending"),
        (2, "partial"),
        (3, "not_covered"),
        (4, "unavailable"),
    ] {
        let t = add_turn(&pool, run, user.id, seq, status, "r", 0).await;
        let (code, b) = call(api_routes(st.clone()), suggest(t)).await;
        assert_eq!(code, StatusCode::CONFLICT, "{status}");
        assert_eq!(b["error"], "not_suggestable");
        if status == "pending" {
            sqlx::query("UPDATE incisive.chat_turns SET status = 'done' WHERE id = $1")
                .bind(t)
                .execute(&pool)
                .await
                .unwrap();
        }
    }
    assert_eq!(
        count(&pool, "SELECT count(*) FROM incisive.topic_qas").await,
        0
    );

    let t = done_turn(&pool, run, &user, 5, &[]).await;
    for _ in 0..3 {
        let (code, b) = call(api_routes(st.clone()), suggest(t)).await;
        assert_eq!(code, StatusCode::OK);
        assert_eq!(b["suggested"], true);
    }
    // Concurrent calls also make one.
    let t2 = done_turn(&pool, run, &user, 6, &[]).await;
    let (a, b) = tokio::join!(
        call(api_routes(st.clone()), suggest(t2)),
        call(api_routes(st.clone()), suggest(t2))
    );
    assert_eq!((a.0, b.0), (StatusCode::OK, StatusCode::OK));
    assert_eq!(
        count(
            &pool,
            "SELECT count(*) FROM incisive.topic_qas WHERE source_turn_id IS NOT NULL"
        )
        .await,
        2
    );
    assert_eq!(
        count(
            &pool,
            &format!("SELECT count(*) FROM incisive.topic_qas WHERE source_turn_id = {t}")
        )
        .await,
        1
    );
    assert!(
        sqlx::query_scalar::<_, bool>("SELECT suggested FROM incisive.chat_turns WHERE id = $1")
            .bind(t)
            .fetch_one(&pool)
            .await
            .unwrap()
    );
    // The turn view says so.
    let (_, v) = call(api_routes(st), get(e, &format!("/api/chat/turns/{t}"))).await;
    assert_eq!(v["suggested"], true);
}

#[tokio::test]
async fn suggest_files_under_resolved_topic() {
    let Some((_g, pool)) = begin().await else {
        return;
    };
    let (user, run, rec) = ready(&pool, &HERNIA).await;
    let record_topic: i64 =
        sqlx::query_scalar("SELECT topic_id FROM incisive.evidence_records WHERE id = $1")
            .bind(rec)
            .fetch_one(&pool)
            .await
            .unwrap();
    // The record's topic was merged into another since the record was made.
    let live: i64 = sqlx::query_scalar(
        "INSERT INTO incisive.topics (label, pairs) \
         SELECT 'live ' || label, pairs FROM incisive.topics WHERE id = $1 RETURNING id",
    )
    .bind(record_topic)
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query("UPDATE incisive.topics SET merged_into = $2 WHERE id = $1")
        .bind(record_topic)
        .bind(live)
        .execute(&pool)
        .await
        .unwrap();

    let t = done_turn(&pool, run, &user, 1, &[]).await;
    let st = no_gemma(pool.clone());
    let (code, _) = call(
        api_routes(st),
        post(
            &user.email,
            &format!("/api/chat/turns/{t}/suggest"),
            json!({}),
        ),
    )
    .await;
    assert_eq!(code, StatusCode::OK);
    let row: (i64, String, String, String, i64, Option<i64>) = sqlx::query_as(
        "SELECT topic_id, question, gemma_reply, status, author_user_id, source_turn_id \
         FROM incisive.topic_qas",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(row.0, live);
    assert_eq!(row.1, ZEBRA_Q);
    assert_eq!(row.2, ZEBRA_REPLY);
    assert_eq!(row.3, "suggested");
    assert_eq!(row.4, user.id);
    assert_eq!(row.5, Some(t));
}

#[tokio::test]
async fn feedback_takes_one_or_minus_one_only() {
    let Some((_g, pool)) = begin().await else {
        return;
    };
    let (user, run, _) = ready(&pool, &HERNIA).await;
    let t = done_turn(&pool, run, &user, 1, &[]).await;
    let st = no_gemma(pool.clone());
    let e = user.email.as_str();
    let uri = format!("/api/chat/turns/{t}/feedback");
    for bad in [
        json!({"value": 0}),
        json!({"value": 2}),
        json!({"value": "1"}),
        json!({}),
        json!([]),
    ] {
        let (code, _) = call(api_routes(st.clone()), post(e, &uri, bad.clone())).await;
        assert_eq!(code, StatusCode::BAD_REQUEST, "{bad}");
    }
    let (code, _) = call(api_routes(st.clone()), post_raw(e, &uri, "not json".into())).await;
    assert_eq!(code, StatusCode::BAD_REQUEST);
    assert_eq!(
        count(
            &pool,
            "SELECT count(*) FROM incisive.chat_turns WHERE feedback IS NOT NULL"
        )
        .await,
        0
    );

    for v in [1, -1] {
        let (code, _) = call(api_routes(st.clone()), post(e, &uri, json!({"value": v}))).await;
        assert_eq!(code, StatusCode::OK);
        let (_, view) = call(
            api_routes(st.clone()),
            get(e, &format!("/api/chat/turns/{t}")),
        )
        .await;
        assert_eq!(view["feedback"], v);
    }
}

#[tokio::test]
async fn body_limit_applies() {
    let Some((_g, pool)) = begin().await else {
        return;
    };
    let (user, run, _) = ready(&pool, &HERNIA).await;
    let t = done_turn(&pool, run, &user, 1, &[]).await;
    let server = gemma_says(ZEBRA_REPLY, 0).await;
    let st = with_gemma(pool.clone(), &server.uri());
    let big = "a".repeat(70 * 1024);
    for (uri, body) in [
        (
            format!("/api/runs/{run}/chat"),
            json!({"question": big, "mode": "clinical"}),
        ),
        (
            format!("/api/chat/turns/{t}/feedback"),
            json!({"value": 1, "pad": big}),
        ),
    ] {
        // (`suggest` reads no body, so there is nothing for the limit to cut.)
        let (code, _) = call(api_routes(st.clone()), post(&user.email, &uri, body)).await;
        assert_eq!(code, StatusCode::PAYLOAD_TOO_LARGE, "{uri}");
    }
    assert_eq!(
        count(&pool, "SELECT count(*) FROM incisive.chat_turns").await,
        1
    );
}

#[tokio::test]
async fn ask_and_read_back_through_the_routes() {
    let Some((_g, pool)) = begin().await else {
        return;
    };
    let (user, run, _) = ready(&pool, &HERNIA).await;
    let server = gemma_says(ZEBRA_REPLY, 1).await;
    let st = with_gemma(pool.clone(), &server.uri());
    let e = user.email.as_str();

    let (code, b) = call(
        api_routes(st.clone()),
        get(e, &format!("/api/runs/{run}/chat")),
    )
    .await;
    assert_eq!(code, StatusCode::OK);
    assert_eq!(b["state"], "ready");
    assert_eq!(b["footer"], lines::FOOTER);
    assert!(b["turns"].as_array().unwrap().is_empty());

    let (code, b) = call(
        api_routes(st.clone()),
        post(e, &format!("/api/runs/{run}/chat"), ask_body(ZEBRA_Q)),
    )
    .await;
    assert_eq!(code, StatusCode::ACCEPTED);
    assert_eq!(b["safety_line"], false);
    let id = b["turn_id"].as_i64().unwrap();
    wait_turn(&pool, id).await;

    let (_, v) = call(
        api_routes(st.clone()),
        get(e, &format!("/api/chat/turns/{id}")),
    )
    .await;
    assert_eq!(v["status"], "done");
    assert_eq!(v["reply"], ZEBRA_REPLY);
    let (_, b) = call(api_routes(st), get(e, &format!("/api/runs/{run}/chat"))).await;
    assert_eq!(b["turns"][0]["id"], id);
    assert!(b["conversation_id"].is_i64());
}

#[tokio::test]
async fn errors_map_to_statuses_and_carry_the_safety_line() {
    let Some((_g, pool)) = begin().await else {
        return;
    };
    let (user, run, _) = ready(&pool, &HERNIA).await;
    let server = gemma_says(ZEBRA_REPLY, 0).await;
    let mut st = with_gemma(pool.clone(), &server.uri());
    let e = user.email.as_str();
    let uri = format!("/api/runs/{run}/chat");
    let ask = |st: &AppState, q: &str| {
        let app = api_routes(st.clone());
        let r = post(e, &uri, ask_body(q));
        async move { call(app, r).await }
    };

    // Empty.
    let (code, b) = ask(&st, "   ").await;
    assert_eq!(
        (code, b["error"].as_str()),
        (StatusCode::BAD_REQUEST, Some("empty"))
    );
    assert_eq!(b["safety_line"], false);
    // Too long, with a symptom past any cut.
    let long = format!("chest pain {}", "x".repeat(1100));
    let (code, b) = ask(&st, &long).await;
    assert_eq!(
        (code, b["error"].as_str()),
        (StatusCode::BAD_REQUEST, Some("too_long"))
    );
    assert_eq!(b["safety_line"], true);
    assert_eq!(b["lines"], json!([lines::WARNING_LINE]));
    // Identifiers: the Ask page's message, the line when it applies.
    let (code, b) = ask(&st, "Mr Sharma, 12 March, has chest pain").await;
    assert_eq!(code, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(b["message"].as_str().unwrap().starts_with(BLOCKED_MESSAGE));
    assert_eq!(b["safety_line"], true);
    assert_eq!(b["lines"], json!([lines::WARNING_LINE]));
    let (code, b) = ask(&st, "Mr Sharma, 12 March, what about his hernia?").await;
    assert_eq!(code, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(b["lines"], json!([]));
    assert_eq!(
        count(&pool, "SELECT count(*) FROM incisive.chat_turns").await,
        0
    );
    // Daily limit.
    st.chat.daily_limit = 0;
    let (code, b) = ask(&st, "chest pain and the zebra recurrence?").await;
    assert_eq!(
        (code, b["error"].as_str()),
        (StatusCode::TOO_MANY_REQUESTS, Some("daily_limit"))
    );
    assert_eq!(b["safety_line"], true);
    assert_eq!(b["lines"], json!([lines::WARNING_LINE]));
    st.chat.daily_limit = 30;
    // In flight, then busy.
    let other = add_turn(&pool, run, user.id, 1, "pending", "", 0).await;
    let (code, b) = ask(&st, ZEBRA_Q).await;
    assert_eq!(
        (code, b["error"].as_str()),
        (StatusCode::CONFLICT, Some("in_flight"))
    );
    sqlx::query("UPDATE incisive.chat_turns SET status = 'done' WHERE id = $1")
        .bind(other)
        .execute(&pool)
        .await
        .unwrap();
    for _ in 0..3 {
        other_pending(&pool, run, 0).await;
    }
    let res = api_routes(st.clone())
        .oneshot(post(e, &uri, ask_body("chest pain zebra?")))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::TOO_MANY_REQUESTS);
    assert!(res.headers().contains_key("retry-after"));
    let b: Value =
        serde_json::from_slice(&res.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(b["error"], "busy");
    assert_eq!(b["message"], lines::BUSY);
    assert_eq!(b["lines"], json!([lines::WARNING_LINE]));
    // Unavailable (no Gemma client).
    let no = no_gemma(pool.clone());
    let (code, b) = ask(&no, "chest pain and the zebra recurrence?").await;
    assert_eq!(
        (code, b["error"].as_str()),
        (StatusCode::SERVICE_UNAVAILABLE, Some("unavailable"))
    );
    assert_eq!(b["message"], lines::UNAVAILABLE);
    assert_eq!(b["lines"], json!([lines::WARNING_LINE]));
    // Not offered (a Quick run).
    let quick = seed_run(&pool, &user, "quick").await;
    let (code, b) = call(
        api_routes(st.clone()),
        post(
            e,
            &format!("/api/runs/{quick}/chat"),
            ask_body("chest pain?"),
        ),
    )
    .await;
    assert_eq!(
        (code, b["error"].as_str()),
        (StatusCode::CONFLICT, Some("not_offered"))
    );
    assert_eq!(b["lines"], json!([lines::WARNING_LINE]));
    // Bad mode, bad body.
    for body in [
        json!({"question": "q", "mode": "Clinical"}),
        json!({"question": "q"}),
        json!({"mode": "plain"}),
    ] {
        let (code, _) = call(api_routes(st.clone()), post(e, &uri, body)).await;
        assert_eq!(code, StatusCode::BAD_REQUEST);
    }
    // A bad mode does not hide symptoms in a question that could be read.
    for mode in [json!("Clinical"), json!(3), Value::Null] {
        let body = json!({"question": "I have a fever", "mode": mode});
        let (code, b) = call(api_routes(st.clone()), post(e, &uri, body)).await;
        assert_eq!(code, StatusCode::BAD_REQUEST);
        assert_eq!(b["safety_line"], json!(true));
        assert_eq!(b["lines"], json!([lines::WARNING_LINE]));
    }
    let body = json!({"question": "I have a fever"});
    let (code, b) = call(api_routes(st.clone()), post(e, &uri, body)).await;
    assert_eq!(code, StatusCode::BAD_REQUEST);
    assert_eq!(b["lines"], json!([lines::WARNING_LINE]));
    let body = json!({"question": "stay?", "mode": "Clinical"});
    let (code, b) = call(api_routes(st.clone()), post(e, &uri, body)).await;
    assert_eq!(code, StatusCode::BAD_REQUEST);
    assert_eq!(b["safety_line"], json!(false));
}

#[tokio::test]
async fn retention_removes_unapproved_qas_with_no_turn_and_priors_with_no_run_or_record_but_keeps_approved_and_linked()
 {
    let Some((_g, pool)) = begin().await else {
        return;
    };
    let (user, run, rec) = ready(&pool, &HERNIA).await;
    let topic: i64 =
        sqlx::query_scalar("SELECT topic_id FROM incisive.evidence_records WHERE id = $1")
            .bind(rec)
            .fetch_one(&pool)
            .await
            .unwrap();
    let turn = done_turn(&pool, run, &user, 1, &[]).await;
    let qa = |status: &'static str, turn: Option<i64>| {
        let pool = pool.clone();
        let uid = user.id;
        async move {
            sqlx::query_scalar::<_, i64>(
                "INSERT INTO incisive.topic_qas \
                 (topic_id, source_turn_id, question, gemma_reply, owner_text, status, author_user_id) \
                 VALUES ($1, $2, 'q', 'r', CASE WHEN $3 = 'approved' THEN 't' END, $3, $4) RETURNING id",
            )
            .bind(topic)
            .bind(turn)
            .bind(status)
            .bind(uid)
            .fetch_one(&pool)
            .await
            .unwrap()
        }
    };
    let gone_suggested = qa("suggested", None).await;
    let gone_rejected = qa("rejected", None).await;
    let kept_approved = qa("approved", None).await;
    let kept_linked = qa("suggested", Some(turn)).await;

    let prior = |run: Option<Uuid>, rec: Option<i64>| {
        let pool = pool.clone();
        async move {
            sqlx::query_scalar::<_, i64>(
                "INSERT INTO incisive.chat_priors (run_id, record_id, status) \
                 VALUES ($1, $2, 'written') RETURNING id",
            )
            .bind(run)
            .bind(rec)
            .fetch_one(&pool)
            .await
            .unwrap()
        }
    };
    let gone_prior = prior(None, None).await;
    let kept_by_record = prior(None, Some(rec)).await;
    let kept_by_run = prior(Some(run), None).await;
    let other_run = seed_run(&pool, &user, "focused").await;
    let kept_both = prior(Some(other_run), Some(rec)).await;

    let (qas, priors) = store::retention_pass(&pool).await.unwrap();
    assert_eq!((qas, priors), (2, 1));

    let left_qas: Vec<i64> = sqlx::query_scalar("SELECT id FROM incisive.topic_qas ORDER BY id")
        .fetch_all(&pool)
        .await
        .unwrap();
    assert_eq!(left_qas, vec![kept_approved, kept_linked]);
    assert!(!left_qas.contains(&gone_suggested) && !left_qas.contains(&gone_rejected));
    let left_priors: Vec<i64> =
        sqlx::query_scalar("SELECT id FROM incisive.chat_priors ORDER BY id")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(left_priors, vec![kept_by_record, kept_by_run, kept_both]);
    assert!(!left_priors.contains(&gone_prior));

    // A second pass finds nothing, and a run's deletion frees its Q&A for the next.
    assert_eq!(store::retention_pass(&pool).await.unwrap(), (0, 0));
    sqlx::query("DELETE FROM incisive.runs WHERE id = $1")
        .bind(run)
        .execute(&pool)
        .await
        .unwrap();
    let (qas, _) = store::retention_pass(&pool).await.unwrap();
    assert_eq!(qas, 1);
    let left: Vec<i64> = sqlx::query_scalar("SELECT id FROM incisive.topic_qas")
        .fetch_all(&pool)
        .await
        .unwrap();
    assert_eq!(left, vec![kept_approved]);
}
fn line(content: &str) -> String {
    format!(
        "{}\n",
        json!({"message": {"role": "assistant", "content": content}, "done": false})
    )
}

const DONE: &str = "{\"done\":true,\"done_reason\":\"stop\"}\n";

fn ndjson(reply: &str) -> ResponseTemplate {
    ResponseTemplate::new(200)
        .set_body_raw(format!("{}{DONE}", line(reply)), "application/x-ndjson")
}

async fn gemma_says(reply: &str, calls: u64) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/chat"))
        .respond_with(ndjson(reply))
        .expect(calls)
        .mount(&server)
        .await;
    server
}

/// Polls straight from the table until the turn has finished.
async fn wait_turn(pool: &PgPool, id: i64) -> String {
    for _ in 0..600 {
        let s: String = sqlx::query_scalar("SELECT status FROM incisive.chat_turns WHERE id = $1")
            .bind(id)
            .fetch_one(pool)
            .await
            .unwrap();
        if s != "pending" {
            return s;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("turn {id} never finished");
}
