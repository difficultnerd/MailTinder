//! Typed identifiers shared across the domain.

use std::fmt;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::DomainError;

macro_rules! uuid_id {
    ($name:ident) => {
        #[derive(
            Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize,
        )]
        #[serde(transparent)]
        pub struct $name(pub Uuid);

        impl $name {
            pub fn new(uuid: Uuid) -> Self {
                Self(uuid)
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(f)
            }
        }
    };
}

uuid_id!(UserId);
uuid_id!(MailboxId);
uuid_id!(JobId);
uuid_id!(RuleId);
uuid_id!(CategoryId);

/// A provider message ID. Opaque to the domain; never logged (S5).
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct MessageId(String);

impl MessageId {
    /// Accepts 1 to 256 characters with no control characters.
    pub fn new(raw: impl Into<String>) -> Result<Self, DomainError> {
        let raw = raw.into();
        if raw.is_empty() || raw.chars().count() > 256 || raw.chars().any(char::is_control) {
            return Err(DomainError::InvalidId);
        }
        Ok(Self(raw))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for MessageId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("MessageId([redacted])")
    }
}

/// A classifier identifier, for example `header_rules@1` or `gemini@flash-lite`.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ClassifierId(String);

impl ClassifierId {
    pub fn new(raw: &str) -> Result<Self, DomainError> {
        if raw.is_empty() || raw.chars().count() > 128 || raw.chars().any(char::is_control) {
            return Err(DomainError::InvalidId);
        }
        Ok(Self(raw.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ClassifierId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// The header-rules classifier identifier.
pub const HEADER_RULES_ID: &str = "header_rules@1";
