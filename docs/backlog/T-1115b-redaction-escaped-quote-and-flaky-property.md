# T-1115b: Redaction: escaped quotes in quoted local parts, and make the `@` property deterministic

| Milestone | Tier | Size | Depends on |
| --- | --- | --- | --- |
| M9 | sonnet | small: about 40 lines of code plus tests | T-1115 |

**Read only these spec sections:** `docs/backlog/T-1115-redaction-hardening.md` (Algorithm 1, EXP-2); S4 5.5. Nothing else is needed.

**Why this task exists (verified 11 Oct 2026 from `main`).** The independent review of PR #20 (finding F11, reproduced twice with the real library on synthetic data) and a CI failure on PR #72 (run 38104948434, job `rust`, 11 Oct 02:25 UTC, `redact.rs:87`) show the same defect:
- `backend/crates/domain/src/redact.rs:54`: `email_quoted` is `"[^"]*"@\S+`. A quoted local part may contain an escaped quote (`\"`), and `[^"]*` stops at it. Input `"private name\" suffix"@example.com` becomes `"private name\[email]`: the engine skips the first quote (no `"@` follows the escaped one) and matches only from the escaped quote onward, so the identifying prefix `private name` survives into the model input.
- `backend/crates/domain/tests/redact.rs:80-88` property `exp_2_redact_no_at_sign_left_in_adjacent_text` draws `left` and `right` from `[^\s\p{C}]{1,40}` (any non-space, non-control character, including `"`, `\` and `@`) and asserts the output has no `@`. Random inputs of this kind occasionally hit the gap, so `main` and every PR is intermittently red in CI (main was green at 02:22 UTC, #72 red at 02:25 UTC, same code). A flaky security test is worse than none: people re-run it.

## Goal

No part of an address survives `redact`, including a quoted local part containing quoted-pairs, and the property test fails or passes reproducibly.

## Files

| Action | Path | What |
| --- | --- | --- |
| Change | `backend/crates/domain/src/redact.rs` | `email_quoted` (line 54) and, if the property still finds counterexamples, the order or shape of the email patterns (lines 54 to 57, 91 to 96) |
| Change | `backend/crates/domain/tests/redact.rs` | table rows below; pin the counterexamples; keep the property |
| Create | `backend/crates/domain/tests/redact.proptest-regressions` | the failing inputs proptest prints ("minimal failing input"), committed so they re-run first every time |

## Algorithm

1. Make the quoted local part honour quoted-pairs: a quoted string is `"` then any run of (a character other than `"` and `\`) or (`\` followed by any character), then `"`, then `@` and the rest of the run: `"(?:[^"\\]|\\.)*"@\S+`. The `regex` crate is linear-time; keep it unbounded or bounded generously (the 65-character case in the table must still match).
2. Run `cargo test -p domain --test redact` repeatedly with a large case count (for example `PROPTEST_CASES=20000`) until the property passes; every counterexample proptest prints is added to the table AS A CONCRETE ROW and to the regressions file, then fixed in the pattern. Do not weaken the property or its generator.
3. If a counterexample shows a pattern ordering problem (a partial replacement leaving an `@`), fix the pattern or add a final pass that replaces any remaining `\S*@\S*` run; do not delete an `@` in isolation.
4. Set proptest's failure persistence so regressions are written next to the test (`ProptestConfig { failure_persistence: ... }`), not the `SourceParallel` default that logs "failed to find lib.rs or main.rs" in CI.

## Acceptance criteria

| ID | Behaviour (one line) |
| --- | --- |
| EXP-2 | A quoted local part containing an escaped quote, a backslash or spaces is replaced as a whole by `[email]` |
| EXP-2 | The `@`-adjacent-text property passes at 20,000 cases on three consecutive runs and its past counterexamples are pinned |

## Tests that must pass

- Table rows added to `exp_2_redact_table`: `"private name\" suffix"@example.com`, `Mail "a\\"@b.test now`, `"a\"b c\"d"@x.example`, and every counterexample proptest prints (all give `[email]` or the sentence with `[email]` in place of the address).
- `exp_2_redact_no_at_sign_left_in_adjacent_text` unchanged in meaning; `exp_2_redact_idempotent` and `exp_2_hostile_input_bounded` still pass.
- `cargo test -p domain --test redact` with `PROPTEST_CASES=20000` three times in a row, recorded in the PR description.

## Edge cases and traps

- Idempotence: the output of `redact` must be unchanged by a second `redact`.
- Do not log inputs or counterexamples to any log; counterexamples in test files must be synthetic (the existing table already uses reserved example domains).
- Do not touch `from_domain` (T-1115 F12 is closed).

## Out of scope

- Sealed-token handling (T-1115c).

## Done when

- The tests above pass and every required check is green (S10 10.1); Definition of done in S10 10.4.
