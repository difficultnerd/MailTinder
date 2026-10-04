# T-607b: Filing suggestions and the keep prompt

| Milestone | Tier | Size | Depends on |
| --- | --- | --- | --- |
| M6 | sonnet | about 250 lines of code plus tests | T-104, T-602c, T-607a |

Split from index row T-607 (categories and filing suggestions).

**Read only these spec sections:** S7 section 5.4 (`suggestion` and `keep_prompt` on `Card`) in `docs/specs/S7-api-contract.md`; `Suggestion` and `CategoryRef` schemas in `docs/specs/S7-api-contract.openapi.yaml`; S2 FL-01 AC1 to AC3, FL-03 AC1 and AC2, FL-04 AC1, SW-04 AC1 and AC3; S9 section 4. Nothing else is needed.

## Goal

Each Feed card ships a filing suggestion (best category plus up to two alternates, with a confidence) and, when keep-learning applies, a keep prompt. The server algorithm is simple, deterministic and fast; on-device Gemini Nano in the browser is optional and may override the name only.

## Files

| Action | Path | What |
| --- | --- | --- |
| Create | `backend/crates/domain/src/filing.rs` | Pure `suggest` and `keep_prompt` |
| Change | `backend/crates/domain/src/lib.rs` | `pub mod filing;` |
| Change | `backend/crates/api/src/services/feed.rs` | Fill `suggestion` and `keep_prompt` on each card |
| Create | `backend/crates/domain/tests/filing_latency.rs` | p95 test |

## Types and signatures

```rust
// backend/crates/domain/src/filing.rs
pub const LEARNED_THRESHOLD: u32 = 3;   // FL-03 AC1 [TUNABLE]
pub const MAX_ALTERNATES: usize = 2;    // S7 Suggestion.alternates maxItems

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Confidence { Learned, Suggested, None }

#[derive(Clone, Debug, PartialEq)]
pub struct Suggestion { pub category: Option<CategoryId>, pub name: Option<String>,
    pub alternates: Vec<(CategoryId, String)>, pub confidence: Confidence }

pub struct FilingInput<'a> {
    pub sender_key: &'a str,
    pub sender_domain: &'a str,                         // part after '@' of the sender key
    pub class: MessageClass,                            // header rules class (FL-01 AC2 fallback)
    pub categories: &'a [Category],
    pub sender_stats: &'a BTreeMap<String, SenderStats>,
}

pub fn suggest(input: &FilingInput<'_>) -> Suggestion;
/// FL-04 AC1: Some((category, name)) when keep-learning applies and a category can be named.
pub fn keep_prompt(input: &FilingInput<'_>, has_file_rule: bool, t: &Tunables) -> Option<(CategoryId, String)>;
/// Header-rules name hint when the user has no categories [DEFAULT].
pub fn name_hint(class: MessageClass) -> Option<&'static str>;
```

## Algorithm

`suggest`:

1. Drop counts for categories that no longer exist.
2. Tier 1, sender history (FL-01 AC1): rank categories by `sender_stats[sender].files[c]` descending, ties broken by `last_filed == c` first, then by name ascending.
3. Tier 2, sender domain history: sum `files` over every other sender whose key ends with `@` plus the same domain; rank the same way (ties by name).
4. Tier 3, most-used categories: sum `files` over all senders; categories never used rank after used ones, by name.
5. The ranked list is tier 1 entries, then tier 2 entries not already listed, then tier 3 entries not already listed.
6. Primary = first entry; alternates = the next two (SW-04 AC1).
7. Confidence:
   - `Learned` when the primary came from tier 1, its count is at least `LEARNED_THRESHOLD`, and it equals `last_filed` (FL-03 AC1: three confirmations of the same suggestion; the app shows one-tap confirm with alternates collapsed);
   - `Suggested` when there is a primary from any tier;
   - `None` when the user has no categories (SW-04 AC3: go straight to naming). Then `category = None`, `name = name_hint(class)`, no alternates.
8. `name_hint` (FL-01 AC2 header rules fallback, `[DEFAULT]` names): `List` gives "Newsletters", `BulkNoHeader` gives "Updates", `Notice` gives "Accounts", `Personal` gives "People", `Suspect` gives `None`. Exhaustive `match`.

`keep_prompt`: `stats.keep_prompt_due(has_file_rule, t)` (T-104: at least 5 keeps, no rejects); if true and `suggest` has a primary category, return it; else `None`.

Feed (T-602c step 11): build `FilingInput` from the loaded state and the card's class; `suggestion` is always an object (`confidence: "none"` when empty); `keep_prompt` per the function; `has_file_rule` is whether an enabled `File` rule for the sender exists. The browser may replace `name` with Gemini Nano output; nothing from it reaches the server except the name the user confirms on a `file` swipe.

## Acceptance criteria

| ID | Behaviour (one line) |
| --- | --- |
| FL-01 AC1 | With sender history the suggestion is the category used most for that sender |
| FL-01 AC2 | Without sender history the suggestion comes from domain history, then overall use, then the header-rules name hint |
| FL-01 AC3 | The server suggestion takes at most 100 ms at p95 |
| FL-03 AC1 | Three confirmations of the same category make the suggestion `learned` |
| FL-03 AC2 | A suggestion never files anything by itself |
| FL-04 AC1 | Five keeps and no rejects add a keep prompt naming a category |
| SW-04 AC1 | The suggestion has the best category first and at most two alternates |
| SW-04 AC3 | With no categories the confidence is `none` |

## Tests that must pass

- `fl_01_ac1_sender_history_wins` (unit, `domain`)
- `fl_01_ac2_domain_then_overall_then_hint` (unit, three cases)
- `fl_01_ac3_suggest_p95_under_100ms` (unit in `domain/tests/filing_latency.rs`: state with 5,000 senders and 200 categories, 1,000 calls timed with `std::time::Instant`, p95 at most 100 ms)
- `fl_03_ac1_learned_after_three` (unit: two gives `suggested`, three gives `learned`; three but `last_filed` differs gives `suggested`)
- `fl_03_ac2_feed_never_files` (service integration: a Feed load with `learned` suggestions makes no `set_labels` call)
- `fl_04_ac1_keep_prompt_after_five_keeps` (unit, plus a service integration test that the card carries `keep_prompt`)
- `fl_04_ac1_no_keep_prompt_after_reject` (unit)
- `sw_04_ac1_at_most_two_alternates` (unit, property: any state gives at most two alternates, none equal to the primary)
- `sw_04_ac3_no_categories_confidence_none` (unit)

## Edge cases and traps

- Deterministic ties: tests compare exact output, so always break ties by name.
- `std::time::Instant` is allowed only in the latency test, never in `domain` code.
- Never include a deleted category; categories can be deleted after stats were recorded.
- `suggestion.name` and alternates' names come from the user's own category names; pass them through `plain_text` in the Feed.
- No provider call in the suggestion path; it must stay in memory to meet FL-01 AC3.

## Out of scope

- Creating filing rules when the keep prompt is accepted: T-608 (API-RULE-2 `file`).
- Client timing in Chrome (FL-01 AC3 browser part): T-1003.

## Done when

- The tests above pass and every required check is green (S10 10.1).
- Definition of done in S10 10.4.
