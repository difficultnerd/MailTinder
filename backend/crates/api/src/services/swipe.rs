//! The swipe pipeline (T-604): validation, idempotency, the classification
//! token check, the fresh re-read, and the `keep`, `skip` and `file` actions.
//!
//! This is the shared machinery T-605 (`reject`) and T-606 (its undo) extend.
//! Message content flows from the provider to the response and nowhere else:
//! nothing here is logged, and the only thing written is the user's state file.

use std::sync::Arc;

use domain::feed::choose_skip_return;
use domain::undo::SwipeRecord;
use domain::user_state::{
    Category, HistoryAction, HistoryEntry, HistoryOutcome, RecentSwipe, SkipReturn, SkipState,
    UserState,
};
use domain::{
    derive_swipe_ids, header_guard, plan_swipe, CategoryId, HeaderRules, MailboxId, MessageId,
    MessageMeta, SenderStats, SwipeAction, SwipeInput, SwipePlan, Tunables, UserId,
};
use ports::{MailError, MailboxCtx, WrappedKey};
use serde::{Deserialize, Serialize};
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

use crate::error::ApiError;
use crate::routes::feed::{CategoryRefDto, ClassificationPayload};
use crate::routes::swipes::{ActionDto, SwipeRequest, SwipeResultDto};
use crate::sealed::{SealedTokens, TokenError, TokenType};
use crate::services::categories::{ensure_label_for, resolve_or_create_category};
use crate::services::user_state_store::UserStateStore;
use crate::session::extract::AuthedSession;
use crate::state::AppState;

/// [DEFAULT] fixed namespace for everything a swipe derives.
pub const NS_SWIPE: Uuid = Uuid::from_u128(0x6d74_7377_0000_4000_8000_0000_0000_0001);
/// `[DEFAULT]` the session's absolute lifetime (S7 3.2): the undo token expiry.
pub const SWIPE_TOKEN_TTL_HOURS: i64 = 12;
/// The provider's inbox label ID.
const INBOX_LABEL: &str = "INBOX";

/// The sealed undo payload: T-105b's record plus what the api needs to reverse
/// its own writes.
#[derive(Serialize, Deserialize, Clone)]
pub struct UndoPayload {
    pub record: SwipeRecord,
    pub swipe_id: Uuid,
    pub history_entry: Option<Uuid>,
    pub na_item: Option<Uuid>,
    pub skip_key: Option<String>,
}

/// The idempotency record for one swipe: the response, and the payload to
/// re-seal if the client retries.
#[derive(Serialize, Deserialize)]
struct StoredSwipe {
    result: SwipeResultDto,
    undo: UndoPayload,
}

/// The deterministic ID of a swipe: UUID v5 of the user ID and the key.
#[must_use]
pub fn swipe_id(user: &UserId, idempotency_key: Uuid) -> Uuid {
    let mut bytes = Vec::with_capacity(32);
    bytes.extend_from_slice(user.0.as_bytes());
    bytes.extend_from_slice(idempotency_key.as_bytes());
    Uuid::new_v5(&NS_SWIPE, &bytes)
}

/// Apply one swipe (API-SW-1).
///
/// # Errors
///
/// `InvalidRequest` for a malformed body or classification token, `NotFound`
/// for a mailbox that is not the user's, `MessageChanged` when the message has
/// moved, `MailboxNeedsSignIn` when a mailbox grant is gone, `ProviderError` or
/// `ProviderUnavailable` from a provider, `RateLimited` for a reject over the
/// daily job limit, `Internal` for a store failure.
pub async fn swipe(
    app: &AppState,
    session: &AuthedSession,
    idempotency_key: Uuid,
    req: SwipeRequest,
) -> Result<SwipeResultDto, ApiError> {
    validate_request(&req)?;
    let user = session.user;
    let wrapped = wrapped_key(app, &user).await?;
    let sealer = SealedTokens::new(Arc::clone(&app.ports.keys), Arc::clone(&app.ports.clock));
    let now = app.ports.clock.now();
    let tunables = Tunables::default();
    let sid = swipe_id(&user, idempotency_key);
    let expires_at = now + Duration::hours(SWIPE_TOKEN_TTL_HOURS);
    let store = UserStateStore::new(Arc::new(app.clone()));
    let seal = SealCtx {
        sealer: &sealer,
        session,
        user: &user,
        wrapped: &wrapped,
        expires_at,
    };
    let loaded = store.load(&user).await?;

    // Step 2: a retry of a swipe that already succeeded is answered from the
    // recorded response, before the re-read, because the message may be gone
    // from the inbox by now.
    if let Some(recent) = loaded
        .state
        .recent_swipes
        .iter()
        .find(|s| s.swipe_id == sid)
    {
        return rebuild(&seal, &recent.result_json).await;
    }

    // Steps 3 and 4: ownership, then the classification token.
    let mailbox = owned_mailbox(app, &user, req.mailbox_id).await?;
    let ctx = app.tokens.mailbox_ctx(app, &user, &mailbox).await?;
    check_classification_token(&sealer, session, &user, &wrapped, &req).await?;

    // Step 5: re-read; the token's class is never used for the action.
    let meta = fresh_message(app, &mailbox, &ctx, &req.message_id).await?;

    // Step 6: the plan.
    let category = match req.action {
        ActionDto::File => Some(
            resolve_or_create_category(
                app,
                &user,
                req.category_id,
                req.new_category_name.as_deref(),
                sid,
            )
            .await?,
        ),
        ActionDto::Keep | ActionDto::Skip | ActionDto::Reject => None,
    };
    let action = swipe_action(req.action, category.as_ref());
    let header_rules = HeaderRules::classify(&meta.facts, &meta.sender);
    let badge = header_guard(&meta.facts, &header_rules, Some(&header_rules)).classification;
    let stats = loaded
        .state
        .sender_stats
        .get(meta.sender.as_str())
        .cloned()
        .unwrap_or_default();
    let plan = plan_swipe(&SwipeInput {
        action,
        meta: &meta,
        badge: &badge,
        stats: &stats,
        ids: derive_swipe_ids(&user, idempotency_key),
        source_swipe: idempotency_key,
        now,
        tunables: &tunables,
    });

    // Step 7: `reject` is carried out by T-605 and returns before the
    // keep, skip and file path below.
    if action == SwipeAction::Reject {
        return crate::services::reject::execute(app, session, &ctx, &meta, &plan, sid).await;
    }

    // Steps 8 and 9: `keep` and `skip` make no provider call; `file` labels.
    let skip_queue = if action == SwipeAction::Skip {
        skip_return(app, &loaded.state, &meta, &tunables)
    } else {
        None
    };
    if let Some(cat) = category.as_ref() {
        apply_file(app, &user, &ctx, &meta, cat).await?;
    }

    // Steps 10 and 11: one state write, then the sealed undo token.
    let undo = undo_payload(&plan, &meta, action, now, sid);
    let result = build_result(&plan, category.as_ref(), &seal, &undo).await?;
    let effects = Effects {
        sid,
        session: session.session_record_id.0,
        now,
        action,
        mailbox: meta.mailbox,
        message_id: meta.id.as_str().to_owned(),
        sender_key: meta.sender.as_str().to_owned(),
        display: meta.from_display.clone(),
        stats_after: plan.stats_after.clone(),
        skip_key: skip_key(&meta.mailbox, &meta.id),
        skip_queue,
        stored_json: stored_json(&result, &undo)?,
    };
    let applied = persist(&store, &user, effects).await?;
    if applied {
        return Ok(result);
    }
    // The losing side of a concurrent request with the same `Idempotency-Key`:
    // answer from the response the winner recorded, exactly as the sequential
    // retry path does, and never apply the effects twice (ASVS V2.3.4).
    answer_recorded(&store, &user, sid, &seal, result).await
}

/// The request body rules (S7 5.5): `file` needs exactly one of `category_id`
/// and `new_category_name`; the others need neither.
fn validate_request(req: &SwipeRequest) -> Result<(), ApiError> {
    let has_id = req.category_id.is_some();
    let has_name = req.new_category_name.is_some();
    if req.action == ActionDto::File {
        if has_id == has_name {
            return Err(ApiError::InvalidRequest {
                fields: vec!["category_id".to_owned()],
            });
        }
        if has_name {
            crate::services::categories::validate_category_name(
                req.new_category_name.as_deref().unwrap_or_default(),
            )?;
        }
    } else if has_id || has_name {
        return Err(ApiError::InvalidRequest {
            fields: vec!["category_id".to_owned()],
        });
    }
    Ok(())
}

/// The `SwipeAction` for the request, with the resolved category for `file`.
fn swipe_action(action: ActionDto, category: Option<&Category>) -> SwipeAction {
    match action {
        ActionDto::Keep => SwipeAction::Keep,
        ActionDto::Skip => SwipeAction::Skip,
        ActionDto::Reject => SwipeAction::Reject,
        ActionDto::File => SwipeAction::File {
            category: category.map_or(CategoryId(Uuid::nil()), |c| c.category_id),
        },
    }
}

/// The user's wrapped `data_key`, needed to seal the undo token.
pub(crate) async fn wrapped_key(app: &AppState, user: &UserId) -> Result<WrappedKey, ApiError> {
    app.ports
        .store
        .users()
        .get(user)
        .await?
        .map(|u| u.record.wrapped_data_key)
        .ok_or(ApiError::Unauthenticated)
}

/// Step 3: the mailbox must belong to the user, else `404` (never `403`, so
/// IDs cannot be probed).
async fn owned_mailbox(
    app: &AppState,
    user: &UserId,
    mailbox_id: Uuid,
) -> Result<MailboxId, ApiError> {
    let record = app
        .ports
        .store
        .mailboxes()
        .get(&MailboxId(mailbox_id))
        .await?
        .ok_or(ApiError::NotFound)?;
    if record.record.user_id != *user {
        return Err(ApiError::NotFound);
    }
    Ok(record.record.mailbox_id)
}

/// Step 4: the classification token. Its clear type must be `classification`
/// and, if it opens, it must name this mailbox and message; every other open
/// failure just means no `classifier_eval` record is written (T-906a).
async fn check_classification_token(
    sealer: &SealedTokens,
    session: &AuthedSession,
    user: &UserId,
    wrapped: &WrappedKey,
    req: &SwipeRequest,
) -> Result<(), ApiError> {
    let opened = sealer
        .open::<ClassificationPayload>(
            TokenType::Classification,
            user,
            wrapped,
            &session.session_record_id,
            &req.classification_token,
        )
        .await;
    match opened {
        Ok(payload) => {
            if payload.mailbox_id != req.mailbox_id || payload.message_id != req.message_id {
                Err(ApiError::InvalidRequest {
                    fields: vec!["classification_token".to_owned()],
                })
            } else {
                Ok(())
            }
        }
        Err(TokenError::Unavailable) => Err(ApiError::ProviderUnavailable {
            mailbox_id: None,
            retry_after_s: None,
        }),
        Err(TokenError::WrongType | TokenError::Malformed) => Err(ApiError::InvalidRequest {
            fields: vec!["classification_token".to_owned()],
        }),
        // Expired, for another user or session, or tampered with: proceed.
        Err(_) => Ok(()),
    }
}

/// Step 5: the fresh re-read. A message that is gone, or has left the inbox,
/// gives `409 message_changed`.
async fn fresh_message(
    app: &AppState,
    mailbox: &MailboxId,
    ctx: &MailboxCtx,
    message_id: &str,
) -> Result<MessageMeta, ApiError> {
    let provider = provider_of(app, mailbox).await?;
    let id = MessageId::new(message_id).map_err(|_| ApiError::InvalidRequest {
        fields: vec!["message_id".to_owned()],
    })?;
    let meta = match app.ports.mail(provider).get_meta(ctx, &id).await {
        Ok(meta) => meta,
        Err(MailError::NotFound) => return Err(ApiError::MessageChanged),
        Err(e) => return Err(crate::services::categories::provider_error(*mailbox, &e)),
    };
    if !meta.labels.contains(INBOX_LABEL) {
        return Err(ApiError::MessageChanged);
    }
    Ok(meta)
}

/// The provider behind a mailbox record.
pub(crate) async fn provider_of(
    app: &AppState,
    mailbox: &MailboxId,
) -> Result<domain::Provider, ApiError> {
    let record = app
        .ports
        .store
        .mailboxes()
        .get(mailbox)
        .await?
        .ok_or(ApiError::NotFound)?;
    Ok(record.record.provider)
}

/// Step 9: label the message and drop it from the inbox.
async fn apply_file(
    app: &AppState,
    user: &UserId,
    ctx: &MailboxCtx,
    meta: &MessageMeta,
    category: &Category,
) -> Result<(), ApiError> {
    let label = ensure_label_for(app, user, ctx, category).await?;
    let provider = provider_of(app, &meta.mailbox).await?;
    let add = domain::LabelSet::from_ids([label]);
    let remove = domain::LabelSet::from_ids([INBOX_LABEL.to_owned()]);
    app.ports
        .mail(provider)
        .set_labels(ctx, &meta.id, &add, &remove)
        .await
        .map_err(|e| crate::services::categories::provider_error(meta.mailbox, &e))?;
    Ok(())
}

/// Step 8: how many cards later a skipped card comes back, or `None` when the
/// session's return cap is reached.
fn skip_return(
    app: &AppState,
    state: &UserState,
    meta: &MessageMeta,
    tunables: &Tunables,
) -> Option<u32> {
    let prior = state
        .skips
        .counts
        .get(&skip_key(&meta.mailbox, &meta.id))
        .copied()
        .unwrap_or(0);
    let mut draw = [0_u8; 8];
    draw.copy_from_slice(&app.ports.rng.uuid_v4().as_bytes()[..8]);
    choose_skip_return(
        prior.saturating_add(1),
        0,
        u64::from_le_bytes(draw),
        tunables,
    )
}

/// The wire key a skip is counted under, `mailbox_id/message_id`.
fn skip_key(mailbox: &MailboxId, id: &MessageId) -> String {
    format!("{}/{}", mailbox.0, id.as_str())
}

/// The sealed undo payload for a swipe.
fn undo_payload(
    plan: &SwipePlan,
    meta: &MessageMeta,
    action: SwipeAction,
    now: OffsetDateTime,
    sid: Uuid,
) -> UndoPayload {
    UndoPayload {
        record: SwipeRecord::from_plan(plan, meta, action, now),
        swipe_id: sid,
        history_entry: matches!(action, SwipeAction::File { .. }).then_some(sid),
        na_item: None,
        skip_key: matches!(action, SwipeAction::Skip).then(|| skip_key(&meta.mailbox, &meta.id)),
    }
}

/// The sealing inputs shared by the response and the retry path.
pub(crate) struct SealCtx<'a> {
    pub(crate) sealer: &'a SealedTokens,
    pub(crate) session: &'a AuthedSession,
    pub(crate) user: &'a UserId,
    pub(crate) wrapped: &'a WrappedKey,
    pub(crate) expires_at: OffsetDateTime,
}

impl SealCtx<'_> {
    /// Seal an undo payload for the current session and expiry.
    pub(crate) async fn seal_undo(&self, undo: &UndoPayload) -> Result<String, ApiError> {
        self.sealer
            .seal(
                TokenType::Undo,
                self.user,
                self.wrapped,
                &self.session.session_record_id,
                self.expires_at,
                undo,
            )
            .await
            .map_err(seal_error)
    }
}

/// Step 11: seal the undo payload and shape the response.
async fn build_result(
    plan: &SwipePlan,
    category: Option<&Category>,
    seal: &SealCtx<'_>,
    undo: &UndoPayload,
) -> Result<SwipeResultDto, ApiError> {
    let undo_token = seal.seal_undo(undo).await?;
    Ok(SwipeResultDto {
        outcome: plan.outcome,
        unsubscribe_due_at: plan.unsubscribe.as_ref().map(|u| u.due_at),
        filed_category: category.map(|c| CategoryRefDto {
            category_id: c.category_id.0,
            name: c.name.clone(),
        }),
        undo_token,
        prompts: Vec::new(),
        achievements_unlocked: Vec::new(),
        boss_defeated: false,
    })
}

/// The serialised idempotency record for one swipe.
pub(crate) fn stored_json(result: &SwipeResultDto, undo: &UndoPayload) -> Result<String, ApiError> {
    serde_json::to_string(&StoredSwipe {
        result: result.clone(),
        undo: undo.clone(),
    })
    .map_err(|_| ApiError::Internal)
}

/// Step 2, retry path: rebuild the recorded response and re-seal its undo
/// payload under the current session.
pub(crate) async fn rebuild(seal: &SealCtx<'_>, json: &str) -> Result<SwipeResultDto, ApiError> {
    let stored: StoredSwipe = serde_json::from_str(json).map_err(|_| ApiError::Internal)?;
    let mut result = stored.result;
    result.undo_token = seal.seal_undo(&stored.undo).await?;
    Ok(result)
}

/// The losing side of a concurrent same-key swipe: the response the winner
/// recorded, re-sealed for the caller's session. `fallback` is used only if the
/// record vanished between the write and this read, which cannot happen in the
/// same request without a concurrent undo.
pub(crate) async fn answer_recorded(
    store: &UserStateStore,
    user: &UserId,
    sid: Uuid,
    seal: &SealCtx<'_>,
    fallback: SwipeResultDto,
) -> Result<SwipeResultDto, ApiError> {
    let stored = store
        .load(user)
        .await?
        .state
        .recent_swipes
        .into_iter()
        .find(|r| r.swipe_id == sid)
        .map(|r| r.result_json);
    match stored {
        Some(json) => rebuild(seal, &json).await,
        None => Ok(fallback),
    }
}

/// Everything step 10's one state write needs.
struct Effects {
    sid: Uuid,
    session: Uuid,
    now: OffsetDateTime,
    action: SwipeAction,
    mailbox: MailboxId,
    message_id: String,
    sender_key: String,
    display: String,
    stats_after: SenderStats,
    skip_key: String,
    skip_queue: Option<u32>,
    stored_json: String,
}

/// Step 10: one `UserStateStore::update`. The closure is pure: it only copies
/// in values computed before the call, so a retry after an `ETag` conflict is
/// safe. `false` means a concurrent request with the same `Idempotency-Key`
/// already applied this swipe, so nothing here was applied.
async fn persist(store: &UserStateStore, user: &UserId, e: Effects) -> Result<bool, ApiError> {
    let sid = e.sid;
    store
        .update_once(user, sid, move |s: &mut UserState| {
            if s.skips.session_record_id != Some(e.session) {
                s.skips = SkipState {
                    session_record_id: Some(e.session),
                    ..SkipState::default()
                };
            }
            match e.action {
                SwipeAction::Skip => {
                    let count = s.skips.counts.entry(e.skip_key.clone()).or_insert(0);
                    *count = count.saturating_add(1);
                }
                SwipeAction::File { .. } => {
                    s.sender_stats
                        .insert(e.sender_key.clone(), e.stats_after.clone());
                    s.totals.triaged = s.totals.triaged.saturating_add(1);
                    s.totals.cleared = s.totals.cleared.saturating_add(1);
                    let _ = s.push_history(HistoryEntry {
                        entry_id: e.sid,
                        at: e.now,
                        mailbox_id: e.mailbox,
                        sender_display: e.display.clone(),
                        action: HistoryAction::Filed,
                        outcome: HistoryOutcome::Done,
                        rule_id: None,
                    });
                }
                SwipeAction::Keep => {
                    s.sender_stats
                        .insert(e.sender_key.clone(), e.stats_after.clone());
                    s.totals.triaged = s.totals.triaged.saturating_add(1);
                }
                SwipeAction::Reject => {}
            }
            if let Some(after_cards) = e.skip_queue {
                s.skips.queue.push(SkipReturn {
                    mailbox_id: e.mailbox,
                    message_id: e.message_id.clone(),
                    after_cards,
                });
            }
            s.recent_swipes.push(RecentSwipe {
                swipe_id: e.sid,
                at: e.now,
                result_json: e.stored_json.clone(),
            });
        })
        .await
}

/// Map a seal failure: a down key service is `503`, anything else `Internal`.
pub(crate) fn seal_error(e: TokenError) -> ApiError {
    if e == TokenError::Unavailable {
        ApiError::ProviderUnavailable {
            mailbox_id: None,
            retry_after_s: None,
        }
    } else {
        ApiError::Internal
    }
}
