//! T-504: step-up by a fresh Google sign-in (S7 3.6; S6 section 4).
//!
//! Sensitive actions (delete account, link or disconnect a mailbox, every
//! admin write) demand a Google re-authentication made within the last five
//! minutes with one of the user's own linked accounts. The check is the
//! ID token's `auth_time`, never the server clock (ASVS V6.8.4, V7.5.1).

use domain::{Provider, ProviderSubjectId, UserId};
use obs::{SecurityEvent, Sensitive};
use ports::store::aad_fields;
use ports::{Aad, AuthIntent, Clock, Prompt, SessionState};
use time::{Duration, OffsetDateTime};

use crate::auth::callback::CallbackContext;
use crate::auth::outcome::Outcome;
use crate::auth::start::{begin_oauth, OAuthParams, StartResponse};
use crate::error::ApiError;
use crate::session::extract::AuthedSession;
use crate::session::store::{LoadedSession, NewCookie, SessionService};
use crate::state::AppState;

/// The step-up window: 5 minutes (S2 `STEP_UP_WINDOW`, decided; `[TUNABLE]`
/// per S7 3.6).
pub const STEP_UP_WINDOW_S: i64 = 300;

/// The `max_age` sent to Google: 300 s (S6 4, S4 3.1, S2 and ASVS V6.8.4).
pub const STEP_UP_MAX_AGE_S: u32 = 300;

/// Allowed clock skew for an `auth_time` slightly in the future.
const SKEW_S: i64 = 60;

/// `Ok(())` when `session.recent_auth_at` is within the step-up window.
///
/// # Errors
///
/// `ApiError::StepUpRequired` when the session has no fresh sign-in, when more
/// than [`STEP_UP_WINDOW_S`] has passed, or when the recorded `auth_time` is
/// more than [`SKEW_S`] in the future.
pub fn require_step_up(session: &AuthedSession, clock: &dyn Clock) -> Result<(), ApiError> {
    let now = clock.now();
    match session.recent_auth_at {
        Some(auth_time) if within_window(auth_time, now) => Ok(()),
        _ => Err(ApiError::StepUpRequired),
    }
}

/// `recent_auth_at` plus the step-up window while that instant is still in the
/// future, else `None` (S7 API-AUTH-3, for T-506: the app skips a round trip
/// that would return `403 step_up_required`).
#[must_use]
pub fn step_up_valid_until(
    session_recent_auth_at: Option<OffsetDateTime>,
    now: OffsetDateTime,
) -> Option<OffsetDateTime> {
    session_recent_auth_at
        .map(|auth_time| auth_time + Duration::seconds(STEP_UP_WINDOW_S))
        .filter(|until| *until > now)
}

/// Start a step-up round trip (API-AUTH-1 `intent: "step_up"`).
///
/// The session stays `Authenticated`; its ID rotates and `pre_auth` holds the
/// round trip (T-501). Nothing about the waiting action is stored (S7 3.6).
///
/// # Errors
///
/// `ApiError::Unauthenticated` when the session is not authenticated;
/// `ApiError::Internal` on a store or key failure.
pub async fn start_step_up(
    state: &AppState,
    session: &LoadedSession,
    authed: &AuthedSession,
) -> Result<(StartResponse, Option<NewCookie>), ApiError> {
    if session.record.record.state != SessionState::Authenticated {
        return Err(ApiError::Unauthenticated);
    }
    let params = OAuthParams {
        scopes: &adapters_gmail::scopes::STEP_UP_SCOPES,
        prompt: Some(Prompt::Login),
        max_age_s: Some(STEP_UP_MAX_AGE_S),
        login_hint: primary_login_hint(state, &authed.user).await,
        mailbox_id: None,
    };
    begin_oauth(state, session, AuthIntent::StepUp, None, params).await
}

/// Finish a step-up round trip (API-AUTH-2). The generic callback has already
/// checked `state`, exchanged the code and validated the ID token.
#[allow(clippy::too_many_lines)]
pub async fn finish_step_up(ctx: &CallbackContext<'_>) -> (Outcome, Option<NewCookie>) {
    let state = ctx.state;
    let record = &ctx.session.record.record;
    let Some(user) = record
        .user_id
        .filter(|_| record.state == SessionState::Authenticated)
    else {
        return (Outcome::Failed, None);
    };
    let Ok(sub) = ProviderSubjectId::new(ctx.claims.sub.clone()) else {
        return (Outcome::Failed, None);
    };
    // The `sub` must belong to one of the user's own linked mailboxes; compare
    // `sub`, never email.
    match state
        .ports
        .store
        .mailboxes()
        .by_subject(Provider::Gmail, &sub)
        .await
    {
        Ok(Some(mailbox)) if mailbox.record.user_id == user => {}
        Ok(_) => {
            step_up_event(state, "wrong_account", &user, None, &ctx.request_id);
            return (Outcome::StepUpWrongAccount, None);
        }
        Err(_) => return (Outcome::Failed, None),
    }
    let now = state.ports.clock.now();
    // A missing claim means Google did not re-authenticate; never default it to
    // `iat` or `now`.
    let Some(auth_time) = ctx.claims.auth_time else {
        step_up_event(state, "stale_auth_time", &user, None, &ctx.request_id);
        return (Outcome::Failed, None);
    };
    if !within_window(auth_time, now) {
        step_up_event(state, "stale_auth_time", &user, None, &ctx.request_id);
        return (Outcome::Failed, None);
    }
    let rotated = SessionService::new(state)
        .rotate(&ctx.session, move |r| {
            r.recent_auth_at = Some(auth_time);
            r.pre_auth = None;
        })
        .await;
    let Ok((cookie, _record)) = rotated else {
        return (Outcome::Failed, None);
    };
    step_up_event(
        state,
        "success",
        &user,
        Some(ctx.claims.amr.clone()),
        &ctx.request_id,
    );
    (Outcome::SteppedUp, Some(cookie))
}

/// True when `auth_time` is inside the window and not far in the future.
fn within_window(auth_time: OffsetDateTime, now: OffsetDateTime) -> bool {
    now - auth_time <= Duration::seconds(STEP_UP_WINDOW_S)
        && auth_time <= now + Duration::seconds(SKEW_S)
}

/// The primary mailbox's address for the `login_hint`, when it can be opened.
/// A failure here never fails the step-up (S7 API-AUTH-1).
async fn primary_login_hint(state: &AppState, user: &UserId) -> Option<Sensitive<String>> {
    let mailboxes = state.ports.store.mailboxes().by_user(user).await.ok()?;
    let primary = mailboxes.into_iter().find(|m| m.record.is_primary)?;
    let user_record = state.ports.store.users().get(user).await.ok()??;
    let aad = Aad {
        user: *user,
        scope: primary.record.mailbox_id.0.to_string(),
        field: aad_fields::MAILBOX_EMAIL,
    };
    let plain = state
        .ports
        .keys
        .open(
            user,
            &user_record.record.wrapped_data_key,
            &aad,
            &primary.record.email_address.0,
        )
        .await
        .ok()?;
    String::from_utf8(plain).ok().map(Sensitive::new)
}

/// Log a step-up security event (ASVS V16.3.1). Never logs `sub`, email or
/// tokens.
fn step_up_event(
    state: &AppState,
    outcome: &'static str,
    user: &UserId,
    amr: Option<Vec<String>>,
    request_id: &crate::http::request_id::RequestId,
) {
    obs::security_event(&SecurityEvent {
        action: "step_up",
        outcome,
        user: Some(obs::Pseudonymiser::new(state.config.rate_key.clone()).pseudo_id(&user.0)),
        request_id: Some(request_id.0),
        amr,
        provider: Some("gmail"),
        method: None,
    });
}
