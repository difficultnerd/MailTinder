//! Service modules that several route handlers share.
//!
//! One module per cross-route concern; a route file stays thin (T-601a).

pub mod account_deletion;
pub mod categories;
pub mod feed;
pub mod jobs;
pub mod list_key;
pub mod mailbox_disconnect;
pub mod mailbox_link;
pub mod progress;
pub mod reject;
pub mod rules;
pub mod swipe;
pub mod undo;
pub mod user_state_store;
