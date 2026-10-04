# T-601b: Mailbox disconnect with app folder move

| Milestone | Tier | Size | Depends on |
| --- | --- | --- | --- |
| M6 | sonnet | about 300 lines of code plus tests | T-304, T-405, T-601a |

Split from index row T-601 (list, link and disconnect).

**Read only these spec sections:** S7 section 5.3 (API-MBX-2) and section 4 (`409 last_mailbox`, `409 app_folder_move_failed`, `403 step_up_required`) in `docs/specs/S7-api-contract.md`; S2 AU-05 AC1 to AC4; S3 "User app folder" paragraph under "Where each entity lives", `UnsubscribeJob` state machine; S5 `mailboxes/{id}` rows and DEL-3; S10 section 6.3 rows "Disconnect cancels" and "Primary mailbox disconnect". Nothing else is needed.

## Goal

A user can disconnect one of several mailboxes (API-MBX-2). If it holds the app folder file, the file moves first to the next mailbox's Drive. Queued jobs are cancelled, Needs Attention items deleted and the Google token revoked and deleted. This task also adds `cancel_queued_job`, the conditional Queued to Cancelled write that the undo race (T-606) and account deletion (T-803) reuse; the runner's claim (T-701) is the other side of the same conditional write.

## Files

| Action | Path | What |
| --- | --- | --- |
| Change | `backend/crates/api/src/routes/mailboxes.rs` | `DELETE /api/v1/mailboxes/{mailbox_id}` handler |
| Create | `backend/crates/api/src/services/mailbox_disconnect.rs` | Disconnect steps in order |
| Create | `backend/crates/api/src/services/jobs.rs` | `cancel_queued_job` shared helper |
| Create | `backend/crates/api/tests/mailbox_disconnect.rs` | Service integration tests |

## Types and signatures

```rust
// Used from earlier tasks (keep merged names): AppState, ApiError, AuthedSession (T-500, T-501),
// require_step_up (T-504), TokenService::mailbox_ctx and ::refresh_token (T-503),
// AppFolderStore (T-405), JobScheduler + TaskName (T-304), JobStatus and JobOutcome (T-106),
// security_event (T-307).

// Store names from T-201b: jobs().get -> Option<Versioned<JobRecord>>, jobs().put(&JobRecord, Precondition),
// jobs().by_mailbox(&MailboxId), Precondition::Matches(Version), StoreError::PreconditionFailed.
// No new store method: the compare-and-set is get, then put with Precondition::Matches(version).

// backend/crates/api/src/services/jobs.rs
pub fn task_name_for(job: &JobId) -> TaskName;      // the name T-605a uses when scheduling
pub enum CancelResult { Cancelled, NotQueued(JobStatus), Missing }
pub async fn cancel_queued_job(app: &AppState, job: &JobId) -> Result<CancelResult, ApiError>;

// backend/crates/api/src/services/mailbox_disconnect.rs
pub async fn disconnect(app: &AppState, session: &AuthedSession, mailbox: &MailboxId)
    -> Result<(), ApiError>;
```

`JobOutcome` is `{ code: JobOutcomeCode, at }` from T-201b; this task writes `code = Cancelled`.

## Algorithm

`cancel_queued_job(job)`:

1. `jobs().get(job)`. `None`: return `Missing`. Status not `Queued`: return `NotQueued(status)`.
2. Build the cancelled record: `status = Cancelled`, `outcome = Some(JobOutcome { code: Cancelled, at: now })`, `target = None`, `expires_at = now + 30 days` (T-201b terminal retention rule). `put(.., Precondition::Matches(version))`.
3. `PreconditionFailed` (the runner claimed it between 1 and 2): re-read once and return `NotQueued(status)` or `Missing`. Never retry the cancel.
4. On success: `JobScheduler::cancel(task_name_for(job))`. Treat `NotFound` and `AlreadyRunning` as success. Return `Cancelled`.

The cancelled job record stays; the next Feed load (T-609) appends "cancelled" to History and deletes it.

`disconnect(mailbox)`, in this order:

1. `require_step_up`; otherwise `403 step_up_required` and nothing changes (AU-05 AC3).
2. Load the mailbox; missing or another user's: `404 not_found`.
3. Count the user's mailboxes (`mailboxes().by_user`); if this is the only one: `409 last_mailbox` (AU-05 AC2).
4. If `is_primary` (AU-05 AC4):
   1. Pick the next mailbox: the other mailbox with the earliest `linked_at` whose status is `connected`. None: `409 app_folder_move_failed`.
   2. Get contexts for both mailboxes (`TokenService::mailbox_ctx`). Either fails: `409 app_folder_move_failed`.
   3. `AppFolderStore::read(old)`. `None` means there is no file to move; skip to step 4.5.
   4. `AppFolderStore::read(new)` for its ETag (a stale copy may exist), then `write(new, bytes, if_match)`. The bytes are copied as they are: the file's AAD names the user, not the mailbox (T-602b), so no re-encryption.
   5. Put the new mailbox with `is_primary = true`, then the old with `false`, each with `Precondition::Matches(version)`; a precondition failure is a move failure (step 4.7).
   6. `AppFolderStore::delete(old)`. If it fails: set primary back to old, delete the new copy (best effort) and return `409 app_folder_move_failed`.
   7. Any failure in 4.2 to 4.5 returns `409 app_folder_move_failed` and leaves the old mailbox primary and connected.
5. Cancel jobs: for every job from `jobs().by_mailbox(mailbox)` with status `queued`, call `cancel_queued_job` (AU-05 AC1).
6. Delete the mailbox's Needs Attention items: page `needs_attention().by_user` and delete those with this `mailbox_id`.
7. Remove the mailbox from the user state file: its Feed cursor entry and its label IDs on categories (`UserStateStore::update`, T-602b). If T-602b is not merged yet, leave a `// T-602b` note in the pull request and skip this step; T-602b adds it.
8. Revoke the refresh token at Google (`IdentityProvider::revoke`). A failed revoke is logged (`security_event` outcome `revoke_failed`) and deletion continues.
9. `mailboxes().delete(mailbox, Precondition::None)`: deletes the record (removes the encrypted refresh token and address).
10. Security event `mailbox_unlink`. Return `204`.

## Acceptance criteria

| ID | Behaviour (one line) |
| --- | --- |
| AU-05 AC1 | Disconnecting one of two mailboxes revokes its token, deletes it, drops its cards and cancels its queued jobs and tasks |
| AU-05 AC2 | Disconnecting the only mailbox returns `409 last_mailbox` and changes nothing |
| AU-05 AC3 | Without a fresh step-up the request returns `403 step_up_required` and changes nothing |
| AU-05 AC4 | Disconnecting the primary mailbox moves the app folder file first; if the move fails nothing is disconnected |
| DEL-3 | Disconnecting revokes the token at the provider and removes the mailbox document |

## Tests that must pass

- `au_05_ac1_disconnect_revokes_and_deletes` (service integration; `fake-google` records the revoke; store has no mailbox document)
- `au_05_ac1_disconnect_cancels_queued_jobs_and_tasks` (service integration; fake `JobScheduler` records the cancel by task name; advancing the virtual clock past `due_at` sends nothing)
- `au_05_ac1_disconnected_mailbox_cards_leave_feed` (service integration; needs T-602c, otherwise asserts the mailbox is absent from API-MBX-1)
- `au_05_ac1_running_job_left_alone` (service integration; a job already `running` is not changed)
- `au_05_ac2_last_mailbox_refused` (service integration)
- `au_05_ac3_disconnect_needs_step_up` (service integration; step-up 5 minutes and 1 second old is refused)
- `au_05_ac4_primary_move_copies_file_then_flips_primary` (service integration; bytes on the new Drive equal the old bytes; old copy deleted)
- `au_05_ac4_move_failure_disconnects_nothing` (service integration; inject a Drive write failure; mailbox still present, still primary, no revoke recorded)
- `au_05_ac4_old_copy_delete_failure_rolls_back` (service integration)
- `del_3_disconnect_revokes_and_removes_document` (service integration)
- `asvs_v8_2_2_disconnect_other_users_mailbox_not_found` (service integration)
- `inv_2_cancelled_job_keeps_outcome_and_expiry` (service integration: the cancelled record has the outcome, no target, and `expires_at` = now plus 30 days)
- `sw_05_ac5_cancel_loses_to_claim_cleanly` (service integration: the in-memory store's version changes between get and put; `cancel_queued_job` returns `NotQueued(Running)` and cancels no task)

## Edge cases and traps

- The cancel must be `put` with `Precondition::Matches(version)` from the same `get`. An unconditional write breaks the undo race (T-606) and the runner claim (T-701).
- Do the app folder move before anything destructive; revoke is last but one, because the move needs the old mailbox's token.
- Never revoke before the file has moved: after revoke the old Drive cannot be read.
- Cancel the Cloud Task only when the conditional put succeeded; otherwise a running job's task would be deleted under it.
- `JobScheduler::cancel` returning `AlreadyRunning` or `NotFound` is a normal outcome, not an error.
- The mailbox ID is a path parameter (allowed: it is a server UUID, not personal data). Never put the address in a log or error body.
- Use `Clock` for every timestamp; no `SystemTime::now()`.

## Out of scope

- Account deletion (all mailboxes): T-803.
- History entries for cancelled jobs: T-609 collects them at the next Feed load.

## Done when

- The tests above pass and every required check is green (S10 10.1).
- Definition of done in S10 10.4.
- `cancel_queued_job` is documented in the pull request so T-606 and T-803 reuse it unchanged.
