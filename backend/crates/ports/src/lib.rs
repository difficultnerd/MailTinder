//! Traits only: every I/O boundary as an object-safe trait.
//!
//! The spec (T-201a) mandates exact signatures without `#[must_use]` on the
//! small value constructors, so the pedantic `must_use_candidate` lint is
//! allowed at the crate level.
#![allow(clippy::must_use_candidate, clippy::doc_markdown)]

pub mod app_folder;
pub mod classifier;
pub mod clock;
pub mod egress;
pub mod identity;
pub mod keys;
pub mod mail;
pub mod ports;
pub mod rng;
pub mod scheduler;
pub mod secrets;
pub mod store;

pub use store::{
    mailbox_id_for, AgeBucket, AuthIntent, BakeoffSnapshotRecord, BakeoffSnapshotRepo, Ciphertext,
    ClassifierEvalRecord, ClassifierEvalRepo, ClassifiersConfig, ConfigRepo, EmailLookupHash,
    EvalHeaderFacts, EvalId, EvalOutcome, HeaderRulesResult, InviteId, InviteRecord, InviteRepo,
    InviteRequestId, InviteRequestRecord, InviteRequestRepo, InviteRequestStatus, JobOutcome,
    JobOutcomeCode, JobRecord, JobRepo, Keyed, ListKeyHash, MailboxRecord, MailboxRepo,
    ModelErrorCode, ModelPrediction, NeedsAttentionId, NeedsAttentionRecord, NeedsAttentionRepo,
    Page, PageRequest, PreAuthFields, Precondition, RateLimitKey, RateLimitRecord, RateLimitRepo,
    Repo, ServerStore, SessionHash, SessionRecord, SessionRecordId, SessionRepo, SessionState,
    Sha256Hash, SnapshotId, StoreCursor, StoreError, SwipeDirection, TextTokensBucket,
    UserPseudoId, UserRecord, UserRepo, Version, Versioned, COLLECTIONS, MAX_PAGE,
};

pub use app_folder::{AppFolderError, AppFolderStore, ETag};
pub use classifier::{Classifier, ClassifierError, ClassifierId, ClassifierInput};
pub use clock::Clock;
pub use egress::{
    EgressError, EgressRequest, EgressResponse, HttpEgress, HttpMethod, OneClickOutcome,
    RefusedRange,
};
pub use identity::{AuthRequest, IdClaims, IdError, IdentityProvider, Prompt, TokenSet};
pub use keys::{Aad, KeyError, KeyService, SystemAad, SystemKeyService, WrappedKey};
pub use mail::{
    ListOrder, MailError, MailProvider, MailboxCtx, MessagePage, PageToken, ProviderCapabilities,
};
pub use ports::Ports;
pub use rng::Rng;
pub use scheduler::{CancelOutcome, JobScheduler, SchedError, TaskName};
pub use secrets::{SecretError, SecretName, Secrets};
