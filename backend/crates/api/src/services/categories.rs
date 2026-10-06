//! Categories and their mailbox labels (T-604; SW-04 AC2, FL-02 AC1).
//!
//! A category is a user-state record with one provider label per mailbox,
//! created lazily on first use. T-607a extends this module.

use std::collections::BTreeMap;
use std::sync::Arc;

use domain::user_state::{Category, UserState};
use domain::{CategoryId, MailboxId, UserId};
use ports::{MailError, MailboxCtx};
use uuid::Uuid;

use crate::error::ApiError;
use crate::services::swipe::NS_SWIPE;
use crate::services::user_state_store::UserStateStore;
use crate::state::AppState;

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
