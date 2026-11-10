//! S1: Quick's answer written in four parts (`QUICK_PARALLEL=on`), with a
//! scripted Claude and wiremock literature (no network). The harness is the
//! Quick team tests'. Needs `TEST_DATABASE_URL` (skipped when unset).
#![allow(dead_code)]

use std::sync::Arc;
use std::time::{Duration, Instant};

use claude_client::{ContentBlock, Effort, MessagesRequest, StopReason, Usage};
use incisive_app::agent::llm::{LlmClient, ScriptedLlm};
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

use incisive_app::agent::PipelineOutcome;
use incisive_app::agent::answer_v3::AnswerV3;
use incisive_app::agent::team::run_team;
use incisive_app::agent::writer_v3::{
    self, SECTION_HEAD, STUDIES_HEADING, Section, reserve_for, sections_reserve,
};

const FINAL_KEY: &str = "Research transcript";
const UNKNOWN_CITE: &str = "99999999";

/// The single writer's reply (as the Quick team tests'), with the Stage 4
/// text fields.
fn writer_json() -> Value {
    json!({
        "bottom_line": {
            "text": format!("Mesh type made no difference to chronic pain [{SEEN_A}; {UNKNOWN_CITE}]."),
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
        "would_change": ["A larger multicentre RCT"],
        "at_a_glance": format!("No difference in chronic pain.\nChronic pain: VAS 1.2 with biological mesh [{SEEN_A}]\nMain caveat: two small trials"),
        "clinical_practice": "Default choice: either mesh.\nCost per case: not known",
        "notable_findings": format!("- Short follow-up [{SEEN_A}].")
    })
}

/// The part of the reply one section writes.
fn part(s: Section) -> Value {
    let w = writer_json();
    let mut m = serde_json::Map::new();
    for f in s.fields() {
        m.insert((*f).to_string(), w[*f].clone());
    }
    Value::Object(m)
}

/// The key of a section's requests.
fn key(s: Section) -> String {
    format!("{SECTION_HEAD}{}", s.name().to_uppercase())
}

fn research_then(llm: ScriptedLlm) -> ScriptedLlm {
    research_script(llm).when(FINAL_KEY).reply_json(
        study_list(&[(SEEN_A, "RCT"), (UNSEEN, "RCT"), (SEEN_B, "SR_MA")]),
        u(),
    )
}

/// Research, then every section's reply, keyed.
fn sections_script(llm: ScriptedLlm) -> ScriptedLlm {
    let mut llm = research_then(llm);
    for s in Section::ALL {
        llm = llm.when(&key(s)).reply_json(part(s), usage(3000, 700));
    }
    llm
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

fn writer_requests(llm: &ScriptedLlm) -> Vec<MessagesRequest> {
    llm.requests()
        .into_iter()
        .filter(|r| request_user_text(r).contains(STUDIES_HEADING))
        .collect()
}

fn section_requests(llm: &ScriptedLlm, s: Section) -> Vec<MessagesRequest> {
    writer_requests(llm)
        .into_iter()
        .filter(|r| request_user_text(r).contains(&key(s)))
        .collect()
}

fn single_requests(llm: &ScriptedLlm) -> Vec<MessagesRequest> {
    writer_requests(llm)
        .into_iter()
        .filter(|r| !request_user_text(r).contains(SECTION_HEAD))
        .collect()
}

fn body_of(out: &PipelineOutcome) -> AnswerV3 {
    match out {
        PipelineOutcome::Done(v)
        | PipelineOutcome::Partial(v)
        | PipelineOutcome::PartialKind(v, _) => serde_json::from_value(v.clone()).unwrap(),
        PipelineOutcome::Failed(k) => panic!("failed: {k}"),
    }
}

/// The single and four-part reserves at the test prices, and the smallest
/// limit that keeps today's research room.
fn reserves() -> (i64, i64, i64) {
    let b = Arc::new(RunBudget::new(
        Uuid::nil(),
        1,
        1,
        Instant::now(),
        prices(),
        88.0,
    ));
    let llm = agent_llm(Arc::new(ScriptedLlm::new()), MODEL);
    let w = DepthSettings::QUICK.writer_max_tokens;
    let single = reserve_for(&llm, &b, QUESTION, w).unwrap();
    let parallel = sections_reserve(&llm, &b, QUESTION, w).unwrap();
    let limit = parallel + (DepthSettings::QUICK.agent_limit_paise - single);
    (single, parallel, limit)
}

/// A Quick context with the switch on (or off) and `limit`.
async fn ctx_with(
    pool: &PgPool,
    server: &MockServer,
    llm: &Arc<ScriptedLlm>,
    limit: i64,
    on: bool,
) -> (RunContext, i64) {
    let (mut ctx, uid) = quick_ctx(
        pool,
        literature(server),
        agent_llm(llm.clone(), MODEL),
        limit,
        DepthSettings::QUICK,
    )
    .await;
    ctx.state.runs.quick_parallel = on;
    (ctx, uid)
}

/// Tests 2 and 3: with the switch on and the limit raised, the four parts
/// are written at once (one call each, the whole prompt, their own schemas
/// and max_tokens), the reserve is the documented sum, the money stays
/// within the limit, and the merged answer is the one a single writer
/// giving the same reply stores.
#[tokio::test]
async fn four_parts_are_written_at_once_and_merged() {
    let Some(pool) = pool().await else { return };
    let server = hernia_server().await;
    let (_single, parallel, limit) = reserves();
    let llm = Arc::new(sections_script(ScriptedLlm::new()));
    let (ctx, uid) = ctx_with(&pool, &server, &llm, limit, true).await;
    let out = run_team(&ctx).await;
    assert!(matches!(out, PipelineOutcome::Done(_)), "{out:?}");
    assert_eq!(ctx.budget.committed().finish_reserve, parallel);
    assert!(ctx.budget.spent() <= limit);
    let one = single_requests(&llm);
    assert!(one.is_empty(), "no single writer");
    let user = {
        let r = &writer_requests(&llm)[0];
        let t = request_user_text(r);
        t[..t.find(SECTION_HEAD).unwrap()].trim_end().to_string()
    };
    for s in Section::ALL {
        let r = section_requests(&llm, s);
        assert_eq!(r.len(), 1, "{s:?} once");
        let r = &r[0];
        assert_eq!(
            r.max_tokens,
            s.max_tokens(DepthSettings::QUICK.writer_max_tokens)
        );
        assert_eq!(r.output_config.as_ref().unwrap().effort, None);
        let schema = match &r.output_config.as_ref().unwrap().format {
            Some(claude_client::OutputFormat::JsonSchema { schema }) => schema.clone(),
            other => panic!("{other:?}"),
        };
        assert_eq!(schema, writer_v3::section_schema(s));
        assert!(r.system.as_ref().unwrap().starts_with(writer_v3::SYSTEM));
        assert!(request_user_text(r).starts_with(&user));
    }
    let a = body_of(&out);
    assert!(a.notes.is_empty(), "consistent: no note {:?}", a.notes);
    let l = lines(&steps_of(&ctx, uid).await);
    assert!(!l.iter().any(|x| x.contains("one piece")), "{l:?}");

    // The single writer with the same reply stores the same answer.
    let single_llm = Arc::new(
        research_then(ScriptedLlm::new())
            .when(STUDIES_HEADING)
            .reply_json(writer_json(), usage(3000, 2800)),
    );
    let (sctx, _) = ctx_with(&pool, &server, &single_llm, limit, false).await;
    let sout = run_team(&sctx).await;
    let s = body_of(&sout);
    assert_eq!(single_requests(&single_llm).len(), 1);
    assert_eq!(a.answer, s.answer, "the same written answer");
    assert_eq!(a.integrity, s.integrity);
    assert_eq!(a.untraced, s.untraced);
    assert_eq!(a.references, s.references);
}

/// Q3: a part whose first reply is unusable is retried once at low effort,
/// alone; the others are called once.
#[tokio::test]
async fn only_the_unusable_part_is_retried_at_low_effort() {
    let Some(pool) = pool().await else { return };
    let server = hernia_server().await;
    let (_, _, limit) = reserves();
    let llm = research_then(ScriptedLlm::new())
        .when(&key(Section::Outcomes))
        .reply_text("not json at all", usage(3000, 100));
    let llm = Arc::new(sections_script_after(llm));
    let (ctx, uid) = ctx_with(&pool, &server, &llm, limit, true).await;
    let out = run_team(&ctx).await;
    assert!(matches!(out, PipelineOutcome::Done(_)), "{out:?}");
    for s in Section::ALL {
        let r = section_requests(&llm, s);
        if s == Section::Outcomes {
            assert_eq!(r.len(), 2);
            assert_eq!(
                r[1].output_config.as_ref().unwrap().effort,
                Some(Effort::Low)
            );
            assert_eq!(r[1].max_tokens, r[0].max_tokens);
        } else {
            assert_eq!(r.len(), 1, "{s:?} not re-called");
        }
    }
    assert!(single_requests(&llm).is_empty());
    assert!(body_of(&out).answer.is_some());
    let l = lines(&steps_of(&ctx, uid).await);
    assert!(!l.iter().any(|x| x.contains("one piece")));
}

/// The section replies after other keyed replies (research is in front).
fn sections_script_after(mut llm: ScriptedLlm) -> ScriptedLlm {
    for s in Section::ALL {
        llm = llm.when(&key(s)).reply_json(part(s), usage(3000, 700));
    }
    llm
}

/// Q3: a part failing twice falls back to the single writer, once; its
/// answer is used; the parts that succeeded are not re-called; the step line
/// says so.
#[tokio::test]
async fn a_part_failing_twice_falls_back_to_one_piece() {
    let Some(pool) = pool().await else { return };
    let server = hernia_server().await;
    let (_, _, limit) = reserves();
    let cut = "{\"applicability\": {\"case_mix\": \"cut";
    let mut llm = research_then(ScriptedLlm::new());
    for _ in 0..2 {
        llm = llm.when(&key(Section::Context)).reply_stop(
            cut,
            StopReason::MaxTokens,
            usage(3000, 5000),
        );
    }
    for s in [Section::Trials, Section::Outcomes, Section::Summary] {
        llm = llm.when(&key(s)).reply_json(part(s), usage(3000, 700));
    }
    let mut one = writer_json();
    one["would_change"] = json!(["From the one-piece writer"]);
    let llm = Arc::new(llm.when(STUDIES_HEADING).reply_json(one, usage(3000, 2800)));
    let (ctx, uid) = ctx_with(&pool, &server, &llm, limit, true).await;
    let out = run_team(&ctx).await;
    assert!(matches!(out, PipelineOutcome::Done(_)), "{out:?}");
    assert_eq!(section_requests(&llm, Section::Context).len(), 2);
    for s in [Section::Trials, Section::Outcomes, Section::Summary] {
        assert_eq!(section_requests(&llm, s).len(), 1, "{s:?}");
    }
    let single = single_requests(&llm);
    assert_eq!(single.len(), 1, "the fallback once");
    assert_eq!(single[0].max_tokens, DepthSettings::QUICK.writer_max_tokens);
    let a = body_of(&out);
    assert_eq!(
        a.answer.unwrap().would_change,
        ["From the one-piece writer"],
        "the one-piece answer is used"
    );
    assert!(
        a.notes.is_empty(),
        "no merge checks on the one-piece answer"
    );
    let l = lines(&steps_of(&ctx, uid).await);
    assert!(
        l.iter()
            .any(|x| x == "Writer: section context failed; writing the answer in one piece"),
        "{l:?}"
    );
}

/// Q3 and money: a part's retry that cannot be held fails the part; the
/// one-piece writer cannot be held either, so the run ends as a refused
/// writer does today (partial, `write_refused`), never over the limit.
#[tokio::test]
async fn a_part_that_cannot_be_held_falls_back_then_ends_as_a_refused_writer() {
    let Some(pool) = pool().await else { return };
    let server = hernia_server().await;
    let (_, _, limit) = reserves();
    // The outcomes' first reply is cut off and charged far more than held:
    // nothing is left for its retry or for the one-piece writer.
    // Charged (at $5 per million output tokens, 88) to about 600 paise
    // short of the limit: less than either hold needs.
    let out_tokens = u32::try_from((limit - 600) * 1_000_000 / (5 * 88 * 100)).unwrap();
    let llm = research_then(ScriptedLlm::new())
        .when(&key(Section::Outcomes))
        .reply_stop(
            "{\"outcomes\": [",
            StopReason::MaxTokens,
            usage(3000, out_tokens),
        );
    let llm = Arc::new(
        sections_script_after(llm)
            .when(STUDIES_HEADING)
            .reply_json(writer_json(), u()),
    );
    let (ctx, _uid) = ctx_with(&pool, &server, &llm, limit, true).await;
    let out = run_team(&ctx).await;
    assert!(
        matches!(out, PipelineOutcome::PartialKind(_, "write_refused")),
        "{out:?}"
    );
    assert_eq!(
        section_requests(&llm, Section::Outcomes).len(),
        1,
        "retry not held"
    );
    assert!(single_requests(&llm).is_empty(), "one piece not held");
    assert!(body_of(&out).answer.is_none());
    assert!(ctx.budget.spent() <= limit, "never over the limit");
}

/// Owner rule 2: with the switch on but the default limit, research would
/// get less room: the run reserves and writes as today, quietly.
#[tokio::test]
async fn too_low_a_limit_writes_in_one_piece_as_today() {
    let Some(pool) = pool().await else { return };
    let server = hernia_server().await;
    let (single, _, limit) = reserves();
    let llm = Arc::new(
        research_then(ScriptedLlm::new())
            .when(STUDIES_HEADING)
            .reply_json(writer_json(), usage(3000, 2800)),
    );
    let (ctx, uid) = ctx_with(&pool, &server, &llm, limit - 1, true).await;
    let out = run_team(&ctx).await;
    assert!(matches!(out, PipelineOutcome::Done(_)), "{out:?}");
    assert_eq!(ctx.budget.committed().finish_reserve, single);
    assert_eq!(single_requests(&llm).len(), 1);
    assert_eq!(writer_requests(&llm).len(), 1);
    // Quiet (review I1): nothing new on the run's steps.
    let l = lines(&steps_of(&ctx, uid).await);
    assert!(!l.iter().any(|x| x.contains("four parts")), "{l:?}");
}

/// Test 4 (switch off): the same limit, the switch off: one writer, today's
/// reserve, no note line.
#[tokio::test]
async fn switch_off_is_todays_writer() {
    let Some(pool) = pool().await else { return };
    let server = hernia_server().await;
    let (single, _, limit) = reserves();
    let llm = Arc::new(
        research_then(ScriptedLlm::new())
            .when(STUDIES_HEADING)
            .reply_json(writer_json(), usage(3000, 2800)),
    );
    let (ctx, uid) = ctx_with(&pool, &server, &llm, limit, false).await;
    let out = run_team(&ctx).await;
    assert!(matches!(out, PipelineOutcome::Done(_)), "{out:?}");
    assert_eq!(ctx.budget.committed().finish_reserve, single);
    assert_eq!(writer_requests(&llm).len(), 1);
    assert!(single_requests(&llm).len() == 1);
    let l = lines(&steps_of(&ctx, uid).await);
    assert!(!l.iter().any(|x| x.contains("four parts")));
}

/// Test 5 in a run: a summary more certain than every outcome gets the note;
/// nothing is changed.
#[tokio::test]
async fn a_summary_more_certain_than_the_outcomes_gets_a_note() {
    let Some(pool) = pool().await else { return };
    let server = hernia_server().await;
    let (_, _, limit) = reserves();
    let mut summary = part(Section::Summary);
    summary["bottom_line"]["certainty"] = json!("High");
    let mut llm = research_then(ScriptedLlm::new())
        .when(&key(Section::Summary))
        .reply_json(summary, usage(3000, 700));
    for s in [Section::Trials, Section::Outcomes, Section::Context] {
        llm = llm.when(&key(s)).reply_json(part(s), usage(3000, 700));
    }
    let llm = Arc::new(llm);
    let (ctx, _) = ctx_with(&pool, &server, &llm, limit, true).await;
    let out = run_team(&ctx).await;
    let a = body_of(&out);
    assert_eq!(a.notes.len(), 1, "{:?}", a.notes);
    assert!(a.notes[0].contains("certainty (High) is higher than any outcome's (highest: Low)"));
    let w = a.answer.unwrap();
    assert_eq!(w.bottom_line.certainty.as_str(), "High", "never rewritten");
}

/// Streaming: four streamed parts give one first-words line, and the draft
/// is gone with the run.
#[tokio::test]
async fn four_streamed_parts_say_first_words_once() {
    let Some(pool) = pool().await else { return };
    let server = hernia_server().await;
    let (_, _, limit) = reserves();
    let llm = Arc::new(sections_script(ScriptedLlm::new()));
    let (mut ctx, uid) = ctx_with(&pool, &server, &llm, limit, true).await;
    ctx.state.stream = true;
    let out = run_team(&ctx).await;
    assert!(matches!(out, PipelineOutcome::Done(_)), "{out:?}");
    // The detached first-words write lands shortly after.
    let mut first = 0;
    for _ in 0..200 {
        let l = lines(&steps_of(&ctx, uid).await);
        first = l
            .iter()
            .filter(|x| x.starts_with("Writer: first words after "))
            .count();
        if first > 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    tokio::time::sleep(Duration::from_millis(100)).await;
    let l = lines(&steps_of(&ctx, uid).await);
    let n = l.iter().filter(|x| x.contains("first words")).count();
    assert_eq!((first, n), (1, 1), "{l:?}");
}

/// Review I3: before the one-piece writer runs, the discarded parts' draft
/// is cleared, so the page waits for the one piece instead of showing it.
struct DraftProbe {
    inner: Arc<ScriptedLlm>,
    pool: PgPool,
    run: std::sync::Mutex<Option<Uuid>>,
    /// The answer drafts stored when the one-piece writer's call arrived.
    seen: std::sync::Mutex<Vec<i64>>,
    /// The answer drafts stored when the failing part's retry arrived
    /// (after the other parts' replies were streamed), polled up to 2 s.
    before: std::sync::Mutex<Vec<i64>>,
}

async fn answer_drafts(pool: &PgPool, run: Uuid) -> i64 {
    sqlx::query_scalar(
        "SELECT count(*) FROM incisive.run_drafts WHERE run_id = $1 AND part = 'answer'",
    )
    .bind(run)
    .fetch_one(pool)
    .await
    .unwrap()
}

#[async_trait::async_trait]
impl LlmClient for DraftProbe {
    async fn call(
        &self,
        req: &MessagesRequest,
    ) -> Result<claude_client::MessagesResponse, claude_client::ClaudeError> {
        let text = request_user_text(req);
        let run = *self.run.lock().unwrap();
        if let Some(run) = run
            && text.contains(&key(Section::Context))
            && text.contains(incisive_app::agent::llm::SCHEMA_NUDGE.trim())
        {
            let mut n = 0;
            for _ in 0..200 {
                n = answer_drafts(&self.pool, run).await;
                if n > 0 {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            self.before.lock().unwrap().push(n);
        }
        if let Some(run) = run
            && text.contains(STUDIES_HEADING)
            && !text.contains(SECTION_HEAD)
        {
            let n: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM incisive.run_drafts WHERE run_id = $1 AND part = 'answer'",
            )
            .bind(run)
            .fetch_one(&self.pool)
            .await
            .unwrap();
            self.seen.lock().unwrap().push(n);
        }
        self.inner.call(req).await
    }
}

#[tokio::test]
async fn the_parts_draft_is_cleared_before_the_one_piece_writer() {
    let Some(pool) = pool().await else { return };
    let server = hernia_server().await;
    let (_, _, limit) = reserves();
    let cut = "{\"applicability\": {\"case_mix\": \"cut";
    let mut llm = research_then(ScriptedLlm::new());
    for _ in 0..2 {
        llm = llm.when(&key(Section::Context)).reply_stop(
            cut,
            StopReason::MaxTokens,
            usage(3000, 5000),
        );
    }
    let llm = Arc::new(
        sections_script_after(llm)
            .when(STUDIES_HEADING)
            .reply_json(writer_json(), usage(3000, 2800)),
    );
    let probe = Arc::new(DraftProbe {
        inner: llm.clone(),
        pool: pool.clone(),
        run: std::sync::Mutex::new(None),
        seen: std::sync::Mutex::new(Vec::new()),
        before: std::sync::Mutex::new(Vec::new()),
    });
    let (mut ctx, uid) = quick_ctx(
        &pool,
        literature(&server),
        agent_llm(probe.clone(), MODEL),
        limit,
        DepthSettings::QUICK,
    )
    .await;
    ctx.state.runs.quick_parallel = true;
    ctx.state.stream = true;
    *probe.run.lock().unwrap() = Some(ctx.run_id);
    let out = run_team(&ctx).await;
    assert!(matches!(out, PipelineOutcome::Done(_)), "{out:?}");
    assert_eq!(single_requests(&llm).len(), 1, "the fallback ran");
    // The parts did write a draft (three whole parts and a cut one) ...
    let l = lines(&steps_of(&ctx, uid).await);
    assert!(l.iter().any(|x| x.contains("one piece")), "{l:?}");
    assert_eq!(*probe.before.lock().unwrap(), [1], "a draft existed before");
    // ... and it was gone when the one-piece writer started.
    assert_eq!(*probe.seen.lock().unwrap(), [0]);
}

// ---------- S3: the empty study list retry beside the four-part reserve ----------

fn empty_list() -> Value {
    json!({
        "searches": [], "selected": [], "considered_not_selected": [], "possible_contradictions": []
    })
}

fn final_requests(llm: &ScriptedLlm) -> Vec<MessagesRequest> {
    llm.requests()
        .into_iter()
        .filter(|r| request_user_text(r).contains(FINAL_KEY) && r.tools.is_empty())
        .filter(|r| !request_user_text(r).contains(STUDIES_HEADING))
        .collect()
}

/// S3 review M1: the retry is a gathering call and leaves the four parts'
/// reserve free. When the first final call's charge leaves room only in the
/// reserve (about ₹1 above it, less than the retry's estimate), the retry is
/// refused although the limit itself still has the reserve's room:
/// `empty_selection`, no retry request, no step line.
#[tokio::test]
async fn the_retry_never_takes_the_four_part_reserve() {
    let Some(pool) = pool().await else { return };
    let server = hernia_server().await;
    let (_single, parallel, limit) = reserves();
    let b = RunBudget::new(Uuid::nil(), 1, 1, Instant::now(), prices(), 88.0);
    let turn_cost = b.cost_of(MODEL, &u()).unwrap();
    // The first final call's cost: spent = limit - reserve - 100 paise.
    let target = limit - parallel - 100 - 3 * turn_cost;
    let mut out_tokens = 0u32;
    while b.cost_of(MODEL, &usage(1000, out_tokens + 1000)).unwrap() <= target {
        out_tokens += 1000;
    }
    let llm = Arc::new(sections_script(
        ScriptedLlm::new()
            .when(FINAL_KEY)
            .reply_json(empty_list(), usage(1000, out_tokens)),
    ));
    let (ctx, uid) = ctx_with(&pool, &server, &llm, limit, true).await;
    let out = run_team(&ctx).await;
    assert!(
        matches!(out, PipelineOutcome::PartialKind(_, "empty_selection")),
        "{out:?}"
    );
    assert_eq!(ctx.budget.committed().finish_reserve, parallel);
    tokio::time::sleep(Duration::from_millis(20)).await;
    let spent = ctx.budget.spent();
    assert!(
        spent <= limit - parallel,
        "the reserve is untouched: {spent}"
    );
    assert!(limit - spent > 500, "the limit itself had room: {spent}");
    assert_eq!(ctx.budget.held(), 0);
    assert_eq!(final_requests(&llm).len(), 1, "no retry");
    assert!(writer_requests(&llm).is_empty());
    let l = lines(&steps_of(&ctx, uid).await);
    assert!(
        !l.contains(&incisive_app::agent::researcher::RETRY_STEP_LINE.to_string()),
        "{l:?}"
    );
}

/// S3 review M1: with room for it, the retry is made beside the four-part
/// reserve and the four parts are still written.
#[tokio::test]
async fn a_retry_beside_the_four_part_reserve_still_leaves_the_writers_room() {
    let Some(pool) = pool().await else { return };
    let server = hernia_server().await;
    let (_single, parallel, limit) = reserves();
    let llm = Arc::new(sections_script(
        ScriptedLlm::new()
            .when(FINAL_KEY)
            .reply_json(empty_list(), u()),
    ));
    let (ctx, uid) = ctx_with(&pool, &server, &llm, limit, true).await;
    let out = run_team(&ctx).await;
    assert!(matches!(out, PipelineOutcome::Done(_)), "{out:?}");
    assert!(body_of(&out).answer.is_some());
    assert_eq!(ctx.budget.committed().finish_reserve, parallel);
    assert!(ctx.budget.spent() <= limit);
    assert_eq!(final_requests(&llm).len(), 2, "one retry");
    for s in Section::ALL {
        assert_eq!(section_requests(&llm, s).len(), 1, "{s:?} once");
    }
    let l = lines(&steps_of(&ctx, uid).await);
    assert!(l.contains(&incisive_app::agent::researcher::RETRY_STEP_LINE.to_string()));
}
