//! A reply that cannot be read is logged without its content (Fix A, item 3):
//! step, model, stop reason, whether a ToolUse block was present, its size
//! and the serde error's category and position. Never reply text.
//!
//! Own test binary with ONE test, so the capture subscriber is the global
//! default before any callsite registers (see `ask_logging.rs`). Gated on
//! `TEST_DATABASE_URL`.

use std::io::Write;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use claude_client::{ContentBlock, StopReason, Usage};
use incisive_app::agent::llm::{LlmCall, ScriptedLlm, StepError, SubmitTool};
use incisive_app::db::{lazy_pool, migrate};
use incisive_app::runs::RunMode;
use incisive_app::runs::budget::RunBudget;
use incisive_app::runs::store::create_run;
use incisive_app::runs::worker::CancelFlag;
use incisive_app::spend::{CapCheck, CapsConfig, Prices, reserve};
use secrecy::SecretString;
use serde::Deserialize;
use serde_json::json;
use uuid::Uuid;

const SECRET: &str = "SECRET-REPLY-MARKER";

#[derive(Clone, Default)]
struct Captured(Arc<Mutex<Vec<u8>>>);

impl Write for Captured {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct Out {
    answer: String,
}

fn usage() -> Usage {
    Usage {
        input_tokens: 10,
        output_tokens: 10,
    }
}

#[tokio::test]
async fn unreadable_replies_are_logged_without_content() {
    let captured = Captured::default();
    let writer = captured.clone();
    tracing::subscriber::set_global_default(
        tracing_subscriber::fmt()
            .with_max_level(tracing::Level::TRACE)
            .with_ansi(false)
            .with_writer(move || writer.clone())
            .finish(),
    )
    .expect("first and only global subscriber in this binary");

    let Ok(url) = std::env::var("TEST_DATABASE_URL") else {
        eprintln!("SKIP: TEST_DATABASE_URL not set");
        return;
    };
    let pool = lazy_pool(&SecretString::from(url)).expect("lazy pool");
    migrate(&pool).await.expect("migrate");

    let email = format!("diag-{}@example.com", Uuid::new_v4());
    let uid: i64 = sqlx::query_scalar(
        "INSERT INTO incisive.users (email, display_name, role, daily_cap_paise) \
         VALUES ($1, 'T', 'surgeon', 100000) RETURNING id",
    )
    .bind(&email)
    .fetch_one(&pool)
    .await
    .unwrap();
    let run = create_run(&pool, uid, "q", RunMode::Focused, 10_000)
        .await
        .unwrap()
        .unwrap();
    let user = incisive_app::identity::find_user(&pool, &email)
        .await
        .unwrap()
        .unwrap();
    let caps = CapsConfig {
        global_monthly_paise: i64::MAX / 4,
    };
    let c = reserve(&pool, &user, &caps, 10_000, run, "run")
        .await
        .unwrap();
    assert!(matches!(c, CapCheck::Ok { .. }));
    let prices = Prices::from_json(r#"{"m-test": {"input": 1.0, "output": 5.0}}"#).unwrap();
    let budget = Arc::new(RunBudget::new(
        run,
        uid,
        10_000,
        Instant::now(),
        prices,
        88.0,
    ));

    // First: a ToolUse block whose input is the wrong type (a data error).
    // Second: no ToolUse block, text that is not JSON (a syntax error).
    let llm = Arc::new(
        ScriptedLlm::new()
            .reply_tool(json!({"answer": 5, "note": SECRET}), usage())
            .reply_blocks(
                vec![ContentBlock::Text {
                    text: format!("I think {SECRET} is the answer"),
                }],
                Some(StopReason::EndTurn),
                usage(),
            ),
    );
    let tool = SubmitTool::new(
        "Submit the answer.",
        json!({"type": "object", "properties": {"answer": {"type": "string"}}}),
    );
    let call = LlmCall::new(
        llm.clone(),
        budget,
        pool.clone(),
        CancelFlag::default(),
        "m-test",
        "screen",
    );
    let err = call
        .submit::<Out>("sys", "user", &tool, 100)
        .await
        .unwrap_err();
    assert_eq!(err, StepError::Parse);
    assert_eq!(llm.requests().len(), 2);

    let logs = String::from_utf8(captured.0.lock().unwrap().clone()).unwrap();
    assert!(!logs.contains(SECRET), "{logs}");
    assert!(!logs.contains("I think"), "{logs}");
    let first = logs
        .lines()
        .find(|l| l.contains("reply had no usable JSON; retrying once"))
        .unwrap_or_else(|| panic!("{logs}"));
    for want in [
        "step=\"screen\"",
        "model=m-test",
        "stop_reason=Some(ToolUse)",
        "tool_use=true",
        "error_category=\"data\"",
        "reply_chars=",
        "error_line=",
        "error_column=",
    ] {
        assert!(first.contains(want), "{want} missing: {first}");
    }
    let second = logs
        .lines()
        .find(|l| l.contains("reply had no usable JSON after retry"))
        .unwrap_or_else(|| panic!("{logs}"));
    for want in [
        "stop_reason=Some(EndTurn)",
        "tool_use=false",
        "error_category=\"syntax\"",
        "error_line=1",
    ] {
        assert!(second.contains(want), "{want} missing: {second}");
    }
}
