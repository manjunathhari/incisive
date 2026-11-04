//! The introduction on the run page, rendered by the real router (SSR), with
//! the database (`TEST_DATABASE_URL`): running, finished and failed, order on
//! the page, scoping, and a page byte-identical when there is nothing to show.

use std::sync::Arc;

use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode},
};
use http_body_util::BodyExt;
use incisive_app::db::{lazy_pool, migrate};
use incisive_app::gate::OfflineClassifier;
use incisive_app::identity::{IapVerifier, JwkSource, find_user_by_id};
use incisive_app::intro_page::{CLOSING, READ, TAG_FINISHED, TAG_RUNNING};
use incisive_app::runs::intro::{Finished, IntroSettings, JobStatus, finish, insert_pending};
use incisive_app::runs::store::{add_step, create_run, save_answer, set_status};
use incisive_app::runs::worker::WorkerHandle;
use incisive_app::runs::{IntroView, RunMode, RunStatus, RunView, RunsConfig, StepKind, api};
use incisive_app::server::AppState;
use incisive_app::spend::{CapsConfig, Prices};
use leptos::config::LeptosOptions;
use secrecy::SecretString;
use serde_json::Value;
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

const MODEL: &str = "m-test";
const P1: &str = "Gastric cancer arises from the lining of the stomach.";
const P2: &str = "The question asks how to time chemotherapy.";

async fn connect() -> Option<PgPool> {
    let Ok(url) = std::env::var("TEST_DATABASE_URL") else {
        eprintln!("SKIP: TEST_DATABASE_URL not set");
        return None;
    };
    let pool = lazy_pool(&SecretString::from(url)).expect("lazy pool");
    migrate(&pool).await.expect("migrate");
    Some(pool)
}

fn state(pool: PgPool, on: bool) -> AppState {
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
        intro: incisive_app::runs::intro::IntroRuntime {
            settings: IntroSettings {
                on,
                ..IntroSettings::default()
            },
            client: None,
        },
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
    let email = format!("intro-ui-{}@example.com", Uuid::new_v4());
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

async fn running_run(pool: &PgPool, uid: i64, mode: RunMode) -> Uuid {
    let run = create_run(pool, uid, "Question about gastric cancer?", mode, 4000)
        .await
        .unwrap()
        .unwrap();
    assert!(
        set_status(pool, run, RunStatus::Running, None)
            .await
            .unwrap()
    );
    add_step(
        pool,
        run,
        StepKind::Search,
        "Researcher: searching PubMed and Europe PMC",
    )
    .await
    .unwrap();
    run
}

async fn finished_run(pool: &PgPool, uid: i64, mode: RunMode) -> Uuid {
    let body = fixture("hernia");
    let q = body["question"].as_str().unwrap().to_string();
    let run = create_run(pool, uid, &q, mode, 4000)
        .await
        .unwrap()
        .unwrap();
    assert!(
        set_status(pool, run, RunStatus::Running, None)
            .await
            .unwrap()
    );
    add_step(
        pool,
        run,
        StepKind::Search,
        "Researcher: searching PubMed and Europe PMC",
    )
    .await
    .unwrap();
    assert!(save_answer(pool, run, &body).await.unwrap());
    assert!(set_status(pool, run, RunStatus::Done, None).await.unwrap());
    run
}

async fn with_row(pool: &PgPool, run: Uuid, status: JobStatus, text: Option<String>) {
    assert!(insert_pending(pool, run, Uuid::new_v4()).await.unwrap());
    let f = Finished {
        reason: None,
        text,
        cost_paise: 55,
    };
    assert!(finish(pool, run, status, &f).await.unwrap());
}

async fn done_row(pool: &PgPool, run: Uuid) {
    with_row(pool, run, JobStatus::Done, Some(format!("{P1}\n\n{P2}"))).await;
}

/// The page outside its scripts, `<!>` markers kept, tokens in place of the
/// run id, the date, nonces and the pulse's phase; streamed chunks in their slots.
fn exact(html: &str, run: Uuid) -> String {
    let date = incisive_app::library::header_date(incisive_app::library::now_utc());
    let mut m = String::new();
    let mut rest = html;
    while let Some(i) = rest.find("<script") {
        m.push_str(&rest[..i]);
        rest = rest[i..]
            .find("</script>")
            .map_or("", |j| &rest[i + j + "</script>".len()..]);
    }
    m.push_str(rest);
    let nonce = regex::Regex::new(r#"(crossorigin|nonce)="[^"]*""#).unwrap();
    let m = nonce.replace_all(&m, "$1=\"NONCE\"").into_owned();
    let delay = regex::Regex::new(r"animation-delay:-\d+ms").unwrap();
    let m = delay.replace_all(&m, "animation-delay:PHASE").into_owned();
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

fn markup(html: &str, run: Uuid) -> String {
    exact(html, run).replace("<!>", "")
}

async fn page(st: &AppState, run: Uuid, email: &str) -> String {
    let (code, html) = send(router(st.clone()), &format!("/runs/{run}"), email).await;
    assert_eq!(code, StatusCode::OK);
    html
}

/// With `INTRO_DUMP=<dir>`, write the page there (debugging only).
fn dump(name: &str, html: &str) {
    if let Ok(d) = std::env::var("INTRO_DUMP") {
        std::fs::write(format!("{d}/{name}.html"), html).unwrap();
    }
}

fn in_order(m: &str, what: &[&str]) {
    let mut from = 0;
    for w in what {
        let at = m[from..]
            .find(w)
            .unwrap_or_else(|| panic!("`{w}` missing or out of order"));
        from += at + w.len();
    }
}

#[tokio::test]
async fn running_shows_the_card_in_its_place_with_plain_text() {
    let Some(pool) = connect().await else { return };
    let st = state(pool.clone(), true);
    let (uid, email) = new_user(&pool).await;
    for mode in [RunMode::Quick, RunMode::Focused] {
        let run = running_run(&pool, uid, mode).await;
        done_row(&pool, run).await;
        let html = page(&st, run, &email).await;
        dump(&format!("running-{mode:?}"), &html);
        let m = markup(&html, run);
        in_order(
            &m,
            &[
                "run-question",
                "rx-seg",
                "in-card",
                "<h2>Introduction</h2>",
                TAG_RUNNING,
                P1,
                P2,
                CLOSING,
                "Results so far",
                "Progress",
            ],
        );
        // Whole, for a page loaded mid-run: nothing hidden, no caret showing.
        assert!(m.contains(&format!("<p>{P1}")), "{m}");
        assert!(m.contains(r#"<p class="in-close">"#), "closing line shown");
        assert!(m.contains(r#"aria-label="Introduction""#));
        assert!(!m.contains(TAG_FINISHED));
        assert!(!m.contains("in-fold\""));
        assert!(!m.contains('\u{20b9}'));
        set_status(&pool, run, RunStatus::Done, None).await.unwrap();
    }
}

#[tokio::test]
async fn finished_folds_in_its_place() {
    let Some(pool) = connect().await else { return };
    let st = state(pool.clone(), true);
    let (uid, email) = new_user(&pool).await;
    for mode in [RunMode::Quick, RunMode::Focused] {
        let run = finished_run(&pool, uid, mode).await;
        done_row(&pool, run).await;
        let html = page(&st, run, &email).await;
        dump(&format!("finished-{mode:?}"), &html);
        let m = markup(&html, run);
        in_order(
            &m,
            &[
                "run-question",
                "rx-seg",
                r#"<details class="in-fold""#,
                "Introduction",
                READ,
                TAG_FINISHED,
                P1,
                P2,
                "</details>",
                "Results",
                "Progress",
                "At a glance",
            ],
        );
        // Folded: `open` is set only in the browser.
        assert!(!m.contains(r#"class="in-fold" open"#));
        assert!(!m.contains(CLOSING));
        assert!(!m.contains(TAG_RUNNING));
        assert!(!m.contains(r#"class="rk-card in-card""#));
        assert!(!m.contains(r#"class="in-caret""#));
    }
}

/// With an expert opinion row the page has another layout: the same place.
#[tokio::test]
async fn folds_in_its_place_beside_an_expert_opinion() {
    let Some(pool) = connect().await else { return };
    let st = state(pool.clone(), true);
    let (uid, email) = new_user(&pool).await;
    let run = finished_run(&pool, uid, RunMode::Focused).await;
    done_row(&pool, run).await;
    sqlx::query("INSERT INTO incisive.run_temporal (run_id, status) VALUES ($1, 'pending')")
        .bind(run)
        .execute(&pool)
        .await
        .unwrap();
    let m = markup(&page(&st, run, &email).await, run);
    in_order(
        &m,
        &[
            "rx-seg",
            r#"<details class="in-fold""#,
            "</details>",
            r#"class="rk-card""#,
        ],
    );
}

#[tokio::test]
async fn a_failed_run_with_a_text_still_folds() {
    let Some(pool) = connect().await else { return };
    let st = state(pool.clone(), true);
    let (uid, email) = new_user(&pool).await;
    let run = running_run(&pool, uid, RunMode::Focused).await;
    done_row(&pool, run).await;
    set_status(&pool, run, RunStatus::Failed, Some("hard_stop"))
        .await
        .unwrap();
    let m = markup(&page(&st, run, &email).await, run);
    assert!(m.contains(r#"<details class="in-fold""#));
    assert!(!m.contains(CLOSING));
}

#[tokio::test]
async fn text_is_escaped() {
    let Some(pool) = connect().await else { return };
    let st = state(pool.clone(), true);
    let (uid, email) = new_user(&pool).await;
    let run = running_run(&pool, uid, RunMode::Quick).await;
    with_row(
        &pool,
        run,
        JobStatus::Done,
        Some("<img src=x onerror=alert(1)> first.\n\n<b>Second.</b>".into()),
    )
    .await;
    let m = markup(&page(&st, run, &email).await, run);
    assert!(!m.contains("<img src=x"));
    assert!(!m.contains("<b>Second"));
    assert!(m.contains("&lt;b&gt;Second."));
    set_status(&pool, run, RunStatus::Done, None).await.unwrap();
}

/// Nothing to show means the page, scripts apart, is exactly the page with no
/// row at all: running or finished, for every status.
#[tokio::test]
async fn nothing_to_show_is_byte_identical() {
    let Some(pool) = connect().await else { return };
    let st = state(pool.clone(), true);
    let (uid, email) = new_user(&pool).await;
    for (status, finished) in [
        (JobStatus::Failed, false),
        (JobStatus::Skipped, false),
        (JobStatus::CapBlocked, false),
        (JobStatus::Failed, true),
        (JobStatus::CapBlocked, true),
    ] {
        let run = if finished {
            finished_run(&pool, uid, RunMode::Focused).await
        } else {
            running_run(&pool, uid, RunMode::Focused).await
        };
        let none = exact(&page(&st, run, &email).await, run);
        with_row(&pool, run, status, None).await;
        let row = exact(&page(&st, run, &email).await, run);
        assert_eq!(row, none, "{status:?} finished={finished}");
        set_status(&pool, run, RunStatus::Done, None).await.unwrap();
    }
}

/// While pending: no card, no spinner, no empty box.
#[tokio::test]
async fn pending_draws_nothing() {
    let Some(pool) = connect().await else { return };
    let st = state(pool.clone(), true);
    let (uid, email) = new_user(&pool).await;
    let run = running_run(&pool, uid, RunMode::Quick).await;
    let none = exact(&page(&st, run, &email).await, run);
    assert!(insert_pending(&pool, run, Uuid::new_v4()).await.unwrap());
    assert_eq!(exact(&page(&st, run, &email).await, run), none);
    set_status(&pool, run, RunStatus::Done, None).await.unwrap();
}

/// `INTRO=off`: a row from an earlier `on` period shows nothing at all, and
/// the page's data carries no trace of it.
#[tokio::test]
async fn off_shows_nothing_and_sends_nothing() {
    let Some(pool) = connect().await else { return };
    let (uid, email) = new_user(&pool).await;
    let off = state(pool.clone(), false);
    let run = finished_run(&pool, uid, RunMode::Focused).await;
    let before = page(&off, run, &email).await;
    done_row(&pool, run).await;
    let after = page(&off, run, &email).await;
    assert_eq!(exact(&after, run), exact(&before, run));
    assert!(!after.contains(P1), "not in the markup or the data");
    assert!(!after.contains("in-fold"));
    // The same run with it on.
    let on = page(&state(pool.clone(), true), run, &email).await;
    assert!(on.contains(P1));
}

#[tokio::test]
async fn an_old_run_has_nothing() {
    let Some(pool) = connect().await else { return };
    let st = state(pool.clone(), true);
    let (uid, email) = new_user(&pool).await;
    let run = finished_run(&pool, uid, RunMode::Quick).await;
    let m = markup(&page(&st, run, &email).await, run);
    assert!(!m.contains("in-fold"));
    assert!(!m.contains("in-card"));
    assert!(!m.contains(TAG_FINISHED));
}

#[tokio::test]
async fn another_user_gets_nothing() {
    let Some(pool) = connect().await else { return };
    let st = state(pool.clone(), true);
    let (uid, email) = new_user(&pool).await;
    let (other, other_email) = new_user(&pool).await;
    let run = running_run(&pool, uid, RunMode::Quick).await;
    done_row(&pool, run).await;
    // The owner sees it; the other user's page has none of it.
    assert!(page(&st, run, &email).await.contains(P1));
    let (_, theirs) = send(router(st.clone()), &format!("/runs/{run}"), &other_email).await;
    assert!(!theirs.contains(P1));
    assert!(!theirs.contains("in-card"));
    // The read itself is "not found", as for the run.
    let them = find_user_by_id(&pool, other).await.unwrap().unwrap();
    assert_eq!(
        api::intro_view(&st, &them, run).await.unwrap_err(),
        api::RunApiError::NotFound
    );
    let owner = find_user_by_id(&pool, uid).await.unwrap().unwrap();
    assert_eq!(
        api::intro_view(&st, &owner, run).await.unwrap(),
        Some(IntroView::Done {
            text: format!("{P1}\n\n{P2}")
        })
    );
    // The JSON route for the run stays as it was: no introduction in it.
    let (code, body) = send(router(st), &format!("/api/runs/{run}"), &email).await;
    assert_eq!(code, StatusCode::OK);
    assert!(!body.contains(P1));
    set_status(&pool, run, RunStatus::Done, None).await.unwrap();
}

/// On the wire a run with nothing to show is what it was before.
#[test]
fn the_field_is_absent_when_empty() {
    let v: Value = serde_json::json!({
        "summary": {
            "id": Uuid::nil(),
            "title": "t",
            "mode": "quick",
            "status": "done",
            "created_at": "2026-11-03T00:00:00Z",
            "finished_at": null,
            "cost_paise": 0,
            "error_kind": null
        },
        "steps": [],
        "answer": null,
        "answer_version": 2
    });
    if let Ok(run) = serde_json::from_value::<RunView>(v) {
        assert_eq!(run.intro, None);
        assert!(serde_json::to_value(&run).unwrap().get("intro").is_none());
    }
    assert_eq!(
        serde_json::to_value(IntroView::Done { text: "x".into() }).unwrap(),
        serde_json::json!({"status": "done", "text": "x"})
    );
    assert_eq!(
        serde_json::to_value(IntroView::Pending).unwrap(),
        serde_json::json!({"status": "pending"})
    );
}

/// No `inner_html` and no money in its code.
#[test]
fn no_inner_html_and_no_rupee_sign() {
    for f in ["src/app/intro.rs", "src/intro_page.rs", "src/runs/intro.rs"] {
        let src = std::fs::read_to_string(format!("{}/{f}", env!("CARGO_MANIFEST_DIR"))).unwrap();
        assert!(!src.contains("inner_html("), "{f}");
        assert!(!src.contains("inner_html="), "{f}");
        assert!(!src.contains('\u{20b9}'), "{f}");
        assert!(!src.contains("20b9"), "{f}");
    }
}
