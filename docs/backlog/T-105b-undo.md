# T-105b: Undo plan and the undo stack

| Milestone | Tier | Size | Depends on |
| --- | --- | --- | --- |
| M1 | sonnet | about 250 lines of code plus property tests | T-105a |

Split from index row T-105 ("Swipe effects by class and the undo stack"); the swipe planner is T-105a.

**Read only these spec sections:** S3 "Swipe and undo" state machine, "Transient" table and INV-6 (`docs/specs/S3-domain-model.md`); S2 SW-05 (all ACs) (`docs/specs/S2-v1-acceptance-criteria.md`); S7 5.5 API-SW-2 (`docs/specs/S7-api-contract.md`); S10 6.3 rows "Undo before due time" and "Undo after send". Nothing else is needed.

## Goal

The domain gets the record of one swipe (`SwipeRecord`, the payload T-604 seals into the undo token), the pure plan that reverses it exactly (restore the previous label set, cancel the job, remove the rule, reverse the stats), the response rules for a job that already ran, and a small LIFO `UndoStack`. A property test proves INV-6: every swipe in a session can be reversed, back to the first, restoring exact label sets.

## Files

| Action | Path | What |
| --- | --- | --- |
| Create | `backend/crates/domain/src/undo.rs` | `SwipeRecord`, `UndoPlan`, `plan_undo`, `JobCancelOutcome`, `UndoResponse`, `undo_response`, `UndoStack` |
| Change | `backend/crates/domain/src/lib.rs` | `pub mod undo; pub use undo::*;` |

## Types and signatures

```rust
#[derive(Clone, PartialEq, Serialize, Deserialize)]   // Debug by hand: redact message ID and sender
pub struct SwipeRecord {
    pub mailbox: MailboxId,
    pub message: MessageId,
    pub sender: SenderKey,
    pub action: SwipeAction,
    pub outcome: SwipeOutcome,
    pub previous_labels: LabelSet,           // exact provider labels before the swipe
    pub job_id: Option<JobId>,
    pub rule_id: Option<RuleId>,
    pub counted_reject_at: Option<OffsetDateTime>,  // set when the swipe added to rejects_counted
    pub at: OffsetDateTime,
}
impl SwipeRecord { pub fn from_plan(plan: &SwipePlan, meta: &MessageMeta, action: SwipeAction, at: OffsetDateTime) -> SwipeRecord; }

#[derive(Clone, Debug, PartialEq)]
pub struct UndoPlan {
    pub restore_labels: Option<LabelSet>,    // Some for every swipe that changed the mailbox; exact previous set
    pub cancel_job: Option<JobId>,
    pub remove_rule: Option<RuleId>,
    pub spam_report_not_recalled: bool,      // SW-05 AC4a
}
pub fn plan_undo(record: &SwipeRecord) -> UndoPlan;
pub fn reverse_stats(record: &SwipeRecord, stats: &mut SenderStats);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JobCancelOutcome { NoJob, Cancelled, AlreadySent, AlreadyFinishedNotSent }

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct UndoResponse { pub restored: bool, pub unsubscribe_already_sent: bool }
pub fn undo_response(cancel: JobCancelOutcome) -> UndoResponse;

#[derive(Clone, Debug, Default)]
pub struct UndoStack<T> { items: Vec<T> }
impl<T> UndoStack<T> {
    pub fn new() -> Self;
    pub fn push(&mut self, item: T);
    pub fn pop(&mut self) -> Option<T>;
    pub fn len(&self) -> usize;
    pub fn is_empty(&self) -> bool;
}
```

## Algorithm

1. `SwipeRecord::from_plan`: copy IDs from the plan (`unsubscribe.job_id`, `rule.rule_id`), `previous_labels = meta.labels.clone()`, `counted_reject_at = Some(at)` when `plan.stats_after.rejects_counted.len()` grew or its last entry is `at`, else `None`.
2. `plan_undo(record)`:
   - `restore_labels = Some(record.previous_labels.clone())` when the outcome changed the mailbox (every outcome except `Kept` and `Skipped`); else `None`.
   - `cancel_job = record.job_id`; `remove_rule = record.rule_id` (SW-05 AC2, AC3: the rule is removed whether or not the job already ran).
   - `spam_report_not_recalled = record.outcome == SwipeOutcome::ReportedSpam` (SW-05 AC4a).
3. `reverse_stats(record, stats)`: `Keep` gives `keeps = keeps.saturating_sub(1)`; `File { category }` decrements `files[category]` (remove the entry at 0); a reject with `counted_reject_at = Some(at)` calls `stats.unrecord_last_reject(at)`; `Skip` changes nothing here (skip counts live in T-602b's `SkipState`, reversed by T-604). Never touch `block_prompt_declined_until`.
4. `undo_response(cancel)`: `restored` is always true (the caller only calls this after the labels were restored; a provider refusal is `502` with the token still valid, S7 5.5). `unsubscribe_already_sent` is true only for `AlreadySent` (SW-05 AC3). The race itself (SW-05 AC5) is decided by T-106's conditional transition; this function only maps its result.
5. `UndoStack`: plain LIFO; `pop` on empty gives `None` (SW-05 AC4: one swipe per tap, back through every swipe of the session).
6. **INV-6 property harness** (tests only, `#[cfg(test)]`): a test-local model of a mailbox, `BTreeMap<MessageId, LabelSet>` with Gmail-like rules: `Trash` adds `TRASH` and removes `INBOX`; `ReportSpamAndTrash` adds `SPAM` and `TRASH` and removes `INBOX`; `ApplyCategory` adds the category's label ID and removes `INBOX`; restore replaces the set exactly. Generate 1 to 30 swipes over 1 to 5 messages with arbitrary starting label sets (always including `INBOX`), run `plan_swipe` (T-105a), apply, push records; then pop and apply every `plan_undo` and `reverse_stats`. Assert every message's label set and the `SenderStats` equal the starting values.

## Acceptance criteria

| ID | Behaviour (one line) |
| --- | --- |
| SW-05 AC1 | Undo restores the exact previous labels and location |
| SW-05 AC2 | Undo of a queued unsubscribe cancels the job and removes the rule |
| SW-05 AC3 | Undo after the unsubscribe was sent still restores and removes the rule, and reports it already went |
| SW-05 AC4 | Undo works back through every swipe of the session, one per tap |
| SW-05 AC4a | Undo of a spam report restores the message and says the report cannot be recalled |
| INV-6 | Every swipe in the current session can be reversed by undo |

## Tests that must pass

- `sw_05_ac1_undo_restores_exact_previous_labels` (unit)
- `sw_05_ac2_undo_cancels_job_and_removes_rule` (unit)
- `sw_05_ac3_undo_after_send_reports_already_sent` (unit)
- `sw_05_ac3_rule_removed_even_when_sent` (unit)
- `sw_05_ac4_undo_walks_back_every_swipe` (property: the stack pops in reverse push order and is empty after N pops)
- `sw_05_ac4a_spam_undo_says_not_recalled` (unit)
- `inv_6_every_swipe_reversible` (property, harness in step 6)
- `undo_keep_and_skip_restore_nothing_in_mailbox` (unit)
- `undo_reverses_counted_reject_only_once` (unit)

## Edge cases and traps

- Restore the stored `previous_labels`, never a guessed set ("remove TRASH, add INBOX"). S3: "Reversal must restore the exact previous label set, not a guessed one."
- The Gmail-like model uses label strings only inside test code. Production `domain` code never names `INBOX`, `TRASH` or `SPAM` (INV-7; those IDs are the adapter's business).
- `SwipeRecord` holds a message ID and sender: `Debug` by hand, redacted. It is serialised into a sealed token (T-303), so it derives `Serialize` and `Deserialize`.
- Stats reversal must not underflow: use `saturating_sub`.
- No undo state is kept on the server (S7 5.5); the `UndoStack` is for the domain property tests and any in-process client. The browser's stack is Dart (T-1002).

## Out of scope

- Sealing the record into the undo token: T-303 and T-604. Calling the provider and Cloud Tasks: T-604, T-606. The job race (SW-05 AC5): T-106 and T-606.

## Done when

- The tests above pass and every required check is green (S10 10.1).
- Definition of done in S10 10.4.
