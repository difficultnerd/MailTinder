//! T-707 Feed integration and pure hook regression tests.
use api::routes::feed::{FeedPage, FeedRequest};
use api::services::{
    delivery_check::{apply_delivery_checks, DeliveryHookInput, ListMail},
    feed::next_page,
    user_state_store::UserStateStore,
};
use api::session::extract::AuthedSession;
use api::{app_state, config::ApiConfig, state::AppState};
use domain::delivery::BusinessCalendar;
use domain::user_state::{HistoryAction, PendingDeliveryCheck, StoredRule, UserState};
use domain::{
    HeaderFacts, MailboxId, MailboxStatus, MessageId, NeedsAttentionReason, Provider,
    ProviderSubjectId, RuleId, SortRule, UserId,
};
use obs::Sensitive;
use ports::store::aad_fields;
use ports::{
    Aad, Ciphertext, KeyService, MailProvider, MailboxCtx, MailboxRecord, Precondition, Rng,
    ServerStore, SessionHash, SessionRecordId, UserRecord,
};
use std::{collections::BTreeSet, sync::Arc};
use testkit::{fake_ports, Fakes, SeedMessage};
use time::{macros::datetime, Duration, OffsetDateTime};
use uuid::Uuid;
type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;
const SENT: OffsetDateTime = datetime!(2026-10-05 09:00 +10);
struct World {
    app: AppState,
    fakes: Fakes,
    user: UserId,
    mailbox: MailboxId,
}
impl World {
    async fn new(enabled: bool) -> TestResult<Self> {
        let (ports, fakes) = fake_ports();
        fakes.clock.set(SENT);
        let config = ApiConfig::new(
            "https://mailtinder.test".into(),
            "fake-client".into(),
            Sensitive::new(b"fake-log-key".to_vec()),
            Sensitive::new(b"fake-email-key".to_vec()),
        )?;
        let mut app = app_state(Arc::new(ports), Arc::new(config));
        app.business_calendar = Arc::new(BusinessCalendar::new(BTreeSet::new()));
        let user = UserId(fakes.rng.uuid_v4());
        let wrapped = fakes.keys.new_user_key(&user).await?;
        fakes
            .store
            .users()
            .put(
                &UserRecord {
                    user_id: user,
                    created_at: SENT,
                    is_admin: false,
                    wrapped_data_key: wrapped.clone(),
                    experiments_consent_version: None,
                    experiments_opted_in_at: None,
                },
                Precondition::MustNotExist,
            )
            .await?;
        let subject = ProviderSubjectId::new("delivery-test")?;
        let mailbox = ports::mailbox_id_for(Provider::Gmail, &subject);
        let email = fakes
            .keys
            .seal(
                &user,
                &wrapped,
                &Aad {
                    user,
                    scope: mailbox.0.to_string(),
                    field: aad_fields::MAILBOX_EMAIL,
                },
                b"delivery@example.com",
            )
            .await?;
        fakes
            .store
            .mailboxes()
            .put(
                &MailboxRecord {
                    mailbox_id: mailbox,
                    user_id: user,
                    provider: Provider::Gmail,
                    provider_subject_id: subject,
                    email_address: Ciphertext(email),
                    status: MailboxStatus::Connected,
                    linked_at: SENT,
                    is_primary: true,
                    refresh_token: None,
                },
                Precondition::MustNotExist,
            )
            .await?;
        app.tokens
            .store_refresh_token(
                &app,
                &user,
                &mailbox,
                Sensitive::new("delivery-refresh".into()),
            )
            .await?;
        fakes
            .identity
            .script_refresh("delivery-refresh", Ok("delivery-access".into()));
        let w = Self {
            app,
            fakes,
            user,
            mailbox,
        };
        let old = w.seed(SENT - Duration::seconds(1));
        let ctx = MailboxCtx {
            mailbox,
            access_token: Sensitive::new("delivery-access".into()),
        };
        let meta = w.fakes.mailbox.get_meta(&ctx, &old).await?;
        let mut rule = SortRule::reject_list_for(&meta, RuleId(w.fakes.rng.uuid_v4()), SENT, None)
            .ok_or("rule")?;
        rule.enabled = enabled;
        w.fakes.mailbox.trash(&ctx, &old).await?;
        w.store()
            .update(&user, |s| {
                s.rules.push(StoredRule {
                    rule: rule.clone(),
                    times_applied: 0,
                    yearly_rate: None,
                });
                s.pending_delivery_checks.push(check(mailbox));
            })
            .await?;
        Ok(w)
    }
    fn store(&self) -> UserStateStore {
        UserStateStore::new(Arc::new(self.app.clone()))
    }
    async fn state(&self) -> TestResult<UserState> {
        Ok(self.store().load(&self.user).await?.state)
    }
    fn seed(&self, received: OffsetDateTime) -> MessageId {
        self.fakes.mailbox.seed(
            &self.mailbox,
            SeedMessage {
                from_display: "Synthetic list".into(),
                from_address: "list@example.com".into(),
                subject: "Synthetic".into(),
                raw_headers: vec![],
                facts: HeaderFacts {
                    list_id: Some("weekly.example.com".into()),
                    list_unsubscribe_present: true,
                    from_authenticated: true,
                    ..HeaderFacts::default()
                },
                preview_text: "Synthetic".into(),
                internal_date: received,
                labels: vec!["INBOX".into()],
            },
        )
    }
    async fn page_at(&self, now: OffsetDateTime) -> TestResult<FeedPage> {
        self.fakes.clock.set(now);
        self.fakes
            .identity
            .script_refresh("delivery-refresh", Ok("delivery-access".into()));
        Ok(next_page(
            &self.app,
            &AuthedSession {
                user: self.user,
                session_record_id: SessionRecordId(Uuid::from_u128(99)),
                is_admin: false,
                recent_auth_at: None,
                session_hash: SessionHash([9; 32]),
            },
            FeedRequest {
                cursor: None,
                limit: 50,
                refresh: true,
            },
        )
        .await?)
    }
    async fn items(&self) -> TestResult<u64> {
        Ok(self
            .fakes
            .store
            .needs_attention()
            .count_for_user(&self.user)
            .await?)
    }
}
fn check(mailbox: MailboxId) -> PendingDeliveryCheck {
    PendingDeliveryCheck {
        sender_key: "list@example.com".into(),
        list_id: Some("weekly.example.com".into()),
        mailbox_id: mailbox,
        unsubscribed_at: SENT,
        mail_seen: false,
        confirm_counted: false,
        pending_ignored_display: None,
    }
}
#[tokio::test]
async fn un_06_ac1_mail_after_5_business_days_raises_item() -> TestResult {
    let w = World::new(true).await?;
    w.seed(datetime!(2026-10-12 09:01 +10));
    w.page_at(SENT + Duration::days(8)).await?;
    assert_eq!(w.items().await?, 1);
    let page = w
        .fakes
        .store
        .needs_attention()
        .by_user(
            &w.user,
            ports::PageRequest {
                limit: 10,
                after: None,
            },
        )
        .await?;
    let item = &page.items.first().ok_or("item")?.record;
    assert_eq!(item.reason_code, NeedsAttentionReason::UnsubscribeIgnored);
    assert!(item.link.is_none());
    let wrapped = w
        .fakes
        .store
        .users()
        .get(&w.user)
        .await?
        .ok_or("user")?
        .record
        .wrapped_data_key;
    let display = w
        .fakes
        .keys
        .open(
            &w.user,
            &wrapped,
            &Aad {
                user: w.user,
                scope: item.item_id.0.to_string(),
                field: aad_fields::NA_SENDER_DISPLAY,
            },
            &item.sender_display.0,
        )
        .await?;
    assert_eq!(display, b"Synthetic list");
    Ok(())
}
#[tokio::test]
async fn un_06_ac1_mail_trashed_by_rule() -> TestResult {
    let w = World::new(true).await?;
    let id = w.seed(SENT + Duration::days(10));
    assert!(w.page_at(SENT + Duration::days(11)).await?.cards.is_empty());
    assert!(w
        .fakes
        .mailbox
        .labels_of(&w.mailbox, &id)
        .ok_or("labels")?
        .contains("TRASH"));
    assert_eq!(
        w.state().await?.history.first().ok_or("history")?.action,
        HistoryAction::TrashedByRule
    );
    assert_eq!(w.items().await?, 1);
    Ok(())
}
#[tokio::test]
async fn un_06_ac1_one_item_per_check() -> TestResult {
    let w = World::new(true).await?;
    for n in 0..3 {
        w.seed(SENT + Duration::days(10) + Duration::minutes(n));
    }
    w.page_at(SENT + Duration::days(11)).await?;
    w.page_at(SENT + Duration::days(12)).await?;
    assert_eq!(w.items().await?, 1);
    assert_eq!(w.state().await?.pending_delivery_checks.len(), 0);
    Ok(())
}
#[tokio::test]
async fn un_06_ac2_day_10_mail_raises_item_never_confirmed() -> TestResult {
    let w = World::new(true).await?;
    w.seed(SENT + Duration::days(10));
    w.page_at(SENT + Duration::days(10)).await?;
    assert_eq!(w.items().await?, 1);
    assert_eq!(w.state().await?.totals.unsubscribes_confirmed, 0);
    w.page_at(SENT + Duration::days(15)).await?;
    assert_eq!(w.state().await?.totals.unsubscribes_confirmed, 0);
    Ok(())
}
#[tokio::test]
async fn st_02_ac1_confirmed_after_14_days_without_mail() -> TestResult {
    let w = World::new(true).await?;
    w.page_at(SENT + Duration::days(14)).await?;
    let s = w.state().await?;
    assert_eq!(s.totals.unsubscribes_confirmed, 1);
    assert!(
        s.pending_delivery_checks
            .first()
            .ok_or("check retained")?
            .confirm_counted
    );
    assert_eq!(w.items().await?, 0);
    Ok(())
}
#[tokio::test]
async fn st_02_ac1_not_confirmed_at_13_days_23_hours() -> TestResult {
    let w = World::new(true).await?;
    w.page_at(SENT + Duration::days(14) - Duration::hours(1))
        .await?;
    assert_eq!(w.state().await?.totals.unsubscribes_confirmed, 0);
    Ok(())
}
#[tokio::test]
async fn st_02_ac1_mail_within_grace_blocks_confirmation() -> TestResult {
    let w = World::new(true).await?;
    w.seed(SENT + Duration::days(2));
    w.page_at(SENT + Duration::days(2)).await?;
    w.page_at(SENT + Duration::days(14)).await?;
    w.page_at(SENT + Duration::days(20)).await?;
    let s = w.state().await?;
    assert_eq!(s.totals.unsubscribes_confirmed, 0);
    assert!(
        s.pending_delivery_checks
            .first()
            .ok_or("retained")?
            .mail_seen
    );
    assert_eq!(w.items().await?, 0);
    Ok(())
}
#[tokio::test]
async fn st_02_ac1_confirmed_counted_once() -> TestResult {
    let w = World::new(true).await?;
    w.page_at(SENT + Duration::days(14)).await?;
    w.page_at(SENT + Duration::days(15)).await?;
    assert_eq!(w.state().await?.totals.unsubscribes_confirmed, 1);
    Ok(())
}
#[tokio::test]
async fn un_06_ac2_confirmed_still_watches_with_disabled_rule() -> TestResult {
    let w = World::new(false).await?;
    w.page_at(SENT + Duration::days(14)).await?;
    w.seed(SENT + Duration::days(16));
    assert_eq!(w.page_at(SENT + Duration::days(17)).await?.cards.len(), 1);
    assert_eq!(w.items().await?, 1);
    assert_eq!(w.state().await?.totals.unsubscribes_confirmed, 1);
    Ok(())
}
#[tokio::test]
async fn delivery_failed_state_write_raises_nothing() -> TestResult {
    let w = World::new(false).await?;
    w.seed(SENT + Duration::days(10));
    w.fakes
        .app_folder
        .fail_op(testkit::app_folder::FolderOp::Write, 1);
    assert!(w.page_at(SENT + Duration::days(11)).await.is_err());
    assert_eq!(w.items().await?, 0);
    assert_eq!(w.state().await?.pending_delivery_checks.len(), 1);
    w.page_at(SENT + Duration::days(11)).await?;
    assert_eq!(w.items().await?, 1);
    Ok(())
}

#[tokio::test]
async fn delivery_sent_and_batched_outcomes_create_one_check() -> TestResult {
    let w = World::new(true).await?;
    w.store()
        .update(&w.user, |s| {
            s.pending_delivery_checks.clear();
        })
        .await?;
    for (n, code) in [
        (1, ports::store::JobOutcomeCode::OneClickAccepted),
        (2, ports::store::JobOutcomeCode::Batched),
    ] {
        let id = domain::JobId(Uuid::from_u128(n));
        w.store()
            .update(&w.user, |s| {
                s.pending_unsubscribes.insert(
                    id.0,
                    domain::user_state::PendingUnsubscribe {
                        mailbox_id: w.mailbox,
                        sender_display: "Synthetic list".into(),
                        sender_key: "list@example.com".into(),
                        list_id: Some("weekly.example.com".into()),
                        rule_id: None,
                        created_at: SENT - Duration::minutes(1),
                    },
                );
            })
            .await?;
        w.fakes
            .store
            .jobs()
            .put(
                &ports::JobRecord {
                    job_id: id,
                    user_id: w.user,
                    mailbox_id: w.mailbox,
                    list_key_hash: ports::ListKeyHash([7; 32]),
                    method: domain::JobMethod::OneClick,
                    target: None,
                    sender_display: None,
                    due_at: SENT,
                    status: domain::JobStatus::Sent,
                    attempts: 1,
                    outcome: Some(ports::store::JobOutcome { code, at: SENT }),
                    expires_at: SENT + Duration::days(30),
                },
                Precondition::MustNotExist,
            )
            .await?;
    }
    w.seed(SENT + Duration::days(2));
    w.page_at(SENT + Duration::days(3)).await?;
    let s = w.state().await?;
    assert_eq!(s.pending_delivery_checks.len(), 1);
    assert!(s.pending_delivery_checks[0].mail_seen);
    assert_eq!(s.pending_delivery_checks[0].unsubscribed_at, SENT);
    assert_eq!(s.totals.senders_unsubscribed, 2);
    assert_eq!(
        s.history
            .iter()
            .filter(|e| e.action == HistoryAction::Unsubscribe)
            .count(),
        2
    );
    assert_eq!(w.items().await?, 0);
    w.page_at(SENT + Duration::days(14)).await?;
    assert_eq!(w.state().await?.totals.unsubscribes_confirmed, 0);
    Ok(())
}

#[tokio::test]
async fn un_06_ac1_failed_item_seal_retries_without_mail() -> TestResult {
    let w = World::new(true).await?;
    w.seed(SENT + Duration::days(10));
    w.fakes.keys.fail_field(aad_fields::NA_SENDER_DISPLAY, 1);
    assert!(w.page_at(SENT + Duration::days(11)).await.is_err());
    assert_eq!(w.items().await?, 0);
    assert_eq!(w.state().await?.pending_delivery_checks.len(), 1);
    // The rule already trashed the only late mail; retry cannot rediscover it.
    w.page_at(SENT + Duration::days(12)).await?;
    w.page_at(SENT + Duration::days(15)).await?;
    assert_eq!(w.items().await?, 1);
    assert_eq!(w.state().await?.pending_delivery_checks.len(), 0);
    assert_eq!(w.state().await?.totals.unsubscribes_confirmed, 0);
    Ok(())
}

#[tokio::test]
async fn delivery_late_fetched_grace_mail_blocks_confirmation() -> TestResult {
    let w = World::new(false).await?;
    w.seed(SENT + Duration::days(2));
    w.page_at(SENT + Duration::days(20)).await?;
    assert_eq!(w.state().await?.totals.unsubscribes_confirmed, 0);
    assert_eq!(w.items().await?, 0);
    Ok(())
}

#[tokio::test]
async fn un_06_ac1_failed_item_write_retries_after_lifetime() -> TestResult {
    let w = World::new(true).await?;
    w.seed(SENT + Duration::days(10));
    w.fakes.store.fail_next_needs_attention_put();
    assert!(w.page_at(SENT + Duration::days(11)).await.is_err());
    assert_eq!(w.items().await?, 0);
    let state = w.state().await?;
    assert_eq!(state.pending_delivery_checks.len(), 1);
    assert_eq!(
        state.pending_delivery_checks[0]
            .pending_ignored_display
            .as_deref(),
        Some("Synthetic list")
    );
    w.page_at(SENT + Duration::days(31)).await?;
    w.page_at(SENT + Duration::days(32)).await?;
    assert_eq!(w.items().await?, 1);
    assert_eq!(w.state().await?.pending_delivery_checks.len(), 0);
    assert_eq!(w.state().await?.totals.unsubscribes_confirmed, 0);
    Ok(())
}

#[tokio::test]
async fn un_06_ac1_failed_acknowledgement_does_not_duplicate_item() -> TestResult {
    let w = World::new(true).await?;
    let mail = [ListMail {
        sender_key: "list@example.com".into(),
        list_id: Some("weekly.example.com".into()),
        mailbox_id: w.mailbox,
        received_at: SENT + Duration::days(10),
        sender_display: "Synthetic list".into(),
    }];
    let ignored = w
        .store()
        .update(&w.user, |state| {
            apply_delivery_checks(
                state,
                &w.app.business_calendar,
                SENT + Duration::days(11),
                &DeliveryHookInput { list_mail: &mail },
            )
        })
        .await?;
    w.fakes
        .app_folder
        .fail_op(testkit::app_folder::FolderOp::Write, 1);
    assert!(
        api::services::delivery_check::publish(&w.app, &w.user, &ignored, 0)
            .await
            .is_err()
    );
    assert_eq!(w.items().await?, 1);
    assert_eq!(w.state().await?.pending_delivery_checks.len(), 1);
    // A crash or failed acknowledgement replays the same durable outbox item.
    w.page_at(SENT + Duration::days(12)).await?;
    assert_eq!(w.items().await?, 1);
    assert_eq!(w.state().await?.pending_delivery_checks.len(), 0);
    Ok(())
}

#[test]
fn delivery_national_holidays_loaded() -> TestResult {
    let cal = api::services::delivery_check::national_calendar();
    for token in [
        "2026-01-01",
        "2026-01-26",
        "2026-04-03",
        "2026-04-06",
        "2026-04-25",
        "2026-12-25",
        "2026-12-28",
        "2027-01-01",
        "2027-01-26",
        "2027-03-26",
        "2027-03-29",
        "2027-04-26",
        "2027-12-27",
        "2027-12-28",
    ] {
        assert!(!cal.is_business_day(time::Date::parse(
            token,
            &time::macros::format_description!("[year]-[month]-[day]")
        )?));
    }
    assert!(cal.is_business_day(time::macros::date!(2026 - 12 - 29)));
    Ok(())
}

#[test]
fn delivery_old_mail_before_send_ignored() -> TestResult {
    let mailbox = MailboxId(Uuid::nil());
    let mut s = UserState::default();
    s.pending_delivery_checks.push(check(mailbox));
    let mail = [ListMail {
        sender_key: "list@example.com".into(),
        list_id: Some("weekly.example.com".into()),
        mailbox_id: mailbox,
        received_at: SENT,
        sender_display: "Synthetic".into(),
    }];
    assert!(apply_delivery_checks(
        &mut s,
        &BusinessCalendar::new(BTreeSet::new()),
        SENT + Duration::days(14),
        &DeliveryHookInput { list_mail: &mail }
    )
    .is_empty());
    assert_eq!(s.totals.unsubscribes_confirmed, 1);
    assert!(!s.pending_delivery_checks[0].mail_seen);
    serde_json::to_vec(&s)?;
    Ok(())
}
#[test]
fn delivery_check_removed_after_lifetime() -> TestResult {
    let mut s = UserState::default();
    s.pending_delivery_checks
        .push(check(MailboxId(Uuid::nil())));
    s.pending_delivery_checks[0].mail_seen = true;
    let cal = BusinessCalendar::new(BTreeSet::new());
    let _ = apply_delivery_checks(
        &mut s,
        &cal,
        SENT + Duration::days(30),
        &DeliveryHookInput { list_mail: &[] },
    );
    assert_eq!(s.pending_delivery_checks.len(), 1);
    let _ = apply_delivery_checks(
        &mut s,
        &cal,
        SENT + Duration::days(30) + Duration::seconds(1),
        &DeliveryHookInput { list_mail: &[] },
    );
    assert_eq!(s.pending_delivery_checks.len(), 0);
    serde_json::to_vec(&s)?;
    Ok(())
}
#[test]
fn delivery_list_keys_require_both_equal() -> TestResult {
    let mut s = UserState::default();
    s.pending_delivery_checks
        .push(check(MailboxId(Uuid::nil())));
    for (sender, list_id) in [
        ("other@example.com", Some("weekly.example.com")),
        ("list@example.com", None),
        ("list@example.com", Some("other.example.com")),
    ] {
        let mail = [ListMail {
            sender_key: sender.into(),
            list_id: list_id.map(str::to_owned),
            mailbox_id: MailboxId(Uuid::nil()),
            received_at: SENT + Duration::days(10),
            sender_display: "Synthetic".into(),
        }];
        assert!(apply_delivery_checks(
            &mut s,
            &BusinessCalendar::new(BTreeSet::new()),
            SENT + Duration::days(11),
            &DeliveryHookInput { list_mail: &mail }
        )
        .is_empty());
        assert!(!s.pending_delivery_checks[0].mail_seen);
    }
    serde_json::to_vec(&s)?;
    Ok(())
}
