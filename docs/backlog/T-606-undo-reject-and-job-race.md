# T-606: Undo of a reject and the race with the job

| Milestone | Tier | Size | Depends on |
| --- | --- | --- | --- |
| M6 | sonnet | about 300 lines of code plus tests | T-105b, T-601b, T-605 |

**Read only these spec sections:** S7 section 5.5 (API-SW-2) in `docs/specs/S7-api-contract.md`; S2 SW-05 AC1 to AC5 and AC4a, UN-01 AC1 and AC3; S3 `UnsubscribeJob` state machine and "Swipe and undo"; S10 section 6.3 rows "Undo before due time", "Undo after send", "Undo racing the run", "Duplicate delivery". Nothing else is needed.

## Goal

`POST /api/v1/swipes/undo` reverses a reject: it cancels the queued job if it can, restores the exact previous labels (also after a spam report), removes the rule and Needs Attention item the swipe created and reverses the counts. When undo and the job's run meet at the due time, exactly one wins: the cancel (nothing sent) or the send (undo says it already went). Never both, never neither.

## Files

| Action | Path | What |
| --- | --- | --- |
| Change | `backend/crates/api/src/services/undo.rs` | `undo_reject` |
| Create | `backend/crates/api/tests/undo_reject.rs` | Service integration tests |
| Create | `backend/crates/api/tests/undo_race.rs` | Property test of the race |
| Create | `backend/crates/testkit/src/runner_standin.rs` | `claim_and_send`: test stand-in for the T-701 claim, same conditional write |

## Types and signatures

```rust
// Used from earlier tasks: UndoPayload (T-604); SwipeRecord, plan_undo, reverse_stats, JobCancelOutcome,
// undo_response (T-105b); cancel_queued_job, CancelResult (T-601b); JobStatus (T-106).

pub async fn undo_reject(app: &AppState, session: &AuthedSession, payload: &UndoPayload)
    -> Result<UndoResponse, ApiError>;

/// Maps the store result to T-105b's outcome. The only place this mapping lives.
pub fn cancel_outcome(r: &CancelResult) -> JobCancelOutcome;
//   Cancelled                         -> Cancelled
//   NotQueued(Cancelled)              -> Cancelled             (an earlier undo attempt cancelled it, then failed later)
//   NotQueued(Running | Sent | NeedsAttention) -> AlreadySent  (the runner claimed it; a request may have gone)
//   NotQueued(Failed | Expired)       -> AlreadyFinishedNotSent
//   NotQueued(Queued)                 -> unreachable by construction; treat as AlreadySent and log
//   Missing                           -> AlreadySent           (collected by a Feed load after it ran)

// testkit/src/runner_standin.rs (tests only)
/// get, then put(status Running, Precondition::Matches(version)); on success one call to the egress fake,
/// then put(status Sent). Returns whether it sent. Uses the same conditional write T-701 must use.
pub async fn claim_and_send(store: &dyn ServerStore, egress: &RecordingEgress, job: &JobId) -> bool;
```

## Algorithm

`undo_reject(payload)`, in this order:

1. If `record.job_id` is `Some`: `cancel_queued_job(job_id)` (T-601b: conditional `Queued` to `Cancelled` on the read version, then cancel the Cloud Task). Map with `cancel_outcome`. Cancel first, so the cancel has the best chance of beating the due time.
2. Restore labels: `restore_labels(ctx, id, exact previous_labels)`. This also lifts the spam label after a `reported_spam` (SW-05 AC4a). Failure: `502 provider_error`; return now. The job (if cancelled) stays `Cancelled`, so a retry maps `NotQueued(Cancelled)` back to `Cancelled` and reports the same answer.
3. If `na_item` is `Some`: `needs_attention().delete(item, Precondition::None)` `[DEFAULT]` (the item belonged to this swipe).
4. One `UserStateStore::update`:
   - remove the rule `record.rule_id` if present (both when cancelled and when already sent, SW-05 AC2, AC3);
   - `reverse_stats(record, stats)`; reverse `totals` (`triaged`, `cleared`, and for a job `unsubscribes_queued`, `round_unsubscribes`; for a removed rule `senders_silenced`); clear `boss_defeated` if this swipe set it;
   - remove the `reported_spam` History entry if `history_entry` is set;
   - when the outcome is `Cancelled`: remove `pending_unsubscribes[job_id]` and push History `{ entry_id: job_id, action: unsubscribe, outcome: cancelled, rule_id }` (UN-01 AC3; `push_history` ignores a duplicate);
   - drop the `RecentSwipe`.
5. When the outcome is `Cancelled`: `jobs().delete(job_id, Precondition::None)` (S10 6.3 "job deleted"). This is last so every earlier failure leaves a retryable `Cancelled` record.
6. Return `undo_response(outcome)`: `unsubscribe_already_sent` is true only for `AlreadySent`. Security event `undo` with the outcome code (no IDs beyond pseudonymous user).

The race, in words: the runner's claim and undo's cancel both write the job conditionally on the version they read while it was `Queued`. Firestore lets one of them through; the other gets `PreconditionFailed` and re-reads. If the cancel wins, the runner's claim fails and it sends nothing (T-701 returns `200` without acting). If the claim wins, the cancel sees `Running` or later and undo reports the request went.

## Acceptance criteria

| ID | Behaviour (one line) |
| --- | --- |
| SW-05 AC1 | Undo of a reject restores the exact previous labels and the card returns |
| SW-05 AC2 | Before the due time undo cancels the job and task and removes the rule; nothing is ever sent |
| SW-05 AC3 | After the send undo restores the message, removes the rule and reports it already went |
| SW-05 AC4 | Undo of rejects works back through the session one per call |
| SW-05 AC4a | Undo of a spam report restores the location and the client is told the report stays |
| SW-05 AC5 | Undo at the due time gives exactly one outcome: cancelled with no request, or sent with undo saying so |
| UN-01 AC1 | A job runs at most once, also when undo and duplicate deliveries interleave |
| UN-01 AC3 | A cancelled unsubscribe is recorded in History |
| INV-6 | Every reject in the session can be undone |

## Tests that must pass

- `sw_05_ac1_undo_reject_restores_exact_labels` (service integration)
- `sw_05_ac2_undo_before_due_cancels_job_task_and_rule` (service integration: task deleted on the fake scheduler, job deleted, rule gone; advance the virtual clock past `due_at` and run the stand-in: zero requests at the egress fake)
- `sw_05_ac3_undo_after_send_reports_already_sent` (service integration: stand-in runs first; response `unsubscribe_already_sent: true`; rule removed; message restored)
- `sw_05_ac3_undo_after_feed_collected_job_reports_sent` (service integration: job record missing)
- `sw_05_ac4_undo_walks_back_two_rejects` (service integration)
- `sw_05_ac4a_undo_spam_restores_location` (service integration: spam label gone, previous labels exact)
- `sw_05_ac5_undo_racing_run_exactly_one_outcome` (property, `proptest`: for generated interleavings of undo's get and put and the stand-in's get, put, send and put, with duplicate deliveries, assert `(requests == 0 && !already_sent && status cancelled)` or `(requests == 1 && already_sent)`)
- `sw_05_ac5_undo_retry_after_restore_failure_same_answer` (service integration: restore fails once; retry returns `already_sent: false` and the job is deleted)
- `un_01_ac1_duplicate_delivery_after_cancel_sends_nothing` (service integration)
- `un_01_ac3_cancel_written_to_history_once` (service integration: two undo attempts give one History entry)
- `inv_6_every_reject_undoable` (property: any mix of reject classes then undo of each restores labels, rules and counts)
- `asvs_v9_2_1_undo_token_from_earlier_session_gone` (service integration: `410 undo_expired`)

## Edge cases and traps

- Never cancel with an unconditional write, and never "check status then delete": both reopen the race.
- Do not delete the Cloud Task unless the conditional cancel succeeded.
- Do not delete the job before the labels are restored; a retry needs the `Cancelled` record to give the right answer.
- Remove only the rule this swipe created (`record.rule_id` is `None` when an identical rule already existed).
- The stand-in lives in `testkit` (dev only) and must use `Precondition::Matches`; T-701 must keep the same write.
- No sleeps in the property test; drive the interleaving with explicit steps and the virtual clock.

## Out of scope

- The runner itself and batching: T-701 to T-703.
- Collecting other job outcomes into History: T-609.

## Done when

- The tests above pass and every required check is green (S10 10.1).
- Definition of done in S10 10.4.
