//! API-AUTH-2: `GET /api/v1/auth/{provider}/callback`, the intent dispatch and
//! the `sign_in` and `join` finishes.

use axum::extract::{Path, Query, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use domain::{
    check_redemption, EmailAddress, InviteState, InviteStatus, MailboxId, MailboxStatus, Provider,
    ProviderSubjectId, RedeemRefusal, UserId,
};
use obs::{OpLog, Pseudonymiser, SecurityEvent, Sensitive};
use ports::store::aad_fields;
use ports::{
    Aad, AuthIntent, Ciphertext, IdClaims, InviteRecord, MailboxRecord, Precondition, SessionState,
    StoreError, TokenSet, UserRecord, Version,
};
use serde::Deserialize;
use time::OffsetDateTime;

use crate::auth::email_key::email_lookup_hash;
use crate::auth::outcome::{redirect, Outcome};
use crate::http::client_ip::client_ip;
use crate::http::request_id::RequestId;
use crate::limits::{policies, LimitSubject};
use crate::session::cookie::clear_cookie;
use crate::session::csrf::tokens_equal;
use crate::session::extract::AuthedSession;
use crate::session::pre_auth::{open_pre_auth, seal_pre_auth, PreAuthPlain};
use crate::session::store::{LoadedSession, NewCookie, SessionService, PRE_AUTH_TTL};
use crate::state::AppState;

const GOOGLE: &str = "google";

/// The callback query. Google's extra parameters are accepted and ignored so
/// `deny_unknown_fields` does not break real sign-ins.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CallbackQuery {
    /// The authorization code.
    pub code: Option<String>,
    /// The OAuth `state`, echoed back.
    pub state: String,
    /// The error code, when the user declined or Google refused.
    pub error: Option<String>,
    /// Google only, ignored.
    pub scope: Option<String>,
    /// Google only, ignored.
    pub authuser: Option<String>,
    /// Google only, ignored.
    pub prompt: Option<String>,
    /// Google only, ignored.
    pub hd: Option<String>,
}

/// Everything an intent handler needs after `state`, code and ID token are
/// verified.
pub struct CallbackContext<'a> {
    /// Shared application state.
    pub state: &'a AppState,
    /// The session being finished.
    pub session: LoadedSession,
    /// The opened `pre_auth` fields.
    pub pre: PreAuthPlain,
    /// The provider tokens.
    pub tokens: TokenSet,
    /// The validated ID token claims.
    pub claims: IdClaims,
    /// The request ID.
    pub request_id: RequestId,
}

/// What a finished callback reports.
struct Finished {
    outcome: Outcome,
    cookie: Option<NewCookie>,
    amr: Option<Vec<String>>,
    user: Option<UserId>,
}

impl Finished {
    fn plain(outcome: Outcome, cookie: Option<NewCookie>) -> Self {
        Self {
            outcome,
            cookie,
            amr: None,
            user: None,
        }
    }
}

/// `GET /api/v1/auth/{provider}/callback`.
#[must_use]
pub async fn callback_handler(
    State(state): State<AppState>,
    Path(provider): Path<String>,
    Query(query): Query<CallbackQuery>,
    headers: HeaderMap,
    request_id: RequestId,
) -> Response {
    if provider != GOOGLE {
        return redirect(&state.config, Outcome::Failed, None);
    }
    let ip = client_ip(&headers, state.config.xff_trusted_hops);
    if let Err(e) = state
        .limits
        .check(&policies::SIGN_IN_IP, LimitSubject::Ip(&ip), request_id)
        .await
    {
        return e.into_response();
    }
    let loaded = match SessionService::new(&state).load(&headers).await {
        Ok(loaded) => loaded,
        Err(e) => return e.into_response(),
    };
    let Some(session) = loaded else {
        return state_invalid(&state, false, request_id);
    };
    let Some(fields) = session.record.record.pre_auth.clone() else {
        return state_invalid(&state, true, request_id);
    };
    let now = state.ports.clock.now();
    if now >= fields.started_at + PRE_AUTH_TTL {
        return state_invalid(&state, true, request_id);
    }
    let Ok(pre) = open_pre_auth(
        state.ports.system_keys.as_ref(),
        &session.record.record.session_record_id,
        &fields,
    )
    .await
    else {
        return state_invalid(&state, true, request_id);
    };
    if !tokens_equal(query.state.as_bytes(), pre.oauth_state.expose().as_bytes()) {
        return state_invalid(&state, true, request_id);
    }
    // Single use from here: the `state`, `nonce` and verifier are spent.
    let cleared = spend_pre_auth(&state, &session).await;
    let finished = after_state(&state, session, pre, &query, request_id, cleared).await;
    event(
        &state,
        "sign_in",
        finished.outcome.code(),
        finished.user.as_ref(),
        finished.amr,
        request_id,
    );
    redirect(&state.config, finished.outcome, finished.cookie)
}

/// The callback after `state` has been checked and the fields spent.
async fn after_state(
    state: &AppState,
    session: LoadedSession,
    pre: PreAuthPlain,
    query: &CallbackQuery,
    request_id: RequestId,
    cleared: bool,
) -> Finished {
    let clears = if cleared {
        Some(NewCookie(clear_cookie()))
    } else {
        None
    };
    if let Some(err) = query.error.as_deref() {
        let outcome = if err == "access_denied" {
            Outcome::Cancelled
        } else {
            Outcome::Failed
        };
        return Finished::plain(outcome, clears);
    }
    let Some(code) = query.code.as_deref() else {
        return Finished::plain(Outcome::Failed, clears);
    };
    let Ok(tokens) = state
        .ports
        .identity
        .exchange(code, &pre.pkce_verifier, &state.config.oauth_redirect_uri)
        .await
    else {
        return Finished::plain(Outcome::Failed, clears);
    };
    let Ok(claims) = state
        .ports
        .identity
        .validate_id_token(&tokens.id_token, pre.nonce.expose())
        .await
    else {
        event(state, "sign_in", "id_token_invalid", None, None, request_id);
        return Finished::plain(Outcome::Failed, clears);
    };
    let sub = ProviderSubjectId::new(claims.sub.clone()).ok();
    let sub_is_linked = match sub.as_ref() {
        Some(sub) => match state
            .ports
            .store
            .mailboxes()
            .by_subject(Provider::Gmail, sub)
            .await
        {
            Ok(found) => found.is_some(),
            Err(_) => return Finished::plain(Outcome::Failed, clears),
        },
        None => false,
    };
    let intent = pre.intent;
    let context = CallbackContext {
        state,
        session,
        pre,
        tokens,
        claims,
        request_id,
    };
    let (outcome, login_cookie) = match intent {
        AuthIntent::SignIn => finish_sign_in(&context).await,
        AuthIntent::Join => finish_join(&context).await,
        AuthIntent::StepUp => crate::auth::step_up::finish_step_up(&context).await,
        AuthIntent::Link => finish_link(&context).await,
        AuthIntent::Reconnect => finish_reconnect(&context).await,
    };
    if !outcome.keeps_tokens() {
        discard_tokens(state, &context.tokens, sub_is_linked).await;
    }
    let user = if outcome.keeps_tokens() {
        linked_user(state, sub.as_ref()).await
    } else {
        None
    };
    Finished {
        outcome,
        cookie: login_cookie.or(clears),
        amr: Some(context.claims.amr.clone()),
        user,
    }
}

/// The user that owns the mailbox for `sub`, if any.
async fn linked_user(state: &AppState, sub: Option<&ProviderSubjectId>) -> Option<UserId> {
    let sub = sub?;
    state
        .ports
        .store
        .mailboxes()
        .by_subject(Provider::Gmail, sub)
        .await
        .ok()
        .flatten()
        .map(|mailbox| mailbox.record.user_id)
}

/// A `sign_in` finish for a returning user (S7 3.4).
pub async fn finish_sign_in(ctx: &CallbackContext<'_>) -> (Outcome, Option<NewCookie>) {
    let state = ctx.state;
    let Ok(sub) = ProviderSubjectId::new(ctx.claims.sub.clone()) else {
        return (Outcome::Failed, None);
    };
    let mailbox = match state
        .ports
        .store
        .mailboxes()
        .by_subject(Provider::Gmail, &sub)
        .await
    {
        Ok(Some(mailbox)) => mailbox,
        Ok(None) => return (Outcome::NotRegistered, None),
        Err(_) => return (Outcome::Failed, None),
    };
    let user = mailbox.record.user_id;
    let Ok((cookie, _record)) = SessionService::new(state)
        .establish(Some(&ctx.session), &user, None)
        .await
    else {
        return (Outcome::Failed, None);
    };
    // A complete new grant replaces the stored refresh token; an incomplete one
    // keeps whatever is there.
    let required = &adapters_gmail::scopes::GMAIL_SCOPES[2..];
    if let Some(refresh) = ctx.tokens.refresh_token.as_ref() {
        let failed = has_grant(&ctx.tokens.granted_scopes, required)
            && state
                .tokens
                .store_refresh_token(state, &user, &mailbox.record.mailbox_id, refresh.clone())
                .await
                .is_err();
        if failed {
            op_failed("sign_in_store_grant");
        }
    }
    (Outcome::SignedIn, Some(cookie))
}

/// A `join` finish: redeem an invite, or ask for one (S7 3.4).
#[allow(clippy::too_many_lines)]
pub async fn finish_join(ctx: &CallbackContext<'_>) -> (Outcome, Option<NewCookie>) {
    let state = ctx.state;
    let Ok(sub) = ProviderSubjectId::new(ctx.claims.sub.clone()) else {
        return (Outcome::Failed, None);
    };
    let linked = match state
        .ports
        .store
        .mailboxes()
        .by_subject(Provider::Gmail, &sub)
        .await
    {
        Ok(found) => found.is_some(),
        Err(_) => return (Outcome::Failed, None),
    };
    if linked {
        // For an existing account: signs in as `sign_in` would; the invite is
        // not touched.
        return finish_sign_in(ctx).await;
    }
    let Ok(email) = EmailAddress::parse(ctx.claims.email.expose().as_str()) else {
        return (Outcome::EmailUnverified, None);
    };
    let Some(token_hash) = ctx.pre.invite_token_hash else {
        if !ctx.claims.email_verified {
            return (Outcome::EmailUnverified, None);
        }
        return request_invite(ctx, &sub, &email).await;
    };
    if state
        .limits
        .check(
            &policies::SIGN_IN_FAILED_SUBJECT,
            LimitSubject::Subject(&ctx.claims.sub),
            ctx.request_id,
        )
        .await
        .is_err()
    {
        return (Outcome::Failed, None);
    }
    let now = state.ports.clock.now();
    let lookup = email_lookup_hash(&state.config.email_lookup_key, &email);
    let Ok(found) = state.ports.store.invites().by_token_hash(&token_hash).await else {
        return (Outcome::Failed, None);
    };
    let invite_state = found.as_ref().map(|v| invite_state(&v.record));
    if let Err(refusal) = check_redemption(
        invite_state.as_ref(),
        Some(&token_hash.0),
        &lookup.0,
        ctx.claims.email_verified,
        now,
    ) {
        return match refusal {
            RedeemRefusal::EmailUnverified => (Outcome::EmailUnverified, None),
            RedeemRefusal::InviteInvalid => (Outcome::InviteInvalid, None),
            RedeemRefusal::EmailMismatch => (Outcome::EmailMismatch, None),
        };
    }
    let Some(versioned) = found else {
        return (Outcome::InviteInvalid, None);
    };
    // Check the grant and the scopes BEFORE claiming the invite, so a granular
    // consent refresh or a missing refresh token does not burn a single-use
    // invite that only an admin can reissue (security review F1).
    let Some(refresh) = ctx.tokens.refresh_token.as_ref() else {
        op_failed("join_missing_grant");
        return (Outcome::Failed, None);
    };
    if !has_grant(
        &ctx.tokens.granted_scopes,
        adapters_gmail::scopes::GMAIL_SCOPES.as_slice(),
    ) {
        op_failed("join_missing_grant");
        return (Outcome::Failed, None);
    }
    // Claim the invite, so it cannot be used twice (V2.3.4).
    let mut claimed = versioned.record.clone();
    claimed.status = InviteStatus::Used;
    match state
        .ports
        .store
        .invites()
        .put(&claimed, Precondition::Matches(versioned.version.clone()))
        .await
    {
        Ok(_) => {}
        Err(StoreError::PreconditionFailed) => return (Outcome::InviteInvalid, None),
        Err(_) => return (Outcome::Failed, None),
    }
    // Every failure from here on puts the invite back to `Pending`: it has
    // already been claimed, and leaving it `Used` strands the invitee
    // (security review F1).
    let created = match create_user_and_mailbox(ctx, &sub, &email, refresh, now).await {
        None => {
            unclaim_invite(state, &claimed).await;
            return (Outcome::Failed, None);
        }
        Some(Created::LinkedElsewhere) => {
            unclaim_invite(state, &claimed).await;
            return (Outcome::MailboxLinkedElsewhere, None);
        }
        Some(Created::Created(user, mailbox_id)) => (user, mailbox_id),
    };
    let (user, mailbox_id) = created;
    let Ok((cookie, _record)) = SessionService::new(state)
        .establish(Some(&ctx.session), &user, ctx.claims.auth_time)
        .await
    else {
        unclaim_invite(state, &claimed).await;
        return (Outcome::Failed, None);
    };
    event(
        state,
        "invite_use",
        "used",
        Some(&user),
        None,
        ctx.request_id,
    );
    event(
        state,
        "mailbox_link",
        "linked",
        Some(&user),
        None,
        ctx.request_id,
    );
    let _ = mailbox_id;
    (Outcome::Joined, Some(cookie))
}

/// Rotate to `pending_invite_request` holding the encrypted email, and report
/// `not_invited` (AU-02 AC1).
async fn request_invite(
    ctx: &CallbackContext<'_>,
    _sub: &ProviderSubjectId,
    email: &EmailAddress,
) -> (Outcome, Option<NewCookie>) {
    let state = ctx.state;
    let now = state.ports.clock.now();
    let plain = PreAuthPlain {
        intent: AuthIntent::Join,
        oauth_state: adapters_gmail::pkce::new_state_or_nonce(state.ports.rng.as_ref()),
        nonce: adapters_gmail::pkce::new_state_or_nonce(state.ports.rng.as_ref()),
        pkce_verifier: adapters_gmail::pkce::new_pkce(state.ports.rng.as_ref()).verifier,
        invite_token_hash: None,
        pending_email: Some(Sensitive::new(email.as_str().to_owned())),
        mailbox_id: None,
        started_at: now,
    };
    let sealed = seal_pre_auth(
        state.ports.system_keys.as_ref(),
        &ctx.session.record.record.session_record_id,
        &plain,
    )
    .await;
    let Ok(sealed) = sealed else {
        return (Outcome::Failed, None);
    };
    let rotated = SessionService::new(state)
        .rotate(&ctx.session, move |record| {
            record.state = SessionState::PendingInviteRequest;
            record.pre_auth = Some(sealed);
        })
        .await;
    match rotated {
        Ok((cookie, _record)) => (Outcome::NotInvited, Some(cookie)),
        Err(_) => (Outcome::Failed, None),
    }
}

/// Finish a `link` round trip (T-601a): link another mailbox to the current
/// user, or refresh it when the Google account is already linked (AU-04).
async fn finish_link(ctx: &CallbackContext<'_>) -> (Outcome, Option<NewCookie>) {
    let state = ctx.state;
    if !ctx.claims.email_verified {
        return (Outcome::EmailUnverified, None);
    }
    let Ok(authed) = AuthedSession::load(state, &ctx.session).await else {
        return (Outcome::Failed, None);
    };
    let Some(refresh) = ctx.tokens.refresh_token.clone() else {
        event(
            state,
            "mailbox_link",
            "failed",
            Some(&authed.user),
            None,
            ctx.request_id,
        );
        return (Outcome::Failed, None);
    };
    let outcome =
        crate::services::mailbox_link::complete_link(state, &authed, &ctx.claims, refresh)
            .await
            .unwrap_or(crate::services::mailbox_link::LinkOutcome::Failed);
    event(
        state,
        "mailbox_link",
        outcome.as_code(),
        Some(&authed.user),
        None,
        ctx.request_id,
    );
    match outcome.outcome() {
        Outcome::Linked => {
            // The session ID rotates (S7 3.2), but `recent_auth_at` is
            // deliberately left alone. The ID token that proved this link is a
            // fresh authentication of the Google account that was linked, not
            // of the user (security review F1), so a `link` must never extend
            // the step-up window. Otherwise a caller holding a stolen session
            // could renew the window forever by linking new accounts of their
            // own.
            let rotated = SessionService::new(state)
                .rotate(&ctx.session, move |record| {
                    record.pre_auth = None;
                })
                .await;
            match rotated {
                Ok((cookie, _record)) => (Outcome::Linked, Some(cookie)),
                Err(_) => (Outcome::Failed, None),
            }
        }
        other => (other, None),
    }
}

/// Finish a `reconnect` round trip (T-601a): refresh a mailbox's grant
/// (ST-03 AC1). The mailbox named in the OAuth state is used, never one derived
/// from the account that was signed in with.
async fn finish_reconnect(ctx: &CallbackContext<'_>) -> (Outcome, Option<NewCookie>) {
    let state = ctx.state;
    if !ctx.claims.email_verified {
        return (Outcome::EmailUnverified, None);
    }
    let Ok(authed) = AuthedSession::load(state, &ctx.session).await else {
        return (Outcome::Failed, None);
    };
    let Some(mailbox_id) = ctx.pre.mailbox_id else {
        event(
            state,
            "mailbox_reconnect",
            "failed",
            Some(&authed.user),
            None,
            ctx.request_id,
        );
        return (Outcome::Failed, None);
    };
    let Some(refresh) = ctx.tokens.refresh_token.clone() else {
        event(
            state,
            "mailbox_reconnect",
            "failed",
            Some(&authed.user),
            None,
            ctx.request_id,
        );
        return (Outcome::Failed, None);
    };
    let outcome = crate::services::mailbox_link::complete_reconnect(
        state,
        &authed,
        &mailbox_id,
        &ctx.claims,
        refresh,
    )
    .await
    .unwrap_or(crate::services::mailbox_link::LinkOutcome::Failed);
    event(
        state,
        "mailbox_reconnect",
        outcome.as_code(),
        Some(&authed.user),
        None,
        ctx.request_id,
    );
    match outcome.outcome() {
        Outcome::Reconnected => {
            let rotated = SessionService::new(state)
                .rotate(&ctx.session, |record| {
                    record.pre_auth = None;
                })
                .await;
            match rotated {
                Ok((cookie, _record)) => (Outcome::Reconnected, Some(cookie)),
                Err(_) => (Outcome::Failed, None),
            }
        }
        other => (other, None),
    }
}

/// What `create_user_and_mailbox` produced.
enum Created {
    Created(UserId, MailboxId),
    LinkedElsewhere,
}

/// Create the user and the primary mailbox. `None` means a hard failure.
async fn create_user_and_mailbox(
    ctx: &CallbackContext<'_>,
    sub: &ProviderSubjectId,
    email: &EmailAddress,
    refresh: &Sensitive<String>,
    now: OffsetDateTime,
) -> Option<Created> {
    let state = ctx.state;
    let user_id = UserId::new(state.ports.rng.uuid_v4());
    let Ok(wrapped) = state.ports.keys.new_user_key(&user_id).await else {
        return None;
    };
    let user = UserRecord {
        user_id,
        created_at: now,
        is_admin: false,
        wrapped_data_key: wrapped,
        experiments_consent_version: None,
        experiments_opted_in_at: None,
    };
    let Ok(user_version) = state
        .ports
        .store
        .users()
        .put(&user, Precondition::MustNotExist)
        .await
    else {
        return None;
    };
    let mailbox_id = ports::mailbox_id_for(Provider::Gmail, sub);
    let aad = Aad {
        user: user_id,
        scope: mailbox_id.0.to_string(),
        field: aad_fields::MAILBOX_EMAIL,
    };
    let sealed_email = state
        .ports
        .keys
        .seal(
            &user_id,
            &user.wrapped_data_key,
            &aad,
            email.as_str().as_bytes(),
        )
        .await;
    let Ok(sealed_email) = sealed_email else {
        remove_user(state, &user_id, &user_version).await;
        return None;
    };
    let sealed_refresh =
        svc_common::mint::seal_refresh_token(&state.ports, &user, &mailbox_id, refresh).await;
    let Ok(sealed_refresh) = sealed_refresh else {
        remove_user(state, &user_id, &user_version).await;
        return None;
    };
    let mailbox = MailboxRecord {
        mailbox_id,
        user_id,
        provider: Provider::Gmail,
        provider_subject_id: sub.clone(),
        email_address: Ciphertext(sealed_email),
        status: MailboxStatus::Connected,
        linked_at: now,
        is_primary: true,
        refresh_token: Some(sealed_refresh),
    };
    match state
        .ports
        .store
        .mailboxes()
        .put(&mailbox, Precondition::MustNotExist)
        .await
    {
        Ok(_) => Some(Created::Created(user_id, mailbox_id)),
        Err(StoreError::AlreadyExists) => {
            remove_user(state, &user_id, &user_version).await;
            Some(Created::LinkedElsewhere)
        }
        Err(_) => {
            remove_user(state, &user_id, &user_version).await;
            None
        }
    }
}

async fn remove_user(state: &AppState, user: &UserId, version: &Version) {
    let _ = state
        .ports
        .store
        .users()
        .delete(user, Precondition::Matches(version.clone()))
        .await;
}

/// Revoke new provider tokens unless the outcome keeps them, and only when
/// `sub` is linked to no user (Google revokes the whole grant).
pub async fn discard_tokens(state: &AppState, tokens: &TokenSet, sub_is_linked: bool) {
    if sub_is_linked {
        return;
    }
    let token = tokens
        .refresh_token
        .as_ref()
        .map_or(&tokens.access_token, |refresh| refresh);
    if state.ports.identity.revoke(token).await.is_err() {
        op_failed("token_revoke");
    }
}

/// True when every required scope was granted.
fn has_grant(granted: &[String], required: &[&str]) -> bool {
    required
        .iter()
        .all(|scope| granted.iter().any(|given| given == scope))
}

/// Put a claimed invite back to `Pending` after a later join step failed, so a
/// single-use invite is not burned by a partial-consent refresh or a transient
/// failure (security review F1). Best effort: the record is re-read, and a
/// record that is no longer `Used` (or is gone) is left alone.
async fn unclaim_invite(state: &AppState, claimed: &InviteRecord) {
    let Ok(Some(current)) = state.ports.store.invites().get(&claimed.invite_id).await else {
        return;
    };
    if current.record.status != InviteStatus::Used {
        return;
    }
    let mut record = current.record;
    record.status = InviteStatus::Pending;
    let _ = state
        .ports
        .store
        .invites()
        .put(&record, Precondition::Matches(current.version))
        .await;
}

fn invite_state(record: &InviteRecord) -> InviteState {
    InviteState {
        status: record.status,
        token_hash: record.token_hash.0,
        email_hash: record.email_lookup.0,
        last_sent_at: record.last_sent_at,
        expires_at: record.expires_at,
    }
}

/// Spend the `pre_auth` fields: delete a pre-auth record and clear the cookie,
/// or clear the field set on an authenticated session.
async fn spend_pre_auth(state: &AppState, session: &LoadedSession) -> bool {
    let record = &session.record.record;
    let precondition = Precondition::Matches(session.record.version.clone());
    match record.state {
        SessionState::PreAuth | SessionState::PendingInviteRequest => {
            let _ = state
                .ports
                .store
                .sessions()
                .delete(&record.session_hash, precondition)
                .await;
            true
        }
        SessionState::Authenticated => {
            let mut updated = record.clone();
            updated.pre_auth = None;
            let _ = state
                .ports
                .store
                .sessions()
                .put(&updated, precondition)
                .await;
            false
        }
    }
}

fn state_invalid(state: &AppState, clear: bool, request_id: RequestId) -> Response {
    event(state, "sign_in", "state_invalid", None, None, request_id);
    let cookie = clear.then(|| NewCookie(clear_cookie()));
    redirect(&state.config, Outcome::Failed, cookie)
}

fn op_failed(op: &'static str) {
    obs::op_log(&OpLog {
        op,
        outcome: "failed",
        status: None,
        latency_ms: None,
    });
}

fn event(
    state: &AppState,
    action: &'static str,
    outcome: &'static str,
    user: Option<&UserId>,
    amr: Option<Vec<String>>,
    request_id: RequestId,
) {
    obs::security_event(&SecurityEvent {
        action,
        outcome,
        user: user.map(|u| Pseudonymiser::new(state.config.rate_key.clone()).pseudo_id(&u.0)),
        request_id: Some(request_id.0),
        amr,
        provider: Some("gmail"),
        method: None,
    });
}
