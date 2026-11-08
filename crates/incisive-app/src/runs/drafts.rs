//! The writers' half-written replies (Stage S1). Server only.
//!
//! While a streamed writer works, the text received so far is kept in
//! `incisive.run_drafts`, at most once every [`WRITE_EVERY`] and only when it
//! has grown, from one detached task per call: a slow database never slows the
//! writer, and a failed write is logged with ids only and dropped. It never
//! touches the run.
//!
//! A draft can be written only while its writer's row is live (the run
//! `running`, the opinion `pending`). The write locks that row (`FOR SHARE`),
//! so it waits for a status change in progress and then sees it, or the change
//! waits for the write and the delete after it removes the draft: a late write
//! cannot bring one back after the final is saved. Each is deleted when the run or the opinion ends
//! ([`clear`]), and recovery removes any left behind ([`sweep`]).
//!
//! Privacy: a draft is reply text, like `run_answers`. It is read only through
//! the run's owner scoping and never logged: ids and sizes only.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use sqlx::PgPool;
use tokio::sync::Notify;
use uuid::Uuid;

use super::{StepKind, store};
use crate::agent::llm::Streaming;
use crate::run_tracker;

/// At most one write this often, per writer.
pub const WRITE_EVERY: Duration = Duration::from_millis(1500);
/// A draft this old has no live writer: writers are stopped long before it.
const STALE_AFTER_MIN: i32 = 30;

/// Which writer a draft belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Part {
    Answer,
    Opinion,
}

impl Part {
    pub fn as_str(self) -> &'static str {
        match self {
            Part::Answer => "answer",
            Part::Opinion => "opinion",
        }
    }
}

/// Store the draft, only while its writer's row is live. False: nothing stored.
pub async fn upsert(pool: &PgPool, run_id: Uuid, part: Part, text: &str) -> sqlx::Result<bool> {
    let live = match part {
        Part::Answer => {
            "SELECT id FROM incisive.runs WHERE id = $1 AND status = 'running' FOR SHARE"
        }
        Part::Opinion => {
            "SELECT run_id FROM incisive.run_temporal WHERE run_id = $1 AND status = 'pending' \
             FOR SHARE"
        }
    };
    let r = sqlx::query(&format!(
        "INSERT INTO incisive.run_drafts (run_id, part, text, chars) \
         SELECT l.id, $2, $3, $4 FROM ({live}) AS l(id) \
         ON CONFLICT (run_id, part) DO UPDATE SET text = EXCLUDED.text, \
           chars = EXCLUDED.chars, updated_at = now()"
    ))
    .bind(run_id)
    .bind(part.as_str())
    .bind(text)
    .bind(i32::try_from(text.chars().count()).unwrap_or(i32::MAX))
    .execute(pool)
    .await?;
    Ok(r.rows_affected() > 0)
}

/// The draft's text, if any. The caller has already checked the run's owner.
pub async fn get(pool: &PgPool, run_id: Uuid, part: Part) -> sqlx::Result<Option<String>> {
    sqlx::query_scalar("SELECT text FROM incisive.run_drafts WHERE run_id = $1 AND part = $2")
        .bind(run_id)
        .bind(part.as_str())
        .fetch_optional(pool)
        .await
}

/// Delete the draft. Never fails the caller: a miss is logged by id, and
/// [`sweep`] removes it later.
pub async fn clear(pool: &PgPool, run_id: Uuid, part: Part) {
    let r = sqlx::query("DELETE FROM incisive.run_drafts WHERE run_id = $1 AND part = $2")
        .bind(run_id)
        .bind(part.as_str())
        .execute(pool)
        .await;
    if r.is_err() {
        tracing::warn!(%run_id, part = part.as_str(), "draft could not be deleted");
    }
}

/// Recovery: remove drafts whose writer's row has ended, and any too old to
/// have a live writer. Returns how many.
pub async fn sweep(pool: &PgPool) -> sqlx::Result<u64> {
    let r = sqlx::query(
        "DELETE FROM incisive.run_drafts d USING incisive.runs r \
         WHERE r.id = d.run_id AND ( \
           (d.part = 'answer' AND r.status <> 'running') \
           OR (d.part = 'opinion' AND NOT EXISTS ( \
                 SELECT 1 FROM incisive.run_temporal t \
                 WHERE t.run_id = d.run_id AND t.status = 'pending')) \
           OR d.updated_at < now() - make_interval(mins => $1))",
    )
    .bind(STALE_AFTER_MIN)
    .execute(pool)
    .await?;
    Ok(r.rows_affected())
}

/// Whether a write is due: the text has grown and [`WRITE_EVERY`] has passed.
#[derive(Debug, Clone, Copy)]
struct Throttle {
    every: Duration,
    last: Option<Instant>,
    written: usize,
}

impl Throttle {
    /// `None`: nothing new. Otherwise how long to wait before writing.
    fn wait(&self, now: Instant, len: usize) -> Option<Duration> {
        if len <= self.written {
            return None;
        }
        Some(self.last.map_or(Duration::ZERO, |l| {
            self.every.saturating_sub(now.saturating_duration_since(l))
        }))
    }

    fn wrote(&mut self, now: Instant, len: usize) {
        self.last = Some(now);
        self.written = len;
    }

    fn reset(&mut self) {
        self.written = 0;
    }
}

#[derive(Default)]
struct Buffer {
    text: String,
    /// Raised at each attempt: the text starts again.
    attempt: u64,
}

struct Shared {
    buffer: Mutex<Buffer>,
    wake: Notify,
    closed: AtomicBool,
}

impl Shared {
    fn lock(&self) -> std::sync::MutexGuard<'_, Buffer> {
        self.buffer.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Held by the hooks: when the last is dropped the call is over, and the
/// writing task makes its last write and ends.
struct Open(Arc<Shared>);

impl Drop for Open {
    fn drop(&mut self) {
        self.0.closed.store(true, Ordering::Release);
        self.0.wake.notify_one();
    }
}

/// The hooks for one writer's streamed calls, with `deadline` as the overall
/// limit: keep the draft, and add the first-words step line once.
pub fn streaming(pool: PgPool, run_id: Uuid, part: Part, deadline: Instant) -> Streaming {
    streaming_every(pool, run_id, part, deadline, WRITE_EVERY)
}

/// [`streaming`] writing at most once every `every` (shorter only in tests).
pub fn streaming_every(
    pool: PgPool,
    run_id: Uuid,
    part: Part,
    deadline: Instant,
    every: Duration,
) -> Streaming {
    let shared = Arc::new(Shared {
        buffer: Mutex::default(),
        wake: Notify::new(),
        closed: AtomicBool::new(false),
    });
    tokio::spawn(write_task(
        pool.clone(),
        run_id,
        part,
        shared.clone(),
        every,
    ));
    let open = Arc::new(Open(shared));
    let started = Arc::new(Mutex::new(Instant::now()));
    let said = Arc::new(AtomicBool::new(false));
    let on_attempt = {
        let (open, started) = (open.clone(), started.clone());
        Arc::new(move || {
            *started.lock().unwrap_or_else(PoisonError::into_inner) = Instant::now();
            let shared = &open.0;
            {
                let mut b = shared.lock();
                b.text.clear();
                b.attempt += 1;
            }
            shared.wake.notify_one();
        })
    };
    let on_text = Arc::new(move |text: &str| {
        if text.is_empty() {
            return;
        }
        let shared = &open.0;
        {
            let mut b = shared.lock();
            // The text so far: only what is new is copied.
            match text.get(b.text.len()..) {
                Some(new) => b.text.push_str(new),
                None => text.clone_into(&mut b.text),
            }
        }
        shared.wake.notify_one();
        if !said.swap(true, Ordering::AcqRel) {
            let elapsed = started
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .elapsed();
            let line = run_tracker::first_words_line(part == Part::Opinion, elapsed.as_secs());
            let pool = pool.clone();
            // Detached: the writer never waits for it.
            tokio::spawn(async move {
                if store::add_step(&pool, run_id, StepKind::Write, &line)
                    .await
                    .is_err()
                {
                    tracing::warn!(%run_id, "first-words line could not be stored");
                }
            });
        }
    });
    Streaming {
        on_attempt,
        on_text,
        deadline: Some(deadline),
    }
}

/// The one task that writes a call's draft, so writes never overlap or arrive
/// out of order. Ends when the call is over, after one last write.
async fn write_task(pool: PgPool, run_id: Uuid, part: Part, shared: Arc<Shared>, every: Duration) {
    let mut throttle = Throttle {
        every,
        last: None,
        written: 0,
    };
    let mut attempt = 0;
    loop {
        let closed = shared.closed.load(Ordering::Acquire);
        let (now_attempt, len) = {
            let b = shared.lock();
            (b.attempt, b.text.len())
        };
        if now_attempt != attempt {
            attempt = now_attempt;
            if throttle.written > 0 {
                // A retry starts again: the page goes back to waiting.
                clear(&pool, run_id, part).await;
                throttle.reset();
            }
        }
        match throttle.wait(Instant::now(), len) {
            Some(wait) if wait.is_zero() || closed => {
                let text = shared.lock().text.clone();
                let now = Instant::now();
                if upsert(&pool, run_id, part, &text).await.is_err() {
                    tracing::warn!(%run_id, part = part.as_str(), bytes = text.len(), "draft could not be stored");
                }
                // A failed write is dropped: the next one carries the text.
                throttle.wrote(now, text.len());
                if closed {
                    return;
                }
            }
            Some(wait) => tokio::time::sleep(wait).await,
            None if closed => return,
            None => shared.wake.notified().await,
        }
    }
}

// ---------- S1: a Quick answer written in four parts ----------
//
// With `QUICK_PARALLEL=on` four calls write one answer at once. Their texts
// are kept apart and written as ONE answer draft of the full reply's shape
// ([`merged_draft`]), by one task at the same cadence, so the page reads it
// with its usual parser and needs no change. The first-words line is added
// once, for the first part to produce words.

/// The text of each part so far, and a count raised at every change (the
/// throttle's "grown").
#[derive(Default)]
struct Parts {
    texts: Vec<String>,
    changes: usize,
}

struct SharedParts {
    parts: Mutex<Parts>,
    wake: Notify,
    closed: AtomicBool,
}

impl SharedParts {
    fn lock(&self) -> std::sync::MutexGuard<'_, Parts> {
        self.parts.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Held by every part's hooks: when the last is dropped every call is over.
struct OpenParts(Arc<SharedParts>);

impl Drop for OpenParts {
    fn drop(&mut self) {
        self.0.closed.store(true, Ordering::Release);
        self.0.wake.notify_one();
    }
}

/// One draft from the parts' texts so far, as one JSON text the page's
/// parser reads (`draft_page::read_prefix`). The fields come in a fixed
/// order, part by part and, within a part, in `fields` order (S1 review M2:
/// the draft's citation numbers, given by first appearance, do not jump as
/// parts arrive). Every part but the last is written as finished JSON (what
/// is read so far); the last part (the summary: the page shows a certainty
/// only once it is whole) comes last as its raw text while it is still
/// open, so the page marks where it is cut off. `None`: nothing yet.
pub fn merged_draft(parts: &[String], fields: &[Vec<String>]) -> Option<String> {
    use crate::draft_page::read_prefix;
    let read: Vec<_> = parts.iter().map(|t| read_prefix(t)).collect();
    let last = parts.len().checked_sub(1)?;
    let raw_open = read[last].value.is_object() && read[last].open.is_some();
    let mut out_fields: Vec<String> = Vec::new();
    for (i, p) in read.iter().enumerate() {
        if i == last && raw_open {
            continue;
        }
        let serde_json::Value::Object(m) = &p.value else {
            continue;
        };
        for k in fields.get(i).map(Vec::as_slice).unwrap_or_default() {
            if let Some(v) = m.get(k) {
                out_fields.push(format!("{}:{v}", serde_json::Value::String(k.clone())));
            }
        }
    }
    let tail = raw_open
        .then(|| parts[last].find('{').map(|at| &parts[last][at + 1..]))
        .flatten()
        .filter(|t| !t.trim().is_empty());
    if out_fields.is_empty() && tail.is_none() {
        return None;
    }
    let mut out = String::from("{");
    out.push_str(&out_fields.join(","));
    match tail {
        Some(t) => {
            if !out_fields.is_empty() {
                out.push(',');
            }
            out.push_str(t);
        }
        None => out.push('}'),
    }
    Some(out)
}

/// The merged draft's writing task, to wait for before the draft is
/// cleared (the one-piece fallback): its last write is then done.
#[must_use = "wait for it before clearing the draft"]
pub struct PartsDraft(tokio::task::JoinHandle<()>);

impl PartsDraft {
    /// Wait (at most 10 s) until every part's call is over and the last
    /// merged write is done.
    pub async fn finished(self) {
        let _ = tokio::time::timeout(Duration::from_secs(10), self.0).await;
    }
}

/// The hooks for one call per entry of `fields` (each part's top-level
/// fields, in order), writing one answer together, with `deadline` as each
/// call's overall limit (see the section docs and [`merged_draft`]).
pub fn streaming_parts(
    pool: PgPool,
    run_id: Uuid,
    deadline: Instant,
    fields: &[&[&str]],
) -> (Vec<Streaming>, PartsDraft) {
    streaming_parts_every(pool, run_id, deadline, fields, WRITE_EVERY)
}

/// [`streaming_parts`] writing at most once every `every` (shorter only in
/// tests).
pub fn streaming_parts_every(
    pool: PgPool,
    run_id: Uuid,
    deadline: Instant,
    fields: &[&[&str]],
    every: Duration,
) -> (Vec<Streaming>, PartsDraft) {
    let n = fields.len();
    let fields: Vec<Vec<String>> = fields
        .iter()
        .map(|f| f.iter().map(|x| (*x).to_string()).collect())
        .collect();
    let shared = Arc::new(SharedParts {
        parts: Mutex::new(Parts {
            texts: vec![String::new(); n],
            changes: 0,
        }),
        wake: Notify::new(),
        closed: AtomicBool::new(false),
    });
    let task = tokio::spawn(write_parts_task(
        pool.clone(),
        run_id,
        shared.clone(),
        fields,
        every,
    ));
    let open = Arc::new(OpenParts(shared));
    // From the first part's first attempt: the first words of any part.
    let started: Arc<Mutex<Option<Instant>>> = Arc::new(Mutex::new(None));
    let said = Arc::new(AtomicBool::new(false));
    let hooks = (0..n)
        .map(|i| {
            let on_attempt = {
                let (open, started) = (open.clone(), started.clone());
                Arc::new(move || {
                    started
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .get_or_insert_with(Instant::now);
                    let shared = &open.0;
                    {
                        let mut p = shared.lock();
                        p.texts[i].clear();
                        p.changes += 1;
                    }
                    shared.wake.notify_one();
                }) as Arc<dyn Fn() + Send + Sync>
            };
            let on_text = {
                let (open, started, said, pool) =
                    (open.clone(), started.clone(), said.clone(), pool.clone());
                Arc::new(move |text: &str| {
                    if text.is_empty() {
                        return;
                    }
                    let shared = &open.0;
                    {
                        let mut p = shared.lock();
                        let b = &mut p.texts[i];
                        match text.get(b.len()..) {
                            Some(new) => b.push_str(new),
                            None => text.clone_into(b),
                        }
                        p.changes += 1;
                    }
                    shared.wake.notify_one();
                    if !said.swap(true, Ordering::AcqRel) {
                        let elapsed = started
                            .lock()
                            .unwrap_or_else(PoisonError::into_inner)
                            .map_or(Duration::ZERO, |s| s.elapsed());
                        let line = run_tracker::first_words_line(false, elapsed.as_secs());
                        let pool = pool.clone();
                        tokio::spawn(async move {
                            if store::add_step(&pool, run_id, StepKind::Write, &line)
                                .await
                                .is_err()
                            {
                                tracing::warn!(%run_id, "first-words line could not be stored");
                            }
                        });
                    }
                }) as Arc<dyn Fn(&str) + Send + Sync>
            };
            Streaming {
                on_attempt,
                on_text,
                deadline: Some(deadline),
            }
        })
        .collect();
    (hooks, PartsDraft(task))
}

/// The one task writing the merged draft ([`write_task`]'s rules: one write
/// at a time, at most once every `every`, only when something changed, one
/// last write when every call is over).
async fn write_parts_task(
    pool: PgPool,
    run_id: Uuid,
    shared: Arc<SharedParts>,
    fields: Vec<Vec<String>>,
    every: Duration,
) {
    let mut throttle = Throttle {
        every,
        last: None,
        written: 0,
    };
    loop {
        let closed = shared.closed.load(Ordering::Acquire);
        let changes = shared.lock().changes;
        match throttle.wait(Instant::now(), changes) {
            Some(wait) if wait.is_zero() || closed => {
                let (text, changes) = {
                    let p = shared.lock();
                    (merged_draft(&p.texts, &fields), p.changes)
                };
                let now = Instant::now();
                if let Some(text) = text
                    && upsert(&pool, run_id, Part::Answer, &text).await.is_err()
                {
                    tracing::warn!(%run_id, part = "answer", bytes = text.len(), "draft could not be stored");
                }
                throttle.wrote(now, changes);
                if closed {
                    return;
                }
            }
            Some(wait) => tokio::time::sleep(wait).await,
            None if closed => return,
            None => shared.wake.notified().await,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn order() -> Vec<Vec<String>> {
        [
            &["studies", "conflicts", "notable_findings"][..],
            &["outcomes"],
            &["applicability", "would_change"],
            &["bottom_line", "at_a_glance", "clinical_practice"],
        ]
        .iter()
        .map(|f| f.iter().map(|x| (*x).to_string()).collect())
        .collect()
    }

    /// S1: four parts half-written merge into one draft the page reads, each
    /// part's content under its own section, the summary's cut-off kept.
    #[test]
    fn four_partial_streams_merge_into_one_draft_the_page_reads() {
        use crate::draft_page::{COMING, read_answer, read_prefix};
        let trials = r#"{"studies": [{"pmid": "111", "label": "RIVAL 2020", "design": "RCT", "n": 102, "setting": "US", "comparison": "r vs l", "key_results": "No difference [111]", "appraisal": "small"}], "conflicts": [], "notable_findings": "Funding by the maker [111]"}"#.to_string();
        let outcomes = r#"{"outcomes": [{"outcome": "Chronic pain", "rct_evidence": "RIVAL", "effect_text": "No difference at 30 d"#.to_string();
        let context = r#"{"applicability": {"case_mix": "primary unilateral", "india_cost_context": "not known", "training": "no bearing"}, "would_change": ["A larger trial"]}"#.to_string();
        let summary =
            r#"{"bottom_line": {"text": "No consistent benefit [111].", "certainty": "Mod"#
                .to_string();
        let parts = vec![trials, outcomes, context, summary];
        let d = merged_draft(&parts, &order()).expect("a draft");
        // One JSON text: open where the summary was cut off.
        let p = read_prefix(&d);
        assert!(p.open.is_some());
        assert_eq!(p.value["studies"][0]["label"], "RIVAL 2020");
        assert_eq!(p.value["would_change"][0], "A larger trial");
        assert_eq!(
            p.value["outcomes"][0]["effect_text"],
            "No difference at 30 d"
        );
        let s = read_answer(&d);
        let get = |t: &str| s.iter().find(|x| x.title == t).unwrap();
        assert!(!get("Bottom line").items.is_empty());
        // The half-written certainty is not shown.
        assert!(
            get("Bottom line")
                .items
                .iter()
                .all(|i| i.label.as_deref().is_none_or(|l| !l.contains("Mod")))
        );
        assert_eq!(get("Outcome by outcome").items.len(), 1);
        assert_eq!(get("The trials").items.len(), 1);
        assert!(!get("Applicability").items.is_empty());
        assert!(!get("Other notable findings from this run").items.is_empty());
        assert!(
            get("At a glance").items.is_empty(),
            "{COMING} until it arrives"
        );
        // A fixed order: the parts' fields in Section order, the summary last.
        let at = |k: &str| d.find(&format!("\"{k}\"")).unwrap();
        assert!(at("studies") < at("conflicts") && at("conflicts") < at("notable_findings"));
        assert!(at("notable_findings") < at("outcomes") && at("outcomes") < at("applicability"));
        assert!(at("applicability") < at("would_change") && at("would_change") < at("bottom_line"));
        // More text in an earlier part never moves a later one ahead of it.
        let mut more = parts.clone();
        more[1].push_str(" [222]\", \"numbers\": []");
        let d2 = merged_draft(&more, &order()).unwrap();
        assert_eq!(d2.find("\"studies\""), d.find("\"studies\""));
        assert!(d2.find("\"bottom_line\"").unwrap() > d2.find("\"would_change\"").unwrap());
        // The summary whole: everything as finished JSON, in the same order.
        let mut whole = parts.clone();
        whole[3] = r#"{"bottom_line": {"text": "x [111].", "certainty": "Low", "certainty_reason": "small"}, "at_a_glance": "No benefit.", "clinical_practice": "Default choice: lap."}"#.into();
        let d = merged_draft(&whole, &order()).unwrap();
        let p = read_prefix(&d);
        assert_eq!(p.value["bottom_line"]["certainty"], "Low");
        assert_eq!(p.value["outcomes"][0]["outcome"], "Chronic pain");
        assert_eq!(
            p.value["outcomes"][0]["effect_text"], "No difference at 30 d",
            "an open earlier part as read so far"
        );
        // All whole: a whole object.
        whole[1] = r#"{"outcomes": []}"#.into();
        let d = merged_draft(&whole, &order()).unwrap();
        assert_eq!(read_prefix(&d).open, None);
        assert!(serde_json::from_str::<serde_json::Value>(&d).is_ok());
        // Nothing yet: no draft.
        assert_eq!(merged_draft(&vec![String::new(); 4], &order()), None);
        assert_eq!(
            merged_draft(&[String::new(), "{".to_string()], &order()),
            None
        );
    }

    #[test]
    fn the_first_write_is_at_once_then_at_most_one_per_interval_and_only_when_grown() {
        let every = Duration::from_millis(1500);
        let t0 = Instant::now();
        let mut t = Throttle {
            every,
            last: None,
            written: 0,
        };
        assert_eq!(t.wait(t0, 0), None, "nothing yet");
        assert_eq!(t.wait(t0, 5), Some(Duration::ZERO));
        t.wrote(t0, 5);
        assert_eq!(t.wait(t0, 5), None, "not grown");
        assert_eq!(
            t.wait(t0 + Duration::from_millis(500), 9),
            Some(Duration::from_millis(1000))
        );
        assert_eq!(t.wait(t0 + every, 9), Some(Duration::ZERO));
        assert_eq!(t.wait(t0 + every * 9, 5), None, "time alone writes nothing");
    }

    #[test]
    fn after_a_retry_shorter_text_is_written_again() {
        let t0 = Instant::now();
        let mut t = Throttle {
            every: Duration::ZERO,
            last: None,
            written: 0,
        };
        t.wrote(t0, 100);
        assert_eq!(t.wait(t0, 3), None);
        t.reset();
        assert_eq!(t.wait(t0, 3), Some(Duration::ZERO));
    }
}
