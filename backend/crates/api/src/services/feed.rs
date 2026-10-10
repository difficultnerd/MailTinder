//! The Feed: paging, merging and card building (T-602c).
//!
//! Message content flows from the provider to the response and nowhere else:
//! nothing here is logged, and the only thing written is the user's state file
//! (positions, skip queue, sender counts), which holds no subject or body.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::Arc;

use domain::feed::{card_visible, feed_order, is_boss};
use domain::filing::{self, FilingInput};
use domain::user_state::{
    HistoryEntry, HistoryOutcome, MailboxPosition, PendingDeliveryCheck, SkipReturn, SkipState,
    UserState,
};
use domain::{
    next_status, HeaderRules, MailboxEvent, MailboxId, MailboxStatus, MessageId, MessageMeta,
    Provider, RuleKind, Tunables, UnsubscribeRoute, UserId, HEADER_RULES_ID,
};
use futures::stream::{self, StreamExt};
use ports::store::{aad_fields, MailboxRecord, Precondition, Versioned};
use ports::{Aad, MailError, MailboxCtx, MessagePage, MessageQuery, WrappedKey};
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

use crate::classify::{classify_page, BakeoffGate, CardToClassify, ClassifiedCard};
use crate::error::ApiError;
use crate::routes::feed::{
    BossDto, CardDto, CategoryRefDto, CursorPayload, FeedPage, FeedRequest, MailboxErrorDto, Phase,
    SuggestionDto, FEED_LIMIT_MAX, SEALED_TTL_HOURS,
};
use crate::sealed::{http_status_for, SealedTokens, TokenError, TokenType};
use crate::services::delivery_check::{self, DeliveryHookInput, IgnoredUnsubscribe, ListMail};
use crate::services::history_catch_up::{
    collect_job_outcomes, delete_collected, CollectedOutcomes,
};
pub use crate::services::rule_actions::apply_rules;
use crate::services::user_state_store::UserStateStore;
use crate::session::extract::AuthedSession;
use crate::state::AppState;
use crate::text::{plain_text, PREVIEW_MAX_CHARS};

/// `[DEFAULT]` matches the S4 5.7 model cap; keeps Gmail quota safe.
pub const PROVIDER_CONCURRENCY: usize = 8;

/// Schema lengths for the card strings (S7 5.4).
const NAME_MAX: usize = 256;
const ADDRESS_MAX: usize = 320;
const SUBJECT_MAX: usize = 998;
const REASON_MAX: usize = 200;
/// `CategoryRef.name` maxLength (S7 5.4).
const CATEGORY_NAME_MAX: usize = 100;
/// The provider's largest page (`MailProvider::list_messages`).
const PROVIDER_PAGE_MAX: u32 = 100;

/// What the rules did to a page (T-609).
#[derive(Debug, Default)]
pub struct RuleApplication {
    /// `(mailbox ID, message ID)` pairs the rules acted on; no cards for these.
    pub acted_on: BTreeSet<(Uuid, String)>,
    /// How many actions were applied.
    pub applied: u32,
    /// Successful provider changes, appended by the Feed's single state update.
    pub entries: Vec<HistoryEntry>,
}

/// A connected mailbox with a usable token for this request.
struct Live {
    id: MailboxId,
    provider: Provider,
    address: String,
    ctx: MailboxCtx,
}

/// One mailbox's answer to a list call.
struct Answer {
    /// Messages kept after dropping the boundary IDs.
    returned: usize,
    /// The provider filled the page, so more may follow.
    more: bool,
}

/// The result of one fetch round.
#[derive(Default)]
struct Fetched {
    metas: Vec<MessageMeta>,
    answers: BTreeMap<Uuid, Answer>,
    /// Any mailbox failed in this round.
    failed: bool,
}

/// A finished card and what the state file records about its sender.
struct Built {
    card: CardDto,
    sender_key: String,
    display: String,
    delivery_mail: ListMail,
}

/// Everything card building reads.
struct Env<'a> {
    app: &'a AppState,
    live: HashMap<Uuid, &'a Live>,
    state: &'a UserState,
    skips: &'a SkipState,
    sealer: &'a SealedTokens,
    user: &'a UserId,
    wrapped: &'a WrappedKey,
    session: &'a AuthedSession,
    tunables: &'a Tunables,
    expires_at: OffsetDateTime,
    gate: crate::experiments::BakeoffGate,
}

/// The next page of cards across all the user's mailboxes (API-FEED-1).
///
/// # Errors
///
/// `InvalidRequest` for a `limit` outside 1 to 50 or a cursor that does not
/// open; `MailboxNeedsSignIn` when the primary mailbox cannot reach the state
/// file; `Internal` on a store or key failure. A failing mailbox is data in
/// `mailbox_errors`, never an error.
pub async fn next_page(
    app: &AppState,
    session: &AuthedSession,
    req: FeedRequest,
) -> Result<FeedPage, ApiError> {
    validate_limit(req.limit)?;
    let user = session.user;
    let store = UserStateStore::new(Arc::new(app.clone()));
    // Without the primary mailbox the state file is out of reach, so no page
    // can be built: every mailbox is listed and the app asks for a sign-in.
    let loaded = match store.load(&user).await {
        Ok(loaded) => loaded,
        Err(ApiError::MailboxNeedsSignIn { .. }) => return all_signed_out(app, &user).await,
        Err(e) => return Err(e),
    };
    let wrapped = wrapped_key(app, &user).await?;
    let sealer = SealedTokens::new(Arc::clone(&app.ports.keys), Arc::clone(&app.ports.clock));
    let tunables = app.tunables.clone();

    // Steps 2 and 3: starting positions and the session-start reset.
    let (mut positions, prior_phase) =
        starting_positions(&sealer, session, &wrapped, &req, &loaded.state).await?;
    let records = app.ports.store.mailboxes().by_user(&user).await?;
    let session_id = session.session_record_id.0;
    start_session(&mut positions, &records, session_id, req.refresh);
    let mut skips = fresh_skips(&loaded.state.skips, session_id);

    // Step 4: contexts.
    let mut errors: BTreeMap<Uuid, &'static str> = BTreeMap::new();
    let live = connect(app, &user, &wrapped, &records, &mut errors).await?;

    // Steps 5 to 7: phase and fetching.
    let start_positions = positions.clone();
    let mut phase = phase_of(&positions, &live);
    let mut phase_changed = prior_phase == Phase::New && phase == Phase::Backlog;
    let mut fetched = fetch_round(app, &live, &positions, phase, req.limit, &mut errors).await;
    if phase == Phase::New && !fetched.failed && fetched.metas.is_empty() {
        for position in positions.values_mut() {
            position.new_done = true;
        }
        phase = Phase::Backlog;
        phase_changed = true;
        fetched = fetch_round(app, &live, &positions, phase, req.limit, &mut errors).await;
    }
    let nothing_fetched = fetched.metas.is_empty();

    // Step 8: merge, newest first, and take one page.
    let mut candidates = std::mem::take(&mut fetched.metas);
    candidates.sort_by(feed_order);
    candidates.truncate(usize::try_from(req.limit).unwrap_or(usize::MAX));
    let taken = candidates;
    let next_positions = advance_positions(&positions, &taken, &fetched.answers, phase);

    // Step 9: rules, then visibility (S3 "Card visibility").
    let rules = apply_rules(app, session, &loaded.state, &taken).await?;
    let page_metas = visible(&taken, &rules, &skips, &tunables);

    // Step 10: skipped cards that are due come back at the end of the page.
    let live_by_id: HashMap<Uuid, &Live> = live.iter().map(|l| (l.id.0, l)).collect();
    let returns = take_due_skips(app, &live_by_id, &mut skips, &page_metas, &tunables).await;

    // Step 11: cards.
    let expires_at = app.ports.clock.now() + Duration::hours(SEALED_TTL_HOURS);
    let env = Env {
        app,
        live: live_by_id,
        state: &loaded.state,
        skips: &skips,
        sealer: &sealer,
        user: &user,
        wrapped: &wrapped,
        session,
        tunables: &tunables,
        expires_at,
        gate: input_gate(app, &user).await,
    };
    let all: Vec<MessageMeta> = page_metas.into_iter().chain(returns).collect();
    let built = build_cards(&env, all).await?;

    // Steps 13 and 14: one state write, then the cursor.
    let collected = collect_job_outcomes(app, &user, &loaded.state).await?;
    let list_mail = delivery_mail(&user, &taken, &rules, &loaded.state, &built);
    let (ignored, confirmed) = persist(
        &store,
        &user,
        start_positions,
        skips.clone(),
        &built,
        app.ports.clock.now(),
        &rules,
        &collected,
        &app.business_calendar,
        &list_mail,
    )
    .await?;
    delivery_check::publish(app, &user, &ignored, confirmed).await?;
    delete_collected(app, &user, &collected).await;
    let next_cursor = if phase == Phase::Backlog && nothing_fetched && errors.is_empty() {
        None
    } else {
        let payload = CursorPayload {
            positions: next_positions,
            phase,
        };
        Some(seal_cursor(&env, &payload).await?)
    };

    // Step 15: errors are data.
    Ok(FeedPage {
        cards: built.into_iter().map(|b| b.card).collect(),
        next_cursor,
        phase,
        phase_changed,
        mailbox_errors: errors
            .into_iter()
            .map(|(mailbox_id, code)| MailboxErrorDto { mailbox_id, code })
            .collect(),
        rule_actions_applied: rules.applied,
    })
}

fn validate_limit(limit: u32) -> Result<(), ApiError> {
    if limit == 0 || limit > FEED_LIMIT_MAX {
        Err(ApiError::InvalidRequest {
            fields: vec!["/limit".to_owned()],
        })
    } else {
        Ok(())
    }
}

async fn wrapped_key(app: &AppState, user: &UserId) -> Result<WrappedKey, ApiError> {
    let record = app
        .ports
        .store
        .users()
        .get(user)
        .await?
        .ok_or(ApiError::Unauthenticated)?;
    Ok(record.record.wrapped_data_key)
}

/// SW-02 AC2: skip counts and the queue last only until a new session.
fn fresh_skips(stored: &SkipState, session: Uuid) -> SkipState {
    if stored.session_record_id == Some(session) {
        stored.clone()
    } else {
        SkipState {
            session_record_id: Some(session),
            ..SkipState::default()
        }
    }
}

/// Step 13: one state write. The closure is pure: it only copies in values
/// computed before the call, so a retry after an `ETag` conflict is safe.
#[allow(clippy::too_many_arguments)]
async fn persist(
    store: &UserStateStore,
    user: &UserId,
    positions: BTreeMap<Uuid, MailboxPosition>,
    skips: SkipState,
    built: &[Built],
    at: OffsetDateTime,
    rules: &RuleApplication,
    collected: &CollectedOutcomes,
    calendar: &domain::delivery::BusinessCalendar,
    list_mail: &[ListMail],
) -> Result<(Vec<IgnoredUnsubscribe>, u64), ApiError> {
    let seen: Vec<(String, String)> = built
        .iter()
        .map(|b| (b.sender_key.clone(), b.display.clone()))
        .collect();
    store
        .update(user, move |s: &mut UserState| {
            for entry in &rules.entries {
                if s.push_history(entry.clone()) {
                    s.totals.cleared = s.totals.cleared.saturating_add(1);
                    if let Some(rule) = s
                        .rules
                        .iter_mut()
                        .find(|r| Some(r.rule.rule_id) == entry.rule_id)
                    {
                        rule.times_applied = rule.times_applied.saturating_add(1);
                    }
                }
            }
            for entry in &collected.entries {
                let newly_pushed = s.push_history(entry.clone());
                s.pending_unsubscribes.remove(&entry.entry_id);
                if newly_pushed && entry.outcome == HistoryOutcome::Sent {
                    s.totals.senders_unsubscribed = s.totals.senders_unsubscribed.saturating_add(1);
                    if let Some((_, pending, unsubscribed_at)) = collected
                        .sent
                        .iter()
                        .find(|(id, _, _)| id.0 == entry.entry_id)
                    {
                        if !s.pending_delivery_checks.iter().any(|check| {
                            check.sender_key == pending.sender_key
                                && check.list_id == pending.list_id
                        }) {
                            s.pending_delivery_checks.push(PendingDeliveryCheck {
                                sender_key: pending.sender_key.clone(),
                                list_id: pending.list_id.clone(),
                                mailbox_id: pending.mailbox_id,
                                unsubscribed_at: *unsubscribed_at,
                                mail_seen: false,
                                confirm_counted: false,
                                pending_ignored_display: None,
                            });
                        }
                    }
                }
            }
            for (id, position) in &positions {
                s.positions.insert(*id, position.clone());
            }
            s.skips = skips.clone();
            for (key, display) in &seen {
                let stats = s.sender_stats.entry(key.clone()).or_default();
                stats.seen = stats.seen.saturating_add(1);
                stats.display.clone_from(display);
                stats.last_seen = Some(at);
            }
            let before = s.totals.unsubscribes_confirmed;
            let ignored = delivery_check::apply_delivery_checks(
                s,
                calendar,
                at,
                &DeliveryHookInput { list_mail },
            );
            (ignored, s.totals.unsubscribes_confirmed - before)
        })
        .await
}

/// Successful reject-list trash actions plus cards actually about to be shown.
fn delivery_mail(
    user: &UserId,
    taken: &[MessageMeta],
    applied: &RuleApplication,
    state: &UserState,
    built: &[Built],
) -> Vec<ListMail> {
    let mut mail: Vec<ListMail> = built.iter().map(|b| b.delivery_mail.clone()).collect();
    let rules: Vec<_> = state.rules.iter().map(|r| r.rule.clone()).collect();
    for meta in taken {
        let Some(rule) = domain::first_match(&rules, meta) else {
            continue;
        };
        if rule.kind != RuleKind::RejectList {
            continue;
        }
        let mut name = Vec::new();
        name.extend_from_slice(user.0.as_bytes());
        name.extend_from_slice(meta.mailbox.0.as_bytes());
        name.extend_from_slice(meta.id.as_str().as_bytes());
        name.extend_from_slice(rule.rule_id.0.as_bytes());
        let entry_id = Uuid::new_v5(&crate::services::rule_actions::NS_RULE_ACTION, &name);
        if applied
            .entries
            .iter()
            .any(|entry| entry.entry_id == entry_id)
        {
            mail.push(ListMail {
                sender_key: meta.sender.as_str().to_owned(),
                list_id: meta.facts.list_id.clone(),
                mailbox_id: meta.mailbox,
                received_at: meta.internal_date,
                sender_display: plain_text(&meta.from_display, NAME_MAX),
            });
        }
    }
    mail
}

/// Step 14: the sealed cursor for the next page.
async fn seal_cursor(env: &Env<'_>, payload: &CursorPayload) -> Result<String, ApiError> {
    env.sealer
        .seal(
            TokenType::Cursor,
            env.user,
            env.wrapped,
            &env.session.session_record_id,
            env.expires_at,
            payload,
        )
        .await
        .map_err(seal_error)
}

/// The taken messages that may show: not acted on by a rule, not skipped past
/// the limit (S3 "Card visibility").
fn visible(
    taken: &[MessageMeta],
    rules: &RuleApplication,
    skips: &SkipState,
    tunables: &Tunables,
) -> Vec<MessageMeta> {
    taken
        .iter()
        .filter(|m| {
            !rules
                .acted_on
                .contains(&(m.mailbox.0, m.id.as_str().to_owned()))
        })
        .filter(|m| card_visible(true, false, skip_count(skips, m), tunables))
        .cloned()
        .collect()
}

/// FD-02 AC3, S7 5.4: every mailbox failed, which is still a `200`.
async fn all_signed_out(app: &AppState, user: &UserId) -> Result<FeedPage, ApiError> {
    let records = app.ports.store.mailboxes().by_user(user).await?;
    Ok(FeedPage {
        cards: Vec::new(),
        next_cursor: None,
        phase: Phase::Backlog,
        phase_changed: false,
        mailbox_errors: records
            .iter()
            .map(|r| MailboxErrorDto {
                mailbox_id: r.record.mailbox_id.0,
                code: "mailbox_needs_sign_in",
            })
            .collect(),
        rule_actions_applied: 0,
    })
}

/// Step 2: the positions the page starts from, and the phase the caller was
/// last shown.
async fn starting_positions(
    sealer: &SealedTokens,
    session: &AuthedSession,
    wrapped: &WrappedKey,
    req: &FeedRequest,
    state: &UserState,
) -> Result<(BTreeMap<Uuid, MailboxPosition>, Phase), ApiError> {
    let Some(token) = &req.cursor else {
        let positions = state.positions.clone();
        let prior = if positions.values().any(|p| !p.new_done) {
            Phase::New
        } else {
            Phase::Backlog
        };
        return Ok((positions, prior));
    };
    let payload: CursorPayload = sealer
        .open(
            TokenType::Cursor,
            &session.user,
            wrapped,
            &session.session_record_id,
            token,
        )
        .await
        .map_err(|e| {
            let (status, _) = http_status_for(TokenType::Cursor, e);
            if status == 503 {
                ApiError::ProviderUnavailable {
                    mailbox_id: None,
                    retry_after_s: None,
                }
            } else {
                ApiError::InvalidRequest {
                    fields: vec!["/cursor".to_owned()],
                }
            }
        })?;
    Ok((payload.positions, payload.phase))
}

/// Step 3: a new session, or `refresh`, makes mail newer than the last card
/// shown "new" again (FD-03 AC1, AC5). The first ever use starts in the
/// backlog, because there is no earlier session to be newer than.
fn start_session(
    positions: &mut BTreeMap<Uuid, MailboxPosition>,
    records: &[Versioned<MailboxRecord>],
    session: Uuid,
    refresh: bool,
) {
    for record in records {
        if record.record.status != MailboxStatus::Connected {
            continue;
        }
        let position = positions.entry(record.record.mailbox_id.0).or_default();
        if refresh || position.session_record_id != Some(session) {
            position.new_floor = position.newest_seen;
            position.new_ceiling = None;
            position.new_done = position.newest_seen.is_none();
            position.session_record_id = Some(session);
        }
    }
}

pub(crate) fn error_code(e: &ApiError) -> Option<&'static str> {
    match e {
        ApiError::MailboxNeedsSignIn { .. } => Some("mailbox_needs_sign_in"),
        ApiError::ProviderUnavailable { .. } => Some("provider_unavailable"),
        ApiError::Internal => None,
        _ => Some("provider_error"),
    }
}

/// Step 4: a context and the decrypted address for each usable mailbox; every
/// other mailbox becomes a `mailbox_errors` entry.
async fn connect(
    app: &AppState,
    user: &UserId,
    wrapped: &WrappedKey,
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
            Ok(ctx) => {
                let aad = Aad {
                    user: *user,
                    scope: id.0.to_string(),
                    field: aad_fields::MAILBOX_EMAIL,
                };
                let plain = app
                    .ports
                    .keys
                    .open(user, wrapped, &aad, &record.record.email_address.0)
                    .await
                    .map_err(|_| ApiError::Internal)?;
                live.push(Live {
                    id,
                    provider: record.record.provider,
                    address: String::from_utf8(plain).map_err(|_| ApiError::Internal)?,
                    ctx,
                });
            }
            Err(e) => {
                let code = error_code(&e).ok_or(ApiError::Internal)?;
                errors.insert(id.0, code);
            }
        }
    }
    Ok(live)
}

/// Step 5: `new` while any usable mailbox still has new mail to page.
fn phase_of(positions: &BTreeMap<Uuid, MailboxPosition>, live: &[Live]) -> Phase {
    let any_new = live
        .iter()
        .any(|l| positions.get(&l.id.0).is_some_and(|p| !p.new_done));
    if any_new {
        Phase::New
    } else {
        Phase::Backlog
    }
}

/// Gmail `before:` is exclusive at second precision, so the bound is the
/// ceiling's second plus one; `boundary_ids` removes the repeats.
fn plus_one_second(t: OffsetDateTime) -> OffsetDateTime {
    OffsetDateTime::from_unix_timestamp(t.unix_timestamp().saturating_add(1)).unwrap_or(t)
}

fn query_for(phase: Phase, position: &MailboxPosition) -> MessageQuery {
    match phase {
        Phase::New => MessageQuery {
            in_inbox: true,
            after: position.new_floor,
            before: position.new_ceiling.map(plus_one_second),
            ..MessageQuery::default()
        },
        Phase::Backlog => MessageQuery {
            in_inbox: true,
            before: position.backlog_ceiling.map(plus_one_second),
            ..MessageQuery::default()
        },
    }
}

/// How a failed provider call shows in `mailbox_errors` (shared with the
/// progress endpoint, T-603).
pub(crate) fn mailbox_error_code(e: &MailError) -> &'static str {
    match e {
        MailError::Unauthorized => "mailbox_needs_sign_in",
        MailError::RateLimited { .. } | MailError::Transient => "provider_unavailable",
        MailError::Forbidden | MailError::NotFound | MailError::Invalid(_) => "provider_error",
    }
}

/// A provider 401 means the grant is gone: the mailbox needs sign-in. Best
/// effort; a lost race means someone else already moved it.
async fn mark_needs_sign_in(app: &AppState, mailbox: &MailboxId) {
    app.tokens.forget(mailbox);
    let Ok(Some(current)) = app.ports.store.mailboxes().get(mailbox).await else {
        return;
    };
    let mut record = current.record.clone();
    record.status = next_status(record.status, MailboxEvent::TokenInvalid);
    if record.status == current.record.status {
        return;
    }
    let _ = app
        .ports
        .store
        .mailboxes()
        .put(&record, Precondition::Matches(current.version))
        .await;
}

type ListResult<'a> = (
    &'a Live,
    u32,
    HashSet<String>,
    Result<MessagePage, MailError>,
);

async fn list_one<'a>(
    app: &AppState,
    live: &'a Live,
    query: MessageQuery,
    max: u32,
    boundary: HashSet<String>,
) -> ListResult<'a> {
    let page = app
        .ports
        .mail(live.provider)
        .list_messages(&live.ctx, &query, None, max)
        .await;
    (live, max, boundary, page)
}

/// Runs the futures with at most [`PROVIDER_CONCURRENCY`] in flight, keeping
/// their order. The futures are built first so each borrows its own data.
pub(crate) async fn bounded<I>(futures: I) -> Vec<<I::Item as std::future::Future>::Output>
where
    I: IntoIterator,
    I::Item: std::future::Future,
{
    stream::iter(futures)
        .buffered(PROVIDER_CONCURRENCY)
        .collect()
        .await
}

async fn fetch_skipped(
    app: &AppState,
    live: &HashMap<Uuid, &Live>,
    entry: SkipReturn,
) -> (SkipReturn, Result<MessageMeta, MailError>) {
    let result = match (
        live.get(&entry.mailbox_id.0),
        MessageId::new(entry.message_id.clone()),
    ) {
        (Some(l), Ok(id)) => app.ports.mail(l.provider).get_meta(&l.ctx, &id).await,
        (_, _) => Err(MailError::Transient),
    };
    (entry, result)
}

/// Step 6: list each mailbox, at most [`PROVIDER_CONCURRENCY`] in flight.
async fn fetch_round(
    app: &AppState,
    live: &[Live],
    positions: &BTreeMap<Uuid, MailboxPosition>,
    phase: Phase,
    limit: u32,
    errors: &mut BTreeMap<Uuid, &'static str>,
) -> Fetched {
    let default = MailboxPosition::default();
    let jobs: Vec<(&Live, MessageQuery, u32, HashSet<String>)> = live
        .iter()
        .filter(|l| !errors.contains_key(&l.id.0))
        .filter_map(|l| {
            let position = positions.get(&l.id.0).unwrap_or(&default);
            if phase == Phase::New && position.new_done {
                return None;
            }
            let boundary: HashSet<String> = position.boundary_ids.iter().cloned().collect();
            let extra = u32::try_from(boundary.len()).unwrap_or(PROVIDER_PAGE_MAX);
            let max = limit.saturating_add(extra).min(PROVIDER_PAGE_MAX);
            Some((l, query_for(phase, position), max, boundary))
        })
        .collect();
    let mut calls = Vec::with_capacity(jobs.len());
    for (l, query, max, boundary) in jobs {
        calls.push(list_one(app, l, query, max, boundary));
    }
    let results: Vec<ListResult<'_>> = bounded(calls).await;
    let mut out = Fetched::default();
    for (l, max, boundary, result) in results {
        match result {
            Ok(page) => {
                let more = u32::try_from(page.items.len()).is_ok_and(|n| n >= max);
                let kept: Vec<MessageMeta> = page
                    .items
                    .into_iter()
                    .filter(|m| !boundary.contains(m.id.as_str()))
                    .collect();
                out.answers.insert(
                    l.id.0,
                    Answer {
                        returned: kept.len(),
                        more,
                    },
                );
                out.metas.extend(kept);
            }
            Err(e) => {
                out.failed = true;
                errors.insert(l.id.0, mailbox_error_code(&e));
                if e == MailError::Unauthorized {
                    mark_needs_sign_in(app, &l.id).await;
                }
            }
        }
    }
    out
}

/// Step 12: move each mailbox past the cards taken from it.
fn advance_positions(
    start: &BTreeMap<Uuid, MailboxPosition>,
    taken: &[MessageMeta],
    answers: &BTreeMap<Uuid, Answer>,
    phase: Phase,
) -> BTreeMap<Uuid, MailboxPosition> {
    let mut next = start.clone();
    for (id, answer) in answers {
        let mine: Vec<&MessageMeta> = taken.iter().filter(|m| m.mailbox.0 == *id).collect();
        let Some(position) = next.get_mut(id) else {
            continue;
        };
        if let Some(newest) = mine.iter().map(|m| m.internal_date).max() {
            position.newest_seen = Some(position.newest_seen.map_or(newest, |s| s.max(newest)));
        }
        if let Some(oldest) = mine.iter().map(|m| m.internal_date).min() {
            let second = oldest.unix_timestamp();
            let previous = match phase {
                Phase::New => position.new_ceiling,
                Phase::Backlog => position.backlog_ceiling,
            };
            let mut boundary: Vec<String> =
                if previous.map(OffsetDateTime::unix_timestamp) == Some(second) {
                    position.boundary_ids.clone()
                } else {
                    Vec::new()
                };
            for m in mine
                .iter()
                .filter(|m| m.internal_date.unix_timestamp() == second)
            {
                if !boundary.iter().any(|b| b == m.id.as_str()) {
                    boundary.push(m.id.as_str().to_owned());
                }
            }
            position.boundary_ids = boundary;
            match phase {
                Phase::New => position.new_ceiling = Some(oldest),
                Phase::Backlog => position.backlog_ceiling = Some(oldest),
            }
        }
        if phase == Phase::New && !answer.more && mine.len() == answer.returned {
            position.new_done = true;
        }
    }
    next
}

fn skip_key(mailbox: Uuid, id: &str) -> String {
    format!("{mailbox}/{id}")
}

fn skip_count(skips: &SkipState, meta: &MessageMeta) -> u8 {
    skips
        .counts
        .get(&skip_key(meta.mailbox.0, meta.id.as_str()))
        .copied()
        .unwrap_or(0)
}

/// Step 10: count the page against the skip queue and fetch the entries that
/// are due. A message that left the inbox is dropped silently (FD-04); one
/// that could not be read stays queued for the next page.
async fn take_due_skips(
    app: &AppState,
    live: &HashMap<Uuid, &Live>,
    skips: &mut SkipState,
    page: &[MessageMeta],
    tunables: &Tunables,
) -> Vec<MessageMeta> {
    let shown = u32::try_from(page.len()).unwrap_or(u32::MAX);
    let on_page: HashSet<(Uuid, &str)> =
        page.iter().map(|m| (m.mailbox.0, m.id.as_str())).collect();
    let mut waiting = Vec::new();
    let mut due = Vec::new();
    for mut entry in std::mem::take(&mut skips.queue) {
        entry.after_cards = entry.after_cards.saturating_sub(shown);
        if on_page.contains(&(entry.mailbox_id.0, entry.message_id.as_str())) {
            continue;
        }
        if entry.after_cards == 0 {
            due.push(entry);
        } else {
            waiting.push(entry);
        }
    }
    let fetched: Vec<(SkipReturn, Result<MessageMeta, MailError>)> =
        bounded(due.into_iter().map(|entry| fetch_skipped(app, live, entry))).await;
    let mut returns = Vec::new();
    for (entry, result) in fetched {
        match result {
            Ok(meta) if meta.labels.contains("INBOX") => {
                let count = skip_count(skips, &meta);
                let room =
                    page.len() + returns.len() < usize::try_from(FEED_LIMIT_MAX).unwrap_or(0);
                if !card_visible(true, false, count, tunables) {
                    continue;
                }
                if room {
                    returns.push(meta);
                } else {
                    waiting.push(entry);
                }
            }
            // Out of the inbox, or gone: silently dropped.
            Ok(_) | Err(MailError::NotFound) => {}
            Err(_) => waiting.push(entry),
        }
    }
    skips.queue = waiting;
    returns
}

fn seal_error(e: TokenError) -> ApiError {
    if e == TokenError::Unavailable {
        ApiError::ProviderUnavailable {
            mailbox_id: None,
            retry_after_s: None,
        }
    } else {
        ApiError::Internal
    }
}

/// T-607b: the filing suggestion and the keep prompt for one card. Pure and in
/// memory (FL-01 AC3), and a suggestion never files anything (FL-03 AC2).
fn filing_dtos(
    env: &Env<'_>,
    sender_key: &str,
    sender_domain: &str,
    class: domain::MessageClass,
) -> (SuggestionDto, Option<CategoryRefDto>) {
    let input = FilingInput {
        sender_key,
        sender_domain,
        class,
        categories: &env.state.categories,
        sender_stats: &env.state.sender_stats,
    };
    let has_file_rule = env.state.rules.iter().any(|stored| {
        stored.rule.enabled
            && stored.rule.kind == RuleKind::File
            && stored.rule.matcher.sender.as_str() == sender_key
    });
    let suggestion = filing::suggest(&input);
    let suggestion = SuggestionDto {
        category_id: suggestion.category.map(|category| category.0),
        name: suggestion
            .name
            .as_deref()
            .map(|name| plain_text(name, CATEGORY_NAME_MAX)),
        alternates: suggestion
            .alternates
            .iter()
            .map(|(category, name)| CategoryRefDto {
                category_id: category.0,
                name: plain_text(name, CATEGORY_NAME_MAX),
            })
            .collect(),
        confidence: suggestion.confidence.as_str(),
    };
    let keep_prompt =
        filing::keep_prompt(&input, has_file_rule, env.tunables).map(|(category, name)| {
            CategoryRefDto {
                category_id: category.0,
                name: plain_text(&name, CATEGORY_NAME_MAX),
            }
        });
    (suggestion, keep_prompt)
}

/// The per-request bake-off gate: T-902 consent AND the T-906b switch, failing
/// closed on missing records or store errors. Any gate injected on the state
/// (integration tests drive the pipeline that way) opens it too; production
/// leaves that field closed.
async fn input_gate(app: &AppState, user: &UserId) -> crate::experiments::BakeoffGate {
    let Ok(Some(user)) = app.ports.store.users().get(user).await else {
        return crate::experiments::BakeoffGate::default();
    };
    let Ok(Some(switches)) = app.ports.store.config().get_classifiers().await else {
        return crate::experiments::BakeoffGate::default();
    };
    crate::experiments::bakeoff_gate(
        &user.record,
        (switches.record.gemini_enabled, switches.record.jev_enabled),
    )
}

/// The effective gate for this request: consent and the model switches, plus
/// the gate injected on the state (tests). The injected field is `Default`
/// (closed) in production, so it never widens the real gate there.
fn effective_gate(env: &Env<'_>) -> BakeoffGate {
    BakeoffGate {
        gemini: env.gate.gemini || env.app.bakeoff_gate.gemini,
        jev: env.gate.jev || env.app.bakeoff_gate.jev,
    }
}

/// One card's fetched text. `text` is what the models see (the full fetch when
/// the gate is open); `preview` is the truncated, sanitised card preview.
struct Prepared {
    meta: MessageMeta,
    text: String,
    preview: String,
}

/// Step 11: fetch each card's text, run one classification pass for the page
/// (T-901), then build the cards.
async fn build_cards(env: &Env<'_>, all: Vec<MessageMeta>) -> Result<Vec<Built>, ApiError> {
    let gate = effective_gate(env);
    let prepared: Vec<Option<Prepared>> =
        bounded(all.into_iter().map(|m| prepare(env, m, gate))).await;
    let prepared: Vec<Prepared> = prepared.into_iter().flatten().collect();
    // One pipeline for every card on the page (CL-01 AC1).
    let to_classify: Vec<CardToClassify<'_>> = prepared
        .iter()
        .map(|p| CardToClassify {
            meta: &p.meta,
            stripped_text: &p.text,
            provider: env
                .live
                .get(&p.meta.mailbox.0)
                .map_or(Provider::Gmail, |l| l.provider),
        })
        .collect();
    let classified = classify_page(
        &env.app.classifiers,
        gate,
        env.app.ports.clock.now(),
        &to_classify,
    )
    .await;
    drop(to_classify);
    let built: Vec<Option<Built>> = bounded(
        prepared
            .into_iter()
            .zip(classified)
            .map(|(prepared, classified)| build_card(env, prepared, classified)),
    )
    .await
    .into_iter()
    .collect::<Result<Vec<_>, ApiError>>()?;
    Ok(built.into_iter().flatten().collect())
}

/// Step 11, first half: the text for one card. `None` drops the message
/// silently (FD-04 AC1).
async fn prepare(env: &Env<'_>, meta: MessageMeta, gate: BakeoffGate) -> Option<Prepared> {
    let live = env.live.get(&meta.mailbox.0)?;
    let provider = env.app.ports.mail(live.provider);
    let text = if gate.is_open() {
        provider
            .get_text(&live.ctx, &meta.id, domain::redact::MODEL_TEXT_FETCH_CHARS)
            .await
    } else {
        provider.get_preview(&live.ctx, &meta.id).await
    };
    match text {
        Ok(text) => {
            let preview = domain::text::sanitise_plain(&text, PREVIEW_MAX_CHARS);
            Some(Prepared {
                meta,
                text,
                preview,
            })
        }
        Err(MailError::NotFound) => None,
        Err(_) => Some(Prepared {
            meta,
            text: String::new(),
            preview: String::new(),
        }),
    }
}

/// Step 11, second half: one card from its classification. The badge is the
/// guarded header-rules answer; model output only travels inside the token.
async fn build_card(
    env: &Env<'_>,
    prepared: Prepared,
    classified: ClassifiedCard,
) -> Result<Option<Built>, ApiError> {
    let Prepared { meta, preview, .. } = prepared;
    let ClassifiedCard {
        badge: shown,
        payload,
    } = classified;
    let Some(live) = env.live.get(&meta.mailbox.0) else {
        return Ok(None);
    };
    let provider = env.app.ports.mail(live.provider);
    let (method, one_click) = match HeaderRules::unsubscribe_route(&meta.facts) {
        UnsubscribeRoute::OneClick(_) => ("one_click", true),
        UnsubscribeRoute::Mailto(_) => ("mailto", false),
        UnsubscribeRoute::ManualLink(_) => ("manual", false),
        UnsubscribeRoute::None => ("none", false),
    };
    let classification_token = env
        .sealer
        .seal(
            TokenType::Classification,
            env.user,
            env.wrapped,
            &env.session.session_record_id,
            env.expires_at,
            &payload,
        )
        .await
        .map_err(seal_error)?;
    let sender_key = meta.sender.as_str().to_owned();
    let boss = if is_boss(&sender_key, &env.state.sender_stats, env.tunables) {
        let query = MessageQuery {
            in_inbox: true,
            from: Some(sender_key.clone()),
            ..MessageQuery::default()
        };
        provider
            .count_messages(&live.ctx, &query)
            .await
            .ok()
            .map(|remaining| BossDto { remaining })
    } else {
        None
    };
    let sender_name = plain_text(&meta.from_display, NAME_MAX);
    // T-607b: the filing suggestion and the keep prompt (FL-01, FL-03, FL-04,
    // SW-04). `filing_dtos` stays pure and in memory (FL-01 AC3) and never
    // files anything (FL-03 AC2).
    let (suggestion, keep_prompt) =
        filing_dtos(env, &sender_key, meta.sender.domain(), shown.class);
    let card = CardDto {
        mailbox_id: meta.mailbox.0,
        message_id: meta.id.as_str().to_owned(),
        received_at: meta.internal_date,
        sender_name: sender_name.clone(),
        sender_address: plain_text(&meta.from_address, ADDRESS_MAX),
        subject: plain_text(&meta.subject, SUBJECT_MAX),
        preview,
        bulk_score: shown.bulk_score,
        bulk_reason: plain_text(&shown.bulk_reason, REASON_MAX),
        class: shown.class.as_str(),
        unsubscribe_method: method,
        has_one_click: one_click && shown.class == domain::MessageClass::List,
        suggestion: Some(suggestion),
        keep_prompt,
        skip_count: skip_count(env.skips, &meta),
        boss,
        provider_web_url: provider.web_url(&live.address, &meta.id),
        classification_token,
        classifier_id: env.session.is_admin.then(|| HEADER_RULES_ID.to_owned()),
    };
    let delivery_mail = ListMail {
        sender_key: sender_key.clone(),
        list_id: meta.facts.list_id.clone(),
        mailbox_id: meta.mailbox,
        received_at: meta.internal_date,
        sender_display: sender_name.clone(),
    };
    Ok(Some(Built {
        card,
        sender_key,
        display: sender_name,
        delivery_mail,
    }))
}
