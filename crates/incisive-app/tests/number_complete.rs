//! Q3: the "not traced" numbers completed against their cited abstracts
//! (`NUMBER_COMPLETE`), with a scripted Claude and wiremock literature (no
//! network), the harness of `team.rs`. Needs `TEST_DATABASE_URL` (skipped
//! when unset).

use std::sync::Arc;
use std::time::{Duration, Instant};

use claude_client::{ClaudeError, ContentBlock, Effort, MessagesRequest, StopReason, Usage};
use incisive_app::agent::answer_v3::{AnswerV3, CompletionRule, Span};
use incisive_app::agent::llm::{LlmClient, ScriptedLlm};
use incisive_app::agent::numbers_complete::{self, NumberCompletion, Prompt};
use incisive_app::agent::writer_v3::{STUDIES_HEADING, StudyInput};
use incisive_app::agent::{AgentLlm, Literature, RunContext};
use incisive_app::ask::{AskOutcome, start_run};
use incisive_app::db::{lazy_pool, migrate};
use incisive_app::gate::OfflineClassifier;
use incisive_app::identity::{IapVerifier, JwkSource};
use incisive_app::lit::europepmc::EuropePmc;
use incisive_app::lit::http::{AllowlistedClient, OaPolicy};
use incisive_app::lit::pubmed::PubMed;
use incisive_app::runs::budget::RunBudget;
use incisive_app::runs::store::{create_run, get_run_for_user};
use incisive_app::runs::worker::{CancelFlag, WorkerHandle};
use incisive_app::runs::{DepthSettings, QuickEngine, RunMode, RunStatus, RunsConfig};
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
const SEEN_A: &str = "41060645";
const SEEN_B: &str = "39979619";
const FINAL_KEY: &str = "Research transcript";
/// In the number completeness step's request only.
const NUMBERS_KEY: &str = "NUMBERS TO CHECK";

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

fn app_state(pool: PgPool, lit: Arc<Literature>, on: bool) -> AppState {
    let runs = RunsConfig {
        number_complete: on,
        ..RunsConfig::default()
    };
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
        runs,
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

fn agent_llm(llm: Arc<dyn LlmClient>) -> Arc<AgentLlm> {
    incisive_app::agent::testing::agent_llm(llm, MODEL)
}

async fn new_user(pool: &PgPool) -> (i64, incisive_app::identity::User) {
    let email = format!("numbers-{}@example.com", Uuid::new_v4());
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

fn tool_call(id: &str, name: &str, input: Value) -> ContentBlock {
    ContentBlock::ToolUse {
        id: id.into(),
        name: name.into(),
        input,
    }
}

fn turn(llm: ScriptedLlm, uses: Vec<ContentBlock>) -> ScriptedLlm {
    let mut content = vec![ContentBlock::Text {
        text: "Searching.".into(),
    }];
    content.extend(uses);
    llm.reply_blocks(content, Some(StopReason::ToolUse), u())
}

fn research_script(llm: ScriptedLlm) -> ScriptedLlm {
    let llm = turn(
        llm,
        vec![tool_call(
            "s1",
            "search_pubmed",
            json!({"query": "inguinal hernia[tiab] AND mesh[tiab]", "max_results": 20}),
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
    llm.reply_blocks(
        vec![ContentBlock::Text {
            text: "Done searching.".into(),
        }],
        Some(StopReason::EndTurn),
        u(),
    )
}

fn study_list() -> Value {
    json!({
        "searches": [],
        "selected": [
            {"pmid": SEEN_A, "design": "RCT", "why": "key study"},
            {"pmid": SEEN_B, "design": "RCT", "why": "key study"}
        ],
        "considered_not_selected": [],
        "possible_contradictions": []
    })
}

/// The writer's bottom line: four numbers no abstract writes as such. Three
/// follow from their cited abstract (a sum, 100 minus a percentage, a
/// difference); 42 is in neither.
fn bottom() -> String {
    format!(
        "Biological mesh had 65 recurrences in all [{SEEN_A}]. 10% did not complete follow up [{SEEN_B}]. \
         Analgesic use differed by 1.7 tablets [{SEEN_B}]. Pain fell 42 points [{SEEN_A}]."
    )
}

fn writer_json(bottom: &str) -> Value {
    json!({
        "bottom_line": {"text": bottom, "certainty": "Low", "certainty_reason": format!("Two trials [{SEEN_B}]")},
        "outcomes": [{
            "outcome": "Recurrence", "rct_evidence": "One RCT",
            "effect_text": format!("Higher with biological mesh [{SEEN_A}]"),
            "numbers": [], "certainty": "Low", "why": "one trial", "pmids": [SEEN_A]
        }],
        "studies": [
            {"pmid": SEEN_A, "label": "BIOLAP (2025)", "design": "RCT", "n": 491, "setting": "Germany", "comparison": "biological vs synthetic", "key_results": format!("more recurrences [{SEEN_A}]"), "appraisal": "self-controlled"}
        ],
        "conflicts": [],
        "applicability": {"case_mix": "bilateral", "india_cost_context": "Not known.", "training": "None."},
        "would_change": ["A larger RCT"]
    })
}

const QUOTE_SUM: &str =
    "biological meshes (53 recurrences [11.2%]) compared with synthetic meshes (12 recurrences";
const QUOTE_PCT: &str = "A total of 156 (90%) patients completed follow up.";
const QUOTE_DIFF: &str = "P 22.9, A 21.2 tablets";

/// The checker's reply: three supported items and one invented quote.
fn checker_json() -> Value {
    json!({"items": [
        {"id": "1", "pmid": SEEN_A, "quote": QUOTE_SUM, "rule": "sum"},
        {"id": "2", "pmid": SEEN_B, "quote": QUOTE_PCT, "rule": "percent_from_counts"},
        {"id": "3", "pmid": SEEN_B, "quote": QUOTE_DIFF, "rule": "difference"},
        {"id": "4", "pmid": SEEN_A, "quote": "pain fell by 42 points", "rule": "as_written"}
    ]})
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

fn numbers_requests(llm: &ScriptedLlm) -> Vec<MessagesRequest> {
    llm.requests()
        .into_iter()
        .filter(|r| request_user_text(r).contains(NUMBERS_KEY))
        .collect()
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

struct Ran {
    answer: AnswerV3,
    raw: Value,
    lines: Vec<String>,
    llm: Arc<ScriptedLlm>,
    charges: Vec<i64>,
}

/// One Quick run through `start_run` and the worker: research, the writer
/// (`bottom` as its bottom line) and, keyed on its request, `checker` for
/// the completeness step.
async fn quick_run(pool: &PgPool, on: bool, bottom: &str, checker: Option<ScriptedLlm>) -> Ran {
    let server = hernia_server().await;
    let base = checker.unwrap_or_default();
    let llm = Arc::new(
        research_script(base)
            .when(FINAL_KEY)
            .reply_json(study_list(), u())
            .when(STUDIES_HEADING)
            .reply_json(writer_json(bottom), usage(3000, 2500)),
    );
    let mut st = app_state(pool.clone(), literature(&server), on);
    st.llm = Some(agent_llm(llm.clone()));
    let (uid, user) = new_user(pool).await;
    let AskOutcome::RunStarted { run_id, .. } =
        start_run(&st, Uuid::new_v4(), &user, QUESTION, RunMode::Quick, None)
            .await
            .unwrap()
    else {
        panic!("not started")
    };
    let mut v = None;
    for _ in 0..2000 {
        let r = get_run_for_user(pool, uid, run_id).await.unwrap().unwrap();
        if !r.summary.status.is_active() {
            v = Some(r);
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let v = v.expect("run finished");
    for _ in 0..2000 {
        if !st.worker.is_running(run_id) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(v.summary.status, RunStatus::Done, "{:?}", v.steps);
    let raw = v.answer.clone().unwrap();
    Ran {
        answer: serde_json::from_value(raw.clone()).unwrap(),
        raw,
        lines: v.steps.iter().map(|s| s.line.clone()).collect(),
        llm,
        charges: charges(pool, uid).await,
    }
}

fn marked(a: &AnswerV3) -> Vec<String> {
    let t = &a.answer.as_ref().unwrap().bottom_line.text;
    a.untraced
        .iter()
        .filter(|s| s.field == "bottom_line.text")
        .map(|s| t[s.start as usize..(s.start + s.len) as usize].to_string())
        .collect()
}

/// Off (the default) and on: the same answer text; on, the three numbers
/// that follow from their cited abstract lose their mark and keep their
/// source's words, 42 stays marked, one small call is made and charged, and
/// the study sheet shows the words.
#[tokio::test]
async fn quick_completes_the_marked_numbers_against_their_abstracts() {
    let Some(pool) = pool().await else { return };
    let checker = ScriptedLlm::new()
        .when(NUMBERS_KEY)
        .reply_json(checker_json(), usage(2000, 300));
    let off = quick_run(&pool, false, &bottom(), Some(checker)).await;
    let checker = ScriptedLlm::new()
        .when(NUMBERS_KEY)
        .reply_json(checker_json(), usage(2000, 300));
    let on = quick_run(&pool, true, &bottom(), Some(checker)).await;

    // Off: as today. Four marks, no call, no new key, no new line.
    assert_eq!(marked(&off.answer), ["65", "10%", "1.7", "42"]);
    assert!(numbers_requests(&off.llm).is_empty());
    assert!(off.raw.get("numbers_completed").is_none());
    assert!(off.raw["integrity"].get("numbers_completed").is_none());
    assert!(
        !off.lines
            .iter()
            .any(|l| l.starts_with("Number completeness"))
    );
    assert_eq!(
        off.charges.len(),
        5,
        "three turns, the study list, the writer"
    );

    // On: the answer's text is byte-identical.
    assert_eq!(
        serde_json::to_string(&on.answer.answer).unwrap(),
        serde_json::to_string(&off.answer.answer).unwrap()
    );
    assert_eq!(marked(&on.answer), ["42"]);
    let i = on.answer.integrity;
    assert_eq!(
        (i.numbers_checked, i.numbers_untraced, i.numbers_completed),
        (
            off.answer.integrity.numbers_checked,
            off.answer.integrity.numbers_untraced - 3,
            3
        )
    );
    let c = &on.answer.numbers_completed;
    let got: Vec<(&str, &str, CompletionRule)> = c
        .iter()
        .map(|c| (c.written.as_str(), c.pmid.as_str(), c.rule))
        .collect();
    assert_eq!(
        got,
        [
            ("65", SEEN_A, CompletionRule::Sum),
            ("10%", SEEN_B, CompletionRule::PercentFromCounts),
            ("1.7", SEEN_B, CompletionRule::Difference)
        ]
    );
    assert_eq!(c[0].quote, QUOTE_SUM);
    assert!(
        on.lines
            .iter()
            .any(|l| l
                == "Number completeness: 3 of 4 completed against their sources; 1 left marked"),
        "{:?}",
        on.lines
    );
    // After the number check line, before the answer is ready.
    let at = |p: &str| on.lines.iter().position(|l| l.starts_with(p)).unwrap();
    assert!(at("Number check:") < at("Number completeness:"));
    assert!(at("Number completeness:") < at("Answer ready"));

    // One call: the completeness model, low effort, small, structured; only
    // the marked numbers and the cited abstracts, never the question.
    let reqs = numbers_requests(&on.llm);
    assert_eq!(reqs.len(), 1);
    let r = &reqs[0];
    assert_eq!(r.model, MODEL);
    assert_eq!(r.max_tokens, numbers_complete::MAX_TOKENS);
    let oc = r.output_config.as_ref().unwrap();
    assert!(oc.format.is_some());
    assert_eq!(oc.effort, Some(Effort::Low));
    assert!(r.system.as_deref().unwrap().contains("NUMBER CHECKER"));
    let t = request_user_text(r);
    assert!(!t.contains(QUESTION) && !t.contains("mesh type change"));
    assert!(t.contains("<source id=\"numbers\">"));
    assert!(t.contains(&format!("<source id=\"pmid-{SEEN_A}\">")));
    assert!(t.contains(&format!("<source id=\"pmid-{SEEN_B}\">")));
    assert!(t.contains("Item 4 | number: \"42\""));
    assert!(
        !t.contains("Higher with biological mesh"),
        "only the marked sentences"
    );
    assert!(t.len() < 12_000, "{}", t.len());
    // Charged like every call; the answer's cost includes it.
    assert_eq!(on.charges.len(), 6);
    assert_eq!(on.answer.cost_paise, on.charges.iter().sum::<i64>());

    // The page: the marks of the completed numbers are gone; the study
    // sheet of each study lists its completed numbers with the words.
    let html_off = incisive_app::app::render_answer_v3(off.answer.clone());
    let html = incisive_app::app::render_answer_v3(on.answer.clone());
    assert!(!html_off.contains("Numbers checked against this abstract"));
    assert_eq!(
        html.matches("Numbers checked against this abstract")
            .count(),
        2
    );
    let sheet_a = &html[html.find("id=\"sheet-1\"").unwrap()..];
    let sheet_a = &sheet_a[..sheet_a.find("</dialog>").unwrap()];
    assert!(sheet_a.contains("Numbers checked against this abstract"));
    assert!(
        sheet_a.contains("biological meshes (53 recurrences [11.2%])"),
        "{sheet_a}"
    );
    assert!(sheet_a.contains("(a sum)"));
    assert!(html.matches("not traced").count() < html_off.matches("not traced").count());
}

/// Nothing marked: no call, no cost, no line, nothing new stored.
#[tokio::test]
async fn no_marks_means_no_call_and_no_cost() {
    let Some(pool) = pool().await else { return };
    let checker = ScriptedLlm::new()
        .when(NUMBERS_KEY)
        .reply_json(checker_json(), u());
    let r = quick_run(
        &pool,
        true,
        &format!("A total of 491 patients were randomised [{SEEN_A}]."),
        Some(checker),
    )
    .await;
    assert!(r.answer.untraced.is_empty(), "{:?}", r.answer.untraced);
    assert!(numbers_requests(&r.llm).is_empty());
    assert_eq!(r.charges.len(), 5);
    assert!(r.raw.get("numbers_completed").is_none());
    assert!(!r.lines.iter().any(|l| l.starts_with("Number completeness")));
}

/// An unusable reply: one call (no retry), charged; every mark stays; the
/// run is done as before.
#[tokio::test]
async fn an_unusable_reply_keeps_every_mark_and_the_run() {
    let Some(pool) = pool().await else { return };
    let checker = ScriptedLlm::new()
        .when(NUMBERS_KEY)
        .reply_text("these all look fine to me", u())
        .when(NUMBERS_KEY)
        .reply_json(checker_json(), u());
    let r = quick_run(&pool, true, &bottom(), Some(checker)).await;
    assert_eq!(marked(&r.answer), ["65", "10%", "1.7", "42"]);
    assert!(r.answer.numbers_completed.is_empty());
    assert_eq!(numbers_requests(&r.llm).len(), 1, "no retry");
    assert_eq!(r.charges.len(), 6);
    assert!(
        r.lines
            .iter()
            .any(|l| l == "Number completeness skipped: unusable reply; 4 left marked"),
        "{:?}",
        r.lines
    );
    assert!(r.raw.get("numbers_completed").is_none());
}

// ---------- the step on its own: every failure keeps the marks ----------

/// A running Quick run's context over `llm`, its budget `limit` and aged
/// by `age`.
async fn ctx(pool: &PgPool, llm: Arc<ScriptedLlm>, limit: i64, age: Duration) -> RunContext {
    let (uid, user) = new_user(pool).await;
    let run = create_run(pool, uid, QUESTION, RunMode::Quick, limit)
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
    incisive_app::runs::store::set_status(pool, run, RunStatus::Running, None)
        .await
        .unwrap();
    let settings = DepthSettings::QUICK;
    let budget = Arc::new(
        RunBudget::new(run, uid, limit, Instant::now(), prices(), 88.0)
            .with_time_limit(settings.agent_time_limit_s)
            .aged_for_test(age),
    );
    let lit = Arc::new(Literature {
        pubmed: PubMed::new(
            AllowlistedClient::new("Incisive/test", OaPolicy::default()),
            None,
        ),
        epmc: EuropePmc::new(
            AllowlistedClient::new("Incisive/test", OaPolicy::default()),
            OaPolicy::default(),
        ),
    });
    RunContext {
        state: app_state(pool.clone(), lit.clone(), true),
        llm: agent_llm(llm),
        lit,
        run_id: run,
        user,
        question: QUESTION.to_string(),
        mode: RunMode::Quick,
        settings,
        budget,
        cancel: CancelFlag::default(),
    }
}

const ABSTRACT_A: &str = "We operated on 10 robotic and 10 open infants under 5 kg.";

fn marked_answer() -> AnswerV3 {
    let text = format!("The series had 20 infants [{SEEN_A}].");
    let s = text.find("20").unwrap();
    serde_json::from_value(json!({
        "version": 3, "question": QUESTION, "mode": "quick", "created_at": "2026-11-09T00:00:00Z",
        "answer": writer_answer(&text), "empty_reason": null, "references": [], "searches": [],
        "integrity": {"dropped_unseen": 0, "dropped_unfetched": 0, "removed_citations": 0,
                      "dropped_numbers": 0, "dropped_studies": 0, "numbers_checked": 1, "numbers_untraced": 1},
        "stopped_early": null, "cost_paise": 0, "elapsed_s": 0,
        "untraced": [Span::new("bottom_line.text", s, s + 2)]
    }))
    .unwrap()
}

fn writer_answer(text: &str) -> Value {
    json!({
        "bottom_line": {"text": text, "certainty": "Low", "certainty_reason": ""},
        "outcomes": [], "studies": [], "conflicts": [],
        "applicability": {"case_mix": "", "india_cost_context": "", "training": ""},
        "would_change": []
    })
}

fn studies() -> Vec<StudyInput> {
    vec![StudyInput {
        pmid: SEEN_A.into(),
        title: "CHI".into(),
        journal: None,
        year: None,
        pub_types: vec![],
        doi: None,
        retracted: false,
        abstract_text: Some(ABSTRACT_A.into()),
        design: "other".into(),
        why: String::new(),
    }]
}

async fn run_step(c: &RunContext) -> Result<NumberCompletion, incisive_app::agent::llm::StepError> {
    numbers_complete::complete(c, &c.llm, &marked_answer(), &studies(), &[], |p| {
        p == SEEN_A
    })
    .await
}

fn sum_reply() -> Value {
    json!({"items": [{"id": "1", "pmid": SEEN_A, "quote": "10 robotic and 10 open", "rule": "sum"}]})
}

/// The step itself completes "20" from "10 … 10" (the scripted reply).
#[tokio::test]
async fn the_step_completes_a_sum() {
    let Some(pool) = pool().await else { return };
    let llm = Arc::new(ScriptedLlm::new().reply_json(sum_reply(), u()));
    let c = ctx(&pool, llm.clone(), 5000, Duration::ZERO).await;
    let out = run_step(&c).await.unwrap();
    assert_eq!(out.completed.len(), 1);
    let mut a = marked_answer();
    let before = serde_json::to_string(&a.answer).unwrap();
    numbers_complete::apply(&mut a, &out);
    assert!(a.untraced.is_empty());
    assert_eq!(serde_json::to_string(&a.answer).unwrap(), before);
    assert_eq!(llm.requests().len(), 1);
}

/// Money, time, an error, a timeout: skipped, the marks kept, no error for
/// the run. Cancellation alone propagates.
#[tokio::test]
async fn every_failure_keeps_the_marks() {
    let Some(pool) = pool().await else { return };
    let keeps = |out: &NumberCompletion, reason: &str| {
        assert_eq!(out.skipped.as_deref(), Some(reason));
        assert!(out.completed.is_empty());
        let mut a = marked_answer();
        let before = a.clone();
        numbers_complete::apply(&mut a, out);
        assert_eq!(a, before);
        assert_eq!(
            out.line(),
            format!("Number completeness skipped: {reason}; 1 left marked")
        );
    };
    // Money: nothing left of the limit. No call is made.
    let llm = Arc::new(ScriptedLlm::new().reply_json(sum_reply(), u()));
    let c = ctx(&pool, llm.clone(), 5000, Duration::ZERO).await;
    c.budget.record_spent_for_test(5000);
    keeps(&run_step(&c).await.unwrap(), "cost limit");
    assert!(llm.requests().is_empty());
    // Time: too little left before the hard stop. No call.
    let llm = Arc::new(ScriptedLlm::new().reply_json(sum_reply(), u()));
    let hard_stop = RunBudget::new(Uuid::nil(), 0, 1, Instant::now(), prices(), 88.0)
        .with_time_limit(DepthSettings::QUICK.agent_time_limit_s)
        .hard_stop();
    let age = hard_stop - Duration::from_secs(5);
    let c = ctx(&pool, llm.clone(), 5000, age).await;
    keeps(&run_step(&c).await.unwrap(), "time");
    assert!(llm.requests().is_empty());
    // An API error.
    let llm = Arc::new(ScriptedLlm::new().fail(ClaudeError::Api {
        status: 400,
        error_type: "invalid_request_error".into(),
        message: "schema".into(),
    }));
    let c = ctx(&pool, llm.clone(), 5000, Duration::ZERO).await;
    keeps(&run_step(&c).await.unwrap(), "error");
    assert_eq!(llm.requests().len(), 1);
    // A reply cut off by its token limit is unusable.
    let llm = Arc::new(ScriptedLlm::new().reply_stop("{\"items\": [", StopReason::MaxTokens, u()));
    let c = ctx(&pool, llm.clone(), 5000, Duration::ZERO).await;
    keeps(&run_step(&c).await.unwrap(), "unusable reply");
    // A reply slower than the wait.
    let llm = Arc::new(
        ScriptedLlm::new()
            .delay(Duration::from_millis(500))
            .reply_json(sum_reply(), u()),
    );
    let c = ctx(&pool, llm.clone(), 5000, Duration::ZERO).await;
    let p = Prompt {
        items: numbers_complete::collect(
            marked_answer().answer.as_ref().unwrap(),
            &marked_answer().untraced,
            &[],
            |p| p == SEEN_A,
        ),
        studies: vec![(SEEN_A.into(), ABSTRACT_A.into())],
    };
    keeps(
        &numbers_complete::complete_waiting(&c, &c.llm, &p, 1, Duration::from_millis(20))
            .await
            .unwrap(),
        "time",
    );
    // Cancelled: the run's own invariant propagates.
    let llm = Arc::new(ScriptedLlm::new().reply_json(sum_reply(), u()));
    let c = ctx(&pool, llm.clone(), 5000, Duration::ZERO).await;
    c.cancel.cancel();
    assert!(matches!(
        run_step(&c).await,
        Err(incisive_app::agent::llm::StepError::Cancelled)
    ));
    // No cited abstract: no call.
    let llm = Arc::new(ScriptedLlm::new().reply_json(sum_reply(), u()));
    let c = ctx(&pool, llm.clone(), 5000, Duration::ZERO).await;
    let out = numbers_complete::complete(&c, &c.llm, &marked_answer(), &[], &[], |p| p == SEEN_A)
        .await
        .unwrap();
    keeps(&out, "no cited abstract");
    assert!(llm.requests().is_empty());
}

/// The study sheet draws stored quotes as text (escaped), in the sheet
/// only (nothing new on the cards).
#[test]
fn the_study_sheet_escapes_the_stored_quote() {
    let mut a = marked_answer();
    a.references = serde_json::from_value(json!([
        {"n": 1, "pmid": SEEN_A, "title": "CHI", "journal": null, "year": 2025, "doi": null, "retracted": false}
    ]))
    .unwrap();
    a.answer.as_mut().unwrap().studies = serde_json::from_value(json!([
        {"pmid": SEEN_A, "label": "CHI (2025)", "design": "RCT", "n": 20, "setting": "", "comparison": "", "key_results": "", "appraisal": ""}
    ]))
    .unwrap();
    a.numbers_completed = serde_json::from_value(json!([{
        "span": a.untraced[0], "written": "20", "pmid": SEEN_A,
        "quote": "10 <b>robotic</b> and 10 open", "rule": "sum"
    }]))
    .unwrap();
    a.untraced.clear();
    let html = incisive_app::app::render_answer_v3(a);
    assert!(
        html.contains("10 &lt;b&gt;robotic&lt;/b&gt; and 10 open"),
        "escaped"
    );
    assert!(!html.contains("<b>robotic"));
    assert_eq!(
        html.matches("Numbers checked against this abstract")
            .count(),
        1
    );
    let sheet = &html[html.find("id=\"sheet-1\"").unwrap()..];
    assert!(
        sheet.find("Numbers checked against this abstract").unwrap()
            < sheet.find("</dialog>").unwrap()
    );
}

/// The deploy workflow passes `NUMBER_COMPLETE` to both steps, writes it
/// only when set, and refuses anything but on or off (ends trimmed).
#[test]
fn deploy_workflow_wires_the_switch() {
    let deploy = std::fs::read_to_string(format!(
        "{}/../../.github/workflows/deploy.yml",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap();
    assert_eq!(
        deploy
            .matches("NUMBER_COMPLETE: ${{ vars.NUMBER_COMPLETE }}")
            .count(),
        2
    );
    assert!(deploy.contains(r#"optional += ["NUMBER_COMPLETE"]"#));
    let start = deploy.find("nc=\"${NUMBER_COMPLETE#").unwrap();
    let end = start + deploy[start..].find("esac").unwrap() + 4;
    let snippet: String = deploy[start..end]
        .lines()
        .map(str::trim_start)
        .collect::<Vec<_>>()
        .join("\n");
    for (value, ok) in [
        ("on", true),
        (" off ", true),
        ("", true),
        ("ON", false),
        ("o n", false),
    ] {
        let status = std::process::Command::new("bash")
            .arg("-c")
            .arg(format!("{snippet}\nexit 0"))
            .env("NUMBER_COMPLETE", value)
            .status()
            .expect("bash");
        assert_eq!(status.success(), ok, "NUMBER_COMPLETE={value:?}");
    }
}
