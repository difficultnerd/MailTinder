//! Shared invite creation and lookup, used by the API admin invite routes
//! (T-505) and the `mt-admin` bootstrap tool (T-507).
//!
//! This is steps 1 to 5 of T-505 `issue_invite`, moved here unchanged: the raw
//! token exists only in this module, is hashed for the record and returned to
//! the caller, which builds the link and drops it. Sending the email stays in
//! `api` (the CLI sends none).

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use domain::{EmailAddress, InviteStatus};
use hmac::{Hmac, Mac};
use obs::Sensitive;
use ports::store::aad_fields;
use ports::{
    Ciphertext, Clock, EmailLookupHash, InviteId, InviteRecord, KeyError, Precondition, Rng,
    ServerStore, Sha256Hash, StoreError, SystemAad, SystemKeyService,
};
use sha2::{Digest, Sha256};
use time::Duration;

/// How long an invite link works `[TUNABLE]` (S2 glossary).
pub const INVITE_TTL: Duration = Duration::days(7);
/// How long a finished invite record is kept before the sweeper purges it (S5).
pub const INVITE_PURGE_AFTER: Duration = Duration::days(30);

/// Which branch `upsert_pending_invite` took.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Upserted {
    /// No pending invite existed; a new record was written.
    Created,
    /// A pending invite existed; it was refreshed with a new token.
    Resent,
}

/// A failure creating or refreshing an invite record.
#[derive(Debug, thiserror::Error)]
pub enum UpsertError {
    /// The store rejected the read or write.
    #[error("store: {0}")]
    Store(#[from] StoreError),
    /// The system key could not seal the address.
    #[error("key: {0}")]
    Key(#[from] KeyError),
}

/// Create or refresh the pending invite for `email`. Returns the stored record,
/// the raw token (the caller builds the link, then drops it) and which branch
/// ran. Sends no email.
///
/// # Errors
///
/// `UpsertError::Store` on a read or write failure; `UpsertError::Key` when the
/// system key cannot seal the address.
pub async fn upsert_pending_invite(
    store: &dyn ServerStore,
    system_keys: &dyn SystemKeyService,
    email_lookup_key: &Sensitive<Vec<u8>>,
    clock: &dyn Clock,
    rng: &dyn Rng,
    email: &EmailAddress,
) -> Result<(InviteRecord, Sensitive<String>, Upserted), UpsertError> {
    let now = clock.now();
    let hash = email_lookup_hash(email_lookup_key, email);
    let existing = store
        .invites()
        .by_email_lookup(&hash)
        .await?
        .into_iter()
        .find(|v| v.record.status == InviteStatus::Pending);

    let raw = URL_SAFE_NO_PAD.encode(rng.bytes32());
    let token_hash = hash_token(&raw);
    let expires_at = now + INVITE_TTL;
    let purge_at = expires_at + INVITE_PURGE_AFTER;

    let (record, upserted) = if let Some(found) = existing {
        let mut record = found.record;
        record.token_hash = token_hash;
        record.last_sent_at = now;
        record.expires_at = expires_at;
        record.purge_at = purge_at;
        store
            .invites()
            .put(&record, Precondition::Matches(found.version))
            .await?;
        (record, Upserted::Resent)
    } else {
        let invite_id = InviteId(rng.uuid_v4());
        let sealed = system_keys
            .seal(
                &SystemAad {
                    scope: invite_id.0.to_string(),
                    field: aad_fields::INVITE_EMAIL,
                },
                email.as_str().as_bytes(),
            )
            .await?;
        let record = InviteRecord {
            invite_id,
            email_address: Ciphertext(sealed),
            email_lookup: hash,
            token_hash,
            status: InviteStatus::Pending,
            created_at: now,
            last_sent_at: now,
            expires_at,
            purge_at,
        };
        store
            .invites()
            .put(&record, Precondition::MustNotExist)
            .await?;
        (record, Upserted::Created)
    };

    Ok((record, Sensitive::new(raw), upserted))
}

/// HMAC-SHA-256 under `email_lookup_key` of the address's lower-case
/// `lookup_form()`. Only the hash is stored or compared, so an invite row never
/// holds a readable address.
#[must_use]
pub fn email_lookup_hash(key: &Sensitive<Vec<u8>>, email: &EmailAddress) -> EmailLookupHash {
    match Hmac::<Sha256>::new_from_slice(key.expose()) {
        Ok(mut mac) => {
            mac.update(email.lookup_form().as_bytes());
            let digest = mac.finalize().into_bytes();
            let mut out = [0u8; 32];
            out.copy_from_slice(&digest);
            EmailLookupHash(out)
        }
        // HMAC accepts every key length, so this is unreachable; a zeroed hash
        // simply matches nothing rather than panicking in a request path.
        Err(_) => EmailLookupHash([0u8; 32]),
    }
}

/// SHA-256 of the 43-character token string, exactly as the redemption path
/// hashes the presented token.
#[must_use]
pub fn hash_token(raw: &str) -> Sha256Hash {
    let digest = Sha256::digest(raw.as_bytes());
    let mut out = [0u8; 32];
    out.copy_from_slice(&digest);
    Sha256Hash(out)
}
