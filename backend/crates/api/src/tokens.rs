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
        let mut cache = self.lock();
        cache.insert(
            (*user, *mailbox),
            CacheEntry {
                token: ctx.access_token.clone(),
                minted_at: now,
            },
        );
        drop(cache);
        Ok(ctx)
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
    /// [`ACCESS_TOKEN_CACHE_S`].
    fn cached(
        &self,
        user: &UserId,
        mailbox: &MailboxId,
        now: OffsetDateTime,
    ) -> Option<Sensitive<String>> {
        let cache = self.lock();
        cache
            .get(&(*user, *mailbox))
            .filter(|entry| now - entry.minted_at < Duration::seconds(ACCESS_TOKEN_CACHE_S))
            .map(|entry| entry.token.clone())
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
