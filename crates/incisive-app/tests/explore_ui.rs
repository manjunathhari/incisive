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
    assert!(
        !markup(&html).contains("Paper six"),
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
    // Before the page hydrates (or with no script) the List shows, and the
    // Map's place and the switch, which need the script, do not.
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
    let tag_with = |needle: &str| -> String {
        let at = html.find(needle).unwrap_or_else(|| panic!("no {needle}"));
        let start = html[..at].rfind('<').unwrap();
        let end = at + html[at..].find('>').unwrap();
        html[start..=end].to_string()
    };
    assert!(
        !tag_with(r#"class="kg-list""#).contains("hidden"),
        "the List is hidden"
    );
    assert!(tag_with(r#"id="kg-map""#).contains(" hidden"));
    assert!(tag_with(r#"class="kg-switch""#).contains(" hidden"));
    assert!(html.contains(r#"data-view="list""#));

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
