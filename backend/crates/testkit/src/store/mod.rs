//! In-memory implementation of every `ServerStore` repository.

mod table;

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use domain::{InviteStatus, JobId, MailboxId, Provider, ProviderSubjectId, UserId};
use ports::store::{
    BakeoffSnapshotRecord, BakeoffSnapshotRepo, ClassifierEvalRecord, ClassifierEvalRepo,
    ClassifiersConfig, ConfigRepo, EmailLookupHash, EvalId, InviteId, InviteRecord, InviteRepo,
    InviteRequestId, InviteRequestRecord, InviteRequestRepo, JobRecord, JobRepo, Keyed,
    ListKeyHash, MailboxRecord, MailboxRepo, NeedsAttentionId, NeedsAttentionRecord,
    NeedsAttentionRepo, Page, PageRequest, Precondition, RateLimitKey, RateLimitRecord,
    RateLimitRepo, Repo, ServerStore, SessionHash, SessionRecord, SessionRepo, SnapshotId,
    StoreCursor, StoreError, UserRecord, UserRepo, Version, Versioned, MAX_PAGE,
};
use time::{Duration, OffsetDateTime};

use self::table::Table;

/// A complete in-memory `ServerStore` with the same concurrency and ordering
/// rules Firestore will have.
pub struct InMemoryServerStore {
    users: UsersImpl,
    mailboxes: MailboxesImpl,
    invites: InvitesImpl,
    invite_requests: InviteRequestsImpl,
    jobs: JobsImpl,
    needs_attention: NeedsAttentionImpl,
    sessions: SessionsImpl,
    classifier_eval: ClassifierEvalImpl,
    bakeoff_snapshots: SnapshotsImpl,
    config: ConfigImpl,
    rate_limits: RateLimitsImpl,
}

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
struct RateLimitEntry {
    key: RateLimitKey,
    window_start: i64,
}

impl Default for InMemoryServerStore {
    fn default() -> Self {
        let fail_next = Arc::new(AtomicU32::new(0));
        Self {
            users: UsersImpl {
                table: Table::new(),
                fail_next: Arc::clone(&fail_next),
            },
            mailboxes: MailboxesImpl {
                table: Table::new(),
                fail_next: Arc::clone(&fail_next),
                race_put: std::sync::Mutex::new(None),
            },
            invites: InvitesImpl {
                table: Table::new(),
                fail_next: Arc::clone(&fail_next),
            },
            invite_requests: InviteRequestsImpl {
                table: Table::new(),
                fail_next: Arc::clone(&fail_next),
            },
            jobs: JobsImpl {
                table: Table::new(),
                fail_next: Arc::clone(&fail_next),
            },
            needs_attention: NeedsAttentionImpl {
                table: Table::new(),
                fail_next: Arc::clone(&fail_next),
            },
            sessions: SessionsImpl {
                table: Table::new(),
                fail_next: Arc::clone(&fail_next),
            },
            classifier_eval: ClassifierEvalImpl {
                table: Table::new(),
                fail_next: Arc::clone(&fail_next),
            },
            bakeoff_snapshots: SnapshotsImpl {
                table: Table::new(),
                fail_next: Arc::clone(&fail_next),
            },
            config: ConfigImpl {
                config: std::sync::Mutex::new(None),
                next_version: AtomicU64::new(0),
                fail_next: Arc::clone(&fail_next),
            },
            rate_limits: RateLimitsImpl {
                rate_limits: std::sync::Mutex::new(BTreeMap::new()),
                fail_next: Arc::clone(&fail_next),
            },
        }
    }
}

impl InMemoryServerStore {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Every document as (collection, `serde_json::Value`), for leak scans.
    #[must_use]
    pub fn export_json(&self) -> Vec<(&'static str, serde_json::Value)> {
        let mut out = Vec::new();
        for v in self.users.table.scan() {
            out.push(("users", j(&v.record)));
        }
        for v in self.mailboxes.table.scan() {
            out.push(("mailboxes", j(&v.record)));
        }
        for v in self.invites.table.scan() {
            out.push(("invites", j(&v.record)));
        }
        for v in self.invite_requests.table.scan() {
            out.push(("invite_requests", j(&v.record)));
        }
        for v in self.jobs.table.scan() {
            out.push(("jobs", j(&v.record)));
        }
        for v in self.needs_attention.table.scan() {
            out.push(("needs_attention", j(&v.record)));
        }
        for v in self.sessions.table.scan() {
            out.push(("sessions", j(&v.record)));
        }
        for v in self.classifier_eval.table.scan() {
            out.push(("classifier_eval", j(&v.record)));
        }
        for v in self.bakeoff_snapshots.table.scan() {
            out.push(("bakeoff_snapshots", j(&v.record)));
        }
        out
    }

    /// Make the next N calls of any method fail with `StoreError::Unavailable`.
    pub fn fail_next(&self, n: u32) {
        for f in [
            &self.users.fail_next,
            &self.mailboxes.fail_next,
            &self.invites.fail_next,
            &self.invite_requests.fail_next,
            &self.jobs.fail_next,
            &self.needs_attention.fail_next,
            &self.sessions.fail_next,
            &self.classifier_eval.fail_next,
            &self.bakeoff_snapshots.fail_next,
            &self.config.fail_next,
            &self.rate_limits.fail_next,
        ] {
            f.store(n, Ordering::SeqCst);
        }
    }

    /// Arrange for a competing mailbox record to be written immediately before
    /// the next `mailboxes().put(.., Precondition::MustNotExist)`, standing in
    /// for a concurrent writer that wins the race. The caller's put then sees
    /// `StoreError::AlreadyExists`, which lets a test drive the race-recovery
    /// path deterministically (INV-3).
    pub fn race_mailbox_put(&self, record: MailboxRecord) {
        *self
            .mailboxes
            .race_put
            .lock()
            .unwrap_or_else(|_| panic!("store poisoned")) = Some(record);
    }
}

impl ServerStore for InMemoryServerStore {
    fn users(&self) -> &dyn UserRepo {
        &self.users
    }
    fn mailboxes(&self) -> &dyn MailboxRepo {
        &self.mailboxes
    }
    fn invites(&self) -> &dyn InviteRepo {
        &self.invites
    }
    fn invite_requests(&self) -> &dyn InviteRequestRepo {
        &self.invite_requests
    }
    fn jobs(&self) -> &dyn JobRepo {
        &self.jobs
    }
    fn needs_attention(&self) -> &dyn NeedsAttentionRepo {
        &self.needs_attention
    }
    fn sessions(&self) -> &dyn SessionRepo {
        &self.sessions
    }
    fn classifier_eval(&self) -> &dyn ClassifierEvalRepo {
        &self.classifier_eval
    }
    fn bakeoff_snapshots(&self) -> &dyn BakeoffSnapshotRepo {
        &self.bakeoff_snapshots
    }
    fn config(&self) -> &dyn ConfigRepo {
        &self.config
    }
    fn rate_limits(&self) -> &dyn RateLimitRepo {
        &self.rate_limits
    }
}

fn j<T: serde::Serialize>(rec: &T) -> serde_json::Value {
    serde_json::to_value(rec).unwrap_or(serde_json::Value::Null)
}

/// How a key renders as a page cursor.
trait CursorKey {
    fn cursor(&self) -> String;
}
macro_rules! uuid_cursor {
    ($t:ty) => {
        impl CursorKey for $t {
            fn cursor(&self) -> String {
                self.0.to_string()
            }
        }
    };
}
uuid_cursor!(UserId);
uuid_cursor!(MailboxId);
uuid_cursor!(InviteId);
uuid_cursor!(InviteRequestId);
uuid_cursor!(JobId);
uuid_cursor!(NeedsAttentionId);
uuid_cursor!(EvalId);
uuid_cursor!(SnapshotId);
impl CursorKey for SessionHash {
    fn cursor(&self) -> String {
        self.to_hex()
    }
}

/// Page `items` (already sorted) by the cursor and limit. `next` is `Some`
/// only when more items remain after the returned window.
fn paginate<K: CursorKey + Ord, R: Keyed<Key = K>>(
    mut items: Vec<Versioned<R>>,
    page: PageRequest,
) -> Page<Versioned<R>> {
    let limit = page.limit.clamp(1, MAX_PAGE) as usize;
    if let Some(after) = page.after {
        if let Some(pos) = items
            .iter()
            .position(|v| CursorKey::cursor(&v.record.key()) == after.0)
        {
            items.drain(..=pos);
        }
    }
    let has_more = items.len() > limit;
    let next = if has_more {
        Some(StoreCursor(CursorKey::cursor(
            &items[limit - 1].record.key(),
        )))
    } else {
        None
    };
    items.truncate(limit);
    Page { items, next }
}

/// Encode a scan position `(order value, record key)` as an opaque cursor.
///
/// The value comes first so a resumed page skips every record whose
/// `(value, key)` is not greater than the cursor, which keeps a scan moving
/// even when a record between pages was deleted meanwhile.
fn encode_position(value: i128, key: &str) -> StoreCursor {
    StoreCursor(format!("{value}\u{1f}{key}"))
}

/// Decode a [`encode_position`] cursor.
fn decode_position(cursor: &StoreCursor) -> Option<(i128, String)> {
    let (value, key) = cursor.0.split_once('\u{1f}')?;
    Some((value.parse().ok()?, key.to_owned()))
}

/// Page `items` for a scan ordered by a numeric field, then the record key.
/// Both form the cursor, so unlike [`paginate`] a page can resume after a
/// record between pages was deleted.
fn paginate_by_value<R: Keyed>(
    mut items: Vec<Versioned<R>>,
    page: &PageRequest,
    value: impl Fn(&R) -> i128,
) -> Page<Versioned<R>>
where
    R::Key: CursorKey,
{
    items.sort_by(|a, b| {
        value(&a.record)
            .cmp(&value(&b.record))
            .then(a.record.key().cursor().cmp(&b.record.key().cursor()))
    });
    if let Some(cursor) = page.after.as_ref().and_then(decode_position) {
        let resume = items
            .iter()
            .position(|v| (value(&v.record), v.record.key().cursor()) > cursor);
        match resume {
            Some(pos) => {
                items.drain(..pos);
            }
            None => items.clear(),
        }
    }
    let limit = page.limit.clamp(1, MAX_PAGE) as usize;
    let has_more = items.len() > limit;
    let next = if has_more {
        let last = &items[limit - 1].record;
        Some(encode_position(value(last), &last.key().cursor()))
    } else {
        None
    };
    items.truncate(limit);
    Page { items, next }
}

struct UsersImpl {
    table: Table<UserId, UserRecord>,
    fail_next: Arc<AtomicU32>,
}
impl UsersImpl {
    fn arm_check(&self) -> Result<(), StoreError> {
        let mut cur = self.fail_next.load(Ordering::SeqCst);
        loop {
            if cur == 0 {
                return Ok(());
            }
            match self
                .fail_next
                .compare_exchange(cur, cur - 1, Ordering::SeqCst, Ordering::SeqCst)
            {
                Ok(_) => return Err(StoreError::Unavailable),
                Err(actual) => cur = actual,
            }
        }
    }
}
#[async_trait]
impl Repo<UserId, UserRecord> for UsersImpl {
    async fn get(&self, k: &UserId) -> Result<Option<Versioned<UserRecord>>, StoreError> {
        self.arm_check()?;
        Ok(self.table.get(k))
    }
    async fn put(&self, r: &UserRecord, pre: Precondition) -> Result<Version, StoreError> {
        self.arm_check()?;
        self.table.put(r, &pre)
    }
    async fn delete(&self, k: &UserId, pre: Precondition) -> Result<(), StoreError> {
        self.arm_check()?;
        self.table.delete(k, &pre)
    }
}
#[async_trait]
impl UserRepo for UsersImpl {
    async fn list(&self, page: PageRequest) -> Result<Page<Versioned<UserRecord>>, StoreError> {
        let mut items = self.table.scan();
        items.sort_by(|a, b| {
            a.record
                .created_at
                .cmp(&b.record.created_at)
                .then(a.record.user_id.cmp(&b.record.user_id))
        });
        Ok(paginate(items, page))
    }
}

struct MailboxesImpl {
    table: Table<MailboxId, MailboxRecord>,
    fail_next: Arc<AtomicU32>,
    /// A record to write immediately before the next `put`, standing in for a
    /// concurrent writer that wins the race (INV-3 tests).
    race_put: std::sync::Mutex<Option<MailboxRecord>>,
}
impl MailboxesImpl {
    fn arm_check(&self) -> Result<(), StoreError> {
        let mut cur = self.fail_next.load(Ordering::SeqCst);
        loop {
            if cur == 0 {
                return Ok(());
            }
            match self
                .fail_next
                .compare_exchange(cur, cur - 1, Ordering::SeqCst, Ordering::SeqCst)
            {
                Ok(_) => return Err(StoreError::Unavailable),
                Err(actual) => cur = actual,
            }
        }
    }
}
#[async_trait]
impl Repo<MailboxId, MailboxRecord> for MailboxesImpl {
    async fn get(&self, k: &MailboxId) -> Result<Option<Versioned<MailboxRecord>>, StoreError> {
        self.arm_check()?;
        Ok(self.table.get(k))
    }
    async fn put(&self, r: &MailboxRecord, pre: Precondition) -> Result<Version, StoreError> {
        self.arm_check()?;
        if pre == Precondition::MustNotExist {
            let racing = self
                .race_put
                .lock()
                .unwrap_or_else(|_| panic!("store poisoned"))
                .take();
            if let Some(racing) = racing {
                // A concurrent writer won the race: the caller's put must now
                // see `AlreadyExists` (the recovery path under test).
                self.table.put(&racing, &Precondition::None)?;
            }
        }
        self.table.put(r, &pre)
    }
    async fn delete(&self, k: &MailboxId, pre: Precondition) -> Result<(), StoreError> {
        self.arm_check()?;
        self.table.delete(k, &pre)
    }
}
#[async_trait]
impl MailboxRepo for MailboxesImpl {
    async fn by_user(&self, user: &UserId) -> Result<Vec<Versioned<MailboxRecord>>, StoreError> {
        let mut items: Vec<_> = self
            .table
            .scan()
            .into_iter()
            .filter(|v| v.record.user_id == *user)
            .collect();
        items.sort_by(|a, b| {
            a.record
                .linked_at
                .cmp(&b.record.linked_at)
                .then(a.record.mailbox_id.cmp(&b.record.mailbox_id))
        });
        Ok(items)
    }
    async fn by_subject(
        &self,
        provider: Provider,
        subject: &ProviderSubjectId,
    ) -> Result<Option<Versioned<MailboxRecord>>, StoreError> {
        Ok(self
            .table
            .scan()
            .into_iter()
            .find(|v| v.record.provider == provider && v.record.provider_subject_id == *subject))
    }
    async fn list(&self, page: PageRequest) -> Result<Page<Versioned<MailboxRecord>>, StoreError> {
        Ok(paginate_by_value(
            self.table.scan(),
            &page,
            |r: &MailboxRecord| r.linked_at.unix_timestamp_nanos(),
        ))
    }
    async fn delete_all_for_user(&self, user: &UserId) -> Result<u64, StoreError> {
        Ok(self.table.retain(|r| r.user_id != *user))
    }
}

struct InvitesImpl {
    table: Table<InviteId, InviteRecord>,
    fail_next: Arc<AtomicU32>,
}
impl InvitesImpl {
    fn arm_check(&self) -> Result<(), StoreError> {
        let mut cur = self.fail_next.load(Ordering::SeqCst);
        loop {
            if cur == 0 {
                return Ok(());
            }
            match self
                .fail_next
                .compare_exchange(cur, cur - 1, Ordering::SeqCst, Ordering::SeqCst)
            {
                Ok(_) => return Err(StoreError::Unavailable),
                Err(actual) => cur = actual,
            }
        }
    }
}
#[async_trait]
impl Repo<InviteId, InviteRecord> for InvitesImpl {
    async fn get(&self, k: &InviteId) -> Result<Option<Versioned<InviteRecord>>, StoreError> {
        self.arm_check()?;
        Ok(self.table.get(k))
    }
    async fn put(&self, r: &InviteRecord, pre: Precondition) -> Result<Version, StoreError> {
        self.arm_check()?;
        self.table.put(r, &pre)
    }
    async fn delete(&self, k: &InviteId, pre: Precondition) -> Result<(), StoreError> {
        self.arm_check()?;
        self.table.delete(k, &pre)
    }
}
#[async_trait]
impl InviteRepo for InvitesImpl {
    async fn by_token_hash(
        &self,
        hash: &ports::store::Sha256Hash,
    ) -> Result<Option<Versioned<InviteRecord>>, StoreError> {
        Ok(self
            .table
            .scan()
            .into_iter()
            .find(|v| v.record.token_hash == *hash))
    }
    async fn by_email_lookup(
        &self,
        hash: &EmailLookupHash,
    ) -> Result<Vec<Versioned<InviteRecord>>, StoreError> {
        Ok(self
            .table
            .scan()
            .into_iter()
            .filter(|v| v.record.email_lookup == *hash)
            .collect())
    }
    async fn list(
        &self,
        status: Option<InviteStatus>,
        page: PageRequest,
    ) -> Result<Page<Versioned<InviteRecord>>, StoreError> {
        let mut items: Vec<_> = self
            .table
            .scan()
            .into_iter()
            .filter(|v| status.map_or(true, |s| v.record.status == s))
            .collect();
        items.sort_by(|a, b| {
            b.record
                .created_at
                .cmp(&a.record.created_at)
                .then(a.record.invite_id.cmp(&b.record.invite_id))
        });
        Ok(paginate(items, page))
    }
    async fn purge_due(
        &self,
        now: OffsetDateTime,
        limit: u32,
    ) -> Result<Vec<InviteId>, StoreError> {
        let mut ids: Vec<_> = self
            .table
            .scan()
            .into_iter()
            .filter(|v| v.record.purge_at <= now)
            .map(|v| v.record.invite_id)
            .collect();
        ids.sort();
        ids.truncate(limit as usize);
        Ok(ids)
    }
}

struct InviteRequestsImpl {
    table: Table<InviteRequestId, InviteRequestRecord>,
    fail_next: Arc<AtomicU32>,
}
impl InviteRequestsImpl {
    fn arm_check(&self) -> Result<(), StoreError> {
        let mut cur = self.fail_next.load(Ordering::SeqCst);
        loop {
            if cur == 0 {
                return Ok(());
            }
            match self
                .fail_next
                .compare_exchange(cur, cur - 1, Ordering::SeqCst, Ordering::SeqCst)
            {
                Ok(_) => return Err(StoreError::Unavailable),
                Err(actual) => cur = actual,
            }
        }
    }
}
#[async_trait]
impl Repo<InviteRequestId, InviteRequestRecord> for InviteRequestsImpl {
    async fn get(
        &self,
        k: &InviteRequestId,
    ) -> Result<Option<Versioned<InviteRequestRecord>>, StoreError> {
        self.arm_check()?;
        Ok(self.table.get(k))
    }
    async fn put(&self, r: &InviteRequestRecord, pre: Precondition) -> Result<Version, StoreError> {
        self.arm_check()?;
        self.table.put(r, &pre)
    }
    async fn delete(&self, k: &InviteRequestId, pre: Precondition) -> Result<(), StoreError> {
        self.arm_check()?;
        self.table.delete(k, &pre)
    }
}
#[async_trait]
impl InviteRequestRepo for InviteRequestsImpl {
    async fn by_email_lookup(
        &self,
        hash: &EmailLookupHash,
    ) -> Result<Option<Versioned<InviteRequestRecord>>, StoreError> {
        Ok(self
            .table
            .scan()
            .into_iter()
            .find(|v| v.record.email_lookup == *hash))
    }
    async fn list(
        &self,
        page: PageRequest,
    ) -> Result<Page<Versioned<InviteRequestRecord>>, StoreError> {
        let mut items = self.table.scan();
        items.sort_by(|a, b| {
            a.record
                .created_at
                .cmp(&b.record.created_at)
                .then(a.record.request_id.cmp(&b.record.request_id))
        });
        Ok(paginate(items, page))
    }
}

struct JobsImpl {
    table: Table<JobId, JobRecord>,
    fail_next: Arc<AtomicU32>,
}
impl JobsImpl {
    fn arm_check(&self) -> Result<(), StoreError> {
        let mut cur = self.fail_next.load(Ordering::SeqCst);
        loop {
            if cur == 0 {
                return Ok(());
            }
            match self
                .fail_next
                .compare_exchange(cur, cur - 1, Ordering::SeqCst, Ordering::SeqCst)
            {
                Ok(_) => return Err(StoreError::Unavailable),
                Err(actual) => cur = actual,
            }
        }
    }
}
#[async_trait]
impl Repo<JobId, JobRecord> for JobsImpl {
    async fn get(&self, k: &JobId) -> Result<Option<Versioned<JobRecord>>, StoreError> {
        self.arm_check()?;
        Ok(self.table.get(k))
    }
    async fn put(&self, r: &JobRecord, pre: Precondition) -> Result<Version, StoreError> {
        self.arm_check()?;
        self.table.put(r, &pre)
    }
    async fn delete(&self, k: &JobId, pre: Precondition) -> Result<(), StoreError> {
        self.arm_check()?;
        self.table.delete(k, &pre)
    }
}
#[async_trait]
impl JobRepo for JobsImpl {
    async fn by_mailbox(
        &self,
        mailbox: &MailboxId,
    ) -> Result<Vec<Versioned<JobRecord>>, StoreError> {
        Ok(self
            .table
            .scan()
            .into_iter()
            .filter(|v| v.record.mailbox_id == *mailbox)
            .collect())
    }
    async fn by_user_with_outcome(
        &self,
        user: &UserId,
        limit: u32,
    ) -> Result<Vec<Versioned<JobRecord>>, StoreError> {
        let mut items: Vec<_> = self
            .table
            .scan()
            .into_iter()
            .filter(|v| v.record.user_id == *user && v.record.outcome.is_some())
            .collect();
        items.sort_by(|a, b| {
            a.record
                .due_at
                .cmp(&b.record.due_at)
                .then(a.record.job_id.cmp(&b.record.job_id))
        });
        items.truncate(limit as usize);
        Ok(items)
    }
    async fn queued_for_list(
        &self,
        user: &UserId,
        list: &ListKeyHash,
    ) -> Result<Vec<Versioned<JobRecord>>, StoreError> {
        Ok(self
            .table
            .scan()
            .into_iter()
            .filter(|v| {
                v.record.user_id == *user
                    && v.record.list_key_hash == *list
                    && v.record.status == domain::JobStatus::Queued
            })
            .collect())
    }
    async fn expires_by(
        &self,
        now: OffsetDateTime,
        page: PageRequest,
    ) -> Result<Page<Versioned<JobRecord>>, StoreError> {
        let items: Vec<_> = self
            .table
            .scan()
            .into_iter()
            .filter(|v| v.record.expires_at <= now)
            .collect();
        Ok(paginate_by_value(items, &page, |r: &JobRecord| {
            r.expires_at.unix_timestamp_nanos()
        }))
    }
    async fn delete_all_for_user(&self, user: &UserId) -> Result<u64, StoreError> {
        Ok(self.table.retain(|r| r.user_id != *user))
    }
}

struct NeedsAttentionImpl {
    table: Table<NeedsAttentionId, NeedsAttentionRecord>,
    fail_next: Arc<AtomicU32>,
}
impl NeedsAttentionImpl {
    fn arm_check(&self) -> Result<(), StoreError> {
        let mut cur = self.fail_next.load(Ordering::SeqCst);
        loop {
            if cur == 0 {
                return Ok(());
            }
            match self
                .fail_next
                .compare_exchange(cur, cur - 1, Ordering::SeqCst, Ordering::SeqCst)
            {
                Ok(_) => return Err(StoreError::Unavailable),
                Err(actual) => cur = actual,
            }
        }
    }
}
#[async_trait]
impl Repo<NeedsAttentionId, NeedsAttentionRecord> for NeedsAttentionImpl {
    async fn get(
        &self,
        k: &NeedsAttentionId,
    ) -> Result<Option<Versioned<NeedsAttentionRecord>>, StoreError> {
        self.arm_check()?;
        Ok(self.table.get(k))
    }
    async fn put(
        &self,
        r: &NeedsAttentionRecord,
        pre: Precondition,
    ) -> Result<Version, StoreError> {
        self.arm_check()?;
        self.table.put(r, &pre)
    }
    async fn delete(&self, k: &NeedsAttentionId, pre: Precondition) -> Result<(), StoreError> {
        self.arm_check()?;
        self.table.delete(k, &pre)
    }
}
#[async_trait]
impl NeedsAttentionRepo for NeedsAttentionImpl {
    async fn by_user(
        &self,
        user: &UserId,
        page: PageRequest,
    ) -> Result<Page<Versioned<NeedsAttentionRecord>>, StoreError> {
        let mut items: Vec<_> = self
            .table
            .scan()
            .into_iter()
            .filter(|v| v.record.user_id == *user)
            .collect();
        items.sort_by(|a, b| {
            b.record
                .created_at
                .cmp(&a.record.created_at)
                .then(a.record.item_id.cmp(&b.record.item_id))
        });
        Ok(paginate(items, page))
    }
    async fn count_for_user(&self, user: &UserId) -> Result<u64, StoreError> {
        Ok(self
            .table
            .scan()
            .into_iter()
            .filter(|v| v.record.user_id == *user)
            .count() as u64)
    }
    async fn expires_by(
        &self,
        now: OffsetDateTime,
        page: PageRequest,
    ) -> Result<Page<NeedsAttentionId>, StoreError> {
        let items: Vec<_> = self
            .table
            .scan()
            .into_iter()
            .filter(|v| v.record.expires_at <= now)
            .collect();
        let paged = paginate_by_value(items, &page, |r: &NeedsAttentionRecord| {
            r.expires_at.unix_timestamp_nanos()
        });
        Ok(Page {
            items: paged.items.into_iter().map(|v| v.record.item_id).collect(),
            next: paged.next,
        })
    }
    async fn delete_all_for_user(&self, user: &UserId) -> Result<u64, StoreError> {
        Ok(self.table.retain(|r| r.user_id != *user))
    }
}

struct SessionsImpl {
    table: Table<SessionHash, SessionRecord>,
    fail_next: Arc<AtomicU32>,
}
impl SessionsImpl {
    fn arm_check(&self) -> Result<(), StoreError> {
        let mut cur = self.fail_next.load(Ordering::SeqCst);
        loop {
            if cur == 0 {
                return Ok(());
            }
            match self
                .fail_next
                .compare_exchange(cur, cur - 1, Ordering::SeqCst, Ordering::SeqCst)
            {
                Ok(_) => return Err(StoreError::Unavailable),
                Err(actual) => cur = actual,
            }
        }
    }
}
#[async_trait]
impl Repo<SessionHash, SessionRecord> for SessionsImpl {
    async fn get(&self, k: &SessionHash) -> Result<Option<Versioned<SessionRecord>>, StoreError> {
        self.arm_check()?;
        Ok(self.table.get(k))
    }
    async fn put(&self, r: &SessionRecord, pre: Precondition) -> Result<Version, StoreError> {
        self.arm_check()?;
        self.table.put(r, &pre)
    }
    async fn delete(&self, k: &SessionHash, pre: Precondition) -> Result<(), StoreError> {
        self.arm_check()?;
        self.table.delete(k, &pre)
    }
}
#[async_trait]
impl SessionRepo for SessionsImpl {
    async fn by_user(&self, user: &UserId) -> Result<Vec<Versioned<SessionRecord>>, StoreError> {
        Ok(self
            .table
            .scan()
            .into_iter()
            .filter(|v| v.record.user_id == Some(*user))
            .collect())
    }
    async fn expires_by(
        &self,
        now: OffsetDateTime,
        page: PageRequest,
    ) -> Result<Page<SessionHash>, StoreError> {
        let items: Vec<_> = self
            .table
            .scan()
            .into_iter()
            .filter(|v| v.record.expires_at <= now)
            .collect();
        let paged = paginate_by_value(items, &page, |r: &SessionRecord| {
            r.expires_at.unix_timestamp_nanos()
        });
        Ok(Page {
            items: paged
                .items
                .into_iter()
                .map(|v| v.record.session_hash)
                .collect(),
            next: paged.next,
        })
    }
    async fn delete_all_for_user(&self, user: &UserId) -> Result<u64, StoreError> {
        Ok(self.table.retain(|r| r.user_id != Some(*user)))
    }
}

struct ClassifierEvalImpl {
    table: Table<EvalId, ClassifierEvalRecord>,
    fail_next: Arc<AtomicU32>,
}
impl ClassifierEvalImpl {
    fn arm_check(&self) -> Result<(), StoreError> {
        let mut cur = self.fail_next.load(Ordering::SeqCst);
        loop {
            if cur == 0 {
                return Ok(());
            }
            match self
                .fail_next
                .compare_exchange(cur, cur - 1, Ordering::SeqCst, Ordering::SeqCst)
            {
                Ok(_) => return Err(StoreError::Unavailable),
                Err(actual) => cur = actual,
            }
        }
    }
}
#[async_trait]
impl Repo<EvalId, ClassifierEvalRecord> for ClassifierEvalImpl {
    async fn get(&self, k: &EvalId) -> Result<Option<Versioned<ClassifierEvalRecord>>, StoreError> {
        self.arm_check()?;
        Ok(self.table.get(k))
    }
    async fn put(
        &self,
        r: &ClassifierEvalRecord,
        pre: Precondition,
    ) -> Result<Version, StoreError> {
        self.arm_check()?;
        self.table.put(r, &pre)
    }
    async fn delete(&self, k: &EvalId, pre: Precondition) -> Result<(), StoreError> {
        self.arm_check()?;
        self.table.delete(k, &pre)
    }
}
#[async_trait]
impl ClassifierEvalRepo for ClassifierEvalImpl {
    async fn range(
        &self,
        from: OffsetDateTime,
        to: OffsetDateTime,
        page: PageRequest,
    ) -> Result<Page<ClassifierEvalRecord>, StoreError> {
        let mut items: Vec<_> = self
            .table
            .scan()
            .into_iter()
            .filter(|v| v.record.created_at >= from && v.record.created_at < to)
            .collect();
        items.sort_by(|a, b| {
            a.record
                .created_at
                .cmp(&b.record.created_at)
                .then(a.record.eval_id.cmp(&b.record.eval_id))
        });
        let paged = paginate(items, page);
        Ok(Page {
            items: paged.items.into_iter().map(|v| v.record).collect(),
            next: paged.next,
        })
    }
    async fn delete_for_users(
        &self,
        ids: &[ports::store::UserPseudoId],
    ) -> Result<u64, StoreError> {
        Ok(self.table.retain(|r| !ids.contains(&r.user_pseudo_id)))
    }
    async fn expires_by(
        &self,
        now: OffsetDateTime,
        _limit: u32,
    ) -> Result<Vec<EvalId>, StoreError> {
        let mut ids: Vec<_> = self
            .table
            .scan()
            .into_iter()
            .filter(|v| v.record.expires_at <= now)
            .map(|v| v.record.eval_id)
            .collect();
        ids.sort();
        Ok(ids)
    }
}

struct SnapshotsImpl {
    table: Table<SnapshotId, BakeoffSnapshotRecord>,
    fail_next: Arc<AtomicU32>,
}
impl SnapshotsImpl {
    fn arm_check(&self) -> Result<(), StoreError> {
        let mut cur = self.fail_next.load(Ordering::SeqCst);
        loop {
            if cur == 0 {
                return Ok(());
            }
            match self
                .fail_next
                .compare_exchange(cur, cur - 1, Ordering::SeqCst, Ordering::SeqCst)
            {
                Ok(_) => return Err(StoreError::Unavailable),
                Err(actual) => cur = actual,
            }
        }
    }
}
#[async_trait]
impl Repo<SnapshotId, BakeoffSnapshotRecord> for SnapshotsImpl {
    async fn get(
        &self,
        k: &SnapshotId,
    ) -> Result<Option<Versioned<BakeoffSnapshotRecord>>, StoreError> {
        self.arm_check()?;
        Ok(self.table.get(k))
    }
    async fn put(
        &self,
        r: &BakeoffSnapshotRecord,
        pre: Precondition,
    ) -> Result<Version, StoreError> {
        self.arm_check()?;
        self.table.put(r, &pre)
    }
    async fn delete(&self, k: &SnapshotId, pre: Precondition) -> Result<(), StoreError> {
        self.arm_check()?;
        self.table.delete(k, &pre)
    }
}
#[async_trait]
impl BakeoffSnapshotRepo for SnapshotsImpl {
    async fn list(
        &self,
        page: PageRequest,
    ) -> Result<Page<Versioned<BakeoffSnapshotRecord>>, StoreError> {
        let mut items = self.table.scan();
        items.sort_by(|a, b| {
            b.record
                .created_at
                .cmp(&a.record.created_at)
                .then(a.record.snapshot_id.cmp(&b.record.snapshot_id))
        });
        Ok(paginate(items, page))
    }
    async fn count(&self) -> Result<u64, StoreError> {
        Ok(self.table.scan().len() as u64)
    }
}

struct ConfigImpl {
    config: std::sync::Mutex<Option<(ClassifiersConfig, u64)>>,
    next_version: AtomicU64,
    fail_next: Arc<AtomicU32>,
}
impl Default for ConfigImpl {
    fn default() -> Self {
        Self {
            config: std::sync::Mutex::new(None),
            next_version: AtomicU64::new(0),
            fail_next: Arc::new(AtomicU32::new(0)),
        }
    }
}
impl ConfigImpl {
    fn arm_check(&self) -> Result<(), StoreError> {
        let mut cur = self.fail_next.load(Ordering::SeqCst);
        loop {
            if cur == 0 {
                return Ok(());
            }
            match self
                .fail_next
                .compare_exchange(cur, cur - 1, Ordering::SeqCst, Ordering::SeqCst)
            {
                Ok(_) => return Err(StoreError::Unavailable),
                Err(actual) => cur = actual,
            }
        }
    }
}
#[async_trait]
impl ConfigRepo for ConfigImpl {
    async fn get_classifiers(&self) -> Result<Option<Versioned<ClassifiersConfig>>, StoreError> {
        self.arm_check()?;
        let guard = self
            .config
            .lock()
            .unwrap_or_else(|_| panic!("store poisoned"));
        Ok(guard.as_ref().map(|(c, v)| Versioned {
            record: c.clone(),
            version: Version(v.to_string()),
        }))
    }
    async fn put_classifiers(
        &self,
        cfg: &ClassifiersConfig,
        pre: Precondition,
    ) -> Result<Version, StoreError> {
        self.arm_check()?;
        let mut guard = self
            .config
            .lock()
            .unwrap_or_else(|_| panic!("store poisoned"));
        let existing = guard.as_ref().map(|(_, v)| v.to_string());
        match pre {
            Precondition::None => {}
            Precondition::MustNotExist => {
                if existing.is_some() {
                    return Err(StoreError::AlreadyExists);
                }
            }
            Precondition::MustExist => {
                if existing.is_none() {
                    return Err(StoreError::PreconditionFailed);
                }
            }
            Precondition::Matches(v) => {
                if existing.as_deref() != Some(v.0.as_str()) {
                    return Err(StoreError::PreconditionFailed);
                }
            }
        }
        let version = self.next_version.fetch_add(1, Ordering::SeqCst) + 1;
        *guard = Some((cfg.clone(), version));
        Ok(Version(version.to_string()))
    }
}

struct RateLimitsImpl {
    rate_limits: std::sync::Mutex<BTreeMap<RateLimitEntry, RateLimitRecord>>,
    fail_next: Arc<AtomicU32>,
}
impl Default for RateLimitsImpl {
    fn default() -> Self {
        Self {
            rate_limits: std::sync::Mutex::new(BTreeMap::new()),
            fail_next: Arc::new(AtomicU32::new(0)),
        }
    }
}
impl RateLimitsImpl {
    fn arm_check(&self) -> Result<(), StoreError> {
        let mut cur = self.fail_next.load(Ordering::SeqCst);
        loop {
            if cur == 0 {
                return Ok(());
            }
            match self
                .fail_next
                .compare_exchange(cur, cur - 1, Ordering::SeqCst, Ordering::SeqCst)
            {
                Ok(_) => return Err(StoreError::Unavailable),
                Err(actual) => cur = actual,
            }
        }
    }
}
#[async_trait]
impl RateLimitRepo for RateLimitsImpl {
    async fn hit(
        &self,
        key: &RateLimitKey,
        window_start: OffsetDateTime,
        window: Duration,
    ) -> Result<u32, StoreError> {
        self.arm_check()?;
        let mut guard = self
            .rate_limits
            .lock()
            .unwrap_or_else(|_| panic!("store poisoned"));
        let entry = RateLimitEntry {
            key: key.clone(),
            window_start: window_start.unix_timestamp(),
        };
        let count = if let Some(rec) = guard.get_mut(&entry) {
            rec.count += 1;
            rec.count
        } else {
            guard.insert(
                entry,
                RateLimitRecord {
                    key: key.0.clone(),
                    window_start,
                    count: 1,
                    expires_at: window_start + window,
                },
            );
            1
        };
        Ok(count)
    }
    async fn expires_by(&self, now: OffsetDateTime, _limit: u32) -> Result<u64, StoreError> {
        self.arm_check()?;
        let mut guard = self
            .rate_limits
            .lock()
            .unwrap_or_else(|_| panic!("store poisoned"));
        let before = guard.len();
        guard.retain(|_, r| r.expires_at > now);
        Ok((before - guard.len()) as u64)
    }
}
