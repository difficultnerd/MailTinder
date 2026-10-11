# T-803: Delete account

| Milestone | Tier | Size | Depends on |
| --- | --- | --- | --- |
| M8 | strong | about 350 lines of code plus tests | T-302, T-504, T-601b, T-602b |

**Read only these spec sections:** S7 section 5.10 (API-ACCT-1) and section 6 (account deletion limit) in `docs/specs/S7-api-contract.md`; the `/account` path in `docs/specs/S7-api-contract.openapi.yaml`; S2 AU-06 AC1 to AC3; S5 "Deletion tests" (DEL-1, DEL-2) and the retention column for every Firestore row; S6 section 5 ("Deletion" paragraph); S10 section 6.3 row "Account deletion order" and section 7.2 row "Crypto". Nothing else is needed.

## Goal

`DELETE /api/v1/account` deletes a user's account in the order AU-06 AC1 requires: app folder file, then queued jobs and their Cloud Tasks, then provider tokens revoked, then the `data_key` destroyed (crypto-shredding) and every server record removed, with a sweep that finishes any leftovers within 24 hours. Labels already applied stay in the mailbox.

## Files

| Action | Path | What |
| --- | --- | --- |
| Create | `backend/crates/api/src/routes/account.rs` | Handler and DTO |
| Create | `backend/crates/api/src/services/account_deletion.rs` | Ordered steps |
| Create | `backend/crates/worker/src/sweeps/deleted_users.rs` | Orphan sweep for records whose user is gone |
| Change | `backend/crates/worker/src/sweeps/mod.rs` | The sweep entry point `run_sweeps` calls the deleted-user sweep; T-706 calls `run_sweeps` from API-INT-2 |
| Change | `backend/crates/api/src/routes/mod.rs` | Mount; Firestore rate limit 3 per user per day |
| Create | `backend/crates/api/tests/account_deletion.rs` | Service integration tests |
| Create | `backend/crates/worker/tests/deleted_users_sweep.rs` | Sweep tests |

## Types and signatures

```rust
// Used: require_step_up (T-504); cancel_queued_job (T-601b); TokenService::refresh_token (T-503);
// AppFolderStore (T-405); IdentityProvider::revoke (T-206 port); repos and delete_all_for_user (T-201b);
// ClassifierEvalRepo::delete_for_users and the user's pseudonymous ID (T-906a, T-307); security_event (T-307).

pub const DELETION_SWEEP_HOURS: i64 = 24;   // S2 AU-06 AC1

#[derive(Serialize)]
pub struct DeletionAccepted { pub deletion_due_by: OffsetDateTime, pub app_folders_not_deleted: Vec<NotDeleted> }
#[derive(Serialize)] pub struct NotDeleted { pub mailbox_id: Uuid, pub email_address: String }

/// Every call the deletion makes, in order. Tests compare this to the fakes' recorded order.
#[derive(Debug, PartialEq, Eq)]
pub enum DeletionStep { AppFolderDeleted, JobsCancelled, TokensRevoked, KeyDestroyed, RecordsDeleted, SessionsEnded }

pub async fn delete_account(app: &AppState, session: &AuthedSession) -> Result<(DeletionAccepted, Vec<DeletionStep>), ApiError>;

// worker
pub async fn sweep_deleted_users(store: &dyn ServerStore, limit: u32, pseudo: &Pseudonymiser) -> Result<u64, StoreError>;
```

## Algorithm

Before anything:

1. `require_step_up`; otherwise `403 step_up_required` and nothing is deleted (AU-06 AC3).
2. Rate limit (3 per day). Security event `account_delete_started`.
3. Load the user, the wrapped key and every mailbox. While the key still exists, decrypt in memory (as `Sensitive`) each mailbox's address and refresh token, and mint access contexts. Failures here are recorded per mailbox, not fatal.

In the request, in this order (AU-06 AC1, S7 5.10):

4. App folder: for the primary mailbox, `AppFolderStore::delete(ctx)`. Also call `delete` for every other mailbox (a stale copy from an interrupted move, AU-05 AC4; a missing file is `Ok`). Any mailbox whose delete failed or had no usable token goes into `app_folders_not_deleted` with the address decrypted in step 3.
5. Jobs: every `queued` job (`jobs().by_mailbox` for each mailbox): `cancel_queued_job` (conditional write, then the Cloud Task cancel). A job already `running` is left to finish; its runner finds no mailbox and no key and ends without sending mail (it cannot mint a token).
6. Tokens: `IdentityProvider::revoke` for each refresh token. A failure is logged (`revoke_failed`) and deletion continues.
7. Key: delete the user record with its `wrapped_data_key` (`users().delete`). From here no encrypted field of this user can be opened (crypto-shredding, DEL-2).
8. Records: `delete_all_for_user` on jobs, needs_attention, mailboxes and sessions; delete the user's `classifier_eval` records. Failures are logged; the sweep finishes them.
9. Sessions: `sessions().delete_all_for_user` (V7.4.2), clear the cookie and send `Clear-Site-Data: "cache", "storage"`.
10. Security event `account_deleted` (pseudonymous ID only). Return `202 { deletion_due_by: now + 24 hours, app_folders_not_deleted }`.

Worker sweep (every run of API-INT-2):

11. For jobs, needs_attention, mailboxes and sessions, find records whose `user_id` has no user document (in pages of `limit`, so live records at the front of a scan cannot hide an orphan), delete every collection of each orphaned user including the `classifier_eval` records of the user's pseudonymous ID, and return the count. Under the virtual clock, after a failed step 8 every record is gone by the next sweep, well within 24 hours (AU-06 AC1). The sweep is reached through the worker's API-INT-2 entry point, `worker::sweeps::run_sweeps`; **T-706 owns the production caller** (its route calls `run_sweeps`), so the "within 24 hours" clause of AU-06 AC1 only holds in production once T-706's `api_int_2_calls_the_deleted_user_sweep` test passes. Until then the sweep is a library function proven by the tests in `backend/crates/worker/tests/deleted_users_sweep.rs`.

Never touched: labels and categories already applied in the mailbox (AU-06 AC2), and `bakeoff_snapshots` (aggregate only, S5).

## Acceptance criteria

| ID | Behaviour (one line) |
| --- | --- |
| AU-06 AC1 | App folder file, then jobs and tasks, then token revoke, then key destroyed; remaining records swept within 24 hours (the "within 24 hours" clause is gated on T-706, see Done when) |
| AU-06 AC2 | Labels already applied stay on the user's messages |
| AU-06 AC3 | Without a fresh step-up nothing is deleted |
| DEL-1 | After deletion no document references the user ID |
| DEL-2 | A copy of an encrypted field taken before deletion cannot be decrypted after it |
| V7.4.2 | Deleting the account ends the user's session |

## Tests that must pass

- `au_06_ac1_deletion_order` (service integration: the Drive fake, the fake scheduler and `fake-google` record calls on one shared sequence; order is app folder, task cancels, revokes; the returned `DeletionStep` list matches)
- `au_06_ac1_provider_unreachable_still_deletes` (service integration: Drive fails; `202` lists that mailbox in `app_folders_not_deleted`; everything else deleted)
- `au_06_ac1_records_swept_within_24_hours` (worker test: inject a store failure in step 8, run the sweep under the virtual clock, no records remain)
- `au_06_ac1_queued_job_never_sends_after_deletion` (service integration: advance the virtual clock past `due_at`, run the T-606 stand-in: zero requests)
- `au_06_ac2_labels_remain` (service integration: filed messages keep their labels)
- `au_06_ac3_deletion_needs_step_up` (service integration: nothing deleted)
- `del_1_no_document_references_user` (service integration: scan every collection of the in-memory store)
- `del_2_old_ciphertext_unreadable_after_deletion` (service integration: copy a mailbox's encrypted refresh token before; after deletion `KeyService::open` cannot be called because the wrapped key is gone, and opening with any other user's key fails)
- `asvs_v7_4_2_deletion_ends_session` (service integration: the old cookie gets `401`)
- `ses_1_deleted_users_session_refused` (service integration)

## Edge cases and traps

- Decrypt the addresses and tokens before step 7; afterwards they are unreadable by design, and the response needs the addresses.
- Steps 4 to 6 must run before the key is destroyed; each needs a token or the key.
- Never revoke before the app folder delete: the delete needs the token.
- A provider failure never stops the deletion (S7 5.10); a store failure in steps 7 and 8 returns `202` anyway once the key is gone, because the sweep finishes the rest. If step 7 itself fails, return `503 provider_unavailable` and change nothing further (the user can retry; steps 4 to 6 are idempotent).
- Do not delete `bakeoff_snapshots`; they hold no per-user data.
- Addresses appear only in the response body, never in logs.
- Use `Clock` for `deletion_due_by`.

## Out of scope

- Admin deletion of another user: not in v1.
- Firestore backups: a backup taken before deletion still holds `wrapped_data_key`, which the shared KMS key can unwrap. The trial keeps no backups (James, 4 October 2026); this task proves DEL-2 against the live store: assert the wrapped key is gone from the live document and that decrypting a copy captured before deletion fails once the roughly one-hour historical-read window (ADR 0004) has passed; a test that cannot wait runs against the emulator and says so.

## Security review checklist

- Step-up is checked before any read of tokens or keys, and a stale `auth_time` (5 minutes and 1 second) is refused.
- The order app folder, jobs and tasks, revoke, key, records is enforced in code and proven by the shared-sequence test, not by comments.
- Job cancellation uses the same conditional write as undo (T-601b); no unconditional status write.
- No decrypted token or address is logged, put in an error body, or kept beyond the request.
- After step 7 nothing in the request tries to decrypt; the response addresses come from step 3.
- Every session of the user is deleted, the cookie cleared and `Clear-Site-Data` sent.
- The orphan sweep only deletes records whose user document is missing, and is bounded per run.
- The route is rate limited with a Firestore counter (3 per day).
- `classifier_eval` records for the user are deleted; snapshots are untouched.

## Done when

- The tests above pass and every required check is green (S10 10.1).
- Definition of done in S10 10.4.
- A strong-tier review ticks the checklist above before merge (S13 section 1).
- **AU-06 AC1's 24-hour clause is not done here.** This task ships the sweep and its tests; the production caller lands with T-706. T-803's AC coverage stays conditional on T-706's hard gate, `api_int_2_calls_the_deleted_user_sweep`, which must pass on the API-INT-2 route.
