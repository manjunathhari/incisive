use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode},
};
use http_body_util::BodyExt;
use incisive_app::config::Look;
use incisive_app::health::{Readiness, health_routes};
use incisive_app::server::{build_router, build_router_with_look};
use leptos::config::LeptosOptions;
use tower::ServiceExt;

fn identity_state() -> incisive_app::server::AppState {
    use incisive_app::identity::{IapVerifier, JwkSource};
    let pool = incisive_app::db::lazy_pool(&secrecy::SecretString::from(
        "postgres://u:p@127.0.0.1:1/none".to_string(),
    ))
    .unwrap();
    let verifier = IapVerifier::new(
        "aud".into(),
        JwkSource::Static(serde_json::from_str(r#"{"keys":[]}"#).unwrap()),
    );
    incisive_app::server::AppState {
        pool,
        verifier: Arc::new(verifier),
        gate: Arc::new(incisive_app::gate::OfflineClassifier),
        prices: incisive_app::spend::Prices::default(),
        usd_inr: 88.0,
        caps: incisive_app::spend::CapsConfig {
            global_monthly_paise: 1,
        },
        gate_model: "unused".into(),
        dev_mode: false,
        runs: incisive_app::runs::RunsConfig::default(),
        // Phase 2 tests: Quick keeps the classic pipeline unless a test says otherwise.
        quick_engine: incisive_app::runs::QuickEngine::Classic,
        full_engine: incisive_app::runs::Engine::Classic,
        completeness: false,
        temporal: Default::default(),
        temporal_client: None,
        stream: false,
        intro: Default::default(),
        graph: Default::default(),
        memory: incisive_app::memory::settings::MemorySettings::default(),
        chat: incisive_app::chat::settings::ChatSettings::default(),
        embedder: None,
        gemma: None,
        worker: incisive_app::runs::worker::WorkerHandle::default(),
        llm: None,
        lit: incisive_app::agent::testing::offline_literature(),
    }
}

struct AlwaysUp;
struct AlwaysDown;
struct Hangs;

#[async_trait]
impl Readiness for AlwaysUp {
    async fn db_ok(&self) -> bool {
        true
    }
}
#[async_trait]
impl Readiness for AlwaysDown {
    async fn db_ok(&self) -> bool {
        false
    }
}
#[async_trait]
impl Readiness for Hangs {
    async fn db_ok(&self) -> bool {
        tokio::time::sleep(Duration::from_secs(10)).await;
        true
    }
}

async fn get(app: Router, path: &str) -> (StatusCode, String) {
    let res = app
        .oneshot(Request::get(path).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = res.status();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    (status, String::from_utf8(bytes.to_vec()).unwrap())
}

fn json(s: &str) -> serde_json::Value {
    serde_json::from_str(s).unwrap()
}

#[tokio::test]
async fn healthz_ok_even_when_db_down() {
    let (status, body) = get(health_routes(Arc::new(AlwaysDown)), "/healthz").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "ok");
}

#[tokio::test]
async fn readyz_up() {
    let (status, body) = get(health_routes(Arc::new(AlwaysUp)), "/readyz").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json(&body), serde_json::json!({"db": "up"}));
}

#[tokio::test]
async fn readyz_down() {
    let (status, body) = get(health_routes(Arc::new(AlwaysDown)), "/readyz").await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(json(&body), serde_json::json!({"db": "down"}));
}

#[tokio::test]
async fn health_alias_ok() {
    let (status, body) = get(health_routes(Arc::new(AlwaysDown)), "/health").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "ok");
}

#[tokio::test]
async fn ready_alias_matches_readyz() {
    let (status, body) = get(health_routes(Arc::new(AlwaysUp)), "/ready").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json(&body), serde_json::json!({"db": "up"}));
    let (status, body) = get(health_routes(Arc::new(AlwaysDown)), "/ready").await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(json(&body), serde_json::json!({"db": "down"}));
}

#[tokio::test(start_paused = true)]
async fn readyz_times_out_as_down() {
    let app = health_routes(Arc::new(Hangs));
    let handle = tokio::spawn(async move { get(app, "/readyz").await });
    tokio::time::advance(Duration::from_secs(3)).await;
    let (status, body) = handle.await.unwrap();
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(json(&body), serde_json::json!({"db": "down"}));
}

#[tokio::test]
async fn home_page_shows_name_and_tagline() {
    let opts = LeptosOptions::builder().output_name("incisive-app").build();
    let (status, body) = get(
        build_router(Arc::new(AlwaysUp), identity_state(), opts),
        "/",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("Incisive"));
    assert!(body.contains(
        "Medical literature \u{2014} searched, checked, summarised \u{b7} <b>Educational tool only</b>"
    ));
    // The phone line: today's date and "Educational tool only".
    assert!(body.contains("\u{b7} <b>Educational tool only</b></span>"));
    assert!(!body.contains("Medical literature, summarised"));
}

#[tokio::test]
async fn browser_code_is_revalidated_on_every_load() {
    // A deploy keeps the /pkg/ file names, so browsers must revalidate them
    // (otherwise a cached old build runs against the new page and nothing
    // on it responds).
    let dir = std::env::temp_dir().join(format!("incisive-site-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("pkg")).unwrap();
    std::fs::write(dir.join("pkg/incisive-app.js"), "export default 1;").unwrap();
    let opts = LeptosOptions::builder()
        .output_name("incisive-app")
        .site_root(dir.to_str().unwrap())
        .build();
    let res = build_router(Arc::new(AlwaysUp), identity_state(), opts)
        .oneshot(
            Request::builder()
                .uri("/pkg/incisive-app.js")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(
        res.headers()
            .get(axum::http::header::CACHE_CONTROL)
            .map(|v| v.to_str().unwrap()),
        Some("no-cache")
    );
    // Pages and look-alike paths are left alone.
    for path in ["/", "/pkgx"] {
        let res = build_router(
            Arc::new(AlwaysUp),
            identity_state(),
            LeptosOptions::builder()
                .output_name("incisive-app")
                .site_root(dir.to_str().unwrap())
                .build(),
        )
        .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
        .await
        .unwrap();
        assert!(
            res.headers()
                .get(axum::http::header::CACHE_CONTROL)
                .is_none(),
            "{path}"
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// Ivory redesign: Figtree is served by the app from the site root (copied
/// there by cargo-leptos from `public/`, `assets-dir`), as `font/woff2`;
/// the page asks Google Fonts for nothing.
#[tokio::test]
async fn figtree_is_self_hosted_and_google_fonts_are_never_requested() {
    let public = concat!(env!("CARGO_MANIFEST_DIR"), "/public");
    let opts = LeptosOptions::builder()
        .output_name("incisive-app")
        .site_root(public)
        .build();
    for w in ["400", "500", "600", "700"] {
        for set in ["latin", "latin-ext"] {
            let path = format!("/fonts/figtree/figtree-{set}-{w}-normal.woff2");
            let res = build_router(Arc::new(AlwaysUp), identity_state(), opts.clone())
                .oneshot(Request::builder().uri(&path).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(res.status(), StatusCode::OK, "{path}");
            assert_eq!(
                res.headers()
                    .get(axum::http::header::CONTENT_TYPE)
                    .map(|v| v.to_str().unwrap()),
                Some("font/woff2"),
                "{path}"
            );
            let bytes = res.into_body().collect().await.unwrap().to_bytes();
            assert_eq!(&bytes[..4], b"wOF2", "{path}");
        }
    }
    // The OFL licence sits next to the files.
    let licence = std::fs::read_to_string(format!("{public}/fonts/figtree/LICENSE.txt")).unwrap();
    assert!(licence.contains("SIL Open Font License, Version 1.1"));

    let (status, body) = get(
        build_router(Arc::new(AlwaysUp), identity_state(), opts),
        "/",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("/fonts/figtree/figtree-latin-400-normal.woff2"));
    assert!(body.contains("font-display:swap"));
    assert!(!body.contains("fonts.googleapis.com"), "{body}");
    assert!(!body.contains("fonts.gstatic.com"));
    let style = incisive_app::app::IVORY_STYLE;
    assert!(!style.contains("googleapis"));
    // The ivory styles never reach the answer content.
    for sel in [".ap-main", ".ap ", ".ap{", ".ap-original", ".answer"] {
        assert!(!style.contains(sel), "{sel}");
    }
}

/// The app with a chosen look, serving static files from `public/`.
fn look_router(look: Look) -> Router {
    let public = concat!(env!("CARGO_MANIFEST_DIR"), "/public");
    let opts = LeptosOptions::builder()
        .output_name("incisive-app")
        .site_root(public)
        .build();
    build_router_with_look(Arc::new(AlwaysUp), identity_state(), opts, look)
}

/// `LOOK` unset or `ivory`: the page carries no trace of the stipple look
/// and still asks for Figtree.
#[tokio::test]
async fn ivory_look_is_unchanged_and_asks_for_nothing_new() {
    let (status, body) = get(look_router(Look::Ivory), "/").await;
    assert_eq!(status, StatusCode::OK);
    for absent in [
        "data-look",
        "hanken-grotesk",
        "source-serif-4",
        "/look/",
        "stipple",
    ] {
        assert!(!body.contains(absent), "{absent}");
    }
    assert!(body.contains("/fonts/figtree/figtree-latin-400-normal.woff2"));
    // The plain builder is the ivory look.
    let opts = LeptosOptions::builder()
        .output_name("incisive-app")
        .site_root(concat!(env!("CARGO_MANIFEST_DIR"), "/public"))
        .build();
    let (_, plain) = get(
        build_router(Arc::new(AlwaysUp), identity_state(), opts),
        "/",
    )
    .await;
    assert!(!plain.contains("data-look"));
    assert!(!plain.contains("stipple"));
    // The page head is as it was: `<html>` bare, three style blocks, then
    // the reload placeholder and the scripts, with nothing new between.
    for page in [&body, &plain] {
        assert!(page.starts_with("<!DOCTYPE html><html lang=\"en\"><head>"));
        assert_eq!(page.matches("<style>").count(), 3);
        assert!(page.contains("</style><!><link rel=\"modulepreload\""));
    }
}

/// `LOOK=stipple`: the page is marked, the stipple styles follow the ivory
/// ones, and its fonts and images are served by the app itself.
#[tokio::test]
async fn stipple_look_serves_its_own_fonts_and_images() {
    let public = concat!(env!("CARGO_MANIFEST_DIR"), "/public");
    let style = incisive_app::app::STIPPLE_STYLE;
    let (status, body) = get(look_router(Look::Stipple), "/").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("<html lang=\"en\" data-look=\"stipple\">"));
    assert!(style.contains("color-scheme: light;"));
    assert!(body.contains("html[data-look=\"stipple\"] { color-scheme: light;"));
    assert!(body.contains("/look/stipple-day.svg"));
    // One more style block than ivory, the last of the four.
    assert_eq!(body.matches("<style>").count(), 4);
    let last = body.rfind("<style>").unwrap();
    assert!(
        body[last..].starts_with("<style>\n@font-face { font-family:'Hanken Grotesk Variable';")
    );
    assert!(
        body.find(".rx-seg").unwrap() < last,
        "after the tracker styles"
    );
    assert!(!body.contains("fonts.googleapis.com"));
    assert!(!body.contains("fonts.gstatic.com"));
    // The not-found page wears the look too.
    let (_, missing) = get(look_router(Look::Stipple), "/no-such-page").await;
    assert!(missing.contains("<html lang=\"en\" data-look=\"stipple\">"));
    assert!(missing.contains("Page not found"));
    // The stipple block comes after the ivory one, so its variables win.
    let ivory_at = body.find(":root { --iv-ground:#F5F3EF;").unwrap();
    let stipple_at = body.find("--iv-ground:#f7f4ec;").unwrap();
    assert!(ivory_at < stipple_at);
    // Ivory's own font is still there for the pieces not yet restyled.
    assert!(body.contains("/fonts/figtree/figtree-latin-400-normal.woff2"));

    for name in [
        "stipple-day.svg",
        "stipple-night.svg",
        "foot-day.webp",
        "foot-night.webp",
        "sai-hero-day.webp",
        "sai-hero-night.webp",
    ] {
        let path = format!("/look/{name}");
        let res = look_router(Look::Stipple)
            .oneshot(Request::builder().uri(&path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK, "{path}");
    }

    // Every font the stipple styles name is served as a real woff2 file.
    let fonts: Vec<&str> = style
        .split("url(")
        .skip(1)
        .map(|rest| rest.split(')').next().unwrap())
        .filter(|u| u.starts_with("/fonts/"))
        .collect();
    assert_eq!(fonts.len(), 4, "{fonts:?}");
    for path in fonts {
        assert!(
            path.starts_with("/fonts/hanken-grotesk/")
                || path.starts_with("/fonts/source-serif-4/"),
            "{path}"
        );
        let res = look_router(Look::Stipple)
            .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK, "{path}");
        assert_eq!(
            res.headers()
                .get(axum::http::header::CONTENT_TYPE)
                .map(|v| v.to_str().unwrap()),
            Some("font/woff2"),
            "{path}"
        );
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&bytes[..4], b"wOF2", "{path}");
    }
    // The OFL licences sit next to the files.
    for dir in ["hanken-grotesk", "source-serif-4"] {
        let licence = std::fs::read_to_string(format!("{public}/fonts/{dir}/LICENSE.txt")).unwrap();
        assert!(
            licence.contains("SIL Open Font License, Version 1.1"),
            "{dir}"
        );
    }
}

/// Every stipple rule is under `html[data-look="stipple"]`, and none reaches
/// the answer content.
#[test]
fn stipple_style_is_scoped_and_leaves_the_answer_alone() {
    let style = incisive_app::app::STIPPLE_STYLE;
    for banned in [
        ".ap-main",
        ".ap ",
        ".ap{",
        ".ap-original",
        ".answer",
        "googleapis",
        "gstatic",
        "building",
        "font-weight:600",
        "font-weight:700",
        "font-weight: 600",
        "font-weight: 700",
        // As a weight or anywhere else.
        "bold",
    ] {
        assert!(!style.contains(banned), "{banned}");
    }
    const SCOPE: &str = "html[data-look=\"stipple\"]";
    let mut rules = 0;
    let mut media_rules = 0;
    for line in style.lines() {
        let Some((selectors, _)) = line.split_once('{') else {
            continue;
        };
        let selectors = selectors.trim();
        if selectors.starts_with("@font-face") {
            continue;
        }
        // A rule inside `@media` is held to the same scope: on its own
        // line (checked like any other), or after the block's opening brace.
        let selectors = if selectors.starts_with("@media") {
            let rest = line.split_once('{').unwrap().1;
            match rest.split_once('{') {
                Some((inner, _)) => {
                    media_rules += 1;
                    inner.trim()
                }
                None => {
                    assert!(rest.trim().is_empty(), "{line}");
                    continue;
                }
            }
        } else {
            selectors
        };
        // Each selector of a list carries the scope.
        for selector in selectors.split(',') {
            assert!(selector.trim().starts_with(SCOPE), "{line}");
        }
        rules += 1;
        // Inside a block a rule's line is indented.
        if line.starts_with("  ") {
            media_rules += 1;
        }
    }
    assert!(rules > 30, "{rules}");
    // The rules inside the `@media` blocks were among those checked.
    assert!(media_rules >= 8, "{media_rules}");
    // No brace is left to a line the loop did not read as a rule or a block.
    for line in style.lines() {
        if line.trim() == "}" || line.contains('{') || !line.contains('}') {
            continue;
        }
        // The one many-line rule: the variables, closed on their last line.
        assert!(line.trim_start().starts_with("--iv-"), "{line}");
    }
    // The variables Task 1 promises, at their day values.
    for needle in [
        "--iv-ground:#f7f4ec;",
        "--iv-accent:#1f6bd0;",
        "--iv-r:18px;",
        "--iv-button:#13263c;",
        "--iv-on-fill:#ffffff;",
        "'Hanken Grotesk Variable'; font-style:normal; font-display:swap; font-weight:300 500;",
        "'Source Serif 4 Variable'; font-style:normal; font-display:swap; font-weight:400;",
        "body::before { content:\"\"; position:fixed; inset:0; z-index:-1; pointer-events:none; background:url(/look/stipple-day.svg) 0 0/480px 480px repeat; }",
        " .iv-h1 { font-weight:300; }",
        " .iv-name { font-weight:300; }",
    ] {
        assert!(style.contains(needle), "{needle}");
    }
    // Left to ivory: the alert and warning colours.
    for kept in ["--iv-alert", "--iv-warn"] {
        assert!(!style.contains(kept), "{kept}");
    }
}

/// Blanks the value of every `name="…"` attribute (the per-request nonce
/// and crossorigin values differ between two requests).
fn blank_attr(html: &str, name: &str) -> String {
    let open = format!("{name}=\"");
    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    while let Some(at) = rest.find(&open) {
        let after = at + open.len();
        out.push_str(&rest[..after]);
        let end = rest[after..].find('"').map_or(rest.len(), |e| after + e);
        rest = &rest[end..];
    }
    out.push_str(rest);
    out
}

/// Cuts the header's brand link (`<a … class="iv-brand">…</a>`) out of a
/// page and returns the page without it and the link itself.
fn cut_brand(html: &str) -> (String, String) {
    let class_at = html.find("class=\"iv-brand\"").expect("brand link");
    let start = html[..class_at].rfind("<a ").unwrap();
    let end = class_at + html[class_at..].find("</a>").unwrap() + "</a>".len();
    (
        format!("{}{}", &html[..start], &html[end..]),
        html[start..end].to_string(),
    )
}

/// The stipple page is the ivory page plus the `data-look` attribute, one
/// last style block and the markup the look draws for itself: the header's
/// brand link on every page, and on Ask the hero opening and the footer.
/// Those pieces are pinned here to the letter and taken out; what is left
/// must be the ivory page, so nothing else can drift apart unnoticed. The
/// ivory side is compared as served, less only its own brand link.
#[tokio::test]
async fn stipple_page_is_the_ivory_page_plus_the_attribute_and_one_style_block() {
    const BRAND: &str = "<a href=\"/\" class=\"iv-brand\"><span aria-hidden=\"true\" class=\"st-badge\">\
        <svg width=\"25\" height=\"25\" viewBox=\"0 0 30 30\" aria-hidden=\"true\" focusable=\"false\">\
        <path d=\"M8 22 L22 8\" stroke=\"#2C4A85\" stroke-width=\"2.6\" stroke-linecap=\"round\"></path>\
        <circle cx=\"22\" cy=\"8\" r=\"2.4\" fill=\"#2C4A85\"></circle></svg></span>\
        <span class=\"st-wordmark\">Incisive</span></a>";
    const HERO: &str = "<div class=\"iv-wrap st-hero\"><div class=\"st-hero-art\" aria-hidden=\"true\"></div>\
        <span class=\"st-label\">Educational tool only</span>";
    const FOOT: &str = "<footer class=\"st-foot\">\
        <span>Incisive \u{b7} Medical literature \u{2014} searched, checked, summarised</span>\
        <span>Educational tool only</span></footer>";
    // Ask, an unknown path, the Library, Activity, a run page and Explore
    // (this router has no app state, so the last three are drawn in their
    // signed-out or not-found state; `tests/explore_ui.rs` compares the
    // full Explore page).
    for path in [
        "/",
        "/no-such-page",
        "/history",
        "/activity",
        "/runs/00000000-0000-0000-0000-000000000000",
        "/explore",
    ] {
        let (_, ivory) = get(look_router(Look::Ivory), path).await;
        // Ivory: no stipple class, nothing from /look/, no mark.
        for gone in ["class=\"st-", "/look/", "data-look"] {
            assert!(!ivory.contains(gone), "{path}: {gone}");
        }
        let (_, stipple) = get(look_router(Look::Stipple), path).await;
        let start = stipple.rfind("<style>").unwrap();
        let end = start + stipple[start..].find("</style>").unwrap() + "</style>".len();
        let stripped = format!("{}{}", &stipple[..start], &stipple[end..]).replacen(
            " data-look=\"stipple\"",
            "",
            1,
        );
        // The brand link: the badge and the wordmark in stipple, the tile,
        // the name and both subtitle lines in ivory.
        let (stripped, stipple_brand) = cut_brand(&stripped);
        let (ivory_rest, ivory_brand) = cut_brand(&ivory);
        assert_eq!(stipple_brand, BRAND, "{path}");
        for kept in [
            "class=\"iv-tile\"",
            "class=\"iv-name\"",
            "iv-sub wide",
            "iv-sub phone",
        ] {
            assert!(ivory_brand.contains(kept), "{path}: {kept}");
        }
        // Ask alone has the hero.
        let on_ask = path == "/";
        assert_eq!(
            stripped.matches(HERO).count(),
            usize::from(on_ask),
            "{path}"
        );
        // Every page closes with the footer line, just before `</main>`.
        assert_eq!(stripped.matches(FOOT).count(), 1, "{path}");
        assert!(stripped.contains(&format!("{FOOT}</main>")), "{path}");
        // The hero band sits above the two columns in stipple; in ivory
        // the same heading and intro are a plain `<div>` at the top of the
        // main column. Put it back there before comparing.
        let mut stripped = stripped.replacen(FOOT, "", 1);
        if on_ask {
            let at = stripped.find(HERO).unwrap();
            let end = at + stripped[at..].find("</p></div>").unwrap() + "</p></div>".len();
            let heading = format!("<div>{}", &stripped[at + HERO.len()..end]);
            stripped.replace_range(at..end, "");
            // Straight after the main column's opening tag.
            assert_eq!(
                stripped.matches("class=\"iv-ask-main\"").count(),
                1,
                "{path}"
            );
            let tag = stripped.find("class=\"iv-ask-main\"").unwrap();
            let open = tag + stripped[tag..].find('>').unwrap() + 1;
            stripped.insert_str(open, &heading);
        }
        assert!(!stripped.contains("class=\"st-"), "{path}");
        let scrub = |s: &str| blank_attr(&blank_attr(s, "nonce"), "crossorigin");
        assert_eq!(scrub(&stripped), scrub(&ivory_rest), "{path}");
    }
}
