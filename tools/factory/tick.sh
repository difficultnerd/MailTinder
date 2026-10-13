#!/usr/bin/env bash
# tick.sh - the hourly factory watch as one tested script (T-1123).
#
# One fixed-layout report (the golden copy lives in tools/test_tick.py). Every
# component the report cannot find, cannot read, or that exits non-zero prints
# `!! MISSING <name>` or `!! FAILED <name>` inside its own section; the other
# sections still run and the script exits 1. No section can be skipped
# silently, which is the AAR 3.134 failure: the v1 watch was an English prompt
# that named tools which did not exist where it said, so those checks were
# skipped for hours and nothing said so.
#
# Read-only except the inbox marker, which is written only after a run with no
# problems. No secret is read. The only network calls are the two staging GETs
# and `gh`.
#
# Every external tool and path is a variable at the top, so tools/test_tick.py
# can run the report in a sandbox with fake tools on PATH.

set -uo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"

# --- external tools ---------------------------------------------------------
PYTHON="${TICK_PYTHON:-python3}"
DATE="${TICK_DATE:-date}"
CLAUDE="${TICK_CLAUDE:-claude}"
BALANCE="${TICK_BALANCE:-deepseek-balance}" # prints the DeepSeek balance in USD
DF="${TICK_DF:-df}"
PGREP="${TICK_PGREP:-pgrep}"
HERMES="${TICK_HERMES:-hermes}" # `hermes cron list` prints the cron table
CURL="${TICK_CURL:-curl}"
GH="${TICK_GH:-gh}"
LS="${TICK_LS:-ls}"

# --- external paths ---------------------------------------------------------
BASE="${TICK_BASE:-$HOME/.hermes/profiles/portal-affordable}"
BOX_REPO="${TICK_BOX_REPO:-/home/ubuntu/MailTinder-factory}"
BLOCKERS="${TICK_BLOCKERS:-$SCRIPT_DIR/blockers.py}"
PENDING="${TICK_PENDING:-$BOX_REPO/PENDING.md}"
PR_STATE="${TICK_PR_STATE:-$BASE/scripts/pr_state.py}"
PICKER="${TICK_PICKER:-$BASE/scripts/mailtinder_pick.py}"
INBOX_DIR="${TICK_INBOX:-$BASE/inbox}"
INBOX_MARKER="${TICK_INBOX_MARKER:-$BASE/state/tick-inbox-marker}"

# --- knobs ------------------------------------------------------------------
STAGING="${TICK_STAGING:-https://staging.mailtinder.dev}"
BALANCE_FLOOR="${TICK_BALANCE_FLOOR:-1.00}"
DISK_PATH="${TICK_DISK_PATH:-/}"
HTTP_TIMEOUT="${TICK_HTTP_TIMEOUT:-10}"
CLOUDFLARED_PATTERN="${TICK_CLOUDFLARED_PATTERN:-cloudflared}"
BUILDER_PATTERN="${TICK_BUILDER_PATTERN:-run-task.sh}"
REVIEW_PATTERN="${TICK_REVIEW_PATTERN:-review-task.sh}"
REWORK_PATTERN="${TICK_REWORK_PATTERN:-rework-task.sh}"
OWNER_PATH_PREFIXES="${TICK_OWNER_PATHS:-.github/ infra/terraform/}"

# COUNT: merged is MERGED ONLY; the built-but-unmerged tasks are counted
# separately so the v1 watch's "merged 120/120" (which folded them in) cannot
# come back. The picker module is the same one blockers.py reads.
COUNT_PY='
import importlib.util
import sys

spec = importlib.util.spec_from_file_location("pick", sys.argv[1])
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)
tasks = module.task_index()
merged = set(module.done_set(merged_only=True))
done = set(module.done_set())
sys.stdout.write(
    "merged %d/%d [built-not-merged %d]\n"
    % (len(merged), len(tasks), len(done - merged))
)
'

# --- report primitives ------------------------------------------------------
PROBLEMS=0

problem() { # a `!!` line, at column 0 so it greps like blockers.py's rows
  printf '%s\n' "$1"
  PROBLEMS=$((PROBLEMS + 1))
}
missing() { problem "!! MISSING $1"; }
failed() { problem "!! FAILED $1"; }
section() { printf '== %s ==\n' "$1"; }
detail() { printf '   %-24s %s\n' "$1" "$2"; }
first_line() { printf '%s' "${1%%$'\n'*}"; }
have() { command -v "$1" >/dev/null 2>&1; }

# box_run CMD... - run CMD with the box checkout as the working directory, so
# `gh` and pr_state.py resolve the same repository blockers.py reads (its
# subprocesses run with cwd=REPO). When the checkout is not there the command
# runs where tick.sh was started; the component checks still say so loudly.
box_run() {
  if [ -d "$BOX_REPO" ]; then
    (
      cd -- "$BOX_REPO" || exit 126
      "$@"
    )
  else
    "$@"
  fi
}

# count_processes PATTERN WORD - how many `pgrep -af PATTERN` matches are not
# the pgrep/timeout lines themselves and (when WORD is given) contain WORD.
# Echoes the count; returns 2 when pgrep itself failed (any status but 0/1).
count_processes() {
  local pattern="$1" word="$2" out status line n=0
  out=$("$PGREP" -af "$pattern" 2>/dev/null)
  status=$?
  if [ "$status" -ne 0 ] && [ "$status" -ne 1 ]; then
    return 2
  fi
  while IFS= read -r line; do
    [ -n "$line" ] || continue
    case $line in
      *pgrep* | *timeout*) continue ;;
    esac
    if [ -n "$word" ]; then
      case $line in
        *"$word"*) ;;
        *) continue ;;
      esac
    fi
    n=$((n + 1))
  done <<<"$out"
  printf '%s' "$n"
  return 0
}

# rework_slot COUNT - the report line for the lane's single rework slot.
rework_slot() {
  case $1 in
    0) printf '%s' free ;;
    '?') printf '%s' unknown ;;
    *) printf '%s' busy ;;
  esac
}

# balance_value TEXT / to_cents VALUE - the first number in the balance tool's
# output, and integer cents so bash can compare it with the floor.
balance_value() {
  if [[ $1 =~ ([0-9]+(\.[0-9]+)?) ]]; then
    printf '%s' "${BASH_REMATCH[1]}"
  fi
}

to_cents() {
  local value="$1" whole frac
  whole="${value%%.*}"
  frac="${value#*.}"
  [ "$frac" = "$value" ] && frac=0
  [ -n "$whole" ] || whole=0
  frac="${frac}00"
  printf '%s' "$((10#$whole * 100 + 10#${frac:0:2}))"
}

probe_staging() { # LABEL URL - a 200 detail line, or a loud FAILED
  local label="$1" url="$2" code status
  code=$("$CURL" -s -o /dev/null -w '%{http_code}' --max-time "$HTTP_TIMEOUT" "$url" 2>/dev/null)
  status=$?
  if [ "$status" -ne 0 ]; then
    failed "staging $label"
    return
  fi
  detail "staging $label" "$code"
  if [ "$code" != 200 ]; then
    failed "staging $label"
  fi
}

# --- 1. CLOCK ---------------------------------------------------------------
section CLOCK

if have "$DATE"; then
  out=$("$DATE" -u '+%Y-%m-%d %H:%M:%S UTC' 2>/dev/null)
  status=$?
  if [ "$status" -eq 0 ]; then detail date "$(first_line "$out")"; else failed "$DATE"; fi
else
  missing "$DATE"
fi

if have "$CLAUDE"; then
  out=$("$CLAUDE" --version 2>/dev/null)
  status=$?
  if [ "$status" -eq 0 ]; then detail claude "$(first_line "$out")"; else failed "$CLAUDE"; fi
else
  missing "$CLAUDE"
fi

if have "$BALANCE"; then
  out=$("$BALANCE" 2>/dev/null)
  status=$?
  if [ "$status" -ne 0 ]; then
    failed "$BALANCE"
  else
    value=$(balance_value "$(first_line "$out")")
    if [ -z "$value" ]; then
      failed "$BALANCE"
    else
      detail balance "$value USD (floor $BALANCE_FLOOR)"
      if [ "$(to_cents "$value")" -lt "$(to_cents "$BALANCE_FLOOR")" ]; then
        problem "!! BALANCE LOW $value < $BALANCE_FLOOR"
      fi
    fi
  fi
else
  missing "$BALANCE"
fi

if have "$DF"; then
  out=$("$DF" -P "$DISK_PATH" 2>/dev/null)
  status=$?
  if [ "$status" -ne 0 ]; then
    failed "$DF"
  else
    pct=""
    while read -r fs _blocks _used _avail capacity _mount; do
      [ -n "$fs" ] || continue
      [ "$fs" = "Filesystem" ] && continue
      case $capacity in
        *%) pct="${capacity%\%}" ;;
      esac
    done <<<"$out"
    if [ -n "$pct" ]; then detail disk "${pct}%"; else failed "$DF"; fi
  fi
else
  missing "$DF"
fi

if have "$PGREP"; then
  if n=$(count_processes "$CLOUDFLARED_PATTERN" ""); then
    detail 'cloudflared processes' "$n"
    if [ "$n" -ne 0 ]; then problem '!! CLOUDFLARED RUNNING'; fi
  else
    failed "$PGREP"
  fi
else
  missing "$PGREP"
fi

if have "$HERMES"; then
  out=$("$HERMES" cron list 2>/dev/null)
  status=$?
  if [ "$status" -ne 0 ]; then
    failed "$HERMES"
  else
    paused=0
    while IFS= read -r line; do
      case $line in
        *PAUSED*) paused=$((paused + 1)) ;;
      esac
    done <<<"$out"
    detail 'PAUSED crons' "$paused"
    if [ "$paused" -ne 0 ]; then problem "!! CRON PAUSED $paused"; fi
  fi
else
  missing "$HERMES"
fi

# --- 2. STAGING -------------------------------------------------------------
section STAGING

if ! have "$CURL"; then
  missing "$CURL"
else
  probe_staging healthz "$STAGING/api/v1/healthz"
  probe_staging root "$STAGING/"
fi

# --- 3. PRS -----------------------------------------------------------------
section PRS

if [ ! -r "$PR_STATE" ]; then
  missing pr_state.py
elif ! have "$PYTHON"; then
  missing "$PYTHON"
else
  out=$(box_run "$PYTHON" "$PR_STATE" 2>/dev/null)
  status=$?
  while IFS= read -r line; do
    [ -n "$line" ] && printf '%s\n' "$line"
  done <<<"$out"
  [ "$status" -eq 0 ] || failed pr_state.py
fi

# Owner-only PRs (paths under .github/ or infra/terraform/) must be in
# PENDING.md, or the owner never learns they are waiting on them.
if [ ! -r "$PENDING" ]; then missing PENDING.md; fi
if ! have "$GH"; then missing "$GH"; fi
if [ -r "$PENDING" ] && have "$GH"; then
  pending=$(<"$PENDING")
  out=$(box_run "$GH" pr list --state open --json number,files --jq '.[] | "\(.number) \(.files[].path)"' 2>/dev/null)
  status=$?
  if [ "$status" -ne 0 ]; then
    failed "$GH"
  else
    seen=""
    while IFS= read -r line; do
      [ -n "$line" ] || continue
      number="${line%% *}"
      path="${line#* }"
      owner=0
      for prefix in $OWNER_PATH_PREFIXES; do
        case $path in
          "$prefix"*) owner=1 ;;
        esac
      done
      [ "$owner" -eq 1 ] || continue
      case " $seen " in
        *" #$number "*) continue ;;
      esac
      seen="$seen #$number"
      case $pending in
        *"#$number"*) continue ;;
      esac
      problem "!! NOT IN PENDING #$number"
    done <<<"$out"
  fi
fi

# --- 4. BLOCKERS ------------------------------------------------------------
section BLOCKERS

if [ ! -r "$BLOCKERS" ]; then
  missing blockers.py
elif ! have "$PYTHON"; then
  missing "$PYTHON"
else
  out=$("$PYTHON" "$BLOCKERS" 2>/dev/null)
  status=$?
  [ -n "$out" ] && printf '%s\n' "$out"
  if [ "$status" -eq 1 ]; then
    # Exit 1 is blockers.py's own report of a DEFECT: its `!!` rows are the
    # explanation, so the status is carried without a second synthetic line.
    PROBLEMS=$((PROBLEMS + 1))
  elif [ "$status" -ne 0 ]; then
    failed blockers.py
  fi
fi

# --- 5. LANES ---------------------------------------------------------------
section LANES

if ! have "$PGREP"; then
  missing "$PGREP"
else
  if builders=$(count_processes "$BUILDER_PATTERN" bash); then
    detail builders "$builders"
  else
    failed "$PGREP"
    builders="?"
  fi
  if reviews=$(count_processes "$REVIEW_PATTERN" ""); then
    detail reviews "$reviews"
  else
    failed "$PGREP"
    reviews="?"
  fi
  if reworks=$(count_processes "$REWORK_PATTERN" ""); then
    detail reworks "$reworks"
  else
    failed "$PGREP"
    reworks="?"
  fi
  detail 'rework slot' "$(rework_slot "$reworks")"
fi

# --- 6. INBOX ---------------------------------------------------------------
section INBOX

newest=""
inbox_state="UNKNOWN"

if [ ! -d "$INBOX_DIR" ] || [ ! -r "$INBOX_DIR" ]; then
  missing inbox
elif ! have "$LS"; then
  missing "$LS"
else
  out=$("$LS" -t "$INBOX_DIR" 2>/dev/null)
  status=$?
  if [ "$status" -ne 0 ]; then
    failed "$LS"
  else
    newest=$(first_line "$out")
    if [ -z "$newest" ]; then
      detail newest '(none)'
      problem '!! INBOX EMPTY'
      inbox_state=EMPTY
    else
      marker=""
      if [ -r "$INBOX_MARKER" ]; then marker=$(first_line "$(<"$INBOX_MARKER")"); fi
      if [ "$newest" = "$marker" ]; then inbox_state=UNCHANGED; else inbox_state=NEW; fi
      detail newest "$newest"
      detail state "$inbox_state"
    fi
  fi
fi

# --- 7. COUNT ---------------------------------------------------------------
section COUNT

if [ ! -r "$PICKER" ]; then
  missing mailtinder_pick.py
elif ! have "$PYTHON"; then
  missing "$PYTHON"
else
  out=$("$PYTHON" -B -c "$COUNT_PY" "$PICKER" 2>/dev/null)
  status=$?
  if [ "$status" -ne 0 ]; then
    failed mailtinder_pick.py
  else
    printf '%s\n' "$(first_line "$out")"
  fi
fi

# --- the one write: the inbox marker, only after a clean run ----------------
if [ "$inbox_state" = "NEW" ] && [ "$PROBLEMS" -eq 0 ]; then
  if ! printf '%s\n' "$newest" >"$INBOX_MARKER" 2>/dev/null; then
    failed 'inbox marker'
  fi
fi

printf '%s\n' "-- tick complete: $PROBLEMS problem(s)"
if [ "$PROBLEMS" -ne 0 ]; then
  exit 1
fi
exit 0
