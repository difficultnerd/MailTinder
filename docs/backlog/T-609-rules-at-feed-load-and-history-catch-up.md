# T-609: Rules applied at Feed load and History catch-up

| Milestone | Tier | Size | Depends on |
| --- | --- | --- | --- |
| M6 | sonnet | about 300 lines of code plus tests | T-405, T-602c, T-608 |

**Read only these spec sections:** S7 section 5.4 (what API-FEED-1 changes) in `docs/specs/S7-api-contract.md`; S2 SR-01 AC2 to AC4, PB-01 AC2, FL-04 AC2, UN-01 AC3; S3 `UnsubscribeJob` row (terminal retention), "Card visibility", INV-4; S5 `jobs/{id}` row and JOB-1; S10 section 6.3 row "App folder writes (option B)" and "Every outcome recorded". Nothing else is needed.

## Goal

Each Feed load (1) applies the user's enabled rules to freshly fetched mail, trashing or filing matches, recording History and keeping them off the Feed, and (2) collects the outcomes of finished unsubscribe jobs into History exactly once, then deletes those job records. T-707's delivery check hooks into the same step.

## Files

| Action | Path | What |
| --- | --- | --- |
| Create | `backend/crates/api/src/services/rule_actions.rs` | `apply_rules` (replaces T-602c's no-op) |
| Create | `backend/crates/api/src/services/history_catch_up.rs` | `collect_job_outcomes` |
| Change | `backend/crates/api/src/services/feed.rs` | Call both; count `rule_actions_applied` |
| Create | `backend/crates/api/tests/feed_rules_and_history.rs` | Service integration tests |

## Types and signatures

```rust
// Used from earlier tasks: first_match, RuleAction (T-104); StoredRule, HistoryEntry, PendingUnsubscribe,
// PendingDeliveryCheck, Totals (T-602b); JobRecord, JobOutcomeCode, JobStatus (T-201b, T-106);
// RuleApplication and the apply_rules signature (T-602c); ensure_label_for (T-604).

pub const NS_RULE_ACTION: Uuid = Uuid::from_u128(0x6d74_7261_0000_4000_8000_000000000001); // [DEFAULT]
pub const MAX_RULE_ACTIONS_PER_LOAD: u32 = 200;   // [DEFAULT] bounds a runaway rule (S6 T13); the rest wait for the next load

pub struct CollectedOutcomes { pub entries: Vec<HistoryEntry>, pub job_ids: Vec<JobId>,
    pub sent: Vec<(JobId, PendingUnsubscribe, OffsetDateTime)> }

pub async fn collect_job_outcomes(app: &AppState, user: &UserId, state: &UserState) -> Result<CollectedOutcomes, ApiError>;
pub fn history_outcome(status: JobStatus) -> Option<HistoryOutcome>;
//   Sent -> Sent, NeedsAttention -> NeedsAttention, Failed -> Failed, Expired -> Expired, Cancelled -> Cancelled,
//   Queued | Running -> None (not finished). Exhaustive match, no `_`.
```

## Algorithm

Rules (inside T-602c step 9, for the metas kept on the page):

1. Enabled rules only (SR-01 AC4). For each meta: `first_match(&rules, meta)` (T-104 handles List-Id, sender-only with List-Unsubscribe and Feedback-ID, SR-01 AC3; trash rules win over file rules).
2. `RuleAction::Trash` (reject_list and block_person, SR-01 AC2, PB-01 AC2): `trash(ctx, id)`. `RuleAction::File` (FL-04 AC2): `ensure_label_for`, then `set_labels(add {label}, remove {INBOX})`.
3. Provider failure on one message: skip it, keep it off this page (it is retried next load), no History entry.
4. Each success: History `{ entry_id: v5(NS_RULE_ACTION, user ++ mailbox ++ message_id ++ rule_id), action: trashed_by_rule | filed_by_rule, outcome: done, rule_id, sender_display }`; `times_applied += 1`; `totals.cleared += 1`. The deterministic ID makes a repeat (two Feed loads racing) a no-op in `push_history`.
5. Stop after `MAX_RULE_ACTIONS_PER_LOAD`; unmatched metas carry on to card building.

History catch-up (before the Feed's single `UserStateStore::update`):

1. `jobs().by_user_with_outcome(user, 500)`: terminal jobs with an outcome.
2. For each: `history_outcome(status)`; build `HistoryEntry { entry_id: job_id, at: outcome.at, mailbox_id, action: unsubscribe, outcome, rule_id, sender_display }` with `sender_display` and `rule_id` from `pending_unsubscribes[job_id]` (missing: "Unknown sender" `[DEFAULT]`, `rule_id` null).
3. Inside the Feed's update closure: `push_history` each (a duplicate entry ID is ignored, so two racing loads append once, S10 6.3); remove the `pending_unsubscribes` entries; for `Sent`, `totals.senders_unsubscribed += 1` only when the entry was newly pushed, and push a `PendingDeliveryCheck { sender_key, list_id, mailbox_id, unsubscribed_at: outcome.at }` (T-707 adds its fields and the check that runs here).
4. After the update succeeded: `jobs().delete(job_id, Precondition::None)` for each collected job. A delete failure is logged and retried next load (the entry ID makes the append idempotent).
5. Never delete a job before its History entry is written.

## Acceptance criteria

| ID | Behaviour (one line) |
| --- | --- |
| SR-01 AC2 | A new message matching a reject rule is trashed at fetch, recorded in History and never shown |
| SR-01 AC3 | Rule matching keeps receipts without List-Unsubscribe and other List-Ids out |
| SR-01 AC4 | A disabled rule acts on nothing |
| PB-01 AC2 | A block rule trashes future mail from that address |
| FL-04 AC2 | A filing rule labels matching mail and records `filed_by_rule` |
| UN-01 AC3 | Each job outcome is appended to History at the next Feed load |
| INV-4 | Every automated change to a mailbox has a matching History entry |
| JOB-1 | A collected terminal job is deleted after its outcome is in History |

## Tests that must pass

- `sr_01_ac2_matching_mail_trashed_and_hidden` (service integration: card absent, message trashed, History entry, `rule_actions_applied` 1)
- `sr_01_ac3_receipt_without_header_survives` (service integration, corpus "same sender, receipt without List-Unsubscribe")
- `sr_01_ac3_other_list_id_not_matched` (service integration)
- `sr_01_ac4_disabled_rule_acts_on_nothing` (service integration)
- `pb_01_ac2_block_rule_trashes_future_mail` (service integration)
- `fl_04_ac2_filing_rule_applied_at_feed` (service integration)
- `un_01_ac3_outcomes_appended_once` (service integration: sent, needs_attention, expired and cancelled jobs give four entries with outcomes sent, needs_attention, expired, cancelled)
- `un_01_ac3_racing_feed_loads_append_once` (service integration: two loads interleaved through the Drive fake's ETag conflict)
- `inv_4_every_rule_action_has_history` (property: any generated rules and inbox; every provider change recorded by `FakeMailbox` has a History entry)
- `job_1_collected_job_deleted_after_history` (service integration; also: a failed History write leaves the job in place)
- `inv_5_rule_actions_never_delete` (service integration)

## Edge cases and traps

- Provider changes happen before the user state update and outside its closure; the closure only appends entries and counts.
- Use the deterministic entry IDs; random IDs break the "exactly once" guarantee when loads race.
- Map job `NeedsAttention` to History `needs_attention`, never `failed`: S7 API-HIST-1 keeps them apart so the user knows to look in Needs Attention.
- Spoofed mail that matches a rule is still trashed (reversible, recorded); rule creation, not matching, is where authentication is checked.
- Never log sender or message ID.

## Out of scope

- The UN-06 delivery check and confirmed unsubscribes: T-707 (it extends step 3 of the catch-up and the rule step).
- History endpoint: T-801.

## Done when

- The tests above pass and every required check is green (S10 10.1).
- Definition of done in S10 10.4.
