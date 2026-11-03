//! The chat's JSON routes. Server only; registered only when `CHAT=on`.
//!
//! Every route is scoped to the signed-in user, as in `runs/api.rs`: someone
//! else's run, turn or citation behaves exactly like a missing one (404), the
//! owner included, and a malformed id is the same 404.
//!
//! An error body is `{error, message, safety_line, lines}`. When a refusal
//! carries the safety line, `safety_line` is true and `lines` holds the
//! fixed warning text, so the page can show it with no answer.
//!
//! Status per error: `NotFound` 404; `Empty`, `TooLong`, `BadFeedback`,
//! `BadMode` and a malformed body 400; `Identifiers` 422; `InFlight`, `NotOffered`,
//! `NotSuggestable` 409; `Busy`, `DailyLimit` 429; `Unavailable` 503;
//! `Failed` 500.
//!
//! Nothing here logs a question, a reply or a passage.

use axum::{
    Json,
    body::Bytes,
    extract::{Path, State},
    http::{HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use super::service::{self, ChatError, ChatView};
use super::{ChatMode, lines};
use crate::identity::CurrentUser;
use crate::server::AppState;

pub const NOT_FOUND_MESSAGE: &str = "Not found.";
pub const BAD_REQUEST_MESSAGE: &str = "That request could not be read.";
pub const EMPTY_MESSAGE: &str = "Type a question first.";
pub const TOO_LONG_MESSAGE: &str = "That question is too long. Please shorten it.";
pub const DAILY_LIMIT_MESSAGE: &str = "You have reached today's chat limit. It resets at midnight.";
pub const NOT_OFFERED_MESSAGE: &str = "Chat is not offered for this run.";
pub const IN_FLIGHT_MESSAGE: &str = "Wait for the current reply to finish.";
pub const NOT_SUGGESTABLE_MESSAGE: &str = "Only a finished answer can be suggested.";
pub const FAILED_MESSAGE: &str = "Chat couldn't load. Please try again.";
/// Seconds a busy refusal asks the client to wait.
const BUSY_RETRY_SECS: &str = "60";

fn parts(e: ChatError) -> (StatusCode, &'static str, String, bool) {
    use ChatError::*;
    match e {
        NotFound => (
            StatusCode::NOT_FOUND,
            "not_found",
            NOT_FOUND_MESSAGE.into(),
            false,
        ),
        Empty => (
            StatusCode::BAD_REQUEST,
            "empty",
            EMPTY_MESSAGE.into(),
            false,
        ),
        TooLong { safety_line } => (
            StatusCode::BAD_REQUEST,
            "too_long",
            TOO_LONG_MESSAGE.into(),
            safety_line,
        ),
        Identifiers {
            message,
            safety_line,
        } => (
            StatusCode::UNPROCESSABLE_ENTITY,
            "identifiers",
            message,
            safety_line,
        ),
        InFlight { safety_line } => (
            StatusCode::CONFLICT,
            "in_flight",
            IN_FLIGHT_MESSAGE.into(),
            safety_line,
        ),
        NotOffered { safety_line } => (
            StatusCode::CONFLICT,
            "not_offered",
            NOT_OFFERED_MESSAGE.into(),
            safety_line,
        ),
        DailyLimit { safety_line } => (
            StatusCode::TOO_MANY_REQUESTS,
            "daily_limit",
            DAILY_LIMIT_MESSAGE.into(),
            safety_line,
        ),
        Busy { safety_line } => (
            StatusCode::TOO_MANY_REQUESTS,
            "busy",
            lines::BUSY.into(),
            safety_line,
        ),
        Unavailable { safety_line } => (
            StatusCode::SERVICE_UNAVAILABLE,
            "unavailable",
            lines::UNAVAILABLE.into(),
            safety_line,
        ),
        Failed => (
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed",
            FAILED_MESSAGE.into(),
            false,
        ),
        BadFeedback => (
            StatusCode::BAD_REQUEST,
            "bad_request",
            BAD_REQUEST_MESSAGE.into(),
            false,
        ),
        BadMode { safety_line } => (
            StatusCode::BAD_REQUEST,
            "bad_request",
            BAD_REQUEST_MESSAGE.into(),
            safety_line,
        ),
        NotSuggestable => (
            StatusCode::CONFLICT,
            "not_suggestable",
            NOT_SUGGESTABLE_MESSAGE.into(),
            false,
        ),
    }
}

fn error_response(e: ChatError) -> Response {
    let (status, kind, message, safety_line) = parts(e);
    let fixed: Vec<&str> = if safety_line {
        vec![lines::WARNING_LINE]
    } else {
        Vec::new()
    };
    let mut res = (
        status,
        Json(json!({
            "error": kind,
            "message": message,
            "safety_line": safety_line,
            "lines": fixed,
        })),
    )
        .into_response();
    if kind == "busy" {
        res.headers_mut().insert(
            header::RETRY_AFTER,
            HeaderValue::from_static(BUSY_RETRY_SECS),
        );
    }
    res
}

fn bad_request() -> Response {
    error_response(ChatError::BadFeedback)
}

fn run_id(raw: &str) -> Result<Uuid, ChatError> {
    Uuid::parse_str(raw).map_err(|_| ChatError::NotFound)
}

fn int_id(raw: &str) -> Result<i64, ChatError> {
    raw.parse::<i64>().map_err(|_| ChatError::NotFound)
}

#[derive(Serialize)]
struct RunChat {
    #[serde(flatten)]
    view: ChatView,
    footer: &'static str,
}

/// `GET /api/runs/{id}/chat`.
pub async fn state_handler(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<String>,
) -> Response {
    let res = match run_id(&id) {
        Ok(id) => service::state_for(&state, &user, id).await,
        Err(e) => Err(e),
    };
    match res {
        Ok(view) => Json(RunChat {
            view,
            footer: lines::FOOTER,
        })
        .into_response(),
        Err(e) => error_response(e),
    }
}

#[derive(Deserialize)]
struct AskBody {
    question: String,
    /// Read loosely: a mode that is missing or the wrong type must not hide
    /// the question from the safety check.
    #[serde(default)]
    mode: serde_json::Value,
}

/// `POST /api/runs/{id}/chat` with `{question, mode}`.
pub async fn ask_handler(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<String>,
    body: Bytes,
) -> Response {
    let run = match run_id(&id) {
        Ok(r) => r,
        Err(e) => return error_response(e),
    };
    let Ok(b) = serde_json::from_slice::<AskBody>(&body) else {
        return bad_request();
    };
    let Some(mode) = b.mode.as_str().and_then(ChatMode::parse) else {
        // The question was readable, so it still gets the safety line.
        return error_response(ChatError::BadMode {
            safety_line: lines::warning_symptoms(&b.question),
        });
    };
    match service::ask(&state, &user, run, &b.question, mode).await {
        Ok(a) => {
            let fixed: Vec<&str> = if a.safety_line {
                vec![lines::WARNING_LINE]
            } else {
                Vec::new()
            };
            (
                StatusCode::ACCEPTED,
                Json(json!({
                    "turn_id": a.turn_id,
                    "safety_line": a.safety_line,
                    "lines": fixed,
                })),
            )
                .into_response()
        }
        Err(e) => error_response(e),
    }
}

/// `GET /api/chat/turns/{id}`.
pub async fn turn_handler(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<String>,
) -> Response {
    let res = match int_id(&id) {
        Ok(id) => service::turn_view(&state, &user, id).await,
        Err(e) => Err(e),
    };
    match res {
        Ok(v) => Json(v).into_response(),
        Err(e) => error_response(e),
    }
}

/// `GET /api/chat/passages/{id}`: the id as stored in a turn's
/// `passage_ids` (positive: a passage; negative: an approved Q&A).
pub async fn passage_handler(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<String>,
) -> Response {
    let res = match int_id(&id) {
        Ok(id) => service::cited_source(&state, &user, id).await,
        Err(e) => Err(e),
    };
    match res {
        Ok(v) => Json(v).into_response(),
        Err(e) => error_response(e),
    }
}

#[derive(Deserialize)]
struct FeedbackBody {
    value: i64,
}

/// `POST /api/chat/turns/{id}/feedback` with `{value: 1 | -1}`.
pub async fn feedback_handler(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<String>,
    body: Bytes,
) -> Response {
    let turn = match int_id(&id) {
        Ok(t) => t,
        Err(e) => return error_response(e),
    };
    let Ok(b) = serde_json::from_slice::<FeedbackBody>(&body) else {
        return bad_request();
    };
    match service::give_feedback(&state, &user, turn, b.value).await {
        Ok(()) => Json(json!({"feedback": b.value})).into_response(),
        Err(e) => error_response(e),
    }
}

/// `POST /api/chat/turns/{id}/suggest`.
pub async fn suggest_handler(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<String>,
) -> Response {
    let res = match int_id(&id) {
        Ok(id) => service::suggest(&state, &user, id).await,
        Err(e) => Err(e),
    };
    match res {
        Ok(_) => Json(json!({"suggested": true})).into_response(),
        Err(e) => error_response(e),
    }
}
