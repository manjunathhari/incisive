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
    ANSWER_VERSION, AnswerV3, IntegrityV3, ReferenceV3, SearchEntry,
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
use incisive_app::agent::{RunContext, team};
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
fn rows() -> Vec<Value> {
    let f = fixture("evidence.json");
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
                "numbers": numbers, "certainty": "Low", "why": "small trials", "pmids": [r.pmid],
                "evidence_type": "rct"
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
        "guidelines": [],
        "at_a_glance": {"verdict": "No consistent patient benefit; costs more.",
            "key_numbers": [{"label": "Cost", "value_text": "$3258 robotic vs $1421 laparoscopic", "pmid": a}],
            "main_caveat": "Small trials with short follow-up."},
        "clinical_practice": {"default_choice": "Laparoscopic repair.", "who_may_benefit": "Not shown.",
            "counselling": "Recovery is similar.", "cost_per_case": "not known in India; $3258 vs $1421 in RIVAL"},
        "notable_findings": [{"text": format!("DIRECT reports surgeon experience inconsistently [{a}]."), "pmids": [a]}]
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
    let f = fixture("evidence.json");
    f["studies"]
        .as_array()
        .unwrap()
        .iter()
        .zip(rows())
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
async fn ctx(pool: &PgPool, llm: Arc<ScriptedLlm>) -> RunContext {
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
    let ctx = ctx(&pool, llm.clone()).await;
    let mut written = writer_v3::write_with(&ctx, &ctx.llm, writer_v3::SYSTEM_FULL, &user)
        .await
        .unwrap_or_else(|_| panic!("the scripted writer reply reads"));
    let req = &llm.requests()[0];
    assert_eq!(req.max_tokens, DepthSettings::FOCUSED.writer_max_tokens);
    let mut integrity = IntegrityV3::default();
    writer_v3::check_citations(&mut written, &selected, &mut integrity);
    let allowed = writer_v3::allowed_numbers(&studies, Some(&full));
    let (untraced, checked) =
        writer_v3::trace_numbers(&written, &allowed, |p| selected.iter().any(|s| s == p));
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
