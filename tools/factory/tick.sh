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
# Every external component gets a deadline, not just the staging probes: one
# component that never returns (a balance call, a `gh` waiting on the network,
# a helper hanging in an import) must not stop the sections after it or the
# failure report itself.
TIMEOUT_CMD="${TICK_TIMEOUT_CMD:-timeout}" # coreutils timeout, the deadline runner
CMD_TIMEOUT="${TICK_CMD_TIMEOUT:-20}"      # seconds any one component may take
# `gh pr list` asks for this many open PRs; reaching it is reported as
# incomplete coverage rather than mistaken for a clean PRS section.
PR_LIMIT="${TICK_PR_LIMIT:-200}"
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

# run_deadline SECS CMD... - run CMD with a hard deadline; 124 when it fires
# (the GNU coreutils `timeout` convention). Every external command below goes
# through here, so a stalled component is reported and the other sections still
# run. GNU timeout signals the command's whole process group, so a helper's own
# children (blockers.py's `git`/`gh`, the picker's imports) are inside the
# deadline too. When `timeout` is absent the command still runs, and the report
# says so once, loudly, before the first section.
run_deadline() {
  local secs="$1"
  shift
  if have "$TIMEOUT_CMD"; then
    "$TIMEOUT_CMD" "$secs" "$@"
  else
    "$@"
  fi
}

# pending_has_pr PENDING_TEXT NUMBER - true only for that exact numeric PR
# reference: "#9" must not be satisfied by "#90" or "#9a". A decimal-prefix
# collision would hide an owner-only PR the owner was never told about.
# TEXT ends in a newline, so the trailing boundary always has a character.
pending_has_pr() {
  [[ $1 =~ (^|[^0-9])#${2}([^0-9]|$) ]]
}

# exe_name TOKEN - the name of an executable token: its basename, so a process
# invoked as /opt/timeout/bin/cloudflared is named `cloudflared`.
exe_name() { printf '%s' "${1##*/}"; }

# is_probe PATTERN LINE - true when a `pgrep -af PATTERN` line is this probe's
# own instrumentation rather than the process asked for. Judged by process
# identity, never by words that merely appear in a path or an argument: a
# tunnel at /opt/timeout/bin/cloudflared, or one carrying `--log /tmp/pgrep.log`
# in its argv, is still a tunnel and must still be counted.
is_probe() {
  local pattern="$1" line="$2" cmd exe child
  cmd="${line#* }" # drop the PID column
  [ "$cmd" = "$line" ] && cmd="$line"
  exe="$(exe_name "${cmd%%[[:space:]]*}")"
  case $exe in
    pgrep | pgrep.exe | pkill | pkill.exe) return 0 ;;
    timeout | gtimeout | timeout.exe)
      # `timeout DURATION CMD ARGS...`: the wrapper itself is not the process
      # being counted. Unwrap it and judge CMD, so a tunnel running under
      # `timeout` is still counted while the wrapper of a separately listed
      # child is not.
      child="${cmd#*[[:space:]]}"   # drop `timeout`
      child="${child#*[[:space:]]}" # drop DURATION
      child="$(exe_name "${child%%[[:space:]]*}")"
      case $child in
        pgrep | pgrep.exe | pkill | pkill.exe) return 0 ;;
      esac
      # The wrapper of a process we are counting is not that process ... unless
      # the wrapped command *is* the process the pattern names (a tunnel under
      # `timeout`), which must still be counted.
      [ "$child" = "$pattern" ] && return 1
      return 0
      ;;
  esac
  return 1
}

# count_processes PATTERN WORD - how many `pgrep -af PATTERN` matches are not
# this probe's own process (see is_probe) and (when WORD is given) contain WORD.
# Echoes the count; returns 2 when pgrep itself failed or timed out.
count_processes() {
  local pattern="$1" word="$2" out status line n=0
  out=$(run_deadline "$CMD_TIMEOUT" "$PGREP" -af "$pattern" 2>/dev/null)
  status=$?
  if [ "$status" -ne 0 ] && [ "$status" -ne 1 ]; then
    return 2
  fi
  while IFS= read -r line; do
    [ -n "$line" ] || continue
    is_probe "$pattern" "$line" && continue
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
  code=$(run_deadline "$CMD_TIMEOUT" "$CURL" -s -o /dev/null -w '%{http_code}' --max-time "$HTTP_TIMEOUT" "$url" 2>/dev/null)
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
# A missing deadline runner is a lost capability for every section, not one
# section's component, so it is reported once, before the report proper.
have "$TIMEOUT_CMD" || problem "!! MISSING $TIMEOUT_CMD (component deadlines disabled)"

section CLOCK

if have "$DATE"; then
  out=$(run_deadline "$CMD_TIMEOUT" "$DATE" -u '+%Y-%m-%d %H:%M:%S UTC' 2>/dev/null)
  status=$?
  if [ "$status" -eq 0 ]; then detail date "$(first_line "$out")"; else failed "$DATE"; fi
else
  missing "$DATE"
fi

if have "$CLAUDE"; then
  out=$(run_deadline "$CMD_TIMEOUT" "$CLAUDE" --version 2>/dev/null)
  status=$?
  if [ "$status" -eq 0 ]; then detail claude "$(first_line "$out")"; else failed "$CLAUDE"; fi
else
  missing "$CLAUDE"
fi

if have "$BALANCE"; then
  out=$(run_deadline "$CMD_TIMEOUT" "$BALANCE" 2>/dev/null)
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
  out=$(run_deadline "$CMD_TIMEOUT" "$DF" -P "$DISK_PATH" 2>/dev/null)
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
  out=$(run_deadline "$CMD_TIMEOUT" "$HERMES" cron list 2>/dev/null)
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
  out=$(box_run run_deadline "$CMD_TIMEOUT" "$PYTHON" "$PR_STATE" 2>/dev/null)
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
  # The trailing newline gives the last line's PR number a boundary too.
  pending="$(<"$PENDING")"$'\n'
  # --limit is explicit: the CLI's own window is 30 PRs, and an owner-only PR
  # past that window would be invisible to this check (an owner who was never
  # told they were being waited on).
  out=$(box_run run_deadline "$CMD_TIMEOUT" "$GH" pr list --state open --limit "$PR_LIMIT" \
    --json number,files --jq '.[] | "\(.number) \(.files[].path)"' 2>/dev/null)
  status=$?
  if [ "$status" -ne 0 ]; then
    failed "$GH"
  else
    seen=""
    listed=""
    listed_count=0
    while IFS= read -r line; do
      [ -n "$line" ] || continue
      number="${line%% *}"
      path="${line#* }"
      case $number in
        '' | *[!0-9]*) continue ;;
      esac
      # How many open PRs the enumeration actually reached, for the cap check.
      case " $listed " in
        *" $number "*) ;;
        *)
          listed="$listed $number"
          listed_count=$((listed_count + 1))
          ;;
      esac
      owner=0
      for prefix in $OWNER_PATH_PREFIXES; do
        case $path in
          "$prefix"*) owner=1 ;;
        esac
      done
      [ "$owner" -eq 1 ] || continue
      case " $seen " in
        *" $number "*) continue ;;
      esac
      seen="$seen $number"
      pending_has_pr "$pending" "$number" && continue
      problem "!! NOT IN PENDING #$number"
    done <<<"$out"
    # Enumeration that stopped at the cap cannot say the section is clean.
    if [ "$listed_count" -ge "$PR_LIMIT" ]; then
      problem "!! PRS INCOMPLETE $listed_count open PR(s) at the --limit $PR_LIMIT cap"
    fi
  fi
fi

# --- 4. BLOCKERS ------------------------------------------------------------
section BLOCKERS

if [ ! -r "$BLOCKERS" ]; then
  missing blockers.py
elif ! have "$PYTHON"; then
  missing "$PYTHON"
else
  out=$(run_deadline "$CMD_TIMEOUT" "$PYTHON" "$BLOCKERS" 2>/dev/null)
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
  out=$(run_deadline "$CMD_TIMEOUT" "$LS" -t "$INBOX_DIR" 2>/dev/null)
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
  out=$(run_deadline "$CMD_TIMEOUT" "$PYTHON" -B -c "$COUNT_PY" "$PICKER" 2>/dev/null)
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
