//! Needs Attention item state machine (S3). An item is open until it is
//! resolved, dismissed or expires after `NEEDS_ATTENTION_TTL`; every exit
//! deletes the record, so there is no stored status (S3, NA-01 AC2).

use std::fmt;

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use url::Url;
use uuid::Uuid;

use crate::tunables::Tunables;

/// Identifier of a Needs Attention item.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct NeedsAttentionId(pub Uuid);

/// Why an item was raised. `reason_code` in S7 5.8; `SignInRequired` is the
/// v1 code for UN-01 AC6 (S7 5.8 lists no code for it; see the task's edge
/// cases). Reserved v2 reasons (`captcha`, `login_required`, `page_unclear`,
/// `page_failed`) are not added in v1.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NeedsAttentionReason {
    HttpsOnlyUnsubscribe,
    OneClickRedirect,
    OneClickAddressRefused,
    UnsubscribeFailed,
    UnsubscribeIgnored,
    JobExpired,
    SignInRequired,
}

/// A new Needs Attention item. `Debug` prints only the reason and whether a
/// link is present, so a URL never lands in a log (S5).
#[derive(Clone, PartialEq, Eq)]
pub struct NewNeedsAttention {
    pub reason: NeedsAttentionReason,
    pub link: Option<Url>,
    pub created_at: OffsetDateTime,
    pub expires_at: OffsetDateTime,
}

impl NewNeedsAttention {
    /// Drops any link whose scheme is not `https` (S7 5.8). `expires_at` is
    /// `created_at + NEEDS_ATTENTION_TTL` (INV-2).
    pub fn new(
        reason: NeedsAttentionReason,
        link: Option<Url>,
        now: OffsetDateTime,
        t: &Tunables,
    ) -> Self {
        let link = link.filter(|l| l.scheme() == "https");
        Self {
            reason,
            link,
            created_at: now,
            expires_at: now + t.needs_attention_ttl,
        }
    }
}

// The spec mandates that Debug prints only the reason and link presence, so
// the timestamps are deliberately omitted.
#[allow(clippy::missing_fields_in_debug)]
impl fmt::Debug for NewNeedsAttention {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NewNeedsAttention")
            .field("reason", &self.reason)
            .field("has_link", &self.link.is_some())
            .finish()
    }
}

/// How an item leaves the open state. Every exit deletes the record (S3), so
/// this is for logging and History only.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NeedsAttentionExit {
    Resolved,
    Dismissed,
    Expired,
}

/// An item has expired once `now` is strictly past `expires_at` (NA-01 AC2).
pub fn needs_attention_expired(expires_at: OffsetDateTime, now: OffsetDateTime) -> bool {
    now > expires_at
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tunables::Tunables;
    use time::Duration;

    fn now() -> OffsetDateTime {
        OffsetDateTime::from_unix_timestamp(1_700_000_000)
            .unwrap_or_else(|_| panic!("valid timestamp"))
    }

    fn https_link() -> Url {
        Url::parse("https://example.com/unsub").unwrap_or_else(|_| panic!("valid url"))
    }

    #[test]
    fn un_05_ac1_item_keeps_https_link_only() -> Result<(), Box<dyn std::error::Error>> {
        let t = Tunables::default();
        for scheme in ["http", "javascript", "data"] {
            let link = Url::parse(&format!("{scheme}://example.com/x"))?;
            let item = NewNeedsAttention::new(
                NeedsAttentionReason::HttpsOnlyUnsubscribe,
                Some(link),
                now(),
                &t,
            );
            assert_eq!(item.link, None, "scheme {scheme} must be dropped");
        }
        let item = NewNeedsAttention::new(
            NeedsAttentionReason::HttpsOnlyUnsubscribe,
            Some(https_link()),
            now(),
            &t,
        );
        assert_eq!(item.link, Some(https_link()));
        assert_eq!(item.reason, NeedsAttentionReason::HttpsOnlyUnsubscribe);
        Ok(())
    }

    #[test]
    fn na_01_ac2_item_expires_after_30_days() {
        let t = Tunables::default();
        let item = NewNeedsAttention::new(NeedsAttentionReason::JobExpired, None, now(), &t);
        let expected = now() + Duration::days(30);
        assert_eq!(item.expires_at, expected);
        // Not yet expired at the boundary.
        assert!(!needs_attention_expired(item.expires_at, item.expires_at));
        // Expired one second later.
        assert!(needs_attention_expired(
            item.expires_at,
            item.expires_at + Duration::seconds(1)
        ));
    }

    #[test]
    fn inv_2_needs_attention_item_has_expires_at() {
        let t = Tunables::default();
        let item = NewNeedsAttention::new(NeedsAttentionReason::UnsubscribeFailed, None, now(), &t);
        assert!(item.expires_at > item.created_at);
    }

    #[test]
    fn un_01_ac6_sign_in_required_reason_serialises() -> Result<(), Box<dyn std::error::Error>> {
        let json = serde_json::to_string(&NeedsAttentionReason::SignInRequired)?;
        assert_eq!(json, "\"sign_in_required\"");
        Ok(())
    }
}
