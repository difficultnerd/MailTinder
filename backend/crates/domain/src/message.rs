//! Message metadata, labels, headers and unsubscribe targets.

use std::collections::BTreeSet;
use std::fmt;

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use url::Url;

use crate::error::DomainError;
use crate::ids::{MailboxId, MessageId};
use crate::sender::SenderKey;

/// Exact provider label IDs, opaque to the domain.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LabelSet(BTreeSet<String>);

impl LabelSet {
    pub fn new() -> Self {
        Self(BTreeSet::new())
    }

    pub fn from_ids<I: IntoIterator<Item = String>>(ids: I) -> Self {
        Self(ids.into_iter().collect())
    }

    pub fn contains(&self, id: &str) -> bool {
        self.0.contains(id)
    }

    pub fn insert(&mut self, id: String) -> bool {
        self.0.insert(id)
    }

    pub fn remove(&mut self, id: &str) -> bool {
        self.0.remove(id)
    }

    pub fn iter(&self) -> impl Iterator<Item = &String> {
        self.0.iter()
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn difference(&self, other: &LabelSet) -> LabelSet {
        LabelSet(self.0.difference(&other.0).cloned().collect())
    }
}

/// A `mailto:` target. Personal data; Debug redacted.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MailtoTarget {
    to: String,
    subject: Option<String>,
    body: Option<String>,
}

impl MailtoTarget {
    /// Refuses CR or LF in any field, an empty `to`, `to` without exactly one
    /// `@`, or `to` over 320 chars. Full RFC 6068 parsing and the cc/bcc
    /// refusal live in T-404; this is the last line of defence.
    pub fn new(to: &str, subject: Option<&str>, body: Option<&str>) -> Result<Self, DomainError> {
        if to.is_empty() || to.chars().count() > 320 || to.matches('@').count() != 1 {
            return Err(DomainError::InvalidMailto);
        }
        if to.contains(['\r', '\n']) {
            return Err(DomainError::InvalidMailto);
        }
        for opt in [subject, body].into_iter().flatten() {
            if opt.contains(['\r', '\n']) {
                return Err(DomainError::InvalidMailto);
            }
        }
        Ok(Self {
            to: to.to_owned(),
            subject: subject.map(str::to_owned),
            body: body.map(str::to_owned),
        })
    }

    pub fn to(&self) -> &str {
        &self.to
    }

    pub fn subject(&self) -> Option<&str> {
        self.subject.as_deref()
    }

    pub fn body(&self) -> Option<&str> {
        self.body.as_deref()
    }
}

impl fmt::Debug for MailtoTarget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("MailtoTarget([redacted])")
    }
}

/// Unsubscribe options, only those covered by a passing DKIM signature
/// (T-406). Debug shows presence only.
#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnsubscribeOptions {
    /// https URI with `List-Unsubscribe-Post: List-Unsubscribe=One-Click`, both covered.
    pub one_click_https: Option<Url>,
    /// Web link (https only; a plain http link is dropped, S6 6), covered.
    pub https: Option<Url>,
    /// Covered.
    pub mailto: Option<MailtoTarget>,
}

impl fmt::Debug for UnsubscribeOptions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("UnsubscribeOptions")
            .field("one_click", &self.one_click_https.is_some())
            .field("https", &self.https.is_some())
            .field("mailto", &self.mailto.is_some())
            .finish()
    }
}

/// Header facts used for classification. Debug prints booleans and presence only.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct HeaderFacts {
    /// DKIM-covered options only (T-406); None when none survive.
    pub list_unsubscribe: Option<UnsubscribeOptions>,
    /// A `List-Unsubscribe` header exists at all, covered or not.
    pub list_unsubscribe_present: bool,
    /// Normalised by the adapter (T-401 step 6).
    pub list_id: Option<String>,
    pub feedback_id: Option<String>,
    pub precedence_bulk: bool,
    pub auto_submitted: bool,
    /// DKIM-aligned From or provider auth pass.
    pub from_authenticated: bool,
    /// A key from `T-401`'s `ESP_HINTS`, badge reason only.
    pub esp_hint: Option<String>,
    pub is_reply_or_thread: bool,
    /// `Reply-To` domain differs from `From` domain.
    pub reply_to_mismatch: bool,
    /// Display name contains an address or domain other than the From domain.
    pub display_name_spoof: bool,
}

impl fmt::Debug for HeaderFacts {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HeaderFacts")
            .field("list_unsubscribe", &self.list_unsubscribe.is_some())
            .field("list_unsubscribe_present", &self.list_unsubscribe_present)
            .field("list_id", &self.list_id.is_some())
            .field("feedback_id", &self.feedback_id.is_some())
            .field("precedence_bulk", &self.precedence_bulk)
            .field("auto_submitted", &self.auto_submitted)
            .field("from_authenticated", &self.from_authenticated)
            .field("esp_hint", &self.esp_hint)
            .field("is_reply_or_thread", &self.is_reply_or_thread)
            .field("reply_to_mismatch", &self.reply_to_mismatch)
            .field("display_name_spoof", &self.display_name_spoof)
            .finish()
    }
}

/// What the Feed needs; no body. Debug redacts personal fields.
#[derive(Clone, PartialEq)]
pub struct MessageMeta {
    pub mailbox: MailboxId,
    pub id: MessageId,
    pub internal_date: OffsetDateTime,
    pub from_display: String,
    pub from_address: String,
    pub sender: SenderKey,
    pub subject: String,
    pub labels: LabelSet,
    pub facts: HeaderFacts,
}

impl fmt::Debug for MessageMeta {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MessageMeta")
            .field("mailbox", &self.mailbox)
            .field("id", &self.id)
            .field("internal_date", &self.internal_date)
            .field("from_display", &"[redacted]")
            .field("from_address", &"[redacted]")
            .field("sender", &self.sender)
            .field("subject", &"[redacted]")
            .field("labels", &self.labels)
            .field("facts", &self.facts)
            .finish()
    }
}
