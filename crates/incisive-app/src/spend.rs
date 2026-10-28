//! Spending caps, in integer paise.
//!
//! Every model call first *reserves* an estimate (`reserve`), then *settles* to the
//! actual cost (`settle`). Reserve is atomic per user and globally, so two
//! simultaneous requests can never both pass when only one fits.
//!
//! "Day" and "month" are Asia/Kolkata calendar periods, computed in SQL.
//! Money is never a float in the database; floats appear only in price config
//! and the one conversion in [`cost_paise`], which rounds up.
//!
//! Overshoot: a reservation is an estimate. The actual charge recorded by
//! [`settle`] can exceed it and land past a cap. That overrun is bounded by one
//! request's difference between estimate and actual, and the next `reserve`
//! sees it (charges count toward every total) and blocks accordingly.
//!
//! Run overshoots (Phase 2), each bounded and accepted:
//! * **Input estimates** (`agent::llm::estimate_usage`). A run's Claude call
//!   can cost more than its hold when its input is token-dense (output is
//!   held in full). One call alone: by its input excess (about 194 paise
//!   for a Haiku screening batch, 520 for a Sonnet full-text extraction, at
//!   a 3× underestimate). Calls in flight together (Screen and Extract run
//!   several, Fix B) launch only with their holds counted twice (Ruling
//!   R-FB3), so the group exceeds the room it was given only by what it
//!   costs beyond twice its holds (about 124 paise for four Haiku batches at
//!   3×). The run's finish reserve absorbs either first.
//! * **Deleting an active run** (final review M-4). `runs::api::delete_one`
//!   releases the whole reservation at once (Ruling R-1b), even while the
//!   worker still has Claude calls in flight. Their late charges then book
//!   with no reservation behind them. If the worker is on another instance it
//!   notices the deletion only at its next heartbeat (up to 20 s) and may
//!   start further calls meanwhile, while the user can already start a new
//!   run in the freed slot. The overrun is bounded by the deleted run's limit.
//! * **Unbooked spend** (final review M-3). A Claude call that was billed but
//!   whose charge could not be booked (`StepError::Unrecorded`) is counted in
//!   the run's local budget only (`RunBudget::count_unbooked`). The worker
//!   keeps just what in-flight calls hold when it releases the reservation,
//!   so that cost then appears in neither the ledger nor a reservation, and
//!   the caps do not count it until it is reconciled by hand from the error
//!   log (`... estimate counted as unbooked, reconcile manually`). It needs a
//!   ledger write to fail twice for one call, and is bounded by that call's
//!   estimate.
//!
//! Lock ordering (deadlock-free): inside one transaction always take the GLOBAL
//! advisory lock first, then the per-user lock. Both are transaction-scoped
//! `pg_advisory_xact_lock(namespace, key)` with two int4 keys, so user ids can
//! never collide with the global key or with another subsystem's locks (the
//! audit log uses its own namespace).

use std::collections::HashMap;
use std::fmt;

use chrono::{DateTime, Utc};
use claude_client::Usage;
use serde::Deserialize;
use sqlx::{PgPool, Row};
use uuid::Uuid;

use crate::identity::{Role, User};

/// Advisory-lock namespace for the single global spend lock (key 0).
pub const LOCK_NS_SPEND_GLOBAL: i32 = 0x5350_0001;
/// Advisory-lock namespace for per-user spend locks (key = low 31 bits of user id).
pub const LOCK_NS_SPEND_USER: i32 = 0x5350_0002;

/// Alert when a total after reservation reaches this percentage of its cap.
const ALERT_PERCENT: i128 = 80;

// ---------- prices and cost ----------

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ModelPrice {
    pub input_usd_per_mtok: f64,
    pub output_usd_per_mtok: f64,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Prices(HashMap<String, ModelPrice>);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PricesError(String);

impl fmt::Display for PricesError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid MODEL_PRICES: {}", self.0)
    }
}

impl std::error::Error for PricesError {}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawPrice {
    input: f64,
    output: f64,
}

impl Prices {
    pub fn new(map: HashMap<String, ModelPrice>) -> Prices {
        Prices(map)
    }

    /// Parse the `MODEL_PRICES` JSON: `{"<model id>": {"input": <usd>, "output": <usd>}}`,
    /// USD per million tokens. Errors name the variable and never echo the input.
    pub fn from_json(json: &str) -> Result<Prices, PricesError> {
        let raw: HashMap<String, RawPrice> = serde_json::from_str(json).map_err(|e| {
            PricesError(format!(
                "expected a JSON object of model -> {{\"input\": usd, \"output\": usd}} (line {}, column {})",
                e.line(),
                e.column()
            ))
        })?;
        if raw.is_empty() {
            return Err(PricesError("no models listed".to_string()));
        }
        let mut map = HashMap::new();
        for (model, p) in raw {
            let ok = |v: f64| v.is_finite() && v >= 0.0;
            if !ok(p.input) || !ok(p.output) {
                return Err(PricesError(format!(
                    "prices for `{model}` must be finite and not negative"
                )));
            }
            map.insert(
                model,
                ModelPrice {
                    input_usd_per_mtok: p.input,
                    output_usd_per_mtok: p.output,
                },
            );
        }
        Ok(Prices(map))
    }

    pub fn get(&self, model: &str) -> Option<&ModelPrice> {
        self.0.get(model)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpendError {
    /// The model has no configured price. Fails closed: never costed as zero.
    UnknownModel(String),
    /// The exchange rate or computed amount is unusable.
    InvalidRate,
}

impl fmt::Display for SpendError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SpendError::UnknownModel(m) => write!(f, "no price configured for model `{m}`"),
            SpendError::InvalidRate => write!(f, "USD_INR or the computed cost is not usable"),
        }
    }
}

impl std::error::Error for SpendError {}

/// Cost of one call in paise, rounded **up** to the next paisa.
///
/// The float product is first rounded to 6 decimal places of a paisa so binary
/// noise (e.g. 100.00000000000001) cannot add a phantom paisa to exact amounts.
pub fn cost_paise(
    prices: &Prices,
    usd_inr: f64,
    model: &str,
    usage: &Usage,
) -> Result<i64, SpendError> {
    let price = prices
        .get(model)
        .ok_or_else(|| SpendError::UnknownModel(model.to_string()))?;
    if !usd_inr.is_finite() || usd_inr <= 0.0 {
        return Err(SpendError::InvalidRate);
    }
    let usd = (f64::from(usage.input_tokens) * price.input_usd_per_mtok
        + f64::from(usage.output_tokens) * price.output_usd_per_mtok)
        / 1_000_000.0;
    let paise = usd * usd_inr * 100.0;
    let paise = (paise * 1e6).round() / 1e6;
    if !paise.is_finite() || paise < 0.0 || paise >= i64::MAX as f64 {
        return Err(SpendError::InvalidRate);
    }
    Ok(paise.ceil() as i64)
}

// ---------- caps ----------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CapsConfig {
    pub global_monthly_paise: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CapCheck {
    Ok { alert: Option<String> },
    Blocked { message: String },
}

/// Default (daily, monthly) caps in paise for a new user of this role.
pub fn default_caps(role: Role) -> (i64, Option<i64>) {
    match role {
        Role::Owner => (150_000, None),
        Role::Surgeon => (30_000, Some(300_000)),
        Role::Fellow => (15_000, Some(150_000)),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Totals {
    pub today_paise: i64,
    pub month_paise: i64,
}

fn rupees(paise: i64) -> String {
    let (r, p) = (paise / 100, (paise % 100).abs());
    if p == 0 {
        format!("\u{20b9}{r}")
    } else {
        format!("\u{20b9}{r}.{p:02}")
    }
}

fn at_least_alert(total: i64, cap: i64) -> bool {
    cap > 0 && i128::from(total) * 100 >= i128::from(cap) * ALERT_PERCENT
}

/// Sums (user day, user month, global month) of reservations + charges for the
/// IST periods containing `now`. Periods are half-open `[start, next start)`.
const SUMS_SQL: &str = "\
WITH b AS (SELECT \
    (date_trunc('day', $2::timestamptz AT TIME ZONE 'Asia/Kolkata')) AT TIME ZONE 'Asia/Kolkata' AS day_start, \
    (date_trunc('month', $2::timestamptz AT TIME ZONE 'Asia/Kolkata')) AT TIME ZONE 'Asia/Kolkata' AS month_start, \
    (date_trunc('month', $2::timestamptz AT TIME ZONE 'Asia/Kolkata') + interval '1 month') AT TIME ZONE 'Asia/Kolkata' AS month_end, \
    (date_trunc('day', $2::timestamptz AT TIME ZONE 'Asia/Kolkata') + interval '1 day') AT TIME ZONE 'Asia/Kolkata' AS day_end) \
SELECT \
  COALESCE(SUM(s.paise) FILTER (WHERE s.user_id = $1 AND s.at >= b.day_start AND s.at < b.day_end), 0)::bigint AS user_day, \
  COALESCE(SUM(s.paise) FILTER (WHERE s.user_id = $1), 0)::bigint AS user_month, \
  COALESCE(SUM(s.paise), 0)::bigint AS global_month \
FROM b LEFT JOIN incisive.spend s ON s.at >= b.month_start AND s.at < b.month_end \
GROUP BY b.day_start";

async fn sums<'e, E>(exec: E, user_id: i64, now: DateTime<Utc>) -> sqlx::Result<(i64, i64, i64)>
where
    E: sqlx::Executor<'e, Database = sqlx::Postgres>,
{
    let row = sqlx::query(SUMS_SQL)
        .bind(user_id)
        .bind(now)
        .fetch_one(exec)
        .await?;
    Ok((
        row.try_get("user_day")?,
        row.try_get("user_month")?,
        row.try_get("global_month")?,
    ))
}

/// The user's reservations + charges for the current IST day and month.
pub async fn totals(pool: &PgPool, user_id: i64) -> sqlx::Result<Totals> {
    totals_at(pool, user_id, Utc::now()).await
}

/// [`totals`] as of an explicit instant (for tests and boundary checks).
pub async fn totals_at(pool: &PgPool, user_id: i64, now: DateTime<Utc>) -> sqlx::Result<Totals> {
    let (day, month, _) = sums(pool, user_id, now).await?;
    Ok(Totals {
        today_paise: day,
        month_paise: month,
    })
}

/// Atomically check the caps and, if the estimate fits, record a reservation.
pub async fn reserve(
    pool: &PgPool,
    user: &User,
    caps: &CapsConfig,
    estimate_paise: i64,
    request_id: Uuid,
    purpose: &str,
) -> sqlx::Result<CapCheck> {
    reserve_at(
        pool,
        user,
        caps,
        estimate_paise,
        request_id,
        purpose,
        Utc::now(),
    )
    .await
}

/// [`reserve`] as of an explicit instant (for tests and boundary checks).
pub async fn reserve_at(
    pool: &PgPool,
    user: &User,
    caps: &CapsConfig,
    estimate_paise: i64,
    request_id: Uuid,
    purpose: &str,
    now: DateTime<Utc>,
) -> sqlx::Result<CapCheck> {
    if estimate_paise < 0 {
        tracing::warn!(%request_id, purpose, "negative spend estimate; treating as zero");
    }
    debug_assert!(estimate_paise >= 0, "negative spend estimate");
    let estimate = estimate_paise.max(0);
    let mut tx = pool.begin().await?;

    // Lock order: global first, then user. Released at commit/rollback.
    sqlx::query("SELECT pg_advisory_xact_lock($1, $2)")
        .bind(LOCK_NS_SPEND_GLOBAL)
        .bind(0_i32)
        .execute(&mut *tx)
        .await?;
    sqlx::query("SELECT pg_advisory_xact_lock($1, $2)")
        .bind(LOCK_NS_SPEND_USER)
        .bind((user.id & 0x7fff_ffff) as i32)
        .execute(&mut *tx)
        .await?;

    let (day, month, global) = sums(&mut *tx, user.id, now).await?;
    let (day, month, global) = (
        i128::from(day) + i128::from(estimate),
        i128::from(month) + i128::from(estimate),
        i128::from(global) + i128::from(estimate),
    );

    let blocked = if day > i128::from(user.daily_cap_paise) {
        Some(format!(
            "You've reached today's Incisive limit ({}). It resets at midnight IST.",
            rupees(user.daily_cap_paise)
        ))
    } else if user
        .monthly_cap_paise
        .is_some_and(|c| month > i128::from(c))
    {
        Some(format!(
            "You've reached this month's Incisive limit ({}). It resets at midnight IST on the 1st.",
            rupees(user.monthly_cap_paise.unwrap_or_default())
        ))
    } else if global > i128::from(caps.global_monthly_paise) {
        Some("Incisive has reached its overall monthly budget. Please tell the owner.".to_string())
    } else {
        None
    };
    if let Some(message) = blocked {
        tx.rollback().await?;
        return Ok(CapCheck::Blocked { message });
    }

    sqlx::query(
        "INSERT INTO incisive.spend (user_id, at, kind, purpose, paise, request_id) \
         VALUES ($1, $2, 'reservation', $3, $4, $5)",
    )
    .bind(user.id)
    .bind(now)
    .bind(purpose)
    .bind(estimate)
    .bind(request_id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;

    // After the reservation, which totals have reached 80% of their cap?
    let clamp = |v: i128| i64::try_from(v).unwrap_or(i64::MAX);
    let alert = if at_least_alert(clamp(day), user.daily_cap_paise) {
        Some(format!(
            "You've used 80% of today's Incisive limit ({}).",
            rupees(user.daily_cap_paise)
        ))
    } else if let Some(c) = user
        .monthly_cap_paise
        .filter(|c| at_least_alert(clamp(month), *c))
    {
        Some(format!(
            "You've used 80% of this month's Incisive limit ({}).",
            rupees(c)
        ))
    } else if at_least_alert(clamp(global), caps.global_monthly_paise) {
        Some("Incisive has used 80% of its overall monthly budget.".to_string())
    } else {
        None
    };
    Ok(CapCheck::Ok { alert })
}

/// Replace the request's reservation with its actual charge, atomically.
///
/// Idempotent: at most one charge exists per `request_id` (partial unique index),
/// so a repeated settle neither double-charges nor errors.
///
/// The charge may exceed the reservation and land past the cap; the overrun is
/// bounded by one request's estimate error and the next `reserve` accounts for it.
/// Pass `model: None, usage: None, paise: 0` when no model call happened.
pub async fn settle(
    pool: &PgPool,
    user_id: i64,
    request_id: Uuid,
    model: Option<&str>,
    usage: Option<&Usage>,
    paise: i64,
) -> sqlx::Result<()> {
    let mut tx = pool.begin().await?;
    let purpose: Option<String> = sqlx::query_scalar(
        "DELETE FROM incisive.spend \
         WHERE request_id = $1 AND user_id = $2 AND kind = 'reservation' RETURNING purpose",
    )
    .bind(request_id)
    .bind(user_id)
    .fetch_optional(&mut *tx)
    .await?;
    if paise < 0 {
        tracing::warn!(%request_id, "negative settle amount; recording zero");
    }
    debug_assert!(paise >= 0, "negative settle amount");
    if purpose.is_none() {
        // Ids and purpose only: never question text.
        tracing::warn!(%request_id, purpose = "unreserved", "settle found no reservation");
    }
    let tokens = |n: u32| i32::try_from(n).unwrap_or(i32::MAX);
    sqlx::query(
        "INSERT INTO incisive.spend \
         (user_id, kind, purpose, model, input_tokens, output_tokens, paise, request_id) \
         VALUES ($1, 'charge', $2, $3, $4, $5, $6, $7) \
         ON CONFLICT (request_id) WHERE kind = 'charge' DO NOTHING",
    )
    .bind(user_id)
    .bind(purpose.unwrap_or_else(|| "unreserved".to_string()))
    .bind(model)
    .bind(usage.map(|u| tokens(u.input_tokens)))
    .bind(usage.map(|u| tokens(u.output_tokens)))
    .bind(paise.max(0))
    .bind(request_id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await
}

/// Charge one Claude call of a run, and shrink the run's reservation by the
/// same amount (floored at zero), in one transaction.
///
/// Takes the same locks in the same order as [`reserve`] (global, then user).
/// Idempotent per `call_id`: a replay inserts no second charge and does not
/// shrink the reservation again. Safe if the reservation is already gone
/// (released, or never made): the charge is still recorded. The real cost is
/// always recorded in full, even beyond the reservation.
///
/// Returns whether a new charge row was inserted (false on a replay). A replay
/// whose user or amount differs from the stored charge is logged (ids only) and
/// changes nothing.
///
/// Charges are dated at settle time (now), not at reservation time, so a run
/// that crosses IST midnight counts in the new day. This is accepted.
pub async fn charge_call(
    pool: &PgPool,
    user_id: i64,
    run_id: Uuid,
    call_id: Uuid,
    model: &str,
    usage: &Usage,
    paise: i64,
) -> sqlx::Result<bool> {
    if paise < 0 {
        tracing::warn!(%run_id, %call_id, "negative call charge; recording zero");
    }
    debug_assert!(paise >= 0, "negative call charge");
    let paise = paise.max(0);
    let tokens = |n: u32| i32::try_from(n).unwrap_or(i32::MAX);
    let mut tx = pool.begin().await?;
    lock_spend(&mut tx, user_id).await?;

    let inserted = sqlx::query(
        "INSERT INTO incisive.spend \
         (user_id, kind, purpose, model, input_tokens, output_tokens, paise, request_id) \
         VALUES ($1, 'charge', $2, $3, $4, $5, $6, $7) \
         ON CONFLICT (request_id) WHERE kind = 'charge' DO NOTHING",
    )
    .bind(user_id)
    .bind(format!("run:{run_id}"))
    .bind(model)
    .bind(tokens(usage.input_tokens))
    .bind(tokens(usage.output_tokens))
    .bind(paise)
    .bind(call_id)
    .execute(&mut *tx)
    .await?
    .rows_affected()
        > 0;
    if inserted {
        sqlx::query(
            "UPDATE incisive.spend SET paise = paise - LEAST(paise, $3) \
             WHERE request_id = $1 AND user_id = $2 AND kind = 'reservation'",
        )
        .bind(run_id)
        .bind(user_id)
        .bind(paise)
        .execute(&mut *tx)
        .await?;
    } else {
        let existing: Option<(i64, i64)> = sqlx::query_as(
            "SELECT user_id, paise FROM incisive.spend WHERE request_id = $1 AND kind = 'charge'",
        )
        .bind(call_id)
        .fetch_optional(&mut *tx)
        .await?;
        if existing.is_some_and(|(u, p)| u != user_id || p != paise) {
            tracing::warn!(%run_id, %call_id, "charge replay differs from the stored charge");
        }
    }
    tx.commit().await?;
    Ok(inserted)
}

/// Delete the run's remaining reservation and return the paise released
/// (0 if there is none, so a second call is harmless).
pub async fn release(pool: &PgPool, user_id: i64, run_id: Uuid) -> sqlx::Result<i64> {
    let mut tx = pool.begin().await?;
    lock_spend(&mut tx, user_id).await?;
    let released: Option<i64> = sqlx::query_scalar(
        "DELETE FROM incisive.spend \
         WHERE request_id = $1 AND user_id = $2 AND kind = 'reservation' RETURNING paise",
    )
    .bind(run_id)
    .bind(user_id)
    .fetch_optional(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(released.unwrap_or(0))
}

/// Release the run's reservation except `keep` paise, which stay reserved for
/// Claude calls still in flight (Ruling R-8d); returns the paise released.
/// With `keep <= 0` this is [`release`]. Their late charges consume what is
/// kept, and the last one releases the rest (`RunBudget::release_if_finished`).
pub async fn release_keeping(
    pool: &PgPool,
    user_id: i64,
    run_id: Uuid,
    keep: i64,
) -> sqlx::Result<i64> {
    if keep <= 0 {
        return release(pool, user_id, run_id).await;
    }
    let mut tx = pool.begin().await?;
    lock_spend(&mut tx, user_id).await?;
    let before: Option<i64> = sqlx::query_scalar(
        "SELECT paise FROM incisive.spend \
         WHERE request_id = $1 AND user_id = $2 AND kind = 'reservation'",
    )
    .bind(run_id)
    .bind(user_id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some(before) = before else {
        tx.commit().await?;
        return Ok(0);
    };
    let after = before.min(keep);
    sqlx::query(
        "UPDATE incisive.spend SET paise = $3 \
         WHERE request_id = $1 AND user_id = $2 AND kind = 'reservation'",
    )
    .bind(run_id)
    .bind(user_id)
    .bind(after)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(before - after)
}

/// The user holding the reservation made under `request_id`, if one remains.
/// Lets a worker release a run's reservation after the run row is gone.
pub async fn reservation_owner(pool: &PgPool, request_id: Uuid) -> sqlx::Result<Option<i64>> {
    sqlx::query_scalar(
        "SELECT user_id FROM incisive.spend WHERE request_id = $1 AND kind = 'reservation' LIMIT 1",
    )
    .bind(request_id)
    .fetch_optional(pool)
    .await
}

/// The global advisory lock, then the user's (the order [`reserve`] uses).
async fn lock_spend(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    user_id: i64,
) -> sqlx::Result<()> {
    sqlx::query("SELECT pg_advisory_xact_lock($1, $2)")
        .bind(LOCK_NS_SPEND_GLOBAL)
        .bind(0_i32)
        .execute(&mut **tx)
        .await?;
    sqlx::query("SELECT pg_advisory_xact_lock($1, $2)")
        .bind(LOCK_NS_SPEND_USER)
        .bind((user_id & 0x7fff_ffff) as i32)
        .execute(&mut **tx)
        .await?;
    Ok(())
}
