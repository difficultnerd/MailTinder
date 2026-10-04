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
mod ids;
mod mailbox;
mod message;
mod sender;
mod tunables;

pub use class::{Classification, MessageClass, SwipeAction};
pub use error::DomainError;
pub use ids::{
    CategoryId, ClassifierId, JobId, MailboxId, MessageId, RuleId, UserId, HEADER_RULES_ID,
};
pub use mailbox::{Mailbox, MailboxIdentity, MailboxStatus, Provider, ProviderSubjectId};
pub use message::{HeaderFacts, LabelSet, MailtoTarget, MessageMeta, UnsubscribeOptions};
pub use sender::{SenderKey, SenderStats, REJECTS_KEPT, RELAY_DOMAINS};
pub use tunables::Tunables;
