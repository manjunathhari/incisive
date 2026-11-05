//! Stage S1: reply text and draft text never reach `tracing` output, whether a
//! streamed writer finishes, breaks or cannot store its draft. Own test binary
//! with ONE test, for the reason given in `ask_logging.rs`: the capture
//! subscriber must be the global default before any callsite registers. To
//! prove the capture sees the path, the test asserts its own log lines are
//! present. Gated on `TEST_DATABASE_URL`.

use std::io::Write;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use claude_client::Usage;
use incisive_app::agent::llm::{LlmCall, LlmClient, ScriptedLlm};
use incisive_app::db::{lazy_pool, migrate};
use incisive_app::runs::budget::RunBudget;
use incisive_app::runs::drafts::{self, Part};
use incisive_app::runs::store::{create_run, set_status};
use incisive_app::runs::worker::CancelFlag;
use incisive_app::runs::{RunMode, RunStatus};
use incisive_app::spend::Prices;
use secrecy::SecretString;
use uuid::Uuid;

#[path = "support/stream_llm.rs"]
mod stream_llm;
use stream_llm::{StreamLlm, broken};

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
async fn reply_and_draft_text_are_never_logged() {
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
        "stream-log-{}@example.com",
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

    let marker = format!("zqxreply{}", Uuid::new_v4().simple());
    let reply = format!("{{\"bottom_line\":{{\"text\":\"{marker}\"}}}}");
    let usage = Usage {
        input_tokens: 10,
        output_tokens: 10,
        ..Usage::default()
    };
    let llm: Arc<dyn LlmClient> = Arc::new(
        StreamLlm::new(Arc::new(ScriptedLlm::new().reply_text(&reply, usage)))
            .breaking(vec![broken(&reply, 100)]),
    );
    let prices = Prices::from_json(r#"{"m-test": {"input": 1.0, "output": 5.0}}"#).unwrap();
    let budget = Arc::new(RunBudget::new(
        run,
        uid,
        100_000,
        Instant::now(),
        prices,
        88.0,
    ));
    let call = |hooks| {
        LlmCall::new(
            llm.clone(),
            budget.clone(),
            pool.clone(),
            CancelFlag::default(),
            "m-test",
            "write",
        )
        .streaming(hooks)
    };
    let soon = Instant::now() + Duration::from_secs(60);
    let every = Duration::from_millis(1);

    // A break, with the draft stored.
    let hooks = drafts::streaming_every(pool.clone(), run, Part::Answer, soon, every);
    assert!(call(hooks).text("s", "u", 100).await.is_err());

    // A whole reply, with a draft that cannot be stored.
    let dead = lazy_pool(&SecretString::from(
        "postgres://nobody:x@127.0.0.1:1/none".to_string(),
    ))
    .unwrap();
    let hooks = drafts::streaming_every(dead, run, Part::Answer, soon, every);
    assert!(call(hooks).text("s", "u", 100).await.is_ok());
    // The detached writers end when their calls do.
    tokio::time::sleep(Duration::from_secs(3)).await;

    let logs = String::from_utf8(cap.0.lock().unwrap().clone()).unwrap();
    assert!(
        logs.contains("claude stream broke off"),
        "the break is logged"
    );
    assert!(
        logs.contains("draft could not be stored"),
        "so is the store"
    );
    assert!(!logs.contains(&marker), "reply text in the logs");
}
