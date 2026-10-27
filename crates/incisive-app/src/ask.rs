//! The ask pipeline: one function, [`run_ask`], behind both `POST /api/ask` and
//! the page's server function.
//!
//! Order (each step audited under one shared request id):
//! identity (by the caller) -> identifier filter -> spend reservation for the gate
//! estimate -> medical gate -> settle -> accepted.
//!
//! The identifier filter runs before anything leaves the app. The question text
//! is never stored or logged: the audit log gets finding kinds, offsets, lengths,
//! a SHA-256, verdicts and money; `tracing` calls here carry ids and fixed kinds
//! only. If any audit append fails, the request fails closed ([`AskFailure`]):
//! nothing is answered without a record.
//!
//! The types below are shared with the browser build; the pipeline itself is
//! server-only.

use serde::{Deserialize, Serialize};

use crate::deid::FindingKind;

pub const ACCEPTED_MESSAGE: &str = "Accepted. The evidence agent arrives in Phase 2.";
pub const BLOCKED_MESSAGE: &str = "Incisive found details that could identify a patient. Please remove them and resend your question.";
pub const RECORD_FAILED_MESSAGE: &str = "Incisive couldn't record this request. Please try again.";
pub const OWNER_ONLY_MESSAGE: &str = "Only the Incisive owner can see the activity log.";
pub const EMPTY_MESSAGE: &str = "Please type your clinical question.";
pub const TOO_LONG_MESSAGE: &str =
    "That question is too long for Incisive (4,000 characters max). Please shorten it.";
/// Longest question accepted, in characters (Unicode scalar values, not bytes).
pub const MAX_QUESTION_CHARS: usize = 4_000;

/// One blocked span: kind, user-facing label and offsets. Never the text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FindingView {
    pub kind: FindingKind,
    pub label: String,
    /// Start of the span as a UTF-8 **byte** offset into the question (not a
    /// character index; JavaScript string indices differ for non-ASCII text).
    pub start: usize,
    /// End of the span (exclusive), as a UTF-8 byte offset.
    pub end: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum AskOutcome {
    BlockedIdentifiers {
        findings: Vec<FindingView>,
        message: String,
    },
    NotMedical {
        message: String,
    },
    Unclear {
        message: String,
    },
    GateUnavailable {
        message: String,
    },
    OverCap {
        message: String,
    },
    Accepted {
        message: String,
        alert: Option<String>,
    },
}

/// The request could not be completed safely. Never an answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AskFailure {
    /// An audit or spend write failed.
    Record,
}

impl AskFailure {
    pub fn message(&self) -> &'static str {
        match self {
            AskFailure::Record => RECORD_FAILED_MESSAGE,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeView {
    pub display_name: String,
    pub role: String,
    pub today_paise: i64,
    pub daily_cap_paise: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActivityRow {
    pub seq: i64,
    pub time_ist: String,
    pub user_email: Option<String>,
    pub event: String,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChainView {
    /// `valid` or `broken`.
    pub status: String,
    pub entries: Option<i64>,
    pub at_seq: Option<i64>,
    /// "Chain verified ✓ (N entries)" or "Chain broken at #N".
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActivityView {
    pub entries: Vec<ActivityRow>,
    pub chain: ChainView,
}

#[cfg(feature = "ssr")]
pub use server::*;

#[cfg(feature = "ssr")]
mod server {
    use axum::{
        Json,
        extract::{Query, State, rejection::JsonRejection},
        http::StatusCode,
        response::{IntoResponse, Response},
    };
    use chrono::FixedOffset;
    use claude_client::Usage;
    use serde::Deserialize;
    use serde_json::json;
    use uuid::Uuid;

    use super::*;
    use crate::audit::{self, ChainStatus, Event};
    use crate::deid;
    use crate::gate::{self, GateError, Verdict};
    use crate::identity::{CurrentUser, IdentityRejection, Role, User};
    use crate::server::AppState;
    use crate::spend::{self, CapCheck, SpendError};

    /// Estimate for one gate call on `question`: the fixed prompt allowance plus
    /// ceil(question UTF-8 bytes / 2) input tokens (conservative for Indic
    /// scripts, which are 3 bytes per character), and the 150-token output cap.
    pub fn gate_estimate(question: &str) -> Usage {
        let question_tokens = u32::try_from(question.len().div_ceil(2)).unwrap_or(u32::MAX);
        Usage {
            input_tokens: gate::GATE_PROMPT_TOKENS.saturating_add(question_tokens),
            output_tokens: gate::GATE_MAX_TOKENS,
        }
    }
    const ACTIVITY_DEFAULT: i64 = 50;
    const ACTIVITY_MAX: i64 = 500;
    const DETAIL_CHARS: usize = 160;
    /// Request body limit for `POST /api/ask` and the page/server-function routes.
    /// A 4,000-character question is at most 16,000 bytes of UTF-8 before encoding.
    pub const ASK_BODY_LIMIT: usize = 64 * 1024;

    async fn record(
        state: &AppState,
        request_id: Uuid,
        user_id: Option<i64>,
        event: Event,
        detail: serde_json::Value,
    ) -> Result<(), AskFailure> {
        audit::append(&state.pool, Some(request_id), user_id, event, detail)
            .await
            .map(|_| ())
            .map_err(|_| {
                // Ids and the event name only; the error could carry stored values.
                tracing::error!(%request_id, event = event.as_str(), "audit append failed");
                AskFailure::Record
            })
    }

    /// The red-box message, plus each distinct kind's extra hint as a sentence.
    fn blocked_message(findings: &[FindingView]) -> String {
        let mut message = BLOCKED_MESSAGE.to_string();
        let mut seen: Vec<&str> = Vec::new();
        for h in findings.iter().filter_map(|f| deid::hint(f.kind)) {
            if seen.contains(&h) {
                continue;
            }
            seen.push(h);
            let mut chars = h.chars();
            if let Some(first) = chars.next() {
                message.push(' ');
                message.extend(first.to_uppercase());
                message.push_str(chars.as_str());
                message.push('.');
            }
        }
        message
    }

    /// The whole pipeline after identity. Never returns `Accepted` unless every
    /// step, including its audit entry, succeeded.
    pub async fn run_ask(
        state: &AppState,
        user: &User,
        question: &str,
    ) -> Result<AskOutcome, AskFailure> {
        // 0. Input limits, before anything else: no scan, audit, reservation or
        // gate. Nothing about the content is decided and the text goes nowhere.
        if question.trim().is_empty() {
            return Ok(AskOutcome::Unclear {
                message: EMPTY_MESSAGE.to_string(),
            });
        }
        if question.chars().count() > MAX_QUESTION_CHARS {
            return Ok(AskOutcome::NotMedical {
                message: TOO_LONG_MESSAGE.to_string(),
            });
        }

        let request_id = Uuid::new_v4();
        let uid = Some(user.id);

        // 1. Identifiers: local, before any network call.
        let findings = deid::scan(question);
        if !findings.is_empty() {
            record(
                state,
                request_id,
                uid,
                Event::DeidBlocked,
                audit::deid_blocked_detail(&findings, question.len()),
            )
            .await?;
            let findings: Vec<FindingView> = findings
                .iter()
                .map(|f| FindingView {
                    kind: f.kind,
                    label: deid::describe(f.kind).to_string(),
                    start: f.start,
                    end: f.end,
                })
                .collect();
            return Ok(AskOutcome::BlockedIdentifiers {
                message: blocked_message(&findings),
                findings,
            });
        }

        // 2. Price the gate estimate. Unpriced fails closed, before reserving.
        let estimate = match spend::cost_paise(
            &state.prices,
            state.usd_inr,
            &state.gate_model,
            &gate_estimate(question),
        ) {
            Ok(p) => p,
            Err(e) => {
                let kind = match e {
                    SpendError::UnknownModel(_) => "unpriced_model",
                    SpendError::InvalidRate => "invalid_rate",
                };
                tracing::error!(%request_id, error_kind = kind, "gate estimate could not be priced");
                record(
                    state,
                    request_id,
                    uid,
                    Event::GateFailed,
                    audit::gate_failed_detail(kind),
                )
                .await?;
                return Ok(AskOutcome::GateUnavailable {
                    message: gate::GATE_ERROR_MESSAGE.to_string(),
                });
            }
        };

        // 3. Reserve it against the caps (atomic).
        let alert = match spend::reserve(
            &state.pool,
            user,
            &state.caps,
            estimate,
            request_id,
            "gate",
        )
        .await
        {
            Ok(CapCheck::Ok { alert }) => alert,
            Ok(CapCheck::Blocked { message }) => {
                record(
                    state,
                    request_id,
                    uid,
                    Event::CapBlocked,
                    audit::cap_blocked_detail(estimate),
                )
                .await?;
                return Ok(AskOutcome::OverCap { message });
            }
            Err(_) => {
                tracing::error!(%request_id, "spend reservation failed");
                return Err(AskFailure::Record);
            }
        };

        // 4. Gate.
        let decision = match gate::check(question, state.gate.as_ref()).await {
            Ok(d) => d,
            Err(e) => {
                // What the failed call may have cost (ruling R-F2): the real usage
                // of a billed but unusable answer; the estimate when the request
                // may have been processed; zero for an HTTP error status.
                let (model, usage, paise) = match &e {
                    GateError::BadResponse { model, usage } => {
                        let paise = spend::cost_paise(&state.prices, state.usd_inr, model, usage)
                            .unwrap_or_else(|_| {
                                tracing::warn!(%request_id, "gate usage could not be priced; charging the estimate");
                                estimate
                            });
                        (Some(model.as_str()), Some(usage), paise)
                    }
                    GateError::Unavailable { maybe_billed: true } => {
                        (Some(state.gate_model.as_str()), None, estimate)
                    }
                    GateError::Unavailable {
                        maybe_billed: false,
                    } => (None, None, 0),
                };
                let settled = spend::settle(&state.pool, user.id, request_id, model, usage, paise)
                    .await
                    .map_err(|_| {
                        tracing::error!(%request_id, "spend settle failed");
                        AskFailure::Record
                    });
                // Audit a charge only when one was booked.
                let charged = if settled.is_ok() && paise > 0 {
                    record(
                        state,
                        request_id,
                        uid,
                        Event::Charged,
                        audit::charged_detail(model, usage, paise),
                    )
                    .await
                } else {
                    Ok(())
                };
                let logged = record(
                    state,
                    request_id,
                    uid,
                    Event::GateFailed,
                    audit::gate_failed_detail(e.kind()),
                )
                .await;
                settled?;
                charged?;
                logged?;
                return Ok(AskOutcome::GateUnavailable {
                    message: e.user_message().to_string(),
                });
            }
        };
        // What the gate call cost: the real usage if a model was called, else zero.
        let charge = match (decision.model.as_deref(), decision.usage.as_ref()) {
            (Some(model), Some(usage)) => {
                let paise = spend::cost_paise(&state.prices, state.usd_inr, model, usage)
                    .unwrap_or_else(|_| {
                        // Fail safe: charge the (priced) estimate rather than nothing.
                        tracing::warn!(%request_id, "gate usage could not be priced; charging the estimate");
                        estimate
                    });
                Some((model, usage, paise))
            }
            _ => None,
        };
        let settle = || async {
            let (model, usage, paise) = match charge {
                Some((m, u, p)) => (Some(m), Some(u), p),
                None => (None, None, 0),
            };
            spend::settle(&state.pool, user.id, request_id, model, usage, paise)
                .await
                .map_err(|_| {
                    tracing::error!(%request_id, "spend settle failed");
                    AskFailure::Record
                })
        };

        if let Err(f) = record(
            state,
            request_id,
            uid,
            Event::GateDecided,
            audit::gate_detail(&decision),
        )
        .await
        {
            // The model call (if any) still happened: record its real cost.
            let _ = settle().await;
            return Err(f);
        }

        // 5. Settle, and audit the charge when a model was called.
        settle().await?;
        if let Some((model, usage, paise)) = charge {
            record(
                state,
                request_id,
                uid,
                Event::Charged,
                audit::charged_detail(Some(model), Some(usage), paise),
            )
            .await?;
        }

        // 6. Outcome.
        match decision.verdict {
            Verdict::Medical => {
                record(
                    state,
                    request_id,
                    uid,
                    Event::Accepted,
                    audit::accepted_detail(question),
                )
                .await?;
                Ok(AskOutcome::Accepted {
                    message: ACCEPTED_MESSAGE.to_string(),
                    alert,
                })
            }
            Verdict::NotMedical => Ok(AskOutcome::NotMedical {
                message: decision.user_message().unwrap_or_default(),
            }),
            Verdict::Unclear => Ok(AskOutcome::Unclear {
                message: decision.user_message().unwrap_or_default(),
            }),
        }
    }

    /// Audit a failed sign-in on an ask entry point: the fixed kind only.
    pub async fn audit_identity_rejection(state: &AppState, rejection: &IdentityRejection) {
        let request_id = Uuid::new_v4();
        if audit::append(
            &state.pool,
            Some(request_id),
            None,
            Event::IdentityRejected,
            audit::identity_rejected_detail(rejection.kind()),
        )
        .await
        .is_err()
        {
            tracing::error!(%request_id, "audit append failed for identity rejection");
        }
    }

    pub async fn me_view(state: &AppState, user: &User) -> Result<MeView, AskFailure> {
        let totals = spend::totals(&state.pool, user.id).await.map_err(|_| {
            tracing::error!(user_id = user.id, "spend totals failed");
            AskFailure::Record
        })?;
        Ok(MeView {
            display_name: user.display_name.clone(),
            role: user.role.as_str().to_string(),
            today_paise: totals.today_paise,
            daily_cap_paise: user.daily_cap_paise,
        })
    }

    pub fn ist() -> FixedOffset {
        FixedOffset::east_opt(5 * 3600 + 30 * 60).expect("IST offset")
    }

    fn short(detail: &serde_json::Value) -> String {
        let full = audit::canonical_detail(detail);
        if full.chars().count() <= DETAIL_CHARS {
            full
        } else {
            let mut s: String = full.chars().take(DETAIL_CHARS).collect();
            s.push('…');
            s
        }
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum ActivityError {
        NotOwner,
        Failed(AskFailure),
    }

    impl ActivityError {
        pub fn message(&self) -> &'static str {
            match self {
                ActivityError::NotOwner => OWNER_ONLY_MESSAGE,
                ActivityError::Failed(f) => f.message(),
            }
        }
    }

    /// Owner only: recent entries (newest first) and the chain check.
    pub async fn activity_view(
        state: &AppState,
        user: &User,
        limit: Option<i64>,
    ) -> Result<ActivityView, ActivityError> {
        if user.role != Role::Owner {
            return Err(ActivityError::NotOwner);
        }
        let limit = limit.unwrap_or(ACTIVITY_DEFAULT).clamp(1, ACTIVITY_MAX);
        load_activity(state, limit)
            .await
            .map_err(ActivityError::Failed)
    }

    async fn load_activity(state: &AppState, limit: i64) -> Result<ActivityView, AskFailure> {
        let fail = |_| {
            tracing::error!("activity log read failed");
            AskFailure::Record
        };
        let entries = audit::recent(&state.pool, limit).await.map_err(fail)?;
        let mut ids: Vec<i64> = entries.iter().filter_map(|e| e.user_id).collect();
        ids.sort_unstable();
        ids.dedup();
        let emails: std::collections::HashMap<i64, String> = sqlx::query_as::<_, (i64, String)>(
            "SELECT id, email FROM incisive.users WHERE id = ANY($1)",
        )
        .bind(&ids)
        .fetch_all(&state.pool)
        .await
        .map_err(fail)?
        .into_iter()
        .collect();
        let chain = match audit::verify_chain(&state.pool).await.map_err(fail)? {
            ChainStatus::Valid { entries } => ChainView {
                status: "valid".into(),
                entries: Some(entries),
                at_seq: None,
                label: format!("Chain verified \u{2713} ({entries} entries)"),
            },
            ChainStatus::Broken { at_seq } => ChainView {
                status: "broken".into(),
                entries: None,
                at_seq: Some(at_seq),
                label: format!("Chain broken at #{at_seq}"),
            },
        };
        let tz = ist();
        Ok(ActivityView {
            entries: entries
                .iter()
                .map(|e| ActivityRow {
                    seq: e.seq,
                    time_ist: e
                        .at
                        .with_timezone(&tz)
                        .format("%Y-%m-%d %H:%M:%S IST")
                        .to_string(),
                    user_email: e.user_id.and_then(|id| emails.get(&id).cloned()),
                    event: e.event.as_str().to_string(),
                    detail: short(&e.detail),
                })
                .collect(),
            chain,
        })
    }

    // ---------- JSON routes ----------

    fn failure_response(f: AskFailure) -> Response {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": "not_recorded", "message": f.message()})),
        )
            .into_response()
    }

    #[derive(Deserialize)]
    pub struct AskRequest {
        pub question: String,
    }

    /// `POST /api/ask`.
    pub async fn ask_handler(
        State(state): State<AppState>,
        user: Result<CurrentUser, IdentityRejection>,
        body: Result<Json<AskRequest>, JsonRejection>,
    ) -> Response {
        let user = match user {
            Ok(CurrentUser(u)) => u,
            Err(rejection) => {
                audit_identity_rejection(&state, &rejection).await;
                return rejection.into_response();
            }
        };
        let Ok(Json(req)) = body else {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({
                    "error": "bad_request",
                    "message": "Send your question as JSON: {\"question\": \"...\"}."
                })),
            )
                .into_response();
        };
        match run_ask(&state, &user, &req.question).await {
            Ok(outcome) => Json(outcome).into_response(),
            Err(f) => failure_response(f),
        }
    }

    /// `GET /api/me`.
    pub async fn me_handler(
        State(state): State<AppState>,
        CurrentUser(user): CurrentUser,
    ) -> Response {
        match me_view(&state, &user).await {
            Ok(me) => Json(me).into_response(),
            Err(f) => failure_response(f),
        }
    }

    #[derive(Deserialize)]
    pub struct ActivityQuery {
        pub limit: Option<i64>,
    }

    /// `GET /api/activity?limit=50` (owner only).
    pub async fn activity_handler(
        State(state): State<AppState>,
        CurrentUser(user): CurrentUser,
        Query(q): Query<ActivityQuery>,
    ) -> Response {
        match activity_view(&state, &user, q.limit).await {
            Err(ActivityError::NotOwner) => (
                StatusCode::FORBIDDEN,
                Json(json!({"error": "owner_only", "message": OWNER_ONLY_MESSAGE})),
            )
                .into_response(),
            Ok(view) => Json(view).into_response(),
            Err(ActivityError::Failed(f)) => failure_response(f),
        }
    }
}
