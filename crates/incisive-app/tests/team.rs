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
        ..Usage::default()
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
        full_engine: incisive_app::runs::Engine::Classic,
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
    // As the worker does: the pipeline's heartbeat needs a running run.
    incisive_app::runs::store::set_status(pool, run, incisive_app::runs::RunStatus::Running, None)
        .await
        .unwrap();
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

// ---------- T5: writer, AnswerV3, the team pipeline ----------

use incisive_app::agent::answer::NO_EVIDENCE;
use incisive_app::agent::answer_v3::{ANSWER_VERSION, AnswerV3};
use incisive_app::agent::team::run_team;
use incisive_app::agent::writer_v3::STUDIES_HEADING;
use incisive_app::agent::{PipelineFuture, PipelineOutcome};
use incisive_app::ask::{AskOutcome, me_view, start_run};
use incisive_app::runs::RunStatus;
use incisive_app::runs::store::save_answer;

const FINAL_KEY: &str = "Research transcript";
const UNKNOWN_CITE: &str = "99999999";

/// The writer's reply: cites the two seen studies, an unknown PMID in text,
/// a number from the unseen study and a trial row for a made-up PMID.
fn writer_json() -> Value {
    json!({
        "bottom_line": {
            "text": format!("Mesh type made no difference to chronic pain [{SEEN_A}; {UNKNOWN_CITE}]. <script>alert(1)</script>"),
            "certainty": "Low",
            "certainty_reason": format!("Two small trials [{SEEN_B}]")
        },
        "outcomes": [{
            "outcome": "Chronic pain",
            "rct_evidence": "Two RCTs",
            "effect_text": format!("No difference [{SEEN_A}]"),
            "numbers": [
                {"label": "VAS", "value": 1.2, "unit": null, "arm": "biological", "ci_low": null, "ci_high": null, "p": "0.4", "pmid": SEEN_A},
                {"label": "VAS", "value": 9.9, "unit": null, "arm": null, "ci_low": 1.0, "ci_high": 2.0, "p": null, "pmid": UNSEEN}
            ],
            "certainty": "Low",
            "why": "small",
            "pmids": [SEEN_A, UNKNOWN_CITE]
        }],
        "studies": [
            {"pmid": SEEN_A, "label": "Biological vs synthetic (2025)", "design": "RCT", "n": 100, "setting": "multicentre", "comparison": "biological vs synthetic", "key_results": format!("no difference [{SEEN_A}]"), "appraisal": "short follow-up"},
            {"pmid": "77777777", "label": "Invented", "design": "RCT", "n": null, "setting": "", "comparison": "", "key_results": "", "appraisal": ""}
        ],
        "conflicts": [{"text": format!("Self-fixating mesh differs [{SEEN_B}]"), "pmids": [SEEN_B]}],
        "applicability": {"case_mix": "primary hernias", "india_cost_context": "Indian mesh costs are not known.", "training": "No particular bearing."},
        "would_change": ["A larger multicentre RCT"]
    })
}

/// A full scripted Quick run: research (two tool turns and an end of turn),
/// the study list (keyed on the transcript) and the writer (keyed on its
/// study heading).
fn team_script() -> ScriptedLlm {
    research_script(ScriptedLlm::new())
        .when(FINAL_KEY)
        .reply_json(
            study_list(&[(SEEN_A, "RCT"), (UNSEEN, "RCT"), (SEEN_B, "SR_MA")]),
            u(),
        )
        .when(STUDIES_HEADING)
        .reply_json(writer_json(), usage(3000, 2500))
}

fn team_state(pool: &PgPool, lit: Arc<Literature>, llm: Arc<AgentLlm>) -> AppState {
    let mut st = app_state(pool.clone(), lit);
    st.llm = Some(llm);
    st
}

/// Poll (up to 20 s) until the run is no longer active.
async fn wait_done(pool: &PgPool, uid: i64, run: Uuid) -> incisive_app::runs::RunView {
    for _ in 0..2000 {
        let v = get_run_for_user(pool, uid, run).await.unwrap().unwrap();
        if !v.summary.status.is_active() {
            return v;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("run {run} did not finish");
}

async fn wait_idle(st: &AppState, run: Uuid) {
    for _ in 0..2000 {
        if !st.worker.is_running(run) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("run {run} still running");
}

async fn run_limit(pool: &PgPool, run: Uuid) -> i64 {
    sqlx::query_scalar("SELECT limit_paise FROM incisive.runs WHERE id = $1")
        .bind(run)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn charges(pool: &PgPool, uid: i64) -> Vec<i64> {
    sqlx::query_scalar(
        "SELECT paise FROM incisive.spend WHERE user_id = $1 AND kind = 'charge' ORDER BY id",
    )
    .bind(uid)
    .fetch_all(pool)
    .await
    .unwrap()
}

async fn new_user(pool: &PgPool) -> (i64, incisive_app::identity::User) {
    let email = format!("team-{}@example.com", Uuid::new_v4());
    let uid: i64 = sqlx::query_scalar(
        "INSERT INTO incisive.users (email, display_name, role, daily_cap_paise) \
         VALUES ($1, 'T', 'surgeon', 100000) RETURNING id",
    )
    .bind(&email)
    .fetch_one(pool)
    .await
    .unwrap();
    let user = incisive_app::identity::find_user(pool, &email)
        .await
        .unwrap()
        .unwrap();
    (uid, user)
}

fn request_user_text(r: &MessagesRequest) -> String {
    r.messages
        .iter()
        .flat_map(|m| &m.content)
        .filter_map(|b| match b {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

/// End to end through `start_run` and the worker: researcher turns →
/// study list (an unseen PMID dropped) → writer (an unknown citation
/// removed, a number and a trial row of unknown studies dropped) → stored
/// AnswerV3 (version 3) → the run view the page reads.
#[tokio::test]
async fn quick_team_run_end_to_end_through_the_worker() {
    let Some(pool) = pool().await else { return };
    let server = hernia_server().await;
    let llm = Arc::new(team_script());
    let st = team_state(&pool, literature(&server), agent_llm(llm.clone(), MODEL));
    let (uid, user) = new_user(&pool).await;

    let out = start_run(&st, Uuid::new_v4(), &user, QUESTION, RunMode::Quick, None)
        .await
        .unwrap();
    let AskOutcome::RunStarted { run_id, .. } = out else {
        panic!("{out:?}")
    };
    // The team's cost limit is reserved: ₹100 (owner ruling R-S2a).
    assert_eq!(run_limit(&pool, run_id).await, 10000);
    let v = wait_done(&pool, uid, run_id).await;
    wait_idle(&st, run_id).await;
    assert_eq!(v.summary.status, RunStatus::Done, "{:?}", v.steps);
    assert_eq!(v.answer_version, ANSWER_VERSION);
    let a: AnswerV3 = serde_json::from_value(v.answer.clone().unwrap()).unwrap();
    assert_eq!(a.version, 3);
    assert_eq!(a.question, QUESTION);
    assert_eq!(a.mode, RunMode::Quick);

    // References: code-built from the fetched records, selection order.
    let refs: Vec<(u16, &str)> = a
        .references
        .iter()
        .map(|r| (r.n, r.pmid.as_str()))
        .collect();
    assert_eq!(refs, [(1, SEEN_A), (2, SEEN_B)]);
    assert!(
        a.references[0]
            .title
            .starts_with("Biological vs Synthetic Mesh")
    );
    assert!(a.references.iter().all(|r| !r.retracted));
    // Integrity: the unseen study, the unknown citations, the number and
    // the invented row.
    assert_eq!(a.integrity.dropped_unseen, 1);
    assert_eq!(a.integrity.removed_citations, 2);
    assert_eq!(a.integrity.dropped_numbers, 1);
    assert_eq!(a.integrity.dropped_studies, 1);
    let w = a.answer.as_ref().unwrap();
    assert!(!w.bottom_line.text.contains(UNKNOWN_CITE));
    assert!(w.bottom_line.text.contains(&format!("[{SEEN_A}]")));
    assert_eq!(w.outcomes[0].numbers.len(), 1);
    assert_eq!(w.outcomes[0].pmids, [SEEN_A]);
    assert_eq!(w.studies.len(), 1);
    // Trail: recorded by code.
    assert_eq!(a.searches.len(), 1);
    assert_eq!(a.searches[0].hits, 327);
    assert_eq!(a.stopped_early, None);
    // Cost: every call charged, and the answer's cost is the run's.
    let rows = charges(&pool, uid).await;
    assert_eq!(rows.len(), 5, "three turns, the study list, the writer");
    assert_eq!(a.cost_paise, rows.iter().sum::<i64>());
    assert_eq!(v.summary.cost_paise, a.cost_paise);

    // Progress lines.
    let l = lines(&v.steps);
    for want in [
        "Researcher: searching PubMed and Europe PMC",
        "Researcher: 1 search so far (turn 1 of 8)",
        "Researcher: selected 2 studies (1 RCT, 1 meta-analysis)",
        "Writer: writing the answer from 2 studies (2 with an abstract)",
        "Citation check: removed 2 unknown citations, dropped 1 number and 1 trial row",
        "Answer ready",
    ] {
        assert!(l.iter().any(|x| x == want), "missing {want:?} in {l:?}");
    }

    // The writer's request: one structured call, writer settings, the
    // abstracts wrapped by PMID, the researcher's notes and search log.
    let reqs = llm.requests();
    let writer: Vec<&MessagesRequest> = reqs
        .iter()
        .filter(|r| request_user_text(r).contains(STUDIES_HEADING))
        .collect();
    assert_eq!(writer.len(), 1);
    let wr = writer[0];
    assert_eq!(wr.max_tokens, DepthSettings::QUICK.writer_max_tokens);
    assert_eq!(wr.max_tokens, 16_000);
    assert!(wr.tools.is_empty());
    let oc = wr.output_config.as_ref().unwrap();
    assert!(oc.format.is_some());
    assert_eq!(
        oc.effort, None,
        "the writer runs at the model's default effort"
    );
    let sys = wr.system.as_deref().unwrap();
    // Stage 4 item 9: written for any clinician; India context kept.
    assert!(!sys.contains("Bengaluru") && sys.contains("Indian cost context"));
    let text = request_user_text(wr);
    assert!(text.contains(&format!("<source id=\"pmid-{SEEN_A}\">")));
    assert!(text.contains(&format!("<source id=\"pmid-{SEEN_B}\">")));
    assert!(!text.contains(&format!("pmid-{UNSEEN}")));
    assert!(
        text.contains(&format!("key study {SEEN_A}")),
        "the researcher's why"
    );
    assert!(text.contains("self-fixating mesh, different result"));
    assert!(text.contains("(327 hits)"));
    assert!(text.contains("Abstract:"));
}

/// The rollback switch: `classic` keeps Quick on the Phase 2 pipeline, its
/// ₹15 limit and its time limit; `team` uses the Phase 3 ones. The Ask page
/// shows the engine's limit.
#[tokio::test]
async fn the_engine_switch_picks_the_pipeline_and_the_limits() {
    let Some(pool) = pool().await else { return };
    fn hard_stop(ctx: &RunContext) -> PipelineFuture<'_> {
        Box::pin(async move {
            PipelineOutcome::Done(json!({
                "hard_stop_s": ctx.budget.hard_stop().as_secs(),
                "wrap_up_s": ctx.budget.wrap_up_after().as_secs()
            }))
        })
    }
    // Team: 6 minutes; research wraps up at 360 − 150 s (the writer's time
    // allowance). Classic: Phase 2's 300 s, wrap-up at 60%.
    for (engine, limit, hard_stop_s, wrap_up_s) in [
        (QuickEngine::Team, 10000, 360 + 60, 360 - 150),
        (QuickEngine::Classic, 1500, 300 + 60, 180),
    ] {
        let mut st = app_state(
            pool.clone(),
            incisive_app::agent::testing::offline_literature(),
        );
        st.quick_engine = engine;
        st.worker = WorkerHandle::with_pipeline(hard_stop);
        st.llm = Some(incisive_app::agent::testing::idle_llm(MODEL));
        let (uid, user) = new_user(&pool).await;
        assert_eq!(me_view(&st, &user).await.unwrap().quick_limit_paise, limit);
        let AskOutcome::RunStarted { run_id, .. } =
            start_run(&st, Uuid::new_v4(), &user, QUESTION, RunMode::Quick, None)
                .await
                .unwrap()
        else {
            panic!()
        };
        assert_eq!(run_limit(&pool, run_id).await, limit, "{engine:?}");
        let v = wait_done(&pool, uid, run_id).await;
        wait_idle(&st, run_id).await;
        assert_eq!(
            v.answer,
            Some(json!({"hard_stop_s": hard_stop_s, "wrap_up_s": wrap_up_s})),
            "{engine:?}"
        );
        // Focused is unchanged by the switch.
        assert_eq!(me_view(&st, &user).await.unwrap().focused_limit_paise, 4000);
    }

    // The default pipeline dispatches on the engine: classic runs Frame.
    let llm = Arc::new(ScriptedLlm::new());
    let mut st = team_state(
        &pool,
        incisive_app::agent::testing::offline_literature(),
        agent_llm(llm.clone(), MODEL),
    );
    st.quick_engine = QuickEngine::Classic;
    let (uid, user) = new_user(&pool).await;
    let AskOutcome::RunStarted { run_id, .. } =
        start_run(&st, Uuid::new_v4(), &user, QUESTION, RunMode::Quick, None)
            .await
            .unwrap()
    else {
        panic!()
    };
    let v = wait_done(&pool, uid, run_id).await;
    wait_idle(&st, run_id).await;
    assert_eq!(v.summary.status, RunStatus::Failed);
    let reqs = llm.requests();
    assert!(
        reqs[0]
            .system
            .as_deref()
            .unwrap()
            .contains("You frame clinical questions"),
        "classic Quick starts with Frame"
    );
}

/// Stage 3 §0: `FULL_ENGINE` picks Full's pipeline and limits on its own.
/// `team`: ₹300, 10 minutes (hard stop 660 s; owner ruling R-S3a),
/// research wrapping up at 600 − 150 (writer) − 180 (review) s. `classic`: exactly Phase 2
/// Focused (₹40, 300 s, wrap-up at 60%, the Frame step first). Quick is
/// untouched by it.
#[tokio::test]
async fn the_full_engine_switch_picks_the_pipeline_and_the_limits() {
    let Some(pool) = pool().await else { return };
    fn timing(ctx: &RunContext) -> PipelineFuture<'_> {
        Box::pin(async move {
            PipelineOutcome::Done(json!({
                "hard_stop_s": ctx.budget.hard_stop().as_secs(),
                "wrap_up_s": ctx.budget.wrap_up_after().as_secs()
            }))
        })
    }
    use incisive_app::runs::Engine;
    for (engine, limit, hard_stop_s, wrap_up_s) in [
        (Engine::Team, 30000, 600 + 60, 600 - 150 - 180),
        (Engine::Classic, 4000, 300 + 60, 180),
    ] {
        let mut st = app_state(
            pool.clone(),
            incisive_app::agent::testing::offline_literature(),
        );
        st.full_engine = engine;
        st.worker = WorkerHandle::with_pipeline(timing);
        st.llm = Some(incisive_app::agent::testing::idle_llm(MODEL));
        let (uid, user) = new_user(&pool).await;
        let me = me_view(&st, &user).await.unwrap();
        assert_eq!(me.focused_limit_paise, limit, "{engine:?}");
        assert_eq!(me.quick_limit_paise, 10000, "Quick keeps its own switch");
        let AskOutcome::RunStarted { run_id, .. } =
            start_run(&st, Uuid::new_v4(), &user, QUESTION, RunMode::Focused, None)
                .await
                .unwrap()
        else {
            panic!()
        };
        assert_eq!(run_limit(&pool, run_id).await, limit, "{engine:?}");
        let v = wait_done(&pool, uid, run_id).await;
        wait_idle(&st, run_id).await;
        assert_eq!(
            v.answer,
            Some(json!({"hard_stop_s": hard_stop_s, "wrap_up_s": wrap_up_s})),
            "{engine:?}"
        );
    }

    // The default pipeline: classic Full runs Phase 2's Frame first.
    let llm = Arc::new(ScriptedLlm::new());
    let mut st = team_state(
        &pool,
        incisive_app::agent::testing::offline_literature(),
        agent_llm(llm.clone(), MODEL),
    );
    st.full_engine = Engine::Classic;
    let (uid, user) = new_user(&pool).await;
    let AskOutcome::RunStarted { run_id, .. } =
        start_run(&st, Uuid::new_v4(), &user, QUESTION, RunMode::Focused, None)
            .await
            .unwrap()
    else {
        panic!()
    };
    let v = wait_done(&pool, uid, run_id).await;
    wait_idle(&st, run_id).await;
    assert_eq!(v.summary.status, RunStatus::Failed);
    let reqs = llm.requests();
    assert!(
        reqs[0]
            .system
            .as_deref()
            .unwrap()
            .contains("You frame clinical questions"),
        "classic Full starts with Frame"
    );
    assert!(
        reqs.iter().all(|r| r.tools.is_empty()),
        "no researcher turn"
    );
}

/// Nothing valid selected after a complete search: done, "No relevant
/// evidence found.", and no writer call.
#[tokio::test]
async fn no_valid_selection_is_no_relevant_evidence_without_a_writer_call() {
    let Some(pool) = pool().await else { return };
    let server = hernia_server().await;
    let llm = Arc::new(
        research_script(ScriptedLlm::new()).reply_json(study_list(&[(UNSEEN, "RCT")]), u()),
    );
    let (ctx, _uid) = quick_ctx(
        &pool,
        literature(&server),
        agent_llm(llm.clone(), MODEL),
        6000,
        DepthSettings::QUICK,
    )
    .await;
    let out = run_team(&ctx).await;
    let PipelineOutcome::Done(body) = out else {
        panic!("{out:?}")
    };
    let a: AnswerV3 = serde_json::from_value(body).unwrap();
    assert!(a.answer.is_none());
    assert_eq!(a.empty_reason.as_deref(), Some(NO_EVIDENCE));
    assert_eq!(a.integrity.dropped_unseen, 1);
    assert!(a.references.is_empty());
    assert_eq!(llm.requests().len(), 4, "no writer call");
}

/// Cancel during research: the turn in flight is charged, nothing after it
/// runs, and the writer is never called.
#[tokio::test]
async fn cancel_mid_research_stops_before_the_writer() {
    let Some(pool) = pool().await else { return };
    let server = hernia_server().await;
    let cancel = CancelFlag::default();
    let c2 = cancel.clone();
    // The first turn cancels the run while it is in flight.
    let llm = Arc::new(team_script().until(
        "<question>",
        Arc::new(move || {
            c2.cancel();
            true
        }),
    ));
    let (mut ctx, uid) = quick_ctx(
        &pool,
        literature(&server),
        agent_llm(llm.clone(), MODEL),
        6000,
        DepthSettings::QUICK,
    )
    .await;
    ctx.cancel = cancel;
    // The run's calls must see this flag: rebuild nothing else.
    assert_eq!(run_team(&ctx).await, PipelineOutcome::Failed("cancelled"));
    let reqs = llm.requests();
    assert_eq!(reqs.len(), 1, "only the first turn was sent");
    assert!(
        !reqs
            .iter()
            .any(|r| request_user_text(r).contains(STUDIES_HEADING))
    );
    // Its charge is booked.
    for _ in 0..200 {
        if charges(&pool, uid).await.len() == 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(charges(&pool, uid).await.len(), 1);
}

/// Past the hard stop during research: the run fails `hard_stop` before any
/// tool runs or the writer is called.
#[tokio::test]
async fn hard_stop_mid_research_fails_the_run() {
    let Some(pool) = pool().await else { return };
    let server = hernia_server().await;
    let llm = Arc::new(team_script().delay(Duration::from_millis(1200)));
    let settings = DepthSettings::QUICK;
    let (uid, run, user) = new_run(&pool, 6000).await;
    incisive_app::runs::store::set_status(&pool, run, RunStatus::Running, None)
        .await
        .unwrap();
    let lit = literature(&server);
    // One second before the hard stop: the first turn (1.2 s) crosses it.
    let hard = Duration::from_secs(u64::from(settings.agent_time_limit_s) + 60);
    let budget = Arc::new(
        RunBudget::new(run, uid, 6000, Instant::now(), prices(), 88.0)
            .with_time_limit(settings.agent_time_limit_s)
            .aged_for_test(hard - Duration::from_secs(1)),
    );
    let ctx = RunContext {
        state: app_state(pool.clone(), lit.clone()),
        llm: agent_llm(llm.clone(), MODEL),
        lit,
        run_id: run,
        user,
        question: QUESTION.to_string(),
        mode: RunMode::Quick,
        settings,
        budget,
        cancel: CancelFlag::default(),
    };
    assert_eq!(run_team(&ctx).await, PipelineOutcome::Failed("hard_stop"));
    assert_eq!(llm.requests().len(), 1);
    assert!(
        server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .all(|r| r.url.path() != ESEARCH),
        "no tool ran after the hard stop"
    );
}

/// Old answers keep loading as version 2; a v3 body is stored as version 3;
/// delete and retention remove both.
#[tokio::test]
async fn answer_versions_are_stored_and_deleted_with_the_run() {
    let Some(pool) = pool().await else { return };
    let (uid, v2_run, _) = new_run(&pool, 100).await;
    incisive_app::runs::store::set_status(&pool, v2_run, RunStatus::Running, None)
        .await
        .unwrap();
    assert!(
        save_answer(&pool, v2_run, &json!({"question": "q", "bottom_line": []}))
            .await
            .unwrap()
    );
    let v = get_run_for_user(&pool, uid, v2_run).await.unwrap().unwrap();
    assert_eq!(v.answer_version, 2);
    // The same run's answer replaced by a v3 body.
    assert!(
        save_answer(&pool, v2_run, &json!({"version": 3, "question": "q"}))
            .await
            .unwrap()
    );
    let v = get_run_for_user(&pool, uid, v2_run).await.unwrap().unwrap();
    assert_eq!(v.answer_version, 3);
    // Delete cascades.
    assert!(
        incisive_app::runs::store::delete_run(&pool, uid, v2_run)
            .await
            .unwrap()
    );
    let left: i64 =
        sqlx::query_scalar("SELECT count(*) FROM incisive.run_answers WHERE run_id = $1")
            .bind(v2_run)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(left, 0);
    // A run with no answer reads as version 2.
    let (uid2, run2, _) = new_run(&pool, 100).await;
    let v = get_run_for_user(&pool, uid2, run2).await.unwrap().unwrap();
    assert_eq!((v.answer, v.answer_version), (None, 2));
}

// ---------- money: the writer is always reached; ₹60 measured ----------

const OPUS: &str = "claude-opus-5-5";

/// Opus-sized usage for a scripted run: each a fraction of what its call
/// holds, as real calls are (input estimated at bytes / 3; output held at
/// `max_tokens`).
fn opus_script(turns: usize) -> ScriptedLlm {
    let mut llm = ScriptedLlm::new();
    for i in 0..turns {
        llm = llm.reply_blocks(
            vec![
                ContentBlock::Text {
                    text: "Searching from another angle.".into(),
                },
                tool_call(
                    &format!("s{i}"),
                    "search_pubmed",
                    json!({"query": format!("mesh[tiab] AND hernia[tiab] AND {i}[dp]"), "max_results": 20}),
                ),
                tool_call(
                    &format!("a{i}"),
                    "get_abstracts",
                    json!({"pmids": [SEEN_A, SEEN_B, "35777031", "9657784", "23355162"]}),
                ),
            ],
            Some(StopReason::ToolUse),
            usage(1500, 1500),
        );
    }
    for _ in 0..3 {
        llm = llm.when(FINAL_KEY).reply_json(
            study_list(&[(SEEN_A, "RCT"), (SEEN_B, "SR_MA")]),
            usage(2000, 800),
        );
    }
    for _ in 0..3 {
        llm = llm
            .when(STUDIES_HEADING)
            .reply_json(writer_json(), usage(2500, 4000));
    }
    llm
}

async fn opus_ctx(
    pool: &PgPool,
    server: &MockServer,
    llm: &Arc<ScriptedLlm>,
    limit: i64,
) -> (RunContext, i64) {
    quick_ctx(
        pool,
        literature(server),
        agent_llm(llm.clone(), OPUS),
        limit,
        DepthSettings::QUICK,
    )
    .await
}

fn writer_calls(llm: &ScriptedLlm) -> usize {
    llm.requests()
        .iter()
        .filter(|r| request_user_text(r).contains(STUDIES_HEADING))
        .count()
}

/// Real Opus prices (USD 4 / 20 per Mtok, ₹88 per USD). For every cost
/// limit from far too small to ample: either no call is made at all
/// (`over_budget`: not even the first turn fits beside the writer's reserve
/// and the researcher's final call), or the researcher ran and the writer
/// was reached; spending never passes the limit. Prints the smallest
/// limit that reaches the writer (the measured minimal Quick run).
#[tokio::test]
async fn the_writer_is_always_reached_under_a_tight_budget() {
    let Some(pool) = pool().await else { return };
    let server = hernia_server().await;
    let mut min_viable = None;
    for limit in (1000..=12000).step_by(250) {
        let llm = Arc::new(opus_script(8));
        let (ctx, uid) = opus_ctx(&pool, &server, &llm, limit).await;
        let out = run_team(&ctx).await;
        let sent = llm.requests().len();
        // Let late charges land.
        tokio::time::sleep(Duration::from_millis(20)).await;
        let spent: i64 = charges(&pool, uid).await.iter().sum();
        assert!(spent <= limit, "limit {limit}: spent {spent}");
        assert_eq!(ctx.budget.held(), 0, "limit {limit}");
        if sent == 0 {
            assert_eq!(out, PipelineOutcome::Failed("over_budget"), "limit {limit}");
            assert!(
                min_viable.is_none(),
                "limit {limit}: refused after a smaller limit ran"
            );
            continue;
        }
        assert_eq!(
            writer_calls(&llm),
            1,
            "limit {limit}: research ran ({sent} calls) but the writer was not reached: {out:?}"
        );
        assert!(
            matches!(out, PipelineOutcome::Done(_) | PipelineOutcome::Partial(_)),
            "limit {limit}: {out:?}"
        );
        min_viable.get_or_insert(limit);
    }
    let mut hi = min_viable.expect("some limit up to ₹70 reaches the writer");
    // Narrow it to the paisa (the property is monotone in the limit).
    let mut lo = hi - 250;
    while hi - lo > 1 {
        let mid = (lo + hi) / 2;
        let llm = Arc::new(opus_script(8));
        let (ctx, _uid) = opus_ctx(&pool, &server, &llm, mid).await;
        let _ = run_team(&ctx).await;
        if writer_calls(&llm) == 1 {
            hi = mid;
        } else {
            assert!(
                llm.requests().is_empty(),
                "limit {mid}: research ran without the writer"
            );
            lo = mid;
        }
    }
    let min_viable = hi;
    // Its parts: the writer's reserve and the first turn's hold, at the
    // same prices; the rest is the researcher's kept final-call share.
    let llm = Arc::new(opus_script(1));
    let (ctx, _uid) = opus_ctx(&pool, &server, &llm, min_viable).await;
    let reserve = incisive_app::agent::writer_v3::reserve_for(
        &ctx.llm,
        &ctx.budget,
        QUESTION,
        DepthSettings::QUICK.writer_max_tokens,
    )
    .unwrap();
    let call = ctx.llm.call(&ctx, OPUS, "researcher");
    let tools = incisive_app::agent::tools::lit::LitTools::new(
        ctx.lit.clone(),
        pool.clone(),
        ctx.run_id,
        QUESTION,
    )
    .registry_of(&incisive_app::agent::researcher::QUICK_TOOLS)
    .defs();
    let first = vec![claude_client::Message {
        role: claude_client::Role::User,
        content: vec![ContentBlock::Text {
            text: incisive_app::agent::researcher::task(QUESTION),
        }],
    }];
    let turn1 = call
        .turn_estimate(
            &incisive_app::agent::researcher::system_prompt(8, false),
            &first,
            &tools,
            None,
            DepthSettings::QUICK.researcher_max_tokens,
        )
        .unwrap();
    eprintln!(
        "MEASURED: smallest Quick limit that reaches the writer at Opus prices (USD_INR 88): {min_viable} paise \
         = writer reserve {reserve} + researcher turn-1 hold {turn1} + researcher final-call share {}",
        min_viable - reserve - turn1
    );
    assert!(
        min_viable <= DepthSettings::QUICK.agent_limit_paise,
        "the Quick limit cannot fit a minimal Quick run: needs {min_viable} paise"
    );
}

/// The ₹100 default at real prices (owner ruling R-S2a): what is reserved,
/// how far research gets and what a run costs, printed for the stage
/// report. Research stopped by the limit with an answer written is `done`
/// with a trail line (R-S2b).
#[tokio::test]
async fn quick_at_the_default_limit_at_real_prices() {
    let Some(pool) = pool().await else { return };
    let server = hernia_server().await;
    let llm = Arc::new(opus_script(8));
    let limit = DepthSettings::QUICK.agent_limit_paise;
    assert_eq!(limit, 10_000);
    let (ctx, uid) = opus_ctx(&pool, &server, &llm, limit).await;
    let out = run_team(&ctx).await;
    tokio::time::sleep(Duration::from_millis(20)).await;
    let rows = charges(&pool, uid).await;
    let reqs = llm.requests();
    let turns = reqs.iter().filter(|r| !r.tools.is_empty()).count();
    let writer_reserve = ctx.budget.finish_reserve();
    let l = lines(&steps_of(&ctx, uid).await);
    let stop_line = l
        .iter()
        .find(|x| x.starts_with("Search stopped at the"))
        .cloned();
    eprintln!(
        "MEASURED at ₹100: writer reserve {writer_reserve} paise; researcher rounds {turns}; calls {}; charges {rows:?} = {} paise; stop line {stop_line:?}",
        reqs.len(),
        rows.iter().sum::<i64>(),
    );
    assert!(matches!(out, PipelineOutcome::Done(_)), "{out:?}");
    assert_eq!(writer_calls(&llm), 1);
    assert!(rows.iter().sum::<i64>() <= limit);
    let want = if turns == 8 {
        "Search stopped at the round cap after 8 rounds".to_string()
    } else {
        format!("Search stopped at the Quick limit after {turns} rounds")
    };
    assert_eq!(stop_line.as_deref(), Some(want.as_str()), "{l:?}");
}

// ---------- review fixes and owner rulings ----------

use incisive_app::agent::answer::STOPPED_NO_ANSWER;
use incisive_app::agent::team::{WRITER_NO_TIME, WRITER_TRUNCATED};

fn truncated(llm: ScriptedLlm, key: &str) -> ScriptedLlm {
    llm.when(key).reply_stop(
        "{\"bottom_line\": {\"text\": \"cut",
        StopReason::MaxTokens,
        usage(3000, 16000),
    )
}

fn research_then(llm: ScriptedLlm) -> ScriptedLlm {
    research_script(llm)
        .when(FINAL_KEY)
        .reply_json(study_list(&[(SEEN_A, "RCT"), (SEEN_B, "SR_MA")]), u())
}

fn body_of(out: &PipelineOutcome) -> AnswerV3 {
    match out {
        PipelineOutcome::Done(v)
        | PipelineOutcome::Partial(v)
        | PipelineOutcome::PartialKind(v, _) => serde_json::from_value(v.clone()).unwrap(),
        PipelineOutcome::Failed(k) => panic!("failed: {k}"),
    }
}

fn writer_requests(llm: &ScriptedLlm) -> Vec<MessagesRequest> {
    llm.requests()
        .into_iter()
        .filter(|r| request_user_text(r).contains(STUDIES_HEADING))
        .collect()
}

/// R-S2b: research stopped at the round cap with an answer written is
/// `done`, with the trail line.
#[tokio::test]
async fn an_answer_after_the_round_cap_is_done_with_a_trail_line() {
    let Some(pool) = pool().await else { return };
    let server = hernia_server().await;
    let llm = Arc::new(team_script());
    let settings = DepthSettings {
        researcher_turns: 2,
        ..DepthSettings::QUICK
    };
    let (ctx, uid) = quick_ctx(
        &pool,
        literature(&server),
        agent_llm(llm.clone(), MODEL),
        6000,
        settings,
    )
    .await;
    let out = run_team(&ctx).await;
    assert!(matches!(out, PipelineOutcome::Done(_)), "{out:?}");
    let a = body_of(&out);
    assert!(a.answer.is_some());
    let l = lines(&steps_of(&ctx, uid).await);
    assert!(
        l.iter()
            .any(|x| x == "Search stopped at the round cap after 2 rounds"),
        "{l:?}"
    );
}

/// Finding 1: a truncated writer reply gets one retry at low effort.
#[tokio::test]
async fn a_truncated_writer_is_retried_once_at_low_effort() {
    let Some(pool) = pool().await else { return };
    let server = hernia_server().await;
    let llm = Arc::new(
        truncated(research_then(ScriptedLlm::new()), STUDIES_HEADING)
            .when(STUDIES_HEADING)
            .reply_json(writer_json(), usage(3000, 2500)),
    );
    let (ctx, _uid) = quick_ctx(
        &pool,
        literature(&server),
        agent_llm(llm.clone(), MODEL),
        6000,
        DepthSettings::QUICK,
    )
    .await;
    let out = run_team(&ctx).await;
    assert!(matches!(out, PipelineOutcome::Done(_)), "{out:?}");
    let w = writer_requests(&llm);
    assert_eq!(w.len(), 2);
    assert_eq!(w[0].output_config.as_ref().unwrap().effort, None);
    assert_eq!(
        w[1].output_config.as_ref().unwrap().effort,
        Some(Effort::Low),
        "the retry runs at low effort"
    );
    assert_eq!(w[1].max_tokens, 16_000);
}

/// Finding 1: truncated twice, the run ends partial with its references,
/// the kind `writer_truncated` and an honest message, never failed.
#[tokio::test]
async fn a_writer_truncated_twice_ends_partial_with_the_references() {
    let Some(pool) = pool().await else { return };
    let server = hernia_server().await;
    let llm = Arc::new(truncated(
        truncated(research_then(ScriptedLlm::new()), STUDIES_HEADING),
        STUDIES_HEADING,
    ));
    let (ctx, _uid) = quick_ctx(
        &pool,
        literature(&server),
        agent_llm(llm.clone(), MODEL),
        6000,
        DepthSettings::QUICK,
    )
    .await;
    let out = run_team(&ctx).await;
    assert!(
        matches!(out, PipelineOutcome::PartialKind(_, "writer_truncated")),
        "{out:?}"
    );
    let a = body_of(&out);
    assert!(a.answer.is_none());
    assert_eq!(a.empty_reason.as_deref(), Some(WRITER_TRUNCATED));
    assert_eq!(a.references.len(), 2);
    assert_eq!(writer_requests(&llm).len(), 2);
}

/// Finding 1: at real prices a truncated writer (charged its full 16,000
/// output tokens) is retried only when the budget allows; otherwise the
/// run ends partial `writer_truncated`. Never failed, never over the limit.
#[tokio::test]
async fn a_truncated_writer_retries_only_when_the_budget_allows() {
    let Some(pool) = pool().await else { return };
    let server = hernia_server().await;
    let (mut once, mut twice) = (false, false);
    for limit in (6000..=16000).step_by(1000) {
        let mut script = ScriptedLlm::new();
        script = turn(
            script,
            vec![tool_call(
                "s1",
                "search_pubmed",
                json!({"query": "mesh[tiab]", "max_results": 5}),
            )],
        );
        script = end_turn(script)
            .when(FINAL_KEY)
            .reply_json(study_list(&[(SEEN_A, "RCT")]), usage(1500, 800));
        for _ in 0..2 {
            script = script.when(STUDIES_HEADING).reply_stop(
                "{",
                StopReason::MaxTokens,
                usage(2500, 16000),
            );
        }
        let llm = Arc::new(script);
        let (ctx, uid) = opus_ctx(&pool, &server, &llm, limit).await;
        let out = run_team(&ctx).await;
        tokio::time::sleep(Duration::from_millis(20)).await;
        let spent: i64 = charges(&pool, uid).await.iter().sum();
        assert!(spent <= limit, "limit {limit}: spent {spent}");
        assert_eq!(ctx.budget.held(), 0, "limit {limit}: nothing left held");
        assert!(
            matches!(out, PipelineOutcome::PartialKind(_, "writer_truncated")),
            "limit {limit}: {out:?}"
        );
        match writer_requests(&llm).len() {
            1 => once = true,
            2 => twice = true,
            n => panic!("limit {limit}: {n} writer calls"),
        }
    }
    assert!(
        once && twice,
        "both the refused and the made retry are seen"
    );
}

/// Finding 1: a researcher whose final study-list call cannot be read
/// (truncated, then truncated again) ends the run partial with the
/// "stopped early" message, not failed; the writer is never called.
#[tokio::test]
async fn a_truncated_researcher_final_call_ends_partial() {
    let Some(pool) = pool().await else { return };
    let server = hernia_server().await;
    let llm = Arc::new(truncated(
        truncated(research_script(ScriptedLlm::new()), FINAL_KEY),
        FINAL_KEY,
    ));
    let (ctx, _uid) = quick_ctx(
        &pool,
        literature(&server),
        agent_llm(llm.clone(), MODEL),
        6000,
        DepthSettings::QUICK,
    )
    .await;
    let out = run_team(&ctx).await;
    assert!(
        matches!(
            out,
            PipelineOutcome::PartialKind(_, "researcher_incomplete")
        ),
        "{out:?}"
    );
    let a = body_of(&out);
    assert_eq!(a.empty_reason.as_deref(), Some(STOPPED_NO_ANSWER));
    assert_eq!(a.searches.len(), 1, "the trail is kept");
    assert!(writer_requests(&llm).is_empty());
}

/// Minor review 2: a rate-limited (or failed) final study-list call after
/// research ends partial with the trail, not failed.
#[tokio::test]
async fn a_rate_limited_researcher_final_call_ends_partial() {
    let Some(pool) = pool().await else { return };
    let server = hernia_server().await;
    let llm = Arc::new(
        research_script(ScriptedLlm::new())
            .when(FINAL_KEY)
            .fail(claude_client::ClaudeError::RateLimited { retry_after: None }),
    );
    let (ctx, _uid) = quick_ctx(
        &pool,
        literature(&server),
        agent_llm(llm.clone(), MODEL),
        6000,
        DepthSettings::QUICK,
    )
    .await;
    let out = run_team(&ctx).await;
    assert!(
        matches!(
            out,
            PipelineOutcome::PartialKind(_, "researcher_incomplete")
        ),
        "{out:?}"
    );
    let a = body_of(&out);
    assert_eq!(a.empty_reason.as_deref(), Some(STOPPED_NO_ANSWER));
    assert_eq!(a.searches.len(), 1, "the trail is kept");
    assert!(writer_requests(&llm).is_empty());
}

/// Finding 2: with less than WRITER_MIN_TIME left before the hard stop, the writer
/// is not started; the run ends partial with the references.
#[tokio::test]
async fn too_little_time_left_skips_the_writer() {
    let Some(pool) = pool().await else { return };
    let server = hernia_server().await;
    let llm = Arc::new(team_script());
    let settings = DepthSettings::QUICK;
    let (uid, run, user) = new_run(&pool, 6000).await;
    incisive_app::runs::store::set_status(&pool, run, RunStatus::Running, None)
        .await
        .unwrap();
    let lit = literature(&server);
    let hard = Duration::from_secs(u64::from(settings.agent_time_limit_s) + 60);
    let budget = Arc::new(
        RunBudget::new(run, uid, 6000, Instant::now(), prices(), 88.0)
            .with_time_limit(settings.agent_time_limit_s)
            .aged_for_test(hard - Duration::from_secs(50)),
    );
    let ctx = RunContext {
        state: app_state(pool.clone(), lit.clone()),
        llm: agent_llm(llm.clone(), MODEL),
        lit,
        run_id: run,
        user,
        question: QUESTION.to_string(),
        mode: RunMode::Quick,
        settings,
        budget,
        cancel: CancelFlag::default(),
    };
    let out = run_team(&ctx).await;
    assert!(
        matches!(out, PipelineOutcome::PartialKind(_, "writer_time")),
        "{out:?}"
    );
    let a = body_of(&out);
    assert_eq!(a.empty_reason.as_deref(), Some(WRITER_NO_TIME));
    assert_eq!(a.references.len(), 2);
    assert!(writer_requests(&llm).is_empty());
}

/// Finding 6: an unpriced writer model fails the run before any research.
#[tokio::test]
async fn an_unpriced_writer_fails_before_research() {
    let Some(pool) = pool().await else { return };
    let server = hernia_server().await;
    let llm = Arc::new(team_script());
    let agent = Arc::new(AgentLlm {
        client: llm.clone(),
        models: AgentModels {
            researcher: MODEL.into(),
            writer: "unpriced-writer".into(),
            ..AgentModels::default()
        },
    });
    let (ctx, _uid) = quick_ctx(
        &pool,
        literature(&server),
        agent,
        6000,
        DepthSettings::QUICK,
    )
    .await;
    assert_eq!(run_team(&ctx).await, PipelineOutcome::Failed("llm"));
    assert!(llm.requests().is_empty(), "no research was paid for");
}

/// A partial outcome with a kind is stored as `partial` with that
/// `error_kind`, and its answer is kept.
#[tokio::test]
async fn the_worker_stores_a_partial_runs_kind() {
    let Some(pool) = pool().await else { return };
    fn kinded(_: &RunContext) -> PipelineFuture<'_> {
        Box::pin(async {
            PipelineOutcome::PartialKind(json!({"version": 3, "x": 1}), "writer_truncated")
        })
    }
    let mut st = app_state(
        pool.clone(),
        incisive_app::agent::testing::offline_literature(),
    );
    st.worker = WorkerHandle::with_pipeline(kinded);
    st.llm = Some(incisive_app::agent::testing::idle_llm(MODEL));
    let (uid, user) = new_user(&pool).await;
    let AskOutcome::RunStarted { run_id, .. } =
        start_run(&st, Uuid::new_v4(), &user, QUESTION, RunMode::Quick, None)
            .await
            .unwrap()
    else {
        panic!()
    };
    let v = wait_done(&pool, uid, run_id).await;
    wait_idle(&st, run_id).await;
    assert_eq!(v.summary.status, RunStatus::Partial);
    assert_eq!(v.answer_version, 3);
    let kind: Option<String> =
        sqlx::query_scalar("SELECT error_kind FROM incisive.runs WHERE id = $1")
            .bind(run_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(kind.as_deref(), Some("writer_truncated"));
}

static WRITER_SEEN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static WRITER_GO: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Like the worker's hard-stop timeout: the team pipeline is dropped as soon
/// as the writer's call is in flight.
fn dropped_mid_writer(ctx: &RunContext) -> PipelineFuture<'_> {
    use std::sync::atomic::Ordering;
    Box::pin(async move {
        tokio::select! {
            o = run_team(ctx) => o,
            _ = async {
                while !WRITER_SEEN.load(Ordering::SeqCst) {
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
            } => PipelineOutcome::Failed("hard_stop"),
        }
    })
}

/// Finding 8: the pipeline dropped while the writer is in flight (as at the
/// hard stop): the writer's charge is still booked, and once it lands
/// nothing of the run's reservation is left held.
#[tokio::test]
async fn dropping_the_run_mid_writer_still_books_the_writer() {
    use std::sync::atomic::Ordering;
    let Some(pool) = pool().await else { return };
    let server = hernia_server().await;
    let llm = Arc::new(team_script().until(
        STUDIES_HEADING,
        Arc::new(|| {
            WRITER_SEEN.store(true, Ordering::SeqCst);
            WRITER_GO.load(Ordering::SeqCst)
        }),
    ));
    let mut st = team_state(&pool, literature(&server), agent_llm(llm.clone(), MODEL));
    st.worker = WorkerHandle::with_pipeline(dropped_mid_writer);
    let (uid, user) = new_user(&pool).await;
    let AskOutcome::RunStarted { run_id, .. } =
        start_run(&st, Uuid::new_v4(), &user, QUESTION, RunMode::Quick, None)
            .await
            .unwrap()
    else {
        panic!()
    };
    let v = wait_done(&pool, uid, run_id).await;
    assert_eq!(v.summary.status, RunStatus::Failed);
    // The worker has finished; the writer's call is still in flight, and its
    // hold keeps that much of the reservation.
    wait_idle(&st, run_id).await;
    let reserved = || async {
        sqlx::query_scalar::<_, i64>(
            "SELECT COALESCE(SUM(paise), 0)::bigint FROM incisive.spend WHERE request_id = $1 AND kind = 'reservation'",
        )
        .bind(run_id)
        .fetch_one(&pool)
        .await
        .unwrap()
    };
    assert_eq!(
        charges(&pool, uid).await.len(),
        4,
        "turns and the study list"
    );
    assert!(reserved().await > 0, "the in-flight writer's hold is kept");
    WRITER_GO.store(true, Ordering::SeqCst);
    for _ in 0..500 {
        if charges(&pool, uid).await.len() == 5 && reserved().await == 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(
        charges(&pool, uid).await.len(),
        5,
        "the writer's charge is booked"
    );
    assert_eq!(reserved().await, 0, "nothing is left held");
    let cost: i64 = sqlx::query_scalar("SELECT cost_paise FROM incisive.runs WHERE id = $1")
        .bind(run_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(cost, charges(&pool, uid).await.iter().sum::<i64>());
}
