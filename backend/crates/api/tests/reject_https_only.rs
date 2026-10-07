//! T-704: an https-only (no one-click, no mailto) DKIM-covered
//! `List-Unsubscribe` raises a Needs Attention item instead of a job, and only
//! an `https` link from that header is stored (UN-04 AC6, UN-05 AC1, SW-03 AC2,
//! ASVS V1.2.2).
//!
//! Service integration: everything runs on `FakeMailbox`, the in-memory store,
//! keys, scheduler and app folder, and the virtual clock. The service is called
//! directly, exactly as T-605's tests do.

#![allow(
    clippy::too_many_lines,
    clippy::similar_names,
    clippy::items_after_statements,
    clippy::needless_pass_by_value,
    clippy::missing_panics_doc,
    clippy::cast_possible_truncation,
    clippy::cast_lossless,
    clippy::assert_is_empty
)]

use std::sync::Arc;

use api::routes::feed::ClassificationPayload;
use api::routes::swipes::{ActionDto, SwipeRequest, SwipeResultDto};
use api::sealed::{SealedTokens, TokenType};
use api::services::reject::NS_NA_ITEM;
use api::services::swipe::swipe;
use api::services::user_state_store::UserStateStore;
use api::session::extract::AuthedSession;
use api::state::AppState;
use api::{app_state, config::ApiConfig};
use domain::user_state::UserState;
use domain::{
    Classification, EmailAddress, HeaderFacts, MailboxId, MessageClass, MessageId,
    NeedsAttentionReason, Provider, ProviderSubjectId, RuleKind, SwipeOutcome, UnsubscribeOptions,
    UserId, HEADER_RULES_ID,
};
use obs::Sensitive;
use ports::store::aad_fields;
use ports::{
    Aad, Ciphertext, Clock, JobRecord, KeyService, MailboxRecord, NeedsAttentionId,
    NeedsAttentionRecord, Page, PageRequest, Precondition, Rng, ServerStore, SessionHash,
    SessionRecordId, UserRecord, Versioned, WrappedKey,
};
use testkit::scheduler::SchedulerEvent;
use testkit::{fake_ports, Fakes, MailOp, SeedMessage};
use time::{Duration, OffsetDateTime};
use url::Url;
use uuid::Uuid;

type TestResult = Result<(), Box<dyn std::error::Error>>;

const ORIGIN: &str = "https://mailtinder.test";
const EMAIL_KEY: &[u8] = b"fake-email-key";
const BASE: i64 = 1_790_000_000;
const LIST_ID: &str = "news.example.com";

fn config() -> Result<ApiConfig, Box<dyn std::error::Error>> {
    Ok(ApiConfig::new(
        ORIGIN.to_owned(),
        "fake-client".into(),
        Sensitive::new(b"fake-log-key".to_vec()),
        Sensitive::new(EMAIL_KEY.to_vec()),
    )?)
}

fn at(offset_s: i64) -> OffsetDateTime {
    OffsetDateTime::from_unix_timestamp(BASE + offset_s).unwrap_or(OffsetDateTime::UNIX_EPOCH)
}

fn classification(class: MessageClass) -> Classification {
    Classification {
        class,
        bulk_score: 10,
        bulk_reason: "fixed".to_owned(),
        confidence: None,
        probabilities: None,
    }
}

/// Corpus "https without one-click, DKIM `h=` covers `List-Unsubscribe`": a
/// page link the user must open, classified `list`.
fn page_link_facts(link: &str) -> Result<HeaderFacts, Box<dyn std::error::Error>> {
    Ok(HeaderFacts {
        list_unsubscribe: Some(UnsubscribeOptions {
            one_click_https: None,
            https: Some(Url::parse(link)?),
            mailto: None,
        }),
        list_unsubscribe_present: true,
        list_id: Some(LIST_ID.to_owned()),
        from_authenticated: true,
        ..HeaderFacts::default()
    })
}

/// Corpus "one-click headers not in DKIM `h=`": the header exists but no option
/// survives the DKIM cover check (S6 6, T-406).
fn uncovered_facts() -> HeaderFacts {
    HeaderFacts {
        list_unsubscribe: None,
        list_unsubscribe_present: true,
        list_id: Some(LIST_ID.to_owned()),
        from_authenticated: true,
        ..HeaderFacts::default()
    }
}

// ---------------------------------------------------------------------------
// The world
// ---------------------------------------------------------------------------

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
                status: domain::MailboxStatus::Connected,
                linked_at: fakes.clock.now(),
                is_primary: true,
                refresh_token: None,
            },
            Precondition::MustNotExist,
        )
        .await?;
    Ok(mailbox_id)
}

impl World {
    async fn new() -> Result<World, Box<dyn std::error::Error>> {
        let (parts, fakes) = fake_ports();
        let app = app_state(Arc::new(parts), Arc::new(config()?));
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
        let primary = seed_mailbox(&fakes, user, "sub-a", "a@example.com").await?;
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

    /// Seed an inbox message from `sender@example.com` with `facts` and a
    /// preview that carries a different link, so the test can show the body is
    /// never read.
    fn seed(&self, sender: &str, facts: HeaderFacts, offset_s: i64, preview: &str) -> MessageId {
        let address = format!("{sender}@example.com");
        self.fakes.mailbox.seed(
            &self.primary,
            SeedMessage {
                from_display: format!("Name of {address}"),
                from_address: address,
                subject: format!("Subject at {offset_s}"),
                raw_headers: Vec::new(),
                facts,
                preview_text: preview.to_owned(),
                internal_date: at(offset_s),
                labels: vec!["INBOX".to_owned(), "UNREAD".to_owned()],
            },
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

    fn sealer(&self) -> SealedTokens {
        SealedTokens::new(
            Arc::clone(&self.app.ports.keys),
            Arc::clone(&self.app.ports.clock),
        )
    }

    async fn classification_token(
        &self,
        session: &AuthedSession,
        message_id: &str,
        class: MessageClass,
    ) -> Result<String, Box<dyn std::error::Error>> {
        let payload = ClassificationPayload {
            mailbox_id: self.primary.0,
            message_id: message_id.to_owned(),
            header_rules: classification(class),
            classifier_id: HEADER_RULES_ID.to_owned(),
        };
        Ok(self
            .sealer()
            .seal(
                TokenType::Classification,
                &self.user,
                &self.wrapped().await?,
                &session.session_record_id,
                self.app.ports.clock.now() + Duration::seconds(3600),
                &payload,
            )
            .await?)
    }

    async fn reject_request(
        &self,
        session: &AuthedSession,
        id: &MessageId,
    ) -> Result<SwipeRequest, Box<dyn std::error::Error>> {
        Ok(SwipeRequest {
            mailbox_id: self.primary.0,
            message_id: id.as_str().to_owned(),
            action: ActionDto::Reject,
            category_id: None,
            new_category_name: None,
            classification_token: self
                .classification_token(session, id.as_str(), MessageClass::Notice)
                .await?,
        })
    }

    /// Reject `id` under idempotency key `key`.
    async fn reject(
        &self,
        session: &AuthedSession,
        key: u128,
        id: &MessageId,
    ) -> Result<SwipeResultDto, Box<dyn std::error::Error>> {
        let req = self.reject_request(session, id).await?;
        Ok(swipe(&self.app, session, Uuid::from_u128(key), req).await?)
    }

    async fn state(&self) -> Result<UserState, Box<dyn std::error::Error>> {
        Ok(UserStateStore::new(Arc::new(self.app.clone()))
            .load(&self.user)
            .await?
            .state)
    }

    async fn jobs(&self) -> Result<Vec<Versioned<JobRecord>>, Box<dyn std::error::Error>> {
        Ok(self.fakes.store.jobs().by_mailbox(&self.primary).await?)
    }

    fn scheduled_count(&self) -> usize {
        self.fakes
            .scheduler
            .events()
            .iter()
            .filter(|e| matches!(e, SchedulerEvent::Scheduled(..)))
            .count()
    }

    async fn items(
        &self,
    ) -> Result<Page<Versioned<NeedsAttentionRecord>>, Box<dyn std::error::Error>> {
        Ok(self
            .fakes
            .store
            .needs_attention()
            .by_user(
                &self.user,
                PageRequest {
                    limit: 10,
                    after: None,
                },
            )
            .await?)
    }

    /// The one item keyed on user and `sender@example.com`.
    async fn item(
        &self,
        sender: &str,
    ) -> Result<Option<NeedsAttentionRecord>, Box<dyn std::error::Error>> {
        let address = format!("{sender}@example.com");
        let mut bytes = Vec::new();
        bytes.extend_from_slice(self.user.0.as_bytes());
        bytes.extend_from_slice(address.as_bytes());
        let id = NeedsAttentionId(Uuid::new_v5(&NS_NA_ITEM, &bytes));
        Ok(self
            .fakes
            .store
            .needs_attention()
            .get(&id)
            .await?
            .map(|v| v.record))
    }

    /// Open one sealed field of an item, so the test can read the plain text.
    async fn open(
        &self,
        scope: Uuid,
        field: &'static str,
        sealed: &Ciphertext,
    ) -> Result<String, Box<dyn std::error::Error>> {
        let aad = Aad {
            user: self.user,
            scope: scope.to_string(),
            field,
        };
        let plain = self
            .fakes
            .keys
            .open(&self.user, &self.wrapped().await?, &aad, &sealed.0)
            .await?;
        Ok(String::from_utf8(plain)?)
    }
}

// ---------------------------------------------------------------------------
// UN-04 AC6, SW-03 AC2
// ---------------------------------------------------------------------------

#[tokio::test]
async fn un_04_ac6_https_only_creates_no_job() -> TestResult {
    let w = World::new().await?;
    let session = w.session(1);
    let id = w.seed(
        "shop",
        page_link_facts("https://example.com/unsub?id=1")?,
        10,
        "Preview with no link",
    );

    let result = w.reject(&session, 1, &id).await?;

    assert_eq!(result.outcome, SwipeOutcome::TrashedUnsubscribeManual);
    assert!(w.jobs().await?.is_empty(), "no unsubscribe job");
    assert_eq!(w.scheduled_count(), 0, "no Cloud Task");
    Ok(())
}

#[tokio::test]
async fn sw_03_ac2_https_only_raises_item_not_job() -> TestResult {
    let w = World::new().await?;
    let session = w.session(1);
    let id = w.seed(
        "shop",
        page_link_facts("https://example.com/unsub?id=2")?,
        10,
        "Preview with no link",
    );

    let result = w.reject(&session, 1, &id).await?;

    assert_eq!(result.outcome, SwipeOutcome::TrashedUnsubscribeManual);
    assert!(w.item("shop").await?.is_some(), "one item raised");
    assert_eq!(w.items().await?.items.len(), 1);
    assert!(w.jobs().await?.is_empty(), "no job instead of the item");
    Ok(())
}

#[tokio::test]
async fn un_04_ac6_item_raised_with_open_unsubscribe_page() -> TestResult {
    let w = World::new().await?;
    let session = w.session(1);
    let link = "https://example.com/unsub?id=3";
    let id = w.seed("shop", page_link_facts(link)?, 10, "Preview with no link");

    w.reject(&session, 1, &id).await?;

    let item = w.item("shop").await?.ok_or("item raised")?;
    assert_eq!(item.reason_code, NeedsAttentionReason::HttpsOnlyUnsubscribe);
    assert_eq!(item.expires_at, item.created_at + Duration::days(30));
    let sealed = item.link.as_ref().ok_or("link stored")?;
    assert_eq!(
        w.open(item.item_id.0, aad_fields::NA_LINK, sealed).await?,
        link
    );
    Ok(())
}

#[tokio::test]
async fn un_04_ac6_trashed_and_reject_rule_created() -> TestResult {
    let w = World::new().await?;
    let session = w.session(1);
    let id = w.seed(
        "shop",
        page_link_facts("https://example.com/unsub?id=4")?,
        10,
        "Preview with no link",
    );

    w.reject(&session, 1, &id).await?;

    assert_eq!(w.fakes.mailbox.calls(MailOp::Trash), 1, "trashed once");
    assert_eq!(w.fakes.mailbox.calls(MailOp::ReportSpam), 0);
    let state = w.state().await?;
    let rule = state.rules.first().ok_or("one rule")?;
    assert_eq!(rule.rule.kind, RuleKind::RejectList);
    assert_eq!(rule.rule.matcher.sender.as_str(), "shop@example.com");
    Ok(())
}

#[tokio::test]
async fn un_04_ac6_link_only_from_dkim_header() -> TestResult {
    let w = World::new().await?;
    let session = w.session(1);
    let header = "https://example.com/unsub?from=header";
    let body = "https://tracker.example.net/unsub?from=body";
    let id = w.seed("shop", page_link_facts(header)?, 10, body);

    w.reject(&session, 1, &id).await?;

    let item = w.item("shop").await?.ok_or("item raised")?;
    let sealed = item.link.as_ref().ok_or("link stored")?;
    let stored = w.open(item.item_id.0, aad_fields::NA_LINK, sealed).await?;
    assert_eq!(stored, header, "the header link, not the body link");
    assert_ne!(stored, body);
    assert_eq!(
        w.fakes.mailbox.calls(MailOp::GetPreview),
        0,
        "the body and preview are never read (S2 SW-03 AC5)"
    );
    Ok(())
}

#[tokio::test]
async fn un_04_ac6_no_dkim_cover_no_item() -> TestResult {
    let w = World::new().await?;
    let session = w.session(1);
    let id = w.seed(
        "news",
        uncovered_facts(),
        10,
        "https://tracker.example.net/unsub",
    );

    let result = w.reject(&session, 1, &id).await?;

    assert_eq!(result.outcome, SwipeOutcome::TrashedListNoUnsubscribe);
    assert!(w.items().await?.items.is_empty(), "no item");
    assert!(w.jobs().await?.is_empty(), "no job");
    assert_eq!(w.scheduled_count(), 0);
    Ok(())
}

#[tokio::test]
async fn un_04_ac6_plain_http_link_never_fetched_item_has_no_link() -> TestResult {
    let w = World::new().await?;
    let session = w.session(1);
    let id = w.seed(
        "plain",
        page_link_facts("http://example.com/unsub?id=5")?,
        10,
        "https://tracker.example.net/unsub",
    );

    let result = w.reject(&session, 1, &id).await?;

    assert_eq!(result.outcome, SwipeOutcome::TrashedUnsubscribeManual);
    let item = w.item("plain").await?.ok_or("item raised")?;
    assert!(item.link.is_none(), "an http link is dropped");
    assert!(
        w.fakes.egress.records().is_empty(),
        "nothing is fetched, not even a HEAD"
    );
    assert!(w.jobs().await?.is_empty());
    Ok(())
}

#[tokio::test]
async fn un_05_ac1_https_only_item_has_sender_link_reason() -> TestResult {
    let w = World::new().await?;
    let session = w.session(1);
    let link = "https://example.com/unsub?reason=1";
    let id = w.seed("shop", page_link_facts(link)?, 10, "Preview with no link");

    w.reject(&session, 1, &id).await?;

    let item = w.item("shop").await?.ok_or("item raised")?;
    assert_eq!(item.reason_code, NeedsAttentionReason::HttpsOnlyUnsubscribe);
    assert_eq!(
        w.open(
            item.item_id.0,
            aad_fields::NA_SENDER_DISPLAY,
            &item.sender_display
        )
        .await?,
        "Name of shop@example.com",
        "the From display name, never the address"
    );
    let sealed = item.link.as_ref().ok_or("link stored")?;
    assert_eq!(
        w.open(item.item_id.0, aad_fields::NA_LINK, sealed).await?,
        link
    );
    Ok(())
}
