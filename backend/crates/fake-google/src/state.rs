//! Shared state for `fake-google`: mailboxes, messages, labels, tokens, events
//! and failure rules. All synthetic; no real data.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use time::OffsetDateTime;

use super::tokens::TokenRecord;

/// A mailbox keyed by its email address (reserved domains only).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct FakeMailboxKey(pub String);

/// A stored message.
#[derive(Clone, Debug)]
pub struct StoredMessage {
    pub id: String,
    /// The raw RFC 5322 bytes.
    pub raw: Vec<u8>,
    pub labels: BTreeSet<String>,
    pub internal_date: OffsetDateTime,
}

/// A label.
#[derive(Clone, Debug)]
pub struct Label {
    pub id: String,
    pub name: String,
    pub kind: String, // "system" or "user"
}

/// One mailbox's state.
#[derive(Default)]
pub struct MailboxState {
    pub messages: BTreeMap<String, StoredMessage>,
    pub labels: BTreeMap<String, Label>, // id -> label
    pub next_id: u64,
    pub next_user_label: u64,
    pub sent: Vec<Vec<u8>>,
}

/// A recorded event.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FakeEvent {
    Request {
        method: String,
        route: String,
        query: Vec<(String, String)>,
    },
    PermanentDeleteAttempted {
        method: String,
        path: String,
    },
}

/// A failure injection rule.
#[derive(Clone, Debug)]
pub struct FailRule {
    pub method: String,
    pub path_prefix: String,
    pub status: u16,
    pub reason: String,
    pub retry_after: Option<String>,
    pub times: u32,
}

/// The whole fake's state.
#[derive(Default)]
pub struct FakeState {
    pub mailboxes: BTreeMap<String, MailboxState>,
    pub tokens: HashMap<String, TokenRecord>,
    pub events: Vec<FakeEvent>,
    pub fail_rules: Vec<FailRule>,
    /// When set, the next `POST /labels` creates the label but still answers 409.
    pub label_create_race: BTreeSet<String>,
}

impl FakeState {
    /// The system labels, in a stable order.
    pub fn system_label_ids() -> &'static [&'static str] {
        &[
            "INBOX",
            "SENT",
            "TRASH",
            "SPAM",
            "UNREAD",
            "STARRED",
            "IMPORTANT",
            "DRAFT",
            "CATEGORY_PERSONAL",
        ]
    }

    /// Ensure a mailbox exists, seeding its system labels.
    pub fn mailbox(&mut self, email: &str) -> &mut MailboxState {
        self.mailboxes.entry(email.to_owned()).or_insert_with(|| {
            let mut mb = MailboxState::default();
            for id in Self::system_label_ids() {
                mb.labels.insert(
                    (*id).to_owned(),
                    Label {
                        id: (*id).to_owned(),
                        name: (*id).to_owned(),
                        kind: "system".to_owned(),
                    },
                );
            }
            mb
        })
    }

    /// Allocate the next message ID (16 lower-case hex chars, Gmail-like).
    pub fn next_message_id(&mut self, email: &str) -> String {
        let mb = self.mailboxes.entry(email.to_owned()).or_default();
        mb.next_id += 1;
        format!("{:016x}", mb.next_id)
    }

    /// Allocate the next user label ID.
    pub fn next_user_label_id(&mut self, email: &str) -> String {
        let mb = self.mailboxes.entry(email.to_owned()).or_default();
        mb.next_user_label += 1;
        format!("Label_{}", mb.next_user_label)
    }
}
