//! Graph smoke (Task 4, run by the owner): the concept-picking call on real
//! runs, offline. This tool never touches the network and never reads a key.
//! The owner sends the bodies with curl in Cloud Shell and brings the raw
//! replies back.
//!
//! ```text
//! graph_smoke --emit-requests <answers dir> <out dir>
//! graph_smoke --check-responses <answers dir> <responses dir> [--usd-inr N]
//! ```
//!
//! * `--emit-requests` reads every `<name>.json` (a stored `AnswerV3`) in the
//!   answers dir and writes `<out dir>/<name>-request.json`: the Messages API
//!   body the graph job sends (system prompt plus the source rule every
//!   `LlmCall` adds, the one user message with empty known names, the output
//!   schema, low effort, `max_tokens` 1200, model `claude-sonnet-5-5`).
//! * `--check-responses` reads `<responses dir>/<name>-response.json` (the raw
//!   API reply, usage included), reads the picks as the job does, runs
//!   `check` against the answer and prints one table row per run. A refusal,
//!   a cut-off reply, a non-JSON reply or a missing file is a row that says
//!   so. Cost is in paise at the app's dev Sonnet price ($2 and $10 per
//!   million tokens), converted at `--usd-inr` (default 88).

use std::path::{Path, PathBuf};

use claude_client::{
    ContentBlock, Effort, Message, MessagesRequest, MessagesResponse, OutputConfig, OutputFormat,
    Role, StopReason,
};
use incisive_app::agent::answer_v3::AnswerV3;
use incisive_app::agent::llm::{OutputSchema, Reply, SOURCE_RULE, read_structured};
use incisive_app::graph::extract::{KnownNames, RawPicks, SYSTEM, check, schema, user_message};
use incisive_app::graph::settings::DEFAULT_MODEL;
use incisive_app::spend::{Prices, cost_paise};

const MAX_TOKENS: u32 = 1200;
const DEFAULT_USD_INR: f64 = 88.0;
/// The app's dev price for Sonnet (config.rs `DEV_MODEL_PRICES`).
const PRICES: &str = r#"{"claude-sonnet-5-5": {"input": 2.0, "output": 10.0}}"#;

fn arg(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1).cloned())
}

/// The value after `flag`, then the next one: `--flag A B`.
fn two(args: &[String], flag: &str) -> Option<(PathBuf, PathBuf)> {
    let i = args.iter().position(|a| a == flag)?;
    Some((
        PathBuf::from(args.get(i + 1)?),
        PathBuf::from(args.get(i + 2)?),
    ))
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let result = if let Some((answers, out)) = two(&args, "--emit-requests") {
        emit_requests(&answers, &out).map(|lines| lines.join("\n"))
    } else if let Some((answers, responses)) = two(&args, "--check-responses") {
        usd_inr(&args).and_then(|rate| check_responses(&answers, &responses, rate))
    } else {
        Err(
            "usage: graph_smoke --emit-requests <answers dir> <out dir> | \
             --check-responses <answers dir> <responses dir> [--usd-inr N]"
                .to_string(),
        )
    };
    match result {
        Ok(text) => println!("{text}"),
        Err(e) => {
            eprintln!("graph_smoke: {e}");
            std::process::exit(1);
        }
    }
}

fn usd_inr(args: &[String]) -> Result<f64, String> {
    match arg(args, "--usd-inr") {
        None => Ok(DEFAULT_USD_INR),
        Some(v) => v
            .trim()
            .parse::<f64>()
            .ok()
            .filter(|r| r.is_finite() && *r > 0.0)
            .ok_or_else(|| "--usd-inr must be a positive number".to_string()),
    }
}

/// The run names in the answers dir: every `<name>.json`, sorted.
fn names(answers: &Path) -> Result<Vec<String>, String> {
    let mut out: Vec<String> = std::fs::read_dir(answers)
        .map_err(|_| "cannot read the answers dir".to_string())?
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let p = e.path();
            (p.extension().is_some_and(|x| x == "json"))
                .then(|| p.file_stem().map(|s| s.to_string_lossy().into_owned()))
                .flatten()
        })
        .collect();
    out.sort();
    if out.is_empty() {
        return Err("no .json answers in the answers dir".to_string());
    }
    Ok(out)
}

fn load(answers: &Path, name: &str) -> Result<AnswerV3, String> {
    let text = std::fs::read_to_string(answers.join(format!("{name}.json")))
        .map_err(|_| format!("cannot read {name}.json"))?;
    serde_json::from_str(&text).map_err(|_| format!("{name}.json is not a stored answer"))
}

/// The request the graph job sends, built as `LlmCall::structured` builds its
/// first call: the source rule appended to the system prompt, one user text
/// message, the schema as `output_config.format`, the step's low effort.
fn request(user: &str) -> MessagesRequest {
    let schema = OutputSchema::new(schema()).with_effort(Effort::Low);
    MessagesRequest {
        model: DEFAULT_MODEL.to_string(),
        max_tokens: MAX_TOKENS,
        system: Some(format!("{SYSTEM}\n\n{SOURCE_RULE}")),
        messages: vec![Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: user.to_string(),
            }],
        }],
        tools: vec![],
        tool_choice: None,
        output_config: Some(OutputConfig {
            format: Some(OutputFormat::JsonSchema {
                schema: schema.schema().clone(),
            }),
            effort: schema.effort(),
        }),
        cache_breakpoints: vec![],
    }
}

fn emit_requests(answers: &Path, out: &Path) -> Result<Vec<String>, String> {
    std::fs::create_dir_all(out).map_err(|_| "cannot create the output directory".to_string())?;
    let mut lines = Vec::new();
    for name in names(answers)? {
        let a = load(answers, &name)?;
        let Some(user) = user_message(&a, &KnownNames::default()) else {
            lines.push(format!("{name}: no answer in this run, skipped"));
            continue;
        };
        let wire = serde_json::to_vec(&request(&user).body())
            .map_err(|_| "cannot serialise the request".to_string())?;
        let path = out.join(format!("{name}-request.json"));
        std::fs::write(&path, &wire).map_err(|_| "cannot write the request".to_string())?;
        lines.push(format!(
            "{name}: request {} bytes -> {}",
            wire.len(),
            path.display()
        ));
    }
    Ok(lines)
}

fn list(v: &[String]) -> String {
    if v.is_empty() {
        "-".to_string()
    } else {
        v.join("; ")
    }
}

/// One table row for a run whose reply could not be used.
fn failed(name: &str, why: &str) -> String {
    format!("{name} | failed: {why}")
}

fn row(name: &str, answers: &Path, responses: &Path, prices: &Prices, rate: f64) -> String {
    let Ok(raw) = std::fs::read_to_string(responses.join(format!("{name}-response.json"))) else {
        return failed(name, "no response file");
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return failed(name, "response is not JSON");
    };
    if v.get("type").and_then(|t| t.as_str()) == Some("error") {
        let kind = v["error"]["type"].as_str().unwrap_or("unknown");
        return failed(name, &format!("api error {kind}"));
    }
    let Ok(resp) = serde_json::from_value::<MessagesResponse>(v) else {
        return failed(name, "not a Messages response");
    };
    let u = resp.usage;
    let tokens = format!("{} in | {} out", u.input_tokens, u.output_tokens);
    let paise = cost_paise(prices, rate, DEFAULT_MODEL, &u)
        .map(|p| p.to_string())
        .unwrap_or_else(|_| "unpriced".to_string());
    let unusable = |why: &str| format!("{name} | failed: {why} | {tokens} | {paise}");
    if resp.stop_reason == Some(StopReason::Refusal) {
        return unusable("refusal");
    }
    if resp.stop_reason == Some(StopReason::MaxTokens) {
        return unusable("truncated (max_tokens)");
    }
    let picks: RawPicks = match read_structured(&Reply::of_response(&resp)) {
        Ok(p) => p,
        Err(f) => return unusable(&format!("unusable reply ({})", f.category)),
    };
    let a = match load(answers, name) {
        Ok(a) => a,
        Err(e) => return unusable(&e),
    };
    let p = check(picks, &a);
    let outcomes: Vec<String> = p
        .outcomes
        .iter()
        .map(|o| format!("{}:{} [{}]", o.row, o.name, o.certainty))
        .collect();
    format!(
        "{name} | {} | {} | {} | {} | {} | {tokens} | {paise}",
        list(&p.diseases),
        list(&p.treatments),
        list(&p.organs),
        list(&outcomes),
        p.dropped,
    )
}

fn check_responses(answers: &Path, responses: &Path, rate: f64) -> Result<String, String> {
    let prices = Prices::from_json(PRICES).map_err(|e| e.to_string())?;
    let mut lines = vec![
        "file | diseases | treatments | organs | outcomes | dropped | input | output | paise"
            .to_string(),
    ];
    for name in names(answers)? {
        lines.push(row(&name, answers, responses, &prices, rate));
    }
    Ok(lines.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURES: [&str; 3] = ["achalasia", "cholecystitis", "hernia"];

    fn fixtures() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/temporal/full")
    }

    fn scratch(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("graph_smoke_{}_{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn reply(stop: &str, text: &str) -> String {
        serde_json::json!({
            "id": "msg_1", "type": "message", "role": "assistant",
            "model": "claude-sonnet-5-5",
            "content": [{"type": "text", "text": text}],
            "stop_reason": stop,
            "usage": {"input_tokens": 1000, "output_tokens": 100}
        })
        .to_string()
    }

    #[test]
    fn emits_three_valid_bodies() {
        let out = scratch("emit");
        emit_requests(&fixtures(), &out).unwrap();
        for n in FIXTURES {
            let v: serde_json::Value = serde_json::from_str(
                &std::fs::read_to_string(out.join(format!("{n}-request.json"))).unwrap(),
            )
            .unwrap();
            assert_eq!(v["model"], "claude-sonnet-5-5");
            assert_eq!(v["max_tokens"], 1200);
            assert_eq!(v["output_config"]["effort"], "low");
            assert_eq!(v["output_config"]["format"]["type"], "json_schema");
            let system = v["system"].as_str().unwrap();
            assert!(system.starts_with(SYSTEM) && system.ends_with(SOURCE_RULE));
            assert!(v.get("tools").is_none() && v.get("tool_choice").is_none());
            assert_eq!(v["messages"].as_array().unwrap().len(), 1);
            let user = v["messages"][0]["content"][0]["text"].as_str().unwrap();
            assert!(user.contains("<source id=\"question\">"));
            assert!(user.contains("<source id=\"known-names\">"));
        }
    }

    #[test]
    fn a_saved_reply_reads_into_the_table() {
        let resp = scratch("table");
        let text = r#"{"diseases":["Inguinal hernia"],"treatments":["Robotic surgery"],"organs":["Abdominal wall","Tail"],"outcomes":[{"row":1,"name":"Pain"},{"row":99,"name":"Far"}]}"#;
        std::fs::write(resp.join("hernia-response.json"), reply("end_turn", text)).unwrap();
        let t = check_responses(&fixtures(), &resp, 88.0).unwrap();
        let lines: Vec<&str> = t.lines().collect();
        assert_eq!(
            lines[0],
            "file | diseases | treatments | organs | outcomes | dropped | input | output | paise"
        );
        let hernia = lines.iter().find(|l| l.starts_with("hernia | ")).unwrap();
        // dropped = organ "Tail" + outcome row 99; $2/$10 per Mtok at 88:
        // (1000*2 + 100*10) / 1e6 USD * 88 * 100 paise = 26.4, rounded up as the app does -> 27.
        assert!(
            hernia.starts_with(
                "hernia | Inguinal hernia | Robotic surgery | Abdominal wall | 1:Pain ["
            ),
            "{hernia}"
        );
        assert!(
            hernia.ends_with("] | 2 | 1000 in | 100 out | 27"),
            "{hernia}"
        );
        assert!(lines.contains(&"achalasia | failed: no response file"));
    }

    #[test]
    fn bad_replies_are_rows_not_panics() {
        let resp = scratch("bad");
        let w = |n: &str, s: String| std::fs::write(resp.join(format!("{n}-response.json")), s);
        w("achalasia", reply("refusal", "")).unwrap();
        w(
            "cholecystitis",
            reply("max_tokens", r#"{"diseases":["Chol"#),
        )
        .unwrap();
        w("hernia", "<html>502</html>".to_string()).unwrap();
        let t = check_responses(&fixtures(), &resp, 88.0).unwrap();
        assert!(
            t.contains("achalasia | failed: refusal | 1000 in | 100 out"),
            "{t}"
        );
        assert!(t.contains("cholecystitis | failed: truncated"), "{t}");
        assert!(t.contains("hernia | failed: response is not JSON"), "{t}");
        w("hernia", reply("end_turn", "not json at all")).unwrap();
        let t = check_responses(&fixtures(), &resp, 88.0).unwrap();
        assert!(t.contains("hernia | failed: unusable reply"), "{t}");
        w(
            "hernia",
            r#"{"type":"error","error":{"type":"overloaded_error"}}"#.into(),
        )
        .unwrap();
        let t = check_responses(&fixtures(), &resp, 88.0).unwrap();
        assert!(
            t.contains("hernia | failed: api error overloaded_error"),
            "{t}"
        );
    }

    #[test]
    fn usd_inr_flag() {
        let a = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(usd_inr(&a(&["x"])).unwrap(), 88.0);
        assert_eq!(usd_inr(&a(&["--usd-inr", "90.5"])).unwrap(), 90.5);
        assert!(usd_inr(&a(&["--usd-inr", "0"])).is_err());
        assert!(usd_inr(&a(&["--usd-inr", "abc"])).is_err());
    }
}
