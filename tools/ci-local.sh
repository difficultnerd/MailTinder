#!/usr/bin/env bash
# ci-local.sh — local pre-push quality gate that mirrors the GitHub Actions
# checks, so CI is "belt and braces" and we catch problems before pushing.
#
# Usage:
#   ./tools/ci-local.sh            # run all quick local checks
#   ./tools/ci-local.sh --full     # also run heavy checks (coverage, csp-smoke)
#   ./tools/ci-local.sh fmt clippy # run only the named checks
#
# A check whose tool is missing is NOT skipped silently: it is reported as a
# FAILURE ("SKIPPED ... a gate that cannot run is NOT a pass"). A suite that
# quietly does not run is worse than no suite, because it reads as a clean pass.
# CI enforces the same rule with the check-integrity jobs in each workflow.
# Exit code is non-zero if any requested check fails or cannot run.
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

# A gate that cannot run must never read as a pass. CI enforces the same rule
# with the check-integrity jobs; this is the local half of it. Without this,
# a missing tool made ci-local.sh print "skip" and still exit 0 - a clean PASS
# for a suite that never ran.
skip_fail() { # skip_fail <name> <reason>
  echo "===== $1 ====="
  echo "!! $1 SKIPPED: $2"
  echo "!! a gate that cannot run is NOT a pass - install the tool or fix the environment"
  FAILED=1
}

REQUESTED=("$@")
# `--full` (or any bare flag) on its own means "run every check", so only a
# request that names a check narrows the run.
SELECTIVE=0
for arg in "$@"; do
  case "$arg" in --*) ;; *) SELECTIVE=1 ;; esac
done
want() {
  # want <name> [--full-only]
  local name="$1"; local full_only="${2:-}"
  if [ "$SELECTIVE" -eq 1 ]; then
    [[ " ${REQUESTED[*]} " == *" $name "* ]] || return 1
  fi
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
    skip_fail rustfmt "cargo not found on PATH"
  fi
fi

want clippy && { if have cargo; then run "clippy (-D warnings)" bash -c "cd backend && cargo clippy --all-targets --all-features --locked -- -D warnings"; else skip_fail clippy "cargo not found on PATH"; fi; }
want test  && { if have cargo; then run "rust tests" bash -c "cd backend && cargo test --all-features"; else skip_fail test "cargo not found on PATH"; fi; }

# ---- cargo-deny ----
want deny && {
  if [ -x "$BIN/cargo-deny" ]; then
    run "cargo-deny (bans/licenses/advisories)" "$BIN/cargo-deny" --manifest-path backend/Cargo.toml check
  else
    skip_fail cargo-deny "tools/.bin/cargo-deny not found (run tools/setup-local-checks.sh)"
  fi
}

# ---- semgrep (privacy + mailtinder rules) ----
want semgrep && {
  if [ -x "$BIN/../.venv-semgrep/bin/semgrep" ]; then
    run "semgrep" "$ROOT/tools/.venv-semgrep/bin/semgrep" scan --metrics=off --error --config .semgrep/privacy.yml --config .semgrep/mailtinder.yml backend app
  else
    skip_fail semgrep "tools/.venv-semgrep not found (run tools/setup-local-checks.sh)"
  fi
}

# ---- gitleaks ----
want gitleaks && {
  if [ -x "$BIN/gitleaks" ]; then
    run "gitleaks" "$BIN/gitleaks" git --config .gitleaks.toml --redact --verbose .
  else
    skip_fail gitleaks "tools/.bin/gitleaks not found (run tools/setup-local-checks.sh)"
  fi
}

# ---- Flutter / Dart ----
want dart && {
  if have flutter && have dart; then
    run "dart format" bash -c "cd app && dart format --output=none --set-exit-if-changed ."
    run "flutter analyze" bash -c "cd app && flutter analyze --fatal-infos"
    run "flutter test" bash -c "cd app && flutter test"
  else
    skip_fail dart "flutter/dart not on PATH"
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
want csp-smoke --full && {
  if have flutter && { have google-chrome || have chromium; }; then
    run "csp-smoke (web build under the shipped CSP)" scripts/csp_check.sh
  else
    skip_fail csp-smoke "needs flutter plus google-chrome or chromium on PATH"
  fi
}

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
