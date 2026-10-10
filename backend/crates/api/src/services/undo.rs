//! Undo of a swipe (T-604; SW-05 AC1, AC4).
//!
//! The token is the only state: it carries the swipe record, so undo runs with
//! no server-side undo stack. A `file` restores the exact previous label set;
//! a `keep` or `skip` only reverses the sender stats and skip count. `reject`
//! undo belongs to T-606.

use std::sync::Arc;

use domain::undo::{
    plan_undo, reverse_stats, undo_response, JobCancelOutcome, SwipeRecord, UndoResponse,
};
use domain::user_state::{HistoryAction, HistoryEntry, HistoryOutcome, UserState};
use domain::{JobStatus, SwipeAction, UserId};
use obs::{Pseudonymiser, SecurityEvent};
use ports::store::NeedsAttentionId;
use ports::{Precondition, WrappedKey};
use time::OffsetDateTime;

use crate::error::ApiError;
use crate::sealed::{http_status_for, SealedTokens, TokenType};
use crate::services::categories::provider_error;
use crate::services::jobs::{cancel_queued_job, CancelResult};
use crate::services::swipe::UndoPayload;
use crate::services::user_state_store::UserStateStore;
use crate::session::extract::AuthedSession;
use crate::state::AppState;

/// Reverse one swipe (API-SW-2).
///
/// # Errors
///
/// `UndoExpired` for a token that does not open or that names a swipe which has
/// already been undone, `NotFound` when the mailbox is
/// gone, `MailboxNeedsSignIn` or `ProviderError` when the provider refuses the
/// restore (the token stays valid), `Internal` for a store failure.
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

    // Step 2: `reject` undo also cancels the job and removes the rule (T-606).
    if payload.record.action == SwipeAction::Reject {
        return undo_reject(app, session, &payload).await;
    }
    let plan = plan_undo(&payload.record);

    // Step 3: the token is single-use. Its swipe record must still be in
    // `recent_swipes`; a record that is gone means this undo already ran (or
    // the record aged out), so the answer is `410` and nothing is touched.
    // Without this a replayed token would reverse later swipes on the same
    // sender and re-apply a label set the user has since changed.
    let store = UserStateStore::new(Arc::new(app.clone()));
    if !store
        .load(&user)
        .await?
        .state
        .recent_swipes
        .iter()
        .any(|r| r.swipe_id == payload.swipe_id)
    {
        return Err(ApiError::UndoExpired);
    }

    // Step 4: restore the provider labels first; a failure changes nothing, so
    // the token stays valid.
    if let Some(exact) = plan.restore_labels.as_ref() {
        if let Err(e) = restore(app, &user, &payload.record, exact).await {
            // The exact previous state was not restored (S10 8, `undo_failed`).
            if restore_failed_conclusively(&e) {
                emit_undo_failed(app, &user, payload.record.action);
            }
            return Err(e);
        }
    }

    // Step 5: one state write reverses everything this swipe recorded. The
    // record is claimed inside that write, so a concurrent second undo makes no
    // change and also answers `410`.
    let record = payload.record.clone();
    let history_entry = payload.history_entry;
    let skip_key = payload.skip_key.clone();
    let swipe = payload.swipe_id;
    let reversed = store
        .update(&user, move |s: &mut UserState| {
            reverse(&record, history_entry, skip_key.as_deref(), swipe, s)
        })
        .await?;
    if !reversed {
        return Err(ApiError::UndoExpired);
    }

    // The `undo` metric (S10 8), once the reversal has committed.
    emit_undo(app, &user, payload.record.action);

    // Step 6: nothing here is a `reject`, so no unsubscribe has ever been sent.
    Ok(UndoResponse {
        restored: true,
        unsubscribe_already_sent: false,
    })
}

/// Reverse stats, skip bookkeeping, History, totals and the idempotency record.
///
/// Returns `false`, and changes nothing, when the swipe record is already gone:
/// this undo has been performed by an earlier or concurrent call and the token
/// must not be applied twice.
fn reverse(
    record: &SwipeRecord,
    history_entry: Option<uuid::Uuid>,
    skip_key: Option<&str>,
    swipe: uuid::Uuid,
    s: &mut UserState,
) -> bool {
    if !s.recent_swipes.iter().any(|r| r.swipe_id == swipe) {
        return false;
    }
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
    true
}

/// Undo a `reject` (T-606; SW-05 AC1 to AC5, UN-01 AC1/AC3, INV-6).
///
/// Cancels the queued job first, so the cancel has the best chance of beating
/// the due time; restores the exact previous labels (also after a spam report);
/// removes the rule and Needs Attention item the swipe created; reverses the
/// counts and the spam History entry; and, when the cancel won, removes the job
/// and appends the cancelled unsubscribe to History (UN-01 AC3).
///
/// # Errors
///
/// `UndoExpired` for a token whose swipe record is already gone (single-use),
/// `NotFound` when the mailbox is gone, `503`/`502` when the provider refuses
/// the restore (the token stays valid), `Internal` for a store failure.
#[allow(clippy::too_many_lines)]
pub async fn undo_reject(
    app: &AppState,
    session: &AuthedSession,
    payload: &UndoPayload,
) -> Result<UndoResponse, ApiError> {
    let user = session.user;
    let record = payload.record.clone();
    let store = UserStateStore::new(Arc::new(app.clone()));

    // The token is single-use (T-604 F1): its swipe record must still be in
    // `recent_swipes`. A gone record means this undo already ran.
    if !store
        .load(&user)
        .await?
        .state
        .recent_swipes
        .iter()
        .any(|r| r.swipe_id == payload.swipe_id)
    {
        return Err(ApiError::UndoExpired);
    }

    // Step 1: cancel the queued job (conditional `queued -> cancelled`) before
    // anything else. A terminal job is mapped to its answer. A job that had
    // already left the queue means the runner sent (or is sending) the
    // unsubscribe even though the undo succeeds: the one real "unsubscribe sent
    // after a successful undo" (S10 8, `unsub_after_undo`; A1).
    let (outcome, send_followed_undo) = match record.job_id {
        Some(job) => {
            let cancel = cancel_queued_job(app, &job).await?;
            (cancel_outcome(&cancel), unsubscribe_sent(cancel))
        }
        None => (JobCancelOutcome::NoJob, false),
    };

    // Step 2: put the exact previous label set back. A failure changes nothing,
    // so the token stays valid; the cancelled job stays `Cancelled`, so a retry
    // maps `NotQueued(Cancelled)` back to `Cancelled` and gives the same answer.
    if let Some(exact) = plan_undo(&record).restore_labels {
        if let Err(e) = restore(app, &user, &record, &exact).await {
            // The exact previous state was not restored (S10 8, `undo_failed`).
            if restore_failed_conclusively(&e) {
                emit_undo_failed(app, &user, SwipeAction::Reject);
            }
            return Err(e);
        }
    }

    // Step 3: the Needs Attention item belonged to this swipe.
    if let Some(item) = payload.na_item {
        app.ports
            .store
            .needs_attention()
            .delete(&NeedsAttentionId(item), Precondition::None)
            .await?;
    }

    // Step 4: one state write reverses every recorded change. The record is
    // claimed inside that write, so a concurrent replay changes nothing and
    // also answers `410`.
    let history_entry = payload.history_entry;
    let swipe = payload.swipe_id;
    let now = app.ports.clock.now();
    let job = record.job_id;
    let claimed = store
        .update(&user, move |s: &mut UserState| {
            reverse_reject(&record, history_entry, now, outcome, swipe, s)
        })
        .await?;
    if !claimed {
        return Err(ApiError::UndoExpired);
    }

    // The `undo` metric (S10 8), once the reversal has committed.
    emit_undo(app, &user, SwipeAction::Reject);

    // The undo succeeded but the unsubscribe had already gone (or was in
    // flight): the exact "unsubscribe sent after a successful undo" (S10 8),
    // emitted only after the state change committed.
    if send_followed_undo {
        emit_unsub_after_undo(app, &user);
    }

    // Step 5: with the cancel won, the job record goes last, so every earlier
    // failure leaves a retryable `Cancelled` record.
    if outcome == JobCancelOutcome::Cancelled {
        if let Some(job) = job {
            app.ports
                .store
                .jobs()
                .delete(&job, Precondition::None)
                .await?;
        }
    }

    // Step 6: `unsubscribe_already_sent` is true only for `AlreadySent`.
    security_event(app, &user, outcome);
    Ok(undo_response(outcome))
}

/// True when the cancel found the job already out of the queue, so the runner
/// had claimed it (`Running`, the send in flight) or already sent it (`Sent`),
/// or the record was collected after it ran (`Missing`). Those are the cases
/// where an unsubscribe follows the user's successful undo (S10 8); a job the
/// undo itself cancelled, or that ended `NeedsAttention`/`Failed`/`Expired`
/// without a send, is not.
#[must_use]
fn unsubscribe_sent(cancel: CancelResult) -> bool {
    matches!(
        cancel,
        CancelResult::NotQueued(JobStatus::Running | JobStatus::Sent) | CancelResult::Missing
    )
}

/// Map the store result to T-105b's outcome. The only place this mapping lives.
#[must_use]
pub fn cancel_outcome(r: &CancelResult) -> JobCancelOutcome {
    match r {
        // An earlier undo attempt cancelled it, then failed later.
        CancelResult::Cancelled | CancelResult::NotQueued(JobStatus::Cancelled) => {
            JobCancelOutcome::Cancelled
        }
        // The runner claimed it; a request may have gone, or the record was
        // collected by a Feed load after it ran. `NotQueued(Queued)` is
        // unreachable by construction; treat it as sent.
        CancelResult::NotQueued(
            JobStatus::Running | JobStatus::Sent | JobStatus::NeedsAttention | JobStatus::Queued,
        )
        | CancelResult::Missing => JobCancelOutcome::AlreadySent,
        CancelResult::NotQueued(JobStatus::Failed | JobStatus::Expired) => {
            JobCancelOutcome::AlreadyFinishedNotSent
        }
    }
}

/// Reverse the state a reject applied. Returns `false`, and changes nothing,
/// when the swipe record is already gone: a concurrent or replayed undo must
/// not be applied twice.
fn reverse_reject(
    record: &SwipeRecord,
    history_entry: Option<uuid::Uuid>,
    now: OffsetDateTime,
    outcome: JobCancelOutcome,
    swipe: uuid::Uuid,
    s: &mut UserState,
) -> bool {
    if !s.recent_swipes.iter().any(|r| r.swipe_id == swipe) {
        return false;
    }
    if let Some(stats) = s.sender_stats.get_mut(record.sender.as_str()) {
        reverse_stats(record, stats);
        // The swipe may have defeated a boss; undo puts the boss back.
        stats.boss_defeated = false;
    }
    // Remove only the rule this swipe created (`rule_id` is `None` when an
    // identical rule already existed).
    if let Some(rule_id) = record.rule_id {
        let before = s.rules.len();
        s.rules.retain(|r| r.rule.rule_id != rule_id);
        if s.rules.len() != before {
            s.totals.senders_silenced = s.totals.senders_silenced.saturating_sub(1);
        }
    }
    // The `reported_spam` History entry this swipe wrote.
    if let Some(entry_id) = history_entry {
        s.history.retain(|h| h.entry_id != entry_id);
    }
    s.totals.triaged = s.totals.triaged.saturating_sub(1);
    s.totals.cleared = s.totals.cleared.saturating_sub(1);
    if record.job_id.is_some() {
        s.totals.unsubscribes_queued = s.totals.unsubscribes_queued.saturating_sub(1);
        s.totals.round_unsubscribes = s.totals.round_unsubscribes.saturating_sub(1);
    }
    // A cancelled job is recorded in History once and leaves `pending`.
    if outcome == JobCancelOutcome::Cancelled {
        if let Some(job) = record.job_id {
            let display = s
                .pending_unsubscribes
                .get(&job.0)
                .map_or_else(String::new, |p| p.sender_display.clone());
            s.pending_unsubscribes.remove(&job.0);
            let _ = s.push_history(HistoryEntry {
                entry_id: job.0,
                at: now,
                mailbox_id: record.mailbox,
                sender_display: display,
                action: HistoryAction::Unsubscribe,
                outcome: HistoryOutcome::Cancelled,
                rule_id: record.rule_id,
            });
        }
    }
    s.recent_swipes.retain(|r| r.swipe_id != swipe);
    true
}

/// The outcome code for the security event; no IDs beyond the pseudonymous user.
fn outcome_code(o: JobCancelOutcome) -> &'static str {
    match o {
        JobCancelOutcome::NoJob => "no_job",
        JobCancelOutcome::Cancelled => "cancelled",
        JobCancelOutcome::AlreadySent => "already_sent",
        JobCancelOutcome::AlreadyFinishedNotSent => "already_finished",
    }
}

/// Security event `undo` with the outcome code (S5; no IDs).
fn security_event(app: &AppState, user: &UserId, outcome: JobCancelOutcome) {
    obs::security_event(&SecurityEvent {
        action: "undo",
        outcome: outcome_code(outcome),
        user: Some(Pseudonymiser::new(app.config.rate_key.clone()).pseudo_id(&user.0)),
        request_id: None,
        amr: None,
        provider: None,
        method: None,
    });
}

/// The `undo` metric (S10 8): the undone action and the pseudonymous user,
/// nothing else. Emitted once the reversal has committed.
fn emit_undo(app: &AppState, user: &UserId, action: SwipeAction) {
    obs::metric_event(&obs::MetricEvent {
        event_type: "undo",
        outcome: crate::services::swipe::action_code(action),
        user: Some(Pseudonymiser::new(app.config.rate_key.clone()).pseudo_id(&user.0)),
        provider: None,
    });
}

/// The `undo_failed` metric (S10 8): the swipe's action was not restored to its
/// exact previous state. Content-free, like every metric event.
fn emit_undo_failed(app: &AppState, user: &UserId, action: SwipeAction) {
    obs::metric_event(&obs::MetricEvent {
        event_type: "undo_failed",
        outcome: crate::services::swipe::action_code(action),
        user: Some(Pseudonymiser::new(app.config.rate_key.clone()).pseudo_id(&user.0)),
        provider: None,
    });
}

/// The `unsub_after_undo` metric (S10 8; A1): an unsubscribe was sent after a
/// successful undo. Content-free, like every metric event.
fn emit_unsub_after_undo(app: &AppState, user: &UserId) {
    obs::metric_event(&obs::MetricEvent {
        event_type: "unsub_after_undo",
        outcome: "sent",
        user: Some(Pseudonymiser::new(app.config.rate_key.clone()).pseudo_id(&user.0)),
        provider: None,
    });
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

/// True when a restore failure is conclusive: the exact previous state cannot
/// be restored by retrying. A retryable provider outage (`503`,
/// `ProviderUnavailable`) is not the "failed to restore the exact previous
/// state" the A1 event counts (S10 8), so it must not page.
fn restore_failed_conclusively(e: &ApiError) -> bool {
    !matches!(e, ApiError::ProviderUnavailable { .. })
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
