//! Stage 4 item 11a: the regression replay. The real payloads of the 29 Oct
//! prototype run of the hernia question (`tests/fixtures/stage4_replay/`:
//! the researcher's studies, the extractor's evidence rows and the
//! skeptic's review) go through the new Full writer-input assembly, a
//! scripted writer (the real writer call, `ScriptedLlm` replying), the
//! post-writer checks (citations, the number trace, code-built notable
//! findings) and the page rendering. The test shows that nothing is lost:
//! every study, outcome, number, reference and caveat that reached the
//! writer and the page before Stage 4 still does, and the new sections
//! appear.
//!
//! With `REPLAY_OUT=<path>` the counts are also written as JSON (the stage
//! report's before/after table compares them with the same replay run on
//! the Stage 3 code).

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use claude_client::Usage;
use incisive_app::agent::answer_v3::{
    ADDED_LABEL, ANSWER_VERSION, AnswerV3, IntegrityV3, ReferenceV3, SearchEntry,
};
use incisive_app::agent::extractor::{
    self, EvidenceRow, Extraction, MAX_OUTCOMES, ReportedRow, SourceText, source_text,
};
use incisive_app::agent::llm::ScriptedLlm;
use incisive_app::agent::llm::wrap_source;
use incisive_app::agent::researcher::Contradiction;
use incisive_app::agent::skeptic::SkepticReview;
use incisive_app::agent::skeptic::{self, SkepticReport};
use incisive_app::agent::writer_v3::{self, FullInput, StudyInput};
use incisive_app::agent::{RunContext, completeness, coverage, team};
use incisive_app::db::{lazy_pool, migrate};
use incisive_app::gate::OfflineClassifier;
use incisive_app::identity::{IapVerifier, JwkSource};
use incisive_app::lit::europepmc::EuropePmc;
use incisive_app::lit::europepmc::FullText;
use incisive_app::lit::http::{AllowlistedClient, OaPolicy};
use incisive_app::lit::pubmed::PubMed;
use incisive_app::runs::budget::RunBudget;
use incisive_app::runs::store::create_run;
use incisive_app::runs::worker::{CancelFlag, WorkerHandle};
use incisive_app::runs::{DepthSettings, Engine, RunMode, RunStatus, RunsConfig};
use incisive_app::server::AppState;
use incisive_app::spend::{CapCheck, CapsConfig, Prices, reserve};
use secrecy::SecretString;
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

const QUESTION: &str = "Does robotic inguinal hernia repair improve pain, complications, recurrence, return to normal activity, or quality of life compared with conventional laparoscopic repair? Assess randomized evidence, operative time, and costs";

fn fixture(name: &str) -> Value {
    let p = format!(
        "{}/tests/fixtures/stage4_replay/{name}",
        env!("CARGO_MANIFEST_DIR")
    );
    serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap()
}

fn s(v: &Value) -> String {
    v.as_str().unwrap_or_default().to_string()
}

/// Number tokens of `text`, as the page and prompts write them (digits,
/// optional decimals).
fn number_tokens(text: &str) -> Vec<String> {
    let re = regex::Regex::new(r"\d+(?:\.\d+)?").unwrap();
    re.find_iter(text).map(|m| m.as_str().to_string()).collect()
}

fn first_integer(text: &str) -> Option<i64> {
    regex::Regex::new(r"\d+")
        .unwrap()
        .find(text)
        .and_then(|m| m.as_str().parse().ok())
}

/// The researcher's selection, as the writer sees it.
fn studies() -> (Vec<StudyInput>, Vec<Contradiction>, Vec<SearchEntry>) {
    let f = fixture("studies.json");
    let studies = f["selected"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| StudyInput {
            pmid: s(&x["pmid"]),
            title: s(&x["title"]),
            journal: x["journal"].as_str().map(String::from),
            year: x["year"].as_i64().map(|y| y as i32),
            pub_types: vec![],
            doi: x["doi"].as_str().map(String::from),
            retracted: false,
            abstract_text: None,
            design: s(&x["design"]),
            why: s(&x["why"]),
        })
        .collect();
    let contradictions = f["possible_contradictions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| Contradiction {
            pmid: s(&c["pmid"]),
            note: s(&c["note"]),
        })
        .collect();
    let searches = f["searches"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| SearchEntry {
            query: s(&x["query"]),
            source: "PubMed".into(),
            hits: x["hits"].as_u64().unwrap_or(0),
        })
        .collect();
    (studies, contradictions, searches)
}

/// The prototype's evidence rows, as verified rows (the prototype traced
/// all of its numbers to verbatim quotes). Its outcomes carry numbers only
/// in text; each number of an outcome's effect that its quote also has is
/// given as a structured number. Field names of both the Stage 3 and Stage
/// 4 rows are given (serde ignores the other), so the same replay runs on
/// both.
fn rows_of(f: &Value) -> Vec<Value> {
    f["studies"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| {
            let outcomes: Vec<Value> = x["outcomes"]
                .as_array()
                .unwrap()
                .iter()
                .map(|o| {
                    let quote = s(&o["quote"]);
                    let q = number_tokens(&quote);
                    let numbers: Vec<Value> = number_tokens(&s(&o["effect"]))
                        .into_iter()
                        .filter(|t| q.contains(t))
                        .take(6)
                        .map(|t| {
                            json!({"label": s(&o["outcome"]), "value": t.parse::<f64>().unwrap(),
                                "unit": null, "arm": null, "ci_low": null, "ci_high": null, "p": null})
                        })
                        .collect();
                    json!({
                        "outcome": o["outcome"], "effect_text": o["effect"], "numbers": numbers,
                        "quote": quote,
                        "source": if o["source"] == "abstract" { "abstract" } else { "full_text" },
                    })
                })
                .collect();
            let full = outcomes.iter().any(|o| o["source"] == "full_text");
            let a = &x["appraisal"];
            json!({
                "pmid": x["pmid"], "text": if full { "full_text" } else { "abstract" },
                "design": x["design"], "n_randomised": first_integer(&s(&x["n"])), "n_analysed": null,
                "setting": x["setting"], "centres": "unclear", "country": "",
                "population": x["population"], "intervention": x["intervention"],
                "comparator": x["comparator"], "follow_up": x["follow_up"], "blinding": x["blinding"],
                "primary_endpoint": x["primary_endpoint"],
                "powered_for_primary": x["powered_for_primary"].as_bool(),
                "surgeon_experience": "", "outcomes": outcomes,
                "appraisal": {"bias": a["bias"], "power": a["power"],
                    "learning_curve": a["learning_curve"], "applicability": a["applicability"]},
                "independence": {"trial_registration": null, "group_or_centre": x["setting"], "note": ""},
                "unverified_outcomes": [], "outcomes_verified": x["outcomes"].as_array().unwrap().len(),
                "numbers_dropped": 0, "numbers_unverified": 0, "texts_cleaned": 0, "corrections": 0
            })
        })
        .collect()
}

/// The prototype skeptic's review (its verified contradictions and caveats).
fn review() -> Value {
    let f = fixture("skeptic.json");
    let contradicting: Vec<Value> = f["contradicting"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| {
            json!({"pmid": c["pmid"], "claim": c["claim"], "quote": c["quote"],
                "strength_vs_rcts": c["strength_vs_rcts"]})
        })
        .collect();
    json!({
        "verdict": "holds_with_caveats", "must_mention": f["must_mention"],
        "contradicting": contradicting, "missed_rcts": [], "guidelines": [], "overlaps": [],
        "searches": [], "dropped_unseen": 0, "dropped_quotes": 0
    })
}

/// The scripted writer's reply: what a writer could write from this
/// evidence, every outcome of every row it was shown, every study, the
/// skeptic's caveats as conflicts, and the Stage 4 fields.
fn scripted_answer(rows: &[EvidenceRow], studies: &[StudyInput], skeptic: &SkepticReview) -> Value {
    let mut outcomes = Vec::new();
    for r in rows {
        for o in r.outcomes.iter().take(MAX_OUTCOMES) {
            let numbers: Vec<Value> = o
                .numbers
                .iter()
                .enumerate()
                .map(|(i, n)| {
                    json!({"label": o.outcome, "value": n.value, "unit": null,
                        "arm": if i % 2 == 0 { "robotic" } else { "laparoscopic" },
                        "ci_low": null, "ci_high": null, "p": null, "pmid": r.pmid})
                })
                .collect();
            outcomes.push(json!({
                "outcome": o.outcome, "rct_evidence": "RCT", "effect_text": format!("{} [{}]", o.effect_text, r.pmid),
                "numbers": numbers, "certainty": "Low", "why": "small trials", "pmids": [r.pmid]
            }));
        }
    }
    let trial_rows: Vec<Value> = studies
        .iter()
        .map(|s| {
            json!({"pmid": s.pmid, "label": format!("Trial {} ({})", s.pmid, s.year.unwrap_or(0)),
                "design": s.design, "n": null, "setting": "as reported",
                "comparison": "Robotic TAPP vs laparoscopic", "key_results": format!("{} [{}]", s.why, s.pmid),
                "appraisal": "see the row"})
        })
        .collect();
    let conflicts: Vec<Value> = skeptic
        .contradicting
        .iter()
        .map(|c| json!({"text": format!("{} [{}]", c.claim, c.pmid), "pmids": [c.pmid]}))
        .chain(
            skeptic
                .must_mention
                .iter()
                .map(|m| json!({"text": m, "pmids": []})),
        )
        .collect();
    let a = &studies[0].pmid;
    json!({
        "bottom_line": {"text": format!("No consistent patient benefit; longer and costlier [{a}]."),
            "certainty": "Low", "certainty_reason": "small, heterogeneous trials"},
        "outcomes": outcomes, "studies": trial_rows, "conflicts": conflicts,
        "applicability": {"case_mix": "primary unilateral", "india_cost_context": "not known",
            "training": "a reasonable index case"},
        "would_change": ["a large multicentre RCT with long follow-up"],
        // R-S4a: three plain strings (no writer guidelines, no evidence
        // type: the page takes those from the skeptic and the designs).
        "at_a_glance": format!("No consistent patient benefit; costs more.\nCost: $3258 robotic vs $1421 laparoscopic [{a}]\nMain caveat: Small trials with short follow-up."),
        "clinical_practice": "Default choice: Laparoscopic repair.\nWho may benefit: Not shown.\nCounselling: Recovery is similar.\nCost per case: not known in India; $3258 vs $1421 in RIVAL",
        "notable_findings": format!("DIRECT reports surgeon experience inconsistently [{a}].")
    })
}

/// Each prototype study's source text, rebuilt from its verbatim quotes:
/// the abstract quotes as the record's abstract, the full-text quotes as a
/// full-text results section (the prototype read both).
fn source_of(x: &Value) -> SourceText {
    let mut abs: Vec<String> = Vec::new();
    let mut full: Vec<String> = Vec::new();
    for o in x["outcomes"].as_array().unwrap() {
        let q = s(&o["quote"]);
        let into = if o["source"] == "abstract" {
            &mut abs
        } else {
            &mut full
        };
        if !into.contains(&q) {
            into.push(q);
        }
    }
    let paper = incisive_app::lit::Paper {
        pmid: Some(s(&x["pmid"])),
        pmcid: None,
        doi: None,
        title: s(&x["short_name"]),
        authors: vec![],
        journal: None,
        year: None,
        pub_types: vec![],
        abstract_text: (!abs.is_empty()).then(|| abs.join(" ")),
        oa_url: None,
        oa_is_pdf: false,
        retracted: false,
        source: incisive_app::lit::PaperSource::PubMed,
    };
    let ft = (!full.is_empty()).then(|| FullText {
        sections: vec![("results".into(), full.join(" "))],
    });
    source_text(&paper, ft.as_ref()).expect("a source")
}

/// The prototype's rows pushed through the extractor's own checks
/// (`first_pass` and `finish_row`) against their rebuilt source texts.
fn checked_rows() -> Vec<EvidenceRow> {
    checked_rows_of(&fixture("evidence.json"))
}

/// [`checked_rows`] of an evidence payload.
fn checked_rows_of(f: &Value) -> Vec<EvidenceRow> {
    f["studies"]
        .as_array()
        .unwrap()
        .iter()
        .zip(rows_of(f))
        .map(|(x, mut v)| {
            let o = v.as_object_mut().unwrap();
            for k in [
                "pmid",
                "text",
                "unverified_outcomes",
                "outcomes_verified",
                "numbers_dropped",
                "numbers_unverified",
                "texts_cleaned",
                "corrections",
            ] {
                o.remove(k);
            }
            o.insert("funding".into(), Value::Null);
            let reported: ReportedRow = serde_json::from_value(v).unwrap();
            extractor::check_reply(&s(&x["pmid"]), &source_of(x), &reported, None)
        })
        .collect()
}

/// The prototype skeptic's report pushed through `skeptic::validate`, its
/// tool results being each quoted study's record (title and abstract) as
/// the app wraps them.
fn validated_review(selected: &[String]) -> SkepticReview {
    let f = fixture("skeptic.json");
    let papers: Vec<Value> = f["contradicting"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| json!({"pmid": c["pmid"], "title": "A study (replay)", "abstract": c["quote"]}))
        .collect();
    let results = vec![wrap_source(
        "t1-1",
        &json!({"papers": papers, "not_found": []}).to_string(),
    )];
    let mut report = review();
    report["missed_rcts"] = json!([]);
    let report: SkepticReport = serde_json::from_value(report).unwrap();
    skeptic::validate(report, &results, selected, vec![])
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

fn prices() -> Prices {
    Prices::from_json(r#"{"m-test": {"input": 1.0, "output": 5.0}}"#).unwrap()
}

/// A running Full run for the writer's call (no literature calls are made).
async fn run_ctx(pool: &PgPool, llm: Arc<ScriptedLlm>) -> RunContext {
    let oa = OaPolicy::default();
    let client = AllowlistedClient::new("Incisive/test", oa.clone())
        .with_base_override("http://127.0.0.1:9")
        .with_retry_backoff(Duration::from_millis(1));
    let lit = Arc::new(incisive_app::agent::Literature {
        pubmed: PubMed::new(client.clone(), None).with_min_interval(Duration::ZERO),
        epmc: EuropePmc::new(client, oa),
    });
    let email = format!("replay-{}@example.com", Uuid::new_v4());
    sqlx::query(
        "INSERT INTO incisive.users (email, display_name, role, daily_cap_paise) VALUES ($1, 'T', 'surgeon', 1000000)",
    )
    .bind(&email)
    .execute(pool)
    .await
    .unwrap();
    let user = incisive_app::identity::find_user(pool, &email)
        .await
        .unwrap()
        .unwrap();
    let limit = 30_000;
    let run = create_run(pool, user.id, QUESTION, RunMode::Focused, limit)
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
    let settings = DepthSettings::FOCUSED;
    let user_id = user.id;
    RunContext {
        state: AppState {
            pool: pool.clone(),
            verifier: Arc::new(IapVerifier::new(
                "aud".into(),
                JwkSource::Static(serde_json::from_str(r#"{"keys":[]}"#).unwrap()),
            )),
            gate: Arc::new(OfflineClassifier),
            prices: prices(),
            usd_inr: 88.0,
            caps,
            gate_model: "m-test".into(),
            dev_mode: true,
            runs: RunsConfig::default(),
            quick_engine: Engine::Team,
            full_engine: Engine::Team,
            completeness: false,
            worker: WorkerHandle::default(),
            llm: None,
            lit: lit.clone(),
        },
        llm: incisive_app::agent::testing::agent_llm(llm, "m-test"),
        lit,
        run_id: run,
        user,
        question: QUESTION.to_string(),
        mode: RunMode::Focused,
        settings,
        budget: Arc::new(
            RunBudget::new(run, user_id, limit, Instant::now(), prices(), 88.0)
                .with_time_limit(settings.agent_time_limit_s),
        ),
        cancel: CancelFlag::default(),
    }
}

/// The page's drawn text (scripts and tags removed, entities decoded).
fn page_text(html: &str) -> String {
    let mut out = String::new();
    let mut rest = html;
    while let Some(i) = rest.find("<script") {
        out.push_str(&rest[..i]);
        rest = rest[i..]
            .find("</script>")
            .map_or("", |j| &rest[i + j + "</script>".len()..]);
    }
    out.push_str(rest);
    // The "not traced" / "not verified" marks are not content: compare
    // the text they follow.
    let marks = regex::Regex::new(r#"<sup[^>]*class="flag"[^>]*>[^<]*</sup>"#).unwrap();
    let out = marks.replace_all(&out, "").to_string();
    let markers = out.replace("<!>", "");
    let out = markers;
    let tags = regex::Regex::new(r"<[^>]*>").unwrap();
    tags.replace_all(&out, " ")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#x27;", "'")
        .replace("&amp;", "&")
}

fn flat(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn count_tokens(text: &str) -> BTreeMap<String, usize> {
    let mut m = BTreeMap::new();
    for t in number_tokens(text) {
        *m.entry(t).or_default() += 1;
    }
    m
}

#[tokio::test]
async fn the_prototype_payloads_replay_with_nothing_lost() {
    let Some(pool) = pool().await else { return };
    let (studies, contradictions, searches) = studies();
    let selected: Vec<String> = studies.iter().map(|s| s.pmid.clone()).collect();
    // Review 1: through the paths Stage 4 changed (the extractor's checks
    // and the skeptic's validation), not around them.
    let rows = checked_rows();
    let review = validated_review(&selected);
    assert_eq!((studies.len(), rows.len()), (10, 10));
    // The Stage 3 baseline (51 outcomes, 174 structured numbers, 6
    // contradictions) is kept: nothing is dropped, everything verified.
    let outcomes: usize = rows.iter().map(|r| r.outcomes.len()).sum();
    let numbers: usize = rows
        .iter()
        .flat_map(|r| &r.outcomes)
        .map(|o| o.numbers.len())
        .sum();
    assert!(outcomes >= 51, "{outcomes} outcomes");
    assert!(numbers >= 174, "{numbers} structured numbers");
    assert!(
        rows.iter().flat_map(|r| &r.outcomes).all(|o| o.verified),
        "every quote found"
    );
    assert!(
        review.contradicting.len() >= 6,
        "{:?}",
        review.contradicting.len()
    );
    assert!(review.leads.is_empty() && review.dropped_unseen == 0);
    // The prototype claim "… gap is 17 min" survives, its 17 marked.
    let (i, c) = review
        .contradicting
        .iter()
        .enumerate()
        .find(|(_, c)| c.claim.contains("17 min"))
        .expect("the 17 min claim is kept");
    let marked: Vec<&str> = review
        .spans
        .iter()
        .filter(|sp| sp.field == format!("contradicting.{i}.text"))
        .map(|sp| &c.claim[sp.start as usize..(sp.start + sp.len) as usize])
        .collect();
    assert_eq!(marked, ["17"], "not verified, not removed");

    // ---- the writer's input ----
    let full = FullInput {
        rows: &rows,
        skeptic: Some(&review),
        skeptic_note: None,
        missed: &[],
    };
    let user = writer_v3::writer_user_full(QUESTION, &studies, &full, &contradictions, &searches);
    let flat_user = flat(&user.replace(writer_v3::NOT_VERIFIED_MARK, ""));
    let mut expected_outcomes = 0;
    let mut expected_numbers = 0;
    for s in &studies {
        assert!(
            user.contains(&format!("PMID {}", s.pmid)),
            "{} lost",
            s.pmid
        );
    }
    for r in &rows {
        assert!(
            user.contains(&format!("<source id=\"row-{}\">", r.pmid)),
            "{}",
            r.pmid
        );
        for o in r.outcomes.iter().take(MAX_OUTCOMES) {
            expected_outcomes += 1;
            let name: String = flat(&o.outcome).chars().take(60).collect();
            assert!(flat_user.contains(&name), "outcome {name:?} lost");
            // Every number of the quote reaches the writer (the quote is
            // shown up to its cap).
            for n in &o.numbers {
                expected_numbers += 1;
                let shown = flat_user.contains(&format!("= {}", fmt(n.value)));
                assert!(shown, "number {} of {} lost", n.value, r.pmid);
            }
        }
    }
    for m in &review.must_mention {
        let head: String = flat(m).chars().take(80).collect();
        assert!(flat_user.contains(&head), "caveat lost: {head}");
    }
    for c in &review.contradicting {
        assert!(
            user.contains(&format!("- [{}] ", c.pmid)),
            "contradiction {} lost",
            c.pmid
        );
    }
    assert!(!user.contains("[?]"));
    assert!(
        user.contains("17 (not verified) min"),
        "the mark reaches the writer"
    );
    // Pinned: what the writer is given from the prototype payloads.
    assert_eq!(expected_outcomes, 51);
    assert_eq!(expected_numbers, 174);
    assert_eq!(review.must_mention.len(), 8);
    assert_eq!(review.contradicting.len(), 6);
    assert_eq!(number_tokens(&user).len(), WRITER_INPUT_NUMBER_TOKENS);

    // ---- the writer (scripted), and the post-writer checks ----
    let reply = scripted_answer(&rows, &studies, &review);
    let llm = Arc::new(
        ScriptedLlm::new()
            .when("STUDIES SELECTED BY THE RESEARCHER")
            .reply_json(
                reply,
                Usage {
                    input_tokens: 30_000,
                    output_tokens: 9_000,
                    ..Usage::default()
                },
            ),
    );
    let ctx = run_ctx(&pool, llm.clone()).await;
    let mut written = writer_v3::write_with(&ctx, &ctx.llm, writer_v3::SYSTEM_FULL, &user)
        .await
        .unwrap_or_else(|_| panic!("the scripted writer reply reads"));
    let req = &llm.requests()[0];
    assert_eq!(req.max_tokens, DepthSettings::FOCUSED.writer_max_tokens);
    let mut integrity = IntegrityV3::default();
    writer_v3::check_citations(&mut written, &selected, &mut integrity);
    // As the team does (R-S4a): the evidence labels from the designs.
    writer_v3::evidence_types(&mut written, |p| {
        studies
            .iter()
            .find(|s| s.pmid == p)
            .and_then(|s| writer_v3::design_class(&s.design))
    });
    let allowed = writer_v3::allowed_numbers(&studies, Some(&full));
    let (untraced, checked) = writer_v3::trace_numbers(
        &written,
        &allowed,
        |p| selected.iter().any(|s| s == p),
        true,
    );
    integrity.numbers_checked = checked;
    integrity.numbers_untraced = untraced.len() as u32;
    let extraction = Extraction {
        rows: rows.clone(),
        ..Extraction::default()
    };
    let f = fixture("studies.json");
    let references: Vec<ReferenceV3> = studies
        .iter()
        .enumerate()
        .map(|(i, s)| ReferenceV3 {
            n: i as u16 + 1,
            pmid: s.pmid.clone(),
            title: s.title.clone(),
            journal: s.journal.clone(),
            year: s.year,
            doi: s.doi.clone(),
            retracted: false,
            found_by_skeptic: false,
        })
        .collect();
    assert_eq!(references.len(), f["selected"].as_array().unwrap().len());
    let answer = AnswerV3 {
        version: ANSWER_VERSION,
        question: QUESTION.into(),
        mode: RunMode::Focused,
        created_at: chrono::Utc::now(),
        answer: Some(written.clone()),
        empty_reason: None,
        references: references.clone(),
        searches: searches.clone(),
        integrity,
        stopped_early: None,
        cost_paise: 13689,
        elapsed_s: 400,
        skeptic: Some(team::skeptic_summary(&review)),
        evidence: team::evidence_summary(&studies, &extraction),
        notes: vec![],
        untraced: untraced.clone(),
        notable_code: team::notable_code(Some(&review), &extraction),
        coverage: None,
        completeness: None,
    };
    // Stored and read back as the run page does.
    let stored: AnswerV3 = serde_json::from_value(serde_json::to_value(&answer).unwrap()).unwrap();
    assert_eq!(stored, answer);

    // ---- the page ----
    let html = incisive_app::app::render_answer_v3(stored);
    let page = flat(&page_text(&html));
    assert!(!page.contains("[?]"));
    for (i, s) in studies.iter().enumerate() {
        assert!(
            page.contains(&format!("Trial {}", s.pmid)),
            "trial row {} lost",
            s.pmid
        );
        assert!(
            html.contains(&format!("id=\"ref-{}\"", i + 1)),
            "reference {} lost",
            i + 1
        );
        let title: String = flat(&s.title).chars().take(50).collect();
        assert!(page.contains(&title), "reference title lost: {title}");
    }
    for o in &written.outcomes {
        let name: String = flat(&o.outcome).chars().take(60).collect();
        assert!(page.contains(&name), "outcome {name:?} lost on the page");
    }
    for m in &review.must_mention {
        let head: String = flat(m).chars().take(80).collect();
        assert!(page.contains(&head), "caveat lost on the page: {head}");
    }
    // Every number of every effect the writer wrote is on the page, as
    // often as the effects write it (review 1: >= the expected count).
    let page_numbers = count_tokens(&page);
    let mut expected: BTreeMap<String, usize> = BTreeMap::new();
    for o in &written.outcomes {
        for (t, n) in count_tokens(&o.effect_text) {
            if t.len() >= 5 && selected.contains(&t) {
                continue; // a citation, drawn as a reference number
            }
            *expected.entry(t).or_default() += n;
        }
    }
    for (t, n) in &expected {
        let got = page_numbers.get(t).copied().unwrap_or(0);
        assert!(got >= *n, "number {t}: {got} on the page, {n} expected");
    }
    // The new sections appear.
    for want in [
        "At a glance:",
        "What this means for clinical practice",
        "Other notable findings from this run",
        "RCTs",
        "Number check:",
        "Copy as text",
    ] {
        assert!(page.contains(want), "{want} missing");
    }
    assert!(page.contains("DIRECT reports surgeon experience inconsistently"));
    assert!(page.contains("Robotic TAPP vs laparoscopic"));
    // No guideline was found by the prototype's searches: no section.
    assert!(!page.contains("Guidelines and consensus statements"));

    // ---- the counts (for the stage report's before/after table) ----
    let counts = json!({
        "writer_input_bytes": user.len(),
        "writer_input_studies": studies.iter().filter(|s| user.contains(&format!("PMID {}", s.pmid))).count(),
        "writer_input_outcomes": expected_outcomes,
        "writer_input_structured_numbers": expected_numbers,
        "writer_input_number_tokens": number_tokens(&user).len(),
        "writer_input_caveats": review.must_mention.len(),
        "writer_input_contradictions": review.contradicting.len(),
        "page_bytes": html.len(),
        "page_trial_rows": written.studies.len(),
        "page_outcome_rows": written.outcomes.len(),
        "page_number_tokens": number_tokens(&page).len(),
        "page_references": references.len(),
        "page_caveats": review.must_mention.iter().filter(|m| page.contains(&flat(m).chars().take(80).collect::<String>())).count(),
        "numbers_checked": checked,
        "numbers_untraced": untraced.len(),
    });
    assert_eq!(counts["page_trial_rows"], 10);
    assert_eq!(counts["page_outcome_rows"], 51);
    assert_eq!(counts["page_references"], 10);
    assert_eq!(counts["page_caveats"], 8);
    // Pinned: what the trace marks on the prototype payloads (22 of 523;
    // see the Stage 4 report §8 for each). Stage 4b review S1: only a
    // number written with no sign of its own may match up to its sign; the
    // six sign-only marks here are all written with a minus of their own
    // ("− 0.02"), so they stay marked, as in Stage 4.
    assert_eq!(counts["numbers_checked"], 523);
    assert_eq!(counts["numbers_untraced"], 22);
    // What the trace marked, as text, for the report.
    let fields = incisive_app::agent::answer_v3::text_fields(&written);
    let marked: Vec<String> = untraced
        .iter()
        .map(|sp| {
            let t = fields
                .iter()
                .find(|(f, _)| *f == sp.field)
                .map_or("", |(_, t)| t);
            let a = sp.start as usize;
            let b = (sp.start + sp.len) as usize;
            if sp.len == 0 || b > t.len() {
                format!("{} (whole)", sp.field)
            } else {
                format!("{}: {}", sp.field, &t[a..b])
            }
        })
        .collect();
    if let Ok(p) = std::env::var("REPLAY_OUT") {
        std::fs::write(
            p,
            serde_json::to_string_pretty(
                &json!({"counts": counts, "untraced": marked, "writer_input": user, "page": page}),
            )
            .unwrap(),
        )
        .unwrap();
    }

    // ---- Stage 4b item 1: the coverage check on the full answer ----
    let known = |p: &str| selected.iter().any(|s| s == p);
    let is_meta = |p: &str| studies.iter().any(|s| s.pmid == p && s.design == "SR/MA");
    let all = coverage::check(&written, &rows, is_meta, Some(&review), known);
    // The scripted writer wrote every outcome of every row it was shown.
    assert_eq!(all.counts.outcomes_missing, 0, "{:?}", all.gaps);
    assert_eq!(all.counts.numbers_missing, 0, "{:?}", all.gaps);
    assert_eq!(all.counts.outcomes_checked, 51);
    // Every verified structured number is checked (the extractor's check
    // flagged the rest of the 174).
    let verified_numbers = rows
        .iter()
        .flat_map(|r| &r.outcomes)
        .filter(|o| o.verified)
        .flat_map(|o| &o.numbers)
        .filter(|n| n.verified)
        .count();
    assert_eq!(all.counts.numbers_checked as usize, verified_numbers);
    assert_eq!(verified_numbers, 136);
    eprintln!("COVERAGE full answer: {:?}", all.counts);

    // ---- Stage 4b item 1: a reconstructed 31 Oct run 1 ----
    // Run 1's extractor had ROGER's workload among its six outcomes (the
    // prototype's had it tenth, past the cap): move it first. Run 1 then
    // left out the workload (NASA-TLX 18.4 vs 34.0), the AIIMS pain at 6 h
    // and 12 h and rescue analgesia: the check reports them as gaps.
    let mut f1 = fixture("evidence.json");
    for x in f1["studies"].as_array_mut().unwrap() {
        if x["pmid"] == "41511872" {
            let o = x["outcomes"].as_array_mut().unwrap();
            let i = o
                .iter()
                .position(|o| s(&o["outcome"]).contains("NASA raw TLX"))
                .unwrap();
            let nasa = o.remove(i);
            o.insert(0, nasa);
        }
    }
    let rows = checked_rows_of(&f1);
    let full1 = scripted_answer(&rows, &studies, &review);
    let mut run1 = serde_json::from_value::<writer_v3::WireAnswer>(full1)
        .expect("the scripted run-1 answer reads")
        .into_answer()
        .0;
    let before = run1.outcomes.len();
    run1.outcomes.retain(|o| {
        !(o.outcome.contains("NASA raw TLX")
            || o.outcome.starts_with("Acute pain (primary), VAS")
            || o.outcome.starts_with("Rescue analgesia"))
    });
    assert_eq!(run1.outcomes.len(), before - 3);
    let cov = coverage::check(&run1, &rows, is_meta, Some(&review), known);
    let gap_numbers = |pmid: &str| -> Vec<f64> {
        cov.gaps
            .iter()
            .filter(|g| g.pmid == pmid)
            .flat_map(|g| g.numbers.clone())
            .collect()
    };
    let roger = gap_numbers("41511872");
    assert!(roger.contains(&18.4), "ROGER NASA-TLX 18.4: {:?}", cov.gaps);
    let aiims_row = rows.iter().find(|r| r.pmid == "42527734").unwrap();
    let pain = aiims_row
        .outcomes
        .iter()
        .find(|o| o.outcome.starts_with("Acute pain (primary), VAS"))
        .unwrap();
    let aiims = gap_numbers("42527734");
    let pain_missing: Vec<f64> = pain
        .numbers
        .iter()
        .map(|n| n.value)
        .filter(|v| aiims.contains(v))
        .collect();
    assert!(
        !pain_missing.is_empty(),
        "AIIMS 6 h / 12 h pain: {:?} not in {:?}",
        pain.numbers,
        cov.gaps
    );
    assert!(
        cov.gaps
            .iter()
            .any(|g| g.pmid == "42527734" && g.outcome.starts_with("Rescue analgesia")),
        "rescue analgesia: {:?}",
        cov.gaps
    );
    eprintln!(
        "COVERAGE run 1: {:?}\n{}\n{:#?}",
        cov.counts,
        cov.line(),
        cov.gaps
    );

    // ---- Stage 4b item 2: the completeness review on run 1 (scripted) ----
    // The real prompt assembly and the real call path (`completeness::review`,
    // a ScriptedLlm replying), the code checks, the store round trip and the
    // page. Nothing the writer wrote changes; additions appear labelled.
    let mut a1 = answer.clone();
    a1.answer = Some(run1.clone());
    a1.coverage = Some(cov.clone());
    let full1 = FullInput {
        rows: &rows,
        skeptic: Some(&review),
        skeptic_note: None,
        missed: &[],
    };
    let comp_user = completeness::user(
        QUESTION,
        &a1.plain_text(),
        &cov,
        &writer_v3::full_blocks(&studies, &full1),
    );
    assert!(
        comp_user.contains("NASA raw TLX"),
        "the gap reaches the review"
    );
    assert!(comp_user.contains("<source id=\"row-41511872\">"));
    assert!(comp_user.len() <= completeness::worst_case_user(QUESTION).len());
    let first_row = run1.outcomes[0].outcome.clone();
    let comp_llm = Arc::new(ScriptedLlm::new().when("GAPS FOUND BY CODE").reply_json(
        json!({
            "added_outcomes": [{"outcome": "Surgeon workload (NASA raw TLX)",
                "effect_text": "Mean 18.4 with robotic TAPP vs 34.0 with TEP (lower = less workload) [41511872]",
                "pmids": ["41511872"], "certainty": "Low", "why": "one trial"},
                {"outcome": "Rescue analgesia", "effect_text": "20.6% vs 17.9%; ARD 2.7% (95% CI -13.7% to 19.1%) [42527734]",
                "pmids": ["42527734"], "certainty": "Low", "why": "one small trial"}],
            "outcome_additions": [{"outcome": first_row, "text": "Pain at 6 h: mean difference 0.28 [42527734].", "pmids": ["42527734"]}],
            "added_conflicts": [],
            "not_added": [{"gap": "AIIMS 12 h CI bound", "reason": "already covered by the 6 h estimate"}],
            "notes": [{"text": "Certainty for workload rests on one trial."}]
        }),
        Usage { input_tokens: 25_000, output_tokens: 1_800, ..Usage::default() },
    ));
    let cctx = run_ctx(&pool, comp_llm.clone()).await;
    let allowed1 = writer_v3::allowed_numbers(&studies, Some(&full1));
    let cx = completeness::Context {
        selected: &selected,
        allowed: &allowed1,
        outcomes: &run1.outcomes,
        gaps_found: cov.gaps.len() as u32,
    };
    let c = completeness::review(&cctx, &cctx.llm, &comp_user, &cx)
        .await
        .unwrap();
    assert_eq!(c.skipped, None);
    assert_eq!(c.added(), 3);
    assert_eq!(c.outcome_additions[0].row, 0);
    assert_eq!(c.removed_citations, 0);
    assert!(
        c.untraced.is_empty(),
        "every added number traces: {:?}",
        c.untraced
    );
    let written_before = serde_json::to_string(&a1.answer).unwrap();
    a1.completeness = Some(c);
    let stored1: AnswerV3 = serde_json::from_value(serde_json::to_value(&a1).unwrap()).unwrap();
    assert_eq!(
        serde_json::to_string(&stored1.answer).unwrap(),
        written_before,
        "add-only"
    );
    let html1 = incisive_app::app::render_answer_v3(stored1.clone());
    let page1 = flat(&page_text(&html1));
    assert!(page1.contains("Mean 18.4 with robotic TAPP vs 34.0 with TEP"));
    assert!(page1.contains("Pain at 6 h: mean difference 0.28"));
    assert!(page1.matches(ADDED_LABEL).count() >= 3);
    assert!(page1.contains("Judged not relevant (1)"));
    let text1 = stored1.plain_text();
    assert!(
        text1.matches(ADDED_LABEL).count() >= 4,
        "rows, sentence and note"
    );
    // Everything the writer wrote is still on the page.
    for o in &run1.outcomes {
        let name: String = flat(&o.outcome).chars().take(60).collect();
        assert!(page1.contains(&name), "{name}");
    }
    eprintln!(
        "COMPLETENESS replay: {}",
        stored1.completeness.as_ref().unwrap().line()
    );
}

// ---------- Part A: the interactive answer page ----------

/// The replay's stored answers, built without a database: the scripted
/// writer reply read as the writer call reads it (`WireAnswer`), the same
/// post-writer checks as above, and a fixed `created_at`. The first is the
/// full Stage 4 answer; the second the reconstructed 31 Oct run 1 with its
/// coverage check and the completeness review's additions (as the scripted
/// review above returns them).
fn replay_answers() -> (AnswerV3, AnswerV3) {
    let (studies, _contradictions, searches) = studies();
    let selected: Vec<String> = studies.iter().map(|s| s.pmid.clone()).collect();
    let rows = checked_rows();
    let review = validated_review(&selected);
    let full = FullInput {
        rows: &rows,
        skeptic: Some(&review),
        skeptic_note: None,
        missed: &[],
    };
    let check = |w: writer_v3::WireAnswer| {
        let (mut written, defaulted) = w.into_answer();
        let mut integrity = IntegrityV3 {
            labels_defaulted: defaulted,
            ..IntegrityV3::default()
        };
        writer_v3::check_citations(&mut written, &selected, &mut integrity);
        writer_v3::evidence_types(&mut written, |p| {
            studies
                .iter()
                .find(|s| s.pmid == p)
                .and_then(|s| writer_v3::design_class(&s.design))
        });
        let allowed = writer_v3::allowed_numbers(&studies, Some(&full));
        let (untraced, checked) = writer_v3::trace_numbers(
            &written,
            &allowed,
            |p| selected.iter().any(|s| s == p),
            true,
        );
        integrity.numbers_checked = checked;
        integrity.numbers_untraced = untraced.len() as u32;
        (written, integrity, untraced)
    };
    let wire = serde_json::from_value(scripted_answer(&rows, &studies, &review)).unwrap();
    let (written, integrity, untraced) = check(wire);
    let extraction = Extraction {
        rows: rows.clone(),
        ..Extraction::default()
    };
    let references: Vec<ReferenceV3> = studies
        .iter()
        .enumerate()
        .map(|(i, s)| ReferenceV3 {
            n: i as u16 + 1,
            pmid: s.pmid.clone(),
            title: s.title.clone(),
            journal: s.journal.clone(),
            year: s.year,
            doi: s.doi.clone(),
            retracted: false,
            found_by_skeptic: false,
        })
        .collect();
    let answer = AnswerV3 {
        version: ANSWER_VERSION,
        question: QUESTION.into(),
        mode: RunMode::Focused,
        created_at: "2026-10-31T09:00:00Z".parse().unwrap(),
        answer: Some(written),
        empty_reason: None,
        references,
        searches,
        integrity,
        stopped_early: None,
        cost_paise: 13689,
        elapsed_s: 400,
        skeptic: Some(team::skeptic_summary(&review)),
        evidence: team::evidence_summary(&studies, &extraction),
        notes: vec![],
        untraced,
        notable_code: team::notable_code(Some(&review), &extraction),
        coverage: None,
        completeness: None,
    };
    // Run 1: three outcomes left out, then the review's additions.
    let mut f1 = fixture("evidence.json");
    for x in f1["studies"].as_array_mut().unwrap() {
        if x["pmid"] == "41511872" {
            let o = x["outcomes"].as_array_mut().unwrap();
            let i = o
                .iter()
                .position(|o| s(&o["outcome"]).contains("NASA raw TLX"))
                .unwrap();
            let nasa = o.remove(i);
            o.insert(0, nasa);
        }
    }
    let rows1 = checked_rows_of(&f1);
    let mut wire1: Value = scripted_answer(&rows1, &studies, &review);
    wire1["outcomes"].as_array_mut().unwrap().retain(|o| {
        let name = s(&o["outcome"]);
        !(name.contains("NASA raw TLX")
            || name.starts_with("Acute pain (primary), VAS")
            || name.starts_with("Rescue analgesia"))
    });
    let (run1, integrity1, untraced1) = check(serde_json::from_value(wire1).unwrap());
    let known = |p: &str| selected.iter().any(|s| s == p);
    let is_meta = |p: &str| studies.iter().any(|s| s.pmid == p && s.design == "SR/MA");
    let cov = coverage::check(&run1, &rows1, is_meta, Some(&review), known);
    let first_row = run1.outcomes[0].outcome.clone();
    let completeness: incisive_app::agent::answer_v3::CompletenessV3 =
        serde_json::from_value(json!({
            "skipped": null,
            "gaps_found": cov.gaps.len(),
            "added_outcomes": [{"outcome": "Surgeon workload (NASA raw TLX)",
                "effect_text": "Mean 18.4 with robotic TAPP vs 34.0 with TEP (lower = less workload) [41511872]",
                "pmids": ["41511872"], "certainty": "Low", "why": "one trial"},
                {"outcome": "Rescue analgesia", "effect_text": "20.6% vs 17.9%; ARD 2.7% (95% CI -13.7% to 19.1%) [42527734]",
                "pmids": ["42527734"], "certainty": "Low", "why": "one small trial"}],
            "outcome_additions": [{"outcome": first_row, "row": 0,
                "text": "Pain at 6 h: mean difference 0.28 [42527734].", "pmids": ["42527734"]}],
            "added_conflicts": [],
            "not_added": [{"gap": "AIIMS 12 h CI bound", "reason": "already covered by the 6 h estimate"}],
            "notes": ["Certainty for workload rests on one trial."],
            "untraced": []
        }))
        .unwrap();
    let mut a1 = answer.clone();
    a1.answer = Some(run1);
    a1.integrity = integrity1;
    a1.untraced = untraced1;
    a1.coverage = Some(cov);
    a1.completeness = Some(completeness);
    (answer, a1)
}

/// Number tokens in the writer's input from the prototype payloads, pinned:
/// the same 1,454 the Stage 3 code gives for the same payloads.
const WRITER_INPUT_NUMBER_TOKENS: usize = 1_454;

fn fmt(v: f64) -> String {
    if v.fract() == 0.0 && v.abs() < 1e15 {
        format!("{v:.0}")
    } else {
        format!("{v}")
    }
}

/// Writes the payload of `.superpowers/phase3/smoke-stage4.sh` (not a test):
/// the app's own Full writer prompt and schema with the real prototype
/// writer input above, the extractor's prompt and schema on an abstract that
/// gives n in words and a funding sentence, and the skeptic's first turn
/// over the prototype's selection.
///   SMOKE_PAYLOAD_OUT=payload.json cargo test -p incisive-app \
///     --test replay_stage4 write_smoke_stage4_payload -- --ignored
#[tokio::test]
#[ignore = "writes the smoke payload; not a test"]
async fn write_smoke_stage4_payload() {
    use incisive_app::agent::extractor::{prompt, row_schema};
    use incisive_app::agent::llm::SOURCE_RULE;
    use incisive_app::agent::researcher;
    use incisive_app::agent::skeptic::Picture;
    use incisive_app::agent::tools::lit::LitTools;
    let out = std::env::var("SMOKE_PAYLOAD_OUT").expect("SMOKE_PAYLOAD_OUT");
    let rule = |s: &str| format!("{s}\n\n{SOURCE_RULE}");
    let (studies, contradictions, searches) = studies();
    let selected: Vec<String> = studies.iter().map(|s| s.pmid.clone()).collect();
    let rows = checked_rows();
    let review = validated_review(&selected);
    let full = FullInput {
        rows: &rows,
        skeptic: Some(&review),
        skeptic_note: None,
        missed: &[],
    };
    let writer_user =
        writer_v3::writer_user_full(QUESTION, &studies, &full, &contradictions, &searches);
    let abs = "Background: Robotic and laparoscopic transabdominal preperitoneal repair were compared in a randomised trial. \
Methods: Sixty patients with a primary unilateral inguinal hernia were randomised to robotic or laparoscopic repair; fifty-nine were analysed. \
Results: Operative time was longer with robotic repair (64.9 vs 31.7 min; p < 0.001). Fifteen of 39 robotic patients and one of 20 laparoscopic patients had resumed normal activity at 14 days (p = 0.006). \
Funding: the trial was funded by the regional health authority; two authors received speaker fees from the robot's manufacturer. \
Conclusions: Robotic repair took longer and may speed the return to normal activity.";
    let paper = incisive_app::lit::Paper {
        pmid: Some("40450465".into()),
        pmcid: None,
        doi: None,
        title: "Robotic versus laparoscopic TAPP: a randomised trial (smoke fixture)".into(),
        authors: vec![],
        journal: Some("Smoke J".into()),
        year: Some(2025),
        pub_types: vec!["Randomized Controlled Trial".into()],
        abstract_text: Some(abs.into()),
        oa_url: None,
        oa_is_pdf: false,
        retracted: false,
        source: incisive_app::lit::PaperSource::PubMed,
    };
    let src = source_text(&paper, None).unwrap();
    let pool = lazy_pool(&SecretString::from(
        "postgres://u:p@127.0.0.1:1/x".to_string(),
    ))
    .unwrap();
    let lit = LitTools::new(
        incisive_app::agent::testing::offline_literature(),
        pool,
        Uuid::nil(),
        QUESTION,
    );
    let picture: Vec<Picture> = studies
        .iter()
        .map(|s| Picture {
            pmid: s.pmid.clone(),
            title: s.title.clone(),
            design: s.design.clone(),
            why: s.why.clone(),
        })
        .collect();
    let payload = json!({
        "question": QUESTION,
        "writer": {
            "system": rule(writer_v3::SYSTEM_FULL),
            "schema": writer_v3::writer_schema(),
            "user": writer_user,
            "max_tokens": DepthSettings::FOCUSED.writer_max_tokens,
            // ROGER: "Robotic TAPP vs laparoscopic TEP", the live arm-order case.
            "arm_pmid": "41511872",
        },
        "extractor": {
            "system": rule(extractor::SYSTEM),
            "schema": row_schema(),
            "user": prompt(QUESTION, "40450465", &src),
            "source_body": src.body(),
            "max_tokens": DepthSettings::FOCUSED.extractor_max_tokens,
        },
        "skeptic": {
            "system": rule(&skeptic::system_prompt(8)),
            "tools": lit.registry_of(&researcher::QUICK_TOOLS).defs(),
            "task": skeptic::task(QUESTION, &picture, &contradictions),
            "max_tokens": DepthSettings::FOCUSED.skeptic_max_tokens,
        }
    });
    std::fs::write(out, serde_json::to_string_pretty(&payload).unwrap()).unwrap();
}

/// Writes the payload of `.superpowers/phase3/probe-writer-schema.sh` (not
/// a test): the real Full writer system prompt and the two writer schema
/// variants (B, the Stage 3 control, and S, live), generated by the app's
/// own `writer_schema_with`, with their complexity.
///   SMOKE_PAYLOAD_OUT=probe.json cargo test -p incisive-app \
///     --test replay_stage4 write_probe_writer_schema_payload -- --ignored
#[tokio::test]
#[ignore = "writes the probe payload; not a test"]
async fn write_probe_writer_schema_payload() {
    use incisive_app::agent::llm::SOURCE_RULE;
    use incisive_app::agent::writer_v3::{SchemaOpts, schema_complexity, writer_schema_with};
    let out = std::env::var("SMOKE_PAYLOAD_OUT").expect("SMOKE_PAYLOAD_OUT");
    // 31 Oct (Cloud Shell probe): every variant with structured Stage 4
    // additions was refused and Stage 3 accepted. Owner ruling R-S4a: the
    // Stage 3 schema plus three plain strings.
    let variants: Vec<Value> = [
        (
            "B",
            "Stage 3, exact: the control (accepted live)",
            SchemaOpts::Stage3,
        ),
        (
            "S",
            "Stage 3 plus 3 plain strings (R-S4a): LIVE",
            SchemaOpts::Stage3PlusText,
        ),
    ]
    .into_iter()
    .map(|(name, what, o)| {
        let schema = writer_schema_with(o);
        let c = schema_complexity(&schema);
        json!({"name": name, "what": what, "schema": schema, "complexity": {
            "bytes": c.bytes, "enum_values": c.enum_values, "nullable_unions": c.nullable_unions,
            "depth": c.depth, "properties": c.properties, "objects": c.objects}})
    })
    .collect();
    let payload = json!({
        "system": format!("{}\n\n{SOURCE_RULE}", writer_v3::SYSTEM_FULL),
        "user": "<question>\nDoes a test question compile?\n</question>\n\nSTUDIES SELECTED BY THE RESEARCHER (0):\n\nWrite the answer now, as JSON matching the schema.",
        "max_tokens": 32,
        "variants": variants,
        "skipped": "Only B (the Stage 3 control) and S (live) are sent (owner ruling R-S4a)."
    });
    std::fs::write(out, serde_json::to_string_pretty(&payload).unwrap()).unwrap();
}

/// The reconstructed 31 Oct run 1 without a database: the prototype rows
/// (ROGER's workload among its six), the validated review, the scripted
/// answer with the workload, the AIIMS 6 h / 12 h pain and rescue analgesia
/// left out, its coverage, and the completeness review's real user message.
fn run1_review_user() -> (String, usize) {
    let (studies, _, searches) = studies();
    let selected: Vec<String> = studies.iter().map(|s| s.pmid.clone()).collect();
    let mut f1 = fixture("evidence.json");
    for x in f1["studies"].as_array_mut().unwrap() {
        if x["pmid"] == "41511872" {
            let o = x["outcomes"].as_array_mut().unwrap();
            let i = o
                .iter()
                .position(|o| s(&o["outcome"]).contains("NASA raw TLX"))
                .unwrap();
            let nasa = o.remove(i);
            o.insert(0, nasa);
        }
    }
    let rows = checked_rows_of(&f1);
    let review = validated_review(&selected);
    let mut run1 =
        serde_json::from_value::<writer_v3::WireAnswer>(scripted_answer(&rows, &studies, &review))
            .unwrap()
            .into_answer()
            .0;
    run1.outcomes.retain(|o| {
        !(o.outcome.contains("NASA raw TLX")
            || o.outcome.starts_with("Acute pain (primary), VAS")
            || o.outcome.starts_with("Rescue analgesia"))
    });
    let known = |p: &str| selected.iter().any(|s| s == p);
    let is_meta = |p: &str| studies.iter().any(|s| s.pmid == p && s.design == "SR/MA");
    let cov = coverage::check(&run1, &rows, is_meta, Some(&review), known);
    let references: Vec<ReferenceV3> = studies
        .iter()
        .enumerate()
        .map(|(i, s)| ReferenceV3 {
            n: i as u16 + 1,
            pmid: s.pmid.clone(),
            title: s.title.clone(),
            journal: s.journal.clone(),
            year: s.year,
            doi: s.doi.clone(),
            retracted: false,
            found_by_skeptic: false,
        })
        .collect();
    let a = AnswerV3 {
        version: ANSWER_VERSION,
        question: QUESTION.into(),
        mode: RunMode::Focused,
        created_at: chrono::Utc::now(),
        answer: Some(run1),
        empty_reason: None,
        references,
        searches,
        integrity: IntegrityV3::default(),
        stopped_early: None,
        cost_paise: 0,
        elapsed_s: 0,
        skeptic: Some(team::skeptic_summary(&review)),
        evidence: vec![],
        notes: vec![],
        untraced: vec![],
        notable_code: vec![],
        coverage: Some(cov.clone()),
        completeness: None,
    };
    let full = FullInput {
        rows: &rows,
        skeptic: Some(&review),
        skeptic_note: None,
        missed: &[],
    };
    (
        completeness::user(
            QUESTION,
            &a.plain_text(),
            &cov,
            &writer_v3::full_blocks(&studies, &full),
        ),
        cov.gaps.len(),
    )
}

/// Writes the payload of `.superpowers/phase3/smoke-stage4b.sh` (not a
/// test): the completeness review's real prompt, schema and user message on
/// the reconstructed run 1 (gaps present), and the skeptic's first turn
/// with its Stage 4b prompt over the prototype's selection.
///   SMOKE_PAYLOAD_OUT=payload.json cargo test -p incisive-app \
///     --test replay_stage4 write_smoke_stage4b_payload -- --ignored
#[tokio::test]
#[ignore = "writes the smoke payload; not a test"]
async fn write_smoke_stage4b_payload() {
    use incisive_app::agent::llm::SOURCE_RULE;
    use incisive_app::agent::researcher;
    use incisive_app::agent::skeptic::Picture;
    use incisive_app::agent::tools::lit::LitTools;
    let out = std::env::var("SMOKE_PAYLOAD_OUT").expect("SMOKE_PAYLOAD_OUT");
    let rule = |s: &str| format!("{s}\n\n{SOURCE_RULE}");
    let (user, gaps) = run1_review_user();
    let (studies, contradictions, _) = studies();
    let pool = lazy_pool(&SecretString::from(
        "postgres://u:p@127.0.0.1:1/x".to_string(),
    ))
    .unwrap();
    let lit = LitTools::new(
        incisive_app::agent::testing::offline_literature(),
        pool,
        Uuid::nil(),
        QUESTION,
    );
    let picture: Vec<Picture> = studies
        .iter()
        .map(|s| Picture {
            pmid: s.pmid.clone(),
            title: s.title.clone(),
            design: s.design.clone(),
            why: s.why.clone(),
        })
        .collect();
    let payload = json!({
        "question": QUESTION,
        "completeness": {
            "model": incisive_app::agent::llm::AgentModels::default().completeness,
            "system": rule(completeness::SYSTEM),
            "schema": completeness::schema(),
            "user": user,
            "gaps": gaps,
            "max_tokens": completeness::MAX_TOKENS,
            "effort": "low",
        },
        "skeptic": {
            "system": rule(&skeptic::system_prompt(8)),
            "tools": lit.registry_of(&researcher::QUICK_TOOLS).defs(),
            "task": skeptic::task(QUESTION, &picture, &contradictions),
            "max_tokens": DepthSettings::FOCUSED.skeptic_max_tokens,
        }
    });
    std::fs::write(out, serde_json::to_string_pretty(&payload).unwrap()).unwrap();
}

/// Writes the payload of `.superpowers/phase3/probe-stage4b-schemas.sh`
/// (not a test): the completeness review's schema with its real system
/// prompt, the skeptic's changed final schema (`extract_next` added) with
/// its real final prompt, and the writer's live schema (unchanged, the
/// control), each with a tiny user message.
///   SMOKE_PAYLOAD_OUT=probe.json cargo test -p incisive-app \
///     --test replay_stage4 write_probe_stage4b_payload -- --ignored
#[tokio::test]
#[ignore = "writes the probe payload; not a test"]
async fn write_probe_stage4b_payload() {
    use incisive_app::agent::llm::SOURCE_RULE;
    use incisive_app::agent::writer_v3::schema_complexity;
    let out = std::env::var("SMOKE_PAYLOAD_OUT").expect("SMOKE_PAYLOAD_OUT");
    let rule = |s: &str| format!("{s}\n\n{SOURCE_RULE}");
    let cx = |v: &Value| {
        let c = schema_complexity(v);
        json!({"bytes": c.bytes, "enum_values": c.enum_values, "nullable_unions": c.nullable_unions,
            "depth": c.depth, "properties": c.properties, "objects": c.objects})
    };
    let models = incisive_app::agent::llm::AgentModels::default();
    let probes = vec![
        json!({"name": "C", "what": "completeness review schema (Stage 4b, new)", "model": models.completeness,
            "effort": "low", "system": rule(completeness::SYSTEM), "schema": completeness::schema(),
            "complexity": cx(&completeness::schema()), "live": true,
            "user": "<question>\nDoes a test question compile?\n</question>\n\nReview the answer for completeness now, as JSON matching the schema."}),
        json!({"name": "K", "what": "skeptic final schema with extract_next (Stage 4b, changed)", "model": models.skeptic,
            "effort": "low", "system": rule(skeptic::FINAL_SYSTEM), "schema": skeptic::report_schema(),
            "complexity": cx(&skeptic::report_schema()), "live": true,
            "user": "<question>\nDoes a test question compile?\n</question>\n\nSubmit your review now, as JSON matching the schema."}),
        json!({"name": "S", "what": "writer live schema (unchanged; the control)", "model": models.writer,
            "effort": null, "system": rule(writer_v3::SYSTEM_FULL), "schema": writer_v3::writer_schema(),
            "complexity": cx(&writer_v3::writer_schema()), "live": false,
            "user": "<question>\nDoes a test question compile?\n</question>\n\nSTUDIES SELECTED BY THE RESEARCHER (0):\n\nWrite the answer now, as JSON matching the schema."}),
    ];
    let payload = json!({"max_tokens": 32, "probes": probes});
    std::fs::write(out, serde_json::to_string_pretty(&payload).unwrap()).unwrap();
}

// ---------- Part A: render tests of the interactive answer page ----------

/// The summary of the original run view (item 13).
const ORIGINAL_SUMMARY: &str = "Original run view (for checking)";

/// The new page's part of the HTML (before the original run view) and the
/// original run view's part.
fn split_page(html: &str) -> (&str, &str) {
    let i = html
        .find("<details class=\"ap-original\"")
        .expect("the original run view is drawn");
    (&html[..i], &html[i..])
}

/// A replay answer with what the 31 Oct runs had and the prototype payloads
/// do not (test data only, no real question): percentages in two arms, a
/// ratio and a difference with CIs, "P < .001", a registration ID marked
/// "not traced", a guideline whose quote the skeptic also gave as evidence
/// against, a skeptic lead on a trial row and a completeness addition
/// citing `[<pmid> as <n>]`.
fn enriched(mut a: AnswerV3) -> AnswerV3 {
    use incisive_app::agent::answer_v3::{
        AddedOutcome, Certainty, CompletenessV3, Number, SkepticItem, SkepticLead, Span,
    };
    let refs = a.ref_numbers();
    let w = a.answer.as_mut().unwrap();
    let p0 = w.studies[0].pmid.clone();
    let p1 = w.studies[1].pmid.clone();
    let p2 = w.studies[2].pmid.clone();
    let n0 = refs[&p0];
    let num = |label: &str,
               value: f64,
               unit: &str,
               arm: Option<&str>,
               ci: Option<(f64, f64)>,
               p: Option<&str>| Number {
        label: label.into(),
        value,
        unit: Some(unit.into()),
        arm: arm.map(String::from),
        ci_low: ci.map(|c| c.0),
        ci_high: ci.map(|c| c.1),
        p: p.map(String::from),
        pmid: p0.clone(),
    };
    w.outcomes[0].numbers.extend([
        num(
            "Same-day discharge",
            95.9,
            "%",
            Some("robotic"),
            None,
            Some("P=0.012"),
        ),
        num(
            "Same-day discharge",
            81.5,
            "%",
            Some("laparoscopic"),
            None,
            Some("P=0.012"),
        ),
        num("Seroma", 0.4, "RR", None, Some((0.08, 2.03)), None),
        num(
            "Length of stay, mean difference",
            0.33,
            "days",
            Some("MD"),
            Some((-0.03, 0.7)),
            Some("P < .001"),
        ),
    ]);
    let appraisal = &mut w.studies[1].appraisal;
    appraisal.push_str(" Same cohort as an earlier report (NTR3593).");
    let at = appraisal.rfind("3593").unwrap();
    a.untraced
        .push(Span::new("studies.1.appraisal", at, at + 4));
    let quote = "Mesh repair is recommended for all adults with an inguinal hernia.";
    let k = a.skeptic.as_mut().unwrap();
    k.guidelines.push(SkepticItem {
        pmid: p1.clone(),
        text: "European Hernia Society: mesh repair for adults".into(),
        quote: Some(quote.into()),
    });
    k.contradicting.push(SkepticItem {
        pmid: p1,
        text: "A guideline recommends mesh repair".into(),
        quote: Some(quote.into()),
    });
    k.leads.push(SkepticLead {
        pmid: p2,
        kind: "contradicting study".into(),
    });
    a.completeness = Some(CompletenessV3 {
        gaps_found: 1,
        added_outcomes: vec![AddedOutcome {
            outcome: "Specific complications".into(),
            effect_text: format!(
                "Seroma RR 0.40 (95% CI 0.08-2.03) [{p0} as {n0}]. Conversion was rare."
            ),
            pmids: vec![p0],
            certainty: Some(Certainty::Low),
            why: "sparse events".into(),
            unsourced: false,
        }],
        ..CompletenessV3::default()
    });
    a
}

/// The replay answers: the two built from the payloads and the enriched one.
fn part_a_answers() -> Vec<(&'static str, AnswerV3)> {
    let (a, a1) = replay_answers();
    let e = enriched(a.clone());
    vec![
        ("replay", a),
        ("replay run 1 with review", a1),
        ("enriched", e),
    ]
}

/// The skeptic's text fields with their span paths.
fn skeptic_fields(k: &incisive_app::agent::answer_v3::SkepticV3) -> Vec<(String, String)> {
    let mut v: Vec<(String, String)> = Vec::new();
    for (i, m) in k.must_mention.iter().enumerate() {
        v.push((format!("must_mention.{i}"), m.clone()));
    }
    for (i, o) in k.overlaps.iter().enumerate() {
        v.push((format!("overlaps.{i}.note"), o.note.clone()));
    }
    for (name, list) in [
        ("missed_rcts", &k.missed_rcts),
        ("extract_next", &k.extract_next),
        ("ma_notes", &k.ma_notes),
        ("contradicting", &k.contradicting),
        ("guidelines", &k.guidelines),
    ] {
        for (i, x) in list.iter().enumerate() {
            v.push((format!("{name}.{i}.text"), x.text.clone()));
        }
    }
    v
}

/// Item 1 and the visibility rules: the replay answers render, in the fixed
/// order, every block they have data for, the original run view last.
#[test]
fn part_a_replay_answers_render_their_blocks_in_order() {
    for (name, a) in part_a_answers() {
        let html = incisive_app::app::render_answer_v3(a.clone());
        let (page, original) = split_page(&html);
        let v = incisive_app::answer_page::visible(&a);
        let mut want: Vec<&str> = vec!["Decision support", "At a glance", "Bottom line"];
        want.push("What this means for clinical practice");
        if v.out_of_100 {
            want.push("Out of 100 patients");
        }
        if v.forest {
            want.push("Every estimate with a 95% CI");
        }
        want.push("Outcome by outcome");
        if v.compare {
            want.push("Compare the arms");
        }
        want.extend([
            "Details",
            "Challenged by the skeptic",
            "The trials",
            "References",
            "Search trail",
            "Cost and time",
            "Copy as text",
        ]);
        let mut at = 0;
        for h in want {
            let i = page[at..]
                .find(h)
                .unwrap_or_else(|| panic!("{name}: {h} missing or out of order"));
            at += i + h.len();
        }
        assert!(original.contains(ORIGINAL_SUMMARY), "{name}");
        // The prototype payloads have no percentages or CIs; the enriched
        // answer has every block.
        if name == "enriched" {
            assert!(v.out_of_100 && v.forest && v.compare && !v.no_numbers);
            assert!(page.contains(r#"role="img" aria-label="96 of 100 people, robotic""#));
            assert!(page.contains("Out of 100 people like those in the trials"));
            assert!(page.contains("Same-day discharge: 95.9% robotic vs 81.5% laparoscopic (p 0.012). Certainty for this outcome: Low."));
            assert!(page.contains("RR 0.4 (0.08\u{2013}2.03)"));
            assert!(page.contains("0.33 days (\u{2212}0.03 to 0.7)"));
            assert!(page.contains("Who these numbers apply to"));
        } else {
            assert!(!v.out_of_100 && !v.forest, "{name}");
            assert!(!page.contains("Out of 100 patients"), "{name}");
        }
        // The replay's numbers have two arms per label in places: paired.
        assert!(v.compare, "{name}");
    }
}

/// Flag, never delete: every mark the earlier page draws is on the new
/// page, by field, except the "not traced" marks on registration IDs
/// (item 12), which are counted. The original run view keeps them all.
#[test]
fn part_a_every_mark_is_still_drawn() {
    use incisive_app::agent::answer_v3::{marked_runs, text_fields, whole_marked};
    use incisive_app::answer_page::{
        Piece, completeness_fields, pieces, suppressed_registration_marks,
    };
    let drawn = |page: &str, key: &str| page.matches(&format!("data-mark=\"{key}\"")).count();
    for (name, a) in part_a_answers() {
        let html = incisive_app::app::render_answer_v3(a.clone());
        let (page, original) = split_page(&html);
        let refs = a.ref_numbers();
        let known = |p: &str| refs.contains_key(p);
        let mut expected_hidden = 0;
        let mut check = |ns: &str,
                         field: &str,
                         text: &str,
                         spans: &[incisive_app::agent::answer_v3::Span],
                         suppress: bool| {
            let ps = pieces(text, field, spans, known, suppress);
            let marks = ps.iter().filter(|p| **p == Piece::Mark).count();
            expected_hidden += ps.iter().filter(|p| **p == Piece::Suppressed).count();
            let runs = marked_runs(text, field, spans)
                .iter()
                .filter(|(_, m)| *m)
                .count();
            assert_eq!(
                marks + ps.iter().filter(|p| **p == Piece::Suppressed).count(),
                runs
            );
            assert!(
                drawn(page, &format!("{ns}{field}")) >= marks,
                "{name}: {ns}{field}: {} drawn, {marks} expected",
                drawn(page, &format!("{ns}{field}"))
            );
        };
        if let Some(w) = &a.answer {
            for (f, t) in text_fields(w) {
                check("w:", &f, t, &a.untraced, true);
            }
            // Whole-field marks: structured numbers and n.
            for sp in a.untraced.iter().filter(|s| s.len == 0) {
                assert!(whole_marked(&sp.field, &a.untraced));
                assert!(
                    drawn(page, &format!("w:{}", sp.field)) >= 1,
                    "{name}: {}",
                    sp.field
                );
            }
        }
        if let Some(c) = &a.completeness {
            for (f, t) in completeness_fields(c) {
                check("c:", &f, t, &c.untraced, true);
            }
        }
        if let Some(k) = &a.skeptic {
            let moved = incisive_app::answer_page::against_moved(&a);
            for (f, t) in skeptic_fields(k) {
                let i: usize = f
                    .split('.')
                    .nth(1)
                    .and_then(|x| x.parse().ok())
                    .unwrap_or(0);
                if f.starts_with("contradicting.") && moved[i] {
                    // Moved under Guidelines only when it carries no mark.
                    assert!(marked_runs(&t, &f, &k.spans).iter().all(|(_, m)| !m));
                    continue;
                }
                check("s:", &f, &t, &k.spans, false);
            }
        }
        for (k, n) in a
            .notable_code
            .iter()
            .enumerate()
            .filter(|(_, n)| !n.repeats_skeptic())
        {
            check(&format!("n{k}:"), "text", &n.text, &n.spans, false);
        }
        for e in a.evidence.iter().filter(|e| e.funding_checked) {
            if let Some(f) = &e.funding {
                check(&format!("e:{}:", e.pmid), "funding", f, &e.spans, false);
            }
        }
        assert_eq!(expected_hidden, suppressed_registration_marks(&a), "{name}");
        // The original run view: today's marks, all of them.
        let today = incisive_app::app::render_original_v3(a.clone(), "");
        let not_traced = |h: &str| h.matches(">not traced</sup>").count();
        let not_verified = |h: &str| h.matches(">not verified</sup>").count();
        assert_eq!(not_traced(original), not_traced(&today), "{name}");
        assert_eq!(not_verified(original), not_verified(&today), "{name}");
        // The new page draws at least as many, less the suppressed ones.
        assert!(
            not_traced(page) + expected_hidden >= not_traced(&today),
            "{name}"
        );
        eprintln!(
            "PART A marks {name}: today {} not traced / {} not verified; new page {} / {}; suppressed {}",
            not_traced(&today),
            not_verified(&today),
            not_traced(page),
            not_verified(page),
            expected_hidden
        );
        if name == "enriched" {
            assert_eq!(expected_hidden, 1, "the NTR3593 mark");
            assert!(
                original.replace("<!>", "").contains("NTR3593<sup"),
                "unsuppressed in the original view"
            );
        } else {
            // The prototype payloads mark no registration ID.
            assert_eq!(expected_hidden, 0, "{name}");
        }
    }
}

/// Item 13: the original run view is present, closed, last, and is today's
/// view with only its ids prefixed; the page has no duplicate ids.
#[test]
fn part_a_original_run_view_is_todays_page_closed_and_last() {
    for (name, a) in part_a_answers() {
        let html = incisive_app::app::render_answer_v3(a.clone());
        let (_, original) = split_page(&html);
        assert!(
            original.starts_with("<details class=\"ap-original\">"),
            "{name}: closed by default (no open attribute)"
        );
        assert!(original.contains(ORIGINAL_SUMMARY));
        assert!(original.contains("the answer as stored, in the earlier layout"));
        // Last: nothing after it but the closing tags of the page.
        let end = original.rfind("</details>").unwrap();
        assert_eq!(
            original[end..].replace("<!>", ""),
            "</details></div>",
            "{name}: last on the page"
        );
        // Today's view, ids prefixed: the same HTML once the prefix is taken off.
        let prefixed = incisive_app::app::render_original_v3(a.clone(), "orig-");
        assert!(
            original.contains(&prefixed),
            "{name}: the view is drawn as is"
        );
        let today = incisive_app::app::render_original_v3(a.clone(), "");
        let unprefixed = prefixed
            .replace("\"#orig-ref-", "\"#ref-")
            .replace("id=\"orig-ref-", "id=\"ref-")
            .replace("id=\"orig-answer-plain-text\"", "id=\"answer-plain-text\"");
        assert_eq!(unprefixed, today, "{name}: only the ids differ");
        // The debug line: counts only.
        let (o, n, st, r, u, cu) = incisive_app::answer_page::debug_counts(&a);
        assert!(original.contains(&format!(
            "{o} outcomes \u{b7} {n} numbers \u{b7} {st} studies \u{b7} {r} references \u{b7} {u} untraced spans ({cu} in the completeness review)"
        )));
        // No id appears twice on the page.
        let ids = regex::Regex::new(r#"\sid="([^"]+)""#).unwrap();
        let mut seen = std::collections::HashSet::new();
        for c in ids.captures_iter(&html) {
            assert!(
                seen.insert(c[1].to_string()),
                "{name}: duplicate id {}",
                &c[1]
            );
        }
        assert!(seen.contains("ref-1") && seen.contains("orig-ref-1") && seen.contains("sheet-1"));
    }
}

/// Item 12 on the enriched answer: p once, `[<pmid> as <n>]` as a chip,
/// guidelines listed once, the skeptic lead in the sheet's neutral box.
/// The original run view keeps today's text, glitches included.
#[test]
fn part_a_display_glitches_are_fixed_on_the_new_page_only() {
    let (a, _) = replay_answers();
    let e = enriched(a);
    let html = incisive_app::app::render_answer_v3(e.clone());
    // For a look at the page (written before the checks).
    if let Ok(out) = std::env::var("PART_A_HTML_OUT") {
        std::fs::write(out, &html).unwrap();
    }
    let (page, original) = split_page(&html);
    let text = |h: &str| flat(&page_text(h));
    let (pt, ot) = (text(page), text(original));
    assert!(pt.contains("p < .001") && !pt.contains("p P <"));
    assert!(ot.contains("p P < .001"), "the original keeps it");
    let p0 = &e.answer.as_ref().unwrap().studies[0].pmid;
    let n0 = e.ref_numbers()[p0];
    let as_n = regex::Regex::new(r"\[\d{5,9} as \d+\]").unwrap();
    assert!(!as_n.is_match(&pt), "no [pmid as n] left");
    assert!(page.replace("<!>", "").contains(&format!(
        "(95% CI 0.08-2.03) <span class=\"cite\">[<a href=\"#ref-{n0}\" rel=\"external\">{n0}</a>]</span>"
    )));
    assert!(ot.contains(&format!("[{p0} as {n0}]")));
    let quote = "Mesh repair is recommended for all adults with an inguinal hernia.";
    assert_eq!(
        pt.matches(quote).count(),
        1,
        "listed once, under Guidelines"
    );
    let g = pt.find("Guidelines and consensus statements").unwrap();
    let q = pt.find(quote).unwrap();
    assert!(g < q && q < pt.find("What would change this answer").unwrap());
    assert_eq!(
        ot.matches(quote).count(),
        3,
        "today: guidelines, evidence against, guideline"
    );
    // The skeptic lead: a badge on the card, the words in the sheet.
    assert!(pt.contains("skeptic lead"));
    assert!(pt.contains("Skeptic lead (unverified): Possible contradicting study"));
    assert!(!pt.contains("Skeptic: Possible"));
    assert!(ot.contains("Skeptic: Possible contradicting study"));
    // Registration IDs: the mark is counted in Checks.
    assert!(pt.contains(
        "1 \u{201c}not traced\u{201d} mark on a trial or review registration ID not drawn"
    ));
    // Copy as text: only the p values differ from the plain text.
    let plain = e.plain_text();
    assert_eq!(
        plain
            .replace("p P < .001", "p < .001")
            .replace("p P=0.012", "p 0.012"),
        e.copy_text()
    );
}

/// Copy as text is byte-identical to today on the replay answers (they
/// carry no p values, so the item 12 fix changes nothing there).
#[test]
fn part_a_copy_text_is_unchanged_on_the_replay() {
    let (a, a1) = replay_answers();
    for x in [a, a1] {
        assert!(
            x.answer
                .as_ref()
                .unwrap()
                .outcomes
                .iter()
                .flat_map(|o| &o.numbers)
                .all(|n| n.p.is_none())
        );
        assert_eq!(x.copy_text(), x.plain_text());
    }
}

/// A Stage 3 shaped answer (no at-a-glance, no clinical practice, no
/// numbers, no completeness), a Quick answer and an empty answer render,
/// their missing blocks hidden, with no empty boxes.
#[test]
fn part_a_stage3_quick_and_empty_answers_render() {
    let (a, _) = replay_answers();
    // Stage 3 shaped.
    let mut s3 = a.clone();
    let w = s3.answer.as_mut().unwrap();
    w.at_a_glance = None;
    w.clinical_practice = None;
    w.notable_findings.clear();
    for o in &mut w.outcomes {
        o.numbers.clear();
    }
    s3.untraced.retain(|sp| !sp.field.contains(".numbers."));
    s3.completeness = None;
    s3.coverage = None;
    let html = incisive_app::app::render_answer_v3(s3);
    let (page, _) = split_page(&html);
    for gone in [
        "At a glance",
        "What this means for clinical practice",
        "Out of 100",
        "Every estimate with a 95% CI",
        "Compare the arms",
        "Completeness review",
        ">Numbers<",
    ] {
        assert!(!page.contains(gone), "Stage 3: {gone} shown");
    }
    assert!(page.contains("This answer has no structured numbers"));
    assert!(page.contains("Outcome by outcome") && page.contains("The trials"));
    // Quick: no skeptic, no evidence rows.
    let mut q = a.clone();
    q.mode = RunMode::Quick;
    q.skeptic = None;
    q.evidence.clear();
    q.notable_code.clear();
    let html = incisive_app::app::render_answer_v3(q);
    let (page, original) = split_page(&html);
    assert!(page.contains(">Quick<"));
    assert!(!page.contains("Challenged by the skeptic"));
    assert!(!page.contains("verified quotes</span>") && !page.contains("verified quote</span>"));
    assert!(original.contains(ORIGINAL_SUMMARY));
    // Empty: its reason, as today.
    let mut empty = a;
    empty.answer = None;
    empty.empty_reason = Some("No relevant evidence found.".into());
    empty.completeness = None;
    let html = incisive_app::app::render_answer_v3(empty);
    let (page, original) = split_page(&html);
    assert!(page.contains("No relevant evidence found."));
    for gone in [
        "Outcome by outcome",
        "At a glance",
        "The trials",
        "This answer has no structured numbers",
    ] {
        assert!(!page.contains(gone), "empty: {gone} shown");
    }
    assert!(page.contains("id=\"ref-1\"") && original.contains("No relevant evidence found."));
}

/// No `inner_html` anywhere in the page's code.
#[test]
fn part_a_uses_no_inner_html() {
    for f in [
        "src/app.rs",
        "src/app/page.rs",
        "src/app/details.rs",
        "src/answer_page.rs",
    ] {
        let src = std::fs::read_to_string(format!("{}/{f}", env!("CARGO_MANIFEST_DIR"))).unwrap();
        let code: String = src
            .lines()
            .filter(|l| !l.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(!code.contains("inner_html"), "{f}");
    }
}

/// The page sizes, for the report (not a check).
#[test]
fn part_a_page_sizes() {
    for (name, a) in part_a_answers() {
        let html = incisive_app::app::render_answer_v3(a.clone());
        let today = incisive_app::app::render_original_v3(a, "");
        let (page, original) = split_page(&html);
        eprintln!(
            "PART A size {name}: before {} bytes; after {} bytes (new page {}, original view {})",
            today.len(),
            html.len(),
            page.len(),
            original.len()
        );
    }
}
