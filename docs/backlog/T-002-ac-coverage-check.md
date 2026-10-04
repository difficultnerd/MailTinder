# T-002: AC coverage script and CI job

| Milestone | Tier | Size | Depends on |
| --- | --- | --- | --- |
| M0 | sonnet | about 300 lines of Python plus 150 lines of unittest | T-001 |

**Read only these spec sections:** S10 2 "Traceability" (test naming and coverage check), S10 7.1 (verification methods), S10 10.1 (`docs/specs/S10-test-strategy.md`); S6 8 row "ASVS register coverage" (`docs/specs/S6-security.md`); the header lines and one table of `docs/security/asvs-l2-register.md` (to see the row format); the "Deletion tests" table of S5 (`docs/specs/S5-data-inventory.md`); `docs/backlog/TEMPLATE.md`. Skim S2 only to see how story headings and AC bullets are written. Nothing else is needed.

## Goal

A stdlib-only Python script, `tools/check_ac_coverage.py`, proves that every acceptance criterion, invariant, S5 verification ID, classifier test ID and ASVS row named by a merged task file has a test whose name carries that ID, and prints overall S2 coverage. A new CI job `ac-coverage` runs it on every push and pull request. T-005 makes the job a required check.

## Files

| Action | Path | What |
| --- | --- | --- |
| Create | `tools/check_ac_coverage.py` | The checker (stdlib only, Python 3.12) |
| Create | `tools/test_check_ac_coverage.py` | `unittest` tests with small inline fixtures |
| Create | `tools/ac_coverage_enforced.txt` | One task ID per line: tasks whose AC tables are enforced |
| Change | `.github/workflows/ci.yml` | New job `ac-coverage` |

## Types and signatures

```python
# tools/check_ac_coverage.py
from dataclasses import dataclass
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent

@dataclass(frozen=True)
class KnownId:
    id: str            # exactly as written in the source, e.g. "SW-03 AC2", "INV-5", "V3.4.3", "SES-1", "GUARD-1"
    source: str        # "S2", "S3", "S5", "S10", "ASVS"
    active: bool       # False for S2 ACs marked "Removed" or "v2."
    verify: str | None # ASVS only: "test", "semgrep", "ci", "dast", "review", "-"
    status: str | None # ASVS only: "Planned", "Verified", ...

def parse_s2(text: str) -> list[KnownId]: ...
def parse_s3_invariants(text: str) -> list[KnownId]: ...
def parse_s5_ids(text: str) -> list[KnownId]: ...
def parse_s10_ids(text: str) -> list[KnownId]: ...
def parse_register(text: str) -> list[KnownId]: ...
def parse_task_acs(text: str) -> list[str]: ...           # first column of the "## Acceptance criteria" table
def test_key(known_id: str) -> str: ...                    # "SW-03 AC2" -> "sw_03_ac2"; "V3.4.3" -> "asvs_v3_4_3"
def rust_test_names(root: Path) -> set[str]: ...
def dart_test_keys(root: Path) -> set[str]: ...             # normalised descriptions
def semgrep_rule_ids(root: Path) -> set[str]: ...
def has_test(key: str, rust: set[str], dart: set[str]) -> bool: ...
def main(argv: list[str] | None = None) -> int: ...         # 0 ok, 1 missing coverage, 2 input error
```

## Algorithm

1. **Load sources** (paths relative to `ROOT`): `docs/specs/S2-v1-acceptance-criteria.md`, `docs/specs/S3-domain-model.md`, `docs/specs/S5-data-inventory.md`, `docs/specs/S10-test-strategy.md`, `docs/security/asvs-l2-register.md`, every `docs/backlog/T-*.md`, and `tools/ac_coverage_enforced.txt`. A missing file is exit code 2 with its path.
2. **S2.** Walk lines. A heading `### XX-NN Title` (regex `^### ([A-Z]{2}-\d{2})\b`) sets the current story. A bullet `- **ACn.**` or `- **ACna.**` (regex `^- \*\*(AC\d+[a-z]?)\.\*\*\s*(.*)`) gives the ID `"<story> <ACn>"`. It is inactive when the text after the bullet starts with `Removed` or `v2.`. Cross-cutting bullets `- **XC-0n.**` give IDs `XC-0n` (no story).
3. **S3.** Regex `\*\*(INV-\d+)\.\*\*`.
4. **S5.** Regex over the whole file: `\b((?:DEL|SES|CFG|JOB|RL|LOG|EXP|JEV)-\d+|(?:INV|NA)-T\d+)\b`. Deduplicate.
5. **S10.** Regex `\b((?:JEV|BAKE|EXP|GUARD|STAT)-\d+)\b`. Deduplicate (EXP and JEV IDs also appear in S5; keep one entry).
6. **Register.** Table rows matching `^\| (V\d+\.\d+\.\d+) \| (\d) \| .* \| ([^|]*) \| ([^|]*) \|\s*$`: the last two cells are `Verify` and `Status`. `verify` is `test` when the cell starts with `test`, else the cell text lower-cased and stripped (`semgrep`, `ci`, `dast`, `review`, `-`).
7. **Test key.** `test_key(id)`: if the ID matches `^V\d+(\.\d+)+$`, return `"asvs_" + id.lower().replace(".", "_")`. Otherwise lower-case, replace every run of characters outside `[a-z0-9]` with one `_`, strip `_` at both ends. Examples: `SW-05 AC4a` gives `sw_05_ac4a`; `INV-T1` gives `inv_t1`; `XC-01` gives `xc_01`.
8. **Rust test names.** For every `*.rs` under `backend/` (skip any path containing `/target/`), collect every identifier after `fn ` with regex `\bfn\s+([a-z0-9_]+)\s*[<(]`. This includes functions inside `proptest! { ... }` blocks.
9. **Dart test keys.** For every `*.dart` under `app/test/` and `app/integration_test/` (if present), regex `\b(?:test|testWidgets)\(\s*(['"])(.+?)\1` gives each description. Normalise a description with the same rule as step 7's second branch (lower-case, non-alphanumeric runs to `_`). So `'SW-03 AC2 reject toast states the delay'` becomes `sw_03_ac2_reject_toast_states_the_delay` and `'ASVS V14.3.1 wipes state on 401'` becomes `asvs_v14_3_1_wipes_state_on_401`.
10. **Match.** `has_test(key, rust, dart)` is true when some name equals `key` or starts with `key + "_"`. The underscore stops `sw_03_ac2` matching `sw_03_ac21_x` and `inv_5` matching `inv_51`.
11. **Semgrep rule IDs.** For every `*.yml` and `*.yaml` under `.semgrep/`, regex `^\s*-\s*id:\s*([A-Za-z0-9_.-]+)` (multiline). A register row with `verify == "semgrep"` is satisfied by a rule ID starting with `"asvs-" + id.lower().replace(".", "-") + "-"` (for example `asvs-v1-2-3-no-string-json`).
12. **Task AC tables.** `parse_task_acs`: find the line `## Acceptance criteria`; read table rows until the next `## ` heading; skip the header and separator rows; take the first cell, strip spaces and backticks. A cell of `None` or empty is skipped. A cell matching `^s9_[a-z0-9_]+$` is an S9 state test name and must exist exactly as a Rust name or Dart key prefix.
13. **Enforcement.** Read `tools/ac_coverage_enforced.txt` (blank lines and `#` comments ignored). For each listed task ID, open the one file `docs/backlog/<ID>-*.md` (exactly one match, else exit 2). For each ID in its AC table:
    1. Unknown ID (not found in steps 2 to 6 and not an `s9_` name): error `unknown ID`.
    2. Inactive S2 ID (Removed or v2): error `inactive ID`.
    3. Register row with `verify` `test`, or any non-register ID: needs `has_test`. Register row with `semgrep`: needs a rule ID (step 11). Register rows with `ci`, `dast`, `review`: no check (S10 7.1 gates those elsewhere).
14. **Always enforced, whatever the task list:** every register row whose `status` starts with `Verified` and whose `verify` is `test` or `semgrep` must have its test or rule (S6 8).
15. **Report** (always printed to stdout, and appended to the file named by the `GITHUB_STEP_SUMMARY` environment variable when it is set):
    - one line per failure: `<task file>: <ID>: missing test (expected a name starting <key>)`;
    - a table per S2 story: story ID, active ACs, ACs with a test, list of uncovered AC IDs;
    - totals: S2 ACs covered of active, INV covered of 7, register `test` rows covered of their total.
16. **Exit code** 0 when nothing failed, 1 when any check in steps 13 or 14 failed, 2 for an input error. `main` takes `argv` so tests can pass `--root <tmpdir>` to point at a fixture tree.
17. **Seed `tools/ac_coverage_enforced.txt`** with a two-line comment header explaining the file (the builder of every later task adds its own task ID in its pull request; a split task adds both IDs, for example `T-105a`), then `T-001`, `T-002`, and every other task ID already merged to `main` when this pull request opens (check merged pull request titles `T-xxx: ...`). `[DEFAULT]` S10 2 says only "tasks merged so far" are enforced but does not say how the script knows; an explicit list kept in the same pull request as the task is the simplest reliable signal.
18. **CI job** in `.github/workflows/ci.yml`, beside `language-policy`:

```yaml
  ac-coverage:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1 # v7
      - run: python3 -m unittest discover -s tools -p 'test_*.py'
      - run: python3 tools/check_ac_coverage.py
```

The runner's own `python3` is enough; do not add `actions/setup-python` and do not `pip install` anything.

## Acceptance criteria

| ID | Behaviour (one line) |
| --- | --- |
| None | This task builds the checker itself. Its behaviour is proven by the `unittest` cases below, which `check_ac_coverage.py` does not scan |

## Tests that must pass

All in `tools/test_check_ac_coverage.py` (`unittest`, level: unit, run by the `ac-coverage` job). Build fixtures as strings or in `tempfile.TemporaryDirectory()`; never read the real specs in these tests.

- `test_parse_s2_story_and_ac_ids` (gives `AU-01 AC1`, `AU-03 AC4a`, `XC-01`)
- `test_parse_s2_removed_and_v2_are_inactive`
- `test_parse_s3_invariants`
- `test_parse_s5_ids_include_inv_t_and_na_t`
- `test_parse_s10_classifier_ids`
- `test_parse_register_verify_and_status`
- `test_test_key_examples` (`SW-03 AC2`, `SW-05 AC4a`, `V3.4.3`, `INV-T1`, `GUARD-1`)
- `test_rust_names_found_in_proptest_block`
- `test_dart_description_normalised`
- `test_prefix_match_needs_underscore_boundary` (`sw_03_ac21_x` does not cover `SW-03 AC2`)
- `test_enforced_task_missing_test_fails_with_exit_1`
- `test_unenforced_task_missing_test_passes`
- `test_unknown_id_in_task_fails`
- `test_inactive_id_in_task_fails`
- `test_semgrep_row_needs_rule_id`
- `test_verified_register_row_always_enforced`
- `test_s9_state_name_needs_exact_test`
- `test_missing_source_file_exit_2`
- `test_report_written_to_step_summary`

## Edge cases and traps

- Standard library only (`re`, `pathlib`, `argparse`, `os`, `sys`, `dataclasses`, `unittest`, `tempfile`). No `pyyaml`, no `toml`; Semgrep files are scanned with a regex.
- Python goes only in `tools/` (CLAUDE.md, `tools/check_language_policy.sh`). Never put a helper under `backend/` or `app/`.
- S2 AC letters are lower case (`AC4a`); keep them as written in the ID and in the key.
- `test_key` for ASVS rows adds `asvs_`; for every other ID it does not. `INV-5` becomes `inv_5`, not `asvs_...`.
- Do not treat `EXP-1` from S5 and `EXP-1` from S10 as two IDs.
- A task file can list the same ID as another task (for example `INV-2` in T-106 and T-107). That is fine; one test anywhere satisfies both.
- Do not fail on S2 ACs that no enforced task names. Overall coverage is a report, not a gate, until every story is built (S10 2).
- The register has 253 requirement rows plus 17 summary rows that also start with `| V` (the chapter summary table, three cells). Only rows with seven cells and an ID like `V1.2.3` are requirements. Read `Verify` and `Status` as the last two cells counted from the right, so a stray `|` in requirement text could never shift them.
- Print no file contents beyond IDs and paths; the specs are not secret, but keep the output short enough to read in a CI log.
- Paths in error messages are relative to the repo root.

## Out of scope

- Making `ac-coverage` a required check and the `CLAUDE.md` line: T-005.
- Checking that a test fails when the behaviour is removed (S10 10.4 item 1): done by hand in each pull request.

## Done when

- The tests above pass and every required check is green (S10 10.1).
- Definition of done in S10 10.4.
- `python3 tools/check_ac_coverage.py` run on `main` with this pull request exits 0 and prints the S2 coverage table.
