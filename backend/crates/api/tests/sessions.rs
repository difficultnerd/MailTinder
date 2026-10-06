//! T-501 session service, cookie, CSRF and extractor integration tests.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::too_many_lines,
    clippy::similar_names
)]

use std::sync::Arc;

use api::session::cookie::{clear_cookie, read_cookie, RawSessionId, SESSION_COOKIE};
use api::session::csrf::{tokens_equal, CSRF_HEADER};
use api::session::extract::{AdminSession, AnySession, AuthedSession, PendingInviteSession};
use api::session::pre_auth::{open_pre_auth, seal_pre_auth, PreAuthPlain};
use api::session::store::{EndReason, LoadedSession, SessionService, IDLE_TIMEOUT, TOUCH_INTERVAL};
use api::state::AppState;
use api::{app_state, build_router_with_routes, config::ApiConfig, http::request_id::RequestId};
use axum::body::Body;
use axum::http::{header, HeaderMap, HeaderValue, Method, Request, StatusCode};
use axum::response::Response;
use axum::routing::{get, post};
use axum::Router;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use domain::UserId;
use obs::Sensitive;
use ports::{
    AuthIntent, Clock, KeyService, Precondition, Rng, ServerStore, SessionHash, SessionRecord,
    SessionRecordId, SessionState, UserRecord, Versioned,
};
use time::Duration;
use tower::ServiceExt;

const ORIGIN: &str = "https://mailtinder.test";

fn fixture() -> Result<(AppState, testkit::Fakes), Box<dyn std::error::Error>> {
    let (ports, fakes) = testkit::fake_ports();
    let config = ApiConfig::new(
        ORIGIN.to_owned(),
        "fake-client".into(),
        Sensitive::new(b"fake-log-key".to_vec()),
        Sensitive::new(b"fake-email-key".to_vec()),
    )?;
    Ok((app_state(Arc::new(ports), Arc::new(config)), fakes))
}

async fn me(_: AuthedSession) -> StatusCode {
    StatusCode::OK
}
async fn write(_: AuthedSession) -> StatusCode {
    StatusCode::NO_CONTENT
}
async fn admin(_: AdminSession) -> StatusCode {
    StatusCode::OK
}
async fn any(_: AnySession) -> StatusCode {
    StatusCode::OK
}
async fn pending(_: PendingInviteSession) -> StatusCode {
    StatusCode::OK
}

fn test_router(state: AppState) -> Router {
    build_router_with_routes(state, |r| {
        r.route("/api/v1/__t/me", get(me))
            .route("/api/v1/__t/write", post(write))
            .route("/api/v1/__t/admin", get(admin))
            .route("/api/v1/__t/any", get(any))
            .route("/api/v1/__t/pending", get(pending))
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

fn cookie_value(header: &HeaderValue) -> Result<String, Box<dyn std::error::Error>> {
    Ok(header
        .to_str()?
        .split(';')
        .next()
        .unwrap_or_default()
        .to_owned())
}

fn cookie_headers(value: &str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(header::COOKIE, HeaderValue::from_str(value).unwrap());
    headers
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

async fn establish(
    state: &AppState,
    user: &UserId,
    current: Option<&LoadedSession>,
) -> Result<(String, Versioned<SessionRecord>), Box<dyn std::error::Error>> {
    let (cookie, record) = SessionService::new(state)
        .establish(current, user, None)
        .await?;
    Ok((cookie_value(&cookie.0)?, record))
}

async fn load(
    state: &AppState,
    cookie: &str,
) -> Result<Option<LoadedSession>, Box<dyn std::error::Error>> {
    Ok(SessionService::new(state)
        .load(&cookie_headers(cookie))
        .await?)
}

struct Authed {
    cookie: String,
    csrf: String,
    hash: SessionHash,
    user: UserId,
}

async fn authed(
    state: &AppState,
    fakes: &testkit::Fakes,
    is_admin: bool,
) -> Result<Authed, Box<dyn std::error::Error>> {
    let user = seed_user(fakes, is_admin).await?;
    let (cookie, record) = establish(state, &user, None).await?;
    Ok(Authed {
        cookie,
        csrf: record.record.csrf_token.clone(),
        hash: record.record.session_hash,
        user,
    })
}

// ---------- Cookie and IDs ----------

#[tokio::test]
async fn au_03_ac5_cookie_attributes_exact() -> Result<(), Box<dyn std::error::Error>> {
    let (state, _) = fixture()?;
    let (cookie, _) = SessionService::new(&state).create_anonymous().await?;
    let raw = cookie.0.to_str()?;
    assert!(raw.starts_with(&format!("{SESSION_COOKIE}=")));
    assert!(raw.contains("Path=/"));
    assert!(raw.contains("Secure"));
    assert!(raw.contains("HttpOnly"));
    assert!(raw.contains("SameSite=Lax"));
    assert!(!raw.contains("Domain"));
    assert!(!raw.contains("Max-Age"));
    assert!(!raw.contains("Expires"));
    Ok(())
}

#[tokio::test]
async fn au_03_ac5_id_rotates_at_sign_in() -> Result<(), Box<dyn std::error::Error>> {
    let (state, fakes) = fixture()?;
    let user = seed_user(&fakes, false).await?;
    let (anon_cookie, _) = SessionService::new(&state).create_anonymous().await?;
    let anon = cookie_value(&anon_cookie.0)?;
    let current = load(&state, &anon).await?.expect("anonymous session");
    let (cookie, _) = establish(&state, &user, Some(&current)).await?;
    assert_ne!(anon, cookie);
    let router = test_router(state.clone());
    assert_eq!(
        call(&router, Method::GET, "/api/v1/__t/me", &[("cookie", &anon)])
            .await?
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        call(
            &router,
            Method::GET,
            "/api/v1/__t/me",
            &[("cookie", &cookie)]
        )
        .await?
        .status(),
        StatusCode::OK
    );
    Ok(())
}

#[tokio::test]
async fn au_03_ac5_idle_timeout_15_minutes() -> Result<(), Box<dyn std::error::Error>> {
    let (state, fakes) = fixture()?;
    let (a, _) = authed_session(&state, &fakes).await?;
    let (b, _) = authed_session(&state, &fakes).await?;
    let router = test_router(state.clone());
    fakes
        .clock
        .advance(Duration::minutes(14) + Duration::seconds(59));
    assert_eq!(
        call(&router, Method::GET, "/api/v1/__t/me", &[("cookie", &a)])
            .await?
            .status(),
        StatusCode::OK
    );
    fakes.clock.advance(Duration::seconds(1));
    assert_eq!(
        call(&router, Method::GET, "/api/v1/__t/me", &[("cookie", &b)])
            .await?
            .status(),
        StatusCode::UNAUTHORIZED
    );
    Ok(())
}

#[tokio::test]
async fn au_03_ac5_absolute_timeout_12_hours_despite_activity(
) -> Result<(), Box<dyn std::error::Error>> {
    let (state, fakes) = fixture()?;
    let (cookie, _) = authed_session(&state, &fakes).await?;
    let router = test_router(state.clone());
    for _ in 0..71 {
        fakes.clock.advance(Duration::minutes(10));
        assert_eq!(
            call(
                &router,
                Method::GET,
                "/api/v1/__t/me",
                &[("cookie", &cookie)]
            )
            .await?
            .status(),
            StatusCode::OK
        );
    }
    fakes.clock.advance(Duration::minutes(10));
    assert_eq!(
        call(
            &router,
            Method::GET,
            "/api/v1/__t/me",
            &[("cookie", &cookie)]
        )
        .await?
        .status(),
        StatusCode::UNAUTHORIZED
    );
    Ok(())
}

async fn authed_session(
    state: &AppState,
    fakes: &testkit::Fakes,
) -> Result<(String, UserId), Box<dyn std::error::Error>> {
    let user = seed_user(fakes, false).await?;
    let (cookie, _) = establish(state, &user, None).await?;
    Ok((cookie, user))
}

// ---------- One session per user ----------

#[tokio::test]
async fn au_07_ac1_new_sign_in_ends_old_session() -> Result<(), Box<dyn std::error::Error>> {
    let (state, fakes) = fixture()?;
    let user = seed_user(&fakes, false).await?;
    let (old, _) = establish(&state, &user, None).await?;
    let current = load(&state, &old).await?.expect("first session");
    let (new, _) = establish(&state, &user, Some(&current)).await?;
    let router = test_router(state.clone());
    assert_eq!(
        call(&router, Method::GET, "/api/v1/__t/me", &[("cookie", &old)])
            .await?
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        call(&router, Method::GET, "/api/v1/__t/me", &[("cookie", &new)])
            .await?
            .status(),
        StatusCode::OK
    );
    Ok(())
}

#[tokio::test]
async fn au_07_ac6_replaced_session_logged() -> Result<(), Box<dyn std::error::Error>> {
    let (state, fakes) = fixture()?;
    let user = seed_user(&fakes, false).await?;
    let (old, _) = establish(&state, &user, None).await?;
    let current = load(&state, &old).await?.expect("first session");
    let clock = obs::arc(obs::FixedClock(fakes.clock.now()));
    let (capture, _guard) = obs::capture("api", clock);
    let (_new, _) = establish(&state, &user, Some(&current)).await?;
    let text = capture.text();
    assert!(text.contains("session_end"), "capture: {text}");
    assert!(text.contains("replaced"), "capture: {text}");
    Ok(())
}

#[tokio::test]
async fn ses_1_one_record_per_user_after_new_sign_in() -> Result<(), Box<dyn std::error::Error>> {
    let (state, fakes) = fixture()?;
    let user = seed_user(&fakes, false).await?;
    let (old, _) = establish(&state, &user, None).await?;
    let current = load(&state, &old).await?.expect("first session");
    let (_new, _) = establish(&state, &user, Some(&current)).await?;
    let records = fakes.store.sessions().by_user(&user).await?;
    assert_eq!(records.len(), 1);
    Ok(())
}

#[tokio::test]
async fn asvs_v7_4_3_single_session_per_user() -> Result<(), Box<dyn std::error::Error>> {
    let (state, fakes) = fixture()?;
    let user = seed_user(&fakes, false).await?;
    let (old, _) = establish(&state, &user, None).await?;
    let current = load(&state, &old).await?.expect("first session");
    let (_new, _) = establish(&state, &user, Some(&current)).await?;
    assert_eq!(fakes.store.sessions().by_user(&user).await?.len(), 1);
    Ok(())
}

#[tokio::test]
async fn asvs_v7_5_2_new_sign_in_ends_old() -> Result<(), Box<dyn std::error::Error>> {
    let (state, fakes) = fixture()?;
    let user = seed_user(&fakes, false).await?;
    let (old, _) = establish(&state, &user, None).await?;
    let current = load(&state, &old).await?.expect("first session");
    let (new, _) = establish(&state, &user, Some(&current)).await?;
    let router = test_router(state.clone());
    assert_eq!(
        call(&router, Method::GET, "/api/v1/__t/me", &[("cookie", &old)])
            .await?
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        call(&router, Method::GET, "/api/v1/__t/me", &[("cookie", &new)])
            .await?
            .status(),
        StatusCode::OK
    );
    Ok(())
}

#[tokio::test]
async fn ses_1_signed_out_session_refused() -> Result<(), Box<dyn std::error::Error>> {
    let (state, fakes) = fixture()?;
    let session = authed(&state, &fakes, false).await?;
    let request_id = RequestId(fakes.rng.uuid_v4());
    SessionService::new(&state)
        .end(
            &session.hash,
            Some(&session.user),
            EndReason::SignedOut,
            request_id,
        )
        .await?;
    let router = test_router(state.clone());
    assert_eq!(
        call(
            &router,
            Method::GET,
            "/api/v1/__t/me",
            &[("cookie", &session.cookie)]
        )
        .await?
        .status(),
        StatusCode::UNAUTHORIZED
    );
    Ok(())
}

#[tokio::test]
async fn end_all_for_user_deletes_every_session() -> Result<(), Box<dyn std::error::Error>> {
    let (state, fakes) = fixture()?;
    let session = authed(&state, &fakes, false).await?;
    let request_id = RequestId(fakes.rng.uuid_v4());
    let count = SessionService::new(&state)
        .end_all_for_user(&session.user, EndReason::AdminEnded, request_id)
        .await?;
    assert_eq!(count, 1);
    assert_eq!(
        fakes.store.sessions().by_user(&session.user).await?.len(),
        0
    );
    assert!(load(&state, &session.cookie).await?.is_none());
    Ok(())
}

// ---------- pre_auth ----------

#[tokio::test]
async fn ses_1_pre_auth_gone_after_10_minutes() -> Result<(), Box<dyn std::error::Error>> {
    let (state, fakes) = fixture()?;
    let (cookie, _) = SessionService::new(&state).create_anonymous().await?;
    let value = cookie_value(&cookie.0)?;
    fakes
        .clock
        .advance(Duration::minutes(9) + Duration::seconds(59));
    assert!(load(&state, &value).await?.is_some());
    fakes.clock.advance(Duration::seconds(1));
    assert!(load(&state, &value).await?.is_none());
    Ok(())
}

#[tokio::test]
async fn ses_1_pre_auth_cleared_on_establish() -> Result<(), Box<dyn std::error::Error>> {
    let (state, fakes) = fixture()?;
    let user = seed_user(&fakes, false).await?;
    let (anon_cookie, _) = SessionService::new(&state).create_anonymous().await?;
    let anon = cookie_value(&anon_cookie.0)?;
    let current = load(&state, &anon).await?.expect("anonymous session");
    let (_cookie, record) = establish(&state, &user, Some(&current)).await?;
    assert!(record.record.pre_auth.is_none());
    Ok(())
}

#[tokio::test]
async fn session_pre_auth_seal_bound_to_record_id() -> Result<(), Box<dyn std::error::Error>> {
    let (state, fakes) = fixture()?;
    let id_a = SessionRecordId(fakes.rng.uuid_v4());
    let id_b = SessionRecordId(fakes.rng.uuid_v4());
    let plain = PreAuthPlain {
        intent: AuthIntent::Link,
        oauth_state: Sensitive::new("state-value".into()),
        nonce: Sensitive::new("nonce-value".into()),
        pkce_verifier: Sensitive::new("verifier-value".into()),
        invite_token_hash: None,
        pending_email: Some(Sensitive::new("person@example.test".into())),
        started_at: fakes.clock.now(),
    };
    let keys = state.ports.system_keys.as_ref();
    let sealed = seal_pre_auth(keys, &id_a, &plain).await?;
    assert!(open_pre_auth(keys, &id_b, &sealed).await.is_err());
    let opened = open_pre_auth(keys, &id_a, &sealed).await?;
    assert_eq!(opened.oauth_state.expose(), "state-value");
    assert_eq!(opened.nonce.expose(), "nonce-value");
    assert_eq!(
        opened.pending_email.expect("pending email").expose(),
        "person@example.test"
    );
    assert_eq!(opened.intent, AuthIntent::Link);
    Ok(())
}

// ---------- Cookie attributes as ASVS rows ----------

#[tokio::test]
async fn asvs_v3_3_1_cookie_secure() -> Result<(), Box<dyn std::error::Error>> {
    let (state, _) = fixture()?;
    let (cookie, _) = SessionService::new(&state).create_anonymous().await?;
    assert!(cookie.0.to_str()?.contains("Secure"));
    Ok(())
}

#[tokio::test]
async fn asvs_v3_3_2_cookie_samesite_lax() -> Result<(), Box<dyn std::error::Error>> {
    let (state, _) = fixture()?;
    let (cookie, _) = SessionService::new(&state).create_anonymous().await?;
    assert!(cookie.0.to_str()?.contains("SameSite=Lax"));
    Ok(())
}

#[tokio::test]
async fn asvs_v3_3_3_cookie_no_domain_path_root() -> Result<(), Box<dyn std::error::Error>> {
    let (state, _) = fixture()?;
    let (cookie, _) = SessionService::new(&state).create_anonymous().await?;
    let raw = cookie.0.to_str()?;
    assert!(raw.contains("Path=/"));
    assert!(!raw.contains("Domain"));
    Ok(())
}

#[tokio::test]
async fn asvs_v3_3_4_cookie_httponly_and_id_not_in_body() -> Result<(), Box<dyn std::error::Error>>
{
    let (state, fakes) = fixture()?;
    let session = authed(&state, &fakes, false).await?;
    let (cookie, _) = SessionService::new(&state).create_anonymous().await?;
    assert!(cookie.0.to_str()?.contains("HttpOnly"));
    let router = test_router(state.clone());
    let resp = call(
        &router,
        Method::GET,
        "/api/v1/__t/me",
        &[("cookie", &session.cookie)],
    )
    .await?;
    let raw = session.cookie.trim_start_matches("__session=").to_owned();
    for value in resp.headers().values() {
        let text = value.to_str().unwrap_or_default();
        assert!(!text.contains(&raw), "session id leaked in a header");
    }
    let body = axum::body::to_bytes(resp.into_body(), 16_384).await?;
    assert!(!String::from_utf8_lossy(&body).contains(&raw));
    Ok(())
}

// ---------- CSRF ----------

#[tokio::test]
async fn asvs_v3_5_1_missing_csrf_token_refused() -> Result<(), Box<dyn std::error::Error>> {
    let (state, fakes) = fixture()?;
    let session = authed(&state, &fakes, false).await?;
    let router = test_router(state.clone());
    let resp = call(
        &router,
        Method::POST,
        "/api/v1/__t/write",
        &[("cookie", &session.cookie), ("origin", ORIGIN)],
    )
    .await?;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    Ok(())
}

#[tokio::test]
async fn asvs_v3_5_1_wrong_csrf_token_refused() -> Result<(), Box<dyn std::error::Error>> {
    let (state, fakes) = fixture()?;
    let session = authed(&state, &fakes, false).await?;
    let router = test_router(state.clone());
    let resp = call(
        &router,
        Method::POST,
        "/api/v1/__t/write",
        &[
            ("cookie", &session.cookie),
            ("origin", ORIGIN),
            (CSRF_HEADER, "not-the-token"),
        ],
    )
    .await?;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    Ok(())
}

#[tokio::test]
async fn asvs_v3_5_1_foreign_origin_refused() -> Result<(), Box<dyn std::error::Error>> {
    let (state, fakes) = fixture()?;
    let session = authed(&state, &fakes, false).await?;
    let router = test_router(state.clone());
    let resp = call(
        &router,
        Method::POST,
        "/api/v1/__t/write",
        &[
            ("cookie", &session.cookie),
            ("origin", "https://evil.example"),
            (CSRF_HEADER, &session.csrf),
        ],
    )
    .await?;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    Ok(())
}

#[tokio::test]
async fn asvs_v3_5_1_missing_origin_refused() -> Result<(), Box<dyn std::error::Error>> {
    let (state, fakes) = fixture()?;
    let session = authed(&state, &fakes, false).await?;
    let router = test_router(state.clone());
    let resp = call(
        &router,
        Method::POST,
        "/api/v1/__t/write",
        &[("cookie", &session.cookie), (CSRF_HEADER, &session.csrf)],
    )
    .await?;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    Ok(())
}

#[tokio::test]
async fn asvs_v3_5_1_token_from_other_session_refused() -> Result<(), Box<dyn std::error::Error>> {
    let (state, fakes) = fixture()?;
    let session = authed(&state, &fakes, false).await?;
    let other = authed(&state, &fakes, false).await?;
    let router = test_router(state.clone());
    let resp = call(
        &router,
        Method::POST,
        "/api/v1/__t/write",
        &[
            ("cookie", &session.cookie),
            ("origin", ORIGIN),
            (CSRF_HEADER, &other.csrf),
        ],
    )
    .await?;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    let ok = call(
        &router,
        Method::POST,
        "/api/v1/__t/write",
        &[
            ("cookie", &session.cookie),
            ("origin", ORIGIN),
            (CSRF_HEADER, &session.csrf),
        ],
    )
    .await?;
    assert_eq!(ok.status(), StatusCode::NO_CONTENT);
    Ok(())
}

#[tokio::test]
async fn asvs_v3_5_3_get_never_requires_or_changes_state() -> Result<(), Box<dyn std::error::Error>>
{
    let (state, fakes) = fixture()?;
    let session = authed(&state, &fakes, false).await?;
    let router = test_router(state.clone());
    // A GET-only route cannot be reached by POST.
    let wrong = call(
        &router,
        Method::POST,
        "/api/v1/__t/me",
        &[
            ("cookie", &session.cookie),
            ("origin", ORIGIN),
            (CSRF_HEADER, &session.csrf),
        ],
    )
    .await?;
    assert_eq!(wrong.status(), StatusCode::METHOD_NOT_ALLOWED);
    // A POST-only route cannot be reached by GET (and needs no token).
    let wrong = call(
        &router,
        Method::GET,
        "/api/v1/__t/write",
        &[("cookie", &session.cookie)],
    )
    .await?;
    assert_eq!(wrong.status(), StatusCode::METHOD_NOT_ALLOWED);
    Ok(())
}

// ---------- Server-side session storage ----------

#[tokio::test]
async fn asvs_v7_2_1_store_holds_hash_not_raw_id() -> Result<(), Box<dyn std::error::Error>> {
    let (state, fakes) = fixture()?;
    let session = authed(&state, &fakes, false).await?;
    let raw = session.cookie.trim_start_matches("__session=").to_owned();
    let dump = format!("{:?}", fakes.store.export_json());
    assert!(!dump.contains(&raw), "raw session id in the store");
    assert!(
        dump.contains(&session.hash.to_hex()),
        "hash not in the store"
    );
    Ok(())
}

#[tokio::test]
async fn asvs_v7_2_2_unknown_cookie_value_not_adopted() -> Result<(), Box<dyn std::error::Error>> {
    let (state, fakes) = fixture()?;
    let chosen = format!("__session={}", "A".repeat(43));
    let router = test_router(state.clone());
    let resp = call(
        &router,
        Method::GET,
        "/api/v1/__t/me",
        &[("cookie", &chosen)],
    )
    .await?;
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    let sessions = fakes
        .store
        .export_json()
        .into_iter()
        .filter(|(name, _)| *name == "sessions")
        .count();
    assert_eq!(sessions, 0);
    Ok(())
}

#[tokio::test]
async fn asvs_v7_2_3_session_id_is_32_random_bytes() -> Result<(), Box<dyn std::error::Error>> {
    let (ports, _) = testkit::fake_ports();
    let raw = RawSessionId::generate(ports.rng.as_ref());
    let bytes = URL_SAFE_NO_PAD.decode(raw.expose())?;
    assert_eq!(bytes.len(), 32);
    Ok(())
}

#[tokio::test]
async fn asvs_v7_2_4_rotate_kills_old_id_keeps_record_id() -> Result<(), Box<dyn std::error::Error>>
{
    let (state, fakes) = fixture()?;
    let session = authed(&state, &fakes, false).await?;
    let current = load(&state, &session.cookie).await?.expect("session");
    let old_record_id = current.record.record.session_record_id;
    let old_created_at = current.record.record.created_at;
    let (new_cookie, new_record) = SessionService::new(&state).rotate(&current, |_| {}).await?;
    let new = cookie_value(&new_cookie.0)?;
    assert_eq!(new_record.record.session_record_id, old_record_id);
    assert_eq!(new_record.record.created_at, old_created_at);
    assert_ne!(new, session.cookie);
    let router = test_router(state.clone());
    assert_eq!(
        call(
            &router,
            Method::GET,
            "/api/v1/__t/me",
            &[("cookie", &session.cookie)]
        )
        .await?
        .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        call(&router, Method::GET, "/api/v1/__t/me", &[("cookie", &new)])
            .await?
            .status(),
        StatusCode::OK
    );
    Ok(())
}

#[tokio::test]
async fn asvs_v7_3_1_idle_timeout() -> Result<(), Box<dyn std::error::Error>> {
    let (state, fakes) = fixture()?;
    let (cookie, _) = authed_session(&state, &fakes).await?;
    fakes.clock.advance(IDLE_TIMEOUT);
    assert!(load(&state, &cookie).await?.is_none());
    Ok(())
}

#[tokio::test]
async fn asvs_v7_3_2_absolute_timeout() -> Result<(), Box<dyn std::error::Error>> {
    let (state, fakes) = fixture()?;
    let (cookie, _) = authed_session(&state, &fakes).await?;
    for _ in 0..71 {
        fakes.clock.advance(Duration::minutes(10));
        assert!(load(&state, &cookie).await?.is_some());
    }
    fakes.clock.advance(Duration::minutes(10));
    assert!(load(&state, &cookie).await?.is_none());
    Ok(())
}

#[tokio::test]
async fn session_touch_written_at_most_once_a_minute() -> Result<(), Box<dyn std::error::Error>> {
    let (state, fakes) = fixture()?;
    let session = authed(&state, &fakes, false).await?;
    let router = test_router(state.clone());
    let before = fakes
        .store
        .sessions()
        .get(&session.hash)
        .await?
        .expect("session")
        .version;
    for _ in 0..3 {
        assert_eq!(
            call(
                &router,
                Method::GET,
                "/api/v1/__t/me",
                &[("cookie", &session.cookie)]
            )
            .await?
            .status(),
            StatusCode::OK
        );
    }
    let unchanged = fakes
        .store
        .sessions()
        .get(&session.hash)
        .await?
        .expect("session")
        .version;
    assert_eq!(before, unchanged, "touched inside the interval");
    fakes.clock.advance(TOUCH_INTERVAL + Duration::seconds(1));
    assert_eq!(
        call(
            &router,
            Method::GET,
            "/api/v1/__t/me",
            &[("cookie", &session.cookie)]
        )
        .await?
        .status(),
        StatusCode::OK
    );
    let touched = fakes
        .store
        .sessions()
        .get(&session.hash)
        .await?
        .expect("session")
        .version;
    assert_ne!(touched, unchanged, "not touched after the interval");
    Ok(())
}

// ---------- Extractors and events ----------

#[tokio::test]
async fn asvs_v8_2_1_user_route_without_session_401() -> Result<(), Box<dyn std::error::Error>> {
    let (state, _) = fixture()?;
    let router = test_router(state.clone());
    assert_eq!(
        call(&router, Method::GET, "/api/v1/__t/me", &[])
            .await?
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        call(&router, Method::GET, "/api/v1/__t/admin", &[])
            .await?
            .status(),
        StatusCode::UNAUTHORIZED
    );
    Ok(())
}

#[tokio::test]
async fn any_session_serves_anonymous_and_authenticated() -> Result<(), Box<dyn std::error::Error>>
{
    let (state, fakes) = fixture()?;
    let (anon, _) = SessionService::new(&state).create_anonymous().await?;
    let anon = cookie_value(&anon.0)?;
    let router = test_router(state.clone());
    assert_eq!(
        call(
            &router,
            Method::GET,
            "/api/v1/__t/any",
            &[("cookie", &anon)]
        )
        .await?
        .status(),
        StatusCode::OK
    );
    let session = authed(&state, &fakes, false).await?;
    assert_eq!(
        call(
            &router,
            Method::GET,
            "/api/v1/__t/any",
            &[("cookie", &session.cookie)]
        )
        .await?
        .status(),
        StatusCode::OK
    );
    Ok(())
}

#[tokio::test]
async fn pending_invite_session_requires_pending_state() -> Result<(), Box<dyn std::error::Error>> {
    let (state, fakes) = fixture()?;
    let (anon, record) = SessionService::new(&state).create_anonymous().await?;
    let anon = cookie_value(&anon.0)?;
    let router = test_router(state.clone());
    assert_eq!(
        call(
            &router,
            Method::GET,
            "/api/v1/__t/pending",
            &[("cookie", &anon)]
        )
        .await?
        .status(),
        StatusCode::UNAUTHORIZED
    );
    // The pending state (owned by T-502b) is exercised directly here.
    let hash = record.record.session_hash;
    let mut updated = record.record.clone();
    updated.state = SessionState::PendingInviteRequest;
    fakes
        .store
        .sessions()
        .put(&updated, Precondition::Matches(record.version.clone()))
        .await?;
    assert_eq!(
        call(
            &router,
            Method::GET,
            "/api/v1/__t/pending",
            &[("cookie", &anon)]
        )
        .await?
        .status(),
        StatusCode::OK
    );
    assert!(fakes.store.sessions().get(&hash).await?.is_some());
    Ok(())
}

#[tokio::test]
async fn asvs_v16_3_2_non_admin_on_admin_route_logged() -> Result<(), Box<dyn std::error::Error>> {
    let (state, fakes) = fixture()?;
    let session = authed(&state, &fakes, false).await?;
    let clock = obs::arc(obs::FixedClock(fakes.clock.now()));
    let (capture, _guard) = obs::capture("api", clock);
    let resp = call(
        &test_router(state.clone()),
        Method::GET,
        "/api/v1/__t/admin",
        &[("cookie", &session.cookie)],
    )
    .await?;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    assert!(capture.text().contains("authz_failure"));
    // An admin passes.
    let admin = authed(&state, &fakes, true).await?;
    assert_eq!(
        call(
            &test_router(state.clone()),
            Method::GET,
            "/api/v1/__t/admin",
            &[("cookie", &admin.cookie)]
        )
        .await?
        .status(),
        StatusCode::OK
    );
    Ok(())
}

#[tokio::test]
async fn asvs_v16_3_3_csrf_failure_logged() -> Result<(), Box<dyn std::error::Error>> {
    let (state, fakes) = fixture()?;
    let session = authed(&state, &fakes, false).await?;
    let clock = obs::arc(obs::FixedClock(fakes.clock.now()));
    let (capture, _guard) = obs::capture("api", clock);
    let resp = call(
        &test_router(state.clone()),
        Method::POST,
        "/api/v1/__t/write",
        &[("cookie", &session.cookie), ("origin", ORIGIN)],
    )
    .await?;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    let text = capture.text();
    assert!(text.contains("csrf_failure"), "capture: {text}");
    Ok(())
}

// ---------- Small unit checks ----------

#[test]
fn cookie_helpers_have_exact_shape() {
    let (ports, _) = testkit::fake_ports();
    let raw = RawSessionId::generate(ports.rng.as_ref());
    let cookie = api::session::cookie::set_cookie(&raw);
    assert!(cookie.to_str().unwrap().starts_with("__session="));
    assert!(clear_cookie().to_str().unwrap().contains("Max-Age=0"));
    assert!(RawSessionId::parse("short").is_none());
    assert!(RawSessionId::parse(&"x".repeat(43)).is_some());
    assert!(read_cookie(&cookie_headers("other=1; __session=nope")).is_none());
    let good = format!("__session={}", raw.expose());
    assert!(read_cookie(&cookie_headers(&good)).is_some());
    assert_eq!(
        format!("{:?}", RawSessionId::generate(ports.rng.as_ref())),
        "RawSessionId([redacted])"
    );
}

#[test]
fn constant_time_token_compare_handles_lengths() {
    assert!(tokens_equal(b"abcd", b"abcd"));
    assert!(!tokens_equal(b"abcd", b"abce"));
    assert!(!tokens_equal(b"abcd", b"abcde"));
    assert!(!tokens_equal(b"", b""));
}

#[test]
fn end_reason_wire_names() {
    assert_eq!(EndReason::SignedOut.wire(), "signed_out");
    assert_eq!(EndReason::Replaced.wire(), "replaced");
    assert_eq!(EndReason::AdminEnded.wire(), "admin_ended");
    assert_eq!(EndReason::AccountDeleted.wire(), "account_deleted");
}

#[tokio::test]
async fn session_load_store_failure_is_internal() -> Result<(), Box<dyn std::error::Error>> {
    let (state, fakes) = fixture()?;
    let session = authed(&state, &fakes, false).await?;
    fakes.store.fail_next(1);
    assert!(SessionService::new(&state)
        .load(&cookie_headers(&session.cookie))
        .await
        .is_err());
    Ok(())
}
