//! Shared service code used by `api`, `unsub` and `worker`, so there is one
//! implementation of internal caller checks, token minting and Needs Attention
//! items (T-503, T-701).
//!
//! This crate depends only on `domain`, `ports` and `obs`: it must never
//! depend on `api`, `unsub` or `worker` (T-503 trap).
//!
//! The spec mandates exact public signatures without `#[must_use]` or
//! `# Errors` doc sections, so the pedantic style lints are allowed at the
//! crate level (as in `domain` and `ports`).
#![allow(
    clippy::must_use_candidate,
    clippy::missing_errors_doc,
    clippy::missing_panics_doc,
    clippy::doc_markdown,
    clippy::module_name_repetitions
)]

pub mod invites;
pub mod links;
pub mod mint;
