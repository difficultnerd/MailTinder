//! Test-only routes, compiled only with the `testkit` feature (S10 3.2).
//!
//! They let the e2e harness (`scripts/e2e.sh`) create an invite without an
//! admin session or a real mailbox: the request goes through the same shared
//! invite service the admin route uses, so the hash is stored and only the raw
//! token is returned. `tests/release_routes.rs` proves they are absent without
//! the feature. The router is merged after the security, CSRF and rate-limit
//! layers, so a synthetic call needs no browser session; it is never part of a
//! release build.
#![allow(
    clippy::missing_errors_doc,
    clippy::missing_panics_doc,
    clippy::must_use_candidate,
    clippy::doc_markdown
)]

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::Response;
use axum::routing::post;
use axum::Router;
use domain::EmailAddress;
use serde::{Deserialize, Serialize};
use svc_common::invites::{upsert_pending_invite, UpsertError};

use crate::error::ApiError;
use crate::http::json::{json_ok, ApiJson};
use crate::state::AppState;

/// The testkit route table, mounted at the app root (not under `/api/v1`).
pub fn router() -> Router<AppState> {
    Router::new().route("/internal/test/invites", post(create_invite))
}

/// `POST /internal/test/invites {email}` body.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateInviteBody {
    pub email: String,
}

/// The raw invite token, returned once to the harness.
#[derive(Serialize)]
struct InviteToken {
    token: String,
}

/// Create a pending invite for `email` and return the raw token.
async fn create_invite(
    State(state): State<AppState>,
    ApiJson(body): ApiJson<CreateInviteBody>,
) -> Result<Response, ApiError> {
    let email = EmailAddress::parse(&body.email).map_err(|_| ApiError::InvalidRequest {
        fields: vec!["/email".to_owned()],
    })?;
    let (_record, raw, _upserted) = upsert_pending_invite(
        state.ports.store.as_ref(),
        state.ports.system_keys.as_ref(),
        &state.config.email_lookup_key,
        state.ports.clock.as_ref(),
        state.ports.rng.as_ref(),
        &email,
    )
    .await
    .map_err(from_upsert)?;
    Ok(json_ok(
        StatusCode::CREATED,
        &InviteToken {
            token: raw.expose().to_owned(),
        },
    ))
}

/// Map the shared invite creation error onto the route's `ApiError` (S7 4).
fn from_upsert(error: UpsertError) -> ApiError {
    match error {
        UpsertError::Store(e) => e.into(),
        UpsertError::Key(_) => ApiError::Internal,
    }
}
