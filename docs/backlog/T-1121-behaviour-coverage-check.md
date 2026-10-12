# T-1121: Make the behaviour tables of newer tasks enforceable (the AC manifest entry for them enforces nothing today)

| Milestone | Tier | Size | Depends on |
| --- | --- | --- | --- |
| M11 | sonnet | small: about 80 lines of Python plus tests in `tools/` | T-1120a |

**Read only these spec sections:** `tools/check_ac_coverage.py` (`parse_task_acs` line 146, the enforced-task loop line 357, `has_test` line 263, `test_key` line 173), `tools/test_check_ac_coverage.py`, `tools/ac_coverage_enforced.txt` (header comment). Nothing else is needed. Do NOT edit `CLAUDE.md` or `.github/`.

## Why this task exists (found by the review of PR #96, finding S-1, 12 Oct 2026)

`CLAUDE.md:14` says a merged task's ID goes into `tools/ac_coverage_enforced.txt`. For an enforced task the checker reads only the table under the heading `## Acceptance criteria` (`parse_task_acs`, lines 146 to 170) and checks that each listed ID has a test. The newer task specs (T-1104b, T-1119, T-1120a/b, T-1104d) have no such heading: their claims live in a `## Behaviours that must hold` table with ids `B1`, `B2`, ... Enrolling them in the manifest therefore enforces nothing: the checker passes with zero rows checked. The builders follow the rule faithfully and get no protection from it.

## Goal

An enrolled task whose spec has a `## Behaviours that must hold` table fails CI unless every `B<n>` row has a test that carries it in its name, and the checker says plainly when an enrolled task checks zero rows.

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
| B1 | A task in the opt-in list whose behaviour table has a row with no matching test fails with the message above |
| B2 | The same task passes once a Rust test named `<task>_<id>_...` exists (and a Dart test key works the same way) |
| B3 | A task NOT in the opt-in list is unaffected, whatever its table says |
| B4 | An enrolled task with no checkable rows prints the warning line and exits 0 |
| B5 | The real repository still passes (`python3 tools/check_ac_coverage.py` exits 0) |

## Tests that must pass

Python unit tests for B1 to B4 in `tools/test_check_ac_coverage.py` (use temporary directories like the existing tests), and B5 as the normal CI run. All required checks green (S10 10.1).

## Edge cases and traps

- The opt-in list starts EMPTY on purpose: tasks already merged (T-1104b, T-1119, T-1120a, T-1104d) use other test names; do not rename their tests here. A later task adds itself in its own PR and names its tests accordingly.
- Do not change how S2/S3/S5/S10/ASVS ids are resolved.
- A one-line note in the opt-in file's header explains the naming rule; do not edit `CLAUDE.md` (owner-only); the owner may fold the rule into it later.

## Out of scope

Back-filling tests for merged tasks, the ASVS register, workflow changes.

## Done when

Tests green, definition of done in S10 10.4.
