//! Bounded concurrency for the steps that make one Claude call (with its
//! retry) per item: Screen (per batch) and Extract (per paper). Fix B.
//! Server only.
//!
//! [`run`] launches items in their original order, at most `max_in_flight`
//! at a time, and returns one [`Outcome`] per item **in item order**, so the
//! step builds its result, trail and step lines exactly as if the items had
//! run one after another (deterministic, and no interleaved lines: the
//! steps write their lines only once every item has settled).
//!
//! **Money.** Nothing here touches the budget except through the step's
//! `launch` closure, which must start the item's first Claude call
//! synchronously ([`super::llm::LlmCall::start_structured`]): when `launch`
//! returns, that call's hold is already taken (or refused). So:
//! * before every launch, the cancel flag and the wrap-up point are
//!   checked: with nothing in flight [`RunBudget::should_wrap_up`] (70%
//!   spent, wrap-up time, or no headroom left above the finish reserve),
//!   exactly as one call at a time; with calls in flight
//!   [`RunBudget::should_wrap_up_settled`], the same without their holds and
//!   reserve shares, which the hold check counts instead (a hold that does
//!   not fit waits for a call to settle, then everything is checked again).
//!   Once wrap-up is reached no new item is launched (the first item is
//!   always launched, as before Fix B);
//! * every hold is taken atomically ([`RunBudget::try_hold_gathering`]):
//!   with nothing in flight against `spent + estimate + finish reserve`
//!   (the single-call rule); with calls in flight against `spent + 2 ×
//!   (held + estimate) + finish reserve` (Ruling R-FB3, room for the calls
//!   in flight to cost more than their holds), so concurrent holds can never
//!   together pass the run's limit, and near it fewer calls run at once. A hold refused while
//!   other calls are in flight is tried again (same item, wrap-up checked
//!   again) after the next one settles, since a settled call usually frees
//!   part of its hold; refused with nothing of its own in flight but another
//!   step's calls holding (Stage 3: the skeptic beside the extractor), it is
//!   tried again after [`OTHERS_POLL`] while wrap-up is not due; refused
//!   with nothing in flight at all (`OverBudget`, as before Fix B), it stops
//!   launching;
//! * calls already in flight finish and are charged (the spawned call task
//!   owns its hold, Ruling R-8b). A JSON retry of an in-flight item is a new
//!   hold, checked against the budget like any call, as before Fix B.
//!
//! **Errors.** `Llm` / `Parse` are the item's own failure ([`Outcome::Done`]
//! with the error; the step counts it). `OverBudget` (from a launch with
//! nothing in flight, or from an item's JSON retry) marks the item
//! [`Outcome::Stopped`] and stops launching. `Cancelled` and every other
//! error (`Unrecorded`, fatal) return at once; the futures of the items
//! still in flight are dropped, but their spawned call tasks still finish
//! and book their charges, exactly as a single call does when the step's
//! future is dropped (Ruling R-8d).

use std::future::Future;
use std::time::{Duration, Instant};

use futures_util::stream::{FuturesUnordered, StreamExt};

use super::llm::StepError;
use crate::runs::budget::RunBudget;
use crate::runs::worker::CancelFlag;

/// What became of one item.
#[derive(Debug)]
pub enum Outcome<T> {
    /// The item's calls finished: `Ok`, or `Err(Llm | Parse)`.
    Done(Result<T, StepError>),
    /// Not read: the step stopped launching (wrap-up point or budget) before
    /// this item, or its own hold was refused (`OverBudget`).
    Stopped,
    /// `launch` said the item needs no call.
    Skipped,
}

/// Times an item that hit a rate limit is put back (Fix B review I-3).
pub const MAX_REQUEUES: u8 = 2;
/// Longest wait before a re-queued item is tried again.
pub const MAX_RATE_LIMIT_WAIT: Duration = Duration::from_secs(20);
/// The wait when the server gave no `retry-after`.
pub const DEFAULT_RATE_LIMIT_WAIT: Duration = Duration::from_secs(5);

#[cfg(test)]
thread_local! {
    /// Passes through the step loop (tests: no busy-waiting).
    static WAKES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// How long an item whose hold was refused beside **another step's**
/// calls in flight waits before it is tried again (Stage 3: the extractor
/// beside the skeptic). Only when this step has nothing of its own in
/// flight to wait for.
pub const OTHERS_POLL: Duration = Duration::from_millis(200);

/// Longest single sleep while an item waits, so a cancel is seen within it.
const CANCEL_POLL: Duration = Duration::from_millis(250);

/// Sleep until `at`, waking early (within [`CANCEL_POLL`]) once `cancel` is
/// set (Fix B re-review N-2). The cancel flag has no notifier, so it is
/// polled in slices.
async fn sleep_or_cancel(at: Instant, cancel: &CancelFlag) {
    loop {
        let left = at.saturating_duration_since(Instant::now());
        if left.is_zero() || cancel.is_cancelled() {
            return;
        }
        tokio::time::sleep(left.min(CANCEL_POLL)).await;
    }
}

/// Run the items `0..items` with at most `max_in_flight` in flight (see the
/// module docs).
///
/// * `launch(i)`: `None` when item `i` needs no call; otherwise the result
///   of starting its first call, which must take its hold before returning.
///   It may be called again for the same item (a deferred hold, or a
///   re-queue after a rate limit).
/// * `settled(i, r)`: called once per item as it finishes for good (in
///   completion order), before the next launch decision, e.g. to raise the
///   finish reserve.
///
/// **Rate limits** (Fix B review I-3). An item whose call still hits a rate
/// limit or overload after the client's own attempts
/// ([`StepError::RateLimited`], never billed) is not failed at once: it is
/// put back up to [`MAX_REQUEUES`] (2) times, after the server's
/// `retry-after` ([`DEFAULT_RATE_LIMIT_WAIT`] without one, at most
/// [`MAX_RATE_LIMIT_WAIT`]), but only if that wait ends before the run's
/// wrap-up time; and the step's concurrency is halved for the rest of the
/// step (at least 1). Still rate limited after that, it is
/// `Done(Err(RateLimited))`: the step counts it as failed (not assessed). A
/// re-queued item the step stops before re-launching is [`Outcome::Stopped`].
/// The count of rate limits is logged (no content).
pub async fn run<T, F>(
    budget: &RunBudget,
    cancel: &CancelFlag,
    step: &'static str,
    max_in_flight: usize,
    items: usize,
    mut launch: impl FnMut(usize) -> Option<Result<F, StepError>>,
    mut settled: impl FnMut(usize, &Result<T, StepError>),
) -> Result<Vec<Outcome<T>>, StepError>
where
    F: Future<Output = Result<T, StepError>>,
{
    let run_id = budget.run_id();
    let mut cap = max_in_flight.max(1);
    let mut out: Vec<Option<Outcome<T>>> = (0..items).map(|_| None).collect();
    let mut requeues: Vec<u8> = vec![0; items];
    // (ready at, item), for items put back after a rate limit.
    let mut waiting: Vec<(Instant, usize)> = Vec::new();
    let mut in_flight = FuturesUnordered::new();
    let mut next = 0usize;
    let mut launched = 0usize;
    let mut launching = true;
    let mut rate_limited = 0u32;

    // A hold was refused beside the calls in flight: nothing is tried again
    // until one of them settles (Fix B re-review N-1: no busy retries).
    let mut blocked = false;

    loop {
        #[cfg(test)]
        WAKES.with(|w| w.set(w.get() + 1));
        while launching && !blocked && in_flight.len() < cap {
            let now = Instant::now();
            // A re-queued item that is due goes first, then the next new one.
            let due = waiting
                .iter()
                .enumerate()
                .filter(|(_, (at, _))| *at <= now)
                .min_by_key(|(_, (at, _))| *at)
                .map(|(k, _)| k);
            if due.is_none() && next >= items {
                break;
            }
            if cancel.is_cancelled() {
                return Err(StepError::Cancelled);
            }
            // With calls in flight, only a wrap-up reason that does not
            // depend on them stops launching; their holds and reserve shares
            // are left to the hold check (refused: wait for one to settle).
            let wrap_up = if in_flight.is_empty() {
                budget.should_wrap_up(now)
            } else {
                budget.should_wrap_up_settled(now)
            };
            if launched > 0 && wrap_up {
                tracing::info!(%run_id, step, launched, "wrap-up reached; no further calls launched");
                launching = false;
                break;
            }
            let i = match due {
                Some(k) => waiting.swap_remove(k).1,
                None => {
                    next += 1;
                    next - 1
                }
            };
            match launch(i) {
                None => out[i] = Some(Outcome::Skipped),
                Some(Ok(f)) => {
                    launched += 1;
                    in_flight.push(async move { (i, f.await) });
                }
                Some(Err(StepError::OverBudget)) if !in_flight.is_empty() => {
                    // The hold does not fit beside the calls in flight; their
                    // real charges are usually below their holds, so try
                    // this item again once one has settled (not before).
                    if due.is_some() {
                        waiting.push((now, i));
                    } else {
                        next = i;
                    }
                    blocked = true;
                    break;
                }
                Some(Err(StepError::OverBudget))
                    if budget.held() > 0 && !budget.should_wrap_up_settled(now) =>
                {
                    // Refused while another step's calls are in flight (Full:
                    // the skeptic's turn beside extraction): this step has
                    // none to wait for, so try the same item again shortly,
                    // with every check made again (Stage 3).
                    waiting.push((now + OTHERS_POLL, i));
                    break;
                }
                Some(Err(StepError::OverBudget)) => {
                    tracing::info!(%run_id, step, launched, "over budget; no further calls launched");
                    out[i] = Some(Outcome::Stopped);
                    launching = false;
                }
                Some(Err(e @ (StepError::Llm | StepError::Parse))) => {
                    launched += 1;
                    let r = Err(e);
                    settled(i, &r);
                    out[i] = Some(Outcome::Done(r));
                }
                Some(Err(e)) => return Err(e),
            }
        }

        // Wait for a call to finish, or for a re-queued item to be due; a
        // due time counts only when the item could launch then (a free slot
        // and no refused hold waiting for a settle), so nothing spins.
        let next_due = if launching && !blocked && in_flight.len() < cap {
            waiting.iter().map(|(at, _)| *at).min()
        } else {
            None
        };
        if next_due.is_some() && cancel.is_cancelled() {
            // Woken by a cancel while an item waits (N-2): stop as a launch
            // would; the calls in flight still charge from their own tasks.
            return Err(StepError::Cancelled);
        }
        let finished = match (in_flight.is_empty(), next_due) {
            (true, None) => break,
            (true, Some(at)) => {
                sleep_or_cancel(at, cancel).await;
                continue;
            }
            (false, None) => in_flight.next().await,
            (false, Some(at)) => {
                tokio::select! {
                    r = in_flight.next() => r,
                    () = sleep_or_cancel(at, cancel) => continue,
                }
            }
        };
        let Some((i, r)) = finished else {
            break;
        };
        blocked = false;
        match r {
            Err(StepError::OverBudget) => {
                tracing::info!(%run_id, step, "over budget; no further calls launched");
                out[i] = Some(Outcome::Stopped);
                launching = false;
            }
            Err(StepError::RateLimited(retry_after)) => {
                rate_limited += 1;
                cap = (cap / 2).max(1);
                let now = Instant::now();
                let wait = retry_after
                    .unwrap_or(DEFAULT_RATE_LIMIT_WAIT)
                    .min(MAX_RATE_LIMIT_WAIT);
                if launching && requeues[i] < MAX_REQUEUES && wait < budget.until_wrap_up(now) {
                    requeues[i] += 1;
                    tracing::info!(%run_id, step, wait_ms = wait.as_millis() as u64, max_in_flight = cap, "rate limited; item re-queued");
                    waiting.push((now + wait, i));
                } else {
                    tracing::warn!(%run_id, step, max_in_flight = cap, "rate limited; item not read");
                    settled(i, &r);
                    out[i] = Some(Outcome::Done(r));
                }
            }
            Ok(_) | Err(StepError::Llm | StepError::Parse) => {
                settled(i, &r);
                out[i] = Some(Outcome::Done(r));
            }
            Err(e) => return Err(e),
        }
    }
    if rate_limited > 0 {
        tracing::warn!(%run_id, step, rate_limited, max_in_flight = cap, "step hit rate limits");
    }
    Ok(out
        .into_iter()
        .map(|o| o.unwrap_or(Outcome::Stopped))
        .collect())
}

#[cfg(all(test, feature = "test-util"))]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use uuid::Uuid;

    use super::*;
    use crate::spend::Prices;

    fn budget(limit: i64) -> Arc<RunBudget> {
        Arc::new(RunBudget::new(
            Uuid::new_v4(),
            0,
            limit,
            Instant::now(),
            Prices::from_json(r#"{"m": {"input": 1.0, "output": 5.0}}"#).unwrap(),
            88.0,
        ))
    }

    /// One launch as the budget saw it: wrap-up due just before it, and
    /// `spent + held + reserve` just after its hold was taken.
    #[derive(Debug, Clone, Copy)]
    struct Launch {
        item: usize,
        wrap_up_due: bool,
        committed: i64,
        in_flight: usize,
    }

    /// Items that hold `hold` paise at launch, take `ms[i]` milliseconds and
    /// are then charged `charge` (the hold is released, `spent` raised).
    async fn drive(
        b: &Arc<RunBudget>,
        cancel: &CancelFlag,
        k: usize,
        ms: &[u64],
        hold: i64,
        charge: i64,
    ) -> (Result<Vec<Outcome<usize>>, StepError>, Vec<Launch>, usize) {
        let log: Mutex<Vec<Launch>> = Mutex::new(Vec::new());
        let now_in_flight = Arc::new(AtomicUsize::new(0));
        let max_seen = Arc::new(AtomicUsize::new(0));
        let r = run(
            b,
            cancel,
            "test",
            k,
            ms.len(),
            |i| {
                let wrap_up_due = b.should_wrap_up(Instant::now());
                let Some(h) = b.try_hold_gathering(hold) else {
                    return Some(Err(StepError::OverBudget));
                };
                let n = now_in_flight.fetch_add(1, Ordering::SeqCst) + 1;
                max_seen.fetch_max(n, Ordering::SeqCst);
                log.lock().unwrap().push(Launch {
                    item: i,
                    wrap_up_due,
                    committed: b.spent() + b.held() + b.finish_reserve(),
                    in_flight: n,
                });
                let (b, now_in_flight, d) = (b.clone(), now_in_flight.clone(), ms[i]);
                Some(Ok(async move {
                    tokio::time::sleep(Duration::from_millis(d)).await;
                    drop(h);
                    b.record_spent_for_test(b.spent() + charge);
                    now_in_flight.fetch_sub(1, Ordering::SeqCst);
                    Ok(i)
                }))
            },
            |_, _| {},
        )
        .await;
        let log = log.into_inner().unwrap();
        (r, log, max_seen.load(Ordering::SeqCst))
    }

    #[tokio::test]
    async fn at_most_k_in_flight_and_outcomes_in_item_order() {
        let b = budget(1_000_000);
        // Later items finish first.
        let ms: Vec<u64> = (0..10).map(|i| 50 - 4 * i).collect();
        let (r, log, max) = drive(&b, &CancelFlag::default(), 4, &ms, 10, 5).await;
        let out = r.unwrap();
        assert_eq!(max, 4);
        assert!(log.iter().all(|l| l.in_flight <= 4));
        let items: Vec<usize> = out
            .iter()
            .map(|o| match o {
                Outcome::Done(Ok(i)) => *i,
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(items, (0..10).collect::<Vec<_>>());
        assert_eq!(b.held(), 0);
    }

    #[tokio::test]
    async fn no_launch_once_wrap_up_is_due_and_never_past_the_limit() {
        // Limit 1,000, reserve 100, holds and charges 100, up to 4 in
        // flight. With calls in flight the holds count twice (R-FB3): four
        // go out (100 + 2 × 300 ... 2 × 400 + 100 = 900), then one more each
        // time the doubled rule fits again (after charges 1, 3 and 5); after
        // the 7th charge spend is at 70%: wrap-up is due, nothing more is
        // launched.
        let b = budget(1_000);
        b.set_finish_reserve(100);
        let ms = vec![10; 12];
        let (r, log, max) = drive(&b, &CancelFlag::default(), 4, &ms, 100, 100).await;
        let out = r.unwrap();
        assert_eq!(max, 4);
        assert!(log.iter().skip(1).all(|l| !l.wrap_up_due), "{log:?}");
        assert!(log.iter().all(|l| l.committed <= 1_000), "{log:?}");
        assert_eq!(log.len(), 7, "{log:?}");
        let launched: Vec<usize> = log.iter().map(|l| l.item).collect();
        assert_eq!(launched, (0..7).collect::<Vec<_>>(), "in item order");
        assert!(b.should_wrap_up(Instant::now()));
        let stopped: Vec<usize> = out
            .iter()
            .enumerate()
            .filter(|(_, o)| matches!(o, Outcome::Stopped))
            .map(|(i, _)| i)
            .collect();
        assert_eq!(stopped, (7..12).collect::<Vec<_>>());
        assert_eq!(b.spent(), 700);
        assert!(b.spent() + b.finish_reserve() <= 1_000);
        assert_eq!(b.held(), 0);
    }

    #[tokio::test]
    async fn seventy_percent_spent_stops_launching() {
        // Limit 3,000, holds and charges 100: the doubled holds always fit
        // up to the 22nd charge (100 × 22 + 2 × 400 = 3,000), so only the
        // 70% rule stops launching: 4, then one per charge until the 21st
        // (2,100) makes wrap-up due; the 3 in flight finish.
        let b = budget(3_000);
        let ms = vec![5; 30];
        let (r, log, _) = drive(&b, &CancelFlag::default(), 4, &ms, 100, 100).await;
        let out = r.unwrap();
        assert!(log.iter().skip(1).all(|l| !l.wrap_up_due), "{log:?}");
        assert_eq!(log.len(), 24);
        assert_eq!(
            out.iter().filter(|o| matches!(o, Outcome::Stopped)).count(),
            6
        );
        assert_eq!(b.spent(), 2_400, "the calls in flight finish");
    }

    #[tokio::test]
    async fn refused_hold_waits_for_a_call_to_settle() {
        // Holds of 200 counted twice in flight (R-FB3): two fit in 1,000
        // (2 × 400), a third (2 × 600) does not; each call costs 100, so it
        // fits once one has settled (100 + 2 × 400 = 900): every item runs,
        // never more than two at once.
        let b = budget(1_000);
        let ms = vec![10; 5];
        let (r, log, max) = drive(&b, &CancelFlag::default(), 4, &ms, 200, 100).await;
        let out = r.unwrap();
        assert_eq!(max, 2);
        assert!(log.iter().all(|l| l.committed <= 1_000), "{log:?}");
        assert!(out.iter().all(|o| matches!(o, Outcome::Done(Ok(_)))));
        assert_eq!(b.spent(), 500);
    }

    #[tokio::test]
    async fn refused_hold_with_nothing_in_flight_stops() {
        let b = budget(1_000);
        let ms = vec![10; 3];
        let (r, log, _) = drive(&b, &CancelFlag::default(), 4, &ms, 1_200, 100).await;
        assert!(log.is_empty());
        assert!(r.unwrap().iter().all(|o| matches!(o, Outcome::Stopped)));
    }

    #[tokio::test]
    async fn cancel_and_fatal_errors_end_the_step() {
        let cancel = CancelFlag::default();
        cancel.cancel();
        let (r, log, _) = drive(&budget(1_000), &cancel, 4, &[10; 3], 10, 10).await;
        assert_eq!(r.unwrap_err(), StepError::Cancelled);
        assert!(log.is_empty());

        // An in-flight item's fatal error returns at once; Llm / Parse are
        // the item's own failure; OverBudget from a retry stops launching.
        let b = budget(1_000);
        let r: Result<Vec<Outcome<()>>, StepError> = run(
            &b,
            &CancelFlag::default(),
            "test",
            2,
            4,
            |i| {
                Some(Ok(async move {
                    match i {
                        0 => Err(StepError::Parse),
                        1 => Err(StepError::OverBudget),
                        _ => Ok(()),
                    }
                }))
            },
            |_, _| {},
        )
        .await;
        let out = r.unwrap();
        assert!(matches!(out[0], Outcome::Done(Err(StepError::Parse))));
        assert!(matches!(out[1], Outcome::Stopped));
        assert!(
            matches!(out[2], Outcome::Done(Ok(()))),
            "launched before the stop"
        );
        assert!(matches!(out[3], Outcome::Stopped));

        let r: Result<Vec<Outcome<()>>, StepError> = run(
            &b,
            &CancelFlag::default(),
            "test",
            2,
            3,
            |i| {
                Some(Ok(async move {
                    if i == 1 {
                        Err(StepError::Unrecorded)
                    } else {
                        Ok(())
                    }
                }))
            },
            |_, _| {},
        )
        .await;
        assert_eq!(r.unwrap_err(), StepError::Unrecorded);
    }

    /// Items 0..n taking `ms` each; item 0's first `fails` attempts are rate
    /// limited (retry-after `ra`). Returns the outcomes and, per launch,
    /// (item, calls in flight including it).
    async fn rate_limited_run(
        b: &Arc<RunBudget>,
        n: usize,
        k: usize,
        fails: usize,
        ra: Option<Duration>,
    ) -> (Vec<Outcome<usize>>, Vec<(usize, usize)>) {
        let attempts = Mutex::new(vec![0usize; n]);
        let log = Mutex::new(Vec::new());
        let now_in_flight = Arc::new(AtomicUsize::new(0));
        let out = run(
            b,
            &CancelFlag::default(),
            "test",
            k,
            n,
            |i| {
                let a = {
                    let mut at = attempts.lock().unwrap();
                    at[i] += 1;
                    at[i]
                };
                let f = now_in_flight.fetch_add(1, Ordering::SeqCst) + 1;
                log.lock().unwrap().push((i, f));
                let now_in_flight = now_in_flight.clone();
                Some(Ok(async move {
                    let limited = i == 0 && a <= fails;
                    if !limited {
                        tokio::time::sleep(Duration::from_millis(20)).await;
                    }
                    now_in_flight.fetch_sub(1, Ordering::SeqCst);
                    if limited {
                        Err(StepError::RateLimited(ra))
                    } else {
                        Ok(i)
                    }
                }))
            },
            |_, _| {},
        )
        .await
        .unwrap();
        (out, log.into_inner().unwrap())
    }

    #[tokio::test]
    async fn rate_limited_item_is_requeued_and_concurrency_halved() {
        let b = budget(1_000_000);
        let (out, log) = rate_limited_run(&b, 8, 4, 1, Some(Duration::from_millis(10))).await;
        // Item 0 failed at once, was put back and then read.
        assert!(
            out.iter().all(|o| matches!(o, Outcome::Done(Ok(_)))),
            "{out:?}"
        );
        assert_eq!(log.iter().filter(|(i, _)| *i == 0).count(), 2);
        // The first four went out together; after the rate limit, never
        // more than 2 at once.
        assert_eq!(log[3].1, 4);
        assert!(log[4..].iter().all(|(_, f)| *f <= 2), "{log:?}");
    }

    #[tokio::test]
    async fn persistent_rate_limit_fails_the_item_after_two_requeues() {
        let b = budget(1_000_000);
        let (out, log) = rate_limited_run(&b, 3, 4, 10, Some(Duration::from_millis(5))).await;
        assert!(matches!(
            out[0],
            Outcome::Done(Err(StepError::RateLimited(_)))
        ));
        assert!(matches!(out[1], Outcome::Done(Ok(1))));
        assert_eq!(
            log.iter().filter(|(i, _)| *i == 0).count(),
            3,
            "1 + 2 re-queues"
        );
    }

    #[tokio::test]
    async fn no_requeue_when_the_wait_would_pass_the_wrap_up_time() {
        // 179.9 s into a 300 s run: wrap-up at 180 s, retry-after 5 s.
        let b = Arc::new(
            RunBudget::new(
                Uuid::new_v4(),
                0,
                1_000_000,
                Instant::now(),
                Prices::from_json(r#"{"m": {"input": 1.0, "output": 5.0}}"#).unwrap(),
                88.0,
            )
            .aged_for_test(Duration::from_millis(179_900)),
        );
        let (out, log) = rate_limited_run(&b, 1, 4, 1, Some(Duration::from_secs(5))).await;
        assert!(matches!(
            out[0],
            Outcome::Done(Err(StepError::RateLimited(_)))
        ));
        assert_eq!(log.len(), 1, "not waited for");
    }

    /// Extract-like items: hold `h`, charge `c`, each adds `g` to the
    /// finish reserve once read; while in flight each keeps `share` of it
    /// free (taken after its hold). Returns (papers read, spent, reserve).
    async fn extract_like(
        limit: i64,
        r0: i64,
        h: i64,
        c: i64,
        g: i64,
        share: i64,
    ) -> (usize, i64, i64) {
        let b = budget(limit);
        b.set_finish_reserve(r0);
        let read = Mutex::new(0i64);
        let out = run(
            &b,
            &CancelFlag::default(),
            "test",
            3,
            4,
            |_| {
                let hold = match b.try_hold_gathering(h) {
                    Some(hold) => hold,
                    None => return Some(Err(StepError::OverBudget)),
                };
                let s = b.reserve_in_flight(share);
                let b = b.clone();
                Some(Ok(async move {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                    drop((hold, s));
                    b.record_spent_for_test(b.spent() + c);
                    Ok(())
                }))
            },
            |_, r: &Result<(), StepError>| {
                if r.is_ok() {
                    let mut n = read.lock().unwrap();
                    *n += 1;
                    b.set_finish_reserve(r0 + g * *n);
                }
            },
        )
        .await
        .unwrap();
        let n = out
            .iter()
            .filter(|o| matches!(o, Outcome::Done(Ok(()))))
            .count();
        assert_eq!(b.in_flight_reserve(), 0);
        (n, b.spent(), b.finish_reserve())
    }

    #[tokio::test]
    async fn in_flight_papers_keep_their_growth_free_so_write_stays_affordable() {
        // Ruling R-FB4. Limit 1,850, finish reserve 600, holds and charges
        // 300, each paper read adds 700 to the reserve. Without shares the
        // second paper launches beside the first (2 × 600 + 600 = 1,800 <=
        // 1,850) and, both read, Write's reserve no longer fits (600 + 2,000
        // > 1,850). With each in-flight paper keeping 700 free it does not
        // launch; once the first is read it no longer fits alone (300 + 300
        // + 1,300 > 1,850): no further launch, and Write is affordable.
        let (read, spent, reserve) = extract_like(1_850, 600, 300, 300, 700, 0).await;
        assert_eq!(read, 2);
        assert!(spent + reserve > 1_850, "without R-FB4 Write is refused");

        let (read, spent, reserve) = extract_like(1_850, 600, 300, 300, 700, 700).await;
        assert_eq!(read, 1);
        assert!(
            spent + reserve <= 1_850,
            "Write is affordable: {spent} + {reserve}"
        );
    }

    #[tokio::test]
    async fn a_lone_paper_launches_as_before() {
        // The share counts only for other launches: one paper, nothing in
        // flight, exactly the single-call rule (300 + 600 = 900 <= 900).
        let b = budget(900);
        b.set_finish_reserve(600);
        let h = b.try_hold_gathering(300).unwrap();
        let s = b.reserve_in_flight(700);
        assert!(
            b.should_wrap_up(Instant::now()),
            "the share counts for others"
        );
        assert!(!b.should_wrap_up_settled(Instant::now()));
        drop((h, s));
        assert_eq!((b.held(), b.in_flight_reserve()), (0, 0));
        assert!(!b.should_wrap_up(Instant::now()));
    }

    /// Item `i` takes `ms[i]` and costs `charges[i]`; `rl`'s first call is
    /// rate limited (retry-after `ra`); each launch takes a hold of
    /// `holds[i]`. Returns the outcomes, launch attempts per item (refused
    /// holds included) and passes through the step loop.
    async fn requeue_case(
        b: &Arc<RunBudget>,
        k: usize,
        ms: &[u64],
        holds: &[i64],
        charges: &[i64],
        rl: usize,
        ra: Duration,
    ) -> (Vec<Outcome<usize>>, Vec<usize>, usize) {
        let attempts = Mutex::new(vec![0usize; ms.len()]);
        let calls = Mutex::new(vec![0usize; ms.len()]);
        WAKES.with(|w| w.set(0));
        let out = run(
            b,
            &CancelFlag::default(),
            "test",
            k,
            ms.len(),
            |i| {
                attempts.lock().unwrap()[i] += 1;
                let Some(h) = b.try_hold_gathering(holds[i]) else {
                    return Some(Err(StepError::OverBudget));
                };
                let a = {
                    let mut c = calls.lock().unwrap();
                    c[i] += 1;
                    c[i]
                };
                let (b, d, cost) = (b.clone(), ms[i], charges[i]);
                Some(Ok(async move {
                    let limited = i == rl && a == 1;
                    if !limited {
                        tokio::time::sleep(Duration::from_millis(d)).await;
                    }
                    drop(h);
                    if limited {
                        Err(StepError::RateLimited(Some(ra)))
                    } else {
                        b.record_spent_for_test(b.spent() + cost);
                        Ok(i)
                    }
                }))
            },
            |_, _| {},
        )
        .await
        .unwrap();
        let wakes = WAKES.with(std::cell::Cell::get);
        (out, attempts.into_inner().unwrap(), wakes)
    }

    #[tokio::test]
    async fn requeued_item_with_no_free_slot_does_not_spin() {
        // N-1 case A: three go out; item 2 is rate limited at once, so the
        // concurrency drops to 1 while items 0 and 1 run for 300 ms. Item 2
        // is due after 10 ms but has no slot: the loop sleeps until a call
        // settles instead of waking again and again.
        let b = budget(1_000_000);
        let (out, attempts, wakes) = requeue_case(
            &b,
            3,
            &[300, 300, 10],
            &[10, 10, 10],
            &[1, 1, 1],
            2,
            Duration::from_millis(10),
        )
        .await;
        assert!(
            out.iter().all(|o| matches!(o, Outcome::Done(Ok(_)))),
            "{out:?}"
        );
        assert_eq!(attempts, vec![1, 1, 2]);
        assert!(wakes <= 8, "{wakes} passes through the loop");
    }

    #[tokio::test]
    async fn requeued_item_whose_hold_is_refused_waits_for_a_settle() {
        // N-1 case B: limit 2,000. Item 0 (hold 400) runs 300 ms; item 1
        // (hold 400) is rate limited at once (due after 10 ms); item 2 (hold
        // 50) settles after 10 ms and costs 500. Item 1 is then due with a
        // free slot, but beside item 0 it needs 500 + 2 × 800 > 2,000: its
        // hold is refused, and it is tried again only once item 0 has
        // settled (then alone it fits): 3 attempts, not hundreds.
        let b = budget(2_000);
        let (out, attempts, wakes) = requeue_case(
            &b,
            4,
            &[300, 10, 10],
            &[400, 400, 50],
            &[1, 1, 500],
            1,
            Duration::from_millis(10),
        )
        .await;
        assert!(
            out.iter().all(|o| matches!(o, Outcome::Done(Ok(_)))),
            "{out:?}"
        );
        assert_eq!(attempts, vec![1, 3, 1], "429, refused once, then read");
        assert!(wakes <= 8, "{wakes} passes through the loop");
    }

    #[tokio::test]
    async fn cancel_wakes_a_requeue_sleep() {
        // N-2: the only item waits 10 s after a rate limit; a cancel 50 ms
        // in ends the step within the poll interval.
        let b = budget(1_000_000);
        let cancel = CancelFlag::default();
        let c2 = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            c2.cancel();
        });
        let t0 = Instant::now();
        let r: Result<Vec<Outcome<()>>, StepError> = run(
            &b,
            &cancel,
            "test",
            4,
            1,
            |_| {
                Some(Ok(async {
                    Err(StepError::RateLimited(Some(Duration::from_secs(10))))
                }))
            },
            |_, _| {},
        )
        .await;
        assert_eq!(r.unwrap_err(), StepError::Cancelled);
        assert!(t0.elapsed() < Duration::from_secs(1), "{:?}", t0.elapsed());
    }
}
