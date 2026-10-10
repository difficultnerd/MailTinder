#!/usr/bin/env bash
# Regenerate the committed golden screenshots (T-1111) and print the images
# that changed. Run from anywhere; the script finds the repository root itself.
#
# Review the diff before committing: the goldens are only evidence when a human
# has looked at the changed images. See docs/golden-tests.md.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root/app"

flutter test --update-goldens --tags golden

echo
echo "Changed golden images:"
git -C "$root" status --porcelain -- app/test/golden/goldens
