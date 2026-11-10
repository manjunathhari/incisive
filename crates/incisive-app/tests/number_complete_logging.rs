//! Q3: the number completeness step never logs the answer's text, the
//! abstracts or the reply. Own test binary with ONE test, for the reason
//! given in `ask_logging.rs`: the capture subscriber must be the global
//! default before any callsite registers. To prove the capture sees the
//! path, the test asserts the step's own log line is present. Gated on
//! `TEST_DATABASE_URL`.

use std::io::Write;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use claude_client::Usage;
use incisive_app::agent::answer_v3::{AnswerV3, Span};
use incisive_app::agent::llm::{LlmCall, ScriptedLlm};
use incisive_app::agent::numbers_complete::{self, Prompt};
use incisive_app::db::{lazy_pool, migrate};
use incisive_app::runs::budget::RunBudget;
use incisive_app::runs::store::{create_run, set_status};
use incisive_app::runs::worker::CancelFlag;
use incisive_app::runs::{RunMode, RunStatus};
use incisive_app::spend::Prices;
use secrecy::SecretString;
use serde_json::json;
use uuid::Uuid;

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

#[tokio::test]
async fn the_step_never_logs_its_texts() {
    let cap = Captured::default();
    let writer = cap.clone();
    tracing::subscriber::set_global_default(
        tracing_subscriber::fmt()
            .with_max_level(tracing::Level::TRACE)
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
    let uid: i64 = sqlx::query_scalar(
        "INSERT INTO incisive.users (email, display_name, role, daily_cap_paise) \
         VALUES ($1, 'Dr Log', 'surgeon', 100000) RETURNING id",
    )
    .bind(format!(
        "numbers-log-{}@example.com",
        Uuid::new_v4().simple()
    ))
    .fetch_one(&pool)
    .await
    .unwrap();
    let run = create_run(&pool, uid, "q", RunMode::Quick, 100_000)
        .await
        .unwrap()
        .unwrap();
    assert!(
        set_status(&pool, run, RunStatus::Running, None)
            .await
            .unwrap()
    );

    let pmid = "11111111";
    let m_text = format!("zqxanswer{}", Uuid::new_v4().simple());
    let m_abs = format!("zqxabstract{}", Uuid::new_v4().simple());
    let m_reply = format!("zqxreply{}", Uuid::new_v4().simple());
    let text = format!("{m_text} had 20 infants [{pmid}].");
    // Find " 20 " (not "20" alone): the random marker before it is hex and
    // can itself contain "20", which put the span in the wrong place.
    let s = text.find(" 20 ").unwrap() + 1;
    let abs = format!("{m_abs}: 10 robotic and 10 open infants.");
    let a: AnswerV3 = serde_json::from_value(json!({
        "version": 3, "question": "q", "mode": "quick", "created_at": "2026-11-09T00:00:00Z",
        "answer": {
            "bottom_line": {"text": text, "certainty": "Low", "certainty_reason": ""},
            "outcomes": [], "studies": [], "conflicts": [],
            "applicability": {"case_mix": "", "india_cost_context": "", "training": ""},
            "would_change": []
        },
        "empty_reason": null, "references": [], "searches": [],
        "integrity": {"dropped_unseen": 0, "dropped_unfetched": 0, "removed_citations": 0,
                      "dropped_numbers": 0, "dropped_studies": 0},
        "stopped_early": null, "cost_paise": 0, "elapsed_s": 0,
        "untraced": [Span::new("bottom_line.text", s, s + 2)]
    }))
    .unwrap();
    let w = a.answer.as_ref().unwrap();
    let p = Prompt {
        items: numbers_complete::collect(w, &a.untraced, &[], |p| p == pmid),
        studies: vec![(pmid.to_string(), abs.clone())],
    };
    assert_eq!(p.items.len(), 1);

    let prices = Prices::from_json(r#"{"m-test": {"input": 1.0, "output": 5.0}}"#).unwrap();
    let budget = Arc::new(RunBudget::new(
        run,
        uid,
        100_000,
        Instant::now(),
        prices,
        88.0,
    ));
    // A reply with the quote and a marker, and one that is unusable.
    for reply in [
        json!({"items": [{"id": "1", "pmid": pmid, "quote": "10 robotic and 10 open", "rule": "sum"}, {"id": "2", "pmid": pmid, "quote": m_reply, "rule": "sum"}]}).to_string(),
        format!("not json {m_reply}"),
    ] {
        let llm = Arc::new(ScriptedLlm::new().reply_text(&reply, Usage::default()));
        let call = LlmCall::new(
            llm,
            budget.clone(),
            pool.clone(),
            CancelFlag::default(),
            "m-test",
            numbers_complete::STEP,
        );
        let out = numbers_complete::call_with(&call, &p, 1, run, std::time::Duration::from_secs(30))
            .await
            .unwrap();
        assert!(out.completed.len() <= 1);
    }
    let logs = String::from_utf8(cap.0.lock().unwrap().clone()).unwrap();
    assert!(
        logs.contains("number completeness done"),
        "the capture sees the step"
    );
    assert!(logs.contains("number completeness reply unusable"));
    for m in [&m_text, &m_abs, &m_reply] {
        assert!(!logs.contains(m.as_str()), "{m} leaked");
    }
}
