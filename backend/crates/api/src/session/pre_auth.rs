//! Sealing and opening the `pre_auth` field set.
//!
//! The fields exist before any user does, so they are sealed with the system
//! key service. The associated data is bound to the session's
//! `session_record_id` and the field name, so a value sealed for one session
//! cannot be opened for another (S6 5; S5 `sessions` `pre_auth` row).

use std::fmt;

use domain::MailboxId;
use obs::Sensitive;
use ports::store::aad_fields;
use ports::{
    AuthIntent, Ciphertext, KeyError, PreAuthFields, SessionRecordId, Sha256Hash, SystemAad,
    SystemKeyService,
};
use time::OffsetDateTime;

use crate::error::ApiError;

/// The clear-text `pre_auth` fields as they exist in memory during an OAuth
/// round trip. `Debug` prints the intent only.
pub struct PreAuthPlain {
    /// Why the round trip was started.
    pub intent: AuthIntent,
    /// The OAuth `state` parameter.
    pub oauth_state: Sensitive<String>,
    /// The OIDC `nonce`.
    pub nonce: Sensitive<String>,
    /// The PKCE code verifier.
    pub pkce_verifier: Sensitive<String>,
    /// The invite token hash, when the intent is `join`.
    pub invite_token_hash: Option<Sha256Hash>,
    /// The email awaiting admin approval, when the intent is a join request.
    pub pending_email: Option<Sensitive<String>>,
    /// The mailbox a `reconnect` round trip is for, when the intent is
    /// `reconnect`; `None` otherwise.
    pub mailbox_id: Option<MailboxId>,
    /// When the round trip started; the 10-minute TTL runs from here.
    pub started_at: OffsetDateTime,
}

impl fmt::Debug for PreAuthPlain {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PreAuthPlain")
            .field("intent", &self.intent)
            .field("started_at", &self.started_at)
            .finish_non_exhaustive()
    }
}

/// Seal every secret field under `SystemAad { scope: session_record_id, field }`.
///
/// # Errors
///
/// `ApiError::Internal` when the key service is unavailable.
pub async fn seal_pre_auth(
    keys: &dyn SystemKeyService,
    record_id: &SessionRecordId,
    p: &PreAuthPlain,
) -> Result<PreAuthFields, ApiError> {
    let scope = record_id.0.to_string();
    Ok(PreAuthFields {
        intent: p.intent,
        oauth_state: seal(
            keys,
            &scope,
            aad_fields::PRE_AUTH_STATE,
            p.oauth_state.expose().as_bytes(),
        )
        .await?,
        nonce: seal(
            keys,
            &scope,
            aad_fields::PRE_AUTH_NONCE,
            p.nonce.expose().as_bytes(),
        )
        .await?,
        pkce_verifier: seal(
            keys,
            &scope,
            aad_fields::PRE_AUTH_PKCE_VERIFIER,
            p.pkce_verifier.expose().as_bytes(),
        )
        .await?,
        invite_token_hash: p.invite_token_hash,
        pending_email: match &p.pending_email {
            Some(email) => Some(
                seal(
                    keys,
                    &scope,
                    aad_fields::PRE_AUTH_PENDING_EMAIL,
                    email.expose().as_bytes(),
                )
                .await?,
            ),
            None => None,
        },
        mailbox_id: p.mailbox_id,
        started_at: p.started_at,
    })
}

/// Open every secret field. A wrong scope, a tampered value or a malformed
/// ciphertext is `ApiError::Unauthenticated` (the callback's caller maps it to
/// `outcome=failed`).
///
/// # Errors
///
/// `ApiError::Unauthenticated` when a field cannot be opened;
/// `ApiError::Internal` when the key service is unavailable.
pub async fn open_pre_auth(
    keys: &dyn SystemKeyService,
    record_id: &SessionRecordId,
    f: &PreAuthFields,
) -> Result<PreAuthPlain, ApiError> {
    let scope = record_id.0.to_string();
    Ok(PreAuthPlain {
        intent: f.intent,
        oauth_state: Sensitive::new(decode(
            open(keys, &scope, aad_fields::PRE_AUTH_STATE, &f.oauth_state).await?,
        )?),
        nonce: Sensitive::new(decode(
            open(keys, &scope, aad_fields::PRE_AUTH_NONCE, &f.nonce).await?,
        )?),
        pkce_verifier: Sensitive::new(decode(
            open(
                keys,
                &scope,
                aad_fields::PRE_AUTH_PKCE_VERIFIER,
                &f.pkce_verifier,
            )
            .await?,
        )?),
        invite_token_hash: f.invite_token_hash,
        pending_email: match &f.pending_email {
            Some(ct) => Some(Sensitive::new(decode(
                open(keys, &scope, aad_fields::PRE_AUTH_PENDING_EMAIL, ct).await?,
            )?)),
            None => None,
        },
        mailbox_id: f.mailbox_id,
        started_at: f.started_at,
    })
}

fn decode(bytes: Vec<u8>) -> Result<String, ApiError> {
    String::from_utf8(bytes).map_err(|_| ApiError::Unauthenticated)
}

async fn seal(
    keys: &dyn SystemKeyService,
    scope: &str,
    field: &'static str,
    plaintext: &[u8],
) -> Result<Ciphertext, ApiError> {
    let aad = SystemAad {
        scope: scope.to_owned(),
        field,
    };
    match keys.seal(&aad, plaintext).await {
        Ok(bytes) => Ok(Ciphertext(bytes)),
        Err(_) => Err(ApiError::Internal),
    }
}

async fn open(
    keys: &dyn SystemKeyService,
    scope: &str,
    field: &'static str,
    ciphertext: &Ciphertext,
) -> Result<Vec<u8>, ApiError> {
    let aad = SystemAad {
        scope: scope.to_owned(),
        field,
    };
    match keys.open(&aad, ciphertext.0.as_slice()).await {
        Ok(bytes) => Ok(bytes),
        Err(KeyError::Unavailable) => Err(ApiError::Internal),
        Err(_) => Err(ApiError::Unauthenticated),
    }
}
