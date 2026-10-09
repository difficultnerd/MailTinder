//! Fakes, fixtures, contract suites (dev-dependency only).
//!
//! The fakes are test infrastructure; the spec (T-202b) mandates exact
//! signatures without `#[must_use]` or `# Panics`/`# Errors` doc sections, so
//! the pedantic style lints are allowed at the crate level.
#![allow(
    clippy::must_use_candidate,
    clippy::missing_panics_doc,
    clippy::missing_errors_doc,
    clippy::return_self_not_must_use,
    clippy::many_single_char_names,
    clippy::single_match,
    clippy::single_match_else,
    clippy::too_many_lines
)]

pub mod app_folder;
pub mod classifier;
pub mod clock;
pub mod contract;
pub mod corpus;
pub mod egress;
pub mod fake_caller;
pub mod fake_classifier;
pub mod fake_jev;
pub mod fake_ports;
pub mod fakes;
pub mod identity;
pub mod keys;
pub mod mailbox;
pub mod null_mail;
pub mod rng;
pub mod runner_standin;
pub mod scheduler;
pub mod secrets;
pub mod store;

pub use app_folder::InMemoryAppFolder;
pub use classifier::FakeClassifier;
pub use clock::{VirtualClock, T0};
pub use egress::{EgressRecord, FakeHttpEgress, Route};
pub use fake_caller::FakeCallerVerifier;
pub use fake_ports::{fake_ports, Fakes};
pub use fakes::invite_mailer::FakeInviteMailer;
pub use identity::FakeIdentityProvider;
pub use keys::{FakeKeyService, FakeSystemKeyService};
pub use mailbox::state::SeedMessage;
pub use mailbox::{FakeMailbox, MailOp, SentRecord};
pub use rng::SeededRng;
pub use runner_standin::{claim_and_send, RecordingEgress};
pub use scheduler::{FakeJobScheduler, SchedulerEvent, TaskState};
pub use secrets::FakeSecrets;
pub use store::InMemoryServerStore;
