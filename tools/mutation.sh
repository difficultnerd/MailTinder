#!/usr/bin/env bash
# mutation.sh — run cargo-mutants over the T-1109 mutation-testing pilot scope.
#
#   tools/mutation.sh [--area NAME] [--shard I/N] [--jobs N] [--file PATH]...
#
# Reads the areas and files from tools/mutation_scope.txt, runs cargo-mutants
# on exactly those files (never on test crates, adapters or generated code),
# keeps its artifacts under target/mutants and then runs
# tools/mutation_report.py, which writes target/mutants/summary.json and exits
# non-zero when an area scores below its floor in tools/mutation_baseline.json.
#
# Options:
#   --area NAME    run only one area from the scope file
#   --shard I/N    run shard I of N (0 <= I < N); cargo-mutants writes to
#                  target/mutants/shard-IofN and the report merges the shards
#   --jobs N       how many cargo build/test tasks cargo-mutants runs in parallel
#   --file PATH    restrict to one file (must appear in the scope file); repeatable
#   --scope FILE   use an alternative scope file (default tools/mutation_scope.txt)
#   --list         validate and print the selected files, then exit (no cargo-mutants)
#
# This is deliberately NOT part of tools/ci-local.sh: a full run is far too slow
# for every push. The nightly run on verify1 calls this script (see
# docs/mutation-testing.md). Install the tool first with tools/setup-local-checks.sh.

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SCOPE="$ROOT/tools/mutation_scope.txt"
BASELINE="$ROOT/tools/mutation_baseline.json"
OUT_ROOT="$ROOT/target/mutants"
CARGO_ROOT="$ROOT/backend"

AREA=""
SHARD=""
JOBS=""
LIST_ONLY=0
REQUESTED_FILES=()

usage() {
  sed -n '2,26p' "${BASH_SOURCE[0]}" | sed -e 's/^# \{0,1\}//'
}

trim() {
  local s="$1"
  s="${s#"${s%%[![:space:]]*}"}"
  s="${s%"${s##*[![:space:]]}"}"
  printf '%s' "$s"
}

# scope_paths <scope-file> <area-or-empty> -> "area<TAB>path" lines
scope_paths() {
  local scope="$1" want="${2:-}" line area rest p
  while IFS= read -r line || [ -n "$line" ]; do
    line="${line%%#*}"
    line="$(trim "$line")"
    [ -z "$line" ] && continue
    case "$line" in *:*) ;; *) continue ;; esac
    area="$(trim "${line%%:*}")"
    [ -z "$area" ] && continue
    [ -n "$want" ] && [ "$area" != "$want" ] && continue
    rest="${line#*:}"
    local IFS=','
    for p in $rest; do
      p="$(trim "$p")"
      [ -z "$p" ] && continue
      printf '%s\t%s\n' "$area" "$p"
    done
  done < "$scope"
}

while [ $# -gt 0 ]; do
  case "$1" in
    --area) AREA="${2:-}"; shift 2 ;;
    --area=*) AREA="${1#*=}"; shift ;;
    --shard) SHARD="${2:-}"; shift 2 ;;
    --shard=*) SHARD="${1#*=}"; shift ;;
    --jobs) JOBS="${2:-}"; shift 2 ;;
    --jobs=*) JOBS="${1#*=}"; shift ;;
    --file) REQUESTED_FILES+=("${2:-}"); shift 2 ;;
    --file=*) REQUESTED_FILES+=("${1#*=}"); shift ;;
    --scope) SCOPE="${2:-}"; shift 2 ;;
    --scope=*) SCOPE="${1#*=}"; shift ;;
    --list) LIST_ONLY=1; shift ;;
    -h|--help) usage; exit 0 ;;
    *) echo "error: unknown argument: $1" >&2; usage >&2; exit 2 ;;
  esac
done

if [ ! -f "$SCOPE" ]; then
  echo "error: scope file not found: $SCOPE" >&2
  exit 2
fi

if [ -n "$SHARD" ]; then
  if [[ ! "$SHARD" =~ ^([0-9]+)/([0-9]+)$ ]]; then
    echo "error: --shard must be I/N, got '$SHARD'" >&2
    exit 2
  fi
  shard_index="${BASH_REMATCH[1]}"
  shard_total="${BASH_REMATCH[2]}"
  if [ "$shard_total" -lt 1 ] || [ "$shard_index" -ge "$shard_total" ]; then
    echo "error: --shard I/N needs 0 <= I < N and N >= 1, got '$SHARD'" >&2
    exit 2
  fi
fi

if [ -n "$JOBS" ] && [[ ! "$JOBS" =~ ^[1-9][0-9]*$ ]]; then
  echo "error: --jobs must be a positive integer, got '$JOBS'" >&2
  exit 2
fi

SELECTED="$(scope_paths "$SCOPE" "$AREA")"
if [ -z "$AREA" ]; then
  if [ -z "$SELECTED" ]; then
    echo "error: no areas found in $SCOPE" >&2
    exit 2
  fi
elif [ -z "$SELECTED" ]; then
  echo "error: no such area in mutation_scope.txt: $AREA" >&2
  exit 2
fi

# The pool a --file must belong to: the selected area, or every scope path.
POOL="$SELECTED"
if [ -z "$AREA" ]; then
  POOL="$(scope_paths "$SCOPE" "")"
fi

if [ "${#REQUESTED_FILES[@]}" -gt 0 ]; then
  for f in "${REQUESTED_FILES[@]}"; do
    if ! printf '%s\n' "$POOL" | cut -f2 | grep -qxF -- "$f"; then
      echo "error: path not in mutation_scope.txt: $f" >&2
      exit 2
    fi
  done
fi

FINAL_PATHS=()
if [ "${#REQUESTED_FILES[@]}" -gt 0 ]; then
  FINAL_PATHS=("${REQUESTED_FILES[@]}")
else
  while IFS=$'\t' read -r _area p; do
    [ -n "$p" ] && FINAL_PATHS+=("$p")
  done <<< "$SELECTED"
fi

if [ "${#FINAL_PATHS[@]}" -eq 0 ]; then
  echo "error: nothing selected to mutate" >&2
  exit 2
fi

if [ "$LIST_ONLY" -eq 1 ]; then
  for p in "${FINAL_PATHS[@]}"; do
    printf '%s\n' "$p"
  done
  exit 0
fi

if ! command -v cargo-mutants >/dev/null 2>&1; then
  echo "error: cargo-mutants not found on PATH" >&2
  echo "  install it with: ./tools/setup-local-checks.sh" >&2
  exit 1
fi

if [ -n "$SHARD" ]; then
  OUT="$OUT_ROOT/shard-${shard_index}of${shard_total}"
else
  OUT="$OUT_ROOT"
fi

CARGO_ARGS=(--output "$OUT" --colors never --no-times)
[ -n "$SHARD" ] && CARGO_ARGS+=(--shard "$SHARD")
[ -n "$JOBS" ] && CARGO_ARGS+=(--jobs "$JOBS")
for p in "${FINAL_PATHS[@]}"; do
  CARGO_ARGS+=(-f "${p#backend/}")
done

echo "==> cargo-mutants ${CARGO_ARGS[*]}"
START=$SECONDS
cargo_rc=0
( cd "$CARGO_ROOT" && cargo-mutants "${CARGO_ARGS[@]}" ) || cargo_rc=$?
ELAPSED=$((SECONDS - START))
echo "==> cargo-mutants finished in ${ELAPSED}s (exit $cargo_rc)"

if ! find "$OUT_ROOT" -name outcomes.json -type f | grep -q .; then
  echo "error: cargo-mutants produced no outcomes.json under $OUT_ROOT" >&2
  exit "${cargo_rc:-1}"
fi

report_rc=0
python3 "$ROOT/tools/mutation_report.py" \
  --input "$OUT_ROOT" \
  --scope "$SCOPE" \
  --baseline "$BASELINE" \
  --json-out "$OUT_ROOT/summary.json" || report_rc=$?

exit "$report_rc"
