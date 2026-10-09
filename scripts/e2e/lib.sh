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

# Field helpers over /proc/<pid>/stat. The comm field (2) is parenthesised and
# may contain spaces, so strip through the last ')'; the remaining fields then
# start at field 3, which makes pgrp field 5 the 3rd token and starttime field
# 22 the 20th. An unreadable /proc entry (a gone pid, or a non-Linux host)
# returns non-zero so the caller can tell "unknown" from a real value.
proc_pgrp() {
  local stat
  stat="$(cat "/proc/$1/stat" 2>/dev/null)" || return 1
  stat="${stat##*)}"
  set -- $stat
  printf '%s' "${3:-}"
}
proc_starttime() {
  local stat
  stat="$(cat "/proc/$1/stat" 2>/dev/null)" || return 1
  stat="${stat##*)}"
  set -- $stat
  printf '%s' "${20:-}"
}

start_bg() { # <name> <logfile> <command...>
  local name="$1" log="$2" pid start
  shift 2
  setsid "$@" >"$log" 2>&1 &
  # setsid(1) execs the command when it is not already a process-group leader,
  # which a non-interactive shell's background job never is, so `$!` is the new
  # session and process-group leader (F7). Its start time is recorded beside the
  # pid: /proc starttime changes when a pid is reused, which is how `stop` tells
  # a group this demo owns from an unrelated one (F3).
  pid=$!
  start="$(proc_starttime "$pid" 2>/dev/null || true)"
  PGIDS+=("$pid")
  printf '%s %s\n' "$pid" "$start" >>"$RUN/service.pgids"
  echo "    started $name (log: $log)"
}

# The .env files hold values only; drop comments and blank lines and let env
# consume the rest as KEY=VALUE.
env_file() { grep -vE '^[[:space:]]*(#|$)' "$1" | xargs; }

# Per-phase timing: a cold CI runner is far slower than a warm developer
# machine, so every step prints how long it has taken so far.
phase() { printf '==> [%3ss] %s\n' "$((SECONDS - START_S))" "$*"; }
