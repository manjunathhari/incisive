//! The Explore page (`/explore`), its nav entry and its server functions,
//! rendered by the real router (SSR) with the database
//! (`TEST_DATABASE_URL`).

use std::sync::Arc;

use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode},
};
use http_body_util::BodyExt;
use incisive_app::db::{lazy_pool, migrate};
use incisive_app::explore_page as ep;
use incisive_app::gate::OfflineClassifier;
use incisive_app::graph::extract::{PickedOutcome, Picks};
use incisive_app::graph::rank::rank_paper;
use incisive_app::graph::settings::{GraphRuntime, GraphSettings};
use incisive_app::graph::store::{PaperRow, write_run};
use incisive_app::graph_view::{DotKind, MapData};
use incisive_app::identity::{IapVerifier, JwkSource};
use incisive_app::runs::RunsConfig;
use incisive_app::runs::worker::WorkerHandle;
use incisive_app::server::AppState;
use incisive_app::spend::{CapsConfig, Prices};
use leptos::config::LeptosOptions;
use secrecy::SecretString;
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

const MODEL: &str = "m-test";

/// The tests share the concept table, so they run one at a time.
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

fn state(pool: PgPool, graph_on: bool) -> AppState {
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
        stream: false,
        intro: Default::default(),
        graph: GraphRuntime {
            settings: GraphSettings {
                on: graph_on,
                ..Default::default()
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

async fn body_of(res: axum::response::Response) -> (StatusCode, String) {
    let status = res.status();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

/// A page, signed in as `email` (or signed out).
async fn get(app: Router, path: &str, email: Option<&str>) -> (StatusCode, String) {
    let mut req = Request::builder().method("GET").uri(path);
    if let Some(e) = email {
        req = req.header("x-dev-user-email", e);
    }
    body_of(app.oneshot(req.body(Body::empty()).unwrap()).await.unwrap()).await
}

/// A server function, called with a JSON body.
async fn call(
    app: Router,
    endpoint: &str,
    json: &str,
    email: Option<&str>,
) -> (StatusCode, String) {
    let mut req = Request::builder()
        .method("POST")
        .uri(format!("/api/fn/{endpoint}"))
        .header("content-type", "application/json")
        .header("accept", "application/json");
    if let Some(e) = email {
        req = req.header("x-dev-user-email", e);
    }
    body_of(
        app.oneshot(req.body(Body::from(json.to_string())).unwrap())
            .await
            .unwrap(),
    )
    .await
}

async fn new_user(pool: &PgPool, role: &str) -> (i64, String) {
    let email = format!("explore-{}@example.com", Uuid::new_v4());
    let uid = sqlx::query_scalar(
        "INSERT INTO incisive.users (email, display_name, role, daily_cap_paise) \
         VALUES ($1, 'Manjunath Haridas', $2, 100000) RETURNING id",
    )
    .bind(&email)
    .bind(role)
    .fetch_one(pool)
    .await
    .unwrap();
    (uid, email)
}

async fn seed_run(pool: &PgPool, user: i64) -> Uuid {
    let run = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO incisive.runs (id, user_id, question, mode, status, limit_paise, created_at) \
         VALUES ($1, $2, 'q', 'focused', 'done', 1000, '2026-10-31 06:00:00+00')",
    )
    .bind(run)
    .bind(user)
    .execute(pool)
    .await
    .unwrap();
    run
}

fn picks(tag: &str) -> Picks {
    Picks {
        diseases: vec![format!("Inguinal hernia {tag}")],
        treatments: vec![format!("Robotic surgery {tag}")],
        organs: vec![format!("Abdominal wall {tag}")],
        outcomes: vec![PickedOutcome {
            row: 0,
            name: format!("Recurrence {tag}"),
            certainty: "Low".into(),
        }],
        dropped: 0,
    }
}

fn paper(pmid: &str, title: &str, year: i32, design: &str) -> PaperRow {
    PaperRow {
        pmid: pmid.into(),
        title: title.into(),
        journal: None,
        year: Some(year),
        doi: None,
        retracted: false,
        rank: rank_paper(design, &[]),
        design: design.into(),
        n: None,
    }
}

/// PMIDs no other test uses.
fn pmid(tag: &str, i: u32) -> String {
    format!("7{}{i:04}", &tag[..7])
}

fn tag() -> String {
    Uuid::new_v4().simple().to_string()
}

/// A run with one disease, organ, treatment and outcome and these papers.
async fn mapped_run(pool: &PgPool, user: i64, tag: &str, papers: &[PaperRow]) -> Uuid {
    let run = seed_run(pool, user).await;
    write_run(pool, run, &picks(tag), papers).await.unwrap();
    run
}

const NOT_ALL: &str = "Every dot comes from your own runs, so this is not all published evidence.";
const LINE_MEANS: &str = r#"A line means "looked at in this run", not "this treatment works"."#;

/// The page outside its scripts (the hydration payload holds every row,
/// folded or not).
fn markup(html: &str) -> String {
    let mut m = String::new();
    let mut rest = html;
    while let Some(i) = rest.find("<script") {
        m.push_str(&rest[..i]);
        rest = rest[i..]
            .find("</script>")
            .map_or("", |j| &rest[i + j + "</script>".len()..]);
    }
    m.push_str(rest);
    m
}

/// The page outside its scripts with streamed chunks put back in their
/// slots, nonces and today's date blanked.
fn settled(html: &str) -> String {
    let date = incisive_app::library::header_date(incisive_app::library::now_utc());
    let nonce = regex::Regex::new(r#"(crossorigin|nonce)="[^"]*""#).unwrap();
    let mut m = nonce
        .replace_all(&markup(html), "$1=\"NONCE\"")
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
    regex::Regex::new(r"<!--s-\d+-[oc]-->")
        .unwrap()
        .replace_all(&m, "")
        .into_owned()
}

/// The `<header>` of a page.
fn header_of(html: &str) -> String {
    let m = settled(html);
    let a = m.find(r#"<header class="iv-head">"#).expect("header");
    let b = a + m[a..].find("</header>").expect("header end") + "</header>".len();
    m[a..b].to_string()
}

const OWNER_HEADER: &str = "tests/fixtures/explore/owner_header_before.json";

/// The first HTML a browser draws with no script: outside scripts and
/// outside any streamed `<template>`.
fn first_html(html: &str) -> String {
    let mut m = String::new();
    let mut rest = markup(html);
    while let Some(i) = rest.find("<template") {
        m.push_str(&rest[..i]);
        rest = rest[i..].find("</template>").map_or(String::new(), |j| {
            rest[i + j + "</template>".len()..].to_string()
        });
    }
    m.push_str(&rest);
    m
}

/// The whole opening tag that holds `needle`.
fn tag_with(html: &str, needle: &str) -> String {
    let at = html.find(needle).unwrap_or_else(|| panic!("no {needle}"));
    let start = html[..at].rfind('<').unwrap();
    let end = at + html[at..].find('>').unwrap();
    html[start..=end].to_string()
}

/// Text as the page writes it (quotes may be escaped in text).
fn unescaped(html: &str) -> String {
    html.replace("&quot;", "\"").replace("&#34;", "\"")
}

#[tokio::test]
async fn with_graph_off_there_is_no_explore_link_and_no_page() {
    let _g = SERIAL.lock().await;
    let Some(pool) = connect().await else { return };
    let (uid, email) = new_user(&pool, "owner").await;
    let t = tag();
    mapped_run(
        &pool,
        uid,
        &t,
        &[paper(&pmid(&t, 1), "A paper", 2020, "RCT")],
    )
    .await;
    let st = state(pool.clone(), false);

    let (s, home) = get(router(st.clone()), "/", Some(&email)).await;
    assert_eq!(s, StatusCode::OK);
    assert!(!home.contains("/explore"), "nav has Explore with GRAPH=off");
    assert!(
        home.contains(r#"href="/activity""#),
        "the owner still sees Activity"
    );
    // The Ask home keeps the CI smoke test's strings.
    assert!(home.contains("searched, checked, summarised"));
    assert!(home.contains(r#"<textarea id="question""#));

    let (s, page) = get(router(st.clone()), "/explore", Some(&email)).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    let m = markup(&page);
    assert!(m.contains("<p>Page not found. Go back to the question box.</p>"));
    assert!(!page.contains(&format!("Inguinal hernia {t}")));
    // Nothing of the Explore page: no styles, no heading, no error box.
    for absent in [
        ".kg-switch",
        "<h1",
        "iv-wrap kg",
        "role=\"alert\"",
        "Loading",
    ] {
        assert!(!m.contains(absent), "{absent} with GRAPH=off");
    }
    // The width of any unknown path, not Explore's wide layout.
    let (s, unknown) = get(router(st.clone()), "/nowhere", Some(&email)).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    let main_tag = |html: &str| {
        let at = html.find("<main").expect("a <main>");
        html[at..at + html[at..].find('>').expect("its end") + 1].to_string()
    };
    assert_eq!(main_tag(&page), main_tag(&unknown));
    assert!(!main_tag(&page).contains("wide"));

    for f in ["explore_list", "explore_map"] {
        let (s, body) = call(router(st.clone()), f, "{}", Some(&email)).await;
        assert_eq!(s, StatusCode::NOT_FOUND, "{f}");
        assert!(!body.contains(&t), "{f}");
    }
}

#[tokio::test]
async fn with_graph_on_the_nav_has_explore() {
    let _g = SERIAL.lock().await;
    let Some(pool) = connect().await else { return };
    let (_, email) = new_user(&pool, "surgeon").await;
    let st = state(pool.clone(), true);
    for path in ["/", "/history", "/explore"] {
        let (s, html) = get(router(st.clone()), path, Some(&email)).await;
        assert_eq!(s, StatusCode::OK, "{path}");
        assert!(
            html.contains(r#"href="/explore" aria-label="Explore, opens your map""#),
            "{path}"
        );
        assert!(
            !html.contains(r#"href="/activity""#),
            "{path}: not the owner"
        );
        assert!(html.contains(r#"<main class="wide">"#), "{path}: wide");
    }
    let (_, html) = get(router(st.clone()), "/explore", Some(&email)).await;
    assert!(
        html.contains(
            r#"href="/explore" aria-label="Explore, opens your map" aria-current="page""#
        ),
        "Explore is the current page"
    );
    let (_, home) = get(router(st), "/", Some(&email)).await;
    assert!(home.contains("searched, checked, summarised"));
    assert!(home.contains(r#"<textarea id="question""#));
}

#[tokio::test]
async fn signed_out_gets_no_data() {
    let _g = SERIAL.lock().await;
    let Some(pool) = connect().await else { return };
    let (uid, _) = new_user(&pool, "surgeon").await;
    let t = tag();
    mapped_run(
        &pool,
        uid,
        &t,
        &[paper(&pmid(&t, 1), "A paper", 2020, "RCT")],
    )
    .await;
    let st = state(pool.clone(), true);

    let (_, page) = get(router(st.clone()), "/explore", None).await;
    assert!(!page.contains(&t), "a signed-out page shows someone's map");
    assert!(!page.contains("Papers, ranked"));
    for f in ["explore_list", "explore_map"] {
        let (s, body) = call(router(st.clone()), f, "{}", None).await;
        assert!(!s.is_success(), "{f}: {s}");
        assert!(!body.contains(&t), "{f}");
    }
}

#[tokio::test]
async fn no_runs_asks_for_a_first_question() {
    let _g = SERIAL.lock().await;
    let Some(pool) = connect().await else { return };
    let (_, email) = new_user(&pool, "surgeon").await;
    let (s, html) = get(router(state(pool, true)), "/explore", Some(&email)).await;
    assert_eq!(s, StatusCode::OK);
    assert!(html.contains("Ask your first question to start your map."));
    assert!(!html.contains(r#"aria-label="View""#), "no Map/List switch");
    assert!(!html.contains(r#"id="kg-map""#), "no map");
    assert!(!html.contains("Papers, ranked"));
}

#[tokio::test]
async fn the_list_groups_folds_and_escapes() {
    let _g = SERIAL.lock().await;
    let Some(pool) = connect().await else { return };
    let (uid, email) = new_user(&pool, "surgeon").await;
    let t = tag();
    let mut papers = vec![
        paper(&pmid(&t, 1), "<script>x</script>", 2024, "RCT"),
        paper(
            &pmid(&t, 2),
            "A mixed meta-analysis",
            2025,
            "Meta-analysis of randomised and observational studies",
        ),
        paper(
            &pmid(&t, 3),
            "A retracted cohort",
            2019,
            "Retrospective cohort",
        ),
        paper(&pmid(&t, 4), "Paper four", 2018, "Retrospective cohort"),
        paper(&pmid(&t, 5), "Paper five", 2017, "Case series"),
        paper(&pmid(&t, 6), "Paper six", 2016, "Case series"),
    ];
    papers[2].retracted = true;
    let run = mapped_run(&pool, uid, &t, &papers).await;
    let (s, html) = get(router(state(pool.clone(), true)), "/explore", Some(&email)).await;
    assert_eq!(s, StatusCode::OK);
    let text = unescaped(&html);

    // The groups, in order, with their counts.
    let at = |s: &str| html.find(s).unwrap_or_else(|| panic!("missing {s}"));
    let order = [
        "Diseases \u{b7} 1",
        "Treatments \u{b7} 1",
        "Outcomes \u{b7} 1",
        "Your questions \u{b7} 1",
        "Papers, ranked \u{b7} 6",
    ];
    for w in order.windows(2) {
        assert!(at(w[0]) < at(w[1]), "{} before {}", w[0], w[1]);
    }
    // Five papers, then the fold.
    assert!(html.contains("Show all 6"));
    assert!(html.contains("Paper five"));
    // (The map has a button for every paper; the List folds.)
    let m = markup(&html);
    let list = &m[m.find(r#"class="kg-list""#).unwrap()..m.find(r#"class="kg-side""#).unwrap()];
    assert!(list.contains("Paper five"));
    assert!(
        !list.contains("Paper six"),
        "the sixth paper is folded away"
    );
    assert!(html.contains(&format!("Inguinal hernia {t} question")));
    assert!(html.contains("Full run \u{b7} 31 Oct 2026"));
    assert!(html.contains("Retracted"));
    assert!(html.contains("includes non-RCT studies"));
    // Organ chips.
    assert!(html.contains(&format!("Abdominal wall {t}")));
    assert!(html.contains(r#"aria-label="Organ""#));
    assert!(html.contains(r#"aria-label="View""#));
    assert!(html.contains("Find on your map"));
    // The list is in the first HTML, but hidden: the page opens on the Map.
    let first = first_html(&html);
    for want in [
        r#"class="kg-list""#,
        "Papers, ranked \u{b7} 6",
        "Paper five",
        "Show all 6",
        NOT_ALL,
        "Tap a dot",
    ] {
        assert!(first.contains(want), "{want} is not in the first HTML");
    }
    assert!(
        tag_with(&html, r#"class="kg-list""#).contains(" hidden"),
        "the List shows"
    );
    assert!(
        !tag_with(&html, r#"id="kg-map""#).contains(" hidden"),
        "the Map is hidden"
    );
    assert!(html.contains(r#"data-view="map""#));

    // The fixed words, exactly.
    assert!(text.contains(NOT_ALL));
    assert!(text.contains(LINE_MEANS));
    assert!(!html.contains('\u{20b9}'), "a price on the page");
    // A label is text, never markup.
    assert!(!html.contains("<script>x</script>"));
    assert!(html.contains("&lt;script&gt;x&lt;/script&gt;"));
    let _ = run;
}

#[tokio::test]
async fn a_papers_card_opens_its_study_sheet() {
    let _g = SERIAL.lock().await;
    let Some(pool) = connect().await else { return };
    let (uid, email) = new_user(&pool, "surgeon").await;
    let t = tag();
    let p = pmid(&t, 1);
    let run = mapped_run(&pool, uid, &t, &[paper(&p, "A trial", 2024, "RCT")]).await;
    sqlx::query(
        "INSERT INTO incisive.run_answers (run_id, body, answer_version) VALUES ($1, $2, 3)",
    )
    .bind(run)
    .bind(serde_json::json!({"version": 3, "references": [
        {"n": 1, "pmid": "1"}, {"n": 4, "pmid": p}
    ]}))
    .execute(&pool)
    .await
    .unwrap();
    let (s, body) = call(
        router(state(pool.clone(), true)),
        "explore_list",
        "{}",
        Some(&email),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{body}");
    let m: MapData = serde_json::from_str(&body).unwrap();
    let c = ep::card_for(&m, &format!("p:{p}")).unwrap();
    assert_eq!(c.sheet, Some(format!("/runs/{run}#ref-4")));
    let _ = email;
}

#[tokio::test]
async fn a_big_map_lays_out_the_top_papers() {
    let _g = SERIAL.lock().await;
    let Some(pool) = connect().await else { return };
    let (uid, email) = new_user(&pool, "surgeon").await;
    let t = tag();
    let papers: Vec<PaperRow> = (0..420)
        .map(|i| {
            paper(
                &pmid(&t, i),
                &format!("Paper {i}"),
                2000 + (i % 20) as i32,
                "RCT",
            )
        })
        .collect();
    mapped_run(&pool, uid, &t, &papers).await;
    let st = state(pool.clone(), true);

    // 5 + 420 dots: the map keeps 395 papers.
    let (s, body) = call(
        router(st.clone()),
        "explore_map",
        r#"{"w": 860, "h": 720}"#,
        Some(&email),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{body}");
    let m: MapData = serde_json::from_str(&body).unwrap();
    assert!(m.dots.len() <= 400, "{}", m.dots.len());
    assert_eq!(m.top_papers, Some(395));
    assert_eq!(
        m.dots.iter().filter(|d| d.kind == DotKind::Paper).count(),
        395
    );
    assert!(m.dots.iter().any(|d| d.x > 0.0 && d.y > 0.0), "laid out");
    assert!(m.dots.iter().all(|d| d.x <= 860.0 && d.y <= 720.0));

    // The list has everything.
    let (_, body) = call(router(st.clone()), "explore_list", "{}", Some(&email)).await;
    let all: MapData = serde_json::from_str(&body).unwrap();
    assert_eq!(all.dots.len(), 425);

    let (_, html) = get(router(st), "/explore", Some(&email)).await;
    assert!(html.contains("Showing the top 395 papers"));
    assert!(html.contains("Papers, ranked \u{b7} 420"));
}

/// With `GRAPH=off` the owner's header (Activity in the nav) is byte for
/// byte as before Explore. With `EXPLORE_NAV_CAPTURE=1` it is written to
/// [`OWNER_HEADER`] instead (done with `ivory.rs` and `ask.rs` from
/// f152318, the commit before Explore).
#[tokio::test]
async fn the_owners_header_is_unchanged_with_graph_off() {
    let _g = SERIAL.lock().await;
    let Some(pool) = connect().await else { return };
    let (_, email) = new_user(&pool, "owner").await;
    let st = state(pool, false);
    let mut now = serde_json::Map::new();
    for path in ["/", "/history", "/activity"] {
        let (s, html) = get(router(st.clone()), path, Some(&email)).await;
        assert_eq!(s, StatusCode::OK, "{path}");
        now.insert(
            path.to_string(),
            serde_json::Value::String(header_of(&html)),
        );
    }
    let file = format!("{}/{OWNER_HEADER}", env!("CARGO_MANIFEST_DIR"));
    if std::env::var("EXPLORE_NAV_CAPTURE").is_ok_and(|v| v == "1") {
        std::fs::create_dir_all(std::path::Path::new(&file).parent().unwrap()).unwrap();
        std::fs::write(&file, serde_json::to_string_pretty(&now).unwrap()).unwrap();
        eprintln!("captured {file}");
        return;
    }
    let before: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
    for (path, h) in &now {
        assert_eq!(&before[path], h, "{path}: the owner's header changed");
        assert!(!h.as_str().unwrap().contains("/explore"));
        assert!(h.as_str().unwrap().contains(r#"href="/activity""#));
    }
}

/// B never reaches A's map: not with A's organ id, and not A's reference
/// numbers for a paper both of them have.
#[tokio::test]
async fn another_users_organ_and_sheets_stay_theirs() {
    let _g = SERIAL.lock().await;
    let Some(pool) = connect().await else { return };
    let (a, email_a) = new_user(&pool, "surgeon").await;
    let (b, email_b) = new_user(&pool, "surgeon").await;
    let (ta, tb) = (tag(), tag());
    let shared = pmid(&ta, 1);
    let only_a = pmid(&ta, 2);
    let run_a = mapped_run(
        &pool,
        a,
        &ta,
        &[
            paper(&shared, "Shared trial", 2024, "RCT"),
            paper(&only_a, "A's trial", 2023, "RCT"),
        ],
    )
    .await;
    let run_b = mapped_run(
        &pool,
        b,
        &tb,
        &[paper(&shared, "Shared trial", 2024, "RCT")],
    )
    .await;
    let answer =
        |n: u16, p: &str| serde_json::json!({"version": 3, "references": [{"n": n, "pmid": p}]});
    for (run, body) in [(run_a, answer(7, &shared)), (run_b, answer(2, &shared))] {
        sqlx::query(
            "INSERT INTO incisive.run_answers (run_id, body, answer_version) VALUES ($1, $2, 3)",
        )
        .bind(run)
        .bind(body)
        .execute(&pool)
        .await
        .unwrap();
    }
    let st = state(pool.clone(), true);
    let (_, body) = call(router(st.clone()), "explore_list", "{}", Some(&email_a)).await;
    let mine: MapData = serde_json::from_str(&body).unwrap();
    let organ_a = mine
        .dots
        .iter()
        .find(|d| d.kind == DotKind::Organ)
        .and_then(|d| d.concept_id)
        .unwrap();
    let shared_a = mine
        .dots
        .iter()
        .find(|d| d.id == format!("p:{shared}"))
        .unwrap();
    assert_eq!(shared_a.sheet.map(|s| (s.run, s.n)), Some((run_a, 7)));

    // B with A's organ: nothing.
    let filter = format!(
        r#"{{"filter": {{"organ": {organ_a}, "year_from": null, "year_to": null, "certainty": null, "hidden": []}}}}"#
    );
    for f in ["explore_list", "explore_map"] {
        let (s, body) = call(router(st.clone()), f, &filter, Some(&email_b)).await;
        assert_eq!(s, StatusCode::OK, "{f}: {body}");
        let m: MapData = serde_json::from_str(&body).unwrap();
        assert!(m.dots.is_empty(), "{f}: {:?}", m.dots);
        assert_eq!(m.runs, 0, "{f}");
        assert!(
            !body.contains(&ta) && !body.contains(&run_a.to_string()),
            "{f}"
        );
    }
    // B's own map: the shared paper opens B's sheet, never A's.
    let (_, body) = call(router(st.clone()), "explore_list", "{}", Some(&email_b)).await;
    let theirs: MapData = serde_json::from_str(&body).unwrap();
    assert!(!body.contains(&run_a.to_string()) && !body.contains(&only_a));
    let shared_b = theirs
        .dots
        .iter()
        .find(|d| d.id == format!("p:{shared}"))
        .unwrap();
    assert_eq!(shared_b.sheet.map(|s| (s.run, s.n)), Some((run_b, 2)));
    let (_, html) = get(router(st), "/explore", Some(&email_b)).await;
    assert!(!html.contains(&run_a.to_string()) && !html.contains(&ta));
}

/// A page with a browser's User-Agent.
async fn get_as(app: Router, path: &str, email: &str, ua: &str) -> (StatusCode, String) {
    let req = Request::builder()
        .method("GET")
        .uri(path)
        .header("x-dev-user-email", email)
        .header("user-agent", ua)
        .body(Body::empty())
        .unwrap();
    body_of(app.oneshot(req).await.unwrap()).await
}

/// The map's dot buttons in a page.
fn dot_buttons(markup: &str) -> usize {
    regex::Regex::new(r#"<button type="button"[^>]*class="kg-dot( on)?""#)
        .unwrap()
        .find_iter(markup)
        .count()
}

const IPHONE: &str = "Mozilla/5.0 (iPhone; CPU iPhone OS 17_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.0 Mobile/15E148 Safari/604.1";

/// The server draws the map: one real button per dot, named by kind and
/// label, laid out for a desktop frame or, for a phone, a phone frame. A
/// question's card has the question, its bottom line and its certainty, and
/// only its owner ever sees them.
#[tokio::test]
async fn the_map_is_drawn_with_a_button_per_dot() {
    let _g = SERIAL.lock().await;
    let Some(pool) = connect().await else { return };
    let (uid, email) = new_user(&pool, "surgeon").await;
    let (_, other) = new_user(&pool, "surgeon").await;
    let t = tag();
    let question = format!("Is robotic hernia repair better {t}?");
    let run = mapped_run(
        &pool,
        uid,
        &t,
        &[
            paper(&pmid(&t, 1), "A trial", 2024, "RCT"),
            paper(&pmid(&t, 2), "A cohort", 2019, "Retrospective cohort"),
        ],
    )
    .await;
    sqlx::query("UPDATE incisive.runs SET question = $1 WHERE id = $2")
        .bind(&question)
        .bind(run)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO incisive.run_answers (run_id, body, answer_version) VALUES ($1, $2, 3)",
    )
    .bind(run)
    .bind(serde_json::json!({"version": 3,
        "answer": {"bottom_line": {"text": "No consistent benefit [1].", "certainty": "Moderate"}},
        "references": [{"n": 1, "pmid": pmid(&t, 1)}]}))
    .execute(&pool)
    .await
    .unwrap();
    let st = state(pool.clone(), true);

    let (s, html) = get(router(st.clone()), "/explore", Some(&email)).await;
    assert_eq!(s, StatusCode::OK);
    let m = markup(&html);
    let (_, body) = call(
        router(st.clone()),
        "explore_map",
        r#"{"w": 860, "h": 720}"#,
        Some(&email),
    )
    .await;
    let map: MapData = serde_json::from_str(&body).unwrap();
    assert_eq!(map.dots.len(), 7);
    assert_eq!(dot_buttons(&m), map.dots.len(), "one button per dot");
    for name in [
        format!("Disease: Inguinal hernia {t}"),
        format!("Organ: Abdominal wall {t}"),
        format!("Treatment: Robotic surgery {t}"),
        format!("Outcome: Recurrence {t}"),
        format!("Your question: Inguinal hernia {t} question"),
        "RCT paper: A trial".to_string(),
        "Retrospective paper: A cohort".to_string(),
    ] {
        assert!(
            m.contains(&format!(r#"aria-label="{name}""#)),
            "no dot named {name}"
        );
    }
    // Lines are drawn, paper lines dashed; the frame is the desktop's.
    assert!(m.contains(r#"viewBox="0 0 860 720""#));
    assert_eq!(m.matches("<line ").count(), map.lines.len());
    assert!(m.contains(r#"stroke-dasharray="3 3""#));
    // Lines keep their screen width when the frame is drawn smaller than
    // its units; the background is a pointer surface, out of the tab order.
    assert!(m.contains(r#"vector-effect="non-scaling-stroke""#));
    let bg = regex::Regex::new(r#"<button[^>]*class="kg-bg"[^>]*>"#)
        .unwrap()
        .find(&m)
        .expect("the background button")
        .as_str()
        .to_string();
    assert!(bg.contains(r#"tabindex="-1""#), "{bg}");
    // The page's phone layout starts where the phone frame does.
    assert_eq!(ep::PHONE_BELOW, 700.0);
    assert!(m.contains("@media (min-width:700px)"));
    assert!(m.contains("@media (max-width:699.98px)"));
    assert!(m.contains("Time-lapse by paper year"));
    assert!(m.contains("Show on the map"));
    // The disease row names the run's certainty.
    assert!(m.contains(&format!("Abdominal wall {t} \u{b7} Certainty: Moderate")));

    // A phone gets the phone frame.
    let (_, phone) = get_as(router(st.clone()), "/explore", &email, IPHONE).await;
    let pm = markup(&phone);
    assert!(pm.contains(r#"viewBox="0 0 358 480""#));
    assert_eq!(dot_buttons(&pm), 7);

    // The question's card, from the user's own runs.
    let (_, body) = call(router(st.clone()), "explore_list", "{}", Some(&email)).await;
    let all: MapData = serde_json::from_str(&body).unwrap();
    let c = ep::card_for(&all, &format!("r:{run}")).unwrap();
    assert_eq!(c.title, question);
    assert_eq!(c.body, "No consistent benefit.");
    assert_eq!(
        c.kicker,
        "Full run \u{b7} 31 Oct 2026 \u{b7} Certainty: Moderate"
    );
    assert_eq!(c.answer, Some(format!("/runs/{run}")));
    // "Run certainty" goes by the run's own certainty (its outcome is Low).
    for (want, runs) in [("Moderate", 1), ("Low", 0)] {
        let f = format!(
            r#"{{"filter": {{"organ": null, "year_from": null, "year_to": null, "certainty": "{want}", "hidden": []}}, "w": 358, "h": 480}}"#
        );
        let (s, body) = call(router(st.clone()), "explore_map", &f, Some(&email)).await;
        assert_eq!(s, StatusCode::OK, "{body}");
        let m: MapData = serde_json::from_str(&body).unwrap();
        assert_eq!(m.runs, runs, "{want}");
        assert!(m.dots.iter().all(|d| d.x <= 358.0 && d.y <= 480.0));
    }

    // Nobody else sees it.
    let (_, theirs) = get(router(st.clone()), "/explore", Some(&other)).await;
    assert!(!theirs.contains(&question) && !theirs.contains(&t));
    let (_, body) = call(router(st), "explore_list", "{}", Some(&other)).await;
    assert!(!body.contains(&question));
}

/// The page opens on the Map: the switch (Map pressed), the frame saying
/// it is drawing, "Open full screen", and a map drawn sharp at any zoom (no
/// stretched layer).
#[tokio::test]
async fn the_page_opens_on_the_map() {
    let _g = SERIAL.lock().await;
    let Some(pool) = connect().await else { return };
    let (uid, email) = new_user(&pool, "surgeon").await;
    let t = tag();
    mapped_run(
        &pool,
        uid,
        &t,
        &[paper(&pmid(&t, 1), "A trial", 2024, "RCT")],
    )
    .await;
    let st = state(pool.clone(), true);
    let pressed = |name: &str| {
        regex::Regex::new(&format!(
            r#"<button type="button" aria-pressed="(true|false)"[^>]*>\s*{name}"#
        ))
        .unwrap()
    };
    let (map_re, list_re) = (pressed("Map"), pressed("List"));
    for ua in [None, Some(IPHONE)] {
        let (s, html) = match ua {
            None => get(router(st.clone()), "/explore", Some(&email)).await,
            Some(ua) => get_as(router(st.clone()), "/explore", &email, ua).await,
        };
        assert_eq!(s, StatusCode::OK);
        let first = first_html(&html);
        // The switch shows from the first HTML, with Map pressed.
        let switch = tag_with(&first, r#"class="kg-switch""#);
        assert!(!switch.contains("hidden"), "{switch}");
        let map_btn = map_re.captures(&first).expect("the Map button");
        assert_eq!(&map_btn[1], "true");
        let list_btn = list_re.captures(&first).expect("the List button");
        assert_eq!(&list_btn[1], "false");
        // The frame says it is drawing until the script runs.
        let frame_at = first.find(r#"class="kg-map-frame""#).expect("the frame");
        let drawing = first
            .find("Drawing your map\u{2026}")
            .expect("Drawing your map…");
        assert!(drawing > frame_at, "the drawing note is inside the frame");
        // The list is there, hidden.
        assert!(tag_with(&first, r#"class="kg-list""#).contains(" hidden"));
        // Full screen.
        let open = tag_with(&first, ">Open full screen<");
        assert!(
            open.starts_with("<button") && open.contains("kg-openfull"),
            "{open}"
        );
        assert!(first.contains(r#"aria-label="Close full screen""#));
        // The map is a dialog only while it is full screen.
        assert!(!first.contains(r#"role="dialog""#));
        assert!(!first.contains("kg-full\""), "opened full screen");
        // Nothing is stretched: no transform on a layer, no will-change.
        let m = markup(&html);
        let style = &m[m.find(".kg-map-frame").expect("the styles")..];
        let style = &style[..style.find("</style>").unwrap()];
        assert!(!style.contains("will-change"), "will-change in the styles");
        assert!(!style.contains("scale("), "scale( in the styles");
        let map = &m[m.find(r#"id="kg-map""#).unwrap()..m.find(r#"class="kg-list""#).unwrap()];
        assert!(!map.contains("scale("), "a scaled layer");
        assert!(!map.contains("--ik"), "counter-scaled dots");
        // No remembered view.
        assert!(!html.contains("incisive.explore.view"));
    }
}

// ---------- Ask about this ----------

const ASK_HOME: &str = "tests/fixtures/explore/ask_home_before.html";

/// The id of the concept of `kind` that `mapped_run` made for `tag`.
async fn concept_id(pool: &PgPool, kind: &str, label: &str) -> i64 {
    sqlx::query_scalar("SELECT id FROM incisive.graph_concepts WHERE kind = $1 AND label = $2")
        .bind(kind)
        .bind(label)
        .fetch_one(pool)
        .await
        .unwrap()
}

/// What the Ask page's question box holds as first drawn.
fn question_box(html: &str) -> String {
    let m = markup(html);
    let a = m.find(r#"<textarea id="question""#).expect("textarea");
    let open_end = a + m[a..].find('>').unwrap() + 1;
    let close = open_end + m[open_end..].find("</textarea>").unwrap();
    let tag = &m[a..open_end];
    // The value is either the element's text or a `value` attribute.
    format!("{tag}|{}", &m[open_end..close])
}

async fn spend_and_runs(pool: &PgPool, user: i64) -> (i64, i64) {
    let runs = sqlx::query_scalar("SELECT count(*) FROM incisive.runs WHERE user_id = $1")
        .bind(user)
        .fetch_one(pool)
        .await
        .unwrap();
    let spend = sqlx::query_scalar("SELECT count(*) FROM incisive.spend WHERE user_id = $1")
        .bind(user)
        .fetch_one(pool)
        .await
        .unwrap();
    (runs, spend)
}

#[tokio::test]
async fn the_explore_page_links_ask_about_this_by_id() {
    let _g = SERIAL.lock().await;
    let Some(pool) = connect().await else { return };
    let (uid, email) = new_user(&pool, "surgeon").await;
    let t = tag();
    mapped_run(
        &pool,
        uid,
        &t,
        &[paper(&pmid(&t, 1), "A trial", 2024, "RCT")],
    )
    .await;
    let disease = concept_id(&pool, "disease", &format!("Inguinal hernia {t}")).await;
    let treatment = concept_id(&pool, "treatment", &format!("Robotic surgery {t}")).await;
    let organ = concept_id(&pool, "organ", &format!("Abdominal wall {t}")).await;
    let outcome = concept_id(&pool, "outcome", &format!("Recurrence {t}")).await;
    let (s, body) = call(
        router(state(pool.clone(), true)),
        "explore_list",
        "{}",
        Some(&email),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{body}");
    let m: MapData = serde_json::from_str(&body).unwrap();
    let ask = |kind: DotKind, id: i64| {
        let d = m.dots.iter().find(|d| d.kind == kind).unwrap();
        assert_eq!(d.concept_id, Some(id));
        ep::card_for(&m, &d.id).unwrap().ask
    };
    // By id only: no name in the link.
    assert_eq!(
        ask(DotKind::Disease, disease),
        Some(format!("/?topic={disease}"))
    );
    assert_eq!(
        ask(DotKind::Treatment, treatment),
        Some(format!("/?topic={treatment}"))
    );
    assert_eq!(ask(DotKind::Organ, organ), None);
    assert_eq!(ask(DotKind::Outcome, outcome), None);
}

#[tokio::test]
async fn topic_label_gives_only_the_users_own_names() {
    let _g = SERIAL.lock().await;
    let Some(pool) = connect().await else { return };
    let (a, a_email) = new_user(&pool, "surgeon").await;
    let (_, b_email) = new_user(&pool, "surgeon").await;
    let t = tag();
    mapped_run(&pool, a, &t, &[]).await;
    let disease = concept_id(&pool, "disease", &format!("Inguinal hernia {t}")).await;
    let ask = |email: Option<&'static str>, id: String, on: bool| {
        let pool = pool.clone();
        let email = email.map(str::to_string);
        async move {
            call(
                router(state(pool, on)),
                "topic_label",
                &format!(r#"{{"concept_id":{id}}}"#),
                email.as_deref(),
            )
            .await
        }
    };
    let _ = ask;
    let own = call(
        router(state(pool.clone(), true)),
        "topic_label",
        &format!(r#"{{"concept_id":{disease}}}"#),
        Some(&a_email),
    )
    .await;
    assert_eq!(own.0, StatusCode::OK, "{}", own.1);
    assert_eq!(
        serde_json::from_str::<Option<String>>(&own.1).unwrap(),
        Some(format!("Inguinal hernia {t}"))
    );
    for (what, id, email, on) in [
        ("another user's concept", disease, Some(&b_email), true),
        ("an id that is nobody's", i64::MAX, Some(&a_email), true),
        ("a negative id", -1, Some(&a_email), true),
        ("GRAPH off", disease, Some(&a_email), false),
    ] {
        let (s, body) = call(
            router(state(pool.clone(), on)),
            "topic_label",
            &format!(r#"{{"concept_id":{id}}}"#),
            email.map(String::as_str),
        )
        .await;
        assert_eq!(s, StatusCode::OK, "{what}: {body}");
        assert_eq!(body.trim(), "null", "{what} gave a name");
    }
    let (s, body) = call(
        router(state(pool.clone(), true)),
        "topic_label",
        &format!(r#"{{"concept_id":{disease}}}"#),
        None,
    )
    .await;
    assert_ne!(s, StatusCode::OK, "signed out got {body}");
    assert!(!body.contains(&t));
}

#[tokio::test]
async fn the_ask_page_puts_no_name_in_its_html_and_starts_nothing() {
    let _g = SERIAL.lock().await;
    let Some(pool) = connect().await else { return };
    let (a, a_email) = new_user(&pool, "surgeon").await;
    let (_, b_email) = new_user(&pool, "surgeon").await;
    let t = tag();
    mapped_run(&pool, a, &t, &[]).await;
    let disease = concept_id(&pool, "disease", &format!("Inguinal hernia {t}")).await;
    let before = spend_and_runs(&pool, a).await;

    // The box is filled in the browser (see the Ask page), so the first
    // HTML carries no name for anyone, and the box is as it was.
    let uri = format!("/?topic={disease}");
    let (s, own) = get(router(state(pool.clone(), true)), &uri, Some(&a_email)).await;
    assert_eq!(s, StatusCode::OK);
    assert!(own.contains(r#"<textarea id="question""#));
    assert!(!own.contains(&t), "the name is in the first HTML");

    let cases = [
        ("another user", uri.clone(), b_email.clone(), true),
        ("a bad id", "/?topic=abc".to_string(), a_email.clone(), true),
        (
            "an unknown id",
            "/?topic=999999999999".to_string(),
            a_email.clone(),
            true,
        ),
        ("GRAPH off", uri.clone(), a_email.clone(), false),
    ];
    for (what, uri, email, on) in cases {
        let (s, html) = get(router(state(pool.clone(), on)), &uri, Some(&email)).await;
        assert_eq!(s, StatusCode::OK, "{what}");
        assert!(!html.contains(&t), "{what}: the name leaked");
        assert_eq!(
            question_box(&html),
            question_box(
                &get(router(state(pool.clone(), on)), "/", Some(&email))
                    .await
                    .1
            ),
            "{what}: the box is not empty"
        );
    }
    // Nothing was started: no run, no spend.
    assert_eq!(spend_and_runs(&pool, a).await, before);
}

/// Ask with no `topic` is what it was before this task.
#[tokio::test]
async fn the_ask_page_without_a_topic_is_unchanged() {
    let _g = SERIAL.lock().await;
    let Some(pool) = connect().await else { return };
    let (_, email) = new_user(&pool, "surgeon").await;
    let (s, html) = get(router(state(pool, true)), "/", Some(&email)).await;
    assert_eq!(s, StatusCode::OK);
    let now = settled(&html);
    let file = format!("{}/{ASK_HOME}", env!("CARGO_MANIFEST_DIR"));
    if std::env::var("EXPLORE_ASK_CAPTURE").is_ok_and(|v| v == "1") {
        std::fs::write(&file, &now).unwrap();
        eprintln!("captured {file}");
        return;
    }
    let before = std::fs::read_to_string(&file).unwrap();
    assert!(now == before, "the Ask page changed with no topic");
}
