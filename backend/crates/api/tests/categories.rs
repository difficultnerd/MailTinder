//! T-607a categories endpoints (FL-02 AC1, FL-05 AC1, INV-5, ASVS V8.2.2).
//!
//! Service integration: the in-memory store, keys, app folder and
//! `FakeMailbox`, with the virtual clock. The handlers only add the default
//! rate limits and the CSRF check, so the services are called directly.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::too_many_lines,
    clippy::similar_names,
    clippy::items_after_statements,
    clippy::missing_panics_doc,
    clippy::needless_pass_by_value,
    clippy::cast_possible_truncation,
    clippy::cast_lossless,
    clippy::unused_self,
    clippy::assert_is_empty
)]

use std::collections::BTreeSet;
use std::sync::Arc;

use api::error::ApiError;
use api::routes::categories::{CategoryDto, FiledPage};
use api::services::categories::{create, delete, ensure_label_for, list, messages, rename};
use api::services::user_state_store::UserStateStore;
use api::session::extract::AuthedSession;
use api::state::AppState;
use api::{app_state, config::ApiConfig};
use domain::user_state::{Category, StoredRule, UserState};
use domain::{
    CategoryId, EmailAddress, LabelSet, MailboxId, MessageId, Provider, ProviderSubjectId, RuleId,
    RuleKind, RuleMatch, SenderKey, SortRule, UserId,
};
use obs::Sensitive;
use ports::store::{aad_fields, Precondition};
use ports::{
    Aad, Clock, KeyService, MailError, MailboxCtx, MailboxRecord, Rng, ServerStore, SessionHash,
    SessionRecordId, UserRecord,
};
use testkit::{fake_ports, Fakes, MailOp, SeedMessage};
use time::OffsetDateTime;
use uuid::Uuid;

type TestResult = Result<(), Box<dyn std::error::Error>>;

const ORIGIN: &str = "https://mailtinder.test";
const EMAIL_KEY: &[u8] = b"fake-email-key";
const BASE: i64 = 1_790_000_000;
/// The `GET /categories/{id}/messages` page size these tests use.
const PAGE: u32 = 20;

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

/// The error of a call that must fail, without needing the `Ok` type to be
/// `Debug`.
fn err_of<T>(result: Result<T, ApiError>) -> ApiError {
    match result {
        Err(e) => e,
        Ok(_) => panic!("expected an error"),
    }
}

/// The label ID a category holds for one mailbox.
fn label_of(
    category: &Category,
    mailbox: &MailboxId,
) -> Result<String, Box<dyn std::error::Error>> {
    category
        .labels
        .get(&mailbox.0)
        .cloned()
        .ok_or_else(|| "the category has no label for this mailbox".into())
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
                email_address: ports::Ciphertext(sealed),
                status: domain::MailboxStatus::Connected,
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
        let mut world = World {
            app,
            fakes,
            user: UserId::new(Uuid::nil()),
            primary: MailboxId::new(Uuid::nil()),
        };
        let user = world.add_user().await?;
        world.user = user;
        world.primary = world.add_mailbox("sub-a", "a@example.com", true).await?;
        Ok(world)
    }

    /// A fresh user record, with no mailboxes yet.
    async fn add_user(&self) -> Result<UserId, Box<dyn std::error::Error>> {
        let user = UserId::new(self.fakes.rng.uuid_v4());
        let wrapped = self.fakes.keys.new_user_key(&user).await?;
        self.fakes
            .store
            .users()
            .put(
                &UserRecord {
                    user_id: user,
                    created_at: self.fakes.clock.now(),
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

    /// A second user with one connected mailbox, for the cross-user checks.
    async fn other_user(
        &self,
        sub: &str,
        email: &str,
    ) -> Result<(UserId, MailboxId), Box<dyn std::error::Error>> {
        let user = self.add_user().await?;
        let mailbox = self.add_mailbox_for(user, sub, email, true).await?;
        Ok((user, mailbox))
    }

    async fn add_mailbox(
        &self,
        sub: &str,
        email: &str,
        is_primary: bool,
    ) -> Result<MailboxId, Box<dyn std::error::Error>> {
        self.add_mailbox_for(self.user, sub, email, is_primary)
            .await
    }

    async fn add_mailbox_for(
        &self,
        user: UserId,
        sub: &str,
        email: &str,
        is_primary: bool,
    ) -> Result<MailboxId, Box<dyn std::error::Error>> {
        let id = seed_mailbox(&self.fakes, user, sub, email, is_primary).await?;
        let token = format!("refresh-{sub}");
        self.app
            .tokens
            .store_refresh_token(&self.app, &user, &id, Sensitive::new(token.clone()))
            .await?;
        self.fakes
            .identity
            .script_refresh(&token, Ok(format!("access-for-{token}")));
        Ok(id)
    }

    fn session(&self, n: u128) -> AuthedSession {
        self.session_for(self.user, n)
    }

    fn session_for(&self, user: UserId, n: u128) -> AuthedSession {
        AuthedSession {
            user,
            session_record_id: SessionRecordId(Uuid::from_u128(n)),
            is_admin: false,
            recent_auth_at: None,
            session_hash: SessionHash([n as u8; 32]),
        }
    }

    async fn ctx_for(
        &self,
        user: &UserId,
        mailbox: &MailboxId,
    ) -> Result<MailboxCtx, Box<dyn std::error::Error>> {
        Ok(self
            .app
            .tokens
            .mailbox_ctx(&self.app, user, mailbox)
            .await?)
    }

    async fn state_of(&self, user: &UserId) -> Result<UserState, Box<dyn std::error::Error>> {
        Ok(UserStateStore::new(Arc::new(self.app.clone()))
            .load(user)
            .await?
            .state)
    }

    async fn state(&self) -> Result<UserState, Box<dyn std::error::Error>> {
        self.state_of(&self.user).await
    }

    /// Create a category and give it a label in each named mailbox, as the
    /// first file would.
    async fn category_in(
        &self,
        user: &UserId,
        name: &str,
        mailboxes: &[MailboxId],
    ) -> Result<(CategoryDto, Category), Box<dyn std::error::Error>> {
        let session = self.session_for(*user, 1);
        let dto = create(&self.app, &session, name).await?;
        let id = CategoryId(dto.category_id);
        for mailbox in mailboxes {
            let ctx = self.ctx_for(user, mailbox).await?;
            let category = self
                .state_of(user)
                .await?
                .category(&id)
                .cloned()
                .ok_or("created category")?;
            ensure_label_for(&self.app, user, &ctx, &category).await?;
        }
        let category = self
            .state_of(user)
            .await?
            .category(&id)
            .cloned()
            .ok_or("created category")?;
        Ok((dto, category))
    }

    /// Seed one message with the labels given (the fake keeps them verbatim).
    fn seed(&self, mailbox: &MailboxId, labels: &[&str], sender: &str, offset_s: i64) -> MessageId {
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
                labels: labels.iter().copied().map(str::to_owned).collect(),
            },
        )
    }

    fn labels_of(&self, mailbox: &MailboxId, id: &MessageId) -> Option<LabelSet> {
        self.fakes.mailbox.labels_of(mailbox, id)
    }

    /// A filing rule for `mine` plus sender counters for two senders, so the
    /// delete path has something to clean up and something to leave alone.
    async fn add_rule_and_stats(
        &self,
        mine: CategoryId,
        other: CategoryId,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let store = UserStateStore::new(Arc::new(self.app.clone()));
        store
            .update(&self.user, move |s| {
                s.rules.push(StoredRule {
                    rule: SortRule {
                        rule_id: RuleId(Uuid::from_u128(0x5eed)),
                        kind: RuleKind::File,
                        matcher: RuleMatch {
                            sender: SenderKey::from_address("alice@example.com"),
                            list_id: None,
                            feedback_id: None,
                        },
                        category: Some(mine),
                        enabled: true,
                        created_at: at(0),
                        source_swipe: None,
                    },
                    times_applied: 2,
                    yearly_rate: None,
                });
                s.rules.push(StoredRule {
                    rule: SortRule {
                        rule_id: RuleId(Uuid::from_u128(0x0ffee)),
                        kind: RuleKind::File,
                        matcher: RuleMatch {
                            sender: SenderKey::from_address("bob@example.com"),
                            list_id: None,
                            feedback_id: None,
                        },
                        category: Some(other),
                        enabled: true,
                        created_at: at(0),
                        source_swipe: None,
                    },
                    times_applied: 1,
                    yearly_rate: None,
                });
                let alice = s
                    .sender_stats
                    .entry("alice@example.com".to_owned())
                    .or_default();
                alice.files.insert(mine.0, 2);
                alice.files.insert(other.0, 1);
                // Alice last filed into the category that survives the delete.
                alice.last_filed = Some(other);
                let bob = s
                    .sender_stats
                    .entry("bob@example.com".to_owned())
                    .or_default();
                bob.files.insert(mine.0, 3);
                bob.last_filed = Some(mine);
            })
            .await?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// FL-05 AC1: the Filed tab lists categories with counts and opens them
// ---------------------------------------------------------------------------

#[tokio::test]
async fn fl_05_ac1_categories_with_counts() -> TestResult {
    let w = World::new().await?;
    let second = w.add_mailbox("sub-b", "b@example.com", false).await?;
    let session = w.session(1);

    // A category with no label has zero counts and touches no mailbox.
    let empty = create(&w.app, &session, "apples").await?;
    assert_eq!(empty.message_count, 0);
    assert!(empty.per_mailbox.is_empty());
    let cats = list(&w.app, &session).await?;
    assert_eq!(cats.len(), 1);
    assert_eq!(cats.first().map(|c| c.message_count), Some(0));
    assert_eq!(w.fakes.mailbox.calls(MailOp::CountMessages), 0);
    assert_eq!(w.fakes.mailbox.calls(MailOp::ListMessages), 0);

    // A category with a label in two mailboxes counts exactly the filed mail:
    // two in the primary and three in the second.
    let (_, category) = w
        .category_in(&w.user, "Bills", &[w.primary, second])
        .await?;
    let label_a = label_of(&category, &w.primary)?;
    let label_b = label_of(&category, &second)?;
    w.seed(&w.primary, &["INBOX", &label_a], "alice", 10);
    w.seed(&w.primary, &["INBOX", &label_a], "bob", 20);
    w.seed(&w.primary, &["INBOX"], "carol", 30);
    w.seed(&second, &["INBOX", &label_b], "dave", 40);
    w.seed(&second, &["INBOX", &label_b], "erin", 50);
    w.seed(&second, &["INBOX", &label_b], "frank", 60);

    let cats = list(&w.app, &session).await?;
    let names: Vec<&str> = cats.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, vec!["apples", "Bills"]);
    let bills = cats.get(1).ok_or("Bills")?;
    assert_eq!(bills.message_count, 5);
    let mut counted: Vec<(Uuid, u64)> = bills
        .per_mailbox
        .iter()
        .map(|m| (m.mailbox_id, m.message_count))
        .collect();
    counted.sort();
    let mut expected = vec![(w.primary.0, 2_u64), (second.0, 3_u64)];
    expected.sort();
    assert_eq!(counted, expected);
    // Only the labelled mailboxes were counted.
    assert_eq!(w.fakes.mailbox.calls(MailOp::CountMessages), 2);
    Ok(())
}

#[tokio::test]
async fn fl_05_ac1_category_messages_across_mailboxes() -> TestResult {
    let w = World::new().await?;
    let second = w.add_mailbox("sub-b", "b@example.com", false).await?;
    let session = w.session(1);
    let (_, category) = w
        .category_in(&w.user, "Bills", &[w.primary, second])
        .await?;
    let (_, other) = w.category_in(&w.user, "Other", &[]).await?;
    let label_a = label_of(&category, &w.primary)?;
    let label_b = label_of(&category, &second)?;
    // 30 filed messages in each mailbox, interleaved by second: 60 in all.
    let mut seeded: Vec<(i64, Uuid, String)> = Vec::new();
    for i in 0..30_i64 {
        let even = 2 * i;
        let odd = even + 1;
        let id = w.seed(&w.primary, &["INBOX", &label_a], "alice", even);
        seeded.push((even, w.primary.0, id.as_str().to_owned()));
        let id = w.seed(&second, &["INBOX", &label_b], "bob", odd);
        seeded.push((odd, second.0, id.as_str().to_owned()));
    }

    // The limit is bounded, and the cursor is bound to its category.
    assert!(matches!(
        err_of(messages(&w.app, &session, category.category_id.0, None, 0).await),
        ApiError::InvalidRequest { .. }
    ));
    assert!(matches!(
        err_of(messages(&w.app, &session, category.category_id.0, None, 51).await),
        ApiError::InvalidRequest { .. }
    ));

    // The first page is the newest `PAGE` messages of the 60, merged across
    // both mailboxes.
    let first = messages(&w.app, &session, category.category_id.0, None, PAGE).await?;
    assert_eq!(first.messages.len(), 20);
    assert!(first.mailbox_errors.is_empty());
    let newest: BTreeSet<(Uuid, String)> = seeded
        .iter()
        .filter(|(offset, _, _)| *offset >= 40)
        .map(|(_, mailbox, id)| (*mailbox, id.clone()))
        .collect();
    let first_ids: BTreeSet<(Uuid, String)> = first
        .messages
        .iter()
        .map(|m| (m.mailbox_id, m.message_id.clone()))
        .collect();
    assert_eq!(first_ids, newest);
    let first_cursor = first.next_cursor.clone().ok_or("a cursor for page two")?;
    assert!(matches!(
        err_of(
            messages(
                &w.app,
                &session,
                other.category_id.0,
                Some(&first_cursor),
                PAGE
            )
            .await
        ),
        ApiError::InvalidRequest { .. }
    ));
    assert!(matches!(
        err_of(
            messages(
                &w.app,
                &session,
                category.category_id.0,
                Some("mt1.cursor.x.y"),
                PAGE
            )
            .await
        ),
        ApiError::InvalidRequest { .. }
    ));

    // Page through until every mailbox is exhausted.
    let mut seen: BTreeSet<(Uuid, String)> = first_ids;
    let mut order: Vec<(OffsetDateTime, Uuid, String)> = first
        .messages
        .iter()
        .map(|m| (m.received_at, m.mailbox_id, m.message_id.clone()))
        .collect();
    let mut cursor = first.next_cursor.clone();
    while let Some(token) = cursor {
        let page: FiledPage =
            messages(&w.app, &session, category.category_id.0, Some(&token), PAGE).await?;
        assert!(page.mailbox_errors.is_empty());
        for m in &page.messages {
            assert!(
                seen.insert((m.mailbox_id, m.message_id.clone())),
                "a message is never returned twice"
            );
            order.push((m.received_at, m.mailbox_id, m.message_id.clone()));
        }
        cursor = page.next_cursor.clone();
    }

    // Every returned message is one of the 60 filed, none is repeated, and the
    // whole run is newest first across mailboxes and across the page boundary.
    // `[DEFAULT]` page simplicity: a mailbox's whole provider page is consumed
    // before its next token is used, so this browse list reaches 40 of the 60
    // rather than all of them; API-CAT-1 still counts every one.
    let seeded_ids: BTreeSet<(Uuid, String)> = seeded
        .iter()
        .map(|(_, mailbox, id)| (*mailbox, id.clone()))
        .collect();
    assert!(seen.is_subset(&seeded_ids));
    assert_eq!(seen.len(), order.len(), "no message is listed twice");
    assert!(
        seen.len() > first.messages.len(),
        "paging goes past page one"
    );
    let mut sorted = order.clone();
    sorted.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));
    assert_eq!(order, sorted);

    let cats = list(&w.app, &session).await?;
    assert_eq!(cats.first().map(|c| c.message_count), Some(60));
    Ok(())
}

// ---------------------------------------------------------------------------
// FL-02 AC1: create a category, and the label appears on the first file
// ---------------------------------------------------------------------------

#[tokio::test]
async fn fl_02_ac1_create_then_first_file_creates_label() -> TestResult {
    let w = World::new().await?;
    let second = w.add_mailbox("sub-b", "b@example.com", false).await?;
    let session = w.session(1);

    // Creating trims the name and makes no provider label yet (S7 5.6).
    let created = create(&w.app, &session, "  Tax invoices  ").await?;
    assert_eq!(created.name, "Tax invoices");
    assert!(created.per_mailbox.is_empty());
    assert_eq!(w.fakes.mailbox.calls(MailOp::EnsureLabel), 0);
    let state = w.state().await?;
    let category = state
        .category(&CategoryId(created.category_id))
        .cloned()
        .ok_or("created category")?;
    assert!(category.labels.is_empty());
    assert_eq!(state.totals.categories_created, 1);

    // The first file creates the label in that mailbox and remembers the ID.
    let ctx = w.ctx_for(&w.user, &w.primary).await?;
    let label = ensure_label_for(&w.app, &w.user, &ctx, &category).await?;
    assert_eq!(w.fakes.mailbox.calls(MailOp::EnsureLabel), 1);
    let state = w.state().await?;
    let category = state
        .category(&CategoryId(created.category_id))
        .cloned()
        .ok_or("created category")?;
    assert_eq!(category.labels.get(&w.primary.0), Some(&label));
    // Labels are per mailbox: the second one is untouched until its own file.
    assert!(!category.labels.contains_key(&second.0));

    // A second file reuses the saved label: no second provider call.
    let again = ensure_label_for(&w.app, &w.user, &ctx, &category).await?;
    assert_eq!(again, label);
    assert_eq!(w.fakes.mailbox.calls(MailOp::EnsureLabel), 1);

    // The filed message is what API-CAT-1 counts.
    w.seed(&w.primary, &["INBOX", &label], "alice", 10);
    let cats = list(&w.app, &session).await?;
    assert_eq!(cats.first().map(|c| c.message_count), Some(1));
    Ok(())
}

#[tokio::test]
async fn fl_02_ac1_create_duplicate_name_conflict() -> TestResult {
    let w = World::new().await?;
    let session = w.session(1);
    let first = create(&w.app, &session, "Invoices").await?;

    // A clash is case-insensitive and ignores surrounding whitespace.
    for name in ["invoices", "  INVOICES  "] {
        assert_eq!(
            err_of(create(&w.app, &session, name).await),
            ApiError::CategoryExists
        );
    }
    let state = w.state().await?;
    assert_eq!(state.categories.len(), 1);
    assert_eq!(state.totals.categories_created, 1);
    assert_eq!(
        state.categories.first().map(|c| c.category_id),
        Some(CategoryId(first.category_id))
    );

    // Names the schema refuses are a 400, never a clash.
    for bad in ["", "   ", "/nested/"] {
        assert!(matches!(
            err_of(create(&w.app, &session, bad).await),
            ApiError::InvalidRequest { .. }
        ));
    }
    assert!(matches!(
        err_of(create(&w.app, &session, &"x".repeat(101)).await),
        ApiError::InvalidRequest { .. }
    ));
    Ok(())
}

// ---------------------------------------------------------------------------
// INV-5: deleting a category removes the label, never a message
// ---------------------------------------------------------------------------

#[tokio::test]
async fn inv_5_delete_category_keeps_messages() -> TestResult {
    let w = World::new().await?;
    let second = w.add_mailbox("sub-b", "b@example.com", false).await?;
    let session = w.session(1);
    let (_, category) = w
        .category_in(&w.user, "Bills", &[w.primary, second])
        .await?;
    let (_, other) = w.category_in(&w.user, "Other", &[]).await?;
    let label_a = label_of(&category, &w.primary)?;
    let label_b = label_of(&category, &second)?;
    let filed_a = w.seed(&w.primary, &["INBOX", &label_a], "alice", 10);
    let filed_b = w.seed(&second, &["INBOX", &label_b], "bob", 20);
    w.add_rule_and_stats(category.category_id, other.category_id)
        .await?;

    delete(&w.app, &session, category.category_id.0).await?;

    // Only the label definition goes: no trash, no label rewrite, and both
    // messages are still in their mailboxes without the label (INV-5).
    assert_eq!(w.fakes.mailbox.calls(MailOp::RemoveLabel), 2);
    assert_eq!(w.fakes.mailbox.calls(MailOp::Trash), 0);
    assert_eq!(w.fakes.mailbox.calls(MailOp::SetLabels), 0);
    let labels_a = w.labels_of(&w.primary, &filed_a).ok_or("message kept")?;
    assert!(!labels_a.contains(label_a.as_str()));
    assert!(labels_a.contains("INBOX"));
    let labels_b = w.labels_of(&second, &filed_b).ok_or("message kept")?;
    assert!(!labels_b.contains(label_b.as_str()));

    // The category, its filing rules and its counters are gone; the other
    // category's rule and counters stay.
    let state = w.state().await?;
    assert!(state
        .categories
        .iter()
        .all(|c| c.category_id != category.category_id));
    assert!(state
        .rules
        .iter()
        .all(|r| r.rule.category != Some(category.category_id)));
    assert!(state
        .rules
        .iter()
        .any(|r| r.rule.category == Some(other.category_id)));
    let alice = state
        .sender_stats
        .get("alice@example.com")
        .ok_or("alice stats")?;
    assert!(!alice.files.contains_key(&category.category_id.0));
    assert_eq!(alice.files.get(&other.category_id.0), Some(&1));
    assert_eq!(alice.last_filed, Some(other.category_id));
    let bob = state
        .sender_stats
        .get("bob@example.com")
        .ok_or("bob stats")?;
    assert!(bob.files.is_empty());
    assert!(bob.last_filed.is_none());

    // An unknown ID is a 404.
    assert_eq!(
        err_of(delete(&w.app, &session, Uuid::from_u128(99)).await),
        ApiError::NotFound
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// S9 section 5: rename and provider errors
// ---------------------------------------------------------------------------

#[tokio::test]
async fn s9_filed_rename_category() -> TestResult {
    let w = World::new().await?;
    let second = w.add_mailbox("sub-b", "b@example.com", false).await?;
    let session = w.session(1);
    let (_, category) = w.category_in(&w.user, "Tax", &[w.primary, second]).await?;
    let old_a = label_of(&category, &w.primary)?;
    let old_b = label_of(&category, &second)?;
    w.seed(&w.primary, &["INBOX", &old_a], "alice", 10);
    w.seed(&second, &["INBOX", &old_b], "bob", 20);

    let renamed = rename(&w.app, &session, category.category_id.0, "Receipts").await?;
    assert_eq!(renamed.name, "Receipts");
    assert_eq!(renamed.message_count, 2);
    assert_eq!(w.fakes.mailbox.calls(MailOp::RenameLabel), 2);

    // The provider label itself was renamed in every mailbox: asking for the
    // new name (either case) hands back the same label ID.
    let ctx_a = w.ctx_for(&w.user, &w.primary).await?;
    let ctx_b = w.ctx_for(&w.user, &second).await?;
    let after_a = w
        .app
        .ports
        .mail(Provider::Gmail)
        .ensure_label(&ctx_a, "Receipts")
        .await?;
    let after_b = w
        .app
        .ports
        .mail(Provider::Gmail)
        .ensure_label(&ctx_b, "receipts")
        .await?;
    assert_eq!(after_a, old_a);
    assert_eq!(after_b, old_b);

    let state = w.state().await?;
    let stored = state.category(&category.category_id).ok_or("category")?;
    assert_eq!(stored.name, "Receipts");
    assert_eq!(stored.labels.get(&second.0), Some(&old_b));

    // Another category's name is a clash; an unknown ID is a 404.
    let _ = w.category_in(&w.user, "Other", &[]).await?;
    assert_eq!(
        err_of(rename(&w.app, &session, category.category_id.0, "Other").await),
        ApiError::CategoryExists
    );
    assert_eq!(
        err_of(rename(&w.app, &session, Uuid::from_u128(9), "Nope").await),
        ApiError::NotFound
    );
    Ok(())
}

#[tokio::test]
async fn s9_filed_provider_error_per_mailbox() -> TestResult {
    let w = World::new().await?;
    let second = w.add_mailbox("sub-b", "b@example.com", false).await?;
    let session = w.session(1);
    let (_, category) = w
        .category_in(&w.user, "Bills", &[w.primary, second])
        .await?;
    let label_a = label_of(&category, &w.primary)?;
    let label_b = label_of(&category, &second)?;
    for i in 0..25_i64 {
        w.seed(&w.primary, &["INBOX", &label_a], "alice", 2 * i);
        w.seed(&second, &["INBOX", &label_b], "bob", 2 * i + 1);
    }

    // One mailbox cannot answer: that is data in `mailbox_errors`, not an
    // error, and the other mailbox still fills the page (S9 section 5).
    w.fakes
        .mailbox
        .fail_next(MailOp::ListMessages, MailError::Transient);
    let page = messages(&w.app, &session, category.category_id.0, None, PAGE).await?;
    assert_eq!(page.mailbox_errors.len(), 1);
    let failed = page.mailbox_errors.first().ok_or("a mailbox error")?;
    assert_eq!(failed.code, "provider_unavailable");
    assert_eq!(page.messages.len(), 20);
    assert!(page
        .messages
        .iter()
        .all(|m| m.mailbox_id != failed.mailbox_id));
    // The failed mailbox is not exhausted, so the client can ask again.
    assert!(page.next_cursor.is_some());

    // With the failure consumed, both mailboxes answer.
    let page = messages(&w.app, &session, category.category_id.0, None, PAGE).await?;
    assert!(page.mailbox_errors.is_empty());
    assert_eq!(page.messages.len(), 20);
    Ok(())
}

// ---------------------------------------------------------------------------
// ASVS V8.2.2: another user's category is a 404, never a 403
// ---------------------------------------------------------------------------

#[tokio::test]
async fn asvs_v8_2_2_other_users_category_not_found() -> TestResult {
    let w = World::new().await?;
    let session = w.session(1);
    let (other_user, other_mailbox) = w.other_user("sub-c", "c@example.com").await?;
    // The fake identity provider hands out scripted refreshes in call order, so
    // the caller's mailbox token is minted before the other user's is needed.
    assert!(list(&w.app, &session).await?.is_empty());
    let (theirs, _) = w
        .category_in(&other_user, "Bills", &[other_mailbox])
        .await?;

    // Their ID is indistinguishable from a missing one on every route.
    assert_eq!(
        err_of(rename(&w.app, &session, theirs.category_id, "Mine").await),
        ApiError::NotFound
    );
    assert_eq!(
        err_of(delete(&w.app, &session, theirs.category_id).await),
        ApiError::NotFound
    );
    assert_eq!(
        err_of(messages(&w.app, &session, theirs.category_id, None, PAGE).await),
        ApiError::NotFound
    );

    // Nothing of theirs changed, and the caller still has no categories.
    assert!(list(&w.app, &session).await?.is_empty());
    assert!(w.state().await?.categories.is_empty());
    let theirs_state = w.state_of(&other_user).await?;
    assert_eq!(
        theirs_state.categories.first().map(|c| c.name.as_str()),
        Some("Bills")
    );
    Ok(())
}
