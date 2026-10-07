//! One repository trait per collection.

use async_trait::async_trait;
use domain::{InviteStatus, JobId, MailboxId, Provider, ProviderSubjectId, UserId};
use time::OffsetDateTime;

use crate::store::ids::{
    EmailLookupHash, EvalId, InviteId, InviteRequestId, ListKeyHash, NeedsAttentionId,
    RateLimitKey, SessionHash, Sha256Hash, SnapshotId, UserPseudoId,
};
use crate::store::records::{
    BakeoffSnapshotRecord, ClassifierEvalRecord, ClassifiersConfig, InviteRecord,
    InviteRequestRecord, JobRecord, MailboxRecord, NeedsAttentionRecord, SessionRecord, UserRecord,
};
use crate::store::{Page, PageRequest, Precondition, Repo, StoreError, Version, Versioned};

/// users. `list` is `created_at` asc, then `user_id`.
#[async_trait]
pub trait UserRepo: Repo<UserId, UserRecord> {
    async fn list(&self, page: PageRequest) -> Result<Page<Versioned<UserRecord>>, StoreError>;
}

/// mailboxes. `by_user` is `linked_at` asc.
#[async_trait]
pub trait MailboxRepo: Repo<MailboxId, MailboxRecord> {
    async fn by_user(&self, user: &UserId) -> Result<Vec<Versioned<MailboxRecord>>, StoreError>;
    async fn by_subject(
        &self,
        provider: Provider,
        subject: &ProviderSubjectId,
    ) -> Result<Option<Versioned<MailboxRecord>>, StoreError>;
    /// Every mailbox, `linked_at` asc then `mailbox_id`, paged. Used by the
    /// account-deletion sweep to find a mailbox whose user document is gone.
    async fn list(&self, page: PageRequest) -> Result<Page<Versioned<MailboxRecord>>, StoreError>;
    async fn delete_all_for_user(&self, user: &UserId) -> Result<u64, StoreError>;
}

/// invites. `list` is `created_at` desc.
#[async_trait]
pub trait InviteRepo: Repo<InviteId, InviteRecord> {
    async fn by_token_hash(
        &self,
        hash: &Sha256Hash,
    ) -> Result<Option<Versioned<InviteRecord>>, StoreError>;
    async fn by_email_lookup(
        &self,
        hash: &EmailLookupHash,
    ) -> Result<Vec<Versioned<InviteRecord>>, StoreError>;
    async fn list(
        &self,
        status: Option<InviteStatus>,
        page: PageRequest,
    ) -> Result<Page<Versioned<InviteRecord>>, StoreError>;
    /// purge_at <= now.
    async fn purge_due(&self, now: OffsetDateTime, limit: u32)
        -> Result<Vec<InviteId>, StoreError>;
}

/// invite_requests. `list` is `created_at` asc.
#[async_trait]
pub trait InviteRequestRepo: Repo<InviteRequestId, InviteRequestRecord> {
    async fn by_email_lookup(
        &self,
        hash: &EmailLookupHash,
    ) -> Result<Option<Versioned<InviteRequestRecord>>, StoreError>;
    async fn list(
        &self,
        page: PageRequest,
    ) -> Result<Page<Versioned<InviteRequestRecord>>, StoreError>;
}

/// jobs.
#[async_trait]
pub trait JobRepo: Repo<JobId, JobRecord> {
    async fn by_mailbox(
        &self,
        mailbox: &MailboxId,
    ) -> Result<Vec<Versioned<JobRecord>>, StoreError>;
    /// outcome set, due_at asc.
    async fn by_user_with_outcome(
        &self,
        user: &UserId,
        limit: u32,
    ) -> Result<Vec<Versioned<JobRecord>>, StoreError>;
    /// status queued.
    async fn queued_for_list(
        &self,
        user: &UserId,
        list: &ListKeyHash,
    ) -> Result<Vec<Versioned<JobRecord>>, StoreError>;
    /// expires_at <= now, asc then `job_id`, paged so a caller can scan past
    /// live records that share the oldest expiries (T-803 AU-06 AC1).
    async fn expires_by(
        &self,
        now: OffsetDateTime,
        page: PageRequest,
    ) -> Result<Page<Versioned<JobRecord>>, StoreError>;
    async fn delete_all_for_user(&self, user: &UserId) -> Result<u64, StoreError>;
}

/// needs_attention. `by_user` is created_at desc.
#[async_trait]
pub trait NeedsAttentionRepo: Repo<NeedsAttentionId, NeedsAttentionRecord> {
    async fn by_user(
        &self,
        user: &UserId,
        page: PageRequest,
    ) -> Result<Page<Versioned<NeedsAttentionRecord>>, StoreError>;
    async fn count_for_user(&self, user: &UserId) -> Result<u64, StoreError>;
    /// expires_at <= now, asc then `item_id`, paged.
    async fn expires_by(
        &self,
        now: OffsetDateTime,
        page: PageRequest,
    ) -> Result<Page<NeedsAttentionId>, StoreError>;
    async fn delete_all_for_user(&self, user: &UserId) -> Result<u64, StoreError>;
}

/// sessions.
#[async_trait]
pub trait SessionRepo: Repo<SessionHash, SessionRecord> {
    async fn by_user(&self, user: &UserId) -> Result<Vec<Versioned<SessionRecord>>, StoreError>;
    /// expires_at <= now, asc then `session_hash`, paged.
    async fn expires_by(
        &self,
        now: OffsetDateTime,
        page: PageRequest,
    ) -> Result<Page<SessionHash>, StoreError>;
    async fn delete_all_for_user(&self, user: &UserId) -> Result<u64, StoreError>;
}

/// classifier_eval. `range` is created_at in [from, to), asc.
#[async_trait]
pub trait ClassifierEvalRepo: Repo<EvalId, ClassifierEvalRecord> {
    async fn range(
        &self,
        from: OffsetDateTime,
        to: OffsetDateTime,
        page: PageRequest,
    ) -> Result<Page<ClassifierEvalRecord>, StoreError>;
    /// Opt-out and account deletion. Takes every pseudo ID the user has had.
    async fn delete_for_users(&self, ids: &[UserPseudoId]) -> Result<u64, StoreError>;
    async fn expires_by(&self, now: OffsetDateTime, limit: u32) -> Result<Vec<EvalId>, StoreError>;
}

/// bakeoff_snapshots. `list` is created_at desc.
#[async_trait]
pub trait BakeoffSnapshotRepo: Repo<SnapshotId, BakeoffSnapshotRecord> {
    async fn list(
        &self,
        page: PageRequest,
    ) -> Result<Page<Versioned<BakeoffSnapshotRecord>>, StoreError>;
    async fn count(&self) -> Result<u64, StoreError>;
}

/// config.
#[async_trait]
pub trait ConfigRepo: Send + Sync {
    async fn get_classifiers(&self) -> Result<Option<Versioned<ClassifiersConfig>>, StoreError>;
    async fn put_classifiers(
        &self,
        cfg: &ClassifiersConfig,
        pre: Precondition,
    ) -> Result<Version, StoreError>;
}

/// rate_limits.
#[async_trait]
pub trait RateLimitRepo: Send + Sync {
    /// Atomically add one to the counter for (key, window_start) and return
    /// the new count.
    async fn hit(
        &self,
        key: &RateLimitKey,
        window_start: OffsetDateTime,
        window: time::Duration,
    ) -> Result<u32, StoreError>;
    /// Deletes, returns count.
    async fn expires_by(&self, now: OffsetDateTime, limit: u32) -> Result<u64, StoreError>;
}
