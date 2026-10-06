//! Categories and their mailbox labels (T-604; SW-04 AC2, FL-02 AC1).
//!
//! A category is a user-state record with one provider label per mailbox,
//! created lazily on first use. T-607a extends this module.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use domain::user_state::{Category, UserState};
use domain::{CategoryId, MailboxId, MailboxStatus, MessageMeta, Provider, UserId};
use ports::store::{aad_fields, MailboxRecord, Versioned};
use ports::{Aad, MailError, MailboxCtx, MessagePage, MessageQuery, PageToken, WrappedKey};
use uuid::Uuid;

use crate::error::ApiError;
use crate::routes::categories::{
    CategoryCursor, CategoryDto, FiledMessageDto, FiledPage, PerMailboxCount, CATEGORY_CURSOR_TTL,
    CATEGORY_LIMIT_MAX,
};
use crate::sealed::{SealedTokens, TokenError, TokenType};
use crate::services::feed::{bounded, error_code, mailbox_error_code};
use crate::services::swipe::NS_SWIPE;
use crate::services::user_state_store::UserStateStore;
use crate::session::extract::AuthedSession;
use crate::state::AppState;
use crate::text::plain_text;

/// The longest a category name may be (schema `CategoryName`).
pub const CATEGORY_NAME_MAX_CHARS: usize = 100;

/// Validate a new category name: trimmed, 1 to 100 characters, and no `/` at
/// the start or end, because Gmail treats `/` as nesting.
///
/// # Errors
///
/// `InvalidRequest` for an empty, over-long or slash-delimited name.
pub fn validate_category_name(raw: &str) -> Result<String, ApiError> {
    let name = raw.trim();
    let bad = name.is_empty()
        || name.chars().count() > CATEGORY_NAME_MAX_CHARS
        || name.starts_with('/')
        || name.ends_with('/');
    if bad {
        return Err(ApiError::InvalidRequest {
            fields: vec!["new_category_name".to_owned()],
        });
    }
    Ok(name.to_owned())
}

/// Map a provider failure to an S7 error, never leaking provider text.
pub(crate) fn provider_error(mailbox: MailboxId, e: &MailError) -> ApiError {
    match e {
        MailError::Unauthorized => ApiError::MailboxNeedsSignIn {
            mailbox_id: Some(mailbox.0),
        },
        MailError::RateLimited { retry_after_s } => ApiError::ProviderUnavailable {
            mailbox_id: Some(mailbox.0),
            retry_after_s: Some(*retry_after_s),
        },
        MailError::Transient => ApiError::ProviderUnavailable {
            mailbox_id: Some(mailbox.0),
            retry_after_s: None,
        },
        MailError::Forbidden | MailError::NotFound | MailError::Invalid(_) => {
            ApiError::ProviderError {
                mailbox_id: Some(mailbox.0),
            }
        }
    }
}

/// Resolve the category for a `file` swipe: an existing `category_id`, or a
/// name reused case-insensitively, or a fresh category derived from `swipe_id`
/// so a retried request resolves to the same record.
///
/// # Errors
///
/// `InvalidRequest` when neither or both identifiers are given, or when the
/// `category_id` is unknown; `MailboxNeedsSignIn` or `Internal` from the state
/// file.
pub async fn resolve_or_create_category(
    app: &AppState,
    user: &UserId,
    id: Option<Uuid>,
    name: Option<&str>,
    swipe_id: Uuid,
) -> Result<Category, ApiError> {
    if id.is_some() == name.is_some() {
        return Err(ApiError::InvalidRequest {
            fields: vec!["category_id".to_owned()],
        });
    }
    let store = UserStateStore::new(Arc::new(app.clone()));
    let loaded = store.load(user).await?;
    if let Some(id) = id {
        return loaded
            .state
            .category(&CategoryId(id))
            .cloned()
            .ok_or(ApiError::InvalidRequest {
                fields: vec!["category_id".to_owned()],
            });
    }
    let raw = name.ok_or(ApiError::InvalidRequest {
        fields: vec!["new_category_name".to_owned()],
    })?;
    let name = validate_category_name(raw)?;
    if let Some(existing) = loaded.state.category_by_name(&name) {
        return Ok(existing.clone());
    }
    let created = Category {
        category_id: CategoryId(Uuid::new_v5(
            &NS_SWIPE,
            format!("category:{swipe_id}").as_bytes(),
        )),
        name,
        labels: BTreeMap::new(),
        created_at: app.ports.clock.now(),
    };
    let store_copy = created.clone();
    store
        .update(user, move |s: &mut UserState| {
            if s.category(&store_copy.category_id).is_none()
                && s.category_by_name(&store_copy.name).is_none()
            {
                s.totals.categories_created = s.totals.categories_created.saturating_add(1);
                s.categories.push(store_copy.clone());
            }
        })
        .await?;
    Ok(created)
}

/// The provider label ID for `category` in `ctx`'s mailbox, created lazily and
/// saved onto the category.
///
/// # Errors
///
/// `NotFound` when the mailbox is gone; `MailboxNeedsSignIn` when its grant is
/// revoked, `ProviderError` or `ProviderUnavailable` from the provider, and
/// `Internal` from the state file.
pub async fn ensure_label_for(
    app: &AppState,
    user: &UserId,
    ctx: &MailboxCtx,
    category: &Category,
) -> Result<String, ApiError> {
    if let Some(existing) = category.labels.get(&ctx.mailbox.0) {
        return Ok(existing.clone());
    }
    let record = app
        .ports
        .store
        .mailboxes()
        .get(&ctx.mailbox)
        .await?
        .ok_or(ApiError::NotFound)?;
    let label = app
        .ports
        .mail(record.record.provider)
        .ensure_label(ctx, &category.name)
        .await
        .map_err(|e| provider_error(ctx.mailbox, &e))?;
    let store = UserStateStore::new(Arc::new(app.clone()));
    let category_id = category.category_id;
    let mailbox = ctx.mailbox.0;
    let saved = label.clone();
    store
        .update(user, move |s: &mut UserState| {
            if let Some(c) = s
                .categories
                .iter_mut()
                .find(|c| c.category_id == category_id)
            {
                c.labels.insert(mailbox, saved.clone());
            }
        })
        .await?;
    Ok(label)
}

// ---------------------------------------------------------------------------
// T-607a: the Filed tab's endpoints (S7 5.6, API-CAT-1 to API-CAT-5)
// ---------------------------------------------------------------------------

/// Schema lengths for the filed-message strings (S7 5.6).
const SENDER_NAME_MAX: usize = 256;
const SUBJECT_MAX: usize = 998;

/// One mailbox to read a category's messages from, with the label ID and the
/// decrypted address the provider web link needs.
struct Target {
    mailbox: Uuid,
    provider: Provider,
    ctx: MailboxCtx,
    label: String,
    address: String,
}

/// The page state a sealed cursor carries.
struct PageState {
    /// `MailboxId` -> the provider page token to resume that mailbox from.
    pages: BTreeMap<Uuid, Option<String>>,
    /// Mailboxes whose last page was returned.
    exhausted: BTreeSet<Uuid>,
}

/// API-CAT-1: every category with its message counts, sorted by name
/// (FL-05 AC1).
///
/// # Errors
///
/// `MailboxNeedsSignIn` or `Internal` from the state file, and a store
/// failure. A mailbox whose count fails is left out of `per_mailbox`.
pub async fn list(app: &AppState, s: &AuthedSession) -> Result<Vec<CategoryDto>, ApiError> {
    let user = s.user;
    let store = UserStateStore::new(Arc::new(app.clone()));
    let loaded = store.load(&user).await?;
    let records = app.ports.store.mailboxes().by_user(&user).await?;
    let mut out = Vec::with_capacity(loaded.state.categories.len());
    for category in &loaded.state.categories {
        let per_mailbox = counts_for(app, &user, category, &records).await;
        out.push(dto(category, per_mailbox));
    }
    out.sort_by_key(|a| a.name.to_lowercase());
    Ok(out)
}

/// API-CAT-2: create a category by name. No label is created until the first
/// file (FL-02 AC1, S7 5.6).
///
/// # Errors
///
/// `InvalidRequest` for a name [`validate_category_name`] refuses;
/// `CategoryExists` for a case-insensitive clash; `MailboxNeedsSignIn` or
/// `Internal` from the state file.
pub async fn create(
    app: &AppState,
    s: &AuthedSession,
    name: &str,
) -> Result<CategoryDto, ApiError> {
    let user = s.user;
    let name = validate_category_name(name)?;
    let store = UserStateStore::new(Arc::new(app.clone()));
    let loaded = store.load(&user).await?;
    if loaded.state.category_by_name(&name).is_some() {
        return Err(ApiError::CategoryExists);
    }
    let created = Category {
        category_id: CategoryId(app.ports.rng.uuid_v4()),
        name,
        labels: BTreeMap::new(),
        created_at: app.ports.clock.now(),
    };
    let pushed = created.clone();
    // One update, with the clash re-checked inside it: two racing creates
    // cannot both win.
    let inserted = store
        .update(&user, move |st: &mut UserState| {
            if st.category_by_name(&pushed.name).is_some() {
                return false;
            }
            st.totals.categories_created = st.totals.categories_created.saturating_add(1);
            st.categories.push(pushed.clone());
            true
        })
        .await?;
    if !inserted {
        return Err(ApiError::CategoryExists);
    }
    Ok(dto(&created, Vec::new()))
}

/// API-CAT-3: rename the provider label in every mailbox that carries one,
/// then the category (S9 section 5).
///
/// # Errors
///
/// `NotFound` for an unknown ID; `InvalidRequest` for a bad name;
/// `CategoryExists` for a clash with another category or a provider label;
/// `ProviderError` when a mailbox refuses for any other reason;
/// `MailboxNeedsSignIn` or `Internal` from the state file.
pub async fn rename(
    app: &AppState,
    s: &AuthedSession,
    id: Uuid,
    name: &str,
) -> Result<CategoryDto, ApiError> {
    let user = s.user;
    let name = validate_category_name(name)?;
    let store = UserStateStore::new(Arc::new(app.clone()));
    let loaded = store.load(&user).await?;
    let category = loaded
        .state
        .category(&CategoryId(id))
        .cloned()
        .ok_or(ApiError::NotFound)?;
    if loaded
        .state
        .category_by_name(&name)
        .is_some_and(|other| other.category_id != category.category_id)
    {
        return Err(ApiError::CategoryExists);
    }
    let records = app.ports.store.mailboxes().by_user(&user).await?;
    // Provider calls first, never inside the state update closure.
    for record in labeled(&category, &records) {
        let mailbox = record.record.mailbox_id;
        let Some(label) = category.labels.get(&mailbox.0) else {
            continue;
        };
        let ctx = app.tokens.mailbox_ctx(app, &user, &mailbox).await?;
        let renamed = app
            .ports
            .mail(record.record.provider)
            .rename_label(&ctx, label, &name)
            .await;
        match renamed {
            Ok(()) => {}
            Err(MailError::Invalid(reason)) if reason == "label_exists" => {
                return Err(ApiError::CategoryExists);
            }
            Err(_) => {
                return Err(ApiError::ProviderError {
                    mailbox_id: Some(mailbox.0),
                });
            }
        }
    }
    save_name(&store, &user, category.category_id, &name).await?;
    let renamed = Category { name, ..category };
    let per_mailbox = counts_for(app, &user, &renamed, &records).await;
    Ok(dto(&renamed, per_mailbox))
}

/// One update writes the category's new name.
async fn save_name(
    store: &UserStateStore,
    user: &UserId,
    id: CategoryId,
    name: &str,
) -> Result<(), ApiError> {
    let new_name = name.to_owned();
    store
        .update(user, move |st: &mut UserState| {
            if let Some(c) = st.categories.iter_mut().find(|c| c.category_id == id) {
                c.name.clone_from(&new_name);
            }
        })
        .await?;
    Ok(())
}

/// API-CAT-4: remove the label from every mailbox and delete the category and
/// its filing rules. No message is ever touched (INV-5, S3).
///
/// # Errors
///
/// `NotFound` for an unknown ID; `ProviderError` when a mailbox refuses to drop
/// the label; `MailboxNeedsSignIn` or `Internal` from the state file.
pub async fn delete(app: &AppState, s: &AuthedSession, id: Uuid) -> Result<(), ApiError> {
    let user = s.user;
    let store = UserStateStore::new(Arc::new(app.clone()));
    let loaded = store.load(&user).await?;
    let category = loaded
        .state
        .category(&CategoryId(id))
        .cloned()
        .ok_or(ApiError::NotFound)?;
    let records = app.ports.store.mailboxes().by_user(&user).await?;
    // `remove_label` drops the label definition only: nothing on this path can
    // delete a message (INV-5).
    for record in labeled(&category, &records) {
        let mailbox = record.record.mailbox_id;
        let Some(label) = category.labels.get(&mailbox.0) else {
            continue;
        };
        let ctx = app.tokens.mailbox_ctx(app, &user, &mailbox).await?;
        app.ports
            .mail(record.record.provider)
            .remove_label(&ctx, label)
            .await
            .map_err(|_| ApiError::ProviderError {
                mailbox_id: Some(mailbox.0),
            })?;
    }
    let cid = category.category_id;
    store
        .update(&user, move |st: &mut UserState| {
            st.categories.retain(|c| c.category_id != cid);
            st.rules.retain(|r| r.rule.category != Some(cid));
            for stats in st.sender_stats.values_mut() {
                stats.files.remove(&cid.0);
                if stats.last_filed == Some(cid) {
                    stats.last_filed = None;
                }
            }
        })
        .await?;
    Ok(())
}

/// API-CAT-5: one page of a category's messages, merged newest first across
/// the mailboxes that carry its label (FL-05 AC1).
///
/// `[DEFAULT]` a mailbox's whole page is consumed before its next token is
/// used, so a few older messages can appear on a later page than strict
/// merging would put them. Good enough for a browsing list.
///
/// # Errors
///
/// `InvalidRequest` for a `limit` outside 1 to 50, or a cursor that does not
/// open or belongs to another category; `NotFound` for an unknown ID;
/// `MailboxNeedsSignIn` or `Internal` from the state file. A mailbox that
/// fails to answer is data in `mailbox_errors`.
pub async fn messages(
    app: &AppState,
    s: &AuthedSession,
    id: Uuid,
    cursor: Option<&str>,
    limit: u32,
) -> Result<FiledPage, ApiError> {
    if limit == 0 || limit > CATEGORY_LIMIT_MAX {
        return Err(invalid("/limit"));
    }
    let user = s.user;
    let store = UserStateStore::new(Arc::new(app.clone()));
    let loaded = store.load(&user).await?;
    let category = loaded
        .state
        .category(&CategoryId(id))
        .cloned()
        .ok_or(ApiError::NotFound)?;
    let wrapped = wrapped_key(app, &user).await?;
    let sealer = SealedTokens::new(Arc::clone(&app.ports.keys), Arc::clone(&app.ports.clock));
    let state = open_cursor(&sealer, s, &wrapped, cursor, id).await?;
    let mut pages = state.pages;
    let mut exhausted = state.exhausted;
    let records = app.ports.store.mailboxes().by_user(&user).await?;

    let mut errors: BTreeMap<Uuid, &'static str> = BTreeMap::new();
    let targets = targets_for(
        app,
        &user,
        &wrapped,
        &category,
        &records,
        &exhausted,
        &mut errors,
    )
    .await?;
    let metas = fetch_round(
        app,
        &targets,
        &mut pages,
        &mut exhausted,
        &mut errors,
        limit,
    )
    .await;
    let messages = build_messages(app, &targets, &metas);

    let any_open = labeled(&category, &records)
        .iter()
        .any(|r| !exhausted.contains(&r.record.mailbox_id.0));
    let next_cursor = if any_open {
        let expires_at = app.ports.clock.now() + CATEGORY_CURSOR_TTL;
        let payload = CategoryCursor {
            category_id: id,
            pages,
            exhausted,
        };
        Some(
            sealer
                .seal(
                    TokenType::Cursor,
                    &user,
                    &wrapped,
                    &s.session_record_id,
                    expires_at,
                    &payload,
                )
                .await
                .map_err(|_| ApiError::Internal)?,
        )
    } else {
        None
    };
    Ok(FiledPage {
        messages,
        next_cursor,
        mailbox_errors: errors
            .into_iter()
            .map(|(mailbox_id, code)| crate::routes::feed::MailboxErrorDto { mailbox_id, code })
            .collect(),
    })
}

/// The context and decrypted address for one mailbox, or the API error that
/// stands for it (its code goes into `mailbox_errors`).
async fn read_target(
    app: &AppState,
    user: &UserId,
    wrapped: &WrappedKey,
    record: &MailboxRecord,
    label: String,
) -> Result<Target, ApiError> {
    let ctx = app
        .tokens
        .mailbox_ctx(app, user, &record.mailbox_id)
        .await?;
    let address = address_of(app, user, wrapped, record).await?;
    Ok(Target {
        mailbox: record.mailbox_id.0,
        provider: record.provider,
        ctx,
        label,
        address,
    })
}

/// One exact label count (T-602a defines the label-only count as exact).
async fn count_one(
    app: &AppState,
    mailbox_id: Uuid,
    provider: Provider,
    ctx: &MailboxCtx,
    label: &str,
) -> (Uuid, Result<u64, MailError>) {
    let query = MessageQuery {
        label: Some(label.to_owned()),
        ..MessageQuery::default()
    };
    let result = app.ports.mail(provider).count_messages(ctx, &query).await;
    (mailbox_id, result)
}

/// One mailbox's page of a category's messages, newest first.
async fn list_one(
    app: &AppState,
    target: &Target,
    page: Option<PageToken>,
    max: u32,
) -> (Uuid, Result<MessagePage, MailError>) {
    let query = MessageQuery {
        label: Some(target.label.clone()),
        ..MessageQuery::default()
    };
    let result = app
        .ports
        .mail(target.provider)
        .list_messages(&target.ctx, &query, page, max)
        .await;
    (target.mailbox, result)
}

/// The mailboxes to ask for this page, each with its label and decrypted
/// address. A mailbox whose context or address cannot be read becomes a
/// `mailbox_errors` entry instead of a target.
async fn targets_for(
    app: &AppState,
    user: &UserId,
    wrapped: &WrappedKey,
    category: &Category,
    records: &[Versioned<MailboxRecord>],
    exhausted: &BTreeSet<Uuid>,
    errors: &mut BTreeMap<Uuid, &'static str>,
) -> Result<Vec<Target>, ApiError> {
    let mut targets = Vec::new();
    for record in labeled(category, records) {
        let mailbox = record.record.mailbox_id;
        if exhausted.contains(&mailbox.0) {
            continue;
        }
        let Some(label) = category.labels.get(&mailbox.0).cloned() else {
            continue;
        };
        match read_target(app, user, wrapped, &record.record, label).await {
            Ok(target) => targets.push(target),
            Err(e) => {
                let code = error_code(&e).ok_or(ApiError::Internal)?;
                errors.insert(mailbox.0, code);
            }
        }
    }
    Ok(targets)
}

/// One round of provider list calls, merged newest first and cut to `limit`.
/// Each mailbox that answers keeps the token to resume it from, or joins
/// `exhausted` when its page was the last; a failing mailbox is an entry in
/// `errors` and keeps its place.
async fn fetch_round(
    app: &AppState,
    targets: &[Target],
    pages: &mut BTreeMap<Uuid, Option<String>>,
    exhausted: &mut BTreeSet<Uuid>,
    errors: &mut BTreeMap<Uuid, &'static str>,
    limit: u32,
) -> Vec<MessageMeta> {
    let mut calls = Vec::with_capacity(targets.len());
    for target in targets {
        let page = pages.get(&target.mailbox).cloned().flatten().map(PageToken);
        calls.push(list_one(app, target, page, limit));
    }
    let mut metas = Vec::new();
    for (mailbox, result) in bounded(calls).await {
        match result {
            Ok(page) => {
                let next = page.next.as_ref().map(|t| t.0.clone());
                if next.is_none() {
                    exhausted.insert(mailbox);
                }
                pages.insert(mailbox, next);
                metas.extend(page.items);
            }
            Err(e) => {
                errors
                    .entry(mailbox)
                    .or_insert_with(|| mailbox_error_code(&e));
            }
        }
    }
    metas.sort_by(|a, b| {
        b.internal_date
            .cmp(&a.internal_date)
            .then_with(|| a.mailbox.0.cmp(&b.mailbox.0))
            .then_with(|| a.id.as_str().cmp(b.id.as_str()))
    });
    metas.truncate(usize::try_from(limit).unwrap_or(usize::MAX));
    metas
}

/// The DTOs for one page, built from the mailboxes that answered.
fn build_messages(
    app: &AppState,
    targets: &[Target],
    metas: &[MessageMeta],
) -> Vec<FiledMessageDto> {
    let by_mailbox: BTreeMap<Uuid, &Target> = targets.iter().map(|t| (t.mailbox, t)).collect();
    let mut messages = Vec::with_capacity(metas.len());
    for meta in metas {
        let Some(target) = by_mailbox.get(&meta.mailbox.0) else {
            continue;
        };
        messages.push(FiledMessageDto {
            mailbox_id: meta.mailbox.0,
            message_id: meta.id.as_str().to_owned(),
            sender_name: plain_text(&meta.from_display, SENDER_NAME_MAX),
            subject: plain_text(&meta.subject, SUBJECT_MAX),
            received_at: meta.internal_date,
            provider_web_url: app
                .ports
                .mail(target.provider)
                .web_url(&target.address, &meta.id),
        });
    }
    messages
}

/// API-CAT-1's counts: one label count per connected mailbox that carries the
/// label. A mailbox that fails is left out.
async fn counts_for(
    app: &AppState,
    user: &UserId,
    category: &Category,
    records: &[Versioned<MailboxRecord>],
) -> Vec<PerMailboxCount> {
    let mut conns = Vec::new();
    for record in labeled(category, records) {
        let mailbox = record.record.mailbox_id;
        let Some(label) = category.labels.get(&mailbox.0) else {
            continue;
        };
        if let Ok(ctx) = app.tokens.mailbox_ctx(app, user, &mailbox).await {
            conns.push((mailbox.0, record.record.provider, ctx, label.clone()));
        }
    }
    let mut calls = Vec::with_capacity(conns.len());
    for (mailbox, provider, ctx, label) in &conns {
        calls.push(count_one(app, *mailbox, *provider, ctx, label));
    }
    let mut out = Vec::new();
    for (mailbox_id, result) in bounded(calls).await {
        if let Ok(message_count) = result {
            out.push(PerMailboxCount {
                mailbox_id,
                message_count,
            });
        }
    }
    out
}

/// The DTO for one category and its counts.
fn dto(category: &Category, per_mailbox: Vec<PerMailboxCount>) -> CategoryDto {
    let message_count = per_mailbox.iter().map(|m| m.message_count).sum();
    CategoryDto {
        category_id: category.category_id.0,
        name: category.name.clone(),
        message_count,
        per_mailbox,
    }
}

/// The user's connected mailboxes that carry one of `category`'s labels.
fn labeled<'a>(
    category: &Category,
    records: &'a [Versioned<MailboxRecord>],
) -> Vec<&'a Versioned<MailboxRecord>> {
    records
        .iter()
        .filter(|r| r.record.status == MailboxStatus::Connected)
        .filter(|r| category.labels.contains_key(&r.record.mailbox_id.0))
        .collect()
}

/// The decrypted address of a mailbox, for the provider web link.
async fn address_of(
    app: &AppState,
    user: &UserId,
    wrapped: &WrappedKey,
    record: &MailboxRecord,
) -> Result<String, ApiError> {
    let aad = Aad {
        user: *user,
        scope: record.mailbox_id.0.to_string(),
        field: aad_fields::MAILBOX_EMAIL,
    };
    let plain = app
        .ports
        .keys
        .open(user, wrapped, &aad, &record.email_address.0)
        .await
        .map_err(|_| ApiError::Internal)?;
    String::from_utf8(plain).map_err(|_| ApiError::Internal)
}

/// The user's wrapped data key, which seals and opens the page cursor.
async fn wrapped_key(app: &AppState, user: &UserId) -> Result<WrappedKey, ApiError> {
    app.ports
        .store
        .users()
        .get(user)
        .await?
        .map(|r| r.record.wrapped_data_key)
        .ok_or(ApiError::Internal)
}

/// The page state a presented cursor carries. Any failure is `400`, except a
/// key outage (`500`).
async fn open_cursor(
    sealer: &SealedTokens,
    s: &AuthedSession,
    wrapped: &WrappedKey,
    cursor: Option<&str>,
    category_id: Uuid,
) -> Result<PageState, ApiError> {
    let Some(token) = cursor else {
        return Ok(PageState {
            pages: BTreeMap::new(),
            exhausted: BTreeSet::new(),
        });
    };
    let payload: CategoryCursor = sealer
        .open(
            TokenType::Cursor,
            &s.user,
            wrapped,
            &s.session_record_id,
            token,
        )
        .await
        .map_err(|e| match e {
            TokenError::Unavailable => ApiError::Internal,
            _ => invalid(""),
        })?;
    if payload.category_id != category_id {
        return Err(invalid(""));
    }
    Ok(PageState {
        pages: payload.pages,
        exhausted: payload.exhausted,
    })
}

/// A `400 invalid_request` naming the offending field.
fn invalid(field: &str) -> ApiError {
    ApiError::InvalidRequest {
        fields: if field.is_empty() {
            Vec::new()
        } else {
            vec![field.to_owned()]
        },
    }
}
