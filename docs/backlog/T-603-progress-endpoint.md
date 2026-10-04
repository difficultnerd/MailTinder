# T-603: Progress endpoint

| Milestone | Tier | Size | Depends on |
| --- | --- | --- | --- |
| M6 | sonnet | about 200 lines of code plus tests | T-108, T-602a, T-602c |

**Read only these spec sections:** S7 section 5.3 (API-PROG-1) in `docs/specs/S7-api-contract.md`; the `/progress` response schema in `docs/specs/S7-api-contract.openapi.yaml`; S2 GM-01 AC1 and AC2, GM-04 AC1 to AC3. Nothing else is needed.

## Goal

`GET /api/v1/progress` returns the inbox count summed across mailboxes, per-mailbox errors, and the current backlog level (calendar year and mail left in it), so the Feed can show "12,431, down 214 today" and "Level 2023: 1,840 left".

## Files

| Action | Path | What |
| --- | --- | --- |
| Create | `backend/crates/api/src/routes/progress.rs` | Handler and DTOs |
| Create | `backend/crates/api/src/services/progress.rs` | Counting and level choice |
| Change | `backend/crates/api/src/routes/mod.rs` | Mount the route (other reads limit, S7 section 6) |
| Create | `backend/crates/api/tests/progress.rs` | Service integration tests |

## Types and signatures

```rust
// Used from earlier tasks: UserStateStore and MailboxPosition (T-602b), MailboxErrorDto and the
// MailError to code mapping (T-602c; move the mapping to a shared fn `mailbox_error_code(&MailError)`
// in services/feed.rs if it is private), MessageQuery and count_messages (T-602a),
// domain::feed::{current_level, level_range, level_progress, LevelProgress} (T-108).

#[derive(Serialize)]
pub struct ProgressDto { pub inbox_count: u64, pub mailbox_errors: Vec<MailboxErrorDto>, pub level: Option<LevelDto> }
#[derive(Serialize)]
pub struct LevelDto { pub year: i32, pub remaining: u64 }

pub const LEVEL_LOOKBACK_YEARS: i32 = 10;   // [DEFAULT] stop searching for a non-empty year after 10 empty ones

pub async fn progress(app: &AppState, session: &AuthedSession) -> Result<ProgressDto, ApiError>;
```

## Algorithm

1. `UserStateStore::load`; mailboxes from `mailboxes().by_user`.
2. For each mailbox with status `connected` (others add their error code and are skipped), concurrently (at most 8 in flight): `inbox_count(ctx)`. Success adds to `inbox_count`; failure adds a `MailboxErrorDto` (GM-01 AC2).
3. Level is `null` while any connected mailbox's position has `new_done == false` (new mail not yet cleared, GM-04 AC1).
4. Otherwise the year: `current_level(max backlog_ceiling over mailboxes, true)` (T-108). `None` gives `level: null`.
5. Remaining: sum over connected mailboxes of `count_messages({ in_inbox: true, after: start - 1s, before: end })` where `(start, end) = level_range(year)` (one date-range count per mailbox, GM-04 AC1). Errors here add the mailbox to `mailbox_errors` once.
6. `level_progress(year, remaining)`: `Continue` gives `{ year, remaining }`. `Complete { next, .. }` repeats step 5 for `next`, at most `LEVEL_LOOKBACK_YEARS` times; if every year is empty, `level: null`. The app shows the level-complete screen when the year it gets differs from the one it showed (GM-04 AC2).
7. Return `200`. Nothing is written: this is a `GET` (S7 principle 4), and the level comes from the stored positions (GM-04 AC3).

## Acceptance criteria

| ID | Behaviour (one line) |
| --- | --- |
| GM-01 AC1 | `inbox_count` is the total inbox count across every mailbox |
| GM-01 AC2 | One failing mailbox is listed in `mailbox_errors` and the rest still count |
| GM-04 AC1 | Once new mail is cleared, `level` gives the year and mail left, by one date-range count per mailbox |
| GM-04 AC2 | When a year has no mail left, the next older year with mail is returned |
| GM-04 AC3 | The level comes from the stored Feed position; the endpoint writes nothing |

## Tests that must pass

- `gm_01_ac1_inbox_count_sums_mailboxes` (service integration, two `FakeMailbox` instances)
- `gm_01_ac2_failing_mailbox_marked_rest_counted` (service integration)
- `gm_04_ac1_level_null_until_new_mail_cleared` (service integration)
- `gm_04_ac1_level_year_and_remaining` (service integration; asserts one count call per mailbox)
- `gm_04_ac2_empty_year_moves_to_next_older` (service integration)
- `gm_04_ac3_progress_writes_nothing` (service integration: the Drive fake records no write)

## Edge cases and traps

- Years are UTC calendar years (`level_range` from T-108). Do not use the server's local time zone.
- Never call `count_messages` per message or page through IDs; one count per mailbox per year.
- A `GET` must not change the user state file or the store.
- Mailbox addresses never go in the response; errors carry `mailbox_id` only.

## Out of scope

- The "down 214 today" difference: the app keeps the session's first count in memory (T-1008).
- Unlocking the `year_cleared` achievement: T-802.

## Done when

- The tests above pass and every required check is green (S10 10.1).
- Definition of done in S10 10.4.
