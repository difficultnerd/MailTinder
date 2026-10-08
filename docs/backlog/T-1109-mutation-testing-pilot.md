# T-1109: Mutation testing pilot: do the tests actually fail when the code is wrong?

| Milestone | Tier | Size | Depends on |
| --- | --- | --- | --- |
| M11 | sonnet | about 250 lines of code plus tests | T-004, T-104, T-105a, T-109 |

**Read only these spec sections:** S10 section on coverage floors; `tools/check_coverage_floors.py` and `tools/setup-local-checks.sh` (the style to copy). Nothing else is needed.

## Goal

Coverage says a line ran; mutation testing says a test would notice if the line were wrong. A pilot gate measures this on the highest-value pure logic, using `cargo-mutants`, and records a baseline score per area so it can only go up.

## Files

| Action | Path | What |
| --- | --- | --- |
| Create | `tools/mutation.sh` | `tools/mutation.sh [--area NAME] [--shard I/N] [--jobs N]`: runs `cargo mutants` for the files listed in `tools/mutation_scope.txt`, writes `target/mutants/` and `target/mutants/summary.json` |
| Create | `tools/mutation_scope.txt` | One line per area: `area: path, path` (pilot: `rules: backend/crates/domain/src/rules.rs`, `swipe: .../swipe.rs`, `undo: .../undo.rs`, `header_rules: .../header_rules.rs`, `redact: .../redact.rs`, `gamification: .../gamification.rs`, `feed: .../feed.rs`) |
| Create | `tools/mutation_report.py` | Reads the cargo-mutants outcomes, prints per area: caught, missed, unviable, timeout, score (caught / (caught + missed)), and the list of **missed** mutants with file and line; exits non-zero if an area is below its floor |
| Create | `tools/mutation_baseline.json` | `{ "area": { "floor": <percent> } }`, initially set to the measured score minus 2 points |
| Create | `tools/test_mutation_report.py` | Unit tests for the report script (like `test_check_coverage_floors.py`) |
| Change | `tools/setup-local-checks.sh` | Install `cargo-mutants` at a pinned version with a checksum, into `tools/.bin` |
| Create | `docs/mutation-testing.md` | How to run it, how to read a missed mutant, how a nightly run on verify1 is scheduled |

## Behaviour

1. Default run is NOT part of `tools/ci-local.sh` (too slow). It is a separate command; a nightly run on verify1 will call `tools/mutation.sh` (the owner or the operator installs that cron; the factory does not edit workflows or cron).
2. `--shard I/N` and `--jobs` must work so the 8-core verify1 can finish the pilot in under 60 minutes; record the real duration in the PR.
3. Mutants in test code, `#[cfg(test)]` modules and generated code are excluded. A mutant that times out counts as caught.
4. `mutation_report.py` has a `--markdown` mode that produces a table the operator can paste into an issue.
5. For each area, the **missed mutants are listed with their line**; the PR body must contain the full list for the first run, and for the three most important areas (`rules`, `swipe`, `undo`) the PR adds tests that kill at least five missed mutants, or explains in one line each why a mutant is equivalent.

## Acceptance criteria

- `mutation_report_scores_and_floors` (python): fixture outcomes give the expected score, and a score below the floor exits 1.
- `mutation_report_lists_missed_mutants_with_lines`.
- `mutation_script_refuses_unlisted_paths` (shell test: a path outside `mutation_scope.txt` is rejected).
- `mutation_baseline_matches_measured_run` (not a test: the PR body shows the real first-run table).

## Out of scope

Putting the mutation run in CI; mutating adapters or the API crate; fixing every surviving mutant.

## Done when

The python tests pass, the belt is green, the PR body shows the first full run (table, duration, missed list) and the new killing tests; definition of done in S10 10.4. Do not edit `.github/workflows/`.
