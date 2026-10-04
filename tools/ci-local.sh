#!/usr/bin/env bash
# ci-local.sh — local pre-push quality gate that mirrors the GitHub Actions
# checks, so CI is "belt and braces" and we catch problems before pushing.
#
# Usage:
#   ./tools/ci-local.sh            # run all quick local checks
#   ./tools/ci-local.sh --full     # also run heavy checks (coverage, csp-smoke)
#   ./tools/ci-local.sh fmt clippy # run only the named checks
#
# Skips (with a note) any check whose tool isn't installed, so it degrades
# gracefully. Exit code is non-zero if any requested check fails.
#
# Tool locations:
#   cargo, rustfmt, clippy      — must be on PATH (rustup toolchain)
#   flutter, dart               — must be on PATH
#   cargo-deny                  — tools/.bin/cargo-deny        (installed via setup)
#   gitleaks                    — tools/.bin/gitleaks          (installed via setup)
#   semgrep                     — tools/.venv-semgrep/bin/semgrep (installed via setup)
#   cargo-llvm-cov (--full)     — cargo install cargo-llvm-cov
# Set up missing tools with:    ./tools/setup-local-checks.sh

set -u

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

export PATH="$ROOT/tools/.bin:$ROOT/tools/.venv-semgrep/bin:$PATH"
BIN="$ROOT/tools/.bin"

FAILED=0
RAN=0
run() { # run <name> <cmd...>
  local name="$1"; shift
  echo ""
  echo "===== $name ====="
  "$@" || { echo "!! $name FAILED"; FAILED=1; }
  RAN=$((RAN+1))
}

have() { command -v "$1" >/dev/null 2>&1; }

REQUESTED=("$@")
want() {
  # want <name> [--full-only]
  local name="$1"; local full_only="${2:-}"
  [ ${#REQUESTED[@]} -eq 0 ] || [[ " ${REQUESTED[*]} " == *" $name "* ]] || return 1
  [ -z "$full_only" ] || [[ " ${REQUESTED[*]} " == *"--full"* ]] || return 1
  return 0
}

# ---- Rust: fmt, clippy, test ----
if want fmt; then
  if (cd backend && cargo fmt --all -- --check) >/dev/null 2>&1; then
    echo "===== rustfmt ====="; echo "ok (no diffs)"
  elif have cargo; then
    (cd backend && cargo fmt --all -- --check) && { echo ok; } || { echo "!! rustfmt FAILED"; FAILED=1; }
  else
    echo "===== rustfmt ====="; echo "skip: cargo not found"
  fi
fi

want clippy && { have cargo && run "clippy (-D warnings)" bash -c "cd backend && cargo clippy --all-targets --all-features --locked -- -D warnings"; }
want test  && { have cargo && run "rust tests" bash -c "cd backend && cargo test --all-features"; }

# ---- cargo-deny ----
want deny && {
  if [ -x "$BIN/cargo-deny" ]; then
    run "cargo-deny (bans/licenses/advisories)" "$BIN/cargo-deny" --manifest-path backend/Cargo.toml check
  else
    echo "===== cargo-deny ====="; echo "skip: tools/.bin/cargo-deny not found (run setup-local-checks.sh)"
  fi
}

# ---- semgrep (privacy + mailtinder rules) ----
want semgrep && {
  if [ -x "$BIN/../.venv-semgrep/bin/semgrep" ]; then
    run "semgrep" "$ROOT/tools/.venv-semgrep/bin/semgrep" scan --metrics=off --error --config .semgrep/privacy.yml --config .semgrep/mailtinder.yml backend app
  else
    echo "===== semgrep ====="; echo "skip: semgrep venv not found"
  fi
}

# ---- gitleaks ----
want gitleaks && {
  if [ -x "$BIN/gitleaks" ]; then
    run "gitleaks" "$BIN/gitleaks" git --config .gitleaks.toml --redact --verbose .
  else
    echo "===== gitleaks ====="; echo "skip: tools/.bin/gitleaks not found"
  fi
}

# ---- Flutter / Dart ----
want dart && {
  if have flutter && have dart; then
    run "dart format" bash -c "cd app && dart format --output=none --set-exit-if-changed ."
    run "flutter analyze" bash -c "cd app && flutter analyze --fatal-infos"
    run "flutter test" bash -c "cd app && flutter test"
  else
    echo "===== flutter/dart ====="; echo "skip: flutter/dart not on PATH"
  fi
}

# ---- Python tool checks (ac-coverage) ----
want ac-coverage && {
  if have python3; then
    run "ac-coverage unit tests" python3 -m unittest discover -s tools -p 'test_*.py'
    run "ac-coverage check" python3 tools/check_ac_coverage.py
  fi
}

# ---- Heavy / opt-in (--full) ----
want coverage --full && {
  if have cargo-llvm-cov && have python3; then
    run "rust coverage (llvm-cov)" bash -c "cd backend && cargo llvm-cov --workspace --all-features --lcov --output-path lcov.info"
    run "flutter coverage" bash -c "cd app && flutter pub get && flutter test --coverage"
    run "coverage floors" python3 tools/check_coverage_floors.py --rust backend/lcov.info --flutter app/coverage/lcov.info
  else
    echo "===== coverage ====="; echo "skip: cargo-llvm-cov not installed (cargo install cargo-llvm-cov)"
  fi
}

# ---- Summary ----
echo ""
echo "=============================================="
if [ "$FAILED" -eq 0 ]; then
  echo "All requested local checks passed ($RAN checks)."
else
  echo "Some checks FAILED — see above. Fix before pushing."
fi
echo "=============================================="
exit "$FAILED"
