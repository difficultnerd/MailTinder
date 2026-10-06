//! The `Aad::field` and `SystemAad::field` constants for every encrypted field.

/// scope = `mailbox_id`
pub const MAILBOX_EMAIL: &str = "mailbox.email_address";
/// scope = `mailbox_id` (S6 5: user + mailbox + field)
pub const MAILBOX_REFRESH_TOKEN: &str = "mailbox.refresh_token";
/// scope = `job_id`
pub const JOB_TARGET: &str = "job.target";
/// scope = `job_id`; sealed by T-605 when the job is queued, reused by T-701 (T-701 trap 3)
pub const JOB_SENDER_DISPLAY: &str = "job.sender_display";
/// scope = `item_id`
pub const NA_SENDER_DISPLAY: &str = "needs_attention.sender_display";
/// scope = `item_id`
pub const NA_LINK: &str = "needs_attention.link";
/// system key, scope = `invite_id`
pub const INVITE_EMAIL: &str = "invite.email_address";
/// system key, scope = `request_id`
pub const INVITE_REQUEST_EMAIL: &str = "invite_request.email_address";
/// system key, scope = `session_record_id`
pub const PRE_AUTH_STATE: &str = "session.pre_auth.oauth_state";
pub const PRE_AUTH_NONCE: &str = "session.pre_auth.nonce";
pub const PRE_AUTH_PKCE_VERIFIER: &str = "session.pre_auth.pkce_verifier";
pub const PRE_AUTH_PENDING_EMAIL: &str = "session.pre_auth.pending_email";
/// scope = `user_id` (T-405)
pub const APP_FOLDER_FILE: &str = "app_folder.file";
/// scope = the literal `"app_folder"`: the file moves between Drives as bytes (T-602b)
pub const APP_FOLDER_USER_STATE: &str = "user_state";
