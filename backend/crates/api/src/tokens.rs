//! `TokenService`: the API's only path to mailbox refresh tokens and access
//! tokens (T-503).
//!
//! A refresh token is sealed by [`svc_common::mint`] and written to the mailbox
//! record; an access token is minted on demand and held for a short while in
//! process memory only (S5 C3, V10.1.1). Nothing here is serialised, logged or
//! shared with `unsub` or `worker`.
#![allow(clippy::module_name_repetitions)]

use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard};

use domain::{next_status, MailboxEvent, MailboxId, UserId};
use obs::Sensitive;
use ports::{MailboxCtx, Precondition, StoreError};
use svc_common::mint::{self, MintError};
use time::{Duration, OffsetDateTime};

use crate::error::ApiError;
use crate::state::AppState;

/// How long a minted access token is reused in process memory `[DEFAULT]`.
/// A Google access token lives 3600 s; the cache is deliberately shorter.
pub const ACCESS_TOKEN_CACHE_S: i64 = 300;

/// One cached access token and the instant it was minted.
struct CacheEntry {
    token: Sensitive<String>,
    minted_at: OffsetDateTime,
}

/// Mint, cache and forget mailbox access tokens.
///
/// The cache is keyed by `(user, mailbox)`, so a token minted for one user is
/// never handed to another (INV-3).
pub struct TokenService {
    cache: Mutex<HashMap<(UserId, MailboxId), CacheEntry>>,
}

impl TokenService {
    /// An empty cache.
    #[must_use]
    pub fn new() -> Self {
        Self {
            cache: Mutex::new(HashMap::new()),
        }
    }

    /// Seal `token` as the mailbox's refresh token, set the mailbox
    /// `Connected` and drop any cached access token.
    ///
    /// The write is conditional on the version read; a lost race re-reads once
    /// and retries.
    ///
    /// # Errors
    ///
    /// `NotFound` when the user or mailbox is missing, or the mailbox is not
    /// the user's; `Internal` on a key or store failure.
    pub async fn store_refresh_token(
        &self,
        state: &AppState,
        user: &UserId,
        mailbox: &MailboxId,
        token: Sensitive<String>,
    ) -> Result<(), ApiError> {
        let user_record = state
            .ports
            .store
            .users()
            .get(user)
            .await?
            .ok_or(ApiError::NotFound)?;
        let versioned = state
            .ports
            .store
            .mailboxes()
            .get(mailbox)
            .await?
            .ok_or(ApiError::NotFound)?;
        if versioned.record.user_id != *user {
            return Err(ApiError::NotFound);
        }
        let sealed = mint::seal_refresh_token(&state.ports, &user_record.record, mailbox, &token)
            .await
            .map_err(|e| map_mint(&e, mailbox))?;
        let mut current = versioned;
        let mut retried = false;
        loop {
            let mut record = current.record.clone();
            record.refresh_token = Some(sealed.clone());
            record.status = next_status(record.status, MailboxEvent::OAuthSucceeded);
            let put = state
                .ports
                .store
                .mailboxes()
                .put(&record, Precondition::Matches(current.version.clone()))
                .await;
            match put {
                Ok(_) => break,
                Err(StoreError::PreconditionFailed) if !retried => {
                    retried = true;
                    current = state
                        .ports
                        .store
                        .mailboxes()
                        .get(mailbox)
                        .await?
                        .ok_or(ApiError::NotFound)?;
                    if current.record.user_id != *user {
                        return Err(ApiError::NotFound);
                    }
                }
                Err(e) => return Err(e.into()),
            }
        }
        self.forget(mailbox);
        Ok(())
    }

    /// An access token for one of the user's mailboxes, for a provider call in
    /// this request. A token minted less than [`ACCESS_TOKEN_CACHE_S`] ago is
    /// reused; otherwise it is minted afresh.
    ///
    /// # Errors
    ///
    /// `MailboxNeedsSignIn` when the stored grant is revoked, `NotFound` when
    /// the mailbox is missing, `ProviderUnavailable` on a transient failure and
    /// `Internal` on a key failure.
    pub async fn mailbox_ctx(
        &self,
        state: &AppState,
        user: &UserId,
        mailbox: &MailboxId,
    ) -> Result<MailboxCtx, ApiError> {
        let now = state.ports.clock.now();
        if let Some(token) = self.cached(user, mailbox, now) {
            return Ok(MailboxCtx {
                mailbox: *mailbox,
                access_token: token,
            });
        }
        let ctx = mint::mint_access_token(&state.ports, user, mailbox)
            .await
            .map_err(|e| map_mint(&e, mailbox))?;
        self.insert_cached(*user, *mailbox, ctx.access_token.clone(), now);
        Ok(ctx)
    }

    /// Cache `token` for `(user, mailbox)`, first dropping every entry that is
    /// already past [`ACCESS_TOKEN_CACHE_S`].
    ///
    /// The cache holds live tokens only: an expired entry is dropped on insert
    /// and on read, so it cannot grow with every mailbox the instance has ever
    /// served (V10.1.1; T-503 security review F5).
    fn insert_cached(
        &self,
        user: UserId,
        mailbox: MailboxId,
        token: Sensitive<String>,
        now: OffsetDateTime,
    ) {
        let mut cache = self.lock();
        cache.retain(|_, entry| Self::fresh(entry, now));
        cache.insert(
            (user, mailbox),
            CacheEntry {
                token,
                minted_at: now,
            },
        );
    }

    /// Whether a cache entry is still inside the reuse window.
    fn fresh(entry: &CacheEntry, now: OffsetDateTime) -> bool {
        now - entry.minted_at < Duration::seconds(ACCESS_TOKEN_CACHE_S)
    }

    /// The decrypted refresh token, only for revocation on disconnect or
    /// deletion (T-601b, T-803).
    ///
    /// # Errors
    ///
    /// `NotFound` when the user or mailbox is missing, or the mailbox is not
    /// the user's; `MailboxNeedsSignIn` when no grant is stored; `Internal` on
    /// a key failure.
    pub async fn refresh_token(
        &self,
        state: &AppState,
        user: &UserId,
        mailbox: &MailboxId,
    ) -> Result<Sensitive<String>, ApiError> {
        let user_record = state
            .ports
            .store
            .users()
            .get(user)
            .await?
            .ok_or(ApiError::NotFound)?;
        let versioned = state
            .ports
            .store
            .mailboxes()
            .get(mailbox)
            .await?
            .ok_or(ApiError::NotFound)?;
        if versioned.record.user_id != *user {
            return Err(ApiError::NotFound);
        }
        mint::open_refresh_token(&state.ports, &user_record.record, &versioned.record)
            .await
            .map_err(|e| map_mint(&e, mailbox))
    }

    /// Drop the cached access token for `mailbox`, after a provider 401 or on
    /// disconnect.
    pub fn forget(&self, mailbox: &MailboxId) {
        let mut cache = self.lock();
        cache.retain(|(_, mb), _| mb != mailbox);
    }

    /// The cached token for `(user, mailbox)` if it is younger than
    /// [`ACCESS_TOKEN_CACHE_S`]. An expired entry is removed here, so a lookup
    /// never leaves a dead token in the map.
    fn cached(
        &self,
        user: &UserId,
        mailbox: &MailboxId,
        now: OffsetDateTime,
    ) -> Option<Sensitive<String>> {
        let mut cache = self.lock();
        let key = (*user, *mailbox);
        match cache.get(&key) {
            Some(entry) if Self::fresh(entry, now) => Some(entry.token.clone()),
            Some(_) => {
                cache.remove(&key);
                None
            }
            None => None,
        }
    }

    fn lock(&self) -> MutexGuard<'_, HashMap<(UserId, MailboxId), CacheEntry>> {
        self.cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl Default for TokenService {
    fn default() -> Self {
        Self::new()
    }
}

/// Map a mint failure to an S7 error. `Crypto` detail never reaches the client.
fn map_mint(e: &MintError, mailbox: &MailboxId) -> ApiError {
    match e {
        MintError::Revoked => ApiError::MailboxNeedsSignIn {
            mailbox_id: Some(mailbox.0),
        },
        MintError::MailboxMissing => ApiError::NotFound,
        MintError::Transient => ApiError::ProviderUnavailable {
            mailbox_id: Some(mailbox.0),
            retry_after_s: None,
        },
        MintError::Crypto => ApiError::Internal,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn key() -> (UserId, MailboxId) {
        (UserId::new(Uuid::new_v4()), MailboxId::new(Uuid::new_v4()))
    }

    fn at(seconds: i64) -> Result<OffsetDateTime, time::error::ComponentRange> {
        OffsetDateTime::from_unix_timestamp(seconds)
    }

    /// ASVS V10.1.1: a cached access token is reused inside the window and
    /// dropped, not merely ignored, once it is older than the window.
    #[test]
    fn asvs_v10_1_1_expired_access_token_is_dropped_on_read(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let service = TokenService::new();
        let (user, mailbox) = key();
        let minted = at(1_700_000_000)?;
        service.insert_cached(
            user,
            mailbox,
            Sensitive::new("access-token".to_owned()),
            minted,
        );

        assert!(
            service
                .cached(
                    &user,
                    &mailbox,
                    minted + Duration::seconds(ACCESS_TOKEN_CACHE_S - 1)
                )
                .is_some(),
            "a token inside the window is reused"
        );
        assert!(
            service
                .cached(
                    &user,
                    &mailbox,
                    minted + Duration::seconds(ACCESS_TOKEN_CACHE_S)
                )
                .is_none(),
            "a token at the window edge is not reused"
        );
        assert!(service.lock().is_empty(), "the expired entry is not kept");
        Ok(())
    }

    /// ASVS V10.1.1: caching a token drops every entry that has already
    /// expired, so the map tracks live tokens only and cannot grow with every
    /// mailbox the instance has served.
    #[test]
    fn asvs_v10_1_1_eviction_on_insert_keeps_live_tokens_only(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let service = TokenService::new();
        let (user, older) = key();
        let (_, newer) = key();
        let minted = at(1_700_000_000)?;
        service.insert_cached(user, older, Sensitive::new("stale".to_owned()), minted);
        service.insert_cached(
            user,
            newer,
            Sensitive::new("fresh".to_owned()),
            minted + Duration::seconds(ACCESS_TOKEN_CACHE_S),
        );
        let cache = service.lock();
        assert_eq!(cache.len(), 1, "the expired entry is evicted");
        assert!(cache.contains_key(&(user, newer)));
        Ok(())
    }
}
