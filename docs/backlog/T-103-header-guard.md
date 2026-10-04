# T-103: Header guard

| Milestone | Tier | Size | Depends on |
| --- | --- | --- | --- |
| M1 | sonnet | about 150 lines of code plus property tests | T-102 |

**Read only these spec sections:** S4 5.2 (`docs/specs/S4-architecture.md`); S10 9.3 (`docs/specs/S10-test-strategy.md`); S2 CL-01 AC2 (`docs/specs/S2-v1-acceptance-criteria.md`); S6 3 row T17. Nothing else is needed.

## Goal

A pure function `header_guard` clamps any classifier's answer to what the headers prove, before it can ever drive the badge: no `list` without a DKIM-covered unsubscribe option, and a model alone cannot move a confident `personal` to `list` or `suspect`. Disagreements are reported so the bake-off can record them. It runs on every card now (T-602c) and matters most the day a model is promoted to the badge.

## Files

| Action | Path | What |
| --- | --- | --- |
| Create | `backend/crates/domain/src/guard.rs` | `header_guard`, `Guarded`, `GuardNote` |
| Change | `backend/crates/domain/src/lib.rs` | `pub mod guard; pub use guard::*;` |

## Types and signatures

```rust
#[derive(Clone, Debug, PartialEq)]
pub struct Guarded {
    pub classification: Classification,   // what may be shown or acted on
    pub notes: Vec<GuardNote>,            // empty when nothing was clamped
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GuardNote {
    ListWithoutCoveredHeader,             // candidate said list; headers do not allow it
    PersonalOverridden,                   // header rules said personal (high confidence); candidate disagreed
}

/// `header_rules` is HeaderRules::classify for the same message; `candidate` is the result to show
/// (header rules themselves during the bake-off, or a promoted model later). `candidate` None means the
/// model failed, timed out or returned invalid output: the header-rules result is used unchanged.
pub fn header_guard(
    facts: &HeaderFacts,
    header_rules: &Classification,
    candidate: Option<&Classification>,
) -> Guarded;
```

## Algorithm

1. `candidate` is `None`: return `header_rules.clone()` with no notes (S4 5.2 third row: failure recorded elsewhere, card unaffected).
2. Start from `c = candidate.clone()`.
3. **Covered header rule** (GUARD-1, CL-01 AC2): if `HeaderRules::unsubscribe_route(facts)` is `UnsubscribeRoute::None` and `c.class == List`, set `c.class` to `BulkNoHeader` when `facts.list_unsubscribe_present`, else to `header_rules.class` if that is not `List`, else `BulkNoHeader`; push `ListWithoutCoveredHeader`.
4. **Personal rule** (GUARD-2): if `header_rules.class == Personal` and `header_rules.bulk_score <= PERSONAL_HIGH_CONFIDENCE_MAX_SCORE` and `c.class` is `List` or `Suspect`, set `c.class = Personal`; push `PersonalOverridden`. `[DEFAULT]` "high confidence" means a header-rules score of 10 or less (T-102 constant); S4 does not define it.
5. When step 3 or 4 changed the class, replace `c.bulk_reason` with `header_rules.bulk_reason` and `c.bulk_score` with `header_rules.bulk_score`, so the shown reason always matches the shown class. Keep `confidence` and `probabilities` from the candidate (they are recorded, not shown).
6. Return `Guarded { classification: c, notes }`.
7. Pure: no clock, no randomness, no I/O.

## Acceptance criteria

| ID | Behaviour (one line) |
| --- | --- |
| GUARD-1 | Without a valid, DKIM-covered `List-Unsubscribe`, the guarded class is never `list`, whatever the model says |
| GUARD-2 | When header rules say `personal` with high confidence, a model alone never makes it `list` or `suspect`, and the disagreement is noted |
| CL-01 AC2 | The guard keeps `list` off any message without a covered header, for every classifier |

## Tests that must pass

- `guard_1_no_list_without_valid_header` (property: any facts with route `None`, any header-rules result, any candidate or `None`: guarded class is not `List`)
- `guard_1_note_recorded_when_clamped` (unit)
- `guard_2_personal_not_overridden` (property: header rules `Personal` with score at most 10, any candidate: guarded class is neither `List` nor `Suspect`, and a `PersonalOverridden` note exists whenever the candidate said `List` or `Suspect`)
- `guard_2_low_confidence_personal_can_change` (unit: header rules `Personal` with score 25, candidate `Notice`: result `Notice`, no note)
- `cl_01_ac2_guard_holds_for_header_rules_as_candidate` (property: candidate = header rules result itself gives back the same class)
- `guard_model_failure_uses_header_rules` (unit)
- `guard_reason_matches_class_after_clamp` (unit)

GUARD-3 (identical side effects with and without model output) needs the swipe planner and lives in T-105a.

## Edge cases and traps

- The guard must call `HeaderRules::unsubscribe_route`, not trust a `has_one_click` flag or a field on the candidate.
- Candidate probabilities may be `NaN`; the guard never does arithmetic on them, so it cannot panic. Generate `NaN` and infinities in the property strategy to prove it.
- Do not "fix" a candidate's `bulk_score` outside 0 to 100 here; parse-time validation is T-904 and T-905. Just never index or divide by it.
- Notes are data for the evaluation record (T-906); never log them with message details.
- Use a shared `facts_strategy()` from T-102's test module if it is `pub(crate)`; otherwise copy it into this module's tests.

## Out of scope

- Recording disagreements in `classifier_eval`: T-906. Calling models: T-904, T-905. Swipe side effects (GUARD-3): T-105a.

## Done when

- The tests above pass and every required check is green (S10 10.1).
- Definition of done in S10 10.4.
