# T-108: Feed ordering, skips, phases, levels and bosses

| Milestone | Tier | Size | Depends on |
| --- | --- | --- | --- |
| M1 | sonnet | about 250 lines of code plus tests | T-101 |

**Read only these spec sections:** S3 "Card visibility" (`docs/specs/S3-domain-model.md`); S2 FD-02 AC1, FD-03 AC1, AC2, AC4, FD-04 AC1, SW-02 AC2, GM-04 (all ACs), GM-08 AC1 and AC3 (`docs/specs/S2-v1-acceptance-criteria.md`); S7 5.3 API-PROG-1 `level` and 5.4 `phase` bullets (`docs/specs/S7-api-contract.md`). Nothing else is needed.

## Goal

Pure Feed helpers in `domain::feed`: the one ordering every Feed page uses, the card visibility rule, where a skipped card comes back (with the injected random draw), the backlog level (calendar year) and level completion, and boss senders. The Feed endpoint (T-602c) and Progress endpoint (T-603) call these; paging and storage stay in the api.

## Files

| Action | Path | What |
| --- | --- | --- |
| Create | `backend/crates/domain/src/feed.rs` | Everything below |
| Change | `backend/crates/domain/src/lib.rs` | `pub mod feed;` (callers use `domain::feed::...`, as T-602c expects) |

## Types and signatures

```rust
/// Newest first by internal_date, then mailbox ID, then message ID (stable, deterministic). FD-02 AC1.
pub fn feed_order(a: &MessageMeta, b: &MessageMeta) -> std::cmp::Ordering;
pub fn sort_for_feed(metas: &mut [MessageMeta]);

/// S3 "Card visibility": hidden when it matched an enabled rule, left the inbox, or reached the skip limit.
pub fn card_visible(in_inbox: bool, matched_enabled_rule: bool, skips_this_session: u8, t: &Tunables) -> bool;

pub const SKIP_RANDOM_SPAN: u32 = 20;   // [DEFAULT] a random return lands 1..=20 cards after the page end
/// Called when a card is skipped. `skips_this_session` includes this skip.
/// Returns how many cards later it comes back (T-602b SkipReturn.after_cards), or None when it must not
/// return this session (SW-02 AC2). `draw` comes from the Rng port (seeded in tests).
pub fn choose_skip_return(skips_this_session: u8, cards_left_on_page: u32, draw: u64, t: &Tunables) -> Option<u32>;

pub fn current_level(backlog_ceiling: Option<OffsetDateTime>, in_backlog: bool) -> Option<i32>;   // GM-04
pub fn level_range(year: i32) -> Option<(OffsetDateTime, OffsetDateTime)>;                        // [start, end) UTC
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LevelProgress { Continue { year: i32 }, Complete { cleared: i32, next: i32 } }
pub fn level_progress(year: i32, remaining: u64) -> LevelProgress;

/// GM-08 AC1: at least boss_min_seen cards seen, among the top boss_top_n senders by `seen`, not defeated.
pub fn is_boss(sender_key: &str, all: &BTreeMap<String, SenderStats>, t: &Tunables) -> bool;
/// GM-08 AC3: a reject of a boss defeats it. Returns true when this reject defeated a boss.
pub fn defeat_boss_on_reject(stats: &mut SenderStats, was_boss: bool) -> bool;
```

## Algorithm

1. `feed_order`: compare `b.internal_date` with `a.internal_date` (newest first), then `a.mailbox` with `b.mailbox`, then `a.id.as_str()` with `b.id.as_str()`. `sort_for_feed` uses `sort_by(feed_order)`.
2. `card_visible`: `in_inbox && !matched_enabled_rule && skips_this_session <= t.skip_max_returns`. A card skipped twice has returned twice; the third skip makes the count 3 and hides it until a new session.
3. `choose_skip_return`:
   1. `skips_this_session > t.skip_max_returns`: `None`.
   2. Else, `[DEFAULT]` the two S2 options are both used: when `draw % 2 == 0` the card returns at the end of the current page (`cards_left_on_page`); otherwise at a random later position, `cards_left_on_page + 1 + (draw / 2) % SKIP_RANDOM_SPAN`. S2 SW-02 AC2 allows either; mixing them keeps the queue from feeling mechanical.
   3. The result is at least 1, so a skipped card never comes straight back as the next card.
4. Levels (GM-04):
   - `current_level`: `None` unless `in_backlog` (AC1: only once new mail is cleared). With a `backlog_ceiling`, the year of that time in UTC; with none (backlog just started), `None` until the first backlog page sets a ceiling.
   - `level_range(year)`: `[year-01-01T00:00:00Z, (year+1)-01-01T00:00:00Z)`; `None` for a year the `time` crate cannot represent. `[DEFAULT]` UTC year boundaries; S2 does not say whose time zone, and one provider date-range query per mailbox needs fixed bounds.
   - `level_progress(year, remaining)`: `remaining == 0` gives `Complete { cleared: year, next: year - 1 }` (AC2), else `Continue { year }`.
   - AC3: the year comes only from the Feed position (`backlog_ceiling`, stored by T-602b); nothing new is stored.
5. Bosses (GM-08):
   - `is_boss`: the sender's stats must exist, have `seen >= t.boss_min_seen` and `boss_defeated == false`. Rank all non-defeated senders with `seen >= boss_min_seen` by `seen` descending, ties by key ascending; the sender is a boss if its rank is below `t.boss_top_n`.
   - `defeat_boss_on_reject`: when `was_boss`, set `boss_defeated = true` and return true; otherwise return false. The caller computes `was_boss` with `is_boss` before applying the reject.
6. Pure: no clock, no randomness (the draw comes in), no I/O.

## Acceptance criteria

| ID | Behaviour (one line) |
| --- | --- |
| FD-02 AC1 | Cards from several mailboxes interleave by received time, newest first, with a stable tie order |
| FD-03 AC4 | Nothing in the ordering or visibility rules caps cards per day |
| FD-04 AC1 | A message no longer in the inbox is not visible |
| SW-02 AC2 | A skipped card returns later, at most `SKIP_MAX_RETURNS` times, using an injected random draw |
| GM-04 AC1 | The level is the calendar year of the backlog position, shown only in the backlog phase |
| GM-04 AC2 | When a year has no mail left, it is complete and the next older year begins |
| GM-04 AC3 | The level comes from the stored Feed position; nothing new is stored |
| GM-08 AC1 | A sender with at least 20 seen and in the top 5 by count is a boss |
| GM-08 AC3 | Rejecting a boss defeats it and it leaves the boss list |

## Tests that must pass

- `fd_02_ac1_interleaved_newest_first` (unit: two mailboxes)
- `fd_02_ac1_order_total_and_stable` (property: sorting any shuffle of the same messages gives the same order)
- `fd_03_ac4_no_daily_cap` (property: `card_visible` never depends on how many cards were shown before)
- `fd_04_ac1_message_left_inbox_hidden` (unit)
- `sw_02_ac2_skip_returns_at_most_twice` (unit: skips 1 and 2 return, skip 3 gives `None` and `card_visible` false)
- `sw_02_ac2_seeded_draw_is_deterministic` (property: same inputs give the same position; the position is at least 1 and at most `cards_left_on_page + SKIP_RANDOM_SPAN`)
- `gm_04_ac1_level_is_backlog_year` (unit)
- `gm_04_ac1_no_level_in_new_phase` (unit)
- `gm_04_ac2_year_complete_moves_to_previous_year` (unit)
- `gm_04_ac3_level_from_position_only` (unit: the function takes only the position, a compile-level guarantee shown by calling it)
- `gm_04_level_range_bounds_utc` (unit)
- `gm_08_ac1_boss_needs_20_seen_and_top_5` (unit: 19 seen is not a boss; sixth by count is not a boss)
- `gm_08_ac1_ties_broken_by_key` (unit)
- `gm_08_ac3_rejecting_boss_defeats_it` (unit: afterwards `is_boss` is false and the next sender moves into the top 5)

## Edge cases and traps

- `MessageMeta.id` is `MessageId` (T-101), which has no `Display`; compare with `as_str()`.
- `SenderStats` keys in the state file are `SenderKey` strings (T-602b); `is_boss` takes `&str` for that reason. T-602c's sketch calls `is_boss(sender_stats, &state.sender_stats)`; the right call is `is_boss(sender.as_str(), &state.sender_stats, &tunables)`.
- Never call an RNG here; the draw is an argument. In the api it comes from the `Rng` port, so tests seed it.
- `card_visible` uses `<=`: two returns are allowed, a third skip hides it. Write the boundary test.
- The level is a calendar year, not a 365-day window. Use `OffsetDateTime::year()` on the UTC value.
- Do not decide `phase` or `phase_changed` here; T-602c owns paging state. These helpers only take what they are given.

## Out of scope

- Fetching, paging, cursors and the skip queue's storage: T-602b, T-602c. The inbox meter and the level count query: T-603. Boss health bar count: T-602c.

## Done when

- The tests above pass and every required check is green (S10 10.1).
- Definition of done in S10 10.4.
