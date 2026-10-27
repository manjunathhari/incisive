//! Per-run money and time budget. Server only.
//!
//! The run's full cost limit is reserved up front (`spend::reserve`). Every
//! Claude call then charges its real cost through [`RunBudget::charge`], which
//! books it against that reservation; the end of the run releases the rest.
//! All amounts are integer paise.

use std::fmt;
use std::sync::atomic::{AtomicI64, Ordering};
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
const HARD_STOP_AFTER: Duration = Duration::from_secs(360);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BudgetError {
    /// The model has no configured price (or the rate is unusable): never free.
    UnknownModel,
    /// The database refused the charge, the run cost update or the audit entry.
    Db,
}

impl fmt::Display for BudgetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BudgetError::UnknownModel => write!(f, "model has no usable price"),
            BudgetError::Db => write!(f, "could not record the charge"),
        }
    }
}

impl std::error::Error for BudgetError {}

#[derive(Debug)]
pub struct RunBudget {
    run_id: Uuid,
    user_id: i64,
    limit_paise: i64,
    started: Instant,
    prices: Prices,
    usd_inr: f64,
    spent: AtomicI64,
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
            spent: AtomicI64::new(0),
        }
    }

    /// Charge one Claude call's real cost to the run. Returns the paise charged.
    ///
    /// Order: book the charge against the reservation, add it to the run's
    /// cost, then audit it (ids, model, tokens and paise only).
    pub async fn charge(
        &self,
        pool: &PgPool,
        model: &str,
        usage: &Usage,
    ) -> Result<i64, BudgetError> {
        let paise = spend::cost_paise(&self.prices, self.usd_inr, model, usage).map_err(|_| {
            // Unpriced model or unusable rate: fail closed, never free.
            tracing::error!(run_id = %self.run_id, "run call has no usable price");
            BudgetError::UnknownModel
        })?;
        let call_id = Uuid::new_v4();
        let db = |what: &'static str| {
            let run_id = self.run_id;
            move |_| {
                // Ids only: the error could carry stored values.
                tracing::error!(%run_id, step = what, "run charge failed");
                BudgetError::Db
            }
        };
        spend::charge_call(
            pool,
            self.user_id,
            self.run_id,
            call_id,
            model,
            usage,
            paise,
        )
        .await
        .map_err(db("charge_call"))?;
        self.spent.fetch_add(paise, Ordering::SeqCst);
        store::add_cost(pool, self.run_id, paise)
            .await
            .map_err(db("add_cost"))?;
        audit::append(
            pool,
            Some(self.run_id),
            Some(self.user_id),
            Event::Charged,
            audit::charged_detail(Some(model), Some(usage), paise),
        )
        .await
        .map_err(db("audit"))?;
        Ok(paise)
    }

    /// Paise charged so far in this run.
    pub fn spent(&self) -> i64 {
        self.spent.load(Ordering::SeqCst)
    }

    pub fn limit_paise(&self) -> i64 {
        self.limit_paise
    }

    /// True once spend reaches 70% of the limit or 3 minutes have elapsed.
    pub fn should_wrap_up(&self, now: Instant) -> bool {
        i128::from(self.spent()) * 100 >= i128::from(self.limit_paise) * WRAP_UP_PERCENT
            || now.saturating_duration_since(self.started) >= WRAP_UP_AFTER
    }

    /// True once 6 minutes have elapsed.
    pub fn past_hard_stop(&self, now: Instant) -> bool {
        now.saturating_duration_since(self.started) >= HARD_STOP_AFTER
    }

    /// Whether a call expected to cost `estimate_paise` still fits the limit.
    pub fn can_afford(&self, estimate_paise: i64) -> bool {
        estimate_paise >= 0
            && i128::from(self.spent()) + i128::from(estimate_paise) <= i128::from(self.limit_paise)
    }

    /// Tests only: pretend `paise` has been spent.
    #[cfg(feature = "test-util")]
    pub fn record_spent_for_test(&self, paise: i64) {
        self.spent.store(paise, Ordering::SeqCst);
    }
}
