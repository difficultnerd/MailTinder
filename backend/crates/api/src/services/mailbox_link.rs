//! T-601a: the `link` and `reconnect` branches of the OAuth callback (S7 3.4;
//! AU-04 AC1-AC6, AU-03 AC7, INV-3).
//!
//! The generic callback in [`crate::auth::callback`] has already validated
//! `state`, exchanged the code and verified the ID token before these run.

use domain::{EmailAddress, MailboxId, MailboxStatus, Provider, ProviderSubjectId};
use obs::Sensitive;
use ports::store::aad_fields;
use ports::{Aad, Ciphertext, IdClaims, MailboxRecord, Precondition, StoreError};

use crate::auth::outcome::Outcome;
use crate::error::ApiError;
use crate::session::extract::AuthedSession;
use crate::state::AppState;

/// The outcome of a `link` or `reconnect` callback, before it is turned into a
/// redirect code.
pub enum LinkOutcome {
    /// A mailbox was linked to the current user (or refreshed for it).
    Linked,
    /// A `needs_sign_in` mailbox's grant was refreshed.
    Reconnected,
    /// The Google account is already linked to a different user (AU-04 AC3).
    MailboxLinkedElsewhere,
    /// Anything else went wrong; the new tokens are dropped.
    Failed,
}

impl LinkOutcome {
    /// The wire code in the redirect URL (S7 3.4).
    #[must_use]
    pub fn as_code(&self) -> &'static str {
        match self {
            Self::Linked => "linked",
            Self::Reconnected => "reconnected",
            Self::MailboxLinkedElsewhere => "mailbox_linked_elsewhere",
            Self::Failed => "failed",
        }
    }

    /// The callback outcome this maps to.
    #[must_use]
    pub fn outcome(&self) -> Outcome {
        match self {
            Self::Linked => Outcome::Linked,
            Self::Reconnected => Outcome::Reconnected,
            Self::MailboxLinkedElsewhere => Outcome::MailboxLinkedElsewhere,
            Self::Failed => Outcome::Failed,
        }
    }
}

/// True when a fresh grant may be written onto a mailbox in `status`.
///
/// `next_status(OAuthSucceeded)` maps every status to `connected`, so writing a
/// grant to a `consent_blocked` mailbox would silently clear the block. Only a
/// real consent fix may do that, and neither `reconnect` (S7 3.4) nor the
/// `link` "already yours" branch is one (T-601a security review F2).
fn may_apply_grant(status: MailboxStatus) -> bool {
    status != MailboxStatus::ConsentBlocked
}

/// Finish a `link` round trip (AU-04 AC1, AC2, AC3, AC6; AU-03 AC7).
///
/// The mailbox is keyed by Google `sub`, never by email. A mailbox that belongs
/// to another user is refused with nothing stored and nothing revoked (Google
/// revokes the whole grant, which would break the other user's mailbox). A
/// mailbox of the caller's own that is `consent_blocked` is refused too: linking
/// the same Google account again must not clear a consent block, the same rule
/// `reconnect` applies (T-601a security review F2).
///
/// # Errors
///
/// `ApiError::Internal` on a store or key failure.
pub async fn complete_link(
    app: &AppState,
    session: &AuthedSession,
    claims: &IdClaims,
    refresh: Sensitive<String>,
) -> Result<LinkOutcome, ApiError> {
    let Ok(sub) = ProviderSubjectId::new(claims.sub.clone()) else {
        return Ok(LinkOutcome::Failed);
    };
    match app
        .ports
        .store
        .mailboxes()
        .by_subject(Provider::Gmail, &sub)
        .await?
    {
        Some(existing) if existing.record.user_id != session.user => {
            Ok(LinkOutcome::MailboxLinkedElsewhere)
        }
        Some(existing) if !may_apply_grant(existing.record.status) => Ok(LinkOutcome::Failed),
        Some(existing) => {
            // Already one of the caller's mailboxes: refresh its grant.
            app.tokens
                .store_refresh_token(app, &session.user, &existing.record.mailbox_id, refresh)
                .await?;
            Ok(LinkOutcome::Linked)
        }
        None => create_linked_mailbox(app, session, claims, &sub, refresh).await,
    }
}

/// Create a new mailbox for `sub` with `Precondition::MustNotExist`, so two
/// racing users cannot both link the same account (INV-3).
async fn create_linked_mailbox(
    app: &AppState,
    session: &AuthedSession,
    claims: &IdClaims,
    sub: &ProviderSubjectId,
    refresh: Sensitive<String>,
) -> Result<LinkOutcome, ApiError> {
    let mailbox_id = ports::mailbox_id_for(Provider::Gmail, sub);
    let Ok(email) = EmailAddress::parse(claims.email.expose().as_str()) else {
        return Ok(LinkOutcome::Failed);
    };
    let Some(user_record) = app.ports.store.users().get(&session.user).await? else {
        return Err(ApiError::Unauthenticated);
    };
    let aad = Aad {
        user: session.user,
        scope: mailbox_id.0.to_string(),
        field: aad_fields::MAILBOX_EMAIL,
    };
    let sealed_email = app
        .ports
        .keys
        .seal(
            &session.user,
            &user_record.record.wrapped_data_key,
            &aad,
            email.as_str().as_bytes(),
        )
        .await
        .map_err(|_| ApiError::Internal)?;
    // `is_primary` only for the first mailbox (S7 5.3).
    let is_primary = app
        .ports
        .store
        .mailboxes()
        .by_user(&session.user)
        .await?
        .is_empty();
    let record = MailboxRecord {
        mailbox_id,
        user_id: session.user,
        provider: Provider::Gmail,
        provider_subject_id: sub.clone(),
        email_address: Ciphertext(sealed_email),
        // Written `needs_sign_in` with no grant: `TokenService::store_refresh_token`
        // is the single writer of the sealed token and moves the mailbox to
        // `connected` (via `next_status(OAuthSucceeded)`), so a failed grant
        // write can never leave a row claiming `connected` with no grant
        // (T-601a security review F4). A retry takes the "already yours" branch
        // below and stores the token.
        status: MailboxStatus::NeedsSignIn,
        linked_at: app.ports.clock.now(),
        is_primary,
        refresh_token: None,
    };
    match app
        .ports
        .store
        .mailboxes()
        .put(&record, Precondition::MustNotExist)
        .await
    {
        Ok(_) => {}
        Err(StoreError::AlreadyExists) => {
            // A race: re-read and apply the same two rules.
            return match app
                .ports
                .store
                .mailboxes()
                .by_subject(Provider::Gmail, sub)
                .await?
            {
                Some(existing) if existing.record.user_id != session.user => {
                    Ok(LinkOutcome::MailboxLinkedElsewhere)
                }
                Some(existing) if !may_apply_grant(existing.record.status) => {
                    Ok(LinkOutcome::Failed)
                }
                Some(existing) => {
                    app.tokens
                        .store_refresh_token(
                            app,
                            &session.user,
                            &existing.record.mailbox_id,
                            refresh,
                        )
                        .await?;
                    Ok(LinkOutcome::Linked)
                }
                None => Ok(LinkOutcome::Failed),
            };
        }
        Err(e) => return Err(e.into()),
    }
    app.tokens
        .store_refresh_token(app, &session.user, &mailbox_id, refresh)
        .await?;
    Ok(LinkOutcome::Linked)
}

/// Finish a `reconnect` round trip (ST-03 AC1).
///
/// The mailbox named in the OAuth state must still belong to the caller, be
/// the same Google account (`sub`) the round trip was started for, and be in
/// `needs_sign_in`; otherwise the new tokens are dropped and the outcome is
/// `failed`.
///
/// The status check is S7 3.4 ("refreshes the tokens of a mailbox in
/// `needs_sign_in` state"): `next_status(OAuthSucceeded)` also moves
/// `consent_blocked` to `connected`, so without it a caller could clear their
/// own consent block without a real consent fix (T-601a security review F3).
///
/// # Errors
///
/// `ApiError::Internal` on a store or key failure.
pub async fn complete_reconnect(
    app: &AppState,
    session: &AuthedSession,
    mailbox: &MailboxId,
    claims: &IdClaims,
    refresh: Sensitive<String>,
) -> Result<LinkOutcome, ApiError> {
    let Some(versioned) = app.ports.store.mailboxes().get(mailbox).await? else {
        return Ok(LinkOutcome::Failed);
    };
    if versioned.record.user_id != session.user {
        // Another user's mailbox: the new tokens are dropped and the refused
        // access is logged (V8.2.2, V16.3.2).
        crate::http::security::authz_failure(app, &session.user, None, None);
        return Ok(LinkOutcome::Failed);
    }
    if versioned.record.provider_subject_id.as_str() != claims.sub
        || versioned.record.status != MailboxStatus::NeedsSignIn
    {
        return Ok(LinkOutcome::Failed);
    }
    app.tokens
        .store_refresh_token(app, &session.user, mailbox, refresh)
        .await?;
    Ok(LinkOutcome::Reconnected)
}
