//! S3: the researcher's empty-study-list warning and its retry log carry
//! numbers only: never the question, a query, the model's reply or the
//! retry line. Own test binary with ONE test, for the reason given in
//! `ask_logging.rs`: the capture subscriber must be the global default before
//! any callsite registers. To prove the capture sees the path, the test
//! asserts its own log lines are present. Gated on `TEST_DATABASE_URL`.

use std::io::Write;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use claude_client::{ContentBlock, StopReason, Usage};
use incisive_app::agent::llm::ScriptedLlm;
use incisive_app::agent::researcher::{RETRY_LINE, research};
use incisive_app::agent::{Literature, RunContext};
use incisive_app::db::{lazy_pool, migrate};
use incisive_app::gate::OfflineClassifier;
use incisive_app::identity::{IapVerifier, JwkSource};
use incisive_app::lit::europepmc::EuropePmc;
use incisive_app::lit::http::{AllowlistedClient, OaPolicy};
use incisive_app::lit::pubmed::PubMed;
use incisive_app::runs::budget::RunBudget;
use incisive_app::runs::store::{create_run, set_status};
use incisive_app::runs::worker::{CancelFlag, WorkerHandle};
use incisive_app::runs::{DepthSettings, QuickEngine, RunMode, RunStatus, RunsConfig};
use incisive_app::server::AppState;
use incisive_app::spend::{CapsConfig, Prices};
use secrecy::SecretString;
use serde_json::json;
use uuid::Uuid;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const MODEL: &str = "m-test";
const MARKER: &str = "Zebrafinch-7741 cholecystectomy timing";
const QUERY: &str = "quokka7741[tiab] AND cholecystectomy[tiab]";
const REPLY_MARK: &str = "Wombat-reply-7741";

#[derive(Clone, Default)]
struct Captured(Arc<Mutex<Vec<u8>>>);

impl Write for Captured {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn lit_fixture(name: &str) -> String {
    std::fs::read_to_string(format!(
        "{}/tests/fixtures/lit/{name}",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap()
}

fn u() -> Usage {
    Usage {
        input_tokens: 1000,
        output_tokens: 200,
        ..Usage::default()
    }
}

fn empty_list() -> serde_json::Value {
    json!({
        "searches": [{"query": REPLY_MARK, "source": "pubmed", "hits": 1}],
        "selected": [], "considered_not_selected": [], "possible_contradictions": []
    })
}

#[tokio::test]
async fn the_empty_study_list_is_logged_with_numbers_only() {
    let cap = Captured::default();
    let writer = cap.clone();
    tracing::subscriber::set_global_default(
        tracing_subscriber::fmt()
            .with_max_level(tracing::Level::TRACE)
            .with_ansi(false)
            .with_writer(move || writer.clone())
            .finish(),
    )
    .expect("first and only global subscriber in this binary");

    let Ok(url) = std::env::var("TEST_DATABASE_URL") else {
        eprintln!("SKIP: TEST_DATABASE_URL not set");
        return;
    };
    let pool = lazy_pool(&SecretString::from(url)).expect("lazy pool");
    migrate(&pool).await.expect("migrate");

    let server = MockServer::start().await;
    for (route, body) in [
        (
            "/entrez/eutils/esearch.fcgi",
            lit_fixture("pubmed_esearch_hernia.json"),
        ),
        (
            "/entrez/eutils/esummary.fcgi",
            lit_fixture("pubmed_esummary_hernia.json"),
        ),
    ] {
        Mock::given(method("GET"))
            .and(path(route))
            .respond_with(ResponseTemplate::new(200).set_body_string(body))
            .mount(&server)
            .await;
    }
    let oa = OaPolicy::default();
    let client = AllowlistedClient::new("Incisive/test", oa.clone())
        .with_base_override(server.uri())
        .with_retry_backoff(Duration::from_millis(1));
    let lit = Arc::new(Literature {
        pubmed: PubMed::new(client.clone(), None).with_min_interval(Duration::ZERO),
        epmc: EuropePmc::new(client, oa),
    });

    // One search, the end of research, then empty twice.
    let llm = Arc::new(
        ScriptedLlm::new()
            .reply_blocks(
                vec![
                    ContentBlock::Text {
                        text: format!("Searching {REPLY_MARK}."),
                    },
                    ContentBlock::ToolUse {
                        id: "s1".into(),
                        name: "search_pubmed".into(),
                        input: json!({"query": QUERY, "max_results": 5}),
                    },
                ],
                Some(StopReason::ToolUse),
                u(),
            )
            .reply_blocks(
                vec![ContentBlock::Text {
                    text: format!("Done {REPLY_MARK}."),
                }],
                Some(StopReason::EndTurn),
                u(),
            )
            .reply_json(empty_list(), u())
            .reply_json(empty_list(), u()),
    );

    let email = format!("s3-log-{}@example.com", Uuid::new_v4());
    let uid: i64 = sqlx::query_scalar(
        "INSERT INTO incisive.users (email, display_name, role, daily_cap_paise) \
         VALUES ($1, 'T', 'surgeon', 100000) RETURNING id",
    )
    .bind(&email)
    .fetch_one(&pool)
    .await
    .unwrap();
    let user = incisive_app::identity::find_user(&pool, &email)
        .await
        .unwrap()
        .unwrap();
    let run = create_run(&pool, uid, MARKER, RunMode::Quick, 6000)
        .await
        .unwrap()
        .unwrap();
    set_status(&pool, run, RunStatus::Running, None)
        .await
        .unwrap();
    let prices = Prices::from_json(r#"{"m-test": {"input": 1.0, "output": 5.0}}"#).unwrap();
    let settings = DepthSettings::QUICK;
    let ctx = RunContext {
        state: AppState {
            pool: pool.clone(),
            verifier: Arc::new(IapVerifier::new(
                "aud".into(),
                JwkSource::Static(serde_json::from_str(r#"{"keys":[]}"#).unwrap()),
            )),
            gate: Arc::new(OfflineClassifier),
            prices: prices.clone(),
            usd_inr: 88.0,
            caps: CapsConfig {
                global_monthly_paise: i64::MAX / 4,
            },
            gate_model: MODEL.into(),
            dev_mode: true,
            runs: RunsConfig::default(),
            quick_engine: QuickEngine::Team,
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
            worker: WorkerHandle::default(),
            llm: None,
            lit: lit.clone(),
        },
        llm: incisive_app::agent::testing::agent_llm(llm.clone(), MODEL),
        lit,
        run_id: run,
        user,
        question: MARKER.to_string(),
        mode: RunMode::Quick,
        settings,
        budget: Arc::new(
            RunBudget::new(run, uid, 6000, Instant::now(), prices, 88.0)
                .with_time_limit(settings.agent_time_limit_s),
        ),
        cancel: CancelFlag::default(),
    };

    let r = research(&ctx, &ctx.llm, ctx.lit.clone()).await.unwrap();
    assert!(r.empty_selection);
    assert_eq!(
        llm.requests().len(),
        4,
        "two turns, the final call, one retry"
    );

    let logs = String::from_utf8(cap.0.lock().unwrap().clone()).unwrap();
    let warn = logs
        .lines()
        .find(|l| l.contains("researcher returned an empty study list"))
        .unwrap_or_else(|| panic!("warning missing from capture:\n{logs}"));
    assert!(warn.contains("WARN"), "{warn}");
    for field in ["seen=5", "reported=0", "considered=0", "rounds=2"] {
        assert!(warn.contains(field), "{field} missing: {warn}");
    }
    let retry = logs
        .lines()
        .find(|l| l.contains("researcher study list retry"))
        .unwrap_or_else(|| panic!("retry outcome missing from capture:\n{logs}"));
    assert!(retry.contains("outcome=\"empty\""), "{retry}");
    for secret in [
        MARKER,
        "Zebrafinch",
        QUERY,
        "quokka7741",
        REPLY_MARK,
        RETRY_LINE,
    ] {
        assert!(!logs.contains(secret), "{secret} reached tracing output");
    }
}
