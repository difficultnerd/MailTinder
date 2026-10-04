# T-005: Update CLAUDE.md and required checks list

| Milestone | Tier | Size | Depends on |
| --- | --- | --- | --- |
| M0 | sonnet | about 60 lines of Markdown, shell and YAML | T-002, T-003, T-004 |

**Read only these spec sections:** S10 2 "Test naming", S10 10.1, S10 10.4 (`docs/specs/S10-test-strategy.md`); S13 4, 5 and 9 (`docs/specs/S13-agent-working-rules.md`). In the repo: `CLAUDE.md`, `README.md`, `tools/apply_branch_protection.sh`, `.pre-commit-config.yaml`, `.github/workflows/ci.yml`, `.github/workflows/privacy.yml`. Nothing else is needed.

## Goal

The repo's agent instructions and branch protection match the new gates. `CLAUDE.md`'s "Before finishing" line runs what CI runs, including the AC coverage check; it states the test naming rule and the definition of done. `ac-coverage` and `coverage` join the branch protection list (S12 backlog seed); James applies it. `e2e` joins later, when T-1101 creates the job.

## Files

| Action | Path | What |
| --- | --- | --- |
| Change | `CLAUDE.md` | New "Before finishing" commands, test naming, definition of done, pointers to S13 and the backlog |
| Change | `tools/apply_branch_protection.sh` | Add `ac-coverage` and `coverage` to `contexts` |
| Change | `README.md` | Core toolchain rows for AC coverage and coverage; required check count and list |
| Change | `.pre-commit-config.yaml` | Pre-push hook running `tools/check_ac_coverage.py` |

## Types and signatures

`CLAUDE.md` after this task (complete file; keep the first, second, fourth and fifth bullets of the current file word for word):

```markdown
# Conventions for agents

- Backend is Rust in `backend/`; front end is Flutter/Dart in `app/`. Do not write Python in either.
- Python is permitted only in `tools/` and `scripts/` for agent tooling and glue; use the standard library where possible.
- Before finishing, run what CI runs and fix everything it reports:
  - `cd backend && cargo fmt --all && cargo clippy --all-targets --all-features --locked -- -D warnings && cargo test --all-features --locked`
  - `cd app && dart format . && flutter analyze --fatal-infos && flutter test`
  - `python3 tools/check_ac_coverage.py`
  - Optional locally, always in CI: `cargo llvm-cov --workspace --all-features --locked --lcov --output-path lcov.info` and `flutter test --coverage`, then `python3 tools/check_coverage_floors.py --rust backend/lcov.info --flutter app/coverage/lcov.info`.
- Never commit secrets. Gitleaks runs in pre-commit and CI.
- Do not add commit signing, container scanning or infrastructure scanning to the core; those are optional layers or out of scope.
- Work one task at a time from `docs/backlog/` and follow `docs/specs/S13-agent-working-rules.md`. Read only the spec sections the task file names.
- Test names carry the ID they prove (S10 2): Rust `<story>_<ac>_<behaviour>` such as `sw_03_ac2_reject_queues_unsubscribe_with_delay`; invariants `inv_5_...`; ASVS rows `asvs_v6_3_3_...`; S5 IDs `ses_1_...`; classifier IDs `guard_1_...`, `bake_1_...`, `stat_1_...`; S9 states without an AC `s9_<screen>_<state>`. Dart descriptions start with the ID: `'SW-03 AC2 reject toast states the delay'`, `'ASVS V14.3.1 ...'`.
- When a task merges, its ID goes in `tools/ac_coverage_enforced.txt` in the same pull request.
- Done means S10 10.4: every listed AC has a test that fails when the behaviour is removed (show it once in the pull request), every touched S9 state has a widget test, every touched ASVS row has its verification, all required checks pass with no retries, no real data in fixtures, and the pull request lists the AC IDs.
- No `println!`, `eprintln!`, `print!`, `eprint!` or `dbg!` (Clippy denies them); log through `tracing` with the S5 allowed fields only. Never log a message body, subject, snippet, address, URL, token or message ID.
- Time only from the `Clock` port and randomness only from the `Rng` port. No `unwrap` or `expect`, tests included; tests return `Result`.
```

`tools/apply_branch_protection.sh` contexts:

```bash
contexts='["rust","dart","language-policy","gitleaks","semgrep","cargo-audit","cargo-deny","dart-licenses","privacy-checks","ac-coverage","coverage"]'
```

`.pre-commit-config.yaml`, new local hook after `dart-analyze`:

```yaml
      - id: ac-coverage
        name: acceptance criteria coverage
        entry: python3 tools/check_ac_coverage.py
        language: system
        pass_filenames: false
        always_run: true
        stages: [pre-push]
```

## Algorithm

1. Replace `CLAUDE.md` with the text above.
2. Update `contexts` in `tools/apply_branch_protection.sh` and its comment ("Names must match the job ids in `.github/workflows/{ci,security,privacy}.yml`. Add `e2e` when T-1101 adds that job.").
3. `README.md`: in the core toolchain table add "Acceptance criteria coverage | `tools/check_ac_coverage.py` | CI `ac-coverage`; pre-push" and "Line coverage floors | `cargo llvm-cov`, `flutter test --coverage`, `tools/check_coverage_floors.py` | CI `coverage`". Change the branch protection note from nine checks to eleven and list them in the same order as `contexts`.
4. Add the pre-push hook.
5. Confirm the job IDs in `ci.yml` are exactly `ac-coverage` and `coverage` (T-002, T-004) and in `privacy.yml` exactly `privacy-checks` (T-003). A mismatch makes branch protection wait forever for a check that never reports.
6. **Step for James (not the builder):** after this pull request merges and both new jobs have run once on `main`, run `tools/apply_branch_protection.sh` with admin rights. The pull request description says this. Until James runs it, the new checks run but do not block merges.
7. **Later, for T-1101:** that task adds `e2e` to `contexts` and to `CLAUDE.md`, and James runs the script again. Note this in the pull request description so it is not lost.

## Acceptance criteria

| ID | Behaviour (one line) |
| --- | --- |
| None | Documentation and configuration only |

## Tests that must pass

- Every existing CI job stays green.
- `bash -n tools/apply_branch_protection.sh` (syntax check) passes.
- `python3 -c "import json,re,sys; s=open('tools/apply_branch_protection.sh').read(); json.loads(re.search(r\"contexts='(.*)'\", s).group(1))"` passes (the contexts string is valid JSON).

## Edge cases and traps

- The CI commands in `CLAUDE.md` must match the workflows exactly (`--all-features --locked`, `--fatal-infos`). S13 5 still shows the shorter commands; do not edit S13 (build threads never edit specs). Report the difference in the pull request so the planning thread can align S13.
- Do not run `tools/apply_branch_protection.sh` yourself; it needs admin rights and changes repository settings. James does it.
- Do not add `e2e` to `contexts` yet: a required check that never runs blocks every pull request.
- Keep `CLAUDE.md` short and imperative; it is read at the start of every agent session.
- Australian English, no em or en dashes in `CLAUDE.md` and `README.md` text.

## Out of scope

- The checks themselves: T-002, T-003, T-004. The `e2e` job: T-1101.

## Done when

- The tests above pass and every required check is green (S10 10.1).
- Definition of done in S10 10.4.
- The pull request description tells James to run `tools/apply_branch_protection.sh`, and notes that `e2e` is added by T-1101.
