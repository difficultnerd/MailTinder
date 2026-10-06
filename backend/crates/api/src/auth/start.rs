//! API-AUTH-1: `POST /api/v1/auth/{provider}/start` and the shared
//! `begin_oauth` every intent uses.

use axum::extract::{Path, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::Response;
use obs::Sensitive;
use ports::{AuthIntent, Prompt, SessionState, Sha256Hash};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::error::ApiError;
use crate::http::client_ip::client_ip;
use crate::http::json::{json_ok, ApiJson};
use crate::http::request_id::RequestId;
use crate::limits::{policies, LimitSubject};
use crate::session::extract::AnySession;
use crate::session::pre_auth::{seal_pre_auth, PreAuthPlain};
use crate::session::store::{LoadedSession, NewCookie, SessionService};
use crate::state::AppState;

/// The provider name accepted in v1 (S7 5.2). Microsoft is v2.
const GOOGLE: &str = "google";

/// The invite token shape (S7 3.4): 43 to 64 base64url characters.
const INVITE_TOKEN_MIN: usize = 43;
const INVITE_TOKEN_MAX: usize = 64;

/// API-AUTH-1 request body.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StartRequest {
    /// Why the round trip is started.
    pub intent: AuthIntentWire,
    /// The invite token from the URL fragment, only with `join`.
    pub invite_token: Option<String>,
    /// The mailbox to reconnect, only with `reconnect`.
    pub mailbox_id: Option<Uuid>,
}

/// The wire form of an intent.
#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthIntentWire {
    SignIn,
    Join,
    Link,
    Reconnect,
    StepUp,
}

impl AuthIntentWire {
    /// The stored intent.
    #[must_use]
    pub fn resolve(&self) -> AuthIntent {
        match self {
            Self::SignIn => AuthIntent::SignIn,
            Self::Join => AuthIntent::Join,
            Self::Link => AuthIntent::Link,
            Self::Reconnect => AuthIntent::Reconnect,
            Self::StepUp => AuthIntent::StepUp,
        }
    }
}

/// API-AUTH-1 response body.
#[derive(Serialize)]
pub struct StartResponse {
    /// The URL the app navigates the browser to.
    pub authorization_url: String,
}

/// What an intent asks Google for.
pub struct OAuthParams {
    /// Exactly the S8 scopes for this intent (AU-04 AC5).
    pub scopes: &'static [&'static str],
    /// The `prompt` value, when the intent sets one.
    pub prompt: Option<Prompt>,
    /// The `max_age` value, when the intent sets one.
    pub max_age_s: Option<u32>,
    /// The `login_hint`, when the intent sets one.
    pub login_hint: Option<Sensitive<String>>,
}

/// New `state`, `nonce` and PKCE; sealed into `pre_auth` on the session; the
/// authorization URL built from them. Shared by every intent.
///
/// # Errors
///
/// `ApiError::Internal` on a store or key failure.
pub async fn begin_oauth(
    state: &AppState,
    session: &LoadedSession,
    intent: AuthIntent,
    invite_token_hash: Option<Sha256Hash>,
    p: OAuthParams,
) -> Result<(StartResponse, Option<NewCookie>), ApiError> {
    let now = state.ports.clock.now();
    let oauth_state = adapters_gmail::pkce::new_state_or_nonce(state.ports.rng.as_ref());
    let nonce = adapters_gmail::pkce::new_state_or_nonce(state.ports.rng.as_ref());
    let pkce = adapters_gmail::pkce::new_pkce(state.ports.rng.as_ref());
    let record_id = session.record.record.session_record_id;
    let plain = PreAuthPlain {
        intent,
        oauth_state: oauth_state.clone(),
        nonce: nonce.clone(),
        pkce_verifier: pkce.verifier.clone(),
        invite_token_hash,
        pending_email: None,
        started_at: now,
    };
    let sealed = seal_pre_auth(state.ports.system_keys.as_ref(), &record_id, &plain).await?;
    // A pre-auth or pending session stays pre-auth (a new intent restarts the
    // round trip); an authenticated session keeps its state for link, reconnect
    // and step-up.
    let next = match session.record.record.state {
        SessionState::PreAuth | SessionState::PendingInviteRequest => SessionState::PreAuth,
        SessionState::Authenticated => SessionState::Authenticated,
    };
    let sealed_for_edit = sealed.clone();
    let (cookie, _record) = SessionService::new(state)
        .rotate(session, move |record| {
            record.state = next;
            record.pre_auth = Some(sealed_for_edit);
        })
        .await?;
    let request = ports::AuthRequest {
        state: oauth_state.expose().clone(),
        nonce: nonce.expose().clone(),
        code_challenge: pkce.challenge,
        redirect_uri: state.config.oauth_redirect_uri.clone(),
        scopes: p.scopes.to_vec(),
        prompt: p.prompt,
        max_age_s: p.max_age_s,
        login_hint: p.login_hint,
    };
    let authorization_url = state.ports.identity.authorize_url(&request).to_string();
    Ok((StartResponse { authorization_url }, Some(cookie)))
}

/// `POST /api/v1/auth/{provider}/start`.
///
/// # Errors
///
/// `400 invalid_request` for a bad provider, intent combination or invite
/// token; `401 unauthenticated` when an authenticated-only intent has no
/// authenticated session; `429 rate_limited` over the IP policy.
pub async fn start_handler(
    State(state): State<AppState>,
    Path(provider): Path<String>,
    session: AnySession,
    request_id: RequestId,
    headers: HeaderMap,
    ApiJson(body): ApiJson<StartRequest>,
) -> Result<Response, ApiError> {
    if provider != GOOGLE {
        return Err(invalid_request("provider"));
    }
    let ip = client_ip(&headers, state.config.xff_trusted_hops);
    state
        .limits
        .check(&policies::SIGN_IN_IP, LimitSubject::Ip(&ip), request_id)
        .await?;

    let intent = body.intent.resolve();
    let state_kind = session.0.record.record.state;
    validate_intent(intent, state_kind)?;

    let invite_token_hash = if let Some(token) = body.invite_token.as_deref() {
        if intent != AuthIntent::Join {
            return Err(invalid_request("invite_token"));
        }
        Some(hash_invite_token(token)?)
    } else {
        None
    };
    if body.mailbox_id.is_some() && intent != AuthIntent::Reconnect {
        return Err(invalid_request("mailbox_id"));
    }

    let (scopes, prompt) = match intent {
        AuthIntent::SignIn => (
            adapters_gmail::scopes::GMAIL_SCOPES.as_slice(),
            Some(Prompt::SelectAccount),
        ),
        AuthIntent::Join => (
            adapters_gmail::scopes::GMAIL_SCOPES.as_slice(),
            Some(Prompt::Consent),
        ),
        // Delegated to T-601a (`link`, `reconnect`) and T-504 (`step_up`).
        AuthIntent::Link | AuthIntent::Reconnect | AuthIntent::StepUp => {
            return Err(invalid_request("intent"));
        }
    };
    let params = OAuthParams {
        scopes,
        prompt,
        max_age_s: None,
        login_hint: None,
    };
    let (body, cookie) = begin_oauth(&state, &session.0, intent, invite_token_hash, params).await?;
    let mut response = json_ok(StatusCode::OK, &body);
    if let Some(cookie) = cookie {
        response.headers_mut().insert(header::SET_COOKIE, cookie.0);
    }
    Ok(response)
}

/// State rules per intent (S7 5.2 step 4).
fn validate_intent(intent: AuthIntent, state_kind: SessionState) -> Result<(), ApiError> {
    match intent {
        AuthIntent::Link | AuthIntent::Reconnect | AuthIntent::StepUp => {
            if state_kind != SessionState::Authenticated {
                return Err(ApiError::Unauthenticated);
            }
            Ok(())
        }
        AuthIntent::SignIn | AuthIntent::Join => match state_kind {
            SessionState::PreAuth | SessionState::PendingInviteRequest => Ok(()),
            SessionState::Authenticated => Err(invalid_request("intent")),
        },
    }
}

/// The invite token shape; its SHA-256 is what the store holds.
fn hash_invite_token(token: &str) -> Result<Sha256Hash, ApiError> {
    let len = token.len();
    let shaped = (INVITE_TOKEN_MIN..=INVITE_TOKEN_MAX).contains(&len)
        && token
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
    if !shaped {
        return Err(invalid_request("invite_token"));
    }
    let digest = Sha256::digest(token.as_bytes());
    let mut out = [0u8; 32];
    out.copy_from_slice(&digest);
    Ok(Sha256Hash(out))
}

fn invalid_request(field: &str) -> ApiError {
    ApiError::InvalidRequest {
        fields: vec![field.to_owned()],
    }
}
