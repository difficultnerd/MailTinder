//! T-601a mailbox list, link and reconnect integration tests (AU-04 AC1-AC6,
//! AU-03 AC7, ST-03 AC1, INV-3, ASVS V8.2.2).
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

use api::session::store::SessionService;
use api::state::AppState;
use api::{app_state, build_router, config::ApiConfig};
use async_trait::async_trait;
use axum::body::Body;
use axum::http::{header, HeaderMap, HeaderValue, Method, Request, StatusCode};
use axum::response::Response;
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

/// A start request recorded by the spy identity provider.
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

/// Build (but do not store) a mailbox record for a user's Google `sub`.
async fn mailbox_record(
    fakes: &testkit::Fakes,
    user: UserId,
    sub: &str,
    email: &str,
    is_primary: bool,
    status: MailboxStatus,
) -> Result<MailboxRecord, Box<dyn std::error::Error>> {
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
    Ok(MailboxRecord {
        mailbox_id,
        user_id: user,
        provider: Provider::Gmail,
        provider_subject_id: subject,
        email_address: Ciphertext(sealed),
        status,
        linked_at: fakes.clock.now(),
        is_primary,
        refresh_token: None,
    })
}

async fn seed_mailbox(
    fakes: &testkit::Fakes,
    user: UserId,
    sub: &str,
    email: &str,
    is_primary: bool,
    status: MailboxStatus,
) -> Result<MailboxId, Box<dyn std::error::Error>> {
    let record = mailbox_record(fakes, user, sub, email, is_primary, status).await?;
    let mailbox_id = record.mailbox_id;
    fakes
        .store
        .mailboxes()
        .put(&record, Precondition::MustNotExist)
        .await?;
    Ok(mailbox_id)
}

fn count(fakes: &testkit::Fakes, collection: &str) -> usize {
    fakes
        .store
        .export_json()
        .iter()
        .filter(|(name, _)| *name == collection)
        .count()
}

struct Authed {
    cookie: String,
    csrf: String,
    user: UserId,
}

/// An authenticated session with a fresh sign-in inside the step-up window.
async fn authed(
    state: &AppState,
    fakes: &testkit::Fakes,
) -> Result<Authed, Box<dyn std::error::Error>> {
    let user = seed_user(fakes, false).await?;
    let (cookie, record) = SessionService::new(state)
        .establish(None, &user, Some(fakes.clock.now() - Duration::seconds(10)))
        .await?;
    Ok(Authed {
        cookie: cookie_value(&cookie.0)?,
        csrf: record.record.csrf_token.clone(),
        user,
    })
}

fn cookie_value(value: &HeaderValue) -> Result<String, Box<dyn std::error::Error>> {
    Ok(value
        .to_str()?
        .split(';')
        .next()
        .unwrap_or_default()
        .to_owned())
}

fn tokens() -> TokenSet {
    TokenSet {
        access_token: Sensitive::new("access-token-A".to_owned()),
        refresh_token: Some(Sensitive::new("refresh-token-R".to_owned())),
        id_token: Sensitive::new("id-token-I".to_owned()),
        expires_in_s: 3600,
        granted_scopes: adapters_gmail::scopes::GMAIL_SCOPES
            .iter()
            .map(|scope| (*scope).to_owned())
            .collect(),
    }
}

fn claims(sub: &str, email: &str) -> IdClaims {
    let issued_at = OffsetDateTime::from_unix_timestamp(1_700_000_000).expect("valid timestamp");
    IdClaims {
        sub: sub.to_owned(),
        email: Sensitive::new(email.to_owned()),
        email_verified: true,
        auth_time: Some(issued_at),
        amr: vec!["pwd".to_owned()],
        issued_at,
        expires_at: issued_at + Duration::hours(1),
    }
}

fn link_body() -> &'static str {
    r#"{"intent":"link","invite_token":null,"mailbox_id":null}"#
}

fn reconnect_body(mailbox_id: &MailboxId) -> String {
    format!(
        r#"{{"intent":"reconnect","invite_token":null,"mailbox_id":"{}"}}"#,
        mailbox_id.0
    )
}

/// Start a round trip: the rotated cookie and the OAuth `state`.
async fn begin(
    router: &Router,
    cookie: &str,
    csrf: &str,
    body: &str,
) -> Result<(String, String), Box<dyn std::error::Error>> {
    let resp = authed_post(router, "/api/v1/auth/google/start", cookie, csrf, body).await?;
    assert_eq!(resp.status(), StatusCode::OK);
    let rotated = set_cookie(&resp).ok_or("rotated cookie")?;
    let json: serde_json::Value = serde_json::from_str(&body_string(resp).await?)?;
    let url = json["authorization_url"]
        .as_str()
        .ok_or("authorization_url")?
        .to_owned();
    Ok((rotated, authorize_state(&url)?))
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

/// A whole successful `link`, returning the new cookie and outcome.
async fn link(
    router: &Router,
    fakes: &testkit::Fakes,
    a: &Authed,
    sub: &str,
    email: &str,
) -> Result<(String, String), Box<dyn std::error::Error>> {
    let (rotated, oauth_state) = begin(router, &a.cookie, &a.csrf, link_body()).await?;
    let resp = callback(router, fakes, &rotated, &oauth_state, claims(sub, email)).await?;
    let new_cookie = set_cookie(&resp).unwrap_or(rotated);
    Ok((new_cookie, outcome(&resp)))
}

async fn mailboxes_json(
    router: &Router,
    cookie: &str,
) -> Result<(StatusCode, serde_json::Value), Box<dyn std::error::Error>> {
    let resp = call(
        router,
        Method::GET,
        "/api/v1/mailboxes",
        &[("cookie", cookie)],
    )
    .await?;
    let status = resp.status();
    let json: serde_json::Value = serde_json::from_str(&body_string(resp).await?)?;
    Ok((status, json))
}

// ---------------------------------------------------------------------------
// AU-04 AC1: a signed-in user with step-up links a Gmail mailbox and it lists.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn au_04_ac1_link_adds_mailbox() -> Result<(), Box<dyn std::error::Error>> {
    let (state, fakes) = fixture()?;
    let router = build_router(state.clone());
    let a = authed(&state, &fakes).await?;
    let (cookie, outcome) = link(&router, &fakes, &a, "sub-new", "new@example.com").await?;
    assert_eq!(outcome, "linked");
    assert_eq!(count(&fakes, "mailboxes"), 1, "one mailbox linked");
    let (status, json) = mailboxes_json(&router, &cookie).await?;
    assert_eq!(status, StatusCode::OK);
    let list = json["mailboxes"].as_array().ok_or("mailboxes array")?;
    assert_eq!(list.len(), 1);
    assert_eq!(list[0]["email_address"], "new@example.com");
    assert_eq!(list[0]["status"], "connected");
    assert_eq!(list[0]["is_primary"], true, "the first mailbox is primary");
    Ok(())
}

// ---------------------------------------------------------------------------
// AU-04 AC2: three Gmail mailboxes on one user are all listed by address.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn au_04_ac2_three_mailboxes_listed_with_addresses() -> Result<(), Box<dyn std::error::Error>>
{
    let (state, fakes) = fixture()?;
    let router = build_router(state.clone());
    let a = authed(&state, &fakes).await?;
    let (cookie, _) = link(&router, &fakes, &a, "sub-1", "one@example.com").await?;
    // The link sets `recent_auth_at`, so the next link needs no fresh step-up.
    let (cookie, _) = link2(&router, &fakes, &cookie, "sub-2", "two@example.com").await?;
    let (cookie, _) = link2(&router, &fakes, &cookie, "sub-3", "three@example.com").await?;
    assert_eq!(count(&fakes, "mailboxes"), 3);
    let (_, json) = mailboxes_json(&router, &cookie).await?;
    let list = json["mailboxes"].as_array().ok_or("mailboxes array")?;
    let mut addresses: Vec<String> = list
        .iter()
        .map(|m| m["email_address"].as_str().unwrap_or_default().to_owned())
        .collect();
    addresses.sort();
    assert_eq!(
        addresses,
        vec![
            "one@example.com".to_owned(),
            "three@example.com".to_owned(),
            "two@example.com".to_owned()
        ]
    );
    Ok(())
}

/// A second link for an already linked-in session (cookie is the live one).
async fn link2(
    router: &Router,
    fakes: &testkit::Fakes,
    cookie: &str,
    sub: &str,
    email: &str,
) -> Result<(String, String), Box<dyn std::error::Error>> {
    let csrf = csrf_for(router, cookie).await?;
    let (rotated, oauth_state) = begin(router, cookie, &csrf, link_body()).await?;
    let resp = callback(router, fakes, &rotated, &oauth_state, claims(sub, email)).await?;
    let new_cookie = set_cookie(&resp).unwrap_or(rotated);
    Ok((new_cookie, outcome(&resp)))
}

/// The live CSRF token for a session cookie.
async fn csrf_for(router: &Router, cookie: &str) -> Result<String, Box<dyn std::error::Error>> {
    let resp = call(
        router,
        Method::GET,
        "/api/v1/session",
        &[("cookie", cookie)],
    )
    .await?;
    let json: serde_json::Value = serde_json::from_str(&body_string(resp).await?)?;
    Ok(json["csrf_token"].as_str().unwrap_or_default().to_owned())
}

// ---------------------------------------------------------------------------
// AU-04 AC3: a mailbox already linked to another user is refused, nothing is
// stored and nothing is revoked.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn au_04_ac3_mailbox_of_other_user_refused() -> Result<(), Box<dyn std::error::Error>> {
    let (state, fakes) = fixture()?;
    let router = build_router(state.clone());
    let a = authed(&state, &fakes).await?;
    let other = seed_user(&fakes, false).await?;
    let owned = seed_mailbox(
        &fakes,
        other,
        "sub-taken",
        "taken@example.com",
        true,
        MailboxStatus::Connected,
    )
    .await?;
    let (_, outcome) = link(&router, &fakes, &a, "sub-taken", "taken@example.com").await?;
    assert_eq!(outcome, "mailbox_linked_elsewhere");
    assert_eq!(count(&fakes, "mailboxes"), 1, "nothing new is stored");
    let stored = fakes
        .store
        .mailboxes()
        .get(&owned)
        .await?
        .ok_or("mailbox")?;
    assert_eq!(stored.record.user_id, other, "it stays with its owner");
    assert!(
        stored.record.refresh_token.is_none(),
        "the new grant is not stored"
    );
    assert!(
        fakes.identity.revoked().is_empty(),
        "nothing is revoked for the other user's grant"
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// AU-04 AC5 / S8: the `link` authorisation URL requests exactly the S8 scopes.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn au_04_ac5_link_requests_only_s8_scopes() -> Result<(), Box<dyn std::error::Error>> {
    let (state, fakes, spy) = fixture_with_spy()?;
    let router = build_router(state.clone());
    let a = authed(&state, &fakes).await?;
    let _ = begin(&router, &a.cookie, &a.csrf, link_body()).await?;
    let requests = spy.requests();
    assert_eq!(requests.len(), 1, "one authorization request");
    let started = requests.first().ok_or("request")?;
    assert_eq!(
        started.scopes,
        adapters_gmail::scopes::GMAIL_SCOPES.to_vec(),
        "exactly the S8 Gmail scopes"
    );
    assert_eq!(started.prompt, Some(Prompt::Consent));
    assert_eq!(started.max_age_s, None, "link does not force a re-auth");
    Ok(())
}

// ---------------------------------------------------------------------------
// AU-04 AC6: `link` without a fresh step-up is refused and links nothing.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn au_04_ac6_link_without_step_up_refused() -> Result<(), Box<dyn std::error::Error>> {
    let (state, fakes) = fixture()?;
    let router = build_router(state.clone());
    let user = seed_user(&fakes, false).await?;
    // A session with no fresh sign-in.
    let (cookie, record) = SessionService::new(&state)
        .establish(None, &user, None)
        .await?;
    let cookie = cookie_value(&cookie.0)?;
    let resp = authed_post(
        &router,
        "/api/v1/auth/google/start",
        &cookie,
        &record.record.csrf_token,
        link_body(),
    )
    .await?;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    assert_eq!(code_of(resp).await?, "step_up_required");
    assert_eq!(count(&fakes, "mailboxes"), 0, "nothing is linked");
    let loaded = SessionService::new(&state)
        .load(&cookie_headers(&cookie))
        .await?
        .ok_or("session")?;
    assert!(
        loaded.record.record.pre_auth.is_none(),
        "no OAuth state is written on a refused link"
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// AU-03 AC7: a mailbox is keyed by Google `sub`, not by email.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn au_03_ac7_mailbox_keyed_by_sub_not_email() -> Result<(), Box<dyn std::error::Error>> {
    let (state, fakes) = fixture()?;
    let router = build_router(state.clone());
    let a = authed(&state, &fakes).await?;
    let (cookie, _) = link(&router, &fakes, &a, "sub-fixed", "first@example.com").await?;
    // The same `sub` with a changed email is the same mailbox.
    let (cookie, outcome) =
        link2(&router, &fakes, &cookie, "sub-fixed", "renamed@example.com").await?;
    assert_eq!(outcome, "linked");
    assert_eq!(count(&fakes, "mailboxes"), 1, "one mailbox for one sub");
    let expected = ports::mailbox_id_for(Provider::Gmail, &ProviderSubjectId::new("sub-fixed")?);
    let stored = fakes
        .store
        .mailboxes()
        .get(&expected)
        .await?
        .ok_or("mailbox")?;
    assert_eq!(stored.record.provider_subject_id.as_str(), "sub-fixed");
    let (_, json) = mailboxes_json(&router, &cookie).await?;
    let list = json["mailboxes"].as_array().ok_or("mailboxes array")?;
    assert_eq!(list.len(), 1);
    assert_eq!(
        list[0]["email_address"], "first@example.com",
        "the original address is kept"
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// ST-03 AC1: API-MBX-1 lists provider, address and status for every mailbox.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn st_03_ac1_list_shows_provider_address_status() -> Result<(), Box<dyn std::error::Error>> {
    let (state, fakes) = fixture()?;
    let router = build_router(state.clone());
    let a = authed(&state, &fakes).await?;
    seed_mailbox(
        &fakes,
        a.user,
        "sub-good",
        "good@example.com",
        true,
        MailboxStatus::Connected,
    )
    .await?;
    seed_mailbox(
        &fakes,
        a.user,
        "sub-flag",
        "flag@example.com",
        false,
        MailboxStatus::NeedsSignIn,
    )
    .await?;
    let resp = call(
        &router,
        Method::GET,
        "/api/v1/mailboxes",
        &[("cookie", &a.cookie)],
    )
    .await?;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        resp.headers()
            .get(header::CACHE_CONTROL)
            .and_then(|v| v.to_str().ok()),
        Some("no-store"),
        "the list carries addresses: never cached"
    );
    let json: serde_json::Value = serde_json::from_str(&body_string(resp).await?)?;
    let list = json["mailboxes"].as_array().ok_or("mailboxes array")?;
    assert_eq!(list.len(), 2);
    let flagged = list
        .iter()
        .find(|m| m["email_address"] == "flag@example.com")
        .ok_or("flag mailbox")?;
    assert_eq!(flagged["provider"], "gmail");
    assert_eq!(flagged["status"], "needs_sign_in");
    assert_eq!(flagged["is_primary"], false);
    assert!(flagged["linked_at"].as_str().is_some(), "linked_at present");
    assert!(
        flagged.get("refresh_token").is_none() && flagged.get("provider_subject_id").is_none(),
        "no secrets in the response"
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// ST-03 AC1: a successful reconnect sets the mailbox back to `connected`.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn st_03_ac1_reconnect_sets_connected() -> Result<(), Box<dyn std::error::Error>> {
    let (state, fakes) = fixture()?;
    let router = build_router(state.clone());
    let a = authed(&state, &fakes).await?;
    let mailbox = seed_mailbox(
        &fakes,
        a.user,
        "sub-reconnect",
        "reconnect@example.com",
        true,
        MailboxStatus::NeedsSignIn,
    )
    .await?;
    let (rotated, oauth_state) =
        begin(&router, &a.cookie, &a.csrf, &reconnect_body(&mailbox)).await?;
    let resp = callback(
        &router,
        &fakes,
        &rotated,
        &oauth_state,
        claims("sub-reconnect", "reconnect@example.com"),
    )
    .await?;
    assert_eq!(outcome(&resp), "reconnected");
    let stored = fakes
        .store
        .mailboxes()
        .get(&mailbox)
        .await?
        .ok_or("mailbox")?;
    assert_eq!(stored.record.status, MailboxStatus::Connected);
    assert!(
        stored.record.refresh_token.is_some(),
        "the new grant is stored"
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// ST-03 AC1: reconnect finishing with a different account than the one named
// fails and touches nothing.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn st_03_ac1_reconnect_with_other_account_fails() -> Result<(), Box<dyn std::error::Error>> {
    let (state, fakes) = fixture()?;
    let router = build_router(state.clone());
    let a = authed(&state, &fakes).await?;
    let mailbox = seed_mailbox(
        &fakes,
        a.user,
        "sub-sick",
        "sick@example.com",
        true,
        MailboxStatus::NeedsSignIn,
    )
    .await?;
    // A second mailbox of the same user: signing in with it must not silently
    // reconnect the first one.
    seed_mailbox(
        &fakes,
        a.user,
        "sub-other",
        "other@example.com",
        false,
        MailboxStatus::Connected,
    )
    .await?;
    let (rotated, oauth_state) =
        begin(&router, &a.cookie, &a.csrf, &reconnect_body(&mailbox)).await?;
    let resp = callback(
        &router,
        &fakes,
        &rotated,
        &oauth_state,
        claims("sub-other", "other@example.com"),
    )
    .await?;
    assert_eq!(outcome(&resp), "failed");
    let stored = fakes
        .store
        .mailboxes()
        .get(&mailbox)
        .await?
        .ok_or("mailbox")?;
    assert_eq!(stored.record.status, MailboxStatus::NeedsSignIn);
    assert!(stored.record.refresh_token.is_none(), "nothing was stored");
    Ok(())
}

// ---------------------------------------------------------------------------
// INV-3: two users linking the same `sub` in turn — exactly one owns it.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn inv_3_second_linker_of_same_sub_refused() -> Result<(), Box<dyn std::error::Error>> {
    let (state, fakes) = fixture()?;
    let router = build_router(state.clone());
    let a = authed(&state, &fakes).await?;
    let b = authed(&state, &fakes).await?;
    let (_, first) = link(&router, &fakes, &a, "sub-race", "race@example.com").await?;
    let (_, second) = link(&router, &fakes, &b, "sub-race", "race@example.com").await?;
    assert_eq!(first, "linked", "the first link wins");
    assert_eq!(second, "mailbox_linked_elsewhere", "the second is refused");
    assert_eq!(count(&fakes, "mailboxes"), 1, "exactly one mailbox");
    let mailbox_id = ports::mailbox_id_for(Provider::Gmail, &ProviderSubjectId::new("sub-race")?);
    let stored = fakes
        .store
        .mailboxes()
        .get(&mailbox_id)
        .await?
        .ok_or("mailbox")?;
    assert_eq!(
        stored.record.user_id, a.user,
        "it belongs to the first user"
    );
    assert!(
        fakes.identity.revoked().is_empty(),
        "the loser's grant is never revoked"
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// INV-3: two users racing to link the same `sub` — the loser's write hits the
// store's `AlreadyExists` and the recovery branch refuses it without taking
// the winner's mailbox (the sequential path above never reaches that branch).
// ---------------------------------------------------------------------------
#[tokio::test]
async fn inv_3_racing_link_one_owner_recovers_from_already_exists(
) -> Result<(), Box<dyn std::error::Error>> {
    let (state, fakes) = fixture()?;
    let router = build_router(state.clone());
    let a = authed(&state, &fakes).await?;
    // The other user's record is staged to land between A's `by_subject`
    // pre-check and A's `put` — exactly the race the recovery branch handles.
    let winner = seed_user(&fakes, false).await?;
    let staged = mailbox_record(
        &fakes,
        winner,
        "sub-race",
        "race@example.com",
        true,
        MailboxStatus::Connected,
    )
    .await?;
    fakes.store.race_mailbox_put(staged);
    let (_, outcome) = link(&router, &fakes, &a, "sub-race", "race@example.com").await?;
    assert_eq!(
        outcome, "mailbox_linked_elsewhere",
        "the race loser is refused, not a failure"
    );
    assert_eq!(count(&fakes, "mailboxes"), 1, "exactly one mailbox");
    let mailbox_id = ports::mailbox_id_for(Provider::Gmail, &ProviderSubjectId::new("sub-race")?);
    let stored = fakes
        .store
        .mailboxes()
        .get(&mailbox_id)
        .await?
        .ok_or("mailbox")?;
    assert_eq!(
        stored.record.user_id, winner,
        "the winner keeps the mailbox"
    );
    assert!(
        stored.record.refresh_token.is_none(),
        "the loser's grant is never stored"
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// INV-3: a user whose own link races itself reuses the mailbox rather than
// failing (the same-user arm of the recovery branch).
// ---------------------------------------------------------------------------
#[tokio::test]
async fn inv_3_racing_same_user_reuses_the_mailbox() -> Result<(), Box<dyn std::error::Error>> {
    let (state, fakes) = fixture()?;
    let router = build_router(state.clone());
    let a = authed(&state, &fakes).await?;
    let staged = mailbox_record(
        &fakes,
        a.user,
        "sub-self",
        "self@example.com",
        true,
        MailboxStatus::Connected,
    )
    .await?;
    fakes.store.race_mailbox_put(staged);
    let (cookie, outcome) = link(&router, &fakes, &a, "sub-self", "self@example.com").await?;
    assert_eq!(outcome, "linked", "the racing link still succeeds");
    assert_eq!(count(&fakes, "mailboxes"), 1, "one mailbox, reused");
    let (_, json) = mailboxes_json(&router, &cookie).await?;
    let list = json["mailboxes"].as_array().ok_or("mailboxes array")?;
    assert_eq!(list.len(), 1);
    let mailbox_id = ports::mailbox_id_for(Provider::Gmail, &ProviderSubjectId::new("sub-self")?);
    let stored = fakes
        .store
        .mailboxes()
        .get(&mailbox_id)
        .await?
        .ok_or("mailbox")?;
    assert!(
        stored.record.refresh_token.is_some(),
        "the raced grant is stored onto the existing mailbox"
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// ASVS V8.2.2: a reconnect for another user's mailbox is `404 not_found`.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn asvs_v8_2_2_reconnect_other_users_mailbox_not_found(
) -> Result<(), Box<dyn std::error::Error>> {
    let (state, fakes) = fixture()?;
    let router = build_router(state.clone());
    let a = authed(&state, &fakes).await?;
    let other = seed_user(&fakes, false).await?;
    let foreign = seed_mailbox(
        &fakes,
        other,
        "sub-foreign",
        "foreign@example.com",
        true,
        MailboxStatus::NeedsSignIn,
    )
    .await?;
    let resp = authed_post(
        &router,
        "/api/v1/auth/google/start",
        &a.cookie,
        &a.csrf,
        &reconnect_body(&foreign),
    )
    .await?;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    assert_eq!(code_of(resp).await?, "not_found");
    Ok(())
}

// ---------------------------------------------------------------------------
// ST-03 AC1: a reconnect hints the mailbox's own address at Google.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn st_03_ac1_reconnect_hints_the_mailbox_address() -> Result<(), Box<dyn std::error::Error>> {
    let (state, fakes, spy) = fixture_with_spy()?;
    let router = build_router(state.clone());
    let a = authed(&state, &fakes).await?;
    let mailbox = seed_mailbox(
        &fakes,
        a.user,
        "sub-hint",
        "hinted@example.com",
        true,
        MailboxStatus::NeedsSignIn,
    )
    .await?;
    let _ = begin(&router, &a.cookie, &a.csrf, &reconnect_body(&mailbox)).await?;
    let requests = spy.requests();
    assert_eq!(requests.len(), 1, "one authorization request");
    assert_eq!(
        requests.first().ok_or("request")?.login_hint.as_deref(),
        Some("hinted@example.com"),
        "the hint is the mailbox's address"
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// A callback for a link never revokes a linked subject's grant.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn link_callback_never_revokes_a_linked_grant() -> Result<(), Box<dyn std::error::Error>> {
    let (state, fakes) = fixture()?;
    let router = build_router(state.clone());
    let a = authed(&state, &fakes).await?;
    let _ = link(&router, &fakes, &a, "sub-safe", "safe@example.com").await?;
    assert!(
        fakes.identity.revoked().is_empty(),
        "a linked subject's grant is kept"
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// AU-04 AC1: a `link` whose grant write fails leaves a mailbox that does not
// claim `connected`, so the stored row can never disagree with the missing
// grant (T-601a security review F4).
// ---------------------------------------------------------------------------
#[tokio::test]
async fn au_04_ac1_failed_grant_write_leaves_no_connected_mailbox(
) -> Result<(), Box<dyn std::error::Error>> {
    let (state, fakes) = fixture()?;
    let router = build_router(state.clone());
    let a = authed(&state, &fakes).await?;
    // The row is written first; sealing its refresh token then fails.
    fakes.keys.fail_field(aad_fields::MAILBOX_REFRESH_TOKEN, 1);
    let (_cookie, outcome) = link(&router, &fakes, &a, "sub-grant", "grant@example.com").await?;
    assert_eq!(outcome, "failed");
    let subject = ProviderSubjectId::new("sub-grant")?;
    let mailbox_id = ports::mailbox_id_for(Provider::Gmail, &subject);
    let stored = fakes
        .store
        .mailboxes()
        .get(&mailbox_id)
        .await?
        .ok_or("mailbox")?;
    assert!(
        stored.record.refresh_token.is_none(),
        "the grant was not stored"
    );
    assert_eq!(
        stored.record.status,
        MailboxStatus::NeedsSignIn,
        "a mailbox with no grant is never `connected`"
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// ST-03 AC1 (S7 3.4): `reconnect` refreshes a mailbox in `needs_sign_in` only,
// so a caller cannot clear a state such as `consent_blocked` with it.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn st_03_ac1_reconnect_refused_for_a_connected_mailbox(
) -> Result<(), Box<dyn std::error::Error>> {
    let (state, fakes) = fixture()?;
    let router = build_router(state.clone());
    let a = authed(&state, &fakes).await?;
    let mailbox = seed_mailbox(
        &fakes,
        a.user,
        "sub-live",
        "live@example.com",
        true,
        MailboxStatus::Connected,
    )
    .await?;
    let (rotated, oauth_state) =
        begin(&router, &a.cookie, &a.csrf, &reconnect_body(&mailbox)).await?;
    let resp = callback(
        &router,
        &fakes,
        &rotated,
        &oauth_state,
        claims("sub-live", "live@example.com"),
    )
    .await?;
    assert_eq!(
        outcome(&resp),
        "failed",
        "reconnect is for needs_sign_in only"
    );
    let stored = fakes
        .store
        .mailboxes()
        .get(&mailbox)
        .await?
        .ok_or("mailbox")?;
    assert_eq!(stored.record.status, MailboxStatus::Connected);
    assert!(
        stored.record.refresh_token.is_none(),
        "the new grant is dropped"
    );
    Ok(())
}
