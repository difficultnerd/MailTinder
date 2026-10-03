#!/usr/bin/env bash
# Copy an optional layer into the repo root. Usage: optional/install.sh <zap|privacy|gpg-signing>
set -euo pipefail
layer="${1:-}"
here="$(cd "$(dirname "$0")" && pwd)"
root="$(dirname "$here")"
case "$layer" in
  zap|privacy|gpg-signing) ;;
  *) echo "Usage: $0 <zap|privacy|gpg-signing>"; exit 1 ;;
esac
cp -rn "$here/$layer/." "$root/"
echo "Installed '$layer'. Read optional/$layer/README.md for the remaining steps."
