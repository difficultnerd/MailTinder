//! T-803: API-ACCT-1, delete the account (S7 5.10; S2 AU-06 AC1-AC3; S5 DEL-1,
//! DEL-2).
//!
//! The order is the point: each step needs what the next one removes. The app
//! folder file goes first (it needs a token), then queued jobs and their Cloud
//! Tasks, then the provider grants, then the user record with its wrapped
//! `data_key` (crypto-shredding), then every other record, then the sessions.
//! Addresses and tokens are opened before the user record goes; afterwards
//! nothing of this user can be decrypted, by design.

use domain::{JobStatus, MailboxId, UserId};
use obs::{Pseudonymiser, SecurityEvent, Sensitive};
use ports::store::aad_fields;
use ports::{Aad, MailboxCtx, Precondition, UserPseudoId, UserRecord};

use crate::auth::step_up::require_step_up;
use crate::error::ApiError;
use crate::routes::account::{DeletionAccepted, NotDeleted};
use crate::services::jobs::cancel_queued_job;
use crate::session::extract::AuthedSession;
use crate::state::AppState;

/// Hours until the worker sweep has finished any leftovers (S2 AU-06 AC1).
pub const DELETION_SWEEP_HOURS: i64 = 24;

/// Every stage the deletion runs, in order. Tests compare this to the fakes'
/// recorded order.
#[derive(Debug, PartialEq, Eq)]
pub enum DeletionStep {
    /// Every mailbox's app folder file was asked to go.
    AppFolderDeleted,
    /// Every queued job was cancelled with its Cloud Task.
    JobsCancelled,
    /// Every refresh token was revoked at the provider.
    TokensRevoked,
    /// The user record, and with it the wrapped `data_key`, is gone.
    KeyDestroyed,
    /// Jobs, Needs Attention items, mailboxes and evaluation records are gone.
    RecordsDeleted,
    /// Every session of the user is gone.
    SessionsEnded,
}

/// What step 3 read while the key still existed. Held in memory for this
/// request only.
struct Prepared {
    mailbox_id: MailboxId,
    is_primary: bool,
    address: String,
    ctx: Option<MailboxCtx>,
    refresh: Option<Sensitive<String>>,
}

/// Delete the signed-in user's account.
///
/// # Errors
///
/// `ApiError::StepUpRequired` without a fresh sign-in, before anything is read
/// or deleted; `Unauthenticated` when the user is already gone;
/// `ProviderUnavailable` when the user record cannot be deleted (nothing
/// further changes and a retry is safe); `Internal` on a store failure while
/// loading. A provider or later store failure never fails the request.
pub async fn delete_account(
    app: &AppState,
    session: &AuthedSession,
) -> Result<(DeletionAccepted, Vec<DeletionStep>), ApiError> {
    // 1. A fresh sign-in; otherwise nothing is deleted (AU-06 AC3).
    require_step_up(session, app.ports.clock.as_ref())?;
    let user = session.user;
    let mut steps = Vec::new();

    // 2. The start is on record before anything changes.
    security_event(app, "started", &user);

    // 3. Open what the later steps need while the key still exists.
    let user_record = app
        .ports
        .store
        .users()
        .get(&user)
        .await?
        .ok_or(ApiError::Unauthenticated)?;
    let mut prepared = prepare(app, &user, &user_record.record).await?;
    // The primary mailbox's file first, then any stale copy (AU-05 AC4).
    prepared.sort_by_key(|p| !p.is_primary);

    // 4. App folder files. A failure or a missing token lists the mailbox.
    let mut not_deleted = Vec::new();
    for p in &prepared {
        let deleted = match &p.ctx {
            Some(ctx) => app.ports.app_folder.delete(ctx).await.is_ok(),
            None => false,
        };
        if !deleted {
            not_deleted.push(NotDeleted {
                mailbox_id: p.mailbox_id.0,
                email_address: p.address.clone(),
            });
        }
    }
    steps.push(DeletionStep::AppFolderDeleted);

    // 5. Queued jobs and their Cloud Tasks. A running job is left to finish:
    //    its runner finds no mailbox and no key and sends nothing.
    for p in &prepared {
        let Ok(jobs) = app.ports.store.jobs().by_mailbox(&p.mailbox_id).await else {
            continue;
        };
        for job in jobs {
            if job.record.status == JobStatus::Queued {
                // The sweep removes the record either way.
                let _ = cancel_queued_job(app, &job.record.job_id).await;
            }
        }
    }
    steps.push(DeletionStep::JobsCancelled);

    // 6. Revoke every grant; a failure is logged and the deletion goes on.
    for p in &prepared {
        if let Some(token) = &p.refresh {
            if app.ports.identity.revoke(token).await.is_err() {
                security_event(app, "revoke_failed", &user);
            }
        }
        app.tokens.forget(&p.mailbox_id);
    }
    steps.push(DeletionStep::TokensRevoked);

    // 7. Destroy the key: no encrypted field of this user opens after this.
    if app
        .ports
        .store
        .users()
        .delete(&user, Precondition::None)
        .await
        .is_err()
    {
        return Err(ApiError::ProviderUnavailable {
            mailbox_id: None,
            retry_after_s: None,
        });
    }
    steps.push(DeletionStep::KeyDestroyed);

    // 8. Everything else. Failures are left for the sweep, which finds a user's
    //    leftovers through their jobs, items and sessions. So if the mailboxes
    //    fail, jobs and items stay as the sweep's trail.
    let store = app.ports.store.as_ref();
    if store.mailboxes().delete_all_for_user(&user).await.is_ok() {
        let _ = store.jobs().delete_all_for_user(&user).await;
        let _ = store.needs_attention().delete_all_for_user(&user).await;
    }
    let pseudo = Pseudonymiser::new(app.config.rate_key.clone()).pseudo_id(&user.0);
    let _ = store
        .classifier_eval()
        .delete_for_users(&[UserPseudoId(pseudo.as_str().to_owned())])
        .await;
    steps.push(DeletionStep::RecordsDeleted);

    // 9. Every session of the user (ASVS V7.4.2); the handler clears the cookie.
    let _ = store.sessions().delete_all_for_user(&user).await;
    steps.push(DeletionStep::SessionsEnded);

    // 10. One pseudonymous event, then the answer.
    security_event(app, "deleted", &user);
    let accepted = DeletionAccepted {
        deletion_due_by: app.ports.clock.now() + time::Duration::hours(DELETION_SWEEP_HOURS),
        app_folders_not_deleted: not_deleted,
    };
    Ok((accepted, steps))
}

/// Step 3: open each mailbox's address and refresh token, and mint its access
/// context, while the key still exists. A failure for one mailbox leaves that
/// piece empty; it never stops the deletion.
async fn prepare(
    app: &AppState,
    user: &UserId,
    user_record: &UserRecord,
) -> Result<Vec<Prepared>, ApiError> {
    let mut prepared = Vec::new();
    for mailbox in app.ports.store.mailboxes().by_user(user).await? {
        let id = mailbox.record.mailbox_id;
        let aad = Aad {
            user: *user,
            scope: id.0.to_string(),
            field: aad_fields::MAILBOX_EMAIL,
        };
        let address = app
            .ports
            .keys
            .open(
                user,
                &user_record.wrapped_data_key,
                &aad,
                &mailbox.record.email_address.0,
            )
            .await
            .ok()
            .and_then(|plain| String::from_utf8(plain).ok())
            .unwrap_or_default();
        prepared.push(Prepared {
            mailbox_id: id,
            is_primary: mailbox.record.is_primary,
            address,
            ctx: app.tokens.mailbox_ctx(app, user, &id).await.ok(),
            refresh: app.tokens.refresh_token(app, user, &id).await.ok(),
        });
    }
    Ok(prepared)
}

/// A pseudonymous `account_delete` event; never an address, token or mailbox ID.
fn security_event(app: &AppState, outcome: &'static str, user: &UserId) {
    obs::security_event(&SecurityEvent {
        action: "account_delete",
        outcome,
        user: Some(Pseudonymiser::new(app.config.rate_key.clone()).pseudo_id(&user.0)),
        request_id: None,
        amr: None,
        provider: Some("gmail"),
        method: None,
    });
}
