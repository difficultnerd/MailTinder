//! Traits only: every I/O boundary as an object-safe trait.
//!
//! The spec (T-201a) mandates exact signatures without `#[must_use]` on the
//! small value constructors, so the pedantic `must_use_candidate` lint is
//! allowed at the crate level.
#![allow(clippy::must_use_candidate)]

pub mod app_folder;
pub mod classifier;
pub mod clock;
pub mod egress;
pub mod identity;
pub mod keys;
pub mod mail;
pub mod rng;
pub mod scheduler;
pub mod secrets;
// T-201b adds store

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
pub use rng::Rng;
pub use scheduler::{CancelOutcome, JobScheduler, SchedError, TaskName};
pub use secrets::{SecretError, SecretName, Secrets};
