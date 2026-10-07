//! The Rule service (T-608): list, create, switch off, delete and the block
//! prompt decline (S7 5.7, API-RULE-1 to API-RULE-5).
//!
//! Every write is one `UserStateStore::update`; provider reads happen before it.
//! A `block_person` matcher comes only from a server-sealed prompt, and a
//! `file` matcher's sender comes only from a fresh provider read, never from the
//! request (S7 5.7).
//!
//! Nothing here is logged: no sender address, List-Id or message ID (S5, C2).

use std::sync::Arc;

use domain::user_state::{HistoryAction, HistoryEntry, HistoryOutcome, StoredRule, UserState};
use domain::{
    CategoryId, MailboxId, MessageId, RuleId, RuleKind, SenderKey, SortRule, Tunables, UserId,
};
use ports::MailError;
use uuid::Uuid;

use crate::error::ApiError;
use crate::routes::rules::{RuleDto, RuleMatchDto, NS_RULE_FROM_PROMPT};
use crate::sealed::{SealedTokens, TokenError, TokenType};
use crate::services::categories::provider_error;
use crate::services::reject::BlockPromptRef;
use crate::services::swipe::wrapped_key;
use crate::services::user_state_store::UserStateStore;
use crate::session::extract::AuthedSession;
use crate::state::AppState;

/// API-RULE-1: the user's rules, newest first, optionally filtered by kind.
///
/// # Errors
///
/// `MailboxNeedsSignIn` or `Internal` from the state file.
pub async fn list(
    app: &AppState,
    s: &AuthedSession,
    kind: Option<&str>,
) -> Result<Vec<RuleDto>, ApiError> {
    let wanted = kind.map(kind_of);
    let store = UserStateStore::new(Arc::new(app.clone()));
    let loaded = store.load(&s.user).await?;
    let mut rules: Vec<StoredRule> = loaded
        .state
        .rules
        .iter()
        .filter(|r| wanted.is_none() || wanted == Some(r.rule.kind))
        .cloned()
        .collect();
    rules.sort_by_key(|a| std::cmp::Reverse(a.rule.created_at));
    Ok(rules.iter().map(dto).collect())
}

/// API-RULE-2 `block_person` (PB-01 AC2): build a block rule from a sealed
/// prompt reference. The sender comes from the prompt, never the client; the
/// rule ID is deterministic, so a retry finds the same rule.
///
/// # Errors
///
/// `InvalidRequest` when the prompt reference does not open or is not a prompt;
/// `ProviderUnavailable` when the key service is down; the state file errors.
pub async fn create_block(
    app: &AppState,
    s: &AuthedSession,
    prompt_ref: &str,
) -> Result<RuleDto, ApiError> {
    let prompt = open_prompt(app, s, prompt_ref).await?;
    no_log(&prompt);
    let sender = SenderKey::from_address(&prompt.sender_key);
    let now = app.ports.clock.now();
    let rule_id = RuleId(Uuid::new_v5(
        &NS_RULE_FROM_PROMPT,
        &namespace_bytes(&s.user, prompt.swipe_id),
    ));
    let stored = StoredRule {
        rule: SortRule::block_person_for(&sender, rule_id, now, Some(prompt.swipe_id)),
        times_applied: 0,
        yearly_rate: None,
    };
    let entry = HistoryEntry {
        entry_id: rule_id.0,
        at: now,
        mailbox_id: MailboxId(prompt.mailbox_id),
        sender_display: prompt.sender_display,
        action: HistoryAction::Blocked,
        outcome: HistoryOutcome::Done,
        rule_id: Some(rule_id),
    };
    let store = UserStateStore::new(Arc::new(app.clone()));
    let find = sender.clone();
    let rule = store
        .update(&s.user, move |st: &mut UserState| -> StoredRule {
            if let Some(existing) = block_rule_for(st, &find) {
                return existing;
            }
            st.rules.push(stored.clone());
            st.totals.people_blocked = st.totals.people_blocked.saturating_add(1);
            st.totals.senders_silenced = st.totals.senders_silenced.saturating_add(1);
            let _ = st.push_history(entry.clone());
            stored.clone()
        })
        .await?;
    Ok(dto(&rule))
}

/// API-RULE-2 `file` (FL-04 AC2): build a filing rule from a message re-read
/// from the provider. The sender comes from that fresh read, never the client.
/// The rule acts from the next Feed load (T-609).
///
/// # Errors
///
/// `NotFound` when the mailbox is not the user's or the category is unknown;
/// `InvalidRequest` for a malformed message ID; `MessageChanged` when the
/// message is gone; provider and state file errors.
pub async fn create_file(
    app: &AppState,
    s: &AuthedSession,
    mailbox_id: Uuid,
    message_id: &str,
    category_id: Uuid,
) -> Result<RuleDto, ApiError> {
    let user = s.user;
    let mailbox = MailboxId(mailbox_id);
    let record = app
        .ports
        .store
        .mailboxes()
        .get(&mailbox)
        .await?
        .filter(|r| r.record.user_id == user)
        .ok_or(ApiError::NotFound)?;
    let store = UserStateStore::new(Arc::new(app.clone()));
    let loaded = store.load(&user).await?;
    if loaded.state.category(&CategoryId(category_id)).is_none() {
        return Err(ApiError::NotFound);
    }
    let ctx = app.tokens.mailbox_ctx(app, &user, &mailbox).await?;
    let id = MessageId::new(message_id).map_err(|_| ApiError::InvalidRequest {
        fields: vec!["message_id".to_owned()],
    })?;
    let meta = match app
        .ports
        .mail(record.record.provider)
        .get_meta(&ctx, &id)
        .await
    {
        Ok(meta) => meta,
        Err(MailError::NotFound) => return Err(ApiError::MessageChanged),
        Err(e) => return Err(provider_error(mailbox, &e)),
    };
    let sender = meta.sender.clone();
    let now = app.ports.clock.now();
    let category = CategoryId(category_id);
    let stored = StoredRule {
        rule: SortRule::file_for(&sender, category, RuleId(app.ports.rng.uuid_v4()), now),
        times_applied: 0,
        yearly_rate: None,
    };
    let find = sender;
    let rule = store
        .update(&user, move |st: &mut UserState| -> StoredRule {
            if let Some(existing) = st.rules.iter().find(|r| {
                r.rule.enabled
                    && r.rule.kind == RuleKind::File
                    && r.rule.matcher.sender == find
                    && r.rule.category == Some(category)
            }) {
                return existing.clone();
            }
            st.rules.push(stored.clone());
            stored.clone()
        })
        .await?;
    Ok(dto(&rule))
}

/// API-RULE-3 (SR-01 AC4, ST-01 AC2): switch a rule on or off.
///
/// # Errors
///
/// `NotFound` for an unknown ID, and the state file errors.
pub async fn patch(
    app: &AppState,
    s: &AuthedSession,
    rule_id: Uuid,
    enabled: bool,
) -> Result<RuleDto, ApiError> {
    let store = UserStateStore::new(Arc::new(app.clone()));
    let id = RuleId(rule_id);
    let existing = store.load(&s.user).await?.state.rule(&id).cloned();
    if existing.is_none() {
        return Err(ApiError::NotFound);
    }
    let rule = store
        .update(&s.user, move |st: &mut UserState| -> Option<StoredRule> {
            st.rules.iter_mut().find(|r| r.rule.rule_id == id).map(|r| {
                r.rule.enabled = enabled;
                r.clone()
            })
        })
        .await?;
    rule.map(|r| dto(&r)).ok_or(ApiError::NotFound)
}

/// API-RULE-4: delete a rule. History entries that name it keep their
/// `rule_id` (the app shows "rule deleted", S9 7.3).
///
/// # Errors
///
/// `NotFound` for an unknown ID, and the state file errors.
pub async fn delete(app: &AppState, s: &AuthedSession, rule_id: Uuid) -> Result<(), ApiError> {
    let store = UserStateStore::new(Arc::new(app.clone()));
    let id = RuleId(rule_id);
    if store.load(&s.user).await?.state.rule(&id).is_none() {
        return Err(ApiError::NotFound);
    }
    store
        .update(&s.user, move |st: &mut UserState| {
            st.rules.retain(|r| r.rule.rule_id != id);
        })
        .await?;
    Ok(())
}

/// API-RULE-5 (PB-01 AC3): decline a block prompt, suppressing it for the
/// sender for 90 days `[TUNABLE]`.
///
/// # Errors
///
/// `InvalidRequest` when the prompt reference does not open or is not a prompt;
/// `ProviderUnavailable` when the key service is down; the state file errors.
pub async fn decline(app: &AppState, s: &AuthedSession, prompt_ref: &str) -> Result<(), ApiError> {
    let prompt = open_prompt(app, s, prompt_ref).await?;
    no_log(&prompt);
    let now = app.ports.clock.now();
    let tunables = Tunables::default();
    let store = UserStateStore::new(Arc::new(app.clone()));
    store
        .update(&s.user, move |st: &mut UserState| {
            st.sender_stats
                .entry(prompt.sender_key.clone())
                .or_default()
                .decline_block_prompt(now, &tunables);
        })
        .await?;
    Ok(())
}

/// The prompt reference as the route files sent it, opened under the session's
/// key. Every failure is `400 invalid_request`, except a key service outage.
async fn open_prompt(
    app: &AppState,
    s: &AuthedSession,
    prompt_ref: &str,
) -> Result<BlockPromptRef, ApiError> {
    let wrapped = wrapped_key(app, &s.user).await?;
    let sealer = SealedTokens::new(Arc::clone(&app.ports.keys), Arc::clone(&app.ports.clock));
    sealer
        .open::<BlockPromptRef>(
            TokenType::PromptRef,
            &s.user,
            &wrapped,
            &s.session_record_id,
            prompt_ref,
        )
        .await
        .map_err(|e| match e {
            TokenError::Unavailable => ApiError::ProviderUnavailable {
                mailbox_id: None,
                retry_after_s: None,
            },
            _ => ApiError::InvalidRequest {
                fields: vec!["prompt_ref".to_owned()],
            },
        })
}

/// `v5(NS_RULE_FROM_PROMPT, user ++ swipe_id)`.
fn namespace_bytes(user: &UserId, swipe_id: Uuid) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(32);
    bytes.extend_from_slice(user.0.as_bytes());
    bytes.extend_from_slice(swipe_id.as_bytes());
    bytes
}

/// The enabled `block_person` rule for a sender, if any.
fn block_rule_for(st: &UserState, sender: &SenderKey) -> Option<StoredRule> {
    st.rules
        .iter()
        .find(|r| {
            r.rule.enabled
                && r.rule.kind == RuleKind::BlockPerson
                && r.rule.matcher.sender == *sender
        })
        .cloned()
}

/// The domain `RuleKind` for an API-RULE-1 `kind` filter value. The route
/// handler has already refused anything else.
fn kind_of(kind: &str) -> RuleKind {
    match kind {
        "reject_list" => RuleKind::RejectList,
        "file" => RuleKind::File,
        _ => RuleKind::BlockPerson,
    }
}

/// Map a stored rule to its wire shape (GM-05 AC2).
fn dto(stored: &StoredRule) -> RuleDto {
    RuleDto {
        rule_id: stored.rule.rule_id.0,
        kind: kind_str(stored.rule.kind),
        r#match: RuleMatchDto {
            sender_address: stored.rule.matcher.sender.as_str().to_owned(),
            list_id: stored.rule.matcher.list_id.clone(),
        },
        category_id: stored.rule.category.map(|c| c.0),
        enabled: stored.rule.enabled,
        created_at: stored.rule.created_at,
        times_applied: stored.times_applied,
        yearly_rate: stored.yearly_rate,
    }
}

/// The wire name of a rule kind.
fn kind_str(kind: RuleKind) -> &'static str {
    match kind {
        RuleKind::RejectList => "reject_list",
        RuleKind::BlockPerson => "block_person",
        RuleKind::File => "file",
    }
}

/// A no-op that consumes a prompt reference without logging it, so the private
/// fields never reach a `Debug` or a log line (S5, C2).
fn no_log(_prompt: &BlockPromptRef) {}
