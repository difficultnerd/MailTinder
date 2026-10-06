//! T-504 step-up integration tests (AU-01 AC5; ASVS V6.8.4, V7.2.4, V7.5.1,
//! V16.3.1).
//!
//! Everything runs through the real router with the scripted identity
//! provider, the in-memory store and keys, and the virtual clock.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::too_many_lines,
    clippy::similar_names,
    clippy::struct_excessive_bools,
    clippy::items_after_statements,
    clippy::needless_pass_by_value
)]

use std::sync::{Arc, Mutex};

use api::sealed::{SealedTokens, TokenType};
use api::session::extract::{SteppedUpAdmin, SteppedUpUser};
use api::session::store::{LoadedSession, SessionService};
use api::state::AppState;
use api::{app_state, build_router_with_routes, config::ApiConfig};
use async_trait::async_trait;
use axum::body::Body;
use axum::http::{header, HeaderMap, HeaderValue, Method, Request, StatusCode};
use axum::response::Response;
use axum::routing::{get, post};
use axum::Router;
use domain::{EmailAddress, MailboxId, MailboxStatus, Provider, ProviderSubjectId, UserId};
use obs::Sensitive;
use ports::store::aad_fields;
use ports::{
    Aad, AuthRequest, Ciphertext, Clock, IdClaims, IdError, IdentityProvider, KeyService,
    MailboxRecord, Precondition, Prompt, Rng, ServerStore, TokenSet, UserRecord,
};
use time::{Duration, OffsetDateTime};
use tower::ServiceExt;
use url::Url;

const ORIGIN: &str = "https://mailtinder.test";
const EMAIL_KEY: &[u8] = b"fake-email-key";

/// S7's step-up route list (section 4, `Auth` column `step-up`), as far as the
/// routes exist. Route tasks append their template when they land (T-505,
/// T-601a, T-601b, T-803, T-804, T-906b, T-908b).
const STEP_UP_ROUTES: &[(Method, &str)] = &[
    (Method::POST, "/api/v1/__t/sensitive"),
    (Method::POST, "/api/v1/admin/__t/write"),
];

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

/// A step-up start recorded by the spy identity provider.
#[derive(Default)]
struct Started {
    prompt: Option<Prompt>,
    max_age_s: Option<u32>,
    scopes: Vec<&'static str>,
    login_hint: Option<String>,
}

/// An identity provider that records each authorization request, then defers to
/// the scripted fake.
struct SpyIdentity {
    inner: Arc<dyn IdentityProvider>,
    seen: Mutex<Vec<Started>>,
}

impl SpyIdentity {
    fn requests(&self) -> Vec<Started> {
        let mut guard = self.seen.lock().unwrap_or_else(|_| panic!("spy poisoned"));
        std::mem::take(&mut *guard)
    }
}

#[async_trait]
impl IdentityProvider for SpyIdentity {
    fn authorize_url(&self, req: &AuthRequest) -> Url {
        self.seen
            .lock()
            .unwrap_or_else(|_| panic!("spy poisoned"))
            .push(Started {
                prompt: req.prompt,
                max_age_s: req.max_age_s,
                scopes: req.scopes.clone(),
                login_hint: req.login_hint.as_ref().map(|h| h.expose().clone()),
            });
        self.inner.authorize_url(req)
    }

    async fn exchange(
        &self,
        code: &str,
        verifier: &Sensitive<String>,
        redirect_uri: &Url,
    ) -> Result<TokenSet, IdError> {
        self.inner.exchange(code, verifier, redirect_uri).await
    }

    async fn validate_id_token(
        &self,
        raw: &Sensitive<String>,
        nonce: &str,
    ) -> Result<IdClaims, IdError> {
        self.inner.validate_id_token(raw, nonce).await
    }

    async fn refresh(&self, refresh: &Sensitive<String>) -> Result<Sensitive<String>, IdError> {
        self.inner.refresh(refresh).await
    }

    async fn revoke(&self, token: &Sensitive<String>) -> Result<(), IdError> {
        self.inner.revoke(token).await
    }
}

fn fixture_with_spy(
) -> Result<(AppState, testkit::Fakes, Arc<SpyIdentity>), Box<dyn std::error::Error>> {
    let (mut ports, fakes) = testkit::fake_ports();
    let spy = Arc::new(SpyIdentity {
        inner: Arc::clone(&fakes.identity) as Arc<dyn IdentityProvider>,
        seen: Mutex::new(Vec::new()),
    });
    ports.identity = Arc::clone(&spy) as Arc<dyn IdentityProvider>;
    Ok((app_state(Arc::new(ports), Arc::new(config()?)), fakes, spy))
}

async fn sensitive(_: SteppedUpUser) -> StatusCode {
    StatusCode::OK
}

async fn admin_write(_: SteppedUpAdmin) -> StatusCode {
    StatusCode::NO_CONTENT
}

fn test_router(state: AppState) -> Router {
    build_router_with_routes(state, |r| {
        r.route("/api/v1/__t/sensitive", get(sensitive).post(sensitive))
            .route("/api/v1/admin/__t/write", post(admin_write))
    })
}

fn cookie_value(header: &HeaderValue) -> Result<String, Box<dyn std::error::Error>> {
    Ok(header
        .to_str()?
        .split(';')
        .next()
        .unwrap_or_default()
        .to_owned())
}

fn set_cookie(resp: &Response) -> Option<String> {
    resp.headers()
        .get(header::SET_COOKIE)
        .and_then(|value| value.to_str().ok())
        .map(|value| value.split(';').next().unwrap_or_default().to_owned())
}

fn cookie_headers(value: &str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(
        header::COOKIE,
        HeaderValue::from_str(value).expect("cookie header"),
    );
    headers
}

async fn body_string(resp: Response) -> Result<String, Box<dyn std::error::Error>> {
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

/// The `outcome` value from a redirect `Location`.
fn outcome(resp: &Response) -> String {
    let location = resp
        .headers()
        .get(header::LOCATION)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    location
        .rsplit_once("outcome=")
        .map(|(_, value)| value.to_owned())
        .unwrap_or_default()
}

/// The `code` value from a problem-details body.
async fn code_of(resp: Response) -> Result<String, Box<dyn std::error::Error>> {
    let body: serde_json::Value = serde_json::from_str(&body_string(resp).await?)?;
    Ok(body["code"].as_str().unwrap_or_default().to_owned())
}

fn authorize_state(url: &str) -> Result<String, Box<dyn std::error::Error>> {
    let parsed = Url::parse(url)?;
    parsed
        .query_pairs()
        .find(|(key, _)| key == "state")
        .map(|(_, value)| value.into_owned())
        .ok_or_else(|| "no state in authorization url".into())
}

async fn load(
    state: &AppState,
    cookie: &str,
) -> Result<Option<LoadedSession>, Box<dyn std::error::Error>> {
    Ok(SessionService::new(state)
        .load(&cookie_headers(cookie))
        .await?)
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
    is_primary: bool,
) -> Result<MailboxId, Box<dyn std::error::Error>> {
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
                is_primary,
                refresh_token: None,
            },
            Precondition::MustNotExist,
        )
        .await?;
    Ok(mailbox_id)
}

struct Authed {
    cookie: String,
    csrf: String,
    user: UserId,
}

async fn authed(
    state: &AppState,
    fakes: &testkit::Fakes,
    is_admin: bool,
    recent_auth_at: Option<OffsetDateTime>,
) -> Result<Authed, Box<dyn std::error::Error>> {
    let user = seed_user(fakes, is_admin).await?;
    let (cookie, record) = SessionService::new(state)
        .establish(None, &user, recent_auth_at)
        .await?;
    Ok(Authed {
        cookie: cookie_value(&cookie.0)?,
        csrf: record.record.csrf_token.clone(),
        user,
    })
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
    csrf: &str,
    body: &str,
) -> Result<Response, Box<dyn std::error::Error>> {
    let req = Request::builder()
        .method(Method::POST)
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::ORIGIN, ORIGIN)
        .header(header::COOKIE, cookie)
        .header("x-csrf-token", csrf)
        .body(Body::from(body.to_owned()))?;
    Ok(router.clone().oneshot(req).await?)
}

fn step_up_body() -> &'static str {
    r#"{"intent":"step_up","invite_token":null,"mailbox_id":null}"#
}

/// Start a step-up round trip: the rotated cookie and the authorization URL.
async fn start_step_up(
    router: &Router,
    cookie: &str,
    csrf: &str,
) -> Result<(String, String), Box<dyn std::error::Error>> {
    let resp = authed_post(
        router,
        "/api/v1/auth/google/start",
        cookie,
        csrf,
        step_up_body(),
    )
    .await?;
    assert_eq!(resp.status(), StatusCode::OK);
    let rotated = set_cookie(&resp).ok_or("rotated cookie")?;
    let json: serde_json::Value = serde_json::from_str(&body_string(resp).await?)?;
    let url = json["authorization_url"]
        .as_str()
        .ok_or("authorization_url")?
        .to_owned();
    Ok((rotated, url))
}

fn tokens() -> TokenSet {
    TokenSet {
        access_token: Sensitive::new("access-token-A".to_owned()),
        refresh_token: Some(Sensitive::new("refresh-token-R".to_owned())),
        id_token: Sensitive::new("id-token-I".to_owned()),
        expires_in_s: 3600,
        granted_scopes: adapters_gmail::scopes::STEP_UP_SCOPES
            .iter()
            .map(|scope| (*scope).to_owned())
            .collect(),
    }
}

fn claims(sub: &str, email: &str, auth_time: Option<OffsetDateTime>) -> IdClaims {
    let issued_at = OffsetDateTime::from_unix_timestamp(1_700_000_000).expect("valid timestamp");
    IdClaims {
        sub: sub.to_owned(),
        email: Sensitive::new(email.to_owned()),
        email_verified: true,
        auth_time,
        amr: vec!["pwd".to_owned()],
        issued_at,
        expires_at: issued_at + Duration::hours(1),
    }
}

/// Drive the callback with scripted tokens and claims.
async fn callback(
    router: &Router,
    fakes: &testkit::Fakes,
    cookie: &str,
    oauth_state: &str,
    claims: IdClaims,
) -> Result<Response, Box<dyn std::error::Error>> {
    fakes.identity.script_exchange(Ok(tokens()));
    fakes.identity.script_claims(Ok(claims));
    call(
        router,
        Method::GET,
        &format!("/api/v1/auth/google/callback?code=code-A&state={oauth_state}"),
        &[("cookie", cookie)],
    )
    .await
}

/// A full successful step-up; returns the fresh cookie and CSRF token.
async fn complete_step_up(
    router: &Router,
    state: &AppState,
    fakes: &testkit::Fakes,
    a: &Authed,
    sub: &str,
    email: &str,
    auth_time: OffsetDateTime,
) -> Result<(String, String), Box<dyn std::error::Error>> {
    let (rotated, url) = start_step_up(router, &a.cookie, &a.csrf).await?;
    let oauth_state = authorize_state(&url)?;
    let resp = callback(
        router,
        fakes,
        &rotated,
        &oauth_state,
        claims(sub, email, Some(auth_time)),
    )
    .await?;
    assert_eq!(outcome(&resp), "stepped_up");
    let cookie = set_cookie(&resp).ok_or("new session cookie")?;
    let loaded = load(state, &cookie).await?.ok_or("rotated session")?;
    Ok((cookie, loaded.record.record.csrf_token.clone()))
}

// ---------------------------------------------------------------------------
// AU-01 AC5: an admin write without a fresh sign-in is refused.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn au_01_ac5_admin_write_without_step_up_refused() -> Result<(), Box<dyn std::error::Error>> {
    let (state, fakes) = fixture()?;
    let router = test_router(state.clone());
    let a = authed(&state, &fakes, true, None).await?;
    let resp = authed_post(&router, "/api/v1/admin/__t/write", &a.cookie, &a.csrf, "{}").await?;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    assert_eq!(code_of(resp).await?, "step_up_required");
    Ok(())
}

#[tokio::test]
async fn au_01_ac5_admin_write_after_step_up_allowed() -> Result<(), Box<dyn std::error::Error>> {
    let (state, fakes) = fixture()?;
    let router = test_router(state.clone());
    let a = authed(&state, &fakes, true, None).await?;
    seed_mailbox(&fakes, a.user, "sub-admin", "admin@example.com", true).await?;
    let now = fakes.clock.now();
    let (cookie, csrf) = complete_step_up(
        &router,
        &state,
        &fakes,
        &a,
        "sub-admin",
        "admin@example.com",
        now,
    )
    .await?;
    let resp = authed_post(&router, "/api/v1/admin/__t/write", &cookie, &csrf, "{}").await?;
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    Ok(())
}

// ---------------------------------------------------------------------------
// V6.8.4: the authorization URL asks Google to re-authenticate.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn asvs_v6_8_4_authorize_url_has_prompt_login_and_max_age_300(
) -> Result<(), Box<dyn std::error::Error>> {
    let (state, fakes, spy) = fixture_with_spy()?;
    let router = test_router(state.clone());
    let a = authed(&state, &fakes, false, None).await?;
    seed_mailbox(&fakes, a.user, "sub-hint", "primary@example.com", true).await?;
    let (_rotated, _url) = start_step_up(&router, &a.cookie, &a.csrf).await?;
    let requests = spy.requests();
    assert_eq!(requests.len(), 1, "one authorization request");
    let started = requests.first().ok_or("request")?;
    assert_eq!(started.prompt, Some(Prompt::Login));
    assert_eq!(started.max_age_s, Some(300));
    assert_eq!(
        started.scopes,
        vec!["openid", "email"],
        "no new scopes are requested"
    );
    assert_eq!(
        started.login_hint.as_deref(),
        Some("primary@example.com"),
        "the hint is the primary mailbox"
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// V6.8.4: a missing or stale `auth_time` is refused.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn asvs_v6_8_4_missing_auth_time_refused() -> Result<(), Box<dyn std::error::Error>> {
    let (state, fakes) = fixture()?;
    let router = test_router(state.clone());
    let a = authed(&state, &fakes, false, None).await?;
    seed_mailbox(&fakes, a.user, "sub-na", "na@example.com", true).await?;
    let (rotated, url) = start_step_up(&router, &a.cookie, &a.csrf).await?;
    let oauth_state = authorize_state(&url)?;
    let resp = callback(
        &router,
        &fakes,
        &rotated,
        &oauth_state,
        claims("sub-na", "na@example.com", None),
    )
    .await?;
    assert_eq!(outcome(&resp), "failed");
    let loaded = load(&state, &rotated).await?.ok_or("session")?;
    assert_eq!(
        loaded.record.record.recent_auth_at, None,
        "nothing changed on a refused step-up"
    );
    Ok(())
}

#[tokio::test]
async fn asvs_v6_8_4_stale_auth_time_refused_even_if_token_fresh(
) -> Result<(), Box<dyn std::error::Error>> {
    let (state, fakes) = fixture()?;
    let router = test_router(state.clone());
    let a = authed(&state, &fakes, false, None).await?;
    seed_mailbox(&fakes, a.user, "sub-stale", "stale@example.com", true).await?;
    let (rotated, url) = start_step_up(&router, &a.cookie, &a.csrf).await?;
    let oauth_state = authorize_state(&url)?;
    // A silently re-issued ID token: fresh token, old `auth_time`.
    let stale = fakes.clock.now() - Duration::seconds(301);
    let resp = callback(
        &router,
        &fakes,
        &rotated,
        &oauth_state,
        claims("sub-stale", "stale@example.com", Some(stale)),
    )
    .await?;
    assert_eq!(outcome(&resp), "failed");
    assert_eq!(set_cookie(&resp), None, "no session change");
    Ok(())
}

// ---------------------------------------------------------------------------
// V7.2.4: the ID rotates at step-up; `session_record_id` is kept.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn asvs_v7_2_4_step_up_rotates_id_keeps_record_id() -> Result<(), Box<dyn std::error::Error>>
{
    let (state, fakes) = fixture()?;
    let router = test_router(state.clone());
    let a = authed(&state, &fakes, false, None).await?;
    seed_mailbox(&fakes, a.user, "sub-rot", "rot@example.com", true).await?;
    let before = load(&state, &a.cookie).await?.ok_or("session")?;
    let record_id = before.record.record.session_record_id;
    // A sealed undo token bound to the session record ID (T-303).
    let sealed = SealedTokens::new(
        Arc::clone(&fakes.keys) as Arc<dyn KeyService>,
        Arc::clone(&fakes.clock) as Arc<dyn Clock>,
    );
    let user_record = fakes.store.users().get(&a.user).await?.ok_or("user")?;
    let token = sealed
        .seal(
            TokenType::Undo,
            &a.user,
            &user_record.record.wrapped_data_key,
            &record_id,
            fakes.clock.now() + Duration::hours(1),
            &"undo-1".to_owned(),
        )
        .await?;
    let (new_cookie, _csrf) = complete_step_up(
        &router,
        &state,
        &fakes,
        &a,
        "sub-rot",
        "rot@example.com",
        fakes.clock.now(),
    )
    .await?;
    assert_ne!(new_cookie, a.cookie, "the cookie value rotates");
    let after = load(&state, &new_cookie).await?.ok_or("new session")?;
    assert_eq!(
        after.record.record.session_record_id, record_id,
        "session_record_id is kept"
    );
    let opened: String = sealed
        .open(
            TokenType::Undo,
            &a.user,
            &user_record.record.wrapped_data_key,
            &after.record.record.session_record_id,
            &token,
        )
        .await?;
    assert_eq!(opened, "undo-1");
    // The old cookie is dead.
    assert!(load(&state, &a.cookie).await?.is_none(), "old ID is dead");
    let old = call(
        &router,
        Method::GET,
        "/api/v1/__t/sensitive",
        &[("cookie", &a.cookie)],
    )
    .await?;
    assert_eq!(old.status(), StatusCode::UNAUTHORIZED);
    Ok(())
}

// ---------------------------------------------------------------------------
// V7.5.1: the window boundary is exactly 300 s.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn asvs_v7_5_1_window_boundary_300_ok_301_refused() -> Result<(), Box<dyn std::error::Error>>
{
    let (state, fakes) = fixture()?;
    let router = test_router(state.clone());
    let a = authed(
        &state,
        &fakes,
        false,
        Some(fakes.clock.now() - Duration::seconds(300)),
    )
    .await?;
    let ok = call(
        &router,
        Method::GET,
        "/api/v1/__t/sensitive",
        &[("cookie", &a.cookie)],
    )
    .await?;
    assert_eq!(ok.status(), StatusCode::OK, "300 s is still inside");
    fakes.clock.advance(Duration::seconds(1));
    let refused = call(
        &router,
        Method::GET,
        "/api/v1/__t/sensitive",
        &[("cookie", &a.cookie)],
    )
    .await?;
    assert_eq!(refused.status(), StatusCode::FORBIDDEN);
    assert_eq!(code_of(refused).await?, "step_up_required");
    Ok(())
}

// ---------------------------------------------------------------------------
// V7.5.1: only one of the user's own linked accounts is accepted.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn asvs_v7_5_1_wrong_google_account_refused_nothing_changed(
) -> Result<(), Box<dyn std::error::Error>> {
    let (state, fakes) = fixture()?;
    let router = test_router(state.clone());
    let a = authed(&state, &fakes, false, None).await?;
    seed_mailbox(&fakes, a.user, "sub-mine", "mine@example.com", true).await?;
    let (rotated, url) = start_step_up(&router, &a.cookie, &a.csrf).await?;
    let oauth_state = authorize_state(&url)?;
    let resp = callback(
        &router,
        &fakes,
        &rotated,
        &oauth_state,
        claims(
            "sub-stranger",
            "stranger@example.com",
            Some(fakes.clock.now()),
        ),
    )
    .await?;
    assert_eq!(outcome(&resp), "step_up_wrong_account");
    let loaded = load(&state, &rotated).await?.ok_or("session")?;
    assert_eq!(
        loaded.record.record.recent_auth_at, None,
        "recent_auth_at is untouched"
    );
    Ok(())
}

#[tokio::test]
async fn asvs_v7_5_1_account_linked_to_other_user_refused() -> Result<(), Box<dyn std::error::Error>>
{
    let (state, fakes) = fixture()?;
    let router = test_router(state.clone());
    let a = authed(&state, &fakes, false, None).await?;
    seed_mailbox(&fakes, a.user, "sub-mine", "mine@example.com", true).await?;
    let other = seed_user(&fakes, false).await?;
    seed_mailbox(&fakes, other, "sub-other", "other@example.com", true).await?;
    let (rotated, url) = start_step_up(&router, &a.cookie, &a.csrf).await?;
    let oauth_state = authorize_state(&url)?;
    let resp = callback(
        &router,
        &fakes,
        &rotated,
        &oauth_state,
        claims("sub-other", "other@example.com", Some(fakes.clock.now())),
    )
    .await?;
    assert_eq!(outcome(&resp), "step_up_wrong_account");
    Ok(())
}

// ---------------------------------------------------------------------------
// V16.3.1: step-up successes and failures are logged.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn asvs_v16_3_1_step_up_success_and_failure_logged() -> Result<(), Box<dyn std::error::Error>>
{
    let (state, fakes) = fixture()?;
    let router = test_router(state.clone());
    let a = authed(&state, &fakes, false, None).await?;
    seed_mailbox(&fakes, a.user, "sub-log", "log@example.com", true).await?;
    let clock = obs::arc(obs::FixedClock(fakes.clock.now()));
    let (capture, _guard) = obs::capture("api", clock);

    complete_step_up(
        &router,
        &state,
        &fakes,
        &a,
        "sub-log",
        "log@example.com",
        fakes.clock.now(),
    )
    .await?;

    let b = authed(&state, &fakes, false, None).await?;
    seed_mailbox(&fakes, b.user, "sub-fail", "fail@example.com", true).await?;
    let (rotated, url) = start_step_up(&router, &b.cookie, &b.csrf).await?;
    let oauth_state = authorize_state(&url)?;
    let stale = fakes.clock.now() - Duration::seconds(400);
    let resp = callback(
        &router,
        &fakes,
        &rotated,
        &oauth_state,
        claims("sub-fail", "fail@example.com", Some(stale)),
    )
    .await?;
    assert_eq!(outcome(&resp), "failed");

    let text = capture.text();
    assert!(
        text.contains("\"action\":\"step_up\""),
        "step_up logged: {text}"
    );
    assert!(
        text.contains("\"outcome\":\"success\""),
        "success logged: {text}"
    );
    assert!(
        text.contains("\"outcome\":\"stale_auth_time\""),
        "failure logged: {text}"
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// The admin check runs before the step-up check.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn step_up_non_admin_gets_forbidden_not_step_up_required(
) -> Result<(), Box<dyn std::error::Error>> {
    let (state, fakes) = fixture()?;
    let router = test_router(state.clone());
    let a = authed(&state, &fakes, false, None).await?;
    let resp = authed_post(&router, "/api/v1/admin/__t/write", &a.cookie, &a.csrf, "{}").await?;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    assert_eq!(code_of(resp).await?, "forbidden");
    Ok(())
}

// ---------------------------------------------------------------------------
// A step-up grant is used only for its ID token: nothing is revoked.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn step_up_tokens_never_revoked() -> Result<(), Box<dyn std::error::Error>> {
    let (state, fakes) = fixture()?;
    let router = test_router(state.clone());
    let a = authed(&state, &fakes, false, None).await?;
    seed_mailbox(&fakes, a.user, "sub-keep", "keep@example.com", true).await?;
    complete_step_up(
        &router,
        &state,
        &fakes,
        &a,
        "sub-keep",
        "keep@example.com",
        fakes.clock.now(),
    )
    .await?;
    assert!(
        fakes.identity.revoked().is_empty(),
        "a linked subject's grant is never revoked"
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// Every route on S7's step-up list rejects a session without step-up.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn step_up_route_list_matches_s7() -> Result<(), Box<dyn std::error::Error>> {
    let (state, fakes) = fixture()?;
    let router = test_router(state.clone());
    let a = authed(&state, &fakes, true, None).await?;
    for (method, path) in STEP_UP_ROUTES {
        let resp = if *method == Method::POST {
            authed_post(&router, path, &a.cookie, &a.csrf, "{}").await?
        } else {
            call(&router, method.clone(), path, &[("cookie", &a.cookie)]).await?
        };
        assert_eq!(resp.status(), StatusCode::FORBIDDEN, "{path} needs step-up");
        assert_eq!(code_of(resp).await?, "step_up_required", "{path}");
    }
    Ok(())
}
