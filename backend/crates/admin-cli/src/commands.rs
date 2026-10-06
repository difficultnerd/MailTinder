//! The three `mt-admin` commands, over the port traits so tests use testkit
//! fakes (T-507).
//!
//! The API never grants the admin flag (S7 3.7); this tool is the only place
//! that does. The raw invite token lives only inside `invite` and inside the
//! link it returns.

use domain::{EmailAddress, UserId};
use obs::{security_event, Pseudonymiser, SecurityEvent, Sensitive};
use ports::{
    Clock, KeyError, PageRequest, Precondition, Rng, ServerStore, StoreError, SystemKeyService,
};
use svc_common::invites::{upsert_pending_invite, UpsertError, Upserted};
use time::format_description::well_known::Rfc3339;
use time::{OffsetDateTime, UtcOffset};
use url::Url;

/// A command failure. Exit codes come from `CliError::exit_code`.
#[derive(Debug, thiserror::Error)]
pub enum CliError {
    /// The user does not exist (or, for `make_admin`, the write clashed twice).
    #[error("not found")]
    NotFound,
    /// The store rejected the read or write.
    #[error("store: {0}")]
    Store(#[from] StoreError),
    /// The system key failed.
    #[error("key: {0}")]
    Key(#[from] KeyError),
    /// The address or user ID on the command line is malformed.
    #[error("bad input")]
    BadInput,
}

impl CliError {
    /// 0 success, 2 bad input, 3 not found, 1 anything else.
    #[must_use]
    pub fn exit_code(&self) -> u8 {
        match self {
            CliError::BadInput => 2,
            CliError::NotFound => 3,
            CliError::Store(_) | CliError::Key(_) => 1,
        }
    }
}

impl From<UpsertError> for CliError {
    fn from(error: UpsertError) -> Self {
        match error {
            UpsertError::Store(e) => CliError::Store(e),
            UpsertError::Key(e) => CliError::Key(e),
        }
    }
}

/// Everything a command needs. Takes ports only, so tests use testkit fakes.
pub struct Deps<'a> {
    /// Firestore in production, the in-memory store in tests.
    pub store: &'a dyn ServerStore,
    /// The `system-fields` KMS key that seals invite addresses (S6 5).
    pub system_keys: &'a dyn SystemKeyService,
    /// The email lookup HMAC key from Secret Manager (T-305).
    pub email_lookup_key: &'a Sensitive<Vec<u8>>,
    /// The system clock, the only source of time.
    pub clock: &'a dyn Clock,
    /// The OS random source, the only source of randomness.
    pub rng: &'a dyn Rng,
    /// The app origin used to build the invite link.
    pub app_origin: &'a Url,
    /// The log pseudonymisation key, for the `admin_action` security event.
    pub pseudo_key: &'a Sensitive<Vec<u8>>,
}

/// Create or refresh the pending invite for `email`; returns the invite link.
/// Sends no email. The raw token is dropped as soon as the link is built.
///
/// # Errors
///
/// `CliError::Store` or `CliError::Key` on a failure; `CliError::BadInput` if
/// the built link is not a valid URL (it always is).
pub async fn invite(d: &Deps<'_>, email: &EmailAddress) -> Result<Url, CliError> {
    let (_record, raw, upserted) = upsert_pending_invite(
        d.store,
        d.system_keys,
        d.email_lookup_key,
        d.clock,
        d.rng,
        email,
    )
    .await?;
    let origin = d.app_origin.as_str();
    let token = raw.expose().as_str();
    let link =
        Url::parse(&format!("{origin}/#/invite?t={token}")).map_err(|_| CliError::BadInput)?;
    security_event(&SecurityEvent {
        action: "invite_create",
        outcome: match upserted {
            Upserted::Created => "created",
            Upserted::Resent => "resent",
        },
        user: None,
        request_id: None,
        amr: None,
        provider: None,
        method: None,
    });
    Ok(link)
}

/// One user row: user ID, creation time, admin flag and mailbox count. It never
/// holds an email address.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UserLine {
    pub user_id: UserId,
    /// RFC 3339 UTC.
    pub created_at: String,
    pub is_admin: bool,
    pub mailbox_count: usize,
}

impl UserLine {
    /// The fixed-width line printed to stdout.
    #[must_use]
    pub fn line(&self) -> String {
        format!(
            "{:<36}  {:<20}  {:<5}  {}",
            self.user_id, self.created_at, self.is_admin, self.mailbox_count
        )
    }
}

/// One line per user: user_id, created_at, is_admin, mailbox_count. No email
/// addresses.
///
/// # Errors
///
/// `CliError::Store` on a read failure; `CliError::BadInput` if a timestamp
/// cannot be rendered (it always can).
pub async fn list_users(d: &Deps<'_>) -> Result<Vec<UserLine>, CliError> {
    /// The largest page the store allows.
    const PAGE: u32 = 100;
    let mut users = Vec::new();
    let mut after = None;
    loop {
        let page = d
            .store
            .users()
            .list(PageRequest { limit: PAGE, after })
            .await?;
        for item in &page.items {
            let mailboxes = d.store.mailboxes().by_user(&item.record.user_id).await?;
            users.push(UserLine {
                user_id: item.record.user_id,
                created_at: rfc3339(item.record.created_at)?,
                is_admin: item.record.is_admin,
                mailbox_count: mailboxes.len(),
            });
        }
        match page.next {
            Some(next) => after = Some(next),
            None => break,
        }
    }
    Ok(users)
}

/// What `make_admin` did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MakeAdminResult {
    /// The flag was set.
    Granted,
    /// The user was already an admin; nothing changed.
    AlreadyAdmin,
}

/// Set `is_admin = true` on that user. Refuses if the user does not exist.
///
/// # Errors
///
/// `CliError::NotFound` if the user does not exist; `CliError::Store` on a read
/// or write failure (the version clash retry once).
pub async fn make_admin(d: &Deps<'_>, user: &UserId) -> Result<MakeAdminResult, CliError> {
    let found = d.store.users().get(user).await?.ok_or(CliError::NotFound)?;
    if found.record.is_admin {
        return Ok(MakeAdminResult::AlreadyAdmin);
    }
    let mut record = found.record;
    record.is_admin = true;
    match d
        .store
        .users()
        .put(&record, Precondition::Matches(found.version))
        .await
    {
        Ok(_) => {}
        Err(StoreError::PreconditionFailed) => {
            // Someone changed the user between the read and the write; retry once.
            let again = d.store.users().get(user).await?.ok_or(CliError::NotFound)?;
            if again.record.is_admin {
                return Ok(MakeAdminResult::AlreadyAdmin);
            }
            let mut record = again.record;
            record.is_admin = true;
            d.store
                .users()
                .put(&record, Precondition::Matches(again.version))
                .await?;
        }
        Err(e) => return Err(e.into()),
    }
    security_event(&SecurityEvent {
        action: "admin_action",
        outcome: "admin_granted",
        user: Some(Pseudonymiser::new(d.pseudo_key.clone()).pseudo_id(&user.0)),
        request_id: None,
        amr: None,
        provider: None,
        method: None,
    });
    Ok(MakeAdminResult::Granted)
}

/// RFC 3339 UTC.
fn rfc3339(at: OffsetDateTime) -> Result<String, CliError> {
    at.to_offset(UtcOffset::UTC)
        .format(&Rfc3339)
        .map_err(|_| CliError::BadInput)
}
