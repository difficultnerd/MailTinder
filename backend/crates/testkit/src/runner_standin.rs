//! T-606: a test stand-in for the T-701 runner claim.
//!
//! `claim_and_send` is the other side of the conditional write `undo` cancels
//! with. It lives in `testkit` (dev only) because T-701 (the real runner) is a
//! separate service; the conditional write it must keep is the same:
//! `queued -> running` under `Precondition::Matches(version)`.
//!
//! The `RecordingEgress` counts one-click requests, so a test can assert that a
//! cancelled job produces zero requests and a claimed one produces exactly one.

use std::sync::atomic::{AtomicU32, Ordering};

use async_trait::async_trait;
use domain::{apply, Applied, JobEvent, JobId, JobState, JobStatus, Tunables};
use ports::{
    EgressError, EgressRequest, EgressResponse, HttpEgress, JobOutcome, JobOutcomeCode, JobRecord,
    OneClickOutcome, Precondition, ServerStore,
};
use url::Url;

/// An egress fake that counts requests; every one-click post is accepted.
#[derive(Default)]
pub struct RecordingEgress {
    requests: AtomicU32,
}

impl RecordingEgress {
    pub fn new() -> Self {
        Self::default()
    }

    /// The number of one-click requests sent so far.
    pub fn requests(&self) -> u32 {
        self.requests.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl HttpEgress for RecordingEgress {
    async fn one_click_post(&self, _url: &Url) -> Result<OneClickOutcome, EgressError> {
        self.requests.fetch_add(1, Ordering::SeqCst);
        Ok(OneClickOutcome::Accepted { status: 200 })
    }

    async fn call(&self, _req: EgressRequest) -> Result<EgressResponse, EgressError> {
        Err(EgressError::NotPermitted)
    }
}

/// The runner's claim, exactly as T-701 must do it: get, then put
/// `status Running` under `Precondition::Matches(version)`; on success one call
/// to the egress fake, then put `status Sent`. Returns whether it sent.
pub async fn claim_and_send(
    store: &dyn ServerStore,
    egress: &RecordingEgress,
    job: &JobId,
) -> bool {
    let Ok(Some(versioned)) = store.jobs().get(job).await else {
        return false;
    };
    if versioned.record.status != JobStatus::Queued {
        return false;
    }
    let now = versioned.record.due_at;
    let state = JobState {
        status: versioned.record.status,
        attempts: versioned.record.attempts,
        due_at: versioned.record.due_at,
        expires_at: versioned.record.expires_at,
    };
    let Ok(Applied::Changed(next)) = apply(
        state,
        JobEvent::Claim { attempt: 1, now },
        &Tunables::default(),
    ) else {
        return false;
    };
    let running = JobRecord {
        status: JobStatus::Running,
        attempts: next.attempts,
        expires_at: next.expires_at,
        ..versioned.record
    };
    let Ok(version) = store
        .jobs()
        .put(&running, Precondition::Matches(versioned.version))
        .await
    else {
        return false;
    };
    // One request; the real runner would read the sealed target first.
    if let Ok(url) = Url::parse("https://lists.example.com/unsubscribe") {
        let _ = egress.one_click_post(&url).await;
    }
    let sent = JobRecord {
        status: JobStatus::Sent,
        outcome: Some(JobOutcome {
            code: JobOutcomeCode::OneClickAccepted,
            at: now,
        }),
        target: None,
        ..running.clone()
    };
    let _ = store
        .jobs()
        .put(&sent, Precondition::Matches(version))
        .await;
    true
}
