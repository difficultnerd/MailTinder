//! T-603 progress endpoint tests (GM-01 AC1, AC2; GM-04 AC1 to AC3; API-PROG-1).
//!
//! Everything runs on `FakeMailbox`, the in-memory store, keys and app folder,
//! and the virtual clock. The counting tests call `progress` directly; the
//! route test goes through the real router.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::too_many_lines,
    clippy::similar_names,
    clippy::items_after_statements,
    clippy::needless_pass_by_value,
    clippy::missing_panics_doc,
    clippy::assert_is_empty
)]

use std::collections::BTreeSet;
use std::sync::Arc;

use api::services::progress::progress;
use api::services::user_state_store::UserStateStore;
use api::session::extract::AuthedSession;
use api::session::store::SessionService;
use api::state::AppState;
use api::{app_state, build_router, config::ApiConfig};
use axum::body::Body;
use axum::http::{header, HeaderValue, Method, Request, StatusCode};
use axum::response::Response;
use axum::Router;
use domain::user_state::{MailboxPosition, UserState};
use domain::{EmailAddress, MailboxId, MailboxStatus, Provider, ProviderSubjectId, UserId};
use obs::Sensitive;
use ports::store::aad_fields;
use ports::{
    Aad, Ciphertext, Clock, KeyService, MailboxRecord, Precondition, Rng, ServerStore, SessionHash,
    SessionRecordId, UserRecord,
};
use testkit::app_folder::FolderOp;
use testkit::{fake_ports, Fakes, MailOp, SeedMessage};
use time::macros::datetime;
use time::{Duration, OffsetDateTime};
use tower::ServiceExt;
use uuid::Uuid;

type TestResult = Result<(), Box<dyn std::error::Error>>;

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

/// A time inside 2023 (the level most tests work through).
fn in_2023() -> OffsetDateTime {
    datetime!(2023-06-15 12:00:00 UTC)
}

/// A time inside 2022.
fn in_2022() -> OffsetDateTime {
    datetime!(2022-06-15 12:00:00 UTC)
}

/// A time inside 2024, the year after the level under test.
fn in_2024() -> OffsetDateTime {
    datetime!(2024-02-01 00:00:00 UTC)
}

struct World {
    app: AppState,
    fakes: Fakes,
    user: UserId,
    primary: MailboxId,
}

async fn seed_mailbox(
    fakes: &Fakes,
    user: UserId,
    sub: &str,
    email: &str,
    is_primary: bool,
) -> Result<MailboxId, Box<dyn std::error::Error>> {
    let subject = ProviderSubjectId::new(sub)?;
    let address = EmailAddress::parse(email)?;
    let mailbox_id = ports::mailbox_id_for(Provider::Gmail, &subject);
    let user_record = fakes.store.users().get(&user).await?.ok_or("user")?;
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

/// The access token the scripted identity provider hands out for `sub`.
fn access_token_for(sub: &str) -> String {
    format!("access-for-refresh-{sub}")
}

impl World {
    async fn new() -> Result<World, Box<dyn std::error::Error>> {
        let (ports, fakes) = fake_ports();
        let app = app_state(Arc::new(ports), Arc::new(config()?));
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
        let mut world = World {
            app,
            fakes,
            user,
            primary: MailboxId::new(Uuid::nil()),
        };
        world.primary = world.add_mailbox("sub-a", "a@example.com", true).await?;
        Ok(world)
    }

    /// A second linked mailbox with a working grant.
    async fn add_mailbox(
        &self,
        sub: &str,
        email: &str,
        is_primary: bool,
    ) -> Result<MailboxId, Box<dyn std::error::Error>> {
        let id = seed_mailbox(&self.fakes, self.user, sub, email, is_primary).await?;
        let token = format!("refresh-{sub}");
        self.app
            .tokens
            .store_refresh_token(&self.app, &self.user, &id, Sensitive::new(token.clone()))
            .await?;
        self.fakes
            .identity
            .script_refresh(&token, Ok(access_token_for(sub)));
        Ok(id)
    }

    fn session(&self) -> AuthedSession {
        AuthedSession {
            user: self.user,
            session_record_id: SessionRecordId(Uuid::from_u128(1)),
            is_admin: false,
            recent_auth_at: None,
            session_hash: SessionHash([1_u8; 32]),
        }
    }

    /// An inbox message in `mailbox`, received at `received_at`.
    fn seed(&self, mailbox: &MailboxId, sender: &str, received_at: OffsetDateTime) {
        self.fakes.mailbox.seed(
            mailbox,
            SeedMessage {
                from_display: sender.to_owned(),
                from_address: format!("{sender}@example.com"),
                subject: format!("Subject from {sender}"),
                raw_headers: vec![],
                facts: domain::HeaderFacts::default(),
                preview_text: String::new(),
                internal_date: received_at,
                labels: vec!["INBOX".to_owned()],
            },
        );
    }

    /// Store a Feed position for `mailbox` in the user's state file.
    async fn set_position(
        &self,
        mailbox: &MailboxId,
        position: MailboxPosition,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let store = UserStateStore::new(Arc::new(self.app.clone()));
        let id = mailbox.0;
        store
            .update(&self.user, move |s: &mut UserState| {
                s.positions.insert(id, position.clone());
            })
            .await?;
        Ok(())
    }

    async fn meter(
        &self,
    ) -> Result<api::routes::progress::ProgressDto, Box<dyn std::error::Error>> {
        Ok(progress(&self.app, &self.session()).await?)
    }
}

/// A position whose new mail is already cleared, at `ceiling`.
fn cleared_at(ceiling: OffsetDateTime) -> MailboxPosition {
    MailboxPosition {
        newest_seen: Some(ceiling),
        new_done: true,
        backlog_ceiling: Some(ceiling),
        ..MailboxPosition::default()
    }
}

// ---------------------------------------------------------------------------
// GM-01 Inbox meter
// ---------------------------------------------------------------------------

#[tokio::test]
async fn gm_01_ac1_inbox_count_sums_mailboxes() -> TestResult {
    let w = World::new().await?;
    let second = w.add_mailbox("sub-b", "b@example.com", false).await?;
    for _ in 0..2 {
        w.seed(&w.primary, "alice", in_2023());
    }
    for _ in 0..3 {
        w.seed(&second, "bob", in_2023());
    }
    let meter = w.meter().await?;
    assert_eq!(meter.inbox_count, 5);
    assert!(meter.mailbox_errors.is_empty());
    assert!(meter.level.is_none(), "no position, so no level yet");
    Ok(())
}

#[tokio::test]
async fn gm_01_ac2_failing_mailbox_marked_rest_counted() -> TestResult {
    let w = World::new().await?;
    let second = w.add_mailbox("sub-b", "b@example.com", false).await?;
    for _ in 0..2 {
        w.seed(&w.primary, "alice", in_2023());
    }
    for _ in 0..3 {
        w.seed(&second, "bob", in_2023());
    }
    // The second mailbox's grant is dead: its count fails, the first still
    // counts, and the failure is data with no address in it.
    w.fakes.mailbox.revoke_token(&access_token_for("sub-b"));

    let meter = w.meter().await?;
    assert_eq!(
        meter.inbox_count, 2,
        "the mailbox that answered still counts"
    );
    assert_eq!(meter.mailbox_errors.len(), 1);
    assert_eq!(meter.mailbox_errors[0].mailbox_id, second.0);
    assert_eq!(meter.mailbox_errors[0].code, "mailbox_needs_sign_in");
    assert_eq!(
        w.fakes.mailbox.calls(MailOp::CountMessages),
        0,
        "no level is being worked through, so no date-range count"
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// GM-04 Backlog years as levels
// ---------------------------------------------------------------------------

#[tokio::test]
async fn gm_04_ac1_level_null_until_new_mail_cleared() -> TestResult {
    let w = World::new().await?;
    for _ in 0..4 {
        w.seed(&w.primary, "alice", in_2023());
    }
    // A stored position that still has new mail to clear.
    w.set_position(
        &w.primary,
        MailboxPosition {
            newest_seen: Some(in_2023()),
            new_done: false,
            backlog_ceiling: Some(in_2023()),
            ..MailboxPosition::default()
        },
    )
    .await?;

    let meter = w.meter().await?;
    assert!(meter.level.is_none(), "level waits for new mail to clear");
    assert_eq!(meter.inbox_count, 4, "the meter still counts");
    assert_eq!(w.fakes.mailbox.calls(MailOp::CountMessages), 0);
    Ok(())
}

#[tokio::test]
async fn gm_04_ac1_level_year_and_remaining() -> TestResult {
    let w = World::new().await?;
    let second = w.add_mailbox("sub-b", "b@example.com", false).await?;
    w.set_position(&w.primary, cleared_at(in_2023())).await?;
    w.set_position(&second, cleared_at(in_2023())).await?;
    // Two in 2023 on the first mailbox, one on the second.
    for _ in 0..2 {
        w.seed(&w.primary, "alice", in_2023());
    }
    w.seed(&second, "bob", in_2023());
    // Neither neighbour year may be counted: 2024 is too new, 2022 too old.
    w.seed(&w.primary, "alice", in_2024());
    w.seed(&second, "bob", in_2022());

    let meter = w.meter().await?;
    let level = meter.level.ok_or("level")?;
    assert_eq!(level.year, 2023);
    assert_eq!(level.remaining, 3);
    assert_eq!(meter.inbox_count, 5);
    assert_eq!(
        w.fakes.mailbox.calls(MailOp::CountMessages),
        2,
        "one date-range count per mailbox, for one year"
    );
    Ok(())
}

#[tokio::test]
async fn gm_04_ac1_year_count_failure_is_data_and_listed_once() -> TestResult {
    let w = World::new().await?;
    let second = w.add_mailbox("sub-b", "b@example.com", false).await?;
    w.set_position(&w.primary, cleared_at(in_2023())).await?;
    w.set_position(&second, cleared_at(in_2023())).await?;
    for _ in 0..2 {
        w.seed(&w.primary, "alice", in_2023());
    }
    w.fakes.mailbox.revoke_token(&access_token_for("sub-b"));

    let meter = w.meter().await?;
    // The dead mailbox fails both the folder total and the date-range count,
    // and is still listed exactly once.
    assert_eq!(meter.mailbox_errors.len(), 1);
    assert_eq!(meter.mailbox_errors[0].mailbox_id, second.0);
    let level = meter.level.ok_or("level")?;
    assert_eq!(level.year, 2023);
    assert_eq!(level.remaining, 2, "the mailbox that answered still counts");
    Ok(())
}

#[tokio::test]
async fn gm_04_ac2_empty_year_moves_to_next_older() -> TestResult {
    let w = World::new().await?;
    w.set_position(&w.primary, cleared_at(in_2023())).await?;
    // Nothing left in 2023; two messages wait in 2022.
    w.seed(&w.primary, "alice", in_2022());
    w.seed(&w.primary, "alice", in_2022());

    let meter = w.meter().await?;
    let level = meter.level.ok_or("level")?;
    assert_eq!(
        level.year, 2022,
        "the empty year is complete, the next begins"
    );
    assert_eq!(level.remaining, 2);
    assert_eq!(
        w.fakes.mailbox.calls(MailOp::CountMessages),
        2,
        "one count for the empty 2023 and one for 2022"
    );
    Ok(())
}

#[tokio::test]
async fn gm_04_ac3_progress_writes_nothing() -> TestResult {
    let w = World::new().await?;
    w.set_position(&w.primary, cleared_at(in_2023())).await?;
    w.seed(&w.primary, "alice", in_2023());

    let store_before = w.fakes.store.export_json();
    // Any write to the Drive app folder in this request would fail loudly.
    w.fakes.app_folder.fail_op(FolderOp::Write, 1);
    let meter = w.meter().await?;

    let level = meter.level.ok_or("level")?;
    assert_eq!(level.year, 2023);
    assert_eq!(level.remaining, 1);
    assert_eq!(meter.inbox_count, 1);
    assert_eq!(
        w.fakes.store.export_json(),
        store_before,
        "a GET leaves the store untouched"
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// The route
// ---------------------------------------------------------------------------

struct Authed {
    cookie: String,
}

async fn authed(w: &World) -> Result<Authed, Box<dyn std::error::Error>> {
    let (cookie, _record) = SessionService::new(&w.app)
        .establish(
            None,
            &w.user,
            Some(w.fakes.clock.now() - Duration::seconds(10)),
        )
        .await?;
    let cookie = cookie
        .0
        .to_str()?
        .split(';')
        .next()
        .unwrap_or_default()
        .to_owned();
    Ok(Authed { cookie })
}

async fn get_progress_route(
    router: &Router,
    cookie: &str,
) -> Result<Response, Box<dyn std::error::Error>> {
    let req = Request::builder()
        .method(Method::GET)
        .uri("/api/v1/progress")
        .header(header::COOKIE, HeaderValue::from_str(cookie)?)
        .body(Body::empty())?;
    Ok(router.clone().oneshot(req).await?)
}

async fn json_of(resp: Response) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await?;
    Ok(serde_json::from_slice(&bytes)?)
}

#[tokio::test]
async fn api_prog_1_get_progress_returns_the_meter() -> TestResult {
    let w = World::new().await?;
    let router = build_router(w.app.clone());
    let a = authed(&w).await?;

    // No position yet: the meter answers, the level is null.
    let resp = get_progress_route(&router, &a.cookie).await?;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        resp.headers()
            .get("ratelimit-policy")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default(),
        "\"reads\";q=120;w=60",
        "a GET spends the other-reads limit (S7 section 6)"
    );
    let json = json_of(resp).await?;
    let keys: BTreeSet<&str> = json
        .as_object()
        .ok_or("object")?
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        keys,
        BTreeSet::from(["inbox_count", "mailbox_errors", "level"])
    );
    assert!(json["level"].is_null());
    assert!(json["mailbox_errors"].as_array().is_some_and(Vec::is_empty));

    // With a stored position and mail in 2023, the level shows the year and
    // what is left.
    w.set_position(&w.primary, cleared_at(in_2023())).await?;
    w.seed(&w.primary, "alice", in_2023());
    let resp = get_progress_route(&router, &a.cookie).await?;
    assert_eq!(resp.status(), StatusCode::OK);
    let json = json_of(resp).await?;
    assert_eq!(json["inbox_count"], 1);
    assert_eq!(json["level"]["year"], 2023);
    assert_eq!(json["level"]["remaining"], 1);
    let level_keys: BTreeSet<&str> = json["level"]
        .as_object()
        .ok_or("level object")?
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(level_keys, BTreeSet::from(["year", "remaining"]));
    Ok(())
}
