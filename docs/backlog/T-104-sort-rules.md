# T-104: Sort rules: reject, block and filing matching

| Milestone | Tier | Size | Depends on |
| --- | --- | --- | --- |
| M1 | sonnet | about 350 lines of code plus tests | T-102 |

**Read only these spec sections:** S3 "User app folder file" table and "Rule matching and counting" (`docs/specs/S3-domain-model.md`); S2 SW-01 AC2, SR-01 (all ACs), PB-01 (all ACs), FL-04 AC1 and AC2 (`docs/specs/S2-v1-acceptance-criteria.md`); S10 5 rows "Same sender, two different List-Ids", "Same sender address, no List-Id" and "Personal-looking message from a known sender with no DKIM-aligned From" (`docs/specs/S10-test-strategy.md`); S6 3 row T16. Nothing else is needed.

## Goal

The `domain` crate gets `SortRule` and the pure logic around it: which enabled rule a fetched message matches, how a reject builds a `reject_list` rule (and when it must not), block and filing rules, and the `SenderStats` counters behind the block prompt (PB-01) and keep-learning prompt (FL-04). Spoofed mail never creates a rule or counts toward a block (SR-01 AC6, PB-01 AC4). The api applies these at Feed load (T-609) and at swipe time (T-605, T-608).

## Files

| Action | Path | What |
| --- | --- | --- |
| Create | `backend/crates/domain/src/rules.rs` | `SortRule`, `RuleKind`, `RuleMatch`, matching and constructors |
| Create | `backend/crates/domain/src/sender_stats.rs` | Counter methods on `SenderStats` (type from T-101) |
| Change | `backend/crates/domain/src/lib.rs` | Module declarations and re-exports |

## Types and signatures

```rust
// rules.rs. Stored in the user state file (T-602b wraps it in StoredRule with times_applied and yearly_rate).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuleKind { RejectList, BlockPerson, File }

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]   // Debug by hand: redacts sender, shows list_id.is_some()
pub struct RuleMatch {
    pub sender: SenderKey,
    pub list_id: Option<String>,
    pub feedback_id: Option<String>,
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]       // Debug by hand (contains RuleMatch)
pub struct SortRule {
    pub rule_id: RuleId,
    pub kind: RuleKind,
    #[serde(rename = "match")]
    pub matcher: RuleMatch,
    pub category: Option<CategoryId>,     // Some only for File
    pub enabled: bool,
    pub created_at: OffsetDateTime,
    pub source_swipe: Option<Uuid>,       // the swipe's Idempotency-Key, when a swipe created it
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuleAction { Trash { rule_id: RuleId }, File { rule_id: RuleId, category: CategoryId } }

impl SortRule {
    pub fn matches(&self, meta: &MessageMeta) -> bool;
    pub fn action(&self) -> Option<RuleAction>;            // None only for a File rule missing its category
    pub fn reject_list_for(meta: &MessageMeta, rule_id: RuleId, now: OffsetDateTime, source: Option<Uuid>) -> Option<SortRule>;
    pub fn block_person_for(sender: &SenderKey, rule_id: RuleId, now: OffsetDateTime, source: Option<Uuid>) -> SortRule;
    pub fn file_for(sender: &SenderKey, category: CategoryId, rule_id: RuleId, now: OffsetDateTime) -> SortRule;
}
pub fn first_match<'a>(rules: &'a [SortRule], meta: &MessageMeta) -> Option<&'a SortRule>;

// sender_stats.rs
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlockPrompt { Ask, NotYet, Declined, NotCounted }

impl SenderStats {
    pub fn record_keep(&mut self);
    /// Counts an authenticated reject (SR-01 AC6, PB-01 AC4) and says whether to ask "Block <name>?".
    /// `personal` is true when the rejected message's class is Personal.
    pub fn record_reject(&mut self, authenticated: bool, personal: bool, now: OffsetDateTime, t: &Tunables) -> BlockPrompt;
    pub fn unrecord_last_reject(&mut self, at: OffsetDateTime);   // for undo (T-105b)
    pub fn record_file(&mut self, category: CategoryId);
    pub fn decline_block_prompt(&mut self, now: OffsetDateTime, t: &Tunables);
    pub fn rejects_within(&self, now: OffsetDateTime, window: Duration) -> u32;
    /// FL-04 AC1: kept at least `keep_learning_threshold` times, never rejected, and no File rule for the sender yet.
    pub fn keep_prompt_due(&self, has_file_rule: bool, t: &Tunables) -> bool;
}
```

## Algorithm

1. **Matching** (`SortRule::matches`), only when `enabled` (SR-01 AC4); the sender must be equal (`meta.sender == matcher.sender`) for every kind:
   - `RejectList` with `list_id: Some(id)`: `meta.facts.list_id.as_deref() == Some(id)` (SR-01 AC3: a different or missing List-Id does not match).
   - `RejectList` with `list_id: None`: `meta.facts.list_unsubscribe_present` must be true, and when `matcher.feedback_id` is `Some(f)`, `meta.facts.feedback_id.as_deref() == Some(f)` (SR-01 AC3: receipts without `List-Unsubscribe` never match).
   - `BlockPerson`: sender equal is enough (PB-01 AC2).
   - `File`: sender equal is enough (FL-04 AC2).
   Compare `list_id` and `feedback_id` exactly; the adapter already normalised them (T-401 step 6).
2. **`first_match`**: among enabled matching rules, prefer `RejectList` and `BlockPerson` (trash) over `File`; ties by older `created_at`, then by `rule_id`. `[DEFAULT]` trash wins over filing because the user rejected the sender after filing it, or blocked a person; the History entry names the rule either way.
3. **`reject_list_for(meta, ...)`**:
   1. `meta.facts.from_authenticated` false: return `None` (SR-01 AC6; S6 T16).
   2. Build `RuleMatch { sender: meta.sender.clone(), list_id: meta.facts.list_id.clone(), feedback_id }` where `feedback_id = meta.facts.feedback_id.clone()` only when `list_id` is `None` (S3: Feedback-ID is the second key of a sender-only rule).
   3. `kind RejectList`, `enabled true`, `category None`.
   The caller (T-105a) decides when a reject creates a rule at all (`list` and `bulk_no_header` only).
4. **`record_reject`**:
   1. `authenticated` false: change nothing, return `NotCounted` (PB-01 AC4).
   2. Push `now` onto `rejects_counted`; if it holds more than `REJECTS_KEPT`, drop the oldest.
   3. `personal` false: return `NotYet`.
   4. `block_prompt_declined_until` is after `now`: return `Declined` (PB-01 AC3).
   5. `rejects_within(now, t.personal_block_window) >= t.personal_block_threshold`: return `Ask`; else `NotYet`.
   `[DEFAULT]` the prompt appears on the reject that brings the count to the threshold (the third). PB-01 AC1 can be read as the fourth, but S9's copy ("You've rejected <name> 3 times") and S10's spoof row ("three such rejects never raise the block prompt") both mean the third.
5. `rejects_within`: count entries in `rejects_counted` with `now - at < window` and `at <= now`.
6. `unrecord_last_reject(at)`: remove the last entry equal to `at`, if any.
7. `record_keep`: `keeps += 1` (SW-01 AC2). `record_file`: `files[category] += 1`, `last_filed = Some(category)`.
8. `decline_block_prompt`: `block_prompt_declined_until = Some(now + t.block_prompt_decline)`.
9. `keep_prompt_due`: `keeps >= t.keep_learning_threshold && rejects_counted.is_empty() && !has_file_rule`. `[DEFAULT]` "never rejected" uses the counted (authenticated) rejects, the only ones stored.
10. All functions are pure apart from mutating `self`; time comes in as `now`.

## Acceptance criteria

| ID | Behaviour (one line) |
| --- | --- |
| SR-01 AC1 | A reject on a list message builds a rule keyed on sender plus List-Id, or sender alone when there is none |
| SR-01 AC2 | A newly fetched message matching an enabled reject rule is found by `first_match` with a trash action |
| SR-01 AC3 | List-Id rules ignore other List-Ids; sender-only rules match only mail with `List-Unsubscribe` and the same Feedback-ID |
| SR-01 AC4 | A disabled rule never matches |
| SR-01 AC6 | A message without an aligned DKIM signature or provider pass creates no reject rule |
| PB-01 AC1 | The third counted personal reject from a sender within 90 days asks to block |
| PB-01 AC2 | A block rule matches every later message from that address |
| PB-01 AC3 | After a decline the prompt is not shown for that sender for 90 days |
| PB-01 AC4 | Unauthenticated rejects are not counted and never raise the prompt |
| FL-04 AC1 | After five keeps and no rejects, the keep-learning prompt is due |
| FL-04 AC2 | Accepting builds a File rule that matches the sender |
| SW-01 AC2 | A keep increments the sender's keep count |

## Tests that must pass

- `sr_01_ac1_rule_keyed_on_sender_and_list_id` (unit)
- `sr_01_ac1_sender_only_rule_keeps_feedback_id` (unit)
- `sr_01_ac2_matching_message_gets_trash_action` (unit)
- `sr_01_ac3_other_list_id_does_not_match` (unit)
- `sr_01_ac3_receipt_without_list_unsubscribe_survives` (unit, S10 corpus row "Same sender address, no List-Id")
- `sr_01_ac3_feedback_id_must_match_when_stored` (unit)
- `sr_01_ac3_list_id_rule_never_matches_other_list` (property: any message whose `list_id` differs from the rule's never matches)
- `sr_01_ac4_disabled_rule_never_matches` (property)
- `sr_01_ac6_unauthenticated_reject_creates_no_rule` (unit)
- `pb_01_ac1_third_personal_reject_asks` (unit)
- `pb_01_ac1_rejects_older_than_90_days_not_counted` (unit, both sides of the boundary)
- `pb_01_ac2_block_rule_matches_any_message_from_sender` (property)
- `pb_01_ac3_decline_suppresses_for_90_days` (unit, both sides of the boundary)
- `pb_01_ac4_three_spoofed_rejects_never_ask` (unit, S10 corpus spoof row)
- `pb_01_ac4_unauthenticated_never_counted` (property: any sequence of rejects with `authenticated` false leaves `rejects_counted` empty and never returns `Ask`)
- `fl_04_ac1_five_keeps_no_rejects_prompt_due` (unit, four keeps not due)
- `fl_04_ac2_file_rule_matches_sender` (unit)
- `sw_01_ac2_keep_counted` (unit)
- `rules_first_match_prefers_trash_then_oldest` (unit)
- `rules_serde_uses_match_key` (unit: JSON has `"match"`, `"kind":"reject_list"`)

## Edge cases and traps

- `match` is a Rust keyword: the field is `matcher`, serialised as `"match"` to keep S3's and S7's name.
- Sender equality uses the normalised `SenderKey` (T-101), which already lower-cased and unwrapped relays. Never compare `from_address`.
- `RuleMatch` and `SortRule` contain a sender address: implement `Debug` by hand (redact `sender`), never derive it (T-003 privacy rule, S5).
- `list_unsubscribe_present` (any header) is the test for "carries `List-Unsubscribe`" in SR-01 AC3, not `list_unsubscribe` (covered options only). A list whose DKIM later fails is still that list.
- A `BlockPerson` or `File` rule never needs `from_authenticated` to match; the restriction is on creating reject rules and counting.
- Time comparisons use `OffsetDateTime` arithmetic with `time::Duration`; write both sides of every boundary as a test (89 days 23 hours, 90 days).
- Keep `rejects_counted` bounded at `REJECTS_KEPT` (20); the state file is the user's and grows forever otherwise.
- Do not create a rule for `Suspect` or `Notice` here; the caller decides by class (T-105a).

## Out of scope

- When a swipe creates a rule and what else it does: T-105a. Applying rules at Feed load and writing History: T-609. Rules endpoints: T-608.
- The yearly-rate figure on a rule (GM-05): T-109 and T-605.
- Filing suggestions (FL-01, FL-03): T-607.

## Done when

- The tests above pass and every required check is green (S10 10.1).
- Definition of done in S10 10.4.
