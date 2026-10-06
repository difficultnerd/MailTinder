//! T-602c Feed endpoint tests (FD-01 to FD-04, SW-02 AC2, GM-08, XC-01,
//! ASVS V9.2.2, S9 empty Feed).
//!
//! Everything runs on `FakeMailbox`, the in-memory store, keys and app folder,
//! and the virtual clock. Service tests call `next_page` directly; the log and
//! token tests go through the real router.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::too_many_lines,
    clippy::similar_names,
    clippy::items_after_statements,
    clippy::needless_pass_by_value,
    clippy::missing_panics_doc,
    clippy::cast_possible_truncation,
    clippy::assert_is_empty
)]

use std::collections::BTreeSet;
use std::sync::Arc;

use api::error::ApiError;
use api::routes::feed::{CardDto, FeedPage, FeedRequest, Phase};
use api::sealed::{SealedTokens, TokenType};
use api::services::feed::{apply_rules, next_page};
use api::services::user_state_store::UserStateStore;
use api::session::extract::AuthedSession;
use api::session::store::SessionService;
use api::state::AppState;
use api::text::plain_text;
use api::{app_state, build_router, config::ApiConfig};
use axum::body::Body;
use axum::http::{header, HeaderValue, Method, Request, StatusCode};
use axum::response::Response;
use axum::Router;
use domain::feed::choose_skip_return;
use domain::user_state::{SkipReturn, UserState};
use domain::{
    EmailAddress, MailboxId, MailboxStatus, MessageId, Provider, ProviderSubjectId, SenderStats,
    Tunables, UserId,
};
use obs::Sensitive;
use ports::store::aad_fields;
use ports::{
    Aad, Ciphertext, Clock, KeyService, MailError, MailProvider, MailboxRecord, Precondition, Rng,
    ServerStore, SessionHash, SessionRecordId, UserRecord,
};
use testkit::{fake_ports, Fakes, MailOp, SeedMessage};
use time::{Duration, OffsetDateTime};
use tower::ServiceExt;
use uuid::Uuid;

type TestResult = Result<(), Box<dyn std::error::Error>>;

const ORIGIN: &str = "https://mailtinder.test";
const EMAIL_KEY: &[u8] = b"fake-email-key";
const BASE: i64 = 1_790_000_000;

/// Tracing caches each callsite's interest process-wide and a scoped capture
/// is seen by one thread only, so the tests that read captured logs hold this.
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn config() -> Result<ApiConfig, Box<dyn std::error::Error>> {
    Ok(ApiConfig::new(
        ORIGIN.to_owned(),
        "fake-client".into(),
        Sensitive::new(b"fake-log-key".to_vec()),
        Sensitive::new(EMAIL_KEY.to_vec()),
    )?)
}

fn at(offset_s: i64) -> OffsetDateTime {
    OffsetDateTime::from_unix_timestamp(BASE + offset_s).unwrap()
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

    /// A linked mailbox with a working grant.
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
            .script_refresh(&token, Ok(format!("access-for-{token}")));
        Ok(id)
    }

    fn session(&self, n: u128, is_admin: bool) -> AuthedSession {
        AuthedSession {
            user: self.user,
            session_record_id: SessionRecordId(Uuid::from_u128(n)),
            is_admin,
            recent_auth_at: None,
            session_hash: SessionHash([n as u8; 32]),
        }
    }

    fn seed(&self, mailbox: &MailboxId, sender: &str, offset_s: i64) -> MessageId {
        self.fakes.mailbox.seed(
            mailbox,
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

    async fn page(
        &self,
        session: &AuthedSession,
        cursor: Option<String>,
        limit: u32,
        refresh: bool,
    ) -> Result<FeedPage, ApiError> {
        next_page(
            &self.app,
            session,
            FeedRequest {
                cursor,
                limit,
                refresh,
            },
        )
        .await
    }

    fn state_store(&self) -> UserStateStore {
        UserStateStore::new(Arc::new(self.app.clone()))
    }

    async fn state(&self) -> Result<UserState, Box<dyn std::error::Error>> {
        Ok(self.state_store().load(&self.user).await?.state)
    }

    fn store_text(&self) -> String {
        self.fakes
            .store
            .export_json()
            .into_iter()
            .map(|(_, v)| v.to_string())
            .collect::<Vec<_>>()
            .join("\n")
    }
}

fn received(page: &FeedPage) -> Vec<i64> {
    page.cards
        .iter()
        .map(|c| c.received_at.unix_timestamp() - BASE)
        .collect()
}

fn ids(page: &FeedPage) -> Vec<String> {
    page.cards.iter().map(|c| c.message_id.clone()).collect()
}

fn skip_key(mailbox: &MailboxId, id: &MessageId) -> String {
    format!("{}/{}", mailbox.0, id.as_str())
}

// ---------------------------------------------------------------------------
// FD-01
// ---------------------------------------------------------------------------

#[tokio::test]
async fn fd_01_ac1_card_has_required_fields() -> TestResult {
    let w = World::new().await?;
    let id = w.seed(&w.primary, "alice", 10);
    let page = w.page(&w.session(1, false), None, 20, false).await?;
    assert_eq!(page.cards.len(), 1);
    let card: &CardDto = &page.cards[0];
    assert_eq!(card.mailbox_id, w.primary.0);
    assert_eq!(card.message_id, id.as_str());
    assert_eq!(card.sender_name, "Name of alice");
    assert_eq!(card.sender_address, "alice@example.com");
    assert_eq!(card.subject, "Subject at 10");
    assert_eq!(card.preview, "Preview at 10");
    assert_eq!(card.received_at, at(10));
    assert!(card.bulk_score <= 100);
    assert!(!card.bulk_reason.is_empty());
    assert!(["list", "bulk_no_header", "notice", "personal", "suspect"].contains(&card.class));
    assert!(card.provider_web_url.starts_with("https://"));
    assert!(card.classification_token.starts_with("mt1.classification."));
    assert!(card.suggestion.is_none() && card.keep_prompt.is_none() && card.boss.is_none());
    assert_eq!(card.skip_count, 0);
    // classifier_id is absent, not null, for a non-admin; present for an admin.
    let json = serde_json::to_value(&page.cards[0])?;
    assert!(json.get("classifier_id").is_none());
    for key in ["suggestion", "keep_prompt", "boss"] {
        assert!(
            json.get(key).is_some_and(serde_json::Value::is_null),
            "{key}"
        );
    }
    let admin = w.page(&w.session(1, true), None, 20, false).await?;
    let json = serde_json::to_value(&admin.cards[0])?;
    assert_eq!(json["classifier_id"], "header_rules@1");
    Ok(())
}

#[tokio::test]
async fn fd_01_ac2_preview_plain_and_capped() -> TestResult {
    let w = World::new().await?;
    let hostile = format!(
        "Hello\u{202E}\u{200B}\u{0007}\n\tworld {}",
        "A".repeat(10_000)
    );
    w.fakes.mailbox.seed(
        &w.primary,
        SeedMessage {
            from_display: "Hostile\u{202E}".to_owned(),
            from_address: "hostile@example.com".to_owned(),
            subject: "Sub\u{0000}ject\u{FEFF}".to_owned(),
            raw_headers: vec![],
            facts: domain::HeaderFacts::default(),
            preview_text: hostile,
            internal_date: at(5),
            labels: vec!["INBOX".to_owned()],
        },
    );
    let page = w.page(&w.session(1, false), None, 20, false).await?;
    let card = &page.cards[0];
    assert_eq!(card.preview.chars().count(), 300);
    assert!(card.preview.starts_with("Hello world AAAA"));
    assert!(!card
        .preview
        .chars()
        .any(|c| c.is_control() || matches!(c, '\u{202E}' | '\u{200B}')));
    assert_eq!(card.subject, "Subject");
    assert_eq!(card.sender_name, "Hostile");
    Ok(())
}

#[test]
fn fd_01_ac2_bidi_and_control_characters_stripped() {
    let hostile = "a\u{202E}b\u{200B}c\u{FEFF}d\u{061C}e\u{2066}f\u{2069}g\u{0007}h\u{0085}i\tj\nk";
    assert_eq!(plain_text(hostile, 100), "abcdefghi j k");
    assert_eq!(plain_text("日本語のメール", 3), "日本語");
}

#[tokio::test]
async fn fd_01_ac3_no_card_content_in_store_or_logs() -> TestResult {
    let _serial = SERIAL.lock().await;
    let w = World::new().await?;
    let corpus = testkit::corpus::load()?;
    for case in &corpus.cases {
        w.fakes.mailbox.seed(&w.primary, case.seed_message());
    }
    let clock = obs::arc(obs::FixedClock(w.fakes.clock.now()));
    let (capture, _guard) = obs::capture("api", clock);
    let page = w.page(&w.session(1, false), None, 50, true).await?;
    assert!(!page.cards.is_empty());
    let mut needles = corpus.all_canaries();
    needles.extend(corpus.all_addresses());
    needles.extend(corpus.all_urls());
    // The server store never holds card content (S5, INV-1).
    let stored = w.store_text();
    for needle in &needles {
        assert!(
            !stored.contains(needle.as_str()),
            "store holds card content"
        );
    }
    // The state file is sealed bytes.
    let ctx = w
        .app
        .tokens
        .mailbox_ctx(&w.app, &w.user, &w.primary)
        .await?;
    let (bytes, _) = ports::AppFolderStore::read(w.app.ports.app_folder.as_ref(), &ctx)
        .await?
        .ok_or("state file")?;
    let text = String::from_utf8_lossy(&bytes);
    for needle in &needles {
        assert!(!text.contains(needle.as_str()), "state file holds content");
    }
    let leaks = obs::scan_for_leaks(&capture.text(), &needles);
    assert!(leaks.is_empty(), "log leak at {leaks:?}");
    Ok(())
}

// ---------------------------------------------------------------------------
// FD-02
// ---------------------------------------------------------------------------

#[tokio::test]
async fn fd_02_ac1_interleaved_newest_first() -> TestResult {
    let w = World::new().await?;
    let b = w.add_mailbox("sub-b", "b@example.com", false).await?;
    for t in [10, 30, 50] {
        w.seed(&w.primary, "a", t);
    }
    for t in [20, 40, 60] {
        w.seed(&b, "b", t);
    }
    let page = w.page(&w.session(1, false), None, 6, false).await?;
    assert_eq!(received(&page), vec![60, 50, 40, 30, 20, 10]);
    let from_b: Vec<bool> = page.cards.iter().map(|c| c.mailbox_id == b.0).collect();
    assert_eq!(from_b, vec![true, false, true, false, true, false]);
    Ok(())
}

#[tokio::test]
async fn fd_02_ac2_card_names_mailbox() -> TestResult {
    let w = World::new().await?;
    let b = w.add_mailbox("sub-b", "b@example.com", false).await?;
    w.seed(&w.primary, "a", 10);
    w.seed(&b, "b", 20);
    let page = w.page(&w.session(1, false), None, 20, false).await?;
    for card in &page.cards {
        let expected = if card.sender_address == "a@example.com" {
            w.primary.0
        } else {
            b.0
        };
        assert_eq!(card.mailbox_id, expected);
    }
    // The badge address differs per mailbox in the web link.
    let urls: BTreeSet<&str> = page
        .cards
        .iter()
        .map(|c| c.provider_web_url.as_str())
        .collect();
    assert_eq!(urls.len(), 2);
    Ok(())
}

#[tokio::test]
async fn fd_02_ac3_failing_mailbox_listed_others_load() -> TestResult {
    let w = World::new().await?;
    let b = w.add_mailbox("sub-b", "b@example.com", false).await?;
    w.seed(&w.primary, "a", 10);
    w.seed(&b, "b", 20);
    // B's grant is revoked at the provider: its cards go missing, A still loads.
    w.fakes.mailbox.revoke_token("access-for-refresh-sub-b");
    let page = w.page(&w.session(1, false), None, 20, false).await?;
    assert_eq!(received(&page), vec![10]);
    assert_eq!(page.mailbox_errors.len(), 1);
    assert_eq!(page.mailbox_errors[0].mailbox_id, b.0);
    assert_eq!(page.mailbox_errors[0].code, "mailbox_needs_sign_in");
    let record = w
        .fakes
        .store
        .mailboxes()
        .get(&b)
        .await?
        .ok_or("mailbox")?
        .record;
    assert_eq!(record.status, MailboxStatus::NeedsSignIn);
    // The next page lists it from its stored status without calling out.
    let again = w.page(&w.session(1, false), None, 20, false).await?;
    assert_eq!(again.mailbox_errors.len(), 1);
    Ok(())
}

#[tokio::test]
async fn fd_02_ac3_all_failing_is_200_with_no_cards() -> TestResult {
    let w = World::new().await?;
    let b = w.add_mailbox("sub-b", "b@example.com", false).await?;
    w.seed(&w.primary, "a", 10);
    w.seed(&b, "b", 20);
    w.fakes
        .mailbox
        .fail_next(MailOp::ListMessages, MailError::Transient);
    w.fakes.mailbox.fail_next(
        MailOp::ListMessages,
        MailError::RateLimited { retry_after_s: 3 },
    );
    let page = w.page(&w.session(1, false), None, 20, false).await?;
    assert!(page.cards.is_empty());
    assert_eq!(page.mailbox_errors.len(), 2);
    assert!(page
        .mailbox_errors
        .iter()
        .all(|e| e.code == "provider_unavailable"));
    // The failed round did not use up the mail: both mailboxes still page.
    assert!(page.next_cursor.is_some());
    let ok = w.page(&w.session(1, false), None, 20, false).await?;
    assert_eq!(ok.cards.len(), 2);

    // The primary needing sign-in cuts off the state file: still a 200.
    let mut record = w
        .fakes
        .store
        .mailboxes()
        .get(&w.primary)
        .await?
        .ok_or("mailbox")?;
    record.record.status = MailboxStatus::NeedsSignIn;
    w.fakes
        .store
        .mailboxes()
        .put(&record.record, Precondition::Matches(record.version))
        .await?;
    w.app.tokens.forget(&w.primary);
    let none = w.page(&w.session(1, false), None, 20, false).await?;
    assert!(none.cards.is_empty());
    assert_eq!(none.mailbox_errors.len(), 2);
    Ok(())
}

// ---------------------------------------------------------------------------
// FD-03
// ---------------------------------------------------------------------------

/// Five old messages read in session 1, two pages (so the position is stored).
async fn read_old_mail(w: &World) -> Result<(), Box<dyn std::error::Error>> {
    for t in 1..=5 {
        w.seed(&w.primary, "old", t);
    }
    let s1 = w.session(1, false);
    let first = w.page(&s1, None, 2, false).await?;
    assert_eq!(received(&first), vec![5, 4]);
    let second = w.page(&s1, first.next_cursor, 2, false).await?;
    assert_eq!(received(&second), vec![3, 2]);
    Ok(())
}

#[tokio::test]
async fn fd_03_ac1_new_mail_before_backlog() -> TestResult {
    let w = World::new().await?;
    read_old_mail(&w).await?;
    w.seed(&w.primary, "new", 100);
    w.seed(&w.primary, "new", 101);
    let s2 = w.session(2, false);
    let page = w.page(&s2, None, 5, false).await?;
    assert_eq!(page.phase, Phase::New);
    assert_eq!(received(&page), vec![101, 100]);
    assert!(!page.phase_changed);
    let next = w.page(&s2, page.next_cursor, 5, false).await?;
    assert_eq!(next.phase, Phase::Backlog);
    assert!(next.cards.iter().all(|c| c.received_at < at(100)));
    Ok(())
}

#[tokio::test]
async fn fd_03_ac2_phase_changed_once() -> TestResult {
    let w = World::new().await?;
    read_old_mail(&w).await?;
    w.seed(&w.primary, "new", 100);
    let s2 = w.session(2, false);
    let new = w.page(&s2, None, 5, false).await?;
    assert_eq!((new.phase, new.phase_changed), (Phase::New, false));
    let divider = w.page(&s2, new.next_cursor, 5, false).await?;
    assert_eq!(
        (divider.phase, divider.phase_changed),
        (Phase::Backlog, true)
    );
    let later = w.page(&s2, divider.next_cursor, 5, false).await?;
    assert_eq!((later.phase, later.phase_changed), (Phase::Backlog, false));

    Ok(())
}

#[tokio::test]
async fn fd_03_ac2_no_new_mail_changes_phase_on_first_page() -> TestResult {
    let w = World::new().await?;
    read_old_mail(&w).await?;
    // A session with no new mail at all: the first page is already the change.
    let s3 = w.session(3, false);
    let first = w.page(&s3, None, 1, false).await?;
    assert_eq!((first.phase, first.phase_changed), (Phase::Backlog, true));
    assert!(first.next_cursor.is_some());
    let second = w.page(&s3, first.next_cursor, 1, false).await?;
    assert_eq!(
        (second.phase, second.phase_changed),
        (Phase::Backlog, false)
    );
    Ok(())
}

#[tokio::test]
async fn fd_03_ac3_null_cursor_resumes_position() -> TestResult {
    let w = World::new().await?;
    for t in 1..=9 {
        w.seed(&w.primary, "m", t);
    }
    let s1 = w.session(1, false);
    let first = w.page(&s1, None, 3, false).await?;
    let second = w.page(&s1, first.next_cursor, 3, false).await?;
    assert_eq!(received(&second), vec![6, 5, 4]);
    // A later visit with no cursor lands on the page last asked for.
    let resumed = w.page(&s1, None, 3, false).await?;
    assert_eq!(received(&resumed), received(&second));
    assert_eq!(ids(&resumed), ids(&second));
    Ok(())
}

#[tokio::test]
async fn fd_03_ac4_no_daily_cap() -> TestResult {
    let w = World::new().await?;
    for t in 0..500 {
        w.seed(&w.primary, "bulk", t);
    }
    let s1 = w.session(1, false);
    let mut seen = BTreeSet::new();
    let mut cursor = None;
    let mut pages = 0;
    loop {
        let page = w.page(&s1, cursor, 50, false).await?;
        pages += 1;
        for id in ids(&page) {
            assert!(seen.insert(id), "a card repeated");
        }
        cursor = page.next_cursor;
        if cursor.is_none() {
            break;
        }
        assert!(pages < 20, "paging did not end");
    }
    assert_eq!(seen.len(), 500);
    assert_eq!(pages, 11, "ten full pages and one empty page");
    Ok(())
}

#[tokio::test]
async fn fd_03_ac4_same_second_messages_neither_gap_nor_repeat() -> TestResult {
    let w = World::new().await?;
    for _ in 0..5 {
        w.seed(&w.primary, "same", 7);
    }
    w.seed(&w.primary, "older", 3);
    let s1 = w.session(1, false);
    let mut seen = BTreeSet::new();
    let mut cursor = None;
    for _ in 0..6 {
        let page = w.page(&s1, cursor, 2, false).await?;
        for id in ids(&page) {
            assert!(seen.insert(id), "a card repeated");
        }
        cursor = page.next_cursor;
        if cursor.is_none() {
            break;
        }
    }
    assert_eq!(seen.len(), 6);
    Ok(())
}

#[tokio::test]
async fn fd_03_ac4_limit_out_of_range_is_invalid_request() -> TestResult {
    let w = World::new().await?;
    for limit in [0, 51] {
        let err = w.page(&w.session(1, false), None, limit, false).await;
        assert_eq!(
            err.err(),
            Some(ApiError::InvalidRequest {
                fields: vec!["/limit".to_owned()]
            })
        );
    }
    Ok(())
}

#[tokio::test]
async fn fd_03_ac5_refresh_finds_new_mail() -> TestResult {
    let w = World::new().await?;
    read_old_mail(&w).await?;
    let s1 = w.session(1, false);
    w.seed(&w.primary, "fresh", 200);
    // Without refresh the same session keeps paging the backlog.
    let quiet = w.page(&s1, None, 5, false).await?;
    assert!(!received(&quiet).contains(&200));
    // With refresh every mailbox is checked for new mail first.
    let refreshed = w.page(&s1, None, 5, true).await?;
    assert_eq!(refreshed.phase, Phase::New);
    assert_eq!(received(&refreshed), vec![200]);
    Ok(())
}

// ---------------------------------------------------------------------------
// FD-04
// ---------------------------------------------------------------------------

#[tokio::test]
async fn fd_04_ac1_moved_message_skipped_silently() -> TestResult {
    let w = World::new().await?;
    w.seed(&w.primary, "a", 30);
    let gone = w.seed(&w.primary, "b", 20);
    w.seed(&w.primary, "c", 10);
    // Gone from the provider between listing and building the card.
    w.fakes
        .mailbox
        .fail_next(MailOp::GetPreview, MailError::NotFound);
    let page = w.page(&w.session(1, false), None, 20, false).await?;
    assert!(page.mailbox_errors.is_empty());
    assert_eq!(page.cards.len(), 2);

    // Trashed since listing: filtered out of the inbox listing.
    let ctx = w
        .app
        .tokens
        .mailbox_ctx(&w.app, &w.user, &w.primary)
        .await?;
    w.fakes.mailbox.trash(&ctx, &gone).await?;
    let again = w.page(&w.session(1, false), None, 20, false).await?;
    assert_eq!(received(&again), vec![30, 10]);

    // A skipped card that has left the inbox is dropped from the queue.
    let key = w.primary.0;
    let gone_id = gone.as_str().to_owned();
    let session = w.session(1, false);
    w.state_store()
        .update(&w.user, move |s: &mut UserState| {
            s.skips.session_record_id = Some(session.session_record_id.0);
            s.skips.queue.push(SkipReturn {
                mailbox_id: MailboxId::new(key),
                message_id: gone_id.clone(),
                after_cards: 1,
            });
        })
        .await?;
    let third = w.page(&w.session(1, false), None, 20, false).await?;
    assert_eq!(received(&third), vec![30, 10]);
    assert!(w.state().await?.skips.queue.is_empty());
    Ok(())
}

// ---------------------------------------------------------------------------
// SW-02 AC2
// ---------------------------------------------------------------------------

/// Record a skip the way the swipe does: bump the count and queue the return
/// from a draw of the seeded `Rng`.
async fn skip_card(
    w: &World,
    session: &AuthedSession,
    id: &MessageId,
    count: u8,
) -> Result<Option<u32>, Box<dyn std::error::Error>> {
    let draw = u64::from_le_bytes(w.fakes.rng.bytes32()[..8].try_into()?);
    let after = choose_skip_return(count, 0, draw, &Tunables::default());
    let key = skip_key(&w.primary, id);
    let mailbox = w.primary;
    let message = id.as_str().to_owned();
    let record = session.session_record_id.0;
    w.state_store()
        .update(&w.user, move |s: &mut UserState| {
            s.skips.session_record_id = Some(record);
            s.skips.counts.insert(key.clone(), count);
            if let Some(after_cards) = after {
                s.skips.queue.push(SkipReturn {
                    mailbox_id: mailbox,
                    message_id: message.clone(),
                    after_cards,
                });
            }
        })
        .await?;
    Ok(after)
}

/// Page until `target` comes back (as a return), at most `max_pages` pages.
async fn page_until_back(
    w: &World,
    session: &AuthedSession,
    mut cursor: Option<String>,
    target: &MessageId,
    max_pages: usize,
) -> Result<(Option<String>, Vec<CardDto>), Box<dyn std::error::Error>> {
    let mut hits = Vec::new();
    for _ in 0..max_pages {
        let page = w.page(session, cursor.clone(), 5, false).await?;
        cursor = page.next_cursor;
        let found = page
            .cards
            .into_iter()
            .filter(|c| c.message_id == target.as_str());
        hits.extend(found);
        if !hits.is_empty() || cursor.is_none() {
            break;
        }
    }
    Ok((cursor, hits))
}

#[tokio::test]
async fn sw_02_ac2_skipped_card_returns_at_most_twice() -> TestResult {
    let w = World::new().await?;
    let target = w.seed(&w.primary, "target", 1000);
    for t in 0..120 {
        w.seed(&w.primary, "filler", t);
    }
    let s1 = w.session(1, false);
    let first = w.page(&s1, None, 5, false).await?;
    assert_eq!(first.cards[0].message_id, target.as_str());
    let mut cursor = first.next_cursor;

    // Skip 1 and skip 2 each bring the card back once.
    for count in [1u8, 2] {
        let after = skip_card(&w, &s1, &target, count).await?;
        assert!(after.is_some());
        let (next, hits) = page_until_back(&w, &s1, cursor, &target, 12).await?;
        cursor = next;
        assert_eq!(hits.len(), 1, "skip {count} returns the card once");
        assert_eq!(hits[0].skip_count, count);
    }
    // Skip 3 does not return it, and the card stays hidden for the session.
    assert_eq!(skip_card(&w, &s1, &target, 3).await?, None);
    let mut again = 0;
    while let Some(c) = cursor {
        let page = w.page(&s1, Some(c), 5, false).await?;
        again += page
            .cards
            .iter()
            .filter(|card| card.message_id == target.as_str())
            .count();
        cursor = page.next_cursor;
    }
    assert_eq!(again, 0);
    Ok(())
}

#[tokio::test]
async fn sw_02_ac2_skips_reset_on_new_session() -> TestResult {
    let w = World::new().await?;
    let target = w.seed(&w.primary, "target", 50);
    w.seed(&w.primary, "other", 40);
    let s1 = w.session(1, false);
    let first = w.page(&s1, None, 5, false).await?;
    assert_eq!(first.cards.len(), 2);
    skip_card(&w, &s1, &target, 2).await?;
    let stored = w.state().await?.skips;
    assert_eq!(stored.counts.len(), 1);
    assert_eq!(stored.queue.len(), 1);

    // A new session starts clean: counts gone, queue gone, the card is back.
    let s2 = w.session(2, false);
    let page = w.page(&s2, None, 5, false).await?;
    let back: Vec<&CardDto> = page
        .cards
        .iter()
        .filter(|c| c.message_id == target.as_str())
        .collect();
    assert_eq!(back.len(), 1);
    assert_eq!(back[0].skip_count, 0);
    let skips = w.state().await?.skips;
    assert!(skips.counts.is_empty() && skips.queue.is_empty());
    assert_eq!(skips.session_record_id, Some(s2.session_record_id.0));
    Ok(())
}

// ---------------------------------------------------------------------------
// GM-08
// ---------------------------------------------------------------------------

async fn set_seen(
    w: &World,
    entries: Vec<(String, u32)>,
) -> Result<(), Box<dyn std::error::Error>> {
    w.state_store()
        .update(&w.user, move |s: &mut UserState| {
            s.sender_stats.clear();
            for (key, seen) in &entries {
                s.sender_stats.insert(
                    key.clone(),
                    SenderStats {
                        seen: *seen,
                        ..SenderStats::default()
                    },
                );
            }
        })
        .await?;
    Ok(())
}

#[tokio::test]
async fn gm_08_ac1_boss_threshold() -> TestResult {
    let w = World::new().await?;
    w.seed(&w.primary, "boss", 10);
    let s1 = w.session(1, false);
    let key = "boss@example.com".to_owned();

    set_seen(&w, vec![(key.clone(), 19)]).await?;
    let page = w.page(&s1, None, 1, false).await?;
    assert!(page.cards[0].boss.is_none(), "19 seen is not a boss");

    set_seen(&w, vec![(key.clone(), 20)]).await?;
    let page = w.page(&s1, None, 1, false).await?;
    assert!(page.cards[0].boss.is_some(), "20 seen and top 5 is a boss");

    // Five senders ahead of it push it out of the top five.
    let mut crowd: Vec<(String, u32)> = (0..5)
        .map(|n| (format!("rival{n}@example.com"), 100 + n))
        .collect();
    crowd.push((key, 20));
    set_seen(&w, crowd).await?;
    let page = w.page(&s1, None, 1, false).await?;
    assert!(page.cards[0].boss.is_none());
    Ok(())
}

#[tokio::test]
async fn gm_08_ac2_boss_remaining_from_one_count() -> TestResult {
    let w = World::new().await?;
    w.seed(&w.primary, "boss", 40);
    for t in [30, 20, 10] {
        w.seed(&w.primary, "boss", t);
    }
    w.seed(&w.primary, "other", 5);
    set_seen(&w, vec![("boss@example.com".to_owned(), 25)]).await?;
    let before = w.fakes.mailbox.calls(MailOp::CountMessages);
    let page = w.page(&w.session(1, false), None, 1, false).await?;
    assert_eq!(page.cards.len(), 1);
    let boss = page.cards[0].boss.as_ref().ok_or("boss")?;
    assert_eq!(boss.remaining, 4, "inbox mail left from the sender");
    assert_eq!(w.fakes.mailbox.calls(MailOp::CountMessages) - before, 1);

    // A failed count leaves the card without a boss bar.
    set_seen(&w, vec![("boss@example.com".to_owned(), 25)]).await?;
    w.fakes
        .mailbox
        .fail_next(MailOp::CountMessages, MailError::Transient);
    let page = w.page(&w.session(1, false), None, 1, false).await?;
    assert!(page.cards[0].boss.is_none());
    Ok(())
}

// ---------------------------------------------------------------------------
// The route: logs, token types and the empty Feed
// ---------------------------------------------------------------------------

struct Authed {
    cookie: String,
    csrf: String,
}

async fn authed(w: &World) -> Result<Authed, Box<dyn std::error::Error>> {
    let (cookie, record) = SessionService::new(&w.app)
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
    Ok(Authed {
        cookie,
        csrf: record.record.csrf_token.clone(),
    })
}

async fn post_feed(
    router: &Router,
    a: &Authed,
    body: &str,
) -> Result<Response, Box<dyn std::error::Error>> {
    let req = Request::builder()
        .method(Method::POST)
        .uri("/api/v1/feed/next")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::ORIGIN, ORIGIN)
        .header(header::COOKIE, HeaderValue::from_str(&a.cookie)?)
        .header("x-csrf-token", &a.csrf)
        .body(Body::from(body.to_owned()))?;
    Ok(router.clone().oneshot(req).await?)
}

async fn json_of(resp: Response) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await?;
    Ok(serde_json::from_slice(&bytes)?)
}

#[tokio::test]
async fn xc_01_feed_logs_hold_no_corpus_values() -> TestResult {
    let _serial = SERIAL.lock().await;
    let w = World::new().await?;
    let corpus = testkit::corpus::load()?;
    for case in &corpus.cases {
        w.fakes.mailbox.seed(&w.primary, case.seed_message());
    }
    let router = build_router(w.app.clone());
    let a = authed(&w).await?;
    let clock = obs::arc(obs::FixedClock(w.fakes.clock.now()));
    let (capture, _guard) = obs::capture("api", clock);
    let resp = post_feed(&router, &a, r#"{"cursor":null,"limit":50,"refresh":true}"#).await?;
    assert_eq!(resp.status(), StatusCode::OK);
    let json = json_of(resp).await?;
    let cards = json["cards"].as_array().ok_or("cards")?;
    assert!(!cards.is_empty());
    // The response matches the FeedPage and Card schemas (additionalProperties
    // false, so the key sets are exact).
    let page_keys: BTreeSet<&str> = json
        .as_object()
        .ok_or("object")?
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        page_keys,
        BTreeSet::from([
            "cards",
            "next_cursor",
            "phase",
            "phase_changed",
            "mailbox_errors",
            "rule_actions_applied"
        ])
    );
    let card_keys: BTreeSet<&str> = cards[0]
        .as_object()
        .ok_or("object")?
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        card_keys,
        BTreeSet::from([
            "mailbox_id",
            "message_id",
            "received_at",
            "sender_name",
            "sender_address",
            "subject",
            "preview",
            "bulk_score",
            "bulk_reason",
            "class",
            "unsubscribe_method",
            "has_one_click",
            "suggestion",
            "keep_prompt",
            "skip_count",
            "boss",
            "provider_web_url",
            "classification_token"
        ])
    );
    let mut needles = corpus.all_canaries();
    needles.extend(corpus.all_addresses());
    needles.extend(corpus.all_urls());
    let leaks = obs::scan_for_leaks(&capture.text(), &needles);
    assert!(leaks.is_empty(), "log leak at {leaks:?}");
    Ok(())
}

#[tokio::test]
async fn asvs_v9_2_2_feed_rejects_undo_token_as_cursor() -> TestResult {
    let w = World::new().await?;
    w.seed(&w.primary, "a", 1);
    let router = build_router(w.app.clone());
    let a = authed(&w).await?;
    // A real, valid undo token for this user and session...
    let session = SessionService::new(&w.app)
        .load(&{
            let mut headers = axum::http::HeaderMap::new();
            headers.insert(header::COOKIE, HeaderValue::from_str(&a.cookie)?);
            headers
        })
        .await?
        .ok_or("session")?;
    let user_record = w.fakes.store.users().get(&w.user).await?.ok_or("user")?;
    let sealer = SealedTokens::new(
        Arc::clone(&w.app.ports.keys),
        Arc::clone(&w.app.ports.clock),
    );
    let undo = sealer
        .seal(
            TokenType::Undo,
            &w.user,
            &user_record.record.wrapped_data_key,
            &session.record.record.session_record_id,
            w.fakes.clock.now() + Duration::hours(1),
            &serde_json::json!({"positions": {}, "phase": "new"}),
        )
        .await?;
    // ...is refused as a cursor.
    let body = serde_json::json!({"cursor": undo, "limit": 5, "refresh": false}).to_string();
    let resp = post_feed(&router, &a, &body).await?;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    assert_eq!(json_of(resp).await?["code"], "invalid_request");
    // A junk cursor is refused the same way.
    let resp = post_feed(&router, &a, r#"{"cursor":"nope","limit":5}"#).await?;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    // A bad limit is the same code, with the field named.
    let resp = post_feed(&router, &a, r#"{"cursor":null,"limit":0}"#).await?;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    assert_eq!(json_of(resp).await?["fields"][0], "/limit");
    Ok(())
}

#[tokio::test]
async fn s9_feed_empty_no_mail() -> TestResult {
    let w = World::new().await?;
    w.add_mailbox("sub-b", "b@example.com", false).await?;
    let page = w.page(&w.session(1, false), None, 20, true).await?;
    assert!(page.cards.is_empty());
    assert!(page.next_cursor.is_none());
    assert!(page.mailbox_errors.is_empty());
    assert_eq!(page.phase, Phase::Backlog);
    assert_eq!(page.rule_actions_applied, 0);
    // The rules hook is a no-op until T-609.
    let state = w.state().await?;
    let outcome = apply_rules(&w.app, &w.session(1, false), &state, &[]).await?;
    assert!(outcome.acted_on.is_empty() && outcome.applied == 0);
    Ok(())
}
