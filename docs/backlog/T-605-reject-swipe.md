# T-605: Reject swipe: trash, rules, block prompts and queued unsubscribe

| Milestone | Tier | Size | Depends on |
| --- | --- | --- | --- |
| M6 | sonnet | about 450 lines of code plus tests | T-104, T-106, T-109, T-304, T-406, T-601b, T-602a, T-604 |

**Read only these spec sections:** S7 section 5.5 (outcome table, idempotency paragraph, `prompts`, `boss_defeated`) and section 5.8 (`link` rules) in `docs/specs/S7-api-contract.md`; S2 SW-03 AC1 to AC5, SR-01 AC1, AC1a, AC6, PB-01 AC1 and AC4, UN-01 AC4, UN-02 AC2, UN-04 AC6, GM-05 AC1 and AC3, GM-08 AC3, CL-01 AC2; S3 "Message classes", "Rule matching and counting", `UnsubscribeJob` row; S5 `jobs/{id}` row and JOB-1; S10 section 6.3 rows "Reject queues job" and "Undo racing the run". Nothing else is needed.

## Goal

The `reject` arm of API-SW-1. It carries out T-105a's `SwipePlan`: trash (or report spam and trash), create the reject rule, raise the https-only Needs Attention item, queue the unsubscribe job and its Cloud Task, count personal rejects and offer the block prompt, store the mail-stopped estimate and defeat a boss. Every write is idempotent on the `Idempotency-Key`, and the job is created so that exactly one of the runner's claim (T-701) and undo's cancel (T-606) can win.

## Files

| Action | Path | What |
| --- | --- | --- |
| Create | `backend/crates/api/src/services/reject.rs` | `execute` and its steps |
| Create | `backend/crates/api/src/services/list_key.rs` | `list_key_hash` |
| Change | `backend/crates/api/src/services/swipe.rs` | Call `reject::execute` from the `reject` arm |
| Change | `backend/crates/api/src/routes/mod.rs` | Firestore rate limit: 300 queued jobs per user per day (S7 section 6) |
| Create | `backend/crates/api/tests/swipes_reject.rs` | Service integration tests |

## Types and signatures

```rust
// Used from earlier tasks: SwipePlan, MailboxChange, UnsubscribePlan, UnsubscribeTarget, ManualUnsubscribePlan,
// SwipeOutcome (T-105a); SortRule (T-104); JobRecord, JobOutcome, NeedsAttentionRecord, Precondition,
// aad_fields (T-201b; JOB_SENDER_DISPLAY added by T-701); task_name_for (T-601b);
// yearly_rate (T-109); is_boss, defeat_boss_on_reject (T-108); UndoPayload, swipe_id (T-604);
// StoredRule, PendingUnsubscribe (T-602b); MessageQuery, count_messages (T-602a).

pub const UNSUB_DELAY_MINUTES: i64 = 5;      // S2 UNSUB_DELAY [TUNABLE], from Tunables
pub const JOB_TTL_MINUTES: i64 = 60;         // S2 JOB_TTL
pub const NEEDS_ATTENTION_TTL_DAYS: i64 = 30; // S2 NEEDS_ATTENTION_TTL
pub const GM05_WINDOW_DAYS: i64 = 90;        // S2 GM-05 AC1
pub const JOBS_PER_DAY: u32 = 300;           // S7 section 6 [TUNABLE]
pub const NS_NA_ITEM: Uuid = Uuid::from_u128(0x6d74_6e61_0000_4000_8000_000000000001); // [DEFAULT]

#[derive(Serialize, Deserialize)]
pub struct BlockPromptRef { pub sender_key: String, pub sender_display: String, pub mailbox_id: Uuid, pub swipe_id: Uuid }

pub async fn execute(app: &AppState, session: &AuthedSession, ctx: &MailboxCtx, meta: &MessageMeta,
    plan: &SwipePlan, swipe_id: Uuid) -> Result<SwipeResultDto, ApiError>;

// services/list_key.rs
/// HMAC-SHA-256 under the email lookup HMAC key (Secret Manager, `api` only), input
/// "list:" + sender_key + "\n" + list_id_or_empty. [DEFAULT] S5 says "list key HMAC" without naming the key.
pub fn list_key_hash(key: &HmacKey, sender: &SenderKey, list_id: Option<&str>) -> ListKeyHash;
```

## Algorithm

`execute(meta, plan)`, after T-604's steps 1 to 6:

1. Rate limit: when `plan.unsubscribe` is `Some`, check the Firestore counter (300 per user per day). Over the limit: `429 rate_limited` before any provider change.
2. Mailbox change (SW-03 AC1, AC3):
   - `Trash`: `trash(ctx, id)`; the returned set is `previous_labels`.
   - `ReportSpamAndTrash` (class `suspect`): `report_spam`, then `trash`; `previous_labels` is the set `report_spam` returned. No rule, no job, no counting.
   - Provider error: `502` or `503`, nothing else done.
3. Queued unsubscribe, when `plan.unsubscribe` is `Some(UnsubscribePlan { job_id, method, target, due_at })` (SW-03 AC2, UN-02 AC2: T-105a only plans one when T-406's DKIM cover holds):
   1. Build `JobRecord { job_id, user_id, mailbox_id, list_key_hash, method, target: seal(target string, aad_fields::JOB_TARGET, scope job_id), sender_display: seal(display), due_at, status: Queued, attempts: 0, outcome: None, expires_at: due_at + JOB_TTL }`. No access token anywhere (UN-01 AC4, JOB-1).
   2. `jobs().put(&record, Precondition::MustNotExist)`; `AlreadyExists` is a retry: success.
   3. `JobScheduler::schedule(job_id, due_at)` with the name `task_name_for(job_id)`; an already-exists answer is success.
   4. If 3.2 or 3.3 fails for another reason: delete the job (`Precondition::None`), `restore_labels(previous_labels)` to undo the trash, return `503 provider_unavailable`. Never leave a job without the trash, or the trash without the job the toast promised.
4. Manual unsubscribe, when `plan.manual_unsubscribe` is `Some` (UN-04 AC6): `NeedsAttentionRecord { item_id: v5(NS_NA_ITEM, user ++ key), reason: https_only_unsubscribe, link: only if the scheme is https, sender_display, expires_at: now + 30 days }`, `put(MustNotExist)`, `AlreadyExists` is success.
5. Mail-stopped estimate (GM-05 AC1, AC3), when `plan.rule` is `Some` or a job was queued: `count_messages({ from: sender, list_id, after: now - 90 days })` (all folders, one call); `yearly_rate(result.ok(), 90 days)`. A failed count gives `None` and the rule is still created.
6. Boss: `was_boss = is_boss(sender_key, &state.sender_stats, &tunables)` read before the update.
7. Block prompt (PB-01 AC1): when `plan.block_prompt`, seal `BlockPromptRef` as `TokenType::PromptRef` (12 hours) and add `{ type: "block_person", prompt_ref, sender_name }`.
8. One `UserStateStore::update`:
   - `sender_stats[sender] = plan.stats_after` (T-104 counted the reject only for authenticated personal mail, PB-01 AC4, SR-01 AC6), then `defeat_boss_on_reject(stats, was_boss)` gives `boss_defeated` (GM-08 AC3).
   - If `plan.rule` is `Some` and no enabled rule with the same match exists, push `StoredRule { rule, times_applied: 0, yearly_rate }` and `totals.senders_silenced += 1`. If an identical rule exists, do not add one and leave `rule_id` out of the undo record (undo must not remove a rule this swipe did not create).
   - Job queued: `pending_unsubscribes[job_id] = PendingUnsubscribe { mailbox_id, sender_display, sender_key, list_id, rule_id, created_at }`, `totals.unsubscribes_queued += 1`, `round_unsubscribes` (reset first if `round_session` differs from the session).
   - `suspect`: History `{ entry_id: swipe_id, action: reported_spam, outcome: done }`.
   - `totals.triaged += 1`, `totals.cleared += 1`; push `RecentSwipe`.
9. Undo payload: `SwipeRecord::from_plan` with `previous_labels`, `job_id`, `rule_id` (only if created), plus `na_item` and `history_entry`. Seal and return with `outcome = plan.outcome`, `unsubscribe_due_at` = `due_at` when queued.

## Acceptance criteria

| ID | Behaviour (one line) |
| --- | --- |
| SW-03 AC1 | Reject moves the message to trash; nothing is permanently deleted |
| SW-03 AC2 | A list with one-click or mailto gets a job due now plus `UNSUB_DELAY`, a task and a reject_list rule |
| SW-03 AC3 | A suspect message is reported as spam and trashed; no job |
| SW-03 AC4 | A personal message is only trashed and the reject is counted for PB-01 |
| SW-03 AC5 | No List-Unsubscribe header: trash and reject_list rule, no unsubscribe attempt |
| SR-01 AC1 | The rule is keyed on sender plus List-Id, or sender alone without one |
| SR-01 AC1a | A relayed message's rule uses the original sender |
| SR-01 AC6 | Mail without DKIM-aligned From or provider pass is trashed but creates no rule and counts nothing |
| PB-01 AC1 | After `PERSONAL_BLOCK_THRESHOLD` counted rejects the next reject returns a `block_person` prompt |
| PB-01 AC4 | Unauthenticated personal rejects never count and never prompt |
| UN-01 AC4 | The job record holds no access token |
| UN-02 AC2 | Without DKIM cover no job of any method is created |
| UN-04 AC6 | An https-only header gives trash, a rule and a Needs Attention item with the header link only |
| CL-01 AC2 | Whatever the token says, a message without DKIM-covered List-Unsubscribe never gets a job |
| GM-05 AC1 | The rule stores the 90-day count scaled to a year, from one count query |
| GM-05 AC3 | A failed count still creates the rule with an unknown rate |
| GM-08 AC3 | Rejecting a boss returns `boss_defeated: true` and removes it from the boss list |
| INV-2 | The job has `expires_at` = `due_at` plus `JOB_TTL` |
| JOB-1 | No job document holds an access token |
| SW-05 AC5 | A queued job is claimed or cancelled exactly once (conditional writes on the job version) |

## Tests that must pass

- `sw_03_ac1_reject_trashes_message` (service integration)
- `sw_03_ac2_reject_queues_unsubscribe_with_delay` (service integration: job record, task named after `job_id` at now plus 5 minutes on the fake scheduler, rule present)
- `sw_03_ac3_suspect_reported_and_trashed_no_job` (service integration)
- `sw_03_ac4_personal_trashed_and_counted` (service integration)
- `sw_03_ac5_no_header_rule_no_job` (service integration, corpus "bulk look-alike with body unsubscribe link")
- `sr_01_ac1_rule_keyed_on_list_id` (service integration)
- `sr_01_ac1a_relay_rule_uses_original_sender` (service integration)
- `sr_01_ac6_spoofed_sender_no_rule` (service integration)
- `pb_01_ac1_block_prompt_after_threshold` (service integration)
- `pb_01_ac4_unauthenticated_rejects_never_prompt` (service integration: three spoofed rejects, no prompt)
- `un_01_ac4_job_holds_no_access_token` (service integration: scan the stored job document)
- `un_02_ac2_no_dkim_cover_no_job` (service integration, corpus "one-click headers not in DKIM h=")
- `un_04_ac6_https_only_raises_item_with_header_link` (service integration; a `http://` link is dropped)
- `cl_01_ac2_token_claiming_list_cannot_create_job` (service integration: a forged-content token is refused at open; a valid token for a `bulk_no_header` message still gives no job)
- `gm_05_ac1_rule_stores_yearly_rate` (service integration)
- `gm_05_ac3_count_failure_rule_still_created` (service integration)
- `gm_08_ac3_reject_defeats_boss` (service integration)
- `inv_2_job_has_expiry` (service integration)
- `job_1_no_access_token_on_job` (service integration)
- `sw_05_ac5_new_job_claimed_once` (property: two conditional writes from `Queued` with the same version, one to `Running` and one to `Cancelled`; exactly one succeeds)
- `asvs_v2_3_4_retried_reject_one_job_one_rule` (service integration: same key twice; one job, one task, one rule)
- `asvs_v2_3_2_job_rate_limit_before_trash` (service integration: the 301st reject of the day gets `429` and the message is not trashed)
- `reject_schedule_failure_restores_message` (service integration)

## Edge cases and traps

- Take unsubscribe targets only from `meta.facts.list_unsubscribe` (T-406 DKIM-covered); never from the body or the client.
- Use T-105a's IDs (`derive_swipe_ids`); never derive a job, rule or task name from the message ID (S7: Cloud Tasks blocks reuse of a deleted name).
- Create the job record before the task: a task firing without a job finds nothing.
- Do not deduplicate jobs per list here; the runner batches (T-701). Each reject gets its own job so its undo is independent.
- `target` and `sender_display` are encrypted on the job; the job carries no address in clear.
- The rate check must run before the provider change.
- Never log the target URL, mailto address or sender.

## Out of scope

- Undo of a reject and the race with the runner from the api side: T-606.
- Running the job: T-701 to T-703. Block prompt accept and decline: T-608. Achievements: T-802.

## Done when

- The tests above pass and every required check is green (S10 10.1).
- Definition of done in S10 10.4.
- The pull request notes the list key HMAC choice so T-701 and S5 can cite it.
