//! Refresh-token sealing and opening, and on-demand access-token minting.
//!
//! A refresh token is stored only as AEAD ciphertext under the owning user's
//! KMS-wrapped `data_key`, bound to the user, the mailbox and the field
//! (V11.3.3). An access token is minted on demand from the stored refresh
//! token and never stored anywhere (UN-01 AC4, JOB-1, V10.1.1). A revoked or
//! invalid refresh token moves the mailbox to `needs_sign_in` and is logged
//! (UN-01 AC6, ST-03 AC1, V7.6.1).

use domain::{next_status, MailboxEvent, MailboxId, MailboxStatus, UserId};
use obs::{security_event, Pseudonymiser, SecurityEvent, Sensitive};
use ports::store::{aad_fields, Ciphertext, MailboxRecord, Precondition, UserRecord, Version};
use ports::{Aad, IdError, KeyError, MailboxCtx, Ports, SecretName};

/// A minting error. `Revoked` is the only variant that mutates state.
#[derive(Debug, thiserror::Error)]
pub enum MintError {
    /// `invalid_grant` from the identity provider: the mailbox is set to
    /// `needs_sign_in` (the refresh token is kept for reconnect).
    #[error("refresh token revoked or invalid")]
    Revoked,
    /// The user or mailbox is missing, or the mailbox is not the user's.
    #[error("mailbox missing")]
    MailboxMissing,
    /// The identity provider or the store is temporarily unavailable.
    #[error("transient")]
    Transient,
    /// A key-service failure; never retried silently.
    #[error("crypto")]
    Crypto,
}

/// Seals `token` as the mailbox's refresh token. Associated data is
/// `{ user, scope: mailbox_id string, field: MAILBOX_REFRESH_TOKEN }`; using
/// the user ID alone would let a token be swapped between a user's mailboxes.
pub async fn seal_refresh_token(
    ports: &Ports,
    user: &UserRecord,
    mailbox: &MailboxId,
    token: &Sensitive<String>,
) -> Result<Ciphertext, MintError> {
    let aad = Aad {
        user: user.user_id,
        scope: mailbox.0.to_string(),
        field: aad_fields::MAILBOX_REFRESH_TOKEN,
    };
    let sealed = ports
        .keys
        .seal(
            &user.user_id,
            &user.wrapped_data_key,
            &aad,
            token.expose().as_bytes(),
        )
        .await
        .map_err(|_| MintError::Crypto)?;
    Ok(Ciphertext(sealed))
}

/// Opens the mailbox's stored refresh token under the user's live `data_key`.
/// A missing token is `Revoked` (there is nothing to try).
pub async fn open_refresh_token(
    ports: &Ports,
    user: &UserRecord,
    mailbox: &MailboxRecord,
) -> Result<Sensitive<String>, MintError> {
    let Some(ciphertext) = mailbox.refresh_token.as_ref() else {
        return Err(MintError::Revoked);
    };
    let aad = Aad {
        user: user.user_id,
        scope: mailbox.mailbox_id.0.to_string(),
        field: aad_fields::MAILBOX_REFRESH_TOKEN,
    };
    let plaintext = ports
        .keys
        .open(&user.user_id, &user.wrapped_data_key, &aad, &ciphertext.0)
        .await
        .map_err(|e| map_open_error(&e))?;
    let token = String::from_utf8(plaintext).map_err(|_| MintError::Crypto)?;
    Ok(Sensitive::new(token))
}

/// Loads the user and mailbox, opens the refresh token and calls
/// `IdentityProvider::refresh`. Never stores the access token.
///
/// On `invalid_grant` the mailbox is moved to `needs_sign_in` with a
/// conditional write (a lost race is ignored) and a security event is logged;
/// only `invalid_grant` changes the status, so a network timeout never wrongly
/// asks the user to sign in again.
pub async fn mint_access_token(
    ports: &Ports,
    user: &UserId,
    mailbox: &MailboxId,
) -> Result<MailboxCtx, MintError> {
    let user_record = ports
        .store
        .users()
        .get(user)
        .await
        .map_err(|_| MintError::Transient)?
        .ok_or(MintError::MailboxMissing)?;
    let versioned = ports
        .store
        .mailboxes()
        .get(mailbox)
        .await
        .map_err(|_| MintError::Transient)?
        .ok_or(MintError::MailboxMissing)?;
    let record = versioned.record;
    // INV-3: a token is opened only for the mailbox's own user. A mailbox ID
    // from a request body is untrusted.
    if record.user_id != *user {
        return Err(MintError::MailboxMissing);
    }
    if record.refresh_token.is_none() || record.status == MailboxStatus::NeedsSignIn {
        return Err(MintError::Revoked);
    }
    let token = open_refresh_token(ports, &user_record.record, &record).await?;
    match ports.identity.refresh(&token).await {
        Ok(access) => Ok(MailboxCtx {
            mailbox: *mailbox,
            access_token: access,
        }),
        Err(IdError::InvalidGrant) => {
            mark_needs_sign_in(ports, &record, versioned.version).await;
            log_sign_in_required(ports, user).await;
            Err(MintError::Revoked)
        }
        Err(_) => Err(MintError::Transient),
    }
}

/// Moves the mailbox to `needs_sign_in` with a version precondition. A
/// `PreconditionFailed` means someone else already changed it, which is fine.
async fn mark_needs_sign_in(ports: &Ports, record: &MailboxRecord, version: Version) {
    let mut updated = record.clone();
    updated.status = next_status(record.status, MailboxEvent::TokenInvalid);
    let _ = ports
        .store
        .mailboxes()
        .put(&updated, Precondition::Matches(version))
        .await;
}

/// Logs a security event for a revoked grant. The user ID appears only as a
/// pseudonym; if the pseudonymisation key is unavailable the event is still
/// written, without a user.
async fn log_sign_in_required(ports: &Ports, user: &UserId) {
    let pseudo = match ports.secrets.get(SecretName::LogPseudonymHmacKey).await {
        Ok(key) => Some(Pseudonymiser::new(key).pseudo_id(&user.0)),
        Err(_) => None,
    };
    security_event(&SecurityEvent {
        action: "sign_in_required",
        outcome: "token_invalid",
        user: pseudo,
        request_id: None,
        amr: None,
        provider: Some("gmail"),
        method: None,
    });
}

/// `Unavailable` is transient; every other key error (wrong key, AAD, tag or
/// malformed ciphertext) is a hard crypto failure.
fn map_open_error(e: &KeyError) -> MintError {
    match e {
        KeyError::Unavailable => MintError::Transient,
        _ => MintError::Crypto,
    }
}
