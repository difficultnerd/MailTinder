# T-801: History endpoint

| Milestone | Tier | Size | Depends on |
| --- | --- | --- | --- |
| M8 | sonnet | about 150 lines of code plus tests | T-609 |

**Read only these spec sections:** S7 section 5.9 (API-HIST-1) and section 2 (pagination) in `docs/specs/S7-api-contract.md`; `HistoryEntry` schema and `/history` path in `docs/specs/S7-api-contract.openapi.yaml`; S2 ST-01 AC1 and AC2, UN-01 AC3; S5 "User app folder" (History trimmed at 12 months). Nothing else is needed.

## Goal

`GET /api/v1/history` lists the user's History from the app folder file, newest first, filtered and paged, so Settings, History (T-1006) can show every automated action and link rule entries to their rule.

## Files

| Action | Path | What |
| --- | --- | --- |
| Create | `backend/crates/api/src/routes/history.rs` | Handler and DTOs |
| Change | `backend/crates/api/src/routes/mod.rs` | Mount the route |
| Create | `backend/crates/api/tests/history.rs` | Service integration tests |

## Types and signatures

```rust
#[derive(Deserialize)] #[serde(deny_unknown_fields)]
pub struct HistoryQuery { pub filter: Option<HistoryFilter>, pub cursor: Option<String>, pub limit: Option<u32> }
#[derive(Deserialize, Clone, Copy)] #[serde(rename_all = "snake_case")]
pub enum HistoryFilter { All, Unsubscribes, RuleActions, Filing }

#[derive(Serialize)] pub struct HistoryEntryDto { pub entry_id: Uuid, pub at: OffsetDateTime, pub mailbox_id: Uuid,
    pub sender_display: String, pub action: HistoryAction, pub outcome: HistoryOutcome, pub rule_id: Option<Uuid> }
#[derive(Serialize)] pub struct HistoryPage { pub entries: Vec<HistoryEntryDto>, pub next_cursor: Option<String> }

#[derive(Serialize, Deserialize)]
pub struct HistoryCursor { pub filter: HistoryFilter, pub before_at: OffsetDateTime, pub before_id: Uuid } // sealed TokenType::Cursor

pub fn matches_filter(f: HistoryFilter, a: HistoryAction) -> bool;
//   All: every action; Unsubscribes: Unsubscribe; RuleActions: TrashedByRule, FiledByRule, Blocked;
//   Filing: Filed, FiledByRule. Exhaustive matches, no `_`.
```

## Algorithm

1. `limit` 1 to 50, default 20, else `400`. `UserStateStore::load` (read only; this is a `GET`).
2. Take `state.history`, drop entries older than 365 days (`[TUNABLE]`, S5), apply `matches_filter`.
3. Sort by `(at, entry_id)` descending (ST-01 AC1).
4. Cursor: open as `TokenType::Cursor`; failure or a filter different from the request's is `400`. Keep entries strictly before `(before_at, before_id)`.
5. Take `limit`; `next_cursor` sealed from the last entry when more remain, else `null`.
6. `sender_display` through `plain_text(.., 256)`.

## Acceptance criteria

| ID | Behaviour (one line) |
| --- | --- |
| ST-01 AC1 | Every automated action is listed with time, sender and mailbox, newest first |
| ST-01 AC2 | Rule-driven entries carry their `rule_id` |
| UN-01 AC3 | Unsubscribe outcomes collected at Feed load appear in History |

## Tests that must pass

- `st_01_ac1_history_newest_first` (service integration)
- `st_01_ac1_filters` (service integration, one case per filter)
- `st_01_ac1_paging_stable` (service integration: 120 entries in pages of 50, none missing or repeated)
- `st_01_ac2_rule_entries_carry_rule_id` (service integration)
- `un_01_ac3_unsubscribe_outcome_listed` (service integration through a Feed load)
- `asvs_v9_2_2_feed_cursor_as_history_cursor_refused` (service integration: a Feed cursor fails here because the payload does not parse; `400`)
- `history_get_writes_nothing` (service integration: no Drive write)

## Edge cases and traps

- A `GET` must not trim or rewrite the file; trimming happens on writes (T-602b).
- Sort ties by `entry_id`, or pages repeat or skip entries with the same second.
- No sender text in logs.

## Out of scope

- Writing History entries: T-604, T-605, T-606, T-608, T-609.

## Done when

- The tests above pass and every required check is green (S10 10.1).
- Definition of done in S10 10.4.
