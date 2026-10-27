//! Allowlisted HTTP client tests. wiremock only; no network, no database.
//! The test-only `with_base_override` maps every allowlisted host name to the
//! mock server, so the allowlist logic still sees the real host names.

use std::time::Duration;

use incisive_app::lit::http::{AllowlistedClient, LitError, OaPolicy};
use incisive_app::lit::{Paper, PaperId, PaperSource};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const EUTILS: &str = "https://eutils.ncbi.nlm.nih.gov";

fn client(server: &MockServer, oa: OaPolicy) -> AllowlistedClient {
    AllowlistedClient::new("Incisive/test", oa)
        .with_base_override(server.uri())
        .with_retry_backoff(Duration::from_millis(1))
}

fn disallowed(r: Result<String, LitError>) -> String {
    match r {
        Err(LitError::Disallowed(h)) => h,
        other => panic!("expected Disallowed, got {other:?}"),
    }
}

#[tokio::test]
async fn refuses_unlisted_host() {
    let server = MockServer::start().await;
    let c = client(&server, OaPolicy::default());
    assert_eq!(
        disallowed(c.get_text("https://example.com/x").await),
        "example.com"
    );
    // Nothing reached the mock server.
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn refuses_host_tricks() {
    let server = MockServer::start().await;
    let c = client(&server, OaPolicy::default());
    for url in [
        "https://eutils.ncbi.nlm.nih.gov@evil.com/x",
        "https://user:pw@eutils.ncbi.nlm.nih.gov/x",
        "https://eutils.ncbi.nlm.nih.gov.evil.com/x",
        "https://eutils.ncbi.nlm.nih.gov:8443/x",
        "https://127.0.0.1/x",
        "https://[::1]/x",
        "http://eutils.ncbi.nlm.nih.gov/x",
        "ftp://eutils.ncbi.nlm.nih.gov/x",
        "not a url",
    ] {
        let r = c.get_text(url).await;
        assert!(matches!(r, Err(LitError::Disallowed(_))), "{url}: {r:?}");
    }
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn allows_listed_host_case_insensitively_and_explicit_443() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/ok"))
        .respond_with(ResponseTemplate::new(200).set_body_string("fine"))
        .mount(&server)
        .await;
    let c = client(&server, OaPolicy::default());
    assert_eq!(
        c.get_text("https://EUTILS.ncbi.nlm.nih.gov/ok")
            .await
            .unwrap(),
        "fine"
    );
    assert_eq!(
        c.get_text("https://eutils.ncbi.nlm.nih.gov:443/ok")
            .await
            .unwrap(),
        "fine"
    );
}

#[tokio::test]
async fn refuses_http_scheme_for_oa_url() {
    let server = MockServer::start().await;
    let oa = OaPolicy::default();
    oa.allow_once("http://europepmc.org/paper.pdf");
    let c = client(&server, oa);
    assert_eq!(
        disallowed(c.get_text("http://europepmc.org/paper.pdf").await),
        "europepmc.org"
    );
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn size_cap_aborts_download() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/big"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![b'a'; 6 * 1024 * 1024]))
        .mount(&server)
        .await;
    let c = client(&server, OaPolicy::default());
    let r = c
        .get_bytes_capped(&format!("{EUTILS}/big"), 5 * 1024 * 1024)
        .await;
    assert!(matches!(r, Err(LitError::TooLarge)), "{r:?}");
    // Under the cap succeeds.
    let ok = c
        .get_bytes_capped(&format!("{EUTILS}/big"), 7 * 1024 * 1024)
        .await
        .unwrap();
    assert_eq!(ok.len(), 6 * 1024 * 1024);
}

#[tokio::test]
async fn redirect_to_disallowed_host_is_refused() {
    let server = MockServer::start().await;
    let oa = OaPolicy::default();
    oa.allow_once("https://europepmc.org/start");
    Mock::given(method("GET"))
        .and(path("/start"))
        .respond_with(
            ResponseTemplate::new(302).insert_header("location", "https://evil.example/steal"),
        )
        .mount(&server)
        .await;
    let c = client(&server, oa);
    let h = disallowed(c.get_text("https://europepmc.org/start").await);
    assert_eq!(h, "evil.example");
    // Only the first hop was ever requested.
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

#[tokio::test]
async fn redirect_from_base_host_to_oa_host_is_refused_even_if_registered() {
    // allow_once covers only the exact registered URL as the *first* request;
    // a redirect target must pass the base allowlist on its own.
    let server = MockServer::start().await;
    let oa = OaPolicy::default();
    oa.allow_once("https://europepmc.org/target");
    Mock::given(method("GET"))
        .and(path("/start"))
        .respond_with(
            ResponseTemplate::new(302).insert_header("location", "https://europepmc.org/target"),
        )
        .mount(&server)
        .await;
    let c = client(&server, oa);
    let r = c.get_text(&format!("{EUTILS}/start")).await;
    assert!(matches!(r, Err(LitError::Disallowed(_))), "{r:?}");
}

#[tokio::test]
async fn redirects_between_allowed_hosts_are_followed_up_to_three_hops() {
    let server = MockServer::start().await;
    for (from, to) in [("/a", "/b"), ("/b", "/c"), ("/c", "/d")] {
        Mock::given(method("GET"))
            .and(path(from))
            .respond_with(
                ResponseTemplate::new(301)
                    .insert_header("location", format!("https://www.ebi.ac.uk{to}").as_str()),
            )
            .mount(&server)
            .await;
    }
    Mock::given(method("GET"))
        .and(path("/d"))
        .respond_with(ResponseTemplate::new(200).set_body_string("end"))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/loop"))
        .respond_with(
            ResponseTemplate::new(302).insert_header("location", "https://www.ebi.ac.uk/loop"),
        )
        .mount(&server)
        .await;
    let c = client(&server, OaPolicy::default());
    // 3 redirects then the final response: allowed.
    assert_eq!(c.get_text(&format!("{EUTILS}/a")).await.unwrap(), "end");
    // A fourth redirect is refused.
    let r = c.get_text(&format!("{EUTILS}/loop")).await;
    assert!(matches!(r, Err(LitError::Http(302))), "{r:?}");
}

#[tokio::test]
async fn retries_5xx_twice_then_fails() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/down"))
        .respond_with(ResponseTemplate::new(503))
        .mount(&server)
        .await;
    let c = client(&server, OaPolicy::default());
    let r = c.get_text(&format!("{EUTILS}/down")).await;
    assert!(matches!(r, Err(LitError::Http(503))), "{r:?}");
    assert_eq!(server.received_requests().await.unwrap().len(), 3);
}

#[tokio::test]
async fn retry_recovers_and_4xx_is_not_retried() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/flaky"))
        .respond_with(ResponseTemplate::new(500))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/flaky"))
        .respond_with(ResponseTemplate::new(200).set_body_string("ok"))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/missing"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;
    let c = client(&server, OaPolicy::default());
    assert_eq!(c.get_text(&format!("{EUTILS}/flaky")).await.unwrap(), "ok");
    let r = c.get_text(&format!("{EUTILS}/missing")).await;
    assert!(matches!(r, Err(LitError::Http(404))), "{r:?}");
    assert_eq!(server.received_requests().await.unwrap().len(), 3); // 2 flaky + 1 missing
}

#[tokio::test]
async fn timeout_is_retried_then_reported() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/slow"))
        .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_millis(600)))
        .mount(&server)
        .await;
    let c = client(&server, OaPolicy::default()).with_timeout(Duration::from_millis(100));
    let r = c.get_text(&format!("{EUTILS}/slow")).await;
    assert!(matches!(r, Err(LitError::Timeout)), "{r:?}");
    assert_eq!(server.received_requests().await.unwrap().len(), 3);
}

#[tokio::test]
async fn allow_once_permits_only_that_url() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string("pdf"))
        .mount(&server)
        .await;
    let oa = OaPolicy::default();
    oa.allow_once("https://europepmc.org/a.pdf");
    let c = client(&server, oa);
    // A different URL on the same host: refused.
    assert_eq!(
        disallowed(c.get_text("https://europepmc.org/b.pdf").await),
        "europepmc.org"
    );
    // The registered URL works once...
    assert_eq!(
        c.get_text("https://europepmc.org/a.pdf").await.unwrap(),
        "pdf"
    );
    // ...and not a second time.
    assert_eq!(
        disallowed(c.get_text("https://europepmc.org/a.pdf").await),
        "europepmc.org"
    );
}

#[tokio::test]
async fn allow_once_survives_retries_of_the_same_fetch() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(502))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string("pdf"))
        .mount(&server)
        .await;
    let oa = OaPolicy::default();
    oa.allow_once("https://europepmc.org/a.pdf");
    let c = client(&server, oa);
    assert_eq!(
        c.get_text("https://europepmc.org/a.pdf").await.unwrap(),
        "pdf"
    );
}

#[tokio::test]
async fn allow_once_is_atomic_under_parallel_fetches() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string("pdf"))
        .mount(&server)
        .await;
    let oa = OaPolicy::default();
    oa.allow_once("https://europepmc.org/a.pdf");
    let c = std::sync::Arc::new(client(&server, oa));
    let mut tasks = Vec::new();
    for _ in 0..8 {
        let c = c.clone();
        tasks.push(tokio::spawn(async move {
            c.get_text("https://europepmc.org/a.pdf").await
        }));
    }
    let mut ok = 0;
    for t in tasks {
        if t.await.unwrap().is_ok() {
            ok += 1;
        }
    }
    assert_eq!(ok, 1);
}

#[tokio::test]
async fn get_json_parses_and_reports_parse_errors() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/j"))
        .respond_with(ResponseTemplate::new(200).set_body_string(r#"{"a": 1}"#))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/bad"))
        .respond_with(ResponseTemplate::new(200).set_body_string("<html>"))
        .mount(&server)
        .await;
    let c = client(&server, OaPolicy::default());
    let v: serde_json::Value = c.get_json(&format!("{EUTILS}/j")).await.unwrap();
    assert_eq!(v["a"], 1);
    let r: Result<serde_json::Value, _> = c.get_json(&format!("{EUTILS}/bad")).await;
    assert!(matches!(r, Err(LitError::Parse)), "{r:?}");
}

#[tokio::test]
async fn sends_the_user_agent() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(wiremock::matchers::header("user-agent", "Incisive/test"))
        .respond_with(ResponseTemplate::new(200).set_body_string("ua"))
        .mount(&server)
        .await;
    let c = client(&server, OaPolicy::default());
    assert_eq!(c.get_text(&format!("{EUTILS}/x")).await.unwrap(), "ua");
}

#[test]
fn default_user_agent_format() {
    let ua = incisive_app::lit::http::default_user_agent();
    assert!(ua.starts_with("Incisive/"), "{ua}");
    assert!(
        ua.ends_with(" (+https://github.com/manjunathhari/incisive)"),
        "{ua}"
    );
}

fn paper(pmid: Option<&str>, doi: Option<&str>) -> Paper {
    Paper {
        pmid: pmid.map(String::from),
        pmcid: None,
        doi: doi.map(String::from),
        title: "T".into(),
        authors: vec![],
        journal: None,
        year: None,
        pub_types: vec![],
        abstract_text: None,
        oa_url: None,
        retracted: false,
        source: PaperSource::PubMed,
    }
}

#[test]
fn paper_key_prefers_pmid_then_lowercased_doi() {
    assert_eq!(PaperId::key(&paper(Some("123"), Some("10.1/ABC"))), "123");
    assert_eq!(PaperId::key(&paper(None, Some("10.1/ABC"))), "10.1/abc");
    assert_eq!(PaperId::key(&paper(None, None)), "title:t");
}
