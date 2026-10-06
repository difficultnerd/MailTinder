//! Route handlers, one module per feature area.

pub mod invite_requests;
pub mod invites;
pub mod mailboxes;
pub mod session;

use axum::routing::{delete, get, post};
use axum::Router;

use crate::state::AppState;

/// The invite, invite-request and session routes (T-505, T-506), relative to the
/// app root.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/session", get(session::get_session))
        .route("/api/v1/mailboxes", get(mailboxes::list_mailboxes))
        .route(
            "/api/v1/mailboxes/:mailbox_id",
            delete(mailboxes::disconnect_mailbox),
        )
        .route("/api/v1/auth/sign-out", post(session::sign_out))
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
