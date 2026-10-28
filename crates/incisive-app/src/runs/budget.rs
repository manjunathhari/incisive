//! Per-run money and time budget. Server only.
//!
//! The run's full cost limit is reserved up front (`spend::reserve`). Every
//! Claude call then charges its real cost through [`RunBudget::charge`], which
//! books it against that reservation; the end of the run releases the rest.
//! All amounts are integer paise.

use std::collections::HashMap;
use std::fmt;
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
    /// From the run's `time_limit_s` ([`RunBudget::with_time_limit`]).
    wrap_up_after: Duration,
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
            wrap_up_after: wrap_up_after(DEFAULT_TIME_LIMIT_S),
            hard_stop_after: hard_stop_after(DEFAULT_TIME_LIMIT_S),
        }
    }

    /// Time limits from the run's `time_limit_s` (`RUN_LIMITS`): wrap-up at
    /// 60% of it, hard stop at it plus 60 s. Without this, the defaults for
    /// 300 s apply (180 s and 360 s).
    pub fn with_time_limit(mut self, time_limit_s: u32) -> RunBudget {
        self.wrap_up_after = wrap_up_after(time_limit_s);
        self.hard_stop_after = hard_stop_after(time_limit_s);
        self
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
    pub fn try_hold_gathering(self: &Arc<Self>, estimate_paise: i64) -> Option<Hold> {
        self.hold_leaving(estimate_paise, true)
    }

    fn hold_leaving(self: &Arc<Self>, estimate_paise: i64, reserve: bool) -> Option<Hold> {
        if estimate_paise < 0 {
            return None;
        }
        let mut st = self.state();
        let kept = if reserve { st.finish_reserve } else { 0 };
        let total = i128::from(st.spent)
            + i128::from(st.held)
            + i128::from(estimate_paise)
            + i128::from(kept);
        if total > i128::from(self.limit_paise) {
            return None;
        }
        st.held += estimate_paise;
        drop(st);
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
    /// `hold` is consumed: its amount is replaced by the real cost.
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
        // On every early return the hold is dropped, freeing its amount.
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
        let mut hold = hold;
        let held = std::mem::take(&mut hold.amount);
        drop(hold);
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

    /// Why the run should wrap up, if it should. "cost limit": 70% of the
    /// limit spent, or no headroom left above the finish reserve
    /// (`spent + held + finish_reserve >= limit`; each gathering call also
    /// needs its own estimate on top, [`RunBudget::try_hold_gathering`]).
    /// "time limit": the wrap-up time elapsed (60% of `time_limit_s`, 3
    /// minutes by default). Cost is reported first.
    pub fn wrap_up_reason(&self, now: Instant) -> Option<&'static str> {
        let (spent, held, reserve) = {
            let st = self.state();
            (st.spent, st.held, st.finish_reserve)
        };
        let limit = i128::from(self.limit_paise);
        if i128::from(spent) * 100 >= limit * WRAP_UP_PERCENT
            || (reserve > 0 && i128::from(spent) + i128::from(held) + i128::from(reserve) >= limit)
        {
            Some("cost limit")
        } else if self.age(now) >= self.wrap_up_after {
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
