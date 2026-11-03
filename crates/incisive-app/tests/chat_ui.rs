//! The chat window on the run page, rendered by the real router (SSR), with
//! the database (`TEST_DATABASE_URL`) and a fake Gemma.
//!
//! With `CHAT=off` the run page with an expert opinion row is compared with
//! the page captured on main aec9928 before this window existed
//! (`tests/fixtures/chat/ui/before_chat.json`), the same way
//! `tests/temporal_ui.rs` compares the page with no row: markup length and
//! fingerprint with Leptos's hydration markers, the marker count and each
//! script block's kind (with the hydration payload's shape).

use std::sync::Arc;

use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode},
};
use http_body_util::BodyExt;
use incisive_app::db::{lazy_pool, migrate};
use incisive_app::gate::OfflineClassifier;
use incisive_app::identity::{IapVerifier, JwkSource};
use incisive_app::runs::store::{create_run, save_answer, set_status};
use incisive_app::runs::worker::WorkerHandle;
use incisive_app::runs::{RunMode, RunStatus, RunsConfig};
use incisive_app::server::AppState;
use incisive_app::spend::{CapsConfig, Prices};
use leptos::config::LeptosOptions;
use secrecy::SecretString;
use serde_json::Value;
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

const MODEL: &str = "m-test";

async fn connect() -> Option<PgPool> {
    let Ok(url) = std::env::var("TEST_DATABASE_URL") else {
        eprintln!("SKIP: TEST_DATABASE_URL not set");
        return None;
    };
    let pool = lazy_pool(&SecretString::from(url)).expect("lazy pool");
    migrate(&pool).await.expect("migrate");
    Some(pool)
}

fn state(pool: PgPool) -> AppState {
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
        gate_model: MODEL.into(),
        dev_mode: true,
        runs: RunsConfig::default(),
        quick_engine: incisive_app::runs::QuickEngine::Classic,
        full_engine: incisive_app::runs::Engine::Classic,
        completeness: false,
        temporal: Default::default(),
        temporal_client: None,
        memory: incisive_app::memory::settings::MemorySettings::default(),
        chat: incisive_app::chat::settings::ChatSettings::default(),
        embedder: None,
        gemma: None,
        worker: WorkerHandle::with_pipeline(incisive_app::agent::testing::placeholder_pipeline),
        llm: Some(incisive_app::agent::testing::idle_llm(MODEL)),
        lit: incisive_app::agent::testing::offline_literature(),
    }
}

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

async fn send(app: Router, path: &str, email: &str) -> (StatusCode, String) {
    let req = Request::builder()
        .method("GET")
        .uri(path)
        .header("x-dev-user-email", email)
        .body(Body::empty())
        .unwrap();
    let res = app.oneshot(req).await.unwrap();
    let status = res.status();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

async fn new_user(pool: &PgPool) -> (i64, String) {
    let email = format!("t4-{}@example.com", Uuid::new_v4());
    let uid = sqlx::query_scalar(
        "INSERT INTO incisive.users (email, display_name, role, daily_cap_paise) \
         VALUES ($1, 'Manjunath Haridas', 'surgeon', 100000) RETURNING id",
    )
    .bind(&email)
    .fetch_one(pool)
    .await
    .unwrap();
    (uid, email)
}

fn fixture(name: &str) -> Value {
    let p = format!(
        "{}/tests/fixtures/temporal/full/{name}.json",
        env!("CARGO_MANIFEST_DIR")
    );
    serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap()
}

/// A finished Full run of `uid` with the stored fixture answer `name`.
async fn full_run(pool: &PgPool, uid: i64, name: &str) -> Uuid {
    let body = fixture(name);
    let q = body["question"].as_str().unwrap().to_string();
    let run = create_run(pool, uid, &q, RunMode::Focused, 4000)
        .await
        .unwrap()
        .unwrap();
    assert!(
        set_status(pool, run, RunStatus::Running, None)
            .await
            .unwrap()
    );
    assert!(save_answer(pool, run, &body).await.unwrap());
    assert!(set_status(pool, run, RunStatus::Done, None).await.unwrap());
    run
}

/// The page's markup for the content checks: scripts out, Leptos's `<!>`
/// markers out, the run id and today's header date replaced by fixed
/// tokens, nonces blanked and each streamed chunk back in its slot.
fn markup(html: &str, run: Uuid) -> String {
    let mut m = String::new();
    let mut rest = html;
    while let Some(i) = rest.find("<script") {
        m.push_str(&rest[..i]);
        rest = rest[i..]
            .find("</script>")
            .map_or("", |j| &rest[i + j + "</script>".len()..]);
    }
    m.push_str(rest);
    // Out-of-order streaming: a Suspense that resolves after the shell is
    // flushed sends its content as `<template id="N-f">` after `</html>`,
    // and its slot (`<!--s-N-o-->` … `<!--s-N-c-->`) holds the fallback;
    // one that resolves in time is drawn in its slot. Each template is put
    // back into its slot and the slot markers dropped (an inline draw has
    // none), so both give the same markup.
    tokens(&m, run).replace("<!>", "")
}

/// The no-row comparison's view of a page.
struct PageShape {
    /// The markup outside the scripts, `<!>` markers kept (tokens as in
    /// [`markup`]).
    markup: String,
    /// The number of `<!>` markers.
    markers: usize,
    /// Each `<script>` block's kind, in page order, except the scripts that
    /// move a streamed chunk into its slot: how many Suspense chunks miss
    /// the shell's flush depends on timing (the chunks' content is in
    /// `markup`, put back in its slot).
    scripts: Vec<String>,
}

/// A page's markup outside its scripts with the hydration markers kept, the
/// marker count and the scripts' kinds ([`script_kind`]).
fn page_shape(html: &str, run: Uuid) -> PageShape {
    let mut outside = String::new();
    let mut scripts = Vec::new();
    let mut rest = html;
    while let Some(i) = rest.find("<script") {
        outside.push_str(&rest[..i]);
        let end = rest[i..].find("</script>").expect("script end") + i + "</script>".len();
        let kind = script_kind(&rest[i..end], run);
        if !kind.ends_with(SWAP) {
            scripts.push(kind);
        }
        rest = &rest[end..];
    }
    outside.push_str(rest);
    let markup = tokens(&outside, run);
    PageShape {
        markers: markup.matches("<!>").count(),
        markup,
        scripts,
    }
}

/// The kind of the scripts that move a streamed chunk into its slot.
const SWAP: &str = "swap a streamed chunk";

/// A `<script>` block's kind: its opening tag (nonce blanked) and what it
/// does; the resolved-resources script lists each resource's id and the
/// FNV-1a of its payload's JSON shape ([`json_shape`]), so a resource whose
/// type changes (a tuple, an extra field) shows.
fn script_kind(s: &str, run: Uuid) -> String {
    let nonce = regex::Regex::new(r#"nonce="[^"]*""#).unwrap();
    let open_end = s.find('>').unwrap() + 1;
    let tag = nonce.replace_all(&s[..open_end], r#"nonce="NONCE""#);
    let body = &s[open_end..s.len() - "</script>".len()];
    let what = if body.contains("mod.hydrate()") {
        "hydrate".to_string()
    } else if body.starts_with("(function() { let id = ") {
        SWAP.to_string()
    } else if body.contains("__RESOLVED_RESOURCES[") {
        let mut parts = Vec::new();
        let mut rest = body;
        while let Some(i) = rest.find("__RESOLVED_RESOURCES[") {
            rest = &rest[i + "__RESOLVED_RESOURCES[".len()..];
            let close = rest.find(']').unwrap();
            let id = rest[..close].to_string();
            rest = rest[close..]
                .trim_start_matches(']')
                .trim_start_matches(" = ");
            let mut it = serde_json::Deserializer::from_str(rest).into_iter::<String>();
            let payload = it.next().expect("a payload").expect("a JSON string");
            rest = &rest[it.byte_offset()..];
            let v: Value = serde_json::from_str(&payload).expect("payload JSON");
            parts.push(format!("{id}:{}", fnv1a(&json_shape(&v))));
        }
        format!("resolved {}", parts.join(" "))
    } else {
        tokens(body, run)
    };
    format!("{tag} {what}")
}

/// A JSON value's shape: object keys (sorted) and value kinds, recursively;
/// array elements in order.
fn json_shape(v: &Value) -> String {
    match v {
        Value::Null => "z".into(),
        Value::Bool(_) => "b".into(),
        Value::Number(_) => "n".into(),
        Value::String(_) => "s".into(),
        Value::Array(a) => format!(
            "[{}]",
            a.iter().map(json_shape).collect::<Vec<_>>().join(",")
        ),
        Value::Object(o) => {
            let mut k: Vec<_> = o.iter().collect();
            k.sort_by(|a, b| a.0.cmp(b.0));
            let f: Vec<String> = k
                .iter()
                .map(|(k, v)| format!("{k}:{}", json_shape(v)))
                .collect();
            format!("{{{}}}", f.join(","))
        }
    }
}

/// The run id, today's header date and the nonces replaced by fixed tokens;
/// each streamed chunk put back in its slot (see [`markup`]).
fn tokens(m: &str, run: Uuid) -> String {
    let date = incisive_app::library::header_date(incisive_app::library::now_utc());
    let nonce = regex::Regex::new(r#"(crossorigin|nonce)="[^"]*""#).unwrap();
    let m = nonce.replace_all(m, "$1=\"NONCE\"").into_owned();
    let mut m = m
        .replace(&run.to_string(), "RUN-ID")
        .replace(&date, "HEADER-DATE");
    let tpl = regex::Regex::new(r#"<template id="(\d+)-f">"#).unwrap();
    while let Some(c) = tpl.captures(&m) {
        let n = c[1].to_string();
        let at = c.get(0).unwrap();
        let close = at.end() + m[at.end()..].find("</template>").expect("template end");
        let content = m[at.end()..close].to_string();
        m.replace_range(at.start()..close + "</template>".len(), "");
        let (o, cl) = (format!("<!--s-{n}-o-->"), format!("<!--s-{n}-c-->"));
        let (Some(a), Some(b)) = (m.find(&o), m.find(&cl)) else {
            panic!("no slot for template {n}");
        };
        m.replace_range(a..b + cl.len(), &content);
    }
    let slot = regex::Regex::new(r"<!--s-\d+-[oc]-->").unwrap();
    slot.replace_all(&m, "").into_owned()
}

/// FNV-1a (64-bit) of `s`, as hex.
fn fnv1a(s: &str) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{h:016x}")
}

use incisive_app::runs::temporal::{Finished, JobStatus, finish, insert_pending};

fn board_fixture(name: &str) -> Value {
    let p = format!(
        "{}/tests/fixtures/temporal/boards/{name}-board.json",
        env!("CARGO_MANIFEST_DIR")
    );
    serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap()
}

fn table_of(name: &str) -> Value {
    let t = incisive_app::temporal::build(
        &incisive_app::temporal::TemporalInput::from_json(&fixture(name).to_string()).unwrap(),
    );
    serde_json::to_value(&t).unwrap()
}

/// Give `run` a temporal row ending `status` (`None`: left pending), with
/// `board` (the stored smoke board unless given) for a done row.
async fn temporal_row(
    pool: &PgPool,
    run: Uuid,
    name: &str,
    status: Option<JobStatus>,
    board: Option<Value>,
) {
    assert!(insert_pending(pool, run, Uuid::new_v4()).await.unwrap());
    let Some(status) = status else { return };
    let f = if status == JobStatus::Done {
        let b = board_fixture(name);
        Finished {
            topic: Some("T".into()),
            evidence_year: Some(2026),
            table: Some(table_of(name)),
            board: Some(board.unwrap_or_else(|| b["board"].clone())),
            flags: Some(b["flags"].clone()),
            ..Finished::default()
        }
    } else {
        Finished {
            reason: Some("x"),
            ..Finished::default()
        }
    };
    assert!(finish(pool, run, status, &f).await.unwrap());
}

const BEFORE: &str = "tests/fixtures/chat/ui/before_chat.json";

/// The run page with `CHAT=off` for each expert opinion status: its
/// [`page_shape`]. With `CHAT_UI_CAPTURE=1` it is written to [`BEFORE`]
/// instead (done on a clean checkout of main aec9928).
async fn chat_off_pages(pool: &PgPool) -> serde_json::Map<String, Value> {
    let st = state(pool.clone());
    assert!(!st.chat.on);
    let (uid, email) = new_user(pool).await;
    let mut out = serde_json::Map::new();
    let cases = [
        ("done", Some(JobStatus::Done)),
        ("pending", None),
        ("failed", Some(JobStatus::Failed)),
    ];
    for (label, status) in cases {
        let run = full_run(pool, uid, "hernia").await;
        temporal_row(pool, run, "hernia", status, None).await;
        let (s, html) = send(router(st.clone()), &format!("/runs/{run}"), &email).await;
        assert_eq!(s, StatusCode::OK, "{label}");
        let shape = page_shape(&html, run);
        out.insert(
            label.to_string(),
            serde_json::json!({
                "len": shape.markup.len(),
                "fnv1a": fnv1a(&shape.markup),
                "markers": shape.markers,
                "scripts": shape.scripts,
            }),
        );
    }
    out
}

#[tokio::test]
async fn answer_and_expert_views_unchanged() {
    let Some(pool) = connect().await else { return };
    let now = chat_off_pages(&pool).await;
    let path = format!("{}/{BEFORE}", env!("CARGO_MANIFEST_DIR"));
    if std::env::var("CHAT_UI_CAPTURE").is_ok_and(|v| v == "1") {
        std::fs::create_dir_all(std::path::Path::new(&path).parent().unwrap()).unwrap();
        std::fs::write(&path, serde_json::to_string_pretty(&now).unwrap()).unwrap();
        eprintln!("captured {path}");
        return;
    }
    let before: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(before.as_object().unwrap().len(), 3);
    for (name, b) in before.as_object().unwrap() {
        assert_eq!(&now[name], b, "{name}: the run page changed with CHAT=off");
    }
}

// ---------- the chat on ----------

use std::time::Duration;

use incisive_app::chat::gemma::GemmaChat;
use incisive_app::chat::job as chat_job;
use incisive_app::chat::lines;
use incisive_app::chat::settings::ChatSettings;
use incisive_app::memory::job as memory_job;
use incisive_app::memory::settings::MemorySettings;
use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// One chat test at a time, on empty chat tables: the queue limit counts
/// pending turns across users.
async fn begin() -> Option<(tokio::sync::MutexGuard<'static, ()>, PgPool)> {
    let guard = SERIAL.lock().await;
    let pool = connect().await?;
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

/// `CHAT=on` with no Gemma client.
fn chat_on(pool: PgPool) -> AppState {
    let mut st = state(pool);
    st.memory = MemorySettings {
        on: true,
        ..MemorySettings::default()
    };
    st.chat = ChatSettings {
        on: true,
        ..ChatSettings::default()
    };
    st
}

/// `CHAT=on` with a Gemma at `uri`.
fn chat_on_gemma(pool: PgPool, uri: &str) -> AppState {
    let mut st = chat_on(pool);
    st.memory.gemma_url = Some(uri.to_string());
    st.memory.gemma_auth = Some(SecretString::from("incisive:pw"));
    st.gemma = Some(Arc::new(
        GemmaChat::from_settings(&st.memory, &st.chat).expect("client"),
    ));
    st
}

fn ndjson(reply: &str) -> ResponseTemplate {
    let line = json!({"message": {"role": "assistant", "content": reply}, "done": false});
    ResponseTemplate::new(200).set_body_raw(
        format!("{line}\n{{\"done\":true,\"done_reason\":\"stop\"}}\n"),
        "application/x-ndjson",
    )
}

async fn gemma_says(reply: &str, delay: Duration) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/chat"))
        .respond_with(ndjson(reply).set_delay(delay))
        .mount(&server)
        .await;
    server
}

/// A finished Full run with its record; prepared unless `prep` is false.
async fn record_run(pool: &PgPool, uid: i64, prep: bool) -> (Uuid, i64) {
    let run = full_run(pool, uid, "hernia").await;
    temporal_row(pool, run, "hernia", Some(JobStatus::Done), None).await;
    let st = chat_on(pool.clone());
    let memory_job::JobOutcome::Done { record_id, .. } = memory_job::process(&st, run).await else {
        panic!("memory job did not record the run");
    };
    if prep {
        chat_job::tick(&st).await;
    }
    (run, record_id)
}

async fn add_passage(pool: &PgPool, rec: i64, seq: i32, text: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO incisive.chat_passages (record_id, kind, seq, ref_ns, comparisons, text) \
         VALUES ($1, 'bottom_line', $2, '{}', '{}', $3) RETURNING id",
    )
    .bind(rec)
    .bind(seq)
    .bind(text)
    .fetch_one(pool)
    .await
    .unwrap()
}

/// What a test stores as a turn; everything else takes a plain default.
#[derive(Default)]
struct T {
    question: &'static str,
    reply: &'static str,
    status: &'static str,
    ids: Vec<i64>,
    checks: Option<Value>,
    safety: bool,
    personal: bool,
    suggested: bool,
    feedback: Option<i16>,
}

async fn add_turn(pool: &PgPool, run: Uuid, uid: i64, seq: i32, t: T) -> i64 {
    let existing: Option<i64> = sqlx::query_scalar(
        "SELECT id FROM incisive.chat_conversations WHERE run_id = $1 AND user_id = $2",
    )
    .bind(run)
    .bind(uid)
    .fetch_optional(pool)
    .await
    .unwrap();
    let conv: i64 = match existing {
        Some(c) => c,
        None => sqlx::query_scalar(
            "INSERT INTO incisive.chat_conversations (run_id, user_id, mode) \
             VALUES ($1, $2, 'clinical') RETURNING id",
        )
        .bind(run)
        .bind(uid)
        .fetch_one(pool)
        .await
        .unwrap(),
    };
    sqlx::query_scalar(
        "INSERT INTO incisive.chat_turns \
         (conversation_id, user_id, seq, question, reply, mode, status, passage_ids, checks, \
          safety_line, personal, suggested, feedback) \
         VALUES ($1, $2, $3, $4, $5, 'clinical', $6, $7, $8, $9, $10, $11, $12) RETURNING id",
    )
    .bind(conv)
    .bind(uid)
    .bind(seq)
    .bind(t.question)
    .bind(t.reply)
    .bind(t.status)
    .bind(&t.ids)
    .bind(t.checks)
    .bind(t.safety)
    .bind(t.personal)
    .bind(t.suggested)
    .bind(t.feedback)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn page(st: &AppState, run: Uuid, email: &str) -> String {
    let (s, html) = send(router(st.clone()), &format!("/runs/{run}"), email).await;
    assert_eq!(s, StatusCode::OK);
    markup(&html, run).replace("<!---->", "")
}

/// The markup with the style blocks out (they name the classes).
fn no_style(m: &str) -> String {
    regex::Regex::new(r"(?s)<style>.*?</style>")
        .unwrap()
        .replace_all(m, "")
        .into_owned()
}

/// The chat view's markup (from its section to the answer wrapper).
fn chat_section(m: &str) -> &str {
    let i = m.find(r#"id="chat""#).expect("the chat section");
    let j = i + m[i..].find("rx-answer").expect("the answer wrapper");
    &m[i..j]
}

/// A server function called as the page's own code does (no `text/html`).
async fn call_fn(st: &AppState, name: &str, body: &str, email: &str) -> (StatusCode, Value) {
    let req = Request::post(format!("/api/fn/{name}"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("accept", "application/json")
        .header("x-dev-user-email", email)
        .body(Body::from(body.to_string()))
        .unwrap();
    let res = router(st.clone()).oneshot(req).await.unwrap();
    let status = res.status();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

/// A plain form post from a page that has not hydrated.
async fn form_post(st: &AppState, name: &str, body: &str, email: &str) -> (StatusCode, String) {
    let req = Request::post(format!("/api/fn/{name}"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("accept", "text/html,application/xhtml+xml")
        .header("referer", "http://localhost/runs/x")
        .header("x-dev-user-email", email)
        .body(Body::from(body.to_string()))
        .unwrap();
    let res = router(st.clone()).oneshot(req).await.unwrap();
    let status = res.status();
    let loc = res
        .headers()
        .get("location")
        .map(|v| v.to_str().unwrap().to_string())
        .unwrap_or_default();
    (status, loc)
}

fn urlenc(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}

async fn count_turns(pool: &PgPool, uid: i64) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM incisive.chat_turns WHERE user_id = $1")
        .bind(uid)
        .fetch_one(pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn option_absent_on_quick_and_no_opinion() {
    let Some((_g, pool)) = begin().await else {
        return;
    };
    let st = chat_on(pool.clone());
    let (uid, email) = new_user(&pool).await;

    // A Full run with no expert opinion row.
    let no_row = full_run(&pool, uid, "hernia").await;
    // Opinion rows that never became an opinion, or are still being written.
    let failed = full_run(&pool, uid, "hernia").await;
    temporal_row(&pool, failed, "hernia", Some(JobStatus::Failed), None).await;
    let writing = full_run(&pool, uid, "hernia").await;
    temporal_row(&pool, writing, "hernia", None, None).await;
    // A Quick run, even with a row.
    let quick = create_run(&pool, uid, "Quick?", RunMode::Quick, 4000)
        .await
        .unwrap()
        .unwrap();
    assert!(
        set_status(&pool, quick, RunStatus::Running, None)
            .await
            .unwrap()
    );
    assert!(
        set_status(&pool, quick, RunStatus::Done, None)
            .await
            .unwrap()
    );

    for run in [no_row, failed, quick] {
        let m = page(&st, run, &email).await;
        assert!(!m.contains(r##"href="#chat""##), "a Chat tab on {run}");
        assert!(!m.contains("Chat \u{b7}"), "a Chat state on {run}");
        assert!(!m.contains("rx-chat"), "chat markup on {run}");
    }
    // Still being written: no option yet, and an empty view ready for it.
    let m = no_style(&page(&st, writing, &email).await);
    assert!(!m.contains(r##"href="#chat""##));
    assert!(!m.contains("Chat \u{b7}"));
    assert!(!m.contains("rx-composer") && !m.contains("rx-seg3"));
}

#[tokio::test]
async fn option_states_match_the_board() {
    let Some((_g, pool)) = begin().await else {
        return;
    };
    let (uid, email) = new_user(&pool).await;
    let server = gemma_says("x", Duration::ZERO).await;
    let on = chat_on_gemma(pool.clone(), &server.uri());

    // Ready: a link to the view, the answer still shown first.
    let (ready, _) = record_run(&pool, uid, true).await;
    let m = page(&on, ready, &email).await;
    assert!(m.contains(r##"href="#chat""##), "ready tab");
    assert!(m.contains(">Chat</a>"), "ready label");
    assert!(!m.contains("Chat \u{b7}"), "ready has no state");
    assert!(
        m.contains(r#"<div class="ap">"#),
        "the answer is still drawn"
    );
    assert!(
        !m[m.find(r#"<div class="ap">"#).unwrap()..].contains("rx-chat"),
        "chat outside the answer content"
    );

    // The record is there but passages are not built yet.
    let (preparing, _) = record_run(&pool, uid, false).await;
    let m = no_style(&page(&on, preparing, &email).await);
    assert!(m.contains("Chat \u{b7} Getting ready"), "getting ready");
    assert!(!m.contains(r##"href="#chat""##));

    // The opinion is done and its record is still to come.
    let coming = full_run(&pool, uid, "hernia").await;
    temporal_row(&pool, coming, "hernia", Some(JobStatus::Done), None).await;
    sqlx::query("INSERT INTO incisive.memory_jobs (run_id) VALUES ($1)")
        .bind(coming)
        .execute(&pool)
        .await
        .unwrap();
    let m = page(&on, coming, &email).await;
    assert!(m.contains("Chat \u{b7} Getting ready"), "record to come");

    // No Gemma client: unavailable, and the answer is untouched.
    let down = chat_on(pool.clone());
    let m = no_style(&page(&down, ready, &email).await);
    assert!(m.contains("Chat \u{b7} Unavailable"), "unavailable");
    assert!(
        m.contains(
            "Chat is unavailable right now. The answer and Expert Opinion are not affected."
        ),
        "unavailable note"
    );
    assert!(!m.contains(r##"href="#chat""##));
    assert!(m.contains(r#"<div class="ap">"#));
}

#[tokio::test]
async fn empty_chat_shows_the_frozen_wording() {
    let Some((_g, pool)) = begin().await else {
        return;
    };
    let (uid, email) = new_user(&pool).await;
    let server = gemma_says("x", Duration::ZERO).await;
    let st = chat_on_gemma(pool.clone(), &server.uri());
    let (run, _) = record_run(&pool, uid, true).await;
    let m = page(&st, run, &email).await;
    let c = chat_section(&m);
    assert!(c.contains(
        "Ask anything about this run. Replies use only its answer, Expert Opinion and papers."
    ));
    assert!(
        c.contains("What does the evidence show about"),
        "starters: {c}"
    );
    assert!(c.contains(r#"action="/api/fn/chat_ask""#), "the form posts");
    assert!(c.contains(r#"method="post""#));
    assert!(c.contains(r#"maxlength="1000""#));
    assert!(c.contains(r#"placeholder="Ask about this evidence""#));
    assert!(c.contains(">Clinical<") && c.contains(">Plain language<"));
    assert!(c.contains(r#"aria-label="Send""#));
    assert!(c.contains(lines::FOOTER));
    // The run is carried by the form, not guessed.
    assert!(c.contains(r#"name="run_id""#) && c.contains("RUN-ID"));
}

#[tokio::test]
async fn citations_render_only_for_given_passages() {
    let Some((_g, pool)) = begin().await else {
        return;
    };
    let (uid, email) = new_user(&pool).await;
    let server = gemma_says("x", Duration::ZERO).await;
    let st = chat_on_gemma(pool.clone(), &server.uri());
    let (run, rec) = record_run(&pool, uid, true).await;
    let a = add_passage(&pool, rec, 9001, "Passage one.").await;
    let b = add_passage(&pool, rec, 9002, "Passage two.").await;
    add_turn(
        &pool,
        run,
        uid,
        1,
        T {
            question: "How long?",
            reply: "Shorter stay [1], fewer leaks [2], and more [9].",
            status: "done",
            ids: vec![a, b],
            checks: Some(json!({"unknown_citations": [9]})),
            ..T::default()
        },
    )
    .await;
    let m = page(&st, run, &email).await;
    let c = chat_section(&m);
    assert!(c.contains(r#"aria-label="Open source 1""#), "{c}");
    assert!(c.contains(r#"aria-label="Open source 2""#));
    assert!(!c.contains(r#"aria-label="Open source 9""#), "no source 9");
    assert!(c.contains("[9]"), "the text is kept");
    assert!(!c.contains("[1]"), "a given citation is a button");
    assert!(
        c.contains("A citation doesn&#x27;t match any source.")
            || c.contains("A citation doesn't match any source.")
    );
}

#[tokio::test]
async fn flag_labels_are_plain_words() {
    let Some((_g, pool)) = begin().await else {
        return;
    };
    let (uid, email) = new_user(&pool).await;
    let server = gemma_says("x", Duration::ZERO).await;
    let st = chat_on_gemma(pool.clone(), &server.uri());
    let (run, _) = record_run(&pool, uid, true).await;
    add_turn(
        &pool,
        run,
        uid,
        1,
        T {
            question: "Q",
            reply: "About 3 days.",
            status: "done",
            checks: Some(json!({
                "not_traced": ["3 days"],
                "unknown_citations": [4],
                "outside_references": ["Smith 2020"],
                "comparison_mismatch": ["open vs lap"],
                "you_should": true,
                "uncited": true,
                "retracted_source": true,
            })),
            ..T::default()
        },
    )
    .await;
    let m = page(&st, run, &email).await;
    let c = chat_section(&m)
        .replace("&quot;", "\"")
        .replace("&#x27;", "'");
    for line in [
        "Check this reply",
        "\"3 days\" is not in the sources cited.",
        "A citation doesn't match any source.",
        "It names a reference that isn't from this evidence.",
        "It mentions a different comparison from the one asked.",
        "It says \"you should\".",
        "It gives no source.",
        "It relies on a retracted paper.",
        "About 3 days.",
    ] {
        assert!(c.contains(line), "missing {line:?}: {c}");
    }
    for raw in [
        "not_traced",
        "unknown_citations",
        "outside_references",
        "comparison_mismatch",
        "you_should",
        "uncited",
        "retracted_source",
    ] {
        assert!(!c.contains(raw), "raw code {raw}");
    }
}

#[tokio::test]
async fn unchecked_and_busy_are_never_clean_or_flags() {
    let Some((_g, pool)) = begin().await else {
        return;
    };
    let (uid, email) = new_user(&pool).await;
    let server = gemma_says("x", Duration::ZERO).await;
    let st = chat_on_gemma(pool.clone(), &server.uri());

    // Swept: the flag is stored.
    let (run, _) = record_run(&pool, uid, true).await;
    add_turn(
        &pool,
        run,
        uid,
        1,
        T {
            question: "Q",
            reply: "So far",
            status: "partial",
            checks: Some(json!({"unchecked": true})),
            ..T::default()
        },
    )
    .await;
    let m = page(&st, run, &email).await;
    let c = chat_section(&m);
    assert!(c.contains("Check this reply") && c.contains("This reply could not be checked."));
    assert!(c.contains("This reply was cut short."));
    assert!(
        !c.contains("Suggest for library"),
        "partial cannot be suggested"
    );

    // No checks stored on a finished reply: unchecked, not clean.
    let (run, _) = record_run(&pool, uid, true).await;
    add_turn(
        &pool,
        run,
        uid,
        1,
        T {
            question: "Q",
            reply: "Text",
            status: "done",
            ..T::default()
        },
    )
    .await;
    let m = page(&st, run, &email).await;
    assert!(chat_section(&m).contains("This reply could not be checked."));

    // Busy: the busy line, never the marker.
    let (run, _) = record_run(&pool, uid, true).await;
    add_turn(
        &pool,
        run,
        uid,
        1,
        T {
            question: "Q",
            status: "unavailable",
            checks: Some(json!({"gemma_busy": true})),
            ..T::default()
        },
    )
    .await;
    let m = page(&st, run, &email).await;
    let c = chat_section(&m);
    assert!(c.contains(lines::BUSY));
    assert!(!c.contains("gemma_busy") && !c.contains("Check this reply"));
    assert!(!c.contains("could not be checked"));
}

#[tokio::test]
async fn statuses_show_their_lines_and_actions() {
    let Some((_g, pool)) = begin().await else {
        return;
    };
    let (uid, email) = new_user(&pool).await;
    let server = gemma_says("x", Duration::ZERO).await;
    let st = chat_on_gemma(pool.clone(), &server.uri());

    // Clean and done: thumbs and Suggest; the fixed lines in their own style.
    let (run, _) = record_run(&pool, uid, true).await;
    add_turn(
        &pool,
        run,
        uid,
        1,
        T {
            question: "Will I go home sooner?",
            reply: "Probably.",
            status: "done",
            checks: Some(json!({})),
            personal: true,
            safety: true,
            feedback: Some(1),
            ..T::default()
        },
    )
    .await;
    let m = page(&st, run, &email).await;
    let c = chat_section(&m);
    assert!(!c.contains("Check this reply"), "clean: {c}");
    assert!(c.contains(r#"aria-label="Helpful""#));
    assert!(c.contains(r#"aria-label="Not helpful""#));
    assert!(c.contains("Suggest for library"));
    assert!(c.contains(lines::SEE_YOUR_SURGEON) && c.contains(lines::WARNING_LINE));
    assert!(c.find(lines::SEE_YOUR_SURGEON) < c.find(lines::WARNING_LINE));
    assert!(c.contains("rx-line-surgeon") && c.contains("rx-line-safety"));
    assert!(c.contains(r#"aria-pressed="true""#), "the stored thumb");
    assert!(
        !c.contains("What does the evidence show about"),
        "no starters"
    );

    // Already suggested.
    let (run, _) = record_run(&pool, uid, true).await;
    add_turn(
        &pool,
        run,
        uid,
        1,
        T {
            question: "Q",
            reply: "R",
            status: "done",
            checks: Some(json!({})),
            suggested: true,
            ..T::default()
        },
    )
    .await;
    let m = page(&st, run, &email).await;
    assert!(chat_section(&m).contains("Suggested for library"));

    // Not covered: the fixed line, thumbs as in the mock, no Suggest, no
    // flag for a reply nothing was checked on.
    let (run, _) = record_run(&pool, uid, true).await;
    add_turn(
        &pool,
        run,
        uid,
        1,
        T {
            question: "Is it better for an obese patient?",
            status: "not_covered",
            ..T::default()
        },
    )
    .await;
    let m = page(&st, run, &email).await;
    let c = chat_section(&m);
    assert!(c.contains(lines::NOT_COVERED));
    assert!(!c.contains("Suggest"), "{c}");
    assert!(!c.contains("Check this reply"));
    assert!(c.contains(r#"aria-label="Helpful""#));

    // A pending turn with text: unchecked wording, no flags or actions; the
    // safety line is already there.
    let (run, _) = record_run(&pool, uid, true).await;
    add_turn(
        &pool,
        run,
        uid,
        1,
        T {
            question: "I am bleeding, is that normal?",
            reply: "So far",
            status: "pending",
            checks: Some(json!({"uncited": true})),
            safety: true,
            personal: true,
            ..T::default()
        },
    )
    .await;
    let m = page(&st, run, &email).await;
    let c = chat_section(&m);
    assert!(c.contains("Still writing. Not checked yet."));
    assert!(!c.contains("Check this reply") && !c.contains("It gives no source."));
    assert!(!c.contains(r#"aria-label="Helpful""#));
    assert!(c.contains(lines::WARNING_LINE));
    assert!(!c.contains(lines::SEE_YOUR_SURGEON), "shown once finished");

    // A pending turn with nothing yet (one pending turn per user).
    let (uid, email) = new_user(&pool).await;
    let (run, _) = record_run(&pool, uid, true).await;
    add_turn(
        &pool,
        run,
        uid,
        1,
        T {
            question: "Q",
            status: "pending",
            ..T::default()
        },
    )
    .await;
    let m = page(&st, run, &email).await;
    let c = chat_section(&m).replace("&#x27;", "'");
    assert!(c.contains("Reading this run's evidence. Replies usually take 15 to 25 seconds."));
    assert!(!c.contains("Still writing"));
}

#[tokio::test]
async fn form_post_works_without_js() {
    let Some((_g, pool)) = begin().await else {
        return;
    };
    let (uid, email) = new_user(&pool).await;
    let server = gemma_says("Zebra recurrence was 5% [1].", Duration::from_secs(2)).await;
    let st = chat_on_gemma(pool.clone(), &server.uri());
    let (run, rec) = record_run(&pool, uid, true).await;
    add_passage(&pool, rec, 9001, "Zebra recurrence was 5 % in this series.").await;

    let (status, loc) = form_post(
        &st,
        "chat_ask",
        &format!(
            "run_id={run}&question={}&mode=plain",
            urlenc("What is the zebra recurrence?")
        ),
        &email,
    )
    .await;
    assert_eq!(status, StatusCode::FOUND);
    assert_eq!(loc, format!("/runs/{run}?chat_wait=1#chat"));

    // The page shows the pending turn and reloads itself.
    let (s, html) = send(router(st.clone()), loc.split('#').next().unwrap(), &email).await;
    assert_eq!(s, StatusCode::OK);
    let m = markup(&html, run).replace("<!---->", "");
    let c = chat_section(&m).replace("&#x27;", "'");
    assert!(c.contains("What is the zebra recurrence?"), "{c}");
    assert!(
        c.contains("Reading this run's evidence") || c.contains("Still writing"),
        "{c}"
    );
    assert!(
        m.contains(r#"http-equiv="refresh""#),
        "reloads while pending"
    );
    assert!(m.contains("?chat_wait=1#chat"), "reloads to the chat view");
    assert!(chat_section(&m).contains("disabled"), "send waits");

    // The same page once the turn has finished: no reload, the reply there.
    let mut last = String::new();
    for _ in 0..200 {
        let (_, html) = send(router(st.clone()), loc.split('#').next().unwrap(), &email).await;
        last = markup(&html, run).replace("<!---->", "");
        if !last.contains(r#"http-equiv="refresh""#) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(!last.contains(r#"http-equiv="refresh""#), "still reloading");
    let c = chat_section(&last);
    assert!(c.contains("Zebra recurrence was 5%"), "{c}");
    assert!(c.contains(r#"aria-label="Open source 1""#));

    // The chosen style is kept: the stored turn and the next form.
    let mode: String =
        sqlx::query_scalar("SELECT mode FROM incisive.chat_turns WHERE user_id = $1")
            .bind(uid)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(mode, "plain");

    // A page with no form post behind it does not reload while pending:
    // the browser code polls.
    let (run2, _) = record_run(&pool, uid, true).await;
    add_turn(
        &pool,
        run2,
        uid,
        1,
        T {
            question: "Q",
            status: "pending",
            ..T::default()
        },
    )
    .await;
    let m = page(&st, run2, &email).await;
    assert!(!m.contains(r#"http-equiv="refresh""#));
}

#[tokio::test]
async fn a_starter_button_posts_its_question() {
    let Some((_g, pool)) = begin().await else {
        return;
    };
    let (uid, email) = new_user(&pool).await;
    let server = gemma_says("Not covered in this evidence.", Duration::ZERO).await;
    let st = chat_on_gemma(pool.clone(), &server.uri());
    let (run, _) = record_run(&pool, uid, true).await;
    let (status, _) = form_post(
        &st,
        "chat_ask",
        &format!(
            "run_id={run}&question=&mode=clinical&starter={}",
            urlenc("What does the evidence show about pain?")
        ),
        &email,
    )
    .await;
    assert_eq!(status, StatusCode::FOUND);
    let q: String =
        sqlx::query_scalar("SELECT question FROM incisive.chat_turns WHERE user_id = $1")
            .bind(uid)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(q, "What does the evidence show about pain?");
}

#[tokio::test]
async fn form_refusal_shows_its_message_and_safety_line_without_the_question() {
    let Some((_g, pool)) = begin().await else {
        return;
    };
    let (uid, email) = new_user(&pool).await;
    let server = gemma_says("x", Duration::ZERO).await;
    let st = chat_on_gemma(pool.clone(), &server.uri());
    let (run, _) = record_run(&pool, uid, true).await;

    // An identifier, with a symptom.
    let (status, loc) = form_post(
        &st,
        "chat_ask",
        &format!(
            "run_id={run}&question={}&mode=clinical",
            urlenc("Mr Sharma, MRN 1234567, is bleeding, what about his hernia?")
        ),
        &email,
    )
    .await;
    assert_eq!(status, StatusCode::FOUND);
    assert!(
        !loc.contains("Sharma") && !loc.contains("bleeding"),
        "{loc}"
    );
    assert_eq!(
        loc,
        format!("/runs/{run}?chat_refused=identifiers&chat_safety=1#chat")
    );
    assert_eq!(count_turns(&pool, uid).await, 0, "nothing stored");
    let (_, html) = send(router(st.clone()), loc.split('#').next().unwrap(), &email).await;
    let m = markup(&html, run).replace("<!---->", "");
    let c = chat_section(&m);
    assert!(c.contains(incisive_app::ask::BLOCKED_MESSAGE), "{c}");
    assert!(c.contains(lines::WARNING_LINE));
    assert!(!m.contains(r#"http-equiv="refresh""#));

    // An empty question has no safety line and its plain message.
    let (_, loc) = form_post(
        &st,
        "chat_ask",
        &format!("run_id={run}&question=&mode=clinical"),
        &email,
    )
    .await;
    assert_eq!(loc, format!("/runs/{run}?chat_refused=empty#chat"));
    let (_, html) = send(router(st.clone()), loc.split('#').next().unwrap(), &email).await;
    let m = markup(&html, run).replace("<!---->", "");
    let c = chat_section(&m);
    assert!(c.contains("Type a question first."));
    assert!(!c.contains(lines::WARNING_LINE));

    // An unknown code in the address shows nothing it names.
    let (_, html) = send(
        router(st.clone()),
        &format!("/runs/{run}?chat_refused=%3Cb%3Ehi"),
        &email,
    )
    .await;
    let m = markup(&html, run);
    assert!(!m.contains("<b>hi") && !m.contains("&lt;b&gt;hi"));
}

#[tokio::test]
async fn server_functions_share_the_rest_routes_rules() {
    let Some((_g, pool)) = begin().await else {
        return;
    };
    let (uid, email) = new_user(&pool).await;
    let (_, other) = new_user(&pool).await;
    let server = gemma_says("Not covered in this evidence.", Duration::ZERO).await;
    let st = chat_on_gemma(pool.clone(), &server.uri());
    let (run, rec) = record_run(&pool, uid, true).await;
    let pid = add_passage(&pool, rec, 9001, "Passage <b>one</b>.").await;
    let qa: i64 = sqlx::query_scalar(
        "INSERT INTO incisive.topic_qas \
         (topic_id, question, gemma_reply, owner_text, status, author_user_id) \
         SELECT topic_id, 'Q?', 'g', 'The owner says so.', 'approved', $2 \
         FROM incisive.evidence_records WHERE id = $1 RETURNING id",
    )
    .bind(rec)
    .bind(uid)
    .fetch_one(&pool)
    .await
    .unwrap();
    let turn = add_turn(
        &pool,
        run,
        uid,
        1,
        T {
            question: "Q",
            reply: "R [1] [2].",
            status: "done",
            ids: vec![pid, -qa],
            checks: Some(json!({})),
            ..T::default()
        },
    )
    .await;

    // Reads, as the page's own code does.
    let (s, v) = call_fn(&st, "my_run_chat", &format!("run_id={run}"), &email).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v["state"], "ready");
    assert_eq!(v["turns"][0]["sources"], json!([pid, -qa]));
    let (s, v) = call_fn(&st, "chat_turn", &format!("turn_id={turn}"), &email).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v["status"], "done");

    // Citation sheets: a passage's kind label, a Q&A's owner label.
    let (s, v) = call_fn(&st, "chat_source", &format!("stored_id={pid}"), &email).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v["label"], "Answer \u{b7} Bottom line");
    assert_eq!(v["text"], "Passage <b>one</b>.");
    let (s, v) = call_fn(&st, "chat_source", &format!("stored_id={}", -qa), &email).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v["label"], "[Owner-reviewed]");
    assert_eq!(v["text"], "The owner says so.");

    // Writes.
    let body = format!("run_id={run}&turn_id={turn}&value=-1");
    let (s, _) = call_fn(&st, "chat_feedback", &body, &email).await;
    assert_eq!(s, StatusCode::OK);
    let (_, v) = call_fn(&st, "chat_turn", &format!("turn_id={turn}"), &email).await;
    assert_eq!(v["feedback"], -1);
    let (s, _) = call_fn(
        &st,
        "chat_feedback",
        &format!("run_id={run}&turn_id={turn}&value=5"),
        &email,
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    let (s, _) = call_fn(
        &st,
        "chat_suggest",
        &format!("run_id={run}&turn_id={turn}"),
        &email,
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let (_, v) = call_fn(&st, "chat_turn", &format!("turn_id={turn}"), &email).await;
    assert_eq!(v["suggested"], true);

    // Someone else's run, turn or citation is "not found", on every call,
    // and asks do not get through.
    let cases = [
        ("my_run_chat", format!("run_id={run}")),
        ("chat_turn", format!("turn_id={turn}")),
        ("chat_source", format!("stored_id={pid}")),
        (
            "chat_feedback",
            format!("run_id={run}&turn_id={turn}&value=1"),
        ),
        ("chat_suggest", format!("run_id={run}&turn_id={turn}")),
    ];
    for (name, body) in &cases {
        let (s, _) = call_fn(&st, name, body, &other).await;
        assert_eq!(s, StatusCode::NOT_FOUND, "{name}");
    }
    let (s, v) = call_fn(
        &st,
        "chat_ask",
        &format!("run_id={run}&question=Hello&mode=clinical"),
        &other,
    )
    .await;
    assert_eq!(s, StatusCode::OK, "a refusal is a value for the page");
    assert_eq!(v["refused"]["message"], "Not found.");

    // A refusal for the page's own code carries the question and the line.
    let (_, v) = call_fn(
        &st,
        "chat_ask",
        &format!(
            "run_id={run}&question={}&mode=nonsense",
            urlenc("I am bleeding")
        ),
        &email,
    )
    .await;
    assert_eq!(v["refused"]["safety_line"], true, "{v}");
    assert_eq!(v["refused"]["question"], "I am bleeding");

    // An ask from the page returns the pending turn.
    let (s, v) = call_fn(
        &st,
        "chat_ask",
        &format!("run_id={run}&question=Hello&mode=plain"),
        &email,
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["accepted"]["status"], "pending");
    assert_eq!(v["accepted"]["question"], "Hello");
    assert_eq!(v["accepted"]["mode"], "plain");
}

#[tokio::test]
async fn chat_server_functions_refuse_with_chat_off() {
    let Some((_g, pool)) = begin().await else {
        return;
    };
    let (uid, email) = new_user(&pool).await;
    let (run, _) = record_run(&pool, uid, true).await;
    let turn = add_turn(
        &pool,
        run,
        uid,
        1,
        T {
            question: "Q",
            reply: "R",
            status: "done",
            ..T::default()
        },
    )
    .await;
    let st = state(pool.clone());
    assert!(!st.chat.on);
    // The page's read is simply "no chat", with no chat query.
    let (s, v) = call_fn(&st, "my_run_chat", &format!("run_id={run}"), &email).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v, Value::Null);
    for (name, body) in [
        ("chat_ask", format!("run_id={run}&question=Hi&mode=plain")),
        ("chat_turn", format!("turn_id={turn}")),
        ("chat_source", format!("stored_id={turn}")),
        (
            "chat_feedback",
            format!("run_id={run}&turn_id={turn}&value=1"),
        ),
        ("chat_suggest", format!("run_id={run}&turn_id={turn}")),
    ] {
        let (s, _) = call_fn(&st, name, &body, &email).await;
        assert_eq!(s, StatusCode::NOT_FOUND, "{name}");
    }
    assert_eq!(count_turns(&pool, uid).await, 1, "nothing asked");
}

#[tokio::test]
async fn chat_text_is_escaped_and_other_users_see_nothing() {
    let Some((_g, pool)) = begin().await else {
        return;
    };
    let (uid, email) = new_user(&pool).await;
    let (_, other) = new_user(&pool).await;
    let server = gemma_says("x", Duration::ZERO).await;
    let st = chat_on_gemma(pool.clone(), &server.uri());
    let (run, _) = record_run(&pool, uid, true).await;
    add_turn(
        &pool,
        run,
        uid,
        1,
        T {
            question: "<img src=x onerror=alert(1)>?",
            reply: "<script>alert(2)</script>",
            status: "done",
            checks: Some(json!({"not_traced": ["<i>3</i>"]})),
            ..T::default()
        },
    )
    .await;
    let m = page(&st, run, &email).await;
    let c = chat_section(&m);
    assert!(
        !c.contains("<img") && !c.contains("<script") && !c.contains("<i>3"),
        "{c}"
    );
    assert!(c.contains("&lt;img") && c.contains("&lt;script"));

    // Another user's run page reveals nothing about the chat.
    let (_, html) = send(router(st.clone()), &format!("/runs/{run}"), &other).await;
    let m = markup(&html, run);
    assert!(!m.contains("onerror") && !m.contains("rx-chat"));
    assert!(!html.contains("onerror"), "nor in the hydration payload");
}

#[tokio::test]
async fn chat_form_post_is_refused_cross_site() {
    let Some((_g, pool)) = begin().await else {
        return;
    };
    let (uid, email) = new_user(&pool).await;
    let server = gemma_says("x", Duration::ZERO).await;
    let st = chat_on_gemma(pool.clone(), &server.uri());
    let (run, _) = record_run(&pool, uid, true).await;
    let req = Request::post("/api/fn/chat_ask")
        .header("content-type", "application/x-www-form-urlencoded")
        .header("accept", "text/html")
        .header("host", "incisive.example")
        .header("origin", "https://evil.example")
        .header("x-dev-user-email", &email)
        .body(Body::from(format!("run_id={run}&question=Hi&mode=plain")))
        .unwrap();
    let res = router(st).oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
    assert_eq!(count_turns(&pool, uid).await, 0);
}

#[tokio::test]
async fn a_refusal_shows_when_the_chat_is_not_ready() {
    let Some((_g, pool)) = begin().await else {
        return;
    };
    let (uid, email) = new_user(&pool).await;

    // The server went down between the page and the form post: the view
    // stays shut, and the message and its safety line still show.
    let down = chat_on(pool.clone());
    let (run, _) = record_run(&pool, uid, true).await;
    let path = format!("/runs/{run}?chat_refused=unavailable&chat_safety=1");
    let (_, html) = send(router(down.clone()), &path, &email).await;
    let m = no_style(&markup(&html, run).replace("<!---->", ""));
    assert!(m.contains(lines::UNAVAILABLE), "the message");
    assert!(m.contains(lines::WARNING_LINE), "the safety line");
    assert!(m.contains("Chat \u{b7} Unavailable"));
    assert!(
        m.contains(r#"<div class="ap">"#),
        "the answer is still shown"
    );
    // Before the chat section, which stays empty for `:target`.
    let at = m.find(r#"id="chat""#).unwrap();
    assert!(m.find(lines::WARNING_LINE).unwrap() < at);
    assert!(!chat_section(&m).contains(lines::WARNING_LINE));

    // A run chat is not offered on: the same.
    let failed = full_run(&pool, uid, "hernia").await;
    temporal_row(&pool, failed, "hernia", Some(JobStatus::Failed), None).await;
    let path = format!("/runs/{failed}?chat_refused=not_offered&chat_safety=1");
    let (_, html) = send(router(down.clone()), &path, &email).await;
    let m = no_style(&markup(&html, failed).replace("<!---->", ""));
    assert!(m.contains(lines::NOT_OFFERED_MESSAGE));
    assert!(m.contains(lines::WARNING_LINE));
    assert!(!m.contains(r##"href="#chat""##) && !m.contains("Chat \u{b7}"));

    // Nothing named: nothing drawn, and the page is as it was.
    let (_, html) = send(router(down.clone()), &format!("/runs/{failed}"), &email).await;
    assert!(!markup(&html, failed).contains("rx-chat"));
}

#[tokio::test]
async fn a_form_refusal_keeps_the_identifier_hint_as_a_code() {
    let Some((_g, pool)) = begin().await else {
        return;
    };
    let (uid, email) = new_user(&pool).await;
    let server = gemma_says("x", Duration::ZERO).await;
    let st = chat_on_gemma(pool.clone(), &server.uri());
    let (run, _) = record_run(&pool, uid, true).await;
    let question = "Secretword\u{200b}here about the hernia?";
    let (status, loc) = form_post(
        &st,
        "chat_ask",
        &format!("run_id={run}&question={}&mode=clinical", urlenc(question)),
        &email,
    )
    .await;
    assert_eq!(status, StatusCode::FOUND);
    assert_eq!(
        loc,
        format!("/runs/{run}?chat_refused=identifiers&chat_hints=unusual_characters#chat")
    );
    assert!(!loc.contains("Secretword"));
    assert_eq!(count_turns(&pool, uid).await, 0);
    let (_, html) = send(router(st.clone()), loc.split('#').next().unwrap(), &email).await;
    let m = markup(&html, run).replace("<!---->", "");
    let c = chat_section(&m);
    // The same message as the page's own code gets.
    let (_, v) = call_fn(
        &st,
        "chat_ask",
        &format!("run_id={run}&question={}&mode=clinical", urlenc(question)),
        &email,
    )
    .await;
    let message = v["refused"]["message"].as_str().unwrap();
    assert!(
        message.len() > incisive_app::ask::BLOCKED_MESSAGE.len(),
        "{message}"
    );
    assert!(c.contains(message), "{c}");

    // A code that is not one adds nothing.
    let path = format!("/runs/{run}?chat_refused=identifiers&chat_hints=%3Cb%3Ehi,phone");
    let (_, html) = send(router(st.clone()), &path, &email).await;
    let m = markup(&html, run).replace("<!---->", "");
    let c = chat_section(&m);
    assert!(c.contains(incisive_app::ask::BLOCKED_MESSAGE));
    assert!(!m.contains("<b>hi") && !m.contains("&lt;b&gt;hi"));
    assert!(!c.contains(message));
}

#[tokio::test]
async fn a_thumb_form_post_returns_to_the_chat() {
    let Some((_g, pool)) = begin().await else {
        return;
    };
    let (uid, email) = new_user(&pool).await;
    let (_, other) = new_user(&pool).await;
    let server = gemma_says("x", Duration::ZERO).await;
    let st = chat_on_gemma(pool.clone(), &server.uri());
    let (run, _) = record_run(&pool, uid, true).await;
    let turn = add_turn(
        &pool,
        run,
        uid,
        1,
        T {
            question: "Q",
            reply: "R",
            status: "done",
            checks: Some(json!({})),
            ..T::default()
        },
    )
    .await;
    let stored = |pool: PgPool| async move {
        sqlx::query_scalar::<_, Option<i16>>(
            "SELECT feedback FROM incisive.chat_turns WHERE id = $1",
        )
        .bind(turn)
        .fetch_one(&pool)
        .await
        .unwrap()
    };
    let body = format!("run_id={run}&turn_id={turn}&value=-1");
    // Someone else's turn: not stored, and "not found" on the way back.
    let (status, loc) = form_post(&st, "chat_feedback", &body, &other).await;
    assert_eq!(status, StatusCode::FOUND);
    assert_eq!(loc, format!("/runs/{run}?chat_refused=not_found#chat"));
    assert_eq!(stored(pool.clone()).await, None);
    // The owner's: stored, back at the chat, and the page shows it pressed.
    let (status, loc) = form_post(&st, "chat_feedback", &body, &email).await;
    assert_eq!(status, StatusCode::FOUND);
    assert_eq!(loc, format!("/runs/{run}#chat"));
    assert_eq!(stored(pool.clone()).await, Some(-1));
    let m = page(&st, run, &email).await;
    assert!(chat_section(&m).contains(r#"aria-pressed="true""#));
}

/// The chat's styles reach neither the answer content nor the whole page.
#[test]
fn chat_style_is_scoped() {
    let style = incisive_app::app::CHAT_STYLE;
    assert!(!style.contains(".ap"));
    for line in style.lines().map(str::trim).filter(|l| !l.is_empty()) {
        assert!(
            line.starts_with(".rx")
                || line.starts_with("/*")
                || line.starts_with("@media")
                || line == "}",
            "{line}"
        );
    }
    // Long unbroken words wrap instead of widening the page on a phone.
    let i = style.find("\n.rx-chat {").unwrap();
    let rule = &style[i..i + style[i..].find('}').unwrap()];
    assert!(rule.contains("overflow-wrap:anywhere"));
    // The frozen desktop column.
    assert!(rule.contains("max-width:760px"));
    // A phone does not zoom on focus (under 16 px it does).
    let phone = &style[style.find("@media (max-width:640px)").unwrap()..];
    assert!(phone.contains(".rx .rx-ask textarea { font-size:16px; }"));
    // A citation's touch area is as wide as the chip: neighbours stay apart.
    let i = style.find(".rx .rx-source::after").unwrap();
    let rule = &style[i..i + style[i..].find('}').unwrap()];
    assert!(
        rule.contains("left:0") && rule.contains("right:0"),
        "{rule}"
    );
    assert!(rule.contains("height:44px") && !rule.contains("width:44px"));
    // A long passage's Close stays in reach.
    let i = style.find(".rx-sheet-head {").unwrap();
    let rule = &style[i..i + style[i..].find('}').unwrap()];
    assert!(rule.contains("position:sticky") && rule.contains("background:"));
    // On a phone the question box stays above the fixed nav.
    assert!(style.contains(".rx-composer { bottom:calc(78px"));
}

/// With `CHAT_UI_PREVIEW=<dir>` and `--ignored`: each state's raw page, for a
/// look in a browser.
#[tokio::test]
#[ignore = "writes the preview pages; run by hand"]
async fn write_the_static_preview() {
    let Ok(out) = std::env::var("CHAT_UI_PREVIEW") else {
        return;
    };
    let Some((_g, pool)) = begin().await else {
        return;
    };
    let server = gemma_says("x", Duration::ZERO).await;
    let st = chat_on_gemma(pool.clone(), &server.uri());
    let long = "People who had robotic surgery probably went home sooner than people who had open surgery [1]. The gap of about four days may be partly because the people having open surgery had larger hernias [2]. Doctors are not very sure of this, because most of these studies did not decide the operation by chance. Averyveryveryveryveryveryveryveryveryveryveryveryveryveryverylongunbrokenword.";
    let cases: Vec<(&str, Option<T>)> = vec![
        ("empty", None),
        (
            "flags",
            Some(T {
                question: "How much sooner do people go home after robotic TAR?",
                reply: long,
                status: "done",
                checks: Some(json!({"not_traced": ["3 days"], "uncited": true})),
                personal: true,
                safety: true,
                feedback: Some(1),
                ..T::default()
            }),
        ),
        (
            "writing",
            Some(T {
                question: "I am bleeding, what now?",
                reply: "So far [1]",
                status: "pending",
                safety: true,
                ..T::default()
            }),
        ),
        (
            "waiting",
            Some(T {
                question: "Q",
                status: "pending",
                ..T::default()
            }),
        ),
        (
            "not-covered",
            Some(T {
                question: "Is robotic TAR better for an obese patient?",
                status: "not_covered",
                ..T::default()
            }),
        ),
    ];
    std::fs::create_dir_all(&out).unwrap();
    for (name, turn) in cases {
        let (uid, email) = new_user(&pool).await;
        let (run, rec) = record_run(&pool, uid, true).await;
        if let Some(mut t) = turn {
            let a = add_passage(&pool, rec, 9001, "One.").await;
            let b = add_passage(&pool, rec, 9002, "Two.").await;
            t.ids = vec![a, b];
            add_turn(&pool, run, uid, 1, t).await;
        }
        let q = if name == "writing" {
            "?chat_wait=1"
        } else {
            ""
        };
        let (_, html) = send(router(st.clone()), &format!("/runs/{run}{q}"), &email).await;
        std::fs::write(format!("{out}/{name}.html"), html).unwrap();
    }
    // A refusal, and the option's states.
    let (uid, email) = new_user(&pool).await;
    let (run, _) = record_run(&pool, uid, true).await;
    let (_, html) = send(
        router(st.clone()),
        &format!("/runs/{run}?chat_refused=identifiers&chat_safety=1"),
        &email,
    )
    .await;
    std::fs::write(format!("{out}/refused.html"), html).unwrap();
    let (_, html) = send(
        router(chat_on(pool.clone())),
        &format!("/runs/{run}"),
        &email,
    )
    .await;
    std::fs::write(format!("{out}/unavailable.html"), html).unwrap();
    let (run, _) = record_run(&pool, uid, false).await;
    let (_, html) = send(router(st.clone()), &format!("/runs/{run}"), &email).await;
    std::fs::write(format!("{out}/getting-ready.html"), html).unwrap();
}
