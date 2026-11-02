//! `chat::service`: the ask flow against the database (`TEST_DATABASE_URL`)
//! with a fake Gemma.

use std::sync::Arc;
use std::time::Duration;

use incisive_app::agent::temporal_writer::TemporalSettings;
use incisive_app::agent::testing::{offline_literature, placeholder_pipeline};
use incisive_app::ask::BLOCKED_MESSAGE;
use incisive_app::chat::gemma::GemmaChat;
use incisive_app::chat::service::{
    self, Asked, ChatError, ChatState, GEMMA_LIMIT, TurnSource, TurnView,
};
use incisive_app::chat::settings::ChatSettings;
use incisive_app::chat::{ChatMode, job, lines, store};
use incisive_app::db::{lazy_pool, migrate};
use incisive_app::gate::OfflineClassifier;
use incisive_app::identity::{IapVerifier, JwkSource, Role, User};
use incisive_app::memory::embed::Embedder;
use incisive_app::memory::job as memory_job;
use incisive_app::memory::settings::MemorySettings;
use incisive_app::runs::RunsConfig;
use incisive_app::runs::worker::WorkerHandle;
use incisive_app::server::AppState;
use incisive_app::spend::{CapsConfig, Prices};
use secrecy::SecretString;
use serde_json::{Value, json};
use sqlx::PgPool;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use uuid::Uuid;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

const ZEBRA_Q: &str = "What is the zebra recurrence?";
const ZEBRA_TEXT: &str = "Zebra recurrence was 5 % in this series.";
const ZEBRA_REPLY: &str = "Zebra recurrence was 5% [1].";
/// Words no passage holds, whatever the fixture.
const OFF_TOPIC: &str = "giraffe xylophone quantum";
const C8_QUESTION: &str =
    "If I have robotic TAR instead of open, will I go home sooner, and how sure is that?";

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
         incisive.chat_conversations, incisive.topic_qas, incisive.memory_jobs, \
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
const ACHALASIA: Fx = Fx {
    body: include_str!("fixtures/temporal/full/achalasia.json"),
    slim: include_str!("fixtures/temporal/achalasia.json"),
    board: include_str!("fixtures/temporal/boards/achalasia-board.json"),
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

async fn gemma_status(code: u16) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/chat"))
        .respond_with(ResponseTemplate::new(code))
        .mount(&server)
        .await;
    server
}

/// A slow Gemma: a chunked 200, then each `(delay, text)` as its own chunk,
/// then the connection is held open if `hold`, or closed.
async fn raw_gemma(pieces: Vec<(Duration, String)>, hold: bool) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (mut sock, _) = listener.accept().await.unwrap();
        let mut buf = vec![0u8; 65536];
        let _ = sock.read(&mut buf).await;
        let head = "HTTP/1.1 200 OK\r\ncontent-type: application/x-ndjson\r\n\
                    transfer-encoding: chunked\r\n\r\n";
        let _ = sock.write_all(head.as_bytes()).await;
        for (delay, text) in pieces {
            tokio::time::sleep(delay).await;
            let chunk = format!("{:x}\r\n{text}\r\n", text.len());
            let _ = sock.write_all(chunk.as_bytes()).await;
            let _ = sock.flush().await;
        }
        if hold {
            tokio::time::sleep(Duration::from_secs(120)).await;
        }
    });
    format!("http://{addr}")
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

async fn finished(st: &AppState, user: &User, a: Asked) -> TurnView {
    wait_turn(&st.pool, a.turn_id).await;
    service::turn_view(st, user, a.turn_id).await.unwrap()
}

async fn stored_reply(pool: &PgPool, id: i64) -> String {
    sqlx::query_scalar("SELECT reply FROM incisive.chat_turns WHERE id = $1")
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap()
}

/// A `pending` turn by a new user on `run`, last written `age_secs` ago.
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

async fn turn_count(pool: &PgPool, user: i64) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM incisive.chat_turns WHERE user_id = $1")
        .bind(user)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn sent_messages(server: &MockServer, call: usize) -> Vec<(String, String)> {
    let reqs = server.received_requests().await.unwrap();
    let body: Value = serde_json::from_slice(&reqs[call].body).unwrap();
    body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| {
            (
                m["role"].as_str().unwrap().to_string(),
                m["content"].as_str().unwrap().to_string(),
            )
        })
        .collect()
}

// ---------- the no-passage rule (C8) ----------

#[tokio::test]
async fn no_passage_above_threshold_never_calls_gemma() {
    let Some((_g, pool)) = begin().await else {
        return;
    };
    let (user, run, _) = ready(&pool, &ACHALASIA).await;
    let server = gemma_says("should never be sent", 0).await;
    let st = with_gemma(pool.clone(), &server.uri());
    let a = service::ask(&st, &user, run, C8_QUESTION, ChatMode::Clinical)
        .await
        .unwrap();
    let v = finished(&st, &user, a).await;
    assert_eq!(v.status, "not_covered");
    assert_eq!(v.reply, lines::NOT_COVERED);
    assert!(v.sources.is_empty());
    // A personal question: the surgeon line goes with it.
    assert!(v.personal);
    assert!(v.lines.contains(&lines::SEE_YOUR_SURGEON));
    assert_eq!(server.received_requests().await.unwrap().len(), 0);
}

// ---------- the normal path ----------

#[tokio::test]
async fn normal_question_stores_reply_passages_checks_timing_mode() {
    let Some((_g, pool)) = begin().await else {
        return;
    };
    let (user, run, rec) = ready(&pool, &HERNIA).await;
    let zebra: i64 = sqlx::query_scalar(
        "SELECT id FROM incisive.chat_passages WHERE record_id = $1 AND seq = 9000",
    )
    .bind(rec)
    .fetch_one(&pool)
    .await
    .unwrap();
    let server = gemma_says(ZEBRA_REPLY, 1).await;
    let st = with_gemma(pool.clone(), &server.uri());
    let a = service::ask(&st, &user, run, ZEBRA_Q, ChatMode::Plain)
        .await
        .unwrap();
    assert!(!a.safety_line);
    let v = finished(&st, &user, a).await;
    assert_eq!(v.status, "done");
    assert_eq!(v.reply, ZEBRA_REPLY);
    assert_eq!(v.question, ZEBRA_Q);
    assert_eq!(v.mode, "plain");
    assert_eq!(v.seq, 1);
    assert_eq!(v.sources[0], TurnSource::Passage(zebra));
    assert!(v.checks.as_ref().unwrap().is_clean(), "{:?}", v.checks);
    assert!(v.latency_ms.is_some());
    assert!(v.lines.is_empty());
    let m = sent_messages(&server, 0).await;
    assert!(m[0].1.contains("plain, everyday"));
    let last = &m.last().unwrap().1;
    assert!(last.contains(&format!("[1] {ZEBRA_TEXT}")));
    assert!(last.ends_with(&format!("Question: {ZEBRA_Q}")));
    // Listed on the run, too.
    let view = service::state_for(&st, &user, run).await.unwrap();
    assert_eq!(view.state, ChatState::Ready);
    assert!(view.conversation_id.is_some());
    assert_eq!(view.turns, vec![v]);
    assert_eq!(view.starters.len(), 3);
}

#[tokio::test]
async fn reply_flags_are_stored_and_text_kept() {
    let Some((_g, pool)) = begin().await else {
        return;
    };
    let (user, run, _) = ready(&pool, &HERNIA).await;
    let reply = "You should know zebra was 77% [9].";
    let server = gemma_says(reply, 1).await;
    let st = with_gemma(pool.clone(), &server.uri());
    let a = service::ask(&st, &user, run, ZEBRA_Q, ChatMode::Clinical)
        .await
        .unwrap();
    let v = finished(&st, &user, a).await;
    assert_eq!(v.status, "done");
    assert_eq!(v.reply, reply, "flag, never delete");
    let c = v.checks.unwrap();
    assert!(c.you_should);
    assert_eq!(c.unknown_citations, vec![9]);
    assert!(!c.not_traced.is_empty());
}

#[tokio::test]
async fn comparison_check_uses_the_asked_key() {
    let Some((_g, pool)) = begin().await else {
        return;
    };
    let (user, run, rec) = ready(&pool, &HERNIA).await;
    add_passage(
        &pool,
        rec,
        9001,
        "Zebra stay after open repair",
        &["open|robotic"],
    )
    .await;
    let server = gemma_says("Zebra favoured laparoscopic repair [1].", 1).await;
    let st = with_gemma(pool.clone(), &server.uri());
    let a = service::ask(
        &st,
        &user,
        run,
        "Is robotic better than open for zebra stay?",
        ChatMode::Clinical,
    )
    .await
    .unwrap();
    let v = finished(&st, &user, a).await;
    assert_eq!(
        v.checks.unwrap().comparison_mismatch,
        vec!["laparoscopic".to_string()]
    );
}

#[tokio::test]
async fn topic_qa_keeps_its_citation_position() {
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
    let qa: i64 = sqlx::query_scalar(
        "INSERT INTO incisive.topic_qas \
         (topic_id, question, gemma_reply, owner_text, status, author_user_id) \
         VALUES ($1, 'zebra?', 'g', 'Zebra recurrence reviewed by the owner', 'approved', $2) \
         RETURNING id",
    )
    .bind(topic)
    .bind(user.id)
    .fetch_one(&pool)
    .await
    .unwrap();
    let server = gemma_says(ZEBRA_REPLY, 1).await;
    let st = with_gemma(pool.clone(), &server.uri());
    let a = service::ask(&st, &user, run, ZEBRA_Q, ChatMode::Clinical)
        .await
        .unwrap();
    let v = finished(&st, &user, a).await;
    let last = sent_messages(&server, 0).await.pop().unwrap().1;
    // Position i in `sources` is citation [i + 1] as Gemma saw it.
    let pos = v
        .sources
        .iter()
        .position(|s| *s == TurnSource::Qa(qa))
        .expect("the Q&A is a source");
    assert!(last.contains(&format!(
        "[{}] Zebra recurrence reviewed by the owner",
        pos + 1
    )));
}

// ---------- Review Focus 1: the safety line on every path ----------

#[tokio::test]
async fn safety_line_on_every_path() {
    let Some((_g, pool)) = begin().await else {
        return;
    };
    let (user, run, _) = ready(&pool, &HERNIA).await;

    // Normal.
    let server = gemma_says(ZEBRA_REPLY, 1).await;
    let st = with_gemma(pool.clone(), &server.uri());
    let a = service::ask(
        &st,
        &user,
        run,
        "chest pain and the zebra recurrence?",
        ChatMode::Clinical,
    )
    .await
    .unwrap();
    assert!(a.safety_line);
    let v = finished(&st, &user, a).await;
    assert_eq!(v.status, "done");
    assert!(v.safety_line);
    assert!(v.lines.contains(&lines::WARNING_LINE));

    // Not covered.
    let q = format!("chest pain {OFF_TOPIC}");
    let a = service::ask(&st, &user, run, &q, ChatMode::Clinical)
        .await
        .unwrap();
    assert!(a.safety_line);
    let v = finished(&st, &user, a).await;
    assert_eq!(v.status, "not_covered");
    assert!(v.lines.contains(&lines::WARNING_LINE));

    // Identifiers.
    match service::ask(
        &st,
        &user,
        run,
        "Mr Sharma, 12 March, has chest pain",
        ChatMode::Clinical,
    )
    .await
    {
        Err(ChatError::Identifiers { safety_line, .. }) => assert!(safety_line),
        other => panic!("{other:?}"),
    }

    // Gemma down, after the turn started.
    let down = gemma_status(503).await;
    let st_down = with_gemma(pool.clone(), &down.uri());
    let a = service::ask(
        &st_down,
        &user,
        run,
        "chest pain and the zebra recurrence?",
        ChatMode::Clinical,
    )
    .await
    .unwrap();
    assert!(a.safety_line);
    let v = finished(&st_down, &user, a).await;
    assert_eq!(v.status, "unavailable");
    assert_eq!(v.reply, lines::UNAVAILABLE);
    assert!(v.lines.contains(&lines::WARNING_LINE));

    // No Gemma at all.
    assert_eq!(
        service::ask(
            &no_gemma(pool.clone()),
            &user,
            run,
            "severe pain and zebra?",
            ChatMode::Clinical
        )
        .await
        .unwrap_err(),
        ChatError::Unavailable { safety_line: true }
    );

    // Daily limit.
    let mut limited = st.clone();
    limited.chat.daily_limit = 0;
    assert_eq!(
        service::ask(&limited, &user, run, "I am vomiting", ChatMode::Clinical)
            .await
            .unwrap_err(),
        ChatError::DailyLimit { safety_line: true }
    );

    // Too long: the symptom may sit past any point, so all of it is read.
    let long = format!("{} I am bleeding heavily", "a ".repeat(1000));
    assert_eq!(
        service::ask(&st, &user, run, &long, ChatMode::Clinical)
            .await
            .unwrap_err(),
        ChatError::TooLong { safety_line: true }
    );

    // Not offered.
    let quick = seed_run(&pool, &user, "quick").await;
    assert_eq!(
        service::ask(
            &st,
            &user,
            quick,
            "I am bleeding heavily",
            ChatMode::Clinical
        )
        .await
        .unwrap_err(),
        ChatError::NotOffered { safety_line: true }
    );

    // A reply already in flight.
    add_turn(&pool, run, user.id, 100, "pending", "", 0).await;
    assert_eq!(
        service::ask(&st, &user, run, "I am bleeding heavily", ChatMode::Clinical)
            .await
            .unwrap_err(),
        ChatError::InFlight { safety_line: true }
    );

    // Busy.
    for _ in 0..3 {
        other_pending(&pool, run, 0).await;
    }
    assert_eq!(
        service::ask(&st, &user, run, "there is pus", ChatMode::Clinical)
            .await
            .unwrap_err(),
        ChatError::Busy { safety_line: true }
    );
    // And no line without symptoms.
    assert_eq!(
        service::ask(&st, &user, run, ZEBRA_Q, ChatMode::Clinical)
            .await
            .unwrap_err(),
        ChatError::Busy { safety_line: false }
    );
}

// ---------- Review Focus 2: identifiers ----------

#[tokio::test]
async fn identifier_blocks_before_store_and_gemma() {
    let Some((_g, pool)) = begin().await else {
        return;
    };
    let (user, run, _) = ready(&pool, &HERNIA).await;
    let server = gemma_says("never", 0).await;
    let st = with_gemma(pool.clone(), &server.uri());
    let q = "Mr Sharma, 12 March, what about his hernia?";
    let err = service::ask(&st, &user, run, q, ChatMode::Clinical)
        .await
        .unwrap_err();
    let ChatError::Identifiers {
        message,
        safety_line,
    } = err
    else {
        panic!("{err:?}");
    };
    assert!(message.starts_with(BLOCKED_MESSAGE), "{message}");
    assert!(!safety_line);
    assert_eq!(turn_count(&pool, user.id).await, 0);
    let convs: i64 =
        sqlx::query_scalar("SELECT count(*) FROM incisive.chat_conversations WHERE user_id = $1")
            .bind(user.id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(convs, 0);
    assert_eq!(server.received_requests().await.unwrap().len(), 0);

    // The audit log holds the kinds and offsets only.
    let rows: Vec<(String, Value)> =
        sqlx::query_as("SELECT event, detail FROM incisive.audit WHERE user_id = $1")
            .bind(user.id)
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].0, "deid_blocked");
    let detail = rows[0].1.to_string();
    assert!(detail.contains("calendar_date"), "{detail}");
    assert!(!detail.contains("Sharma"), "{detail}");
    assert!(!detail.contains("March"), "{detail}");
    assert!(!detail.contains("hernia"), "{detail}");

    // The Ask page's hint comes along.
    let err = service::ask(
        &st,
        &user,
        run,
        "Is \u{202e}robotic better?",
        ChatMode::Clinical,
    )
    .await
    .unwrap_err();
    let ChatError::Identifiers { message, .. } = err else {
        panic!("{err:?}");
    };
    assert!(message.contains("Please retype the question"), "{message}");
}

#[tokio::test]
async fn empty_and_too_long_are_refused_first() {
    let Some((_g, pool)) = begin().await else {
        return;
    };
    let (user, run, _) = ready(&pool, &HERNIA).await;
    let st = no_gemma(pool.clone());
    assert_eq!(
        service::ask(&st, &user, run, "  \n ", ChatMode::Clinical)
            .await
            .unwrap_err(),
        ChatError::Empty
    );
    // Length comes before everything: not even a run check.
    assert_eq!(
        service::ask(
            &st,
            &user,
            Uuid::new_v4(),
            &"a".repeat(1001),
            ChatMode::Clinical
        )
        .await
        .unwrap_err(),
        ChatError::TooLong { safety_line: false }
    );
    // 1000 is allowed through (then stops for no Gemma). Characters, not bytes.
    assert_eq!(
        service::ask(&st, &user, run, &"é".repeat(1000), ChatMode::Clinical)
            .await
            .unwrap_err(),
        ChatError::Unavailable { safety_line: false }
    );
}

// ---------- Review Focus 4: the caller goes away; instances die ----------

#[tokio::test]
async fn reply_finishes_after_caller_goes_away() {
    let Some((_g, pool)) = begin().await else {
        return;
    };
    let (user, run, _) = ready(&pool, &HERNIA).await;
    let url = raw_gemma(
        vec![
            (Duration::from_millis(50), line("Zebra recurrence ")),
            (Duration::from_millis(300), line("was 5% [1].")),
            (Duration::ZERO, DONE.to_string()),
        ],
        false,
    )
    .await;
    let st = with_gemma(pool.clone(), &url);
    // The caller is a task that is gone by the time the reply arrives, and
    // nobody polls through the service.
    let (st2, user2) = (st.clone(), user.clone());
    let caller = tokio::spawn(async move {
        service::ask(&st2, &user2, run, ZEBRA_Q, ChatMode::Clinical)
            .await
            .unwrap()
    });
    let a = caller.await.unwrap();
    assert_eq!(wait_turn(&pool, a.turn_id).await, "done");
    assert_eq!(stored_reply(&pool, a.turn_id).await, ZEBRA_REPLY);
}

#[tokio::test]
async fn second_ask_while_pending_is_in_flight() {
    let Some((_g, pool)) = begin().await else {
        return;
    };
    let (user, run, _) = ready(&pool, &HERNIA).await;
    let url = raw_gemma(vec![(Duration::from_secs(2), line("x"))], true).await;
    let st = with_gemma(pool.clone(), &url);
    let a = service::ask(&st, &user, run, ZEBRA_Q, ChatMode::Clinical)
        .await
        .unwrap();
    assert_eq!(
        service::ask(&st, &user, run, ZEBRA_Q, ChatMode::Clinical)
            .await
            .unwrap_err(),
        ChatError::InFlight { safety_line: false }
    );
    assert_eq!(turn_count(&pool, user.id).await, 1);
    let v = service::turn_view(&st, &user, a.turn_id).await.unwrap();
    assert_eq!(v.status, "pending");
}

#[tokio::test]
async fn dead_instance_turn_unblocks_user() {
    let Some((_g, pool)) = begin().await else {
        return;
    };
    let (user, run, _) = ready(&pool, &HERNIA).await;
    let server = gemma_says(ZEBRA_REPLY, 1).await;
    let st = with_gemma(pool.clone(), &server.uri());

    // Written 30 s ago: still in flight.
    let t = add_turn(&pool, run, user.id, 1, "pending", "half a", 30).await;
    assert_eq!(
        service::ask(&st, &user, run, ZEBRA_Q, ChatMode::Clinical)
            .await
            .unwrap_err(),
        ChatError::InFlight { safety_line: false }
    );
    // Written 91 s ago: its instance is dead; the ask itself frees the slot.
    sqlx::query(
        "UPDATE incisive.chat_turns SET updated_at = now() - interval '91 seconds' WHERE id = $1",
    )
    .bind(t)
    .execute(&pool)
    .await
    .unwrap();
    let a = service::ask(&st, &user, run, ZEBRA_Q, ChatMode::Clinical)
        .await
        .unwrap();
    assert_eq!(wait_turn(&pool, t).await, "partial");
    assert_eq!(stored_reply(&pool, t).await, "half a");
    assert_eq!(wait_turn(&pool, a.turn_id).await, "done");
    let seq: i32 = sqlx::query_scalar("SELECT seq FROM incisive.chat_turns WHERE id = $1")
        .bind(a.turn_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(seq, 2, "same conversation, next number");
}

#[tokio::test]
async fn updated_at_moves_while_streaming_and_text_shows() {
    let Some((_g, pool)) = begin().await else {
        return;
    };
    let (user, run, _) = ready(&pool, &HERNIA).await;
    let url = raw_gemma(
        vec![
            (Duration::from_millis(50), line("Zebra ")),
            (Duration::from_millis(1500), line("recurrence ")),
            (Duration::from_millis(1500), line("was 5% [1].")),
            (Duration::ZERO, DONE.to_string()),
        ],
        false,
    )
    .await;
    let st = with_gemma(pool.clone(), &url);
    let a = service::ask(&st, &user, run, ZEBRA_Q, ChatMode::Clinical)
        .await
        .unwrap();
    let read = || async {
        let r: (String, String, chrono::DateTime<chrono::Utc>) = sqlx::query_as(
            "SELECT status, reply, updated_at FROM incisive.chat_turns WHERE id = $1",
        )
        .bind(a.turn_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        r
    };
    let mut first = None;
    let mut moved = false;
    for _ in 0..300 {
        let (status, reply, at) = read().await;
        if status != "pending" {
            break;
        }
        if reply == "Zebra " && first.is_none() {
            // The sources are written before the first text.
            let v = service::turn_view(&st, &user, a.turn_id).await.unwrap();
            assert_eq!(v.reply, "Zebra ", "text so far is visible");
            assert!(!v.sources.is_empty());
            first = Some(at);
        }
        if let Some(f) = first
            && reply == "Zebra recurrence "
        {
            assert!(at > f, "updated_at must follow each write");
            moved = true;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(first.is_some(), "text never showed while pending");
    assert!(moved, "second write never showed while pending");
    assert_eq!(wait_turn(&pool, a.turn_id).await, "done");
    assert_eq!(stored_reply(&pool, a.turn_id).await, ZEBRA_REPLY);
}

#[tokio::test]
async fn writer_does_not_overwrite_a_turn_the_sweep_finished() {
    let Some((_g, pool)) = begin().await else {
        return;
    };
    let (user, run, _) = ready(&pool, &HERNIA).await;
    let url = raw_gemma(
        vec![
            (Duration::from_millis(50), line("Zebra ")),
            (Duration::from_millis(1500), line("recurrence [1].")),
            (Duration::ZERO, DONE.to_string()),
        ],
        false,
    )
    .await;
    let st = with_gemma(pool.clone(), &url);
    let a = service::ask(&st, &user, run, ZEBRA_Q, ChatMode::Clinical)
        .await
        .unwrap();
    // Mid-stream: the first text is stored, more is on the way.
    for _ in 0..300 {
        if stored_reply(&pool, a.turn_id).await == "Zebra " {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(stored_reply(&pool, a.turn_id).await, "Zebra ");
    // The sweep got there first: it wrote its verdict on what was stored.
    sqlx::query("UPDATE incisive.chat_turns SET status = 'partial', reply = 'swept' WHERE id = $1")
        .bind(a.turn_id)
        .execute(&pool)
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(2500)).await;
    let (status, reply, latency): (String, String, Option<i32>) =
        sqlx::query_as("SELECT status, reply, latency_ms FROM incisive.chat_turns WHERE id = $1")
            .bind(a.turn_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!((status.as_str(), reply.as_str()), ("partial", "swept"));
    assert_eq!(latency, None);
}

#[tokio::test]
async fn every_turn_write_leaves_a_finished_turn_alone() {
    let Some((_g, pool)) = begin().await else {
        return;
    };
    let (user, run, _) = ready(&pool, &HERNIA).await;
    let t = add_turn(&pool, run, user.id, 1, "partial", "swept", 0).await;
    assert!(!store::set_turn_sources(&pool, t, &[1]).await.unwrap());
    assert!(!store::set_turn_reply(&pool, t, "late").await.unwrap());
    assert!(
        !store::finish_turn(&pool, t, "done", "late", None, 5)
            .await
            .unwrap()
    );
    assert!(!store::fail_turn(&pool, t).await.unwrap());
    let (status, reply, sources, latency): (String, String, Vec<i64>, Option<i32>) =
        sqlx::query_as(
            "SELECT status, reply, passage_ids, latency_ms FROM incisive.chat_turns WHERE id = $1",
        )
        .bind(t)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!((status.as_str(), reply.as_str()), ("partial", "swept"));
    assert!(sources.is_empty());
    assert_eq!(latency, None);
}

#[tokio::test]
async fn panic_in_the_task_finishes_unavailable() {
    let Some((_g, pool)) = begin().await else {
        return;
    };
    let (user, run, _) = ready(&pool, &HERNIA).await;
    let t = add_turn(&pool, run, user.id, 1, "pending", "", 0).await;
    service::guard_turn(pool.clone(), t, async { panic!("boom") }).await;
    assert_eq!(wait_turn(&pool, t).await, "unavailable");
    // A task that ran normally leaves the turn alone.
    let t2 = add_turn(&pool, run, user.id, 2, "pending", "", 0).await;
    service::guard_turn(pool.clone(), t2, async {}).await;
    let s: String = sqlx::query_scalar("SELECT status FROM incisive.chat_turns WHERE id = $1")
        .bind(t2)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(s, "pending");
}

// ---------- limits ----------

#[tokio::test]
async fn limit_counts_per_user_today_ist() {
    let Some((_g, pool)) = begin().await else {
        return;
    };
    let (user, run, _) = ready(&pool, &HERNIA).await;
    let server = gemma_says(ZEBRA_REPLY, 1).await;
    let mut st = with_gemma(pool.clone(), &server.uri());
    st.chat.daily_limit = 2;

    // Yesterday in IST, even if it is the same day in UTC, does not count.
    let old = add_turn(&pool, run, user.id, 1, "done", "x", 0).await;
    let old2 = add_turn(&pool, run, user.id, 2, "done", "x", 0).await;
    sqlx::query(
        "UPDATE incisive.chat_turns SET created_at = \
         (date_trunc('day', now() AT TIME ZONE 'Asia/Kolkata') AT TIME ZONE 'Asia/Kolkata') \
         - interval '1 minute' WHERE id = ANY($1)",
    )
    .bind(vec![old, old2])
    .execute(&pool)
    .await
    .unwrap();
    // Someone else's turns today do not count.
    for _ in 0..2 {
        let other = other_pending(&pool, run, 0).await;
        sqlx::query("UPDATE incisive.chat_turns SET status = 'done' WHERE id = $1")
            .bind(other)
            .execute(&pool)
            .await
            .unwrap();
    }
    // The first minute of today does count.
    let today = add_turn(&pool, run, user.id, 3, "done", "x", 0).await;
    sqlx::query(
        "UPDATE incisive.chat_turns SET created_at = \
         (date_trunc('day', now() AT TIME ZONE 'Asia/Kolkata') AT TIME ZONE 'Asia/Kolkata') \
         WHERE id = $1",
    )
    .bind(today)
    .execute(&pool)
    .await
    .unwrap();

    // One used of two: this one goes through.
    let a = service::ask(&st, &user, run, ZEBRA_Q, ChatMode::Clinical)
        .await
        .unwrap();
    assert_eq!(wait_turn(&pool, a.turn_id).await, "done");
    // Two used of two.
    assert_eq!(
        service::ask(&st, &user, run, ZEBRA_Q, ChatMode::Clinical)
            .await
            .unwrap_err(),
        ChatError::DailyLimit { safety_line: false }
    );
}

#[tokio::test]
async fn fourth_pending_is_busy() {
    let Some((_g, pool)) = begin().await else {
        return;
    };
    let (user, run, _) = ready(&pool, &HERNIA).await;
    let server = gemma_says(ZEBRA_REPLY, 1).await;
    let st = with_gemma(pool.clone(), &server.uri());
    let mut others = Vec::new();
    for _ in 0..3 {
        others.push(other_pending(&pool, run, 0).await);
    }
    assert_eq!(
        service::ask(&st, &user, run, ZEBRA_Q, ChatMode::Clinical)
            .await
            .unwrap_err(),
        ChatError::Busy { safety_line: false }
    );
    assert_eq!(turn_count(&pool, user.id).await, 0, "nothing stored");
    // A dead instance's turn is not in the queue.
    sqlx::query(
        "UPDATE incisive.chat_turns SET updated_at = now() - interval '91 seconds' WHERE id = $1",
    )
    .bind(others[0])
    .execute(&pool)
    .await
    .unwrap();
    let a = service::ask(&st, &user, run, ZEBRA_Q, ChatMode::Clinical)
        .await
        .unwrap();
    assert_eq!(wait_turn(&pool, a.turn_id).await, "done");
}

// ---------- Gemma failing ----------

#[tokio::test]
async fn gemma_down_gives_unavailable_and_answer_unaffected() {
    let Some((_g, pool)) = begin().await else {
        return;
    };
    let (user, run, rec) = ready(&pool, &HERNIA).await;
    let snapshot = || async {
        let a: Value =
            sqlx::query_scalar("SELECT body FROM incisive.run_answers WHERE run_id = $1")
                .bind(run)
                .fetch_one(&pool)
                .await
                .unwrap();
        let t: String =
            sqlx::query_scalar("SELECT status FROM incisive.run_temporal WHERE run_id = $1")
                .bind(run)
                .fetch_one(&pool)
                .await
                .unwrap();
        let r: Value =
            sqlx::query_scalar("SELECT answer FROM incisive.evidence_records WHERE id = $1")
                .bind(rec)
                .fetch_one(&pool)
                .await
                .unwrap();
        (a, t, r)
    };
    let before = snapshot().await;
    // Server error; then nothing listening at all.
    let down = gemma_status(503).await;
    let refused = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let dead_url = format!("http://{}", refused.local_addr().unwrap());
    drop(refused);
    for uri in [down.uri(), dead_url] {
        let st = with_gemma(pool.clone(), &uri);
        let a = service::ask(&st, &user, run, ZEBRA_Q, ChatMode::Clinical)
            .await
            .unwrap();
        let v = finished(&st, &user, a).await;
        assert_eq!(v.status, "unavailable");
        assert_eq!(v.reply, lines::UNAVAILABLE);
        assert_eq!(stored_reply(&pool, v.id).await, "");
    }
    assert_eq!(snapshot().await, before);
}

#[tokio::test]
async fn cut_at_60s_is_partial_with_text() {
    assert_eq!(GEMMA_LIMIT, Duration::from_secs(60));
    let Some((_g, pool)) = begin().await else {
        return;
    };
    let (user, run, _) = ready(&pool, &HERNIA).await;
    let url = raw_gemma(
        vec![(
            Duration::from_millis(20),
            line("Zebra recurrence was 5% [1]"),
        )],
        true,
    )
    .await;
    let st = with_gemma(pool.clone(), &url);
    let a = service::ask_limited(
        &st,
        &user,
        run,
        ZEBRA_Q,
        ChatMode::Clinical,
        Duration::from_millis(1000),
    )
    .await
    .unwrap();
    let v = finished(&st, &user, a).await;
    assert_eq!(v.status, "partial");
    assert_eq!(v.reply, "Zebra recurrence was 5% [1]");
    assert!(v.checks.is_some(), "a partial reply is still checked");
    // The limit with nothing written: no text, nothing to keep.
    let url = raw_gemma(vec![], true).await;
    let st = with_gemma(pool.clone(), &url);
    let a = service::ask_limited(
        &st,
        &user,
        run,
        ZEBRA_Q,
        ChatMode::Clinical,
        Duration::from_millis(500),
    )
    .await
    .unwrap();
    assert_eq!(wait_turn(&pool, a.turn_id).await, "unavailable");
}

#[tokio::test]
async fn stream_error_midway_keeps_text_as_partial() {
    let Some((_g, pool)) = begin().await else {
        return;
    };
    let (user, run, _) = ready(&pool, &HERNIA).await;
    // Text, then the connection closes with no `done` line.
    let url = raw_gemma(vec![(Duration::from_millis(20), line("Zebra was"))], false).await;
    let st = with_gemma(pool.clone(), &url);
    let a = service::ask(&st, &user, run, ZEBRA_Q, ChatMode::Clinical)
        .await
        .unwrap();
    let v = finished(&st, &user, a).await;
    assert_eq!(v.status, "partial");
    assert_eq!(v.reply, "Zebra was");
}

#[tokio::test]
async fn http_429_shows_the_busy_line() {
    let Some((_g, pool)) = begin().await else {
        return;
    };
    let (user, run, _) = ready(&pool, &HERNIA).await;
    let server = gemma_status(429).await;
    let st = with_gemma(pool.clone(), &server.uri());
    let a = service::ask(&st, &user, run, ZEBRA_Q, ChatMode::Clinical)
        .await
        .unwrap();
    let v = finished(&st, &user, a).await;
    assert_eq!(v.status, "unavailable");
    assert_eq!(v.reply, lines::BUSY);
    assert!(v.lines.is_empty());
    // The busy marker is not a reply flag.
    assert_eq!(v.checks, None);

    // With symptoms the line goes with the busy reply.
    let a = service::ask(
        &st,
        &user,
        run,
        "chest pain and the zebra recurrence?",
        ChatMode::Clinical,
    )
    .await
    .unwrap();
    assert!(a.safety_line);
    let v = finished(&st, &user, a).await;
    assert_eq!(v.reply, lines::BUSY);
    assert!(v.safety_line);
    assert!(v.lines.contains(&lines::WARNING_LINE));
}

#[tokio::test]
async fn gemma_saying_not_covered_is_stored_as_not_covered() {
    let Some((_g, pool)) = begin().await else {
        return;
    };
    let (user, run, _) = ready(&pool, &HERNIA).await;
    for said in [
        "Not covered in this evidence.",
        "Not covered in this evidence",
        "  NOT COVERED IN THIS EVIDENCE.\n",
    ] {
        let server = gemma_says(said, 1).await;
        let st = with_gemma(pool.clone(), &server.uri());
        let a = service::ask(&st, &user, run, ZEBRA_Q, ChatMode::Clinical)
            .await
            .unwrap();
        let v = finished(&st, &user, a).await;
        assert_eq!(v.status, "not_covered", "{said:?}");
        assert_eq!(v.reply, lines::NOT_COVERED);
    }
    // More than that sentence is an ordinary reply.
    let said = "Not covered in this evidence, but zebra was 5% [1].";
    let server = gemma_says(said, 1).await;
    let st = with_gemma(pool.clone(), &server.uri());
    let a = service::ask(&st, &user, run, ZEBRA_Q, ChatMode::Clinical)
        .await
        .unwrap();
    assert_eq!(finished(&st, &user, a).await.status, "done");
}

#[tokio::test]
async fn embed_failure_means_words_only() {
    let Some((_g, pool)) = begin().await else {
        return;
    };
    let (user, run, _) = ready(&pool, &HERNIA).await;
    let server = gemma_says(ZEBRA_REPLY, 1).await;
    Mock::given(method("POST"))
        .and(path("/api/embed"))
        .respond_with(ResponseTemplate::new(503))
        .expect(1)
        .mount(&server)
        .await;
    let mut st = with_gemma(pool.clone(), &server.uri());
    st.embedder = Some(Arc::new(Embedder::from_settings(&st.memory).unwrap()));
    let a = service::ask(&st, &user, run, ZEBRA_Q, ChatMode::Clinical)
        .await
        .unwrap();
    assert_eq!(finished(&st, &user, a).await.status, "done");
}

// ---------- history and follow-ups ----------

#[tokio::test]
async fn history_is_last_three_finished_turns_without_citations() {
    let Some((_g, pool)) = begin().await else {
        return;
    };
    let (user, run, _) = ready(&pool, &HERNIA).await;
    for (seq, status, reply) in [
        (1, "done", "oldest [1]"),
        (2, "done", "first kept [1]."),
        (3, "not_covered", "never sent"),
        (4, "partial", "second kept [2, 3] and more"),
        (5, "unavailable", ""),
        (6, "done", "third kept [1-2]"),
    ] {
        add_asked_turn(
            &pool,
            run,
            user.id,
            seq,
            &format!("q{seq}"),
            status,
            reply,
            0,
        )
        .await;
    }
    let server = gemma_says(ZEBRA_REPLY, 1).await;
    let st = with_gemma(pool.clone(), &server.uri());
    let a = service::ask(&st, &user, run, ZEBRA_Q, ChatMode::Clinical)
        .await
        .unwrap();
    assert_eq!(wait_turn(&pool, a.turn_id).await, "done");
    let m = sent_messages(&server, 0).await;
    let earlier: Vec<(String, String)> = m[1..m.len() - 1].to_vec();
    let want = [
        ("user", "q2"),
        ("assistant", "first kept."),
        ("user", "q4"),
        ("assistant", "second kept and more"),
        ("user", "q6"),
        ("assistant", "third kept"),
    ];
    assert_eq!(
        earlier,
        want.map(|(r, c)| (r.to_string(), c.to_string())).to_vec()
    );
}

#[tokio::test]
async fn short_follow_up_retrieves_and_calls_gemma() {
    let Some((_g, pool)) = begin().await else {
        return;
    };
    let (user, run, _) = ready(&pool, &HERNIA).await;
    let server = gemma_says(ZEBRA_REPLY, 2).await;
    let st = with_gemma(pool.clone(), &server.uri());
    // With nothing before it there are no words to find.
    let a = service::ask(&st, &user, run, "how sure is that?", ChatMode::Clinical)
        .await
        .unwrap();
    assert_eq!(wait_turn(&pool, a.turn_id).await, "not_covered");

    let a = service::ask(&st, &user, run, ZEBRA_Q, ChatMode::Clinical)
        .await
        .unwrap();
    assert_eq!(wait_turn(&pool, a.turn_id).await, "done");

    let a = service::ask(&st, &user, run, "how sure is that?", ChatMode::Clinical)
        .await
        .unwrap();
    let v = finished(&st, &user, a).await;
    assert_eq!(v.status, "done");
    assert!(!v.sources.is_empty());
    assert_eq!(server.received_requests().await.unwrap().len(), 2);
    let m = sent_messages(&server, 1).await;
    let last = &m.last().unwrap().1;
    assert!(last.contains(ZEBRA_TEXT));
    // Gemma is asked what was asked, not the joined search text.
    assert!(last.ends_with("Question: how sure is that?"));
}

#[tokio::test]
async fn follow_up_stays_on_the_asked_comparison() {
    let Some((_g, pool)) = begin().await else {
        return;
    };
    let (user, run, rec) = ready(&pool, &HERNIA).await;
    add_passage(&pool, rec, 9001, "Zebra stay open text", &["open|robotic"]).await;
    add_passage(
        &pool,
        rec,
        9002,
        "Zebra stay laparoscopic text",
        &["laparoscopic|robotic"],
    )
    .await;
    let server = gemma_says(ZEBRA_REPLY, 2).await;
    let st = with_gemma(pool.clone(), &server.uri());
    let a = service::ask(
        &st,
        &user,
        run,
        "Is robotic better than open for zebra stay?",
        ChatMode::Clinical,
    )
    .await
    .unwrap();
    assert_eq!(wait_turn(&pool, a.turn_id).await, "done");
    let a = service::ask(&st, &user, run, "how sure is that?", ChatMode::Clinical)
        .await
        .unwrap();
    assert_eq!(wait_turn(&pool, a.turn_id).await, "done");
    let last = sent_messages(&server, 1).await.pop().unwrap().1;
    assert!(last.contains("Zebra stay open text"), "{last}");
    assert!(!last.contains("Zebra stay laparoscopic text"), "{last}");
}

// ---------- state ----------

#[tokio::test]
async fn quick_run_and_run_without_opinion_not_offered() {
    let Some((_g, pool)) = begin().await else {
        return;
    };
    let server = gemma_says("never", 0).await;
    let st = with_gemma(pool.clone(), &server.uri());
    let user = seed_user_role(&pool, Role::Surgeon).await;

    let quick = seed_run(&pool, &user, "quick").await;
    let no_opinion = seed_run(&pool, &user, "focused").await;
    let failed_opinion = seed_run(&pool, &user, "focused").await;
    sqlx::query("INSERT INTO incisive.run_temporal (run_id, status) VALUES ($1, 'failed')")
        .bind(failed_opinion)
        .execute(&pool)
        .await
        .unwrap();
    // The opinion finished but the memory job gave up: no record is coming.
    let no_record = seed_run(&pool, &user, "focused").await;
    sqlx::query("INSERT INTO incisive.run_temporal (run_id, status) VALUES ($1, 'done')")
        .bind(no_record)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO incisive.memory_jobs (run_id, status) VALUES ($1, 'skipped')")
        .bind(no_record)
        .execute(&pool)
        .await
        .unwrap();

    for run in [quick, no_opinion, failed_opinion, no_record] {
        let v = service::state_for(&st, &user, run).await.unwrap();
        assert_eq!(v.state, ChatState::NotOffered);
        assert!(v.turns.is_empty());
        assert_eq!(
            service::ask(&st, &user, run, ZEBRA_Q, ChatMode::Clinical)
                .await
                .unwrap_err(),
            ChatError::NotOffered { safety_line: false }
        );
    }
    // Not offered wins over Gemma being absent: the option is simply not there.
    let v = service::state_for(&no_gemma(pool.clone()), &user, quick)
        .await
        .unwrap();
    assert_eq!(v.state, ChatState::NotOffered);
}

#[tokio::test]
async fn getting_ready_until_prep_ok() {
    let Some((_g, pool)) = begin().await else {
        return;
    };
    let server = gemma_says("never", 0).await;
    let st = with_gemma(pool.clone(), &server.uri());

    // The opinion is finished and its record is still on the way.
    let user = seed_user_role(&pool, Role::Surgeon).await;
    let waiting = seed_run(&pool, &user, "focused").await;
    sqlx::query("INSERT INTO incisive.run_temporal (run_id, status) VALUES ($1, 'done')")
        .bind(waiting)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO incisive.memory_jobs (run_id) VALUES ($1)")
        .bind(waiting)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        service::state_for(&st, &user, waiting).await.unwrap().state,
        ChatState::GettingReady
    );

    // A record, no prep yet.
    let (user, run, _) = seed_record(&pool, &HERNIA).await;
    let v = service::state_for(&st, &user, run).await.unwrap();
    assert_eq!(v.state, ChatState::GettingReady);
    assert!(v.starters.is_empty());
    assert_eq!(
        service::ask(&st, &user, run, "chest pain?", ChatMode::Clinical)
            .await
            .unwrap_err(),
        ChatError::Unavailable { safety_line: true }
    );
    job::tick(&st).await;
    assert_eq!(
        service::state_for(&st, &user, run).await.unwrap().state,
        ChatState::Ready
    );
}

#[tokio::test]
async fn failed_prep_is_unavailable() {
    let Some((_g, pool)) = begin().await else {
        return;
    };
    let server = gemma_says("never", 0).await;
    let st = with_gemma(pool.clone(), &server.uri());
    let (user, run, rec) = seed_record(&pool, &HERNIA).await;
    sqlx::query(
        "INSERT INTO incisive.chat_prep (record_id, status, passages, error_kind) \
         VALUES ($1, 'failed', 0, 'bad')",
    )
    .bind(rec)
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(
        service::state_for(&st, &user, run).await.unwrap().state,
        ChatState::Unavailable
    );
    assert_eq!(
        service::ask(&st, &user, run, ZEBRA_Q, ChatMode::Clinical)
            .await
            .unwrap_err(),
        ChatError::Unavailable { safety_line: false }
    );
    // No client at all, on a fully prepared run.
    let (user, run, _) = ready(&pool, &HERNIA).await;
    assert_eq!(
        service::state_for(&no_gemma(pool.clone()), &user, run)
            .await
            .unwrap()
            .state,
        ChatState::Unavailable
    );
}

#[tokio::test]
async fn someone_elses_run_is_not_found() {
    let Some((_g, pool)) = begin().await else {
        return;
    };
    let (owner_of_run, run, _) = ready(&pool, &HERNIA).await;
    let server = gemma_says(ZEBRA_REPLY, 1).await;
    let st = with_gemma(pool.clone(), &server.uri());
    let a = service::ask(&st, &owner_of_run, run, ZEBRA_Q, ChatMode::Clinical)
        .await
        .unwrap();
    wait_turn(&pool, a.turn_id).await;

    let stranger = seed_user_role(&pool, Role::Surgeon).await;
    let site_owner = seed_user_role(&pool, Role::Owner).await;
    for u in [&stranger, &site_owner] {
        assert_eq!(
            service::state_for(&st, u, run).await.unwrap_err(),
            ChatError::NotFound
        );
        assert_eq!(
            service::ask(&st, u, run, ZEBRA_Q, ChatMode::Clinical)
                .await
                .unwrap_err(),
            ChatError::NotFound
        );
        assert_eq!(
            service::turn_view(&st, u, a.turn_id).await.unwrap_err(),
            ChatError::NotFound
        );
    }
    assert_eq!(
        service::state_for(&st, &owner_of_run, Uuid::new_v4())
            .await
            .unwrap_err(),
        ChatError::NotFound
    );
    assert_eq!(
        service::turn_view(&st, &owner_of_run, i64::MAX)
            .await
            .unwrap_err(),
        ChatError::NotFound
    );
}
