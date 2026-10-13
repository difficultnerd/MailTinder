//! T-906b kill-switch service integration tests: API-ADM-8 / API-ADM-9 and the
//! Feed gate they drive (S2 CL-04 AC1, AC8; CL-02 AC2; AU-01 AC5; S5
//! `config/classifiers` CFG-1; S7 5.13; ASVS V8.2.1, V16.3.3).
//!
//! Everything runs through the real router and the real Feed service on the
//! in-memory fakes, the virtual clock and scripted `FakeClassifier`s, so a
//! switch can be flipped and the next request observed without a deploy. The
//! JEV-1 journeys against `fake-vertex`/`fake-jev` live in
//! `tests/jev_1_no_model_call.rs`.

#![allow(
    clippy::too_many_lines,
    clippy::items_after_statements,
    clippy::similar_names,
    clippy::needless_pass_by_value
)]

use std::sync::Arc;

use api::classify::switches::{ClassifierSwitches, EnvSwitches};
use api::classify::ClassifierSet;
use api::experiments::CURRENT_CONSENT_VERSION;
use api::routes::feed::{FeedPage, FeedRequest};
use api::services::feed::next_page;
use api::session::extract::AuthedSession;
use api::session::store::SessionService;
use api::state::AppState;
use api::{app_state_with_classifiers, build_router, config::ApiConfig};
use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use axum::response::Response;
use axum::Router;
use domain::{
    Classification, EmailAddress, MailboxId, MailboxStatus, MessageClass, MessageId, Provider,
    ProviderSubjectId, UserId,
};
use obs::{Pseudonymiser, Sensitive};
use ports::store::aad_fields;
use ports::{
    Aad, Ciphertext, Classifier, ClassifiersConfig, Clock, KeyService, MailboxRecord, Precondition,
    Rng, ServerStore, SessionHash, SessionRecordId, UserRecord,
};
use testkit::fake_classifier::{FakeClassifier, Scripted};
use testkit::{fake_ports, Fakes, SeedMessage};
use time::{Duration, OffsetDateTime};
use tower::ServiceExt;
use uuid::Uuid;

type TestResult = Result<(), Box<dyn std::error::Error>>;

const ORIGIN: &str = "https://mailtinder.test";
const EMAIL_KEY: &[u8] = b"fake-email-key";
const LOG_KEY: &[u8] = b"fake-log-key";
const BASE: i64 = 1_790_000_000;
/// The step-up window is 5 minutes (S6 4); this is one second past it.
const JUST_PAST_STEP_UP: Duration = Duration::seconds(301);

/// The scoped log capture is per-thread; serialise the tests that assert on a
/// captured security event so a neighbour cannot blank it out.
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn config() -> Result<ApiConfig, Box<dyn std::error::Error>> {
    Ok(ApiConfig::new(
        ORIGIN.to_owned(),
        "fake-client".into(),
        Sensitive::new(LOG_KEY.to_vec()),
        Sensitive::new(EMAIL_KEY.to_vec()),
    )?)
}

fn at(offset_s: i64) -> OffsetDateTime {
    // Every fixture timestamp is inside the representable range.
    OffsetDateTime::from_unix_timestamp(BASE + offset_s).unwrap_or(OffsetDateTime::UNIX_EPOCH)
}

fn answer() -> Classification {
    Classification {
        class: MessageClass::List,
        bulk_score: 40,
        bulk_reason: "model".to_owned(),
        confidence: Some(0.8),
        probabilities: Some([0.2; 5]),
    }
}

/// The pseudonymous ID a log line carries for `user`, computed with the same
/// log key the fixture hands the app (S5 logs).
fn pseudo_of(user: &UserId) -> String {
    Pseudonymiser::new(Sensitive::new(LOG_KEY.to_vec()))
        .pseudo_id(&user.0)
        .as_str()
        .to_owned()
}

struct World {
    app: AppState,
    fakes: Fakes,
    /// The user whose Feed the gate governs.
    user: UserId,
    /// The admin who flips the switches.
    admin: UserId,
    primary: MailboxId,
    gemini: Arc<FakeClassifier>,
    jev: Arc<FakeClassifier>,
}

impl World {
    /// A world with both models wired, both start-up switches on, an optional
    /// `config/classifiers` document, a consenting or non-consenting user and
    /// one seeded message.
    async fn new(
        consent: bool,
        switches: EnvSwitches,
        document: Option<(bool, bool)>,
    ) -> Result<World, Box<dyn std::error::Error>> {
        let (ports, fakes) = fake_ports();
        let gemini = Arc::new(FakeClassifier::new("gemini@flash-lite"));
        let jev = Arc::new(FakeClassifier::new("jev@1.13.0"));
        gemini.script_all(Scripted::Answer(answer()));
        jev.script_all(Scripted::Answer(answer()));
        let set = ClassifierSet {
            gemini: Some(Arc::clone(&gemini) as Arc<dyn Classifier>),
            jev: Some(Arc::clone(&jev) as Arc<dyn Classifier>),
        };
        let mut app = app_state_with_classifiers(Arc::new(ports), Arc::new(config()?), set);
        // The start-up environment as the task's cases need it; the document is
        // the other half of the effective switch.
        app.switches = Arc::new(ClassifierSwitches::new(switches));

        let user = seed_user(&fakes, false, consent).await?;
        let admin = seed_user(&fakes, true, false).await?;
        let primary = seed_mailbox(&fakes, user).await?;
        grant(&app, &fakes, user, &primary).await?;
        if let Some((gemini, jev)) = document {
            seed_document(&fakes, gemini, jev).await?;
        }
        Ok(World {
            app,
            fakes,
            user,
            admin,
            primary,
            gemini,
            jev,
        })
    }

    /// The Feed service for `user`'s whole mailbox.
    async fn feed(&self) -> Result<FeedPage, api::error::ApiError> {
        next_page(
            &self.app,
            &self.session(),
            FeedRequest {
                cursor: None,
                limit: 50,
                refresh: false,
            },
        )
        .await
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

    fn seed(&self, sender: &str, offset_s: i64) -> MessageId {
        self.fakes.mailbox.seed(
            &self.primary,
            SeedMessage {
                from_display: format!("Name of {sender}"),
                from_address: format!("{sender}@example.com"),
                subject: format!("Subject at {offset_s}"),
                raw_headers: vec![],
                facts: domain::HeaderFacts::default(),
                preview_text: format!("Preview at {offset_s}"),
                internal_date: at(offset_s),
                labels: vec!["INBOX".to_owned()],
            },
        )
    }

    /// A signed-in session for `user`; `recent_auth_at` is the step-up state.
    async fn authed(
        &self,
        user: UserId,
        recent_auth_at: Option<OffsetDateTime>,
    ) -> Result<Authed, Box<dyn std::error::Error>> {
        let (cookie, record) = SessionService::new(&self.app)
            .establish(None, &user, recent_auth_at)
            .await?;
        Ok(Authed {
            cookie: cookie
                .0
                .to_str()?
                .split(';')
                .next()
                .unwrap_or_default()
                .to_owned(),
            csrf: record.record.csrf_token.clone(),
        })
    }

    fn router(&self) -> Router {
        build_router(self.app.clone())
    }

    /// `PATCH /api/v1/admin/experiments/classifier` (API-ADM-9).
    async fn patch(
        &self,
        who: &Authed,
        body: &str,
    ) -> Result<Response, Box<dyn std::error::Error>> {
        let req = Request::builder()
            .method(Method::PATCH)
            .uri("/api/v1/admin/experiments/classifier")
            .header(header::ORIGIN, ORIGIN)
            .header(header::COOKIE, who.cookie.as_str())
            .header("x-csrf-token", who.csrf.as_str())
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_owned()))?;
        Ok(self.router().oneshot(req).await?)
    }

    /// `GET /api/v1/admin/experiments/classifier` (API-ADM-8).
    async fn get(&self, who: &Authed) -> Result<Response, Box<dyn std::error::Error>> {
        let req = Request::builder()
            .method(Method::GET)
            .uri("/api/v1/admin/experiments/classifier")
            .header(header::COOKIE, who.cookie.as_str())
            .body(Body::empty())?;
        Ok(self.router().oneshot(req).await?)
    }

    /// The stored document's switches and timestamp.
    async fn document(
        &self,
    ) -> Result<Option<(bool, bool, OffsetDateTime)>, Box<dyn std::error::Error>> {
        Ok(self.fakes.store.config().get_classifiers().await?.map(|v| {
            (
                v.record.gemini_enabled,
                v.record.jev_enabled,
                v.record.updated_at,
            )
        }))
    }
}

struct Authed {
    cookie: String,
    csrf: String,
}

async fn seed_user(
    fakes: &Fakes,
    is_admin: bool,
    consent: bool,
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
                experiments_consent_version: consent.then(|| CURRENT_CONSENT_VERSION.to_owned()),
                experiments_opted_in_at: consent.then(|| fakes.clock.now()),
            },
            Precondition::MustNotExist,
        )
        .await?;
    Ok(user)
}

async fn seed_mailbox(
    fakes: &Fakes,
    user: UserId,
) -> Result<MailboxId, Box<dyn std::error::Error>> {
    let subject = ProviderSubjectId::new("sub-switch")?;
    let address = EmailAddress::parse("switch@example.com")?;
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
    Ok(mailbox_id)
}

/// A working grant, so the Feed can fetch the mailbox.
async fn grant(
    app: &AppState,
    fakes: &Fakes,
    user: UserId,
    mailbox: &MailboxId,
) -> Result<(), Box<dyn std::error::Error>> {
    let token = "refresh-switch".to_owned();
    app.tokens
        .store_refresh_token(app, &user, mailbox, Sensitive::new(token.clone()))
        .await?;
    fakes
        .identity
        .script_refresh(&token, Ok(format!("access-for-{token}")));
    Ok(())
}

/// The `config/classifiers` document every switch case starts from.
async fn seed_document(
    fakes: &Fakes,
    gemini: bool,
    jev: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    fakes
        .store
        .config()
        .put_classifiers(
            &ClassifiersConfig {
                gemini_enabled: gemini,
                jev_enabled: jev,
                updated_at: fakes.clock.now(),
            },
            Precondition::MustNotExist,
        )
        .await?;
    Ok(())
}

async fn json_of(resp: Response) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await?;
    Ok(serde_json::from_slice(&bytes)?)
}

async fn code_of(resp: Response) -> Result<String, Box<dyn std::error::Error>> {
    Ok(json_of(resp).await?["code"]
        .as_str()
        .unwrap_or_default()
        .to_owned())
}

fn both_on() -> EnvSwitches {
    EnvSwitches {
        gemini: true,
        jev: true,
    }
}

/// The parsed `security` lines of a capture (S5 logs).
fn security_events(text: &str) -> Vec<serde_json::Value> {
    let mut events = Vec::new();
    for line in text.lines() {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if value.get("event").and_then(|v| v.as_str()) == Some("security") {
            events.push(value);
        }
    }
    events
}

fn field<'a>(value: &'a serde_json::Value, key: &str) -> &'a str {
    value.get(key).and_then(|v| v.as_str()).unwrap_or_default()
}

// ---------------------------------------------------------------------------
// CL-04 AC1: an admin switches a model off without a deploy; the change applies
// at once on the instance that wrote it, and within a minute elsewhere.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn cl_04_ac1_switch_off_without_deploy() -> TestResult {
    let w = World::new(true, both_on(), Some((true, true))).await?;
    w.seed("list", 10);
    let first = w.feed().await?;
    assert_eq!(first.cards.len(), 1);
    assert!(w.gemini.call_count() > 0, "the first Feed calls Gemini");
    assert!(w.jev.call_count() > 0, "the first Feed calls Jev");
    let jev_before = w.jev.call_count();

    // API-ADM-9, with a fresh sign-in.
    let admin = w.authed(w.admin, Some(w.fakes.clock.now())).await?;
    let resp = w
        .patch(&admin, r#"{"models":[{"model":"gemini","enabled":false}]}"#)
        .await?;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = json_of(resp).await?;
    assert_eq!(body["models"][0]["model"], "gemini");
    assert_eq!(body["models"][0]["enabled"], false);
    assert_eq!(body["models"][1]["model"], "jev");
    assert_eq!(body["models"][1]["enabled"], true);
    assert_eq!(
        body["models"][0]["changed_at"],
        body["models"][1]["changed_at"]
    );

    // The next Feed request on this instance makes no Gemini call at all, and
    // the other model carries on.
    let gemini_after = w.gemini.call_count();
    let second = w.feed().await?;
    assert_eq!(second.cards.len(), 1);
    assert_eq!(
        w.gemini.call_count(),
        gemini_after,
        "no Gemini call after the switch"
    );
    assert!(
        w.jev.call_count() > jev_before,
        "the other model carries on"
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// CL-04 AC8: changing a kill switch needs step-up.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn cl_04_ac8_kill_switch_needs_step_up() -> TestResult {
    let w = World::new(true, both_on(), Some((true, true))).await?;
    let before = w.document().await?;
    let admin = w.authed(w.admin, None).await?;
    let resp = w
        .patch(&admin, r#"{"models":[{"model":"jev","enabled":false}]}"#)
        .await?;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    assert_eq!(code_of(resp).await?, "step_up_required");
    assert_eq!(w.document().await?, before, "the document is unchanged");
    Ok(())
}

// ---------------------------------------------------------------------------
// AU-01 AC5: a step-up older than the five-minute window is refused too.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn au_01_ac5_kill_switch_refused_without_step_up() -> TestResult {
    let w = World::new(true, both_on(), Some((true, true))).await?;
    let before = w.document().await?;
    let stale = w.fakes.clock.now() - JUST_PAST_STEP_UP;
    let admin = w.authed(w.admin, Some(stale)).await?;
    let resp = w
        .patch(&admin, r#"{"models":[{"model":"gemini","enabled":false}]}"#)
        .await?;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    assert_eq!(code_of(resp).await?, "step_up_required");
    assert_eq!(w.document().await?, before, "the document is unchanged");
    Ok(())
}

// ---------------------------------------------------------------------------
// CFG-1: the document holds only its three allowed fields.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn cfg_1_config_doc_holds_only_allowed_fields() -> TestResult {
    let w = World::new(true, both_on(), Some((true, true))).await?;
    let admin = w.authed(w.admin, Some(w.fakes.clock.now())).await?;
    w.fakes.clock.advance(Duration::seconds(30));
    let resp = w
        .patch(
            &admin,
            r#"{"models":[{"model":"gemini","enabled":false},{"model":"jev","enabled":true}]}"#,
        )
        .await?;
    assert_eq!(resp.status(), StatusCode::OK);

    let stored = w
        .fakes
        .store
        .config()
        .get_classifiers()
        .await?
        .ok_or("a document exists after the write")?;
    assert_eq!(
        (stored.record.gemini_enabled, stored.record.jev_enabled),
        (false, true)
    );
    assert_eq!(stored.record.updated_at, w.fakes.clock.now());
    let json = serde_json::to_value(&stored.record)?;
    let mut keys: Vec<String> = json
        .as_object()
        .ok_or("the document is an object")?
        .keys()
        .cloned()
        .collect();
    keys.sort();
    assert_eq!(
        keys,
        ["gemini_enabled", "jev_enabled", "updated_at"],
        "CFG-1: only the three allowed fields"
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// CFG-1: only an admin write with step-up changes the switches.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn cfg_1_only_admin_with_step_up_changes_switches() -> TestResult {
    let w = World::new(true, both_on(), Some((true, true))).await?;
    let before = w.document().await?;
    // A step-up user who is not an admin changes nothing.
    let stranger = seed_user(&w.fakes, false, false).await?;
    let non_admin = w.authed(stranger, Some(w.fakes.clock.now())).await?;
    let resp = w
        .patch(
            &non_admin,
            r#"{"models":[{"model":"gemini","enabled":false}]}"#,
        )
        .await?;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    assert_eq!(code_of(resp).await?, "forbidden");
    assert_eq!(w.document().await?, before, "the document is unchanged");

    // The stepped-up admin does.
    let admin = w.authed(w.admin, Some(w.fakes.clock.now())).await?;
    let resp = w
        .patch(&admin, r#"{"models":[{"model":"gemini","enabled":false}]}"#)
        .await?;
    assert_eq!(resp.status(), StatusCode::OK);
    let after = w.document().await?;
    assert_ne!(after, before, "the admin's write lands");
    Ok(())
}

// ---------------------------------------------------------------------------
// ASVS V8.2.1: neither read nor write is available to a non-admin.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn asvs_v8_2_1_admin_classifier_refused_for_non_admin() -> TestResult {
    let w = World::new(true, both_on(), Some((true, true))).await?;
    let before = w.document().await?;
    let stranger = seed_user(&w.fakes, false, false).await?;
    let non_admin = w.authed(stranger, Some(w.fakes.clock.now())).await?;
    let read = w.get(&non_admin).await?;
    assert_eq!(read.status(), StatusCode::FORBIDDEN);
    assert_eq!(code_of(read).await?, "forbidden");
    let write = w
        .patch(
            &non_admin,
            r#"{"models":[{"model":"jev","enabled":false}]}"#,
        )
        .await?;
    assert_eq!(write.status(), StatusCode::FORBIDDEN);
    assert_eq!(code_of(write).await?, "forbidden");
    assert_eq!(w.document().await?, before, "the document is unchanged");
    Ok(())
}

// ---------------------------------------------------------------------------
// ASVS V16.3.3 / S6 7: every kill-switch change is logged, pseudonymously, with
// the model and its new value.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn asvs_v16_3_3_kill_switch_change_logged() -> TestResult {
    let w = World::new(true, both_on(), Some((true, true))).await?;
    let admin = w.authed(w.admin, Some(w.fakes.clock.now())).await?;
    let clock = obs::arc(obs::FixedClock(w.fakes.clock.now()));
    let (capture, _guard) = obs::capture("api", clock);
    let resp = w
        .patch(&admin, r#"{"models":[{"model":"jev","enabled":false}]}"#)
        .await?;
    assert_eq!(resp.status(), StatusCode::OK);

    let events = security_events(&capture.text());
    let change = events
        .iter()
        .find(|e| field(e, "action") == "kill_switch_change")
        .ok_or("a kill_switch_change event")?;
    assert_eq!(
        field(change, "outcome"),
        "jev_disabled",
        "the model and value"
    );
    assert_eq!(field(change, "user_pseudo"), pseudo_of(&w.admin));
    assert!(
        !field(change, "request_id").is_empty(),
        "the line joins on its request ID"
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// S10 10.4: a PATCH that repeats the current value still writes and logs, so
// the admin sees an effect.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn cfg_1_repeat_value_still_writes_and_logs() -> TestResult {
    let _serial = SERIAL.lock().await;
    let w = World::new(true, both_on(), Some((true, true))).await?;
    let before = w.document().await?;
    let admin = w.authed(w.admin, Some(w.fakes.clock.now())).await?;
    let clock = obs::arc(obs::FixedClock(w.fakes.clock.now()));
    let (capture, _guard) = obs::capture("api", clock);
    w.fakes.clock.advance(Duration::seconds(5));
    let resp = w
        .patch(&admin, r#"{"models":[{"model":"gemini","enabled":true}]}"#)
        .await?;
    assert_eq!(resp.status(), StatusCode::OK);
    let after = w.document().await?;
    assert_eq!(
        after.map(|(g, j, _)| (g, j)),
        before.map(|(g, j, _)| (g, j)),
        "the values are unchanged"
    );
    assert_ne!(
        after.map(|(_, _, at)| at),
        before.map(|(_, _, at)| at),
        "updated_at moves so the admin sees an effect"
    );
    assert!(
        security_events(&capture.text())
            .iter()
            .any(|e| field(e, "outcome") == "gemini_enabled"),
        "the repeat still logs"
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// CL-02 AC2: without consent the Feed calls neither model. The egress-guard
// half of this AC runs against the real classifiers in
// `tests/jev_1_no_model_call.rs` (`cl_02_ac2_no_model_call_without_consent_end_to_end`).
// ---------------------------------------------------------------------------
#[tokio::test]
async fn cl_02_ac2_feed_gate_stays_closed_without_consent() -> TestResult {
    let w = World::new(false, both_on(), Some((true, true))).await?;
    w.seed("list", 10);
    let page = w.feed().await?;
    assert_eq!(page.cards.len(), 1);
    assert_eq!(w.gemini.call_count(), 0, "no Gemini call without consent");
    assert_eq!(w.jev.call_count(), 0, "no Jev call without consent");
    Ok(())
}
