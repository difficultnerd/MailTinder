//! Route handlers, one module per feature area.

pub mod account;
pub mod admin_users;
pub mod categories;
pub mod experiments;
pub mod feed;
pub mod invite_requests;
pub mod invites;
pub mod mailboxes;
pub mod needs_attention;
pub mod progress;
pub mod session;
pub mod swipes;

use axum::routing::{delete, get, patch, post};
use axum::Router;

use crate::state::AppState;

/// The invite, invite-request and session routes (T-505, T-506), relative to the
/// app root.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/session", get(session::get_session))
        .route(
            "/api/v1/me/experiments",
            get(experiments::get_experiments).put(experiments::put_experiments),
        )
        .route("/api/v1/account", delete(account::delete_account))
        .route("/api/v1/mailboxes", get(mailboxes::list_mailboxes))
        .route(
            "/api/v1/mailboxes/:mailbox_id",
            delete(mailboxes::disconnect_mailbox),
        )
        .route("/api/v1/feed/next", post(feed::next_page_handler))
        // The 300-jobs-per-user-per-day Firestore limit (`policies::UNSUB_JOBS`)
        // is checked inside `services::reject::execute`, before the provider
        // change and only for a reject that queues a job: one hit per swipe.
        .route("/api/v1/swipes", post(swipes::create))
        .route("/api/v1/swipes/undo", post(swipes::undo))
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
        // API-CAT-1 to API-CAT-5 (T-607a). Reads and writes use the default
        // rate limits through `default_limit_layer`, and the writes are
        // CSRF-checked by `csrf_layer` like every other `/api/v1` route.
        .route(
            "/api/v1/categories",
            get(categories::list).post(categories::create),
        )
        .route(
            "/api/v1/categories/:category_id",
            patch(categories::rename).delete(categories::delete),
        )
        .route(
            "/api/v1/categories/:category_id/messages",
            get(categories::messages),
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
        .route("/api/v1/admin/users", get(admin_users::list))
        .route(
            "/api/v1/admin/users/:user_id/sessions",
            delete(admin_users::end),
        )
}
