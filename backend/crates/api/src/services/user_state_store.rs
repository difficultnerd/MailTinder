//! The user's state file in the primary mailbox's app folder (T-602b).
//!
//! Sealed under the user's `data_key` with an AAD that names the user, not the
//! mailbox, so the file can move between Drives as bytes. Changes go through
//! `update`, which re-runs on a fresh copy after every `ETag` conflict.

use std::sync::Arc;

use domain::user_state::{UserState, USER_STATE_VERSION};
use domain::UserId;
use obs::{Pseudonymiser, SecurityEvent};
use ports::store::aad_fields;
use ports::{Aad, AppFolderError, ETag, MailboxCtx, WrappedKey};

use uuid::Uuid;

use crate::error::ApiError;
use crate::state::AppState;

/// [DEFAULT] three tries covers two racing tabs
pub const UPDATE_ATTEMPTS: u32 = 3;

/// The `Aad::scope` of the state file: a literal, not a mailbox or user id.
const SCOPE: &str = "app_folder";

pub struct Loaded {
    pub state: UserState,
    pub etag: Option<ETag>,
}

pub struct UserStateStore {
    state: Arc<AppState>,
}

impl UserStateStore {
    #[must_use]
    pub fn new(state: Arc<AppState>) -> Self {
        Self { state }
    }

    /// The primary mailbox context for the user's file.
    async fn context(&self, user: &UserId) -> Result<MailboxCtx, ApiError> {
        let mailboxes = self.state.ports.store.mailboxes().by_user(user).await?;
        let primary = mailboxes
            .iter()
            .find(|m| m.record.is_primary)
            .map(|m| m.record.mailbox_id)
            .ok_or(ApiError::Internal)?;
        self.state
            .tokens
            .mailbox_ctx(&self.state, user, &primary)
            .await
            .map_err(|_| ApiError::MailboxNeedsSignIn {
                mailbox_id: Some(primary.0),
            })
    }

    async fn wrapped_key(&self, user: &UserId) -> Result<WrappedKey, ApiError> {
        self.state
            .ports
            .store
            .users()
            .get(user)
            .await?
            .map(|u| u.record.wrapped_data_key)
            .ok_or(ApiError::Internal)
    }

    fn aad(user: &UserId) -> Aad {
        Aad {
            user: *user,
            scope: SCOPE.to_owned(),
            field: aad_fields::APP_FOLDER_USER_STATE,
        }
    }

    /// # Errors
    /// `Internal` when the file is unreadable or a newer version;
    /// `MailboxNeedsSignIn` when the primary mailbox has no usable token.
    pub async fn load(&self, user: &UserId) -> Result<Loaded, ApiError> {
        let ctx = self.context(user).await?;
        self.load_with(user, &ctx).await
    }

    async fn load_with(&self, user: &UserId, ctx: &MailboxCtx) -> Result<Loaded, ApiError> {
        let Some((bytes, etag)) = self.state.ports.app_folder.read(ctx).await? else {
            return Ok(Loaded {
                state: UserState {
                    version: USER_STATE_VERSION,
                    ..UserState::default()
                },
                etag: None,
            });
        };
        let wrapped = self.wrapped_key(user).await?;
        let Ok(plain) = self
            .state
            .ports
            .keys
            .open(user, &wrapped, &Self::aad(user), &bytes)
            .await
        else {
            return Err(self.unreadable(user));
        };
        let Ok(state) = serde_json::from_slice::<UserState>(&plain) else {
            return Err(self.unreadable(user));
        };
        if state.version > USER_STATE_VERSION {
            return Err(self.unreadable(user));
        }
        Ok(Loaded {
            state,
            etag: Some(etag),
        })
    }

    /// Re-runs `f` on a fresh copy after every `ETag` conflict. `f` must be
    /// pure: no provider or store calls.
    ///
    /// # Errors
    /// Any `load` error, a provider error from the write, or
    /// `ProviderUnavailable` after `UPDATE_ATTEMPTS` conflicts.
    pub async fn update<R, F>(&self, user: &UserId, f: F) -> Result<R, ApiError>
    where
        F: Fn(&mut UserState) -> R + Send,
        R: Send,
    {
        for _ in 0..UPDATE_ATTEMPTS {
            let ctx = self.context(user).await?;
            let mut loaded = self.load_with(user, &ctx).await?;
            let out = f(&mut loaded.state);
            loaded.state.trim(self.state.ports.clock.now());
            let plain = serde_json::to_vec(&loaded.state).map_err(|_| ApiError::Internal)?;
            let wrapped = self.wrapped_key(user).await?;
            let sealed = self
                .state
                .ports
                .keys
                .seal(user, &wrapped, &Self::aad(user), &plain)
                .await
                .map_err(|_| ApiError::Internal)?;
            match self
                .state
                .ports
                .app_folder
                .write(&ctx, &sealed, loaded.etag.as_ref())
                .await
            {
                Ok(_) => return Ok(out),
                Err(AppFolderError::Conflict) => {}
                Err(AppFolderError::Mail(e)) => return Err(ApiError::from(e)),
            }
        }
        Err(ApiError::ProviderUnavailable {
            mailbox_id: None,
            retry_after_s: Some(1),
        })
    }

    /// `update`, but applied at most once per swipe. When a `RecentSwipe` with
    /// `swipe_id` is already in the state, `f` is not applied and `false` is
    /// returned.
    ///
    /// The check runs inside the write closure, so an `ETag`-conflict retry
    /// re-reads the state and re-evaluates it: two concurrent requests with the
    /// same `Idempotency-Key` cannot both apply, whichever order they resolve
    /// in (ASVS V2.3.4). The losing side answers from the recorded response.
    ///
    /// # Errors
    /// As `update`.
    pub async fn update_once<F>(
        &self,
        user: &UserId,
        swipe_id: Uuid,
        f: F,
    ) -> Result<bool, ApiError>
    where
        F: Fn(&mut UserState) + Send,
    {
        self.update_once_result(user, swipe_id, move |s| {
            f(s);
            Ok(())
        })
        .await
    }

    /// Apply a fallible swipe closure once, discarding mutations on failure.
    ///
    /// # Errors
    /// As `update`, or the error returned by the closure.
    pub async fn update_once_result<F>(
        &self,
        user: &UserId,
        swipe_id: Uuid,
        f: F,
    ) -> Result<bool, ApiError>
    where
        F: Fn(&mut UserState) -> Result<(), ApiError> + Send,
    {
        self.update(user, move |s: &mut UserState| {
            if s.recent_swipes.iter().any(|r| r.swipe_id == swipe_id) {
                return Ok(false);
            }
            let mut candidate = s.clone();
            f(&mut candidate)?;
            *s = candidate;
            Ok(true)
        })
        .await?
    }

    /// The file exists but cannot be read; it is never overwritten.
    fn unreadable(&self, user: &UserId) -> ApiError {
        security_event_unreadable(&self.state, user);
        ApiError::Internal
    }
}

/// A pseudonymous security event; never state, an address or a token.
fn security_event_unreadable(app: &AppState, user: &UserId) {
    obs::security_event(&SecurityEvent {
        action: "app_folder",
        outcome: "unreadable",
        user: Some(Pseudonymiser::new(app.config.rate_key.clone()).pseudo_id(&user.0)),
        request_id: None,
        amr: None,
        provider: None,
        method: None,
    });
}
