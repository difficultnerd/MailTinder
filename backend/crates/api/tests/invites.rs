//! T-505 invite and invite-request integration tests (AU-01, AU-02, INV-T1,
//! INV-T2, ASVS V6.4.1, V2.4.1).
//!
//! Everything runs through the real router with the fake identity provider,
//! the in-memory store and keys, the recording invite mailer and the virtual
//! clock.

#![allow(clippy::too_many_lines)]

use std::sync::Arc;

use api::session::pre_auth::{seal_pre_auth, PreAuthPlain};
use api::session::store::SessionService;
use api::state::AppState;
use api::{app_state, build_router, config::ApiConfig};
use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use axum::response::Response;
use axum::Router;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use domain::{EmailAddress, MailboxStatus, Provider, ProviderSubjectId, UserId};
use obs::Sensitive;
use ports::store::aad_fields;
use ports::{
    Aad, AuthIntent, Ciphertext, Clock, IdClaims, InviteRequestId, InviteRequestRecord,
    InviteRequestStatus, KeyService, MailboxRecord, Precondition, Rng, ServerStore, SessionState,
    Sha256Hash, SystemAad, SystemKeyService, TokenSet, UserRecord,
};
use sha2::{Digest, Sha256};
use svc_common::mint::seal_refresh_token;
use time::{Duration, OffsetDateTime};
use tower::ServiceExt;
use url::Url;

type TestResult = Result<(), Box<dyn std::error::Error>>;

const ORIGIN: &str = "https://mailtinder.test";
const EMAIL_KEY: &[u8] = b"fake-email-key";
const INVITEE: &str = "invitee@example.com";
const IP: &str = "198.51.100.7";

/// Tracing caches each callsite's interest process-wide, and a scoped capture
/// subscriber is only seen by its own thread, so tests that run side by side
/// can blank out the capture of the one asserting on a security event. Every
/// test therefore holds this lock, which makes the log assertion deterministic.
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

async fn fixture() -> Result<
    (
        AppState,
        testkit::Fakes,
        tokio::sync::MutexGuard<'static, ()>,
    ),
    Box<dyn std::error::Error>,
> {
    let serial = SERIAL.lock().await;
    let (ports, fakes) = testkit::fake_ports();
    let config = ApiConfig::new(
        ORIGIN.to_owned(),
        "fake-client".into(),
        Sensitive::new(b"fake-log-key".to_vec()),
        Sensitive::new(EMAIL_KEY.to_vec()),
    )?;
    Ok((app_state(Arc::new(ports), Arc::new(config)), fakes, serial))
}

fn cookie_pair(header: &axum::http::HeaderValue) -> Result<String, Box<dyn std::error::Error>> {
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
        .and_then(|v| v.to_str().ok())
        .map(|v| v.split(';').next().unwrap_or_default().to_owned())
}

async fn body_string(resp: Response) -> Result<String, Box<dyn std::error::Error>> {
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

async fn json_of(resp: Response) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
    Ok(serde_json::from_str(&body_string(resp).await?)?)
}

async fn code_of(resp: Response) -> Result<String, Box<dyn std::error::Error>> {
    Ok(json_of(resp).await?["code"]
        .as_str()
        .unwrap_or_default()
        .to_owned())
}

fn sha(token: &str) -> Sha256Hash {
    let digest = Sha256::digest(token.as_bytes());
    let mut out = [0u8; 32];
    out.copy_from_slice(&digest);
    Sha256Hash(out)
}

fn lookup(email: &str) -> Result<ports::EmailLookupHash, Box<dyn std::error::Error>> {
    Ok(api::auth::email_key::email_lookup_hash(
        &Sensitive::new(EMAIL_KEY.to_vec()),
        &EmailAddress::parse(email)?,
    ))
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

/// A primary mailbox with a sealed refresh token, and the scripted access
/// token its send needs.
async fn seed_mailbox(
    state: &AppState,
    fakes: &testkit::Fakes,
    user: UserId,
    sub: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let subject = ProviderSubjectId::new(sub)?;
    let mailbox_id = ports::mailbox_id_for(Provider::Gmail, &subject);
    let user_record = fakes
        .store
        .users()
        .get(&user)
        .await?
        .ok_or("seeded user")?
        .record;
    let email = fakes
        .keys
        .seal(
            &user,
            &user_record.wrapped_data_key,
            &Aad {
                user,
                scope: mailbox_id.0.to_string(),
                field: aad_fields::MAILBOX_EMAIL,
            },
            b"admin@example.com",
        )
        .await?;
    let refresh = format!("refresh-{sub}");
    let sealed = seal_refresh_token(
        &state.ports,
        &user_record,
        &mailbox_id,
        &Sensitive::new(refresh.clone()),
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
                email_address: Ciphertext(email),
                status: MailboxStatus::Connected,
                linked_at: fakes.clock.now(),
                is_primary: true,
                refresh_token: Some(sealed),
            },
            Precondition::MustNotExist,
        )
        .await?;
    for _ in 0..4 {
        fakes
            .identity
            .script_refresh(&refresh, Ok("access-token".to_owned()));
    }
    Ok(())
}

struct Authed {
    cookie: String,
    csrf: String,
}

/// A signed-in session; `step_up` gives it a fresh sign-in.
async fn session_for(
    state: &AppState,
    user: &UserId,
    step_up: bool,
) -> Result<Authed, Box<dyn std::error::Error>> {
    let recent = step_up.then(|| state.ports.clock.now());
    let (cookie, record) = SessionService::new(state)
        .establish(None, user, recent)
        .await?;
    Ok(Authed {
        cookie: cookie_pair(&cookie.0)?,
        csrf: record.record.csrf_token.clone(),
    })
}

/// An admin with a primary mailbox, step-up fresh or not.
async fn admin(
    state: &AppState,
    fakes: &testkit::Fakes,
    sub: &str,
    step_up: bool,
) -> Result<Authed, Box<dyn std::error::Error>> {
    let user = seed_user(fakes, true).await?;
    seed_mailbox(state, fakes, user, sub).await?;
    session_for(state, &user, step_up).await
}

/// A session in `pending_invite_request` holding `email`.
async fn pending_session(
    state: &AppState,
    fakes: &testkit::Fakes,
    email: &str,
) -> Result<Authed, Box<dyn std::error::Error>> {
    let (cookie, created) = SessionService::new(state).create_anonymous().await?;
    let mut record = created.record.clone();
    let plain = PreAuthPlain {
        intent: AuthIntent::Join,
        oauth_state: Sensitive::new(String::new()),
        nonce: Sensitive::new(String::new()),
        pkce_verifier: Sensitive::new(String::new()),
        invite_token_hash: None,
        pending_email: Some(Sensitive::new(email.to_owned())),
        mailbox_id: None,
        started_at: fakes.clock.now(),
    };
    record.state = SessionState::PendingInviteRequest;
    record.pre_auth = Some(
        seal_pre_auth(
            state.ports.system_keys.as_ref(),
            &record.session_record_id,
            &plain,
        )
        .await?,
    );
    fakes
        .store
        .sessions()
        .put(&record, Precondition::None)
        .await?;
    Ok(Authed {
        cookie: cookie_pair(&cookie.0)?,
        csrf: record.csrf_token,
    })
}

async fn call(
    router: &Router,
    method: Method,
    uri: &str,
    who: &Authed,
    body: Option<&str>,
) -> Result<Response, Box<dyn std::error::Error>> {
    let mut req = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::ORIGIN, ORIGIN)
        .header(header::COOKIE, &who.cookie)
        .header("x-csrf-token", &who.csrf)
        .header("x-forwarded-for", IP);
    if body.is_some() {
        req = req.header(header::CONTENT_TYPE, "application/json");
    }
    let req = req.body(Body::from(body.unwrap_or_default().to_owned()))?;
    Ok(router.clone().oneshot(req).await?)
}

async fn invite(
    router: &Router,
    who: &Authed,
    email: &str,
) -> Result<Response, Box<dyn std::error::Error>> {
    let body = serde_json::json!({ "email_address": email }).to_string();
    call(
        router,
        Method::POST,
        "/api/v1/admin/invites",
        who,
        Some(&body),
    )
    .await
}

async fn request_invite(
    router: &Router,
    who: &Authed,
) -> Result<Response, Box<dyn std::error::Error>> {
    call(
        router,
        Method::POST,
        "/api/v1/invite-requests",
        who,
        Some("{}"),
    )
    .await
}

/// The token in the most recent invite email.
fn last_token(fakes: &testkit::Fakes) -> Result<String, Box<dyn std::error::Error>> {
    let (_, link) = fakes.invite_mailer.sent().pop().ok_or("no invite sent")?;
    token_of(&link)
}

fn token_of(link: &str) -> Result<String, Box<dyn std::error::Error>> {
    let prefix = format!("{ORIGIN}/#/invite?t=");
    link.strip_prefix(&prefix)
        .map(str::to_owned)
        .ok_or_else(|| "link has the wrong shape".into())
}

fn store_dump(fakes: &testkit::Fakes) -> Result<String, Box<dyn std::error::Error>> {
    Ok(serde_json::to_string(&fakes.store.export_json())?)
}

fn count(fakes: &testkit::Fakes, collection: &str) -> usize {
    fakes
        .store
        .export_json()
        .iter()
        .filter(|(name, _)| *name == collection)
        .count()
}

/// Redeem `token` for `email` through the T-502b join flow; the outcome.
async fn redeem(
    router: &Router,
    state: &AppState,
    fakes: &testkit::Fakes,
    token: &str,
    email: &str,
    sub: &str,
) -> Result<String, Box<dyn std::error::Error>> {
    let granted = adapters_gmail::scopes::GMAIL_SCOPES
        .iter()
        .map(|s| (*s).to_owned())
        .collect();
    redeem_with(router, state, fakes, token, email, sub, granted, true).await
}

/// The same, with the granted scopes (and the refresh token) under the test's
/// control, so a partial-consent refresh can be driven (security review F1).
#[allow(clippy::too_many_arguments)]
async fn redeem_with(
    router: &Router,
    state: &AppState,
    fakes: &testkit::Fakes,
    token: &str,
    email: &str,
    sub: &str,
    granted_scopes: Vec<String>,
    with_refresh: bool,
) -> Result<String, Box<dyn std::error::Error>> {
    let (cookie, created) = SessionService::new(state).create_anonymous().await?;
    let cookie = cookie_pair(&cookie.0)?;
    let body = serde_json::json!({ "intent": "join", "invite_token": token, "mailbox_id": null });
    let req = Request::builder()
        .method(Method::POST)
        .uri("/api/v1/auth/google/start")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::ORIGIN, ORIGIN)
        .header(header::COOKIE, &cookie)
        .header("x-csrf-token", &created.record.csrf_token)
        .body(Body::from(serde_json::to_vec(&body)?))?;
    let resp = router.clone().oneshot(req).await?;
    assert_eq!(resp.status(), StatusCode::OK);
    let rotated = set_cookie(&resp).ok_or("rotated cookie")?;
    let json = json_of(resp).await?;
    let url = Url::parse(json["authorization_url"].as_str().ok_or("url")?)?;
    let oauth_state = url
        .query_pairs()
        .find(|(k, _)| k == "state")
        .map(|(_, v)| v.into_owned())
        .ok_or("state")?;
    fakes.identity.script_exchange(Ok(TokenSet {
        access_token: Sensitive::new("access-token-A".to_owned()),
        refresh_token: with_refresh.then(|| Sensitive::new("refresh-token-R".to_owned())),
        id_token: Sensitive::new("id-token-I".to_owned()),
        expires_in_s: 3600,
        granted_scopes,
    }));
    let issued = OffsetDateTime::from_unix_timestamp(1_700_000_000)?;
    fakes.identity.script_claims(Ok(IdClaims {
        sub: sub.to_owned(),
        email: Sensitive::new(email.to_owned()),
        email_verified: true,
        auth_time: None,
        amr: vec![],
        issued_at: issued,
        expires_at: issued + Duration::hours(1),
    }));
    let resp = router
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!(
                    "/api/v1/auth/google/callback?code=code-A&state={oauth_state}"
                ))
                .header(header::COOKIE, rotated)
                .body(Body::empty())?,
        )
        .await?;
    let location = resp
        .headers()
        .get(header::LOCATION)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    Ok(location
        .rsplit_once("outcome=")
        .map(|(_, v)| v.to_owned())
        .unwrap_or_default())
}

/// Put an invite request straight into the store.
async fn seed_request(
    fakes: &testkit::Fakes,
    email: &str,
) -> Result<InviteRequestId, Box<dyn std::error::Error>> {
    let id = InviteRequestId(fakes.rng.uuid_v4());
    let sealed = fakes
        .system_keys
        .seal(
            &SystemAad {
                scope: id.0.to_string(),
                field: aad_fields::INVITE_REQUEST_EMAIL,
            },
            email.as_bytes(),
        )
        .await?;
    fakes
        .store
        .invite_requests()
        .put(
            &InviteRequestRecord {
                request_id: id,
                email_address: Ciphertext(sealed),
                email_lookup: lookup(email)?,
                created_at: fakes.clock.now(),
                status: InviteRequestStatus::Pending,
            },
            Precondition::MustNotExist,
        )
        .await?;
    Ok(id)
}

// ---------------------------------------------------------------------------
// AU-01 AC1: an invite is recorded and a single-use link is emailed.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn au_01_ac1_invite_created_and_email_sent_with_token_link() -> TestResult {
    let (state, fakes, _serial) = fixture().await?;
    let router = build_router(state.clone());
    let admin = admin(&state, &fakes, "sub-admin", true).await?;

    let resp = invite(&router, &admin, INVITEE).await?;
    assert_eq!(resp.status(), StatusCode::CREATED);
    let body = body_string(resp).await?;
    let json: serde_json::Value = serde_json::from_str(&body)?;
    assert_eq!(json["status"], "pending");
    assert_eq!(json["email_address"], INVITEE);

    let sent = fakes.invite_mailer.sent();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].0.as_str(), INVITEE);
    let token = token_of(&sent[0].1)?;
    assert_eq!(token.len(), 43);
    assert!(
        !body.contains(&token),
        "the response never carries the token"
    );
    assert_eq!(count(&fakes, "invites"), 1);
    Ok(())
}

#[tokio::test]
async fn au_01_ac1_stored_hash_matches_link_token_and_expires_in_7_days() -> TestResult {
    let (state, fakes, _serial) = fixture().await?;
    let router = build_router(state.clone());
    let admin = admin(&state, &fakes, "sub-admin", true).await?;
    let now = fakes.clock.now();

    let resp = invite(&router, &admin, INVITEE).await?;
    assert_eq!(resp.status(), StatusCode::CREATED);
    let token = last_token(&fakes)?;

    let stored = fakes
        .store
        .invites()
        .by_token_hash(&sha(&token))
        .await?
        .ok_or("the link token's hash is stored")?;
    assert_eq!(stored.record.token_hash, sha(&token));
    assert_eq!(stored.record.expires_at, now + Duration::days(7));
    assert_eq!(stored.record.last_sent_at, now);
    assert_eq!(stored.record.email_lookup, lookup(INVITEE)?);
    Ok(())
}

// ---------------------------------------------------------------------------
// AU-01 AC2: inviting again re-sends with a new token; the old one dies.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn au_01_ac2_reinvite_resends_new_token_old_refused() -> TestResult {
    let (state, fakes, _serial) = fixture().await?;
    let router = build_router(state.clone());
    let admin = admin(&state, &fakes, "sub-admin", true).await?;

    assert_eq!(
        invite(&router, &admin, INVITEE).await?.status(),
        StatusCode::CREATED
    );
    let old = last_token(&fakes)?;
    // The same address in other case is the same invite.
    assert_eq!(
        invite(&router, &admin, "Invitee@Example.com")
            .await?
            .status(),
        StatusCode::OK
    );
    let new = last_token(&fakes)?;
    assert_ne!(old, new);
    assert_eq!(fakes.invite_mailer.sent().len(), 2);
    assert_eq!(count(&fakes, "invites"), 1, "no duplicate record");

    assert_eq!(
        redeem(&router, &state, &fakes, &old, INVITEE, "sub-old").await?,
        "invite_invalid"
    );
    assert_eq!(
        redeem(&router, &state, &fakes, &new, INVITEE, "sub-new").await?,
        "joined"
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// Security review F1: a join that does not deliver the whole Gmail grant is
// refused BEFORE the single-use invite is claimed, so the invitee can retry
// with the same link instead of needing a new invite from the admin.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn au_01_ac1_partial_consent_does_not_burn_the_invite() -> TestResult {
    let (state, fakes, _serial) = fixture().await?;
    let router = build_router(state.clone());
    let admin = admin(&state, &fakes, "sub-admin", true).await?;

    assert_eq!(
        invite(&router, &admin, INVITEE).await?.status(),
        StatusCode::CREATED
    );
    let token = last_token(&fakes)?;
    // Granular consent: the invitee unticked all but the first two scopes.
    let partial = adapters_gmail::scopes::GMAIL_SCOPES[..2]
        .iter()
        .map(|s| (*s).to_owned())
        .collect();
    assert_eq!(
        redeem_with(
            &router,
            &state,
            &fakes,
            &token,
            INVITEE,
            "sub-partial",
            partial,
            true
        )
        .await?,
        "failed"
    );
    let stored = fakes
        .store
        .invites()
        .by_token_hash(&sha(&token))
        .await?
        .ok_or("the invite record survives")?;
    assert_eq!(
        stored.record.status,
        domain::InviteStatus::Pending,
        "the single-use invite is not burned by a partial grant"
    );
    assert_eq!(
        count(&fakes, "mailboxes"),
        1,
        "nothing is linked beyond the admin's own mailbox"
    );
    // The full-grant retry with the same token now works.
    assert_eq!(
        redeem(&router, &state, &fakes, &token, INVITEE, "sub-partial").await?,
        "joined"
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// Security review F1: a join whose refresh token is missing does not burn the
// invite either.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn au_01_ac1_missing_grant_does_not_burn_the_invite() -> TestResult {
    let (state, fakes, _serial) = fixture().await?;
    let router = build_router(state.clone());
    let admin = admin(&state, &fakes, "sub-admin", true).await?;

    assert_eq!(
        invite(&router, &admin, INVITEE).await?.status(),
        StatusCode::CREATED
    );
    let token = last_token(&fakes)?;
    let full = adapters_gmail::scopes::GMAIL_SCOPES
        .iter()
        .map(|s| (*s).to_owned())
        .collect();
    assert_eq!(
        redeem_with(
            &router,
            &state,
            &fakes,
            &token,
            INVITEE,
            "sub-nogrant",
            full,
            false
        )
        .await?,
        "failed"
    );
    let stored = fakes
        .store
        .invites()
        .by_token_hash(&sha(&token))
        .await?
        .ok_or("the invite record survives")?;
    assert_eq!(
        stored.record.status,
        domain::InviteStatus::Pending,
        "a missing grant does not burn the invite"
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// AU-01 AC3: a non-admin is refused and it is logged.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn au_01_ac3_non_admin_invite_403_and_logged() -> TestResult {
    let (state, fakes, _serial) = fixture().await?;
    let router = build_router(state.clone());
    let user = seed_user(&fakes, false).await?;
    let who = session_for(&state, &user, true).await?;
    let clock = obs::arc(obs::FixedClock(fakes.clock.now()));
    let (capture, _guard) = obs::capture("api", clock);

    let resp = invite(&router, &who, INVITEE).await?;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    assert_eq!(code_of(resp).await?, "forbidden");
    let text = capture.text();
    assert!(text.contains("authz_failure"), "capture: {text}");
    assert_eq!(fakes.invite_mailer.sent().len(), 0);
    assert_eq!(count(&fakes, "invites"), 0);
    Ok(())
}

// ---------------------------------------------------------------------------
// AU-01 AC4: a revoked invite cannot be used to sign in.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn au_01_ac4_revoked_invite_sign_in_refused() -> TestResult {
    let (state, fakes, _serial) = fixture().await?;
    let router = build_router(state.clone());
    let admin = admin(&state, &fakes, "sub-admin", true).await?;

    let created = json_of(invite(&router, &admin, INVITEE).await?).await?;
    let token = last_token(&fakes)?;
    let id = created["invite_id"].as_str().ok_or("invite_id")?;
    let resp = call(
        &router,
        Method::DELETE,
        &format!("/api/v1/admin/invites/{id}"),
        &admin,
        None,
    )
    .await?;
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    assert_eq!(
        redeem(&router, &state, &fakes, &token, INVITEE, "sub-revoked").await?,
        "invite_invalid"
    );
    assert_eq!(count(&fakes, "users"), 1, "only the admin exists");
    Ok(())
}

// ---------------------------------------------------------------------------
// AU-01 AC5: every admin write needs step-up.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn au_01_ac5_each_admin_write_without_step_up_refused() -> TestResult {
    let (state, fakes, _serial) = fixture().await?;
    let router = build_router(state.clone());
    let fresh = admin(&state, &fakes, "sub-admin", true).await?;
    let no_step_up = {
        let user = seed_user(&fakes, true).await?;
        seed_mailbox(&state, &fakes, user, "sub-no_step_up").await?;
        session_for(&state, &user, false).await?
    };
    let created = json_of(invite(&router, &fresh, INVITEE).await?).await?;
    let invite_id = created["invite_id"].as_str().ok_or("invite_id")?.to_owned();
    let request_id = seed_request(&fakes, "asker@example.com").await?.0;
    let sent_before = fakes.invite_mailer.sent().len();

    let body = r#"{"email_address":"other@example.com"}"#;
    let cases = [
        (Method::POST, "/api/v1/admin/invites".to_owned(), Some(body)),
        (
            Method::POST,
            format!("/api/v1/admin/invites/{invite_id}/resend"),
            None,
        ),
        (
            Method::DELETE,
            format!("/api/v1/admin/invites/{invite_id}"),
            None,
        ),
        (
            Method::POST,
            format!("/api/v1/admin/invite-requests/{request_id}/approve"),
            None,
        ),
        (
            Method::POST,
            format!("/api/v1/admin/invite-requests/{request_id}/decline"),
            None,
        ),
    ];
    for (method, uri, body) in cases {
        let resp = call(&router, method.clone(), &uri, &no_step_up, body).await?;
        assert_eq!(resp.status(), StatusCode::FORBIDDEN, "{method} {uri}");
        assert_eq!(code_of(resp).await?, "step_up_required", "{method} {uri}");
    }
    assert_eq!(
        fakes.invite_mailer.sent().len(),
        sent_before,
        "nothing sent"
    );
    assert_eq!(count(&fakes, "invite_requests"), 1, "request untouched");
    assert_eq!(count(&fakes, "invites"), 1, "invite untouched");
    Ok(())
}

// ---------------------------------------------------------------------------
// AU-02 AC2: the admin sees requests with address and time.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn au_02_ac2_admin_lists_requests_with_address_and_time() -> TestResult {
    let (state, fakes, _serial) = fixture().await?;
    let router = build_router(state.clone());
    let admin = admin(&state, &fakes, "sub-admin", false).await?;

    let first = pending_session(&state, &fakes, "first@example.com").await?;
    assert_eq!(
        request_invite(&router, &first).await?.status(),
        StatusCode::ACCEPTED
    );
    fakes.clock.advance(Duration::minutes(1));
    let second = pending_session(&state, &fakes, "second@example.com").await?;
    assert_eq!(
        request_invite(&router, &second).await?.status(),
        StatusCode::ACCEPTED
    );

    let resp = call(
        &router,
        Method::GET,
        "/api/v1/admin/invite-requests",
        &admin,
        None,
    )
    .await?;
    assert_eq!(resp.status(), StatusCode::OK);
    let json = json_of(resp).await?;
    let requests = json["requests"].as_array().ok_or("requests")?;
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0]["email_address"], "first@example.com");
    assert_eq!(requests[1]["email_address"], "second@example.com");
    assert_eq!(requests[0]["created_at"], "2026-10-05T00:00:00Z");
    assert_eq!(requests[1]["created_at"], "2026-10-05T00:01:00Z");
    assert!(requests[0]["request_id"].is_string());
    assert!(json["next_cursor"].is_null());
    Ok(())
}

// ---------------------------------------------------------------------------
// AU-02 AC3: approve turns the request into an invite; decline is silent.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn au_02_ac3_approve_creates_invite_and_deletes_request() -> TestResult {
    let (state, fakes, _serial) = fixture().await?;
    let router = build_router(state.clone());
    let admin = admin(&state, &fakes, "sub-admin", true).await?;
    let id = seed_request(&fakes, INVITEE).await?;

    let resp = call(
        &router,
        Method::POST,
        &format!("/api/v1/admin/invite-requests/{}/approve", id.0),
        &admin,
        None,
    )
    .await?;
    assert_eq!(resp.status(), StatusCode::CREATED);
    let json = json_of(resp).await?;
    assert_eq!(json["email_address"], INVITEE);
    assert_eq!(json["status"], "pending");

    assert_eq!(count(&fakes, "invite_requests"), 0);
    let invites = fakes
        .store
        .invites()
        .by_email_lookup(&lookup(INVITEE)?)
        .await?;
    assert_eq!(invites.len(), 1);
    assert_eq!(fakes.invite_mailer.sent().len(), 1);
    assert_eq!(fakes.invite_mailer.sent()[0].0.as_str(), INVITEE);
    Ok(())
}

#[tokio::test]
async fn au_02_ac3_decline_deletes_request() -> TestResult {
    let (state, fakes, _serial) = fixture().await?;
    let router = build_router(state.clone());
    let admin = admin(&state, &fakes, "sub-admin", true).await?;
    let id = seed_request(&fakes, INVITEE).await?;

    let resp = call(
        &router,
        Method::POST,
        &format!("/api/v1/admin/invite-requests/{}/decline", id.0),
        &admin,
        None,
    )
    .await?;
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    assert!(fakes.store.invite_requests().get(&id).await?.is_none());
    assert!(
        fakes.invite_mailer.sent().is_empty(),
        "the requester is not told"
    );

    let again = call(
        &router,
        Method::POST,
        &format!("/api/v1/admin/invite-requests/{}/decline", id.0),
        &admin,
        None,
    )
    .await?;
    assert_eq!(again.status(), StatusCode::NOT_FOUND);
    Ok(())
}

// ---------------------------------------------------------------------------
// AU-02 AC4: the sixth request from one IP in an hour is limited.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn au_02_ac4_sixth_request_from_ip_in_hour_429() -> TestResult {
    let (state, fakes, _serial) = fixture().await?;
    let router = build_router(state.clone());
    for n in 0..5 {
        let who = pending_session(&state, &fakes, &format!("person{n}@example.com")).await?;
        assert_eq!(
            request_invite(&router, &who).await?.status(),
            StatusCode::ACCEPTED,
            "request {n}"
        );
    }
    let sixth = pending_session(&state, &fakes, "person5@example.com").await?;
    let resp = request_invite(&router, &sixth).await?;
    assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(code_of(resp).await?, "rate_limited");
    assert_eq!(count(&fakes, "invite_requests"), 5);
    Ok(())
}

// ---------------------------------------------------------------------------
// INV-T1: invite records hold only the token hash and a purge time.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn inv_t1_no_raw_token_in_store_dump() -> TestResult {
    let (state, fakes, _serial) = fixture().await?;
    let router = build_router(state.clone());
    let admin = admin(&state, &fakes, "sub-admin", true).await?;
    assert_eq!(
        invite(&router, &admin, INVITEE).await?.status(),
        StatusCode::CREATED
    );
    let token = last_token(&fakes)?;
    let dump = store_dump(&fakes)?;
    assert!(dump.contains("invites"));
    assert!(!dump.contains(&token), "raw token in the store");
    assert!(!dump.contains(INVITEE), "readable address in the store");
    Ok(())
}

#[tokio::test]
async fn inv_t1_purge_at_set_on_create_and_revoke() -> TestResult {
    let (state, fakes, _serial) = fixture().await?;
    let router = build_router(state.clone());
    let admin = admin(&state, &fakes, "sub-admin", true).await?;
    let start = fakes.clock.now();

    let created = json_of(invite(&router, &admin, INVITEE).await?).await?;
    let id = created["invite_id"].as_str().ok_or("invite_id")?;
    let token = last_token(&fakes)?;
    let stored = fakes
        .store
        .invites()
        .by_token_hash(&sha(&token))
        .await?
        .ok_or("invite")?;
    assert_eq!(
        stored.record.purge_at,
        start + Duration::days(7) + Duration::days(30)
    );

    fakes.clock.advance(Duration::minutes(2));
    let revoke_admin = session_for(&state, &stored_admin(&fakes).await?, true).await?;
    let resp = call(
        &router,
        Method::DELETE,
        &format!("/api/v1/admin/invites/{id}"),
        &revoke_admin,
        None,
    )
    .await?;
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    let revoked = fakes
        .store
        .invites()
        .by_token_hash(&sha(&token))
        .await?
        .ok_or("invite")?;
    assert_eq!(revoked.record.status, domain::InviteStatus::Revoked);
    assert_eq!(
        revoked.record.purge_at,
        fakes.clock.now() + Duration::days(30)
    );
    Ok(())
}

/// The seeded admin's user ID, for a second session after the clock moved.
async fn stored_admin(fakes: &testkit::Fakes) -> Result<UserId, Box<dyn std::error::Error>> {
    let page = fakes
        .store
        .users()
        .list(ports::PageRequest {
            limit: 10,
            after: None,
        })
        .await?;
    page.items
        .into_iter()
        .find(|u| u.record.is_admin)
        .map(|u| u.record.user_id)
        .ok_or_else(|| "no admin".into())
}

// ---------------------------------------------------------------------------
// INV-T2: a declined request leaves no record.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn inv_t2_decline_leaves_no_record() -> TestResult {
    let (state, fakes, _serial) = fixture().await?;
    let router = build_router(state.clone());
    let admin = admin(&state, &fakes, "sub-admin", true).await?;
    let id = seed_request(&fakes, INVITEE).await?;
    assert_eq!(count(&fakes, "invite_requests"), 1);

    let resp = call(
        &router,
        Method::POST,
        &format!("/api/v1/admin/invite-requests/{}/decline", id.0),
        &admin,
        None,
    )
    .await?;
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    assert_eq!(count(&fakes, "invite_requests"), 0);
    assert_eq!(count(&fakes, "invites"), 0, "decline makes no invite");
    assert!(fakes
        .store
        .invite_requests()
        .by_email_lookup(&lookup(INVITEE)?)
        .await?
        .is_none());
    Ok(())
}

// ---------------------------------------------------------------------------
// ASVS V6.4.1: 256-bit, single-use, hash-only, 7-day invite tokens.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn asvs_v6_4_1_invite_token_256_bits_hash_only() -> TestResult {
    let (state, fakes, _serial) = fixture().await?;
    let router = build_router(state.clone());
    let admin = admin(&state, &fakes, "sub-admin", true).await?;
    let now = fakes.clock.now();

    assert_eq!(
        invite(&router, &admin, INVITEE).await?.status(),
        StatusCode::CREATED
    );
    assert_eq!(
        invite(&router, &admin, "second@example.com")
            .await?
            .status(),
        StatusCode::CREATED
    );
    let sent = fakes.invite_mailer.sent();
    let first = token_of(&sent[0].1)?;
    let second = token_of(&sent[1].1)?;
    assert_ne!(first, second, "tokens are random per invite");
    assert_eq!(URL_SAFE_NO_PAD.decode(&first)?.len(), 32, "256 bits");

    let stored = fakes
        .store
        .invites()
        .by_token_hash(&sha(&first))
        .await?
        .ok_or("invite")?;
    assert_eq!(stored.record.expires_at, now + Duration::days(7));
    assert!(!store_dump(&fakes)?.contains(&first), "hash only");

    // Single use: the second redemption is refused.
    assert_eq!(
        redeem(&router, &state, &fakes, &first, INVITEE, "sub-one").await?,
        "joined"
    );
    assert_eq!(
        redeem(&router, &state, &fakes, &first, INVITEE, "sub-two").await?,
        "invite_invalid"
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// ASVS V2.4.1: admin invite sends are limited to 50 a day.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn asvs_v2_4_1_admin_invite_sends_limited_50_per_day() -> TestResult {
    let (state, fakes, _serial) = fixture().await?;
    let router = build_router(state.clone());
    let user = seed_user(&fakes, true).await?;
    seed_mailbox(&state, &fakes, user, "sub-admin").await?;
    let who = session_for(&state, &user, true).await?;
    let request_id = api::http::request_id::RequestId(fakes.rng.uuid_v4());
    // 49 sends already counted today; the route's own check is the 50th.
    for _ in 0..49 {
        state
            .limits
            .check(
                &api::limits::policies::ADMIN_INVITE_SENDS,
                api::limits::LimitSubject::User(&user),
                request_id,
            )
            .await?;
    }
    assert_eq!(
        invite(&router, &who, INVITEE).await?.status(),
        StatusCode::CREATED,
        "the 50th send passes"
    );
    let resp = invite(&router, &who, "next@example.com").await?;
    assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(code_of(resp).await?, "rate_limited");
    assert_eq!(fakes.invite_mailer.sent().len(), 1);
    Ok(())
}

// ---------------------------------------------------------------------------
// API-INV-1 answers the same whatever exists already.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn invite_request_same_response_whether_or_not_exists() -> TestResult {
    let (state, fakes, _serial) = fixture().await?;
    let router = build_router(state.clone());
    let admin = admin(&state, &fakes, "sub-admin", true).await?;
    assert_eq!(
        invite(&router, &admin, "invited@example.com")
            .await?
            .status(),
        StatusCode::CREATED
    );
    seed_request(&fakes, "asked@example.com").await?;
    let requests_before = count(&fakes, "invite_requests");

    let mut seen = Vec::new();
    for email in [
        "fresh@example.com",
        "asked@example.com",
        "invited@example.com",
    ] {
        let who = pending_session(&state, &fakes, email).await?;
        let resp = request_invite(&router, &who).await?;
        let status = resp.status();
        let content_type = resp.headers().get(header::CONTENT_TYPE).cloned();
        seen.push((status, content_type, body_string(resp).await?));
    }
    assert!(
        seen.iter().all(|s| s == &seen[0]),
        "responses differ: {seen:?}"
    );
    assert_eq!(seen[0].0, StatusCode::ACCEPTED);
    assert_eq!(seen[0].2, "");
    // Only the fresh address made a record.
    assert_eq!(count(&fakes, "invite_requests"), requests_before + 1);
    Ok(())
}

#[tokio::test]
async fn invite_request_needs_pending_state_401_otherwise() -> TestResult {
    let (state, fakes, _serial) = fixture().await?;
    let router = build_router(state.clone());

    let (cookie, created) = SessionService::new(&state).create_anonymous().await?;
    let anonymous = Authed {
        cookie: cookie_pair(&cookie.0)?,
        csrf: created.record.csrf_token.clone(),
    };
    assert_eq!(
        request_invite(&router, &anonymous).await?.status(),
        StatusCode::UNAUTHORIZED
    );

    let user = seed_user(&fakes, false).await?;
    let signed_in = session_for(&state, &user, true).await?;
    assert_eq!(
        request_invite(&router, &signed_in).await?.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(count(&fakes, "invite_requests"), 0);
    Ok(())
}

// ---------------------------------------------------------------------------
// A stored pending invite past its expiry is listed as expired.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn invite_list_shows_expired_for_pending_past_expiry() -> TestResult {
    let (state, fakes, _serial) = fixture().await?;
    let router = build_router(state.clone());
    let admin = admin(&state, &fakes, "sub-admin", true).await?;
    assert_eq!(
        invite(&router, &admin, INVITEE).await?.status(),
        StatusCode::CREATED
    );
    let token = last_token(&fakes)?;
    let stored = fakes
        .store
        .invites()
        .by_token_hash(&sha(&token))
        .await?
        .ok_or("invite")?;
    let mut record = stored.record.clone();
    record.expires_at = fakes.clock.now() - Duration::seconds(1);
    fakes
        .store
        .invites()
        .put(&record, Precondition::Matches(stored.version))
        .await?;

    let resp = call(&router, Method::GET, "/api/v1/admin/invites", &admin, None).await?;
    assert_eq!(resp.status(), StatusCode::OK);
    let json = json_of(resp).await?;
    assert_eq!(json["invites"][0]["status"], "expired");
    assert_eq!(json["invites"][0]["email_address"], INVITEE);
    assert_eq!(
        fakes
            .store
            .invites()
            .get(&record.invite_id)
            .await?
            .ok_or("invite")?
            .record
            .status,
        domain::InviteStatus::Pending,
        "the stored status is untouched"
    );
    Ok(())
}

#[tokio::test]
async fn invite_cursor_from_other_admin_or_tampered_is_400() -> TestResult {
    let (state, fakes, _serial) = fixture().await?;
    let router = build_router(state.clone());
    let one = admin(&state, &fakes, "sub-one", true).await?;
    let two = admin(&state, &fakes, "sub-two", true).await?;
    for email in ["a@example.com", "b@example.com", "c@example.com"] {
        assert_eq!(
            invite(&router, &one, email).await?.status(),
            StatusCode::CREATED
        );
    }

    let first = call(
        &router,
        Method::GET,
        "/api/v1/admin/invites?limit=1",
        &one,
        None,
    )
    .await?;
    assert_eq!(first.status(), StatusCode::OK);
    let page = json_of(first).await?;
    assert_eq!(page["invites"].as_array().ok_or("invites")?.len(), 1);
    let cursor = page["next_cursor"]
        .as_str()
        .ok_or("next_cursor")?
        .to_owned();

    let next = call(
        &router,
        Method::GET,
        &format!("/api/v1/admin/invites?limit=1&cursor={cursor}"),
        &one,
        None,
    )
    .await?;
    assert_eq!(next.status(), StatusCode::OK, "the owner can use it");

    let mut tampered = cursor.clone();
    tampered.push('A');
    for (who, bad) in [
        (&two, cursor.as_str()),
        (&one, tampered.as_str()),
        (&one, "x"),
    ] {
        let resp = call(
            &router,
            Method::GET,
            &format!("/api/v1/admin/invites?cursor={bad}"),
            who,
            None,
        )
        .await?;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        assert_eq!(code_of(resp).await?, "invalid_request");
    }
    let bad_limit = call(
        &router,
        Method::GET,
        "/api/v1/admin/invites?limit=51",
        &one,
        None,
    )
    .await?;
    assert_eq!(bad_limit.status(), StatusCode::BAD_REQUEST);
    Ok(())
}
