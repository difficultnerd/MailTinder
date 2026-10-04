//! Mailbox identity and provider types.

use std::fmt;

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use crate::error::DomainError;
use crate::ids::{MailboxId, UserId};

/// The mail provider. Graph is added in v2; never match with `_` so v2 forces
/// every match to be reviewed (XC-02).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Provider {
    Gmail,
}

/// The provider's subject identifier (Google `sub`). Personal data; Debug redacted.
#[derive(Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ProviderSubjectId(String);

impl ProviderSubjectId {
    /// Accepts 1 to 255 characters with no control characters.
    pub fn new(raw: impl Into<String>) -> Result<Self, DomainError> {
        let raw = raw.into();
        if raw.is_empty() || raw.chars().count() > 255 || raw.chars().any(char::is_control) {
            return Err(DomainError::InvalidId);
        }
        Ok(Self(raw))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for ProviderSubjectId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ProviderSubjectId([redacted])")
    }
}

/// A mailbox identity is provider plus provider subject ID, never an email
/// address (AU-03 AC7).
// nosemgrep: privacy-rust-derive-debug-on-sensitive-struct -- subject is an opaque ProviderSubjectId, not raw PII
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct MailboxIdentity {
    pub provider: Provider,
    pub subject: ProviderSubjectId,
}

/// Connection status of a mailbox. Transitions are T-107; `ConsentBlocked` is v2 only.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MailboxStatus {
    Connected,
    NeedsSignIn,
    ConsentBlocked,
}

/// A mailbox owned by exactly one user (INV-3).
// nosemgrep: privacy-rust-derive-debug-on-sensitive-struct -- fields are opaque IDs (UserId, MailboxId, ProviderSubjectId)
#[derive(Clone, Debug, PartialEq)]
pub struct Mailbox {
    pub id: MailboxId,
    pub user: UserId,
    pub identity: MailboxIdentity,
    pub status: MailboxStatus,
    pub linked_at: OffsetDateTime,
    pub is_primary: bool,
}

impl Mailbox {
    /// INV-3: a mailbox has exactly one owning user.
    pub fn owned_by(&self, user: &UserId) -> bool {
        &self.user == user
    }
}
