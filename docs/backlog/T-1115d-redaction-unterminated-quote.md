# T-1115d: Redaction: an unterminated quote with an escaped quote leaves the name prefix

| Milestone | Tier | Size | Depends on |
| --- | --- | --- | --- |
| M9 | sonnet | tiny: about 10 lines of code plus tests | T-1115b |

**Read only these spec sections:** `backend/crates/domain/src/redact.rs` (the `email_quoted`, `email_literal` and `email_at` patterns and the order of the `replace_all` calls); `backend/crates/domain/tests/redact.rs`; `docs/backlog/T-1115b-redaction-escaped-quote-and-flaky-property.md`. Nothing else is needed.

**Why this task exists (reported 11 Oct 2026 by the Sol review of PR #72, finding R2; checked here by reading the patterns, NOT yet reproduced by running the test: the first step is to reproduce).** T-1115b (PR #84) changed `email_quoted` from `"[^"]*"@\S+` to a quoted-pair-aware pattern (`"(?:[^"\\]|\\.)*"@\S+`). The review reports that a quoted local part whose only closing quote is escaped, for example `Mail "private name\"@example.com now`, is no longer matched as a whole: the new pattern cannot close the string, so only the later `\S*@\S+` pass runs and the output is `Mail "private [email] now`, leaving the identifying prefix `private` (with the opening quote). The old pattern treated the backslash literally and consumed the whole address. So the T-1115b fix regressed this malformed-input shape while fixing the well-formed escaped-quote shape.

## Goal

No part of a quoted local part survives redaction, whether the quoted string is well formed, contains escaped quotes, or is unterminated.

## Files

| Action | Path | What |
| --- | --- | --- |
| Change | `backend/crates/domain/src/redact.rs` | add a fallback pattern or ordering so an opening quote followed by text up to an address-like `@` run is replaced as a whole |
| Change | `backend/crates/domain/tests/redact.rs` | table rows below; keep the committed proptest regressions |

## Algorithm

0. **Reproduce first.** Add the failing row `Mail "private name\"@example.com now` (expected `Mail [email] now`) and run `cargo test -p domain --test redact` to see it fail. Write the observed output in the PR description.
1. After the quoted-pair pattern, add a fallback written so that NOTHING can consume the `@`: an opening quote, then a LAZY run of any non-quote character or a backslash-quote pair, then an `@` run: `"(?:[^"]|\\")*?@\S+`. A backslash is an ordinary character here (`[^"]` takes it), so a backslash immediately before the `@` (`"private name\@example.com`) cannot hide it; and `\"` is also accepted as a pair so an escaped closing quote does not stop the run (`"private name\"@example.com`). The earlier candidates fail: `"[^"\n]*@\S+` cannot cross the escaped quote (review D1), and `"(?:[^"\\]|\\.)*@\S+` lets `\\.` swallow a backslash+`@` pair so the prefix survives when the sole `@` follows a backslash (review D5). Lazy matching stops at the FIRST `@`; the other email patterns handle the rest of the address. Over-redaction of prose between a stray quote and an address is accepted (it fails closed for privacy) and must be shown in a test row.
2. Do not weaken any existing table row or the committed `redact.proptest-regressions` file (it holds one seed, `word a@[!]@ word`); the earlier counterexample `word ®@[𐞀]@ word` is a table row in `redact.rs`, not a persisted seed: keep that row.
3. **Bump the redaction method version** (`backend/crates/domain/src/redact.rs:8`, currently `"1"`) to `"2"` and record in the PR description that T-1115, T-1115b and this task changed redaction behaviour without a bump: the classifier evaluation records the input method version (S5 `input_version`), so results from before and after must be distinguishable. Update any test that asserts the version.

## Acceptance criteria

| ID | Behaviour |
| --- | --- |
| EXP-2 | `Mail "private name\"@example.com now` gives `Mail [email] now` |
| EXP-2 | Well-formed quoted addresses, escaped-quote addresses and address literals still give `[email]` as before |

## Tests that must pass

- New table rows: the reproduction above; `"a\"b"@x.example`; `"unterminated name@x.example`; `"\"@x`; `"private name\@example.com` (a backslash immediately before the only `@`, review D5). All previous rows pass.
- `exp_2_redact_no_at_sign_left_in_adjacent_text`, the idempotence property and `exp_2_hostile_input_bounded` pass at `PROPTEST_CASES=20000`, three runs in a row.
- All required checks green (S10 10.1); Definition of done S10 10.4.

## Edge cases and traps

- A fallback that is too greedy could swallow non-address text between a stray quote and a distant `@`; the pattern must stop at whitespace-normalised boundaries only as far as the existing `\S+` allows; add TWO rows: (a) ordinary quoted prose without any `@` is untouched; (b) prose with a stray quote, several words, then an ordinary address (`He said "hello there and write to a@b.example today`) loses the address and, by design, the words between the stray quote and the address (assert no `@` and no part of the address remain; the prose is allowed to be redacted).
- Do not log inputs; counterexamples in tests must be synthetic.

## Out of scope

- Token handling, other redaction patterns, `from_domain`.

## Done when

- The tests above pass and every required check is green; Definition of done S10 10.4.
