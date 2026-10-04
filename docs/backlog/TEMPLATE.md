# T-xxx: Title

| Milestone | Tier | Size | Depends on |
| --- | --- | --- | --- |
| Mx | sonnet or strong | about N lines of code plus tests | T-yyy, T-zzz |

**Read only these spec sections:** S7 5.4 (`docs/specs/S7-api-contract.md`), S2 FD-01 to FD-04, ... Nothing else is needed.

## Goal

Two or three sentences: what exists after this task that did not before, and who uses it.

## Files

| Action | Path | What |
| --- | --- | --- |
| Create | `backend/crates/domain/src/feed.rs` | Feed ordering |
| Change | `backend/crates/domain/src/lib.rs` | `pub mod feed;` |

## Types and signatures

```rust
// Exact public items the task adds. Builders keep these names and shapes.
```

## Algorithm

Numbered, "good enough" steps. Prefer the simple version; note where a smarter version is deliberately left for later.

## Acceptance criteria

| ID | Behaviour (one line) |
| --- | --- |
| FD-01 AC1 | ... |

`tools/check_ac_coverage.py` reads this section. Every ID here needs a test whose name carries it.

## Tests that must pass

- `fd_01_ac1_one_card_returned` (unit, `domain`)
- ...

## Edge cases and traps

Things a builder is likely to get wrong, each one line.

## Out of scope

What a later task does instead (name it).

## Done when

- The tests above pass and every required check is green (S10 10.1).
- Definition of done in S10 10.4.
- Anything specific to this task.
