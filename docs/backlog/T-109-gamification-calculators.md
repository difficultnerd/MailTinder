# T-109: Gamification calculators

| Milestone | Tier | Size | Depends on |
| --- | --- | --- | --- |
| M1 | sonnet | about 200 lines of code plus tests | T-101 |

**Read only these spec sections:** S2 section 10 intro paragraph, GM-05 (all ACs), GM-06 (all ACs), ST-02 AC2 (`docs/specs/S2-v1-acceptance-criteria.md`); S7 5.5 `achievements_unlocked` paragraph and 5.9 API-STAT-1 (`docs/specs/S7-api-contract.md`); S5 "User app folder" table rows "Mail stopped estimate" and "Achievements". Nothing else is needed.

## Goal

Pure calculators in `domain::gamification`: the yearly mail-stopped estimate from a 90-day count (GM-05), the Stats total over enabled rules (ST-02 AC2), and the fixed achievement list with an idempotent unlock check (GM-06). The swipe handler (T-605) and Stats endpoint (T-802) call them. Nothing here adds storage beyond the achievement ID and date S5 already lists.

GM-03 (end-of-round card) is not in this task: S2 GM-03 AC2 keeps round totals in browser memory only, so it belongs to the Flutter task T-1008.

## Files

| Action | Path | What |
| --- | --- | --- |
| Create | `backend/crates/domain/src/gamification.rs` | Everything below |
| Change | `backend/crates/domain/src/lib.rs` | `pub mod gamification;` |

## Types and signatures

```rust
/// GM-05 AC1: yearly rate from the count of the sender's matching messages over the past `window` (90 days).
/// `count` None (the count query failed) gives None (GM-05 AC3: shown as unknown).
pub fn yearly_rate(count: Option<u64>, window: Duration) -> Option<u32>;

/// ST-02 AC2: sum of yearly_rate over enabled rules, ignoring None.
pub fn mail_stopped_per_year<I: IntoIterator<Item = (bool /* enabled */, Option<u32>)>>(rules: I) -> u64;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AchievementId {
    FirstUnsubscribe, SendersSilenced100, YearCleared, Cleared1000,
    FirstFilingCategory, FirstBlockedPerson, TenUnsubscribesInRound,
}
impl AchievementId { pub const ALL: [AchievementId; 7]; pub fn as_str(self) -> &'static str; }

/// Running totals the api keeps in the user state file (T-602b `Totals`, plus the fields below).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AchievementProgress {
    pub unsubscribes_queued: u64,      // jobs queued by rejects, all time
    pub senders_silenced: u64,         // reject_list and block_person rules created, all time
    pub years_cleared: u64,            // GM-04 level completions
    pub cleared: u64,                  // rejects plus files, all time (T-602b Totals.cleared)
    pub categories_created: u64,
    pub people_blocked: u64,
    pub round_unsubscribes: u64,       // unsubscribes queued in the current round (see Algorithm 4)
}

/// GM-06: returns the achievements newly unlocked by `progress`, given those already unlocked.
pub fn newly_unlocked(progress: &AchievementProgress, already: &BTreeSet<AchievementId>) -> Vec<AchievementId>;
```

## Algorithm

1. `yearly_rate(Some(n), window)`: `round(n * 365 / window_days)` with integer arithmetic (`(n * 365 + days / 2) / days`), saturating to `u32::MAX`. `window_days` is `window.whole_days()`; a window under one day is treated as one day. `None` gives `None`.
2. `mail_stopped_per_year`: sum of `rate` for entries with `enabled == true` and `Some(rate)`, as `u64`.
3. Achievement thresholds (GM-06 AC1, the fixed list): `FirstUnsubscribe` at `unsubscribes_queued >= 1`; `SendersSilenced100` at `senders_silenced >= 100`; `YearCleared` at `years_cleared >= 1`; `Cleared1000` at `cleared >= 1000`; `FirstFilingCategory` at `categories_created >= 1`; `FirstBlockedPerson` at `people_blocked >= 1`; `TenUnsubscribesInRound` at `round_unsubscribes >= 10`.
4. `[DEFAULT]` definitions S2 leaves open: "cleared" counts rejects and files (messages that left the inbox), not keeps or skips; "senders silenced" counts reject_list and block_person rules created; "first unsubscribe" counts a queued job, not a confirmed one, so the celebration comes with the swipe. "A round" for `TenUnsubscribesInRound` is the server session (one `session_record_id`), because round totals live only in the browser (GM-03 AC2) and the server, which writes achievements, cannot see them. The api resets `round_unsubscribes` when the session record changes.
5. `newly_unlocked`: every `AchievementId::ALL` entry whose threshold holds and which is not in `already`, in `ALL` order. Already unlocked achievements are never returned again, so unlocks are idempotent and a retried swipe cannot unlock twice (GM-06 AC2 stores ID and date only; the caller adds the date).
6. Pure: no clock (the caller stamps `unlocked_at`), no I/O.

## Acceptance criteria

| ID | Behaviour (one line) |
| --- | --- |
| GM-05 AC1 | The yearly rate is the 90-day count scaled to a year, as one integer |
| GM-05 AC3 | A failed count gives an unknown rate, and the rule is still created by the caller |
| ST-02 AC2 | Mail stopped per year is the sum of yearly rates over enabled rules, ignoring unknowns |
| GM-06 AC1 | The fixed list of seven achievements unlocks at its thresholds |
| GM-06 AC2 | Only the achievement ID (and the caller's date) is produced; unlocking is idempotent |

## Tests that must pass

- `gm_05_ac1_ninety_day_count_scaled_to_year` (unit: 90 gives 365; 0 gives 0; 1 gives 4)
- `gm_05_ac1_rate_saturates` (unit: `u64::MAX` gives `u32::MAX`)
- `gm_05_ac3_failed_count_is_unknown` (unit)
- `st_02_ac2_total_ignores_disabled_and_unknown` (unit)
- `st_02_ac2_total_is_sum` (property: equals a hand-written fold over generated rules)
- `gm_06_ac1_each_threshold` (unit: one case per achievement, just below and at the threshold)
- `gm_06_ac2_unlock_is_idempotent` (property: feeding the result back as `already` gives an empty list)
- `gm_06_ac1_ids_serialise_snake_case` (unit: `first_unsubscribe`, `senders_silenced_100`, `ten_unsubscribes_in_round`)

## Edge cases and traps

- Integer arithmetic only for the rate; no `f64` rounding surprises. Use `checked_mul` or `saturating_mul` before dividing.
- `AchievementId` strings are the wire IDs in S7 (`first_unsubscribe` is S7's example); keep `serde(rename_all = "snake_case")`. Check that `SendersSilenced100` serialises as `senders_silenced_100` and `Cleared1000` as `cleared_1000`; add explicit `#[serde(rename = "...")]` if the derive splits the digits differently.
- Do not store anything new: `AchievementProgress` is computed by the api from fields T-602b already keeps (`Totals`, rules, categories) plus the per-session unsubscribe counter. Report any field the api must add.
- Never match `AchievementId` with `_`.

## Out of scope

- The count query and storing the rate on the rule: T-605. Stats endpoint: T-802. Celebrations and the round card (GM-03): T-1008.

## Done when

- The tests above pass and every required check is green (S10 10.1).
- Definition of done in S10 10.4.
