//! The `ServerStore` port: one typed repository per Firestore collection.

pub mod aad_fields;
pub mod ids;
pub mod records;
pub mod repos;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

pub use crate::store::ids::{
    Ciphertext, EmailLookupHash, EvalId, InviteId, InviteRequestId, ListKeyHash, NeedsAttentionId,
    RateLimitKey, SessionHash, SessionRecordId, Sha256Hash, SnapshotId, UserPseudoId,
};
pub use crate::store::records::{
    mailbox_id_for, AgeBucket, AuthIntent, BakeoffSnapshotRecord, ClassifierEvalRecord,
    ClassifiersConfig, EvalHeaderFacts, EvalOutcome, HeaderRulesResult, InviteRecord,
    InviteRequestRecord, InviteRequestStatus, JobOutcome, JobOutcomeCode, JobRecord, MailboxRecord,
    ModelErrorCode, ModelPrediction, NeedsAttentionRecord, PreAuthFields, RateLimitRecord,
    SessionRecord, SessionState, SwipeDirection, TextTokensBucket, UserRecord, MAILBOX_NAMESPACE,
};
pub use crate::store::repos::{
    BakeoffSnapshotRepo, ClassifierEvalRepo, ConfigRepo, InviteRepo, InviteRequestRepo, JobRepo,
    MailboxRepo, NeedsAttentionRepo, RateLimitRepo, SessionRepo, UserRepo,
};

/// An opaque record version for optimistic concurrency.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Version(pub String);

/// A precondition on a write.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Precondition {
    None,
    MustNotExist,
    MustExist,
    Matches(Version),
}

/// A record plus its version.
#[derive(Clone, Debug, PartialEq)]
pub struct Versioned<T> {
    pub record: T,
    pub version: Version,
}

/// An internal paging cursor. The api seals it before it leaves.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoreCursor(pub String);

/// A paging request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PageRequest {
    pub limit: u32,
    pub after: Option<StoreCursor>,
}

/// A page of items.
#[derive(Clone, Debug, PartialEq)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub next: Option<StoreCursor>,
}

/// The maximum page size.
pub const MAX_PAGE: u32 = 100;

/// A store error.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum StoreError {
    #[error("already exists")]
    AlreadyExists,
    #[error("precondition failed")]
    PreconditionFailed,
    #[error("store unavailable")]
    Unavailable,
    #[error("corrupt record: {0}")]
    Corrupt(&'static str),
    #[error("invalid: {0}")]
    Invalid(&'static str),
}

/// The eleven Firestore collections allowed by S5.
pub const COLLECTIONS: [&str; 11] = [
    "users",
    "mailboxes",
    "invites",
    "invite_requests",
    "jobs",
    "needs_attention",
    "sessions",
    "classifier_eval",
    "bakeoff_snapshots",
    "config",
    "rate_limits",
];

/// A record knows its own document key.
pub trait Keyed {
    type Key: Send + Sync;
    fn key(&self) -> Self::Key;
}

/// A typed repository for one collection.
#[async_trait]
pub trait Repo<K: Send + Sync, R: Keyed<Key = K> + Send + Sync>: Send + Sync {
    async fn get(&self, key: &K) -> Result<Option<Versioned<R>>, StoreError>;
    /// Create or replace. Returns the new version.
    async fn put(&self, record: &R, pre: Precondition) -> Result<Version, StoreError>;
    /// Missing document with `Precondition::None` is `Ok` (idempotent).
    async fn delete(&self, key: &K, pre: Precondition) -> Result<(), StoreError>;
}

/// Hands out one repository per collection.
pub trait ServerStore: Send + Sync {
    fn users(&self) -> &dyn UserRepo;
    fn mailboxes(&self) -> &dyn MailboxRepo;
    fn invites(&self) -> &dyn InviteRepo;
    fn invite_requests(&self) -> &dyn InviteRequestRepo;
    fn jobs(&self) -> &dyn JobRepo;
    fn needs_attention(&self) -> &dyn NeedsAttentionRepo;
    fn sessions(&self) -> &dyn SessionRepo;
    fn classifier_eval(&self) -> &dyn ClassifierEvalRepo;
    fn bakeoff_snapshots(&self) -> &dyn BakeoffSnapshotRepo;
    fn config(&self) -> &dyn ConfigRepo;
    fn rate_limits(&self) -> &dyn RateLimitRepo;
}
