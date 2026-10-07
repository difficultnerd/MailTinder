//! An in-memory `MailProvider` that models Gmail's label behaviour.

pub mod state;

use std::collections::{BTreeSet, HashMap, VecDeque};
use std::sync::Mutex;

use async_trait::async_trait;
use domain::{LabelSet, MailboxId, MailtoTarget, MessageId, MessageMeta, Provider, SenderKey};
use ports::{
    ListOrder, MailError, MailProvider, MailboxCtx, MessagePage, MessageQuery, PageToken,
    ProviderCapabilities,
};

use self::state::{add_user_label, find_user_label_id, MailboxState, SeedMessage};

/// The system label IDs Gmail uses.
pub const SYS_INBOX: &str = "INBOX";
pub const SYS_TRASH: &str = "TRASH";
pub const SYS_SPAM: &str = "SPAM";
pub const SYS_SENT: &str = "SENT";
pub const SYS_UNREAD: &str = "UNREAD";

/// Labels that `set_labels` refuses (T-403).
const REFUSED_LABELS: [&str; 5] = ["TRASH", "SPAM", "SENT", "DRAFT", "CHAT"];

/// The user label the Gmail adapter applies to a sent `mailto:` message
/// (UN-03 AC2). Modelled here so a service test sees the same sent-mailbox
/// behaviour the real adapter produces.
pub const MAIL_TINDER_LABEL: &str = "Mail Tinder";

/// A record of a sent mailto message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SentRecord {
    pub mailbox: MailboxId,
    pub to: String,
    pub subject: Option<String>,
    pub body: Option<String>,
}

/// The fake's page size.
pub const LIST_PAGE_SIZE: usize = 20;

/// A mailbox operation, used to script failures and count calls.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MailOp {
    ListInbox,
    GetMeta,
    GetPreview,
    SetLabels,
    Trash,
    ReportSpam,
    RestoreLabels,
    EnsureLabel,
    SendMailto,
    InboxCount,
    ListMessages,
    CountMessages,
    RenameLabel,
    RemoveLabel,
}

/// An in-memory `MailProvider`.
pub struct FakeMailbox {
    mailboxes: Mutex<HashMap<MailboxId, MailboxState>>,
    failures: Mutex<HashMap<MailOp, VecDeque<MailError>>>,
    calls: Mutex<HashMap<MailOp, u64>>,
    revoked: Mutex<Vec<String>>,
}

impl FakeMailbox {
    pub fn new() -> Self {
        Self {
            mailboxes: Mutex::new(HashMap::new()),
            failures: Mutex::new(HashMap::new()),
            calls: Mutex::new(HashMap::new()),
            revoked: Mutex::new(Vec::new()),
        }
    }

    fn record_call(&self, op: MailOp) {
        let mut calls = self
            .calls
            .lock()
            .unwrap_or_else(|_| panic!("mailbox poisoned"));
        *calls.entry(op).or_insert(0) += 1;
    }

    fn check(&self, op: MailOp, token: &str) -> Result<(), MailError> {
        self.record_call(op);
        if self
            .revoked
            .lock()
            .unwrap_or_else(|_| panic!("mailbox poisoned"))
            .iter()
            .any(|t| t == token)
        {
            return Err(MailError::Unauthorized);
        }
        let mut failures = self
            .failures
            .lock()
            .unwrap_or_else(|_| panic!("mailbox poisoned"));
        if let Some(q) = failures.get_mut(&op) {
            if let Some(err) = q.pop_front() {
                return Err(err);
            }
        }
        Ok(())
    }

    /// Seed a message; returns its ID (`m0001`, `m0002`, ...).
    pub fn seed(&self, mb: &MailboxId, msg: SeedMessage) -> MessageId {
        let mut map = self
            .mailboxes
            .lock()
            .unwrap_or_else(|_| panic!("mailbox poisoned"));
        let state = map.entry(*mb).or_default();
        state.next_id += 1;
        let id = MessageId::new(format!("m{:04}", state.next_id)).unwrap_or_else(|_| panic!("id"));
        let labels: BTreeSet<String> = msg.labels.iter().cloned().collect();
        state
            .messages
            .insert(id.clone(), state::StoredMessage { seed: msg, labels });
        id
    }

    /// Read back the current labels of a message.
    pub fn labels_of(&self, mb: &MailboxId, id: &MessageId) -> Option<LabelSet> {
        let map = self
            .mailboxes
            .lock()
            .unwrap_or_else(|_| panic!("mailbox poisoned"));
        map.get(mb)
            .and_then(|s| s.messages.get(id))
            .map(|m| LabelSet::from_ids(m.labels.iter().cloned().collect::<Vec<String>>()))
    }

    /// The sent log.
    pub fn sent(&self) -> Vec<SentRecord> {
        let map = self
            .mailboxes
            .lock()
            .unwrap_or_else(|_| panic!("mailbox poisoned"));
        map.values().flat_map(|s| s.sent.clone()).collect()
    }

    pub fn fail_next(&self, op: MailOp, err: MailError) {
        self.failures
            .lock()
            .unwrap_or_else(|_| panic!("mailbox poisoned"))
            .entry(op)
            .or_default()
            .push_back(err);
    }

    pub fn calls(&self, op: MailOp) -> u64 {
        *self
            .calls
            .lock()
            .unwrap_or_else(|_| panic!("mailbox poisoned"))
            .get(&op)
            .unwrap_or(&0)
    }

    pub fn revoke_token(&self, token: &str) {
        self.revoked
            .lock()
            .unwrap_or_else(|_| panic!("mailbox poisoned"))
            .push(token.to_owned());
    }

    fn to_meta(state: &MailboxState, mb: &MailboxId, id: &MessageId) -> MessageMeta {
        let m = &state.messages[id];
        MessageMeta {
            mailbox: *mb,
            id: id.clone(),
            internal_date: m.seed.internal_date,
            from_display: m.seed.from_display.clone(),
            from_address: m.seed.from_address.clone(),
            sender: SenderKey::from_address(&m.seed.from_address),
            subject: m.seed.subject.clone(),
            labels: LabelSet::from_ids(m.labels.iter().cloned().collect::<Vec<String>>()),
            facts: m.seed.facts.clone(),
        }
    }
}

impl Default for FakeMailbox {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl MailProvider for FakeMailbox {
    fn provider(&self) -> Provider {
        Provider::Gmail
    }
    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            labels_are_sets: true,
            spam_is_label: true,
        }
    }

    async fn list_inbox(
        &self,
        mb: &MailboxCtx,
        page: Option<PageToken>,
        order: ListOrder,
    ) -> Result<MessagePage, MailError> {
        self.check(MailOp::ListInbox, mb.access_token.expose())?;
        let map = self
            .mailboxes
            .lock()
            .unwrap_or_else(|_| panic!("mailbox poisoned"));
        let Some(state) = map.get(&mb.mailbox) else {
            return Ok(MessagePage {
                items: vec![],
                next: None,
            });
        };
        let mut items: Vec<_> = state
            .messages
            .iter()
            .filter(|(_, m)| {
                m.labels.contains(SYS_INBOX)
                    && !m.labels.contains(SYS_TRASH)
                    && !m.labels.contains(SYS_SPAM)
            })
            .collect();
        items.sort_by(|a, b| {
            b.1.seed
                .internal_date
                .cmp(&a.1.seed.internal_date)
                .then_with(|| a.0.as_str().cmp(b.0.as_str()))
        });
        if let ListOrder::NewerThan(t) = order {
            items.retain(|(_, m)| m.seed.internal_date > t);
        }
        if let ListOrder::OlderThan(t) = order {
            items.retain(|(_, m)| m.seed.internal_date < t);
        }
        // Compute offset from page token.
        let offset = match &page {
            Some(PageToken(s)) if s.starts_with("o:") => s[2..]
                .parse::<usize>()
                .map_err(|_| MailError::Invalid("bad_page_token".into()))?,
            Some(_) => return Err(MailError::Invalid("bad_page_token".into())),
            None => 0,
        };
        let all: Vec<_> = items.into_iter().collect();
        let next_offset = offset + LIST_PAGE_SIZE;
        let window: Vec<_> = all.iter().skip(offset).take(LIST_PAGE_SIZE).collect();
        let has_more = all.len() > next_offset;
        let items = window
            .into_iter()
            .map(|(id, _)| Self::to_meta(state, &mb.mailbox, id))
            .collect();
        let next = if has_more {
            Some(PageToken(format!("o:{next_offset}")))
        } else {
            None
        };
        Ok(MessagePage { items, next })
    }

    async fn get_meta(&self, mb: &MailboxCtx, id: &MessageId) -> Result<MessageMeta, MailError> {
        self.check(MailOp::GetMeta, mb.access_token.expose())?;
        let map = self
            .mailboxes
            .lock()
            .unwrap_or_else(|_| panic!("mailbox poisoned"));
        let Some(state) = map.get(&mb.mailbox) else {
            return Err(MailError::NotFound);
        };
        if state.messages.contains_key(id) {
            Ok(Self::to_meta(state, &mb.mailbox, id))
        } else {
            Err(MailError::NotFound)
        }
    }

    async fn get_preview(&self, mb: &MailboxCtx, id: &MessageId) -> Result<String, MailError> {
        self.check(MailOp::GetPreview, mb.access_token.expose())?;
        let map = self
            .mailboxes
            .lock()
            .unwrap_or_else(|_| panic!("mailbox poisoned"));
        let Some(state) = map.get(&mb.mailbox) else {
            return Err(MailError::NotFound);
        };
        state
            .messages
            .get(id)
            .map(|m| m.seed.preview_text.clone())
            .ok_or(MailError::NotFound)
    }

    async fn set_labels(
        &self,
        mb: &MailboxCtx,
        id: &MessageId,
        add: &LabelSet,
        remove: &LabelSet,
    ) -> Result<LabelSet, MailError> {
        self.check(MailOp::SetLabels, mb.access_token.expose())?;
        for l in add.iter().chain(remove.iter()) {
            if REFUSED_LABELS.contains(&l.as_str()) {
                return Err(MailError::Invalid("label_not_allowed".into()));
            }
        }
        let mut map = self
            .mailboxes
            .lock()
            .unwrap_or_else(|_| panic!("mailbox poisoned"));
        let Some(state) = map.get_mut(&mb.mailbox) else {
            return Err(MailError::NotFound);
        };
        let Some(msg) = state.messages.get_mut(id) else {
            return Err(MailError::NotFound);
        };
        // Unknown user label IDs in add are invalid.
        for l in add.iter() {
            if l.starts_with("Label_") && !state.user_label_names.contains_key(l) {
                return Err(MailError::Invalid("unknown_label".into()));
            }
        }
        for l in remove.iter() {
            msg.labels.remove(l);
        }
        for l in add.iter() {
            msg.labels.insert(l.clone());
        }
        Ok(LabelSet::from_ids(
            msg.labels.iter().cloned().collect::<Vec<String>>(),
        ))
    }

    async fn trash(&self, mb: &MailboxCtx, id: &MessageId) -> Result<LabelSet, MailError> {
        self.check(MailOp::Trash, mb.access_token.expose())?;
        let mut map = self
            .mailboxes
            .lock()
            .unwrap_or_else(|_| panic!("mailbox poisoned"));
        let Some(state) = map.get_mut(&mb.mailbox) else {
            return Err(MailError::NotFound);
        };
        let Some(msg) = state.messages.get_mut(id) else {
            return Err(MailError::NotFound);
        };
        let before = msg.labels.clone();
        msg.labels.insert(SYS_TRASH.to_owned());
        msg.labels.remove(SYS_INBOX);
        Ok(LabelSet::from_ids(
            before.into_iter().collect::<Vec<String>>(),
        ))
    }

    async fn report_spam(&self, mb: &MailboxCtx, id: &MessageId) -> Result<LabelSet, MailError> {
        self.check(MailOp::ReportSpam, mb.access_token.expose())?;
        let mut map = self
            .mailboxes
            .lock()
            .unwrap_or_else(|_| panic!("mailbox poisoned"));
        let Some(state) = map.get_mut(&mb.mailbox) else {
            return Err(MailError::NotFound);
        };
        let Some(msg) = state.messages.get_mut(id) else {
            return Err(MailError::NotFound);
        };
        let before = msg.labels.clone();
        msg.labels.insert(SYS_SPAM.to_owned());
        msg.labels.remove(SYS_INBOX);
        Ok(LabelSet::from_ids(
            before.into_iter().collect::<Vec<String>>(),
        ))
    }

    async fn restore_labels(
        &self,
        mb: &MailboxCtx,
        id: &MessageId,
        exact: &LabelSet,
    ) -> Result<(), MailError> {
        self.check(MailOp::RestoreLabels, mb.access_token.expose())?;
        let mut map = self
            .mailboxes
            .lock()
            .unwrap_or_else(|_| panic!("mailbox poisoned"));
        let Some(state) = map.get_mut(&mb.mailbox) else {
            return Err(MailError::NotFound);
        };
        let Some(msg) = state.messages.get_mut(id) else {
            return Err(MailError::NotFound);
        };
        let kept: BTreeSet<String> = exact
            .iter()
            .filter(|l| !l.starts_with("Label_") || state.user_label_names.contains_key(l.as_str()))
            .cloned()
            .collect();
        msg.labels = kept;
        Ok(())
    }

    async fn ensure_label(&self, mb: &MailboxCtx, name: &str) -> Result<String, MailError> {
        self.check(MailOp::EnsureLabel, mb.access_token.expose())?;
        let mut map = self
            .mailboxes
            .lock()
            .unwrap_or_else(|_| panic!("mailbox poisoned"));
        let state = map.entry(mb.mailbox).or_default();
        if let Some(id) = find_user_label_id(state, name) {
            return Ok(id);
        }
        Ok(add_user_label(state, name))
    }

    async fn send_mailto(&self, mb: &MailboxCtx, to: &MailtoTarget) -> Result<(), MailError> {
        self.check(MailOp::SendMailto, mb.access_token.expose())?;
        let mut map = self
            .mailboxes
            .lock()
            .unwrap_or_else(|_| panic!("mailbox poisoned"));
        // The real adapter files the sent message under the "Mail Tinder" label
        // (UN-03 AC2); mirror that so a service test sees the same behaviour.
        let label = {
            let state = map.entry(mb.mailbox).or_default();
            let label = find_user_label_id(state, MAIL_TINDER_LABEL)
                .unwrap_or_else(|| add_user_label(state, MAIL_TINDER_LABEL));
            state.sent.push(SentRecord {
                mailbox: mb.mailbox,
                to: to.to().to_owned(),
                subject: to.subject().map(str::to_owned),
                body: to.body().map(str::to_owned),
            });
            label
        };
        let seed = SeedMessage {
            from_display: String::new(),
            from_address: String::new(),
            subject: String::new(),
            raw_headers: vec![],
            facts: domain::HeaderFacts::default(),
            preview_text: String::new(),
            internal_date: time::OffsetDateTime::now_utc(),
            labels: vec![SYS_SENT.to_owned(), label],
        };
        self.seed_inner(&mut map, &mb.mailbox, seed);
        Ok(())
    }

    async fn inbox_count(&self, mb: &MailboxCtx) -> Result<u64, MailError> {
        self.check(MailOp::InboxCount, mb.access_token.expose())?;
        let map = self
            .mailboxes
            .lock()
            .unwrap_or_else(|_| panic!("mailbox poisoned"));
        let Some(state) = map.get(&mb.mailbox) else {
            return Ok(0);
        };
        Ok(state
            .messages
            .iter()
            .filter(|(_, m)| {
                m.labels.contains(SYS_INBOX)
                    && !m.labels.contains(SYS_TRASH)
                    && !m.labels.contains(SYS_SPAM)
            })
            .count() as u64)
    }

    async fn list_messages(
        &self,
        mb: &MailboxCtx,
        q: &MessageQuery,
        page: Option<PageToken>,
        max: u32,
    ) -> Result<MessagePage, MailError> {
        self.check(MailOp::ListMessages, mb.access_token.expose())?;
        let map = self
            .mailboxes
            .lock()
            .unwrap_or_else(|_| panic!("mailbox poisoned"));
        let Some(state) = map.get(&mb.mailbox) else {
            return Ok(MessagePage {
                items: vec![],
                next: None,
            });
        };
        let mut items: Vec<_> = state
            .messages
            .iter()
            .filter(|(_, m)| matches_query(m, q))
            .collect();
        items.sort_by(|a, b| {
            b.1.seed
                .internal_date
                .cmp(&a.1.seed.internal_date)
                .then_with(|| a.0.as_str().cmp(b.0.as_str()))
        });
        let offset = match &page {
            Some(PageToken(s)) if s.starts_with("o:") => s[2..]
                .parse::<usize>()
                .map_err(|_| MailError::Invalid("bad_page_token".into()))?,
            Some(_) => return Err(MailError::Invalid("bad_page_token".into())),
            None => 0,
        };
        let size = max.clamp(1, 100) as usize;
        let next_offset = offset + size;
        let has_more = items.len() > next_offset;
        let window = items
            .iter()
            .skip(offset)
            .take(size)
            .map(|(id, _)| Self::to_meta(state, &mb.mailbox, id))
            .collect();
        Ok(MessagePage {
            items: window,
            next: has_more.then(|| PageToken(format!("o:{next_offset}"))),
        })
    }

    async fn count_messages(&self, mb: &MailboxCtx, q: &MessageQuery) -> Result<u64, MailError> {
        self.check(MailOp::CountMessages, mb.access_token.expose())?;
        let map = self
            .mailboxes
            .lock()
            .unwrap_or_else(|_| panic!("mailbox poisoned"));
        let Some(state) = map.get(&mb.mailbox) else {
            return Ok(0);
        };
        Ok(state
            .messages
            .values()
            .filter(|m| matches_query(m, q))
            .count() as u64)
    }

    async fn rename_label(
        &self,
        mb: &MailboxCtx,
        label_id: &str,
        new_name: &str,
    ) -> Result<(), MailError> {
        self.check(MailOp::RenameLabel, mb.access_token.expose())?;
        let mut map = self
            .mailboxes
            .lock()
            .unwrap_or_else(|_| panic!("mailbox poisoned"));
        let Some(state) = map.get_mut(&mb.mailbox) else {
            return Err(MailError::NotFound);
        };
        let Some(old_name) = state.user_label_names.get(label_id).cloned() else {
            return Err(MailError::NotFound);
        };
        let lowered = new_name.to_lowercase();
        if state
            .user_labels
            .get(&lowered)
            .is_some_and(|other| other != label_id)
        {
            return Err(MailError::Invalid("label_exists".into()));
        }
        state.user_labels.remove(&old_name.to_lowercase());
        state.user_labels.insert(lowered, label_id.to_owned());
        state
            .user_label_names
            .insert(label_id.to_owned(), new_name.to_owned());
        Ok(())
    }

    async fn remove_label(&self, mb: &MailboxCtx, label_id: &str) -> Result<(), MailError> {
        self.check(MailOp::RemoveLabel, mb.access_token.expose())?;
        let mut map = self
            .mailboxes
            .lock()
            .unwrap_or_else(|_| panic!("mailbox poisoned"));
        let Some(state) = map.get_mut(&mb.mailbox) else {
            return Err(MailError::NotFound);
        };
        let Some(name) = state.user_label_names.remove(label_id) else {
            return Err(MailError::NotFound);
        };
        state.user_labels.remove(&name.to_lowercase());
        // Only the label goes; every message stays (INV-5).
        for msg in state.messages.values_mut() {
            msg.labels.remove(label_id);
        }
        Ok(())
    }

    fn web_url(&self, mailbox_address: &str, id: &MessageId) -> String {
        let encoded: String =
            url::form_urlencoded::byte_serialize(id.as_str().as_bytes()).collect();
        let Ok(mut url) = url::Url::parse("https://mail.google.com/mail/") else {
            return "https://mail.google.com/mail/".to_owned();
        };
        url.query_pairs_mut()
            .append_pair("authuser", mailbox_address);
        url.set_fragment(Some(&format!("all/{encoded}")));
        url.into()
    }
}

/// Evaluate a typed query against one stored message.
fn matches_query(m: &state::StoredMessage, q: &MessageQuery) -> bool {
    let in_inbox = !q.in_inbox
        || (m.labels.contains(SYS_INBOX)
            && !m.labels.contains(SYS_TRASH)
            && !m.labels.contains(SYS_SPAM));
    let label = q.label.as_ref().map_or(true, |l| m.labels.contains(l));
    let after = q.after.map_or(true, |t| m.seed.internal_date > t);
    let before = q.before.map_or(true, |t| m.seed.internal_date < t);
    let from = q.from.as_ref().map_or(true, |f| {
        SenderKey::from_address(&m.seed.from_address).as_str() == f
    });
    let list = q.list_id.as_ref().map_or(true, |l| {
        m.seed.facts.list_id.as_deref() == Some(l.as_str())
    });
    in_inbox && label && after && before && from && list
}

trait SeedInner {
    fn seed_inner(
        &self,
        map: &mut HashMap<MailboxId, MailboxState>,
        mb: &MailboxId,
        msg: SeedMessage,
    ) -> MessageId;
}
impl SeedInner for FakeMailbox {
    fn seed_inner(
        &self,
        map: &mut HashMap<MailboxId, MailboxState>,
        mb: &MailboxId,
        msg: SeedMessage,
    ) -> MessageId {
        let state = map.entry(*mb).or_default();
        state.next_id += 1;
        let id = MessageId::new(format!("m{:04}", state.next_id)).unwrap_or_else(|_| panic!("id"));
        let labels: BTreeSet<String> = msg.labels.iter().cloned().collect();
        state
            .messages
            .insert(id.clone(), state::StoredMessage { seed: msg, labels });
        id
    }
}
