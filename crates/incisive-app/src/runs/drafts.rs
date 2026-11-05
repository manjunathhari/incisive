//! The writers' half-written replies (Stage S1). Server only.
//!
//! While a streamed writer works, the text received so far is kept in
//! `incisive.run_drafts`, at most once every [`WRITE_EVERY`] and only when it
//! has grown, from one detached task per call: a slow database never slows the
//! writer, and a failed write is logged with ids only and dropped. It never
//! touches the run.
//!
//! A draft can be written only while its writer's row is live (the run
//! `running`, the opinion `pending`), so a late write cannot bring one back
//! after the final is saved. Each is deleted when the run or the opinion ends
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
        Part::Answer => "SELECT id FROM incisive.runs WHERE id = $1 AND status = 'running'",
        Part::Opinion => {
            "SELECT run_id FROM incisive.run_temporal WHERE run_id = $1 AND status = 'pending'"
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

#[cfg(test)]
mod tests {
    use super::*;

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
