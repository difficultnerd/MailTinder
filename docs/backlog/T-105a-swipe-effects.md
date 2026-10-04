# T-105a: Swipe effects by class

| Milestone | Tier | Size | Depends on |
| --- | --- | --- | --- |
| M1 | sonnet | about 300 lines of code plus tests | T-102, T-103, T-104 |

Split from index row T-105 ("Swipe effects by class and the undo stack"); the undo half is T-105b. T-103 and T-104 are added dependencies: GUARD-3 needs the guard, and rejects build rules.

**Read only these spec sections:** S3 "Message classes" and "Rule matching and counting" (`docs/specs/S3-domain-model.md`); S2 SW-01 AC1, SW-02 AC1, SW-03 (all ACs), SW-04 AC2, SR-01 AC6, PB-01 AC4, UN-04 AC6 (`docs/specs/S2-v1-acceptance-criteria.md`); S7 5.5 API-SW-1 `outcome` table and the "Idempotency" paragraph (`docs/specs/S7-api-contract.md`); S10 9.3 GUARD-1 and GUARD-3. Nothing else is needed.

## Goal

A pure planner turns one swipe into a `SwipePlan`: the mailbox change (none, trash, report spam and trash, apply a category), the unsubscribe job to queue (method and target) or the Needs Attention link to raise, the sort rule to create, the sender-stats change and the S7 `outcome`. It reads the header facts only, never a model's answer, so whatever classifier later drives the badge, side effects stay the same (GUARD-3). The api (T-604, T-605) executes the plan; this task does no I/O.

## Files

| Action | Path | What |
| --- | --- | --- |
| Create | `backend/crates/domain/src/swipe.rs` | `SwipeInput`, `SwipePlan`, `plan_swipe`, `derive_swipe_ids` |
| Change | `backend/crates/domain/src/lib.rs` | `pub mod swipe; pub use swipe::*;` |

## Types and signatures

```rust
pub struct SwipeInput<'a> {
    pub action: SwipeAction,
    pub meta: &'a MessageMeta,           // freshly re-read from the provider (S7 5.5)
    pub badge: &'a Classification,       // what the card showed; display only, MUST NOT influence the plan
    pub stats: &'a SenderStats,          // before this swipe
    pub ids: SwipeIds,
    pub source_swipe: Uuid,              // Idempotency-Key
    pub now: OffsetDateTime,
    pub tunables: &'a Tunables,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SwipeIds { pub job_id: JobId, pub rule_id: RuleId }

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MailboxChange { None, Trash, ReportSpamAndTrash, ApplyCategory { category: CategoryId } }

#[derive(Clone, PartialEq, Eq)]                 // Debug by hand: method only
pub struct UnsubscribePlan { pub job_id: JobId, pub method: JobMethod, pub target: UnsubscribeTarget, pub due_at: OffsetDateTime }
#[derive(Clone, PartialEq, Eq)]                 // Debug by hand: variant only
pub enum UnsubscribeTarget { OneClick(Url), Mailto(MailtoTarget) }

#[derive(Clone, PartialEq, Eq)]                 // Debug by hand: link.is_some() only
pub struct ManualUnsubscribePlan { pub link: Option<Url> }   // NA reason https_only_unsubscribe (S7 5.8)

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SwipeOutcome { Kept, Skipped, TrashedUnsubscribeQueued, TrashedUnsubscribeManual,
                        TrashedListNoUnsubscribe, Trashed, ReportedSpam, Filed }

#[derive(Clone, Debug, PartialEq)]
pub struct SwipePlan {
    pub class: MessageClass,                    // HeaderRules class from the facts (the action class)
    pub change: MailboxChange,
    pub unsubscribe: Option<UnsubscribePlan>,
    pub manual_unsubscribe: Option<ManualUnsubscribePlan>,
    pub rule: Option<SortRule>,
    pub stats_after: SenderStats,               // stats with this swipe applied
    pub block_prompt: bool,                     // PB-01 AC1
    pub outcome: SwipeOutcome,
}

pub fn plan_swipe(input: &SwipeInput<'_>) -> SwipePlan;
pub fn derive_swipe_ids(user: &UserId, idempotency_key: Uuid) -> SwipeIds;
```

`JobMethod` is defined by T-106 (`OneClick`, `Mailto`). If T-106 has not merged, define `JobMethod` in `domain/src/unsubscribe_job.rs` with exactly `#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)] #[serde(rename_all = "snake_case")] pub enum JobMethod { OneClick, Mailto }` and T-106 reuses it.

## Algorithm

1. `class = HeaderRules::classify(&meta.facts, &meta.sender).class` and `route = HeaderRules::unsubscribe_route(&meta.facts)`. Ignore `input.badge` entirely.
2. `stats_after = stats.clone()`; `block_prompt = false`; `unsubscribe`, `manual_unsubscribe`, `rule` start as `None`.
3. By action:
   - `Keep`: `change None`, `stats_after.record_keep()`, outcome `Kept` (SW-01 AC1, AC2).
   - `Skip`: `change None`, no stats change, outcome `Skipped` (SW-02 AC1). Skip counting and re-insertion are T-108 and T-602c.
   - `File { category }`: `change ApplyCategory { category }`, `stats_after.record_file(category)`, outcome `Filed` (SW-04 AC2: the adapter applies the label and removes the inbox label).
   - `Reject`: by `class`:
     - `List`: `change Trash`. `route` `OneClick(u)` or `Mailto(t)`: `unsubscribe = Some(UnsubscribePlan { job_id: ids.job_id, method, target, due_at: now + tunables.unsub_delay })`, outcome `TrashedUnsubscribeQueued` (SW-03 AC2). `route` `ManualLink(link)`: `manual_unsubscribe = Some(ManualUnsubscribePlan { link })`, outcome `TrashedUnsubscribeManual` (UN-04 AC6). Rule: `SortRule::reject_list_for(meta, ids.rule_id, now, Some(source_swipe))`. Count: `stats_after.record_reject(meta.facts.from_authenticated, false, now, t)`.
     - `BulkNoHeader`: `change Trash`, rule as for `List`, count as for `List`, outcome `TrashedListNoUnsubscribe`; never an unsubscribe of any kind (SW-03 AC5).
     - `Notice`: `change Trash`, no rule, count as for `List` with `personal` false, outcome `Trashed`.
     - `Personal`: `change Trash`, no rule, `block_prompt = stats_after.record_reject(authenticated, true, now, t) == BlockPrompt::Ask`, outcome `Trashed` (SW-03 AC4, PB-01).
     - `Suspect`: `change ReportSpamAndTrash`, no rule, no count, no unsubscribe, outcome `ReportedSpam` (SW-03 AC3; S6 6 last bullet).
   - A reject with `ManualLink` or a job when `from_authenticated` is false still trashes and still queues the job; only the rule and the count depend on `from_authenticated` (SR-01 AC6; spike E1 ESP-only DKIM case).
4. Return the plan.
5. `derive_swipe_ids(user, key)`: `job_id = Uuid::new_v5(&user.0, format!("job:{key}").as_bytes())`, `rule_id = Uuid::new_v5(&user.0, format!("rule:{key}").as_bytes())` `[DEFAULT]` (S7 5.5 says UUID v5 of the user ID and the Idempotency-Key; two distinct names keep the job and rule IDs different).

## Acceptance criteria

| ID | Behaviour (one line) |
| --- | --- |
| SW-01 AC1 | Keep plans no mailbox change |
| SW-02 AC1 | Skip plans no mailbox change |
| SW-03 AC1 | Every reject plans a trash, never a delete |
| SW-03 AC2 | A list reject with one-click or mailto plans a job due `now + UNSUB_DELAY` and a reject rule |
| SW-03 AC3 | A suspect reject plans report spam and trash, and no job |
| SW-03 AC4 | A personal reject only trashes, and counts toward the block prompt |
| SW-03 AC5 | A message without a covered header gets no unsubscribe of any kind; `bulk_no_header` gets a reject rule |
| SW-04 AC2 | A file swipe plans the category label |
| UN-04 AC6 | An https-only list plans trash, a reject rule and a manual unsubscribe link, and no job |
| SR-01 AC6 | An unauthenticated message gets no rule and no count, but is still trashed |
| PB-01 AC4 | Unauthenticated personal rejects never raise the block prompt |
| GUARD-1 | Without a valid covered header no unsubscribe job is planned, whatever any model says |
| GUARD-3 | Plans are identical with any model output as the badge and with none |

## Tests that must pass

- `sw_01_ac1_keep_changes_nothing` (unit)
- `sw_02_ac1_skip_changes_nothing` (unit)
- `sw_03_ac1_reject_always_trash_never_delete` (property: every reject plan's change is `Trash` or `ReportSpamAndTrash`)
- `sw_03_ac2_reject_queues_unsubscribe_with_delay` (unit: `due_at == now + 5 minutes`)
- `sw_03_ac2_mailto_route_queues_mailto_job` (unit)
- `sw_03_ac3_suspect_reported_no_job` (unit)
- `sw_03_ac4_personal_trash_and_count` (unit)
- `sw_03_ac5_no_header_no_unsubscribe_attempt` (property: `list_unsubscribe_present` false gives no `unsubscribe` and no `manual_unsubscribe`)
- `sw_03_ac5_bulk_no_header_creates_reject_rule` (unit)
- `sw_04_ac2_file_applies_category` (unit)
- `un_04_ac6_https_only_manual_link_no_job` (unit)
- `un_04_ac6_plain_http_manual_item_without_link` (unit)
- `sr_01_ac6_spoofed_list_trashed_no_rule` (unit)
- `pb_01_ac4_spoofed_personal_never_prompts` (unit)
- `guard_1_no_job_without_valid_header` (property: facts with route `None`, any badge: `unsubscribe` is `None`)
- `guard_3_models_cannot_act` (property: for generated facts, stats and any model `Classification` (any class, score, NaN probabilities) passed through `header_guard` as the badge, `plan_swipe` equals the plan with the header-rules badge, for every `SwipeAction`)
- `swipe_ids_stable_and_distinct` (unit: same inputs give the same IDs; job and rule IDs differ; another user gives other IDs)

## Edge cases and traps

- The `badge` parameter exists only so GUARD-3 can be tested; reading it anywhere in `plan_swipe` is a bug. Name it `_badge` in a destructure if Clippy complains about an unused field, but do not use it.
- Never plan a delete: there is no delete variant, and there must never be one (INV-5).
- Do not trust any class or target from the client; everything comes from `meta.facts` (S7 5.5).
- Notice rejects create no rule (S3 table: trash only), although SW-03 AC5 read alone suggests every headerless message gets one; S3 and S7 (`trashed` for notice and personal) are the more specific sources.
- `UnsubscribePlan` holds a URL or mailto address: implement `Debug` by hand.
- Keep the plan deterministic: no clock (use `input.now`), no randomness.
- Property strategies: reuse T-102's facts strategy; generate `SenderStats` with 0 to 5 recent rejects.

## Out of scope

- Undo plans and the undo stack: T-105b. Executing plans, idempotent retries, Cloud Tasks: T-604, T-605. Job state machine: T-106.
- The GM-05 yearly rate on a new rule: T-109 and T-605. Block prompt endpoints: T-608.

## Done when

- The tests above pass and every required check is green (S10 10.1).
- Definition of done in S10 10.4.
