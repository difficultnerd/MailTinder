# project-template

GitHub template for personal projects: Rust backend (`backend/`), Flutter front end for iOS, Android and web (`app/`). Python is allowed only for agent tooling and glue scripts (`tools/`, `scripts/`). Every tool is free and open source.

## Use it

1. Click **Use this template** on GitHub.
2. Generate Flutter platform folders: `cd app && flutter create . --platforms=ios,android,web --project-name app`
3. Install local hooks: `pip install pre-commit && pre-commit install` (also needs `cargo`, `dart`/`flutter` on PATH).
4. Apply branch protection once CI has run at least once on `main`: `tools/apply_branch_protection.sh`
5. Add a `LICENSE` file for the new project (none is shipped).
6. Edit `backend/deny.toml` and `tools/dart_license_policy.json` if your licence policy differs.

## Local checks (pre-push gate)

CI is the source of truth, but a local gate catches problems before you push so
CI runs are "belt and braces". `tools/ci-local.sh` mirrors the CI checks:

```sh
./tools/setup-local-checks.sh   # one-time: installs cargo-deny, gitleaks, semgrep venv
./tools/ci-local.sh             # run all quick local checks
./tools/ci-local.sh --full      # also run heavy checks (coverage, csp-smoke)
./tools/ci-local.sh fmt clippy  # run only named checks
```

It runs `rustfmt`, `clippy -D warnings`, `cargo test`, `cargo-deny`,
`semgrep` (privacy + mailtinder rules), `gitleaks`, `dart format` /
`flutter analyze` / `flutter test`, and `ac-coverage`. Any check whose tool
isn't installed is skipped with a note; the script exits non-zero if a
requested check fails. Tools install into `tools/.bin` and
`tools/.venv-semgrep` (both gitignored). Rust (`cargo`/`rustfmt`/`clippy`) and
Flutter/Dart must already be on `PATH`.

## Core toolchain

| Concern | Tool | Where it runs |
|---|---|---|
| Secret scanning | Gitleaks | pre-commit; CI `gitleaks` (full history, every push and PR) |
| Dependency updates | Dependabot (cargo, pub, pip, github-actions) | `.github/dependabot.yml` |
| Rust advisories | cargo-audit | CI `cargo-audit`, plus weekly schedule |
| Static analysis | Clippy, Dart analyzer, Semgrep | Clippy and analyzer: pre-push and CI `rust`, `dart`; Semgrep: CI `semgrep` |
| Format | rustfmt, `dart format` | pre-commit; CI `rust`, `dart` |
| Licences | cargo-deny (Rust), `tools/dart_license_check.py` (Dart) | CI `cargo-deny`, `dart-licenses` |
| Privacy | `.semgrep/privacy.yml`, `.semgrep/mailtinder.yml`, `backend/clippy.toml` | CI `privacy-checks` |
| Acceptance criteria coverage | `tools/check_ac_coverage.py` | CI `ac-coverage`; pre-push |
| Line coverage floors | `cargo llvm-cov`, `flutter test --coverage`, `tools/check_coverage_floors.py` | CI `coverage` |
| Hook orchestration | pre-commit | `.pre-commit-config.yaml` |
| Branch protection | `tools/apply_branch_protection.sh` | one-off, via `gh` |

Pre-commit runs secret scanning, formatting and housekeeping on commit; Clippy and the Dart analyzer run on push because they are slower. Semgrep and the audits run in CI only.

Notes:

- Dart has no maintained cargo-deny equivalent, so the licence check is a small stdlib-only Python script that classifies each resolved package's LICENSE file against an allowlist. Unknown or missing licences fail the job; add a reviewed `overrides` entry to accept one.
- Dependabot also raises security alerts if enabled under Settings > Code security. Turn on Dependabot alerts and security updates there; the config file only covers version updates.
- Branch protection requires all eleven checks (`rust`, `dart`, `language-policy`, `gitleaks`, `semgrep`, `cargo-audit`, `cargo-deny`, `dart-licenses`, `privacy-checks`, `ac-coverage`, `coverage`), up-to-date branches, resolved conversations, no force pushes and no deletion, and applies to admins. It requires no review approvals, which suits a solo repo. On GitHub Free, branch protection works for public repos only.
- No commit signing is configured in the core.
- GitHub Actions are pinned to full commit SHAs (with the version in a trailing comment), and Dependabot proposes updates. Dependabot waits 7 days (`cooldown`) before proposing a newly published version.
- Container and infrastructure scanning is out of scope for now.
- Semgrep rule packs `p/default`, `p/security-audit` and `p/rust` are fetched from the free public registry at run time. Semgrep has no Dart-specific pack, so Dart code is covered only by generic rules. If a pack name is retired, the job fails loudly; adjust it in `.github/workflows/security.yml`.

## Optional layers

Not active by default. Copy one into a repo with `optional/install.sh <layer>` (existing files are never overwritten), then follow its README.

| Layer | Use when | Contents |
|---|---|---|
| `optional/zap` | Live web login flow or API | Scheduled ZAP baseline and API scans against a staging URL |
| `optional/gpg-signing` | Verified authorship required | PR signature check, script to require signed commits on `main`, setup guide |

## Layout

```
backend/   Rust (Cargo.toml, deny.toml, rustfmt.toml)
app/       Flutter (pubspec.yaml, analysis_options.yaml)
tools/     Python/bash glue: licence check, language policy, branch protection
optional/  Opt-in layers
.github/   Workflows and Dependabot config
```
