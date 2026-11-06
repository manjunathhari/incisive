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
    assert!(page.contains("Page not found"));
    assert!(!page.contains(&format!("Inguinal hernia {t}")));

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
