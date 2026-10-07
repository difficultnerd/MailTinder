//! The `worker` Cloud Run service: Cloud Scheduler calls it every 15 minutes to
//! expire overdue unsubscribe jobs and purge expired records (T-706, S7 5.12
//! API-INT-2).
//!
//! Exactly one route: `POST /internal/v1/sweep`. The OIDC caller check runs
//! before anything else; a failure is a `401` with an empty body and a logged
//! `internal_auth_failed` security event. A sweep with a failed step answers
//! `500` so Cloud Scheduler retries and monitoring sees it.
#![allow(
    clippy::must_use_candidate,
    clippy::missing_errors_doc,
    clippy::missing_panics_doc,
    clippy::doc_markdown,
    clippy::module_name_repetitions
)]

pub mod sweep;
pub mod sweeps;

use axum::extract::State;
use axum::http::header::AUTHORIZATION;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::Router;
use obs::{security_event, SecurityEvent};
use svc_common::internal_auth::verify_internal_caller;

use crate::sweep::{run_sweep, WorkerState};

/// The one route template, for the log registry.
pub const ROUTE_TEMPLATES: &[&str] = &["/internal/v1/sweep"];

/// The one internal route.
pub const SWEEP_PATH: &str = "/internal/v1/sweep";

/// Build the router. One route, no test hooks.
pub fn router(state: WorkerState) -> Router {
    Router::new()
        .route(SWEEP_PATH, post(sweep_handler))
        .with_state(state)
}

/// The internal sweep handler.
async fn sweep_handler(State(state): State<WorkerState>, headers: HeaderMap) -> Response {
    // 1. The caller check runs before any other work.
    let authorization = headers.get(AUTHORIZATION).and_then(|v| v.to_str().ok());
    if verify_internal_caller(state.ports.caller.as_ref(), &state.auth, authorization)
        .await
        .is_err()
    {
        security_event(&SecurityEvent {
            action: "internal_auth_failed",
            outcome: "unauthenticated",
            user: None,
            request_id: None,
            amr: None,
            provider: None,
            method: None,
        });
        // 401 with an empty body.
        return StatusCode::UNAUTHORIZED.into_response();
    }

    // 2. Sweep. Counts only, never an ID, link or name (S7 5.12). A failed step
    // answers 500 with the same body so Cloud Scheduler retries.
    let counts = run_sweep(&state).await;
    let status = if counts.failed_steps.is_empty() {
        StatusCode::OK
    } else {
        StatusCode::INTERNAL_SERVER_ERROR
    };
    (status, axum::Json(counts)).into_response()
}
