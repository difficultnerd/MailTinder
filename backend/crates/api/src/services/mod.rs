//! Service modules that several route handlers share.
//!
//! One module per cross-route concern; a route file stays thin (T-601a).

pub mod account_deletion;
pub mod categories;
pub mod delivery_check;
pub mod feed;
pub mod history_catch_up;
pub mod jobs;
pub mod list_key;
pub mod mailbox_disconnect;
pub mod mailbox_link;
pub mod progress;
pub mod reject;
pub mod rule_actions;
pub mod rules;
pub mod swipe;
pub mod undo;
pub mod user_state_store;
