//! The ask flow: everything between a question and a stored turn.
//!
//! `ask` runs its checks in order (length, safety and personal detection,
//! identifiers, run ownership and readiness, daily limit, busy), stores a
//! `pending` turn and starts a task that runs independently of the caller:
//! embed the query, retrieve, stream Gemma's reply into the turn, check it,
//! finish it. The browser can leave; the turn still finishes, and a dead
//! instance's turn is resolved by the stale sweep.
//!
//! Safety: every refusal of a question says whether it carries the safety
//! line, including one too long to accept and a database failure
//! (`Unavailable`). The exceptions are `Empty` (no question) and `NotFound`
//! (nothing about the run may be implied). `Failed` is only for the calls
//! that have no question to look at.
//!
//! Nothing here logs question, reply or passage text; logs carry ids and
//! fixed codes, and a panic's payload is dropped unread.

use std::future::Future;
use std::panic::AssertUnwindSafe;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures_util::FutureExt;
use serde::Serialize;
use serde_json::{Value, json};
use uuid::Uuid;

use super::checks::{self, Checks};
use super::gemma::{GemmaError, StreamEnd};
use super::prompt::{self, GroundedPassages};
use super::store::{self, NewTurn, PendingTurn, RecordRef, RunFacts, TurnRow};
use super::{ChatMode, comparison, job, lines, passages, retrieve};
use crate::ask::blocked_message;
use crate::audit::{self, Event};
use crate::deid;
use crate::identity::User;
use crate::server::AppState;

/// Gemma's limit for one reply.
pub const GEMMA_LIMIT: Duration = Duration::from_secs(60);
/// The question's embedding; past this the search is words only.
pub const EMBED_LIMIT: Duration = Duration::from_secs(3);
/// The reply so far is stored at most this often.
pub const WRITE_EVERY: Duration = Duration::from_millis(500);
/// Longest question, in characters.
pub const MAX_QUESTION_CHARS: usize = 1000;
/// Replies in flight across all users at which a new one is refused.
pub const MAX_PENDING: i64 = 3;
/// Key in an `unavailable` turn's `checks`: Gemma refused for load, so the
/// reader gets the busy line rather than the unavailable one.
const BUSY_KEY: &str = "gemma_busy";

/// Whether chat can be used on a run.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ChatState {
    /// No Gemma client, or this record's passages could not be built.
    Unavailable,
    /// A Quick run, or no finished opinion and no record to come.
    NotOffered,
    /// The passages have not been built yet.
    GettingReady,
    Ready,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChatError {
    /// Plain: nothing about the run may be implied.
    NotFound,
    NotOffered {
        safety_line: bool,
    },
    /// Plain: there is no question.
    Empty,
    TooLong {
        safety_line: bool,
    },
    Identifiers {
        message: String,
        safety_line: bool,
    },
    InFlight {
        safety_line: bool,
    },
    DailyLimit {
        safety_line: bool,
    },
    Busy {
        safety_line: bool,
    },
    Unavailable {
        safety_line: bool,
    },
    /// A store error on a call that reads no question.
    Failed,
    /// Feedback other than 1 or -1.
    BadFeedback,
    /// A readable question with a mode that is not one.
    BadMode {
        safety_line: bool,
    },
    /// A turn that is not `done`, or whose run has no record to file under.
    NotSuggestable,
}

/// A question accepted; its reply is on the way.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Asked {
    pub turn_id: i64,
    pub safety_line: bool,
}

/// What a citation points at.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnSource {
    Passage(i64),
    Qa(i64),
}

/// `passage_ids` keeps every hit in citation order so position `i` is `[i+1]`,
/// which means a Q&A must hold a place too: it is stored as its id negated.
/// Passage ids are always positive (`BIGSERIAL`), so a lookup of a passage id
/// in `passage_ids` can never match a Q&A.
fn source_of(stored: i64) -> TurnSource {
    if stored < 0 {
        TurnSource::Qa(-stored)
    } else {
        TurnSource::Passage(stored)
    }
}

fn stored_source(h: &retrieve::Hit) -> i64 {
    match (h.passage_id, h.qa_id) {
        (Some(p), _) => p,
        (None, Some(q)) => -q,
        (None, None) => 0,
    }
}

/// One turn as shown to its own user. The fixed lines are chosen here, at
/// display time, from the stored flags and status.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct TurnView {
    pub id: i64,
    pub seq: i32,
    pub question: String,
    /// The text to show: the reply so far, or the fixed line for a turn that
    /// has no answer. `status` of `partial` means it was cut short.
    pub reply: String,
    pub status: String,
    pub mode: String,
    /// Position `i` is citation `[i+1]`.
    pub sources: Vec<TurnSource>,
    pub checks: Option<Checks>,
    /// Fixed lines to show with this turn, most urgent first.
    pub lines: Vec<&'static str>,
    pub safety_line: bool,
    pub personal: bool,
    pub feedback: Option<i16>,
    pub suggested: bool,
    pub latency_ms: Option<i32>,
}

/// A run's chat: its state, starters when it is ready, and this user's turns.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ChatView {
    pub state: ChatState,
    pub starters: Vec<String>,
    pub conversation_id: Option<i64>,
    pub turns: Vec<TurnView>,
}

fn view_of(t: TurnRow) -> TurnView {
    let busy = is_busy(t.checks.as_ref());
    let reply = match t.status.as_str() {
        "not_covered" => lines::NOT_COVERED.to_string(),
        "unavailable" if busy => lines::BUSY.to_string(),
        "unavailable" => lines::UNAVAILABLE.to_string(),
        _ => t.reply,
    };
    let mut fixed = Vec::new();
    if t.safety_line {
        fixed.push(lines::WARNING_LINE);
    }
    if t.personal {
        fixed.push(lines::SEE_YOUR_SURGEON);
    }
    TurnView {
        id: t.id,
        seq: t.seq,
        question: t.question,
        reply,
        status: t.status,
        mode: t.mode,
        sources: t.passage_ids.into_iter().map(source_of).collect(),
        // The busy marker is not a flag: test for it first, because any JSON
        // object now reads as `Checks`.
        checks: if busy {
            None
        } else {
            t.checks.and_then(|c| serde_json::from_value(c).ok())
        },
        lines: fixed,
        safety_line: t.safety_line,
        personal: t.personal,
        feedback: t.feedback,
        suggested: t.suggested,
        latency_ms: t.latency_ms,
    }
}

fn is_busy(checks: Option<&Value>) -> bool {
    checks
        .and_then(|c| c.get(BUSY_KEY))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

/// The rules for a run's state. Not offered comes first: if there is nothing
/// to chat about, Gemma's health does not matter.
fn decide(f: &RunFacts, has_gemma: bool) -> ChatState {
    if f.mode != "focused" {
        return ChatState::NotOffered;
    }
    if f.record.is_none() {
        let opinion_done = f.opinion.as_deref() == Some("done");
        let record_coming = matches!(f.memory_job.as_deref(), Some("pending" | "running"));
        if !(opinion_done && record_coming) {
            return ChatState::NotOffered;
        }
    }
    if !has_gemma {
        return ChatState::Unavailable;
    }
    match f.prep.as_deref() {
        Some("ok") => ChatState::Ready,
        Some(_) => ChatState::Unavailable,
        None => ChatState::GettingReady,
    }
}

/// The run's chat for its own user; anyone else gets `NotFound`, the owner
/// included (the owner reads others' chats only in `/chats`).
pub async fn state_for(state: &AppState, user: &User, run_id: Uuid) -> Result<ChatView, ChatError> {
    let facts = store::run_facts(&state.pool, user.id, run_id)
        .await
        .map_err(|_| store_error("run_facts"))?
        .ok_or(ChatError::NotFound)?;
    let chat_state = decide(&facts, state.gemma.is_some());
    let (conversation_id, rows) = store::conversation_for_run(&state.pool, user.id, run_id)
        .await
        .map_err(|_| store_error("turns"))?;
    let starters = match (chat_state, facts.record) {
        (ChatState::Ready, Some(rec)) => store::record_bodies(&state.pool, rec.id)
            .await
            .map(|r| passages::starters(&r.answer))
            .map_err(|_| store_error("record"))?,
        _ => Vec::new(),
    };
    Ok(ChatView {
        state: chat_state,
        starters,
        conversation_id,
        turns: rows.into_iter().map(view_of).collect(),
    })
}

/// One turn for its own user.
pub async fn turn_view(state: &AppState, user: &User, turn_id: i64) -> Result<TurnView, ChatError> {
    store::turn_for_user(&state.pool, user.id, turn_id)
        .await
        .map_err(|_| store_error("turn"))?
        .map(view_of)
        .ok_or(ChatError::NotFound)
}

/// Records this user's thumbs on their own turn. Anyone else's turn is
/// `NotFound`.
pub async fn give_feedback(
    state: &AppState,
    user: &User,
    turn_id: i64,
    value: i64,
) -> Result<(), ChatError> {
    let value = match value {
        1 => 1i16,
        -1 => -1i16,
        _ => return Err(ChatError::BadFeedback),
    };
    match store::set_feedback(&state.pool, user.id, turn_id, value).await {
        Ok(true) => Ok(()),
        Ok(false) => Err(ChatError::NotFound),
        Err(_) => Err(store_error("feedback")),
    }
}

/// Files the user's own `done` turn as a suggested Q&A for the owner to
/// review. Returns the Q&A's id; asking twice returns the same one.
pub async fn suggest(state: &AppState, user: &User, turn_id: i64) -> Result<i64, ChatError> {
    match store::suggest_turn(&state.pool, user.id, turn_id).await {
        Ok(store::SuggestOutcome::Filed { qa_id, .. }) => Ok(qa_id),
        Ok(store::SuggestOutcome::NotFound) => Err(ChatError::NotFound),
        Ok(_) => Err(ChatError::NotSuggestable),
        Err(_) => Err(store_error("suggest")),
    }
}

/// The source behind a citation tap, if it is in one of the user's turns.
pub async fn cited_source(
    state: &AppState,
    user: &User,
    stored_id: i64,
) -> Result<store::CitedSource, ChatError> {
    store::cited_source(&state.pool, user.id, stored_id)
        .await
        .map_err(|_| store_error("source"))?
        .ok_or(ChatError::NotFound)
}

fn store_error(code: &'static str) -> ChatError {
    tracing::warn!(code, "chat store error");
    ChatError::Failed
}

/// Accepts a question. Returns once the turn is stored; the reply is written
/// by a task that outlives the caller.
pub async fn ask(
    state: &AppState,
    user: &User,
    run_id: Uuid,
    question: &str,
    mode: ChatMode,
) -> Result<Asked, ChatError> {
    ask_limited(state, user, run_id, question, mode, GEMMA_LIMIT).await
}

/// [`ask`] with its Gemma limit given, so a test of the cut need not take 60 s.
#[doc(hidden)]
pub async fn ask_limited(
    state: &AppState,
    user: &User,
    run_id: Uuid,
    question: &str,
    mode: ChatMode,
    limit: Duration,
) -> Result<Asked, ChatError> {
    let text = question.trim();
    if text.is_empty() {
        return Err(ChatError::Empty);
    }
    // Read before the length check, over the whole text: a very long question
    // may carry the symptom past any point.
    let safety_line = lines::warning_symptoms(text);
    if text.chars().count() > MAX_QUESTION_CHARS {
        return Err(ChatError::TooLong { safety_line });
    }

    let personal = lines::personal(text);
    // From here on a store failure is "unavailable" with the line, not a
    // bare failure: the person may have symptoms.
    let unavailable = |code: &'static str| {
        tracing::warn!(code, "chat ask store error");
        ChatError::Unavailable { safety_line }
    };

    let findings = deid::scan(question);
    if !findings.is_empty() {
        // Fails closed like the Ask page: nothing is stored or sent either way.
        audit::append(
            &state.pool,
            Some(Uuid::new_v4()),
            Some(user.id),
            Event::DeidBlocked,
            audit::deid_blocked_detail(&findings, question.len()),
        )
        .await
        .map_err(|_| unavailable("audit"))?;
        return Err(ChatError::Identifiers {
            message: blocked_message(findings.iter().map(|f| f.kind)),
            safety_line,
        });
    }

    let facts = store::run_facts(&state.pool, user.id, run_id)
        .await
        .map_err(|_| unavailable("run_facts"))?
        .ok_or(ChatError::NotFound)?;
    let record = match (decide(&facts, state.gemma.is_some()), facts.record) {
        (ChatState::NotOffered, _) => return Err(ChatError::NotOffered { safety_line }),
        (ChatState::Ready, Some(record)) => record,
        // Getting ready, or a client or prep problem: no answer for now.
        _ => return Err(ChatError::Unavailable { safety_line }),
    };

    let today = store::turns_today(&state.pool, user.id)
        .await
        .map_err(|_| unavailable("limit"))?;
    if today >= state.chat.daily_limit {
        return Err(ChatError::DailyLimit { safety_line });
    }

    // A dead instance's turn must neither lock its user out nor fill the
    // queue, so stale turns are settled before either is judged.
    store::resolve_stale_turns(&state.pool)
        .await
        .map_err(|_| unavailable("stale"))?;
    let pending = store::pending_turns(&state.pool)
        .await
        .map_err(|_| unavailable("pending"))?;
    if pending >= MAX_PENDING {
        return Err(ChatError::Busy { safety_line });
    }

    let turn = store::insert_pending_turn(
        &state.pool,
        &PendingTurn {
            run_id,
            user_id: user.id,
            question: text,
            mode: mode.as_str(),
            safety_line,
            personal,
        },
    )
    .await
    .map_err(|_| unavailable("insert"))?
    .ok_or(ChatError::InFlight { safety_line })?;

    let job = TurnJob {
        state: state.clone(),
        turn,
        record,
        question: text.to_string(),
        mode,
        limit,
    };
    let pool = state.pool.clone();
    tokio::spawn(guard_turn(pool, turn.id, answer_turn(job)));

    Ok(Asked {
        turn_id: turn.id,
        safety_line,
    })
}

/// Runs a turn's task; if it panics, the turn finishes `unavailable`. The
/// payload is dropped unread: it could quote stored text.
#[doc(hidden)]
pub async fn guard_turn<F: Future<Output = ()>>(pool: sqlx::PgPool, turn_id: i64, task: F) {
    if AssertUnwindSafe(task).catch_unwind().await.is_err() {
        tracing::error!(turn_id, "chat turn panicked");
        if store::fail_turn(&pool, turn_id).await.is_err() {
            // The stale sweep will resolve it.
            tracing::warn!(code = "fail_turn", turn_id, "chat turn not finished");
        }
    }
}

struct TurnJob {
    state: AppState,
    turn: NewTurn,
    record: RecordRef,
    question: String,
    mode: ChatMode,
    limit: Duration,
}

async fn answer_turn(job: TurnJob) {
    let id = job.turn.id;
    if let Err(code) = reply_to(&job).await {
        tracing::warn!(code, turn_id = id, "chat turn failed");
        if store::fail_turn(&job.state.pool, id).await.is_err() {
            tracing::warn!(code = "fail_turn", turn_id = id, "chat turn not finished");
        }
    }
}

/// The question's embedding, or `None` for a words-only search. A failure is
/// never fatal: retrieval works from words alone.
async fn query_vector(state: &AppState, query: &str) -> Option<Vec<f32>> {
    let embedder = state.embedder.as_ref()?;
    match embedder.embed(query, EMBED_LIMIT).await {
        Ok(v) => Some(v),
        Err(e) => {
            tracing::warn!(code = e.code(), "chat question not embedded");
            None
        }
    }
}

/// `Err` is a fixed code, for a turn that finishes `unavailable`.
async fn reply_to(job: &TurnJob) -> Result<(), &'static str> {
    let started = Instant::now();
    let pool = &job.state.pool;
    let turn = job.turn;

    let previous = store::previous_question(pool, turn.conversation_id, turn.seq)
        .await
        .map_err(|_| "previous")?;
    let query = retrieve::query_text(&job.question, previous.as_deref());
    let record = store::record_bodies(pool, job.record.id)
        .await
        .map_err(|_| "record")?;
    let pairs = job::record_pairs(&record)?;
    // What the question names; a short follow-up stays on what it follows
    // ("how sure is that?" after "robotic or open?"). An arm the record does
    // not compare holds the search to that arm ("and open?").
    let asked =
        comparison::focus(&job.question, &pairs).or_else(|| comparison::focus(&query, &pairs));

    let vector = query_vector(&job.state, &query).await;
    let hits = retrieve::top(
        pool,
        job.record.id,
        job.record.topic_id,
        asked.as_deref(),
        &query,
        vector.as_deref(),
        job.state.chat.min_score,
    )
    .await
    .map_err(|_| "retrieve")?;

    // C8: no passages, no request. `GroundedPassages` cannot be built empty.
    let Some(grounded) = GroundedPassages::new(hits) else {
        finish(job, started, "not_covered", "", None).await;
        return Ok(());
    };
    let sources: Vec<i64> = grounded.hits().iter().map(stored_source).collect();
    if !store::set_turn_sources(pool, turn.id, &sources)
        .await
        .map_err(|_| "sources")?
    {
        return Ok(()); // Already finished by the sweep.
    }

    let history: Vec<(String, String)> = store::history_turns(
        pool,
        turn.conversation_id,
        turn.seq,
        prompt::HISTORY_TURNS as i64,
    )
    .await
    .map_err(|_| "history")?
    .into_iter()
    .map(|(q, a)| (q, checks::strip_citations(&a)))
    .collect();
    let request = prompt::grounded(&grounded, &job.question, &history, job.mode);

    let gemma = job.state.gemma.as_ref().ok_or("no_gemma")?;
    let buffer = Arc::new(Mutex::new(String::new()));
    let sink = Arc::clone(&buffer);
    let stream = gemma.stream(&request, job.limit, move |t| {
        // Postgres text cannot hold NUL.
        locked(&sink).push_str(&t.replace('\0', ""));
    });
    tokio::pin!(stream);

    let mut every = tokio::time::interval(WRITE_EVERY);
    every.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut written = String::new();
    let end = loop {
        tokio::select! {
            end = &mut stream => break end,
            _ = every.tick() => {
                let now = locked(&buffer).clone();
                if now == written {
                    continue;
                }
                match store::set_turn_reply(pool, turn.id, &now).await {
                    Ok(true) => written = now,
                    // Finished by the sweep (or the run has gone): stop
                    // holding Gemma for a reply nobody will store.
                    Ok(false) => return Ok(()),
                    // Try again next tick; the final write may still land.
                    Err(_) => tracing::warn!(code = "reply_write", "chat reply not written"),
                }
            }
        }
    };
    if let Err(e) = &end {
        tracing::warn!(code = e.code(), turn_id = turn.id, "chat reply failed");
    }
    let text = locked(&buffer).trim().to_string();

    match settle(&text, &end) {
        Settled::Busy => {
            let busy = json!({ BUSY_KEY: true });
            finish(job, started, "unavailable", "", Some(&busy)).await;
        }
        Settled::Unavailable => finish(job, started, "unavailable", "", None).await,
        Settled::NotCovered => finish(job, started, "not_covered", &text, None).await,
        Settled::Reply(status) => {
            let texts: Vec<&str> = grounded.hits().iter().map(|h| h.text.as_str()).collect();
            let found = checks::run(&text, &texts, asked.as_deref(), &pairs);
            let found = serde_json::to_value(&found).map_err(|_| "checks")?;
            finish(job, started, status, &text, Some(&found)).await;
        }
    }
    Ok(())
}

fn locked(m: &Mutex<String>) -> std::sync::MutexGuard<'_, String> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

#[derive(Debug, PartialEq, Eq)]
enum Settled {
    Busy,
    Unavailable,
    NotCovered,
    Reply(&'static str),
}

/// What a finished stream amounts to. Text already received is always kept.
fn settle(text: &str, end: &Result<StreamEnd, GemmaError>) -> Settled {
    if text.is_empty() {
        return match end {
            Err(GemmaError::Http(429)) => Settled::Busy,
            // An error, or a reply with nothing in it.
            _ => Settled::Unavailable,
        };
    }
    if lines::is_not_covered(text) {
        return Settled::NotCovered;
    }
    match end {
        Ok(StreamEnd::Complete) => Settled::Reply("done"),
        Ok(StreamEnd::CutAtLimit) | Err(_) => Settled::Reply("partial"),
    }
}

/// The final write. A failure leaves the turn to the stale sweep, which keeps
/// the text last written.
async fn finish(
    job: &TurnJob,
    started: Instant,
    status: &'static str,
    reply: &str,
    checks: Option<&Value>,
) {
    let latency_ms = i32::try_from(started.elapsed().as_millis()).unwrap_or(i32::MAX);
    if store::finish_turn(
        &job.state.pool,
        job.turn.id,
        status,
        reply,
        checks,
        latency_ms,
    )
    .await
    .is_err()
    {
        tracing::warn!(
            code = "finish",
            turn_id = job.turn.id,
            "chat turn not finished"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts(
        mode: &str,
        opinion: Option<&str>,
        job: Option<&str>,
        prep: Option<&str>,
        rec: bool,
    ) -> RunFacts {
        RunFacts {
            mode: mode.to_string(),
            opinion: opinion.map(str::to_string),
            memory_job: job.map(str::to_string),
            record: rec.then_some(RecordRef { id: 1, topic_id: 1 }),
            prep: prep.map(str::to_string),
        }
    }

    #[test]
    fn state_rules() {
        use ChatState::*;
        let d = |f: RunFacts, g| decide(&f, g);
        assert_eq!(
            d(facts("quick", Some("done"), None, Some("ok"), true), true),
            NotOffered
        );
        assert_eq!(
            d(facts("focused", None, None, None, false), true),
            NotOffered
        );
        assert_eq!(
            d(
                facts("focused", Some("failed"), Some("pending"), None, false),
                true
            ),
            NotOffered
        );
        assert_eq!(
            d(facts("focused", Some("done"), None, None, false), true),
            NotOffered
        );
        assert_eq!(
            d(
                facts("focused", Some("done"), Some("skipped"), None, false),
                true
            ),
            NotOffered
        );
        assert_eq!(
            d(
                facts("focused", Some("done"), Some("pending"), None, false),
                true
            ),
            GettingReady
        );
        assert_eq!(
            d(
                facts("focused", Some("done"), Some("done"), None, true),
                true
            ),
            GettingReady
        );
        assert_eq!(
            d(
                facts("focused", Some("done"), Some("done"), Some("ok"), true),
                true
            ),
            Ready
        );
        assert_eq!(
            d(
                facts("focused", Some("done"), Some("done"), Some("failed"), true),
                true
            ),
            Unavailable
        );
        assert_eq!(
            d(
                facts("focused", Some("done"), Some("done"), Some("ok"), true),
                false
            ),
            Unavailable
        );
        assert_eq!(
            d(
                facts("focused", Some("done"), Some("pending"), None, false),
                false
            ),
            Unavailable
        );
    }

    #[test]
    fn settling_a_stream() {
        let ok = Ok(StreamEnd::Complete);
        let cut = Ok(StreamEnd::CutAtLimit);
        let down = Err(GemmaError::Http(503));
        let busy = Err(GemmaError::Http(429));
        assert_eq!(settle("x", &ok), Settled::Reply("done"));
        assert_eq!(settle("x", &cut), Settled::Reply("partial"));
        assert_eq!(settle("x", &down), Settled::Reply("partial"));
        assert_eq!(settle("x", &busy), Settled::Reply("partial"));
        assert_eq!(settle("", &down), Settled::Unavailable);
        assert_eq!(settle("", &ok), Settled::Unavailable);
        assert_eq!(settle("", &busy), Settled::Busy);
        assert_eq!(
            settle("Not covered in this evidence", &ok),
            Settled::NotCovered
        );
        assert_eq!(
            settle("Not covered in this evidence.", &cut),
            Settled::NotCovered
        );
    }

    fn row(status: &str, reply: &str, checks: Option<Value>) -> TurnRow {
        TurnRow {
            id: 1,
            seq: 1,
            question: "q".into(),
            reply: reply.into(),
            status: status.into(),
            mode: "clinical".into(),
            passage_ids: Vec::new(),
            checks,
            safety_line: false,
            personal: false,
            feedback: None,
            suggested: false,
            latency_ms: None,
        }
    }

    #[test]
    fn the_busy_marker_is_not_read_as_checks() {
        let v = view_of(row("unavailable", "", Some(json!({BUSY_KEY: true}))));
        assert_eq!(v.reply, lines::BUSY);
        assert_eq!(v.checks, None);
    }

    #[test]
    fn a_swept_partial_turn_is_not_clean() {
        let v = view_of(row("partial", "so far", Some(json!({"unchecked": true}))));
        assert_eq!(v.reply, "so far");
        let c = v.checks.expect("reported, not hidden");
        assert!(c.unchecked);
        assert!(!c.is_clean());
    }

    #[test]
    fn an_older_row_keeps_its_flags() {
        let v = view_of(row("done", "r", Some(json!({"uncited": true}))));
        let c = v.checks.expect("older row reads");
        assert!(c.uncited && !c.is_clean());
    }

    #[test]
    fn a_qa_source_is_stored_negated_and_read_back() {
        assert_eq!(source_of(7), TurnSource::Passage(7));
        assert_eq!(source_of(-7), TurnSource::Qa(7));
    }
}
