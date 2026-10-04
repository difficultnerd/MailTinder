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
