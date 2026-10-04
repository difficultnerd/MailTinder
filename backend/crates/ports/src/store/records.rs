//! One record struct per Firestore collection plus nested types.

use std::fmt;

use domain::{
    InviteStatus, JobId, JobMethod, JobStatus, MailboxId, MailboxStatus, MessageClass,
    NeedsAttentionReason, Provider, ProviderSubjectId, UserId,
};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::keys::WrappedKey;
use crate::store::ids::{
    Ciphertext, EmailLookupHash, EvalId, InviteId, InviteRequestId, ListKeyHash, NeedsAttentionId,
    SessionHash, SessionRecordId, Sha256Hash, SnapshotId, UserPseudoId,
};
use crate::store::Keyed;

/// users/`{user_id}`
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UserRecord {
    pub user_id: UserId,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    pub is_admin: bool,
    pub wrapped_data_key: WrappedKey,
    pub experiments_consent_version: Option<String>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub experiments_opted_in_at: Option<OffsetDateTime>,
}

/// mailboxes/`{mailbox_id}`. Debug by hand.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MailboxRecord {
    pub mailbox_id: MailboxId,
    /// INV-3: exactly one owner, never optional.
    pub user_id: UserId,
    pub provider: Provider,
    pub provider_subject_id: ProviderSubjectId,
    pub email_address: Ciphertext,
    pub status: MailboxStatus,
    #[serde(with = "time::serde::rfc3339")]
    pub linked_at: OffsetDateTime,
    pub is_primary: bool,
    pub refresh_token: Option<Ciphertext>,
}

// The spec mandates Debug prints only the key field.
#[allow(clippy::missing_fields_in_debug)]
impl fmt::Debug for MailboxRecord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MailboxRecord")
            .field("mailbox_id", &self.mailbox_id)
            .finish()
    }
}

/// invites/`{invite_id}`. Debug by hand.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InviteRecord {
    pub invite_id: InviteId,
    pub email_address: Ciphertext,
    pub email_lookup: EmailLookupHash,
    pub token_hash: Sha256Hash,
    pub status: InviteStatus,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub last_sent_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub expires_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub purge_at: OffsetDateTime,
}

// The spec mandates Debug prints only the key field.
#[allow(clippy::missing_fields_in_debug)]
impl fmt::Debug for InviteRecord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("InviteRecord")
            .field("invite_id", &self.invite_id)
            .finish()
    }
}

/// invite_requests/`{request_id}`. Debug by hand.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InviteRequestStatus {
    Pending,
}

/// invite_requests/`{request_id}`. Debug by hand.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InviteRequestRecord {
    pub request_id: InviteRequestId,
    pub email_address: Ciphertext,
    pub email_lookup: EmailLookupHash,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    pub status: InviteRequestStatus,
}

// The spec mandates Debug prints only the key field.
#[allow(clippy::missing_fields_in_debug)]
impl fmt::Debug for InviteRequestRecord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("InviteRequestRecord")
            .field("request_id", &self.request_id)
            .finish()
    }
}

/// The outcome code of a finished unsubscribe job.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobOutcomeCode {
    OneClickAccepted,
    MailtoSent,
    Redirected,
    AddressRefused,
    HttpRejected,
    TimedOut,
    RetriesExhausted,
    TokenInvalid,
    Expired,
    Cancelled,
}

/// The outcome of a finished job.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JobOutcome {
    pub code: JobOutcomeCode,
    #[serde(with = "time::serde::rfc3339")]
    pub at: OffsetDateTime,
}

/// jobs/`{job_id}`. Derive `Debug` is fine (no sensitive names).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JobRecord {
    pub job_id: JobId,
    pub user_id: UserId,
    pub mailbox_id: MailboxId,
    pub list_key_hash: ListKeyHash,
    pub method: JobMethod,
    /// None once terminal.
    pub target: Option<Ciphertext>,
    #[serde(with = "time::serde::rfc3339")]
    pub due_at: OffsetDateTime,
    pub status: JobStatus,
    pub attempts: u32,
    pub outcome: Option<JobOutcome>,
    #[serde(with = "time::serde::rfc3339")]
    pub expires_at: OffsetDateTime,
}

/// needs_attention/`{item_id}`
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NeedsAttentionRecord {
    pub item_id: NeedsAttentionId,
    pub user_id: UserId,
    pub mailbox_id: MailboxId,
    pub sender_display: Ciphertext,
    pub link: Option<Ciphertext>,
    pub reason_code: NeedsAttentionReason,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub expires_at: OffsetDateTime,
}

// The spec mandates Debug prints only the key field.
#[allow(clippy::missing_fields_in_debug)]
impl fmt::Debug for NeedsAttentionRecord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NeedsAttentionRecord")
            .field("item_id", &self.item_id)
            .finish()
    }
}

/// The state of a session.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionState {
    PreAuth,
    PendingInviteRequest,
    Authenticated,
}

/// The intent of an auth round trip.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthIntent {
    SignIn,
    Join,
    Link,
    Reconnect,
    StepUp,
}

/// Fields present before authentication. Debug by hand.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreAuthFields {
    pub intent: AuthIntent,
    pub oauth_state: Ciphertext,
    pub nonce: Ciphertext,
    pub pkce_verifier: Ciphertext,
    pub invite_token_hash: Option<Sha256Hash>,
    pub pending_email: Option<Ciphertext>,
    #[serde(with = "time::serde::rfc3339")]
    pub started_at: OffsetDateTime,
}

// The spec mandates Debug prints only the key field.
#[allow(clippy::missing_fields_in_debug)]
impl fmt::Debug for PreAuthFields {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PreAuthFields")
            .field("intent", &self.intent)
            .field("started_at", &self.started_at)
            .finish()
    }
}

/// sessions/`{session_hash hex}`. Debug by hand.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionRecord {
    pub session_hash: SessionHash,
    pub session_record_id: SessionRecordId,
    pub state: SessionState,
    pub user_id: Option<UserId>,
    pub csrf_token: String,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub last_seen_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339::option")]
    pub recent_auth_at: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339")]
    pub expires_at: OffsetDateTime,
    pub pre_auth: Option<PreAuthFields>,
}

// The spec mandates Debug prints only the key field.
#[allow(clippy::missing_fields_in_debug)]
impl fmt::Debug for SessionRecord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SessionRecord")
            .field("session_record_id", &self.session_record_id)
            .finish()
    }
}

/// A swipe direction.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SwipeDirection {
    Left,
    Right,
    Up,
    Down,
}

/// An age bucket for the bake-off.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum AgeBucket {
    #[serde(rename = "<7d")]
    Under7d,
    #[serde(rename = "7d-90d")]
    D7To90d,
    #[serde(rename = "90d-1y")]
    D90To1y,
    #[serde(rename = "1y-5y")]
    Y1To5y,
    #[serde(rename = ">5y")]
    Over5y,
}

/// A text-token bucket for the bake-off.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TextTokensBucket {
    #[serde(rename = "<100")]
    Under100,
    #[serde(rename = "100-300")]
    T100To300,
    #[serde(rename = ">300")]
    Over300,
}

/// A model error code.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelErrorCode {
    Timeout,
    Http,
    InvalidOutput,
    Disabled,
    Unavailable,
}

/// The header-rules result for a bake-off eval.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HeaderRulesResult {
    pub class: MessageClass,
    pub score: u8,
}

/// A model prediction for a bake-off eval.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelPrediction {
    pub class: Option<MessageClass>,
    pub score: Option<u8>,
    pub confidence: Option<f32>,
    pub probabilities: Option<[f32; 5]>,
    pub model_version: String,
    pub latency_ms: u32,
    pub input_tokens: Option<u32>,
    pub error_code: Option<ModelErrorCode>,
}

/// The user's swipe outcome for a bake-off eval.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvalOutcome {
    pub direction: SwipeDirection,
    pub undone: bool,
    pub time_to_swipe_ms: u32,
}

/// Header facts for a bake-off eval. Booleans only.
#[allow(clippy::struct_excessive_bools)]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvalHeaderFacts {
    pub has_list_unsubscribe: bool,
    pub dkim_covers_list_unsubscribe: bool,
    pub has_list_unsubscribe_post: bool,
    pub has_list_id: bool,
    pub has_feedback_id: bool,
    pub precedence_bulk: bool,
    pub auto_submitted: bool,
    pub from_authenticated: bool,
}

/// classifier_eval/`{eval_id}`
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClassifierEvalRecord {
    pub eval_id: EvalId,
    pub user_pseudo_id: UserPseudoId,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub expires_at: OffsetDateTime,
    pub header_rules: HeaderRulesResult,
    pub gemini: Option<ModelPrediction>,
    pub jev: Option<ModelPrediction>,
    pub outcome: EvalOutcome,
    pub header_facts: EvalHeaderFacts,
    pub provider: Provider,
    pub age_bucket: AgeBucket,
    pub text_tokens_bucket: TextTokensBucket,
    pub lang_is_english: bool,
    pub input_version: String,
    pub question_version: String,
    pub price_version: String,
}

/// bakeoff_snapshots/`{snapshot_id}`
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BakeoffSnapshotRecord {
    pub snapshot_id: SnapshotId,
    pub name: String,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    pub labelled_swipes: u64,
    pub query_json: String,
    pub report_json: String,
}

/// `config/classifiers` (the only config document).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClassifiersConfig {
    pub gemini_enabled: bool,
    pub jev_enabled: bool,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

/// `rate_limits/{key}:{window_start unix seconds}`
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RateLimitRecord {
    pub key: String,
    #[serde(with = "time::serde::rfc3339", rename = "window_start_at")]
    pub window_start: OffsetDateTime,
    pub count: u32,
    #[serde(with = "time::serde::rfc3339")]
    pub expires_at: OffsetDateTime,
}

impl Keyed for UserRecord {
    type Key = UserId;
    fn key(&self) -> UserId {
        self.user_id
    }
}
impl Keyed for MailboxRecord {
    type Key = MailboxId;
    fn key(&self) -> MailboxId {
        self.mailbox_id
    }
}
impl Keyed for InviteRecord {
    type Key = InviteId;
    fn key(&self) -> InviteId {
        self.invite_id
    }
}
impl Keyed for InviteRequestRecord {
    type Key = InviteRequestId;
    fn key(&self) -> InviteRequestId {
        self.request_id
    }
}
impl Keyed for JobRecord {
    type Key = JobId;
    fn key(&self) -> JobId {
        self.job_id
    }
}
impl Keyed for NeedsAttentionRecord {
    type Key = NeedsAttentionId;
    fn key(&self) -> NeedsAttentionId {
        self.item_id
    }
}
impl Keyed for SessionRecord {
    type Key = SessionHash;
    fn key(&self) -> SessionHash {
        self.session_hash
    }
}
impl Keyed for ClassifierEvalRecord {
    type Key = EvalId;
    fn key(&self) -> EvalId {
        self.eval_id
    }
}
impl Keyed for BakeoffSnapshotRecord {
    type Key = SnapshotId;
    fn key(&self) -> SnapshotId {
        self.snapshot_id
    }
}

/// A fixed namespace for deriving mailbox IDs from provider + subject.
pub const MAILBOX_NAMESPACE: Uuid = Uuid::from_u128(0x6ba7_b810_9dad_11d1_80b4_00c0_4fd4_30c8);

/// A deterministic mailbox ID from provider + subject, so uniqueness on
/// (provider, provider_subject_id) needs no second collection.
pub fn mailbox_id_for(provider: Provider, subject: &ProviderSubjectId) -> MailboxId {
    let provider_str = match provider {
        Provider::Gmail => "gmail",
    };
    let key = format!("{provider_str}:{}", subject.as_str());
    MailboxId(Uuid::new_v5(&MAILBOX_NAMESPACE, key.as_bytes()))
}
