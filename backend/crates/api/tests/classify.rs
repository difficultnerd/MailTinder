//! T-901 classifier wiring and sealed classification (CL-01 AC1/AC3, CL-03
//! AC1/AC2/AC4, BAKE-2 to BAKE-5).
//!
//! Service integration: `classify_page` runs on `FakeClassifier`s with paused
//! tokio time; the Feed and swipe run on the in-memory fakes.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::too_many_lines,
    clippy::similar_names,
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::assert_is_empty
)]

use std::sync::Arc;
use std::time::Duration;

use api::classify::{
    age_bucket, classify_page, BakeoffGate, CardToClassify, ClassifiedCard, ClassifierSet,
    MODEL_CONCURRENCY,
};
use api::error::ApiError;
use api::routes::feed::{ClassificationPayload, FeedRequest};
use api::routes::swipes::{ActionDto, SwipeRequest};
use api::sealed::{SealedTokens, TokenType};
use api::services::feed::next_page;
use api::services::swipe::swipe;
use api::session::extract::AuthedSession;
use api::{app_state, config::ApiConfig};
use domain::{
    header_guard, Classification, EmailAddress, HeaderFacts, HeaderRules, LabelSet, MailboxId,
    MessageClass, MessageId, MessageMeta, Provider, ProviderSubjectId, SenderKey, SwipeOutcome,
    UserId, HEADER_RULES_ID,
};
use obs::Sensitive;
use ports::store::aad_fields;
use ports::{
    Aad, AgeBucket, Ciphertext, Classifier, ClassifierError, Clock, KeyService, MailboxRecord,
    ModelErrorCode, ModelPrediction, Precondition, Rng, ServerStore, SessionHash, SessionRecordId,
    UserRecord,
};
use proptest::prelude::*;
use testkit::contract::classifier::{classifier_contract, HeaderRulesClassifier};
use testkit::fake_classifier::{FakeClassifier, Scripted};
use testkit::{fake_ports, Fakes, SeedMessage};
use time::OffsetDateTime;
use url::Url;
use uuid::Uuid;

type TestResult = Result<(), Box<dyn std::error::Error>>;

const BASE: i64 = 1_790_000_000;

fn at(offset_s: i64) -> OffsetDateTime {
    OffsetDateTime::from_unix_timestamp(BASE + offset_s).unwrap()
}

fn answer(class: MessageClass) -> Classification {
    Classification {
        class,
        bulk_score: 50,
        bulk_reason: "model says".to_owned(),
        confidence: Some(0.9),
        probabilities: Some([0.2; 5]),
    }
}

fn list_facts() -> HeaderFacts {
    HeaderFacts {
        list_unsubscribe: Some(domain::UnsubscribeOptions {
            one_click_https: Some(Url::parse("https://lists.example.com/u?x=1").unwrap()),
            https: None,
            mailto: None,
        }),
        list_unsubscribe_present: true,
        list_id: Some("news.example.com".to_owned()),
        from_authenticated: true,
        ..HeaderFacts::default()
    }
}

fn meta(n: u32, facts: HeaderFacts) -> MessageMeta {
    MessageMeta {
        mailbox: MailboxId::new(Uuid::from_u128(7)),
        id: MessageId::new(format!("m{n:04}")).unwrap(),
        internal_date: at(0),
        from_display: format!("Sender {n}"),
        from_address: format!("s{n}@example.com"),
        sender: SenderKey::from_address(&format!("s{n}@example.com")),
        subject: format!("Subject {n}"),
        labels: LabelSet::from_ids(vec!["INBOX".to_owned()]),
        facts,
    }
}

fn set_of(g: &Arc<FakeClassifier>, j: &Arc<FakeClassifier>) -> ClassifierSet {
    let gemini: Arc<dyn Classifier> = g.clone();
    let jev: Arc<dyn Classifier> = j.clone();
    ClassifierSet {
        gemini: Some(gemini),
        jev: Some(jev),
    }
}

const OPEN: BakeoffGate = BakeoffGate {
    gemini: true,
    jev: true,
};

async fn run(set: &ClassifierSet, gate: BakeoffGate, metas: &[MessageMeta]) -> Vec<ClassifiedCard> {
    let cards: Vec<CardToClassify<'_>> = metas
        .iter()
        .map(|m| CardToClassify {
            meta: m,
            stripped_text: "",
            provider: Provider::Gmail,
        })
        .collect();
    classify_page(set, gate, at(100), &cards).await
}

fn expected_badge(m: &MessageMeta) -> Classification {
    let rules = HeaderRules::classify(&m.facts, &m.sender);
    header_guard(&m.facts, &rules, Some(&rules)).classification
}

fn pair(
    script_g: Scripted,
    script_j: Scripted,
) -> (Arc<FakeClassifier>, Arc<FakeClassifier>, ClassifierSet) {
    let g = Arc::new(FakeClassifier::new("gemini@test"));
    let j = Arc::new(FakeClassifier::new("jev@test"));
    g.script_all(script_g);
    j.script_all(script_j);
    let set = set_of(&g, &j);
    (g, j, set)
}

fn code_of(p: Option<&ModelPrediction>) -> Option<ModelErrorCode> {
    p.and_then(|p| p.error_code)
}

// ---------------------------------------------------------------------------
// Pipeline
// ---------------------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn cl_01_ac1_classification_goes_through_pipeline() -> TestResult {
    // A fake registered in the set is called by the Feed with no Feed change.
    let mut w = World::new().await?;
    let (g, j, set) = pair(
        Scripted::Answer(answer(MessageClass::Personal)),
        Scripted::Answer(answer(MessageClass::Personal)),
    );
    w.app.classifiers = set;
    w.app.bakeoff_gate = OPEN;
    w.seed("news", list_facts(), 10);
    let page = next_page(&w.app, &w.session(1), feed_request()).await?;
    assert_eq!(page.cards.len(), 1);
    assert_eq!(g.call_count(), 1);
    assert_eq!(j.call_count(), 1);
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn cl_01_ac3_badge_from_header_rules_only() -> TestResult {
    let mut w = World::new().await?;
    let (_g, _j, set) = pair(
        Scripted::Answer(answer(MessageClass::Personal)),
        Scripted::Answer(answer(MessageClass::Personal)),
    );
    w.app.classifiers = set;
    w.app.bakeoff_gate = OPEN;
    w.seed("news", list_facts(), 10);
    let page = next_page(&w.app, &w.session(1), feed_request()).await?;
    let card = &page.cards[0];
    let expected = expected_badge(&meta(1, list_facts()));
    assert_eq!(card.class, expected.class.as_str());
    assert_eq!(card.bulk_score, expected.bulk_score);
    assert_eq!(card.bulk_reason, expected.bulk_reason);
    let json = serde_json::to_string(card)?;
    assert!(!json.contains("gemini") && !json.contains("jev") && !json.contains("model says"));
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn cl_03_ac1_both_models_called_in_parallel() -> TestResult {
    let (g, j, set) = pair(
        Scripted::Delay(
            Duration::from_secs(1),
            Box::new(Scripted::Answer(answer(MessageClass::List))),
        ),
        Scripted::Delay(
            Duration::from_secs(1),
            Box::new(Scripted::Answer(answer(MessageClass::List))),
        ),
    );
    let metas = vec![meta(1, list_facts())];
    let start = tokio::time::Instant::now();
    let out = run(&set, OPEN, &metas).await;
    assert_eq!(start.elapsed(), Duration::from_secs(1));
    let bakeoff = out[0].payload.bakeoff.as_ref().ok_or("bakeoff")?;
    assert!(bakeoff
        .gemini
        .as_ref()
        .ok_or("gemini")?
        .error_code
        .is_none());
    assert!(bakeoff.jev.as_ref().ok_or("jev")?.error_code.is_none());
    assert_eq!((g.call_count(), j.call_count()), (1, 1));
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn cl_03_ac2_timeout_recorded_card_unaffected() -> TestResult {
    let slow = || {
        Scripted::Delay(
            Duration::from_secs(10),
            Box::new(Scripted::Answer(answer(MessageClass::Personal))),
        )
    };
    let (_g, _j, set) = pair(slow(), slow());
    let metas = vec![meta(1, list_facts())];
    let start = tokio::time::Instant::now();
    let out = run(&set, OPEN, &metas).await;
    assert_eq!(start.elapsed(), Duration::from_secs(2));
    let bakeoff = out[0].payload.bakeoff.as_ref().ok_or("bakeoff")?;
    assert_eq!(
        code_of(bakeoff.gemini.as_ref()),
        Some(ModelErrorCode::Timeout)
    );
    assert_eq!(code_of(bakeoff.jev.as_ref()), Some(ModelErrorCode::Timeout));
    assert_eq!(out[0].badge, expected_badge(&metas[0]));
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn bake_2_both_models_called_per_card() -> TestResult {
    let (g, j, set) = pair(
        Scripted::Answer(answer(MessageClass::List)),
        Scripted::Answer(answer(MessageClass::List)),
    );
    let metas: Vec<MessageMeta> = (0..20).map(|n| meta(n, list_facts())).collect();
    run(&set, OPEN, &metas).await;
    let key =
        |i: &ports::ClassifierInput| (i.from_domain.clone(), i.list_id.clone(), i.subject.clone());
    assert_eq!(g.calls().len(), 20);
    assert_eq!(j.calls().len(), 20);
    // Same inputs, element by element (completion order is not significant).
    let mut gc = g.calls();
    let mut jc = j.calls();
    gc.sort_by_key(|i| format!("{}{}", i.from_domain, i.auth_summary));
    jc.sort_by_key(|i| format!("{}{}", i.from_domain, i.auth_summary));
    for (a, b) in gc.iter().zip(jc.iter()) {
        assert_eq!(a, b);
        assert_eq!(key(a), key(b));
        // No subject or text before T-903.
        assert!(a.subject.is_empty() && a.text.is_empty());
    }
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn bake_2_concurrency_never_exceeds_cap() -> TestResult {
    let slow = || {
        Scripted::Delay(
            Duration::from_millis(500),
            Box::new(Scripted::Answer(answer(MessageClass::List))),
        )
    };
    let (g, j, set) = pair(slow(), slow());
    let metas: Vec<MessageMeta> = (0..50).map(|n| meta(n, list_facts())).collect();
    run(&set, OPEN, &metas).await;
    assert_eq!(g.call_count(), 50);
    assert!(g.max_in_flight() <= MODEL_CONCURRENCY);
    assert!(j.max_in_flight() <= MODEL_CONCURRENCY);
    assert!(g.max_in_flight() > 1);
    Ok(())
}

fn model_outputs() -> impl Strategy<Value = Scripted> {
    prop_oneof![
        (0usize..5, 0u8..=255, proptest::option::of(-1.0f32..2.0)).prop_map(|(c, s, conf)| {
            Scripted::Answer(Classification {
                class: MessageClass::ALL[c],
                bulk_score: s,
                bulk_reason: "x".to_owned(),
                confidence: conf,
                probabilities: None,
            })
        }),
        Just(Scripted::Error(ClassifierError::Timeout)),
        Just(Scripted::Error(ClassifierError::Http(500))),
        Just(Scripted::Error(ClassifierError::InvalidOutput)),
    ]
}

proptest! {
    #[test]
    fn bake_3_badge_stays_on_header_rules(g in model_outputs(), j in model_outputs(), list in any::<bool>()) {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .start_paused(true)
            .build()
            .map_err(|e| TestCaseError::fail(e.to_string()))?;
        let facts = if list { list_facts() } else { HeaderFacts::default() };
        let metas = vec![meta(1, facts)];
        let (_g, _j, set) = pair(g, j);
        let out = rt.block_on(run(&set, OPEN, &metas));
        let rules = HeaderRules::classify(&metas[0].facts, &metas[0].sender);
        prop_assert_eq!(&out[0].badge, &expected_badge(&metas[0]));
        prop_assert_eq!(&out[0].payload.header_rules, &rules);
        prop_assert_eq!(out[0].payload.classifier_id.as_str(), HEADER_RULES_ID);
    }
}

#[tokio::test(start_paused = true)]
async fn bake_4_failure_isolated() -> TestResult {
    let bad_score = Classification {
        bulk_score: 101,
        ..answer(MessageClass::List)
    };
    let nan_conf = Classification {
        confidence: Some(f32::NAN),
        ..answer(MessageClass::List)
    };
    let table: Vec<(Scripted, ModelErrorCode)> = vec![
        (
            Scripted::Error(ClassifierError::Timeout),
            ModelErrorCode::Timeout,
        ),
        (
            Scripted::Error(ClassifierError::Http(429)),
            ModelErrorCode::Http,
        ),
        (
            Scripted::Error(ClassifierError::Http(500)),
            ModelErrorCode::Http,
        ),
        (
            Scripted::Error(ClassifierError::Http(503)),
            ModelErrorCode::Http,
        ),
        (
            Scripted::Error(ClassifierError::Unavailable),
            ModelErrorCode::Unavailable,
        ),
        (
            Scripted::Error(ClassifierError::InvalidOutput),
            ModelErrorCode::InvalidOutput,
        ),
        (Scripted::Answer(bad_score), ModelErrorCode::InvalidOutput),
        (Scripted::Answer(nan_conf), ModelErrorCode::InvalidOutput),
    ];
    let good = || Scripted::Answer(answer(MessageClass::List));
    let metas = vec![meta(1, list_facts())];
    for (script, code) in table {
        for (gem_bad, jev_bad) in [(true, false), (false, true), (true, true)] {
            let pick = |bad: bool| if bad { script.clone() } else { good() };
            let (_g, _j, set) = pair(pick(gem_bad), pick(jev_bad));
            let out = run(&set, OPEN, &metas).await;
            assert_eq!(out[0].badge, expected_badge(&metas[0]));
            let b = out[0].payload.bakeoff.as_ref().ok_or("bakeoff")?;
            assert_eq!(code_of(b.gemini.as_ref()), gem_bad.then_some(code));
            assert_eq!(code_of(b.jev.as_ref()), jev_bad.then_some(code));
            assert!(b.gemini.is_some() && b.jev.is_some());
        }
    }
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn bake_5_closed_gate_calls_no_model() -> TestResult {
    let (g, j, set) = pair(
        Scripted::Answer(answer(MessageClass::List)),
        Scripted::Answer(answer(MessageClass::List)),
    );
    let metas = vec![meta(1, list_facts())];
    let out = run(&set, BakeoffGate::default(), &metas).await;
    assert!(out[0].payload.bakeoff.is_none());
    let one = BakeoffGate {
        gemini: true,
        jev: false,
    };
    let out = run(&set, one, &metas).await;
    let b = out[0].payload.bakeoff.as_ref().ok_or("bakeoff")?;
    assert!(b.gemini.is_some() && b.jev.is_none());
    assert_eq!((g.call_count(), j.call_count()), (1, 0));
    Ok(())
}

// ---------------------------------------------------------------------------
// Sealed token and swipe
// ---------------------------------------------------------------------------

struct World {
    app: api::state::AppState,
    fakes: Fakes,
    user: UserId,
    primary: MailboxId,
}

fn feed_request() -> FeedRequest {
    serde_json::from_str(r#"{"cursor":null,"limit":10}"#).unwrap()
}

impl World {
    async fn new() -> Result<World, Box<dyn std::error::Error>> {
        let (ports, fakes) = fake_ports();
        let config = ApiConfig::new(
            "https://mailtinder.test".to_owned(),
            "fake-client".into(),
            Sensitive::new(b"fake-log-key".to_vec()),
            Sensitive::new(b"fake-email-key".to_vec()),
        )?;
        let app = app_state(Arc::new(ports), Arc::new(config));
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
                    experiments_consent_version: None,
                    experiments_opted_in_at: None,
                },
                Precondition::MustNotExist,
            )
            .await?;
        let subject = ProviderSubjectId::new("sub-a")?;
        let address = EmailAddress::parse("a@example.com")?;
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
        let token = "refresh-sub-a".to_owned();
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

    fn seed(&self, sender: &str, facts: HeaderFacts, offset_s: i64) -> MessageId {
        self.fakes.mailbox.seed(
            &self.primary,
            SeedMessage {
                from_display: format!("Name of {sender}"),
                from_address: format!("{sender}@example.com"),
                subject: format!("Subject at {offset_s}"),
                raw_headers: vec![],
                facts,
                preview_text: format!("Preview at {offset_s}"),
                internal_date: self.fakes.clock.now() - time::Duration::seconds(offset_s),
                labels: vec!["INBOX".to_owned(), "UNREAD".to_owned()],
            },
        )
    }

    fn sealer(&self) -> SealedTokens {
        SealedTokens::new(
            Arc::clone(&self.app.ports.keys),
            Arc::clone(&self.app.ports.clock),
        )
    }

    async fn open(
        &self,
        session: &AuthedSession,
        token: &str,
    ) -> Result<ClassificationPayload, Box<dyn std::error::Error>> {
        let wrapped = self.wrapped().await?;
        Ok(self
            .sealer()
            .open::<ClassificationPayload>(
                TokenType::Classification,
                &self.user,
                &wrapped,
                &session.session_record_id,
                token,
            )
            .await?)
    }

    async fn wrapped(&self) -> Result<ports::WrappedKey, Box<dyn std::error::Error>> {
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

    /// A token with no bake-off, claiming `class`, valid for `ttl_s`.
    async fn seal(
        &self,
        session: &AuthedSession,
        id: &str,
        class: MessageClass,
        ttl_s: i64,
    ) -> Result<String, Box<dyn std::error::Error>> {
        let payload = ClassificationPayload {
            mailbox_id: self.primary.0,
            message_id: id.to_owned(),
            header_rules: Classification {
                class,
                bulk_score: 90,
                bulk_reason: "token".to_owned(),
                confidence: None,
                probabilities: None,
            },
            classifier_id: HEADER_RULES_ID.to_owned(),
            issued_at: self.fakes.clock.now(),
            bakeoff: None,
        };
        Ok(self
            .sealer()
            .seal(
                TokenType::Classification,
                &self.user,
                &self.wrapped().await?,
                &session.session_record_id,
                self.fakes.clock.now() + time::Duration::seconds(ttl_s),
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
}

/// A World with both fakes open and one list message; returns its card token.
async fn world_with_card() -> Result<
    (
        World,
        Arc<FakeClassifier>,
        Arc<FakeClassifier>,
        String,
        String,
    ),
    Box<dyn std::error::Error>,
> {
    let mut w = World::new().await?;
    let (g, j, set) = pair(
        Scripted::Answer(answer(MessageClass::Personal)),
        Scripted::Error(ClassifierError::Http(503)),
    );
    w.app.classifiers = set;
    w.app.bakeoff_gate = OPEN;
    w.seed("news", list_facts(), 10);
    let page = next_page(&w.app, &w.session(1), feed_request()).await?;
    let card = page.cards.first().ok_or("card")?;
    Ok((
        w,
        g,
        j,
        card.classification_token.clone(),
        card.message_id.clone(),
    ))
}

#[tokio::test(start_paused = true)]
async fn bake_5_sealed_token_carries_both() -> TestResult {
    let (w, _g, _j, token, _id) = world_with_card().await?;
    let payload = w.open(&w.session(1), &token).await?;
    let b = payload.bakeoff.ok_or("bakeoff")?;
    let gem = b.gemini.ok_or("gemini")?;
    assert_eq!(gem.class, Some(MessageClass::Personal));
    assert_eq!(gem.model_version, "gemini@test");
    assert_eq!(code_of(b.jev.as_ref()), Some(ModelErrorCode::Http));
    assert_eq!(b.price_version, "1");
    assert_eq!(payload.classifier_id, HEADER_RULES_ID);
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn bake_5_no_model_call_at_swipe() -> TestResult {
    let (w, g, j, token, id) = world_with_card().await?;
    let (gc, jc) = (g.call_count(), j.call_count());
    let result = swipe(
        &w.app,
        &w.session(1),
        Uuid::from_u128(1),
        w.request(&id, ActionDto::Keep, token),
    )
    .await?;
    assert_eq!(result.outcome, SwipeOutcome::Kept);
    assert_eq!((g.call_count(), j.call_count()), (gc, jc));
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn bake_5_expired_token_refused() -> TestResult {
    let w = World::new().await?;
    let session = w.session(1);
    let id = w.seed("news", list_facts(), 10);
    let token = w
        .seal(&session, id.as_str(), MessageClass::List, 60)
        .await?;
    // The token expires before the swipe is sent.
    w.fakes.clock.advance(time::Duration::seconds(120));
    let err = swipe(
        &w.app,
        &session,
        Uuid::from_u128(1),
        w.request(id.as_str(), ActionDto::Keep, token),
    )
    .await
    .err()
    .ok_or("must be refused")?;
    assert!(matches!(err, ApiError::InvalidRequest { .. }));
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn cl_03_ac4_swipe_rejects_tampered_or_other_user_token() -> TestResult {
    let (w, _g, _j, token, id) = world_with_card().await?;
    // Tampered: flip the last character.
    let mut bytes = token.clone().into_bytes();
    let last = bytes.len() - 1;
    bytes[last] = if bytes[last] == b'A' { b'B' } else { b'A' };
    let tampered = String::from_utf8(bytes)?;
    let err = swipe(
        &w.app,
        &w.session(1),
        Uuid::from_u128(1),
        w.request(&id, ActionDto::Keep, tampered),
    )
    .await
    .err()
    .ok_or("tampered must be refused")?;
    assert!(matches!(err, ApiError::InvalidRequest { .. }));
    // Sealed to another session: cannot be opened here.
    let err = swipe(
        &w.app,
        &w.session(2),
        Uuid::from_u128(2),
        w.request(&id, ActionDto::Keep, token),
    )
    .await
    .err()
    .ok_or("other session must be refused")?;
    assert!(matches!(err, ApiError::InvalidRequest { .. }));
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn cl_03_ac4_swipe_reapplies_guard() -> TestResult {
    let w = World::new().await?;
    let session = w.session(1);
    // Fresh headers: bulk look-alike with no unsubscribe header at all.
    let facts = HeaderFacts {
        precedence_bulk: true,
        ..HeaderFacts::default()
    };
    let id = w.seed("alerts", facts, 10);
    // The token claims `list`.
    let token = w
        .seal(&session, id.as_str(), MessageClass::List, 3600)
        .await?;
    let result = swipe(
        &w.app,
        &session,
        Uuid::from_u128(1),
        w.request(id.as_str(), ActionDto::Reject, token),
    )
    .await?;
    assert_eq!(result.outcome, SwipeOutcome::TrashedListNoUnsubscribe);
    assert!(w
        .fakes
        .store
        .jobs()
        .by_mailbox(&w.primary)
        .await?
        .is_empty());
    Ok(())
}

// ---------------------------------------------------------------------------
// Contract and unit
// ---------------------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn classifier_contract_header_rules_and_fake() -> TestResult {
    classifier_contract(|| Arc::new(HeaderRulesClassifier)).await?;
    classifier_contract(|| {
        let f = FakeClassifier::new("fake@1");
        f.script_all(Scripted::Answer(answer(MessageClass::List)));
        Arc::new(f)
    })
    .await?;
    Ok(())
}

#[test]
fn age_bucket_boundaries() {
    let now = at(0);
    let ago = |d: time::Duration| age_bucket(now - d, now);
    let day = time::Duration::days;
    assert_eq!(ago(day(6) + time::Duration::hours(23)), AgeBucket::Under7d);
    assert_eq!(ago(day(7)), AgeBucket::D7To90d);
    assert_eq!(ago(day(90)), AgeBucket::D90To1y);
    assert_eq!(ago(day(365)), AgeBucket::Y1To5y);
    assert_eq!(ago(day(5 * 365)), AgeBucket::Over5y);
}
