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
//! * before every launch, the cancel flag and the wrap-up point
//!   ([`RunBudget::should_wrap_up`]: 70% spent, wrap-up time, or no headroom
//!   left above the finish reserve, with every in-flight hold counted) are
//!   checked; once wrap-up is reached no new item is launched (the first
//!   item is always launched, as before Fix B);
//! * every hold is taken atomically against `spent + held + estimate +
//!   finish reserve` ([`RunBudget::try_hold_gathering`]), so concurrent
//!   holds can never together pass the run's limit. A hold refused while
//!   other calls are in flight is tried again (same item, wrap-up checked
//!   again) after the next one settles, since a settled call usually frees
//!   part of its hold; refused with nothing in flight (`OverBudget`, as
//!   before Fix B), it stops launching;
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
use std::time::Instant;

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

/// Run the items `0..items` with at most `max_in_flight` in flight (see the
/// module docs).
///
/// * `launch(i)`: `None` when item `i` needs no call; otherwise the result
///   of starting its first call, which must take its hold before returning.
/// * `settled(i, r)`: called as each item finishes (in completion order),
///   before the next launch decision, e.g. to raise the finish reserve.
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
    let max_in_flight = max_in_flight.max(1);
    let mut out: Vec<Option<Outcome<T>>> = (0..items).map(|_| None).collect();
    let mut in_flight = FuturesUnordered::new();
    let mut next = 0usize;
    let mut launched = 0usize;
    let mut launching = true;

    loop {
        while launching && in_flight.len() < max_in_flight && next < items {
            if cancel.is_cancelled() {
                return Err(StepError::Cancelled);
            }
            if launched > 0 && budget.should_wrap_up(Instant::now()) {
                tracing::info!(%run_id, step, launched, "wrap-up reached; no further calls launched");
                launching = false;
                break;
            }
            let i = next;
            next += 1;
            match launch(i) {
                None => out[i] = Some(Outcome::Skipped),
                Some(Ok(f)) => {
                    launched += 1;
                    in_flight.push(async move { (i, f.await) });
                }
                Some(Err(StepError::OverBudget)) if !in_flight.is_empty() => {
                    // The hold does not fit beside the calls in flight; their
                    // real charges are usually below their holds, so try
                    // this item again once one has settled.
                    next = i;
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
        let Some((i, r)) = in_flight.next().await else {
            break;
        };
        match r {
            Err(StepError::OverBudget) => {
                tracing::info!(%run_id, step, "over budget; no further calls launched");
                out[i] = Some(Outcome::Stopped);
                launching = false;
            }
            Ok(_) | Err(StepError::Llm | StepError::Parse) => {
                settled(i, &r);
                out[i] = Some(Outcome::Done(r));
            }
            Err(e) => return Err(e),
        }
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
        // Limit 1,000, reserve 100, holds and charges 100 each, 4 in flight.
        // After the 5th charge, spent 500 + 3 held (300) + reserve = 1,000:
        // no headroom left, so wrap-up is due; nothing more is launched,
        // the calls in flight finish.
        let b = budget(1_000);
        b.set_finish_reserve(100);
        let ms = vec![10; 12];
        let (r, log, max) = drive(&b, &CancelFlag::default(), 4, &ms, 100, 100).await;
        let out = r.unwrap();
        assert_eq!(max, 4);
        assert!(log.iter().skip(1).all(|l| !l.wrap_up_due), "{log:?}");
        assert!(log.iter().all(|l| l.committed <= 1_000), "{log:?}");
        assert_eq!(log.len(), 9, "4, then one per settled call until wrap-up");
        let launched: Vec<usize> = log.iter().map(|l| l.item).collect();
        assert_eq!(launched, (0..9).collect::<Vec<_>>(), "in item order");
        assert!(b.should_wrap_up(Instant::now()));
        let stopped: Vec<usize> = out
            .iter()
            .enumerate()
            .filter(|(_, o)| matches!(o, Outcome::Stopped))
            .map(|(i, _)| i)
            .collect();
        assert_eq!(stopped, vec![9, 10, 11]);
        assert_eq!(b.spent(), 900);
        assert!(b.spent() + b.finish_reserve() <= 1_000);
        assert_eq!(b.held(), 0);
    }

    #[tokio::test]
    async fn seventy_percent_spent_stops_launching() {
        // Limit 2,000, holds and charges 100: the holds always fit (at most
        // 1,400 + 400), so only the 70% rule stops launching: 4, then one
        // per charge until the 14th charge (1,400) makes wrap-up due; the
        // 3 in flight finish.
        let b = budget(2_000);
        let ms = vec![10; 20];
        let (r, log, _) = drive(&b, &CancelFlag::default(), 4, &ms, 100, 100).await;
        let out = r.unwrap();
        assert!(log.iter().skip(1).all(|l| !l.wrap_up_due), "{log:?}");
        assert_eq!(log.len(), 17);
        assert_eq!(
            out.iter().filter(|o| matches!(o, Outcome::Stopped)).count(),
            3
        );
        assert_eq!(b.spent(), 1_700, "the calls in flight finish");
    }

    #[tokio::test]
    async fn refused_hold_waits_for_a_call_to_settle() {
        // Room for two holds of 400 in 1,000; each call costs 100. The third
        // hold is refused while two are in flight, then fits once one has
        // settled: every item runs, never more than two at once.
        let b = budget(1_000);
        let ms = vec![10; 5];
        let (r, log, max) = drive(&b, &CancelFlag::default(), 4, &ms, 400, 100).await;
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
}
