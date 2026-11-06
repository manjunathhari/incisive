//! Stage S1: the page while a writer works, server-rendered through the real
//! routes. Nothing here has run in a browser. Needs `TEST_DATABASE_URL`
//! (skipped when unset).

use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use incisive_app::agent::testing::{idle_llm, offline_literature, placeholder_pipeline};
use incisive_app::db::{lazy_pool, migrate};
use incisive_app::gate::OfflineClassifier;
use incisive_app::identity::{IapVerifier, JwkSource};
use incisive_app::runs::drafts::{self, Part};
use incisive_app::runs::store::{add_step, create_run, save_answer, set_status};
use incisive_app::runs::temporal::{Finished, JobStatus, finish, insert_pending};
use incisive_app::runs::worker::WorkerHandle;
use incisive_app::runs::{RunMode, RunStatus, RunsConfig, StepKind};
use incisive_app::server::AppState;
use incisive_app::spend::{CapsConfig, Prices};
use leptos::config::LeptosOptions;
use secrecy::SecretString;
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

const MODEL: &str = "m-test";
const BANNER: &str = "Being written: checks not yet run";

async fn connect() -> Option<PgPool> {
    let Ok(url) = std::env::var("TEST_DATABASE_URL") else {
        eprintln!("SKIP: TEST_DATABASE_URL not set");
        return None;
    };
    let pool = lazy_pool(&SecretString::from(url)).expect("lazy pool");
    migrate(&pool).await.expect("migrate");
    Some(pool)
}

fn state(pool: PgPool, stream: bool) -> AppState {
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
        stream,
        intro: Default::default(),
        graph: Default::default(),
        memory: Default::default(),
        chat: Default::default(),
        embedder: None,
        gemma: None,
        worker: WorkerHandle::with_pipeline(placeholder_pipeline),
        llm: Some(idle_llm(MODEL)),
        lit: offline_literature(),
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

async fn page(st: &AppState, run: Uuid, email: &str) -> (StatusCode, String) {
    let req = Request::builder()
        .method("GET")
        .uri(format!("/runs/{run}"))
        .header("x-dev-user-email", email)
        .body(Body::empty())
        .unwrap();
    let res = router(st.clone()).oneshot(req).await.unwrap();
    let status = res.status();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

async fn new_user(pool: &PgPool) -> (i64, String) {
    let email = format!("s1-{}@example.com", Uuid::new_v4());
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

/// A Full run, running, at the writer.
async fn at_the_writer(pool: &PgPool, uid: i64) -> Uuid {
    let run = create_run(
        pool,
        uid,
        "Does robotic repair help?",
        RunMode::Focused,
        40000,
    )
    .await
    .unwrap()
    .unwrap();
    assert!(
        set_status(pool, run, RunStatus::Running, None)
            .await
            .unwrap()
    );
    for line in [
        "Researcher: selected 15 studies (15 RCTs)",
        "Writer: writing the answer from 15 studies (15 with an abstract)",
    ] {
        add_step(pool, run, StepKind::Write, line).await.unwrap();
    }
    run
}

fn fixture(name: &str) -> Value {
    let p = format!(
        "{}/tests/fixtures/temporal/full/{name}.json",
        env!("CARGO_MANIFEST_DIR")
    );
    serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap()
}

/// A finished Full run with the stored fixture answer.
async fn finished_full_run(pool: &PgPool, uid: i64) -> Uuid {
    let body = fixture("hernia");
    let run = create_run(
        pool,
        uid,
        body["question"].as_str().unwrap(),
        RunMode::Focused,
        4000,
    )
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

/// The page without its scripts (they carry nonces and the resource payload),
/// nonces blanked, and each streamed chunk back in its slot (how many miss the
/// shell's flush depends on timing), as `temporal_ui` reads a page.
fn markup(html: &str) -> String {
    let mut out = String::new();
    let mut rest = html;
    while let Some(i) = rest.find("<script") {
        out.push_str(&rest[..i]);
        rest = rest[i..]
            .find("</script>")
            .map_or("", |j| &rest[i + j + "</script>".len()..]);
    }
    out.push_str(rest);
    let nonce = regex::Regex::new(r#"(crossorigin|nonce)="[^"]*""#).unwrap();
    let mut m = nonce.replace_all(&out, "$1=\"NONCE\"").into_owned();
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
    slot.replace_all(&m, "").replace("<!>", "")
}

/// A part of a reply: at a glance and bottom line in, the bottom line cut off.
fn reply_so_far() -> String {
    concat!(
        r#"{"applicability":{"case_mix":"Mostly primary hernias.","india_cost_context":"Cost differs.","training":"Short curve."},"#,
        r#""at_a_glance":"Verdict: Robotic repair is no better <b>so far</b>.\n- Pain: 4 vs 5 [41060645]","#,
        r#""bottom_line":{"certainty":"Low","certainty_reason":"Few small trials.","text":"Randomised trials show no consistent benefit and usually takes"#
    )
    .to_string()
}

fn at(html: &str, s: &str) -> usize {
    html.find(s).unwrap_or_else(|| panic!("missing {s:?}"))
}

#[tokio::test]
async fn the_draft_shows_under_the_banner_in_page_order_with_coming_rows() {
    let Some(pool) = connect().await else { return };
    let (uid, email) = new_user(&pool).await;
    let run = at_the_writer(&pool, uid).await;
    assert!(
        drafts::upsert(&pool, run, Part::Answer, &reply_so_far())
            .await
            .unwrap()
    );
    let st = state(pool.clone(), true);
    let (s, html) = page(&st, run, &email).await;
    assert_eq!(s, StatusCode::OK);
    let m = markup(&html);
    assert!(m.contains(BANNER), "the banner");
    // Page order, whatever order they arrived in (applicability came first).
    let order = [
        "At a glance",
        "Bottom line",
        "What this means for clinical practice",
        "Outcome by outcome",
        "Conflicting evidence",
        "Applicability",
        "What would change this answer",
        "The trials",
    ];
    let mut last = at(&m, BANNER);
    for t in order {
        let i = last + at(&m[last..], t);
        assert!(i >= last, "{t}");
        last = i;
    }
    // Not arrived: "Coming". Arrived: no "Coming" beside it.
    assert!(m.contains(r#"<div class="dr-coming">What this means for clinical practice"#));
    assert!(m.contains(r#"<div class="dr-coming">Outcome by outcome"#));
    assert!(!m.contains(r#"<div class="dr-coming">Bottom line"#));
    assert!(!m.contains(r#"<div class="dr-coming">Applicability"#));
    // The last words of the unfinished text are faded.
    assert!(
        m.contains(r#"<span class="dr-fade"> and usually takes"#),
        "{m}"
    );
    // The citation reads [n]; no PMID on the page.
    assert!(m.contains("[1]"));
    assert!(!m.contains("41060645"));
    // Escaped: never markup from the model.
    assert!(m.contains("&lt;b&gt;so far&lt;/b&gt;"));
    assert!(!m.contains("<b>so far"));
    // The results card is folded to a row with the clock, and still there.
    assert!(m.contains(r#"class="dr-fold""#));
    let fold = at(&m, r#"class="dr-fold""#);
    assert!(m[fold..].contains("Results so far"));
    assert!(m[fold..].contains("dr-clock"));
    assert!(m[fold..].contains(r#"class="rk-card""#));
    // No checks are shown on the draft.
    assert!(!m.contains("not traced"));
    assert!(!m.contains("Copy as text"));
}

/// A page opened or reloaded mid-run reads the same draft.
#[tokio::test]
async fn a_reload_mid_run_shows_the_same_draft() {
    let Some(pool) = connect().await else { return };
    let (uid, email) = new_user(&pool).await;
    let run = at_the_writer(&pool, uid).await;
    assert!(
        drafts::upsert(&pool, run, Part::Answer, &reply_so_far())
            .await
            .unwrap()
    );
    let st = state(pool.clone(), true);
    let (_, a) = page(&st, run, &email).await;
    let (_, b) = page(&st, run, &email).await;
    assert!(markup(&a).contains(BANNER));
    assert!(markup(&b).contains(BANNER));
    // The hydration payload carries the same text.
    assert!(b.contains("Randomised trials show no consistent benefit"));
}

/// Before the first words: as today, no banner and the card unfolded.
#[tokio::test]
async fn before_the_first_words_the_page_is_as_it_was() {
    let Some(pool) = connect().await else { return };
    let (uid, email) = new_user(&pool).await;
    let run = at_the_writer(&pool, uid).await;
    let on = state(pool.clone(), true);
    let off = state(pool.clone(), false);
    let (_, a) = page(&on, run, &email).await;
    let (_, b) = page(&off, run, &email).await;
    let m = markup(&a);
    assert!(!m.contains(BANNER));
    assert!(!m.contains("dr-fold"));
    assert!(!m.contains(".dr-banner"), "no styles either");
    assert!(strip_clock(&m) == strip_clock(&markup(&b)), "the same page");
    // A draft with nothing to draw yet is no draft.
    assert!(
        drafts::upsert(&pool, run, Part::Answer, "{\"appl")
            .await
            .unwrap()
    );
    assert!(!markup(&page(&on, run, &email).await.1).contains(BANNER));
}

/// The clock and the fill are drawn from the time now.
fn strip_clock(m: &str) -> String {
    let m = strip(&strip(m, "rk-clock\">", "<"), "style=\"background", "\"");
    strip(&m, "animation-delay:-", "\"")
}

fn strip(m: &str, open: &str, close: &str) -> String {
    let mut out = String::new();
    let mut rest = m;
    while let Some(i) = rest.find(open) {
        out.push_str(&rest[..i + open.len()]);
        let after = &rest[i + open.len()..];
        rest = after.find(close).map_or("", |j| &after[j..]);
    }
    out.push_str(rest);
    out
}

/// `STREAM=off`: a draft left in the table is never shown.
#[tokio::test]
async fn stream_off_shows_no_draft() {
    let Some(pool) = connect().await else { return };
    let (uid, email) = new_user(&pool).await;
    let run = at_the_writer(&pool, uid).await;
    assert!(
        drafts::upsert(&pool, run, Part::Answer, &reply_so_far())
            .await
            .unwrap()
    );
    let st = state(pool.clone(), false);
    let (_, html) = page(&st, run, &email).await;
    assert!(!html.contains(BANNER));
    assert!(!html.contains("Randomised trials show no consistent"));
}

/// Another user sees nothing of it.
#[tokio::test]
async fn another_user_gets_not_found_and_no_text() {
    let Some(pool) = connect().await else { return };
    let (uid, _) = new_user(&pool).await;
    let (_, other) = new_user(&pool).await;
    let run = at_the_writer(&pool, uid).await;
    assert!(
        drafts::upsert(&pool, run, Part::Answer, &reply_so_far())
            .await
            .unwrap()
    );
    let st = state(pool.clone(), true);
    let (_, html) = page(&st, run, &other).await;
    assert!(!html.contains(BANNER));
    assert!(!html.contains("Randomised trials show no consistent"));
    let req = Request::builder()
        .uri(format!("/api/runs/{run}"))
        .header("x-dev-user-email", &other)
        .body(Body::empty())
        .unwrap();
    let res = router(st).oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

/// The final replaces the draft: once the answer is saved, the normal page.
#[tokio::test]
async fn the_final_replaces_the_draft() {
    let Some(pool) = connect().await else { return };
    let (uid, email) = new_user(&pool).await;
    let run = at_the_writer(&pool, uid).await;
    assert!(
        drafts::upsert(&pool, run, Part::Answer, &reply_so_far())
            .await
            .unwrap()
    );
    assert!(save_answer(&pool, run, &fixture("hernia")).await.unwrap());
    assert!(set_status(&pool, run, RunStatus::Done, None).await.unwrap());
    let st = state(pool.clone(), true);
    let m = markup(&page(&st, run, &email).await.1);
    assert!(!m.contains(BANNER));
    assert!(!m.contains("dr-coming"));
    assert!(m.contains("ap-main"), "the normal answer page");
    assert!(m.contains("Copy as text"));
}

/// Finished runs: the same page whichever way `STREAM` is set.
#[tokio::test]
async fn finished_pages_do_not_depend_on_the_switch() {
    let Some(pool) = connect().await else { return };
    let (uid, email) = new_user(&pool).await;
    let cases = [
        ("no row", None),
        ("done", Some(JobStatus::Done)),
        ("failed", Some(JobStatus::Failed)),
    ];
    for (label, status) in cases {
        let run = finished_full_run(&pool, uid).await;
        if let Some(status) = status {
            assert!(insert_pending(&pool, run, Uuid::new_v4()).await.unwrap());
            let f = Finished {
                reason: Some("x"),
                ..Finished::default()
            };
            assert!(finish(&pool, run, status, &f).await.unwrap());
        }
        let a = page(&state(pool.clone(), true), run, &email).await.1;
        let b = page(&state(pool.clone(), false), run, &email).await.1;
        assert!(markup(&a) == markup(&b), "{label}: the same page");
    }
}

/// An old run has no draft and draws as it always did.
#[tokio::test]
async fn an_old_run_renders_with_no_draft() {
    let Some(pool) = connect().await else { return };
    let (uid, email) = new_user(&pool).await;
    let run = finished_full_run(&pool, uid).await;
    let st = state(pool.clone(), true);
    let (s, html) = page(&st, run, &email).await;
    assert_eq!(s, StatusCode::OK);
    assert!(!html.contains(BANNER));
    assert!(!html.contains("\"draft\""));
}

// ---------- the Expert opinion ----------

fn opinion_so_far() -> String {
    json!({
        "at_a_glance": {"expert": "Selective <i>use</i> [1].", "original_quote": "No benefit", "today": "Lap first"},
    })
    .to_string()
        .trim_end_matches('}')
        .to_string()
        + r#","decisions":[{"choice":"Laparoscopic","reason":"Cheaper.","scenario":"Primary hernia"},{"choice":"Robot"#
}

async fn pending_opinion(pool: &PgPool, uid: i64) -> Uuid {
    let run = finished_full_run(pool, uid).await;
    assert!(insert_pending(pool, run, Uuid::new_v4()).await.unwrap());
    run
}

#[tokio::test]
async fn the_opinions_draft_opens_from_its_pill_under_the_same_banner() {
    let Some(pool) = connect().await else { return };
    let (uid, email) = new_user(&pool).await;
    let run = pending_opinion(&pool, uid).await;
    assert!(
        drafts::upsert(&pool, run, Part::Opinion, &opinion_so_far())
            .await
            .unwrap()
    );
    let st = state(pool.clone(), true);
    let m = markup(&page(&st, run, &email).await.1);
    // The pill: still filling, and now a link to the view.
    let tab = at(&m, r#"id="rx-tab-expert""#);
    let start = m[..tab].rfind('<').unwrap();
    let end = tab + at(&m[tab..], ">");
    let tag = &m[start..end];
    assert!(tag.starts_with("<a"), "{tag}");
    assert!(tag.contains(r##"href="#expert""##), "{tag}");
    assert!(tag.contains(r#"data-state="filling""#), "{tag}");
    // The view: the same pattern, in the Expert view's order, escaped.
    let view = at(&m, r#"id="expert""#);
    let v = &m[view..];
    assert!(v.contains(BANNER));
    let a = at(v, "Expert view on why");
    let b = at(v, "At a glance");
    let c = at(v, "Decision by scenario");
    assert!(a < b && b < c);
    assert!(v.contains(r#"<div class="dr-coming">Expert opinion"#));
    assert!(v.contains("Selective &lt;i&gt;use&lt;/i&gt;"));
    assert!(v.contains("[1]"));
    assert!(v.contains(r#"dr-fade"#), "the unfinished decision");
    // The answer stays on the page, as the view it was.
    assert!(m.contains("ap-main"));
    assert!(!m[..view].contains(BANNER), "no banner over the answer");
}

/// Pending with nothing yet, or finished: no banner and the pill as before.
#[tokio::test]
async fn the_opinion_pill_is_as_before_without_a_draft() {
    let Some(pool) = connect().await else { return };
    let (uid, email) = new_user(&pool).await;
    let run = pending_opinion(&pool, uid).await;
    let on = state(pool.clone(), true);
    let m = markup(&page(&on, run, &email).await.1);
    assert!(!m.contains(BANNER));
    let tab = at(&m, r#"id="rx-tab-expert""#);
    let start = m[..tab].rfind('<').unwrap();
    assert!(m[start..tab].starts_with("<button"));
    // Failed with its draft left behind: shown nowhere.
    assert!(
        drafts::upsert(&pool, run, Part::Opinion, &opinion_so_far())
            .await
            .unwrap()
    );
    let f = Finished {
        reason: Some("api error"),
        ..Finished::default()
    };
    assert!(finish(&pool, run, JobStatus::Failed, &f).await.unwrap());
    let m = markup(&page(&on, run, &email).await.1);
    assert!(!m.contains(BANNER));
    assert!(!m.contains("Selective"));
}
