//! Per-mailbox state for `FakeMailbox`: messages, labels and the sent log.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use domain::{HeaderFacts, MessageId};
use time::OffsetDateTime;

/// A seeded message.
#[derive(Clone, Debug)]
pub struct SeedMessage {
    pub from_display: String,
    /// example.com addresses only.
    pub from_address: String,
    pub subject: String,
    /// In order; used by fake-google, ignored by `FakeMailbox`.
    pub raw_headers: Vec<(String, String)>,
    /// `FakeMailbox` returns these as-is (adapters compute their own).
    pub facts: HeaderFacts,
    /// Plain text.
    pub preview_text: String,
    pub internal_date: OffsetDateTime,
    /// e.g. `["INBOX", "UNREAD"]`.
    pub labels: Vec<String>,
}

/// A stored message.
#[derive(Clone, Debug)]
pub struct StoredMessage {
    pub seed: SeedMessage,
    pub labels: BTreeSet<String>,
}

/// The state of one mailbox.
#[derive(Default)]
pub struct MailboxState {
    pub messages: HashMap<MessageId, StoredMessage>,
    pub next_id: u64,
    pub user_label_counter: u64,
    /// User label name (lower-cased) -> label ID.
    pub user_labels: BTreeMap<String, String>,
    /// User label ID -> name.
    pub user_label_names: BTreeMap<String, String>,
    pub sent: Vec<super::SentRecord>,
}

/// Find a user label ID by name, case-insensitive.
pub fn find_user_label_id(state: &MailboxState, name: &str) -> Option<String> {
    state.user_labels.get(&name.to_lowercase()).cloned()
}

/// Record a new user label, returning its ID.
pub fn add_user_label(state: &mut MailboxState, name: &str) -> String {
    state.user_label_counter += 1;
    let id = format!("Label_{}", state.user_label_counter);
    state.user_labels.insert(name.to_lowercase(), id.clone());
    state.user_label_names.insert(id.clone(), name.to_owned());
    id
}
