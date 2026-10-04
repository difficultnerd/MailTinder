# T-304: Cloud Tasks JobScheduler

| Milestone | Tier | Size | Depends on |
| --- | --- | --- | --- |
| M3 | sonnet | about 250 lines of code plus tests | T-202b, T-301 |

**Read only these spec sections:** S4 1 rows "Delayed, cancellable unsubscribe jobs" and "Job execution", S4 3.3 steps 1, 2 and 4 (`docs/specs/S4-architecture.md`); S7 5.12 API-INT-1 (`docs/specs/S7-api-contract.md`); S3 "UnsubscribeJob" state machine paragraph under it (`docs/specs/S3-domain-model.md`); `docs/backlog/T-201a-port-traits.md` ("scheduler.rs" block); `docs/backlog/T-202b-fakes-ports-and-virtual-clock.md` (`FakeJobScheduler` rules). Nothing else is needed.

## Goal

`adapters-gcp` gains `CloudTasksScheduler`, the production `JobScheduler`: one Cloud Task per unsubscribe job, named after the job ID, scheduled for the due time, calling `unsub`'s API-INT-1 with a Google-signed OIDC token, and cancellable by deleting the task. A shared contract suite proves it and `FakeJobScheduler` behave the same.

## Files

| Action | Path | What |
| --- | --- | --- |
| Create | `backend/crates/adapters-gcp/src/tasks.rs` | `CloudTasksScheduler`, `TasksConfig` |
| Create | `backend/crates/testkit/src/contract/job_scheduler.rs` | shared suite |
| Change | `backend/crates/testkit/src/contract/mod.rs` | `pub mod job_scheduler;` |
| Create | `backend/crates/testkit/tests/job_scheduler_fake.rs` | suite against `FakeJobScheduler` |
| Change | `backend/crates/testkit/src/scheduler.rs` | `impl SchedulerControl for FakeJobScheduler` (calls `start` and `finish`) |
| Create | `backend/crates/adapters-gcp/tests/tasks_rest.rs` | suite and request-shape tests against a local `axum` stub of the Cloud Tasks REST API |

## Types and signatures

```rust
pub struct TasksConfig {
    pub queue: String,              // projects/{p}/locations/us-central1/queues/unsubscribe
    pub unsub_base: Url,            // https://unsub-<hash>-uc.a.run.app
    pub invoker_sa_email: String,   // service account Cloud Tasks signs the OIDC token as
    pub audience: String,           // the audience unsub checks (S7 5.12)
}
pub const DISPATCH_DEADLINE_S: u64 = 60; // [DEFAULT] one-click timeout is 10 s; leaves room for token minting
pub struct CloudTasksScheduler { http: Arc<GcpHttp>, cfg: TasksConfig, base: Url } // base = https://cloudtasks.googleapis.com/v2/
impl CloudTasksScheduler {
    pub fn new(http: Arc<GcpHttp>, cfg: TasksConfig) -> Self;
    #[cfg(any(test, feature = "test-support"))] pub fn with_base(http: Arc<GcpHttp>, cfg: TasksConfig, base: Url) -> Self;
}
impl JobScheduler for CloudTasksScheduler { /* below */ }

// testkit/src/contract/job_scheduler.rs
pub struct SchedulerTarget { pub scheduler: Arc<dyn JobScheduler>, pub control: Arc<dyn SchedulerControl> }
#[async_trait::async_trait]
pub trait SchedulerControl: Send + Sync {
    async fn mark_running(&self, task: &TaskName) -> Result<(), String>;   // simulate a dispatched attempt
    async fn mark_done(&self, task: &TaskName) -> Result<(), String>;
}
pub async fn job_scheduler<F, Fut>(make: F) -> Result<(), String>
where F: Fn() -> Fut, Fut: Future<Output = SchedulerTarget>;
```

## Algorithm

1. `schedule(job, due_at)`:
   1. `name = TaskName::for_job(job)`; full name `{queue}/tasks/{name}`.
   2. `POST {base}{queue}/tasks` with JSON:
      `{"task": {"name": "<full name>", "scheduleTime": "<due_at RFC 3339 UTC>", "dispatchDeadline": "60s",
        "httpRequest": {"url": "<unsub_base>/internal/v1/unsubscribe-jobs/<job uuid>/run", "httpMethod": "POST",
        "headers": {"Content-Type": "application/json"}, "body": "<base64 of {}>",
        "oidcToken": {"serviceAccountEmail": "<invoker_sa_email>", "audience": "<audience>"}}}}`.
   3. `200`: return `TaskName(name)`. `409 ALREADY_EXISTS`: the task exists already (a retried request, or Cloud Tasks' name de-duplication); return `TaskName(name)` (idempotent per job). Other errors: `429`, `5xx`, timeout give `SchedError::Unavailable`; `400`, `403`, `404` give `Rejected("tasks_create")`.
2. `cancel(task)`: `DELETE {base}{queue}/tasks/{task}`. `200` gives `Cancelled`; `404 NOT_FOUND` gives `NotFound` (already ran, already deleted, or never existed); `400 FAILED_PRECONDITION` gives `AlreadyRunning` `[ASSUMES]` (the attempt is dispatched; the job record's conditional transition decides the race, SW-05 AC5); `429`, `5xx` give `Unavailable`.
3. The job ID is in the URL path, and the body is `{}`: no target, address or token travels in the task (S7 5.12 loads the job by ID).
4. Retries belong to the queue configuration (`maxAttempts` 4, `minBackoff` 30 s, `maxBackoff` 5 min, S7 5.12), set in Terraform (T-1102). This code never re-enqueues a task.
5. Contract suite cases: schedule returns the job's task name; scheduling the same job twice returns the same name and does not create a second task; cancel of a pending task is `Cancelled`; cancel again is `NotFound`; cancel of a running task is `AlreadyRunning`; cancel of a done task is `NotFound`; cancel of an unknown name is `NotFound`.
6. The `axum` stub for `tasks_rest.rs` keeps a map of tasks keyed by full name, answers `POST` (409 on a duplicate name), `DELETE` (404 when missing, 400 `FAILED_PRECONDITION` when marked running), and implements `SchedulerControl`. It also records the request JSON for the shape test.

## Acceptance criteria

| ID | Behaviour (one line) |
| --- | --- |
| None | No S2 AC is proved here. T-605 (SW-03 AC2), T-606 (SW-05 AC2, AC5) and T-701 (UN-01 AC1) prove behaviour on top of this contract |

## Tests that must pass

- `job_scheduler_contract_fake` (contract, `testkit`).
- `job_scheduler_contract_cloud_tasks_stub` (contract, `adapters-gcp/tests/tasks_rest.rs`).
- `cloud_tasks_create_body_shape` (unit against the stub: task name, `scheduleTime` in UTC, URL path with the job ID, empty JSON body, `oidcToken` with the configured email and audience, `dispatchDeadline` 60 s, no other headers).
- `cloud_tasks_create_conflict_is_idempotent` (unit).
- `cloud_tasks_errors_map_to_sched_error` (unit: 429 and 503 give `Unavailable`; 403 gives `Rejected`).

## Edge cases and traps

- `scheduleTime` must be RFC 3339 in UTC (`Z`); a local offset is accepted by the API but makes logs confusing.
- `body` in `httpRequest` is base64 (standard, padded) of the bytes, not a JSON object.
- Do not add an `Authorization` header to the task: `oidcToken` makes Cloud Tasks sign one; a static header would be a long-lived credential (ASVS V13.2.1).
- Cloud Tasks refuses to re-create a deleted task name for a while; this is fine because a new reject makes a new job ID, but never reuse a job ID after an undo.
- A successful `cancel` does not prove the job did not run; T-606 must still use the conditional job transition.
- Do not log the task URL or name together with a user ID; log `route = "tasks.create"` and the status only (T-307).
- Calls go through `GcpHttp` (platform hosts), not `HttpEgress`.

## Out of scope

- Queue creation, retry config and IAM (T-1102). The `unsub` endpoint and its OIDC check (T-701). Undo and its race (T-606).
- Staging smoke against real Cloud Tasks (T-1105).

## Done when

- The tests above pass and every required check is green (S10 10.1).
- Definition of done in S10 10.4.
