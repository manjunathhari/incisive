//! Per-run money and time budget. Server only.
//!
//! The run's full cost limit is reserved up front (`spend::reserve`). Every
//! Claude call then charges its real cost through [`RunBudget::charge`], which
//! books it against that reservation; the end of the run releases the rest.
//! All amounts are integer paise.

use std::collections::HashMap;
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use claude_client::Usage;
use sqlx::PgPool;
use uuid::Uuid;

use super::store;
use crate::audit::{self, Event};
use crate::spend::{self, Prices};

/// Wrap up (jump to Write + Check) once this share of the cost limit is spent.
const WRAP_UP_PERCENT: i128 = 70;
/// The default time limit (`DepthSettings::time_limit_s`), 300 s.
const DEFAULT_TIME_LIMIT_S: u32 = 300;

/// Wrap up (jump to Write + Check) once 60% of `time_limit_s` has passed:
/// 180 s at the default 300 s.
pub const fn wrap_up_after(time_limit_s: u32) -> Duration {
    Duration::from_millis(time_limit_s as u64 * 600)
}

fn millis(d: Duration) -> u64 {
    u64::try_from(d.as_millis()).unwrap_or(u64::MAX)
}

/// The hard stop for `time_limit_s`: the limit plus 60 s (360 s at the
/// default 300 s). Past it the run is failed.
pub const fn hard_stop_after(time_limit_s: u32) -> Duration {
    Duration::from_secs(time_limit_s as u64 + 60)
}

#[derive(Debug)]
pub enum BudgetError {
    /// The model has no configured price (or the rate is unusable): never free.
    /// Nothing was booked.
    UnknownModel,
    /// The charge was not booked (the ledger transaction failed). Safe to retry
    /// with the same call id.
    NotBooked(sqlx::Error),
    /// The charge **was** booked and `spent` is updated, but the run's cost
    /// update or the audit entry failed. Do not treat as unbooked: the money is
    /// booked. A retry with the same call id books nothing again and only
    /// completes the missing record.
    BookedButNotRecorded(sqlx::Error),
}

impl fmt::Display for BudgetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BudgetError::UnknownModel => write!(f, "model has no usable price"),
            BudgetError::NotBooked(_) => write!(f, "could not book the charge"),
            BudgetError::BookedButNotRecorded(_) => {
                write!(f, "charge booked but its record is incomplete")
            }
        }
    }
}

impl std::error::Error for BudgetError {}

/// What has been done for one call id, so a retry never repeats a step.
#[derive(Debug, Clone, Copy)]
struct CallState {
    paise: i64,
    cost_added: bool,
    audited: bool,
}

#[derive(Debug, Default)]
struct State {
    spent: i64,
    held: i64,
    calls: HashMap<Uuid, CallState>,
    /// Set by the worker once the run has ended ([`RunBudget::finish`]).
    finished: bool,
    /// Paise kept free for the finishing steps (Write, and Check at
    /// `MAX_CLAIMS`): gathering calls may not hold into it
    /// ([`RunBudget::try_hold_gathering`]). Ruling R-13c.
    finish_reserve: i64,
    /// What the papers still being extracted may add to the finish reserve
    /// once their findings are in ([`InFlightReserve`], Ruling R-FB4).
    /// Counted with `finish_reserve` everywhere.
    in_flight_reserve: i64,
}

/// A share of the finish reserve kept for one paper being extracted: what
/// its findings may add to Write's and Check's holds (Ruling R-FB4).
/// Dropped when the paper's call finishes, just before Extract raises the
/// finish reserve for the findings it actually gave.
#[derive(Debug)]
#[must_use = "the share is released when dropped"]
pub struct InFlightReserve {
    budget: Arc<RunBudget>,
    amount: i64,
}

impl InFlightReserve {
    /// Paise this share keeps free.
    pub fn amount(&self) -> i64 {
        self.amount
    }
}

impl Drop for InFlightReserve {
    fn drop(&mut self) {
        self.budget.state().in_flight_reserve -= self.amount;
    }
}

/// One consistent snapshot of a budget's money ([`RunBudget::committed`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Committed {
    pub spent: i64,
    pub held: i64,
    pub finish_reserve: i64,
    pub in_flight_reserve: i64,
}

/// An in-flight reservation of part of the run's limit, taken before a Claude
/// call. Pass it to [`RunBudget::charge`] to turn it into real spend; dropping
/// it without charging releases it.
#[derive(Debug)]
#[must_use = "a Hold frees its amount when dropped; pass it to RunBudget::charge"]
pub struct Hold {
    budget: Arc<RunBudget>,
    amount: i64,
}

impl Hold {
    pub fn amount(&self) -> i64 {
        self.amount
    }
}

impl Drop for Hold {
    fn drop(&mut self) {
        if self.amount > 0 {
            self.budget.state().held -= self.amount;
        }
    }
}

#[derive(Debug)]
pub struct RunBudget {
    run_id: Uuid,
    user_id: i64,
    limit_paise: i64,
    started: Instant,
    prices: Prices,
    usd_inr: f64,
    state: Mutex<State>,
    /// Added to the run's age (tests only, [`RunBudget::aged_for_test`]), so
    /// time-limit tests do not depend on the host's monotonic clock.
    age_offset: Duration,
    /// From the run's `time_limit_s` ([`RunBudget::with_time_limit`]), in
    /// milliseconds; moved by the team pipeline between its phases
    /// ([`RunBudget::set_wrap_up_after`]).
    wrap_up_after_ms: AtomicU64,
    hard_stop_after: Duration,
}

impl RunBudget {
    pub fn new(
        run_id: Uuid,
        user_id: i64,
        limit_paise: i64,
        started: Instant,
        prices: Prices,
        usd_inr: f64,
    ) -> RunBudget {
        RunBudget {
            run_id,
            user_id,
            limit_paise,
            started,
            prices,
            usd_inr,
            state: Mutex::new(State::default()),
            age_offset: Duration::ZERO,
            wrap_up_after_ms: AtomicU64::new(millis(wrap_up_after(DEFAULT_TIME_LIMIT_S))),
            hard_stop_after: hard_stop_after(DEFAULT_TIME_LIMIT_S),
        }
    }

    /// Time limits from the run's `time_limit_s` (`RUN_LIMITS`): wrap-up at
    /// 60% of it, hard stop at it plus 60 s. Without this, the defaults for
    /// 300 s apply (180 s and 360 s).
    pub fn with_time_limit(mut self, time_limit_s: u32) -> RunBudget {
        self.wrap_up_after_ms = AtomicU64::new(millis(wrap_up_after(time_limit_s)));
        self.hard_stop_after = hard_stop_after(time_limit_s);
        self
    }

    /// Wrap up at `after` from the start instead (Phase 3: the time limit
    /// less the writer's allowance). Call after [`RunBudget::with_time_limit`].
    pub fn with_wrap_up_after(self, after: Duration) -> RunBudget {
        self.set_wrap_up_after(after);
        self
    }

    /// Move the wrap-up point to `after` from the start (Phase 3 Full: once
    /// research is done, the review phase wraps up at the time limit less
    /// the writer's allowance).
    pub fn set_wrap_up_after(&self, after: Duration) {
        self.wrap_up_after_ms.store(millis(after), Ordering::SeqCst);
    }

    /// When this run wraps up for time.
    pub fn wrap_up_after(&self) -> Duration {
        Duration::from_millis(self.wrap_up_after_ms.load(Ordering::SeqCst))
    }

    /// Time left before the hard stop; zero once it has passed.
    pub fn until_hard_stop(&self, now: Instant) -> Duration {
        self.hard_stop_after.saturating_sub(self.age(now))
    }

    /// When this run is failed for time (see [`RunBudget::with_time_limit`]).
    pub fn hard_stop(&self) -> Duration {
        self.hard_stop_after
    }

    /// Tests only: the run behaves as if it started `by` earlier.
    #[cfg(feature = "test-util")]
    pub fn aged_for_test(mut self, by: Duration) -> RunBudget {
        self.age_offset = by;
        self
    }

    /// Time since the run started.
    fn age(&self, now: Instant) -> Duration {
        now.saturating_duration_since(self.started) + self.age_offset
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Atomically hold `estimate_paise` of the limit for one call about to be
    /// made. Succeeds only if `spent + held + estimate <= limit`, so concurrent
    /// calls cannot all pass the check before any is charged.
    ///
    /// The hold owns a share of the budget, so it can move into a task that
    /// outlives the caller.
    pub fn try_hold(self: &Arc<Self>, estimate_paise: i64) -> Option<Hold> {
        self.hold_leaving(estimate_paise, false)
    }

    /// [`RunBudget::try_hold`] for a gathering call (Frame through Extract):
    /// it must also leave the finish reserve free, so the evidence gathered
    /// can always be written up and checked. Ruling R-13c.
    ///
    /// With other calls in flight (`held > 0`; Screen and Extract run several
    /// at once, Fix B), the holds count twice (Ruling R-FB3): it needs
    /// `spent + 2 × (held + estimate) + finish_reserve <= limit`. The doubled
    /// part is room for the calls in flight to cost more than their holds
    /// (input tokens are estimated), so concurrency does not widen the
    /// worst-case overshoot. With nothing in flight it is exactly the
    /// single-call rule `spent + estimate + finish_reserve <= limit`.
    pub fn try_hold_gathering(self: &Arc<Self>, estimate_paise: i64) -> Option<Hold> {
        self.hold_leaving(estimate_paise, true)
    }

    /// [`RunBudget::try_hold_gathering`] (or, with `gathering == false`,
    /// [`RunBudget::try_hold`]) for the call `share` was kept for: the share
    /// is released and the hold taken under one lock, so no concurrent call
    /// can take the headroom in between (Phase 3: the agent loop keeps its
    /// final structured call's estimate free while it runs tool turns, then
    /// makes that call from it). The share is gone either way; refused, the
    /// caller makes no call.
    ///
    /// **In-flight holds count once here** (Stage 3, the deferred Stage 1
    /// issue), exactly as the loop's own affordability check (`Final::fits`)
    /// counts them, so a share that fits is a hold that fits even while
    /// other calls (the extractor's) are in flight. The doubling of R-FB3
    /// is room for calls in flight to cost more than their holds; every
    /// gathering launch beside this share already left that room *and* the
    /// share free (each counts itself and all in-flight holds twice, plus
    /// the share once), so moving the share into `held` takes no headroom
    /// another call was promised. The call made from the share is then a
    /// single call like any other: its own input excess is the documented
    /// single-call figure (`llm::estimate_usage`), absorbed by the finish
    /// reserve first, and later launches count its hold twice.
    pub fn try_hold_releasing(
        self: &Arc<Self>,
        estimate_paise: i64,
        share: InFlightReserve,
        gathering: bool,
    ) -> Option<Hold> {
        let mut share = share;
        let amount = std::mem::take(&mut share.amount);
        // Dropped with nothing left (its own lock is taken and freed here).
        drop(share);
        let mut st = self.state();
        st.in_flight_reserve -= amount;
        self.hold_locked(&mut st, estimate_paise, gathering, false)
    }

    fn hold_leaving(self: &Arc<Self>, estimate_paise: i64, reserve: bool) -> Option<Hold> {
        let mut st = self.state();
        self.hold_locked(&mut st, estimate_paise, reserve, reserve)
    }

    /// `reserve`: leave the finish reserve and the in-flight shares free;
    /// `double`: count in-flight holds twice when there are any (R-FB3).
    fn hold_locked(
        self: &Arc<Self>,
        st: &mut MutexGuard<'_, State>,
        estimate_paise: i64,
        reserve: bool,
        double: bool,
    ) -> Option<Hold> {
        if estimate_paise < 0 {
            return None;
        }
        let kept = if reserve {
            st.finish_reserve + st.in_flight_reserve
        } else {
            0
        };
        // In-flight headroom (Ruling R-FB3): a gathering call launched while
        // others are in flight must fit with every in-flight hold, its own
        // included, counted twice. With nothing in flight the rule is the
        // one a single call always had.
        let factor = if double && st.held > 0 { 2 } else { 1 };
        let total = i128::from(st.spent)
            + factor * (i128::from(st.held) + i128::from(estimate_paise))
            + i128::from(kept);
        if total > i128::from(self.limit_paise) {
            return None;
        }
        st.held += estimate_paise;
        Some(Hold {
            budget: Arc::clone(self),
            amount: estimate_paise,
        })
    }

    /// Charge one Claude call's real cost to the run and return the paise.
    ///
    /// The caller makes `call_id` once per Claude call and reuses it on every
    /// retry of that call: the ledger holds at most one charge per call id, and
    /// this budget counts it, adds it to the run's cost and audits it once.
    /// `hold` is consumed: its amount is replaced by the real cost, or freed
    /// when nothing was booked. A caller that will re-send the charge should
    /// use [`RunBudget::charge_held`], which keeps the hold until the money is
    /// booked.
    ///
    /// Order: book against the reservation, count it in `spent`, add it to the
    /// run's cost, then audit it (ids, model, tokens and paise only).
    /// See [`BudgetError`] for what each failure means.
    pub async fn charge(
        &self,
        pool: &PgPool,
        hold: Hold,
        call_id: Uuid,
        model: &str,
        usage: &Usage,
    ) -> Result<i64, BudgetError> {
        let mut hold = hold;
        self.charge_held(pool, &mut hold, call_id, model, usage)
            .await
    }

    /// [`RunBudget::charge`] that leaves `hold` in place unless the charge is
    /// booked (Fix B review I-2): on `UnknownModel` or `NotBooked` the hold
    /// still counts in `held`, so no concurrent call can take that headroom
    /// while the caller re-sends the charge or counts the money as unbooked.
    /// Once booked (including `BookedButNotRecorded`), the hold's amount is
    /// zero: it has become spend.
    pub async fn charge_held(
        &self,
        pool: &PgPool,
        hold: &mut Hold,
        call_id: Uuid,
        model: &str,
        usage: &Usage,
    ) -> Result<i64, BudgetError> {
        let paise = spend::cost_paise(&self.prices, self.usd_inr, model, usage).map_err(|_| {
            // Unpriced model or unusable rate: fail closed, never free.
            tracing::error!(run_id = %self.run_id, "run call has no usable price");
            BudgetError::UnknownModel
        })?;
        let inserted = spend::charge_call(
            pool,
            self.user_id,
            self.run_id,
            call_id,
            model,
            usage,
            paise,
        )
        .await
        .map_err(|e| {
            // Ids only: the error could carry stored values.
            tracing::error!(run_id = %self.run_id, %call_id, "run charge not booked");
            BudgetError::NotBooked(e)
        })?;

        // Booked. Convert the hold into spend exactly once per call id.
        let held = std::mem::take(&mut hold.amount);
        let (paise, mut call) = {
            let mut st = self.state();
            st.held -= held;
            match st.calls.get(&call_id) {
                Some(c) => (c.paise, *c),
                None => {
                    if !inserted {
                        // Booked by an earlier attempt whose outcome this
                        // budget never saw; count it, never book it again.
                        tracing::warn!(run_id = %self.run_id, %call_id, "charge replayed without local record");
                    }
                    st.spent += paise;
                    let c = CallState {
                        paise,
                        cost_added: false,
                        audited: false,
                    };
                    st.calls.insert(call_id, c);
                    (paise, c)
                }
            }
        };

        let incomplete = |what: &'static str| {
            let run_id = self.run_id;
            move |e| {
                tracing::error!(%run_id, %call_id, step = what, "run charge booked but not recorded");
                BudgetError::BookedButNotRecorded(e)
            }
        };
        if !call.cost_added {
            store::add_cost(pool, self.run_id, paise)
                .await
                .map_err(incomplete("add_cost"))?;
            call.cost_added = true;
            self.state().calls.insert(call_id, call);
        }
        if !call.audited {
            audit::append(
                pool,
                Some(self.run_id),
                Some(self.user_id),
                Event::Charged,
                audit::charged_detail(Some(model), Some(usage), paise),
            )
            .await
            .map_err(incomplete("audit"))?;
            call.audited = true;
            self.state().calls.insert(call_id, call);
        }
        Ok(paise)
    }

    /// The run this budget belongs to (for logging).
    pub fn run_id(&self) -> Uuid {
        self.run_id
    }

    /// Cost in paise of `usage` on `model`, rounded up (`spend::cost_paise`).
    /// Used for the pre-call estimate; an unpriced model is an error.
    pub fn cost_of(&self, model: &str, usage: &Usage) -> Result<i64, spend::SpendError> {
        spend::cost_paise(&self.prices, self.usd_inr, model, usage)
    }

    /// A hold of nothing, for re-sending a `charge` whose first attempt
    /// already consumed the real hold. Always succeeds.
    pub fn empty_hold(self: &Arc<Self>) -> Hold {
        Hold {
            budget: Arc::clone(self),
            amount: 0,
        }
    }

    /// Count `paise` as spent without a ledger row: a Claude call was made and
    /// billed but its charge could not be booked. Keeps the headroom from being
    /// reused; the run is failed by the caller and reconciled from the error log.
    ///
    /// Local only: once the run ends, the reservation is released except what
    /// in-flight calls hold ([`RunBudget::finish`]), so the caps stop counting
    /// this amount until it is reconciled (final review M-3; see the `spend`
    /// module's overshoot notes).
    pub fn count_unbooked(&self, paise: i64) {
        self.state().spent += paise.max(0);
    }

    /// Spent, held, the finish reserve and the in-flight shares, read under
    /// **one** lock, so a check over them sees one consistent moment (Stage 3
    /// review: concurrent calls change them between separate reads).
    pub fn committed(&self) -> Committed {
        let st = self.state();
        Committed {
            spent: st.spent,
            held: st.held,
            finish_reserve: st.finish_reserve,
            in_flight_reserve: st.in_flight_reserve,
        }
    }

    /// Paise currently held by Claude calls in flight (taken, not yet charged
    /// or dropped).
    pub fn held(&self) -> i64 {
        self.state().held
    }

    /// The worker ends the run: mark the budget finished and return what is
    /// still held by calls in flight. The worker releases the reservation but
    /// keeps this much of it; the last of those calls to settle releases the
    /// rest ([`RunBudget::release_if_finished`]). Ruling R-8d.
    pub fn finish(&self) -> i64 {
        let mut st = self.state();
        st.finished = true;
        st.held
    }

    /// After a Claude call has settled (charged, or its hold dropped): if the
    /// run has already ended and no call is in flight any more, release what
    /// is left of the reservation. Idempotent; a failure is logged (ids only)
    /// and left to the recovery sweep.
    pub async fn release_if_finished(&self, pool: &PgPool) {
        let idle = {
            let st = self.state();
            st.finished && st.held == 0
        };
        if idle
            && spend::release(pool, self.user_id, self.run_id)
                .await
                .is_err()
        {
            tracing::error!(run_id = %self.run_id, "late reservation release failed");
        }
    }

    /// Time since the run started.
    pub fn elapsed(&self, now: Instant) -> Duration {
        self.age(now)
    }

    /// Set the finish reserve (see [`RunBudget::try_hold_gathering`]). The
    /// pipeline sets it once the question is framed; Extract raises it as
    /// findings accumulate.
    pub fn set_finish_reserve(&self, paise: i64) {
        self.state().finish_reserve = paise.max(0);
    }

    pub fn finish_reserve(&self) -> i64 {
        self.state().finish_reserve
    }

    /// Keep `paise` more of the finish reserve free while one paper is being
    /// extracted, for what its findings may add (Ruling R-FB4). Extract takes
    /// it right after the paper's hold, so a paper never reserves for itself
    /// (with one paper at a time nothing changes): it counts for every
    /// launch while the paper is in flight.
    pub fn reserve_in_flight(self: &Arc<Self>, paise: i64) -> InFlightReserve {
        let amount = paise.max(0);
        self.state().in_flight_reserve += amount;
        InFlightReserve {
            budget: Arc::clone(self),
            amount,
        }
    }

    /// The finish-reserve share kept for papers in flight.
    pub fn in_flight_reserve(&self) -> i64 {
        self.state().in_flight_reserve
    }

    /// Why the run should wrap up, if it should. "cost limit": 70% of the
    /// limit spent, or no headroom left above the finish reserve
    /// (`spent + held + finish_reserve >= limit`; each gathering call also
    /// needs its own estimate on top, [`RunBudget::try_hold_gathering`]).
    /// "time limit": the wrap-up time elapsed (60% of `time_limit_s`, 3
    /// minutes by default). Cost is reported first.
    pub fn wrap_up_reason(&self, now: Instant) -> Option<&'static str> {
        let (spent, held, reserve) = {
            let st = self.state();
            (st.spent, st.held, st.finish_reserve + st.in_flight_reserve)
        };
        let limit = i128::from(self.limit_paise);
        if i128::from(spent) * 100 >= limit * WRAP_UP_PERCENT
            || (reserve > 0 && i128::from(spent) + i128::from(held) + i128::from(reserve) >= limit)
        {
            Some("cost limit")
        } else if self.age(now) >= self.wrap_up_after() {
            Some("time limit")
        } else {
            None
        }
    }

    /// Paise charged so far in this run.
    pub fn spent(&self) -> i64 {
        self.state().spent
    }

    pub fn limit_paise(&self) -> i64 {
        self.limit_paise
    }

    /// Time left before the wrap-up time (60% of `time_limit_s`); zero once
    /// it has passed.
    pub fn until_wrap_up(&self, now: Instant) -> Duration {
        self.wrap_up_after().saturating_sub(self.age(now))
    }

    /// [`RunBudget::should_wrap_up`] leaving out what calls in flight hold
    /// and the finish-reserve shares of papers in flight: 70% spent, the
    /// wrap-up time, or no headroom above the finish reserve for what is
    /// already spent. A step with calls in flight stops launching only on
    /// these; the transient lack of headroom that in-flight amounts cause
    /// is left to the hold check, which refuses (and the step waits for a
    /// call to settle). With nothing in flight it equals `should_wrap_up`.
    pub fn should_wrap_up_settled(&self, now: Instant) -> bool {
        let (spent, reserve) = {
            let st = self.state();
            (st.spent, st.finish_reserve)
        };
        let limit = i128::from(self.limit_paise);
        i128::from(spent) * 100 >= limit * WRAP_UP_PERCENT
            || (reserve > 0 && i128::from(spent) + i128::from(reserve) >= limit)
            || self.age(now) >= self.wrap_up_after()
    }

    /// True when [`RunBudget::wrap_up_reason`] gives a reason.
    pub fn should_wrap_up(&self, now: Instant) -> bool {
        self.wrap_up_reason(now).is_some()
    }

    /// True once the hard stop has passed (`time_limit_s` + 60 s; 6 minutes
    /// by default).
    pub fn past_hard_stop(&self, now: Instant) -> bool {
        self.age(now) >= self.hard_stop_after
    }

    /// Read-only check against `spent` and current holds. It is **not** a
    /// reservation: concurrent callers can all pass it. Callers must use
    /// [`RunBudget::try_hold`] before every Claude call.
    pub fn can_afford(&self, estimate_paise: i64) -> bool {
        let st = self.state();
        estimate_paise >= 0
            && i128::from(st.spent) + i128::from(st.held) + i128::from(estimate_paise)
                <= i128::from(self.limit_paise)
    }

    /// Tests only: pretend `paise` has been spent.
    #[cfg(feature = "test-util")]
    pub fn record_spent_for_test(&self, paise: i64) {
        self.state().spent = paise;
    }
}
