# Conventions for agents

- Specs live in `docs/`. Read `docs/CONTEXT.md` first, then the spec a task names under `docs/specs/`; `docs/security/asvs-l2-register.md` maps OWASP ASVS 5.0 Level 2. Research behind the decisions is in `research/`.
- Backend is Rust in `backend/`; front end is Flutter/Dart in `app/`. Do not write Python in either.
- Python is permitted only in `tools/` and `scripts/` for agent tooling and glue; use the standard library where possible.
- Before finishing: `cd backend && cargo fmt --all && cargo clippy --all-targets -- -D warnings && cargo test`; `cd app && dart format . && flutter analyze && flutter test`.
- Never commit secrets. Gitleaks runs in pre-commit and CI.
- Do not add commit signing, container scanning or infrastructure scanning to the core; those are optional layers or out of scope.
