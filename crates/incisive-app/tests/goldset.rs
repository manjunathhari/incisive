//! The gold-set runner's per-question path (`src/bin/goldset.rs`) against the
//! test database: a real run through `ask::start_run` and the background
//! worker, with the scripted Claude client and the recorded literature
//! fixtures served locally. Nothing leaves the machine.

// The binary's source, as a module (its `main` is unused here).
#[allow(dead_code)]
#[path = "../src/bin/goldset.rs"]
mod goldset;

use std::sync::Arc;
use std::time::Duration;

use claude_client::Usage;
use goldset::{
    Landmark, Question, Recheck, ensure_goldset_user, recheck_refs, render_report, run_question,
    totals,
};
use incisive_app::agent::Literature;
use incisive_app::agent::answer::Reference;
use incisive_app::agent::llm::ScriptedLlm;
use incisive_app::db::{lazy_pool, migrate};
use incisive_app::gate::OfflineClassifier;
use incisive_app::identity::{IapVerifier, JwkSource, Role};
use incisive_app::lit::europepmc::EuropePmc;
use incisive_app::lit::http::{AllowlistedClient, OaPolicy};
use incisive_app::lit::pubmed::PubMed;
use incisive_app::runs::worker::WorkerHandle;
use incisive_app::runs::{RunMode, RunsConfig};
use incisive_app::server::AppState;
use incisive_app::spend::{CapsConfig, Prices};
use secrecy::SecretString;
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const MODEL: &str = "m-test";
const QUESTION: &str =
    "In adults having colorectal resection, does ICG angiography reduce anastomotic leak?";
/// A verbatim clause of the meta-analysis abstract (efetch fixture).
const META_ABS_QUOTE: &str = "ICGFA significantly reduced overall anastomotic leak (risk ratio 0\u{b7}66 [95% CI 0\u{b7}56-0\u{b7}78]";
/// A verbatim sentence of the RCT abstract (efetch fixture).
const ICG_VERBATIM: &str = "Overall, the anastomotic leak rate was 5.8% (33/567) in the ICG fluorescence imaging group vs 7.9% (45/569) in the control group";

async fn pool() -> Option<PgPool> {
    let Ok(url) = std::env::var("TEST_DATABASE_URL") else {
        eprintln!("SKIP: TEST_DATABASE_URL not set");
        return None;
    };
    let pool = lazy_pool(&SecretString::from(url)).expect("lazy pool");
    migrate(&pool).await.expect("migrate");
    Some(pool)
}

fn fixture(dir: &str, name: &str) -> String {
    std::fs::read_to_string(format!(
        "{}/tests/fixtures/{dir}/{name}",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap()
}

fn agent_json(name: &str) -> serde_json::Value {
    serde_json::from_str(&fixture("agent", name)).unwrap()
}

fn u() -> Usage {
    Usage {
        input_tokens: 100,
        output_tokens: 20,
    }
}

/// The ICG run of the Task 13 end-to-end test: frame, search (propose,
/// accept), two screening batches (p-1 and p-2 included), two extractions,
/// three drafted claims and one check per claim.
fn script() -> Arc<ScriptedLlm> {
    let screen: Vec<serde_json::Value> = (1..=30)
        .map(|i| {
            json!({"id": format!("p-{i}"), "include": i <= 2, "code": "population", "reason": "r"})
        })
        .collect();
    let finding = |quote: &str, outcome: &str, result: &str| json!({"outcome": outcome, "result": result, "quote": quote});
    let meta = json!({
        "design": "systematic review and meta-analysis",
        "n": 4754,
        "population": "adults having colorectal resection",
        "intervention": "ICG fluorescence angiography",
        "comparison": "standard assessment",
        "findings": [finding(META_ABS_QUOTE, "anastomotic leak", "risk ratio 0.66")],
    });
    let rct = json!({
        "design": "randomized controlled trial",
        "n": 1136,
        "population": "adults having laparoscopic colorectal resection",
        "intervention": "ICG fluorescence angiography",
        "comparison": "no ICG",
        "findings": [finding(ICG_VERBATIM, "anastomotic leak", "5.8% vs 7.9%")],
    });
    let claims = json!({
        "claims": [
            {"text": "ICG angiography reduced anastomotic leak across randomised trials", "refs": [1], "confidence": "strong"},
            {"text": "In one RCT the leak rate was 5.8% with ICG vs 7.9% without", "refs": [2], "confidence": "moderate"},
            {"text": "Both sources point the same way", "refs": [1, 2, 2], "confidence": "strong"},
        ],
        "insufficient": false,
    });
    let mut llm = ScriptedLlm::new()
        .reply_json(agent_json("frame_pico.json"), u())
        .reply_json(agent_json("search_initial.json"), u())
        .reply_json(agent_json("search_accept.json"), u())
        .reply_json(json!(screen), u())
        .reply_json(json!(screen), u())
        .reply_json(meta, u())
        .reply_json(rct, u())
        .reply_json(claims, u());
    for v in ["supported", "supported", "partial"] {
        llm = llm.reply_json(json!([{"claim": 1, "verdict": v}]), u());
    }
    Arc::new(llm)
}

async fn mount(server: &MockServer, route: &str, body: String) {
    Mock::given(method("GET"))
        .and(path(route))
        .respond_with(ResponseTemplate::new(200).set_body_string(body))
        .mount(server)
        .await;
}

/// PubMed and Europe PMC, answered from the recorded ICG fixtures.
async fn icg_server() -> MockServer {
    let server = MockServer::start().await;
    let lit = |n: &str| fixture("lit", n);
    mount(
        &server,
        "/entrez/eutils/esearch.fcgi",
        lit("pubmed_esearch_icg.json"),
    )
    .await;
    mount(
        &server,
        "/entrez/eutils/esummary.fcgi",
        lit("pubmed_esummary_icg.json"),
    )
    .await;
    mount(
        &server,
        "/entrez/eutils/efetch.fcgi",
        lit("pubmed_efetch_icg.xml"),
    )
    .await;
    mount(
        &server,
        "/europepmc/webservices/rest/search",
        lit("epmc_search_icg.json"),
    )
    .await;
    server
}

fn literature(server: &MockServer) -> Literature {
    let oa = OaPolicy::default();
    let client = AllowlistedClient::new("Incisive/test", oa.clone())
        .with_base_override(server.uri())
        .with_retry_backoff(Duration::from_millis(1));
    Literature {
        pubmed: PubMed::new(client.clone(), None).with_min_interval(Duration::ZERO),
        epmc: EuropePmc::new(client, oa),
    }
}

fn state(pool: PgPool, llm: Arc<ScriptedLlm>, server: &MockServer) -> AppState {
    let lit = literature(server);
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
        dev_mode: false,
        runs: RunsConfig::default(),
        // The real pipeline.
        worker: WorkerHandle::default(),
        llm: Some(incisive_app::agent::testing::agent_llm(llm, MODEL)),
        lit: Arc::new(lit),
    }
}

fn landmark(pmid: Option<&str>, doi: Option<&str>, hint: &str) -> Landmark {
    Landmark {
        pmid: pmid.map(str::to_string),
        doi: doi.map(str::to_string),
        title_hint: hint.to_string(),
    }
}

#[tokio::test]
async fn goldset_user_is_created_then_reused_with_the_new_cap() {
    let Some(pool) = pool().await else { return };
    let first = ensure_goldset_user(&pool, 80_000).await.unwrap();
    assert_eq!(first.email, goldset::GOLDSET_EMAIL);
    assert_eq!(first.role, Role::Surgeon);
    assert_eq!(first.daily_cap_paise, 80_000);
    let again = ensure_goldset_user(&pool, 123_400).await.unwrap();
    assert_eq!(again.id, first.id);
    assert_eq!(again.daily_cap_paise, 123_400);
    assert!(again.monthly_cap_paise.is_some_and(|m| m >= 123_400));
    assert!(again.active);
}

#[tokio::test]
async fn run_question_reports_a_real_budgeted_run() {
    let Some(pool) = pool().await else { return };
    // A fresh surgeon (as the gold-set user, but alone in its spend rows).
    let email = format!("goldset-test-{}@example.com", Uuid::new_v4());
    sqlx::query(
        "INSERT INTO incisive.users (email, display_name, role, daily_cap_paise, monthly_cap_paise) \
         VALUES ($1, 'T', 'surgeon', 80000, 300000)",
    )
    .bind(&email)
    .execute(&pool)
    .await
    .unwrap();
    let user = incisive_app::identity::find_user(&pool, &email)
        .await
        .unwrap()
        .unwrap();

    let server = icg_server().await;
    let llm = script();
    let state = state(pool.clone(), llm.clone(), &server);
    let q = Question {
        id: "icg-leak".into(),
        question: QUESTION.into(),
        landmarks: vec![
            // The meta-analysis, by PMID.
            landmark(Some("41871586"), None, "not the title of anything here"),
            // The RCT, by DOI only (other case, resolver prefix).
            landmark(
                None,
                Some("https://doi.org/10.1001/JAMASURG.2025.0006"),
                "not the title of anything here",
            ),
            // The RCT again, by title hint only.
            landmark(
                None,
                None,
                "indocyanine green fluorescence imaging in prevention of colorectal",
            ),
            // IntAct: screened out by the script, so missed.
            landmark(
                Some("40690925"),
                None,
                "Intraoperative fluorescence angiography with indocyanine green",
            ),
        ],
    };

    let row = tokio::time::timeout(
        Duration::from_secs(60),
        run_question(
            &state,
            &user,
            &q,
            RunMode::Focused,
            Duration::from_millis(20),
        ),
    )
    .await
    .expect("run finished within 60 s");

    assert_eq!(row.status, "done", "{row:?}");
    assert_eq!(llm.remaining(), 0, "every scripted call was made");
    assert_eq!((row.landmarks_hit, row.landmarks_total), (3, 4), "{row:?}");
    assert_eq!(row.missed, ["PMID 40690925"]);
    assert_eq!(row.references, 2);
    assert_eq!((row.refs_verified, row.refs_drafted), (2, 2));
    // Both final references looked up again (PubMed esummary) and matched.
    assert_eq!(
        row.recheck,
        Some(Recheck {
            resolved: 2,
            total: 2,
            problems: vec![],
        })
    );
    assert_eq!((row.claims_drafted, row.claims_removed), (3, 0));
    // The question is cut to its first 80 characters.
    assert!(QUESTION.chars().count() > 80);
    assert!(row.title.chars().count() <= 80);
    assert!(row.title.starts_with(&QUESTION[..70]) && row.title.ends_with('\u{2026}'));

    // The cost is the run's: every charge of this user, which is only this run.
    let charges: i64 = sqlx::query_scalar(
        "SELECT COALESCE(SUM(paise), 0)::bigint FROM incisive.spend WHERE user_id = $1 AND kind = 'charge'",
    )
    .bind(user.id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(charges > 0);
    assert_eq!(row.cost_paise, charges);
    // And the reservation was released: only the charges remain.
    let today = incisive_app::spend::totals(&pool, user.id).await.unwrap();
    assert_eq!(today.today_paise, charges);

    let t = totals(std::slice::from_ref(&row), 4_000, Duration::from_secs(300));
    assert_eq!((t.over_cost, t.over_time, t.unresolved), (0, 0, 0));
    let report = render_report(&[row], &t, "focused", 4_000, Duration::from_secs(300));
    assert!(report.contains("| icg-leak |"), "{report}");
    assert!(
        report.contains("| done | 3/4 | PMID 40690925 | 2 | 2/2 (100%) | 2/2 (100%) |"),
        "{report}"
    );
}

#[tokio::test]
async fn a_question_that_cannot_start_is_a_failed_row() {
    let Some(pool) = pool().await else { return };
    // A daily cap below the Focused limit: the reservation is refused.
    let email = format!("goldset-test-{}@example.com", Uuid::new_v4());
    sqlx::query(
        "INSERT INTO incisive.users (email, display_name, role, daily_cap_paise, monthly_cap_paise) \
         VALUES ($1, 'T', 'surgeon', 100, 300000)",
    )
    .bind(&email)
    .execute(&pool)
    .await
    .unwrap();
    let user = incisive_app::identity::find_user(&pool, &email)
        .await
        .unwrap()
        .unwrap();
    let server = MockServer::start().await;
    let llm = Arc::new(ScriptedLlm::new());
    let state = state(pool.clone(), llm, &server);
    let q = Question {
        id: "x".into(),
        question: QUESTION.into(),
        landmarks: vec![landmark(Some("1"), None, "some title hint here ok")],
    };
    let row = run_question(
        &state,
        &user,
        &q,
        RunMode::Focused,
        Duration::from_millis(20),
    )
    .await;
    assert_eq!(row.status, "failed");
    assert_eq!(row.detail.as_deref(), Some("over_cap"));
    assert_eq!((row.landmarks_hit, row.landmarks_total), (0, 1));
    assert_eq!(row.cost_paise, 0);
    assert!(server.received_requests().await.unwrap().is_empty());
}

fn reference(ref_id: u8, pmid: Option<&str>, doi: Option<&str>, title: &str) -> Reference {
    Reference {
        ref_id,
        title: title.to_string(),
        journal: None,
        year: None,
        pmid: pmid.map(str::to_string),
        doi: doi.map(str::to_string),
        study_type: "RCT".to_string(),
    }
}

#[tokio::test]
async fn recheck_finds_a_title_mismatch_by_pmid() {
    let server = icg_server().await;
    let lit = literature(&server);
    let refs = [
        // As PubMed has it (case and punctuation differ: still the same).
        reference(
            1,
            Some("40042831"),
            None,
            "indocyanine green fluorescence imaging in prevention of colorectal anastomotic leakage - a randomized clinical trial",
        ),
        // A real PMID with another paper's title.
        reference(
            2,
            Some("41871586"),
            None,
            "Laparoscopic versus open mesh repair for inguinal hernia",
        ),
    ];
    let c = recheck_refs(&lit, &refs).await;
    assert_eq!((c.resolved, c.total), (1, 2));
    assert_eq!(c.problems, ["ref 2 (PMID 41871586): title differs"]);
    // One esummary batch; Europe PMC not asked.
    let reqs = server.received_requests().await.unwrap();
    assert!(
        reqs.iter()
            .all(|r| r.url.path() == "/entrez/eutils/esummary.fcgi"),
        "{reqs:?}"
    );
    assert_eq!(reqs.len(), 1);

    let rows = [goldset::Row {
        recheck: Some(c),
        ..failed_row()
    }];
    let t = totals(&rows, 4_000, Duration::from_secs(300));
    assert_eq!(t.unresolved, 1);
    let report = render_report(&rows, &t, "focused", 4_000, Duration::from_secs(300));
    assert!(
        report.contains("**FLAG** 1/2 (50%): ref 2 (PMID 41871586): title differs"),
        "{report}"
    );
}

#[tokio::test]
async fn recheck_looks_up_doi_only_refs_in_europe_pmc() {
    let server = icg_server().await;
    let lit = literature(&server);
    let refs = [
        reference(
            1,
            None,
            Some("https://doi.org/10.1007/S10151-026-03388-4"),
            "Indocyanine green fluorescence angiography for prevention of anastomotic leak in colorectal surgery: a graded systematic review and meta-analysis with trial sequential analysis of randomized controlled trials.",
        ),
        reference(2, None, Some("10.9999/not-in-the-hits"), "Some paper"),
        reference(3, None, None, "No identifiers"),
    ];
    let c = recheck_refs(&lit, &refs).await;
    assert_eq!((c.resolved, c.total), (1, 3));
    assert_eq!(
        c.problems,
        [
            "ref 2 (DOI 10.9999/not-in-the-hits): not found in Europe PMC",
            "ref 3: no PMID or DOI",
        ]
    );
    let reqs = server.received_requests().await.unwrap();
    assert!(
        reqs.iter()
            .all(|r| r.url.path() == "/europepmc/webservices/rest/search")
    );
}

fn failed_row() -> goldset::Row {
    goldset::Row {
        id: "x".into(),
        title: "x".into(),
        status: "done".into(),
        detail: None,
        landmarks_hit: 0,
        landmarks_total: 0,
        missed: vec![],
        references: 2,
        refs_verified: 2,
        refs_drafted: 2,
        recheck: None,
        claims_drafted: 0,
        claims_removed: 0,
        cost_paise: 0,
        wall: Duration::from_secs(1),
    }
}
