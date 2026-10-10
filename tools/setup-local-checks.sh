#!/usr/bin/env bash
# setup-local-checks.sh — install the tools that tools/ci-local.sh needs.
#
# Installs into tools/.bin (cargo-deny, gitleaks, cargo-mutants) and
# tools/.venv-semgrep.
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

# ---- cargo-mutants (T-1109 mutation-testing pilot) ----
# Pinned release with a pinned SHA-256; verify the checksum before installing.
CM_VER="v27.1.0"
CM_SHA256="dfe6dc37d0342c891d2829b5a695aa57c2d0edecef7e7d0399a30cc6e206411e"
if [ ! -x "$BIN/cargo-mutants" ]; then
  if [ "$ARCH" = "x86_64" ]; then
    echo "Installing cargo-mutants $CM_VER ($ARCH)..."
    CM_TMP="$(mktemp -d)"
    curl -sSfL "https://github.com/sourcefrog/cargo-mutants/releases/download/$CM_VER/cargo-mutants-x86_64-unknown-linux-gnu.tar.gz" -o "$CM_TMP/cm.tgz"
    echo "$CM_SHA256  $CM_TMP/cm.tgz" | sha256sum -c -
    tar -xzf "$CM_TMP/cm.tgz" -C "$BIN" cargo-mutants
    chmod +x "$BIN/cargo-mutants"
    rm -rf "$CM_TMP"
    echo "  cargo-mutants $("$BIN/cargo-mutants" --version)"
  else
    echo "No prebuilt cargo-mutants for $ARCH; building pinned $CM_VER with cargo..."
    # Install under tools/.bin (git-ignored), never under a tracked path: an earlier fallback root of tools/.cargo-mutants was swept into a
    # commit by `git add -A` and put a 9.5 MB executable on main (AAR 3.79).
    cargo install cargo-mutants --version "${CM_VER#v}" --locked --root "$BIN/cargo-mutants-root"
    ln -sf "$BIN/cargo-mutants-root/bin/cargo-mutants" "$BIN/cargo-mutants"
    echo "  cargo-mutants $("$BIN/cargo-mutants" --version)"
  fi
else
  echo "cargo-mutants already installed."
fi

echo ""
echo "Done. Run the gate with: ./tools/ci-local.sh"
