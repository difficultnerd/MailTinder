#!/usr/bin/env bash
# scripts/demo.sh - the one-command demo stack (T-1108a).
#
# Brings up the same e2e stack scripts/e2e.sh uses (Firestore emulator,
# fake-google, unsub-testbed, `api` and the Flutter web build served by
# scripts/e2e_host.py), leaves it running on 127.0.0.1 and prints the URL.
# `stop` kills every process group it started (even after a crash) and prints a
# leak census; `status` says whether it is up; `--check` is the automatic test:
# start, fetch the page and the health route, stop, and report nothing left.
#
# Usage:
#   scripts/demo.sh [start] [--host <loopback addr>]   # start and leave running
#   scripts/demo.sh stop                               # stop and print the census
#   scripts/demo.sh status                             # is it running?
#   scripts/demo.sh --check                            # automatic start/fetch/stop
#
# Everything binds loopback only; a non-loopback host is refused.
set -euo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO"

# Shared start/stop helpers, also used by scripts/e2e.sh (T-1108a).
# shellcheck source=scripts/e2e/lib.sh
source "$REPO/scripts/e2e/lib.sh"

STATE_DIR="$REPO/target/demo"
STATE="$STATE_DIR/state.json"
LOGS="$REPO/target/demo-logs"
RUN="$STATE_DIR/run"
PGIDS=()
START_S=$SECONDS
DEMO_HOST="${DEMO_HOST:-127.0.0.1}"
COMMAND="start"

usage() { sed -n '2,9p' "$0" | sed 's/^# \{0,1\}//'; }

while [[ $# -gt 0 ]]; do
  case "$1" in
    start) COMMAND="start"; shift ;;
    stop) COMMAND="stop"; shift ;;
    status) COMMAND="status"; shift ;;
    --check) COMMAND="check"; shift ;;
    --host) DEMO_HOST="${2:?--host needs an address}"; shift 2 ;;
    -h|--help) usage; exit 0 ;;
    *) echo "demo.sh: unknown argument: $1" >&2; exit 2 ;;
  esac
done

# Loopback only (behaviour 1): 127.0.0.0/8 and the two localhost spellings.
is_loopback() {
  case "$1" in
    127.*|localhost|::1|"[::1]") return 0 ;;
    *) return 1 ;;
  esac
}
require_loopback() {
  if ! is_loopback "$DEMO_HOST"; then
    echo "demo.sh: refusing non-loopback host '$DEMO_HOST' (use 127.0.0.1 or localhost)" >&2
    exit 2
  fi
}

# Every process group this demo started: the recorded state plus the crash log
# scripts/e2e/lib.sh's start_bg keeps in $RUN/service.pgids.
pgids_from_state() {
  [[ -f "$STATE" ]] || return 0
  python3 - "$STATE" <<'PY'
import json, sys
try:
    data = json.load(open(sys.argv[1]))
except Exception:
    sys.exit(0)
for pid in data.get("pgids", []):
    print(pid)
PY
}
all_pgids() {
  {
    pgids_from_state
    cat "$RUN/service.pgids" 2>/dev/null || true
  } | grep -E '^[0-9]+$' | sort -u || true
}
state_url() {
  [[ -f "$STATE" ]] || return 1
  python3 - "$STATE" <<'PY'
import json, sys
try:
    print(json.load(open(sys.argv[1])).get("url", ""))
except Exception:
    print("")
PY
}

demo_running() {
  local url pgid
  url="$(state_url)" || return 1
  [[ -n "$url" ]] || return 1
  curl -sf --max-time 2 "$url/" >/dev/null 2>&1 || return 1
  while read -r pgid; do
    [[ -n "$pgid" ]] || continue
    if ps -eo pgid=,stat= | grep -E "^[[:space:]]*$pgid[[:space:]]+[^Z]" >/dev/null; then
      return 0
    fi
  done < <(all_pgids)
  return 1
}

write_state() {
  mkdir -p "$STATE_DIR"
  python3 - "$STATE" "$DEMO_HOST" "$HOST_PORT" \
    "$FIRESTORE_PORT" "$FAKE_PORT" "$TESTBED_PORT" "$UNSUB_PORT" "$API_PORT" \
    ${PGIDS[@]+"${PGIDS[@]}"} <<'PY'
import json, sys
path, host, host_port, fs, fake, testbed, unsub, api = sys.argv[1:9]
json.dump(
    {
        "host": host,
        "url": f"http://{host}:{host_port}",
        "ports": {
            "firestore": int(fs),
            "fake_google": int(fake),
            "unsub_testbed": int(testbed),
            "unsub": int(unsub),
            "api": int(api),
            "host": int(host_port),
        },
        "pgids": [int(p) for p in sys.argv[9:]],
    },
    open(path, "w"),
    indent=2,
)
PY
}

# Build only what is missing, so `--check` reuses an e2e.sh build and stays
# under the 3-minute budget; DEMO_FORCE_BUILD=1 rebuilds anyway.
demo_build() {
  if [[ "${DEMO_FORCE_BUILD:-0}" != "1" ]] \
    && [[ -x "$REPO/backend/target/debug/api" ]] \
    && [[ -x "$REPO/backend/target/debug/unsub" ]] \
    && [[ -x "$REPO/backend/target/debug/fake-google" ]] \
    && [[ -x "$REPO/backend/target/debug/unsub-testbed" ]] \
    && [[ -f "$REPO/app/build/web/index.html" ]]; then
    phase "reusing the existing build"
    return 0
  fi
  phase "building services in test configuration"
  (cd backend && cargo build --locked -p api -p unsub --features api/testkit,unsub/testkit)
  (cd backend && cargo build --locked -p fake-google -p unsub-testbed)
  phase "building the Flutter web app with the e2e define"
  (cd app && flutter build web --release --no-web-resources-cdn --dart-define=MT_E2E=true)
}

demo_start() {
  require_loopback
  if demo_running; then
    echo "demo: already running at $(state_url)"
    return 0
  fi
  mkdir -p "$LOGS" "$RUN"
  rm -f "$RUN/service.pgids" "$STATE"

  demo_build

  phase "Firestore emulator"
  FIRESTORE_PORT="$(free_port)"
  export FIRESTORE_EMULATOR_HOST="127.0.0.1:$FIRESTORE_PORT"
  start_bg firestore "$LOGS/firestore.log" \
    gcloud emulators firestore start --host-port="$FIRESTORE_EMULATOR_HOST" --quiet
  wait_http "http://$FIRESTORE_EMULATOR_HOST/"

  phase "fake-google"
  FAKE_PORT_FILE="$RUN/fake-google.port"
  start_bg fake-google "$LOGS/fake-google.jsonl" \
    env FAKE_GOOGLE_PORT_FILE="$FAKE_PORT_FILE" "$REPO/backend/target/debug/fake-google"
  FAKE_PORT="$(wait_port_file "$FAKE_PORT_FILE" 30)"

  phase "unsub-testbed"
  TESTBED_PORT_FILE="$RUN/unsub-testbed.port"
  start_bg unsub-testbed "$LOGS/unsub-testbed.jsonl" \
    env TESTBED_PORT_FILE="$TESTBED_PORT_FILE" "$REPO/backend/target/debug/unsub-testbed"
  TESTBED_PORT="$(wait_port_file "$TESTBED_PORT_FILE" 30)"

  UNSUB_PORT="$(free_port)"
  API_PORT="$(free_port)"
  HOST_PORT="$(free_port)"

  phase "unsub on 127.0.0.1:$UNSUB_PORT"
  start_bg unsub "$LOGS/unsub.jsonl" env \
    $(env_file "$REPO/scripts/e2e/unsub.env") \
    PORT="$UNSUB_PORT" \
    UNSUB_BASE_URL="http://localhost:$UNSUB_PORT" \
    FIRESTORE_EMULATOR_HOST="$FIRESTORE_EMULATOR_HOST" \
    "$REPO/backend/target/debug/unsub"

  phase "api on 127.0.0.1:$API_PORT"
  start_bg api "$LOGS/api.jsonl" env \
    $(env_file "$REPO/scripts/e2e/api.env") \
    MT_E2E=1 \
    APP_ORIGIN="http://$DEMO_HOST:$HOST_PORT" \
    FAKE_GOOGLE_URL="http://127.0.0.1:$FAKE_PORT" \
    PORT="$API_PORT" \
    FIRESTORE_EMULATOR_HOST="$FIRESTORE_EMULATOR_HOST" \
    "$REPO/backend/target/debug/api"
  wait_http "http://127.0.0.1:$API_PORT/api/v1/healthz"

  phase "serving the web build on http://$DEMO_HOST:$HOST_PORT"
  HOST_PORT_FILE="$RUN/host.port"
  start_bg e2e-host "$LOGS/e2e-host.log" python3 "$REPO/scripts/e2e_host.py" \
    --root "$REPO/app/build/web" \
    --firebase-json "$REPO/firebase.json" \
    --api "http://127.0.0.1:$API_PORT" \
    --port "$HOST_PORT" \
    --port-file "$HOST_PORT_FILE"
  HOST_PORT="$(wait_port_file "$HOST_PORT_FILE" 30)"

  write_state
  echo "demo: running at http://$DEMO_HOST:$HOST_PORT"
}

demo_stop() {
  local pgids pgid remaining=0
  pgids="$(all_pgids | tr '\n' ' ')"
  if [[ -z "${pgids// /}" ]]; then
    rm -f "$STATE"
    echo "demo: nothing running"
    return 0
  fi
  for pgid in $pgids; do kill -TERM -- "-$pgid" 2>/dev/null || true; done
  sleep 2
  for pgid in $pgids; do kill -KILL -- "-$pgid" 2>/dev/null || true; done
  sleep 1
  for pgid in $pgids; do
    if ps -eo pgid=,stat= | grep -E "^[[:space:]]*$pgid[[:space:]]+[^Z]" >/dev/null; then
      echo "demo: process group $pgid still alive" >&2
      remaining=1
    fi
  done
  rm -f "$STATE" "$RUN/service.pgids"
  if (( remaining )); then
    echo "demo: leak census: live processes remain" >&2
    return 1
  fi
  echo "demo: leak census: nothing left"
}

demo_status() {
  if demo_running; then
    echo "demo: running at $(state_url)"
  else
    echo "demo: not running"
    return 1
  fi
}

demo_check() {
  require_loopback
  # Start from nothing so the check proves start and stop, not a leftover.
  demo_stop >/dev/null 2>&1 || true
  trap 'demo_stop >/dev/null 2>&1 || true' EXIT

  demo_start
  local url
  url="$(state_url)"
  mkdir -p "$STATE_DIR"

  phase "fetching the page"
  curl -sf --max-time 15 "$url/" -o "$STATE_DIR/check-page.html"
  if ! grep -qi '<html' "$STATE_DIR/check-page.html"; then
    echo "demo: the page did not look like the app" >&2
    return 1
  fi

  phase "fetching the health route"
  curl -sf --max-time 15 "$url/api/v1/healthz" -o "$STATE_DIR/check-health.json"

  phase "stopping"
  demo_stop
  echo "demo: check ok - page and health served, stack stopped, nothing left"
}

case "$COMMAND" in
  start) demo_start ;;
  stop) demo_stop ;;
  status) demo_status ;;
  check) demo_check ;;
esac
