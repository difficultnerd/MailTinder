//! The `reject` arm of API-SW-1 (T-605): trash (or report spam and trash), the
//! reject rule, the queued unsubscribe job and its task, the https-only Needs
//! Attention item, the mail-stopped estimate, the block prompt and boss defeat.
//!
//! Called from `swipe::swipe` after the shared steps (idempotency, ownership,
//! classification token, fresh re-read, plan). The daily job limit
//! (`policies::UNSUB_JOBS`) is checked here, before any provider change, so the
//! route handler stays thin and a swipe counts one hit.
//!
//! Nothing here is logged: the target, the sender and the message ID never
//! leave the sealed job, the sealed item and the user's state file.

use std::sync::Arc;

use domain::feed::{defeat_boss_on_reject, is_boss};
use domain::undo::SwipeRecord;
use domain::user_state::{
    HistoryAction, HistoryEntry, HistoryOutcome, PendingUnsubscribe, RecentSwipe, StoredRule,
    UserState,
};
use domain::{
    yearly_rate, JobId, JobState, JobStatus, LabelSet, MailboxChange, MailboxId, MessageMeta,
    NeedsAttentionReason, NewNeedsAttention, RuleId, SenderStats, SortRule, SwipeAction, SwipePlan,
    Tunables, UnsubscribePlan, UnsubscribeTarget, UserId,
};
use ports::store::{aad_fields, NeedsAttentionId};
use ports::{
    Aad, Ciphertext, JobRecord, MailError, MailProvider, MailboxCtx, MessageQuery,
    NeedsAttentionRecord, Precondition, StoreError, WrappedKey,
};
use serde::{Deserialize, Serialize};
use time::{Duration, OffsetDateTime};
use url::Url;
use uuid::Uuid;

use crate::error::ApiError;
use crate::http::request_id::RequestId;
use crate::limits::{policies, LimitSubject};
use crate::routes::swipes::{PromptDto, PromptKindDto, SwipeResultDto};
use crate::sealed::{SealedTokens, TokenType};
use crate::services::categories::provider_error;
use crate::services::list_key::{list_key_hash, HmacKey};
use crate::services::swipe::{
    provider_of, seal_error, stored_json, wrapped_key, SealCtx, UndoPayload, SWIPE_TOKEN_TTL_HOURS,
};
use crate::services::user_state_store::UserStateStore;
use crate::session::extract::AuthedSession;
use crate::state::AppState;

/// S2 `UNSUB_DELAY` [TUNABLE]: lives in `Tunables`; the planner applies it.
pub const UNSUB_DELAY_MINUTES: i64 = 5;
/// S2 `JOB_TTL`: lives in `Tunables`; `JobState::new_queued` applies it.
pub const JOB_TTL_MINUTES: i64 = 60;
/// S2 `NEEDS_ATTENTION_TTL`: lives in `Tunables`; `NewNeedsAttention` applies it.
pub const NEEDS_ATTENTION_TTL_DAYS: i64 = 30;
/// S2 GM-05 AC1: the window the mail-stopped estimate is counted over.
pub const GM05_WINDOW_DAYS: i64 = 90;
/// S7 section 6 [TUNABLE]: queued jobs per user per day (`policies::UNSUB_JOBS`).
pub const JOBS_PER_DAY: u32 = 300;
/// [DEFAULT] fixed namespace for the Needs Attention item ID.
pub const NS_NA_ITEM: Uuid = Uuid::from_u128(0x6d74_6e61_0000_4000_8000_0000_0000_0001);

/// The sealed content of a `block_person` prompt reference (PB-01 AC1), opened
/// by T-608.
#[derive(Serialize, Deserialize)]
pub struct BlockPromptRef {
    /// The sender the block rule would match.
    pub sender_key: String,
    /// The display name the prompt showed.
    pub sender_display: String,
    /// The mailbox the rejected message was in.
    pub mailbox_id: Uuid,
    /// The swipe that raised the prompt.
    pub swipe_id: Uuid,
}

/// Carry out a planned `reject` (API-SW-1, steps 1 to 9 of T-605).
///
/// # Errors
///
/// `RateLimited` over the daily job limit (before any provider change),
/// `ProviderError`, `ProviderUnavailable` or `MailboxNeedsSignIn` from the
/// provider, `ProviderUnavailable` when the job cannot be stored or scheduled
/// (the trash is then undone), `Internal` for a store failure.
pub async fn execute(
    app: &AppState,
    session: &AuthedSession,
    ctx: &MailboxCtx,
    meta: &MessageMeta,
    plan: &SwipePlan,
    swipe_id: Uuid,
) -> Result<SwipeResultDto, ApiError> {
    let user = session.user;
    let now = app.ports.clock.now();
    let tunables = Tunables::default();

    // Step 1: the rate limit runs before the provider change.
    if plan.unsubscribe.is_some() {
        let rid = RequestId(app.ports.rng.uuid_v4());
        app.limits
            .check(&policies::UNSUB_JOBS, LimitSubject::User(&user), rid)
            .await?;
    }

    let provider = provider_of(app, &meta.mailbox).await?;
    let mail = app.ports.mail(provider);
    let wrapped = wrapped_key(app, &user).await?;

    // Step 2: the provider change.
    let previous_labels = change_mailbox(&**mail, ctx, meta, plan).await?;

    // Steps 3 and 4: the job (never without the trash) and the manual item.
    if let Some(unsub) = &plan.unsubscribe {
        let queued = queue_job(app, &user, &wrapped, meta, unsub, &tunables).await;
        if let Err(e) = queued {
            rollback_job(app, &**mail, ctx, meta, unsub.job_id, &previous_labels).await;
            return Err(e);
        }
    }
    let na_item = match &plan.manual_unsubscribe {
        Some(manual) => {
            let link = manual.link.as_ref();
            Some(raise_item(app, &user, &wrapped, meta, link, now, &tunables).await?)
        }
        None => None,
    };

    // Step 5: the mail-stopped estimate, one count over all folders.
    let rate = if plan.rule.is_some() || plan.unsubscribe.is_some() {
        mail_stopped_rate(&**mail, ctx, meta, now).await
    } else {
        None
    };

    // Steps 6 to 9: read state, build the response, write once.
    let store = UserStateStore::new(Arc::new(app.clone()));
    let loaded = store.load(&user).await?;
    let state = &loaded.state;
    let was_boss = is_boss(meta.sender.as_str(), &state.sender_stats, &tunables);
    let mut stats_after = plan.stats_after.clone();
    let boss_defeated = defeat_boss_on_reject(&mut stats_after, was_boss);
    let new_rule = new_rule_for(state, plan, rate);

    let expires_at = now + Duration::hours(SWIPE_TOKEN_TTL_HOURS);
    let sealer = SealedTokens::new(Arc::clone(&app.ports.keys), Arc::clone(&app.ports.clock));
    let seal = SealCtx {
        sealer: &sealer,
        session,
        user: &user,
        wrapped: &wrapped,
        expires_at,
    };
    let mut prompts = Vec::new();
    if plan.block_prompt {
        prompts.push(block_prompt(&seal, meta, swipe_id).await?);
    }

    let suspect = plan.change == MailboxChange::ReportSpamAndTrash;
    let undo = undo_payload(
        plan,
        meta,
        now,
        swipe_id,
        previous_labels,
        new_rule.as_ref(),
        na_item,
    );
    let result = SwipeResultDto {
        outcome: plan.outcome,
        unsubscribe_due_at: plan.unsubscribe.as_ref().map(|u| u.due_at),
        filed_category: None,
        undo_token: seal.seal_undo(&undo).await?,
        prompts,
        achievements_unlocked: Vec::new(),
        boss_defeated,
    };
    let writes = Writes {
        sid: swipe_id,
        session: session.session_record_id.0,
        now,
        mailbox: meta.mailbox,
        sender_key: meta.sender.as_str().to_owned(),
        display: meta.from_display.clone(),
        list_id: meta.facts.list_id.clone(),
        stats_after,
        new_rule,
        job: plan.unsubscribe.as_ref().map(|u| u.job_id),
        suspect,
        stored_json: stored_json(&result, &undo)?,
    };
    persist(&store, &user, writes).await?;
    Ok(result)
}

/// Step 2: trash, or report spam and then trash. Returns the labels before the
/// change (`previous_labels`).
async fn change_mailbox(
    mail: &dyn MailProvider,
    ctx: &MailboxCtx,
    meta: &MessageMeta,
    plan: &SwipePlan,
) -> Result<LabelSet, ApiError> {
    let to_api = |e: MailError| provider_error(meta.mailbox, &e);
    if plan.change == MailboxChange::ReportSpamAndTrash {
        let previous = mail.report_spam(ctx, &meta.id).await.map_err(to_api)?;
        mail.trash(ctx, &meta.id).await.map_err(to_api)?;
        Ok(previous)
    } else {
        mail.trash(ctx, &meta.id).await.map_err(to_api)
    }
}

/// The rule this swipe creates: `None` when the plan has none or an enabled
/// rule with the same match exists already (undo must not remove that one).
fn new_rule_for(state: &UserState, plan: &SwipePlan, rate: Option<u32>) -> Option<StoredRule> {
    plan.rule
        .as_ref()
        .filter(|rule| !state.rules.iter().any(|r| same_rule(&r.rule, rule)))
        .map(|rule| StoredRule {
            rule: rule.clone(),
            times_applied: 0,
            yearly_rate: rate,
        })
}

/// Step 9: the undo payload. `rule_id` is set only when this swipe created the
/// rule.
fn undo_payload(
    plan: &SwipePlan,
    meta: &MessageMeta,
    now: OffsetDateTime,
    swipe_id: Uuid,
    previous_labels: LabelSet,
    new_rule: Option<&StoredRule>,
    na_item: Option<NeedsAttentionId>,
) -> UndoPayload {
    let mut record = SwipeRecord::from_plan(plan, meta, SwipeAction::Reject, now);
    record.previous_labels = previous_labels;
    record.rule_id = new_rule.map(|r| r.rule.rule_id);
    UndoPayload {
        record,
        swipe_id,
        history_entry: (plan.change == MailboxChange::ReportSpamAndTrash).then_some(swipe_id),
        na_item: na_item.map(|id| id.0),
        skip_key: None,
    }
}

/// The plaintext sealed as the job target: the method name, then the target
/// parts, joined with `\n`. A one-click URL is `one_click\n<url>`; a mailto is
/// `mailto\n<to>\n<subject>\n<body>` with empty strings for absent parts (the
/// `MailtoTarget` constructor refuses a newline in any part). T-701 splits on
/// `\n` to read it back.
fn target_string(target: &UnsubscribeTarget) -> String {
    match target {
        UnsubscribeTarget::OneClick(url) => format!("one_click\n{url}"),
        UnsubscribeTarget::Mailto(m) => format!(
            "mailto\n{}\n{}\n{}",
            m.to(),
            m.subject().unwrap_or_default(),
            m.body().unwrap_or_default()
        ),
    }
}

/// Seal one field under the user's key with the AAD `{user, scope, field}`.
async fn seal_field(
    app: &AppState,
    user: &UserId,
    wrapped: &WrappedKey,
    scope: Uuid,
    field: &'static str,
    plain: &str,
) -> Result<Ciphertext, ApiError> {
    let aad = Aad {
        user: *user,
        scope: scope.to_string(),
        field,
    };
    app.ports
        .keys
        .seal(user, wrapped, &aad, plain.as_bytes())
        .await
        .map(Ciphertext)
        .map_err(|_| ApiError::Internal)
}

/// Step 3: create the job record, then its task. `AlreadyExists` on the record
/// is a retry and counts as success. Any failure is `503`.
async fn queue_job(
    app: &AppState,
    user: &UserId,
    wrapped: &WrappedKey,
    meta: &MessageMeta,
    unsub: &UnsubscribePlan,
    tunables: &Tunables,
) -> Result<(), ApiError> {
    let unavailable = || ApiError::ProviderUnavailable {
        mailbox_id: Some(meta.mailbox.0),
        retry_after_s: None,
    };
    let scope = unsub.job_id.0;
    let target = seal_field(
        app,
        user,
        wrapped,
        scope,
        aad_fields::JOB_TARGET,
        &target_string(&unsub.target),
    )
    .await
    .map_err(|_| unavailable())?;
    let sender_display = seal_field(
        app,
        user,
        wrapped,
        scope,
        aad_fields::JOB_SENDER_DISPLAY,
        &meta.from_display,
    )
    .await
    .map_err(|_| unavailable())?;
    let state = JobState::new_queued(unsub.due_at, tunables);
    let key = HmacKey(app.config.email_lookup_key.clone());
    let record = JobRecord {
        job_id: unsub.job_id,
        user_id: *user,
        mailbox_id: meta.mailbox,
        list_key_hash: list_key_hash(&key, &meta.sender, meta.facts.list_id.as_deref()),
        method: unsub.method,
        target: Some(target),
        sender_display: Some(sender_display),
        due_at: unsub.due_at,
        status: JobStatus::Queued,
        attempts: 0,
        outcome: None,
        expires_at: state.expires_at,
    };
    match app
        .ports
        .store
        .jobs()
        .put(&record, Precondition::MustNotExist)
        .await
    {
        Ok(_) | Err(StoreError::AlreadyExists) => {}
        Err(_) => return Err(unavailable()),
    }
    // The scheduler is idempotent per job: a second schedule is a success.
    app.ports
        .scheduler
        .schedule(&unsub.job_id, unsub.due_at)
        .await
        .map(|_| ())
        .map_err(|_| unavailable())
}

/// Step 3.4: remove the job and put the exact labels back. Best effort: the
/// caller is already returning an error.
async fn rollback_job(
    app: &AppState,
    mail: &dyn MailProvider,
    ctx: &MailboxCtx,
    meta: &MessageMeta,
    job: JobId,
    previous: &LabelSet,
) {
    let _ = app
        .ports
        .store
        .jobs()
        .delete(&job, Precondition::None)
        .await;
    let _ = mail.restore_labels(ctx, &meta.id, previous).await;
}

/// Step 4: the https-only Needs Attention item, keyed on user and sender so a
/// retry or a second reject of the same sender does not duplicate it.
async fn raise_item(
    app: &AppState,
    user: &UserId,
    wrapped: &WrappedKey,
    meta: &MessageMeta,
    link: Option<&Url>,
    now: OffsetDateTime,
    tunables: &Tunables,
) -> Result<NeedsAttentionId, ApiError> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(user.0.as_bytes());
    bytes.extend_from_slice(meta.sender.as_str().as_bytes());
    let item_id = NeedsAttentionId(Uuid::new_v5(&NS_NA_ITEM, &bytes));
    let item = NewNeedsAttention::new(
        NeedsAttentionReason::HttpsOnlyUnsubscribe,
        link.cloned(),
        now,
        tunables,
    );
    let scope = item_id.0;
    let sender_display = seal_field(
        app,
        user,
        wrapped,
        scope,
        aad_fields::NA_SENDER_DISPLAY,
        &meta.from_display,
    )
    .await?;
    let link = match &item.link {
        Some(url) => {
            Some(seal_field(app, user, wrapped, scope, aad_fields::NA_LINK, url.as_str()).await?)
        }
        None => None,
    };
    let record = NeedsAttentionRecord {
        item_id,
        user_id: *user,
        mailbox_id: meta.mailbox,
        sender_display,
        link,
        reason_code: item.reason,
        created_at: item.created_at,
        expires_at: item.expires_at,
    };
    match app
        .ports
        .store
        .needs_attention()
        .put(&record, Precondition::MustNotExist)
        .await
    {
        Ok(_) | Err(StoreError::AlreadyExists) => Ok(item_id),
        Err(e) => Err(e.into()),
    }
}

/// Step 5: the 90-day count of the sender's (and list's) mail, scaled to a
/// year. A failed count is `None`; the rule is still created (GM-05 AC3).
async fn mail_stopped_rate(
    mail: &dyn MailProvider,
    ctx: &MailboxCtx,
    meta: &MessageMeta,
    now: OffsetDateTime,
) -> Option<u32> {
    let window = Duration::days(GM05_WINDOW_DAYS);
    let query = MessageQuery {
        after: Some(now - window),
        from: Some(meta.sender.as_str().to_owned()),
        list_id: meta.facts.list_id.clone(),
        ..MessageQuery::default()
    };
    let count = mail.count_messages(ctx, &query).await.ok();
    yearly_rate(count, window)
}

/// Step 7: the sealed `block_person` prompt, valid as long as the undo token.
async fn block_prompt(
    seal: &SealCtx<'_>,
    meta: &MessageMeta,
    swipe_id: Uuid,
) -> Result<PromptDto, ApiError> {
    let payload = BlockPromptRef {
        sender_key: meta.sender.as_str().to_owned(),
        sender_display: meta.from_display.clone(),
        mailbox_id: meta.mailbox.0,
        swipe_id,
    };
    let prompt_ref = seal
        .sealer
        .seal(
            TokenType::PromptRef,
            seal.user,
            seal.wrapped,
            &seal.session.session_record_id,
            seal.expires_at,
            &payload,
        )
        .await
        .map_err(seal_error)?;
    Ok(PromptDto {
        kind: PromptKindDto::BlockPerson,
        prompt_ref,
        sender_name: meta.from_display.clone(),
    })
}

/// Two rules match the same mail: same kind and same matcher.
fn same_rule(a: &SortRule, b: &SortRule) -> bool {
    a.enabled && a.kind == b.kind && a.matcher == b.matcher
}

/// Everything step 8's one state write needs, all computed beforehand.
struct Writes {
    sid: Uuid,
    session: Uuid,
    now: OffsetDateTime,
    mailbox: MailboxId,
    sender_key: String,
    display: String,
    list_id: Option<String>,
    stats_after: SenderStats,
    new_rule: Option<StoredRule>,
    job: Option<JobId>,
    suspect: bool,
    stored_json: String,
}

/// Step 8: one `UserStateStore::update`. The closure is pure over `w`, so a
/// retry after an `ETag` conflict is safe.
async fn persist(store: &UserStateStore, user: &UserId, w: Writes) -> Result<(), ApiError> {
    store
        .update(user, move |s: &mut UserState| apply(s, &w))
        .await
}

/// The state change of one reject.
fn apply(s: &mut UserState, w: &Writes) {
    s.sender_stats
        .insert(w.sender_key.clone(), w.stats_after.clone());
    let mut rule_id: Option<RuleId> = None;
    if let Some(stored) = &w.new_rule {
        if !s.rules.iter().any(|r| same_rule(&r.rule, &stored.rule)) {
            rule_id = Some(stored.rule.rule_id);
            s.rules.push(stored.clone());
            s.totals.senders_silenced = s.totals.senders_silenced.saturating_add(1);
        }
    }
    if let Some(job) = w.job {
        s.pending_unsubscribes.insert(
            job.0,
            PendingUnsubscribe {
                mailbox_id: w.mailbox,
                sender_display: w.display.clone(),
                sender_key: w.sender_key.clone(),
                list_id: w.list_id.clone(),
                rule_id,
                created_at: w.now,
            },
        );
        s.totals.unsubscribes_queued = s.totals.unsubscribes_queued.saturating_add(1);
        if s.totals.round_session != Some(w.session) {
            s.totals.round_session = Some(w.session);
            s.totals.round_unsubscribes = 0;
        }
        s.totals.round_unsubscribes = s.totals.round_unsubscribes.saturating_add(1);
    }
    if w.suspect {
        let _ = s.push_history(HistoryEntry {
            entry_id: w.sid,
            at: w.now,
            mailbox_id: w.mailbox,
            sender_display: w.display.clone(),
            action: HistoryAction::ReportedSpam,
            outcome: HistoryOutcome::Done,
            rule_id: None,
        });
    }
    s.totals.triaged = s.totals.triaged.saturating_add(1);
    s.totals.cleared = s.totals.cleared.saturating_add(1);
    s.recent_swipes.push(RecentSwipe {
        swipe_id: w.sid,
        at: w.now,
        result_json: w.stored_json.clone(),
    });
}
