//! `SessionService`: create, load, touch, rotate, establish and end sessions.
//!
//! Every time comparison uses the `Clock` port; the store only ever sees the
//! SHA-256 hash of the raw session ID (S7 3.2; S6 4; S3 `Session`).

use axum::http::HeaderMap;
use domain::UserId;
use obs::{PseudoId, Pseudonymiser, SecurityEvent, Sensitive};
use ports::{
    AuthIntent, Precondition, SessionHash, SessionRecord, SessionRecordId, SessionRepo,
    SessionState, StoreError, Versioned,
};
use time::{Duration, OffsetDateTime};

use crate::error::ApiError;
use crate::http::request_id::RequestId;
use crate::session::cookie::{read_cookie, set_cookie, RawSessionId};
use crate::session::pre_auth::{seal_pre_auth, PreAuthPlain};
use crate::state::AppState;

/// Idle timeout: 15 minutes (decided, S6 9).
pub const IDLE_TIMEOUT: Duration = Duration::minutes(15);
/// Absolute lifetime: 12 hours (decided, S6 9).
pub const ABSOLUTE_TIMEOUT: Duration = Duration::hours(12);
/// `pre_auth` TTL: 10 minutes `[TUNABLE]` (S7 3.2).
pub const PRE_AUTH_TTL: Duration = Duration::minutes(10);
/// Write `last_seen_at` at most once a minute `[DEFAULT]`.
pub const TOUCH_INTERVAL: Duration = Duration::seconds(60);

/// The exact `Set-Cookie` value a handler adds to a response.
pub struct NewCookie(pub axum::http::HeaderValue);

/// A loaded session: the raw ID (needed to re-issue the cookie) and the store
/// record with its version.
#[derive(Clone, Debug)]
pub struct LoadedSession {
    /// The raw ID from the request cookie.
    pub raw: RawSessionId,
    /// The live record and its optimistic-concurrency version.
    pub record: Versioned<SessionRecord>,
}

/// Why a session ended; the wire names are the security-event outcomes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EndReason {
    /// The user chose "Sign out".
    SignedOut,
    /// A new sign-in replaced it.
    Replaced,
    /// An admin ended it.
    AdminEnded,
    /// The account was deleted.
    AccountDeleted,
}

impl EndReason {
    /// The security-event outcome string.
    #[must_use]
    pub fn wire(self) -> &'static str {
        match self {
            Self::SignedOut => "signed_out",
            Self::Replaced => "replaced",
            Self::AdminEnded => "admin_ended",
            Self::AccountDeleted => "account_deleted",
        }
    }
}

/// Creates and manages sessions against the `AppState` ports.
pub struct SessionService<'a> {
    state: &'a AppState,
}

impl<'a> SessionService<'a> {
    /// Build a service over the shared application state.
    #[must_use]
    pub fn new(state: &'a AppState) -> Self {
        Self { state }
    }

    fn sessions(&self) -> &dyn SessionRepo {
        self.state.ports.store.sessions()
    }

    fn raw_id(&self) -> RawSessionId {
        RawSessionId::generate(self.state.ports.rng.as_ref())
    }

    fn csrf_token(&self) -> String {
        use base64::engine::general_purpose::URL_SAFE_NO_PAD;
        use base64::Engine;
        URL_SAFE_NO_PAD.encode(self.state.ports.rng.as_ref().bytes32())
    }

    fn pseudo(&self, user: Option<&UserId>) -> Option<PseudoId> {
        user.map(|u| Pseudonymiser::new(self.state.config.rate_key.clone()).pseudo_id(&u.0))
    }

    fn session_end(&self, user: Option<&UserId>, reason: EndReason, request_id: Option<RequestId>) {
        obs::security_event(&SecurityEvent {
            action: "session_end",
            outcome: reason.wire(),
            user: self.pseudo(user),
            request_id: request_id.map(|r| r.0),
            amr: None,
            provider: None,
            method: None,
        });
    }

    /// New anonymous `pre_auth` session (S7 API-AUTH-3 state "anonymous").
    ///
    /// # Errors
    ///
    /// `ApiError::Internal` on a store or key failure.
    pub async fn create_anonymous(
        &self,
    ) -> Result<(NewCookie, Versioned<SessionRecord>), ApiError> {
        let now = self.state.ports.clock.now();
        let raw = self.raw_id();
        let record_id = SessionRecordId(self.state.ports.rng.as_ref().uuid_v4());
        let plain = PreAuthPlain {
            intent: AuthIntent::SignIn,
            oauth_state: Sensitive::new(String::new()),
            nonce: Sensitive::new(String::new()),
            pkce_verifier: Sensitive::new(String::new()),
            invite_token_hash: None,
            pending_email: None,
            started_at: now,
        };
        let sealed =
            seal_pre_auth(self.state.ports.system_keys.as_ref(), &record_id, &plain).await?;
        let record = SessionRecord {
            session_hash: raw.hash(),
            session_record_id: record_id,
            state: SessionState::PreAuth,
            user_id: None,
            csrf_token: self.csrf_token(),
            created_at: now,
            last_seen_at: now,
            recent_auth_at: None,
            expires_at: now + IDLE_TIMEOUT,
            pre_auth: Some(sealed),
        };
        let version = self
            .sessions()
            .put(&record, Precondition::MustNotExist)
            .await?;
        Ok((NewCookie(set_cookie(&raw)), Versioned { record, version }))
    }

    /// Cookie to live record, enforcing every timeout. A dead record is deleted
    /// when seen; `last_seen_at` is touched at most once a minute.
    ///
    /// # Errors
    ///
    /// `ApiError::Internal` on a store failure.
    pub async fn load(&self, headers: &HeaderMap) -> Result<Option<LoadedSession>, ApiError> {
        let Some(raw) = read_cookie(headers) else {
            return Ok(None);
        };
        let hash = raw.hash();
        let Some(mut versioned) = self.sessions().get(&hash).await? else {
            return Ok(None);
        };
        let now = self.state.ports.clock.now();
        if Self::expired(&versioned.record, now) {
            let _ = self
                .sessions()
                .delete(&hash, Precondition::Matches(versioned.version.clone()))
                .await;
            return Ok(None);
        }
        if versioned.record.state == SessionState::Authenticated {
            let Some(user_id) = versioned.record.user_id else {
                let _ = self.sessions().delete(&hash, Precondition::None).await;
                return Ok(None);
            };
            if self
                .state
                .ports
                .store
                .users()
                .get(&user_id)
                .await?
                .is_none()
            {
                let _ = self.sessions().delete(&hash, Precondition::None).await;
                return Ok(None);
            }
        }

        let clear_pre_auth = versioned.record.state == SessionState::Authenticated
            && versioned
                .record
                .pre_auth
                .as_ref()
                .is_some_and(|p| now >= p.started_at + PRE_AUTH_TTL);
        let touch_due = now - versioned.record.last_seen_at >= TOUCH_INTERVAL;
        if touch_due || clear_pre_auth {
            let mut updated = versioned.record.clone();
            updated.last_seen_at = now;
            updated.expires_at = Self::expires_at(&updated, now);
            if clear_pre_auth {
                updated.pre_auth = None;
            }
            match self
                .sessions()
                .put(&updated, Precondition::Matches(versioned.version.clone()))
                .await
            {
                Ok(version) => {
                    versioned = Versioned {
                        record: updated,
                        version,
                    }
                }
                Err(StoreError::PreconditionFailed) => match self.sessions().get(&hash).await? {
                    Some(fresh) => versioned = fresh,
                    None => return Ok(None),
                },
                Err(e) => return Err(e.into()),
            }
        }
        Ok(Some(LoadedSession {
            raw,
            record: versioned,
        }))
    }

    /// New cookie value, same `session_record_id`, same `created_at`;
    /// `edit` changes other fields. The old record is deleted.
    ///
    /// # Errors
    ///
    /// `ApiError::Internal` on a store failure.
    pub async fn rotate(
        &self,
        current: &LoadedSession,
        edit: impl FnOnce(&mut SessionRecord) + Send,
    ) -> Result<(NewCookie, Versioned<SessionRecord>), ApiError> {
        let now = self.state.ports.clock.now();
        let raw = self.raw_id();
        let mut record = current.record.record.clone();
        record.session_hash = raw.hash();
        // `session_record_id`, `created_at`, `user_id` and `recent_auth_at` are
        // kept: rotation does not reset the absolute lifetime (V7.3.2).
        record.csrf_token = self.csrf_token();
        record.last_seen_at = now;
        record.expires_at = Self::expires_at(&record, now);
        edit(&mut record);
        let version = self
            .sessions()
            .put(&record, Precondition::MustNotExist)
            .await?;
        let _ = self
            .sessions()
            .delete(&current.record.record.session_hash, Precondition::None)
            .await;
        Ok((NewCookie(set_cookie(&raw)), Versioned { record, version }))
    }

    /// Sign-in and join: a brand-new authenticated session for `user`. Deletes
    /// `current` and every other session of the user, logging
    /// `session_end`/`replaced` for each (AU-07 AC1, AC6).
    ///
    /// # Errors
    ///
    /// `ApiError::Internal` on a store failure.
    pub async fn establish(
        &self,
        current: Option<&LoadedSession>,
        user: &UserId,
        recent_auth_at: Option<OffsetDateTime>,
    ) -> Result<(NewCookie, Versioned<SessionRecord>), ApiError> {
        let now = self.state.ports.clock.now();
        let raw = self.raw_id();
        let record = SessionRecord {
            session_hash: raw.hash(),
            session_record_id: SessionRecordId(self.state.ports.rng.as_ref().uuid_v4()),
            state: SessionState::Authenticated,
            user_id: Some(*user),
            csrf_token: self.csrf_token(),
            created_at: now,
            last_seen_at: now,
            recent_auth_at,
            expires_at: now + IDLE_TIMEOUT,
            pre_auth: None,
        };
        let version = self
            .sessions()
            .put(&record, Precondition::MustNotExist)
            .await?;
        if let Some(cur) = current {
            let _ = self
                .sessions()
                .delete(&cur.record.record.session_hash, Precondition::None)
                .await;
            if cur.record.record.user_id == Some(*user) {
                self.session_end(Some(user), EndReason::Replaced, None);
            }
        }
        for other in self.sessions().by_user(user).await? {
            if other.record.session_hash != record.session_hash {
                let _ = self
                    .sessions()
                    .delete(&other.record.session_hash, Precondition::None)
                    .await;
                self.session_end(Some(user), EndReason::Replaced, None);
            }
        }
        Ok((NewCookie(set_cookie(&raw)), Versioned { record, version }))
    }

    /// Delete one record and log `session_end` with the reason.
    ///
    /// # Errors
    ///
    /// `ApiError::Internal` on a store failure.
    pub async fn end(
        &self,
        hash: &SessionHash,
        user: Option<&UserId>,
        reason: EndReason,
        request_id: RequestId,
    ) -> Result<(), ApiError> {
        self.sessions().delete(hash, Precondition::None).await?;
        self.session_end(user, reason, Some(request_id));
        Ok(())
    }

    /// Admin end and account deletion: delete every session of the user,
    /// logging each. Returns how many records were deleted.
    ///
    /// # Errors
    ///
    /// `ApiError::Internal` on a store failure.
    pub async fn end_all_for_user(
        &self,
        user: &UserId,
        reason: EndReason,
        request_id: RequestId,
    ) -> Result<u64, ApiError> {
        let mut count = 0;
        for record in self.sessions().by_user(user).await? {
            self.sessions()
                .delete(&record.record.session_hash, Precondition::None)
                .await?;
            self.session_end(Some(user), reason, Some(request_id));
            count += 1;
        }
        Ok(count)
    }

    /// The stored TTL value: `min(now + IDLE_TIMEOUT, created_at + ABSOLUTE)`.
    fn expires_at(record: &SessionRecord, now: OffsetDateTime) -> OffsetDateTime {
        let idle = now + IDLE_TIMEOUT;
        let absolute = record.created_at + ABSOLUTE_TIMEOUT;
        if idle < absolute {
            idle
        } else {
            absolute
        }
    }

    /// Any of the three timeouts has passed: absolute, idle or `pre_auth`.
    fn expired(record: &SessionRecord, now: OffsetDateTime) -> bool {
        if now >= record.created_at + ABSOLUTE_TIMEOUT {
            return true;
        }
        if now >= record.last_seen_at + IDLE_TIMEOUT {
            return true;
        }
        matches!(
            record.state,
            SessionState::PreAuth | SessionState::PendingInviteRequest
        ) && record
            .pre_auth
            .as_ref()
            .map_or(true, |p| now >= p.started_at + PRE_AUTH_TTL)
    }
}
