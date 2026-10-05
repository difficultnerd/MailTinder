#!/usr/bin/env bash
# Start the Firestore emulator on 127.0.0.1:8085 and wait until it answers.
# Installs nothing; requires gcloud with the cloud-firestore-emulator component.
set -euo pipefail

HOST_PORT="${FIRESTORE_EMULATOR_HOST:-127.0.0.1:8085}"
HOST="${HOST_PORT%%:*}"
PORT="${HOST_PORT##*:}"

# Start the emulator in the background if it is not already answering.
if ! curl -sf -m 2 "http://${HOST}:${PORT}/" >/dev/null 2>&1; then
  gcloud emulators firestore start \
    --host-port="${HOST}:${PORT}" \
    --quiet >/tmp/firestore-emulator.log 2>&1 &
fi

# Poll until it answers (60 s cap).
for _ in $(seq 1 60); do
  if curl -sf -m 2 "http://${HOST}:${PORT}/" >/dev/null 2>&1; then
    echo "Firestore emulator up at ${HOST}:${PORT}"
    exit 0
  fi
  sleep 1
done

echo "Firestore emulator failed to start within 60s" >&2
cat /tmp/firestore-emulator.log >&2 2>/dev/null || true
exit 1
