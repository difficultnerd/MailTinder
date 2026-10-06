//! T-606: the race between undo's cancel and the job's run (SW-05 AC5,
//! UN-01 AC1).
//!
//! The cancel (undo) and the claim (the runner, stood in for by
//! `testkit::claim_and_send`) are two conditional writes against the same job
//! version. Firestore lets one through; the other re-reads and reports the
//! fresh state. This property drives both orderings and asserts the outcome is
//! always exactly one of {cancelled, no request sent} or {sent, undo says it
//! already went} - never both, never neither.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::too_many_lines,
    clippy::similar_names,
    clippy::items_after_statements,
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap
)]

use std::sync::Arc;

use api::services::undo::cancel_outcome;
use api::{app_state, config::ApiConfig};
use domain::undo::JobCancelOutcome;
use domain::{JobId, JobMethod, JobStatus, MailboxId, UserId};
use obs::Sensitive;
use ports::store::ListKeyHash;
use ports::{Ciphertext, JobRecord, Precondition, ServerStore};
use proptest::prelude::*;
use testkit::{claim_and_send, fake_ports, RecordingEgress};
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

const BASE: i64 = 1_790_000_000;

fn config() -> Result<ApiConfig, String> {
    ApiConfig::new(
        "https://mailtinder.test".to_owned(),
        "fake-client".into(),
        Sensitive::new(b"fake-log-key".to_vec()),
        Sensitive::new(b"fake-email-key".to_vec()),
    )
    .map_err(|e| format!("{e:?}"))
}

fn queued_job(n: u128) -> JobRecord {
    let due = OffsetDateTime::from_unix_timestamp(BASE).unwrap();
    JobRecord {
        job_id: JobId(Uuid::from_u128(n)),
        user_id: UserId::new(Uuid::from_u128(n ^ 1)),
        mailbox_id: MailboxId::new(Uuid::from_u128(n ^ 2)),
        list_key_hash: ListKeyHash([7; 32]),
        method: JobMethod::OneClick,
        target: Some(Ciphertext(vec![1, 2, 3])),
        due_at: due,
        status: JobStatus::Queued,
        attempts: 0,
        outcome: None,
        expires_at: due + Duration::minutes(60),
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]

    #[test]
    fn sw_05_ac5_undo_racing_run_exactly_one_outcome(
        n in 1u128..u128::MAX,
        claim_first in any::<bool>(),
        duplicate in any::<bool>(),
    ) {
        let (ports, fakes) = fake_ports();
        let egress = RecordingEgress::new();
        let outcome = futures::executor::block_on(async {
            let app = app_state(Arc::new(ports), Arc::new(config()?));
            let created = queued_job(n);
            let job = created.job_id;
            fakes
                .store
                .jobs()
                .put(&created, Precondition::MustNotExist)
                .await
                .map_err(|e| format!("{e:?}"))?;
            let store: &dyn ports::ServerStore = &*fakes.store;

            // The two orderings of the two conditional writes.
            let (cancel, sent) = if claim_first {
                let sent = claim_and_send(store, &egress, &job).await;
                let cancel = cancel_outcome(&api::services::jobs::cancel_queued_job(&app, &job).await.map_err(|e| format!("{e:?}"))?);
                (cancel, sent)
            } else {
                let cancel = cancel_outcome(&api::services::jobs::cancel_queued_job(&app, &job).await.map_err(|e| format!("{e:?}"))?);
                let sent = claim_and_send(store, &egress, &job).await;
                (cancel, sent)
            };
            // A duplicate delivery afterwards must change nothing.
            if duplicate {
                let _ = claim_and_send(store, &egress, &job).await;
                let _ = api::services::jobs::cancel_queued_job(&app, &job).await.map_err(|e| format!("{e:?}"))?;
            }
            let status = fakes
                .store
                .jobs()
                .get(&job)
                .await
                .map_err(|e| format!("{e:?}"))?
                .map(|v| v.record.status);
            Ok::<_, String>((cancel, sent, status))
        });
        let (cancel, sent, status) = outcome.map_err(TestCaseError::fail)?;
        let requests = egress.requests();

        let already_sent = cancel == JobCancelOutcome::AlreadySent;
        // Exactly one of the two worlds.
        prop_assert!(
            cancel == JobCancelOutcome::Cancelled || already_sent,
            "the cancel must either win or report it already went: {cancel:?} / sent {sent} / {status:?}"
        );
        if already_sent {
            prop_assert_eq!(requests, 1, "the request went exactly once");
            prop_assert!(sent, "undo reports the send the runner made");
            prop_assert!(
                matches!(status, Some(JobStatus::Running | JobStatus::Sent)),
                "the runner owns the job: {status:?}"
            );
        } else {
            prop_assert!(!sent, "a cancelled job is never claimed");
            prop_assert_eq!(requests, 0, "a cancelled job sends nothing");
            prop_assert_eq!(status, Some(JobStatus::Cancelled));
        }
    }
}
