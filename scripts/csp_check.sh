#!/bin/bash
# Build the Flutter web app, serve it under the firebase.json headers, load it
# in headless Chrome, and fail on any CSP violation or a non-rendered app.
# Runs in CI (csp-smoke) where Chrome is installed.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REPORTS="$(mktemp)"
DOM="$(mktemp)"
SERVER_PID=""

cleanup() {
  if [ -n "$SERVER_PID" ]; then kill "$SERVER_PID" 2>/dev/null || true; fi
  rm -f "$REPORTS" "$DOM"
}
trap cleanup EXIT

# 1. Build the app (self-hosted resources, no service worker).
cd "$REPO_ROOT/app"
flutter build web --release --no-web-resources-cdn --pwa-strategy=none

# 2. Start the CSP test server and read its port.
cd "$REPO_ROOT"
python3 scripts/csp_serve.py \
  --root app/build/web --config firebase.json --port 0 --reports "$REPORTS" \
  > /tmp/csp_serve.out 2>&1 &
SERVER_PID=$!
for _ in $(seq 1 50); do
  if grep -q '^PORT ' /tmp/csp_serve.out 2>/dev/null; then break; fi
  sleep 0.2
done
PORT="$(grep -oE '^PORT [0-9]+' /tmp/csp_serve.out | awk '{print $2}')"
if [ -z "$PORT" ]; then
  echo "FAIL: csp_serve.py did not report a port"
  cat /tmp/csp_serve.out
  exit 1
fi

# 3. Load the app in headless Chrome.
google-chrome --headless=new --disable-gpu --no-sandbox \
  --virtual-time-budget=20000 --dump-dom "http://127.0.0.1:${PORT}/" > "$DOM" 2>/dev/null \
  || chromium --headless=new --disable-gpu --no-sandbox \
     --virtual-time-budget=20000 --dump-dom "http://127.0.0.1:${PORT}/" > "$DOM" 2>/dev/null \
  || true

# 4. Fail on any CSP report or a non-rendered app.
VIOLATIONS="$(wc -l < "$REPORTS")"
if [ "$VIOLATIONS" -ne 0 ]; then
  echo "FAIL: $VIOLATIONS CSP violation(s) reported:"
  cat "$REPORTS"
  exit 1
fi
if ! grep -qE 'flutter-view|flt-glass-pane' "$DOM"; then
  echo "FAIL: app did not render (no flutter-view or flt-glass-pane in DOM)"
  exit 1
fi

echo "OK: app rendered under the shipped CSP with zero violations"
