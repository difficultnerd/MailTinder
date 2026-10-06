//! T-803 step 11: the orphan sweep (S2 AU-06 AC1).

#![allow(clippy::unwrap_used, clippy::expect_used)]

use domain::{
    JobId, JobMethod, JobStatus, MailboxStatus, NeedsAttentionReason, Provider, ProviderSubjectId,
    UserId,
};
use ports::{
    Ciphertext, Clock, KeyService, ListKeyHash, MailboxRecord, NeedsAttentionId,
    NeedsAttentionRecord, Precondition, Rng, ServerStore, SessionHash, SessionRecord,
    SessionRecordId, SessionState, UserRecord,
};
use testkit::{fake_ports, Fakes};
use time::Duration;
use worker::sweeps::deleted_users::{sweep_deleted_users, DELETION_SWEEP_HOURS};

/// One user's job, Needs Attention item, mailbox and session. The user record
/// itself is left to the caller, so a "deleted" user is one without it.
async fn seed_records(fakes: &Fakes, user: UserId) -> Result<(), Box<dyn std::error::Error>> {
    let now = fakes.clock.now();
    let subject = ProviderSubjectId::new(format!("sub-{}", user.0))?;
    let mailbox_id = ports::mailbox_id_for(Provider::Gmail, &subject);
    let store = &fakes.store;
    store
        .mailboxes()
        .put(
            &MailboxRecord {
                mailbox_id,
                user_id: user,
                provider: Provider::Gmail,
                provider_subject_id: subject,
                email_address: Ciphertext(vec![1]),
                status: MailboxStatus::Connected,
                linked_at: now,
                is_primary: true,
                refresh_token: None,
            },
            Precondition::MustNotExist,
        )
        .await?;
    store
        .jobs()
        .put(
            &ports::JobRecord {
                job_id: JobId::new(fakes.rng.uuid_v4()),
                user_id: user,
                mailbox_id,
                list_key_hash: ListKeyHash([7u8; 32]),
                method: JobMethod::OneClick,
                target: Some(Ciphertext(vec![1, 2, 3])),
                due_at: now + Duration::minutes(5),
                status: JobStatus::Queued,
                attempts: 0,
                outcome: None,
                expires_at: now + Duration::hours(1),
            },
            Precondition::MustNotExist,
        )
        .await?;
    store
        .needs_attention()
        .put(
            &NeedsAttentionRecord {
                item_id: NeedsAttentionId(fakes.rng.uuid_v4()),
                user_id: user,
                mailbox_id,
                sender_display: Ciphertext(vec![9]),
                link: None,
                reason_code: NeedsAttentionReason::HttpsOnlyUnsubscribe,
                created_at: now,
                expires_at: now + Duration::days(30),
            },
            Precondition::MustNotExist,
        )
        .await?;
    let mut hash = [0u8; 32];
    hash[..16].copy_from_slice(user.0.as_bytes());
    store
        .sessions()
        .put(
            &SessionRecord {
                session_hash: SessionHash(hash),
                session_record_id: SessionRecordId(fakes.rng.uuid_v4()),
                state: SessionState::Authenticated,
                user_id: Some(user),
                csrf_token: "csrf".into(),
                created_at: now,
                last_seen_at: now,
                recent_auth_at: Some(now),
                expires_at: now + Duration::hours(12),
                pre_auth: None,
            },
            Precondition::MustNotExist,
        )
        .await?;
    Ok(())
}

async fn seed_user_record(fakes: &Fakes, user: UserId) -> Result<(), Box<dyn std::error::Error>> {
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
    Ok(())
}

/// A deletion whose step 8 failed leaves records and no user document. One
/// sweep under the virtual clock, inside the 24 hours, removes them all and
/// leaves a live user's records alone.
#[tokio::test]
async fn au_06_ac1_records_swept_within_24_hours() -> Result<(), Box<dyn std::error::Error>> {
    let (_ports, fakes) = fake_ports();
    let gone = UserId::new(fakes.rng.uuid_v4());
    let live = UserId::new(fakes.rng.uuid_v4());
    seed_user_record(&fakes, live).await?;
    seed_records(&fakes, gone).await?;
    seed_records(&fakes, live).await?;

    fakes
        .clock
        .advance(Duration::hours(DELETION_SWEEP_HOURS - 1));
    let deleted = sweep_deleted_users(fakes.store.as_ref(), 100).await?;
    assert_eq!(deleted, 4, "mailbox, job, item and session");

    let dump = serde_json::to_string(&fakes.store.export_json())?;
    assert!(
        !dump.contains(&gone.0.to_string()),
        "no record references the deleted user"
    );
    assert_eq!(fakes.store.mailboxes().by_user(&live).await?.len(), 1);
    assert_eq!(
        fakes
            .store
            .jobs()
            .by_mailbox(&ports::mailbox_id_for(
                Provider::Gmail,
                &ProviderSubjectId::new(format!("sub-{}", live.0))?,
            ))
            .await?
            .len(),
        1
    );
    assert_eq!(fakes.store.sessions().by_user(&live).await?.len(), 1);
    assert_eq!(
        sweep_deleted_users(fakes.store.as_ref(), 100).await?,
        0,
        "a second run finds nothing"
    );
    Ok(())
}
