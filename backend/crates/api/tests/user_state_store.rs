//! T-602b user state file integration tests (S2 SR-01 AC5, FD-03 AC3, S3
//! INV-1, INV-4).
//!
//! The service runs against the in-memory store and keys, the scripted
//! identity provider and the in-memory app folder.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::too_many_lines,
    clippy::similar_names,
    clippy::items_after_statements,
    clippy::needless_pass_by_value,
    clippy::missing_panics_doc
)]

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Barrier};

use api::error::ApiError;
use api::services::user_state_store::{UserStateStore, UPDATE_ATTEMPTS};
use api::state::AppState;
use api::{app_state, config::ApiConfig};
use domain::user_state::{
    HistoryAction, HistoryEntry, HistoryOutcome, MailboxPosition, StoredRule, UserState,
    USER_STATE_VERSION,
};
use domain::{
    EmailAddress, MailboxId, MailboxStatus, Provider, ProviderSubjectId, RuleId, RuleKind,
    RuleMatch, SenderKey, SenderStats, SortRule, UserId,
};
use obs::Sensitive;
use ports::store::aad_fields;
use ports::{
    Aad, AppFolderStore, Ciphertext, Clock, KeyService, MailboxCtx, MailboxRecord, Precondition,
    Rng, ServerStore, UserRecord,
};
use testkit::{fake_ports, Fakes};
use uuid::Uuid;

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

fn fixture() -> Result<(AppState, Fakes), Box<dyn std::error::Error>> {
    let (ports, fakes) = fake_ports();
    Ok((app_state(Arc::new(ports), Arc::new(config()?)), fakes))
}

async fn seed_user(fakes: &Fakes) -> Result<UserId, Box<dyn std::error::Error>> {
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
    Ok(user)
}

async fn seed_mailbox(
    fakes: &Fakes,
    user: UserId,
    sub: &str,
    email: &str,
    is_primary: bool,
    status: MailboxStatus,
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
                email_address: Ciphertext(sealed),
                status,
                linked_at: fakes.clock.now(),
                is_primary,
                refresh_token: None,
            },
            Precondition::MustNotExist,
        )
        .await?;
    Ok(mailbox_id)
}

async fn grant(
    state: &AppState,
    fakes: &Fakes,
    user: &UserId,
    mailbox: &MailboxId,
    token: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    state
        .tokens
        .store_refresh_token(state, user, mailbox, Sensitive::new(token.to_owned()))
        .await?;
    fakes
        .identity
        .script_refresh(token, Ok(format!("access-for-{token}")));
    Ok(())
}

struct World {
    app: Arc<AppState>,
    fakes: Fakes,
    user: UserId,
    mailbox: MailboxId,
    store: Arc<UserStateStore>,
}

async fn world() -> Result<World, Box<dyn std::error::Error>> {
    let (state, fakes) = fixture()?;
    let user = seed_user(&fakes).await?;
    let mailbox = seed_mailbox(
        &fakes,
        user,
        "sub-primary",
        "primary@example.com",
        true,
        MailboxStatus::Connected,
    )
    .await?;
    grant(&state, &fakes, &user, &mailbox, "refresh-primary").await?;
    let app = Arc::new(state);
    let store = Arc::new(UserStateStore::new(Arc::clone(&app)));
    Ok(World {
        app,
        fakes,
        user,
        mailbox,
        store,
    })
}

impl World {
    async fn ctx(&self) -> Result<MailboxCtx, Box<dyn std::error::Error>> {
        Ok(self
            .app
            .tokens
            .mailbox_ctx(&self.app, &self.user, &self.mailbox)
            .await?)
    }

    /// Every server store document as JSON text, for leak scans.
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

fn history(n: u128, now: time::OffsetDateTime) -> HistoryEntry {
    HistoryEntry {
        entry_id: Uuid::from_u128(n),
        at: now,
        mailbox_id: MailboxId(Uuid::from_u128(1)),
        sender_display: format!("Sender {n}"),
        action: HistoryAction::Unsubscribe,
        outcome: HistoryOutcome::Sent,
        rule_id: None,
    }
}

#[tokio::test]
async fn sr_01_ac5_rules_written_only_to_app_folder() -> Result<(), Box<dyn std::error::Error>> {
    let w = world().await?;
    let sender = "zebra-distinctive-sender@rules.example";
    let now = w.fakes.clock.now();
    w.store
        .update(&w.user, |s: &mut UserState| {
            s.rules.push(StoredRule {
                rule: SortRule {
                    rule_id: RuleId(Uuid::from_u128(42)),
                    kind: RuleKind::RejectList,
                    matcher: RuleMatch {
                        sender: SenderKey::from_address(sender),
                        list_id: None,
                        feedback_id: None,
                    },
                    category: None,
                    enabled: true,
                    created_at: now,
                    source_swipe: None,
                },
                times_applied: 0,
                yearly_rate: None,
            });
        })
        .await?;
    assert!(!w.store_text().contains(sender));
    let (bytes, _) = w
        .fakes
        .app_folder
        .read(&w.ctx().await?)
        .await?
        .ok_or("file written")?;
    assert!(!String::from_utf8_lossy(&bytes).contains(sender));
    let loaded = w.store.load(&w.user).await?;
    assert_eq!(loaded.state.rules.len(), 1);
    Ok(())
}

#[tokio::test]
async fn fd_03_ac3_positions_survive_new_session() -> Result<(), Box<dyn std::error::Error>> {
    let w = world().await?;
    let now = w.fakes.clock.now();
    let position = MailboxPosition {
        newest_seen: Some(now),
        new_done: true,
        boundary_ids: vec!["m1".into()],
        ..MailboxPosition::default()
    };
    let expected = position.clone();
    let key = w.mailbox.0;
    w.store
        .update(&w.user, move |s: &mut UserState| {
            s.positions.insert(key, position.clone());
        })
        .await?;
    // A new session builds a fresh store over the same folder.
    let again = UserStateStore::new(Arc::clone(&w.app));
    let loaded = again.load(&w.user).await?;
    assert_eq!(loaded.state.positions.get(&key), Some(&expected));
    Ok(())
}

#[tokio::test]
async fn inv_1_user_state_never_in_server_store() -> Result<(), Box<dyn std::error::Error>> {
    let w = world().await?;
    let now = w.fakes.clock.now();
    for n in 0..3u128 {
        let key = format!("quokka-sender-{n}@leak.example");
        let display = format!("Quokka Display {n}");
        w.store
            .update(&w.user, move |s: &mut UserState| {
                s.sender_stats.insert(
                    key.clone(),
                    SenderStats {
                        display: display.clone(),
                        ..SenderStats::default()
                    },
                );
                s.push_history(HistoryEntry {
                    sender_display: display.clone(),
                    ..history(n + 100, now)
                });
            })
            .await?;
    }
    let text = w.store_text();
    assert!(!text.contains("quokka"));
    assert!(!text.contains("Quokka"));
    assert!(!text.contains("sender_stats"));
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn inv_4_concurrent_updates_keep_both_history_entries(
) -> Result<(), Box<dyn std::error::Error>> {
    let w = world().await?;
    let now = w.fakes.clock.now();
    // Pre-write so both racers start from the same non-None ETag.
    w.store.update(&w.user, |_: &mut UserState| {}).await?;
    let barrier = Arc::new(Barrier::new(2));
    let mut handles = Vec::new();
    for n in [1u128, 2] {
        let store = Arc::clone(&w.store);
        let user = w.user;
        let barrier = Arc::clone(&barrier);
        handles.push(tokio::spawn(async move {
            let waited = AtomicBool::new(false);
            store
                .update(&user, |s: &mut UserState| {
                    // Both loads finish before either writes; only the first
                    // run of each closure waits, retries pass straight on.
                    if !waited.swap(true, Ordering::SeqCst) {
                        barrier.wait();
                    }
                    s.push_history(history(n, now));
                })
                .await
        }));
    }
    for h in handles {
        h.await??;
    }
    let loaded = w.store.load(&w.user).await?;
    let ids: Vec<u128> = loaded
        .state
        .history
        .iter()
        .map(|e| e.entry_id.as_u128())
        .collect();
    assert!(ids.contains(&1) && ids.contains(&2), "{ids:?}");
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn inv_4_update_gives_up_after_three_conflicts() -> Result<(), Box<dyn std::error::Error>> {
    let w = world().await?;
    w.store.update(&w.user, |_: &mut UserState| {}).await?;
    let ctx = w.ctx().await?;
    let runs = AtomicU32::new(0);
    let folder = Arc::clone(&w.fakes.app_folder);
    let handle = tokio::runtime::Handle::current();
    let result = w
        .store
        .update(&w.user, |_: &mut UserState| {
            runs.fetch_add(1, Ordering::SeqCst);
            // Test double: a competing writer bumps the ETag between this
            // closure's load and its write, so every write conflicts.
            tokio::task::block_in_place(|| {
                handle.block_on(async {
                    if let Ok(Some((bytes, etag))) = folder.read(&ctx).await {
                        let _ = folder.write(&ctx, &bytes, Some(&etag)).await;
                    }
                });
            });
        })
        .await;
    assert!(matches!(
        result,
        Err(ApiError::ProviderUnavailable {
            retry_after_s: Some(1),
            ..
        })
    ));
    assert_eq!(runs.load(Ordering::SeqCst), UPDATE_ATTEMPTS);
    Ok(())
}

#[tokio::test]
async fn user_state_missing_file_starts_empty() -> Result<(), Box<dyn std::error::Error>> {
    let w = world().await?;
    let loaded = w.store.load(&w.user).await?;
    assert_eq!(loaded.state.version, USER_STATE_VERSION);
    assert_eq!(
        loaded.state,
        UserState {
            version: USER_STATE_VERSION,
            ..UserState::default()
        }
    );
    assert!(loaded.etag.is_none());
    Ok(())
}

#[tokio::test]
async fn user_state_undecryptable_file_not_overwritten() -> Result<(), Box<dyn std::error::Error>> {
    let w = world().await?;
    let ctx = w.ctx().await?;
    let corrupt = b"not a sealed state file".to_vec();
    w.fakes.app_folder.write(&ctx, &corrupt, None).await?;
    assert!(matches!(
        w.store.load(&w.user).await,
        Err(ApiError::Internal)
    ));
    assert!(matches!(
        w.store.update(&w.user, |_: &mut UserState| {}).await,
        Err(ApiError::Internal)
    ));
    let (bytes, _) = w.fakes.app_folder.read(&ctx).await?.ok_or("file kept")?;
    assert_eq!(bytes, corrupt);
    Ok(())
}
