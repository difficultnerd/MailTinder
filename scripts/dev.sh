#!/usr/bin/env bash
# Dev orchestration for MailTinder.
#
# Starts the Firestore emulator (if not already up), the fake-google test
# double, and serves the Flutter web app so the stack is testable in one
# command. Prints the URLs to hit.
#
# Usage:
#   ./scripts/dev.sh            # emulator + fake-google + serve app (dev)
#   ./scripts/dev.sh --build    # also do a production web build first
#   ./scripts/dev.sh --api      # also start the API service (needs M5 built)
#
# Env overrides:
#   FAKE_GOOGLE_ADDR   fake-google bind (default 127.0.0.1:9099)
#   APP_PORT           flutter web-server port (default 8080)
#   API_ORIGIN         base URL the app talks to (default: same-origin)
set -euo pipefail
REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO"

FAKE_GOOGLE_ADDR="${FAKE_GOOGLE_ADDR:-127.0.0.1:9099}"
APP_PORT="${APP_PORT:-8080}"
API_ORIGIN="${API_ORIGIN:-}"

# 1. Firestore emulator (idempotent; exits fast if already answering).
echo "==> Firestore emulator"
./scripts/firestore-emulator.sh

# 2. fake-google test double (Gmail/Drive/OAuth stand-in).
echo "==> fake-google on ${FAKE_GOOGLE_ADDR}"
if ! curl -sf -m 2 "http://${FAKE_GOOGLE_ADDR}/" >/dev/null 2>&1; then
  (cd backend && FAKE_GOOGLE_ADDR="$FAKE_GOOGLE_ADDR" cargo run -p fake-google >/tmp/fake-google.log 2>&1 &)
  echo "    started (log: /tmp/fake-google.log)"
else
  echo "    already running"
fi

# 3. Optional production web build.
if [[ "${1:-}" == "--build" ]]; then
  echo "==> flutter build web"
  (cd app && flutter build web)
fi

# 4. Serve the app.
echo "==> serving app on http://localhost:${APP_PORT}"
DART_DEFINES=()
if [[ -n "$API_ORIGIN" ]]; then
  DART_DEFINES+=(--dart-define=API_ORIGIN="$API_ORIGIN")
fi
cd app
exec flutter run -d web-server --web-port "$APP_PORT" --web-hostname 0.0.0.0 "${DART_DEFINES[@]}"
