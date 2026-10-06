//! Service modules that several route handlers share.
//!
//! One module per cross-route concern; a route file stays thin (T-601a).

pub mod feed;
pub mod jobs;
pub mod mailbox_disconnect;
pub mod mailbox_link;
pub mod progress;
pub mod user_state_store;
