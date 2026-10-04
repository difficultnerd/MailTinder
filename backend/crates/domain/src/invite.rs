//! Invite state machine (S3). An invite is pending until it is used, revoked
//! or expires after `INVITE_TTL`; re-sending replaces the token hash so the old
//! token stops working (AU-01 AC2). Redemption needs both a valid token and a
//! matching verified email (AU-03 AC6).

use std::fmt;

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::error::DomainError;
use crate::tunables::Tunables;

/// Identifier of an invite.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct InviteId(pub Uuid);

/// Identifier of an invite request.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct InviteRequestId(pub Uuid);

/// Lifecycle status of an invite.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InviteStatus {
    Pending,
    Used,
    Revoked,
    Expired,
}

/// The current state of an invite. Holds hashes only, never a raw token or
/// email address; hashing and HMAC live in the api. `Debug` prints only the
/// status and times, so a hash never lands in a log.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct InviteState {
    pub status: InviteStatus,
    /// SHA-256 of the current invite token.
    pub token_hash: [u8; 32],
    /// Keyed hash of the invited address (HMAC outside domain).
    pub email_hash: [u8; 32],
    pub last_sent_at: OffsetDateTime,
    /// `last_sent_at + INVITE_TTL`.
    pub expires_at: OffsetDateTime,
}

/// A transition applied to an invite.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InviteEvent {
    Resend {
        new_token_hash: [u8; 32],
        now: OffsetDateTime,
    },
    Revoke,
    Use {
        now: OffsetDateTime,
    },
    Expire {
        now: OffsetDateTime,
    },
}

impl InviteState {
    /// A new invite is `Pending`, sent now, expiring `INVITE_TTL` later
    /// (AU-01 AC1).
    pub fn new_pending(
        token_hash: [u8; 32],
        email_hash: [u8; 32],
        now: OffsetDateTime,
        t: &Tunables,
    ) -> Self {
        Self {
            status: InviteStatus::Pending,
            token_hash,
            email_hash,
            last_sent_at: now,
            expires_at: now + t.invite_ttl,
        }
    }

    /// Applies a transition, returning the new state or `TransitionNotAllowed`.
    pub fn apply(self, event: InviteEvent, t: &Tunables) -> Result<InviteState, DomainError> {
        match event {
            InviteEvent::Resend {
                new_token_hash,
                now,
            } => match self.status {
                InviteStatus::Pending | InviteStatus::Expired => Ok(InviteState {
                    status: InviteStatus::Pending,
                    token_hash: new_token_hash,
                    email_hash: self.email_hash,
                    last_sent_at: now,
                    expires_at: now + t.invite_ttl,
                }),
                InviteStatus::Used | InviteStatus::Revoked => {
                    Err(DomainError::TransitionNotAllowed)
                }
            },
            InviteEvent::Revoke => match self.status {
                InviteStatus::Pending | InviteStatus::Expired => Ok(InviteState {
                    status: InviteStatus::Revoked,
                    ..self
                }),
                InviteStatus::Used | InviteStatus::Revoked => {
                    Err(DomainError::TransitionNotAllowed)
                }
            },
            InviteEvent::Use { now } => match self.status {
                InviteStatus::Pending if now <= self.expires_at => Ok(InviteState {
                    status: InviteStatus::Used,
                    ..self
                }),
                _ => Err(DomainError::TransitionNotAllowed),
            },
            InviteEvent::Expire { now } => match self.status {
                InviteStatus::Pending if now > self.expires_at => Ok(InviteState {
                    status: InviteStatus::Expired,
                    ..self
                }),
                _ => Err(DomainError::TransitionNotAllowed),
            },
        }
    }
}

impl fmt::Debug for InviteState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("InviteState")
            .field("status", &self.status)
            .field("last_sent_at", &self.last_sent_at)
            .field("expires_at", &self.expires_at)
            .finish()
    }
}

/// Why a redemption attempt was refused. Maps to the S7 3.4 outcomes
/// `email_unverified`, `invite_invalid` and `email_mismatch`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RedeemRefusal {
    EmailUnverified,
    InviteInvalid,
    EmailMismatch,
}

/// Checks a redemption attempt. The provider's verification is checked first,
/// then the token, then the email, so an unverified account learns nothing
/// about the invite (AU-03 AC3, AC6, AC2). On `Ok(())` the caller applies
/// `Use` in the same storage transaction.
pub fn check_redemption(
    invite: Option<&InviteState>,
    presented_token_hash: Option<&[u8; 32]>,
    verified_email_hash: &[u8; 32],
    email_verified: bool,
    now: OffsetDateTime,
) -> Result<(), RedeemRefusal> {
    if !email_verified {
        return Err(RedeemRefusal::EmailUnverified);
    }
    let invite = match invite {
        Some(invite) if invite.status == InviteStatus::Pending && now <= invite.expires_at => {
            invite
        }
        _ => return Err(RedeemRefusal::InviteInvalid),
    };
    let presented = match presented_token_hash {
        Some(hash) => hash,
        None => return Err(RedeemRefusal::InviteInvalid),
    };
    if invite.token_hash != *presented {
        return Err(RedeemRefusal::InviteInvalid);
    }
    if invite.email_hash != *verified_email_hash {
        return Err(RedeemRefusal::EmailMismatch);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tunables::Tunables;
    use proptest::prelude::*;
    use time::Duration;

    fn now() -> OffsetDateTime {
        OffsetDateTime::from_unix_timestamp(1_700_000_000).expect("valid timestamp")
    }

    fn hash(byte: u8) -> [u8; 32] {
        [byte; 32]
    }

    fn pending(t: &Tunables) -> InviteState {
        InviteState::new_pending(hash(1), hash(2), now(), t)
    }

    #[test]
    fn au_01_ac1_new_invite_expires_after_seven_days() {
        let t = Tunables::default();
        let invite = pending(&t);
        assert_eq!(invite.status, InviteStatus::Pending);
        assert_eq!(invite.last_sent_at, now());
        assert_eq!(invite.expires_at, now() + Duration::days(7));
    }

    #[test]
    fn au_01_ac2_resend_voids_old_token() {
        let t = Tunables::default();
        let invite = pending(&t);
        let resent = invite
            .apply(
                InviteEvent::Resend {
                    new_token_hash: hash(9),
                    now: now() + Duration::days(1),
                },
                &t,
            )
            .expect("resend from pending");
        assert_eq!(resent.status, InviteStatus::Pending);
        assert_eq!(resent.token_hash, hash(9));
        assert_eq!(resent.last_sent_at, now() + Duration::days(1));
        assert_eq!(
            resent.expires_at,
            now() + Duration::days(1) + Duration::days(7)
        );

        // Old token refused, new token passes.
        assert_eq!(
            check_redemption(
                Some(&resent),
                Some(&hash(1)),
                &hash(2),
                true,
                now() + Duration::days(1),
            ),
            Err(RedeemRefusal::InviteInvalid)
        );
        assert_eq!(
            check_redemption(
                Some(&resent),
                Some(&hash(9)),
                &hash(2),
                true,
                now() + Duration::days(1),
            ),
            Ok(())
        );
    }

    #[test]
    fn au_01_ac4_revoked_invite_refused() {
        let t = Tunables::default();
        let invite = pending(&t)
            .apply(InviteEvent::Revoke, &t)
            .expect("revoke from pending");
        assert_eq!(invite.status, InviteStatus::Revoked);
        assert_eq!(
            check_redemption(Some(&invite), Some(&hash(1)), &hash(2), true, now()),
            Err(RedeemRefusal::InviteInvalid)
        );
    }

    #[test]
    fn au_03_ac2_email_mismatch_refused() {
        let t = Tunables::default();
        let invite = pending(&t);
        assert_eq!(
            check_redemption(Some(&invite), Some(&hash(1)), &hash(99), true, now()),
            Err(RedeemRefusal::EmailMismatch)
        );
    }

    #[test]
    fn au_03_ac3_unverified_email_refused() {
        let t = Tunables::default();
        let invite = pending(&t);
        assert_eq!(
            check_redemption(Some(&invite), Some(&hash(1)), &hash(2), false, now()),
            Err(RedeemRefusal::EmailUnverified)
        );
    }

    #[test]
    fn au_03_ac6_missing_token_refused_even_if_email_matches() {
        let t = Tunables::default();
        let invite = pending(&t);
        assert_eq!(
            check_redemption(Some(&invite), None, &hash(2), true, now()),
            Err(RedeemRefusal::InviteInvalid)
        );
        assert_eq!(
            check_redemption(None, Some(&hash(1)), &hash(2), true, now()),
            Err(RedeemRefusal::InviteInvalid)
        );
    }

    proptest! {
        #[test]
        fn au_03_ac6_used_expired_revoked_or_replaced_refused(
            status in prop_oneof![
                Just(InviteStatus::Used),
                Just(InviteStatus::Revoked),
                Just(InviteStatus::Expired),
            ],
            past_expiry in proptest::bool::ANY,
            different_hash in proptest::bool::ANY,
        ) {
            let t = Tunables::default();
            let mut invite = pending(&t);
            invite.status = status;
            if past_expiry {
                invite.expires_at = now() - Duration::seconds(1);
            }
            if different_hash {
                invite.token_hash = hash(7);
            }
            let presented = hash(1);
            let result = check_redemption(Some(&invite), Some(&presented), &hash(2), true, now());
            prop_assert_eq!(result, Err(RedeemRefusal::InviteInvalid));
        }
    }

    #[test]
    fn invite_used_cannot_be_resent_or_revoked() {
        let t = Tunables::default();
        let used = pending(&t)
            .apply(InviteEvent::Use { now: now() }, &t)
            .expect("use from pending");
        assert_eq!(used.status, InviteStatus::Used);
        assert_eq!(
            used.apply(
                InviteEvent::Resend {
                    new_token_hash: hash(9),
                    now: now(),
                },
                &t,
            ),
            Err(DomainError::TransitionNotAllowed)
        );
        assert_eq!(
            used.apply(InviteEvent::Revoke, &t),
            Err(DomainError::TransitionNotAllowed)
        );
    }

    #[test]
    fn invite_redemption_at_exact_expiry_allowed() {
        let t = Tunables::default();
        let invite = pending(&t);
        // `now == expires_at` passes.
        assert_eq!(
            check_redemption(
                Some(&invite),
                Some(&hash(1)),
                &hash(2),
                true,
                invite.expires_at
            ),
            Ok(())
        );
        // One second later refused.
        assert_eq!(
            check_redemption(
                Some(&invite),
                Some(&hash(1)),
                &hash(2),
                true,
                invite.expires_at + Duration::seconds(1),
            ),
            Err(RedeemRefusal::InviteInvalid)
        );
    }
}
