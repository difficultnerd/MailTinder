#!/usr/bin/env bash
# External WebDriver journeys: full-page OAuth cannot run inside integration_test.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
journey=()
if (($#)); then
  if [[ $# != 2 || $1 != --journey || $2 != sign_in ]]; then
    printf 'Usage: scripts/e2e.sh [--journey sign_in]\n' >&2
    exit 2
  fi
  journey=(--test "$2")
fi
logs="$ROOT/target/e2e-logs"
mkdir -p "$logs"
work="$(mktemp -d "${TMPDIR:-$ROOT/target}/e2e.XXXXXX")"
pids=()
cleanup() {
  local pid
  for pid in "${pids[@]}"; do kill -- "-$pid" 2>/dev/null || true; done
  for pid in "${pids[@]}"; do wait "$pid" 2>/dev/null || true; done
  rm -rf "$work"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
start() {
  local name=$1; shift
  setsid "$@" > "$logs/$name.jsonl" 2>&1 &
  pids+=("$!")
}
wait_port_file() {
  local path=$1
  for ((i=0;i<300;i++)); do [[ -s $path ]] && return; sleep .1; done
  printf 'Service did not publish its port: %s\n' "$path" >&2; return 1
}
wait_http() {
  local url=$1
  for ((i=0;i<300;i++)); do curl -fsS --max-time 1 "$url" >/dev/null 2>&1 && return; sleep .1; done
  printf 'Service readiness failed\n' >&2; return 1
}
port() { python3 scripts/free_port.py; }
(cd backend && cargo build -q --locked -p api -p unsub --features api/testkit,unsub/testkit && cargo build -q --locked -p fake-google -p unsub-testbed)
(cd app && flutter build web --release --no-web-resources-cdn --dart-define=MT_E2E=true) > "$logs/flutter-build.log" 2>&1
export FIRESTORE_EMULATOR_HOST="127.0.0.1:$(port)"
export API_ADDR="127.0.0.1:$(port)" UNSUB_ADDR="127.0.0.1:$(port)"
host_port=$(port)
export MT_E2E_APP_URL="http://localhost:$host_port"
start firestore gcloud emulators firestore start --host-port="$FIRESTORE_EMULATOR_HOST" --quiet
wait_http "http://$FIRESTORE_EMULATOR_HOST/"
start fake-google env FAKE_GOOGLE_ADDR=127.0.0.1:0 backend/target/debug/fake-google --port-file "$work/google.port"
start unsub-testbed env TESTBED_ADDR=127.0.0.1:0 backend/target/debug/unsub-testbed --port-file "$work/testbed.port"
wait_port_file "$work/google.port"
wait_port_file "$work/testbed.port"
export MT_E2E_FAKE_GOOGLE_URL="http://localhost:$(<"$work/google.port")/"
export MT_E2E_TESTBED_URL="http://localhost:$(<"$work/testbed.port")/"
export MT_E2E_API_URL="http://localhost:${API_ADDR##*:}/"
set -a
source scripts/e2e/unsub.env
start unsub backend/target/debug/unsub
source scripts/e2e/api.env
set +a
start api backend/target/debug/api
wait_http "${MT_E2E_API_URL%/}/api/v1/healthz"
# Register only the local redirect URI with the synthetic OAuth server.
python3 -c 'import json,os,urllib.request; u=os.environ["MT_E2E_FAKE_GOOGLE_URL"]+"__fake/identity/clients"; b=json.dumps({"client_id":"e2e-client","client_secret":"synthetic-client-secret","redirect_uris":[os.environ["MT_E2E_APP_URL"]+"/api/v1/auth/google/callback"]}).encode(); urllib.request.urlopen(urllib.request.Request(u,data=b,headers={"Content-Type":"application/json"})).close()'
start host python3 scripts/e2e_host.py --root app/build/web --firebase-json firebase.json --api "http://$API_ADDR" --port "$host_port" --port-file "$work/host.port"
wait_port_file "$work/host.port"
wait_http "$MT_E2E_APP_URL/"
cd_port=$(port)
export MT_E2E_WEBDRIVER_URL="http://localhost:$cd_port"
driver="${CHROMEWEBDRIVER:+$CHROMEWEBDRIVER/}chromedriver"
start chromedriver "$driver" --port="$cd_port" --allowed-ips=127.0.0.1
wait_http "$MT_E2E_WEBDRIVER_URL/status"
(cd backend && cargo test -q --locked -p e2e "${journey[@]}" -- --ignored --test-threads=1)
