//! Quick in four parts (S1, `QUICK_PARALLEL`): an offline comparison of the
//! single Quick writer and the four section writers on the same stored
//! study input, for the owner to run against the real API (Cloud Shell, with
//! only curl). No database, no PubMed, no key; it never calls Claude.
//!
//! ```text
//! quick_parallel_smoke --emit-requests DIR [--fixture DIR] [--max-tokens N]
//! quick_parallel_smoke --check-responses DIR [--fixture DIR]
//! ```
//!
//! The study input is the stored hernia selection (`tests/fixtures/
//! stage4_replay`: `studies.json` for the selection, contradiction notes and
//! search log; each study's abstract rebuilt from the abstract quotes in
//! `evidence.json`), or another directory of the same two files.
//!
//! * `--emit-requests DIR` writes `DIR/single-request.json` (exactly the
//!   single Quick writer's Messages API body) and `DIR/section-<name>-request.json`
//!   for `trials`, `outcomes`, `context` and `summary` (exactly the four
//!   section writers' bodies, as a run with `QUICK_PARALLEL=on` sends them),
//!   and prints the reserves: the one-piece reserve, the four parts' reserve
//!   and the Quick `agent_limit_paise` that keeps today's research room.
//!   Send each body with curl to `/v1/messages` and save the raw response as
//!   `DIR/<same name>-response.json`.
//! * `--check-responses DIR` reads the five responses, builds the single
//!   answer and the merged four-part answer exactly as the run does (the same
//!   wire types, `into_answer`, citation check, evidence labels and number
//!   trace, then the merge checks), prints them side by side (tokens, cost,
//!   stop reasons, per-field counts and sizes, the checks' counts and notes)
//!   and writes `DIR/single-answer.json` and `DIR/parallel-answer.json`.
//!
//! Prices: `MODEL_PRICES` and `USD_INR` when set, else Opus at $4/$20 per
//! million tokens and 88. Model: `AGENT_MODELS`'s `writer`, else Opus 5.5.

#[cfg(not(feature = "ssr"))]
fn main() {}

#[cfg(feature = "ssr")]
fn main() {
    real::main();
}

#[cfg(feature = "ssr")]
mod real {
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use std::time::Instant;

    use claude_client::{ClaudeError, MessagesRequest, MessagesResponse};
    use incisive_app::agent::AgentLlm;
    use incisive_app::agent::answer_v3::{IntegrityV3, SearchEntry, WrittenAnswer};
    use incisive_app::agent::llm::{AgentModels, LlmClient, Reply, read_structured};
    use incisive_app::agent::researcher::Contradiction;
    use incisive_app::agent::writer_v3::{
        self as w, ContextPart, OutcomesPart, Section, StudyInput, SummaryPart, TrialsPart,
        WireAnswer,
    };
    use incisive_app::runs::DepthSettings;
    use incisive_app::runs::budget::RunBudget;
    use incisive_app::spend::{Prices, cost_paise};
    use serde_json::{Value, json};

    const DEFAULT_PRICES: &str = r#"{"claude-opus-5-5": {"input": 4.0, "output": 20.0}}"#;
    /// The stored replay's question.
    const QUESTION: &str = "Does robotic inguinal hernia repair improve pain, complications, recurrence, return to normal activity, or quality of life compared with conventional laparoscopic repair? Assess randomized evidence, operative time, and costs";

    /// Never called: the reserves need an agent client, not a call.
    struct NoCalls;

    #[async_trait::async_trait]
    impl LlmClient for NoCalls {
        async fn call(&self, _: &MessagesRequest) -> Result<MessagesResponse, ClaudeError> {
            Err(ClaudeError::Api {
                status: 0,
                error_type: "offline".into(),
                message: "quick_parallel_smoke makes no calls".into(),
            })
        }
    }

    fn arg(args: &[String], name: &str) -> Option<String> {
        args.iter()
            .position(|a| a == name)
            .and_then(|i| args.get(i + 1).cloned())
    }

    pub fn main() {
        let args: Vec<String> = std::env::args().collect();
        let r = if let Some(dir) = arg(&args, "--emit-requests") {
            emit(&args, Path::new(&dir))
        } else if let Some(dir) = arg(&args, "--check-responses") {
            check(&args, Path::new(&dir))
        } else {
            Err("usage: quick_parallel_smoke --emit-requests DIR | --check-responses DIR [--fixture DIR] [--max-tokens N]".into())
        };
        if let Err(e) = r {
            eprintln!("quick_parallel_smoke: {e}");
            std::process::exit(1);
        }
    }

    fn model() -> Result<String, String> {
        Ok(match std::env::var("AGENT_MODELS") {
            Ok(j) if !j.trim().is_empty() => AgentModels::from_json(&j)?.writer,
            _ => AgentModels::default().writer,
        })
    }

    fn prices_and_rate() -> Result<(Prices, f64), String> {
        let prices = match std::env::var("MODEL_PRICES") {
            Ok(j) if !j.trim().is_empty() => Prices::from_json(&j).map_err(|e| e.to_string())?,
            _ => Prices::from_json(DEFAULT_PRICES).map_err(|e| e.to_string())?,
        };
        let usd_inr: f64 = std::env::var("USD_INR")
            .ok()
            .and_then(|v| v.trim().parse().ok())
            .unwrap_or(88.0);
        Ok((prices, usd_inr))
    }

    fn max_tokens(args: &[String]) -> Result<u32, String> {
        match arg(args, "--max-tokens") {
            None => Ok(DepthSettings::QUICK.writer_max_tokens),
            Some(v) => v
                .trim()
                .parse::<u32>()
                .ok()
                .filter(|n| (1..=64_000).contains(n))
                .ok_or_else(|| "--max-tokens must be 1 to 64000".to_string()),
        }
    }

    fn read_json(p: &Path) -> Result<Value, String> {
        let t = std::fs::read_to_string(p).map_err(|_| format!("cannot read {}", p.display()))?;
        serde_json::from_str(&t).map_err(|_| format!("{} is not JSON", p.display()))
    }

    fn s(v: &Value) -> String {
        v.as_str().unwrap_or_default().to_string()
    }

    /// The stored selection as the writer sees it.
    fn input(args: &[String]) -> Result<(Vec<StudyInput>, String), String> {
        let dir = arg(args, "--fixture")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/stage4_replay")
            });
        let f = read_json(&dir.join("studies.json"))?;
        let ev = read_json(&dir.join("evidence.json"))?;
        let abstract_of = |pmid: &str| -> Option<String> {
            let x = ev["studies"]
                .as_array()?
                .iter()
                .find(|x| x["pmid"].as_str() == Some(pmid))?;
            let mut quotes: Vec<String> = Vec::new();
            for o in x["outcomes"].as_array()? {
                let q = s(&o["quote"]);
                if o["source"] == "abstract" && !q.is_empty() && !quotes.contains(&q) {
                    quotes.push(q);
                }
            }
            (!quotes.is_empty()).then(|| quotes.join(" "))
        };
        let studies: Vec<StudyInput> = f["selected"]
            .as_array()
            .ok_or("studies.json has no selection")?
            .iter()
            .map(|x| StudyInput {
                pmid: s(&x["pmid"]),
                title: s(&x["title"]),
                journal: x["journal"].as_str().map(String::from),
                year: x["year"].as_i64().and_then(|y| i32::try_from(y).ok()),
                pub_types: vec![],
                doi: x["doi"].as_str().map(String::from),
                retracted: false,
                abstract_text: abstract_of(x["pmid"].as_str().unwrap_or_default()),
                design: s(&x["design"]),
                why: s(&x["why"]),
            })
            .collect();
        let contradictions: Vec<Contradiction> = f["possible_contradictions"]
            .as_array()
            .map(|a| {
                a.iter()
                    .map(|c| Contradiction {
                        pmid: s(&c["pmid"]),
                        note: s(&c["note"]),
                    })
                    .collect()
            })
            .unwrap_or_default();
        let searches: Vec<SearchEntry> = f["searches"]
            .as_array()
            .map(|a| {
                a.iter()
                    .map(|x| SearchEntry {
                        query: s(&x["query"]),
                        source: "PubMed".into(),
                        hits: x["hits"].as_u64().unwrap_or(0),
                    })
                    .collect()
            })
            .unwrap_or_default();
        let user = w::writer_user(QUESTION, &studies, &contradictions, &searches);
        Ok((studies, user))
    }

    fn file(name: &str, kind: &str) -> String {
        format!("{name}-{kind}.json")
    }

    fn names() -> Vec<(String, Option<Section>)> {
        std::iter::once(("single".to_string(), None))
            .chain(
                Section::ALL
                    .iter()
                    .map(|s| (format!("section-{}", s.name()), Some(*s))),
            )
            .collect()
    }

    fn emit(args: &[String], dir: &Path) -> Result<(), String> {
        std::fs::create_dir_all(dir).map_err(|_| "cannot create the output directory")?;
        let max = max_tokens(args)?;
        let model = model()?;
        let (prices, usd_inr) = prices_and_rate()?;
        let (studies, user) = input(args)?;
        for (name, sec) in names() {
            let req = match sec {
                None => w::quick_request(&model, &user, max),
                Some(s) => w::section_request(&model, &user, s, max),
            };
            let body =
                serde_json::to_vec(&req.body()).map_err(|_| "cannot serialise the request")?;
            let p = dir.join(file(&name, "request"));
            std::fs::write(&p, &body).map_err(|_| "cannot write the request")?;
            println!(
                "{name}: max_tokens {} · request {} bytes -> {}",
                req.max_tokens,
                body.len(),
                p.display()
            );
        }
        let budget = Arc::new(RunBudget::new(
            uuid::Uuid::nil(),
            0,
            0,
            Instant::now(),
            prices,
            usd_inr,
        ));
        let llm = AgentLlm {
            client: Arc::new(NoCalls),
            models: AgentModels {
                writer: model.clone(),
                ..AgentModels::default()
            },
        };
        let single = w::reserve_for(&llm, &budget, QUESTION, max).ok_or("writer model unpriced")?;
        let parts =
            w::sections_reserve(&llm, &budget, QUESTION, max).ok_or("writer model unpriced")?;
        let room = DepthSettings::QUICK.agent_limit_paise - single;
        println!(
            "{} studies · user message {} bytes",
            studies.len(),
            user.len()
        );
        println!(
            "reserves (worst case, no cache hit): one piece {single} paise · four parts {parts} paise"
        );
        println!(
            "today's research room under the default Quick limit ({}): {room} paise",
            DepthSettings::QUICK.agent_limit_paise
        );
        println!(
            "Quick agent_limit_paise that keeps it with QUICK_PARALLEL=on: at least {}",
            parts + room
        );
        Ok(())
    }

    /// The response's reply, its usage line, or why there is none.
    fn reply(dir: &Path, name: &str, model: &str) -> Result<(Reply, String), String> {
        let v = read_json(&dir.join(file(name, "response")))?;
        if v.get("type").and_then(Value::as_str) == Some("error") {
            return Err(format!(
                "api error {}",
                v["error"]["type"].as_str().unwrap_or("unknown")
            ));
        }
        let resp: MessagesResponse =
            serde_json::from_value(v).map_err(|_| "not a Messages response".to_string())?;
        let (prices, usd_inr) = prices_and_rate()?;
        let u = resp.usage;
        let cost = cost_paise(&prices, usd_inr, model, &u)
            .map(|p| format!("\u{20b9}{:.2}", p as f64 / 100.0))
            .unwrap_or_else(|_| "unpriced".into());
        let line = format!(
            "{} in, {} out · cost {cost} · stop {:?}",
            u.input_tokens, u.output_tokens, resp.stop_reason
        );
        Ok((Reply::of_response(&resp), line))
    }

    fn part<T: serde::de::DeserializeOwned>(r: &Reply) -> Result<T, String> {
        read_structured::<T>(r).map_err(|f| format!("unreadable ({})", f.category))
    }

    /// The run's checks on a written answer.
    fn checked(mut a: WrittenAnswer, studies: &[StudyInput]) -> (WrittenAnswer, IntegrityV3) {
        let selected: Vec<String> = studies.iter().map(|s| s.pmid.clone()).collect();
        let mut i = IntegrityV3::default();
        w::check_citations(&mut a, &selected, &mut i);
        w::evidence_types(&mut a, |p| {
            studies
                .iter()
                .find(|s| s.pmid == p)
                .and_then(|s| w::design_class(&s.design))
        });
        let allowed = w::allowed_numbers(studies, None);
        let (untraced, checked) =
            w::trace_numbers(&a, &allowed, |p| selected.iter().any(|s| s == p), false);
        i.numbers_checked = checked;
        i.numbers_untraced = u32::try_from(untraced.len()).unwrap_or(u32::MAX);
        (a, i)
    }

    fn size(v: &Value, k: &str) -> (usize, usize) {
        let x = &v[k];
        let n = x.as_array().map_or(usize::from(!x.is_null()), Vec::len);
        (n, x.to_string().len())
    }

    fn check(args: &[String], dir: &Path) -> Result<(), String> {
        let model = model()?;
        let (studies, _) = input(args)?;
        let mut answers: Vec<(&str, Option<(WrittenAnswer, u32)>)> = Vec::new();
        // The single writer.
        println!("== single writer ==");
        let single = match reply(dir, "single", &model) {
            Ok((r, line)) => {
                println!("  {line}");
                part::<WireAnswer>(&r).map(WireAnswer::into_answer)
            }
            Err(e) => Err(e),
        };
        match &single {
            Ok(_) => println!("  read"),
            Err(e) => println!("  failed: {e}"),
        }
        answers.push(("single", single.ok()));
        // The four parts.
        println!("== four parts ==");
        let mut replies: Vec<Option<Reply>> = Vec::new();
        for s in Section::ALL {
            let name = format!("section-{}", s.name());
            match reply(dir, &name, &model) {
                Ok((r, line)) => {
                    println!("  {}: {line}", s.name());
                    replies.push(Some(r));
                }
                Err(e) => {
                    println!("  {}: failed: {e}", s.name());
                    replies.push(None);
                }
            }
        }
        // Review M4: truncation measured before release.
        let stops: Vec<String> = Section::ALL
            .iter()
            .zip(&replies)
            .map(|(s, r)| match r {
                Some(r) if r.truncated() => format!(
                    "{} CUT OFF at max_tokens {} ({} out)",
                    s.name(),
                    s.max_tokens(max_tokens(args).unwrap_or(20_000)),
                    r.output_tokens
                ),
                Some(r) => format!("{} {:?}", s.name(), r.stop_reason),
                None => format!("{} no reply", s.name()),
            })
            .collect();
        println!("  stop reasons: {}", stops.join(" · "));
        let merged = (|| -> Result<(WrittenAnswer, u32), String> {
            let get = |i: usize| replies[i].as_ref().ok_or("missing".to_string());
            let t: TrialsPart = part(get(0)?).map_err(|e| format!("trials {e}"))?;
            let o: OutcomesPart = part(get(1)?).map_err(|e| format!("outcomes {e}"))?;
            let c: ContextPart = part(get(2)?).map_err(|e| format!("context {e}"))?;
            let s: SummaryPart = part(get(3)?).map_err(|e| format!("summary {e}"))?;
            Ok(w::merge_sections(t, o, c, s).into_answer())
        })();
        match &merged {
            Ok(_) => println!("  merged"),
            Err(e) => println!("  not merged: {e} (a run would write it in one piece)"),
        }
        answers.push(("parallel", merged.ok()));
        // Side by side.
        let mut rows: Vec<(String, Vec<String>)> = Vec::new();
        let mut push = |label: String, i: usize, v: String| {
            if let Some(r) = rows.iter_mut().find(|(l, _)| *l == label) {
                r.1[i] = v;
            } else {
                let mut cells = vec!["-".to_string(); 2];
                cells[i] = v;
                rows.push((label, cells));
            }
        };
        for (i, (name, a)) in answers.iter().enumerate() {
            let Some((a, defaulted)) = a else { continue };
            let (a, integrity) = checked(a.clone(), &studies);
            let v = serde_json::to_value(&a).map_err(|_| "cannot serialise an answer")?;
            for k in [
                "bottom_line",
                "outcomes",
                "studies",
                "conflicts",
                "applicability",
                "would_change",
                "at_a_glance",
                "clinical_practice",
                "notable_findings",
            ] {
                let (n, bytes) = size(&v, k);
                push(format!("{k} (items, bytes)"), i, format!("{n}, {bytes}"));
            }
            let numbers: usize = a.outcomes.iter().map(|o| o.numbers.len()).sum();
            push("structured numbers".into(), i, numbers.to_string());
            push(
                "bottom-line certainty".into(),
                i,
                a.bottom_line.certainty.as_str().into(),
            );
            push("labels defaulted".into(), i, defaulted.to_string());
            push(
                "citations removed".into(),
                i,
                integrity.removed_citations.to_string(),
            );
            push(
                "numbers / trial rows dropped".into(),
                i,
                format!(
                    "{} / {}",
                    integrity.dropped_numbers, integrity.dropped_studies
                ),
            );
            push(
                "numbers untraced of checked".into(),
                i,
                format!(
                    "{} of {}",
                    integrity.numbers_untraced, integrity.numbers_checked
                ),
            );
            let notes = w::merge_checks(&a);
            push("consistency notes".into(), i, notes.len().to_string());
            for n in &notes {
                println!("  [{name}] {n}");
            }
            let out = dir.join(format!("{name}-answer.json"));
            std::fs::write(
                &out,
                serde_json::to_string_pretty(
                    &json!({"answer": v, "integrity": integrity, "notes": notes}),
                )
                .unwrap_or_default(),
            )
            .map_err(|_| "cannot write an answer")?;
            println!("  saved {}", out.display());
        }
        println!("\n{:<34} {:<22} {:<22}", "", "single", "four parts");
        for (label, cells) in rows {
            println!("{label:<34} {:<22} {:<22}", cells[0], cells[1]);
        }
        Ok(())
    }
}
