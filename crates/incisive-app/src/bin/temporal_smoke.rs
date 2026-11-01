//! Temporal smoke (T5 proof, run by the owner): the real expert-writer
//! prompt and `ExpertBoardV1` schema on the three stored fixtures, with no
//! database and no PubMed. One Claude call per fixture.
//!
//! ```text
//! temporal_smoke [--out DIR] [--only NAME] [--max-tokens N] [--print-schema]
//! ```
//!
//! For each fixture (`tests/fixtures/temporal/full/<name>.json` when it
//! exists, else the slim `tests/fixtures/temporal/<name>.json`) it builds
//! the T1 table, sends exactly the request the after-run job sends
//! (`temporal_writer::request`), runs the code checks and prints: status,
//! tokens, seconds, a cost estimate, the flag counts and the three At a
//! glance lines. Each board (with its flags) is saved as
//! `<out>/<name>-board.json`.
//!
//! Needs `ANTHROPIC_API_KEY` in the environment (the owner's script reads it
//! from Secret Manager; it is never printed). The model is `AGENT_MODELS`'s
//! `temporal` key, else Opus 5.5. The cost estimate uses `MODEL_PRICES` and
//! `USD_INR` when set, else Opus at $4/$20 per million tokens and 88.
//! `--print-schema` prints the schema as JSON and makes no call.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use incisive_app::agent::answer_v3::AnswerV3;
use incisive_app::agent::llm::AgentModels;
use incisive_app::agent::temporal_writer::{self as writer, CheckContext};
use incisive_app::spend::{Prices, cost_paise};
use incisive_app::temporal::{TemporalInput, build};
use secrecy::SecretString;
use serde_json::json;

const FIXTURES: [&str; 3] = ["cholecystitis", "hernia", "achalasia"];
const DEFAULT_PRICES: &str = r#"{"claude-opus-5-5": {"input": 4.0, "output": 20.0}}"#;

fn arg(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1).cloned())
}

fn fixture(name: &str) -> Result<(String, &'static str), String> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/temporal");
    let full = dir.join("full").join(format!("{name}.json"));
    if let Ok(s) = std::fs::read_to_string(&full) {
        return Ok((s, "full"));
    }
    std::fs::read_to_string(dir.join(format!("{name}.json")))
        .map(|s| (s, "slim"))
        .map_err(|_| format!("no fixture for {name}"))
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--print-schema") {
        println!("{}", writer::schema());
        return;
    }
    if let Err(e) = run(&args).await {
        eprintln!("temporal_smoke: {e}");
        std::process::exit(1);
    }
}

async fn run(args: &[String]) -> Result<(), String> {
    let out = PathBuf::from(arg(args, "--out").unwrap_or_else(|| "temporal-smoke".into()));
    std::fs::create_dir_all(&out).map_err(|_| "cannot create the output directory")?;
    let max_tokens: u32 = match arg(args, "--max-tokens") {
        Some(v) => v.parse().map_err(|_| "--max-tokens must be a number")?,
        None => writer::DEFAULT_MAX_TOKENS,
    };
    let only = arg(args, "--only");
    let key = std::env::var("ANTHROPIC_API_KEY")
        .ok()
        .filter(|k| !k.trim().is_empty())
        .ok_or("missing ANTHROPIC_API_KEY")?;
    let client = claude_client::Client::new(SecretString::from(key.trim().to_string()))
        .with_request_timeout(writer::TIME_LIMIT + Duration::from_secs(10));
    let model = match std::env::var("AGENT_MODELS") {
        Ok(j) if !j.trim().is_empty() => AgentModels::from_json(&j)?.temporal,
        _ => AgentModels::default().temporal,
    };
    let prices = match std::env::var("MODEL_PRICES") {
        Ok(j) if !j.trim().is_empty() => Prices::from_json(&j).map_err(|e| e.to_string())?,
        _ => Prices::from_json(DEFAULT_PRICES).map_err(|e| e.to_string())?,
    };
    let usd_inr: f64 = std::env::var("USD_INR")
        .ok()
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(88.0);
    println!(
        "model {model} · max_tokens {max_tokens} · time limit {}s",
        writer::TIME_LIMIT.as_secs()
    );
    for name in FIXTURES {
        if only.as_deref().is_some_and(|o| o != name) {
            continue;
        }
        let (json_text, kind) = fixture(name)?;
        let body: serde_json::Value =
            serde_json::from_str(&json_text).map_err(|_| "bad fixture")?;
        let input = TemporalInput::from_json(&json_text).map_err(|_| "bad fixture")?;
        let table = build(&input);
        let answer_text = match serde_json::from_value::<AnswerV3>(body.clone()) {
            Ok(a) => a.plain_text(),
            Err(_) => writer::render_input(&input),
        };
        let user = writer::user(&answer_text, &table, &input);
        let req = writer::request(&model, &user, max_tokens);
        println!(
            "\n== {name} ({kind} body; prompt {} KB) ==",
            user.len() / 1024
        );
        let started = Instant::now();
        let resp = match tokio::time::timeout(writer::TIME_LIMIT, client.create_message(&req)).await
        {
            Err(_) => {
                println!("status: failed (time, {}s)", started.elapsed().as_secs());
                continue;
            }
            Ok(Err(e)) => {
                // The error kind only: never the request.
                let kind = match &e {
                    claude_client::ClaudeError::Api {
                        status, error_type, ..
                    } => {
                        format!("api {status} {error_type}")
                    }
                    claude_client::ClaudeError::RateLimited { .. } => "rate limited".into(),
                    claude_client::ClaudeError::Overloaded => "overloaded".into(),
                    claude_client::ClaudeError::Transport(_) => "transport".into(),
                    claude_client::ClaudeError::Decode { status, .. } => format!("decode {status}"),
                };
                println!("status: failed ({kind})");
                continue;
            }
            Ok(Ok(r)) => r,
        };
        let secs = started.elapsed().as_secs_f64();
        let u = resp.usage;
        let cost = cost_paise(&prices, usd_inr, &model, &u)
            .map(|p| format!("\u{20b9}{:.2}", p as f64 / 100.0))
            .unwrap_or_else(|_| "unpriced".into());
        println!(
            "tokens: {} in, {} out · {secs:.0}s · cost about {cost} · stop {:?}",
            u.input_tokens, u.output_tokens, resp.stop_reason
        );
        let board = match writer::read_board(&resp) {
            Ok(b) => b,
            Err(kind) => {
                println!("status: failed ({kind})");
                continue;
            }
        };
        let refs = writer::reference_numbers(&input);
        let markers = writer::marker_reference_numbers(&input, &table);
        let flags = writer::check(
            &board,
            &CheckContext {
                answer_text: &answer_text,
                table: &table,
                references: &refs,
                marker_references: &markers,
            },
        );
        println!("status: done · topic: {}", board.topic);
        println!("flags ({}): {}", flags.total(), flags.counts());
        for f in flags
            .not_traced
            .iter()
            .chain(&flags.cost_country)
            .chain(&flags.unknown_references)
            .chain(&flags.unknown_ids)
            .chain(&flags.meta_called_trial)
            .chain(&flags.glance_stronger)
            .chain(&flags.shape)
            .take(40)
        {
            println!("  - {}: {}", f.field, f.text);
        }
        println!("At a glance");
        println!("  original: {}", board.at_a_glance.original_quote);
        println!("  expert:   {}", board.at_a_glance.expert);
        println!("  today:    {}", board.at_a_glance.today);
        let saved = json!({
            "fixture": name, "body": kind, "model": model,
            "input_tokens": u.input_tokens, "output_tokens": u.output_tokens,
            "seconds": secs, "board": board, "flags": flags,
        });
        let path = out.join(format!("{name}-board.json"));
        std::fs::write(
            &path,
            serde_json::to_string_pretty(&saved).unwrap_or_default(),
        )
        .map_err(|_| "cannot write the board")?;
        println!("saved {}", path.display());
    }
    Ok(())
}
