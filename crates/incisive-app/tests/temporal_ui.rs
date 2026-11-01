//! Temporal T4 with the database (`TEST_DATABASE_URL`): the run page's
//! "Answer | Expert opinion" option, the `/run/:id/temporal` board page and
//! their scoping, rendered by the real router (SSR).
//!
//! The run page with no temporal row is compared with the markup captured
//! on main d2a4c16 before T4 (`tests/fixtures/temporal/ui/before_t4.json`):
//! the length and FNV-1a fingerprint of the page's markup with its scripts
//! taken out, the run id and today's header date replaced by fixed tokens.

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

/// The page's markup: scripts out, Leptos's `<!>` markers out, the run id
/// and today's header date replaced by fixed tokens, nonces blanked and each
/// streamed chunk back in its slot.
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
    let date = incisive_app::library::header_date(incisive_app::library::now_utc());
    // The per-request nonce of the hydration links.
    let nonce = regex::Regex::new(r#"(crossorigin|nonce)="[^"]*""#).unwrap();
    let m = nonce.replace_all(&m, "$1=\"NONCE\"").into_owned();
    let mut m = m
        .replace("<!>", "")
        .replace(&run.to_string(), "RUN-ID")
        .replace(&date, "HEADER-DATE");
    // Out-of-order streaming: a Suspense that resolves after the shell is
    // flushed sends its content as `<template id="N-f">` after `</html>`,
    // and its slot (`<!--s-N-o-->` … `<!--s-N-c-->`) holds the fallback;
    // one that resolves in time is drawn in its slot. Each template is put
    // back into its slot and the slot markers dropped (an inline draw has
    // none), so both give the same markup.
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

/// With `T4_DUMP=<dir>`, write `m` there (debugging only).
fn dump(name: &str, m: &str) {
    if let Ok(d) = std::env::var("T4_DUMP") {
        std::fs::write(format!("{d}/{name}.html"), m).unwrap();
    }
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

const BEFORE: &str = "tests/fixtures/temporal/ui/before_t4.json";

/// The run page of each fixture's Full run with no temporal row (`TEMPORAL`
/// off, an older run): its markup's length and fingerprint. With `T4_CAPTURE=1` the markup is written
/// to [`BEFORE`] (done once, on main d2a4c16, before any T4 change).
async fn no_row_pages(pool: &PgPool) -> serde_json::Map<String, Value> {
    let st = state(pool.clone());
    let (uid, email) = new_user(pool).await;
    let mut out = serde_json::Map::new();
    for name in ["hernia", "achalasia", "cholecystitis"] {
        let run = full_run(pool, uid, name).await;
        let (s, html) = send(router(st.clone()), &format!("/runs/{run}"), &email).await;
        assert_eq!(s, StatusCode::OK, "{name}");
        let m = markup(&html, run);
        dump(&format!("noRow-{name}"), &m);
        out.insert(
            name.to_string(),
            serde_json::json!({"len": m.len(), "fnv1a": fnv1a(&m)}),
        );
    }
    out
}

/// T4: the run page with no temporal row is byte-identical to the page
/// before T4 (no option, no style, no Expert Opinion markup).
#[tokio::test]
async fn t4_run_page_without_a_temporal_row_is_byte_identical() {
    let Some(pool) = connect().await else { return };
    let now = no_row_pages(&pool).await;
    let path = format!("{}/{BEFORE}", env!("CARGO_MANIFEST_DIR"));
    if std::env::var("T4_CAPTURE").is_ok_and(|v| v == "1") {
        std::fs::create_dir_all(std::path::Path::new(&path).parent().unwrap()).unwrap();
        std::fs::write(&path, serde_json::to_string_pretty(&now).unwrap()).unwrap();
        eprintln!("T4 captured {path}");
        return;
    }
    let before: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(before.as_object().unwrap().len(), 3);
    for (name, b) in before.as_object().unwrap() {
        assert_eq!(&now[name], b, "{name}: the run page changed");
    }
}

// ---------- with a temporal row ----------

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

/// Where the element whose `class` attribute is `class` starts (Leptos
/// writes `class` after the other attributes).
fn element_at(m: &str, class: &str) -> Option<usize> {
    let c = m.find(&format!(r#"class="{class}""#))?;
    m[..c].rfind('<')
}

/// The option bar's markup (the `rx-seg` `<div>` … `</div>`).
fn option_markup(m: &str) -> &str {
    let i = element_at(m, "rx-seg").expect("the option bar");
    let j = i + m[i..].find("</div>").unwrap();
    &m[i..j]
}

/// The run page's answer content: `<div class="ap">` to the end of
/// `<main>` (the answer is the page's last part).
fn answer_content(m: &str) -> &str {
    let i = m.find(r#"<div class="ap">"#).expect("the answer page");
    let j = i + m[i..].find("</main>").expect("the end of main");
    &m[i..j]
}

/// T4: with a row, each status renders the right option; only a ready
/// opinion is selectable and has the dot, the hint and the view.
#[tokio::test]
async fn t4_each_status_renders_its_option() {
    let Some(pool) = connect().await else { return };
    let st = state(pool.clone());
    let (uid, email) = new_user(&pool).await;
    let cases = [
        (None, "Expert opinion \u{b7} being written", false),
        (Some(JobStatus::Done), "Expert opinion", true),
        (
            Some(JobStatus::Skipped),
            "Expert opinion \u{b7} not enough studies",
            false,
        ),
        (
            Some(JobStatus::Failed),
            "Expert opinion \u{b7} unavailable",
            false,
        ),
        (
            Some(JobStatus::CapBlocked),
            "Expert opinion \u{b7} not run (daily limit)",
            false,
        ),
    ];
    for (status, text, ready) in cases {
        let run = full_run(&pool, uid, "hernia").await;
        temporal_row(&pool, run, "hernia", status, None).await;
        let (s, html) = send(router(st.clone()), &format!("/runs/{run}"), &email).await;
        assert_eq!(s, StatusCode::OK, "{text}");
        let m = markup(&html, run);
        dump(
            &format!("status-{}", status.map_or("pending", |s| s.as_str())),
            &m,
        );
        let bar = option_markup(&m);
        assert!(bar.contains(">Answer</a>"), "{bar}");
        assert!(bar.contains(&format!("{text}</")), "{text}: {bar}");
        assert_eq!(bar.contains("rx-dot"), ready, "{text}");
        // Selectable: a link to #expert; otherwise a disabled button.
        assert_eq!(bar.contains(r##"href="#expert""##), ready, "{text}");
        assert_eq!(bar.contains("disabled"), !ready, "{text}");
        // The Answer view is shown first (the server never sees the hash).
        assert!(bar.contains(r#"class="rx-tab on""#) || bar.contains(r#"class="on rx-tab""#));
        assert_eq!(
            m.contains("Expert opinion ready: how this evidence moved over time"),
            ready,
            "{text}"
        );
        // The option sits outside the answer content, above it.
        let ap = m.find(r#"<div class="ap-main">"#).unwrap();
        assert!(element_at(&m, "rx-seg").unwrap() < ap);
        assert!(!answer_content(&m).contains("rx-seg"));
        // The view: drawn (hidden) only when ready, in the mockup's order.
        if ready {
            let v = &m[m.find(r#"id="expert""#).unwrap()..];
            let at = |needle: &str| {
                v.find(needle)
                    .unwrap_or_else(|| panic!("missing {needle:?}"))
            };
            assert!(at("Expert opinion \u{b7} evidence over time") < at("My view on why"));
            assert!(at("My view on why") < at("At a glance"));
            assert!(at("At a glance") < at(">Original<"));
            assert!(at(">Original<") < at(">Expert<"));
            assert!(at(">Expert<") < at(">Today<"));
            assert!(at(">Today<") < at("Decision by scenario"));
            assert!(at("Decision by scenario") < at("Open the Temporal board"));
            assert!(at("Open the Temporal board") < at("Ask this evidence"));
            assert!(v.contains(&format!(r#"href="/run/{}/temporal""#, "RUN-ID")));
            let ask = &v[at("Ask this evidence") - 200..at("Ask this evidence")];
            assert!(ask.contains("disabled"), "{ask}");
            assert!(v.contains(">Later<"));
            // Citations link to the study sheets on the page.
            assert!(
                v.contains(r##"<a href="#ref-"##)
                    && v.contains(r#"rel="external" class="rx-cite">"#)
            );
            // Hidden until chosen: the view and the answer wrapper.
            let sec = &m[m[..m.find(r#"id="expert""#).unwrap()]
                .rfind("<section")
                .unwrap()..];
            assert!(sec[..sec.find('>').unwrap()].contains("hidden"));
        } else {
            // The view's section is there (it fills in when the opinion is
            // ready) but empty, so `#expert` opens nothing.
            let i = m.find(r#"id="expert""#).unwrap();
            let sec = &m[i..];
            let body = &sec[sec.find('>').unwrap() + 1..sec.find("</section>").unwrap()];
            assert!(body.replace("<!---->", "").is_empty(), "{text}: {body}");
            assert!(!m.contains("My view on why"), "{text}");
        }
    }
}

/// T4: with a row, the answer content is byte-identical to the page with
/// no row (the option, the view and the wrapper sit outside it).
#[tokio::test]
async fn t4_answer_content_is_unchanged_with_a_row() {
    let Some(pool) = connect().await else { return };
    let st = state(pool.clone());
    let (uid, email) = new_user(&pool).await;
    for name in ["hernia", "achalasia", "cholecystitis"] {
        let plain = full_run(&pool, uid, name).await;
        let with = full_run(&pool, uid, name).await;
        temporal_row(&pool, with, name, Some(JobStatus::Done), None).await;
        let (_, a) = send(router(st.clone()), &format!("/runs/{plain}"), &email).await;
        let (_, b) = send(router(st.clone()), &format!("/runs/{with}"), &email).await;
        let (a, b) = (markup(&a, plain), markup(&b, with));
        dump(&format!("plain-{name}"), &a);
        dump(&format!("with-{name}"), &b);
        let (ca, cb) = (answer_content(&a), answer_content(&b));
        // With a row the answer wrapper and the page wrapper close after it.
        assert_eq!(cb.strip_suffix("</div></div>"), Some(ca), "{name}");
        assert!(b.contains(r#"<div class="rx-answer">"#));
        assert!(!a.contains("rx-answer") && !a.contains("rx-seg") && !a.contains(".rt-panel"));
    }
}

/// T4: the board page renders all seven steps for the three fixtures, with
/// the Rust-drawn SVG, the ledger labels, the checks folded at the bottom
/// and the citations linking to the run page's study sheets.
#[tokio::test]
async fn t4_board_page_renders_all_seven_steps() {
    let Some(pool) = connect().await else { return };
    let st = state(pool.clone());
    let (uid, email) = new_user(&pool).await;
    for name in ["hernia", "achalasia", "cholecystitis"] {
        let run = full_run(&pool, uid, name).await;
        temporal_row(&pool, run, name, Some(JobStatus::Done), None).await;
        let (s, html) = send(router(st.clone()), &format!("/run/{run}/temporal"), &email).await;
        assert_eq!(s, StatusCode::OK, "{name}");
        let m = markup(&html, run);
        dump(&format!("board-{name}"), &m);
        // The shared header and the back link.
        assert!(m.contains(r#"<header class="iv-head">"#));
        assert!(m.contains(r#"href="/runs/RUN-ID#expert""#));
        assert!(m.contains("Back to the run"));
        assert_eq!(m.matches("<h1").count(), 1, "{name}");
        let at = |needle: &str| {
            m.find(needle)
                .unwrap_or_else(|| panic!("{name}: missing {needle:?}"))
        };
        let steps = [
            "When the patients were treated",
            "What each era showed, and what it taught",
            "Linked progression",
            "Latest-period researcher and skeptic",
            "Hypothesis scoreboard after the debate",
            "Temporal knowledge ledger",
            "Expert opinion</h2>",
            "Expert decision: what should we conclude today?",
            "What the next five years could show, and what it would change",
            "What would settle it",
            "<summary>Checks",
        ];
        for w in steps.windows(2) {
            assert!(at(w[0]) < at(w[1]), "{name}: {} before {}", w[0], w[1]);
        }
        for n in 1..=7 {
            assert!(m.contains(&format!("<b>Step {n}</b>")), "{name}: step {n}");
        }
        // The charts, drawn as SVG: the timeline and one per linked panel.
        let t: incisive_app::temporal::TemporalTable =
            serde_json::from_value(table_of(name)).unwrap();
        let panels = incisive_app::expert_page::linked_panels(&t).len();
        assert!(panels >= 1, "{name}");
        assert_eq!(m.matches("<svg viewBox=").count(), 1 + panels, "{name}");
        assert!(m.contains("class=\"rt-bar"));
        assert!(m.contains("no difference</text>"));
        // Era cards: one per era of the table.
        assert_eq!(m.matches("<article class=\"rt-era").count(), t.eras.len());
        // Debate: researcher left, skeptic right.
        assert!(m.contains(r#"<div class="rt-msg r">"#));
        assert!(m.contains(r#"<div class="rt-msg s">"#));
        assert!(m.contains("Round 1 \u{b7} "));
        // Checks folded (closed), listing the stored flags.
        let checks = &m[at("<details class=\"rt-checks\">")..];
        assert!(checks.contains("Flag, never delete"));
        // No cost outcome is drawn from the table.
        assert!(!m.contains(">Cost</span>"), "{name}");
        // Citations: links to the run page's sheets.
        assert!(m.contains(r#"<a href="/runs/RUN-ID#ref-"#));
        assert!(m.contains(r#"" rel="external" class="rt-cite">"#));
        // The run page's answer is not on this page.
        assert!(!m.contains(r#"<div class="ap-main">"#));
    }
    // Ledger labels: hernia treatment eras, achalasia "known by".
    let run = full_run(&pool, uid, "achalasia").await;
    temporal_row(&pool, run, "achalasia", Some(JobStatus::Done), None).await;
    let (_, html) = send(router(st.clone()), &format!("/run/{run}/temporal"), &email).await;
    let html = markup(&html, run);
    assert!(html.contains("<th>Known now, 2026</th>"));
    assert!(html.contains("<th>Known by 2010</th>"));
    assert!(html.contains("Columns are the years results became known."));
    assert!(html.contains("No studies in this era."));
    let run = full_run(&pool, uid, "hernia").await;
    temporal_row(&pool, run, "hernia", Some(JobStatus::Done), None).await;
    let (_, html) = send(router(st.clone()), &format!("/run/{run}/temporal"), &email).await;
    let html = markup(&html, run);
    assert!(html.contains("<th>Now, after 2021\u{2013}26</th>"));
    // Hernia's flags: derived 33.2 twice, one glance word, listed as stored.
    assert!(html.contains("Derived (2)"));
    assert!(html.contains("At a glance stronger than the opinion (1)"));
    assert!(html.contains("<code>at_a_glance.expert</code>: fewer"));
    assert!(html.contains(">split</text>"));
}

/// T4: board text is escaped: a `<script>` in the board renders inert on
/// the board page and in the run page's Expert Opinion view.
#[tokio::test]
async fn t4_board_text_is_escaped() {
    let Some(pool) = connect().await else { return };
    let st = state(pool.clone());
    let (uid, email) = new_user(&pool).await;
    let evil = "<script>alert(1)</script><img src=x onerror=alert(2)>";
    let mut b = board_fixture("hernia")["board"].clone();
    b["opinion"]["verdict"] = Value::String(format!("Verdict {evil} [1]."));
    b["opinion"]["why"] = Value::String(format!("Why {evil}"));
    b["debate"][0]["points"][0] = Value::String(format!("Point {evil}"));
    b["ledger"]["rows"][0]["cells"][0] = Value::String(evil.into());
    b["topic"] = Value::String(format!("Topic {evil}"));
    b["eras"][0]["era_id"] = Value::String(format!("e-{evil}"));
    let run = full_run(&pool, uid, "hernia").await;
    temporal_row(&pool, run, "hernia", Some(JobStatus::Done), Some(b)).await;
    for path in [format!("/run/{run}/temporal"), format!("/runs/{run}")] {
        let (s, html) = send(router(st.clone()), &path, &email).await;
        assert_eq!(s, StatusCode::OK, "{path}");
        let m = markup(&html, run);
        assert!(!m.contains("<script>alert(1)"), "{path}");
        assert!(!m.contains("<img src=x"), "{path}");
        assert!(
            m.contains("&lt;script&gt;alert(1)&lt;/script&gt;"),
            "{path}"
        );
    }
    // The citation after the escaped text still links (board page).
    let (_, html) = send(router(st.clone()), &format!("/run/{run}/temporal"), &email).await;
    assert!(html.contains(&format!(
        r#"&lt;img src=x onerror=alert(2)&gt; [<a href="/runs/{run}#ref-1" rel="external" class="rt-cite">1</a>]."#
    )));
    // The unknown era note is kept and marked.
    assert!(html.contains("Not an era of the table"));
}

/// T4: the board page for another user's run is "not found", as the run
/// page is; so is the server function. The owner gets the board.
#[tokio::test]
async fn t4_board_page_of_another_users_run_is_not_found() {
    let Some(pool) = connect().await else { return };
    let st = state(pool.clone());
    let (owner, owner_email) = new_user(&pool).await;
    let (_, other_email) = new_user(&pool).await;
    let run = full_run(&pool, owner, "cholecystitis").await;
    temporal_row(&pool, run, "cholecystitis", Some(JobStatus::Done), None).await;
    let topic = board_fixture("cholecystitis")["board"]["topic"]
        .as_str()
        .unwrap()
        .to_string();

    let (s, html) = send(
        router(st.clone()),
        &format!("/run/{run}/temporal"),
        &other_email,
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let not_found = incisive_app::runs::RUN_NOT_FOUND_MESSAGE.replace('\'', "&#x27;");
    assert!(
        html.contains(&not_found) || html.contains(incisive_app::runs::RUN_NOT_FOUND_MESSAGE),
        "{html}"
    );
    assert!(!html.contains(&topic));
    assert!(!html.contains("Step 1"));
    // The server function: 404.
    let req = Request::post("/api/fn/my_run_temporal")
        .header("content-type", "application/x-www-form-urlencoded")
        .header("x-dev-user-email", &other_email)
        .body(Body::from(format!("run_id={run}")))
        .unwrap();
    let res = router(st.clone()).oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
    // A malformed id: not found.
    let (_, html) = send(router(st.clone()), "/run/not-a-uuid/temporal", &owner_email).await;
    assert!(html.contains(&not_found) || html.contains(incisive_app::runs::RUN_NOT_FOUND_MESSAGE));
    // The owner sees it.
    let (s, html) = send(
        router(st.clone()),
        &format!("/run/{run}/temporal"),
        &owner_email,
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert!(html.contains(&topic.replace('\'', "&#x27;")) || html.contains(&topic));
    // A run of the owner's with no row, and one not ready: said plainly.
    let plain = full_run(&pool, owner, "hernia").await;
    let (_, html) = send(
        router(st.clone()),
        &format!("/run/{plain}/temporal"),
        &owner_email,
    )
    .await;
    assert!(html.contains("This question has no expert opinion."));
    let pending = full_run(&pool, owner, "hernia").await;
    temporal_row(&pool, pending, "hernia", None, None).await;
    let (_, html) = send(
        router(st.clone()),
        &format!("/run/{pending}/temporal"),
        &owner_email,
    )
    .await;
    assert!(html.contains("The expert opinion is being written."));
    assert!(!html.contains("Step 1"));
}

// ---------- the owner's static preview ----------

/// A rendered page made static for the preview: no hydration links, the
/// fonts and the links between the preview's pages made relative.
fn static_page(m: &str, name: &str) -> String {
    let links = regex::Regex::new(r#"<link rel="(?:modulepreload|preload)"[^>]*>"#).unwrap();
    links
        .replace_all(m, "")
        .replace("url(/fonts/", "url(fonts/")
        .replace(
            r#"href="/run/RUN-ID/temporal""#,
            &format!(r#"href="board-{name}.html""#),
        )
        .replace(
            r#"href="/runs/RUN-ID#"#,
            &format!(r#"href="run-{name}-done.html#"#),
        )
}

/// With `T4_PREVIEW=1`: write the owner's static preview to
/// `.superpowers/temporal/t4-preview/` (rendered by SSR from the fixtures):
/// the three board pages, the run page in each option state (hernia) and
/// ready for the other two fixtures, an option-states sheet and an index.
#[tokio::test]
async fn t4_write_the_static_preview() {
    if !std::env::var("T4_PREVIEW").is_ok_and(|v| v == "1") {
        return;
    }
    let Some(pool) = connect().await else { return };
    let st = state(pool.clone());
    let (uid, email) = new_user(&pool).await;
    let root = format!("{}/../..", env!("CARGO_MANIFEST_DIR"));
    let out = format!("{root}/.superpowers/temporal/t4-preview");
    let _ = std::fs::remove_dir_all(&out);
    std::fs::create_dir_all(format!("{out}/fonts/figtree")).unwrap();
    let fonts = format!("{}/public/fonts/figtree", env!("CARGO_MANIFEST_DIR"));
    for f in std::fs::read_dir(&fonts).unwrap() {
        let f = f.unwrap();
        std::fs::copy(
            f.path(),
            format!("{out}/fonts/figtree/{}", f.file_name().to_string_lossy()),
        )
        .unwrap();
    }
    let write = |file: &str, html: &str| std::fs::write(format!("{out}/{file}"), html).unwrap();
    let mut bars: Vec<(String, String)> = Vec::new();
    for name in ["hernia", "achalasia", "cholecystitis"] {
        let run = full_run(&pool, uid, name).await;
        temporal_row(&pool, run, name, Some(JobStatus::Done), None).await;
        let (_, html) = send(router(st.clone()), &format!("/run/{run}/temporal"), &email).await;
        write(
            &format!("board-{name}.html"),
            &static_page(&markup(&html, run), name),
        );
        let (_, html) = send(router(st.clone()), &format!("/runs/{run}"), &email).await;
        write(
            &format!("run-{name}-done.html"),
            &static_page(&markup(&html, run), name),
        );
    }
    let states = [
        ("pending", None),
        ("done", Some(JobStatus::Done)),
        ("skipped", Some(JobStatus::Skipped)),
        ("failed", Some(JobStatus::Failed)),
        ("cap_blocked", Some(JobStatus::CapBlocked)),
    ];
    for (label, status) in states {
        let run = full_run(&pool, uid, "hernia").await;
        temporal_row(&pool, run, "hernia", status, None).await;
        let (_, html) = send(router(st.clone()), &format!("/runs/{run}"), &email).await;
        let m = static_page(&markup(&html, run), "hernia");
        if label != "done" {
            write(&format!("run-hernia-{label}.html"), &m);
        }
        // The option bar and the hint under it, for the states sheet.
        let i = element_at(&m, "rx-seg").unwrap();
        let end = m[i..]
            .find("<h3>Progress")
            .or_else(|| m[i..].find("<details class=\"run-fold\""))
            .map_or(i + m[i..].find("</div>").unwrap() + 6, |j| i + j);
        bars.push((label.to_string(), m[i..end].to_string()));
    }
    let plain = full_run(&pool, uid, "hernia").await;
    let (_, html) = send(router(st.clone()), &format!("/runs/{plain}"), &email).await;
    write(
        "run-hernia-no-row.html",
        &static_page(&markup(&html, plain), "hernia"),
    );

    let styles = format!(
        "<style>{}</style><style>{}</style>",
        incisive_app::app::IVORY_STYLE.replace("url(/fonts/", "url(fonts/"),
        incisive_app::app::EXPERT_STYLE
    );
    let sheet: String = bars
        .iter()
        .map(|(l, b)| {
            format!(r#"<section class="pv-state"><h2>{l}</h2><div class="rx">{b}</div></section>"#)
        })
        .collect();
    write(
        "option-states.html",
        &format!(
            r#"<!DOCTYPE html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1"><title>Option states</title>{styles}<style>body{{margin:0;padding:20px 16px 48px;font-family:var(--iv-font);color:var(--iv-ink)}} .pv-grid{{display:grid;grid-template-columns:repeat(auto-fit,minmax(min(100%,360px),1fr));gap:18px;max-width:1180px;margin:0 auto}} .pv-state h2{{font-size:.74rem;letter-spacing:.07em;text-transform:uppercase;color:var(--iv-muted);margin:0 0 8px}} h1{{max-width:1180px;margin:0 auto 16px;font-size:1.3rem}}</style></head><body><h1>Run page: the "Answer | Expert opinion" option, by status (hernia, SSR)</h1><div class="pv-grid">{sheet}</div></body></html>"#
        ),
    );
    write(
        "index.html",
        &format!(
            r#"<!DOCTYPE html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1"><title>T4 preview</title>{styles}<style>body{{margin:0;padding:24px 16px 48px;font-family:var(--iv-font);color:var(--iv-ink);line-height:1.5}} main{{max-width:760px;margin:0 auto;display:flex;flex-direction:column;gap:14px}} ul{{margin:0;padding-left:1.1rem}} li{{margin:6px 0}} a{{color:var(--iv-accent)}} p{{margin:0;color:var(--iv-body)}}</style></head><body><main>
<h1>Incisive T4 preview: the Expert Opinion screens</h1>
<p>Rendered by the server (SSR) from the three fixture runs and the owner's smoke boards; scripts are taken out, so each page shows what a phone shows before the app takes over. Open a run page's "Expert opinion" option (or add <code>#expert</code>) to see the Expert Opinion view. Narrow the window to phone width to check the layout.</p>
<h2>Temporal board pages (/run/:id/temporal)</h2>
<ul><li><a href="board-cholecystitis.html">Cholecystitis</a></li><li><a href="board-hernia.html">Hernia</a> (treatment-era ledger, a split era, flags)</li><li><a href="board-achalasia.html">Achalasia</a> ("known by" ledger, an empty era, follow-up reports)</li></ul>
<h2>Run page</h2>
<ul><li><a href="option-states.html">The option in all five states, side by side</a></li>
<li>Ready: <a href="run-hernia-done.html">hernia</a> (<a href="run-hernia-done.html#expert">Expert opinion view</a>), <a href="run-achalasia-done.html#expert">achalasia</a>, <a href="run-cholecystitis-done.html#expert">cholecystitis</a></li>
<li><a href="run-hernia-pending.html">Being written</a> · <a href="run-hernia-skipped.html">Not enough studies</a> · <a href="run-hernia-failed.html">Unavailable</a> · <a href="run-hernia-cap_blocked.html">Not run (daily limit)</a></li>
<li><a href="run-hernia-no-row.html">No expert opinion row</a> (the page exactly as before T4)</li></ul>
</main></body></html>"#
        ),
    );
    eprintln!("T4 preview written to {out}");
}
