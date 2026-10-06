//! Route handlers, one module per feature area.

pub mod feed;
pub mod invite_requests;
pub mod invites;
pub mod mailboxes;
pub mod needs_attention;
pub mod progress;
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
        .route("/api/v1/feed/next", post(feed::next_page_handler))
        .route("/api/v1/progress", get(progress::get_progress))
        .route("/api/v1/needs-attention", get(needs_attention::list))
        .route(
            "/api/v1/needs-attention/:item_id/resolve",
            post(needs_attention::resolve),
        )
        .route(
            "/api/v1/needs-attention/:item_id/dismiss",
            post(needs_attention::dismiss),
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
