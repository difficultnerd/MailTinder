#!/usr/bin/env bash
# scripts/demo_reload.sh - hot-reload the running dev demo (T-1113).
#
# `scripts/demo.sh --dev` runs the app through `flutter run -d web-server`;
# `flutter run --pid-file <f>` writes the tool's pid to <f> and hot-reloads on
# SIGUSR1 (hot-restarts on SIGUSR2) - the documented Flutter mechanism. So an
# agent can edit a Dart file and refresh the running view without restarting
# anything:
#
#     scripts/demo_reload.sh
#
# The helper only signals the dev server; the caller waits for the change to be
# served. It never touches the terminal, so it is safe from a coding agent.
set -euo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# Test seam (like the other DEMO_* seams): point at another run directory.
# Default is the directory demo.sh writes its state into.
RUN="${DEMO_RUN_DIR:-$REPO/target/demo/run}"
PID_FILE="$RUN/flutter.pid"

if [[ ! -s "$PID_FILE" ]]; then
  echo "demo_reload: no dev server pid file at $PID_FILE (is 'scripts/demo.sh --dev' running?)" >&2
  exit 1
fi
pid="$(cat "$PID_FILE")"
if [[ ! "$pid" =~ ^[0-9]+$ ]]; then
  echo "demo_reload: unusable pid in $PID_FILE: '$pid'" >&2
  exit 1
fi
if ! kill -0 "$pid" 2>/dev/null; then
  echo "demo_reload: dev server (pid $pid) is not running" >&2
  exit 1
fi
kill -USR1 "$pid"
echo "demo_reload: asked the Flutter dev server (pid $pid) to hot reload"
