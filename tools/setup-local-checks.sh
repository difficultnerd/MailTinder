#!/usr/bin/env bash
# setup-local-checks.sh — install the tools that tools/ci-local.sh needs.
#
# Installs into tools/.bin (cargo-deny, gitleaks) and tools/.venv-semgrep.
# Rust (cargo/rustfmt/clippy) and Flutter/Dart must already be on PATH.
#
# Usage: ./tools/setup-local-checks.sh

set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BIN="$ROOT/tools/.bin"
mkdir -p "$BIN"

# Detect architecture for prebuilt binaries.
ARCH="$(uname -m)"
case "$ARCH" in
  x86_64)  DENY_ARCH="x86_64-unknown-linux-gnu"; GL_ARCH="linux_x64" ;;
  aarch64) DENY_ARCH="aarch64-unknown-linux-musl"; GL_ARCH="linux_arm64" ;;
  *) echo "unsupported arch: $ARCH"; exit 1 ;;
esac

# ---- cargo-deny ----
if [ ! -x "$BIN/cargo-deny" ]; then
  echo "Installing cargo-deny ($DENY_ARCH)..."
  VER="$(curl -sSfL https://api.github.com/repos/embarkstudios/cargo-deny/releases/latest \
    | grep -oP '"tag_name":\s*"\K[^"]+' | head -1)"
  curl -sSfL "https://github.com/embarkstudios/cargo-deny/releases/download/$VER/cargo-deny-$VER-$DENY_ARCH.tar.gz" -o "$BIN/cd.tgz"
  tar -xzf "$BIN/cd.tgz" -C "$BIN"
  find "$BIN" -name cargo-deny -type f -exec mv {} "$BIN/cargo-deny" \;
  chmod +x "$BIN/cargo-deny"
  rm -rf "$BIN"/cargo-deny-* "$BIN/cd.tgz"
  echo "  cargo-deny $("$BIN/cargo-deny" --version)"
else
  echo "cargo-deny already installed."
fi

# ---- gitleaks ----
if [ ! -x "$BIN/gitleaks" ]; then
  echo "Installing gitleaks ($GL_ARCH)..."
  VER="$(curl -sSfL https://api.github.com/repos/gitleaks/gitleaks/releases/latest \
    | grep -oP '"tag_name":\s*"\K[^"]+' | head -1)"
  curl -sSfL "https://github.com/gitleaks/gitleaks/releases/download/$VER/gitleaks_${VER#v}_${GL_ARCH}.tar.gz" -o "$BIN/gl.tgz"
  tar -xzf "$BIN/gl.tgz" -C "$BIN" gitleaks
  chmod +x "$BIN/gitleaks"
  rm -f "$BIN/gl.tgz"
  echo "  gitleaks $("$BIN/gitleaks" version)"
else
  echo "gitleaks already installed."
fi

# ---- semgrep (project venv) ----
if [ ! -x "$ROOT/tools/.venv-semgrep/bin/semgrep" ]; then
  echo "Installing semgrep into tools/.venv-semgrep..."
  python3 -m venv "$ROOT/tools/.venv-semgrep"
  "$ROOT/tools/.venv-semgrep/bin/pip" install -q semgrep
  echo "  semgrep $("$ROOT/tools/.venv-semgrep/bin/semgrep" --version)"
else
  echo "semgrep already installed."
fi

echo ""
echo "Done. Run the gate with: ./tools/ci-local.sh"
