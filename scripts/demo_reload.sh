#!/usr/bin/env bash
# scripts/demo_reload.sh - trigger a hot reload of `demo.sh --dev` (T-1113).
#
# `flutter run -d web-server` reads single-character commands from its stdin:
# `r` hot reloads and `R` hot restarts. demo.sh starts it with a FIFO as stdin
# (target/demo/run/flutter-dev.stdin), kept open by a long-lived writer, so any
# process - including a coding agent - can drive a reload without owning the
# terminal. This helper writes the command to that FIFO.
set -euo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
FIFO="$REPO/target/demo/run/flutter-dev.stdin"
COMMAND="r"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --restart) COMMAND="R"; shift ;;
    -h|--help)
      cat <<'EOF'
demo_reload.sh [--restart]   hot reload (r, default) or hot restart (R) the
                             `demo.sh --dev` Flutter web server
EOF
      exit 0 ;;
    *) echo "demo_reload.sh: unknown argument: $1" >&2; exit 2 ;;
  esac
done

if [[ ! -p "$FIFO" ]]; then
  echo "demo_reload.sh: no dev server stdin at $FIFO - start it with scripts/demo.sh --dev" >&2
  exit 1
fi

# The dev server holds the read end and demo.sh keeps a writer on the FIFO, so
# this open never blocks and never makes the dev server read EOF.
printf '%s\n' "$COMMAND" > "$FIFO"

case "$COMMAND" in
  r) echo "demo_reload.sh: sent 'r' (hot reload)" ;;
  R) echo "demo_reload.sh: sent 'R' (hot restart)" ;;
esac
