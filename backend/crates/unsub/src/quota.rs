//! The per-mailbox daily `mailto:` send quota (T-703, S7 section 6, ASVS
//! V2.3.2 and V2.4.1).
//!
//! The counter is a Firestore `rate_limits` document, so it is shared across
//! Cloud Run instances; a per-instance counter would cap nothing once more than
//! one instance is running (V2.4.1). `RateLimitRepo::hit` is a single atomic
//! increment, so concurrent jobs cannot both read the same value and slip past
//! the cap (V2.3.2).

use domain::MailboxId;
use obs::{security_event, Pseudonymiser, SecurityEvent};
use ports::store::RateLimitKey;
use ports::{Ports, SecretName};
use svc_common::SvcError;
use time::OffsetDateTime;

/// The per-mailbox daily `mailto:` cap (S7 section 6) `[TUNABLE]`.
pub const MAILTO_DAILY_LIMIT: u32 = 100;

/// One day in seconds; the quota window is a UTC day.
const DAY_SECONDS: i64 = 86_400;

/// Whether one more `mailto:` send is allowed today.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QuotaDecision {
    /// The send may proceed; the unit has already been spent.
    Allowed,
    /// The mailbox has reached its daily cap; nothing must be sent.
    Exceeded,
}

/// Spend one unit of `mailbox`'s UTC-day `mailto:` quota and report whether the
/// send is allowed.
///
/// The unit is spent even when the send later fails `[DEFAULT: conservative;
/// it protects the mailbox's sending reputation if something loops]`. A store
/// failure is reported so the caller retries rather than sending uncapped.
///
/// # Errors
///
/// Returns [`SvcError::Store`] when the counter cannot be incremented.
pub async fn take_mailto_quota(
    ports: &Ports,
    mailbox: &MailboxId,
    now: OffsetDateTime,
) -> Result<QuotaDecision, SvcError> {
    let count = ports
        .store
        .rate_limits()
        .hit(
            &RateLimitKey(format!("mailto:{mailbox}")),
            utc_day_start(now),
            time::Duration::days(1),
        )
        .await
        .map_err(|_| SvcError::Store)?;
    if count > MAILTO_DAILY_LIMIT {
        log_rate_limit_hit(ports, mailbox).await;
        return Ok(QuotaDecision::Exceeded);
    }
    Ok(QuotaDecision::Allowed)
}

/// Midnight UTC on `now`'s day: the start of the quota window.
fn utc_day_start(now: OffsetDateTime) -> OffsetDateTime {
    let seconds = now.unix_timestamp();
    let start = seconds - seconds.rem_euclid(DAY_SECONDS);
    OffsetDateTime::from_unix_timestamp(start).unwrap_or(now)
}

/// One `rate_limit_hit` security event when a mailbox is over its cap. The
/// owner appears only as a pseudonym, and only the scope (`mailto`) is named.
async fn log_rate_limit_hit(ports: &Ports, mailbox: &MailboxId) {
    let pseudo = match ports.store.mailboxes().get(mailbox).await {
        Ok(Some(versioned)) => pseudo_id(ports, &versioned.record.user_id.0).await,
        _ => None,
    };
    security_event(&SecurityEvent {
        action: "rate_limit_hit",
        outcome: "refused",
        user: pseudo,
        request_id: None,
        amr: None,
        provider: None,
        method: Some("mailto"),
    });
}

/// The HMAC pseudonym of a user ID, or `None` when the key is unavailable.
async fn pseudo_id(ports: &Ports, user: &uuid::Uuid) -> Option<obs::PseudoId> {
    match ports.secrets.get(SecretName::LogPseudonymHmacKey).await {
        Ok(key) => Some(Pseudonymiser::new(key).pseudo_id(user)),
        Err(_) => None,
    }
}
