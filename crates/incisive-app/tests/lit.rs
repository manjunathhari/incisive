//! Allowlisted HTTP client tests. wiremock only; no network, no database.
//! The test-only `with_base_override` maps every allowlisted host name to the
//! mock server, so the allowlist logic still sees the real host names.

use std::time::Duration;

use incisive_app::lit::http::{AllowlistedClient, FULL_TEXT_MAX, LitError, OaPolicy};
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
    assert!(matches!(r, Err(LitError::TooManyRedirects)), "{r:?}");
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

#[tokio::test]
async fn first_hop_bypass_probes_are_refused() {
    let server = MockServer::start().await;
    let c = client(&server, OaPolicy::default());
    for url in [
        "https://eutils.ncbi.nlm.nih.gov./x",      // trailing dot
        "https://\u{435}utils.ncbi.nlm.nih.gov/x", // Cyrillic e lookalike
        "https://2130706433/x",                    // decimal IP
        "https://0177.0.0.1/x",                    // octal IP
        "https://0x7f.1/x",                        // hex IP
    ] {
        let r = c.get_text(url).await;
        assert!(matches!(r, Err(LitError::Disallowed(_))), "{url}: {r:?}");
    }
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn redirect_location_probes() {
    let server = MockServer::start().await;
    for (from, loc) in [
        ("/proto", "//evil.com/x"),
        ("/userinfo", "https://user@www.ebi.ac.uk/x"),
        ("/downgrade", "http://www.ebi.ac.uk/x"),
        ("/rel", "/landed"),
    ] {
        Mock::given(method("GET"))
            .and(path(from))
            .respond_with(ResponseTemplate::new(302).insert_header("location", loc))
            .mount(&server)
            .await;
    }
    Mock::given(method("GET"))
        .and(path("/landed"))
        .respond_with(ResponseTemplate::new(200).set_body_string("landed"))
        .mount(&server)
        .await;
    let c = client(&server, OaPolicy::default());
    assert_eq!(
        disallowed(c.get_text(&format!("{EUTILS}/proto")).await),
        "evil.com"
    );
    for p in ["userinfo", "downgrade"] {
        let r = c.get_text(&format!("{EUTILS}/{p}")).await;
        assert!(matches!(r, Err(LitError::Disallowed(_))), "{p}: {r:?}");
    }
    // A relative Location resolves against the logical URL, is re-checked, and is followed.
    assert_eq!(
        c.get_text(&format!("{EUTILS}/rel")).await.unwrap(),
        "landed"
    );
}

#[tokio::test]
async fn network_errors_are_retried_then_reported() {
    // A server that accepts and immediately drops every connection.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let accepts = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
    let n = accepts.clone();
    tokio::spawn(async move {
        loop {
            if let Ok((sock, _)) = listener.accept().await {
                n.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                drop(sock);
            }
        }
    });
    let c = AllowlistedClient::new("Incisive/test", OaPolicy::default())
        .with_base_override(format!("http://{addr}"))
        .with_retry_backoff(Duration::from_millis(1));
    let r = c.get_text(&format!("{EUTILS}/x")).await;
    assert!(matches!(r, Err(LitError::Network)), "{r:?}");
    assert_eq!(accepts.load(std::sync::atomic::Ordering::SeqCst), 3);
}

#[tokio::test]
async fn granted_url_is_capped_at_full_text_max() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![b'a'; 6 * 1024 * 1024]))
        .mount(&server)
        .await;
    assert_eq!(FULL_TEXT_MAX, 5 * 1024 * 1024);
    let oa = OaPolicy::default();
    oa.allow_once("https://europepmc.org/a.pdf");
    oa.allow_once("https://europepmc.org/b.pdf");
    let c = client(&server, oa);
    // get_text allows 8 MB for base hosts, but a granted URL is clamped to 5 MB.
    let r = c.get_text("https://europepmc.org/a.pdf").await;
    assert!(matches!(r, Err(LitError::TooLarge)), "{r:?}");
    // Even a larger explicit cap is clamped.
    let r = c
        .get_bytes_capped("https://europepmc.org/b.pdf", 10 * 1024 * 1024)
        .await;
    assert!(matches!(r, Err(LitError::TooLarge)), "{r:?}");
    // A base host with the same body is fine under the 8 MB default.
    assert_eq!(
        c.get_text(&format!("{EUTILS}/x")).await.unwrap().len(),
        6 * 1024 * 1024
    );
}

#[tokio::test]
async fn too_many_redirects_is_its_own_error() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(301).insert_header("location", "https://www.ebi.ac.uk/loop"),
        )
        .mount(&server)
        .await;
    let c = client(&server, OaPolicy::default());
    let r = c.get_text(&format!("{EUTILS}/loop")).await;
    assert!(matches!(r, Err(LitError::TooManyRedirects)), "{r:?}");
}

#[tokio::test]
async fn grant_survives_an_exhausted_retryable_failure() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(503))
        .up_to_n_times(3)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string("pdf"))
        .mount(&server)
        .await;
    let oa = OaPolicy::default();
    oa.allow_once("https://europepmc.org/a.pdf");
    let c = client(&server, oa);
    let r = c.get_text("https://europepmc.org/a.pdf").await;
    assert!(matches!(r, Err(LitError::Http(503))), "{r:?}");
    // Not consumed by the failed fetch: a later attempt succeeds, once.
    assert_eq!(
        c.get_text("https://europepmc.org/a.pdf").await.unwrap(),
        "pdf"
    );
    assert!(c.get_text("https://europepmc.org/a.pdf").await.is_err());
}

#[tokio::test]
async fn grant_set_is_bounded_and_expires() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string("pdf"))
        .mount(&server)
        .await;
    // Capacity: the oldest grant is dropped.
    let oa = OaPolicy::with_limits(2, Duration::from_secs(3600));
    oa.allow_once("https://europepmc.org/1.pdf");
    oa.allow_once("https://europepmc.org/2.pdf");
    oa.allow_once("https://europepmc.org/3.pdf");
    let c = client(&server, oa);
    assert!(c.get_text("https://europepmc.org/1.pdf").await.is_err());
    assert!(c.get_text("https://europepmc.org/2.pdf").await.is_ok());
    assert!(c.get_text("https://europepmc.org/3.pdf").await.is_ok());
    // TTL: an old grant is refused.
    let oa = OaPolicy::with_limits(10, Duration::from_millis(20));
    oa.allow_once("https://europepmc.org/old.pdf");
    tokio::time::sleep(Duration::from_millis(60)).await;
    let c = client(&server, oa);
    assert!(c.get_text("https://europepmc.org/old.pdf").await.is_err());
    // Defaults.
    assert_eq!(incisive_app::lit::http::MAX_GRANTS, 1000);
}

// ---------------------------------------------------------------------------
// PubMed client and paper cache (Task 6)
// ---------------------------------------------------------------------------

mod pubmed_tests {
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    use incisive_app::db::{lazy_pool, migrate};
    use incisive_app::lit::cache;
    use incisive_app::lit::http::{AllowlistedClient, LitError, OaPolicy};
    use incisive_app::lit::pubmed::{PubMed, default_min_interval, parse_efetch};
    use incisive_app::lit::{Paper, PaperSource};
    use secrecy::SecretString;
    use wiremock::matchers::{method, path, query_param, query_param_is_missing};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const ESEARCH: &str = "/entrez/eutils/esearch.fcgi";
    const ESUMMARY: &str = "/entrez/eutils/esummary.fcgi";
    const EFETCH: &str = "/entrez/eutils/efetch.fcgi";

    fn fixture(name: &str) -> String {
        let p = format!("{}/tests/fixtures/lit/{name}", env!("CARGO_MANIFEST_DIR"));
        std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{p}: {e}"))
    }

    fn pubmed(server: &MockServer, key: Option<&str>) -> PubMed {
        let client = AllowlistedClient::new("Incisive/test", OaPolicy::default())
            .with_base_override(server.uri())
            .with_retry_backoff(Duration::from_millis(1));
        PubMed::new(client, key.map(SecretString::from)).with_min_interval(Duration::ZERO)
    }

    async fn serve(server: &MockServer, route: &str, body: String, ct: &str) {
        Mock::given(method("GET"))
            .and(path(route))
            .respond_with(ResponseTemplate::new(200).set_body_raw(body, ct))
            .mount(server)
            .await;
    }

    fn ids(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[tokio::test]
    async fn parses_esearch_count_and_ids() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(ESEARCH))
            .and(query_param("db", "pubmed"))
            .and(query_param("retmode", "json"))
            .and(query_param("sort", "relevance"))
            .and(query_param("retmax", "20"))
            .and(query_param("tool", "incisive"))
            .and(query_param(
                "term",
                "indocyanine green[tiab] AND leak[tiab]",
            ))
            .and(query_param_is_missing("api_key"))
            .and(query_param_is_missing("email"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_raw(fixture("pubmed_esearch_icg.json"), "application/json"),
            )
            .expect(1)
            .mount(&server)
            .await;
        let hits = pubmed(&server, None)
            .search("indocyanine green[tiab] AND leak[tiab]", 20)
            .await
            .unwrap();
        assert_eq!(hits.count, 126);
        assert_eq!(hits.pmids.len(), 20);
        assert_eq!(hits.pmids[0], "40042831");
        assert_eq!(hits.pmids[19], "33642951");
    }

    #[tokio::test]
    async fn esearch_with_no_results_is_empty_not_error() {
        let server = MockServer::start().await;
        serve(
            &server,
            ESEARCH,
            r#"{"header":{},"esearchresult":{"count":"0","retmax":"0","retstart":"0","idlist":[]}}"#
                .into(),
            "application/json",
        )
        .await;
        let hits = pubmed(&server, None).search("zzz", 5).await.unwrap();
        assert_eq!(hits.count, 0);
        assert!(hits.pmids.is_empty());
    }

    #[tokio::test]
    async fn malformed_esearch_is_parse_error() {
        let server = MockServer::start().await;
        serve(&server, ESEARCH, "<html>oops</html>".into(), "text/html").await;
        assert_eq!(
            pubmed(&server, None).search("x", 5).await.unwrap_err(),
            LitError::Parse
        );
    }

    #[tokio::test]
    async fn email_is_sent_when_configured() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(ESEARCH))
            .and(query_param("email", "ops@example.com"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                r#"{"esearchresult":{"count":"0","idlist":[]}}"#,
                "application/json",
            ))
            .expect(1)
            .mount(&server)
            .await;
        pubmed(&server, None)
            .with_email(Some("ops@example.com".into()))
            .search("x", 5)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn parses_esummary_into_papers() {
        let server = MockServer::start().await;
        serve(
            &server,
            ESUMMARY,
            fixture("pubmed_esummary_icg.json"),
            "application/json",
        )
        .await;
        let want = ids(&["40042831", "41871586", "40690925", "33820705", "38577071"]);
        let papers = pubmed(&server, None).summaries(&want).await.unwrap();
        assert_eq!(papers.len(), 5);
        let p = &papers[0];
        assert_eq!(p.pmid.as_deref(), Some("40042831"));
        assert_eq!(p.pmcid.as_deref(), Some("PMC11883591"));
        assert_eq!(p.doi.as_deref(), Some("10.1001/jamasurg.2025.0006"));
        assert!(
            p.title
                .starts_with("Indocyanine Green Fluorescence Imaging")
        );
        assert_eq!(p.authors.len(), 10);
        assert_eq!(p.authors[0], "Rinne JKA");
        assert_eq!(p.authors[9], "Kössi J");
        assert_eq!(p.year, Some(2025));
        assert!(p.journal.as_deref().unwrap().contains("JAMA"));
        assert!(
            p.pub_types
                .contains(&"Randomized Controlled Trial".to_string())
        );
        assert!(p.abstract_text.is_none());
        assert!(p.oa_url.is_none());
        assert!(!p.retracted);
        assert_eq!(p.source, PaperSource::PubMed);
        // No PMC id for this one.
        assert_eq!(papers[1].pmcid, None);
        assert_eq!(papers[1].year, Some(2026));
        assert_eq!(papers[4].pmcid.as_deref(), Some("PMC10989327"));
    }

    #[tokio::test]
    async fn missing_pmid_is_absent_not_error() {
        let server = MockServer::start().await;
        // Real NCBI shape: an unknown id is listed in `uids` with an `error` field.
        serve(
            &server,
            ESUMMARY,
            fixture("pubmed_esummary_missing.json"),
            "application/json",
        )
        .await;
        let papers = pubmed(&server, None)
            .summaries(&ids(&["40042831", "99999999"]))
            .await
            .unwrap();
        assert_eq!(papers.len(), 1);
        assert_eq!(papers[0].pmid.as_deref(), Some("40042831"));

        // Abstracts: an unknown id gives an empty article set (real NCBI shape).
        let server = MockServer::start().await;
        serve(
            &server,
            EFETCH,
            fixture("pubmed_efetch_icg.xml"),
            "text/xml",
        )
        .await;
        let abs = pubmed(&server, None)
            .abstracts(&ids(&["40042831", "99999999"]))
            .await
            .unwrap();
        assert!(abs.contains_key("40042831"));
        assert!(!abs.contains_key("99999999"));

        // No ids at all: no request, empty result.
        let server = MockServer::start().await;
        assert!(
            pubmed(&server, None)
                .summaries(&[])
                .await
                .unwrap()
                .is_empty()
        );
        assert!(
            pubmed(&server, None)
                .abstracts(&[])
                .await
                .unwrap()
                .is_empty()
        );
        assert!(server.received_requests().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn parses_structured_abstract_sections() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(EFETCH))
            .and(query_param("db", "pubmed"))
            .and(query_param("rettype", "abstract"))
            .and(query_param("retmode", "xml"))
            .and(query_param(
                "id",
                "40042831,41871586,40690925,33820705,38577071",
            ))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_raw(fixture("pubmed_efetch_icg.xml"), "text/xml"),
            )
            .expect(1)
            .mount(&server)
            .await;
        let abs = pubmed(&server, None)
            .abstracts(&ids(&[
                "40042831", "41871586", "40690925", "33820705", "38577071",
            ]))
            .await
            .unwrap();
        assert_eq!(abs.len(), 5);

        let a = &abs["40042831"];
        assert!(a.starts_with("IMPORTANCE: "), "{a}");
        let sections: Vec<&str> = a.split("\n\n").collect();
        assert_eq!(sections.len(), 8);
        assert!(sections[1].starts_with("OBJECTIVE: "));
        assert!(sections[2].starts_with("DESIGN, SETTING, AND PARTICIPANTS: "));
        assert!(sections.iter().all(|s| !s.trim().is_empty()));
        assert!(
            !a.contains('<') && !a.contains("&#") && !a.contains("&amp;"),
            "{a}"
        );

        // Unstructured abstract: plain text, no label prefix, no section breaks.
        let plain = &abs["38577071"];
        assert!(!plain.contains("\n\n"));
        // Inline markup (<i>) is flattened, text kept.
        assert!(!plain.contains("<i>") && !plain.contains("</i>"));
        assert!(plain.contains("In this editorial we comment on the article by"));
    }

    #[tokio::test]
    async fn entities_are_unescaped() {
        let xml = r#"<?xml version="1.0" ?>
<!DOCTYPE PubmedArticleSet PUBLIC "-//NLM//DTD PubMedArticle, 1st January 2025//EN" "x.dtd">
<PubmedArticleSet><PubmedArticle><MedlineCitation><PMID Version="1">1</PMID><Article>
<Abstract><AbstractText Label="R&amp;D">p &lt; 0.05 &amp; HR&#x2009;=&#8201;0.5, 5 &#177; 2 &gt; 1</AbstractText></Abstract>
</Article></MedlineCitation></PubmedArticle></PubmedArticleSet>"#;
        let recs = parse_efetch(xml).unwrap();
        assert_eq!(recs.len(), 1);
        assert_eq!(
            recs[0].abstract_text.as_deref(),
            Some("R&D: p < 0.05 & HR\u{2009}=\u{2009}0.5, 5 \u{b1} 2 > 1")
        );
    }

    #[tokio::test]
    async fn detects_retraction() {
        // esummary: the pub type says so.
        let server = MockServer::start().await;
        serve(
            &server,
            ESUMMARY,
            fixture("pubmed_esummary_retracted.json"),
            "application/json",
        )
        .await;
        let papers = pubmed(&server, None)
            .summaries(&ids(&["35422152"]))
            .await
            .unwrap();
        assert!(
            papers[0]
                .pub_types
                .iter()
                .any(|t| t == "Retracted Publication")
        );
        assert!(papers[0].retracted);

        // efetch: real record, both signals.
        let recs = parse_efetch(&fixture("pubmed_efetch_retracted.xml")).unwrap();
        assert_eq!(recs.len(), 1);
        assert_eq!(recs[0].pmid, "35422152");
        assert!(recs[0].retracted);
        assert!(
            recs[0]
                .abstract_text
                .as_deref()
                .unwrap()
                .starts_with("Laser technology")
        );

        // Ordinary records, including ones with CommentIn/ErratumIn notices, are not retracted.
        for f in ["pubmed_efetch_icg.xml", "pubmed_efetch_hernia.xml"] {
            for r in parse_efetch(&fixture(f)).unwrap() {
                assert!(!r.retracted, "{f} {}", r.pmid);
            }
        }

        // Synthetic: only the RetractionIn comment (no pub type) is enough.
        let only_comment = r#"<PubmedArticleSet><PubmedArticle><MedlineCitation><PMID>7</PMID>
<Article><PublicationTypeList><PublicationType UI="D016428">Journal Article</PublicationType></PublicationTypeList></Article>
<CommentsCorrectionsList><CommentsCorrections RefType="RetractionIn"><RefSource>X</RefSource><PMID Version="1">8</PMID></CommentsCorrections></CommentsCorrectionsList>
</MedlineCitation></PubmedArticle></PubmedArticleSet>"#;
        let recs = parse_efetch(only_comment).unwrap();
        assert_eq!(
            recs[0].pmid, "7",
            "the PMID inside the comment must not win"
        );
        assert!(recs[0].retracted);

        // Synthetic: only the pub type.
        let only_type = r#"<PubmedArticleSet><PubmedArticle><MedlineCitation><PMID>9</PMID>
<Article><PublicationTypeList><PublicationType>Retracted Publication</PublicationType></PublicationTypeList></Article>
</MedlineCitation></PubmedArticle></PubmedArticleSet>"#;
        assert!(parse_efetch(only_type).unwrap()[0].retracted);

        // "Retraction of Publication" is the notice itself, not a retracted paper.
        let notice = r#"<PubmedArticleSet><PubmedArticle><MedlineCitation><PMID>10</PMID>
<Article><PublicationTypeList><PublicationType>Retraction of Publication</PublicationType></PublicationTypeList></Article>
<CommentsCorrectionsList><CommentsCorrections RefType="RetractionOf"/></CommentsCorrectionsList>
</MedlineCitation></PubmedArticle></PubmedArticleSet>"#;
        assert!(!parse_efetch(notice).unwrap()[0].retracted);
    }

    #[tokio::test]
    async fn retraction_flows_through_abstracts_call() {
        // `abstracts` returns the abstract only; `details` carries the retraction flag too.
        let server = MockServer::start().await;
        serve(
            &server,
            EFETCH,
            fixture("pubmed_efetch_retracted.xml"),
            "text/xml",
        )
        .await;
        let d = pubmed(&server, None)
            .details(&ids(&["35422152"]))
            .await
            .unwrap();
        assert!(d["35422152"].retracted);
    }

    #[tokio::test]
    async fn unexpected_xml_is_parse_error_not_panic() {
        for bad in [
            "",
            "not xml at all",
            "<PubmedArticleSet><PubmedArticle><MedlineCitation><PMID>1</PMID>", // truncated
            "<a><b></a>",
            "<PubmedArticleSet><PubmedArticle><Abstract><AbstractText>x&bogus;y</AbstractText>",
            "\u{0}\u{1}<<<>>>",
        ] {
            let r = std::panic::catch_unwind(|| parse_efetch(bad));
            let r = r.unwrap_or_else(|_| panic!("panicked on {bad:?}"));
            assert!(
                matches!(r, Err(LitError::Parse)) || r.as_ref().is_ok_and(|v| v.is_empty()),
                "{bad:?} -> {r:?}"
            );
        }
        // A well-formed set with no articles is fine and empty.
        assert!(
            parse_efetch("<PubmedArticleSet></PubmedArticleSet>")
                .unwrap()
                .is_empty()
        );
    }

    #[derive(Clone, Default)]
    struct Capture(Arc<Mutex<Vec<u8>>>);
    impl std::io::Write for Capture {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(b);
            Ok(b.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn api_key_is_sent_when_set_and_never_logged() {
        const KEY: &str = "k3y-SECRET-0123456789abcdef";
        let logs = Capture::default();
        let writer = logs.clone();
        let _guard = tracing::subscriber::set_default(
            tracing_subscriber::fmt()
                .with_max_level(tracing::Level::TRACE)
                .with_writer(move || writer.clone())
                .finish(),
        );

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(ESEARCH))
            .and(query_param("api_key", KEY))
            .and(query_param("tool", "incisive"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                r#"{"esearchresult":{"count":"1","idlist":["1"]}}"#,
                "application/json",
            ))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path(ESUMMARY))
            .and(query_param("api_key", KEY))
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;
        let pm = pubmed(&server, Some(KEY)).with_email(Some("ops@example.com".into()));
        pm.search("x", 5).await.unwrap();

        // A failing call: neither the error nor its Display carries the key.
        let err = pm.summaries(&ids(&["1"])).await.unwrap_err();
        assert!(matches!(err, LitError::Http(500)), "{err:?}");
        assert!(!format!("{err} {err:?}").contains(KEY));

        // Debug of the client does not reveal it.
        let dbg = format!("{pm:?}");
        assert!(!dbg.contains(KEY), "{dbg}");

        // And nothing written to the log mentions it.
        let logged = String::from_utf8_lossy(&logs.0.lock().unwrap()).into_owned();
        assert!(!logged.contains(KEY), "{logged}");
        assert!(!logged.contains("k3y-SECRET"), "{logged}");
    }

    #[test]
    fn pacing_defaults_are_3_per_second_without_a_key_and_10_with_one() {
        assert_eq!(default_min_interval(false), Duration::from_millis(334));
        assert_eq!(default_min_interval(true), Duration::from_millis(100));
    }

    #[tokio::test]
    async fn requests_are_paced() {
        let server = MockServer::start().await;
        serve(
            &server,
            ESEARCH,
            r#"{"esearchresult":{"count":"0","idlist":[]}}"#.into(),
            "application/json",
        )
        .await;
        let pm = pubmed(&server, None).with_min_interval(Duration::from_millis(60));
        let start = Instant::now();
        for _ in 0..4 {
            pm.search("x", 1).await.unwrap();
        }
        // 4 requests need 3 gaps of 60 ms.
        assert!(
            start.elapsed() >= Duration::from_millis(180),
            "{:?}",
            start.elapsed()
        );
        // Concurrent callers share the same pacer.
        let start = Instant::now();
        let (a, b, c) = tokio::join!(pm.search("x", 1), pm.search("x", 1), pm.search("x", 1));
        a.unwrap();
        b.unwrap();
        c.unwrap();
        assert!(
            start.elapsed() >= Duration::from_millis(120),
            "{:?}",
            start.elapsed()
        );
    }

    // ----- cache (DB-gated) -----

    async fn pool() -> Option<sqlx::PgPool> {
        let Ok(url) = std::env::var("TEST_DATABASE_URL") else {
            eprintln!("SKIP: TEST_DATABASE_URL not set");
            return None;
        };
        let pool = lazy_pool(&SecretString::from(url)).expect("lazy pool");
        migrate(&pool).await.expect("migrate");
        Some(pool)
    }

    fn paper(pmid: &str, title: &str) -> Paper {
        Paper {
            pmid: Some(pmid.into()),
            pmcid: None,
            doi: Some(format!("10.1000/{pmid}")),
            title: title.into(),
            authors: vec!["Rinne JKA".into(), "Köss\u{ed}, J; Jr".into()],
            journal: Some("JAMA Surg".into()),
            year: Some(2025),
            pub_types: vec![
                "Journal Article".into(),
                "Randomized Controlled Trial".into(),
            ],
            abstract_text: Some("BACKGROUND: x\n\nMETHODS: y".into()),
            oa_url: Some("https://europepmc.org/articles/PMC1?pdf=render".into()),
            retracted: false,
            source: PaperSource::PubMed,
        }
    }

    #[tokio::test]
    async fn cache_round_trip_and_staleness() {
        let Some(pool) = pool().await else { return };
        // Unique numeric-looking ids so parallel runs and reruns never collide.
        let tag = uuid::Uuid::new_v4().simple().to_string();
        let fresh = format!("t6-fresh-{tag}");
        let aging = format!("t6-aging-{tag}");
        let old = format!("t6-old-{tag}");
        let gone = format!("t6-gone-{tag}");
        let mut retracted = paper(&format!("t6-retr-{tag}"), "Retracted one");
        retracted.retracted = true;
        retracted.abstract_text = None;
        retracted.oa_url = None;
        retracted.doi = None;

        cache::put_many(
            &pool,
            &[
                paper(&fresh, "Fresh"),
                paper(&aging, "Aging"),
                paper(&old, "Old"),
                retracted.clone(),
                Paper {
                    pmid: None,
                    ..paper("x", "No pmid is skipped")
                },
            ],
        )
        .await
        .unwrap();
        cache::put_many(&pool, &[]).await.unwrap();

        let set_age = |pmid: String, days: i32| {
            let pool = pool.clone();
            async move {
                sqlx::query(
                    "UPDATE incisive.paper_cache SET fetched_at = now() - make_interval(days => $2) WHERE pmid = $1",
                )
                .bind(pmid)
                .bind(days)
                .execute(&pool)
                .await
                .unwrap();
            }
        };
        set_age(aging.clone(), 29).await;
        set_age(old.clone(), 31).await;

        let want = vec![
            fresh.clone(),
            aging.clone(),
            old.clone(),
            retracted.pmid.clone().unwrap(),
            gone.clone(),
        ];
        let got = cache::get_many(&pool, &want).await.unwrap();
        let by: std::collections::HashMap<_, _> =
            got.iter().map(|p| (p.pmid.clone().unwrap(), p)).collect();
        // Fresh and 29-day-old are returned; 31-day-old (stale) and never-cached are not.
        assert!(by.contains_key(&fresh));
        assert!(by.contains_key(&aging));
        assert!(!by.contains_key(&old), "stale entries are not served");
        assert!(!by.contains_key(&gone));
        assert_eq!(got.len(), 3);

        // Full round trip of every stored field.
        let mut expect = paper(&fresh, "Fresh");
        expect.pmcid = None; // not stored by the 0005 schema
        assert_eq!(by[&fresh], &expect);
        assert_eq!(by[retracted.pmid.as_deref().unwrap()], &retracted);
        assert!(by[retracted.pmid.as_deref().unwrap()].retracted);

        // put_many refreshes an existing row (and overwrites its content).
        let mut updated = paper(&old, "Old, refetched");
        updated.retracted = true;
        cache::put_many(&pool, &[updated]).await.unwrap();
        let got = cache::get_many(&pool, std::slice::from_ref(&old))
            .await
            .unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].title, "Old, refetched");
        assert!(got[0].retracted);

        // Empty input.
        assert!(cache::get_many(&pool, &[]).await.unwrap().is_empty());

        // prune: removes rows older than 90 days only.
        set_age(fresh.clone(), 91).await;
        set_age(aging.clone(), 89).await;
        let removed = cache::prune(&pool).await.unwrap();
        assert!(removed >= 1);
        let left: Vec<String> =
            sqlx::query_scalar("SELECT pmid FROM incisive.paper_cache WHERE pmid = ANY($1)")
                .bind(vec![fresh.clone(), aging.clone()])
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(
            left,
            vec![aging.clone()],
            "91-day row pruned, 89-day row kept"
        );

        // Clean up our rows.
        sqlx::query("DELETE FROM incisive.paper_cache WHERE pmid LIKE $1")
            .bind(format!("t6-%-{tag}"))
            .execute(&pool)
            .await
            .unwrap();
    }
}
