//! T-502b sign-in, join and invite-redemption integration tests.
//!
//! Everything runs through the real router with the fake identity provider,
//! fake store and keys, and the virtual clock.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::too_many_lines,
    clippy::similar_names,
    clippy::struct_excessive_bools
)]

use std::sync::Arc;

use api::auth::callback::discard_tokens;
use api::session::store::SessionService;
use api::state::AppState;
use api::{app_state, build_router, config::ApiConfig};
use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use axum::response::Response;
use axum::Router;
use domain::{EmailAddress, InviteStatus, MailboxStatus, Provider, ProviderSubjectId, UserId};
use obs::Sensitive;
use ports::store::aad_fields;
use ports::{
    Aad, Ciphertext, Clock, IdClaims, InviteId, InviteRecord, KeyService, MailboxRecord,
    Precondition, Rng, ServerStore, Sha256Hash, TokenSet, UserRecord,
};
use sha2::{Digest, Sha256};
use time::{Duration, OffsetDateTime};
use tower::ServiceExt;
use url::Url;

const ORIGIN: &str = "https://mailtinder.test";
const EMAIL_KEY: &[u8] = b"fake-email-key";
const INVITE_TOKEN: &str = "abcdefghijklmnopqrstuvwxyz0123456789ABCDEFG";

fn fixture() -> Result<(AppState, testkit::Fakes), Box<dyn std::error::Error>> {
    let (ports, fakes) = testkit::fake_ports();
    let config = ApiConfig::new(
        ORIGIN.to_owned(),
        "fake-client".into(),
        Sensitive::new(b"fake-log-key".to_vec()),
        Sensitive::new(EMAIL_KEY.to_vec()),
    )?;
    Ok((app_state(Arc::new(ports), Arc::new(config)), fakes))
}

fn cookie_value(header: &axum::http::HeaderValue) -> Result<String, Box<dyn std::error::Error>> {
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

async fn body_string(resp: Response) -> Result<String, Box<dyn std::error::Error>> {
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

/// A fresh anonymous `pre_auth` session: cookie value and CSRF token.
async fn anonymous(state: &AppState) -> Result<(String, String), Box<dyn std::error::Error>> {
    let (cookie, record) = SessionService::new(state).create_anonymous().await?;
    Ok((cookie_value(&cookie.0)?, record.record.csrf_token.clone()))
}

async fn start(
    router: &Router,
    cookie: &str,
    csrf: &str,
    body: &serde_json::Value,
) -> Result<Response, Box<dyn std::error::Error>> {
    let request = Request::builder()
        .method(Method::POST)
        .uri("/api/v1/auth/google/start")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::ORIGIN, ORIGIN)
        .header(header::COOKIE, cookie)
        .header("x-csrf-token", csrf)
        .body(Body::from(serde_json::to_vec(body)?))?;
    Ok(router.clone().oneshot(request).await?)
}

async fn start_provider(
    router: &Router,
    provider: &str,
    cookie: &str,
    csrf: &str,
    body: &serde_json::Value,
) -> Result<Response, Box<dyn std::error::Error>> {
    let request = Request::builder()
        .method(Method::POST)
        .uri(format!("/api/v1/auth/{provider}/start"))
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::ORIGIN, ORIGIN)
        .header(header::COOKIE, cookie)
        .header("x-csrf-token", csrf)
        .body(Body::from(serde_json::to_vec(body)?))?;
    Ok(router.clone().oneshot(request).await?)
}

async fn callback(
    router: &Router,
    cookie: &str,
    query: &str,
) -> Result<Response, Box<dyn std::error::Error>> {
    let mut builder = Request::builder()
        .method(Method::GET)
        .uri(format!("/api/v1/auth/google/callback?{query}"));
    if !cookie.is_empty() {
        builder = builder.header(header::COOKIE, cookie);
    }
    Ok(router.clone().oneshot(builder.body(Body::empty())?).await?)
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

fn authorize_state(url: &str) -> Result<String, Box<dyn std::error::Error>> {
    let parsed = Url::parse(url)?;
    parsed
        .query_pairs()
        .find(|(key, _)| key == "state")
        .map(|(_, value)| value.into_owned())
        .ok_or_else(|| "no state in authorization url".into())
}

/// The started round trip: the rotated cookie and the OAuth `state`.
struct Begun {
    cookie: String,
    state: String,
}

async fn begin(
    router: &Router,
    state: &AppState,
    body: &serde_json::Value,
) -> Result<Begun, Box<dyn std::error::Error>> {
    let (cookie, csrf) = anonymous(state).await?;
    let resp = start(router, &cookie, &csrf, body).await?;
    assert_eq!(resp.status(), StatusCode::OK);
    let cookie = set_cookie(&resp).ok_or("rotated cookie")?;
    let json: serde_json::Value = serde_json::from_str(&body_string(resp).await?)?;
    let url = json["authorization_url"]
        .as_str()
        .ok_or("authorization_url")?;
    Ok(Begun {
        cookie,
        state: authorize_state(url)?,
    })
}

fn tokens(refresh: bool) -> TokenSet {
    TokenSet {
        access_token: Sensitive::new("access-token-A".to_owned()),
        refresh_token: refresh.then(|| Sensitive::new("refresh-token-R".to_owned())),
        id_token: Sensitive::new("id-token-I".to_owned()),
        expires_in_s: 3600,
        granted_scopes: adapters_gmail::scopes::GMAIL_SCOPES
            .iter()
            .map(|scope| (*scope).to_owned())
            .collect(),
    }
}

fn claims(sub: &str, email: &str, verified: bool) -> IdClaims {
    let now = OffsetDateTime::from_unix_timestamp(1_700_000_000).expect("valid timestamp");
    IdClaims {
        sub: sub.to_owned(),
        email: Sensitive::new(email.to_owned()),
        email_verified: verified,
        auth_time: None,
        amr: vec![],
        issued_at: now,
        expires_at: now + Duration::hours(1),
    }
}

async fn seed_user(fakes: &testkit::Fakes) -> Result<UserId, Box<dyn std::error::Error>> {
    let user = UserId::new(fakes.rng.uuid_v4());
    let wrapped = fakes.keys.new_user_key(&user).await?;
    fakes
        .store
        .users()
        .put(
            &UserRecord {
                user_id: user,
                created_at: fakes.clock.now(),
                is_admin: false,
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
) -> Result<domain::MailboxId, Box<dyn std::error::Error>> {
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
    Ok(mailbox_id)
}

fn token_hash(token: &str) -> Sha256Hash {
    let digest = Sha256::digest(token.as_bytes());
    let mut out = [0u8; 32];
    out.copy_from_slice(&digest);
    Sha256Hash(out)
}

async fn seed_invite(
    fakes: &testkit::Fakes,
    token: &str,
    email: &str,
    status: InviteStatus,
    expires_in: Duration,
) -> Result<InviteId, Box<dyn std::error::Error>> {
    let invite_id = InviteId(fakes.rng.uuid_v4());
    let address = EmailAddress::parse(email)?;
    let lookup =
        api::auth::email_key::email_lookup_hash(&Sensitive::new(EMAIL_KEY.to_vec()), &address);
    let now = fakes.clock.now();
    fakes
        .store
        .invites()
        .put(
            &InviteRecord {
                invite_id,
                email_address: Ciphertext(vec![1, 2, 3]),
                email_lookup: lookup,
                token_hash: token_hash(token),
                status,
                created_at: now,
                last_sent_at: now,
                expires_at: now + expires_in,
                purge_at: now + Duration::days(30),
            },
            Precondition::MustNotExist,
        )
        .await?;
    Ok(invite_id)
}

fn dump(fakes: &testkit::Fakes) -> Result<String, Box<dyn std::error::Error>> {
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

fn join_body(token: Option<&str>) -> serde_json::Value {
    serde_json::json!({ "intent": "join", "invite_token": token, "mailbox_id": null })
}

fn sign_in_body() -> serde_json::Value {
    serde_json::json!({ "intent": "sign_in", "invite_token": null, "mailbox_id": null })
}

fn query(state: &str) -> String {
    format!("code=code-A&state={state}")
}

fn query_with_google_extras(state: &str) -> String {
    format!(
        "code=code-A&state={state}&scope=openid%20email&authuser=0&prompt=consent&hd=example.com"
    )
}

// ---------------------------------------------------------------------------
// AU-03 AC1: an invited person joins and lands on the Feed.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn au_03_ac1_invited_user_joins_and_lands_on_feed() -> Result<(), Box<dyn std::error::Error>>
{
    let (state, fakes) = fixture()?;
    let router = build_router(state.clone());
    let invite = seed_invite(
        &fakes,
        INVITE_TOKEN,
        "invited@example.com",
        InviteStatus::Pending,
        Duration::days(7),
    )
    .await?;
    let begun = begin(&router, &state, &join_body(Some(INVITE_TOKEN))).await?;
    fakes.identity.script_exchange(Ok(tokens(true)));
    fakes
        .identity
        .script_claims(Ok(claims("sub-invited", "invited@example.com", true)));
    let resp = callback(&router, &begun.cookie, &query(&begun.state)).await?;
    assert_eq!(resp.status(), StatusCode::FOUND);
    assert_eq!(outcome(&resp), "joined");
    assert_eq!(count(&fakes, "users"), 1, "one user");
    assert_eq!(count(&fakes, "mailboxes"), 1, "one primary mailbox");
    let record = fakes.store.invites().get(&invite).await?.ok_or("invite")?;
    assert_eq!(record.record.status, InviteStatus::Used);
    Ok(())
}

// ---------------------------------------------------------------------------
// AU-03 AC2: a different, unlinked account is refused and its tokens revoked.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn au_03_ac2_email_mismatch_refused_and_tokens_revoked(
) -> Result<(), Box<dyn std::error::Error>> {
    let (state, fakes) = fixture()?;
    let router = build_router(state.clone());
    seed_invite(
        &fakes,
        INVITE_TOKEN,
        "invited@example.com",
        InviteStatus::Pending,
        Duration::days(7),
    )
    .await?;
    let begun = begin(&router, &state, &join_body(Some(INVITE_TOKEN))).await?;
    fakes.identity.script_exchange(Ok(tokens(true)));
    fakes
        .identity
        .script_claims(Ok(claims("sub-other", "other@example.com", true)));
    let resp = callback(&router, &begun.cookie, &query(&begun.state)).await?;
    assert_eq!(outcome(&resp), "email_mismatch");
    assert_eq!(count(&fakes, "users"), 0, "no user created");
    assert!(
        !fakes.identity.revoked().is_empty(),
        "the new grant was revoked"
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// AU-03 AC3: an unverified email is refused.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn au_03_ac3_unverified_email_refused() -> Result<(), Box<dyn std::error::Error>> {
    let (state, fakes) = fixture()?;
    let router = build_router(state.clone());
    seed_invite(
        &fakes,
        INVITE_TOKEN,
        "invited@example.com",
        InviteStatus::Pending,
        Duration::days(7),
    )
    .await?;
    let begun = begin(&router, &state, &join_body(Some(INVITE_TOKEN))).await?;
    fakes.identity.script_exchange(Ok(tokens(true)));
    fakes
        .identity
        .script_claims(Ok(claims("sub-invited", "invited@example.com", false)));
    let resp = callback(&router, &begun.cookie, &query(&begun.state)).await?;
    assert_eq!(outcome(&resp), "email_unverified");
    assert_eq!(count(&fakes, "users"), 0);
    Ok(())
}

// ---------------------------------------------------------------------------
// AU-03 AC4: an existing user signs in with any linked Gmail mailbox.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn au_03_ac4_existing_user_signs_in_with_second_linked_mailbox(
) -> Result<(), Box<dyn std::error::Error>> {
    let (state, fakes) = fixture()?;
    let router = build_router(state.clone());
    let user = seed_user(&fakes).await?;
    seed_mailbox(&fakes, user, "sub-first", "first@example.com").await?;
    seed_mailbox(&fakes, user, "sub-second", "second@example.com").await?;
    let begun = begin(&router, &state, &sign_in_body()).await?;
    fakes.identity.script_exchange(Ok(tokens(true)));
    fakes
        .identity
        .script_claims(Ok(claims("sub-second", "second@example.com", true)));
    let resp = callback(&router, &begun.cookie, &query(&begun.state)).await?;
    assert_eq!(outcome(&resp), "signed_in");
    assert!(set_cookie(&resp).is_some(), "a new session cookie");
    Ok(())
}

// ---------------------------------------------------------------------------
// AU-03 AC6: a join with no token reaches the request-invite state.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn au_03_ac6_missing_token_with_matching_email_is_request_state_not_join(
) -> Result<(), Box<dyn std::error::Error>> {
    let (state, fakes) = fixture()?;
    let router = build_router(state.clone());
    let begun = begin(&router, &state, &join_body(None)).await?;
    fakes.identity.script_exchange(Ok(tokens(true)));
    fakes
        .identity
        .script_claims(Ok(claims("sub-new", "new@example.com", true)));
    let resp = callback(&router, &begun.cookie, &query(&begun.state)).await?;
    assert_eq!(outcome(&resp), "not_invited");
    assert_eq!(count(&fakes, "users"), 0);
    let cookie = set_cookie(&resp).ok_or("pending cookie")?;
    let loaded = SessionService::new(&state)
        .load(&cookie_headers(&cookie))
        .await?
        .ok_or("pending session")?;
    assert_eq!(
        loaded.record.record.state,
        ports::SessionState::PendingInviteRequest
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// AU-03 AC6: used, expired and replaced tokens are all refused.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn au_03_ac6_used_token_refused() -> Result<(), Box<dyn std::error::Error>> {
    refuse_token(
        InviteStatus::Used,
        Duration::days(7),
        INVITE_TOKEN,
        INVITE_TOKEN,
    )
    .await
}

#[tokio::test]
async fn au_03_ac6_expired_token_refused() -> Result<(), Box<dyn std::error::Error>> {
    refuse_token(
        InviteStatus::Pending,
        Duration::seconds(-1),
        INVITE_TOKEN,
        INVITE_TOKEN,
    )
    .await
}

#[tokio::test]
async fn au_03_ac6_replaced_token_refused() -> Result<(), Box<dyn std::error::Error>> {
    const REPLACEMENT: &str = "zabcdefghijklmnopqrstuvwxyz0123456789ABCDEF";
    refuse_token(
        InviteStatus::Pending,
        Duration::days(7),
        REPLACEMENT,
        INVITE_TOKEN,
    )
    .await
}

/// Seed an invite under `stored` but present `presented`, expecting refusal.
async fn refuse_token(
    status: InviteStatus,
    expires_in: Duration,
    stored: &str,
    presented: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let (state, fakes) = fixture()?;
    let router = build_router(state.clone());
    seed_invite(&fakes, stored, "invited@example.com", status, expires_in).await?;
    let begun = begin(&router, &state, &join_body(Some(presented))).await?;
    fakes.identity.script_exchange(Ok(tokens(true)));
    fakes
        .identity
        .script_claims(Ok(claims("sub-invited", "invited@example.com", true)));
    let resp = callback(&router, &begun.cookie, &query(&begun.state)).await?;
    assert_eq!(outcome(&resp), "invite_invalid");
    assert_eq!(count(&fakes, "users"), 0);
    Ok(())
}

// ---------------------------------------------------------------------------
// AU-03 AC7 / V10.5.2: the mailbox is keyed by Google `sub`.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn au_03_ac7_mailbox_id_derived_from_sub() -> Result<(), Box<dyn std::error::Error>> {
    let (state, fakes) = fixture()?;
    let router = build_router(state.clone());
    seed_invite(
        &fakes,
        INVITE_TOKEN,
        "invited@example.com",
        InviteStatus::Pending,
        Duration::days(7),
    )
    .await?;
    let begun = begin(&router, &state, &join_body(Some(INVITE_TOKEN))).await?;
    fakes.identity.script_exchange(Ok(tokens(true)));
    fakes
        .identity
        .script_claims(Ok(claims("sub-xyz", "invited@example.com", true)));
    let resp = callback(&router, &begun.cookie, &query(&begun.state)).await?;
    assert_eq!(outcome(&resp), "joined");
    let expected = ports::mailbox_id_for(Provider::Gmail, &ProviderSubjectId::new("sub-xyz")?);
    assert!(
        fakes.store.mailboxes().get(&expected).await?.is_some(),
        "mailbox id derives from sub"
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// AU-02 AC1: an uninvited person reaches the request state; no token is kept.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn au_02_ac1_uninvited_reaches_pending_state_and_no_token_kept(
) -> Result<(), Box<dyn std::error::Error>> {
    let (state, fakes) = fixture()?;
    let router = build_router(state.clone());
    let begun = begin(&router, &state, &join_body(None)).await?;
    fakes.identity.script_exchange(Ok(tokens(true)));
    fakes
        .identity
        .script_claims(Ok(claims("sub-uninvited", "nobody@example.com", true)));
    let resp = callback(&router, &begun.cookie, &query(&begun.state)).await?;
    assert_eq!(outcome(&resp), "not_invited");
    assert_eq!(count(&fakes, "mailboxes"), 0, "no mailbox token kept");
    assert!(
        !fakes.identity.revoked().is_empty(),
        "no token is kept for an uninvited person"
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// AU-01 AC4: a revoked invite cannot be redeemed.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn au_01_ac4_revoked_invite_refused() -> Result<(), Box<dyn std::error::Error>> {
    refuse_token(
        InviteStatus::Revoked,
        Duration::days(7),
        INVITE_TOKEN,
        INVITE_TOKEN,
    )
    .await
}

// ---------------------------------------------------------------------------
// V2.3.4 / V6.4.1: an invite is claimed with a conditional write.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn asvs_v2_3_4_two_concurrent_redemptions_one_wins() -> Result<(), Box<dyn std::error::Error>>
{
    let (state, fakes) = fixture()?;
    let router = build_router(state.clone());
    seed_invite(
        &fakes,
        INVITE_TOKEN,
        "invited@example.com",
        InviteStatus::Pending,
        Duration::days(7),
    )
    .await?;
    let first = begin(&router, &state, &join_body(Some(INVITE_TOKEN))).await?;
    let second = begin(&router, &state, &join_body(Some(INVITE_TOKEN))).await?;
    fakes.identity.script_exchange(Ok(tokens(true)));
    fakes
        .identity
        .script_claims(Ok(claims("sub-one", "invited@example.com", true)));
    fakes.identity.script_exchange(Ok(tokens(true)));
    fakes
        .identity
        .script_claims(Ok(claims("sub-two", "invited@example.com", true)));
    let first = callback(&router, &first.cookie, &query(&first.state)).await?;
    assert_eq!(outcome(&first), "joined");
    let second = callback(&router, &second.cookie, &query(&second.state)).await?;
    assert_eq!(outcome(&second), "invite_invalid");
    assert_eq!(count(&fakes, "users"), 1, "exactly one user");
    assert_eq!(count(&fakes, "mailboxes"), 1);
    Ok(())
}

#[tokio::test]
async fn asvs_v6_4_1_invite_single_use() -> Result<(), Box<dyn std::error::Error>> {
    let (state, fakes) = fixture()?;
    let router = build_router(state.clone());
    seed_invite(
        &fakes,
        INVITE_TOKEN,
        "invited@example.com",
        InviteStatus::Pending,
        Duration::days(7),
    )
    .await?;
    let first = begin(&router, &state, &join_body(Some(INVITE_TOKEN))).await?;
    fakes.identity.script_exchange(Ok(tokens(true)));
    fakes
        .identity
        .script_claims(Ok(claims("sub-one", "invited@example.com", true)));
    let first = callback(&router, &first.cookie, &query(&first.state)).await?;
    assert_eq!(outcome(&first), "joined");
    let again = begin(&router, &state, &join_body(Some(INVITE_TOKEN))).await?;
    fakes.identity.script_exchange(Ok(tokens(true)));
    fakes
        .identity
        .script_claims(Ok(claims("sub-two", "invited@example.com", true)));
    let again = callback(&router, &again.cookie, &query(&again.state)).await?;
    assert_eq!(outcome(&again), "invite_invalid");
    Ok(())
}

// ---------------------------------------------------------------------------
// V3.7.2 / V14.2.1: the redirect carries the outcome only.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn asvs_v3_7_2_callback_redirects_only_to_app_origin(
) -> Result<(), Box<dyn std::error::Error>> {
    let (state, fakes) = fixture()?;
    let router = build_router(state.clone());
    let begun = begin(&router, &state, &join_body(None)).await?;
    fakes.identity.script_exchange(Ok(tokens(true)));
    fakes
        .identity
        .script_claims(Ok(claims("sub-x", "x@example.com", true)));
    let resp = callback(&router, &begun.cookie, &query(&begun.state)).await?;
    let location = resp
        .headers()
        .get(header::LOCATION)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    assert!(
        location.starts_with(&format!("{ORIGIN}/#/auth/result?outcome=")),
        "fixed app origin: {location}"
    );
    Ok(())
}

#[tokio::test]
async fn asvs_v14_2_1_redirect_has_outcome_only() -> Result<(), Box<dyn std::error::Error>> {
    let (state, fakes) = fixture()?;
    let router = build_router(state.clone());
    let begun = begin(&router, &state, &join_body(None)).await?;
    fakes.identity.script_exchange(Ok(tokens(true)));
    fakes
        .identity
        .script_claims(Ok(claims("sub-x", "x@example.com", true)));
    let resp = callback(&router, &begun.cookie, &query(&begun.state)).await?;
    let location = resp
        .headers()
        .get(header::LOCATION)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    assert_eq!(
        location,
        format!("{ORIGIN}/#/auth/result?outcome=not_invited")
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// V6.3.1: rate limits per IP and per Google subject.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn asvs_v6_3_1_sign_in_ip_limit() -> Result<(), Box<dyn std::error::Error>> {
    let (state, fakes) = fixture()?;
    let router = build_router(state.clone());
    for _ in 0..20 {
        let (cookie, csrf) = anonymous(&state).await?;
        let resp = start(&router, &cookie, &csrf, &sign_in_body()).await?;
        assert_eq!(resp.status(), StatusCode::OK);
    }
    let (cookie, csrf) = anonymous(&state).await?;
    let resp = start(&router, &cookie, &csrf, &sign_in_body()).await?;
    assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);
    let _ = fakes;
    Ok(())
}

#[tokio::test]
async fn asvs_v6_3_1_redemption_attempts_per_subject_limited(
) -> Result<(), Box<dyn std::error::Error>> {
    let (state, fakes) = fixture()?;
    let router = build_router(state.clone());
    seed_invite(
        &fakes,
        INVITE_TOKEN,
        "invited@example.com",
        InviteStatus::Pending,
        Duration::days(7),
    )
    .await?;
    let mut last = String::new();
    for attempt in 0..11 {
        // A distinct client IP per attempt keeps the per-IP policy out of the
        // way so the per-subject counter is what is exercised.
        let forwarded = format!("10.0.{attempt}.7");
        let (cookie, csrf) = anonymous(&state).await?;
        let started = Request::builder()
            .method(Method::POST)
            .uri("/api/v1/auth/google/start")
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::ORIGIN, ORIGIN)
            .header(header::COOKIE, &cookie)
            .header("x-csrf-token", &csrf)
            .header("x-forwarded-for", &forwarded)
            .body(Body::from(serde_json::to_vec(&join_body(Some(
                INVITE_TOKEN,
            )))?))?;
        let started = router.clone().oneshot(started).await?;
        assert_eq!(started.status(), StatusCode::OK);
        let rotated = set_cookie(&started).ok_or("rotated cookie")?;
        let json: serde_json::Value = serde_json::from_str(&body_string(started).await?)?;
        let url = json["authorization_url"]
            .as_str()
            .ok_or("authorization_url")?;
        let callback_state = authorize_state(url)?;
        fakes.identity.script_exchange(Ok(tokens(true)));
        // The mismatching email keeps the subject unlinked, so every attempt
        // reaches the per-subject counter.
        fakes
            .identity
            .script_claims(Ok(claims("sub-limit", "other@example.com", true)));
        let request = Request::builder()
            .method(Method::GET)
            .uri(format!(
                "/api/v1/auth/google/callback?code=code-A&state={callback_state}"
            ))
            .header(header::COOKIE, &rotated)
            .header("x-forwarded-for", &forwarded)
            .body(Body::empty())?;
        let resp = router.clone().oneshot(request).await?;
        last = outcome(&resp);
    }
    assert_eq!(last, "failed", "the eleventh attempt is refused");
    Ok(())
}

// ---------------------------------------------------------------------------
// V6.3.3: `amr` is recorded with the sign-in event when present.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn asvs_v6_3_3_amr_recorded_when_present() -> Result<(), Box<dyn std::error::Error>> {
    let (state, fakes) = fixture()?;
    let router = build_router(state.clone());
    let user = seed_user(&fakes).await?;
    seed_mailbox(&fakes, user, "sub-amr", "amr@example.com").await?;
    let begun = begin(&router, &state, &sign_in_body()).await?;
    let mut claims = claims("sub-amr", "amr@example.com", true);
    claims.amr = vec!["pwd".to_owned(), "mfa".to_owned()];
    fakes.identity.script_exchange(Ok(tokens(true)));
    fakes.identity.script_claims(Ok(claims));
    let clock = obs::arc(obs::FixedClock(fakes.clock.now()));
    let (capture, _guard) = obs::capture("api", clock);
    let resp = callback(&router, &begun.cookie, &query(&begun.state)).await?;
    assert_eq!(outcome(&resp), "signed_in");
    let text = capture.text();
    assert!(text.contains("mfa"), "amr recorded: {text}");
    Ok(())
}

// ---------------------------------------------------------------------------
// V6.3.4: only the Google provider is accepted.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn asvs_v6_3_4_only_google_provider_accepted() -> Result<(), Box<dyn std::error::Error>> {
    let (state, _fakes) = fixture()?;
    let router = build_router(state.clone());
    let (cookie, csrf) = anonymous(&state).await?;
    let resp = start_provider(&router, "microsoft", &cookie, &csrf, &sign_in_body()).await?;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    Ok(())
}

// ---------------------------------------------------------------------------
// V6.8.1 / AU-03 AC7: a linked account signs in whatever the email says.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn asvs_v6_8_1_linked_sub_signs_in_regardless_of_email_change(
) -> Result<(), Box<dyn std::error::Error>> {
    let (state, fakes) = fixture()?;
    let router = build_router(state.clone());
    let user = seed_user(&fakes).await?;
    seed_mailbox(&fakes, user, "sub-stable", "old@example.com").await?;
    let begun = begin(&router, &state, &sign_in_body()).await?;
    fakes.identity.script_exchange(Ok(tokens(true)));
    fakes
        .identity
        .script_claims(Ok(claims("sub-stable", "renamed@example.com", true)));
    let resp = callback(&router, &begun.cookie, &query(&begun.state)).await?;
    assert_eq!(outcome(&resp), "signed_in");
    Ok(())
}

// ---------------------------------------------------------------------------
// V7.6.2: a callback without a started flow creates no session.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn asvs_v7_6_2_callback_without_started_flow_creates_no_session(
) -> Result<(), Box<dyn std::error::Error>> {
    let (state, fakes) = fixture()?;
    let router = build_router(state.clone());
    let resp = callback(&router, "", &query("never-started")).await?;
    assert_eq!(resp.status(), StatusCode::FOUND);
    assert_eq!(outcome(&resp), "failed");
    assert_eq!(count(&fakes, "sessions"), 0, "no session created");
    Ok(())
}

// ---------------------------------------------------------------------------
// V10.1.1: no provider token reaches the browser.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn asvs_v10_1_1_no_token_in_any_response_or_cookie() -> Result<(), Box<dyn std::error::Error>>
{
    let (state, fakes) = fixture()?;
    let router = build_router(state.clone());
    seed_invite(
        &fakes,
        INVITE_TOKEN,
        "invited@example.com",
        InviteStatus::Pending,
        Duration::days(7),
    )
    .await?;
    let (cookie, csrf) = anonymous(&state).await?;
    let started = start(&router, &cookie, &csrf, &join_body(Some(INVITE_TOKEN))).await?;
    let started_cookie = set_cookie(&started).ok_or("cookie")?;
    let started_headers = format!("{:?}", started.headers());
    let body: serde_json::Value = serde_json::from_str(&body_string(started).await?)?;
    let url = body["authorization_url"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    let begun = Begun {
        cookie: started_cookie,
        state: authorize_state(&url)?,
    };
    fakes.identity.script_exchange(Ok(tokens(true)));
    fakes
        .identity
        .script_claims(Ok(claims("sub-leak", "invited@example.com", true)));
    let resp = callback(&router, &begun.cookie, &query(&begun.state)).await?;
    let mut surface = format!("{} {:?} {:?}", url, started_headers, resp.headers());
    let location = resp
        .headers()
        .get(header::LOCATION)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    surface.push_str(&location);
    for secret in ["refresh-token-R", "access-token-A", "id-token-I"] {
        assert!(!surface.contains(secret), "leaked {secret} in {surface}");
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// V10.1.2: `state`, `nonce` and the verifier are sealed, never stored clear.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn asvs_v10_1_2_state_nonce_verifier_random_and_sealed(
) -> Result<(), Box<dyn std::error::Error>> {
    let (state, fakes) = fixture()?;
    let router = build_router(state.clone());
    let begun = begin(&router, &state, &sign_in_body()).await?;
    let stored = dump(&fakes)?;
    assert!(
        !stored.contains(&begun.state),
        "the state is not stored in the clear"
    );
    let second = begin(&router, &state, &sign_in_body()).await?;
    assert_ne!(begun.state, second.state, "state is random per round trip");
    Ok(())
}

// ---------------------------------------------------------------------------
// V10.2.2: a `state` mismatch fails and is logged.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn asvs_v10_2_2_state_mismatch_failed_and_logged() -> Result<(), Box<dyn std::error::Error>> {
    let (state, fakes) = fixture()?;
    let router = build_router(state.clone());
    let begun = begin(&router, &state, &sign_in_body()).await?;
    let clock = obs::arc(obs::FixedClock(fakes.clock.now()));
    let (capture, _guard) = obs::capture("api", clock);
    let resp = callback(&router, &begun.cookie, &query("not-the-state")).await?;
    assert_eq!(outcome(&resp), "failed");
    let text = capture.text();
    assert!(text.contains("state_invalid"), "logged: {text}");
    Ok(())
}

// ---------------------------------------------------------------------------
// V10.5.2: users are keyed by Google `sub`.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn asvs_v10_5_2_user_keyed_by_sub() -> Result<(), Box<dyn std::error::Error>> {
    let (state, fakes) = fixture()?;
    let router = build_router(state.clone());
    seed_invite(
        &fakes,
        INVITE_TOKEN,
        "invited@example.com",
        InviteStatus::Pending,
        Duration::days(7),
    )
    .await?;
    let begun = begin(&router, &state, &join_body(Some(INVITE_TOKEN))).await?;
    fakes.identity.script_exchange(Ok(tokens(true)));
    fakes
        .identity
        .script_claims(Ok(claims("sub-keyed", "invited@example.com", true)));
    let resp = callback(&router, &begun.cookie, &query(&begun.state)).await?;
    assert_eq!(outcome(&resp), "joined");
    let mailbox_id = ports::mailbox_id_for(Provider::Gmail, &ProviderSubjectId::new("sub-keyed")?);
    let mailbox = fakes
        .store
        .mailboxes()
        .get(&mailbox_id)
        .await?
        .ok_or("mailbox")?;
    assert_eq!(mailbox.record.provider_subject_id.as_str(), "sub-keyed");
    assert!(
        fakes
            .store
            .users()
            .get(&mailbox.record.user_id)
            .await?
            .is_some(),
        "the mailbox belongs to a real user"
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// V16.3.1: every sign-in success and failure is logged.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn asvs_v16_3_1_sign_in_success_and_failure_logged() -> Result<(), Box<dyn std::error::Error>>
{
    let (state, fakes) = fixture()?;
    let router = build_router(state.clone());
    let user = seed_user(&fakes).await?;
    seed_mailbox(&fakes, user, "sub-log", "log@example.com").await?;
    let clock = obs::arc(obs::FixedClock(fakes.clock.now()));
    let (capture, _guard) = obs::capture("api", clock);

    let good = begin(&router, &state, &sign_in_body()).await?;
    fakes.identity.script_exchange(Ok(tokens(true)));
    fakes
        .identity
        .script_claims(Ok(claims("sub-log", "log@example.com", true)));
    let good = callback(&router, &good.cookie, &query(&good.state)).await?;
    assert_eq!(outcome(&good), "signed_in");

    let missing = begin(&router, &state, &sign_in_body()).await?;
    fakes
        .identity
        .script_claims(Ok(claims("sub-none", "none@example.com", true)));
    fakes.identity.script_exchange(Ok(tokens(true)));
    let bad = callback(&router, &missing.cookie, &query(&missing.state)).await?;
    assert_eq!(outcome(&bad), "not_registered");

    let text = capture.text();
    assert!(
        text.contains("\"outcome\":\"signed_in\""),
        "success: {text}"
    );
    assert!(
        text.contains("\"outcome\":\"not_registered\""),
        "failure: {text}"
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// SES-1: the callback is single use; a replayed `state` fails.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn ses_1_pre_auth_cleared_after_callback() -> Result<(), Box<dyn std::error::Error>> {
    let (state, fakes) = fixture()?;
    let router = build_router(state.clone());
    let user = seed_user(&fakes).await?;
    seed_mailbox(&fakes, user, "sub-replay", "replay@example.com").await?;
    let begun = begin(&router, &state, &sign_in_body()).await?;
    fakes.identity.script_exchange(Ok(tokens(true)));
    fakes
        .identity
        .script_claims(Ok(claims("sub-replay", "replay@example.com", true)));
    let first = callback(&router, &begun.cookie, &query(&begun.state)).await?;
    assert_eq!(outcome(&first), "signed_in");
    let replay = callback(&router, &begun.cookie, &query(&begun.state)).await?;
    assert_eq!(outcome(&replay), "failed");
    Ok(())
}

// ---------------------------------------------------------------------------
// A linked subject never has its grant revoked.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn sign_in_linked_account_tokens_never_revoked_on_failure(
) -> Result<(), Box<dyn std::error::Error>> {
    let (state, fakes) = fixture()?;
    discard_tokens(&state, &tokens(true), true).await;
    assert_eq!(
        fakes.identity.revoked().len(),
        0,
        "a linked subject is never revoked"
    );
    discard_tokens(&state, &tokens(true), false).await;
    assert_eq!(fakes.identity.revoked().len(), 1, "an unlinked one is");
    Ok(())
}

// ---------------------------------------------------------------------------
// Google's extra callback parameters are accepted.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn sign_in_callback_accepts_google_extra_query_params(
) -> Result<(), Box<dyn std::error::Error>> {
    let (state, fakes) = fixture()?;
    let router = build_router(state.clone());
    let user = seed_user(&fakes).await?;
    seed_mailbox(&fakes, user, "sub-extra", "extra@example.com").await?;
    let begun = begin(&router, &state, &sign_in_body()).await?;
    fakes.identity.script_exchange(Ok(tokens(true)));
    fakes
        .identity
        .script_claims(Ok(claims("sub-extra", "extra@example.com", true)));
    let resp = callback(
        &router,
        &begun.cookie,
        &query_with_google_extras(&begun.state),
    )
    .await?;
    assert_eq!(resp.status(), StatusCode::FOUND, "accepted, not a 400");
    assert_eq!(outcome(&resp), "signed_in");
    Ok(())
}

fn cookie_headers(value: &str) -> axum::http::HeaderMap {
    let mut headers = axum::http::HeaderMap::new();
    headers.insert(
        header::COOKIE,
        axum::http::HeaderValue::from_str(value).expect("cookie header"),
    );
    headers
}
