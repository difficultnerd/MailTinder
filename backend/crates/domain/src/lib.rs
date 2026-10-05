//! Entities, classes, rules and state machines. No I/O and no provider types (S3 INV-7).
//!
//! The spec (T-101) mandates exact public signatures without `#[must_use]` or
//! `# Errors` doc sections, and `HeaderFacts` legitimately carries more than
//! three bool fields. These pedantic style lints conflict with that mandated
//! API shape, so they are allowed at the crate level rather than suppressing
//! individual items.
#![allow(
    clippy::must_use_candidate,
    clippy::return_self_not_must_use,
    clippy::missing_errors_doc,
    clippy::struct_field_names,
    clippy::struct_excessive_bools
)]

mod class;
mod error;
pub mod feed;
mod gamification;
pub mod guard;
mod header_rules;
mod ids;
mod invite;
mod mailbox;
mod mailbox_status;
mod message;
mod needs_attention;
mod rules;
mod sender;
mod sender_stats;
pub mod swipe;
pub mod text;
mod tunables;
pub mod undo;
mod unsubscribe_job;

pub use class::{Classification, MessageClass, SwipeAction};
pub use error::DomainError;
pub use gamification::{
    mail_stopped_per_year, newly_unlocked, yearly_rate, AchievementId, AchievementProgress,
};
pub use guard::{header_guard, GuardNote, Guarded};
pub use header_rules::{
    HeaderRules, UnsubscribeRoute, BULK_REASON_MAX_CHARS, ESP_NAMES,
    PERSONAL_HIGH_CONFIDENCE_MAX_SCORE,
};
pub use ids::{
    CategoryId, ClassifierId, JobId, MailboxId, MessageId, RuleId, UserId, HEADER_RULES_ID,
};
pub use invite::{
    check_redemption, InviteEvent, InviteId, InviteRequestId, InviteState, InviteStatus,
    RedeemRefusal,
};
pub use mailbox::{Mailbox, MailboxIdentity, MailboxStatus, Provider, ProviderSubjectId};
pub use mailbox_status::{next_status, MailboxEvent};
pub use message::{HeaderFacts, LabelSet, MailtoTarget, MessageMeta, UnsubscribeOptions};
pub use needs_attention::{
    needs_attention_expired, NeedsAttentionExit, NeedsAttentionId, NeedsAttentionReason,
    NewNeedsAttention,
};
pub use rules::{first_match, RuleAction, RuleKind, RuleMatch, SortRule};
pub use sender::{SenderKey, SenderStats, REJECTS_KEPT, RELAY_DOMAINS};
pub use sender_stats::BlockPrompt;
pub use swipe::{
    derive_swipe_ids, plan_swipe, MailboxChange, ManualUnsubscribePlan, SwipeIds, SwipeInput,
    SwipeOutcome, SwipePlan, UnsubscribePlan, UnsubscribeTarget,
};
pub use tunables::Tunables;
pub use undo::{
    plan_undo, reverse_stats, undo_response, JobCancelOutcome, SwipeRecord, UndoPlan, UndoResponse,
    UndoStack,
};
pub use unsubscribe_job::*;
