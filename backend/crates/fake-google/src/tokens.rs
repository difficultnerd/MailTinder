//! Bearer token registry and scope checks for `fake-google`.

use std::collections::HashSet;

use time::OffsetDateTime;

use super::state::FakeMailboxKey;

/// A minted bearer token.
#[derive(Clone, Debug)]
pub struct TokenRecord {
    pub mailbox: FakeMailboxKey,
    pub scopes: HashSet<String>,
    pub expires_at: OffsetDateTime,
}

/// The token registry lives inside the shared state; this module only defines
/// the record type and the scope constants.
pub const GMAIL_MODIFY: &str = "https://www.googleapis.com/auth/gmail.modify";
pub const GMAIL_SEND: &str = "https://www.googleapis.com/auth/gmail.send";
pub const DRIVE_APPDATA: &str = "https://www.googleapis.com/auth/drive.appdata";
