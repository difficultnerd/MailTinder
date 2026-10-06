//! T-804 admin-users integration tests (AU-07 AC5, AC6; AU-01 AC5; ASVS
//! V7.4.5, V8.2.1; S9 section 7.6).
//!
//! Everything runs through the real router with the fakes and the virtual
//! clock, mirroring the invite tests: the admin list joins live records, and
//! ending a session is a stepped-up admin write.

#![allow(clippy::too_many_lines)]

use std::sync::Arc;

use api::session::extract::AuthedSession;
use api::session::store::SessionService;
use api::state::AppState;
use api::{app_state, build_router_with_routes, config::ApiConfig};
use axum::body::Body;
use axum::http::{header, HeaderValue, Method, Request, StatusCode};
use axum::response::Response;
use axum::routing::get;
use axum::Router;
use domain::{
    EmailAddress, JobId, JobMethod, JobStatus, MailboxStatus, Provider, ProviderSubjectId, UserId,
};
use obs::{Pseudonymiser, Sensitive};
use ports::store::aad_fields;
use ports::{
    Aad, Ciphertext, Clock, JobRecord, KeyService, ListKeyHash, MailboxRecord, Precondition, Rng,
    ServerStore, UserRecord,
};
use time::{Duration, OffsetDateTime};
use tower::ServiceExt;

type TestResult = Result<(), Box<dyn std::error::Error>>;

const ORIGIN: &str = "https://mailtinder.test";
const EMAIL_KEY: &[u8] = b"fake-email-key";
const LOG_KEY: &[u8] = b"fake-log-key";

/// The scoped log capture is per-thread; serialise the tests that assert on a
/// captured security event so a neighbour cannot blank it out.
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
        Sensitive::new(LOG_KEY.to_vec()),
        Sensitive::new(EMAIL_KEY.to_vec()),
    )?;
    Ok((app_state(Arc::new(ports), Arc::new(config)), fakes, serial))
}

async fn me(_: AuthedSession) -> StatusCode {
    StatusCode::OK
}

fn test_router(state: AppState) -> Router {
    build_router_with_routes(state, |r| r.route("/api/v1/__t/me", get(me)))
}

fn cookie_pair(header: &HeaderValue) -> Result<String, Box<dyn std::error::Error>> {
    Ok(header
        .to_str()?
        .split(';')
        .next()
        .unwrap_or_default()
        .to_owned())
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

/// The pseudonymous ID a log line carries for `user`, computed with the same
/// log key the fixture hands the app (S5 logs).
fn pseudo_of(user: &UserId) -> String {
    Pseudonymiser::new(Sensitive::new(LOG_KEY.to_vec()))
        .pseudo_id(&user.0)
        .as_str()
        .to_owned()
}

/// A string field of a parsed log line, if present.
fn field<'a>(value: &'a serde_json::Value, key: &str) -> Option<&'a str> {
    value.get(key).and_then(|v| v.as_str())
}

/// The parsed `security` lines of a capture, so a test asserts on fields rather
/// than substrings (a removed pseudonymous ID would otherwise pass).
fn security_events(text: &str) -> Vec<serde_json::Value> {
    let mut events = Vec::new();
    for line in text.lines() {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if field(&value, "event") == Some("security") {
            events.push(value);
        }
    }
    events
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
    let sealed = fakes
        .keys
        .seal(
            &user,
            &user_record.record.wrapped_data_key,
            &Aad {
                user,
                scope: mailbox_id.0.to_string(),
                field: aad_fields::MAILBOX_EMAIL,
            },
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

/// A signed-in session; `step_up` gives it a fresh Google sign-in.
async fn session_for(
    state: &AppState,
    user: UserId,
    step_up: bool,
) -> Result<Authed, Box<dyn std::error::Error>> {
    let recent = step_up.then(|| state.ports.clock.now());
    let (cookie, record) = SessionService::new(state)
        .establish(None, &user, recent)
        .await?;
    Ok(Authed {
        cookie: cookie_pair(&cookie.0)?,
        csrf: record.record.csrf_token.clone(),
        user,
    })
}

/// An admin with a mailbox, step-up fresh or not.
async fn admin(
    state: &AppState,
    fakes: &testkit::Fakes,
    sub: &str,
    step_up: bool,
) -> Result<Authed, Box<dyn std::error::Error>> {
    let user = seed_user(fakes, true).await?;
    seed_mailbox(fakes, user, sub, "admin@example.com").await?;
    session_for(state, user, step_up).await
}

async fn call(
    router: &Router,
    method: Method,
    uri: &str,
    who: &Authed,
) -> Result<Response, Box<dyn std::error::Error>> {
    let req = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::ORIGIN, ORIGIN)
        .header(header::COOKIE, &who.cookie)
        .header("x-csrf-token", &who.csrf)
        .body(Body::empty())?;
    Ok(router.clone().oneshot(req).await?)
}

/// A queued unsubscribe job for `user`, so a session end can be shown not to
/// touch it.
async fn seed_queued_job(
    fakes: &testkit::Fakes,
    user: UserId,
) -> Result<JobId, Box<dyn std::error::Error>> {
    let job_id = JobId(fakes.rng.uuid_v4());
    let now = fakes.clock.now();
    fakes
        .store
        .jobs()
        .put(
            &JobRecord {
                job_id,
                user_id: user,
                mailbox_id: ports::mailbox_id_for(
                    Provider::Gmail,
                    &ProviderSubjectId::new("sub-target")?,
                ),
                list_key_hash: ListKeyHash([0u8; 32]),
                method: JobMethod::Mailto,
                target: None,
                // Added by T-605/T-701 on main; absent for this plain job.
                sender_display: None,
                due_at: now,
                status: JobStatus::Queued,
                attempts: 0,
                outcome: None,
                expires_at: now + Duration::days(30),
            },
            Precondition::MustNotExist,
        )
        .await?;
    Ok(job_id)
}

/// A target user with a mailbox and a live session.
async fn target(
    state: &AppState,
    fakes: &testkit::Fakes,
) -> Result<Authed, Box<dyn std::error::Error>> {
    let user = seed_user(fakes, false).await?;
    seed_mailbox(fakes, user, "sub-target", "target@example.com").await?;
    session_for(state, user, true).await
}

// ---------------------------------------------------------------------------
// AU-07 AC5: an admin ends a user's session; that user's next request is 401.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn au_07_ac5_admin_ends_user_session() -> TestResult {
    let (state, fakes, _serial) = fixture().await?;
    let router = test_router(state.clone());
    let admin = admin(&state, &fakes, "sub-admin", true).await?;
    let victim = target(&state, &fakes).await?;

    // The victim can act before the admin intervenes.
    let before = call(&router, Method::GET, "/api/v1/__t/me", &victim).await?;
    assert_eq!(before.status(), StatusCode::OK);

    let resp = call(
        &router,
        Method::DELETE,
        &format!("/api/v1/admin/users/{}/sessions", victim.user.0),
        &admin,
    )
    .await?;
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    let after = call(&router, Method::GET, "/api/v1/__t/me", &victim).await?;
    assert_eq!(after.status(), StatusCode::UNAUTHORIZED);
    assert!(
        fakes
            .store
            .sessions()
            .by_user(&victim.user)
            .await?
            .is_empty(),
        "no session remains for the victim"
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// AU-07 AC5 / S7 5.11: queued unsubscribe jobs keep running after a session end.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn au_07_ac5_jobs_keep_running_after_session_end() -> TestResult {
    let (state, fakes, _serial) = fixture().await?;
    let router = test_router(state.clone());
    let admin = admin(&state, &fakes, "sub-admin", true).await?;
    let victim = target(&state, &fakes).await?;
    let job_id = seed_queued_job(&fakes, victim.user).await?;

    let resp = call(
        &router,
        Method::DELETE,
        &format!("/api/v1/admin/users/{}/sessions", victim.user.0),
        &admin,
    )
    .await?;
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    let job = fakes
        .store
        .jobs()
        .get(&job_id)
        .await?
        .ok_or("job survives")?;
    assert_eq!(
        job.record.status,
        JobStatus::Queued,
        "a queued job needs no session and keeps running"
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// AU-07 AC6: an admin ending a session is a security event. Two correlated
// lines are written, one naming the target and one naming the actor; they share
// the request ID, the documented join key (S6 section 7). Neither carries an
// address.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn au_07_ac6_admin_session_end_logged() -> TestResult {
    let (state, fakes, _serial) = fixture().await?;
    let router = test_router(state.clone());
    let admin = admin(&state, &fakes, "sub-admin", true).await?;
    let victim = target(&state, &fakes).await?;
    let clock = obs::arc(obs::FixedClock(fakes.clock.now()));
    let (capture, _guard) = obs::capture("api", clock);

    let resp = call(
        &router,
        Method::DELETE,
        &format!("/api/v1/admin/users/{}/sessions", victim.user.0),
        &admin,
    )
    .await?;
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    let text = capture.text();
    let events = security_events(&text);
    let victim_pseudo = pseudo_of(&victim.user);
    let admin_pseudo = pseudo_of(&admin.user);

    // The target's session end is logged, named for the target.
    let target_events: Vec<&serde_json::Value> = events
        .iter()
        .filter(|e| field(e, "action") == Some("session_end"))
        .collect();
    assert_eq!(
        target_events.len(),
        1,
        "one session_end per deleted session: {text}"
    );
    assert_eq!(
        field(target_events[0], "outcome"),
        Some("admin_ended"),
        "with the admin-ended outcome: {text}"
    );
    assert_eq!(
        field(target_events[0], "user_pseudo"),
        Some(victim_pseudo.as_str()),
        "the target's pseudonymous ID, not the address: {text}"
    );

    // The actor's admin action is logged, named for the admin.
    let actor_events: Vec<&serde_json::Value> = events
        .iter()
        .filter(|e| field(e, "action") == Some("admin_action"))
        .collect();
    assert_eq!(actor_events.len(), 1, "one admin_action: {text}");
    assert_eq!(
        field(actor_events[0], "user_pseudo"),
        Some(admin_pseudo.as_str()),
        "the admin's pseudonymous ID: {text}"
    );

    // The documented join key: the two lines share the request ID, so an
    // investigator can attribute the termination to the admin (S6 section 7).
    let target_request_id = field(target_events[0], "request_id");
    assert!(
        target_request_id.is_some_and(|id| !id.is_empty()),
        "the target line carries a request ID: {text}"
    );
    assert_eq!(
        field(actor_events[0], "request_id"),
        target_request_id,
        "the actor and target lines share the request ID: {text}"
    );
    assert_ne!(
        field(target_events[0], "user_pseudo"),
        field(actor_events[0], "user_pseudo"),
        "the two lines name different principals: {text}"
    );

    assert!(
        !text.contains("target@example.com") && !text.contains("admin@example.com"),
        "no address in the security log: {text}"
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// AU-01 AC5: ending a session needs step-up.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn au_01_ac5_end_session_needs_step_up() -> TestResult {
    let (state, fakes, _serial) = fixture().await?;
    let router = test_router(state.clone());
    let admin = admin(&state, &fakes, "sub-admin", false).await?;
    let victim = target(&state, &fakes).await?;

    let resp = call(
        &router,
        Method::DELETE,
        &format!("/api/v1/admin/users/{}/sessions", victim.user.0),
        &admin,
    )
    .await?;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    assert_eq!(code_of(resp).await?, "step_up_required");
    assert_eq!(
        fakes.store.sessions().by_user(&victim.user).await?.len(),
        1,
        "without step-up nothing changes"
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// ASVS V7.4.5: administrators can end an individual user's session.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn asvs_v7_4_5_admin_terminates_session() -> TestResult {
    let (state, fakes, _serial) = fixture().await?;
    let router = test_router(state.clone());
    let admin = admin(&state, &fakes, "sub-admin", true).await?;
    let victim = target(&state, &fakes).await?;

    let resp = call(
        &router,
        Method::DELETE,
        &format!("/api/v1/admin/users/{}/sessions", victim.user.0),
        &admin,
    )
    .await?;
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    let remaining = fakes.store.sessions().by_user(&victim.user).await?;
    assert!(remaining.is_empty(), "the administrator ends the session");
    Ok(())
}

// ---------------------------------------------------------------------------
// ASVS V8.2.1: a non-admin is refused on both routes, and it is logged.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn asvs_v8_2_1_non_admin_refused_and_logged() -> TestResult {
    let (state, fakes, _serial) = fixture().await?;
    let router = test_router(state.clone());
    let user = seed_user(&fakes, false).await?;
    seed_mailbox(&fakes, user, "sub-user", "user@example.com").await?;
    let who = session_for(&state, user, true).await?;
    let victim = target(&state, &fakes).await?;
    let clock = obs::arc(obs::FixedClock(fakes.clock.now()));
    let (capture, _guard) = obs::capture("api", clock);

    let list = call(&router, Method::GET, "/api/v1/admin/users", &who).await?;
    assert_eq!(list.status(), StatusCode::FORBIDDEN);
    assert_eq!(code_of(list).await?, "forbidden");

    let end = call(
        &router,
        Method::DELETE,
        &format!("/api/v1/admin/users/{}/sessions", victim.user.0),
        &who,
    )
    .await?;
    assert_eq!(end.status(), StatusCode::FORBIDDEN);
    assert_eq!(code_of(end).await?, "forbidden");
    assert_eq!(
        fakes.store.sessions().by_user(&victim.user).await?.len(),
        1,
        "a non-admin changes nothing"
    );
    let text = capture.text();
    assert!(
        text.contains("authz_failure"),
        "refusals are logged: {text}"
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// S9 section 7.6: the list shows the earliest address, mailbox count and
// whether the user is signed in.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn s9_admin_users_list_fields() -> TestResult {
    let (state, fakes, _serial) = fixture().await?;
    let router = test_router(state.clone());
    let admin = admin(&state, &fakes, "sub-admin", true).await?;

    // A signed-in user with two mailboxes; the earliest is the one they joined
    // with, even after a later one is linked.
    let signed_in = seed_user(&fakes, false).await?;
    seed_mailbox(&fakes, signed_in, "sub-first", "first@example.com").await?;
    fakes.clock.advance(Duration::minutes(1));
    seed_mailbox(&fakes, signed_in, "sub-second", "second@example.com").await?;
    let live = session_for(&state, signed_in, true).await?;
    let seen_at = fakes.clock.now();
    let _ = live;

    // A signed-out user with one mailbox and no session.
    let signed_out = seed_user(&fakes, false).await?;
    seed_mailbox(&fakes, signed_out, "sub-out", "out@example.com").await?;

    let resp = call(&router, Method::GET, "/api/v1/admin/users", &admin).await?;
    assert_eq!(resp.status(), StatusCode::OK);
    let json = json_of(resp).await?;
    let users = json["users"].as_array().ok_or("users array")?;

    let row = users
        .iter()
        .find(|u| u["user_id"] == serde_json::json!(signed_in.0.to_string()))
        .ok_or("signed-in user listed")?;
    assert_eq!(row["email_address"], "first@example.com");
    assert_eq!(row["is_admin"], false);
    assert_eq!(row["mailbox_count"], 2);
    assert_eq!(row["signed_in"], true);
    assert_eq!(row["created_at"].as_str().ok_or("created_at")?.len(), 20);
    let last_seen = OffsetDateTime::parse(
        row["last_seen_at"].as_str().ok_or("last_seen_at")?,
        &time::format_description::well_known::Rfc3339,
    )?;
    assert_eq!(last_seen, seen_at);

    let out = users
        .iter()
        .find(|u| u["user_id"] == serde_json::json!(signed_out.0.to_string()))
        .ok_or("signed-out user listed")?;
    assert_eq!(out["email_address"], "out@example.com");
    assert_eq!(out["mailbox_count"], 1);
    assert_eq!(out["signed_in"], false);
    assert!(out["last_seen_at"].is_null());
    Ok(())
}

// ---------------------------------------------------------------------------
// S7 section 6: admin session kills are limited to 20 per admin per day.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn admin_users_rate_limit_20_per_day() -> TestResult {
    let (state, fakes, _serial) = fixture().await?;
    let router = test_router(state.clone());
    let admin = admin(&state, &fakes, "sub-admin", true).await?;
    let victim = target(&state, &fakes).await?;
    let uri = format!("/api/v1/admin/users/{}/sessions", victim.user.0);

    for n in 0..20 {
        let resp = call(&router, Method::DELETE, &uri, &admin).await?;
        assert_eq!(resp.status(), StatusCode::NO_CONTENT, "call {n} passes");
    }
    let resp = call(&router, Method::DELETE, &uri, &admin).await?;
    assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(code_of(resp).await?, "rate_limited");
    Ok(())
}
