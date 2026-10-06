//! Undo of a swipe (T-604; SW-05 AC1, AC4).
//!
//! The token is the only state: it carries the swipe record, so undo runs with
//! no server-side undo stack. A `file` restores the exact previous label set;
//! a `keep` or `skip` only reverses the sender stats and skip count. `reject`
//! undo belongs to T-606.

use std::sync::Arc;

use domain::undo::{plan_undo, reverse_stats, SwipeRecord, UndoResponse};
use domain::user_state::UserState;
use domain::{SwipeAction, UserId};
use ports::WrappedKey;

use crate::error::ApiError;
use crate::sealed::{http_status_for, SealedTokens, TokenType};
use crate::services::categories::provider_error;
use crate::services::swipe::UndoPayload;
use crate::services::user_state_store::UserStateStore;
use crate::session::extract::AuthedSession;
use crate::state::AppState;

/// Reverse one swipe (API-SW-2).
///
/// # Errors
///
/// `UndoExpired` for a token that does not open, `NotFound` when the mailbox is
/// gone, `MailboxNeedsSignIn` or `ProviderError` when the provider refuses the
/// restore (the token stays valid), `Internal` for a `reject` (T-606) or a
/// store failure.
pub async fn undo(
    app: &AppState,
    session: &AuthedSession,
    token: &str,
) -> Result<UndoResponse, ApiError> {
    let user = session.user;
    let wrapped = wrapped_key(app, &user).await?;
    let sealer = SealedTokens::new(Arc::clone(&app.ports.keys), Arc::clone(&app.ports.clock));
    // Step 1: any failure to open is `410`.
    let payload: UndoPayload = sealer
        .open(
            TokenType::Undo,
            &user,
            &wrapped,
            &session.session_record_id,
            token,
        )
        .await
        .map_err(open_error)?;

    // Step 2: `reject` undo belongs to T-606.
    if payload.record.action == SwipeAction::Reject {
        return Err(ApiError::Internal);
    }
    let plan = plan_undo(&payload.record);

    // Step 3: restore the provider labels first; a failure changes nothing, so
    // the token stays valid.
    if let Some(exact) = plan.restore_labels.as_ref() {
        restore(app, &user, &payload.record, exact).await?;
    }

    // Step 4: one state write reverses everything this swipe recorded.
    let store = UserStateStore::new(Arc::new(app.clone()));
    let record = payload.record.clone();
    let history_entry = payload.history_entry;
    let skip_key = payload.skip_key.clone();
    let swipe = payload.swipe_id;
    store
        .update(&user, move |s: &mut UserState| {
            reverse(&record, history_entry, skip_key.as_deref(), swipe, s);
        })
        .await?;

    // Step 5: nothing here is a `reject`, so no unsubscribe has ever been sent.
    Ok(UndoResponse {
        restored: true,
        unsubscribe_already_sent: false,
    })
}

/// Reverse stats, skip bookkeeping, History, totals and the idempotency record.
fn reverse(
    record: &SwipeRecord,
    history_entry: Option<uuid::Uuid>,
    skip_key: Option<&str>,
    swipe: uuid::Uuid,
    s: &mut UserState,
) {
    if let Some(stats) = s.sender_stats.get_mut(record.sender.as_str()) {
        reverse_stats(record, stats);
    }
    if let Some(entry_id) = history_entry {
        s.history.retain(|h| h.entry_id != entry_id);
    }
    if let Some(key) = skip_key {
        if let Some(count) = s.skips.counts.get_mut(key) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                s.skips.counts.remove(key);
            }
        }
        s.skips
            .queue
            .retain(|q| format!("{}/{}", q.mailbox_id.0, q.message_id) != key);
    }
    match record.action {
        SwipeAction::Keep => {
            s.totals.triaged = s.totals.triaged.saturating_sub(1);
        }
        SwipeAction::File { .. } => {
            s.totals.triaged = s.totals.triaged.saturating_sub(1);
            s.totals.cleared = s.totals.cleared.saturating_sub(1);
        }
        SwipeAction::Skip | SwipeAction::Reject => {}
    }
    s.recent_swipes.retain(|r| r.swipe_id != swipe);
}

/// The user's wrapped `data_key`, needed to open the undo token.
async fn wrapped_key(app: &AppState, user: &UserId) -> Result<WrappedKey, ApiError> {
    app.ports
        .store
        .users()
        .get(user)
        .await?
        .map(|u| u.record.wrapped_data_key)
        .ok_or(ApiError::Unauthenticated)
}

/// Step 3: put the exact previous label set back. Returns `502`/`503` and
/// changes nothing on failure.
async fn restore(
    app: &AppState,
    user: &UserId,
    record: &SwipeRecord,
    exact: &domain::LabelSet,
) -> Result<(), ApiError> {
    let mailbox = record.mailbox;
    let stored = app
        .ports
        .store
        .mailboxes()
        .get(&mailbox)
        .await?
        .ok_or(ApiError::NotFound)?;
    if stored.record.user_id != *user {
        return Err(ApiError::NotFound);
    }
    let ctx = app.tokens.mailbox_ctx(app, user, &mailbox).await?;
    app.ports
        .mail(stored.record.provider)
        .restore_labels(&ctx, &record.message, exact)
        .await
        .map_err(|e| provider_error(mailbox, &e))
}

/// Step 1: `410 undo_expired` for every open failure, except a down key
/// service, which is `503`.
fn open_error(e: crate::sealed::TokenError) -> ApiError {
    let (status, _) = http_status_for(TokenType::Undo, e);
    if status == 503 {
        ApiError::ProviderUnavailable {
            mailbox_id: None,
            retry_after_s: None,
        }
    } else {
        ApiError::UndoExpired
    }
}
