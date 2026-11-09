//! S1c: the finished At a glance (and context) of a Quick answer written in
//! four parts, shown early. A scripted Claude and wiremock literature (no
//! network), the harness of `quick_parallel.rs`; the page server-rendered
//! through the real routes, as `stream_ui.rs` does. Needs `TEST_DATABASE_URL`
//! (skipped when unset). Nothing here has run in a browser.
#![allow(dead_code)]

use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use claude_client::{ContentBlock, MessagesRequest, StopReason, Usage};
use http_body_util::BodyExt;
use incisive_app::agent::answer_v3::AnswerV3;
use incisive_app::agent::llm::{LlmClient, ScriptedLlm};
use incisive_app::agent::team::{early_body, run_team};
use incisive_app::agent::writer_v3::{
    ContextPart, PartEvent, SECTION_HEAD, STUDIES_HEADING, Section, SummaryPart, reserve_for,
    sections_reserve,
};
use incisive_app::agent::{AgentLlm, Literature, PipelineFuture, PipelineOutcome, RunContext};
use incisive_app::db::{lazy_pool, migrate};
use incisive_app::draft_page::{EarlyAnswer, REWRITING};
use incisive_app::gate::OfflineClassifier;
use incisive_app::identity::{IapVerifier, JwkSource};
use incisive_app::lit::europepmc::EuropePmc;
use incisive_app::lit::http::{AllowlistedClient, OaPolicy};
use incisive_app::lit::pubmed::PubMed;
use incisive_app::runs::budget::RunBudget;
use incisive_app::runs::drafts::{self, Part};
use incisive_app::runs::store::{add_step, create_run, save_answer, set_status};
use incisive_app::runs::worker::{CancelFlag, WorkerHandle, recover_stale_runs};
use incisive_app::runs::{
    DepthSettings, QuickEngine, RunMode, RunStatus, RunsConfig, StepKind, api,
};
use incisive_app::server::AppState;
use incisive_app::spend::{CapCheck, CapsConfig, Prices, reserve};
use leptos::config::LeptosOptions;
use secrecy::SecretString;
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const MODEL: &str = "m-test";
const QUESTION: &str = "In adults with a primary inguinal hernia, does mesh type change chronic pain after laparoscopic repair?";
const SEEN_A: &str = "41060645";
const SEEN_B: &str = "39979619";
const UNSEEN: &str = "12345678";
const UNKNOWN_CITE: &str = "99999999";
const FINAL_KEY: &str = "Research transcript";
const BANNER: &str = "Being written: checks not yet run";

// ---------- harness (as quick_parallel.rs) ----------

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

async fn pool() -> Option<PgPool> {
    let Ok(url) = std::env::var("TEST_DATABASE_URL") else {
        eprintln!("SKIP: TEST_DATABASE_URL not set");
        return None;
    };
    let pool = lazy_pool(&SecretString::from(url)).expect("lazy pool");
    migrate(&pool).await.expect("migrate");
    Some(pool)
}

fn lit_fixture(name: &str) -> String {
    std::fs::read_to_string(format!(
        "{}/tests/fixtures/lit/{name}",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap()
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
        "/entrez/eutils/esearch.fcgi",
        200,
        lit_fixture("pubmed_esearch_hernia.json"),
    )
    .await;
    mount(
        &server,
        "/entrez/eutils/esummary.fcgi",
        200,
        lit_fixture("pubmed_esummary_hernia.json"),
    )
    .await;
    mount(
        &server,
        "/entrez/eutils/efetch.fcgi",
        200,
        lit_fixture("pubmed_efetch_hernia.xml"),
    )
    .await;
    mount(&server, "/entrez/eutils/elink.fcgi", 500, String::new()).await;
    mount(
        &server,
        "/europepmc/webservices/rest/search",
        500,
        String::new(),
    )
    .await;
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
        memory: Default::default(),
        chat: Default::default(),
        embedder: None,
        gemma: None,
        worker: WorkerHandle::with_pipeline(incisive_app::agent::testing::placeholder_pipeline),
        llm: Some(incisive_app::agent::testing::idle_llm(MODEL)),
        lit,
    }
}

fn agent_llm(llm: Arc<dyn LlmClient>) -> Arc<AgentLlm> {
    incisive_app::agent::testing::agent_llm(llm, MODEL)
}

async fn new_user(pool: &PgPool) -> (i64, String) {
    let email = format!("s1c-{}@example.com", Uuid::new_v4());
    let uid: i64 = sqlx::query_scalar(
        "INSERT INTO incisive.users (email, display_name, role, daily_cap_paise) \
         VALUES ($1, 'T', 'surgeon', 100000) RETURNING id",
    )
    .bind(&email)
    .fetch_one(pool)
    .await
    .unwrap();
    (uid, email)
}

/// A Quick run with `limit` reserved (queued).
async fn reserved_run(pool: &PgPool, uid: i64, email: &str, limit: i64) -> Uuid {
    let run = create_run(pool, uid, QUESTION, RunMode::Quick, limit)
        .await
        .unwrap()
        .unwrap();
    let user = incisive_app::identity::find_user(pool, email)
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
    run
}

/// A running Quick run's context, the switch `on` or off.
async fn ctx_with(
    pool: &PgPool,
    server: &MockServer,
    llm: Arc<dyn LlmClient>,
    limit: i64,
    on: bool,
) -> RunContext {
    let (uid, email) = new_user(pool).await;
    let run = reserved_run(pool, uid, &email, limit).await;
    set_status(pool, run, RunStatus::Running, None)
        .await
        .unwrap();
    let settings = DepthSettings::QUICK;
    let budget = Arc::new(
        RunBudget::new(run, uid, limit, Instant::now(), prices(), 88.0)
            .with_time_limit(settings.agent_time_limit_s),
    );
    let lit = literature(server);
    let mut state = app_state(pool.clone(), lit.clone());
    state.runs.quick_parallel = on;
    let user = incisive_app::identity::find_user(pool, &email)
        .await
        .unwrap()
        .unwrap();
    RunContext {
        state,
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

fn research_then(llm: ScriptedLlm) -> ScriptedLlm {
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
    let llm = llm.reply_blocks(
        vec![ContentBlock::Text {
            text: "Done searching.".into(),
        }],
        Some(StopReason::EndTurn),
        u(),
    );
    llm.when(FINAL_KEY).reply_json(
        json!({
            "searches": [{"query": "made up", "source": "pubmed", "hits": 999}],
            "selected": [
                {"pmid": SEEN_A, "design": "RCT", "why": "key"},
                {"pmid": UNSEEN, "design": "RCT", "why": "key"},
                {"pmid": SEEN_B, "design": "SR_MA", "why": "key"}
            ],
            "considered_not_selected": [],
            "possible_contradictions": []
        }),
        u(),
    )
}

/// The whole reply: the summary and context parts hold a citation the check
/// removes and numbers the trace cannot find (marked "not traced").
fn writer_json() -> Value {
    json!({
        "bottom_line": {
            "text": format!("Mesh type made no difference to chronic pain in 413 patients [{SEEN_A}; {UNKNOWN_CITE}]."),
            "certainty": "Low",
            "certainty_reason": format!("Two small trials [{SEEN_B}]")
        },
        "outcomes": [{
            "outcome": "Chronic pain",
            "rct_evidence": "Two RCTs",
            "effect_text": format!("No difference [{SEEN_A}]"),
            "numbers": [
                {"label": "VAS", "value": 1.2, "unit": null, "arm": "biological", "ci_low": null, "ci_high": null, "p": "0.4", "pmid": SEEN_A}
            ],
            "certainty": "Low",
            "why": "small",
            "pmids": [SEEN_A, UNKNOWN_CITE]
        }],
        "studies": [
            {"pmid": SEEN_A, "label": "Biological vs synthetic (2025)", "design": "RCT", "n": 100, "setting": "multicentre", "comparison": "biological vs synthetic", "key_results": format!("no difference [{SEEN_A}]"), "appraisal": "short follow-up"}
        ],
        "conflicts": [{"text": format!("Self-fixating mesh differs [{SEEN_B}]"), "pmids": [SEEN_B]}],
        "applicability": {"case_mix": format!("primary hernias, 731 men [{SEEN_B}; {UNKNOWN_CITE}]"), "india_cost_context": "Indian mesh costs are not known.", "training": "No particular bearing."},
        "would_change": [format!("A larger multicentre RCT of 2000 patients [{UNKNOWN_CITE}]")],
        "at_a_glance": format!("No difference in chronic pain.\nChronic pain: VAS 1.2 vs 8.7 with biological mesh [{SEEN_A}]\nMain caveat: two small trials"),
        "clinical_practice": "Default choice: either mesh.\nCost per case: about 4321 rupees",
        "notable_findings": format!("- Short follow-up [{SEEN_A}].")
    })
}

fn part(s: Section) -> Value {
    let w = writer_json();
    let mut m = serde_json::Map::new();
    for f in s.fields() {
        m.insert((*f).to_string(), w[*f].clone());
    }
    Value::Object(m)
}

fn key(s: Section) -> String {
    format!("{SECTION_HEAD}{}", s.name().to_uppercase())
}

fn sections_after(mut llm: ScriptedLlm, which: &[Section]) -> ScriptedLlm {
    for s in which {
        llm = llm.when(&key(*s)).reply_json(part(*s), usage(3000, 700));
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

fn body_of(out: &PipelineOutcome) -> AnswerV3 {
    match out {
        PipelineOutcome::Done(v)
        | PipelineOutcome::Partial(v)
        | PipelineOutcome::PartialKind(v, _) => serde_json::from_value(v.clone()).unwrap(),
        PipelineOutcome::Failed(k) => panic!("failed: {k}"),
    }
}

/// The smallest limit that writes in four parts (as quick_parallel.rs).
fn parallel_limit() -> i64 {
    let b = Arc::new(RunBudget::new(
        Uuid::nil(),
        1,
        1,
        Instant::now(),
        prices(),
        88.0,
    ));
    let llm = agent_llm(Arc::new(ScriptedLlm::new()));
    let w = DepthSettings::QUICK.writer_max_tokens;
    let single = reserve_for(&llm, &b, QUESTION, w).unwrap();
    let parallel = sections_reserve(&llm, &b, QUESTION, w).unwrap();
    parallel + (DepthSettings::QUICK.agent_limit_paise - single)
}

async fn has_early(pool: &PgPool, run: Uuid) -> bool {
    drafts::get(pool, run, Part::Early).await.unwrap().is_some()
}

async fn early_of(pool: &PgPool, run: Uuid) -> Option<EarlyAnswer> {
    drafts::get(pool, run, Part::Early)
        .await
        .unwrap()
        .map(|t| serde_json::from_str(&t).expect("early answer JSON"))
}

const EARLY_FIELDS: [&str; 5] = [
    "at_a_glance.",
    "bottom_line.",
    "clinical_practice.",
    "applicability.",
    "would_change.",
];

// ---------- the engine ----------

/// The early answer holds exactly what the final page shows for the summary
/// and context parts: the same text after the citation check, the same "not
/// traced" marks, the same references (so the same citation numbers).
#[tokio::test]
async fn the_early_answer_equals_the_finals_matching_fields() {
    let Some(pool) = pool().await else { return };
    let server = hernia_server().await;
    let llm = Arc::new(sections_after(
        research_then(ScriptedLlm::new()),
        &Section::ALL,
    ));
    let ctx = ctx_with(&pool, &server, llm, parallel_limit(), true).await;
    let out = run_team(&ctx).await;
    assert!(matches!(out, PipelineOutcome::Done(_)), "{out:?}");
    let a = body_of(&out);
    // Without the worker nothing has cleared it yet: the last one stored.
    let e = early_of(&pool, ctx.run_id).await.expect("an early answer");
    assert!(e.summary && e.context);
    let (ew, fw) = (
        e.answer.answer.as_ref().unwrap(),
        a.answer.as_ref().unwrap(),
    );
    assert_eq!(ew.bottom_line, fw.bottom_line);
    assert_eq!(ew.at_a_glance, fw.at_a_glance);
    assert_eq!(ew.clinical_practice, fw.clinical_practice);
    assert_eq!(ew.applicability, fw.applicability);
    assert_eq!(ew.would_change, fw.would_change);
    assert!(ew.at_a_glance.is_some() && ew.clinical_practice.is_some());
    // The citation check ran: the unknown study is gone, the known kept.
    assert!(!ew.bottom_line.text.contains(UNKNOWN_CITE));
    assert!(ew.bottom_line.text.contains(SEEN_A));
    assert!(!ew.applicability.case_mix.contains(UNKNOWN_CITE));
    assert!(ew.applicability.case_mix.contains(SEEN_B));
    assert!(!ew.would_change[0].contains(UNKNOWN_CITE));
    // Nothing of the other parts.
    assert!(ew.outcomes.is_empty() && ew.studies.is_empty() && ew.conflicts.is_empty());
    assert!(ew.notable_findings.is_empty());
    // The number check ran: the same marks on these fields.
    let ours = |spans: &[incisive_app::agent::answer_v3::Span]| {
        let mut v: Vec<_> = spans
            .iter()
            .filter(|s| EARLY_FIELDS.iter().any(|f| s.field.starts_with(f)))
            .cloned()
            .collect();
        v.sort_by(|x, y| (&x.field, x.start).cmp(&(&y.field, y.start)));
        v
    };
    assert!(
        !ours(&e.answer.untraced).is_empty(),
        "the test needs a mark"
    );
    assert_eq!(ours(&e.answer.untraced), ours(&a.untraced));
    assert_eq!(e.answer.untraced.len(), ours(&e.answer.untraced).len());
    // The same reference numbers.
    assert!(!a.references.is_empty());
    assert_eq!(e.answer.references, a.references);
    assert_eq!(e.answer.ref_numbers(), a.ref_numbers());
}

/// The parts that finish early are reported as soon as each is read: with
/// the trials part's call held back, the summary and context are stored
/// before it is answered.
struct HoldTrials {
    inner: Arc<ScriptedLlm>,
    pool: PgPool,
    run: std::sync::Mutex<Option<Uuid>>,
    seen: std::sync::Mutex<Option<EarlyAnswer>>,
}

#[async_trait::async_trait]
impl LlmClient for HoldTrials {
    async fn call(
        &self,
        req: &MessagesRequest,
    ) -> Result<claude_client::MessagesResponse, claude_client::ClaudeError> {
        let run = *self.run.lock().unwrap();
        if let Some(run) = run
            && request_user_text(req).contains(&key(Section::Trials))
        {
            let mut got = None;
            for _ in 0..300 {
                got = early_of(&self.pool, run).await;
                if got.as_ref().is_some_and(|e| e.summary && e.context) {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            *self.seen.lock().unwrap() = got;
        }
        self.inner.call(req).await
    }
}

#[tokio::test]
async fn the_summary_and_context_are_stored_before_the_trials_are_in() {
    let Some(pool) = pool().await else { return };
    let server = hernia_server().await;
    let inner = Arc::new(sections_after(
        research_then(ScriptedLlm::new()),
        &Section::ALL,
    ));
    let probe = Arc::new(HoldTrials {
        inner,
        pool: pool.clone(),
        run: std::sync::Mutex::new(None),
        seen: std::sync::Mutex::new(None),
    });
    let ctx = ctx_with(&pool, &server, probe.clone(), parallel_limit(), true).await;
    *probe.run.lock().unwrap() = Some(ctx.run_id);
    let out = run_team(&ctx).await;
    assert!(matches!(out, PipelineOutcome::Done(_)), "{out:?}");
    let seen = probe
        .seen
        .lock()
        .unwrap()
        .clone()
        .expect("stored while the trials were held");
    assert!(seen.summary && seen.context);
    let a = body_of(&out);
    assert_eq!(
        seen.answer.answer.unwrap().bottom_line,
        a.answer.unwrap().bottom_line
    );
}

/// D1 A: a part fails twice, the answer is written in one piece, and the
/// early answer (the summary, stored before) is deleted.
struct EarlyProbe {
    inner: Arc<ScriptedLlm>,
    pool: PgPool,
    run: std::sync::Mutex<Option<Uuid>>,
    /// Early rows seen when the failing part's retry arrived.
    before: std::sync::Mutex<Vec<bool>>,
    /// Early rows seen (after up to 2 s) when the one-piece writer arrived.
    at_one_piece: std::sync::Mutex<Vec<bool>>,
}

#[async_trait::async_trait]
impl LlmClient for EarlyProbe {
    async fn call(
        &self,
        req: &MessagesRequest,
    ) -> Result<claude_client::MessagesResponse, claude_client::ClaudeError> {
        let text = request_user_text(req);
        let run = *self.run.lock().unwrap();
        if let Some(run) = run
            && text.contains(&key(Section::Outcomes))
            && text.contains(incisive_app::agent::llm::SCHEMA_NUDGE.trim())
        {
            let mut has = false;
            for _ in 0..200 {
                has = early_of(&self.pool, run).await.is_some();
                if has {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            self.before.lock().unwrap().push(has);
        }
        if let Some(run) = run
            && text.contains(STUDIES_HEADING)
            && !text.contains(SECTION_HEAD)
        {
            let mut has = true;
            for _ in 0..200 {
                has = early_of(&self.pool, run).await.is_some();
                if !has {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            self.at_one_piece.lock().unwrap().push(has);
        }
        self.inner.call(req).await
    }
}

#[tokio::test]
async fn the_one_piece_fallback_deletes_the_early_answer() {
    let Some(pool) = pool().await else { return };
    let server = hernia_server().await;
    let cut = "{\"outcomes\": [{\"outcome\": \"cut";
    let mut llm = research_then(ScriptedLlm::new());
    for _ in 0..2 {
        llm = llm.when(&key(Section::Outcomes)).reply_stop(
            cut,
            StopReason::MaxTokens,
            usage(3000, 5000),
        );
    }
    let llm = sections_after(llm, &[Section::Trials, Section::Context, Section::Summary]);
    let mut one = writer_json();
    one["bottom_line"]["text"] = json!("From the one-piece writer.");
    let inner = Arc::new(llm.when(STUDIES_HEADING).reply_json(one, usage(3000, 2800)));
    let probe = Arc::new(EarlyProbe {
        inner,
        pool: pool.clone(),
        run: std::sync::Mutex::new(None),
        before: std::sync::Mutex::new(Vec::new()),
        at_one_piece: std::sync::Mutex::new(Vec::new()),
    });
    let ctx = ctx_with(&pool, &server, probe.clone(), parallel_limit(), true).await;
    *probe.run.lock().unwrap() = Some(ctx.run_id);
    let out = run_team(&ctx).await;
    assert!(matches!(out, PipelineOutcome::Done(_)), "{out:?}");
    assert_eq!(*probe.before.lock().unwrap(), [true], "shown before");
    assert_eq!(
        *probe.at_one_piece.lock().unwrap(),
        [false],
        "gone for the one piece"
    );
    // And it stays gone: nothing stored it again.
    assert!(early_of(&pool, ctx.run_id).await.is_none());
    let a = body_of(&out);
    assert_eq!(
        a.answer.unwrap().bottom_line.text,
        "From the one-piece writer."
    );
}

/// Holds chosen parts' calls until a condition, and watches the early row.
struct Ordered {
    inner: Arc<ScriptedLlm>,
    pool: PgPool,
    run: std::sync::Mutex<Option<Uuid>>,
    /// Set when the trials part's retry has been answered.
    trials_done: std::sync::atomic::AtomicBool,
    /// Case A: hold the summary and context until the trials failed for good.
    hold_summary: bool,
    /// Case B: hold the trials retry until the early row exists, and the
    /// outcomes until it is gone (recorded).
    hold_trials: bool,
    outcomes_saw_gone: std::sync::Mutex<Option<bool>>,
}

impl Ordered {
    fn new(
        inner: Arc<ScriptedLlm>,
        pool: PgPool,
        hold_summary: bool,
        hold_trials: bool,
    ) -> Ordered {
        Ordered {
            inner,
            pool,
            run: std::sync::Mutex::new(None),
            trials_done: std::sync::atomic::AtomicBool::new(false),
            hold_summary,
            hold_trials,
            outcomes_saw_gone: std::sync::Mutex::new(None),
        }
    }
}

#[async_trait::async_trait]
impl LlmClient for Ordered {
    async fn call(
        &self,
        req: &MessagesRequest,
    ) -> Result<claude_client::MessagesResponse, claude_client::ClaudeError> {
        use std::sync::atomic::Ordering;
        let text = request_user_text(req);
        let run = (*self.run.lock().unwrap()).unwrap();
        let retry = text.contains(incisive_app::agent::llm::SCHEMA_NUDGE.trim());
        let is = |s: Section| text.contains(&key(s));
        if self.hold_summary && (is(Section::Summary) || is(Section::Context)) {
            for _ in 0..300 {
                if self.trials_done.load(Ordering::Acquire) {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            // The trials' final failure is reported by now.
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        if self.hold_trials && is(Section::Trials) && retry {
            for _ in 0..300 {
                if has_early(&self.pool, run).await {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }
        if self.hold_trials && is(Section::Outcomes) {
            let mut gone = false;
            for _ in 0..300 {
                if self.trials_done.load(Ordering::Acquire) && !has_early(&self.pool, run).await {
                    gone = true;
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            *self.outcomes_saw_gone.lock().unwrap() = Some(gone);
        }
        let r = self.inner.call(req).await;
        if is(Section::Trials) && retry {
            self.trials_done.store(true, Ordering::Release);
        }
        r
    }
}

/// The trials part cut off twice, the other parts and the one piece scripted.
fn trials_fail_script() -> Arc<ScriptedLlm> {
    let cut = "{\"studies\": [{\"pmid\": \"cut";
    let mut llm = research_then(ScriptedLlm::new());
    for _ in 0..2 {
        llm = llm.when(&key(Section::Trials)).reply_stop(
            cut,
            StopReason::MaxTokens,
            usage(3000, 5000),
        );
    }
    let llm = sections_after(
        llm,
        &[Section::Outcomes, Section::Context, Section::Summary],
    );
    Arc::new(
        llm.when(STUDIES_HEADING)
            .reply_json(writer_json(), usage(3000, 2800)),
    )
}

/// Polls the early row until stopped; true if it was ever there.
fn watch_early(
    pool: PgPool,
    run: Uuid,
) -> (
    Arc<std::sync::atomic::AtomicBool>,
    tokio::task::JoinHandle<bool>,
) {
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let s = stop.clone();
    let h = tokio::spawn(async move {
        let mut seen = false;
        while !s.load(std::sync::atomic::Ordering::Acquire) {
            seen |= has_early(&pool, run).await;
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
        seen
    });
    (stop, h)
}

/// Review 1: a part that fails for good before the summary is read means
/// no early answer is ever stored (the summary read after it is not).
#[tokio::test]
async fn a_part_failing_before_the_summary_stores_no_early_answer() {
    let Some(pool) = pool().await else { return };
    let server = hernia_server().await;
    let probe = Arc::new(Ordered::new(
        trials_fail_script(),
        pool.clone(),
        true,
        false,
    ));
    let ctx = ctx_with(&pool, &server, probe.clone(), parallel_limit(), true).await;
    *probe.run.lock().unwrap() = Some(ctx.run_id);
    let (stop, watch) = watch_early(pool.clone(), ctx.run_id);
    let out = run_team(&ctx).await;
    stop.store(true, std::sync::atomic::Ordering::Release);
    assert!(matches!(out, PipelineOutcome::Done(_)), "{out:?}");
    assert!(probe.trials_done.load(std::sync::atomic::Ordering::Acquire));
    assert!(!watch.await.unwrap(), "never stored");
    assert!(!has_early(&pool, ctx.run_id).await);
}

/// Review 1: a part failing for good after the summary was stored deletes
/// it at once, before the other parts are in (the outcomes, held, see it
/// gone before they answer).
#[tokio::test]
async fn a_part_failing_after_the_summary_deletes_it_before_the_others_finish() {
    let Some(pool) = pool().await else { return };
    let server = hernia_server().await;
    let probe = Arc::new(Ordered::new(
        trials_fail_script(),
        pool.clone(),
        false,
        true,
    ));
    let ctx = ctx_with(&pool, &server, probe.clone(), parallel_limit(), true).await;
    *probe.run.lock().unwrap() = Some(ctx.run_id);
    let (stop, watch) = watch_early(pool.clone(), ctx.run_id);
    let out = run_team(&ctx).await;
    stop.store(true, std::sync::atomic::Ordering::Release);
    assert!(matches!(out, PipelineOutcome::Done(_)), "{out:?}");
    assert!(watch.await.unwrap(), "stored first");
    assert_eq!(
        *probe.outcomes_saw_gone.lock().unwrap(),
        Some(true),
        "deleted before the outcomes were in"
    );
    assert!(!has_early(&pool, ctx.run_id).await);
}

/// Review 4: reporting the parts changes nothing in the answer: the writer
/// with a channel gives what it gives without one, for the same replies.
#[tokio::test]
async fn reporting_the_parts_leaves_the_answer_unchanged() {
    let Some(pool) = pool().await else { return };
    let server = hernia_server().await;
    let user = format!("{STUDIES_HEADING}\nthe same user message");
    let mut got = Vec::new();
    for with in [false, true] {
        let llm = Arc::new(sections_after(ScriptedLlm::new(), &Section::ALL));
        let ctx = ctx_with(&pool, &server, llm, parallel_limit(), true).await;
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let r = incisive_app::agent::writer_v3::write_quick_parallel_with(
            &ctx,
            &ctx.llm,
            &user,
            with.then_some(tx),
        )
        .await;
        let Ok(r) = r else { panic!("writer failed") };
        let mut events = 0;
        while let Ok(e) = rx.try_recv() {
            assert!(matches!(e, PartEvent::Summary(_) | PartEvent::Context(_)));
            events += 1;
        }
        assert_eq!(events, if with { 2 } else { 0 });
        got.push(r);
    }
    assert!(got[0].2 && got[1].2, "merged");
    assert_eq!(got[0].0, got[1].0);
    assert_eq!(got[0].1, got[1].1);
}

/// The switch off, or a run that writes in one piece: no early answer.
#[tokio::test]
async fn switch_off_or_a_one_piece_run_stores_no_early_answer() {
    let Some(pool) = pool().await else { return };
    let server = hernia_server().await;
    let limit = parallel_limit();
    for (label, limit, on) in [
        ("switch off", limit, false),
        ("limit too low", limit - 1, true),
    ] {
        let llm = Arc::new(
            research_then(ScriptedLlm::new())
                .when(STUDIES_HEADING)
                .reply_json(writer_json(), usage(3000, 2800)),
        );
        let ctx = ctx_with(&pool, &server, llm.clone(), limit, on).await;
        let out = run_team(&ctx).await;
        assert!(matches!(out, PipelineOutcome::Done(_)), "{label}: {out:?}");
        assert!(
            llm.requests()
                .iter()
                .all(|r| !request_user_text(r).contains(SECTION_HEAD)),
            "{label}: one piece"
        );
        assert!(early_of(&pool, ctx.run_id).await.is_none(), "{label}");
    }
}

// ---------- the row's life ----------

/// Stores an early answer, then ends as the outcome named by the run's
/// question-free cost limit (see `ends`).
fn stores_early_then_ends(ctx: &RunContext) -> PipelineFuture<'_> {
    Box::pin(async move {
        assert!(
            drafts::upsert(&ctx.state.pool, ctx.run_id, Part::Early, "{}")
                .await
                .unwrap()
        );
        match ctx.budget.limit_paise() {
            1001 => PipelineOutcome::Done(json!({"version": 3})),
            1002 => PipelineOutcome::PartialKind(json!({"version": 3}), "writer_truncated"),
            _ => PipelineOutcome::Failed("llm"),
        }
    })
}

async fn wait_idle(st: &AppState, run: Uuid) {
    for _ in 0..500 {
        if !st.worker.is_running(run) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("run did not finish within 5 s");
}

#[tokio::test]
async fn the_early_row_goes_when_the_run_ends_any_way() {
    let Some(pool) = pool().await else { return };
    let st = app_state(
        pool.clone(),
        incisive_app::agent::testing::offline_literature(),
    );
    for (limit, want) in [
        (1001, RunStatus::Done),
        (1002, RunStatus::Partial),
        (1003, RunStatus::Failed),
    ] {
        let (uid, email) = new_user(&pool).await;
        let run = reserved_run(&pool, uid, &email, limit).await;
        st.worker
            .spawn_with(st.clone(), run, stores_early_then_ends);
        wait_idle(&st, run).await;
        let v = incisive_app::runs::store::get_run_for_user(&pool, uid, run)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(v.summary.status, want);
        assert!(!has_early(&pool, run).await, "{want:?}: deleted");
        // A late write stores nothing.
        assert!(!drafts::upsert(&pool, run, Part::Early, "{}").await.unwrap());
    }
}

#[tokio::test]
async fn recovery_removes_stale_early_rows() {
    let Some(pool) = pool().await else { return };
    let (uid, email) = new_user(&pool).await;
    // Ended: swept. Live: kept.
    let ended = reserved_run(&pool, uid, &email, 1000).await;
    set_status(&pool, ended, RunStatus::Running, None)
        .await
        .unwrap();
    assert!(
        drafts::upsert(&pool, ended, Part::Early, "{}")
            .await
            .unwrap()
    );
    set_status(&pool, ended, RunStatus::Failed, Some("x"))
        .await
        .unwrap();
    let (uid2, email2) = new_user(&pool).await;
    let live = reserved_run(&pool, uid2, &email2, 1000).await;
    set_status(&pool, live, RunStatus::Running, None)
        .await
        .unwrap();
    sqlx::query("UPDATE incisive.runs SET heartbeat_at = now() WHERE id = $1")
        .bind(live)
        .execute(&pool)
        .await
        .unwrap();
    assert!(
        drafts::upsert(&pool, live, Part::Early, "{}")
            .await
            .unwrap()
    );
    drafts::sweep(&pool).await.unwrap();
    assert!(!has_early(&pool, ended).await);
    assert!(has_early(&pool, live).await);
    // An abandoned run is interrupted by recovery, and its early row goes.
    sqlx::query("UPDATE incisive.runs SET heartbeat_at = now() - interval '1 hour' WHERE id = $1")
        .bind(live)
        .execute(&pool)
        .await
        .unwrap();
    recover_stale_runs(&pool).await.unwrap();
    assert!(!has_early(&pool, live).await);
}

// ---------- the page ----------

fn ui_state(pool: PgPool, stream: bool, on: bool) -> AppState {
    let mut st = app_state(pool, incisive_app::agent::testing::offline_literature());
    st.quick_engine = QuickEngine::Classic;
    st.stream = stream;
    st.runs.quick_parallel = on;
    st
}

struct AlwaysUp;

#[async_trait::async_trait]
impl incisive_app::health::Readiness for AlwaysUp {
    async fn db_ok(&self) -> bool {
        true
    }
}

fn router(st: AppState) -> Router {
    let opts = LeptosOptions::builder().output_name("incisive-app").build();
    incisive_app::server::build_router(Arc::new(AlwaysUp), st, opts)
}

async fn page(st: &AppState, run: Uuid, email: &str) -> (StatusCode, String) {
    let req = Request::builder()
        .method("GET")
        .uri(format!("/runs/{run}"))
        .header("x-dev-user-email", email)
        .body(Body::empty())
        .unwrap();
    let res = router(st.clone()).oneshot(req).await.unwrap();
    let status = res.status();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

/// As `stream_ui.rs` reads a page: no scripts, nonces blanked, streamed
/// chunks back in their slots.
fn markup(html: &str) -> String {
    let mut out = String::new();
    let mut rest = html;
    while let Some(i) = rest.find("<script") {
        out.push_str(&rest[..i]);
        rest = rest[i..]
            .find("</script>")
            .map_or("", |j| &rest[i + j + "</script>".len()..]);
    }
    out.push_str(rest);
    let nonce = regex::Regex::new(r#"(crossorigin|nonce)="[^"]*""#).unwrap();
    let mut m = nonce.replace_all(&out, "$1=\"NONCE\"").into_owned();
    let tpl = regex::Regex::new(r#"<template id="(\d+)-f">"#).unwrap();
    while let Some(c) = tpl.captures(&m) {
        let n = c[1].to_string();
        let at = c.get(0).unwrap();
        let close = at.end() + m[at.end()..].find("</template>").expect("template end");
        let content = m[at.end()..close].to_string();
        m.replace_range(at.start()..close + "</template>".len(), "");
        let (o, cl) = (format!("<!--s-{n}-o-->"), format!("<!--s-{n}-c-->"));
        let (Some(a), Some(b)) = (m.find(&o), m.find(&cl)) else {
            panic!("no slot for template {n}");
        };
        m.replace_range(a..b + cl.len(), &content);
    }
    let slot = regex::Regex::new(r"<!--s-\d+-[oc]-->").unwrap();
    slot.replace_all(&m, "").replace("<!>", "")
}

fn strip(m: &str, open: &str, close: &str) -> String {
    let mut out = String::new();
    let mut rest = m;
    while let Some(i) = rest.find(open) {
        out.push_str(&rest[..i + open.len()]);
        let after = &rest[i + open.len()..];
        rest = after.find(close).map_or("", |j| &after[j..]);
    }
    out.push_str(rest);
    out
}

/// The clock and the fill are drawn from the time now.
fn strip_clock(m: &str) -> String {
    let m = strip(&strip(m, "rk-clock\">", "<"), "style=\"background", "\"");
    let m = strip(&m, "dr-clock\">", "<");
    strip(&m, "animation-delay:-", "\"")
}

fn at(html: &str, s: &str) -> usize {
    html.find(s).unwrap_or_else(|| panic!("missing {s:?}"))
}

/// `html` from `start` to the first `end` after it.
fn piece<'a>(html: &'a str, start: &str, end: &str) -> &'a str {
    let a = at(html, start);
    let b = a + at(&html[a..], end) + end.len();
    &html[a..b]
}

/// The hydration payload names the early answer (plain or escaped JSON).
fn early_key(html: &str) -> bool {
    html.contains("\"early\"") || html.contains("\\\"early\\\"")
}

fn fixture() -> Value {
    let p = format!(
        "{}/tests/fixtures/temporal/full/hernia.json",
        env!("CARGO_MANIFEST_DIR")
    );
    serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap()
}

/// The fixture's summary and context parts as the engine makes an early
/// answer of them (`team::early_body`), on the fixture's answer as the base.
fn early_via_body(summary: bool, context: bool) -> EarlyAnswer {
    let f = fixture();
    let w = &f["answer"];
    let s: SummaryPart = serde_json::from_value(json!({
        "bottom_line": w["bottom_line"],
        "at_a_glance": w["at_a_glance"],
        "clinical_practice": w["clinical_practice"],
    }))
    .unwrap();
    let c: ContextPart = serde_json::from_value(json!({
        "applicability": w["applicability"],
        "would_change": w["would_change"],
    }))
    .unwrap();
    let mut base: AnswerV3 = serde_json::from_value(f).unwrap();
    base.answer = None;
    let selected: Vec<String> = base.references.iter().map(|r| r.pmid.clone()).collect();
    early_body(
        &base,
        summary.then_some(&s),
        context.then_some(&c),
        &selected,
        &[],
    )
}

/// A real four-part run (`run_team`, the scripted replies): the early
/// answer it stored, as stored, and its final answer.
async fn a_real_four_part_run(pool: &PgPool) -> (String, Value) {
    let server = hernia_server().await;
    let llm = Arc::new(sections_after(
        research_then(ScriptedLlm::new()),
        &Section::ALL,
    ));
    let ctx = ctx_with(pool, &server, llm, parallel_limit(), true).await;
    let out = run_team(&ctx).await;
    let body = match out {
        PipelineOutcome::Done(v) => v,
        other => panic!("{other:?}"),
    };
    let early = drafts::get(pool, ctx.run_id, Part::Early)
        .await
        .unwrap()
        .expect("an early answer");
    (early, body)
}

/// The glance card with the tiles' source labels blanked: the study's
/// short name comes from the trials part, so the early card reads "Ref [n]"
/// where the final reads the name (accepted by the owner); the reference
/// number in it is kept.
fn without_tile_names(card: &str) -> String {
    let re = regex::Regex::new(r#"(<a [^>]*ap-tile-src[^>]*>)([^<]*?)( \[\d+\])?(</a>)"#).unwrap();
    re.replace_all(card, "$1NAME$3$4").into_owned()
}

/// A running Quick run at the writer (four parts).
async fn writing(pool: &PgPool, uid: i64) -> Uuid {
    let run = create_run(pool, uid, "Does robotic repair help?", RunMode::Quick, 4000)
        .await
        .unwrap()
        .unwrap();
    assert!(
        set_status(pool, run, RunStatus::Running, None)
            .await
            .unwrap()
    );
    for line in [
        "Researcher: selected 10 studies (8 RCTs)",
        "Writer: writing the answer from 10 studies (10 with an abstract)",
    ] {
        add_step(pool, run, StepKind::Write, line).await.unwrap();
    }
    run
}

async fn store_early(pool: &PgPool, run: Uuid, e: &EarlyAnswer) {
    assert!(
        drafts::upsert(pool, run, Part::Early, &serde_json::to_string(e).unwrap())
            .await
            .unwrap()
    );
}

/// A half-written answer draft (the four parts' merged draft).
fn draft_text() -> String {
    r#"{"bottom_line":{"text":"Zebra draft words so far and usually takes"#.to_string()
}

/// The early sections are drawn by the final page's own components (same
/// markup, same citation numbers, same marks), the rest as quiet
/// placeholders in the mockup's order; no draft banner; the results card
/// folded as with a draft.
#[tokio::test]
async fn the_early_sections_render_with_the_final_components() {
    let Some(pool) = pool().await else { return };
    // The early answer and the final answer of one real run.
    let (early, final_body) = a_real_four_part_run(&pool).await;
    let (uid, email) = new_user(&pool).await;
    let run = writing(&pool, uid).await;
    assert!(
        drafts::upsert(&pool, run, Part::Early, &early)
            .await
            .unwrap()
    );
    // A draft too: not shown while the early sections are.
    assert!(
        drafts::upsert(&pool, run, Part::Answer, &draft_text())
            .await
            .unwrap()
    );
    let st = ui_state(pool.clone(), true, true);
    let (s, html) = page(&st, run, &email).await;
    assert_eq!(s, StatusCode::OK);
    let m = markup(&html);
    assert!(!m.contains(BANNER), "no draft banner");
    assert!(!m.contains("Zebra draft words so far"), "no draft");
    assert!(m.contains(r#"class="dr-fold""#), "the results card folded");
    // The finished page of the same run's final answer.
    let (uid2, email2) = new_user(&pool).await;
    let fin = create_run(&pool, uid2, QUESTION, RunMode::Quick, 4000)
        .await
        .unwrap()
        .unwrap();
    assert!(
        set_status(&pool, fin, RunStatus::Running, None)
            .await
            .unwrap()
    );
    assert!(save_answer(&pool, fin, &final_body).await.unwrap());
    assert!(set_status(&pool, fin, RunStatus::Done, None).await.unwrap());
    let f = markup(&page(&st, fin, &email2).await.1);
    // Same markup, whole, for the bottom line (certainty pill), clinical
    // practice and applicability / what would change: text, citation chips
    // (the same reference numbers) and "not traced" marks.
    for (start, end) in [
        (
            r#"<section aria-label="Bottom line" class="ap-card">"#,
            "</section>",
        ),
        (
            r#"<section aria-label="What this means for clinical practice" class="ap-card">"#,
            "</section>",
        ),
        (
            r#"<details aria-label="Applicability" class="ap-sec ap-fold">"#,
            "</details>",
        ),
        (
            r#"<details aria-label="What would change this answer" class="ap-sec ap-fold">"#,
            "</details>",
        ),
    ] {
        assert_eq!(piece(&m, start, end), piece(&f, start, end), "{start}");
    }
    let bl = piece(
        &m,
        r#"aria-label="Bottom line" class="ap-card">"#,
        "</section>",
    );
    assert!(bl.contains("Certainty: "));
    assert!(bl.contains(r##"href="#ref-"##), "citation chips");
    assert!(m.contains("data-mark=\"w:"), "a not-traced mark");
    // The dark At a glance card: the whole card the same, but for the
    // tiles' source names.
    let g_start = r#"<section aria-label="At a glance" class="ap-card ap-glance">"#;
    let (eg, fg) = (
        piece(&m, g_start, "</section>"),
        piece(&f, g_start, "</section>"),
    );
    assert!(eg.contains(r##"href="#ref-"##), "the card cites");
    assert_eq!(without_tile_names(eg), without_tile_names(fg));
    // Placeholders, in the mockup's order, after the finished sections.
    let order = [
        g_start,
        r#"aria-label="Bottom line""#,
        r#"aria-label="What this means for clinical practice""#,
        r#"<section aria-label="Outcome by outcome" class="ap-wait">"#,
        "After outcomes",
        r#"<section aria-label="The trials · Conflicting evidence" class="ap-wait">"#,
        r#"aria-label="Applicability""#,
        r#"aria-label="What would change this answer""#,
    ];
    let mut last = 0;
    for t in order {
        let i = last + at(&m[last..], t);
        last = i;
    }
    assert!(m.contains("Out of 100 people · Every estimate with 95% CI"));
    assert_eq!(m.matches(">Being written<").count(), 2);
    // Nothing of the parts still being written: no outcome cards, no trial
    // cards, no Copy as text.
    assert!(!m.contains("Copy as text"));
    assert!(!f.contains("ap-wait"));
    // Escaped text only: the hydration payload carries the early JSON.
    assert!(early_key(&html));
}

/// The context part alone (the summary still being written): its sections,
/// and a placeholder for the summary's.
#[tokio::test]
async fn the_context_alone_shows_with_a_placeholder_for_the_summary() {
    let Some(pool) = pool().await else { return };
    let (uid, email) = new_user(&pool).await;
    let run = writing(&pool, uid).await;
    store_early(&pool, run, &early_via_body(false, true)).await;
    let st = ui_state(pool.clone(), true, true);
    let m = markup(&page(&st, run, &email).await.1);
    assert!(m.contains(r#"aria-label="At a glance · Bottom line · Clinical practice""#));
    assert!(!m.contains("ap-card ap-glance"));
    assert!(!m.contains("Certainty: "));
    assert!(m.contains(r#"aria-label="Applicability""#));
    assert_eq!(m.matches(">Being written<").count(), 3);
}

/// D1 A: once the one-piece line is on the run, the early sections are not
/// drawn (even while the row is still there) and the page says it is being
/// rewritten; with the one piece's draft, the line sits over the banner.
#[tokio::test]
async fn a_one_piece_rewrite_takes_the_early_sections_away_with_one_line() {
    let Some(pool) = pool().await else { return };
    let (uid, email) = new_user(&pool).await;
    let run = writing(&pool, uid).await;
    store_early(&pool, run, &early_via_body(true, true)).await;
    add_step(
        &pool,
        run,
        StepKind::Write,
        &incisive_app::run_tracker::one_piece_line("outcomes"),
    )
    .await
    .unwrap();
    let st = ui_state(pool.clone(), true, true);
    let m = markup(&page(&st, run, &email).await.1);
    assert!(m.contains(REWRITING));
    assert!(!m.contains("ap-card ap-glance") && !m.contains("class=\"ap-wait\""));
    assert!(!m.contains(BANNER));
    // The early row deleted, the one piece's words arrive: the line over the
    // usual draft.
    drafts::clear(&pool, run, Part::Early).await;
    assert!(
        drafts::upsert(&pool, run, Part::Answer, &draft_text())
            .await
            .unwrap()
    );
    let m = markup(&page(&st, run, &email).await.1);
    assert!(at(&m, REWRITING) < at(&m, BANNER));
    assert!(m.contains("Zebra draft words so far"));
    assert_eq!(m.matches(REWRITING).count(), 1);
}

/// The switch off, a one-piece run, an old run: the page is today's. With no
/// early row the switch changes nothing; with the switch off an early row is
/// never read; the wire has no `early`.
#[tokio::test]
async fn without_an_early_answer_the_page_is_todays() {
    let Some(pool) = pool().await else { return };
    let (uid, email) = new_user(&pool).await;
    let off = ui_state(pool.clone(), true, false);
    let on = ui_state(pool.clone(), true, true);
    // Running, before and with a draft.
    let run = writing(&pool, uid).await;
    for with_draft in [false, true] {
        if with_draft {
            assert!(
                drafts::upsert(&pool, run, Part::Answer, &draft_text())
                    .await
                    .unwrap()
            );
        }
        let a = page(&off, run, &email).await.1;
        let b = page(&on, run, &email).await.1;
        assert!(
            strip_clock(&markup(&a)) == strip_clock(&markup(&b)),
            "draft {with_draft}"
        );
        assert!(!early_key(&b));
        assert!(!markup(&b).contains("ap-wait"));
    }
    // An early row with the switch off: not read.
    let before = strip_clock(&markup(&page(&off, run, &email).await.1));
    store_early(&pool, run, &early_via_body(true, true)).await;
    let after = page(&off, run, &email).await.1;
    assert!(strip_clock(&markup(&after)) == before);
    assert!(!early_key(&after));
    let user = incisive_app::identity::find_user(&pool, &email)
        .await
        .unwrap()
        .unwrap();
    let wire = serde_json::to_value(api::get_view(&off, &user, run).await.unwrap()).unwrap();
    assert!(wire.get("early").is_none());
    // On: given to the owner, with no draft beside it.
    let v = api::get_view(&on, &user, run).await.unwrap();
    assert!(v.early.is_some() && v.draft.is_none());
    // Finished: the final page, whatever the switch and any row left.
    assert!(save_answer(&pool, run, &fixture()).await.unwrap());
    assert!(
        api::get_view(&on, &user, run)
            .await
            .unwrap()
            .early
            .is_none()
    );
    assert!(set_status(&pool, run, RunStatus::Done, None).await.unwrap());
    let a = markup(&page(&off, run, &email).await.1);
    let b = markup(&page(&on, run, &email).await.1);
    assert!(a == b, "finished: the same page");
    assert!(b.contains("Copy as text") && !b.contains("ap-wait"));
}

/// Another user sees nothing of it.
#[tokio::test]
async fn another_user_gets_no_early_answer() {
    let Some(pool) = pool().await else { return };
    let (uid, _) = new_user(&pool).await;
    let (_, other) = new_user(&pool).await;
    let run = writing(&pool, uid).await;
    store_early(&pool, run, &early_via_body(true, true)).await;
    let st = ui_state(pool.clone(), true, true);
    let html = page(&st, run, &other).await.1;
    assert!(!html.contains("ap-card ap-glance") && !early_key(&html));
}

/// Owner (21:27): the run's top cards are capped at the answer's width, so
/// their right edges line up on desktop; the phone layout is untouched (the
/// cap only binds above 760 px).
#[tokio::test]
async fn the_top_cards_share_the_answers_width() {
    let Some(pool) = pool().await else { return };
    let (uid, email) = new_user(&pool).await;
    let run = writing(&pool, uid).await;
    let st = ui_state(pool.clone(), true, true);
    let html = page(&st, run, &email).await.1;
    let rule = r#".run-chrome .rk-card, .run-chrome .rx-seg, .run-chrome details[class$="-fold"] { max-width:760px; }"#;
    assert!(html.contains(rule), "the cap");
    assert!(
        html.contains(".ap-main { max-width:760px;"),
        "the answer's width"
    );
    // Not inside a phone-only block: the rule comes before the tracker's
    // first @media.
    let style = &html[at(&html, ".rk-card { margin:0 0 10px;")..];
    assert!(at(style, rule) < at(style, "@media (max-width:640px)"));
}

/// The page knows every part's one-piece line (its names are the engine's).
#[test]
fn every_parts_one_piece_line_reads_as_rewriting() {
    for s in Section::ALL {
        let step = incisive_app::runs::StepLine {
            seq: 1,
            at: chrono::Utc::now(),
            kind: StepKind::Write,
            line: incisive_app::run_tracker::one_piece_line(s.name()),
        };
        assert!(incisive_app::draft_page::rewriting(&[step]), "{s:?}");
    }
}

// ---------- S1e: the answer page's layout (`ANSWER_LAYOUT=on`) ----------

fn layout_state(pool: PgPool, layout: bool) -> AppState {
    let mut st = ui_state(pool, true, true);
    st.runs.answer_layout = layout;
    st
}

/// The hydration payload names the layout (plain or escaped JSON).
fn layout_key(html: &str) -> bool {
    html.contains("\"answer_layout\"") || html.contains("\\\"answer_layout\\\"")
}

/// A finished Quick run whose answer is `body`.
async fn finished(pool: &PgPool, uid: i64, body: &Value) -> Uuid {
    let run = create_run(pool, uid, QUESTION, RunMode::Quick, 4000)
        .await
        .unwrap()
        .unwrap();
    assert!(
        set_status(pool, run, RunStatus::Running, None)
            .await
            .unwrap()
    );
    for line in [
        "Researcher: selected 10 studies (8 RCTs)",
        "Writer: writing the answer from 10 studies (10 with an abstract)",
    ] {
        add_step(pool, run, StepKind::Write, line).await.unwrap();
    }
    assert!(save_answer(pool, run, body).await.unwrap());
    assert!(set_status(pool, run, RunStatus::Done, None).await.unwrap());
    run
}

/// S1e: the switch reaches the page on the run it draws (the server
/// function sets it; the hydration payload carries it only when on), and
/// the finished page is drawn in the new layout; with it off the page, the
/// payload and the JSON route are today's. With `ANSWER_LAYOUT_QUICK_HTML_OUT`
/// set, the served page with the switch on is written there (measuring).
#[tokio::test]
async fn the_switch_reaches_the_finished_page_and_off_is_todays() {
    let Some(pool) = pool().await else { return };
    let (_, final_body) = a_real_four_part_run(&pool).await;
    let (uid, email) = new_user(&pool).await;
    let run = finished(&pool, uid, &final_body).await;
    let off = layout_state(pool.clone(), false);
    let on = layout_state(pool.clone(), true);
    let (s, html_off) = page(&off, run, &email).await;
    assert_eq!(s, StatusCode::OK);
    let (_, html_on) = page(&on, run, &email).await;
    if let Ok(out) = std::env::var("ANSWER_LAYOUT_QUICK_HTML_OUT") {
        std::fs::write(out, &html_on).unwrap();
    }
    assert!(!layout_key(&html_off) && !html_off.contains("ap-lx"));
    assert!(layout_key(&html_on));
    let m = markup(&html_on);
    assert!(m.contains(r#"<div class="ap ap-lx">"#));
    assert!(at(&m, "Why this conclusion") < at(&m, r#"<div class="ap-details">"#));
    assert!(
        at(&m, r#"aria-label="What this means for clinical practice""#)
            < at(&m, "Why this conclusion")
    );
    // The answer page in the layout is the one `render_answer_v3_layout`
    // draws (the unit of the replay tests).
    let a: AnswerV3 = serde_json::from_value(final_body.clone()).unwrap();
    let lone = incisive_app::app::render_answer_v3_layout(a.clone());
    let s = |h: &str| h.replace("<!>", "");
    assert!(
        s(&m).contains(&s(&lone[lone.find("<div class=\"ap ap-lx\">").unwrap()
            ..lone.find("<details class=\"ap-original\"").unwrap()]))
    );
    if let Ok(out) = std::env::var("ANSWER_LAYOUT_QUICK_LONE_OUT") {
        std::fs::write(out, &lone).unwrap();
    }
    // Off: the JSON route never carries it; the off page is the page
    // `render_answer_v3` draws.
    let user = incisive_app::identity::find_user(&pool, &email)
        .await
        .unwrap()
        .unwrap();
    for st in [&off, &on] {
        let wire = serde_json::to_value(api::get_view(st, &user, run).await.unwrap()).unwrap();
        assert!(wire.get("answer_layout").is_none());
    }
    let today = incisive_app::app::render_answer_v3(a);
    let mo = markup(&html_off);
    assert!(s(&mo).contains(&s(
        &today[..today.find("<details class=\"ap-original\"").unwrap()]
    )));
}

/// S1e: while a four-part Quick answer is written, the placeholders follow
/// the new order: the summary's sections, "Why this conclusion" with the CI
/// and outcomes placeholders side by side ("Being written"), "Counselling
/// dashboard" with Out of 100 and Compare the arms ("After outcomes"), then
/// the Details rows; the finished sections' markup as with the switch off.
#[tokio::test]
async fn the_early_page_follows_the_new_order() {
    let Some(pool) = pool().await else { return };
    let (uid, email) = new_user(&pool).await;
    let run = writing(&pool, uid).await;
    store_early(&pool, run, &early_via_body(true, true)).await;
    let off = markup(
        &page(&layout_state(pool.clone(), false), run, &email)
            .await
            .1,
    );
    let html = page(&layout_state(pool.clone(), true), run, &email).await.1;
    if let Ok(out) = std::env::var("ANSWER_LAYOUT_EARLY_HTML_OUT") {
        std::fs::write(out, &html).unwrap();
    }
    let m = markup(&html);
    assert!(!off.contains("ap-lx"));
    assert!(m.contains(r#"<div class="ap-main ap-early">"#) && m.contains("ap ap-lx"));
    let order = [
        r#"<section aria-label="At a glance" class="ap-card ap-glance">"#,
        r#"aria-label="Bottom line""#,
        r#"aria-label="What this means for clinical practice""#,
        r#"<h2 class="ap-lx-h">Why this conclusion</h2>"#,
        r#"<div class="ap-lx-pair">"#,
        r#"<section aria-label="Every estimate with a 95% CI" class="ap-wait">"#,
        r#"<section aria-label="Outcome by outcome" class="ap-wait">"#,
        r#"<h2 class="ap-lx-h">Counselling dashboard</h2>"#,
        r#"<div class="ap-lx-pair">"#,
        r#"<section aria-label="Out of 100 patients" class="ap-wait">"#,
        r#"<section aria-label="Compare the arms" class="ap-wait">"#,
        r#"<div class="ap-details">"#,
        r#"<h3 class="ap-details-title">Details</h3>"#,
        r#"<section aria-label="The trials · Conflicting evidence" class="ap-wait">"#,
        r#"aria-label="Applicability""#,
        r#"aria-label="What would change this answer""#,
    ];
    let mut last = 0;
    for t in order {
        last += at(&m[last..], t) + t.len();
    }
    // The pairs: CI and outcomes in one, Out of 100 and Compare in the other.
    let why = piece(
        &m,
        r#"<h2 class="ap-lx-h">Why this conclusion</h2>"#,
        "Counselling dashboard",
    );
    assert_eq!(why.matches(">Being written<").count(), 2);
    let dash = piece(&m, "Counselling dashboard", r#"<div class="ap-details">"#);
    assert_eq!(dash.matches(">After outcomes<").count(), 2);
    assert!(
        !m.contains(r#"class="ap-lx-more""#) && !m.contains("ap-lx-pair fold"),
        "no fold while written"
    );
    for (start, end) in [
        (
            r#"<section aria-label="Bottom line" class="ap-card">"#,
            "</section>",
        ),
        (
            r#"<section aria-label="What this means for clinical practice" class="ap-card">"#,
            "</section>",
        ),
        (
            r#"<details aria-label="Applicability" class="ap-sec ap-fold">"#,
            "</details>",
        ),
        (
            r#"<details aria-label="What would change this answer" class="ap-sec ap-fold">"#,
            "</details>",
        ),
    ] {
        assert_eq!(piece(&m, start, end), piece(&off, start, end), "{start}");
    }
    // The summary still being written: its placeholder first, the context
    // under the Details heading.
    let (uid2, email2) = new_user(&pool).await;
    let run2 = writing(&pool, uid2).await;
    store_early(&pool, run2, &early_via_body(false, true)).await;
    let m2 = markup(
        &page(&layout_state(pool.clone(), true), run2, &email2)
            .await
            .1,
    );
    assert!(
        at(
            &m2,
            r#"aria-label="At a glance · Bottom line · Clinical practice""#
        ) < at(&m2, "Why this conclusion")
    );
    assert!(
        at(&m2, r#"<h3 class="ap-details-title">Details</h3>"#)
            < at(&m2, r#"aria-label="Applicability""#)
    );
}

/// S1e: the deploy workflow passes `ANSWER_LAYOUT` to both steps, writes it
/// only when set, and refuses anything but on or off (ends trimmed); the
/// infra README documents it.
#[test]
fn deploy_workflow_wires_the_layout_switch() {
    let root = format!("{}/../..", env!("CARGO_MANIFEST_DIR"));
    let deploy = std::fs::read_to_string(format!("{root}/.github/workflows/deploy.yml")).unwrap();
    assert_eq!(
        deploy
            .matches("ANSWER_LAYOUT: ${{ vars.ANSWER_LAYOUT }}")
            .count(),
        2
    );
    assert!(deploy.contains(r#"optional += ["ANSWER_LAYOUT"]"#));
    let start = deploy.find("al=\"${ANSWER_LAYOUT#").unwrap();
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
            .env("ANSWER_LAYOUT", value)
            .status()
            .expect("bash");
        assert_eq!(status.success(), ok, "ANSWER_LAYOUT={value:?}");
    }
    let readme = std::fs::read_to_string(format!("{root}/infra/README.md")).unwrap();
    assert!(readme.contains("| `ANSWER_LAYOUT` | no |"));
}

/// S1e: no `inner_html` in the page's view code (escaped text only).
#[test]
fn the_layout_view_writes_no_raw_html() {
    let src =
        std::fs::read_to_string(format!("{}/src/app/page.rs", env!("CARGO_MANIFEST_DIR"))).unwrap();
    assert!(!src.contains("inner_html="));
    assert!(src.contains("fn layout_page_view") && src.contains("fn early_layout_view"));
}

/// S1e (review M5): with the switch off the early page's answer part is byte
/// for byte what main drew before S1e (787442e;
/// `tests/fixtures/s1e/main_early_ap.html`, the served page's
/// `<div class="ap">` with scripts and streaming slots read as `markup`).
#[tokio::test]
async fn the_early_page_off_is_byte_identical_to_main() {
    let Some(pool) = pool().await else { return };
    let (uid, email) = new_user(&pool).await;
    let run = writing(&pool, uid).await;
    store_early(&pool, run, &early_via_body(true, true)).await;
    let m = markup(
        &page(&layout_state(pool.clone(), false), run, &email)
            .await
            .1,
    );
    let start = at(&m, r#"<div class="ap"><div class="ap-main ap-early">"#);
    let mut depth = 0usize;
    let mut end = start;
    let tags = regex::Regex::new(r"<div\b|</div>").unwrap();
    for t in tags.find_iter(&m[start..]) {
        if t.as_str() == "</div>" {
            depth -= 1;
            if depth == 0 {
                end = start + t.end();
                break;
            }
        } else {
            depth += 1;
        }
    }
    let golden = include_str!("fixtures/s1e/main_early_ap.html");
    assert!(&m[start..end] == golden, "differs from main");
}
