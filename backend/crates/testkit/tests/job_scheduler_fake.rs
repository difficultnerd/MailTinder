//! T-304: the `JobScheduler` contract suite against `FakeJobScheduler`.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::sync::Arc;

use testkit::contract::job_scheduler::{job_scheduler, SchedulerTarget};
use testkit::FakeJobScheduler;

#[tokio::test]
async fn job_scheduler_contract_fake() -> Result<(), String> {
    job_scheduler(|| async {
        let fake = Arc::new(FakeJobScheduler::new());
        SchedulerTarget {
            scheduler: fake.clone(),
            control: fake,
        }
    })
    .await
}
