# T-701: unsub service skeleton and token minting

| Milestone | Tier | Size | Depends on |
| --- | --- | --- | --- |
| M7 | strong | about 600 lines of code plus tests | T-106, T-503 |

**Read only these spec sections:** S7 5.12 API-INT-1 (`docs/specs/S7-api-contract.md`), S7 3.5 (but see trap 1), S3 "UnsubscribeJob" entity row and the UnsubscribeJob state machine (`docs/specs/S3-domain-model.md`), S2 UN-01 AC1 to AC6 and UN-05 AC1, AC2 (`docs/specs/S2-v1-acceptance-criteria.md`), S4 3.3 steps 3 and 4 and the S4 2 role row for `unsub` (`docs/specs/S4-architecture.md`), S5 `jobs/{id}` row and test JOB-1 (`docs/specs/S5-data-inventory.md`), S6 5 paragraph "Access tokens" and S6 7 (`docs/specs/S6-security.md`), S10 6.3 rows "Undo racing the run", "Duplicate delivery", "Batching", "Retry and give up", "Jobs without a session", "Every outcome recorded" (`docs/specs/S10-test-strategy.md`), ASVS register rows V7.6.1, V8.3.1, V16.3.2, V16.5.3 (`docs/security/asvs-l2-register.md`). Nothing else is needed.

## Goal

Create the `unsub` Cloud Run service: one internal route (API-INT-1) that Cloud Tasks calls when an unsubscribe job falls due. It checks the caller's Google OIDC token, claims the job with a conditional state change, mints a mailbox access token from the stored refresh token when the method needs one, hands the job to a method sender (one-click in T-702, mailto in T-703), and records every outcome on the job record, in the security log and as a metric event. It also creates the shared library crate `svc-common` that `api`, `unsub` and `worker` use for internal caller checks, token minting and Needs Attention items.

## Files

| Action | Path | What |
| --- | --- | --- |
| Change | `backend/crates/svc-common/Cargo.toml` | Crate created by T-503; add any dependencies these modules need |
| Change | `backend/crates/svc-common/src/lib.rs` | `pub mod internal_auth; pub mod mint; pub mod needs_attention; pub mod job_record;` |
| Create | `backend/crates/svc-common/src/internal_auth.rs` | OIDC caller check for internal routes |
| Keep | `backend/crates/svc-common/src/mint.rs` | Created by T-503 (`mint_access_token`); use it, do not redefine |
| Create | `backend/crates/svc-common/src/needs_attention.rs` | `raise_item` (encrypts fields, writes the item) |
| Create | `backend/crates/svc-common/src/job_record.rs` | Encrypt and decrypt job `target` and `sender_display` |
| Create | `backend/crates/ports/src/caller.rs` | `CallerVerifier` port |
| Change | `backend/crates/ports/src/lib.rs` | `pub mod caller;` and add `caller: Arc<dyn CallerVerifier>` to `Ports` |
| Create | `backend/crates/adapters-gcp/src/oidc_caller.rs` | `GoogleCallerVerifier` (JWKS fetched through `HttpEgress`, cached 1 hour) |
| Create | `backend/crates/testkit/src/fake_caller.rs` | `FakeCallerVerifier` (accepts tokens the test minted) |
| Create | `backend/crates/unsub/src/main.rs` | Binary: config, `Ports`, bind `PORT` |
| Create | `backend/crates/unsub/src/lib.rs` | `pub fn router(state: UnsubState) -> axum::Router` |
| Create | `backend/crates/unsub/src/runner.rs` | `run_job` (claim, dispatch, finish) |
| Create | `backend/crates/unsub/src/sender.rs` | `UnsubSender` trait and `SendResult` |
| Create | `backend/crates/unsub/src/config.rs` | `UnsubConfig` from environment |
| Create | `backend/crates/unsub/tests/run_job.rs` | Service integration tests |
| Change | `backend/Cargo.toml` | Add `crates/svc-common` to workspace members |

## Types and signatures

```rust
// ports/src/caller.rs
#[async_trait]
pub trait CallerVerifier: Send + Sync {
    /// Verifies a Google-signed OIDC ID token: signature against Google's keys, `iss`,
    /// `aud` equal to `audience`, `exp`, and returns the verified `email`.
    async fn verify(&self, bearer: &Sensitive<String>, audience: &str) -> Result<VerifiedCaller, CallerAuthError>;
}
pub struct VerifiedCaller { pub email: String, pub email_verified: bool }
#[derive(Debug, thiserror::Error)]
pub enum CallerAuthError { #[error("malformed")] Malformed, #[error("signature")] BadSignature,
    #[error("issuer")] WrongIssuer, #[error("audience")] WrongAudience, #[error("expired")] Expired,
    #[error("keys unavailable")] KeysUnavailable }

// svc-common/src/internal_auth.rs
pub struct InternalAuthConfig { pub audience: String, pub allowed_caller_email: String }
pub async fn verify_internal_caller(
    verifier: &dyn CallerVerifier, cfg: &InternalAuthConfig, authorization_header: Option<&str>,
) -> Result<(), InternalAuthError>;   // InternalAuthError maps to 401 in every service
#[derive(Debug, thiserror::Error)]
pub enum InternalAuthError { #[error("missing")] Missing, #[error("invalid")] Invalid(CallerAuthError), #[error("caller")] WrongCaller }

// svc-common/src/mint.rs
#[derive(Debug, thiserror::Error)]
pub enum MintError {
    #[error("refresh token revoked or invalid")] Revoked,   // invalid_grant: set mailbox needs_sign_in
    #[error("mailbox missing")] MailboxMissing,
    #[error("transient")] Transient,                       // identity provider 5xx or network
    #[error("crypto")] Crypto,                             // KeyService failure; never retried silently
}
/// Loads the mailbox, opens the refresh token with Aad { user, scope: mailbox id, field: aad_fields::MAILBOX_REFRESH_TOKEN },
/// calls IdentityProvider::refresh. Never stores the access token anywhere.
/// On MintError::Revoked it sets the mailbox status to needs_sign_in (conditional write).
pub async fn mint_access_token(ports: &Ports, user: &UserId, mailbox: &MailboxId) -> Result<MailboxCtx, MintError>;

// svc-common/src/needs_attention.rs
pub struct NewItem<'a> {
    pub user: &'a UserId, pub mailbox: &'a MailboxId,
    pub sender_display: &'a str, pub link: Option<&'a url::Url>, pub reason: NeedsAttentionReason,
}
/// Validates the link (https only, see T-704 `safe_link`), encrypts sender_display and link under the
/// user's data_key (aad_fields::NA_SENDER_DISPLAY and NA_LINK from T-201b, scope = item id), sets created_at = now
/// and expires_at = now + NEEDS_ATTENTION_TTL (30 days), writes a NeedsAttentionRecord (T-201b) with
/// Precondition::MustNotExist. Returns the item ID.
pub async fn raise_item(ports: &Ports, item: NewItem<'_>) -> Result<NeedsAttentionId, SvcError>;

// unsub/src/sender.rs
pub enum SendResult {
    Sent { code: JobOutcomeCode },                    // OneClickAccepted or MailtoSent
    Retryable { code: JobOutcomeCode },               // HttpRejected, TimedOut, or Transient (see below); final attempt records RetriesExhausted
    NeedsAttention { reason: NeedsAttentionReason, code: JobOutcomeCode },
    TokenRevoked,                                     // records TokenInvalid
}
#[async_trait]
pub trait UnsubSender: Send + Sync {
    fn method(&self) -> JobMethod;                    // OneClick or Mailto (domain, T-106)
    async fn send(&self, ports: &Ports, job: &ClaimedJob) -> SendResult;
}
pub struct ClaimedJob {
    pub job: JobRecord,                               // T-201b record, status Running
    pub target: Sensitive<String>,                    // decrypted target (https URL or mailto URI)
    pub sender_display: Sensitive<String>,
}

// unsub/src/runner.rs
pub const MAX_ATTEMPTS: u32 = 4;                      // Cloud Tasks maxAttempts (S7 5.12) [TUNABLE in queue config]
pub const OUTCOME_RETENTION: time::Duration = time::Duration::days(30); // S3, S5 jobs row [TUNABLE]
pub struct Delivery { pub attempt: u32 }              // from X-CloudTasks-TaskRetryCount, 0 on first delivery
pub enum RunResponse { Done, Retry }                  // 200 and 503
pub async fn run_job(state: &UnsubState, job_id: &JobId, delivery: Delivery) -> Result<RunResponse, UnsubError>;

pub struct UnsubState { pub ports: Ports, pub senders: Vec<Arc<dyn UnsubSender>>, pub auth: InternalAuthConfig }
```

Types used and owned elsewhere: `JobRecord`, `JobOutcome { code, at }`, `JobOutcomeCode`, `NeedsAttentionRecord`, `aad_fields`, `Precondition`, `Versioned` and the `JobRepo` helpers (`get`, `put`, `queued_for_list`, `by_user_with_outcome`) from T-201b; `JobStatus` (`Queued`, `Running`, `Sent`, `Cancelled`, `NeedsAttention`, `Failed`, `Expired`) and `JobMethod` from T-106; `NeedsAttentionReason` from T-107. A conditional write is `jobs().put(&record, Precondition::Matches(version))`; `StoreError::PreconditionFailed` is the conflict.

Shared additions this task makes (list them in the PR and in CONVENTIONS.md):

- `JobOutcomeCode` gains `Batched` (another job for the list already sent), `MailboxRemoved`, `OwnerMismatch` and `Refused` (target invalid, method unavailable, mailto quota or provider refusal). Keep the T-201b values for everything else.
- `NeedsAttentionReason` gains `SignInRequired` (serialised `sign_in_required`) if T-107 lacks it (trap 2).
- `JobRecord` gains `sender_display: Option<Ciphertext>` with `aad_fields::JOB_SENDER_DISPLAY = "job.sender_display"` (scope = job_id), written by T-605 and cleared at finish (trap 3). Add it to the S5 field allowlist in `tests/store_schema.rs` and report the S5 change.

## Algorithm

Handler for `POST /internal/v1/unsubscribe-jobs/{job_id}/run`:

1. Call `verify_internal_caller` with the `Authorization: Bearer` header, audience from config (`UNSUB_AUDIENCE`), allowed caller from config (`UNSUB_TASKS_CALLER`, the Cloud Tasks service account email). On failure return `401` with an empty body and write a security event `internal_auth_failed` (route template, outcome, request ID; never the token).
2. Parse `job_id` as UUID. Not a UUID: `400`, security event `invalid_request`.
3. Read `X-CloudTasks-TaskRetryCount` as `attempt` (missing or unparsable: 0).
4. Load the job. Missing: return `200` (undo raced the task; S7 5.12).
5. Ownership re-check (ASVS V8.3.1): load the mailbox `job.mailbox_id`. Missing: finish the job `Cancelled` with outcome code `MailboxRemoved`. Mailbox `user_id` differs from `job.user_id`: finish `Failed` with outcome code `OwnerMismatch`, security event `job_owner_mismatch`, raise no Needs Attention item. Both return `200`.
6. `now = clock.now()`. If `now > job.expires_at` and the job is not terminal: finish `Expired` with code `Expired` and reason `job_expired` (Needs Attention item raised), return `200`.
7. If `now < job.due_at`: return `Retry` (`503`) without claiming. Cloud Tasks never delivers early in practice; this guards the undo window.
8. Claim with a conditional update on the version read in step 4:
   - `Queued` to `Running` with `attempts = attempt`; or
   - `Running` to `Running` with `attempts = attempt` only when `job.attempts < attempt` (a redelivery after a retryable failure or a crash);
   - anything else (terminal, or `Running` with `attempts >= attempt`, which is a duplicate delivery): return `200` and do nothing.
   - Conflict on the conditional write: re-read once; if now terminal (for example undo cancelled it) return `200`; otherwise return `Retry`.
9. Batching (UN-01 AC2): `jobs().by_user_with_outcome(user, 500)`; if any other record has the same `list_key_hash` and status `Sent`, finish this job `Sent` with outcome code `Batched` and send nothing.
10. Decrypt `target` and `sender_display` (`job_record::open`, Aad fields `JOB_TARGET` and `JOB_SENDER_DISPLAY`, scope = job ID). Find the sender whose `method()` equals `job.method`. No sender registered: finish `NeedsAttention` with reason `unsubscribe_failed`, code `Refused`.
11. Call `sender.send(...)`. Senders that need the mailbox call `mint_access_token` themselves (UN-01 AC5: one-click never mints).
12. Map the `SendResult`:
    - `Sent { code }`: finish `Sent` with that code. Then, for every job from `jobs().queued_for_list(user, list_key_hash)` other than this one, conditionally move it `Queued` to `Sent` with outcome code `Batched` and call `JobScheduler::cancel` on its task (task name is its job ID; `AlreadyRunning` and not-found are fine). A conflict on one sibling is skipped, not retried.
    - `Retryable { code }` and `attempt + 1 < MAX_ATTEMPTS`: leave the job `Running` (attempts already recorded), log the code, return `Retry` (`503`). The job never schedules itself again (S3: Cloud Tasks is the only retry layer).
    - `Retryable` on the final attempt (`attempt + 1 >= MAX_ATTEMPTS`): finish `NeedsAttention` with reason `unsubscribe_failed` and code `RetriesExhausted`.
    - `NeedsAttention { reason, code }`: finish `NeedsAttention` with that reason.
    - `TokenRevoked`: finish `Failed` with reason `sign_in_required` and code `TokenInvalid`; `mint_access_token` has already set the mailbox to `needs_sign_in`.
13. "Finish" means one conditional update from `Running` (or from `Queued` in steps 5 and 6) to the terminal status that:
    - sets `outcome = JobOutcome { code, at: now }`;
    - clears `target` (S3: a terminal job keeps only its outcome) but keeps `sender_display` until the item is raised;
    - sets `expires_at = now + OUTCOME_RETENTION` so JOB-1 holds for terminal records (see trap 4);
    - then, for `NeedsAttention`, `Failed` (except `OwnerMismatch`) and `Expired`, calls `raise_item` with the decrypted `sender_display`, the link (the target only when it is an https URL, else `None`) and the reason;
    - then clears `sender_display` with a second update (or in the same update after the item is written; order: item first, so a crash leaves a retryable job rather than a silent one);
    - writes one security event `unsub_job_outcome` (pseudonymous user ID, job outcome code, method, request ID) and one metric event `unsub_outcome` (event type, outcome code, pseudonymous user ID, provider, time).
14. Return `200` for `Done`, `503` for `Retry`. Bodies are empty JSON `{}`.

`mint_access_token`:

1. Load the mailbox; missing gives `MailboxMissing`.
2. Load the user's wrapped `data_key`; `KeyService::open` the refresh token with `Aad { user, scope: mailbox_id string, field: aad_fields::MAILBOX_REFRESH_TOKEN }`. Failure gives `Crypto`.
3. `IdentityProvider::refresh`. `invalid_grant` (or the T-206 equivalent `IdError`) gives `Revoked`: conditionally set mailbox `status = needs_sign_in`, log security event `sign_in_required`. Network or 5xx gives `Transient`.
4. Return `MailboxCtx { mailbox, access_token }`. The access token lives only in memory.

T-503 puts `TokenService::mailbox_ctx` in the `api` crate. `unsub` cannot depend on `api`, so move that logic into `svc-common::mint` in this PR and make `api`'s `TokenService::mailbox_ctx` a thin wrapper over it; behaviour must stay identical and T-503's tests must still pass. Do not import from the `api` crate.

`GoogleCallerVerifier`: fetch Google's JWKS from `https://www.googleapis.com/oauth2/v3/certs` through `HttpEgress::call`, cache for 1 hour (Clock based), verify RS256 signature with the `jsonwebtoken` crate `[DEFAULT: same crate T-502 uses for ID tokens, if any]`, require `iss` in {`https://accounts.google.com`, `accounts.google.com`}, `aud` equal to the configured audience, `exp` in the future with 60 s leeway, `email_verified` true.

## Acceptance criteria

| ID | Behaviour (one line) |
| --- | --- |
| UN-01 AC1 | A due job runs exactly once, including under duplicate delivery |
| UN-01 AC2 | Several jobs for one list send one request |
| UN-01 AC3 | Every outcome is stored on the job record and written to the security log |
| UN-01 AC4 | The job mints an access token at run time and stores none; it runs with the user signed out |
| UN-01 AC6 | A revoked or invalid refresh token ends the job `failed` with a "Sign in again" item |
| UN-05 AC1 | A job that cannot complete raises an item with sender, link and reason |
| UN-05 AC2 | No job loops: Cloud Tasks is the only retry layer and the final attempt ends in Needs Attention |
| SW-05 AC5 | Claim and undo cancel race: exactly one wins |
| INV-2 | Every job record carries `expires_at`, including terminal ones |
| JOB-1 | No job record holds an access token; a terminal job keeps only its outcome |
| V8.3.1 | The runner re-checks that the mailbox belongs to the job's user |
| V7.6.1 | A revoked Google grant ends the job with "Sign in again" |
| V16.3.2 | Failed internal authentication is logged |
| V16.5.3 | A job is never marked sent when the sender reports an error |

## Tests that must pass

- `un_01_ac1_due_job_runs_exactly_once` (service integration, `unsub`)
- `un_01_ac1_duplicate_delivery_is_noop` (service integration: same attempt delivered twice, one send)
- `un_01_ac1_missing_job_returns_200` (service integration)
- `un_01_ac2_batched_jobs_send_one_request` (service integration: three queued jobs, one list key, one send; siblings `Sent` with code `Batched`, their tasks cancelled)
- `un_01_ac2_job_after_sent_sibling_sends_nothing` (service integration)
- `un_01_ac3_every_terminal_outcome_logged_and_stored` (service integration: sent, needs_attention, failed, expired each give one outcome on the record and one `unsub_job_outcome` event)
- `un_01_ac4_access_token_minted_at_run_not_stored` (service integration: fake store write hook asserts no field holds the minted token)
- `un_01_ac4_runs_without_session` (service integration: no session record exists)
- `un_01_ac6_revoked_refresh_token_fails_with_sign_in_item` (service integration, `invalid_grant` from the identity fake)
- `un_01_ac6_mailbox_set_needs_sign_in` (service integration)
- `un_05_ac1_failed_job_creates_item_with_sender_link_reason` (service integration)
- `un_05_ac2_retry_until_final_attempt_then_needs_attention` (service integration: attempts 0 to 2 return 503, attempt 3 ends `needs_attention`)
- `un_05_ac2_job_never_requeues_itself` (service integration: fake `JobScheduler` records zero `schedule` calls from `unsub`)
- `sw_05_ac5_claim_and_cancel_exactly_one_wins` (property, `proptest`: interleave the runner's claim with a `Queued` to `Cancelled` conditional write; exactly one of {cancelled and zero sends} or {sent and cancel refused})
- `inv_2_terminal_job_has_expires_at` (unit, `unsub`)
- `job_1_no_access_token_on_job_record` (service integration)
- `job_1_terminal_job_keeps_only_outcome` (service integration: target cleared, sender_display cleared after the item)
- `asvs_v8_3_1_runner_rechecks_mailbox_owner` (service integration)
- `asvs_v7_6_1_revoked_grant_ends_job_with_sign_in_item` (service integration)
- `asvs_v16_3_2_internal_auth_failure_logged` (service integration)
- `asvs_v16_5_3_job_never_marked_sent_on_error` (property: any non-`Sent` `SendResult` never yields status `Sent`)
- `api_int_1_rejects_missing_token`, `api_int_1_rejects_wrong_audience`, `api_int_1_rejects_wrong_caller_email` (service integration)
- `api_int_1_release_build_has_no_test_routes` (unit: router route list has exactly one route)

## Edge cases and traps

1. S7 3.5 says jobs hold an access token minted at queue time. That is out of date: S2 UN-01 AC4, S5 JOB-1 and S6 5 say no access token is ever stored on a job. Follow S2, S5 and S6.
2. S7 5.8 and the OpenAPI `reason_code` enum have no value for "Sign in again", but S2 UN-01 AC6 and S9 6 need one. Use `sign_in_required` `[DEFAULT]` and keep the enum change in the same PR as T-705; this gap is reported to the spec owner.
3. S3 lists no sender display on the job, but every Needs Attention item needs one. Store it encrypted on the job (`sender_display`, `aad_fields::JOB_SENDER_DISPLAY`) and clear it at finish. If T-605 is merged before this field exists, a job without it gets the sender display "this sender" `[DEFAULT]` in its item.
4. S3 keeps terminal jobs for up to 30 days while JOB-1 says no job survives its `expires_at`. Resetting `expires_at` to `now + 30 days` at the terminal transition satisfies both. Do not leave the old `due_at + 1 hour` value on a terminal job or the sweeper (T-706) deletes the outcome before the Feed collects it.
5. Do not use `SystemTime::now()` or `Instant::now()` for job logic; use `Clock`.
6. Never log the target, the sender display, the mailbox address, the access token or the bearer token. Use `Sensitive<T>` for all of them.
7. A `Running` job is not "already done": a redelivery with a higher `X-CloudTasks-TaskRetryCount` must be able to reclaim it, or a crash leaves it stuck until expiry. A redelivery with the same count is a duplicate and must do nothing.
8. Do not return `4xx` for transient problems; Cloud Tasks retries any non-2xx, but `503` is the documented retry signal.
9. Do not match `_` on `Provider` or `JobMethod`; list every variant so v2 forces a review.
10. Batching is by `(user_id, list_key_hash)`, never across users.
11. The conditional writes must compare a version (Firestore update time precondition), not re-read and then write.
12. `svc-common` must not depend on `api`, `unsub` or `worker`. The binaries depend on it.
13. The Google JWKS host `www.googleapis.com` is not in the S4 5.7 `unsub` allowlist; add it to the `unsub` and `worker` allowlists in this PR (T-306 owns the constant) and report it.

## Out of scope

- The one-click POST and its SSRF checks: T-702. The mailto send: T-703.
- The api side of undo and the race from the api's view: T-606.
- Creating jobs at reject time: T-605. Appending outcomes to History: T-602 or T-609.
- Expiring jobs that never ran: the sweeper in T-706.
- Needs Attention endpoints: T-705.

## Security review checklist

- The OIDC check runs before any other work and checks signature, issuer, audience, expiry and the exact caller email; a token for `worker` or `api` audiences is refused.
- No code path stores an access token or writes the decrypted target, sender display or address to logs, errors or metrics.
- The claim is a single conditional write; there is no window where two deliveries or a delivery and an undo both proceed.
- The final-attempt branch cannot loop and cannot mark `Sent`.
- `mint_access_token` uses the correct `Aad` (user, mailbox, field) and sets `needs_sign_in` on `invalid_grant` only.
- Ownership is re-checked from the mailbox record, not trusted from the job.
- The release build exposes exactly one route and no test hooks.
- `svc-common::needs_attention::raise_item` refuses non-https links.

## Done when

- The tests above pass and every required check is green (S10 10.1).
- Definition of done in S10 10.4.
- `cargo run -p unsub` starts and answers `401` to an unauthenticated call on the internal route.
- The PR description lists the CONVENTIONS additions (`svc-common` crate, `CallerVerifier` port, `jobs()` helpers, job `sender_display`) and the CONVENTIONS.md edit lands in the same PR.
