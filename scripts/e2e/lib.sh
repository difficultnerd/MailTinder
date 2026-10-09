#!/usr/bin/env bash
# Shared local-stack start/stop helpers (T-1108a).
#
# Sourced by scripts/e2e.sh and scripts/demo.sh; it defines functions only, so
# sourcing it has no side effects. The caller provides:
#   REPO    - repository root
#   RUN     - scratch dir for a run (port files, service.pgids)
#   START_S - $SECONDS when the caller started, for phase() timing
#   PGIDS   - a bash array the caller owns; start_bg appends to it
#
# Every helper keeps the conventions scripts/e2e.sh had before the split: each
# service is a new session/process group, ports are free ports on loopback, and
# nothing is written outside RUN and the log files the caller chose.
#
# shellcheck shell=bash

# Print a free TCP port on loopback (scripts/free_port.py; stdlib only).
free_port() { python3 "$REPO/scripts/free_port.py"; }

# Wait until an HTTP URL answers, up to 60 s. Timing goes to stderr.
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

# Per-phase timing: a cold CI runner is far slower than a warm developer
# machine, so every step prints how long it has taken so far.
phase() { printf '==> [%3ss] %s\n' "$((SECONDS - START_S))" "$*"; }
