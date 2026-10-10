//! Test-only routes, compiled only with the `testkit` feature (S10 3.2).
//!
//! They let the e2e harness (`scripts/e2e.sh`) create an invite without an
//! admin session or a real mailbox: the request goes through the same shared
//! invite service the admin route uses, so the hash is stored and only the raw
//! token is returned. `tests/release_routes.rs` proves the invite route is absent
//! without the feature or runtime `MT_E2E=1`, and returns 201 with both enabled.
//! The router is merged after the security, CSRF and rate-limit
//! layers, so a synthetic call needs no browser session; it is never part of a
//! production build.
//!
//! T-1101c adds three more: `advance-clock` moves the e2e clock forward (S10
//! 6.3: a journey passes a queued job's due time, or the account-deletion sweep
//! horizon, without waiting for real hours), `unsub-delay` overrides the delay
//! a queued job is planned with for one journey, and `sweep` runs the worker's
//! account-deletion backstop once over the emulator store.
#![allow(
    clippy::missing_errors_doc,
    clippy::missing_panics_doc,
    clippy::must_use_candidate,
    clippy::doc_markdown
)]

use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Extension, State};
use axum::http::StatusCode;
use axum::response::Response;
use axum::routing::post;
use axum::Router;
use domain::EmailAddress;
use obs::Pseudonymiser;
use serde::{Deserialize, Serialize};
use serde_json::json;
use svc_common::invites::{upsert_pending_invite, UpsertError};
use testkit::OffsetClock;

use crate::error::ApiError;
use crate::http::json::{json_ok, ApiJson};
use crate::state::AppState;

/// The testkit route table, mounted at the app root (not under `/api/v1`).
///
/// `clock` is the api's own e2e clock ([`crate::startup_e2e::e2e_clock`]), so
/// advancing it moves the `now` every service the api runs reads.
pub fn router(clock: Arc<OffsetClock>) -> Router<AppState> {
    Router::new()
        .route("/internal/test/invites", post(create_invite))
        .route("/internal/test/advance-clock", post(advance_clock))
        .route("/internal/test/unsub-delay", post(set_unsub_delay))
        .route("/internal/test/sweep", post(sweep))
        .layer(Extension(clock))
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

/// `POST /internal/test/advance-clock {seconds}` body.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdvanceClockBody {
    pub seconds: i64,
}

/// How far ahead of the real clock the e2e clock now reads.
#[derive(Serialize)]
struct ClockOffset {
    offset_s: i64,
}

/// Move the e2e clock `seconds` further forward. Nothing waits real time for a
/// queued job's due time, the undo window or the deletion sweep horizon: the
/// journey jumps the clock past them (T-1101c).
async fn advance_clock(
    Extension(clock): Extension<Arc<OffsetClock>>,
    ApiJson(body): ApiJson<AdvanceClockBody>,
) -> Result<Response, ApiError> {
    clock.advance(time::Duration::seconds(body.seconds));
    Ok(json_ok(
        StatusCode::OK,
        &ClockOffset {
            offset_s: clock.offset().whole_seconds(),
        },
    ))
}

/// `POST /internal/test/unsub-delay {seconds}` body; `null` clears the override.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UnsubDelayBody {
    pub seconds: Option<u64>,
}

/// Override the delay a queued unsubscribe's due time is planned with, for the
/// rest of the run, or clear the override.
///
/// A journey that must disconnect a mailbox while one of its unsubscribes is
/// still queued needs the job's due time to sit beyond the UI round trip the
/// disconnect takes; clearing it restores the delay the run started with
/// (T-1101c).
async fn set_unsub_delay(ApiJson(body): ApiJson<UnsubDelayBody>) -> Result<Response, ApiError> {
    let delay = body.seconds.map(Duration::from_secs);
    testkit::e2e::set_unsub_delay(delay);
    Ok(json_ok(StatusCode::OK, &json!({ "seconds": body.seconds })))
}

/// Run the worker's account-deletion backstop once (S2 AU-06 AC1) and report
/// what it removed. The 24-hour sweep the production `worker` runs is a route
/// away from the api only in production; here the same entry point runs in
/// process, over this run's emulator store (T-1101c).
async fn sweep(State(state): State<AppState>) -> Result<Response, ApiError> {
    let pseudo = Pseudonymiser::new(state.config.rate_key.clone());
    let counts = worker::sweeps::run_sweeps(state.ports.store.as_ref(), &pseudo).await?;
    Ok(json_ok(
        StatusCode::OK,
        &json!({ "deleted_records": counts.deleted_records }),
    ))
}

/// Map the shared invite creation error onto the route's `ApiError` (S7 4).
fn from_upsert(error: UpsertError) -> ApiError {
    match error {
        UpsertError::Store(e) => e.into(),
        UpsertError::Key(_) => ApiError::Internal,
    }
}
