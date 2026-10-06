//! T-506 session endpoint and sign-out integration tests (AU-07 AC1, AC2, AC6;
//! SES-1; ASVS V7.4.1, V14.3.1).
//!
//! Everything runs through the real router with the fakes and the virtual clock.

#![allow(clippy::too_many_lines, clippy::items_after_statements)]

use std::sync::Arc;

use api::session::cookie::RawSessionId;
use api::session::extract::AuthedSession;
use api::session::pre_auth::{seal_pre_auth, PreAuthPlain};
use api::session::store::{LoadedSession, SessionService};
use api::state::AppState;
use api::{app_state, build_router_with_routes, config::ApiConfig};
use axum::body::Body;
use axum::http::{header, HeaderValue, Method, Request, StatusCode};
use axum::response::Response;
use axum::routing::get;
use axum::Router;
use domain::{EmailAddress, MailboxStatus, Provider, ProviderSubjectId, UserId};
use obs::Sensitive;
use ports::store::aad_fields;
use ports::{
    Aad, AuthIntent, Ciphertext, Clock, KeyService, MailboxRecord, Precondition, Rng, ServerStore,
    SessionState, UserRecord,
};
use time::{Duration, OffsetDateTime};
use tower::ServiceExt;

const ORIGIN: &str = "https://mailtinder.test";
const EMAIL_KEY: &[u8] = b"fake-email-key";

fn config() -> Result<ApiConfig, Box<dyn std::error::Error>> {
    Ok(ApiConfig::new(
        ORIGIN.to_owned(),
        "fake-client".into(),
        Sensitive::new(b"fake-log-key".to_vec()),
        Sensitive::new(EMAIL_KEY.to_vec()),
    )?)
}

fn fixture() -> Result<(AppState, testkit::Fakes), Box<dyn std::error::Error>> {
    let (ports, fakes) = testkit::fake_ports();
    Ok((app_state(Arc::new(ports), Arc::new(config()?)), fakes))
}

async fn me(_: AuthedSession) -> StatusCode {
    StatusCode::OK
}

fn test_router(state: AppState) -> Router {
    build_router_with_routes(state, |r| r.route("/api/v1/__t/me", get(me)))
}

fn cookie_value(header: &HeaderValue) -> Result<String, Box<dyn std::error::Error>> {
    Ok(header
        .to_str()?
        .split(';')
        .next()
        .unwrap_or_default()
        .to_owned())
}

/// The full `Set-Cookie` header value (attributes included).
fn set_cookie_full(resp: &Response) -> Option<String> {
    resp.headers()
        .get(header::SET_COOKIE)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned)
}

fn raw_from_cookie(value: &HeaderValue) -> Result<RawSessionId, Box<dyn std::error::Error>> {
    let text = value.to_str()?;
    let pair = text.split(';').next().ok_or("cookie pair")?;
    let (_, raw) = pair.split_once('=').ok_or("cookie value")?;
    Ok(RawSessionId::parse(raw).ok_or("43-char raw session id")?)
}

async fn body_string(resp: Response) -> Result<String, Box<dyn std::error::Error>> {
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

async fn json_of(resp: Response) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
    Ok(serde_json::from_str(&body_string(resp).await?)?)
}

async fn code_of(resp: Response) -> Result<String, Box<dyn std::error::Error>> {
    let body: serde_json::Value = serde_json::from_str(&body_string(resp).await?)?;
    Ok(body["code"].as_str().unwrap_or_default().to_owned())
}

async fn call(
    router: &Router,
    method: Method,
    uri: &str,
    headers: &[(&str, &str)],
) -> Result<Response, Box<dyn std::error::Error>> {
    let mut req = Request::builder().method(method).uri(uri);
    for (name, value) in headers {
        req = req.header(*name, *value);
    }
    Ok(router.clone().oneshot(req.body(Body::empty())?).await?)
}

/// A CSRF-protected POST with the session cookie.
async fn authed_post(
    router: &Router,
    uri: &str,
    cookie: &str,
    csrf: Option<&str>,
) -> Result<Response, Box<dyn std::error::Error>> {
    let mut req = Request::builder()
        .method(Method::POST)
        .uri(uri)
        .header(header::ORIGIN, ORIGIN)
        .header(header::COOKIE, cookie);
    if let Some(csrf) = csrf {
        req = req.header("x-csrf-token", csrf);
    }
    Ok(router.clone().oneshot(req.body(Body::empty())?).await?)
}

async fn seed_user(
    fakes: &testkit::Fakes,
    is_admin: bool,
) -> Result<UserId, Box<dyn std::error::Error>> {
    let user = UserId::new(fakes.rng.uuid_v4());
    let wrapped = fakes.keys.new_user_key(&user).await?;
    fakes
        .store
        .users()
        .put(
            &UserRecord {
                user_id: user,
                created_at: fakes.clock.now(),
                is_admin,
                wrapped_data_key: wrapped,
                experiments_consent_version: None,
                experiments_opted_in_at: None,
            },
            Precondition::MustNotExist,
        )
        .await?;
    Ok(user)
}

async fn seed_mailbox(
    fakes: &testkit::Fakes,
    user: UserId,
    sub: &str,
    email: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let subject = ProviderSubjectId::new(sub)?;
    let address = EmailAddress::parse(email)?;
    let mailbox_id = ports::mailbox_id_for(Provider::Gmail, &subject);
    let user_record = fakes.store.users().get(&user).await?.ok_or("seeded user")?;
    let aad = Aad {
        user,
        scope: mailbox_id.0.to_string(),
        field: aad_fields::MAILBOX_EMAIL,
    };
    let sealed = fakes
        .keys
        .seal(
            &user,
            &user_record.record.wrapped_data_key,
            &aad,
            address.as_str().as_bytes(),
        )
        .await?;
    fakes
        .store
        .mailboxes()
        .put(
            &MailboxRecord {
                mailbox_id,
                user_id: user,
                provider: Provider::Gmail,
                provider_subject_id: subject,
                email_address: Ciphertext(sealed),
                status: MailboxStatus::Connected,
                linked_at: fakes.clock.now(),
                is_primary: true,
                refresh_token: None,
            },
            Precondition::MustNotExist,
        )
        .await?;
    Ok(())
}

struct Authed {
    cookie: String,
    csrf: String,
    user: UserId,
}

async fn authed(
    state: &AppState,
    fakes: &testkit::Fakes,
    recent_auth_at: Option<OffsetDateTime>,
) -> Result<Authed, Box<dyn std::error::Error>> {
    let user = seed_user(fakes, false).await?;
    let (cookie, record) = SessionService::new(state)
        .establish(None, &user, recent_auth_at)
        .await?;
    Ok(Authed {
        cookie: cookie_value(&cookie.0)?,
        csrf: record.record.csrf_token.clone(),
        user,
    })
}

/// A `pending_invite_request` session carrying `email`.
async fn pending(
    state: &AppState,
    email: &str,
) -> Result<(String, String), Box<dyn std::error::Error>> {
    let service = SessionService::new(state);
    let (cookie, versioned) = service.create_anonymous().await?;
    let raw = raw_from_cookie(&cookie.0)?;
    let loaded = LoadedSession {
        raw,
        record: versioned,
    };
    let record_id = loaded.record.record.session_record_id;
    let plain = PreAuthPlain {
        intent: AuthIntent::Join,
        oauth_state: Sensitive::new(String::new()),
        nonce: Sensitive::new(String::new()),
        pkce_verifier: Sensitive::new(String::new()),
        invite_token_hash: None,
        pending_email: Some(Sensitive::new(email.to_owned())),
        mailbox_id: None,
        started_at: state.ports.clock.now(),
    };
    let sealed = seal_pre_auth(state.ports.system_keys.as_ref(), &record_id, &plain).await?;
    let (new_cookie, versioned) = service
        .rotate(&loaded, |r| {
            r.state = SessionState::PendingInviteRequest;
            r.pre_auth = Some(sealed);
        })
        .await?;
    Ok((
        cookie_value(&new_cookie.0)?,
        versioned.record.csrf_token.clone(),
    ))
}

fn rfc3339_parse(value: &str) -> Result<OffsetDateTime, Box<dyn std::error::Error>> {
    Ok(OffsetDateTime::parse(
        value,
        &time::format_description::well_known::Rfc3339,
    )?)
}

// ---------------------------------------------------------------------------
// API-AUTH-3: never 401; creates an anonymous session when there is none.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn session_endpoint_never_401_and_creates_anonymous() -> Result<(), Box<dyn std::error::Error>>
{
    let (state, fakes) = fixture()?;
    let router = test_router(state.clone());

    // No cookie at all.
    let resp = call(&router, Method::GET, "/api/v1/session", &[]).await?;
    assert_eq!(resp.status(), StatusCode::OK);
    let cookie = set_cookie_full(&resp).ok_or("anonymous session cookie")?;
    let json = json_of(resp).await?;
    assert_eq!(json["state"], "anonymous");
    assert!(
        json["csrf_token"].as_str().is_some_and(|s| !s.is_empty()),
        "a CSRF token is always present"
    );
    assert!(json["user"].is_null());
    assert!(json["mailboxes"].as_array().is_some_and(Vec::is_empty));

    // A client-chosen value finds no record and is never adopted.
    let garbage = format!("__session={}", "A".repeat(43));
    let resp = call(
        &router,
        Method::GET,
        "/api/v1/session",
        &[("cookie", &garbage)],
    )
    .await?;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(json_of(resp).await?["state"], "anonymous");

    // An expired `pre_auth` cookie (10-minute TTL) is also anonymous.
    fakes.clock.advance(Duration::minutes(11));
    let pair = cookie.split(';').next().ok_or("cookie pair")?;
    let resp = call(&router, Method::GET, "/api/v1/session", &[("cookie", pair)]).await?;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(json_of(resp).await?["state"], "anonymous");
    Ok(())
}

// ---------------------------------------------------------------------------
// API-AUTH-3: the authenticated body matches the OpenAPI `Session` schema.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn session_endpoint_authenticated_shape_matches_openapi(
) -> Result<(), Box<dyn std::error::Error>> {
    let (state, fakes) = fixture()?;
    let router = test_router(state.clone());
    let a = authed(&state, &fakes, None).await?;
    seed_mailbox(&fakes, a.user, "sub-shape", "shape@example.com").await?;

    let resp = call(
        &router,
        Method::GET,
        "/api/v1/session",
        &[("cookie", &a.cookie)],
    )
    .await?;
    assert_eq!(resp.status(), StatusCode::OK);
    let json = json_of(resp).await?;

    assert_eq!(json["state"], "authenticated");
    assert_eq!(json["csrf_token"], a.csrf);
    assert!(json["pending_invite_email"].is_null());
    assert!(json["step_up_valid_until"].is_null());
    assert_eq!(json["user"]["user_id"], a.user.0.to_string());
    assert_eq!(json["user"]["is_admin"], false);

    let mailboxes = json["mailboxes"].as_array().ok_or("mailboxes array")?;
    assert_eq!(mailboxes.len(), 1);
    let mailbox = &mailboxes[0];
    assert!(mailbox["mailbox_id"].is_string());
    assert_eq!(mailbox["provider"], "gmail");
    assert_eq!(mailbox["email_address"], "shape@example.com");
    assert_eq!(mailbox["status"], "connected");
    assert_eq!(mailbox["is_primary"], true);
    assert!(mailbox["linked_at"].is_string());

    assert!(json["idle_expires_at"].is_string());
    assert!(json["absolute_expires_at"].is_string());
    let absolute = rfc3339_parse(json["absolute_expires_at"].as_str().ok_or("absolute")?)?;
    let idle = rfc3339_parse(json["idle_expires_at"].as_str().ok_or("idle")?)?;
    assert!(
        idle <= absolute,
        "the idle deadline never exceeds the absolute one"
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// API-AUTH-3: `step_up_valid_until` is null once the window has passed.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn session_endpoint_step_up_valid_until_null_after_window(
) -> Result<(), Box<dyn std::error::Error>> {
    let (state, fakes) = fixture()?;
    let router = test_router(state.clone());
    let now = fakes.clock.now();
    let a = authed(&state, &fakes, Some(now)).await?;

    let resp = call(
        &router,
        Method::GET,
        "/api/v1/session",
        &[("cookie", &a.cookie)],
    )
    .await?;
    let json = json_of(resp).await?;
    let until = rfc3339_parse(json["step_up_valid_until"].as_str().ok_or("step_up")?)?;
    assert_eq!(until, now + Duration::seconds(300));

    fakes.clock.advance(Duration::seconds(301));
    let resp = call(
        &router,
        Method::GET,
        "/api/v1/session",
        &[("cookie", &a.cookie)],
    )
    .await?;
    assert!(
        json_of(resp).await?["step_up_valid_until"].is_null(),
        "the window has passed"
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// API-AUTH-3: `pending_invite_email` appears only in that state.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn session_endpoint_pending_shows_pending_email_only_in_that_state(
) -> Result<(), Box<dyn std::error::Error>> {
    let (state, fakes) = fixture()?;
    let router = test_router(state.clone());
    let (cookie, _csrf) = pending(&state, "pending@example.com").await?;

    let resp = call(
        &router,
        Method::GET,
        "/api/v1/session",
        &[("cookie", &cookie)],
    )
    .await?;
    assert_eq!(resp.status(), StatusCode::OK);
    let json = json_of(resp).await?;
    assert_eq!(json["state"], "pending_invite_request");
    assert_eq!(json["pending_invite_email"], "pending@example.com");
    assert!(json["user"].is_null());
    assert!(json["mailboxes"].as_array().is_some_and(Vec::is_empty));
    assert!(json["idle_expires_at"].is_null());
    assert!(json["absolute_expires_at"].is_null());

    let a = authed(&state, &fakes, None).await?;
    let resp = call(
        &router,
        Method::GET,
        "/api/v1/session",
        &[("cookie", &a.cookie)],
    )
    .await?;
    let json = json_of(resp).await?;
    assert!(json["pending_invite_email"].is_null());
    Ok(())
}

// ---------------------------------------------------------------------------
// AU-07 AC1: a new sign-in ends the old session; the old browser is anonymous.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn au_07_ac1_old_session_reports_anonymous_after_new_sign_in(
) -> Result<(), Box<dyn std::error::Error>> {
    let (state, fakes) = fixture()?;
    let router = test_router(state.clone());
    let old = authed(&state, &fakes, None).await?;

    // A second sign-in for the same user replaces the first.
    let (new_cookie, _record) = SessionService::new(&state)
        .establish(None, &old.user, None)
        .await?;
    let new_cookie = cookie_value(&new_cookie.0)?;

    let resp = call(
        &router,
        Method::GET,
        "/api/v1/session",
        &[("cookie", &old.cookie)],
    )
    .await?;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(json_of(resp).await?["state"], "anonymous");

    let resp = call(
        &router,
        Method::GET,
        "/api/v1/__t/me",
        &[("cookie", &old.cookie)],
    )
    .await?;
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

    let resp = call(
        &router,
        Method::GET,
        "/api/v1/__t/me",
        &[("cookie", &new_cookie)],
    )
    .await?;
    assert_eq!(resp.status(), StatusCode::OK, "the new session still works");
    Ok(())
}

// ---------------------------------------------------------------------------
// AU-07 AC2 / V14.3.1: sign-out deletes the record and clears client data.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn au_07_ac2_sign_out_deletes_record_and_clears_site_data(
) -> Result<(), Box<dyn std::error::Error>> {
    let (state, fakes) = fixture()?;
    let router = test_router(state.clone());
    let a = authed(&state, &fakes, None).await?;

    let resp = authed_post(&router, "/api/v1/auth/sign-out", &a.cookie, Some(&a.csrf)).await?;
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    assert_eq!(
        set_cookie_full(&resp).as_deref(),
        Some("__session=; Path=/; Secure; HttpOnly; SameSite=Lax; Max-Age=0"),
        "the cookie is cleared with Max-Age=0"
    );
    assert_eq!(
        resp.headers()
            .get("clear-site-data")
            .and_then(|v| v.to_str().ok()),
        Some("\"cache\", \"storage\""),
        "exactly the cache and storage values"
    );
    Ok(())
}

#[tokio::test]
async fn asvs_v14_3_1_clear_site_data_cache_and_storage() -> Result<(), Box<dyn std::error::Error>>
{
    let (state, fakes) = fixture()?;
    let router = test_router(state.clone());
    let a = authed(&state, &fakes, None).await?;
    let resp = authed_post(&router, "/api/v1/auth/sign-out", &a.cookie, Some(&a.csrf)).await?;
    let value = resp
        .headers()
        .get("clear-site-data")
        .and_then(|v| v.to_str().ok())
        .ok_or("Clear-Site-Data header")?;
    assert_eq!(value, "\"cache\", \"storage\"");
    assert_ne!(value, "\"cookies\"", "never clears cookies mid-request");
    Ok(())
}

// ---------------------------------------------------------------------------
// V7.4.1 / SES-1: the server record is gone and the cookie cannot be reused.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn asvs_v7_4_1_sign_out_removes_server_record() -> Result<(), Box<dyn std::error::Error>> {
    let (state, fakes) = fixture()?;
    let router = test_router(state.clone());
    let a = authed(&state, &fakes, None).await?;
    authed_post(&router, "/api/v1/auth/sign-out", &a.cookie, Some(&a.csrf)).await?;
    let sessions = fakes.store.sessions().by_user(&a.user).await?;
    assert!(sessions.is_empty(), "no session remains for the user");
    Ok(())
}

#[tokio::test]
async fn ses_1_signed_out_cookie_refused_on_user_route() -> Result<(), Box<dyn std::error::Error>> {
    let (state, fakes) = fixture()?;
    let router = test_router(state.clone());
    let a = authed(&state, &fakes, None).await?;
    authed_post(&router, "/api/v1/auth/sign-out", &a.cookie, Some(&a.csrf)).await?;
    let resp = call(
        &router,
        Method::GET,
        "/api/v1/__t/me",
        &[("cookie", &a.cookie)],
    )
    .await?;
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    Ok(())
}

// ---------------------------------------------------------------------------
// AU-07 AC6: sign-out is recorded as a security event.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn au_07_ac6_sign_out_logged() -> Result<(), Box<dyn std::error::Error>> {
    let (state, fakes) = fixture()?;
    let router = test_router(state.clone());
    let a = authed(&state, &fakes, None).await?;
    let clock = obs::arc(obs::FixedClock(fakes.clock.now()));
    let (capture, _guard) = obs::capture("api", clock);

    authed_post(&router, "/api/v1/auth/sign-out", &a.cookie, Some(&a.csrf)).await?;

    let text = capture.text();
    assert!(
        text.contains("\"action\":\"session_end\""),
        "session_end logged: {text}"
    );
    assert!(
        text.contains("\"outcome\":\"signed_out\""),
        "signed_out logged: {text}"
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// CSRF: sign-out without a token is refused.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn sign_out_without_csrf_token_refused() -> Result<(), Box<dyn std::error::Error>> {
    let (state, fakes) = fixture()?;
    let router = test_router(state.clone());
    let a = authed(&state, &fakes, None).await?;
    let resp = authed_post(&router, "/api/v1/auth/sign-out", &a.cookie, None).await?;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    assert_eq!(code_of(resp).await?, "csrf_failed");
    Ok(())
}
