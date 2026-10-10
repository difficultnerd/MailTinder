//! Closed registries of every value that may appear in a log line.
//!
//! A value that is not in one of these lists is replaced with
//! `"[unregistered]"` and counted, so a reviewer can find stray log calls.
//! The lists are closed on purpose: a task that logs a new action, outcome or
//! operation adds its name here in the same pull request.

use std::sync::atomic::{AtomicU64, Ordering};

pub const EVENTS: &[&str] = &["request", "security", "metric", "op", "panic"];

pub const ACTIONS: &[&str] = &[
    "sign_in",
    "step_up",
    "session_end",
    "session_ended_by_admin",
    "mailbox_link",
    "mailbox_reconnect",
    "mailbox_unlink",
    "app_folder",
    "invite_create",
    "invite_revoke",
    "invite_use",
    "invite_request_approve",
    "invite_request_decline",
    "experiments_consent",
    "experiments_opt_in",
    "experiments_opt_out",
    "kill_switch_change",
    "snapshot_save",
    "snapshot_delete",
    "admin_action",
    "authz_failure",
    "csrf_failure",
    "rate_limit_hit",
    "unsub_job_outcome",
    "internal_auth_failed",
    "job_owner_mismatch",
    "invalid_request",
    "account_delete",
    "sign_in_required",
    "egress_tls_failure",
    "egress_refused",
    "token_revoke",
    // Metric event types (S10 8).
    "swipe",
    "undo",
    "unsub_outcome",
    "delivery_check_outcome",
    "undo_failed",
    "unsub_after_undo",
    "history_missing",
];

pub const OUTCOMES: &[&str] = &[
    "success",
    "unreadable",
    "failure",
    "refused",
    "expired",
    "replaced",
    "admin_ended",
    "signed_out",
    "sent",
    "cancelled",
    "needs_attention",
    "failed",
    "tls_failure",
    "host_refused",
    "address_refused",
    "redirected",
    "timed_out",
    "rejected",
    "revoke_failed",
    "token_invalid",
    // Job outcome codes (T-701, S3).
    "one_click_accepted",
    "mailto_sent",
    "http_rejected",
    "retries_exhausted",
    "batched",
    "mailbox_removed",
    "owner_mismatch",
    "rate_limited",
    // Account deletion (T-803).
    "started",
    "deleted",
    // Sign-in and invite outcomes (T-502b, S7 3.4).
    "signed_in",
    "joined",
    "linked",
    "reconnected",
    "stepped_up",
    // Step-up outcomes (T-504).
    "wrong_account",
    "stale_auth_time",
    "not_invited",
    "not_registered",
    "invite_invalid",
    "email_mismatch",
    "email_unverified",
    "mailbox_linked_elsewhere",
    "step_up_wrong_account",
    "consent_blocked",
    "state_invalid",
    "id_token_invalid",
    "used",
    // S7 section 4 error codes.
    "invalid_request",
    "unauthenticated",
    "csrf_failed",
    "forbidden",
    "step_up_required",
    "not_found",
    "app_folder_move_failed",
    "last_mailbox",
    "mailbox_needs_sign_in",
    "message_changed",
    "category_exists",
    "consent_outdated",
    "experiment_unavailable",
    "versions_mixed",
    "snapshot_limit",
    "not_acceptable",
    "undo_expired",
    "payload_too_large",
    "unsupported_media_type",
    "rate_limited",
    "provider_error",
    "provider_unavailable",
    "internal_error",
    // Swipe actions, carried by the `swipe` and `undo` metric outcomes
    // (S10 8, T-1114).
    "keep",
    "skip",
    "file",
    "reject",
];

pub const OPS: &[&str] = &[
    "gmail.messages.list",
    "gmail.messages.get",
    "gmail.messages.modify",
    "gmail.messages.send",
    "gmail.threads.list",
    "gmail.labels.list",
    "drive.files.update",
    "drive.files.get",
    "drive.files.create",
    "egress.one_click",
    "egress.call",
    "unsub.one_click",
    "tasks.create",
    "kms.decrypt",
    "kms.encrypt",
    "secrets.access",
    "firestore.get",
    "firestore.commit",
    "firestore.run_query",
    "identity_config_error",
    // Sign-in and invite operations (T-502b).
    "token_revoke",
    "sign_in_store_grant",
    "join_missing_grant",
    // Unsubscribe token minting (T-703).
    "unsub.mint",
    // The runner delivery of an unsubscribe job (T-1114), including the benign
    // race where a successful undo cancelled the job before the delivery ran.
    "unsub.run",
];

pub const AMR_VALUES: &[&str] = &[
    "pwd", "mfa", "otp", "hwk", "swk", "sms", "user", "pin", "fpt", "face", "kba",
];

pub const PROVIDERS: &[&str] = &["gmail", "graph"];

pub const JOB_METHODS: &[&str] = &["one_click", "mailto"];

static HTTP_ROUTES: std::sync::OnceLock<&'static [&'static str]> = std::sync::OnceLock::new();
static UNREGISTERED: AtomicU64 = AtomicU64::new(0);

/// Called once at start-up by each service with its router's route templates,
/// e.g. "/api/v1/swipes". Templates only, never raw paths.
pub fn register_http_routes(routes: &'static [&'static str]) {
    let _ = HTTP_ROUTES.set(routes);
}

/// True if `value` is a registered HTTP route template or an operation name.
pub fn is_route(value: &str) -> bool {
    if OPS.contains(&value) {
        return true;
    }
    HTTP_ROUTES
        .get()
        .is_some_and(|routes| routes.contains(&value))
}

/// Count a value that was replaced because it was not in a registry.
pub fn count_unregistered() {
    UNREGISTERED.fetch_add(1, Ordering::Relaxed);
}

/// Number of values replaced because they were not in a registry.
pub fn unregistered_count() -> u64 {
    UNREGISTERED.load(Ordering::Relaxed)
}

/// True if `value` is in `list`.
#[must_use]
pub fn in_list(list: &[&str], value: &str) -> bool {
    list.contains(&value)
}
