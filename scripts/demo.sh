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
# Everything binds loopback only - 127.0.0.0/8, `localhost` or IPv6 `::1`. The
# host is validated strictly and is the address the web server actually binds.
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
# Phone mode (T-1108c): a cloudflared quick tunnel to the front door plus a
# generated access code. Test seams only: DEMO_CLOUDFLARED (binary path),
# DEMO_CLOUDFLARED_SHA256 (pin file) and DEMO_TUNNEL_TARGET (target URL, still
# required to be loopback) exist so the tunnel can be driven with a stub.
PHONE=0
ACCESS_CODE=""
ACCESS_CODE_FILE=""
PUBLIC_URL=""
TUNNEL_TARGET=""
CLOUDFLARED_PIN="${DEMO_CLOUDFLARED_SHA256:-$REPO/scripts/demo/cloudflared.sha256}"

usage() {
  cat <<'EOF'
demo.sh [start] [--host <loopback addr>] [--phone]   start and leave the stack running
demo.sh stop                               stop and print the leak census
demo.sh status                             is the stack running?
demo.sh --check                            automatic start, fetch, stop

--phone exposes the front door through a cloudflared quick tunnel protected by
a generated access code. cloudflared must be the pinned release on PATH or at
~/bin/cloudflared; --check and --phone cannot be combined.
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    start) COMMAND="start"; shift ;;
    stop) COMMAND="stop"; shift ;;
    status) COMMAND="status"; shift ;;
    --check) COMMAND="check"; shift ;;
    --phone) PHONE=1; shift ;;
    --host) DEMO_HOST="${2:?--host needs an address}"; shift 2 ;;
    -h|--help) usage; exit 0 ;;
    *) echo "demo.sh: unknown argument: $1" >&2; exit 2 ;;
  esac
done

# Accept the bracketed IPv6 spelling too ([::1] -> ::1).
DEMO_HOST="${DEMO_HOST#[}"
DEMO_HOST="${DEMO_HOST%]}"

# Loopback only (behaviour 1). Matching a strict dotted-quad is deliberate: the
# old `127.*` glob also accepted DNS names that merely start with `127.`, such
# as `127.evil.example`, which then reached APP_ORIGIN and the printed URL
# (T-1108a F1).
is_loopback() {
  local host="${1#[}" octet
  host="${host%]}"
  case "$host" in
    localhost|::1) return 0 ;;
  esac
  [[ "$host" =~ ^127(\.[0-9]{1,3}){3}$ ]] || return 1
  local IFS=.
  for octet in $host; do
    (( 10#$octet <= 255 )) || return 1
  done
  return 0
}
require_loopback() {
  if ! is_loopback "$DEMO_HOST"; then
    echo "demo.sh: refusing non-loopback host '$DEMO_HOST' (use 127.0.0.1, localhost or ::1)" >&2
    exit 2
  fi
}
# The host as it appears inside a URL: an IPv6 literal needs brackets.
url_host() {
  case "$DEMO_HOST" in
    *:*) printf '[%s]' "$DEMO_HOST" ;;
    *) printf '%s' "$DEMO_HOST" ;;
  esac
}

# ---------------------------------------------------------------- phone mode --
# T-1108c: the tunnel target is the front door on loopback, never any of the
# other services (behaviour 1). The URL form is parsed so an encoded or bracketed
# spelling cannot slip a non-loopback host past the guard.

# True when a URL's host is loopback (the same strict rule as --host).
is_loopback_url() {
  local url="$1" host
  case "$url" in
    http://*|https://*) ;;
    *) return 1 ;;
  esac
  host="$(python3 - "$url" <<'PY'
import sys, urllib.parse
try:
    print(urllib.parse.urlsplit(sys.argv[1]).hostname or "")
except ValueError:
    print("")
PY
)"
  is_loopback "$host"
}
require_loopback_url() {
  if ! is_loopback_url "$1"; then
    echo "demo.sh: refusing non-loopback tunnel target '$1' (the front door must stay on loopback)" >&2
    exit 2
  fi
}

# The pinned release's Linux artefact for this machine, or nothing.
cloudflared_arch() {
  case "$(uname -m)" in
    x86_64|amd64) printf 'linux-amd64' ;;
    aarch64|arm64) printf 'linux-arm64' ;;
    *) return 1 ;;
  esac
}

# The cloudflared to use: the DEMO_CLOUDFLARED override, PATH, or ~/bin.
cloudflared_bin() {
  if [[ -n "${DEMO_CLOUDFLARED:-}" ]]; then
    # An override that is not actually executable is treated as not found.
    [[ -x "$DEMO_CLOUDFLARED" ]] || return 1
    printf '%s' "$DEMO_CLOUDFLARED"
    return 0
  fi
  if command -v cloudflared >/dev/null 2>&1; then
    command -v cloudflared
    return 0
  fi
  if [[ -x "$HOME/bin/cloudflared" ]]; then
    printf '%s' "$HOME/bin/cloudflared"
    return 0
  fi
  return 1
}

pinned_version() {
  [[ -f "$CLOUDFLARED_PIN" ]] || return 0
  awk '$1 == "version" {print $2; exit}' "$CLOUDFLARED_PIN" 2>/dev/null || true
}
pinned_sha() {
  [[ -f "$CLOUDFLARED_PIN" ]] || return 0
  awk -v a="$1" '$1 == "sha256" && $2 == a {print $3; exit}' "$CLOUDFLARED_PIN" 2>/dev/null || true
}

# Verify cloudflared against the pin BEFORE it is ever executed, and run exactly the bytes that were verified.
# Order (security review N1/N2): (1) copy the candidate into a private mode-700 file, (2) check the SHA-256 of THAT COPY, (3) only then run the
# copy with --version, (4) remember the copy as CLOUDFLARED_VERIFIED; start_tunnel executes only that path. The original path is never
# executed, so it cannot be swapped between verification and use, and an unverified binary is never run.
verify_cloudflared() {
  local bin arch expected_version expected_sha version actual_sha copy
  if ! bin="$(cloudflared_bin)"; then
    echo "demo.sh: --phone needs cloudflared on PATH or at ~/bin/cloudflared" >&2
    exit 2
  fi
  if ! arch="$(cloudflared_arch)"; then
    echo "demo.sh: refusing cloudflared: unsupported architecture $(uname -m)" >&2
    exit 2
  fi
  expected_version="$(pinned_version)"
  expected_sha="$(pinned_sha "$arch")"
  if [[ -z "$expected_version" || -z "$expected_sha" ]]; then
    echo "demo.sh: refusing cloudflared: no pin for $arch in $CLOUDFLARED_PIN" >&2
    exit 2
  fi
  mkdir -p "$RUN"
  copy="$RUN/cloudflared"
  rm -f -- "$copy"
  ( umask 077; cp -- "$bin" "$copy" ) || { echo "demo.sh: refusing cloudflared: cannot copy it for verification" >&2; exit 2; }
  chmod 700 "$copy"
  actual_sha="$(sha256sum "$copy" | awk '{print $1}')"
  if [[ "$actual_sha" != "$expected_sha" ]]; then
    rm -f -- "$copy"
    echo "demo.sh: refusing cloudflared: SHA-256 mismatch for $arch" >&2
    exit 2
  fi
  version="$("$copy" --version 2>/dev/null || true)"
  if [[ "$version" != *"$expected_version"* ]]; then
    rm -f -- "$copy"
    echo "demo.sh: refusing cloudflared: expected version $expected_version, got '${version:-none}'" >&2
    exit 2
  fi
  CLOUDFLARED_VERIFIED="$copy"
}

# 16 characters from [A-Za-z0-9] from the OS random source (>= 95 bits).
generate_access_code() {
  python3 - <<'PY'
import secrets
import string

alphabet = string.ascii_letters + string.digits
print("".join(secrets.choice(alphabet) for _ in range(16)))
PY
}

# Validate the tunnel target and cloudflared, then generate the access code into
# a mode-600 file. Runs before anything is started or built, so a bad binary or
# target costs nothing and the code is never an argument (N2).
phone_prepare() {
  verify_cloudflared
  if [[ -n "${DEMO_TUNNEL_TARGET:-}" ]]; then
    require_loopback_url "$DEMO_TUNNEL_TARGET"
  fi
  ACCESS_CODE="$(generate_access_code)"
  if [[ ! "$ACCESS_CODE" =~ ^[A-Za-z0-9]{16}$ ]]; then
    echo "demo.sh: refusing to start: could not generate an access code" >&2
    exit 2
  fi
  mkdir -p "$RUN"
  ACCESS_CODE_FILE="$RUN/access-code"
  local old_umask
  old_umask="$(umask)"
  umask 077
  printf '%s\n' "$ACCESS_CODE" > "$ACCESS_CODE_FILE"
  umask "$old_umask"
  chmod 600 "$ACCESS_CODE_FILE"
}

# Start the quick tunnel to the loopback front door and read the public URL it
# prints. The target is whatever phone_prepare/demo_start computed.
start_tunnel() {
  local log="$LOGS/cloudflared.log" bin waited=0
  bin="${CLOUDFLARED_VERIFIED:-}"
  if [[ -z "$bin" || ! -x "$bin" ]]; then
    echo "demo.sh: refusing to start the tunnel: no verified cloudflared (internal error)" >&2
    return 1
  fi
  start_bg cloudflared "$log" "$bin" tunnel --url "$TUNNEL_TARGET" --no-autoupdate
  while (( waited < 60 )); do
    PUBLIC_URL="$(grep -Eom1 'https://[A-Za-z0-9-]+\.trycloudflare\.com' "$log" 2>/dev/null || true)"
    [[ -n "$PUBLIC_URL" ]] && break
    sleep 1
    waited=$((waited + 1))
  done
  if [[ -z "$PUBLIC_URL" ]]; then
    echo "demo.sh: cloudflared did not report a public URL within ${waited}s" >&2
    return 1
  fi
  echo "    tunnel up: $PUBLIC_URL"
}

# Every process group this demo started, as "pgid <starttime>" lines: the
# recorded state plus the crash log scripts/e2e/lib.sh's start_bg appends to.
pgids_from_state() {
  [[ -f "$STATE" ]] || return 0
  python3 - "$STATE" <<'PY'
import json, sys
try:
    data = json.load(open(sys.argv[1]))
except Exception:
    sys.exit(0)
for entry in data.get("pgids", []):
    if isinstance(entry, dict):
        print(entry.get("pgid", ""), entry.get("start", ""))
    else:
        print(entry, "")
PY
}
all_pgids() {
  {
    pgids_from_state
    cat "$RUN/service.pgids" 2>/dev/null || true
  } | awk 'NF && $1 ~ /^[0-9]+$/ {print $1, ($2 == "" ? "-" : $2)}' | sort -u || true
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
# One field from the state file (empty when absent). Used for the phone-mode
# public URL and access-code file; the code itself is never written to state.
state_field() {
  [[ -f "$STATE" ]] || return 1
  python3 - "$STATE" "$1" <<'PY'
import json, sys
try:
    print(json.load(open(sys.argv[1])).get(sys.argv[2], ""))
except Exception:
    print("")
PY
}

# True only when the group still looks like one this demo started: the id is a
# real process group (0 and 1 are rejected - `kill -- -0` would signal the
# caller's own group) and, when a start-time token was recorded, the leader's
# current /proc start time matches. The token changes when a pid is reused, so a
# stale or tampered state file cannot make `stop` signal an unrelated group
# (T-1108a F3).
pgid_is_ours() {
  local pgid="$1" start="$2" now
  [[ "$pgid" =~ ^[0-9]+$ ]] || return 1
  (( pgid >= 2 )) || return 1
  if [[ -z "$start" || "$start" == "-" ]]; then
    # Nothing to verify against. On Linux /proc always exists, so an absent
    # token means the record is untrustworthy and the group is not ours.
    [[ -r /proc/self/stat ]] && return 1
    return 0
  fi
  now="$(proc_starttime "$pgid" 2>/dev/null || true)"
  [[ -n "$now" && "$now" == "$start" ]]
}

demo_running() {
  local url pgid start code_file code
  url="$(state_url)" || return 1
  [[ -n "$url" ]] || return 1
  # Phone mode puts the whole front door behind the access code, so a bare curl
  # is answered 401; authenticate from the mode-600 file when it is recorded.
  # The code is fed to curl over stdin through `--config -`, never as an
  # argument: `-u "demo:$(cat ...)"` would put the only credential protecting
  # the public tunnel into /proc/<pid>/cmdline for the length of every status or
  # start check (T-1108c F1). `printf` is a shell builtin, so the code never
  # reaches any process's argv on the way in either.
  code_file="$(state_field access_code_file)"
  if [[ -n "$code_file" && -r "$code_file" ]]; then
    code="$(<"$code_file")"
    printf 'user = "demo:%s"\n' "$code" \
      | curl -sf --max-time 2 --config - "$url/" >/dev/null 2>&1 || return 1
  else
    curl -sf --max-time 2 "$url/" >/dev/null 2>&1 || return 1
  fi
  while read -r pgid start; do
    [[ -n "$pgid" ]] || continue
    if ps -eo pgid=,stat= | grep -E "^[[:space:]]*$pgid[[:space:]]+[^Z]" >/dev/null; then
      return 0
    fi
  done < <(all_pgids)
  return 1
}

write_state() {
  mkdir -p "$STATE_DIR"
  python3 - "$STATE" "$DEMO_HOST" "$(url_host)" "$HOST_PORT" \
    "$FIRESTORE_PORT" "$FAKE_PORT" "$TESTBED_PORT" "$UNSUB_PORT" "$API_PORT" \
    "$RUN/service.pgids" "$PUBLIC_URL" "$ACCESS_CODE_FILE" <<'PY'
import json, sys
(
    path, host, url_host, host_port, fs, fake, testbed, unsub, api, pgfile,
    public_url, access_code_file,
) = sys.argv[1:13]
pgids = []
try:
    with open(pgfile) as handle:
        for line in handle:
            parts = line.split()
            if parts and parts[0].isdigit():
                pgids.append(
                    {"pgid": int(parts[0]), "start": parts[1] if len(parts) > 1 else ""}
                )
except FileNotFoundError:
    pass
json.dump(
    {
        "host": host,
        "url": f"http://{url_host}:{host_port}",
        "public_url": public_url,
        "access_code_file": access_code_file,
        "ports": {
            "firestore": int(fs),
            "fake_google": int(fake),
            "unsub_testbed": int(testbed),
            "unsub": int(unsub),
            "api": int(api),
            "host": int(host_port),
        },
        "pgids": pgids,
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
  # Phone mode fails fast: verify the tunnel binary and generate the access code
  # before anything is built or started (behaviour 2).
  if (( PHONE )); then
    phone_prepare
  fi
  mkdir -p "$LOGS" "$RUN"
  # Clear per-run scratch. The *.port files matter: wait_port_file returns the
  # first non-empty file it sees, so a leftover port from a previous run would
  # be read as this run's port. In --phone that made the printed front-door URL
  # (read back from host.port) disagree with the loopback port the tunnel was
  # actually pointed at (chosen a moment earlier) - the CI-only front-door
  # mismatch (T-1108c).
  rm -f "$RUN/service.pgids" "$STATE" "$RUN"/*.port

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

  if (( PHONE )); then
    # The tunnel exposes the front door only; never api, unsub, fakes or the
    # emulator. The target is the same loopback address the front door binds.
    TUNNEL_TARGET="${DEMO_TUNNEL_TARGET:-http://$(url_host):$HOST_PORT}"
    require_loopback_url "$TUNNEL_TARGET"
  fi

  phase "unsub on 127.0.0.1:$UNSUB_PORT"
  start_bg unsub "$LOGS/unsub.jsonl" env \
    $(env_file "$REPO/scripts/e2e/unsub.env") \
    PORT="$UNSUB_PORT" \
    UNSUB_BASE_URL="http://localhost:$UNSUB_PORT" \
    FIRESTORE_EMULATOR_HOST="$FIRESTORE_EMULATOR_HOST" \
    "$REPO/backend/target/debug/unsub"

  # Behaviour 0a/0b: the tunnel is up first so the api is handed the public
  # https origin for the OAuth redirect URI, the Secure cookies and the
  # browser-facing authorisation URL.
  if (( PHONE )); then
    phase "tunnel to the front door on 127.0.0.1:$HOST_PORT"
    start_tunnel
  fi

  phase "api on 127.0.0.1:$API_PORT"
  if (( PHONE )); then
    start_bg api "$LOGS/api.jsonl" env \
      $(env_file "$REPO/scripts/e2e/api.env") \
      MT_E2E=1 \
      APP_ORIGIN="$PUBLIC_URL" \
      MT_PUBLIC_BASE_URL="$PUBLIC_URL" \
      FAKE_GOOGLE_URL="http://127.0.0.1:$FAKE_PORT" \
      PORT="$API_PORT" \
      FIRESTORE_EMULATOR_HOST="$FIRESTORE_EMULATOR_HOST" \
      "$REPO/backend/target/debug/api"
  else
    start_bg api "$LOGS/api.jsonl" env \
      $(env_file "$REPO/scripts/e2e/api.env") \
      MT_E2E=1 \
      APP_ORIGIN="http://$(url_host):$HOST_PORT" \
      FAKE_GOOGLE_URL="http://127.0.0.1:$FAKE_PORT" \
      PORT="$API_PORT" \
      FIRESTORE_EMULATOR_HOST="$FIRESTORE_EMULATOR_HOST" \
      "$REPO/backend/target/debug/api"
  fi
  wait_http "http://127.0.0.1:$API_PORT/api/v1/healthz"

  phase "serving the web build on http://$(url_host):$HOST_PORT"
  HOST_PORT_FILE="$RUN/host.port"
  if (( PHONE )); then
    start_bg e2e-host "$LOGS/e2e-host.log" python3 "$REPO/scripts/e2e_host.py" \
      --root "$REPO/app/build/web" \
      --firebase-json "$REPO/firebase.json" \
      --host "$DEMO_HOST" \
      --api "http://127.0.0.1:$API_PORT" \
      --port "$HOST_PORT" \
      --port-file "$HOST_PORT_FILE" \
      --fake-google "http://127.0.0.1:$FAKE_PORT" \
      --access-code-file "$ACCESS_CODE_FILE"
  else
    start_bg e2e-host "$LOGS/e2e-host.log" python3 "$REPO/scripts/e2e_host.py" \
      --root "$REPO/app/build/web" \
      --firebase-json "$REPO/firebase.json" \
      --host "$DEMO_HOST" \
      --api "http://127.0.0.1:$API_PORT" \
      --port "$HOST_PORT" \
      --port-file "$HOST_PORT_FILE"
  fi
  HOST_PORT="$(wait_port_file "$HOST_PORT_FILE" 30)"

  write_state
  phase "seeding the demo mailbox and invite (T-1108b)"
  # The fake OAuth client's redirect_uri must be the origin the BROWSER uses: the public tunnel URL in --phone mode (the api builds redirect_uri
  # from APP_ORIGIN = that URL), the local address otherwise. Registering the local one in phone mode made "Continue with Google" fail with
  # invalid_request on the first real phone sign-in (AAR 3.89).
  local seed_origin="http://$(url_host):$HOST_PORT"
  if (( PHONE )); then seed_origin="$PUBLIC_URL"; fi
  python3 "$REPO/scripts/demo_seed.py" \
    --fake-google-url "http://127.0.0.1:$FAKE_PORT" \
    --api-url "http://127.0.0.1:$API_PORT" \
    --app-origin "$seed_origin"
  echo "demo: running at http://$(url_host):$HOST_PORT"
  if (( PHONE )); then
    # Printed once, to the terminal only; never logged or committed.
    echo "demo: phone URL: $PUBLIC_URL"
    echo "demo: access code (sign in as user 'demo'): $ACCESS_CODE"
  fi
}

demo_stop() {
  local pgid start remaining=0 ours=()
  while read -r pgid start; do
    [[ -n "$pgid" ]] || continue
    if pgid_is_ours "$pgid" "$start"; then
      ours+=("$pgid")
    else
      echo "demo: ignoring process group $pgid (not this demo's, or already gone)" >&2
    fi
  done < <(all_pgids)

  if (( ${#ours[@]} == 0 )); then
    rm -f "$STATE"
    echo "demo: nothing running"
    return 0
  fi
  for pgid in "${ours[@]}"; do kill -TERM -- "-$pgid" 2>/dev/null || true; done
  sleep 2
  for pgid in "${ours[@]}"; do kill -KILL -- "-$pgid" 2>/dev/null || true; done
  sleep 1
  for pgid in "${ours[@]}"; do
    if ps -eo pgid=,stat= | grep -E "^[[:space:]]*$pgid[[:space:]]+[^Z]" >/dev/null; then
      echo "demo: process group $pgid still alive" >&2
      remaining=1
    fi
  done
  rm -f "$STATE" "$RUN/service.pgids" "$RUN/access-code"
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
  if (( PHONE )); then
    echo "demo: --check cannot be combined with --phone" >&2
    return 2
  fi
  # Never kill a demo the user is already running (F4): refuse instead of
  # silently tearing their stack down, and start from nothing only when it is
  # already down.
  if demo_running; then
    echo "demo: --check refuses to stop a running demo; run 'scripts/demo.sh stop' first" >&2
    return 2
  fi
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
