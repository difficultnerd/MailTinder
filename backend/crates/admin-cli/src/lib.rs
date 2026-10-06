//! `mt-admin` library: the command implementations and their error types, so
//! the binary stays thin and the tests can drive the commands directly.
//!
//! The spec mandates exact public signatures without `#[must_use]` or `# Errors`
//! doc sections, so the pedantic style lints are allowed at the crate level (as
//! in `domain`, `ports` and `svc-common`).
#![allow(
    clippy::must_use_candidate,
    clippy::missing_errors_doc,
    clippy::missing_panics_doc,
    clippy::doc_markdown,
    clippy::module_name_repetitions
)]

pub mod commands;
