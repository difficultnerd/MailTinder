//! T-609: feed-time rule actions and terminal unsubscribe collection.
#![allow(clippy::too_many_lines)]

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use api::routes::feed::{FeedPage, FeedRequest};
use api::services::feed::next_page;
use api::services::user_state_store::UserStateStore;
use api::session::extract::AuthedSession;
use api::state::AppState;
use api::{app_state, config::ApiConfig};
use async_trait::async_trait;
use domain::user_state::{
    Category, HistoryAction, HistoryOutcome, PendingUnsubscribe, StoredRule, UserState,
};
use domain::{
    CategoryId, EmailAddress, HeaderFacts, JobId, MailboxId, MailboxStatus, MessageId, Provider,
    ProviderSubjectId, RuleId, SenderKey, UserId,
};
use domain::{JobMethod, JobStatus, SortRule};
use obs::Sensitive;
use ports::store::aad_fields;
use ports::{
    Aad, AppFolderError, AppFolderStore, Ciphertext, Clock, ETag, JobRecord, KeyService,
    ListKeyHash, MailError, MailProvider, MailboxCtx, MailboxRecord, Precondition, Rng,
    ServerStore, SessionHash, SessionRecordId, UserRecord,
};
use proptest::prelude::*;
use testkit::app_folder::FolderOp;
use testkit::{fake_ports, Fakes, InMemoryAppFolder, MailOp, SeedMessage};
use time::Duration;
use tokio::sync::Barrier;
use uuid::Uuid;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

/// Hold two writes after their reads, so one uses a stale Drive `ETag`.
struct RacingFolder {
    inner: Arc<InMemoryAppFolder>,
    remaining: AtomicUsize,
    gate: Barrier,
    conflicts: AtomicUsize,
}

#[async_trait]
impl AppFolderStore for RacingFolder {
    async fn read(&self, ctx: &MailboxCtx) -> Result<Option<(Vec<u8>, ETag)>, MailError> {
        self.inner.read(ctx).await
    }

    async fn write(
        &self,
        ctx: &MailboxCtx,
        bytes: &[u8],
        etag: Option<&ETag>,
    ) -> Result<ETag, AppFolderError> {
        let mut remaining = self.remaining.load(Ordering::SeqCst);
        while remaining > 0 {
            match self.remaining.compare_exchange(
                remaining,
                remaining - 1,
                Ordering::SeqCst,
                Ordering::SeqCst,
            ) {
                Ok(_) => {
                    self.gate.wait().await;
                    break;
                }
                Err(actual) => remaining = actual,
            }
        }
        let result = self.inner.write(ctx, bytes, etag).await;
        if matches!(result, Err(AppFolderError::Conflict)) {
            self.conflicts.fetch_add(1, Ordering::SeqCst);
        }
        result
    }

    async fn delete(&self, ctx: &MailboxCtx) -> Result<(), MailError> {
        self.inner.delete(ctx).await
    }
}

struct World {
    app: AppState,
    fakes: Fakes,
    user: UserId,
    mailbox: MailboxId,
    racing: Arc<RacingFolder>,
}

impl World {
    async fn new() -> TestResult<Self> {
        let (mut ports, fakes) = fake_ports();
        let racing = Arc::new(RacingFolder {
            inner: Arc::clone(&fakes.app_folder),
            remaining: AtomicUsize::new(0),
            gate: Barrier::new(2),
            conflicts: AtomicUsize::new(0),
        });
        ports.app_folder = racing.clone();
        let config = ApiConfig::new(
            "https://mailtinder.test".into(),
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
        let subject = ProviderSubjectId::new("rules-test-subject")?;
        let mailbox = ports::mailbox_id_for(Provider::Gmail, &subject);
        let address = EmailAddress::parse("rules@example.com")?;
        let aad = Aad {
            user,
            scope: mailbox.0.to_string(),
            field: aad_fields::MAILBOX_EMAIL,
        };
        let email = fakes
            .keys
            .seal(&user, &wrapped, &aad, address.as_str().as_bytes())
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
                    linked_at: fakes.clock.now(),
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
                Sensitive::new("rules-refresh".into()),
            )
            .await?;
        fakes
            .identity
            .script_refresh("rules-refresh", Ok("rules-access".into()));
        Ok(Self {
            app,
            fakes,
            user,
            mailbox,
            racing,
        })
    }

    fn session(&self) -> AuthedSession {
        AuthedSession {
            user: self.user,
            session_record_id: SessionRecordId(Uuid::from_u128(99)),
            is_admin: false,
            recent_auth_at: None,
            session_hash: SessionHash([9; 32]),
        }
    }

    fn store(&self) -> UserStateStore {
        UserStateStore::new(Arc::new(self.app.clone()))
    }

    async fn state(&self) -> TestResult<UserState> {
        Ok(self.store().load(&self.user).await?.state)
    }

    async fn page(&self) -> TestResult<FeedPage> {
        Ok(next_page(
            &self.app,
            &self.session(),
            FeedRequest {
                cursor: None,
                limit: 50,
                refresh: true,
            },
        )
        .await?)
    }

    fn seed(&self, sender: &str, facts: HeaderFacts) -> MessageId {
        self.fakes.mailbox.seed(
            &self.mailbox,
            SeedMessage {
                from_display: "Synthetic sender".into(),
                from_address: sender.into(),
                subject: "Synthetic message".into(),
                raw_headers: vec![],
                facts,
                preview_text: "Synthetic preview".into(),
                internal_date: self.fakes.clock.now() - Duration::minutes(1),
                labels: vec!["INBOX".into()],
            },
        )
    }

    fn ctx(&self) -> MailboxCtx {
        MailboxCtx {
            mailbox: self.mailbox,
            access_token: Sensitive::new("rules-access".into()),
        }
    }

    async fn add_rule(&self, rule: SortRule) -> TestResult {
        self.store()
            .update(&self.user, |state| {
                state.rules.push(StoredRule {
                    rule: rule.clone(),
                    times_applied: 0,
                    yearly_rate: Some(12),
                });
            })
            .await?;
        Ok(())
    }

    async fn reject_rule(&self, facts: HeaderFacts) -> TestResult<SortRule> {
        let id = self.seed("list@example.com", facts);
        let meta = self.fakes.mailbox.get_meta(&self.ctx(), &id).await?;
        let rule = SortRule::reject_list_for(
            &meta,
            RuleId::new(self.fakes.rng.uuid_v4()),
            self.fakes.clock.now(),
            None,
        )
        .ok_or("authenticated rule fixture")?;
        self.add_rule(rule.clone()).await?;
        Ok(rule)
    }

    async fn filing_rule(&self, sender: &str) -> TestResult<SortRule> {
        let category = CategoryId::new(self.fakes.rng.uuid_v4());
        let rule = SortRule::file_for(
            &SenderKey::from_address(sender),
            category,
            RuleId::new(self.fakes.rng.uuid_v4()),
            self.fakes.clock.now(),
        );
        self.store()
            .update(&self.user, |state| {
                state.categories.push(Category {
                    category_id: category,
                    name: "Synthetic category".into(),
                    labels: BTreeMap::new(),
                    created_at: self.fakes.clock.now(),
                });
            })
            .await?;
        self.add_rule(rule.clone()).await?;
        Ok(rule)
    }

    fn labels(&self, id: &MessageId) -> TestResult<domain::LabelSet> {
        self.fakes
            .mailbox
            .labels_of(&self.mailbox, id)
            .ok_or_else(|| "rule permanently deleted a message".into())
    }

    async fn terminal_job(&self, status: JobStatus, n: u128) -> TestResult<JobId> {
        let id = JobId(Uuid::from_u128(n));
        let now = self.fakes.clock.now();
        self.store()
            .update(&self.user, |state| {
                state.pending_unsubscribes.insert(
                    id.0,
                    PendingUnsubscribe {
                        mailbox_id: self.mailbox,
                        sender_display: "Synthetic list".into(),
                        sender_key: format!("list{n}@example.com"),
                        list_id: Some(format!("list{n}.example.com")),
                        rule_id: None,
                        created_at: now - Duration::minutes(1),
                    },
                );
            })
            .await?;
        self.fakes
            .store
            .jobs()
            .put(
                &JobRecord {
                    job_id: id,
                    user_id: self.user,
                    mailbox_id: self.mailbox,
                    list_key_hash: ListKeyHash([7; 32]),
                    method: JobMethod::OneClick,
                    target: None,
                    sender_display: None,
                    due_at: now - Duration::seconds(1),
                    status,
                    attempts: 1,
                    outcome: Some(ports::store::JobOutcome {
                        code: match status {
                            JobStatus::Sent => ports::store::JobOutcomeCode::OneClickAccepted,
                            JobStatus::NeedsAttention => ports::store::JobOutcomeCode::HttpRejected,
                            JobStatus::Failed => ports::store::JobOutcomeCode::TokenInvalid,
                            JobStatus::Expired => ports::store::JobOutcomeCode::Expired,
                            JobStatus::Cancelled => ports::store::JobOutcomeCode::Cancelled,
                            JobStatus::Queued | JobStatus::Running => {
                                return Err("not terminal".into())
                            }
                        },
                        at: now,
                    }),
                    expires_at: now + Duration::days(1),
                },
                Precondition::MustNotExist,
            )
            .await?;
        Ok(id)
    }
}

fn list_facts(list_id: Option<&str>) -> HeaderFacts {
    HeaderFacts {
        from_authenticated: true,
        list_unsubscribe_present: true,
        list_id: list_id.map(str::to_owned),
        ..HeaderFacts::default()
    }
}

async fn assert_action(
    world: &World,
    rule: &SortRule,
    count: u64,
    action: HistoryAction,
) -> TestResult {
    let state = world.state().await?;
    assert_eq!(
        state
            .rule(&rule.rule_id)
            .ok_or("missing rule")?
            .times_applied,
        count
    );
    let history: Vec<_> = state
        .history
        .iter()
        .filter(|entry| entry.rule_id == Some(rule.rule_id))
        .collect();
    assert_eq!(u64::try_from(history.len())?, count);
    assert!(history.iter().all(|entry| entry.action == action
        && entry.outcome == HistoryOutcome::Done
        && entry.mailbox_id == world.mailbox));
    let unique: BTreeSet<_> = history.iter().map(|entry| entry.entry_id).collect();
    assert_eq!(unique.len(), history.len());
    Ok(())
}

#[tokio::test]
async fn sr_01_ac2_matching_mail_trashed_and_hidden() -> TestResult {
    let world = World::new().await?;
    let rule = world
        .reject_rule(list_facts(Some("weekly.example.com")))
        .await?;
    let future = world.seed("list@example.com", list_facts(Some("weekly.example.com")));
    let unrelated = world.seed("other@example.com", list_facts(Some("weekly.example.com")));
    let page = world.page().await?;
    assert_eq!(page.cards.len(), 1);
    assert!(world.labels(&future)?.contains("TRASH"));
    assert!(!world.labels(&future)?.contains("INBOX"));
    assert!(world.labels(&unrelated)?.contains("INBOX"));
    assert_eq!(world.fakes.mailbox.calls(MailOp::Trash), 2);
    assert_action(&world, &rule, 2, HistoryAction::TrashedByRule).await
}

#[tokio::test]
async fn sr_01_ac3_receipt_without_header_survives() -> TestResult {
    let world = World::new().await?;
    let rule = world.reject_rule(list_facts(None)).await?;
    let receipt = world.seed("list@example.com", HeaderFacts::default());
    assert_eq!(world.page().await?.cards.len(), 1);
    assert!(world.labels(&receipt)?.contains("INBOX"));
    assert!(!world.labels(&receipt)?.contains("TRASH"));
    assert_action(&world, &rule, 1, HistoryAction::TrashedByRule).await
}

#[tokio::test]
async fn sr_01_ac3_other_list_id_not_matched() -> TestResult {
    let world = World::new().await?;
    let rule = world
        .reject_rule(list_facts(Some("weekly.example.com")))
        .await?;
    let other = world.seed("list@example.com", list_facts(Some("receipts.example.com")));
    assert_eq!(world.page().await?.cards.len(), 1);
    assert!(world.labels(&other)?.contains("INBOX"));
    assert_action(&world, &rule, 1, HistoryAction::TrashedByRule).await
}

#[tokio::test]
async fn sr_01_ac4_disabled_rule_acts_on_nothing() -> TestResult {
    let world = World::new().await?;
    let rule = world.reject_rule(list_facts(None)).await?;
    world
        .store()
        .update(&world.user, |state| {
            for stored in &mut state.rules {
                stored.rule.enabled = false;
            }
        })
        .await?;
    let future = world.seed("list@example.com", list_facts(None));
    assert_eq!(world.page().await?.cards.len(), 2);
    assert!(world.labels(&future)?.contains("INBOX"));
    assert_eq!(world.fakes.mailbox.calls(MailOp::Trash), 0);
    assert_action(&world, &rule, 0, HistoryAction::TrashedByRule).await
}

#[tokio::test]
async fn pb_01_ac2_block_rule_trashes_future_mail() -> TestResult {
    let world = World::new().await?;
    let rule = SortRule::block_person_for(
        &SenderKey::from_address("person@example.com"),
        RuleId::new(world.fakes.rng.uuid_v4()),
        world.fakes.clock.now(),
        None,
    );
    world.add_rule(rule.clone()).await?;
    let id = world.seed("person@example.com", HeaderFacts::default());
    assert!(world.page().await?.cards.is_empty());
    assert!(world.labels(&id)?.contains("TRASH"));
    assert_action(&world, &rule, 1, HistoryAction::TrashedByRule).await
}

#[tokio::test]
async fn fl_04_ac2_filing_rule_applied_at_feed() -> TestResult {
    let world = World::new().await?;
    let rule = world.filing_rule("person@example.com").await?;
    let id = world.seed("person@example.com", HeaderFacts::default());
    assert!(world.page().await?.cards.is_empty());
    let state = world.state().await?;
    let label = state
        .category(&rule.category.ok_or("filing category")?)
        .ok_or("saved category")?
        .labels
        .get(&world.mailbox.0)
        .ok_or("provider label saved")?;
    let labels = world.labels(&id)?;
    assert!(labels.contains(label));
    assert!(!labels.contains("INBOX"));
    assert!(!labels.contains("TRASH"));
    assert_eq!(world.fakes.mailbox.calls(MailOp::SetLabels), 1);
    assert_action(&world, &rule, 1, HistoryAction::FiledByRule).await
}

#[tokio::test]
async fn un_01_ac3_outcomes_appended_once() -> TestResult {
    let world = World::new().await?;
    let cases = [
        (JobStatus::Sent, HistoryOutcome::Sent),
        (JobStatus::NeedsAttention, HistoryOutcome::NeedsAttention),
        (JobStatus::Failed, HistoryOutcome::Failed),
        (JobStatus::Expired, HistoryOutcome::Expired),
        (JobStatus::Cancelled, HistoryOutcome::Cancelled),
    ];
    let mut jobs = Vec::new();
    for (index, (status, _)) in cases.iter().enumerate() {
        jobs.push(
            world
                .terminal_job(*status, 100 + u128::try_from(index)?)
                .await?,
        );
    }
    world.page().await?;
    world.page().await?;
    let state = world.state().await?;
    assert_eq!(state.history.len(), cases.len());
    for ((_, outcome), id) in cases.iter().zip(&jobs) {
        let entry = state
            .history
            .iter()
            .find(|entry| entry.entry_id == id.0)
            .ok_or("job history")?;
        assert_eq!(entry.action, HistoryAction::Unsubscribe);
        assert_eq!(entry.outcome, *outcome);
        assert_eq!(entry.mailbox_id, world.mailbox);
        assert_eq!(entry.sender_display, "Synthetic list");
        assert!(world.fakes.store.jobs().get(id).await?.is_none());
    }
    assert!(state.pending_unsubscribes.is_empty());
    assert_eq!(state.pending_delivery_checks.len(), 1);
    let check = state
        .pending_delivery_checks
        .first()
        .ok_or("sent delivery check")?;
    assert_eq!(check.sender_key, "list100@example.com");
    assert_eq!(check.list_id.as_deref(), Some("list100.example.com"));
    assert_eq!(check.mailbox_id, world.mailbox);
    assert_eq!(state.totals.senders_unsubscribed, 1);
    Ok(())
}

#[tokio::test]
async fn un_01_ac3_racing_feed_loads_append_once() -> TestResult {
    let world = World::new().await?;
    let id = world.terminal_job(JobStatus::Sent, 200).await?;
    world.racing.remaining.store(2, Ordering::SeqCst);
    let (first, second) = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        tokio::join!(world.page(), world.page())
    })
    .await?;
    first?;
    second?;
    assert!(
        world.racing.conflicts.load(Ordering::SeqCst) >= 1,
        "test must force an actual stale ETag write"
    );
    let state = world.state().await?;
    assert_eq!(
        state
            .history
            .iter()
            .filter(|entry| entry.entry_id == id.0)
            .count(),
        1
    );
    assert_eq!(state.totals.senders_unsubscribed, 1);
    assert_eq!(state.pending_delivery_checks.len(), 1);
    assert!(state.pending_unsubscribes.is_empty());
    assert!(world.fakes.store.jobs().get(&id).await?.is_none());
    Ok(())
}

#[tokio::test]
async fn job_1_collected_job_deleted_after_history() -> TestResult {
    let world = World::new().await?;
    let id = world.terminal_job(JobStatus::Sent, 300).await?;
    world.fakes.app_folder.fail_op(FolderOp::Write, 1);
    assert!(world.page().await.is_err());
    assert!(
        world.fakes.store.jobs().get(&id).await?.is_some(),
        "failed history write must retain the job"
    );
    let state = world.state().await?;
    assert_eq!(state.history.len(), 0);
    assert!(state.pending_unsubscribes.contains_key(&id.0));
    assert_eq!(state.totals.senders_unsubscribed, 0);
    world.page().await?;
    let state = world.state().await?;
    assert_eq!(
        state
            .history
            .iter()
            .filter(|entry| entry.entry_id == id.0)
            .count(),
        1
    );
    assert!(world.fakes.store.jobs().get(&id).await?.is_none());
    assert!(!state.pending_unsubscribes.contains_key(&id.0));
    Ok(())
}

#[tokio::test]
async fn un_01_ac3_missing_pending_uses_unknown_sender() -> TestResult {
    let world = World::new().await?;
    let id = world.terminal_job(JobStatus::Sent, 401).await?;
    world
        .store()
        .update(&world.user, |state| {
            state.pending_unsubscribes.clear();
        })
        .await?;
    world.page().await?;
    let state = world.state().await?;
    let entry = state
        .history
        .iter()
        .find(|entry| entry.entry_id == id.0)
        .ok_or("history entry")?;
    assert_eq!(entry.sender_display, "Unknown sender");
    assert_eq!(entry.rule_id, None);
    assert_eq!(state.totals.senders_unsubscribed, 1);
    assert_eq!(state.pending_delivery_checks.len(), 0);
    Ok(())
}

#[tokio::test]
async fn sr_01_ac2_rule_action_cap_and_deterministic_ids() -> TestResult {
    use api::services::rule_actions::{apply_rules, MAX_RULE_ACTIONS_PER_LOAD};
    let world = World::new().await?;
    let rule = SortRule::block_person_for(
        &SenderKey::from_address("bounded@example.com"),
        RuleId::new(world.fakes.rng.uuid_v4()),
        world.fakes.clock.now(),
        None,
    );
    world.add_rule(rule).await?;
    let mut metas = Vec::new();
    for _ in 0..=MAX_RULE_ACTIONS_PER_LOAD {
        let id = world.seed("bounded@example.com", HeaderFacts::default());
        metas.push(world.fakes.mailbox.get_meta(&world.ctx(), &id).await?);
    }
    let state = world.state().await?;
    let first = apply_rules(&world.app, &world.session(), &state, &metas).await?;
    let second = apply_rules(&world.app, &world.session(), &state, &metas).await?;
    assert_eq!(first.applied, MAX_RULE_ACTIONS_PER_LOAD);
    assert_eq!(first.acted_on.len(), metas.len());
    assert_eq!(
        first.entries.iter().map(|e| e.entry_id).collect::<Vec<_>>(),
        second
            .entries
            .iter()
            .map(|e| e.entry_id)
            .collect::<Vec<_>>()
    );
    let deferred = metas.last().ok_or("deferred message")?;
    assert!(world.labels(&deferred.id)?.contains("INBOX"));
    Ok(())
}

#[test]
#[allow(clippy::unnecessary_wraps)]
fn un_01_ac3_unfinished_jobs_have_no_history_outcome() -> TestResult {
    use api::services::history_catch_up::history_outcome;
    assert_eq!(history_outcome(JobStatus::Queued), None);
    assert_eq!(history_outcome(JobStatus::Running), None);
    Ok(())
}

#[tokio::test]
async fn sr_01_ac2_provider_failure_hidden_without_history_then_retry_counted_once() -> TestResult {
    let world = World::new().await?;
    let rule = world.reject_rule(list_facts(None)).await?;
    world
        .fakes
        .mailbox
        .fail_next(MailOp::Trash, MailError::Transient);
    assert!(world.page().await?.cards.is_empty());
    assert_action(&world, &rule, 0, HistoryAction::TrashedByRule).await?;
    assert!(world.page().await?.cards.is_empty());
    assert_action(&world, &rule, 1, HistoryAction::TrashedByRule).await?;
    world.page().await?;
    assert_action(&world, &rule, 1, HistoryAction::TrashedByRule).await
}

#[tokio::test]
async fn fl_04_ac2_provider_failure_does_not_count_filing() -> TestResult {
    let world = World::new().await?;
    let rule = world.filing_rule("person@example.com").await?;
    let id = world.seed("person@example.com", HeaderFacts::default());
    world
        .fakes
        .mailbox
        .fail_next(MailOp::SetLabels, MailError::Transient);
    assert!(world.page().await?.cards.is_empty());
    assert!(world.labels(&id)?.contains("INBOX"));
    assert_action(&world, &rule, 0, HistoryAction::FiledByRule).await?;
    world.page().await?;
    world.page().await?;
    assert_action(&world, &rule, 1, HistoryAction::FiledByRule).await
}

/// Generated independent senders, rule kinds, enabled flags and inbox sizes.
async fn generated_rule_actions(cases: &[(u8, bool, u8)]) -> TestResult {
    let world = World::new().await?;
    let mut expected = Vec::new();
    let mut messages = Vec::new();
    for (index, (kind, enabled, count)) in cases.iter().enumerate() {
        let sender = format!("generated{index}@example.com");
        let mut rule = match kind {
            0 => SortRule::block_person_for(
                &SenderKey::from_address(&sender),
                RuleId::new(world.fakes.rng.uuid_v4()),
                world.fakes.clock.now(),
                None,
            ),
            1 => world.filing_rule(&sender).await?,
            _ => {
                let id = world.seed(&sender, list_facts(Some("generated.example.com")));
                messages.push((id.clone(), *enabled, false));
                let meta = world.fakes.mailbox.get_meta(&world.ctx(), &id).await?;
                SortRule::reject_list_for(
                    &meta,
                    RuleId::new(world.fakes.rng.uuid_v4()),
                    world.fakes.clock.now(),
                    None,
                )
                .ok_or("generated reject rule")?
            }
        };
        rule.enabled = *enabled;
        if *kind == 1 {
            world
                .store()
                .update(&world.user, |state| {
                    for stored in &mut state.rules {
                        if stored.rule.rule_id == rule.rule_id {
                            stored.rule.enabled = *enabled;
                        }
                    }
                })
                .await?;
        } else {
            world.add_rule(rule.clone()).await?;
        }
        for _ in 0..*count {
            let id = world.seed(&sender, list_facts(Some("generated.example.com")));
            messages.push((id, *enabled, *kind == 1));
        }
        let applied = if *enabled {
            u64::from(*count) + u64::from(*kind == 2)
        } else {
            0
        };
        expected.push((
            rule,
            applied,
            if *kind == 1 {
                HistoryAction::FiledByRule
            } else {
                HistoryAction::TrashedByRule
            },
        ));
    }
    world.page().await?;
    let state = world.state().await?;
    let total = expected.iter().map(|(_, count, _)| count).sum::<u64>();
    assert_eq!(u64::try_from(state.history.len())?, total);
    assert_eq!(state.totals.cleared, total);
    for (rule, count, action) in &expected {
        assert_action(&world, rule, *count, *action).await?;
    }
    for (id, enabled, filed) in &messages {
        let labels = world.labels(id)?;
        assert_eq!(labels.contains("INBOX"), !enabled);
        assert_eq!(labels.contains("TRASH"), *enabled && !filed);
    }
    world.page().await?;
    for (rule, count, action) in &expected {
        assert_action(&world, rule, *count, *action).await?;
    }
    assert_eq!(world.state().await?.totals.cleared, total);
    Ok(())
}

#[test]
fn inv_4_every_rule_action_has_history() -> TestResult {
    let runtime = tokio::runtime::Runtime::new()?;
    let mut runner = proptest::test_runner::TestRunner::new(ProptestConfig {
        cases: 32,
        failure_persistence: None,
        ..ProptestConfig::default()
    });
    runner.run(
        &proptest::collection::vec((0_u8..3, any::<bool>(), 1_u8..4), 1..6),
        |cases| {
            runtime
                .block_on(generated_rule_actions(&cases))
                .map_err(|error| proptest::test_runner::TestCaseError::fail(error.to_string()))
        },
    )?;
    Ok(())
}

#[tokio::test]
async fn inv_5_rule_actions_never_delete() -> TestResult {
    generated_rule_actions(&[(0, true, 2), (1, true, 2), (2, true, 2), (0, false, 1)]).await
}
