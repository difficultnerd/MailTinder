//! The `unsub` Cloud Run service: Cloud Tasks calls it when an unsubscribe
//! job falls due (T-701, S7 5.12 API-INT-1).
//!
//! Exactly one route: `POST /internal/v1/unsubscribe-jobs/{job_id}/run`. The
//! OIDC caller check runs before anything else; a failure is a `401` with an
//! empty body and a logged `internal_auth_failed` security event.
#![allow(
    clippy::must_use_candidate,
    clippy::missing_errors_doc,
    clippy::missing_panics_doc,
    clippy::doc_markdown,
    clippy::module_name_repetitions
)]

pub mod config;
pub mod mailto;
pub mod one_click;
pub mod quota;
pub mod runner;
pub mod sender;

use axum::extract::{Path, State};
use axum::http::header::AUTHORIZATION;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::Router;
use domain::JobId;
use obs::{security_event, SecurityEvent};
use svc_common::internal_auth::verify_internal_caller;

use crate::runner::{run_job, Delivery, RunResponse, UnsubState};

/// The one route template, for the log registry. A release build has no other
/// route (test `api_int_1_release_build_has_no_test_routes`).
pub const ROUTE_TEMPLATES: &[&str] = &["/internal/v1/unsubscribe-jobs/{job_id}/run"];

/// The one internal route.
pub const RUN_PATH: &str = "/internal/v1/unsubscribe-jobs/:job_id/run";

/// The header Cloud Tasks uses to report the retry count.
pub const RETRY_COUNT_HEADER: &str = "X-CloudTasks-TaskRetryCount";

/// Build the router. One route, no test hooks.
pub fn router(state: UnsubState) -> Router {
    Router::new()
        .route(RUN_PATH, post(run_handler))
        .with_state(state)
}

/// The internal run handler.
async fn run_handler(
    State(state): State<UnsubState>,
    Path(job_id): Path<String>,
    headers: HeaderMap,
) -> Response {
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

    // 2. The job ID must be a UUID.
    let Ok(uuid) = uuid::Uuid::parse_str(&job_id) else {
        security_event(&SecurityEvent {
            action: "invalid_request",
            outcome: "invalid_request",
            user: None,
            request_id: None,
            amr: None,
            provider: None,
            method: None,
        });
        return StatusCode::BAD_REQUEST.into_response();
    };

    // 3. The retry count is the attempt; missing or unparsable is 0.
    let delivery = Delivery::from_retry_count(
        headers
            .get(RETRY_COUNT_HEADER)
            .and_then(|v| v.to_str().ok()),
    );

    // 14. 200 for done, 503 for retry; bodies are empty JSON.
    match run_job(&state, &JobId(uuid), delivery).await {
        Ok(RunResponse::Done) => (StatusCode::OK, "{}").into_response(),
        Ok(RunResponse::Retry) | Err(_) => (StatusCode::SERVICE_UNAVAILABLE, "{}").into_response(),
    }
}
