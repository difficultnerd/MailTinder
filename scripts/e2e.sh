#!/usr/bin/env bash
# End-to-end harness (T-1101a): build, start, run, stop.
#
# One command brings up the whole local stack - fake-google, unsub-testbed, the
# Firestore emulator, `api` and `unsub` in test configuration, and the Flutter
# web build served with the firebase.json headers - and runs the WebDriver
# journeys against it (S10 3.2/3.3). Every service binds 127.0.0.1 on a free
# port; every child is killed on exit, even on failure.
#
# Usage:
#   scripts/e2e.sh [--journey <name>]   # <name> is an e2e test target, e.g. sign_in
#
# Env overrides: CHROMEWEBDRIVER (dir containing chromedriver), GITHUB_ACTIONS.
set -euo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO"

# Per-phase timing: a cold CI runner is far slower than a warm developer machine,
# so every step prints how long it has taken so far.
START_S=$SECONDS
phase() { printf '==> [%3ss] %s\n' "$((SECONDS - START_S))" "$*"; }

JOURNEY=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --journey) JOURNEY="${2:?--journey needs a name}"; shift 2 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done

LOGS="$REPO/target/e2e-logs"
RUN="$REPO/target/e2e-run"
rm -rf "$RUN"
mkdir -p "$LOGS" "$RUN"

# The harness writes failure artifacts (screenshot, page text/source, semantics,
# console log, ChromeDriver tail) here; the `e2e` CI job uploads this folder on
# failure, so the folder exists before the first journey runs.
export MT_E2E_LOG_DIR="$LOGS"

# `$CHROMEWEBDRIVER` is set on GitHub-hosted runners; locally chromedriver is on
# PATH.
CHROMEDRIVER="${CHROMEWEBDRIVER:+$CHROMEWEBDRIVER/}chromedriver"

# Services own process groups, including gcloud's Java and ChromeDriver's Chrome.
PGIDS=()
cleanup() {
  local status=$? pgid remaining=0
  trap - EXIT INT TERM
  for pgid in "${PGIDS[@]}"; do
    kill -TERM -- "-$pgid" 2>/dev/null || true
  done
  sleep 3
  for pgid in "${PGIDS[@]}"; do
    kill -KILL -- "-$pgid" 2>/dev/null || true
  done
  wait 2>/dev/null || true
  # Ignore already-dead zombies; fail if any live member escaped termination.
  for pgid in "${PGIDS[@]}"; do
    if ps -eo pgid=,stat= | grep -E "^[[:space:]]*$pgid[[:space:]]+[^Z]" >/dev/null; then
      echo "cleanup failed for process group $pgid" >&2
      remaining=1
    fi
  done
  if (( remaining )); then status=1; else echo "==> cleanup: no live service processes remain"; fi
  exit "$status"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

free_port() { python3 "$REPO/scripts/free_port.py"; }

wait_http() {
  local url="$1" waited=0
  while (( waited < 60 )); do
    if curl -sf --max-time 2 "$url" >/dev/null; then
      echo "    ready after ${waited}s: $url" >&2
      return 0
    fi
    sleep 1
    waited=$((waited + 1))
  done
  echo "timed out after ${waited}s waiting for $url" >&2
  return 1
}

# Wait up to `max` seconds for a service's port file and echo its contents.
wait_port_file() {
  local file="$1" max="${2:-30}" waited=0
  while [[ ! -s "$file" ]]; do
    if (( waited >= max )); then
      echo "timed out waiting for $file" >&2
      return 1
    fi
    sleep 1
    waited=$((waited + 1))
  done
  # Timing goes to stderr: stdout is the port value the caller captures.
  echo "    $file ready after ${waited}s" >&2
  tr -d '[:space:]' < "$file"
}

start_bg() { # <name> <logfile> <command...>
  local name="$1" log="$2"
  shift 2
  setsid "$@" >"$log" 2>&1 &
  PGIDS+=("$!")
  printf '%s\n' "$!" >>"$RUN/service.pgids"
  echo "    started $name (log: $log)"
}

# The .env files hold values only; drop comments and blank lines and let env
# consume the rest as KEY=VALUE.
env_file() { grep -vE '^[[:space:]]*(#|$)' "$1" | xargs; }

phase "building services in test configuration"
(cd backend && cargo build --locked -p api -p unsub --features api/testkit,unsub/testkit)
(cd backend && cargo build --locked -p fake-google -p unsub-testbed)

phase "building the Flutter web app with the e2e define"
(cd app && flutter build web --release --no-web-resources-cdn --dart-define=MT_E2E=true)

phase "Firestore emulator"
FIRESTORE_PORT="$(free_port)"
export FIRESTORE_EMULATOR_HOST="127.0.0.1:$FIRESTORE_PORT"
start_bg firestore "$LOGS/firestore.log" \
  gcloud emulators firestore start --host-port="$FIRESTORE_EMULATOR_HOST" --quiet
# The api's Firestore client must see the emulator accepting connections before
# the api starts, or the first request fails on a cold runner (T-1101a CI).
wait_http "http://$FIRESTORE_EMULATOR_HOST/"

phase "fake-google"
FAKE_PORT_FILE="$RUN/fake-google.port"
start_bg fake-google "$LOGS/fake-google.jsonl" \
  env FAKE_GOOGLE_PORT_FILE="$FAKE_PORT_FILE" "$REPO/backend/target/debug/fake-google"
FAKE_PORT="$(wait_port_file "$FAKE_PORT_FILE" 30)"
export MT_E2E_FAKE_GOOGLE_URL="http://localhost:$FAKE_PORT"

phase "unsub-testbed"
TESTBED_PORT_FILE="$RUN/unsub-testbed.port"
start_bg unsub-testbed "$LOGS/unsub-testbed.jsonl" \
  env TESTBED_PORT_FILE="$TESTBED_PORT_FILE" "$REPO/backend/target/debug/unsub-testbed"
TESTBED_PORT="$(wait_port_file "$TESTBED_PORT_FILE" 30)"
export MT_E2E_TESTBED_URL="http://localhost:$TESTBED_PORT"

UNSUB_PORT="$(free_port)"
API_PORT="$(free_port)"
HOST_PORT="$(free_port)"
CD_PORT="$(free_port)"

# The discovered values come after the env files so they win: the files carry a
# placeholder emulator host, but this run's emulator is on the free port above.
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
  APP_ORIGIN="http://localhost:$HOST_PORT" \
  FAKE_GOOGLE_URL="http://127.0.0.1:$FAKE_PORT" \
  PORT="$API_PORT" \
  FIRESTORE_EMULATOR_HOST="$FIRESTORE_EMULATOR_HOST" \
  "$REPO/backend/target/debug/api"
wait_http "http://127.0.0.1:$API_PORT/api/v1/healthz"

phase "serving the web build on http://localhost:$HOST_PORT"
HOST_PORT_FILE="$RUN/host.port"
start_bg e2e-host "$LOGS/e2e-host.log" python3 "$REPO/scripts/e2e_host.py" \
  --root "$REPO/app/build/web" \
  --firebase-json "$REPO/firebase.json" \
  --api "http://127.0.0.1:$API_PORT" \
  --port "$HOST_PORT" \
  --port-file "$HOST_PORT_FILE"
HOST_PORT="$(wait_port_file "$HOST_PORT_FILE" 30)"
# Use `localhost` in the URLs so Chrome accepts `Secure` cookies over http.
export MT_E2E_APP_URL="http://localhost:$HOST_PORT"
export MT_E2E_API_URL="http://127.0.0.1:$API_PORT"

phase "chromedriver on 127.0.0.1:$CD_PORT"
# `--verbose` makes ChromeDriver log each command and any browser crash; the
# harness tails the file into the failure diagnostics.
start_bg chromedriver "$LOGS/chromedriver.log" "$CHROMEDRIVER" --port="$CD_PORT" --verbose
wait_http "http://127.0.0.1:$CD_PORT/status"
export MT_E2E_WEBDRIVER_URL="http://127.0.0.1:$CD_PORT"
export MT_E2E_CHROMEDRIVER_LOG="$LOGS/chromedriver.log"

phase "journeys"
# `--nocapture` prints each journey step's elapsed time as it happens, so a slow
# step is visible in the CI log even before the failure diagnostics land.
if [[ -n "$JOURNEY" ]]; then
  (cd backend && cargo test --locked -p e2e --test "$JOURNEY" -- --ignored --test-threads=1 --nocapture)
else
  (cd backend && cargo test --locked -p e2e -- --ignored --test-threads=1 --nocapture)
fi

phase "e2e passed"
