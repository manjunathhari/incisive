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
/// ... or once this much time has passed.
const WRAP_UP_AFTER: Duration = Duration::from_secs(180);
/// Past this the run is failed.
pub const HARD_STOP_AFTER: Duration = Duration::from_secs(360);

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
        }
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
        if estimate_paise < 0 {
            return None;
        }
        let mut st = self.state();
        let total = i128::from(st.spent) + i128::from(st.held) + i128::from(estimate_paise);
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
    pub fn count_unbooked(&self, paise: i64) {
        self.state().spent += paise.max(0);
    }

    /// Paise charged so far in this run.
    pub fn spent(&self) -> i64 {
        self.state().spent
    }

    pub fn limit_paise(&self) -> i64 {
        self.limit_paise
    }

    /// True once spend reaches 70% of the limit or 3 minutes have elapsed.
    /// Counts real spend only, not in-flight holds.
    pub fn should_wrap_up(&self, now: Instant) -> bool {
        i128::from(self.spent()) * 100 >= i128::from(self.limit_paise) * WRAP_UP_PERCENT
            || now.saturating_duration_since(self.started) >= WRAP_UP_AFTER
    }

    /// True once 6 minutes have elapsed.
    pub fn past_hard_stop(&self, now: Instant) -> bool {
        now.saturating_duration_since(self.started) >= HARD_STOP_AFTER
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
