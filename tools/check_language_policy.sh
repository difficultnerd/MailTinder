#!/usr/bin/env bash
# Python is allowed only for agent tooling and glue scripts (tools/, scripts/, optional/**/tools/).
# Fail if Python files appear in the application code under backend/ or app/.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"

bad=$(find backend app -type f \( -name '*.py' -o -name 'requirements*.txt' -o -name 'pyproject.toml' \) \
  -not -path '*/target/*' -not -path '*/.dart_tool/*' -not -path '*/build/*' 2>/dev/null || true)

if [ -n "$bad" ]; then
  echo "Python is not permitted in core application code:"
  echo "$bad"
  exit 1
fi
echo "Language policy OK."
