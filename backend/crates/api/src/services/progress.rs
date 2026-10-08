//! API-PROG-1 progress: the inbox meter and the backlog level (T-603).
//!
//! Completed backlog years are recorded once; incomplete levels are read-only.
//! Nothing is logged but the request itself. Counting costs one
//! folder-total call per mailbox plus one date-range count per mailbox per
//! year examined (GM-01 AC2, GM-04 AC1).

use std::collections::BTreeMap;
use std::sync::Arc;

use domain::feed::{current_level, level_progress, level_range, LevelProgress};
use domain::user_state::MailboxPosition;
use domain::{MailboxId, MailboxStatus, Provider, UserId};
use ports::store::{MailboxRecord, Versioned};
use ports::{MailError, MailboxCtx, MessageQuery};
use time::Duration;
use uuid::Uuid;

use crate::error::ApiError;
use crate::routes::feed::MailboxErrorDto;
use crate::routes::progress::{LevelDto, ProgressDto, LEVEL_LOOKBACK_YEARS};
use crate::services::feed::{bounded, error_code, mailbox_error_code};
use crate::services::user_state_store::UserStateStore;
use crate::session::extract::AuthedSession;
use crate::state::AppState;

/// A connected mailbox with a usable token for this request.
struct Live {
    id: MailboxId,
    provider: Provider,
    ctx: MailboxCtx,
}

/// The inbox meter and the current backlog level (API-PROG-1).
///
/// # Errors
///
/// Whatever [`UserStateStore::load`] returns (`MailboxNeedsSignIn` when the
/// primary mailbox cannot read the state file), a store failure, or
/// `Internal` when a mailbox context cannot be built. A mailbox that fails to
/// answer is data in `mailbox_errors`, never an error here.
pub async fn progress(app: &AppState, session: &AuthedSession) -> Result<ProgressDto, ApiError> {
    let user = session.user;
    let store = UserStateStore::new(Arc::new(app.clone()));
    let loaded = store.load(&user).await?;
    let records = app.ports.store.mailboxes().by_user(&user).await?;

    let mut errors: BTreeMap<Uuid, &'static str> = BTreeMap::new();
    let live = connect(app, &user, &records, &mut errors).await?;

    // Step 2: one folder-total call per connected mailbox, at most
    // `PROVIDER_CONCURRENCY` in flight. A failure is data (GM-01 AC2).
    let mut calls = Vec::with_capacity(live.len());
    for l in &live {
        calls.push(inbox_total(app, l));
    }
    let mut inbox_count = 0_u64;
    for (mailbox, result) in bounded(calls).await {
        match result {
            Ok(count) => inbox_count = inbox_count.saturating_add(count),
            Err(e) => {
                errors
                    .entry(mailbox)
                    .or_insert_with(|| mailbox_error_code(&e));
            }
        }
    }

    // Step 3: the level waits until new mail is cleared on every connected
    // mailbox (GM-04 AC1). No mailbox means nothing to clear.
    let empty = MailboxPosition::default();
    let in_backlog = live.iter().all(|l| {
        loaded
            .state
            .positions
            .get(&l.id.0)
            .unwrap_or(&empty)
            .new_done
    });

    // Step 4: the newest stored backlog position is the year being worked
    // through (GM-04 AC3: from the stored Feed position, never from new mail).
    let ceiling = live
        .iter()
        .filter_map(|l| {
            loaded
                .state
                .positions
                .get(&l.id.0)
                .and_then(|p| p.backlog_ceiling)
        })
        .max();
    let Some(mut year) = current_level(ceiling, in_backlog) else {
        return Ok(finish(inbox_count, errors, None));
    };

    // Steps 5 and 6: one date-range count per mailbox per year, moving to the
    // next older year while the current one has no mail left (GM-04 AC1, AC2).
    let mut empty_years = 0_i32;
    loop {
        let remaining = year_remaining(app, &live, year, &mut errors).await;
        match level_progress(year, remaining) {
            LevelProgress::Continue { year } => {
                return Ok(finish(
                    inbox_count,
                    errors,
                    Some(LevelDto { year, remaining }),
                ));
            }
            LevelProgress::Complete { cleared, next } => {
                if empty_years >= LEVEL_LOOKBACK_YEARS {
                    return Ok(finish(inbox_count, errors, None));
                }
                if errors.is_empty() && !loaded.state.totals.levels_cleared.contains(&cleared) {
                    let now = app.ports.clock.now();
                    store
                        .update(&user, |state| {
                            if !state.totals.levels_cleared.contains(&cleared) {
                                state.totals.levels_cleared.push(cleared);
                                state.totals.years_cleared =
                                    state.totals.levels_cleared.len() as u64;
                                crate::services::achievements::record_unlocks(
                                    state,
                                    session.session_record_id.0,
                                    now,
                                );
                            }
                        })
                        .await?;
                }
                empty_years += 1;
                year = next;
            }
        }
    }
}

/// The response, with `mailbox_errors` in mailbox-ID order.
fn finish(
    inbox_count: u64,
    errors: BTreeMap<Uuid, &'static str>,
    level: Option<LevelDto>,
) -> ProgressDto {
    ProgressDto {
        inbox_count,
        mailbox_errors: errors
            .into_iter()
            .map(|(mailbox_id, code)| MailboxErrorDto { mailbox_id, code })
            .collect(),
        level,
    }
}

/// Step 1: a context for every connected mailbox; every other mailbox is a
/// `mailbox_errors` entry.
async fn connect(
    app: &AppState,
    user: &UserId,
    records: &[Versioned<MailboxRecord>],
    errors: &mut BTreeMap<Uuid, &'static str>,
) -> Result<Vec<Live>, ApiError> {
    let mut live = Vec::new();
    for record in records {
        let id = record.record.mailbox_id;
        match record.record.status {
            MailboxStatus::NeedsSignIn => {
                errors.insert(id.0, "mailbox_needs_sign_in");
                continue;
            }
            MailboxStatus::ConsentBlocked => {
                errors.insert(id.0, "consent_blocked");
                continue;
            }
            MailboxStatus::Connected => {}
        }
        match app.tokens.mailbox_ctx(app, user, &id).await {
            Ok(ctx) => live.push(Live {
                id,
                provider: record.record.provider,
                ctx,
            }),
            Err(e) => {
                let code = error_code(&e).ok_or(ApiError::Internal)?;
                errors.insert(id.0, code);
            }
        }
    }
    Ok(live)
}

/// One mailbox's inbox total, kept with its mailbox ID for the error map.
async fn inbox_total(app: &AppState, live: &Live) -> (Uuid, Result<u64, MailError>) {
    let count = app.ports.mail(live.provider).inbox_count(&live.ctx).await;
    (live.id.0, count)
}

/// Step 5: mail left in `year` across the connected mailboxes, by one
/// date-range count each. A failing mailbox contributes nothing and is marked
/// once, however many years are examined (GM-01 AC2).
async fn year_remaining(
    app: &AppState,
    live: &[Live],
    year: i32,
    errors: &mut BTreeMap<Uuid, &'static str>,
) -> u64 {
    let Some((start, end)) = level_range(year) else {
        return 0;
    };
    // `[start, end)` at second precision: the provider's bounds are exclusive,
    // so the range opens one second before the year does.
    let query = MessageQuery {
        in_inbox: true,
        after: Some(start - Duration::seconds(1)),
        before: Some(end),
        ..MessageQuery::default()
    };
    let mut calls = Vec::with_capacity(live.len());
    for l in live {
        calls.push(count_year(app, l, &query));
    }
    let mut total = 0_u64;
    for (mailbox, result) in bounded(calls).await {
        match result {
            Ok(count) => total = total.saturating_add(count),
            Err(e) => {
                errors
                    .entry(mailbox)
                    .or_insert_with(|| mailbox_error_code(&e));
            }
        }
    }
    total
}

async fn count_year(
    app: &AppState,
    live: &Live,
    query: &MessageQuery,
) -> (Uuid, Result<u64, MailError>) {
    let count = app
        .ports
        .mail(live.provider)
        .count_messages(&live.ctx, query)
        .await;
    (live.id.0, count)
}
