//! T-906a evaluation records (CL-03 AC1 to AC3, BAKE-6, EXP-1).
//!
//! Service integration: the Feed and swipe services run on the in-memory fakes,
//! scripted `FakeClassifier`s and the virtual clock, so a card's sealed token
//! carries real predictions and every record is read back out of the store.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::too_many_lines,
    clippy::similar_names,
    clippy::items_after_statements,
    clippy::missing_panics_doc,
    clippy::needless_pass_by_value,
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::assert_is_empty,
    clippy::bool_assert_comparison
)]

use std::sync::Arc;

use api::classify::eval::{build_eval_record, eval_id_for, EVAL_NAMESPACE, EVAL_TTL_DAYS};
use api::classify::{eval_header_facts, prediction_ok, BakeoffGate, BakeoffPayload, ClassifierSet};
use api::experiments::CURRENT_CONSENT_VERSION;
use api::routes::feed::{ClassificationPayload, FeedRequest};
use api::routes::swipes::{ActionDto, SwipeRequest};
use api::sealed::{SealedTokens, TokenType};
use api::services::feed::next_page;
use api::services::swipe::{swipe, UndoPayload};
use api::services::undo::undo;
use api::session::extract::AuthedSession;
use api::{app_state, config::ApiConfig};
use domain::bakeoff::labels::{label_for, Label};
use domain::{
    derive_swipe_ids, Classification, EmailAddress, HeaderFacts, MailboxId, MessageClass,
    MessageId, Provider, ProviderSubjectId, SwipeOutcome, UserId, HEADER_RULES_ID,
};
use obs::{Pseudonymiser, Sensitive};
use ports::store::aad_fields;
use ports::{
    Aad, AgeBucket, Ciphertext, Classifier, ClassifierEvalRecord, ClassifierId, Clock, EvalId,
    KeyService, MailboxRecord, ModelErrorCode, ModelPrediction, PageRequest, Precondition, Rng,
    ServerStore, SessionHash, SessionRecordId, SwipeDirection as PortsDirection, TextTokensBucket,
    UserRecord, WrappedKey,
};
use testkit::corpus::{load, Corpus};
use testkit::fake_classifier::{FakeClassifier, Scripted};
use testkit::{fake_ports, Fakes, SeedMessage};
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// Both models open for this request (T-901's injected gate).
const OPEN: BakeoffGate = BakeoffGate {
    gemini: true,
    jev: true,
};

fn key(n: u128) -> Uuid {
    Uuid::from_u128(n)
}

fn answer(class: MessageClass) -> Classification {
    Classification {
        class,
        bulk_score: 40,
        bulk_reason: "model".to_owned(),
        confidence: Some(0.8),
        probabilities: Some([0.2; 5]),
    }
}

/// A mailing list with a DKIM-covered one-click unsubscribe.
fn list_facts() -> HeaderFacts {
    HeaderFacts {
        list_unsubscribe: Some(domain::UnsubscribeOptions {
            one_click_https: Some(url::Url::parse("https://lists.example.com/u?x=1").unwrap()),
            https: None,
            mailto: None,
        }),
        list_unsubscribe_present: true,
        list_id: Some("news.example.com".to_owned()),
        from_authenticated: true,
        ..HeaderFacts::default()
    }
}

/// One plain message: a reject trashes it without creating a job or a rule.
fn plain_facts() -> HeaderFacts {
    HeaderFacts {
        from_authenticated: true,
        ..HeaderFacts::default()
    }
}

fn pair(
    script_g: Scripted,
    script_j: Scripted,
) -> (Arc<FakeClassifier>, Arc<FakeClassifier>, ClassifierSet) {
    let g = Arc::new(FakeClassifier::new("gemini@test"));
    let j = Arc::new(FakeClassifier::new("jev@test"));
    g.script_all(script_g);
    j.script_all(script_j);
    let gemini: Arc<dyn Classifier> = g.clone();
    let jev: Arc<dyn Classifier> = j.clone();
    (
        g,
        j,
        ClassifierSet {
            gemini: Some(gemini),
            jev: Some(jev),
        },
    )
}

/// The ports direction of a `domain` label direction (the two are separate
/// types, so a conversion is needed where they meet).
fn ports_direction(d: domain::bakeoff::labels::SwipeDirection) -> PortsDirection {
    match d {
        domain::bakeoff::labels::SwipeDirection::Left => PortsDirection::Left,
        domain::bakeoff::labels::SwipeDirection::Right => PortsDirection::Right,
        domain::bakeoff::labels::SwipeDirection::Up => PortsDirection::Up,
        domain::bakeoff::labels::SwipeDirection::Down => PortsDirection::Down,
    }
}

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

struct World {
    app: api::state::AppState,
    fakes: Fakes,
    user: UserId,
    primary: MailboxId,
}

fn config() -> Result<ApiConfig, Box<dyn std::error::Error>> {
    Ok(ApiConfig::new(
        "https://mailtinder.test".to_owned(),
        "fake-client".into(),
        Sensitive::new(b"fake-log-key".to_vec()),
        Sensitive::new(b"fake-email-key".to_vec()),
    )?)
}

impl World {
    async fn new(consent: bool) -> Result<World, Box<dyn std::error::Error>> {
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
                    wrapped_data_key: wrapped.clone(),
                    experiments_consent_version: consent
                        .then(|| CURRENT_CONSENT_VERSION.to_owned()),
                    experiments_opted_in_at: consent.then(|| fakes.clock.now()),
                },
                Precondition::MustNotExist,
            )
            .await?;
        let subject = ProviderSubjectId::new("sub-eval")?;
        let address = EmailAddress::parse("eval@example.com")?;
        let primary = ports::mailbox_id_for(Provider::Gmail, &subject);
        let aad = Aad {
            user,
            scope: primary.0.to_string(),
            field: aad_fields::MAILBOX_EMAIL,
        };
        let sealed = fakes
            .keys
            .seal(&user, &wrapped, &aad, address.as_str().as_bytes())
            .await?;
        fakes
            .store
            .mailboxes()
            .put(
                &MailboxRecord {
                    mailbox_id: primary,
                    user_id: user,
                    provider: Provider::Gmail,
                    provider_subject_id: subject,
                    email_address: Ciphertext(sealed),
                    status: domain::MailboxStatus::Connected,
                    linked_at: fakes.clock.now(),
                    is_primary: true,
                    refresh_token: None,
                },
                Precondition::MustNotExist,
            )
            .await?;
        let token = "refresh-sub-eval".to_owned();
        app.tokens
            .store_refresh_token(&app, &user, &primary, Sensitive::new(token.clone()))
            .await?;
        fakes
            .identity
            .script_refresh(&token, Ok(format!("access-for-{token}")));
        Ok(World {
            app,
            fakes,
            user,
            primary,
        })
    }

    fn session(&self, n: u128) -> AuthedSession {
        AuthedSession {
            user: self.user,
            session_record_id: SessionRecordId(Uuid::from_u128(n)),
            is_admin: false,
            recent_auth_at: None,
            session_hash: SessionHash([n as u8; 32]),
        }
    }

    /// Turn the bake-off on for this request, so the served token carries both
    /// models' predictions even when the user's consent record says otherwise.
    fn open_gate(&mut self, set: ClassifierSet) {
        self.app.classifiers = set;
        self.app.bakeoff_gate = OPEN;
    }

    fn seed_message(&self, seed: SeedMessage) -> MessageId {
        self.fakes.mailbox.seed(&self.primary, seed)
    }

    fn seed(&self, sender: &str, facts: HeaderFacts, offset_s: i64) -> MessageId {
        self.seed_message(SeedMessage {
            from_display: format!("Name of {sender}"),
            from_address: format!("{sender}@example.com"),
            subject: format!("Subject from {sender}"),
            raw_headers: vec![],
            facts,
            preview_text: format!("Preview from {sender}"),
            internal_date: self.fakes.clock.now() - Duration::seconds(offset_s),
            labels: vec!["INBOX".to_owned()],
        })
    }

    /// The Feed page's `(message_id, classification_token)` pairs.
    async fn cards(
        &self,
        session: &AuthedSession,
        limit: u32,
    ) -> Result<Vec<(String, String)>, Box<dyn std::error::Error>> {
        let page = next_page(
            &self.app,
            session,
            FeedRequest {
                cursor: None,
                limit,
                refresh: false,
            },
        )
        .await?;
        Ok(page
            .cards
            .into_iter()
            .map(|card| (card.message_id, card.classification_token))
            .collect())
    }

    fn sealer(&self) -> SealedTokens {
        SealedTokens::new(
            Arc::clone(&self.app.ports.keys),
            Arc::clone(&self.app.ports.clock),
        )
    }

    async fn wrapped(&self) -> Result<WrappedKey, Box<dyn std::error::Error>> {
        Ok(self
            .fakes
            .store
            .users()
            .get(&self.user)
            .await?
            .ok_or("user")?
            .record
            .wrapped_data_key)
    }

    async fn open_payload(
        &self,
        session: &AuthedSession,
        token: &str,
    ) -> Result<ClassificationPayload, Box<dyn std::error::Error>> {
        Ok(self
            .sealer()
            .open::<ClassificationPayload>(
                TokenType::Classification,
                &self.user,
                &self.wrapped().await?,
                &session.session_record_id,
                token,
            )
            .await?)
    }

    async fn open_undo(
        &self,
        session: &AuthedSession,
        token: &str,
    ) -> Result<UndoPayload, Box<dyn std::error::Error>> {
        Ok(self
            .sealer()
            .open::<UndoPayload>(
                TokenType::Undo,
                &self.user,
                &self.wrapped().await?,
                &session.session_record_id,
                token,
            )
            .await?)
    }

    /// Seal a card token carrying `gemini` and `jev` by hand, for the tests that
    /// need a fixed payload.
    async fn seal(
        &self,
        session: &AuthedSession,
        id: &str,
        class: MessageClass,
        gemini: Option<ModelPrediction>,
        jev: Option<ModelPrediction>,
        issued_at: OffsetDateTime,
    ) -> Result<String, Box<dyn std::error::Error>> {
        let payload = ClassificationPayload {
            mailbox_id: self.primary.0,
            message_id: id.to_owned(),
            header_rules: Classification {
                class,
                bulk_score: 90,
                bulk_reason: "rules".to_owned(),
                confidence: None,
                probabilities: None,
            },
            classifier_id: HEADER_RULES_ID.to_owned(),
            issued_at,
            bakeoff: Some(BakeoffPayload {
                gemini,
                jev,
                header_facts: eval_header_facts(&list_facts()),
                provider: Provider::Gmail,
                age_bucket: AgeBucket::Under7d,
                text_tokens_bucket: TextTokensBucket::Under100,
                lang_is_english: true,
                input_version: "1".to_owned(),
                question_version: "1".to_owned(),
                price_version: "1".to_owned(),
            }),
        };
        Ok(self
            .sealer()
            .seal(
                TokenType::Classification,
                &self.user,
                &self.wrapped().await?,
                &session.session_record_id,
                self.fakes.clock.now() + Duration::hours(1),
                &payload,
            )
            .await?)
    }

    fn request(&self, id: &str, action: ActionDto, token: String) -> SwipeRequest {
        SwipeRequest {
            mailbox_id: self.primary.0,
            message_id: id.to_owned(),
            action,
            category_id: None,
            new_category_name: None,
            classification_token: token,
        }
    }

    /// Every stored `classifier_eval` record.
    async fn records(&self) -> Result<Vec<ClassifierEvalRecord>, Box<dyn std::error::Error>> {
        let from = OffsetDateTime::UNIX_EPOCH;
        let page = self
            .fakes
            .store
            .classifier_eval()
            .range(
                from,
                from + Duration::days(365_000),
                PageRequest {
                    limit: 100,
                    after: None,
                },
            )
            .await?;
        Ok(page.items)
    }

    async fn record(
        &self,
        k: Uuid,
    ) -> Result<Option<ClassifierEvalRecord>, Box<dyn std::error::Error>> {
        Ok(self
            .fakes
            .store
            .classifier_eval()
            .get(&eval_id_for(&self.user, &k))
            .await?
            .map(|v| v.record))
    }

    /// The current pseudonymous ID the record must carry.
    fn pseudo(&self) -> String {
        Pseudonymiser::new(self.app.config.rate_key.clone())
            .pseudo_id(&self.user.0)
            .as_str()
            .to_owned()
    }

    /// Opt out by clearing both consent fields (T-902, EXP-4).
    async fn opt_out(&self) -> Result<(), Box<dyn std::error::Error>> {
        let mut record = self
            .fakes
            .store
            .users()
            .get(&self.user)
            .await?
            .ok_or("user")?
            .record;
        record.experiments_consent_version = None;
        record.experiments_opted_in_at = None;
        self.fakes
            .store
            .users()
            .put(&record, Precondition::None)
            .await?;
        Ok(())
    }
}

/// A world with both models open and one list message; returns the card.
async fn world_with_card(
    script_g: Scripted,
    script_j: Scripted,
) -> Result<(World, String, String), Box<dyn std::error::Error>> {
    let mut w = World::new(true).await?;
    let (_, _, set) = pair(script_g, script_j);
    w.open_gate(set);
    w.seed("news", list_facts(), 10);
    let session = w.session(1);
    let (id, token) = w
        .cards(&session, 10)
        .await?
        .first()
        .cloned()
        .ok_or("card")?;
    Ok((w, id, token))
}

// ---------------------------------------------------------------------------
// CL-03 AC1, AC2, AC3
// ---------------------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn cl_03_ac1_swipe_records_both_predictions() -> TestResult {
    let (w, id, token) = world_with_card(
        Scripted::Answer(answer(MessageClass::Personal)),
        Scripted::Answer(answer(MessageClass::List)),
    )
    .await?;
    let session = w.session(1);
    // The card was served at one instant and swiped 1500 ms later, so the
    // record's time to swipe comes from the clock, not the client.
    w.fakes.clock.advance(Duration::milliseconds(1500));
    let result = swipe(
        &w.app,
        &session,
        key(1),
        w.request(&id, ActionDto::Keep, token),
    )
    .await?;
    assert_eq!(result.outcome, SwipeOutcome::Kept);

    let records = w.records().await?;
    assert_eq!(records.len(), 1);
    let record = &records[0];
    assert_eq!(record.eval_id, eval_id_for(&w.user, &key(1)));
    assert_eq!(record.user_pseudo_id.0, w.pseudo());
    let gemini = record.gemini.as_ref().ok_or("gemini")?;
    assert_eq!(gemini.class, Some(MessageClass::Personal));
    assert_eq!(gemini.model_version, "gemini@test");
    assert_eq!(gemini.score, Some(40));
    assert!(gemini.error_code.is_none());
    let jev = record.jev.as_ref().ok_or("jev")?;
    assert_eq!(jev.class, Some(MessageClass::List));
    assert_eq!(jev.model_version, "jev@test");
    assert!(jev.error_code.is_none());
    assert_eq!(record.outcome.direction, PortsDirection::Right);
    assert_eq!(record.outcome.time_to_swipe_ms, 1500);
    assert_eq!(record.outcome.undone, false);
    assert_eq!(
        record.expires_at,
        record.created_at + Duration::days(EVAL_TTL_DAYS)
    );
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn cl_03_ac2_failed_model_recorded_with_error_code() -> TestResult {
    // Jev never answers inside the 2 s model timeout; Gemini answers.
    let (w, id, token) = world_with_card(
        Scripted::Answer(answer(MessageClass::Personal)),
        Scripted::Delay(
            std::time::Duration::from_secs(10),
            Box::new(Scripted::Answer(answer(MessageClass::List))),
        ),
    )
    .await?;
    let session = w.session(1);
    let result = swipe(
        &w.app,
        &session,
        key(1),
        w.request(&id, ActionDto::Keep, token),
    )
    .await?;
    assert_eq!(result.outcome, SwipeOutcome::Kept);

    let records = w.records().await?;
    let record = records.first().ok_or("record")?;
    let jev = record.jev.as_ref().ok_or("jev")?;
    assert_eq!(jev.error_code, Some(ModelErrorCode::Timeout));
    assert!(jev.class.is_none() && jev.score.is_none());
    let gemini = record.gemini.as_ref().ok_or("gemini")?;
    assert_eq!(gemini.class, Some(MessageClass::Personal));
    assert_eq!(gemini.error_code, None);
    // The failure is recorded for that model only: both predictions are there.
    assert!(record.gemini.is_some() && record.jev.is_some());
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn cl_03_ac3_one_record_per_swipe() -> TestResult {
    let (w, id, token) = world_with_card(
        Scripted::Answer(answer(MessageClass::List)),
        Scripted::Answer(answer(MessageClass::List)),
    )
    .await?;
    let session = w.session(1);
    swipe(
        &w.app,
        &session,
        key(1),
        w.request(&id, ActionDto::Keep, token),
    )
    .await?;
    let records = w.records().await?;
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].outcome.direction, PortsDirection::Right);
    assert_eq!(Some(records[0].clone()), w.record(key(1)).await?);
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn cl_03_ac3_retried_swipe_writes_one_record() -> TestResult {
    let (w, id, token) = world_with_card(
        Scripted::Answer(answer(MessageClass::List)),
        Scripted::Answer(answer(MessageClass::List)),
    )
    .await?;
    let session = w.session(1);
    let first = swipe(
        &w.app,
        &session,
        key(1),
        w.request(&id, ActionDto::Keep, token.clone()),
    )
    .await?;
    // The same `Idempotency-Key`: the recorded response is replayed.
    let second = swipe(
        &w.app,
        &session,
        key(1),
        w.request(&id, ActionDto::Keep, token),
    )
    .await?;
    assert_eq!(first.outcome, second.outcome);
    assert_eq!(w.records().await?.len(), 1);
    assert!(w.record(key(1)).await?.is_some());
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn cl_03_ac3_record_has_no_sender_subject_text_or_message_id() -> TestResult {
    let (w, id, token) = world_with_card(
        Scripted::Answer(answer(MessageClass::List)),
        Scripted::Answer(answer(MessageClass::List)),
    )
    .await?;
    let session = w.session(1);
    swipe(
        &w.app,
        &session,
        key(1),
        w.request(&id, ActionDto::Keep, token),
    )
    .await?;
    let record = w.record(key(1)).await?.ok_or("record")?;
    let json = serde_json::to_string(&record)?.to_lowercase();
    assert!(!json.contains(&id.to_lowercase()));
    assert!(!json.contains(&w.primary.0.to_string()));
    for leaked in [
        "name of news",
        "news@example.com",
        "subject from news",
        "preview from news",
        "example.com",
    ] {
        assert!(!json.contains(leaked), "record leaked {leaked}");
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// BAKE-6
// ---------------------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn bake_6_swipe_labels_both() -> TestResult {
    let w = World::new(true).await?;
    let session = w.session(1);
    let now = w.fakes.clock.now();
    let gemini = Some(prediction_ok(
        &ClassifierId("gemini@test".to_owned()),
        &answer(MessageClass::List),
        12,
        Some(30),
    ));
    let cases: Vec<(ActionDto, PortsDirection, Label)> = vec![
        (ActionDto::Reject, PortsDirection::Left, Label::Junk),
        (ActionDto::Keep, PortsDirection::Right, Label::Wanted),
        (ActionDto::File, PortsDirection::Up, Label::Wanted),
    ];
    let mut expected = 0;
    for (n, (action, direction, label)) in cases.into_iter().enumerate() {
        let id = w.seed("sender", plain_facts(), 5);
        let token = w
            .seal(
                &session,
                id.as_str(),
                MessageClass::List,
                gemini.clone(),
                None,
                now,
            )
            .await?;
        let mut req = w.request(id.as_str(), action, token);
        if action == ActionDto::File {
            req.new_category_name = Some("Newsletters".to_owned());
        }
        swipe(&w.app, &session, key(0x100 + n as u128), req).await?;
        expected += 1;
        let records = w.records().await?;
        assert_eq!(records.len(), expected);
        // Records created in the same instant are ordered by ID, so the record
        // is looked up by the ID the swipe derives rather than by position.
        let record = w.record(key(0x100 + n as u128)).await?.ok_or("record")?;
        assert_eq!(record.outcome.direction, direction);
        assert_eq!(record.outcome.undone, false);
        // The direction carries the label (S4 5.6, CR-01 1.6), the record
        // itself stores no label.
        assert_eq!(
            label_for(
                match direction {
                    PortsDirection::Left => domain::bakeoff::labels::SwipeDirection::Left,
                    PortsDirection::Right => domain::bakeoff::labels::SwipeDirection::Right,
                    PortsDirection::Up => domain::bakeoff::labels::SwipeDirection::Up,
                    PortsDirection::Down => domain::bakeoff::labels::SwipeDirection::Down,
                },
                record.outcome.undone
            ),
            Some(label)
        );
    }
    assert_eq!(
        ports_direction(domain::bakeoff::labels::SwipeDirection::Left),
        PortsDirection::Left
    );
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn bake_6_undo_flips_same_record() -> TestResult {
    let (w, id, token) = world_with_card(
        Scripted::Answer(answer(MessageClass::Personal)),
        Scripted::Answer(answer(MessageClass::List)),
    )
    .await?;
    let session = w.session(1);
    let result = swipe(
        &w.app,
        &session,
        key(1),
        w.request(&id, ActionDto::Keep, token),
    )
    .await?;
    let before = w.record(key(1)).await?.ok_or("record")?;
    assert_eq!(before.outcome.undone, false);

    // The undo token names the record the swipe wrote.
    let payload = w.open_undo(&session, &result.undo_token).await?;
    assert_eq!(payload.eval_id, Some(before.eval_id.0));

    undo(&w.app, &session, &result.undo_token).await?;
    let after = w.record(key(1)).await?.ok_or("record")?;
    assert_eq!(after.outcome.undone, true);
    assert_eq!(after.eval_id, before.eval_id);
    assert_eq!(after.outcome.direction, before.outcome.direction);
    // One record, flipped: the counts never doubled.
    assert_eq!(w.records().await?.len(), 1);
    assert_eq!(
        label_for(domain::bakeoff::labels::SwipeDirection::Right, true),
        Some(Label::Junk)
    );
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn bake_6_undo_flips_reject_record() -> TestResult {
    // A reject is the Junk label, so its undo must flip the same record too
    // (BAKE-6), not just a keep's.
    let w = World::new(true).await?;
    let session = w.session(1);
    let now = w.fakes.clock.now();
    let id = w.seed("bulk", plain_facts(), 10);
    let gemini = Some(prediction_ok(
        &ClassifierId("gemini@test".to_owned()),
        &answer(MessageClass::List),
        12,
        Some(30),
    ));
    let token = w
        .seal(&session, id.as_str(), MessageClass::List, gemini, None, now)
        .await?;
    let result = swipe(
        &w.app,
        &session,
        key(1),
        w.request(id.as_str(), ActionDto::Reject, token),
    )
    .await?;

    let before = w.record(key(1)).await?.ok_or("record")?;
    assert_eq!(before.outcome.direction, PortsDirection::Left);
    assert_eq!(before.outcome.undone, false);
    // Left is the Junk label (S3, S4 5.6).
    assert_eq!(
        label_for(
            domain::bakeoff::labels::SwipeDirection::Left,
            before.outcome.undone
        ),
        Some(Label::Junk)
    );

    // The undo token names the record the reject wrote.
    let payload = w.open_undo(&session, &result.undo_token).await?;
    assert_eq!(payload.eval_id, Some(before.eval_id.0));

    undo(&w.app, &session, &result.undo_token).await?;
    let after = w.record(key(1)).await?.ok_or("record")?;
    assert_eq!(after.outcome.undone, true);
    assert_eq!(after.eval_id, before.eval_id);
    assert_eq!(after.outcome.direction, before.outcome.direction);
    // One record, flipped: the undo withdrew the Junk label on the same record.
    assert_eq!(w.records().await?.len(), 1);
    assert_eq!(
        label_for(domain::bakeoff::labels::SwipeDirection::Left, true),
        Some(Label::Wanted)
    );
    Ok(())
}

/// A store outage on the eval write is best effort: it must not fail the swipe,
/// and it must leave a usable undo behind (backlog edge case).
#[tokio::test(start_paused = true)]
async fn exp_1_failed_eval_write_leaves_swipe_unaffected() -> TestResult {
    let (w, id, token) = world_with_card(
        Scripted::Answer(answer(MessageClass::Personal)),
        Scripted::Answer(answer(MessageClass::List)),
    )
    .await?;
    let session = w.session(1);
    // The eval store refuses the write; the swipe must still land.
    w.fakes.store.fail_next_classifier_eval_put();
    let result = swipe(
        &w.app,
        &session,
        key(1),
        w.request(&id, ActionDto::Keep, token),
    )
    .await?;
    assert_eq!(result.outcome, SwipeOutcome::Kept);
    // Nothing was recorded, and the undo token names no record.
    assert!(w.records().await?.is_empty());
    assert!(w
        .open_undo(&session, &result.undo_token)
        .await?
        .eval_id
        .is_none());
    // Undoing a swipe that wrote no record still reverses it.
    assert!(undo(&w.app, &session, &result.undo_token).await?.restored);
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn exp_1_failed_eval_write_leaves_reject_unaffected() -> TestResult {
    let w = World::new(true).await?;
    let session = w.session(1);
    let now = w.fakes.clock.now();
    let id = w.seed("bulk", plain_facts(), 10);
    let gemini = Some(prediction_ok(
        &ClassifierId("gemini@test".to_owned()),
        &answer(MessageClass::List),
        12,
        Some(30),
    ));
    let token = w
        .seal(&session, id.as_str(), MessageClass::List, gemini, None, now)
        .await?;
    w.fakes.store.fail_next_classifier_eval_put();
    let result = swipe(
        &w.app,
        &session,
        key(1),
        w.request(id.as_str(), ActionDto::Reject, token),
    )
    .await?;
    assert_eq!(result.outcome, SwipeOutcome::Trashed);
    assert!(w.records().await?.is_empty());
    assert!(w
        .open_undo(&session, &result.undo_token)
        .await?
        .eval_id
        .is_none());
    assert!(undo(&w.app, &session, &result.undo_token).await?.restored);
    Ok(())
}

/// A store outage on the undo flip is best effort: it must not fail the undo,
/// and the record it could not flip is left exactly as it was.
#[tokio::test(start_paused = true)]
async fn bake_6_failed_undo_flip_leaves_undo_unaffected() -> TestResult {
    let (w, id, token) = world_with_card(
        Scripted::Answer(answer(MessageClass::Personal)),
        Scripted::Answer(answer(MessageClass::List)),
    )
    .await?;
    let session = w.session(1);
    let result = swipe(
        &w.app,
        &session,
        key(1),
        w.request(&id, ActionDto::Keep, token),
    )
    .await?;
    let before = w.record(key(1)).await?.ok_or("record")?;
    assert_eq!(before.outcome.undone, false);

    // The store refuses the flip write; the undo must still reverse the swipe.
    w.fakes.store.fail_next_classifier_eval_put();
    assert!(undo(&w.app, &session, &result.undo_token).await?.restored);

    // The flip failed, so the record is unchanged; it still exists, once.
    let after = w.record(key(1)).await?.ok_or("record")?;
    assert_eq!(after.outcome.undone, false);
    assert_eq!(after.eval_id, before.eval_id);
    assert_eq!(w.records().await?.len(), 1);
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn bake_6_down_writes_no_record() -> TestResult {
    let (w, id, token) = world_with_card(
        Scripted::Answer(answer(MessageClass::List)),
        Scripted::Answer(answer(MessageClass::List)),
    )
    .await?;
    let session = w.session(1);
    let result = swipe(
        &w.app,
        &session,
        key(1),
        w.request(&id, ActionDto::Skip, token),
    )
    .await?;
    assert_eq!(result.outcome, SwipeOutcome::Skipped);
    assert!(w.records().await?.is_empty());
    assert!(w.record(key(1)).await?.is_none());
    Ok(())
}

// ---------------------------------------------------------------------------
// EXP-1
// ---------------------------------------------------------------------------

/// Every string a record must never hold: the corpus canaries, addresses,
/// domains, subject fragments, URLs and message IDs.
fn corpus_needles(corpus: &Corpus, cards: &[(String, String)]) -> Vec<String> {
    let mut needles: Vec<String> = Vec::new();
    for case in &corpus.cases {
        needles.extend(case.canaries());
        needles.push(case.spec.from_address.clone());
        if let Some(reply) = case.spec.reply_to.clone() {
            needles.push(reply);
        }
        if let Some(list_id) = case.spec.list_id.clone() {
            needles.push(list_id);
        }
        needles.push(case.spec.subject.clone());
        needles.push(case.spec.from_display.clone());
        for word in case.spec.subject.split_whitespace() {
            if word.chars().count() >= 8 {
                needles.push(word.to_owned());
            }
        }
        if let Some((_, domain)) = case.spec.from_address.split_once('@') {
            needles.push(domain.to_owned());
        }
    }
    needles.extend(corpus.all_urls());
    needles.extend(cards.iter().map(|(id, _)| id.clone()));
    needles.retain(|n| n.chars().count() >= 6);
    needles.sort();
    needles.dedup();
    needles
}

#[tokio::test(start_paused = true)]
async fn exp_1_eval_record_holds_no_corpus_string() -> TestResult {
    let corpus = load()?;
    assert!(!corpus.cases.is_empty());
    let mut w = World::new(true).await?;
    let (_, _, set) = pair(
        Scripted::Answer(answer(MessageClass::List)),
        Scripted::Answer(answer(MessageClass::Personal)),
    );
    w.open_gate(set);
    let session = w.session(1);
    for case in &corpus.cases {
        w.seed_message(case.seed_message());
    }
    // One page carries every corpus card (24 cases, the page cap is 50).
    let cards = w.cards(&session, 50).await?;
    assert!(!cards.is_empty());
    for (n, (id, token)) in cards.iter().enumerate() {
        let result = swipe(
            &w.app,
            &session,
            key(0xE00 + n as u128),
            w.request(id, ActionDto::Keep, token.clone()),
        )
        .await?;
        assert_eq!(result.outcome, SwipeOutcome::Kept);
    }

    let records = w.records().await?;
    assert_eq!(records.len(), cards.len());
    // Every string a record holds, values only: the schema's own key names
    // (`has_list_unsubscribe`) are not content and cannot leak anything.
    let value = serde_json::to_value(&records)?;
    let mut strings = Vec::new();
    string_values(&value, &mut strings);
    assert!(!strings.is_empty());
    for needle in corpus_needles(&corpus, &cards) {
        for held in &strings {
            assert!(
                !held.contains(&needle),
                "a classifier_eval record holds {needle}"
            );
        }
    }
    // Every record is still fully populated after the scan.
    assert!(records
        .iter()
        .all(|r| r.gemini.is_some() && r.jev.is_some()));
    Ok(())
}

/// The S5 and S4 5.6 allowed key set, recursively.
const ALLOWED_KEYS: &[&str] = &[
    "age_bucket",
    "auto_submitted",
    "class",
    "confidence",
    "created_at",
    "dkim_covers_list_unsubscribe",
    "direction",
    "error_code",
    "eval_id",
    "expires_at",
    "from_authenticated",
    "gemini",
    "has_feedback_id",
    "has_list_id",
    "has_list_unsubscribe",
    "has_list_unsubscribe_post",
    "header_facts",
    "header_rules",
    "input_tokens",
    "input_version",
    "jev",
    "lang_is_english",
    "latency_ms",
    "model_version",
    "outcome",
    "precedence_bulk",
    "price_version",
    "probabilities",
    "provider",
    "question_version",
    "score",
    "text_tokens_bucket",
    "time_to_swipe_ms",
    "undone",
    "user_pseudo_id",
];

fn keys_of(value: &serde_json::Value) -> Vec<String> {
    let mut keys: Vec<String> = value
        .as_object()
        .map(|o| o.keys().cloned().collect())
        .unwrap_or_default();
    keys.sort();
    keys
}

/// Every string inside `value`, keys excluded.
fn string_values(value: &serde_json::Value, out: &mut Vec<String>) {
    match value {
        serde_json::Value::Object(map) => {
            for nested in map.values() {
                string_values(nested, out);
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                string_values(item, out);
            }
        }
        serde_json::Value::String(s) => out.push(s.clone()),
        _ => {}
    }
}

fn deep_keys(value: &serde_json::Value, out: &mut Vec<String>) {
    match value {
        serde_json::Value::Object(map) => {
            for (key, nested) in map {
                out.push(key.clone());
                deep_keys(nested, out);
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                deep_keys(item, out);
            }
        }
        _ => {}
    }
}

#[tokio::test(start_paused = true)]
async fn exp_1_eval_record_fields_match_allowed_schema() -> TestResult {
    let (w, id, token) = world_with_card(
        Scripted::Answer(answer(MessageClass::List)),
        Scripted::Answer(answer(MessageClass::List)),
    )
    .await?;
    let session = w.session(1);
    swipe(
        &w.app,
        &session,
        key(1),
        w.request(&id, ActionDto::Keep, token),
    )
    .await?;
    let record = w.record(key(1)).await?.ok_or("record")?;
    let value = serde_json::to_value(&record)?;

    let mut expected = vec![
        "age_bucket",
        "created_at",
        "eval_id",
        "expires_at",
        "gemini",
        "header_facts",
        "header_rules",
        "input_version",
        "jev",
        "lang_is_english",
        "outcome",
        "price_version",
        "provider",
        "question_version",
        "text_tokens_bucket",
        "user_pseudo_id",
    ];
    expected.sort_unstable();
    assert_eq!(keys_of(&value), expected);
    assert_eq!(
        keys_of(&value["header_rules"]),
        ["class", "score"].map(str::to_owned)
    );
    assert_eq!(
        keys_of(&value["outcome"]),
        ["direction", "time_to_swipe_ms", "undone"].map(str::to_owned)
    );
    assert_eq!(
        keys_of(&value["header_facts"]),
        [
            "auto_submitted",
            "dkim_covers_list_unsubscribe",
            "from_authenticated",
            "has_feedback_id",
            "has_list_id",
            "has_list_unsubscribe",
            "has_list_unsubscribe_post",
            "precedence_bulk",
        ]
        .map(str::to_owned)
    );
    for model in ["gemini", "jev"] {
        assert_eq!(
            keys_of(&value[model]),
            [
                "class",
                "confidence",
                "error_code",
                "input_tokens",
                "latency_ms",
                "model_version",
                "probabilities",
                "score",
            ]
            .map(str::to_owned)
        );
    }
    // Recursively: no key anywhere outside the S5 allowed set, and no `arm`.
    let mut found = Vec::new();
    deep_keys(&value, &mut found);
    for key in &found {
        assert!(
            ALLOWED_KEYS.contains(&key.as_str()),
            "unexpected record key {key}"
        );
    }
    assert!(!found.iter().any(|k| k == "arm"));
    Ok(())
}

#[test]
fn exp_1_eval_id_not_from_message_id() {
    let user = UserId::new(Uuid::from_u128(7));
    let k = Uuid::from_u128(11);
    // Stable, and the same for two swipes: the message plays no part.
    assert_eq!(eval_id_for(&user, &k), eval_id_for(&user, &k));
    assert_ne!(
        eval_id_for(&user, &k),
        eval_id_for(&user, &Uuid::from_u128(12))
    );
    assert_ne!(
        eval_id_for(&user, &k),
        eval_id_for(&UserId::new(Uuid::from_u128(8)), &k)
    );
    // An ID derived from a message ID under the eval namespace is never the
    // swipe's ID, so the message cannot be recovered from it (S7 5.13).
    let from_message = EvalId(Uuid::new_v5(&EVAL_NAMESPACE, b"message-identifier-1"));
    assert_ne!(eval_id_for(&user, &k), from_message);
}

#[tokio::test(start_paused = true)]
async fn exp_1_unswiped_card_leaves_no_record() -> TestResult {
    let (w, _id, token) = world_with_card(
        Scripted::Answer(answer(MessageClass::List)),
        Scripted::Answer(answer(MessageClass::List)),
    )
    .await?;
    // The card was served with both predictions sealed into it and never
    // swiped.
    let session = w.session(1);
    let payload = w.open_payload(&session, &token).await?;
    assert!(payload.bakeoff.is_some());
    assert!(w.records().await?.is_empty());
    Ok(())
}

#[test]
fn exp_1_record_has_180_day_expiry() {
    assert_eq!(EVAL_TTL_DAYS, 180);
    let now = OffsetDateTime::from_unix_timestamp(1_790_000_000).unwrap();
    let payload = ClassificationPayload {
        mailbox_id: Uuid::from_u128(1),
        message_id: "m1".to_owned(),
        header_rules: Classification {
            class: MessageClass::List,
            bulk_score: 90,
            bulk_reason: "rules".to_owned(),
            confidence: None,
            probabilities: None,
        },
        classifier_id: HEADER_RULES_ID.to_owned(),
        issued_at: now,
        bakeoff: Some(BakeoffPayload {
            gemini: Some(prediction_ok(
                &ClassifierId("gemini@test".to_owned()),
                &answer(MessageClass::List),
                12,
                None,
            )),
            jev: None,
            header_facts: eval_header_facts(&list_facts()),
            provider: Provider::Gmail,
            age_bucket: AgeBucket::Under7d,
            text_tokens_bucket: TextTokensBucket::Under100,
            lang_is_english: true,
            input_version: "1".to_owned(),
            question_version: "1".to_owned(),
            price_version: "1".to_owned(),
        }),
    };
    let bakeoff = payload.bakeoff.as_ref().ok_or("bakeoff").unwrap();
    let record = build_eval_record(
        ports::UserPseudoId("a".repeat(32)),
        &payload,
        bakeoff,
        PortsDirection::Right,
        now,
        EvalId(Uuid::from_u128(3)),
    );
    assert_eq!(record.created_at, now);
    assert_eq!(record.expires_at, now + Duration::days(180));
    // The TTL's length is exactly the retention S5 gives the collection.
    assert_eq!((record.expires_at - record.created_at).whole_days(), 180);
}

#[tokio::test(start_paused = true)]
async fn exp_1_no_record_after_opt_out() -> TestResult {
    let (w, id, token) = world_with_card(
        Scripted::Answer(answer(MessageClass::List)),
        Scripted::Answer(answer(MessageClass::List)),
    )
    .await?;
    let session = w.session(1);
    // The stale token still holds both predictions...
    let payload = w.open_payload(&session, &token).await?;
    assert!(payload.bakeoff.is_some());
    // ...but the user opted out between the Feed and the swipe.
    w.opt_out().await?;
    let result = swipe(
        &w.app,
        &session,
        key(1),
        w.request(&id, ActionDto::Keep, token),
    )
    .await?;
    assert_eq!(result.outcome, SwipeOutcome::Kept);
    assert!(w.records().await?.is_empty());
    assert!(w
        .open_undo(&session, &result.undo_token)
        .await?
        .eval_id
        .is_none());
    Ok(())
}

#[test]
fn eval_id_differs_from_job_id() {
    let user = UserId::new(Uuid::from_u128(7));
    let k = Uuid::from_u128(11);
    let eval_id = eval_id_for(&user, &k);
    // The T-605 derivation: the same (user, Idempotency-Key) never collides
    // with the eval ID, or the undo token could name the wrong document.
    let ids = derive_swipe_ids(&user, k);
    assert_ne!(eval_id.0, ids.job_id.0);
    assert_ne!(eval_id.0, ids.rule_id.0);
    assert_eq!(eval_id.0.get_version_num(), 5);
    assert_eq!(eval_id.0, eval_id_for(&user, &k).0);
}
