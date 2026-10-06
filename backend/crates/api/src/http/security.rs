//! The `authz_failure` security event (S6 section 5; ASVS V16.3.2).
//!
//! One helper so every refused authorisation logs the same fields: the
//! pseudonymous user, the request ID and the route verb. Never an address, a
//! mailbox ID, an invite ID or a token (S5, S6 5).

use domain::UserId;
use obs::{Pseudonymiser, SecurityEvent};
use uuid::Uuid;

use crate::state::AppState;

/// Log one refused authorisation attempt (event `security`, action
/// `authz_failure`, outcome `refused`).
///
/// `method` is the lower-case route verb (`"get"`, `"post"`, `"delete"`) when
/// the caller knows it; `request_id` is `None` in paths without the extractor.
pub(crate) fn authz_failure(
    state: &AppState,
    user: &UserId,
    request_id: Option<Uuid>,
    method: Option<&'static str>,
) {
    obs::security_event(&SecurityEvent {
        action: "authz_failure",
        outcome: "refused",
        user: Some(Pseudonymiser::new(state.config.rate_key.clone()).pseudo_id(&user.0)),
        request_id,
        amr: None,
        provider: None,
        method,
    });
}
