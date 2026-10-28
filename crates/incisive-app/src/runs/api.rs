//! Run status and deletion: the JSON routes and the shared operations behind
//! the server functions. Server only.
//!
//! Every operation is scoped to the signed-in user: another user's run behaves
//! exactly like a missing one (404), owner included. Nothing here logs or
//! audits question text; audit entries carry run ids only.
//!
//! Deleting a run (ruling R-1b), in order:
//! 1. if the run is active, ask this instance's worker to stop (best effort:
//!    the worker may be on another instance, which notices the row is gone);
//! 2. delete the rows;
//! 3. release the run's spend reservation (idempotent; charges stand);
//! 4. audit `run_deleted` (ids only).

use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::Deserialize;
use serde_json::json;
use uuid::Uuid;

use super::{RunSummary, RunView, store};
use crate::audit::{self, Event};
use crate::identity::{CurrentUser, User};
use crate::server::AppState;
use crate::spend;

pub const NOT_FOUND_MESSAGE: &str = super::RUN_NOT_FOUND_MESSAGE;
pub const RUN_API_FAILED_MESSAGE: &str = "Incisive couldn't load your questions. Please try again.";
pub const DEFAULT_LIMIT: i64 = 20;
pub const MAX_LIMIT: i64 = 100;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunApiError {
    NotFound,
    Failed,
}

impl RunApiError {
    pub fn message(&self) -> &'static str {
        match self {
            RunApiError::NotFound => NOT_FOUND_MESSAGE,
            RunApiError::Failed => RUN_API_FAILED_MESSAGE,
        }
    }
}

fn failed(what: &'static str, user_id: i64) -> RunApiError {
    // Fixed text and ids only: a database error could echo stored values.
    tracing::error!(user_id, what, "run api failed");
    RunApiError::Failed
}

/// Clamp a requested list size to 1..=100 (default 20).
pub fn clamp_limit(limit: Option<i64>) -> i64 {
    limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT)
}

/// The user's runs, newest first.
pub async fn list_view(
    state: &AppState,
    user: &User,
    limit: Option<i64>,
) -> Result<Vec<RunSummary>, RunApiError> {
    store::list_runs(&state.pool, user.id, clamp_limit(limit))
        .await
        .map_err(|_| failed("list", user.id))
}

/// One of the user's runs with its steps and answer.
pub async fn get_view(state: &AppState, user: &User, run_id: Uuid) -> Result<RunView, RunApiError> {
    match store::get_run_for_user(&state.pool, user.id, run_id).await {
        Ok(Some(v)) => Ok(v),
        Ok(None) => Err(RunApiError::NotFound),
        Err(_) => Err(failed("get", user.id)),
    }
}

async fn audit_deleted(state: &AppState, user: &User, run_id: Uuid) {
    if audit::append(
        &state.pool,
        Some(run_id),
        Some(user.id),
        Event::RunDeleted,
        audit::run_deleted_detail(run_id),
    )
    .await
    .is_err()
    {
        // The run is already gone; there is nothing to undo.
        tracing::error!(%run_id, "audit append failed for run_deleted");
    }
}

async fn release_reservation(state: &AppState, user: &User, run_id: Uuid) {
    if spend::release(&state.pool, user.id, run_id).await.is_err() {
        // Startup/periodic recovery cannot see a deleted run; the worker's own
        // release is the backstop. Log the id only.
        tracing::error!(%run_id, "reservation release failed after delete");
    }
}

/// Delete one of the user's runs.
pub async fn delete_one(state: &AppState, user: &User, run_id: Uuid) -> Result<(), RunApiError> {
    let view = get_view(state, user, run_id).await?;
    if view.summary.status.is_active() {
        // Best effort: false when the worker is on another instance.
        let _ = state.worker.cancel(run_id);
    }
    // The whole reservation is released below, even if the worker still has
    // Claude calls in flight: their late charges book without headroom, a
    // bounded overshoot (final review M-4; `spend` module, overshoot notes).
    match store::delete_run(&state.pool, user.id, run_id).await {
        Ok(true) => {}
        // Deleted by someone else between the read and the delete.
        Ok(false) => return Err(RunApiError::NotFound),
        Err(_) => return Err(failed("delete", user.id)),
    }
    release_reservation(state, user, run_id).await;
    audit_deleted(state, user, run_id).await;
    Ok(())
}

/// Delete all of the user's runs; returns how many.
pub async fn delete_all(state: &AppState, user: &User) -> Result<u64, RunApiError> {
    let active = store::active_run_ids(&state.pool, user.id)
        .await
        .map_err(|_| failed("delete_all", user.id))?;
    for id in active {
        let _ = state.worker.cancel(id);
    }
    let deleted = store::delete_all_runs_returning(&state.pool, user.id)
        .await
        .map_err(|_| failed("delete_all", user.id))?;
    for (id, was_active) in &deleted {
        if *was_active {
            release_reservation(state, user, *id).await;
        }
        audit_deleted(state, user, *id).await;
    }
    Ok(deleted.len() as u64)
}

fn error_response(e: RunApiError) -> Response {
    let (status, kind) = match e {
        RunApiError::NotFound => (StatusCode::NOT_FOUND, "not_found"),
        RunApiError::Failed => (StatusCode::INTERNAL_SERVER_ERROR, "failed"),
    };
    (status, Json(json!({"error": kind, "message": e.message()}))).into_response()
}

/// A malformed id cannot match any run: same 404 as a missing one.
fn parse_id(raw: &str) -> Result<Uuid, RunApiError> {
    Uuid::parse_str(raw).map_err(|_| RunApiError::NotFound)
}

#[derive(Deserialize)]
pub struct ListQuery {
    pub limit: Option<i64>,
}

/// `GET /api/runs?limit=20`.
pub async fn list_handler(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Query(q): Query<ListQuery>,
) -> Response {
    match list_view(&state, &user, q.limit).await {
        Ok(v) => Json(v).into_response(),
        Err(e) => error_response(e),
    }
}

/// `GET /api/runs/{id}`.
pub async fn get_handler(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<String>,
) -> Response {
    let res = match parse_id(&id) {
        Ok(id) => get_view(&state, &user, id).await,
        Err(e) => Err(e),
    };
    match res {
        Ok(v) => Json(v).into_response(),
        Err(e) => error_response(e),
    }
}

/// `DELETE /api/runs/{id}`.
pub async fn delete_handler(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
    Path(id): Path<String>,
) -> Response {
    let res = match parse_id(&id) {
        Ok(id) => delete_one(&state, &user, id).await,
        Err(e) => Err(e),
    };
    match res {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => error_response(e),
    }
}

/// `DELETE /api/runs`.
pub async fn delete_all_handler(
    State(state): State<AppState>,
    CurrentUser(user): CurrentUser,
) -> Response {
    match delete_all(&state, &user).await {
        Ok(n) => Json(json!({"deleted": n})).into_response(),
        Err(e) => error_response(e),
    }
}
