# T-1121: Make the behaviour tables of newer tasks enforceable (the AC manifest entry for them enforces nothing today)

| Milestone | Tier | Size | Depends on |
| --- | --- | --- | --- |
| M11 | sonnet | small: about 80 lines of Python plus tests in `tools/` | T-1120a |

**Read only these spec sections:** `tools/check_ac_coverage.py` (`parse_task_acs` line 146, the enforced-task loop line 357, `has_test` line 263, `test_key` line 173), `tools/test_check_ac_coverage.py`, `tools/ac_coverage_enforced.txt` (header comment). Nothing else is needed. Do NOT edit `CLAUDE.md` or `.github/`.

## Why this task exists (found by the review of PR #96, finding S-1, 12 Oct 2026)

`CLAUDE.md:14` says a merged task's ID goes into `tools/ac_coverage_enforced.txt`. For an enforced task the checker reads only the table under the heading `## Acceptance criteria` (`parse_task_acs`, lines 146 to 170) and checks that each listed ID has a test. The newer task specs (T-1104b, T-1119, T-1120a/b, T-1104d) have no such heading: their claims live in a `## Behaviours that must hold` table with ids `B1`, `B2`, ... Enrolling them in the manifest therefore enforces nothing: the checker passes with zero rows checked. The builders follow the rule faithfully and get no protection from it.

## Goal

An OPTED-IN task fails CI unless its spec resolves to exactly one file with at least one recognised behaviour row and every row has a REAL test (Rust, Dart or Python) that carries the row id in its name; and the checker says plainly, with a count, when an enrolled task enforces zero criteria.

## Files

| Action | Path | What |
| --- | --- | --- |
| Change | `tools/check_ac_coverage.py` | parse behaviour tables; new opt-in manifest; zero-rows message |
| Create | `tools/behaviour_coverage_enforced.txt` | opt-in list: header comment plus `T-1121` only (so no merged task is retro-broken); later tasks add themselves |
| Change | `tools/test_check_ac_coverage.py` | tests below |

## Algorithm

1. `parse_task_behaviours(text)`: rows under `## Behaviours that must hold` whose first cell matches `^(B[0-9]+|S-[0-9]+)$`. Return the ids.
2. **Key form.** For task `T-1121` and id `B2` the required name prefix is `t1121_b2_`; for `S-1` it is `t1104d_s_1_` (lowercase the task id, remove its dash, lowercase the row id and replace `-` with `_`).
3. **Real tests only (this check uses its own collectors; the existing `rust_test_names` / `dart_test_keys` used by the AC check are NOT changed).** Rust: a `fn` whose preceding non-blank attribute lines include `#[test]`, `#[tokio::test]` or a `#[test_case` form, with comments stripped before scanning, so a helper or a commented-out `fn` never counts. Dart: a `test(` / `testWidgets(` call whose arguments contain no `skip:`. Python: a function named `test_<prefix>...` (for example `test_t1121_b2_...`) in `tools/test_*.py` or `scripts/test_*.py`, outside comments.
4. **Opted-in tasks** (listed in `tools/behaviour_coverage_enforced.txt`): FAIL if the task file does not resolve uniquely (`docs/backlog/<id>-*.md`), if the behaviour table is absent or has no recognised rows, or if any row has no real test with its prefix. Message for a missing test: `<task path>: <id>: missing test (expected a name starting t1121_b2_)`.
5. **Diagnostic for everything else.** For each task in `ac_coverage_enforced.txt`, count the criteria actually enforced (AC rows plus behaviour rows IF the task is opted in). If the count is 0, print `<task>: enrolled but enforces 0 criteria (<n> behaviour rows exist and the task is not opted in)`; a warning, exit code unchanged, so grandfathered tasks stay visible without breaking.
6. Exit codes and messages for the existing paths do not change.

## Behaviours that must hold` table with ids `B1`, `B2`, ... Enrolling them in the manifest therefore enforces nothing: the checker passes with zero rows checked. The builders follow the rule faithfully and get no protection from it.

## Goal

An OPTED-IN task fails CI unless its spec resolves to exactly one file with at least one recognised behaviour row and every row has a REAL test (Rust, Dart or Python) that carries the row id in its name; and the checker says plainly, with a count, when an enrolled task enforces zero criteria.

## Files

| Action | Path | What |
| --- | --- | --- |
| Change | `tools/check_ac_coverage.py` | parse behaviour tables; new opt-in manifest; zero-rows message |
| Create | `tools/behaviour_coverage_enforced.txt` | opt-in list, starts with a header comment and NO task (so no merged task is retro-broken); later tasks add themselves |
| Change | `tools/test_check_ac_coverage.py` | tests below |

## Algorithm

1. `parse_task_behaviours(text)`: rows under `## Behaviours that must hold` whose first cell matches `^B[0-9]+$` (also `S-<n>` for the hardening tasks). Return the ids.
2. For every task in `tools/behaviour_coverage_enforced.txt`: for each id, require a Rust or Dart test whose name starts with `<task lower, dashes removed>_<id lower>_` (for example `t1121_b2_`), using the existing `has_test`. Missing: print `<task path>: <id>: missing test (expected a name starting t1121_b2_)` and fail the run.
3. For any task in `ac_coverage_enforced.txt` that has NEITHER an `## Acceptance criteria` table with rows NOR a behaviour table, print a warning line `<task>: enrolled but no checkable rows` (a warning, not a failure, so existing tasks do not break); the opt-in list is the place to turn it into a failure.
4. Exit codes and messages for the existing paths do not change.

## Behaviours that must hold

| # | Behaviour |
| --- | --- |
| B1 | A task in the opt-in list whose behaviour table has a row with no real matching test fails with the message above |
| B2 | The same task passes once a Rust test named `<task>_<id>_...` exists (and a Dart test key works the same way) |
| B3 | A task NOT in the opt-in list is unaffected, whatever its table says |
| B4 | An AC-enrolled task that is not opted in and enforces 0 criteria prints the warning with its row count and exits 0 (including a task that has behaviour rows) |
| B5 | The real repository still passes (`python3 tools/check_ac_coverage.py` exits 0) |
| B6 | An opted-in task whose file is missing or ambiguous, whose heading was deleted, or whose table has no recognised rows FAILS |
| B7 | A helper `fn`, a commented-out `fn`, and a Dart test with `skip:` do NOT satisfy a row; a `#[test]` fn, an active Dart test and a Python `test_<prefix>` function do |
| B8 | `S-1` of `T-1104d` is looked up as `t1104d_s_1_` and found |

## Tests that must pass

Python unit tests for B1 to B4 and B6 to B8 in `tools/test_check_ac_coverage.py` (named with the new convention, e.g. `test_t1121_b1_...`, and added to the opt-in list in this same PR so the task eats its own cooking) (use temporary directories like the existing tests), and B5 as the normal CI run. All required checks green (S10 10.1).

## Edge cases and traps

- The opt-in list starts with only `T-1121` itself; every OTHER task is left out on purpose: tasks already merged (T-1104b, T-1119, T-1120a, T-1104d) use other test names; do not rename their tests here. A later task adds itself in its own PR and names its tests accordingly.
- Do not change how S2/S3/S5/S10/ASVS ids are resolved.
- A one-line note in the opt-in file's header explains the naming rule; do not edit `CLAUDE.md` (owner-only); the owner may fold the rule into it later.

## Out of scope

Back-filling tests for merged tasks, the ASVS register, workflow changes.

## Done when

Tests green, definition of done in S10 10.4.
