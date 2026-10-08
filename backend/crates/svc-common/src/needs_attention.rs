//! Raising a Needs Attention item from a service other than `api` (T-701,
//! UN-05 AC1).
//!
//! `api`'s Needs Attention routes read the items; `unsub` raises them. The
//! item's `sender_display` and `link` are sealed under the owning user's
//! `data_key`, bound to the item ID and the field name (S6 5). A link that is
//! not `https` is refused rather than stored (S7 5.8).
//!
//! Debug for the item prints only the ID, so a sender address or a URL cannot
//! reach a log through this struct (S5).

use domain::{MailboxId, NeedsAttentionReason, UserId};
use ports::store::{aad_fields, Ciphertext, NeedsAttentionId, NeedsAttentionRecord, Precondition};
use ports::{Aad, Ports};
use time::Duration;
use url::Url;

use crate::error::SvcError;

/// How long an item lives before the sweeper deletes it (S2 `[TUNABLE]`; the
/// same 30 days as `Tunables::needs_attention_ttl`).
pub const NEEDS_ATTENTION_TTL: Duration = Duration::days(30);

/// A new Needs Attention item to raise. The strings are borrowed so the caller
/// keeps ownership of the decrypted values.
pub struct NewItem<'a> {
    pub user: &'a UserId,
    pub mailbox: &'a MailboxId,
    pub sender_display: &'a str,
    pub link: Option<&'a Url>,
    pub reason: NeedsAttentionReason,
}

/// Validate the link, seal the fields, and write the item with
/// `Precondition::MustNotExist`. Returns the item ID.
pub async fn raise_item(ports: &Ports, item: NewItem<'_>) -> Result<NeedsAttentionId, SvcError> {
    raise_item_with_id(ports, NeedsAttentionId(ports.rng.uuid_v4()), item).await
}

/// Publish an outbox item idempotently using a stable, caller-owned ID.
pub async fn raise_item_with_id(
    ports: &Ports,
    item_id: NeedsAttentionId,
    item: NewItem<'_>,
) -> Result<NeedsAttentionId, SvcError> {
    // A non-https link is a programming error, not data to store (S7 5.8).
    let link = match item.link {
        None => None,
        Some(url) if url.scheme() == "https" => Some(url.clone()),
        Some(_) => return Err(SvcError::Invalid("needs attention link must be https")),
    };
    let user = ports
        .store
        .users()
        .get(item.user)
        .await
        .map_err(|_| SvcError::Store)?
        .ok_or(SvcError::NotFound)?;
    let scope = item_id.0.to_string();

    let sender_display = seal(
        ports,
        &user.record.wrapped_data_key,
        item.user,
        &scope,
        aad_fields::NA_SENDER_DISPLAY,
        item.sender_display.as_bytes(),
    )
    .await?;
    let link_ciphertext = match &link {
        Some(url) => Some(
            seal(
                ports,
                &user.record.wrapped_data_key,
                item.user,
                &scope,
                aad_fields::NA_LINK,
                url.as_str().as_bytes(),
            )
            .await?,
        ),
        None => None,
    };

    let now = ports.clock.now();
    let record = NeedsAttentionRecord {
        item_id,
        user_id: *item.user,
        mailbox_id: *item.mailbox,
        sender_display,
        link: link_ciphertext,
        reason_code: item.reason,
        created_at: now,
        expires_at: now + NEEDS_ATTENTION_TTL,
    };
    match ports
        .store
        .needs_attention()
        .put(&record, Precondition::MustNotExist)
        .await
    {
        Ok(_) => {}
        Err(ports::StoreError::AlreadyExists) => {
            let existing = ports
                .store
                .needs_attention()
                .get(&item_id)
                .await
                .map_err(|_| SvcError::Store)?
                .ok_or(SvcError::Store)?;
            if existing.record.user_id != *item.user
                || existing.record.mailbox_id != *item.mailbox
                || existing.record.reason_code != item.reason
            {
                return Err(SvcError::Store);
            }
        }
        Err(_) => return Err(SvcError::Store),
    }
    Ok(item_id)
}

/// Seal one field under the user's `data_key` with the item ID scope.
async fn seal(
    ports: &Ports,
    wrapped: &ports::WrappedKey,
    user: &UserId,
    scope: &str,
    field: &'static str,
    plaintext: &[u8],
) -> Result<Ciphertext, SvcError> {
    let aad = Aad {
        user: *user,
        scope: scope.to_owned(),
        field,
    };
    ports
        .keys
        .seal(user, wrapped, &aad, plaintext)
        .await
        .map(Ciphertext)
        .map_err(|_| SvcError::Crypto)
}
