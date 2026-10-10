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
# demo.sh records "<pid> <proc start time>" here when it starts the dev server;
# a pid on its own is not proof the process is ours (T-1113 review F3).
TOKEN_FILE="$RUN/flutter.pid.start"

# proc_starttime: field 22 of /proc/<pid>/stat, the ownership token.
# shellcheck source=scripts/e2e/lib.sh
source "$REPO/scripts/e2e/lib.sh"

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
# A pid read from a file is not proof the process is ours: after `stop` (or a
# crash) the pid can be reused by an unrelated process, and SIGUSR1 kills a
# process with no handler. Refuse unless the recorded start time still matches,
# the same ownership check scripts/demo.sh uses before signalling a group
# (T-1113 review F3).
if [[ ! -s "$TOKEN_FILE" ]]; then
  echo "demo_reload: refusing: no start-time token at $TOKEN_FILE (was this run started by 'scripts/demo.sh --dev'?)" >&2
  exit 1
fi
read -r token_pid token_start < "$TOKEN_FILE" || true
now="$(proc_starttime "$pid" 2>/dev/null || true)"
if [[ "$token_pid" != "$pid" || -z "$now" || "$now" != "$token_start" ]]; then
  echo "demo_reload: refusing: pid $pid is not the dev server this run started" >&2
  exit 1
fi
kill -USR1 "$pid"
echo "demo_reload: asked the Flutter dev server (pid $pid) to hot reload"
