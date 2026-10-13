//! T-906b: JEV-1 and CL-02 AC2 end to end, against the real classifiers and the
//! `fake-vertex` / `fake-jev` servers (S10 9.2 JEV-1, S2 CL-02 AC2, S2 CL-04
//! AC1; S5 `config/classifiers`).
//!
//! The `api` service runs in process with `GeminiClassifier` pointed at
//! `fake-vertex` and `JevClassifier` at `fake-jev` through one test `HttpEgress`
//! that routes ONLY those two model hosts: any other call is recorded as a
//! violation, so a regression fails twice. Each scenario triages the whole
//! corpus Feed and then asserts what the fakes saw.

#![allow(
    clippy::too_many_lines,
    clippy::items_after_statements,
    clippy::similar_names,
    clippy::needless_pass_by_value
)]

use std::sync::Arc;

use adapters_gcp::gemini::{GcpTokenSource, GeminiClassifier, GeminiConfig};
use adapters_gcp::token_source::StaticTokenSource;
use adapters_models::jev::{JevClassifier, JevConfig};
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
use axum::Router;
use domain::{EmailAddress, MailboxId, MailboxStatus, Provider, ProviderSubjectId, UserId};
use obs::Sensitive;
use ports::store::aad_fields;
use ports::{
    Aad, Ciphertext, Classifier, ClassifiersConfig, Clock, KeyService, MailboxRecord, Precondition,
    Rng, ServerStore, SessionHash, SessionRecordId, UserRecord,
};
use testkit::fake_jev::{FakeJev, FakeJevHandle, TEST_KEY};
use testkit::fake_vertex::{FakeVertexHandle, MODEL_VERSION};
use testkit::{fake_ports, EgressRecord, FakeHttpEgress, Fakes};
use time::Duration;
use tower::ServiceExt;
use uuid::Uuid;

type TestResult = Result<(), Box<dyn std::error::Error>>;

const ORIGIN: &str = "https://mailtinder.test";
const EMAIL_KEY: &[u8] = b"fake-email-key";
const LOG_KEY: &[u8] = b"fake-log-key";
const PROJECT: &str = "demo-mailtinder";
/// The regional Vertex host the classifier uses in production (S4 5.1).
const VERTEX_HOST: &str = "us-central1-aiplatform.googleapis.com";
/// The `TypeSafe` host the Jev classifier uses in production (S4 5.1).
const JEV_HOST: &str = "api.typesafe.ai";

fn config() -> Result<ApiConfig, Box<dyn std::error::Error>> {
    Ok(ApiConfig::new(
        ORIGIN.to_owned(),
        "fake-client".into(),
        Sensitive::new(LOG_KEY.to_vec()),
        Sensitive::new(EMAIL_KEY.to_vec()),
    )?)
}

fn both_on() -> EnvSwitches {
    EnvSwitches {
        gemini: true,
        jev: true,
    }
}

struct World {
    app: AppState,
    fakes: Fakes,
    user: UserId,
    primary: MailboxId,
    /// The egress the model classifiers use; every host but the two model hosts
    /// is refused and recorded.
    egress: Arc<FakeHttpEgress>,
    vertex: FakeVertexHandle,
    fake_jev: FakeJevHandle,
}

impl World {
    /// The whole corpus is triaged by the Feed; the model hosts are the two
    /// fakes and nothing else.
    async fn new(
        consent: bool,
        switches: EnvSwitches,
        document: Option<(bool, bool)>,
    ) -> Result<World, Box<dyn std::error::Error>> {
        let (ports, fakes) = fake_ports();
        let vertex = testkit::fake_vertex::start().await?;
        let fake_jev = FakeJev::start().await?;
        let egress = Arc::new(FakeHttpEgress::new());
        egress.forward(VERTEX_HOST, vertex.addr());
        egress.forward(JEV_HOST, fake_jev.addr());

        let tokens: Arc<dyn GcpTokenSource> =
            Arc::new(StaticTokenSource(Sensitive::new("test-token".to_owned())));
        let gemini = GeminiClassifier::new(
            GeminiConfig::new(PROJECT.to_owned(), MODEL_VERSION.to_owned()),
            Arc::clone(&egress) as Arc<dyn ports::HttpEgress>,
            tokens,
        );
        let jev = JevClassifier::new(
            JevConfig::production()?,
            Arc::clone(&egress) as Arc<dyn ports::HttpEgress>,
            Sensitive::new(TEST_KEY.to_owned()),
        );
        let set = ClassifierSet {
            gemini: Some(Arc::new(gemini) as Arc<dyn Classifier>),
            jev: Some(Arc::new(jev) as Arc<dyn Classifier>),
        };

        let mut app = app_state_with_classifiers(Arc::new(ports), Arc::new(config()?), set);
        // The start-up environment as the scenario needs it (CLASSIFIER_*).
        app.switches = Arc::new(ClassifierSwitches::new(switches));

        let user = seed_user(&fakes, consent).await?;
        let primary = seed_mailbox(&fakes, user).await?;
        grant(&app, &fakes, user, &primary).await?;
        if let Some((gemini, jev)) = document {
            seed_document(&fakes, gemini, jev).await?;
        }
        Ok(World {
            app,
            fakes,
            user,
            primary,
            egress,
            vertex,
            fake_jev,
        })
    }

    /// Seed the whole corpus mailbox (S10 9.2 JEV-1).
    fn seed_corpus(&self) -> Result<usize, Box<dyn std::error::Error>> {
        let corpus = testkit::corpus::load()?;
        for case in &corpus.cases {
            self.fakes.mailbox.seed(&self.primary, case.seed_message());
        }
        Ok(corpus.cases.len())
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

    /// One whole-corpus Feed request.
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

    /// Requests `fake-vertex` recorded (Gemini).
    fn gemini_requests(&self) -> usize {
        self.vertex.requests().len()
    }

    /// Requests `fake-jev` recorded (Jev).
    fn jev_requests(&self) -> usize {
        self.fake_jev.requests().len()
    }

    /// The egress records for one model host: the guard the scenarios assert
    /// through (an unrouted host is recorded too, with `allowed: false`).
    fn guarded(&self, host: &str) -> Vec<EgressRecord> {
        self.egress
            .records()
            .into_iter()
            .filter(|r| r.host == host)
            .collect()
    }

    /// Any egress call to a host that is not one of the two model fakes.
    fn violations(&self) -> Vec<EgressRecord> {
        self.egress.violations()
    }

    /// Stand in for a second `api` instance: flip the document without touching
    /// this instance's cache, so the change lands on the next check.
    async fn flip_from_another_instance(
        &self,
        gemini: bool,
        jev: bool,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let current = self
            .fakes
            .store
            .config()
            .get_classifiers()
            .await?
            .ok_or("a document exists")?;
        self.fakes
            .store
            .config()
            .put_classifiers(
                &ClassifiersConfig {
                    gemini_enabled: gemini,
                    jev_enabled: jev,
                    updated_at: self.fakes.clock.now(),
                },
                Precondition::Matches(current.version),
            )
            .await?;
        Ok(())
    }

    fn router(&self) -> Router {
        build_router(self.app.clone())
    }

    /// The API-EXP-2 opt-out, through the real router.
    async fn opt_out(&self) -> Result<StatusCode, Box<dyn std::error::Error>> {
        let (cookie, record) = SessionService::new(&self.app)
            .establish(None, &self.user, Some(self.fakes.clock.now()))
            .await?;
        let cookie = cookie
            .0
            .to_str()?
            .split(';')
            .next()
            .unwrap_or_default()
            .to_owned();
        let req = Request::builder()
            .method(Method::PUT)
            .uri("/api/v1/me/experiments")
            .header(header::ORIGIN, ORIGIN)
            .header(header::COOKIE, cookie)
            .header("x-csrf-token", record.record.csrf_token.as_str())
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                r#"{"classifier_bakeoff":{"opted_in":false,"consent_version":null}}"#,
            ))?;
        let resp = self.router().oneshot(req).await?;
        Ok(resp.status())
    }
}

async fn seed_user(fakes: &Fakes, consent: bool) -> Result<UserId, Box<dyn std::error::Error>> {
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
    let subject = ProviderSubjectId::new("sub-jev")?;
    let address = EmailAddress::parse("jev@example.com")?;
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
    let token = "refresh-jev".to_owned();
    app.tokens
        .store_refresh_token(app, &user, mailbox, Sensitive::new(token.clone()))
        .await?;
    fakes
        .identity
        .script_refresh(&token, Ok(format!("access-for-{token}")));
    Ok(())
}

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

// ---------------------------------------------------------------------------
// JEV-1 scenario 1 and S10 9.2: a user without consent triages the whole
// corpus and both fakes record zero requests.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn jev_1_no_model_call_without_consent() -> TestResult {
    let w = World::new(false, both_on(), Some((true, true))).await?;
    let cases = w.seed_corpus()?;
    let page = w.feed().await?;
    assert!(!page.cards.is_empty(), "the corpus Feed serves cards");
    assert!(
        page.cards.len() <= cases,
        "every card comes from the corpus"
    );
    assert_eq!(w.gemini_requests(), 0, "fake-vertex saw no request");
    assert_eq!(w.jev_requests(), 0, "fake-jev saw no request");
    Ok(())
}

// ---------------------------------------------------------------------------
// CL-02 AC2: the same run, asserted through the egress guard: nothing reached
// the wire, and no host outside the two model hosts was tried.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn cl_02_ac2_no_model_call_without_consent_end_to_end() -> TestResult {
    let w = World::new(false, both_on(), Some((true, true))).await?;
    w.seed_corpus()?;
    let page = w.feed().await?;
    assert!(!page.cards.is_empty());
    assert!(
        w.guarded(VERTEX_HOST).is_empty(),
        "no Vertex call reaches the egress"
    );
    assert!(
        w.guarded(JEV_HOST).is_empty(),
        "no TypeSafe call reaches the egress"
    );
    assert!(
        w.violations().is_empty(),
        "no host outside the model allowlist was tried"
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// JEV-1 scenario 2: `CLASSIFIER_JEV_ENABLED=false` at start-up means zero Jev
// requests while Gemini carries on.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn jev_1_env_kill_switch_stops_model() -> TestResult {
    let env = EnvSwitches {
        gemini: true,
        jev: false,
    };
    let w = World::new(true, env, Some((true, true))).await?;
    w.seed_corpus()?;
    let page = w.feed().await?;
    assert!(!page.cards.is_empty());
    assert_eq!(w.jev_requests(), 0, "the Jev switch was off at start-up");
    assert!(w.gemini_requests() > 0, "Gemini still classifies");
    Ok(())
}

// ---------------------------------------------------------------------------
// JEV-1 scenario 3: another instance flips the document; after the next check
// (60 s) this instance stops calling Jev.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn jev_1_config_flip_stops_model_after_next_check() -> TestResult {
    let w = World::new(true, both_on(), Some((true, true))).await?;
    w.seed_corpus()?;
    let first = w.feed().await?;
    assert!(!first.cards.is_empty());
    let gemini_before = w.gemini_requests();
    let jev_before = w.jev_requests();
    assert!(gemini_before > 0 && jev_before > 0, "both models ran");

    // A second api instance writes the document: this instance's cache is not
    // invalidated, so it keeps the old value until the next check.
    w.flip_from_another_instance(true, false).await?;
    let inside_window = w.feed().await?;
    assert!(!inside_window.cards.is_empty());
    assert!(
        w.jev_requests() > jev_before,
        "inside SWITCH_REFRESH the cached switch still allows Jev"
    );

    // Advance the clock past the refresh: the next Feed request re-reads the
    // document and stops calling Jev, while Gemini carries on.
    w.fakes.clock.advance(Duration::seconds(61));
    let gemini_after_window = w.gemini_requests();
    let jev_after_window = w.jev_requests();
    let third = w.feed().await?;
    assert!(!third.cards.is_empty());
    assert_eq!(
        w.jev_requests(),
        jev_after_window,
        "no further Jev request after the next check"
    );
    assert!(
        w.gemini_requests() > gemini_after_window,
        "Gemini carries on"
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// JEV-1 scenario 4: consent withdrawn mid-session stops both models after the
// opt-out response.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn jev_1_consent_withdrawn_mid_session() -> TestResult {
    let w = World::new(true, both_on(), Some((true, true))).await?;
    w.seed_corpus()?;
    let first = w.feed().await?;
    assert!(!first.cards.is_empty());
    assert!(w.gemini_requests() > 0 && w.jev_requests() > 0);

    assert_eq!(w.opt_out().await?, StatusCode::OK);
    let gemini_after = w.gemini_requests();
    let jev_after = w.jev_requests();
    let second = w.feed().await?;
    assert!(!second.cards.is_empty(), "the Feed still serves cards");
    assert_eq!(
        w.gemini_requests(),
        gemini_after,
        "no Gemini request after the opt-out"
    );
    assert_eq!(
        w.jev_requests(),
        jev_after,
        "no Jev request after the opt-out"
    );
    Ok(())
}
