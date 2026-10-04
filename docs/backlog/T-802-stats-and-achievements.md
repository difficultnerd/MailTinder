# T-802: Stats endpoint and achievement unlocking

| Milestone | Tier | Size | Depends on |
| --- | --- | --- | --- |
| M8 | sonnet | about 250 lines of code plus tests | T-109, T-603, T-605, T-608, T-707 |

**Read only these spec sections:** S7 section 5.9 (API-STAT-1) and section 5.5 (`achievements_unlocked`) in `docs/specs/S7-api-contract.md`; `/stats` path and `Achievement` schema in `docs/specs/S7-api-contract.openapi.yaml`; S2 ST-02 AC1 to AC3, GM-05 AC2, GM-06 AC1 to AC3. Nothing else is needed.

## Goal

`GET /api/v1/stats` reports emails triaged, senders unsubscribed, unsubscribes confirmed working, mail stopped per year and unlocked achievements. Achievements are unlocked where the counts change (swipes, block rules, category creation, level completion) and returned on the swipe response for the celebration.

## Files

| Action | Path | What |
| --- | --- | --- |
| Create | `backend/crates/api/src/routes/stats.rs` | Handler and DTO |
| Create | `backend/crates/api/src/services/achievements.rs` | `record_unlocks` |
| Change | `backend/crates/api/src/services/swipe.rs` | Call `record_unlocks` inside the swipe's update; fill `achievements_unlocked` |
| Change | `backend/crates/api/src/services/reject.rs` | Same, for rejects |
| Change | `backend/crates/api/src/services/rules.rs` | Same, after a block rule |
| Change | `backend/crates/api/src/services/categories.rs` | Same, after a category is created |
| Change | `backend/crates/api/src/services/progress.rs` | On level completion: `years_cleared += 1` and unlock (one update) |
| Create | `backend/crates/api/tests/stats.rs` | Service integration tests |

## Types and signatures

```rust
// Used: AchievementId, AchievementProgress, newly_unlocked, mail_stopped_per_year (T-109); Totals, AchievementRecord (T-602b).

#[derive(Serialize)]
pub struct StatsDto { pub emails_triaged: u64, pub senders_unsubscribed: u64, pub unsubscribes_confirmed: u64,
    pub mail_stopped_per_year: u64, pub achievements: Vec<AchievementDto> }
#[derive(Serialize)] pub struct AchievementDto { pub achievement_id: &'static str, pub unlocked_at: OffsetDateTime }

/// Pure; call inside a UserStateStore::update closure after the totals change.
/// Appends new AchievementRecords stamped `now` and returns them.
pub fn record_unlocks(state: &mut UserState, session: Uuid, now: OffsetDateTime) -> Vec<AchievementRecord>;
pub fn progress_from(totals: &Totals) -> AchievementProgress;
```

## Algorithm

1. `progress_from`: copy `unsubscribes_queued`, `senders_silenced`, `years_cleared`, `cleared`, `categories_created`, `people_blocked`; `round_unsubscribes` only when `totals.round_session == Some(session)`, else 0 (T-109 Algorithm 4: a round is one server session).
2. `record_unlocks`: `already` = IDs in `state.achievements`; `newly_unlocked(progress, already)`; push each with `unlocked_at = now`; return them. Idempotent: a re-run of the closure after an ETag conflict cannot unlock twice.
3. Swipes and rules: call `record_unlocks` at the end of the existing update closure; map the result into `achievements_unlocked` (GM-06 AC3). A retried swipe served from `recent_swipes` returns the stored list.
4. Progress (T-603): when `level_progress` returns `Complete { cleared, .. }`, run one update that pushes `cleared` into a new `Totals.levels_cleared: Vec<i32>` (`#[serde(default)]`) if absent, sets `years_cleared = levels_cleared.len()`, and calls `record_unlocks` `[DEFAULT]` (the list stops one year being counted twice when Progress is called again).
5. API-STAT-1: load state (read only). `emails_triaged = totals.triaged`; `senders_unsubscribed = totals.senders_unsubscribed`; `unsubscribes_confirmed = totals.unsubscribes_confirmed` (T-707); `mail_stopped_per_year = mail_stopped_per_year(rules.map(|r| (r.rule.enabled, r.yearly_rate)))` (ST-02 AC2); `achievements` = records in unlock order, unlocked only (ST-02 AC3; the app greys the rest).

## Acceptance criteria

| ID | Behaviour (one line) |
| --- | --- |
| ST-02 AC1 | Stats show emails triaged, senders unsubscribed and unsubscribes confirmed working |
| ST-02 AC2 | Mail stopped per year is the sum of yearly rates over enabled rules |
| ST-02 AC3 | Stats list unlocked achievements with dates |
| GM-05 AC2 | Stats show the mail stopped total |
| GM-06 AC1 | The fixed achievements unlock at their thresholds through real swipes and rules |
| GM-06 AC2 | Only ID and unlock date are stored, and each unlocks once |
| GM-06 AC3 | An unlock is returned on the swipe that caused it |

## Tests that must pass

- `st_02_ac1_counts_reported` (service integration)
- `st_02_ac2_total_from_enabled_rules` (service integration: a disabled rule and a null rate are ignored)
- `st_02_ac3_unlocked_achievements_listed` (service integration)
- `gm_05_ac2_stats_mail_stopped_total` (service integration)
- `gm_06_ac1_first_unsubscribe_on_first_queued_reject` (service integration)
- `gm_06_ac1_first_filing_category_on_create` (service integration)
- `gm_06_ac1_first_blocked_person_on_block_rule` (service integration)
- `gm_06_ac1_year_cleared_on_level_complete` (service integration through API-PROG-1)
- `gm_06_ac1_ten_unsubscribes_in_one_session` (service integration: a new session resets the round)
- `gm_06_ac2_unlock_once_under_retry_and_conflict` (service integration: ETag conflict re-runs the closure; one record)
- `gm_06_ac3_swipe_returns_unlock` (service integration)

## Edge cases and traps

- Stamp `unlocked_at` from `Clock`, passed into the closure; never call the clock inside `domain`.
- `record_unlocks` must be pure so it can run inside the update closure.
- A `GET` writes nothing; only API-PROG-1's level completion writes, and only when the level is complete.
- Achievement IDs on the wire are T-109's `as_str()` values; do not invent new spellings.

## Out of scope

- Delivery check and the confirmed count: T-707. Celebrations and the round card: T-1008.

## Done when

- The tests above pass and every required check is green (S10 10.1).
- Definition of done in S10 10.4.
- The pull request notes the `levels_cleared` field added to `Totals`.
