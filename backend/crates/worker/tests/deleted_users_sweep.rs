//! T-803 step 11: the orphan sweep (S2 AU-06 AC1).

#![allow(clippy::unwrap_used, clippy::expect_used)]

use domain::{
    JobId, JobMethod, JobStatus, MailboxStatus, NeedsAttentionReason, Provider, ProviderSubjectId,
    UserId,
};
use obs::{Pseudonymiser, Sensitive};
use ports::{
    Ciphertext, Clock, KeyService, ListKeyHash, MailboxRecord, NeedsAttentionId,
    NeedsAttentionRecord, PageRequest, Precondition, Rng, ServerStore, SessionHash, SessionRecord,
    SessionRecordId, SessionState, UserRecord,
};
use testkit::{fake_ports, Fakes};
use time::{Duration, OffsetDateTime};
use worker::sweeps::deleted_users::{sweep_deleted_users, DELETION_SWEEP_HOURS};
use worker::sweeps::run_sweeps;

/// The sweep needs the same key the API derives pseudonyms with (T-803 F3).
fn pseudonymiser() -> Pseudonymiser {
    Pseudonymiser::new(Sensitive::new(vec![9u8; 32]))
}

/// One user's job, Needs Attention item, mailbox and session, all expiring at
/// `expires_at`. The user record itself is left to the caller, so a "deleted"
/// user is one without it.
async fn seed_records(
    fakes: &Fakes,
    user: UserId,
    expires_at: OffsetDateTime,
) -> Result<(), Box<dyn std::error::Error>> {
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
                sender_display: None,
                due_at: now + Duration::minutes(5),
                status: JobStatus::Queued,
                attempts: 0,
                outcome: None,
                expires_at,
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
                expires_at,
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
                expires_at,
                pre_auth: None,
            },
            Precondition::MustNotExist,
        )
        .await?;
    Ok(())
}

/// A mailbox and nothing else: a deletion whose mailbox delete failed and that
/// left no job, item or session behind (T-803 F3).
async fn seed_mailbox_only(fakes: &Fakes, user: UserId) -> Result<(), Box<dyn std::error::Error>> {
    let now = fakes.clock.now();
    let subject = ProviderSubjectId::new(format!("sub-{}", user.0))?;
    fakes
        .store
        .mailboxes()
        .put(
            &MailboxRecord {
                mailbox_id: ports::mailbox_id_for(Provider::Gmail, &subject),
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

/// Every record of the user is gone from the store dump.
fn assert_no_record_for(fakes: &Fakes, user: UserId) -> Result<(), Box<dyn std::error::Error>> {
    let dump = serde_json::to_string(&fakes.store.export_json())?;
    assert!(
        !dump.contains(&user.0.to_string()),
        "no record references the deleted user"
    );
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
    let expiry = fakes.clock.now() + Duration::hours(1);
    seed_user_record(&fakes, live).await?;
    seed_records(&fakes, gone, expiry).await?;
    seed_records(&fakes, live, expiry).await?;

    fakes
        .clock
        .advance(Duration::hours(DELETION_SWEEP_HOURS - 1));
    let deleted = sweep_deleted_users(fakes.store.as_ref(), 100, &pseudonymiser()).await?;
    assert_eq!(deleted, 4, "mailbox, job, item and session");

    assert_no_record_for(&fakes, gone)?;
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
        sweep_deleted_users(fakes.store.as_ref(), 100, &pseudonymiser()).await?,
        0,
        "a second run finds nothing"
    );
    Ok(())
}

/// F1: the sweep is reachable through the worker's API-INT-2 entry point
/// (`run_sweeps`). This test drives the entry point directly: it proves the
/// entry point completes a failed deletion, not that a production caller
/// exists. T-706 mounts the route that calls it, and its test
/// `api_int_2_calls_the_deleted_user_sweep` is the hard gate for the
/// "within 24 hours" clause of AU-06 AC1.
#[tokio::test]
async fn au_06_ac1_sweep_entry_point_completes_a_failed_deletion(
) -> Result<(), Box<dyn std::error::Error>> {
    let (_ports, fakes) = fake_ports();
    let gone = UserId::new(fakes.rng.uuid_v4());
    seed_records(&fakes, gone, fakes.clock.now() + Duration::hours(1)).await?;

    let counts = run_sweeps(fakes.store.as_ref(), &pseudonymiser()).await?;
    assert_eq!(counts.deleted_records, 4);
    assert_no_record_for(&fakes, gone)?;
    Ok(())
}

/// F2: the orphan is not the oldest record. Live users' records fill the
/// first `limit` slots of every scan, so a single window would never reach it.
#[tokio::test]
async fn au_06_ac1_orphan_swept_past_a_full_window_of_live_records(
) -> Result<(), Box<dyn std::error::Error>> {
    let (_ports, fakes) = fake_ports();
    let now = fakes.clock.now();
    for n in 0..12u8 {
        let live = UserId::new(fakes.rng.uuid_v4());
        seed_user_record(&fakes, live).await?;
        // Older expiries than the orphan's, so they come first in every scan.
        seed_records(&fakes, live, now - Duration::days(100 - i64::from(n))).await?;
    }
    let gone = UserId::new(fakes.rng.uuid_v4());
    seed_records(&fakes, gone, now + Duration::hours(1)).await?;

    // Two records a page: the orphan sits past the twelfth live record.
    let deleted = sweep_deleted_users(fakes.store.as_ref(), 2, &pseudonymiser()).await?;
    assert_eq!(deleted, 4, "the orphan's mailbox, job, item and session");
    assert_no_record_for(&fakes, gone)?;
    Ok(())
}

/// F3: a user whose only leftover is a mailbox is still found.
#[tokio::test]
async fn au_06_ac1_mailbox_only_orphan_swept() -> Result<(), Box<dyn std::error::Error>> {
    let (_ports, fakes) = fake_ports();
    let gone = UserId::new(fakes.rng.uuid_v4());
    seed_mailbox_only(&fakes, gone).await?;

    let deleted = sweep_deleted_users(fakes.store.as_ref(), 100, &pseudonymiser()).await?;
    assert_eq!(deleted, 1, "the mailbox");
    assert_no_record_for(&fakes, gone)?;
    Ok(())
}

/// F3: evaluation records hold the pseudonymous ID, so they need the
/// pseudonymiser to be swept.
#[tokio::test]
async fn au_06_ac1_eval_records_deleted_for_an_orphan() -> Result<(), Box<dyn std::error::Error>> {
    let (_ports, fakes) = fake_ports();
    let gone = UserId::new(fakes.rng.uuid_v4());
    let live = UserId::new(fakes.rng.uuid_v4());
    seed_user_record(&fakes, live).await?;
    let pseudo = pseudonymiser();
    let gone_pseudo = pseudo.pseudo_id(&gone.0).as_str().to_owned();
    let live_pseudo = pseudo.pseudo_id(&live.0).as_str().to_owned();
    let record = testkit::contract::server_store::samples::eval(&gone_pseudo, 1);
    let expires_at = record.expires_at;
    fakes
        .store
        .classifier_eval()
        .put(&record, Precondition::MustNotExist)
        .await?;
    fakes
        .store
        .classifier_eval()
        .put(
            &testkit::contract::server_store::samples::eval(&live_pseudo, 2),
            Precondition::MustNotExist,
        )
        .await?;
    seed_records(&fakes, gone, expires_at).await?;

    sweep_deleted_users(fakes.store.as_ref(), 100, &pseudo).await?;

    let from = OffsetDateTime::from_unix_timestamp(0)?;
    let to = fakes.clock.now() + Duration::days(3650);
    let page = fakes
        .store
        .classifier_eval()
        .range(
            from,
            to,
            PageRequest {
                limit: 100,
                after: None,
            },
        )
        .await?;
    assert_eq!(
        page.items.len(),
        1,
        "the orphan's evaluation record is gone; the live user's stays"
    );
    assert_eq!(page.items[0].user_pseudo_id.0, live_pseudo);
    Ok(())
}
