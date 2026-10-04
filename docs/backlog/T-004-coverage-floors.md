# T-004: Coverage job with floors

| Milestone | Tier | Size | Depends on |
| --- | --- | --- | --- |
| M0 | sonnet | about 200 lines of Python and YAML plus 100 lines of unittest | T-001 |

**Read only these spec sections:** S10 10.1 row `coverage`, S10 10.2 (`docs/specs/S10-test-strategy.md`). In the repo: `.github/workflows/ci.yml`, `.gitignore`. Nothing else is needed.

## Goal

A CI job `coverage` measures line coverage for the Rust workspace (`cargo llvm-cov`) and the Flutter app (`flutter test --coverage`) and fails when any area is below its floor: `domain` 90%, every other Rust crate 75% (excluding `testkit` and the test-only binaries), Flutter `lib/` 75% (decision T3). Floors live in one JSON file and can only go up. T-005 makes the job required.

## Files

| Action | Path | What |
| --- | --- | --- |
| Create | `tools/check_coverage_floors.py` | Reads two LCOV files and the floors file, prints a table, exits non-zero below a floor |
| Create | `tools/coverage_floors.json` | Floors and exclusions |
| Create | `tools/test_check_coverage_floors.py` | `unittest` tests with inline LCOV fixtures |
| Change | `.github/workflows/ci.yml` | New job `coverage` |
| Change | `.gitignore` | Add `backend/lcov.info`, `app/coverage/` |

## Types and signatures

`tools/coverage_floors.json`:

```json
{
  "rust": {
    "domain": 90,
    "default": 75,
    "exclude_crates": ["testkit", "fake-google", "unsub-testbed"],
    "exclude_files": ["src/main.rs"]
  },
  "flutter": {
    "lib": 75,
    "exclude_files": ["lib/main.dart"]
  }
}
```

```python
# tools/check_coverage_floors.py (stdlib only)
MINIMUMS = {"domain": 90, "rust_default": 75, "flutter_lib": 75}   # S10 10.2; floors may never go below these

@dataclass
class Totals:
    found: int = 0      # LF
    hit: int = 0        # LH
    def percent(self) -> float | None: ...   # None when found == 0

def parse_lcov(text: str) -> dict[str, Totals]: ...           # per source file (SF:), from LF and LH lines
def rust_crate_of(path: str) -> str | None: ...                 # ".../backend/crates/<name>/src/..." -> "<name>"
def rust_area_totals(files: dict[str, Totals], cfg: dict) -> dict[str, Totals]: ...
def flutter_totals(files: dict[str, Totals], cfg: dict) -> Totals: ...
def check_minimums(cfg: dict) -> list[str]: ...                 # errors if a floor is below MINIMUMS
def main(argv: list[str] | None = None) -> int: ...             # --rust <lcov> --flutter <lcov>; 0 ok, 1 below floor, 2 input error
```

## Algorithm

1. `parse_lcov`: read records separated by `end_of_record`. `SF:` gives the path; `LF:` and `LH:` give found and hit lines. If a record has `DA:` lines but no `LF`/`LH`, count `DA` lines (found) and `DA` lines with a non-zero count (hit).
2. `rust_crate_of`: find the segment `crates/<name>/src/` in the path (absolute paths from `cargo llvm-cov` are fine); return `<name>`, else `None` (files outside the workspace crates, such as the standard library, are ignored).
3. Drop files whose path ends with any `exclude_files` entry, and crates listed in `exclude_crates`. Sum the rest per crate.
4. For each crate: floor is `rust[<name>]` if present, else `rust.default`. A crate with `found == 0` (no instrumented lines yet) passes and prints `no lines`. `[DEFAULT]` an empty skeleton crate has nothing to cover, and failing it would block every early task.
5. Flutter: keep files whose path contains `lib/`, drop `exclude_files`, sum, compare with `flutter.lib`. Zero lines passes.
6. `check_minimums`: every floor in the JSON must be at least the matching `MINIMUMS` value. A lower value is an error (S10 10.2: floors only ratchet up). Raising a floor is allowed.
7. Print a table: area, lines found, lines hit, percent with one decimal, floor, `ok` or `BELOW`. Append the same table to `$GITHUB_STEP_SUMMARY` when set.
8. Exit 1 if any area is below its floor or a minimum check fails; 2 if an input file is missing or unreadable.
9. CI job in `.github/workflows/ci.yml`:

```yaml
  coverage:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1 # v7
      - uses: dtolnay/rust-toolchain@89b12181fb390509a0842a86cc55eeb8eb928c1d # stable
        with:
          toolchain: stable
          components: llvm-tools-preview
      - uses: Swatinem/rust-cache@6323deb102c322ba6fcbdcafc7e3dddab59af2b6 # v2
        with:
          workspaces: backend
      - uses: taiki-e/install-action@83ac0ad63c0167e6f06796fab0fce28db1bf3db0 # v2
        with:
          tool: cargo-llvm-cov
      - uses: subosito/flutter-action@1a449444c387b1966244ae4d4f8c696479add0b2 # v2
        with:
          channel: stable
          cache: true
      - run: cargo llvm-cov --workspace --all-features --locked --lcov --output-path lcov.info
        working-directory: backend
      - run: flutter pub get && flutter test --coverage
        working-directory: app
      - run: python3 -m unittest discover -s tools -p 'test_check_coverage_floors.py'
      - run: python3 tools/check_coverage_floors.py --rust backend/lcov.info --flutter app/coverage/lcov.info
```

Use exactly these pinned action SHAs; they are the ones the template already trusts.

## Acceptance criteria

| ID | Behaviour (one line) |
| --- | --- |
| None | CI tooling; proven by the `unittest` cases below |

## Tests that must pass

All in `tools/test_check_coverage_floors.py` (`unittest`, unit level):

- `test_parse_lcov_lf_lh`
- `test_parse_lcov_counts_da_when_lf_missing`
- `test_rust_crate_of_absolute_path`
- `test_domain_below_90_fails`
- `test_other_crate_at_75_passes`
- `test_testkit_and_fakes_excluded`
- `test_main_rs_excluded`
- `test_crate_with_no_lines_passes`
- `test_flutter_main_dart_excluded`
- `test_floor_below_minimum_is_error`
- `test_missing_lcov_exit_2`
- CI job `coverage` green on `main` with the T-001 skeleton.

## Edge cases and traps

- `cargo llvm-cov` needs the `llvm-tools-preview` component; without it the job fails with a confusing "llvm-profdata not found".
- Coverage counts lines in `#[cfg(test)]` modules too. That is fine; do not try to strip them.
- Binaries keep `main.rs` thin (parse config, call a function in `lib.rs`), which is why `src/main.rs` is excluded. Say so in a comment in the JSON's neighbour script, not in the JSON (JSON has no comments).
- `fake-google` and `unsub-testbed` are test infrastructure like `testkit` `[DEFAULT]` (S10 10.2 excludes `testkit`; these binaries exist only for tests).
- Never lower a floor or add a crate to `exclude_crates` to get green (S13 4.6). If a later task needs an exclusion, it asks the planning thread.
- `flutter test --coverage` writes `app/coverage/lcov.info` with paths relative to `app/` (`lib/...`); the Rust file has absolute paths. Handle both.
- Python stays in `tools/`; standard library only.
- Do not duplicate `cargo test` in the `rust` job; the `coverage` job runs the tests again under instrumentation, which is expected.

## Out of scope

- Making `coverage` a required check: T-005.
- `cargo-mutants` nightly report (S10 10.2): not in M0.

## Done when

- The tests above pass and every required check is green (S10 10.1).
- Definition of done in S10 10.4.
- The job's step summary shows one row per Rust crate and one Flutter row.
