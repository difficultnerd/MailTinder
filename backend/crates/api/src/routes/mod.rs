//! Route handlers, one module per feature area.

pub mod invite_requests;
pub mod invites;

use axum::routing::{delete, get, post};
use axum::Router;

use crate::state::AppState;

/// The invite and invite-request routes (T-505), relative to the app root.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/invite-requests", post(invite_requests::request))
        .route(
            "/api/v1/admin/invites",
            get(invites::list).post(invites::create),
        )
        .route(
            "/api/v1/admin/invites/:invite_id/resend",
            post(invites::resend),
        )
        .route("/api/v1/admin/invites/:invite_id", delete(invites::revoke))
        .route("/api/v1/admin/invite-requests", get(invite_requests::list))
        .route(
            "/api/v1/admin/invite-requests/:request_id/approve",
            post(invite_requests::approve),
        )
        .route(
            "/api/v1/admin/invite-requests/:request_id/decline",
            post(invite_requests::decline),
        )
}
