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
        oa_is_pdf: false,
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
            pmcid: Some("PMC123".into()),
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
            oa_url: Some("https://europepmc.org/articles/PMC1".into()),
            oa_is_pdf: false,
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
        expect.pmcid = Some("PMC123".into()); // stored since migration 0007
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

mod fix_round_1 {
    use std::time::{Duration, Instant};

    use incisive_app::db::{lazy_pool, migrate};
    use incisive_app::lit::cache;
    use incisive_app::lit::http::{AllowlistedClient, OaPolicy};
    use incisive_app::lit::pubmed::{PubMed, parse_efetch};
    use incisive_app::lit::{Paper, PaperSource};
    use secrecy::SecretString;
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn pubmed(server: &MockServer) -> PubMed {
        let client = AllowlistedClient::new("Incisive/test", OaPolicy::default())
            .with_base_override(server.uri())
            .with_retry_backoff(Duration::from_millis(1));
        PubMed::new(client, None)
    }

    #[tokio::test]
    async fn retry_attempts_are_spaced_by_the_gap() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/entrez/eutils/esearch.fcgi"))
            .respond_with(ResponseTemplate::new(503))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/entrez/eutils/esearch.fcgi"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                r#"{"esearchresult":{"count":"0","idlist":[]}}"#,
                "application/json",
            ))
            .mount(&server)
            .await;
        let gap = Duration::from_millis(200);
        let pm = pubmed(&server).with_min_interval(gap);
        // Use the first (immediate) call as the reference slot.
        let t0 = Instant::now();
        pm.search("x", 1).await.unwrap(); // attempt 1 (503) at ~0, attempt 2 at >= gap
        assert!(
            t0.elapsed() >= gap,
            "retry was not paced: {:?}",
            t0.elapsed()
        );
        assert_eq!(server.received_requests().await.unwrap().len(), 2);
    }

    #[tokio::test]
    async fn hostile_query_is_encoded_inside_term() {
        let server = MockServer::start().await;
        let hostile = r#"a&api_key=x "b" [mh]"#;
        Mock::given(method("GET"))
            .and(path("/entrez/eutils/esearch.fcgi"))
            .and(query_param("term", hostile))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                r#"{"esearchresult":{"count":"0","idlist":[]}}"#,
                "application/json",
            ))
            .expect(1)
            .mount(&server)
            .await;
        pubmed(&server).search(hostile, 3).await.unwrap();
        let reqs = server.received_requests().await.unwrap();
        let keys: Vec<String> = reqs[0]
            .url
            .query_pairs()
            .map(|(k, _)| k.into_owned())
            .collect();
        assert!(!keys.contains(&"api_key".to_string()), "{keys:?}");
        assert_eq!(keys.iter().filter(|k| *k == "term").count(), 1);
        assert_eq!(
            keys.len(),
            6,
            "db, term, retmax, sort, retmode, tool: {keys:?}"
        );
    }

    #[test]
    fn record_without_pmid_is_skipped_rest_parses() {
        let xml = r#"<PubmedArticleSet>
<PubmedArticle><MedlineCitation><Article><Abstract><AbstractText>orphan</AbstractText></Abstract></Article></MedlineCitation></PubmedArticle>
<PubmedArticle><MedlineCitation><PMID>5</PMID><Article><Abstract><AbstractText>kept&nbsp;ok</AbstractText></Abstract></Article></MedlineCitation></PubmedArticle>
</PubmedArticleSet>"#;
        let recs = parse_efetch(xml).unwrap();
        assert_eq!(recs.len(), 1);
        assert_eq!(recs[0].pmid, "5");
        assert_eq!(recs[0].abstract_text.as_deref(), Some("kept ok"));
    }

    #[tokio::test]
    async fn pmcid_idtype_is_accepted() {
        let server = MockServer::start().await;
        let body = r#"{"result":{"uids":["1","2"],
"1":{"uid":"1","title":"A","pubdate":"2020","articleids":[{"idtype":"pmcid","value":"pmc-id: PMC777;"}]},
"2":{"uid":"2","title":"B","pubdate":"2020","articleids":[{"idtype":"pmc","value":"PMC888"},{"idtype":"pmcid","value":"pmc-id: PMC888;"}]}}}"#;
        Mock::given(method("GET"))
            .and(path("/entrez/eutils/esummary.fcgi"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(body, "application/json"))
            .mount(&server)
            .await;
        let _ = SecretString::from("unused");
        let ps = pubmed(&server)
            .summaries(&["1".to_string(), "2".to_string()])
            .await
            .unwrap();
        assert_eq!(ps[0].pmcid.as_deref(), Some("PMC777"));
        assert_eq!(ps[1].pmcid.as_deref(), Some("PMC888"));
    }

    fn paper(pmid: &str) -> Paper {
        Paper {
            pmid: Some(pmid.into()),
            pmcid: None,
            doi: Some("10.1/x".into()),
            title: "T".into(),
            authors: vec![],
            journal: None,
            year: Some(2024),
            pub_types: vec![],
            abstract_text: Some("BACKGROUND: kept".into()),
            oa_url: Some("https://europepmc.org/x".into()),
            oa_is_pdf: false,
            retracted: true,
            source: PaperSource::PubMed,
        }
    }

    #[tokio::test]
    async fn refresh_keeps_cached_data_and_retraction_is_sticky() {
        let Ok(url) = std::env::var("TEST_DATABASE_URL") else {
            eprintln!("SKIP: TEST_DATABASE_URL not set");
            return;
        };
        let pool = lazy_pool(&SecretString::from(url)).unwrap();
        migrate(&pool).await.unwrap();
        let id = format!("t6fix-{}", uuid::Uuid::new_v4().simple());
        cache::put_many(&pool, &[paper(&id)]).await.unwrap();

        // Summary-only refresh: no abstract, no oa_url, no doi, not retracted.
        let mut refresh = paper(&id);
        refresh.abstract_text = None;
        refresh.oa_url = None;
        refresh.doi = None;
        refresh.retracted = false;
        refresh.title = "T2".into();
        cache::put_many(&pool, &[refresh]).await.unwrap();

        let got = cache::get_many(&pool, std::slice::from_ref(&id))
            .await
            .unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].title, "T2", "fresh fields still update");
        assert_eq!(got[0].abstract_text.as_deref(), Some("BACKGROUND: kept"));
        assert_eq!(got[0].oa_url.as_deref(), Some("https://europepmc.org/x"));
        assert_eq!(got[0].doi.as_deref(), Some("10.1/x"));
        assert!(got[0].retracted, "retracted stays true once set");

        // New non-null values do replace.
        let mut upd = paper(&id);
        upd.abstract_text = Some("NEW".into());
        cache::put_many(&pool, &[upd]).await.unwrap();
        let got = cache::get_many(&pool, std::slice::from_ref(&id))
            .await
            .unwrap();
        assert_eq!(got[0].abstract_text.as_deref(), Some("NEW"));

        sqlx::query("DELETE FROM incisive.paper_cache WHERE pmid = $1")
            .bind(&id)
            .execute(&pool)
            .await
            .unwrap();
    }
}

// ---------------------------------------------------------------------------
// Europe PMC, Unpaywall and the merge (Task 7)
// ---------------------------------------------------------------------------

mod epmc_unpaywall_tests {
    use std::time::Duration;

    use incisive_app::db::{lazy_pool, migrate};
    use incisive_app::lit::cache;
    use incisive_app::lit::europepmc::{
        EuropePmc, FULL_TEXT_CHARS, full_text_from_xml, merge_results,
    };
    use incisive_app::lit::http::{AllowlistedClient, LitError, OaPolicy};
    use incisive_app::lit::unpaywall::Unpaywall;
    use incisive_app::lit::{OaLink, Paper, PaperSource, url_is_pdf};
    use secrecy::SecretString;
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const SEARCH: &str = "/europepmc/webservices/rest/search";
    const TOPIC1: &str = r#"(indocyanine green OR ICG) AND "anastomotic leak" AND colorectal"#;

    fn fixture(name: &str) -> String {
        let p = format!("{}/tests/fixtures/lit/{name}", env!("CARGO_MANIFEST_DIR"));
        std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{p}: {e}"))
    }

    fn client(server: &MockServer, oa: &OaPolicy) -> AllowlistedClient {
        AllowlistedClient::new("Incisive/test", oa.clone())
            .with_base_override(server.uri())
            .with_retry_backoff(Duration::from_millis(1))
    }

    fn epmc(server: &MockServer, oa: &OaPolicy) -> EuropePmc {
        EuropePmc::new(client(server, oa), oa.clone())
    }

    fn unpaywall(server: &MockServer, oa: &OaPolicy) -> Unpaywall {
        Unpaywall::new(client(server, oa), oa.clone(), "ci@example.invalid".into())
    }

    async fn serve_json(server: &MockServer, route: &str, body: String) {
        Mock::given(method("GET"))
            .and(path(route))
            .respond_with(ResponseTemplate::new(200).set_body_raw(body, "application/json"))
            .mount(server)
            .await;
    }

    // ----- Europe PMC search -----

    #[tokio::test]
    async fn epmc_search_parses_ids_and_oa_flag() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(SEARCH))
            .and(query_param("query", TOPIC1))
            .and(query_param("resultType", "core"))
            .and(query_param("format", "json"))
            .and(query_param("pageSize", "10"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_raw(fixture("epmc_search_icg.json"), "application/json"),
            )
            .expect(1)
            .mount(&server)
            .await;
        let oa = OaPolicy::default();
        let papers = epmc(&server, &oa).search(TOPIC1, 10).await.unwrap();
        assert_eq!(papers.len(), 10);
        assert!(papers.iter().all(|p| p.source == PaperSource::EuropePmc));

        // MED record, open access, with a PMCID.
        let p = papers
            .iter()
            .find(|p| p.pmid.as_deref() == Some("42778806"))
            .unwrap();
        assert_eq!(p.pmcid.as_deref(), Some("PMC13601099"));
        assert_eq!(p.doi.as_deref(), Some("10.1007/s10151-026-03388-4"));
        assert_eq!(p.journal.as_deref(), Some("Techniques in coloproctology"));
        assert_eq!(p.year, Some(2026));
        assert!(
            p.title
                .starts_with("Indocyanine green fluorescence angiograp")
        );
        assert!(!p.authors.is_empty());
        assert!(p.pub_types.contains(&"Meta-Analysis".to_string()));
        assert!(!p.retracted);
        let abs = p.abstract_text.as_deref().unwrap();
        assert!(
            abs.contains("Anastomotic leak (AL) is a major driver"),
            "{abs}"
        );
        assert!(!abs.contains('<'), "markup is stripped: {abs}");
        let url = p.oa_url.as_deref().unwrap();
        assert!(url.starts_with("https://"), "{url}");
        assert!(!p.oa_is_pdf, "HTML is preferred to the PDF entries");

        // Closed MED record: no PMCID, no OA url.
        let closed = papers
            .iter()
            .find(|p| p.pmid.as_deref() == Some("42891491"))
            .unwrap();
        assert_eq!(closed.pmcid, None);
        assert_eq!(closed.oa_url, None);
        // PMC-only record (no PMID, no DOI).
        let pmc_only = papers
            .iter()
            .find(|p| p.pmcid.as_deref() == Some("PMC13675078"))
            .unwrap();
        assert_eq!(pmc_only.pmid, None);
        assert_eq!(pmc_only.doi, None);
        assert!(pmc_only.oa_url.is_some());
        // Preprint with neither.
        assert!(papers.iter().any(|p| p.pmid.is_none()
            && p.pmcid.is_none()
            && p.oa_url.is_none()
            && p.doi.is_some()));
    }

    #[tokio::test]
    async fn epmc_search_registers_open_access_urls_once_and_only_those() {
        let server = MockServer::start().await;
        serve_json(&server, SEARCH, fixture("epmc_search_icg.json")).await;
        Mock::given(method("GET"))
            .and(path("/articles/PMC13601099"))
            .respond_with(ResponseTemplate::new(200).set_body_string("<html>full</html>"))
            .mount(&server)
            .await;
        let oa = OaPolicy::default();
        let e = epmc(&server, &oa);
        let papers = e.search(TOPIC1, 10).await.unwrap();
        let granted = papers
            .iter()
            .find(|p| p.pmid.as_deref() == Some("42778806"))
            .and_then(|p| p.oa_url.clone())
            .unwrap();
        assert_eq!(granted, "https://europepmc.org/articles/PMC13601099");
        let c = client(&server, &oa);
        assert_eq!(c.get_text(&granted).await.unwrap(), "<html>full</html>");
        // One-time: the second fetch is refused.
        assert!(matches!(
            c.get_text(&granted).await,
            Err(LitError::Disallowed(_))
        ));
        // A closed record's own landing page was never registered.
        assert!(matches!(
            c.get_text("https://doi.org/10.7759/cureus.117951").await,
            Err(LitError::Disallowed(_))
        ));
    }

    #[tokio::test]
    async fn epmc_only_grants_https_urls_of_open_access_records() {
        let body = r#"{"resultList":{"result":[
{"id":"1","source":"MED","pmid":"1","pmcid":"PMC1","title":"http only","isOpenAccess":"Y",
 "fullTextUrlList":{"fullTextUrl":[{"availabilityCode":"OA","availability":"Open access","documentStyle":"html","site":"Europe_PMC","url":"http://evil.example/a"}]}},
{"id":"2","source":"MED","pmid":"2","pmcid":"PMC2","title":"not open access","isOpenAccess":"N",
 "fullTextUrlList":{"fullTextUrl":[{"availabilityCode":"OA","availability":"Open access","documentStyle":"html","site":"Europe_PMC","url":"https://evil.example/b"}]}},
{"id":"3","source":"MED","pmid":"3","title":"subscription link","isOpenAccess":"Y",
 "fullTextUrlList":{"fullTextUrl":[{"availabilityCode":"S","availability":"Subscription required","documentStyle":"html","site":"Elsevier","url":"https://evil.example/c"}]}},
{"id":"4","source":"MED","pmid":"4","pmcid":"PMC4","title":"no list, has pmcid","isOpenAccess":"Y"}
]}}"#;
        let server = MockServer::start().await;
        serve_json(&server, SEARCH, body.to_string()).await;
        let oa = OaPolicy::default();
        let papers = epmc(&server, &oa).search("x", 5).await.unwrap();
        assert_eq!(papers[0].oa_url, None, "http is never granted");
        assert_eq!(papers[1].oa_url, None, "closed record");
        assert_eq!(papers[2].oa_url, None, "subscription link");
        // No synthesised fallback: an OA record without a list gets no link.
        assert_eq!(papers[3].oa_url, None);
        assert!(papers.iter().all(|p| !p.oa_is_pdf));
        let c = client(&server, &oa);
        for u in [
            "http://evil.example/a",
            "https://evil.example/b",
            "https://evil.example/c",
        ] {
            assert!(matches!(c.get_text(u).await, Err(LitError::Disallowed(_))));
        }
    }

    #[tokio::test]
    async fn epmc_search_reports_odd_responses() {
        let server = MockServer::start().await;
        serve_json(&server, SEARCH, r#"{"hitCount":0}"#.into()).await;
        let r = epmc(&server, &OaPolicy::default()).search("x", 5).await;
        assert_eq!(r, Err(LitError::Parse));
        let server = MockServer::start().await;
        serve_json(
            &server,
            SEARCH,
            r#"{"hitCount":0,"resultList":{"result":[]}}"#.into(),
        )
        .await;
        let r = epmc(&server, &OaPolicy::default()).search("x", 5).await;
        assert_eq!(r.unwrap(), vec![]);
    }

    // ----- Europe PMC full text -----

    #[tokio::test]
    async fn epmc_full_text_sections_in_order_and_truncated() {
        // The real fixture first.
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/europepmc/webservices/rest/PMC13375015/fullTextXML"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_raw(fixture("epmc_fulltext.xml"), "application/xml"),
            )
            .expect(1)
            .mount(&server)
            .await;
        let ft = epmc(&server, &OaPolicy::default())
            .full_text("PMC13375015")
            .await
            .unwrap();
        let names: Vec<&str> = ft.sections.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["abstract", "methods", "results", "discussion"]);
        let get = |n: &str| &ft.sections.iter().find(|(k, _)| k == n).unwrap().1;
        assert!(get("abstract").contains("Indocyanine green fluorescence angiography"));
        assert!(
            !get("abstract").contains("<p>") && !get("abstract").contains("<title"),
            "no markup"
        );
        assert!(get("methods").len() > 3000);
        assert!(get("results").len() > 3000);
        assert!(get("discussion").len() > 3000);
        // Neither the references nor the disclosures leak into a section.
        assert!(ft.sections.iter().all(|(_, t)| !t.contains("Disclosures")));
        let total: usize = ft.sections.iter().map(|(_, t)| t.chars().count()).sum();
        assert!(total <= FULL_TEXT_CHARS);
    }

    fn article(abs: usize, methods: usize, results: usize, discussion: usize) -> String {
        let w = |c: char, n: usize| c.to_string().repeat(n);
        format!(
            r#"<article><front><article-meta><abstract><p>{}</p></abstract></article-meta></front><body>
<sec><title>Introduction</title><p>intro</p></sec>
<sec><title>Discussion</title><p>{}</p></sec>
<sec><title>Results</title><p>{}</p></sec>
<sec><title>Methods</title><p>{}</p></sec>
</body></article>"#,
            w('a', abs),
            w('d', discussion),
            w('r', results),
            w('m', methods)
        )
    }

    fn lens(xml: &str) -> Vec<(String, usize)> {
        full_text_from_xml(xml)
            .unwrap()
            .sections
            .into_iter()
            .map(|(n, t)| (n, t.chars().count()))
            .collect()
    }

    #[test]
    fn full_text_is_ordered_by_kind_not_by_document_position() {
        let l = lens(&article(10, 20, 30, 40));
        assert_eq!(
            l,
            vec![
                ("abstract".to_string(), 10),
                ("methods".to_string(), 20),
                ("results".to_string(), 30),
                ("discussion".to_string(), 40)
            ]
        );
    }

    #[test]
    fn full_text_truncates_discussion_first_then_results() {
        // Over by 5,000: only the discussion shrinks.
        let l = lens(&article(1000, 9000, 30_000, 25_000));
        let sum: usize = l.iter().map(|(_, n)| n).sum();
        assert_eq!(sum, FULL_TEXT_CHARS);
        assert_eq!(l[0].1, 1000);
        assert_eq!(l[1].1, 9000);
        assert_eq!(l[2].1, 30_000);
        assert_eq!(l[3].1, 20_000);
        // Discussion alone cannot absorb it: it goes, then results shrink.
        let l = lens(&article(1000, 9000, 60_000, 5000));
        assert_eq!(
            l.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(),
            ["abstract", "methods", "results"]
        );
        assert_eq!(l.iter().map(|(_, n)| n).sum::<usize>(), FULL_TEXT_CHARS);
        assert_eq!(l[2].1, 50_000);
        // Multi-byte text is cut on a character boundary.
        let xml = format!(
            "<article><body><sec><title>Results</title><p>{}</p></sec></body></article>",
            "é".repeat(70_000)
        );
        let ft = full_text_from_xml(&xml).unwrap();
        assert_eq!(ft.sections[0].1.chars().count(), FULL_TEXT_CHARS);
    }

    #[test]
    fn full_text_matches_titles_and_sec_type_and_ignores_the_rest() {
        let xml = r#"<?xml version="1.0"?>
<!DOCTYPE article>
<article><front><article-meta>
<abstract abstract-type="graphical"><p>graphical thing</p></abstract>
<abstract><sec><title>Background</title><p>Real &amp; <italic>proper</italic> abstract.</p></sec></abstract>
</article-meta></front><body>
<sec sec-type="intro"><title>Background</title><p>skip me</p></sec>
<sec sec-type="materials|methods"><title>Study design</title><p>Design text.</p>
  <sec><title>Subsection</title><p>Nested text.</p><table-wrap><table><tr><td>CELL</td></tr></table></table-wrap></sec></sec>
<sec><title>Patients and Methods</title><p>More methods <xref ref-type="bibr" rid="r1">12</xref>.</p></sec>
<sec><title>RESULTS</title><p>Result text.</p><fig><caption><p>FIGCAPTION</p></caption></fig></sec>
<sec><title>Discussion and conclusions</title><p>Disc text.</p></sec>
<sec sec-type="conclusions"><title>Conclusions</title><p>concl</p></sec>
<sec sec-type="ref-list"><title>References</title><p>REFS</p></sec>
</body><back><ref-list><ref>BACKREF</ref></ref-list></back></article>"#;
        let ft = full_text_from_xml(xml).unwrap();
        let names: Vec<&str> = ft.sections.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["abstract", "methods", "results", "discussion"]);
        let get = |n: &str| ft.sections.iter().find(|(k, _)| k == n).unwrap().1.clone();
        let abs = get("abstract");
        assert!(abs.contains("Real & proper abstract."), "{abs}");
        assert!(!abs.contains("graphical"));
        let m = get("methods");
        assert!(
            m.contains("Design text.") && m.contains("Nested text."),
            "{m}"
        );
        assert!(m.contains("More methods"), "{m}");
        assert!(!m.contains("CELL"), "tables are skipped: {m}");
        assert!(!m.contains("12"), "citation numbers are skipped: {m}");
        assert!(!get("results").contains("FIGCAPTION"));
        assert!(get("discussion").contains("Disc text."));
        let all: String = ft.sections.iter().map(|(_, t)| t.as_str()).collect();
        assert!(!all.contains("REFS") && !all.contains("BACKREF") && !all.contains("skip me"));
    }

    #[test]
    fn full_text_never_panics_on_odd_xml() {
        for bad in [
            "",
            "not xml at all",
            "<article>",
            "<article><body><sec><title>Results</title><p>x</sec></body></article>",
            "</article>",
            "<error>not available</error>",
            "<article><body/></article>",
            "<<<>>>&&&",
            "<article><body><sec><title>Methods</title><p>&bogus; &#xD800; &#99999999; ok</p></sec></body></article>",
        ] {
            // Ok or Err are both fine; panicking is not.
            let _ = full_text_from_xml(bad);
        }
        assert_eq!(full_text_from_xml("<error>x</error>"), Err(LitError::Parse));
        assert_eq!(
            full_text_from_xml("<article><body/></article>"),
            Err(LitError::Parse)
        );
        // Deep nesting does not blow the stack.
        let deep = format!(
            "<article><body><sec><title>Methods</title>{}x{}</sec></body></article>",
            "<sec>".repeat(50_000),
            "</sec>".repeat(50_000)
        );
        let _ = full_text_from_xml(&deep);
    }

    #[tokio::test]
    async fn epmc_full_text_rejects_malformed_pmcid_without_a_request() {
        let server = MockServer::start().await;
        let e = epmc(&server, &OaPolicy::default());
        for bad in [
            "",
            "PMC",
            "12345",
            "PMC12/../x",
            "pmc123",
            "PMC1?x=1",
            "PMC12 34",
        ] {
            assert_eq!(e.full_text(bad).await, Err(LitError::Parse), "{bad:?}");
        }
        assert!(server.received_requests().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn epmc_full_text_reports_http_errors() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/europepmc/webservices/rest/PMC9/fullTextXML"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;
        let r = epmc(&server, &OaPolicy::default()).full_text("PMC9").await;
        assert_eq!(r, Err(LitError::Http(404)));
    }

    // ----- Unpaywall -----

    fn unpaywall_route(doi: &str) -> String {
        format!("/v2/{doi}")
    }

    #[tokio::test]
    async fn unpaywall_returns_https_url_or_none() {
        let server = MockServer::start().await;
        let doi_pdf = "10.1007/s10151-026-03388-4";
        let doi_url = "10.1056/NEJMoa2034577";
        let doi_closed = "10.1016/j.surg.2019.05.025";
        for (doi, f) in [
            (doi_pdf, "unpaywall_oa_pdf.json"),
            (doi_url, "unpaywall_oa_url_only.json"),
            (doi_closed, "unpaywall_closed.json"),
        ] {
            Mock::given(method("GET"))
                .and(path(unpaywall_route(doi)))
                .and(query_param("email", "ci@example.invalid"))
                .respond_with(
                    ResponseTemplate::new(200).set_body_raw(fixture(f), "application/json"),
                )
                .expect(1)
                .mount(&server)
                .await;
        }
        let oa = OaPolicy::default();
        let u = unpaywall(&server, &oa);
        // url_for_pdf is preferred.
        let pdf = u.best_oa(doi_pdf).await.unwrap().unwrap();
        assert_eq!(
            pdf,
            OaLink {
                url: "https://link.springer.com/content/pdf/10.1007/s10151-026-03388-4.pdf".into(),
                is_pdf: true
            }
        );
        // No url_for_pdf: falls back to url.
        let url = u.best_oa(doi_url).await.unwrap().unwrap();
        assert_eq!(
            url,
            OaLink {
                url: "https://www.ncbi.nlm.nih.gov/pmc/articles/7745181".into(),
                is_pdf: false
            }
        );
        // Not open access.
        assert_eq!(u.best_oa(doi_closed).await.unwrap(), None);
        // The returned URLs were registered, once each; nothing else was.
        let c = client(&server, &oa);
        for (url, body_path) in [
            (
                pdf.url.as_str(),
                "/content/pdf/10.1007/s10151-026-03388-4.pdf",
            ),
            (url.url.as_str(), "/pmc/articles/7745181"),
        ] {
            Mock::given(method("GET"))
                .and(path(body_path))
                .respond_with(ResponseTemplate::new(200).set_body_string("ok"))
                .mount(&server)
                .await;
            assert_eq!(c.get_text(url).await.unwrap(), "ok");
            assert!(matches!(
                c.get_text(url).await,
                Err(LitError::Disallowed(_))
            ));
        }
        assert!(matches!(
            c.get_text("https://doi.org/10.1016/j.surg.2019.05.025")
                .await,
            Err(LitError::Disallowed(_))
        ));
    }

    #[tokio::test]
    async fn unpaywall_http_url_is_ignored() {
        let body = |pdf: &str, url: &str| {
            format!(r#"{{"is_oa":true,"best_oa_location":{{"url_for_pdf":{pdf},"url":{url}}}}}"#)
        };
        let server = MockServer::start().await;
        serve_json(
            &server,
            "/v2/10.1/a",
            body(
                r#""http://insecure.example/a.pdf""#,
                r#""http://insecure.example/a""#,
            ),
        )
        .await;
        // http pdf but https landing url: the https one is used.
        serve_json(
            &server,
            "/v2/10.1/b",
            body(
                r#""http://insecure.example/b.pdf""#,
                r#""https://ok.example/b""#,
            ),
        )
        .await;
        serve_json(&server, "/v2/10.1/c", body("null", "null")).await;
        serve_json(
            &server,
            "/v2/10.1/d",
            r#"{"is_oa":false,"best_oa_location":null}"#.into(),
        )
        .await;
        serve_json(&server, "/v2/10.1/e", r#"{"is_oa":true}"#.into()).await;
        let oa = OaPolicy::default();
        let u = unpaywall(&server, &oa);
        assert_eq!(u.best_oa("10.1/a").await.unwrap(), None);
        assert_eq!(
            u.best_oa("10.1/b").await.unwrap(),
            Some(OaLink {
                url: "https://ok.example/b".into(),
                is_pdf: false
            })
        );
        for d in ["10.1/c", "10.1/d", "10.1/e"] {
            assert_eq!(u.best_oa(d).await.unwrap(), None, "{d}");
        }
        let c = client(&server, &oa);
        assert!(matches!(
            c.get_text("http://insecure.example/a").await,
            Err(LitError::Disallowed(_))
        ));
    }

    #[tokio::test]
    async fn unpaywall_prefers_html_and_flags_pdfs() {
        let loc = |pdf: &str, url: &str, landing: &str| {
            format!(
                r#"{{"is_oa":true,"best_oa_location":{{"url_for_pdf":{pdf},"url":{url},"url_for_landing_page":{landing}}}}}"#
            )
        };
        let server = MockServer::start().await;
        // html url + pdf: html wins.
        serve_json(
            &server,
            "/v2/10.1/h",
            loc(
                r#""https://x.example/a.pdf""#,
                r#""https://x.example/a""#,
                "null",
            ),
        )
        .await;
        // url is the pdf itself: pdf, flagged.
        serve_json(
            &server,
            "/v2/10.1/p",
            loc(
                "null",
                r#""https://x.example/b.PDF""#,
                r#""https://doi.org/10.1/p""#,
            ),
        )
        .await;
        // no url: the https landing page is the html link.
        serve_json(
            &server,
            "/v2/10.1/l",
            loc(
                r#""https://x.example/c.pdf""#,
                "null",
                r#""https://x.example/c""#,
            ),
        )
        .await;
        // pdf only.
        serve_json(
            &server,
            "/v2/10.1/o",
            loc(r#""https://x.example/d.pdf""#, "null", "null"),
        )
        .await;
        let u = unpaywall(&server, &OaPolicy::default());
        let get = |d: &'static str| {
            let u = &u;
            async move { u.best_oa(d).await.unwrap().unwrap() }
        };
        assert_eq!(
            get("10.1/h").await,
            OaLink {
                url: "https://x.example/a".into(),
                is_pdf: false
            }
        );
        assert_eq!(
            get("10.1/p").await,
            OaLink {
                url: "https://x.example/b.PDF".into(),
                is_pdf: true
            }
        );
        assert_eq!(
            get("10.1/l").await,
            OaLink {
                url: "https://x.example/c".into(),
                is_pdf: false
            }
        );
        assert_eq!(
            get("10.1/o").await,
            OaLink {
                url: "https://x.example/d.pdf".into(),
                is_pdf: true
            }
        );
    }

    #[tokio::test]
    async fn unpaywall_rejects_malformed_dois_without_a_request() {
        let server = MockServer::start().await;
        let u = unpaywall(&server, &OaPolicy::default());
        for bad in [
            "",
            "11.1/x",
            "doi:10.1/x",
            "10.1/../../etc",
            "10.1/a..b",
            "/v2/10.1/x",
        ] {
            assert_eq!(u.best_oa(bad).await.unwrap(), None, "{bad:?}");
        }
        assert!(server.received_requests().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn epmc_flags_pdf_links_and_prefers_html() {
        let entry = |style: &str, site: &str, url: &str| {
            format!(
                r#"{{"availabilityCode":"OA","documentStyle":"{style}","site":"{site}","url":"{url}"}}"#
            )
        };
        let body = format!(
            r#"{{"resultList":{{"result":[
{{"id":"1","source":"MED","pmid":"1","isOpenAccess":"Y","title":"pdf only","fullTextUrlList":{{"fullTextUrl":[{}]}}}},
{{"id":"2","source":"MED","pmid":"2","isOpenAccess":"Y","title":"both","fullTextUrlList":{{"fullTextUrl":[{},{}]}}}},
{{"id":"3","source":"MED","pmid":"3","isOpenAccess":"Y","title":"pdf suffix","fullTextUrlList":{{"fullTextUrl":[{}]}}}}
]}}}}"#,
            entry(
                "pdf",
                "Europe_PMC",
                "https://europepmc.org/articles/PMC1?pdf=render"
            ),
            entry(
                "pdf",
                "Europe_PMC",
                "https://europepmc.org/articles/PMC2?pdf=render"
            ),
            entry(
                "html",
                "PubMedCentral",
                "https://www.ncbi.nlm.nih.gov/pmc/articles/PMC2/"
            ),
            entry("html", "Publisher", "https://pub.example/paper.pdf"),
        );
        let server = MockServer::start().await;
        serve_json(&server, SEARCH, body).await;
        let papers = epmc(&server, &OaPolicy::default())
            .search("x", 5)
            .await
            .unwrap();
        assert!(papers[0].oa_is_pdf, "documentStyle pdf");
        assert_eq!(
            papers[0].oa_url.as_deref(),
            Some("https://europepmc.org/articles/PMC1?pdf=render")
        );
        assert!(!papers[1].oa_is_pdf, "html beats pdf");
        assert_eq!(
            papers[1].oa_url.as_deref(),
            Some("https://www.ncbi.nlm.nih.gov/pmc/articles/PMC2/")
        );
        assert!(papers[2].oa_is_pdf, ".pdf suffix");
        assert!(
            url_is_pdf("https://a.example/x.PDF")
                && url_is_pdf("https://europepmc.org/articles/PMC1?pdf=render")
        );
        assert!(!url_is_pdf("https://a.example/pdf/x") && !url_is_pdf("not a url"));
    }

    #[tokio::test]
    async fn unpaywall_unknown_doi_is_none_and_errors_are_reported() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v2/10.1/missing"))
            .respond_with(
                ResponseTemplate::new(404).set_body_string(
                    r#"{"HTTP_status_code":404,"error":true,"message":"not found"}"#,
                ),
            )
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/v2/10.1/down"))
            .respond_with(ResponseTemplate::new(503))
            .mount(&server)
            .await;
        serve_json(&server, "/v2/10.1/junk", "not json".into()).await;
        let u = unpaywall(&server, &OaPolicy::default());
        assert_eq!(u.best_oa("10.1/missing").await.unwrap(), None);
        assert_eq!(u.best_oa("10.1/down").await, Err(LitError::Http(503)));
        assert_eq!(u.best_oa("10.1/junk").await, Err(LitError::Parse));
        // Blank DOIs never reach the network.
        let before = server.received_requests().await.unwrap().len();
        assert_eq!(u.best_oa("  ").await.unwrap(), None);
        assert_eq!(server.received_requests().await.unwrap().len(), before);
    }

    // ----- merge -----

    fn p(pmid: Option<&str>, doi: Option<&str>, title: &str, src: PaperSource) -> Paper {
        Paper {
            pmid: pmid.map(String::from),
            pmcid: None,
            doi: doi.map(String::from),
            title: title.into(),
            authors: vec![],
            journal: None,
            year: None,
            pub_types: vec![],
            abstract_text: None,
            oa_url: None,
            oa_is_pdf: false,
            retracted: false,
            source: src,
        }
    }

    #[test]
    fn merge_dedupes_by_pmid_then_doi() {
        let mut pm1 = p(
            Some("1"),
            Some("10.1/AAA"),
            "PubMed title",
            PaperSource::PubMed,
        );
        pm1.authors = vec!["A B".into()];
        pm1.pub_types = vec!["Journal Article".into()];
        pm1.abstract_text = Some("pubmed abstract".into());
        let pm2 = p(Some("2"), None, "Two", PaperSource::PubMed);
        let pm3 = p(Some("3"), Some("10.1/ccc"), "Three", PaperSource::PubMed);

        // Same PMID.
        let mut e1 = p(Some("1"), None, "EPMC title", PaperSource::EuropePmc);
        e1.pmcid = Some("PMC11".into());
        e1.oa_url = Some("https://europepmc.org/articles/PMC11".into());
        e1.abstract_text = Some("epmc abstract".into());
        e1.journal = Some("J".into());
        e1.year = Some(2024);
        e1.pub_types = vec!["Journal Article".into(), "Review".into()];
        // Same DOI (different case), PMID missing on the EPMC side.
        let mut e3 = p(
            None,
            Some("10.1/CCC"),
            "Three (epmc)",
            PaperSource::EuropePmc,
        );
        e3.pmcid = Some("PMC33".into());
        e3.retracted = true;
        // Same DOI but a different PMID: not the same record.
        let e4 = p(
            Some("99"),
            Some("10.1/ccc"),
            "Other pmid",
            PaperSource::EuropePmc,
        );
        // Only in EPMC.
        let e5 = p(Some("5"), Some("10.1/eee"), "Five", PaperSource::EuropePmc);
        // A duplicate inside the EPMC list itself.
        let e5b = p(None, Some("10.1/EEE"), "Five again", PaperSource::EuropePmc);

        let out = merge_results(vec![pm1, pm2, pm3], vec![e1, e3, e4, e5, e5b]);
        let key = |x: &Paper| x.pmid.clone().or(x.doi.clone());
        assert_eq!(
            out.iter().map(key).collect::<Vec<_>>(),
            vec![
                Some("1".into()),
                Some("2".into()),
                Some("3".into()),
                Some("99".into()),
                Some("5".into()),
            ]
        );
        let m1 = &out[0];
        assert_eq!(m1.source, PaperSource::Both);
        assert_eq!(m1.title, "PubMed title", "PubMed values win when present");
        assert_eq!(m1.abstract_text.as_deref(), Some("pubmed abstract"));
        assert_eq!(m1.doi.as_deref(), Some("10.1/AAA"));
        assert_eq!(m1.pmcid.as_deref(), Some("PMC11"), "null filled from EPMC");
        assert_eq!(
            m1.oa_url.as_deref(),
            Some("https://europepmc.org/articles/PMC11")
        );
        assert_eq!(m1.journal.as_deref(), Some("J"));
        assert_eq!(m1.year, Some(2024));
        assert_eq!(m1.authors, vec!["A B".to_string()]);
        assert_eq!(m1.pub_types, vec!["Journal Article", "Review"]);
        assert_eq!(out[1].source, PaperSource::PubMed);
        let m3 = &out[2];
        assert_eq!(m3.source, PaperSource::Both);
        assert_eq!(m3.pmid.as_deref(), Some("3"));
        assert_eq!(m3.pmcid.as_deref(), Some("PMC33"));
        assert!(m3.retracted, "retracted is sticky across sources");
        assert_eq!(out[3].source, PaperSource::EuropePmc);
        assert_eq!(out[4].source, PaperSource::EuropePmc);
        assert_eq!(out.len(), 5);
    }

    #[test]
    fn merge_matches_a_pmc_only_record_by_pmcid() {
        let mut pm = p(Some("7"), None, "Seven", PaperSource::PubMed);
        pm.pmcid = Some("PMC70".into());
        let mut e = p(None, None, "Seven (pmc)", PaperSource::EuropePmc);
        e.pmcid = Some("PMC70".into());
        e.oa_url = Some("https://europepmc.org/articles/PMC70".into());
        let out = merge_results(vec![pm], vec![e]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].source, PaperSource::Both);
        assert!(out[0].oa_url.is_some());
    }

    #[test]
    fn merge_keeps_pubmed_order() {
        let pubmed: Vec<Paper> = ["30", "10", "20"]
            .iter()
            .map(|i| p(Some(i), None, i, PaperSource::PubMed))
            .collect();
        let epmc = vec![
            p(Some("99"), None, "e-only-1", PaperSource::EuropePmc),
            p(Some("20"), None, "dup", PaperSource::EuropePmc),
            p(Some("10"), None, "dup", PaperSource::EuropePmc),
            p(Some("98"), None, "e-only-2", PaperSource::EuropePmc),
        ];
        let out = merge_results(pubmed, epmc);
        let ids: Vec<_> = out.iter().map(|x| x.pmid.clone().unwrap()).collect();
        assert_eq!(ids, ["30", "10", "20", "99", "98"]);
        assert_eq!(merge_results(vec![], vec![]), vec![]);
    }

    // ----- cache keeps pmcid (DB-gated) -----

    #[tokio::test]
    async fn cache_stores_pmcid_and_keeps_it_on_refresh() {
        let Ok(url) = std::env::var("TEST_DATABASE_URL") else {
            eprintln!("SKIP: TEST_DATABASE_URL not set");
            return;
        };
        let pool = lazy_pool(&SecretString::from(url)).expect("lazy pool");
        migrate(&pool).await.expect("migrate");
        let tag = uuid::Uuid::new_v4().simple().to_string();
        let pmid = format!("t7-{tag}");
        let mut a = p(Some(&pmid), Some("10.1/x"), "T", PaperSource::PubMed);
        a.pmcid = Some("PMC555".into());
        cache::put_many(&pool, std::slice::from_ref(&a))
            .await
            .unwrap();
        let got = cache::get_many(&pool, std::slice::from_ref(&pmid))
            .await
            .unwrap();
        assert_eq!(got[0].pmcid.as_deref(), Some("PMC555"));
        // A refresh without a pmcid keeps the stored one.
        a.pmcid = None;
        cache::put_many(&pool, std::slice::from_ref(&a))
            .await
            .unwrap();
        let got = cache::get_many(&pool, std::slice::from_ref(&pmid))
            .await
            .unwrap();
        assert_eq!(got[0].pmcid.as_deref(), Some("PMC555"));
        // A newer pmcid replaces it.
        a.pmcid = Some("PMC556".into());
        cache::put_many(&pool, std::slice::from_ref(&a))
            .await
            .unwrap();
        let got = cache::get_many(&pool, std::slice::from_ref(&pmid))
            .await
            .unwrap();
        assert_eq!(got[0].pmcid.as_deref(), Some("PMC556"));
        // oa_is_pdf is recovered from the stored URL's shape.
        a.oa_url = Some("https://x.example/paper.pdf".into());
        cache::put_many(&pool, std::slice::from_ref(&a))
            .await
            .unwrap();
        let got = cache::get_many(&pool, std::slice::from_ref(&pmid))
            .await
            .unwrap();
        assert!(got[0].oa_is_pdf);
        a.oa_url = Some("https://x.example/paper".into());
        cache::put_many(&pool, std::slice::from_ref(&a))
            .await
            .unwrap();
        let got = cache::get_many(&pool, std::slice::from_ref(&pmid))
            .await
            .unwrap();
        assert!(!got[0].oa_is_pdf);
        sqlx::query("DELETE FROM incisive.paper_cache WHERE pmid = $1")
            .bind(&pmid)
            .execute(&pool)
            .await
            .unwrap();
    }
}
