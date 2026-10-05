//! `MailSeeder` implementation for `fake-google` (T-205a).

use std::sync::Arc;

use async_trait::async_trait;
use domain::{LabelSet, MailboxId, MessageId};
use testkit::contract::mail_provider::MailSeeder;
use testkit::mailbox::state::SeedMessage;
use testkit::mailbox::SentRecord;

use super::state::{FakeMailboxKey, FakeState};

/// A `MailSeeder` bound to one mailbox.
pub struct FakeGoogleSeeder {
    state: Arc<std::sync::Mutex<FakeState>>,
    mailbox: FakeMailboxKey,
}

impl FakeGoogleSeeder {
    pub fn new(state: Arc<std::sync::Mutex<FakeState>>, mailbox: FakeMailboxKey) -> Self {
        Self { state, mailbox }
    }
}

#[async_trait]
impl MailSeeder for FakeGoogleSeeder {
    async fn seed(&self, msg: &SeedMessage) -> Result<MessageId, String> {
        let mut st = self.state.lock().unwrap();
        let id = st.next_message_id(&self.mailbox.0);
        let mb = st
            .mailboxes
            .get_mut(&self.mailbox.0)
            .ok_or_else(|| "mailbox missing".to_owned())?;
        mb.messages.insert(
            id.clone(),
            super::state::StoredMessage {
                id: id.clone(),
                raw: seed_to_eml(msg),
                labels: msg.labels.iter().cloned().collect(),
                internal_date: msg.internal_date,
            },
        );
        MessageId::new(&id).map_err(|_| "bad id".to_owned())
    }

    async fn labels_of(&self, id: &MessageId) -> Result<LabelSet, String> {
        let st = self.state.lock().unwrap();
        let mb = st
            .mailboxes
            .get(&self.mailbox.0)
            .ok_or_else(|| "mailbox missing".to_owned())?;
        let m = mb
            .messages
            .get(id.as_str())
            .ok_or_else(|| "message missing".to_owned())?;
        Ok(LabelSet::from_ids(m.labels.iter().cloned()))
    }

    async fn sent(&self) -> Result<Vec<SentRecord>, String> {
        let st = self.state.lock().unwrap();
        let mb = st
            .mailboxes
            .get(&self.mailbox.0)
            .ok_or_else(|| "mailbox missing".to_owned())?;
        Ok(mb
            .sent
            .iter()
            .map(|raw| SentRecord {
                mailbox: MailboxId(uuid::Uuid::from_u128(0)),
                to: header_value(raw, "To").unwrap_or_default(),
                subject: header_value(raw, "Subject"),
                body: None,
            })
            .collect())
    }

    async fn permanent_delete_attempts(&self) -> Result<u64, String> {
        let st = self.state.lock().unwrap();
        Ok(st
            .events
            .iter()
            .filter(|e| matches!(e, super::state::FakeEvent::PermanentDeleteAttempted { .. }))
            .count() as u64)
    }
}

/// The value of the first `name` header in a raw message, trimmed.
fn header_value(raw: &[u8], name: &str) -> Option<String> {
    let text = String::from_utf8_lossy(raw);
    for line in text.split("\r\n") {
        if line.is_empty() {
            break;
        }
        if let Some((key, value)) = line.split_once(':') {
            if key.eq_ignore_ascii_case(name) {
                return Some(value.trim().to_owned());
            }
        }
    }
    None
}

/// Render a minimal `.eml` from the seed's headers and preview text.
fn seed_to_eml(msg: &SeedMessage) -> Vec<u8> {
    let mut s = String::new();
    s.push_str("From: ");
    s.push_str(&msg.from_display);
    s.push_str(" <");
    s.push_str(&msg.from_address);
    s.push_str(">\r\n");
    s.push_str("Subject: ");
    s.push_str(&msg.subject);
    s.push_str("\r\n");
    for (k, v) in &msg.raw_headers {
        s.push_str(k);
        s.push_str(": ");
        s.push_str(v);
        s.push_str("\r\n");
    }
    s.push_str("\r\n");
    s.push_str(&msg.preview_text);
    s.into_bytes()
}
