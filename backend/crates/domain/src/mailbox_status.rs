//! Mailbox status state machine (S3). `MailboxStatus` is T-101's; this module
//! owns its transitions. `ConsentBlocked` is v2 Microsoft only; nothing in v1
//! sends it. "Removed" is not a status: disconnecting deletes the record.
//!
//! Every pair is enumerated (no `_` on `MailboxStatus`) so a new variant
//! forces a review of each transition.

use crate::mailbox::MailboxStatus;

/// An event that can change a mailbox's status.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MailboxEvent {
    TokenInvalid,
    OAuthSucceeded,
    ConsentBlocked,
}

/// The next status after `event` from `current`. Every pair not listed in S3
/// returns `current` unchanged.
pub fn next_status(current: MailboxStatus, event: MailboxEvent) -> MailboxStatus {
    match (current, event) {
        (MailboxStatus::Connected, MailboxEvent::TokenInvalid) => MailboxStatus::NeedsSignIn,
        (MailboxStatus::NeedsSignIn, MailboxEvent::OAuthSucceeded) => MailboxStatus::Connected,
        (MailboxStatus::ConsentBlocked, MailboxEvent::OAuthSucceeded) => MailboxStatus::Connected,
        (MailboxStatus::Connected, MailboxEvent::ConsentBlocked) => MailboxStatus::ConsentBlocked,
        (MailboxStatus::NeedsSignIn, MailboxEvent::ConsentBlocked) => MailboxStatus::ConsentBlocked,
        (MailboxStatus::ConsentBlocked, MailboxEvent::ConsentBlocked) => {
            MailboxStatus::ConsentBlocked
        }
        (MailboxStatus::Connected, MailboxEvent::OAuthSucceeded) => MailboxStatus::Connected,
        (MailboxStatus::NeedsSignIn, MailboxEvent::TokenInvalid) => MailboxStatus::NeedsSignIn,
        (MailboxStatus::ConsentBlocked, MailboxEvent::TokenInvalid) => {
            MailboxStatus::ConsentBlocked
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn un_01_ac6_token_invalid_sets_needs_sign_in() {
        assert_eq!(
            next_status(MailboxStatus::Connected, MailboxEvent::TokenInvalid),
            MailboxStatus::NeedsSignIn
        );
    }

    #[test]
    fn mailbox_status_oauth_success_reconnects() {
        assert_eq!(
            next_status(MailboxStatus::NeedsSignIn, MailboxEvent::OAuthSucceeded),
            MailboxStatus::Connected
        );
        assert_eq!(
            next_status(MailboxStatus::ConsentBlocked, MailboxEvent::OAuthSucceeded),
            MailboxStatus::Connected
        );
    }

    #[test]
    fn st_03_ac1_mailbox_status_values() {
        assert_eq!(
            serde_json::to_string(&MailboxStatus::Connected).expect("serialise"),
            "\"connected\""
        );
        assert_eq!(
            serde_json::to_string(&MailboxStatus::NeedsSignIn).expect("serialise"),
            "\"needs_sign_in\""
        );
        assert_eq!(
            serde_json::to_string(&MailboxStatus::ConsentBlocked).expect("serialise"),
            "\"consent_blocked\""
        );
    }
}
