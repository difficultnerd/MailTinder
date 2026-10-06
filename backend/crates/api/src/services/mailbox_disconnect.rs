//! T-601b: API-MBX-2, disconnect one of several mailboxes (S7 5.3; S2 AU-05
//! AC1-AC4, DEL-3, INV-2, ASVS V8.2.2).
//!
//! The order is deliberate: the app folder file moves before anything
//! destructive, the provider grant is revoked only after the old Drive can no
//! longer be needed, and the mailbox document is deleted last.

use domain::{JobStatus, MailboxId, MailboxStatus, UserId};
use obs::{Pseudonymiser, SecurityEvent};
use ports::store::records::MailboxRecord;
use ports::{MailboxCtx, PageRequest, Precondition, Versioned, MAX_PAGE};

use crate::auth::step_up::require_step_up;
use crate::error::ApiError;
use crate::services::jobs::cancel_queued_job;
use crate::session::extract::AuthedSession;
use crate::state::AppState;

/// Disconnect one mailbox (API-MBX-2).
///
/// # Errors
///
/// `ApiError::StepUpRequired` without a fresh Google sign-in; `NotFound` for a
/// missing mailbox or another user's; `LastMailbox` when it is the only one;
/// `AppFolderMoveFailed` when the primary's app folder file cannot move;
/// `Internal` on a store or provider failure.
pub async fn disconnect(
    app: &AppState,
    session: &AuthedSession,
    mailbox: &MailboxId,
) -> Result<(), ApiError> {
    // 1. A fresh sign-in; otherwise nothing changes (AU-05 AC3).
    require_step_up(session, app.ports.clock.as_ref())?;

    // 2. Missing or another user's is a flat 404 (ASVS V8.2.2).
    let Some(target) = app.ports.store.mailboxes().get(mailbox).await? else {
        return Err(ApiError::NotFound);
    };
    if target.record.user_id != session.user {
        return Err(ApiError::NotFound);
    }

    // 3. The last mailbox cannot be disconnected (AU-05 AC2).
    let all = app.ports.store.mailboxes().by_user(&session.user).await?;
    if all.len() <= 1 {
        return Err(ApiError::LastMailbox);
    }

    // 4. The primary's app folder file moves first (AU-05 AC4).
    if target.record.is_primary {
        move_app_folder(app, &session.user, &target, &all).await?;
    }

    // 5. Queued jobs are cancelled, and their Cloud Tasks with them (AC1).
    for job in app.ports.store.jobs().by_mailbox(mailbox).await? {
        if job.record.status == JobStatus::Queued {
            cancel_queued_job(app, &job.record.job_id).await?;
        }
    }

    // 6. The mailbox's Needs Attention items go.
    delete_needs_attention(app, &session.user, mailbox).await?;

    // 7. The user state file (Feed cursor, label IDs) is T-602b; it is not
    //    merged yet, so there is nothing to remove here.

    // 8. Revoke at the provider; a failure is logged and deletion continues.
    revoke_grant(app, &session.user, mailbox).await;
    app.tokens.forget(mailbox);

    // 9. Remove the document (the encrypted address and refresh token with it).
    app.ports
        .store
        .mailboxes()
        .delete(mailbox, Precondition::None)
        .await?;

    // 10. One pseudonymous security event, no address or token (DEL-3, S5).
    security_event(app, "mailbox_unlink", "success", &session.user);
    Ok(())
}

/// Copy the app folder file to the next linked mailbox, which becomes primary,
/// then delete the old copy. Any failure leaves the old mailbox primary and
/// connected and returns `409 app_folder_move_failed` (AU-05 AC4).
async fn move_app_folder(
    app: &AppState,
    user: &UserId,
    old: &Versioned<MailboxRecord>,
    all: &[Versioned<MailboxRecord>],
) -> Result<(), ApiError> {
    // 4.1 The next mailbox: earliest `linked_at` among the connected ones.
    let next = all
        .iter()
        .filter(|m| m.record.mailbox_id != old.record.mailbox_id)
        .filter(|m| m.record.status == MailboxStatus::Connected)
        .min_by_key(|m| m.record.linked_at)
        .ok_or(ApiError::AppFolderMoveFailed)?;

    // 4.2 Both mailboxes need a live provider context.
    let old_ctx = ctx(app, user, &old.record.mailbox_id).await?;
    let new_ctx = ctx(app, user, &next.record.mailbox_id).await?;

    // 4.3 No file yet: there is nothing to copy, just move the flag.
    let Some((bytes, _etag)) = app
        .ports
        .app_folder
        .read(&old_ctx)
        .await
        .map_err(|_| ApiError::AppFolderMoveFailed)?
    else {
        return flip_primary(app, old, next).await;
    };

    // 4.4 Copy the bytes as they are: the file's AAD names the user, never the
    //     mailbox (T-602b), so there is no re-encryption. A stale copy on the
    //     new Drive is replaced under its own ETag.
    let etag = app
        .ports
        .app_folder
        .read(&new_ctx)
        .await
        .map_err(|_| ApiError::AppFolderMoveFailed)?
        .map(|(_, tag)| tag);
    app.ports
        .app_folder
        .write(&new_ctx, &bytes, etag.as_ref())
        .await
        .map_err(|_| ApiError::AppFolderMoveFailed)?;

    // 4.5 The new mailbox becomes primary, then the old one stops being.
    flip_primary(app, old, next).await?;

    // 4.6 Delete the old copy; if that fails, undo the move.
    if app.ports.app_folder.delete(&old_ctx).await.is_err() {
        let _ = app.ports.app_folder.delete(&new_ctx).await;
        restore_primary(app, &old.record.mailbox_id, &next.record.mailbox_id).await;
        return Err(ApiError::AppFolderMoveFailed);
    }
    Ok(())
}

/// Flip `is_primary` from `old` to `next` with a version precondition each. A
/// failure rolls the flag back so two primaries are never left behind.
async fn flip_primary(
    app: &AppState,
    old: &Versioned<MailboxRecord>,
    next: &Versioned<MailboxRecord>,
) -> Result<(), ApiError> {
    let mut new_record = next.record.clone();
    new_record.is_primary = true;
    app.ports
        .store
        .mailboxes()
        .put(&new_record, Precondition::Matches(next.version.clone()))
        .await
        .map_err(|_| ApiError::AppFolderMoveFailed)?;
    let mut old_record = old.record.clone();
    old_record.is_primary = false;
    let flipped = app
        .ports
        .store
        .mailboxes()
        .put(&old_record, Precondition::Matches(old.version.clone()))
        .await;
    if flipped.is_err() {
        reset_primary(app, &next.record.mailbox_id).await;
        return Err(ApiError::AppFolderMoveFailed);
    }
    Ok(())
}

/// Best effort: clear `is_primary` on one mailbox, re-reading its version.
async fn reset_primary(app: &AppState, mailbox: &MailboxId) {
    if let Ok(Some(versioned)) = app.ports.store.mailboxes().get(mailbox).await {
        let version = versioned.version;
        let mut record = versioned.record;
        record.is_primary = false;
        let _ = app
            .ports
            .store
            .mailboxes()
            .put(&record, Precondition::Matches(version))
            .await;
    }
}

/// Best effort: put the flag back on the old mailbox and off the new one.
async fn restore_primary(app: &AppState, old: &MailboxId, new: &MailboxId) {
    if let Ok(Some(versioned)) = app.ports.store.mailboxes().get(old).await {
        let version = versioned.version;
        let mut record = versioned.record;
        record.is_primary = true;
        let _ = app
            .ports
            .store
            .mailboxes()
            .put(&record, Precondition::Matches(version))
            .await;
    }
    reset_primary(app, new).await;
}

/// A provider context for one mailbox; any failure is a move failure.
async fn ctx(app: &AppState, user: &UserId, mailbox: &MailboxId) -> Result<MailboxCtx, ApiError> {
    app.tokens
        .mailbox_ctx(app, user, mailbox)
        .await
        .map_err(|_| ApiError::AppFolderMoveFailed)
}

/// Delete every Needs Attention item that belongs to `mailbox`.
async fn delete_needs_attention(
    app: &AppState,
    user: &UserId,
    mailbox: &MailboxId,
) -> Result<(), ApiError> {
    let mut doomed = Vec::new();
    let mut after = None;
    loop {
        let page = app
            .ports
            .store
            .needs_attention()
            .by_user(
                user,
                PageRequest {
                    limit: MAX_PAGE,
                    after,
                },
            )
            .await?;
        for item in &page.items {
            if item.record.mailbox_id == *mailbox {
                doomed.push(item.record.item_id);
            }
        }
        match page.next {
            Some(cursor) => after = Some(cursor),
            None => break,
        }
    }
    for item_id in doomed {
        app.ports
            .store
            .needs_attention()
            .delete(&item_id, Precondition::None)
            .await?;
    }
    Ok(())
}

/// Revoke the mailbox's refresh token at Google. A missing or failed revoke is
/// logged (`revoke_failed`) and deletion continues: the record goes either way.
async fn revoke_grant(app: &AppState, user: &UserId, mailbox: &MailboxId) {
    let Ok(token) = app.tokens.refresh_token(app, user, mailbox).await else {
        security_event(app, "mailbox_unlink", "revoke_failed", user);
        return;
    };
    if app.ports.identity.revoke(&token).await.is_err() {
        security_event(app, "mailbox_unlink", "revoke_failed", user);
    }
}

/// A pseudonymous security event; never an address, token or mailbox ID.
fn security_event(app: &AppState, action: &'static str, outcome: &'static str, user: &UserId) {
    obs::security_event(&SecurityEvent {
        action,
        outcome,
        user: Some(Pseudonymiser::new(app.config.rate_key.clone()).pseudo_id(&user.0)),
        request_id: None,
        amr: None,
        provider: Some("gmail"),
        method: None,
    });
}
