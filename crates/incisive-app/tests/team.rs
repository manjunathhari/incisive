//! Phase 3 Stage 2: the evidence team for Quick mode (researcher → writer →
//! stored AnswerV3), with a scripted Claude and wiremock literature (no
//! network). Needs `TEST_DATABASE_URL` (skipped when unset).

use std::sync::Arc;
use std::time::{Duration, Instant};

use claude_client::{ContentBlock, Effort, MessagesRequest, StopReason, Usage};
use incisive_app::agent::llm::{AgentModels, LlmClient, ScriptedLlm};
use incisive_app::agent::researcher::research;
use incisive_app::agent::{AgentLlm, Literature, RunContext};
use incisive_app::db::{lazy_pool, migrate};
use incisive_app::gate::OfflineClassifier;
use incisive_app::identity::{IapVerifier, JwkSource};
use incisive_app::lit::europepmc::EuropePmc;
use incisive_app::lit::http::{AllowlistedClient, OaPolicy};
use incisive_app::lit::pubmed::PubMed;
use incisive_app::runs::budget::RunBudget;
use incisive_app::runs::store::{create_run, get_run_for_user};
use incisive_app::runs::worker::{CancelFlag, WorkerHandle};
use incisive_app::runs::{DepthSettings, QuickEngine, RunMode, RunsConfig, StepLine};
use incisive_app::server::AppState;
use incisive_app::spend::{CapCheck, CapsConfig, Prices, reserve};
use secrecy::SecretString;
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const MODEL: &str = "m-test";
const QUESTION: &str = "In adults with a primary inguinal hernia, does mesh type change chronic pain after laparoscopic repair?";
const ESEARCH: &str = "/entrez/eutils/esearch.fcgi";
const ESUMMARY: &str = "/entrez/eutils/esummary.fcgi";
const EFETCH: &str = "/entrez/eutils/efetch.fcgi";
const ELINK: &str = "/entrez/eutils/elink.fcgi";
const EPMC_SEARCH: &str = "/europepmc/webservices/rest/search";
/// In the hernia esearch + esummary fixtures (Randomized Controlled Trials).
const SEEN_A: &str = "41060645";
const SEEN_B: &str = "39979619";
/// Never returned by any fixture.
const UNSEEN: &str = "12345678";

fn usage(i: u32, o: u32) -> Usage {
    Usage {
        input_tokens: i,
        output_tokens: o,
    }
}

fn u() -> Usage {
    usage(1000, 200)
}

fn prices() -> Prices {
    Prices::from_json(
        r#"{"m-test": {"input": 1.0, "output": 5.0}, "claude-opus-5-5": {"input": 4.0, "output": 20.0}}"#,
    )
    .unwrap()
}

fn lit_fixture(name: &str) -> String {
    std::fs::read_to_string(format!(
        "{}/tests/fixtures/lit/{name}",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap()
}

async fn pool() -> Option<PgPool> {
    let Ok(url) = std::env::var("TEST_DATABASE_URL") else {
        eprintln!("SKIP: TEST_DATABASE_URL not set");
        return None;
    };
    let pool = lazy_pool(&SecretString::from(url)).expect("lazy pool");
    migrate(&pool).await.expect("migrate");
    Some(pool)
}

fn literature(server: &MockServer) -> Arc<Literature> {
    let oa = OaPolicy::default();
    let client = AllowlistedClient::new("Incisive/test", oa.clone())
        .with_base_override(server.uri())
        .with_retry_backoff(Duration::from_millis(1));
    Arc::new(Literature {
        pubmed: PubMed::new(client.clone(), None).with_min_interval(Duration::ZERO),
        epmc: EuropePmc::new(client, oa),
    })
}

async fn mount(server: &MockServer, route: &str, status: u16, body: String) {
    Mock::given(method("GET"))
        .and(path(route))
        .respond_with(ResponseTemplate::new(status).set_body_string(body))
        .mount(server)
        .await;
}

/// PubMed hernia fixtures: esearch (count 327), esummary (5 RCTs), efetch
/// (their abstracts); elink and Europe PMC answer nothing useful.
async fn hernia_server() -> MockServer {
    let server = MockServer::start().await;
    mount(
        &server,
        ESEARCH,
        200,
        lit_fixture("pubmed_esearch_hernia.json"),
    )
    .await;
    mount(
        &server,
        ESUMMARY,
        200,
        lit_fixture("pubmed_esummary_hernia.json"),
    )
    .await;
    mount(
        &server,
        EFETCH,
        200,
        lit_fixture("pubmed_efetch_hernia.xml"),
    )
    .await;
    mount(&server, ELINK, 500, String::new()).await;
    mount(&server, EPMC_SEARCH, 500, String::new()).await;
    server
}

fn app_state(pool: PgPool, lit: Arc<Literature>) -> AppState {
    AppState {
        pool,
        verifier: Arc::new(IapVerifier::new(
            "aud".into(),
            JwkSource::Static(serde_json::from_str(r#"{"keys":[]}"#).unwrap()),
        )),
        gate: Arc::new(OfflineClassifier),
        prices: prices(),
        usd_inr: 88.0,
        caps: CapsConfig {
            global_monthly_paise: i64::MAX / 4,
        },
        gate_model: MODEL.into(),
        dev_mode: true,
        runs: RunsConfig::default(),
        quick_engine: QuickEngine::Team,
        worker: WorkerHandle::default(),
        llm: None,
        lit,
    }
}

fn agent_llm(llm: Arc<dyn LlmClient>, model: &str) -> Arc<AgentLlm> {
    incisive_app::agent::testing::agent_llm(llm, model)
}

/// A fresh user (daily cap ₹1,000) and a Quick run with `limit` reserved.
async fn new_run(pool: &PgPool, limit: i64) -> (i64, Uuid, incisive_app::identity::User) {
    let email = format!("team-{}@example.com", Uuid::new_v4());
    let uid: i64 = sqlx::query_scalar(
        "INSERT INTO incisive.users (email, display_name, role, daily_cap_paise) \
         VALUES ($1, 'T', 'surgeon', 100000) RETURNING id",
    )
    .bind(&email)
    .fetch_one(pool)
    .await
    .unwrap();
    let run = create_run(pool, uid, QUESTION, RunMode::Quick, limit)
        .await
        .unwrap()
        .unwrap();
    let user = incisive_app::identity::find_user(pool, &email)
        .await
        .unwrap()
        .unwrap();
    let caps = CapsConfig {
        global_monthly_paise: i64::MAX / 4,
    };
    let c = reserve(pool, &user, &caps, limit, run, "run")
        .await
        .unwrap();
    assert!(matches!(c, CapCheck::Ok { .. }));
    (uid, run, user)
}

/// A Quick run context over `lit`, `llm` and a budget of `limit`.
async fn quick_ctx(
    pool: &PgPool,
    lit: Arc<Literature>,
    llm: Arc<AgentLlm>,
    limit: i64,
    settings: DepthSettings,
) -> (RunContext, i64) {
    let (uid, run, user) = new_run(pool, limit).await;
    let budget = Arc::new(
        RunBudget::new(run, uid, limit, Instant::now(), prices(), 88.0)
            .with_time_limit(settings.agent_time_limit_s),
    );
    let ctx = RunContext {
        state: app_state(pool.clone(), lit.clone()),
        llm,
        lit,
        run_id: run,
        user,
        question: QUESTION.to_string(),
        mode: RunMode::Quick,
        settings,
        budget,
        cancel: CancelFlag::default(),
    };
    (ctx, uid)
}

async fn steps_of(ctx: &RunContext, uid: i64) -> Vec<StepLine> {
    get_run_for_user(&ctx.state.pool, uid, ctx.run_id)
        .await
        .unwrap()
        .unwrap()
        .steps
}

fn lines(steps: &[StepLine]) -> Vec<String> {
    steps.iter().map(|s| s.line.clone()).collect()
}

fn tool_call(id: &str, name: &str, input: Value) -> ContentBlock {
    ContentBlock::ToolUse {
        id: id.into(),
        name: name.into(),
        input,
    }
}

/// A researcher turn calling `uses`.
fn turn(llm: ScriptedLlm, uses: Vec<ContentBlock>) -> ScriptedLlm {
    let mut content = vec![ContentBlock::Text {
        text: "Searching.".into(),
    }];
    content.extend(uses);
    llm.reply_blocks(content, Some(StopReason::ToolUse), u())
}

fn end_turn(llm: ScriptedLlm) -> ScriptedLlm {
    llm.reply_blocks(
        vec![ContentBlock::Text {
            text: "Done searching.".into(),
        }],
        Some(StopReason::EndTurn),
        u(),
    )
}

/// The researcher's study list selecting `selected` (pmid, design).
fn study_list(selected: &[(&str, &str)]) -> Value {
    json!({
        "searches": [{"query": "made up", "source": "pubmed", "hits": 999}],
        "selected": selected.iter().map(|(p, d)| json!({"pmid": p, "design": d, "why": format!("key study {p}")})).collect::<Vec<_>>(),
        "considered_not_selected": [{"pmid": "9657784", "reason": "open repair"}],
        "possible_contradictions": [
            {"pmid": SEEN_B, "note": "self-fixating mesh, different result"},
            {"pmid": UNSEEN, "note": "from memory"}
        ]
    })
}

/// Two research turns (a PubMed search, then abstracts), then end of turn.
fn research_script(llm: ScriptedLlm) -> ScriptedLlm {
    let llm = turn(
        llm,
        vec![tool_call(
            "s1",
            "search_pubmed",
            json!({"query": "inguinal hernia[tiab] AND mesh[tiab] AND randomized controlled trial[pt]", "max_results": 20}),
        )],
    );
    let llm = turn(
        llm,
        vec![tool_call(
            "a1",
            "get_abstracts",
            json!({"pmids": [SEEN_A, SEEN_B]}),
        )],
    );
    end_turn(llm)
}

fn tool_names(r: &MessagesRequest) -> Vec<String> {
    r.tools.iter().map(|t| t.name.clone()).collect()
}

// ---------- T4: the researcher ----------

#[tokio::test]
async fn researcher_records_searches_drops_unseen_and_reports_progress() {
    let Some(pool) = pool().await else { return };
    let server = hernia_server().await;
    let scripted = research_script(ScriptedLlm::new()).reply_json(
        study_list(&[
            (SEEN_A, "RCT"),
            (UNSEEN, "RCT"),
            (SEEN_B, "SR_MA"),
            (SEEN_A, "other"),
        ]),
        u(),
    );
    let llm = Arc::new(scripted);
    let (ctx, uid) = quick_ctx(
        &pool,
        literature(&server),
        agent_llm(llm.clone(), MODEL),
        6000,
        DepthSettings::QUICK,
    )
    .await;

    let r = research(&ctx, &ctx.llm, ctx.lit.clone()).await.unwrap();

    // Validation: the unseen PMID and the duplicate are gone.
    let ids: Vec<&str> = r.selected.iter().map(|s| s.pmid.as_str()).collect();
    assert_eq!(ids, [SEEN_A, SEEN_B]);
    assert_eq!(r.dropped_unseen, 1);
    assert_eq!(r.contradictions.len(), 1, "only notes on seen studies");
    assert_eq!(r.contradictions[0].pmid, SEEN_B);
    // The trail is what the tool returned, not what the model reported.
    assert_eq!(r.searches.len(), 1);
    assert_eq!(r.searches[0].source, "PubMed");
    assert_eq!(r.searches[0].hits, 327);
    assert!(r.searches[0].query.starts_with("inguinal hernia[tiab]"));
    assert_eq!(r.stopped_early(), None);

    // Progress lines: one per turn, then the selection, then the drop note.
    let l = lines(&steps_of(&ctx, uid).await);
    assert_eq!(
        l,
        [
            "Researcher: 1 search so far (turn 1 of 8)",
            "Researcher: 1 search so far (turn 2 of 8)",
            "Researcher: 1 search so far (turn 3 of 8)",
            "Researcher: selected 2 studies (1 RCT, 1 meta-analysis)",
            "Researcher: dropped 1 selected study not returned by this run's searches (dropped_unseen)",
        ]
    );
    for s in &l {
        assert!(!s.contains("mesh type change"), "question leaked: {s}");
    }

    // Requests: Quick tools only (no full text), the turn budget in the
    // system prompt, default effort on turns, low effort and the schema on
    // the final call.
    let reqs = llm.requests();
    assert_eq!(reqs.len(), 4, "three turns and the final call");
    for r in &reqs[..3] {
        assert_eq!(
            tool_names(r),
            [
                "search_pubmed",
                "search_europepmc",
                "get_abstracts",
                "find_related"
            ]
        );
        assert!(
            r.system
                .as_deref()
                .unwrap()
                .contains("at most 8 tool turns")
        );
        assert!(r.output_config.is_none(), "default effort on turns");
        assert_eq!(r.max_tokens, DepthSettings::QUICK.researcher_max_tokens);
        assert_eq!(r.model, MODEL);
    }
    let fin = &reqs[3];
    assert!(fin.tools.is_empty());
    let oc = fin.output_config.as_ref().expect("structured final call");
    assert!(oc.format.is_some());
    assert_eq!(oc.effort, Some(Effort::Low));
    // The question reaches the model as data, between tags.
    let first: String = reqs[0]
        .messages
        .iter()
        .flat_map(|m| &m.content)
        .filter_map(|b| match b {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert!(first.contains(&format!("<question>\n{QUESTION}\n</question>")));
}

#[tokio::test]
async fn researcher_uses_the_researcher_model_and_turn_settings() {
    let Some(pool) = pool().await else { return };
    let server = hernia_server().await;
    // Turn cap 1: one turn, then the final call.
    let llm = Arc::new(
        turn(
            ScriptedLlm::new(),
            vec![tool_call(
                "s1",
                "search_pubmed",
                json!({"query": "mesh[tiab] AND hernia[tiab]", "max_results": 5}),
            )],
        )
        .reply_json(study_list(&[(SEEN_A, "RCT")]), u()),
    );
    let models = AgentModels {
        researcher: "m-test".into(),
        writer: "unused-writer".into(),
        ..AgentModels::default()
    };
    let agent = Arc::new(AgentLlm {
        client: llm.clone(),
        models,
    });
    let settings = DepthSettings {
        researcher_turns: 1,
        researcher_max_tokens: 1234,
        ..DepthSettings::QUICK
    };
    let (ctx, uid) = quick_ctx(&pool, literature(&server), agent, 6000, settings).await;
    let r = research(&ctx, &ctx.llm, ctx.lit.clone()).await.unwrap();
    assert_eq!(r.selected.len(), 1);
    let reqs = llm.requests();
    assert_eq!(reqs.len(), 2);
    assert!(reqs.iter().all(|r| r.model == "m-test"));
    assert!(reqs.iter().all(|r| r.max_tokens == 1234));
    assert!(
        reqs[0]
            .system
            .as_deref()
            .unwrap()
            .contains("at most 1 tool turns")
    );
    let l = lines(&steps_of(&ctx, uid).await);
    assert_eq!(l[0], "Researcher: 1 search so far (turn 1 of 1)");
    assert_eq!(
        l[1],
        "Researcher: selected 1 study (1 RCT, 0 meta-analyses)"
    );
}

#[tokio::test]
async fn a_selection_seen_only_in_a_withheld_result_is_dropped() {
    let Some(pool) = pool().await else { return };
    let server = hernia_server().await;
    // A turn calling a tool the model was not offered: an error result, so
    // nothing in it counts as seen.
    let llm = Arc::new(
        turn(
            ScriptedLlm::new(),
            vec![tool_call("x1", "get_full_text", json!({"pmcid": "PMC1"}))],
        )
        .reply_blocks(
            vec![ContentBlock::Text { text: "ok".into() }],
            Some(StopReason::EndTurn),
            u(),
        )
        .reply_json(study_list(&[(SEEN_A, "RCT")]), u()),
    );
    let (ctx, _uid) = quick_ctx(
        &pool,
        literature(&server),
        agent_llm(llm.clone(), MODEL),
        6000,
        DepthSettings::QUICK,
    )
    .await;
    let r = research(&ctx, &ctx.llm, ctx.lit.clone()).await.unwrap();
    assert!(r.selected.is_empty());
    assert_eq!(r.dropped_unseen, 1);
    assert!(r.searches.is_empty());
}
