#!/usr/bin/env bash
#
# T-1118: tests for scripts/infra/set-secret.sh.
#
# The script's job is a property, not an action: the secret VALUE never reaches
# an argument, an environment variable, an output stream or a file (OWN AC1),
# and it never creates or writes a container whose name Terraform did not
# create (OWN AC2). So these tests put a `gcloud` stub first on PATH that
# records every argv and every byte of its stdin, then assert where the value
# did and did not appear.
#
# Run: bash scripts/infra/tests/set_secret_test.sh
#
# No network, no credentials, no real secret: the value below is a canary
# string, and the stub is the only `gcloud` any of this ever executes.

set -euo pipefail

HERE=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
readonly HERE
readonly SCRIPT="${HERE}/../set-secret.sh"

# A canary, not a secret: it is only ever compared, never used against Google.
readonly VALUE='canary-2f1c9d-8b74-not-a-real-secret'

status=0

fail() {
  printf '  %s\n' "$*" >&2
}

sandbox() {
  mktemp -d "${TMPDIR:-/tmp}/set-secret-test.XXXXXX"
}

# A `gcloud` that records what it was given: every argv on one line in
# $GCLOUD_ARGV_LOG, and the stdin of a `versions add` verbatim in
# $GCLOUD_STDIN_LOG. $GCLOUD_DESCRIBE_STATUS decides whether the container
# "exists"; a `versions add` prints a version resource name, the shape the real
# command returns with --format=value(name).
install_stub() {
  local dir=$1
  cat >"${dir}/gcloud" <<'STUB'
#!/usr/bin/env bash
printf '%s\n' "$*" >>"${GCLOUD_ARGV_LOG}"
if [[ $* == *"versions add"* ]]; then
  cat >"${GCLOUD_STDIN_LOG}"
  printf 'projects/stub-project/secrets/stub/versions/7\n'
  exit 0
fi
exit "${GCLOUD_DESCRIBE_STATUS:-0}"
STUB
  chmod +x "${dir}/gcloud"
}

# Sets up a sandbox and exports the stub's environment for one test.
prepare() {
  local box=$1
  export GCLOUD_ARGV_LOG="${box}/argv.log"
  export GCLOUD_STDIN_LOG="${box}/stdin.log"
  export GCLOUD_DESCRIBE_STATUS=0
  install_stub "$box"
  : >"${GCLOUD_ARGV_LOG}"
  : >"${GCLOUD_STDIN_LOG}"
}

# OWN AC1: the value appears only on gcloud's stdin, and in no argument, in no
# output and in no file.
own_ac1_value_never_in_argv_or_output() {
  local box=$1
  prepare "$box"
  local out rc
  if out=$(printf '%s\n' "$VALUE" | PATH="${box}:${PATH}" "$SCRIPT" \
      --project demo-project --name jev-api-key 2>"${box}/stderr.log"); then
    rc=0
  else
    rc=$?
  fi
  if [[ $rc -ne 0 ]]; then
    fail "own_ac1: a valid run was refused (exit ${rc})"
    cat "${box}/stderr.log" >&2
    return 1
  fi

  # It prints the version number, not the value and not the resource name.
  if [[ $out != "7" ]]; then
    fail "own_ac1: expected only the version number 7, got '${out}'"
    return 1
  fi

  # It reached gcloud, verbatim, on stdin...
  if [[ $(cat "${GCLOUD_STDIN_LOG}") != "$VALUE" ]]; then
    fail "own_ac1: the value did not reach gcloud on stdin"
    return 1
  fi
  if ! grep -qF -- 'versions add' "${GCLOUD_ARGV_LOG}"; then
    fail "own_ac1: gcloud never saw the add call"
    return 1
  fi

  # ...and nowhere else.
  if grep -qF -- "$VALUE" "${GCLOUD_ARGV_LOG}"; then
    fail "own_ac1: the value appears in gcloud's argv"
    return 1
  fi
  if grep -qF -- "$VALUE" "${box}/stderr.log"; then
    fail "own_ac1: the value appears on stderr"
    return 1
  fi
  if [[ $out == *"$VALUE"* ]]; then
    fail "own_ac1: the value appears on stdout"
    return 1
  fi

  local leaks
  leaks=$(grep -rlF -- "$VALUE" "$box" 2>/dev/null || true)
  leaks=$(printf '%s\n' "$leaks" | grep -v -x -F -- "$GCLOUD_STDIN_LOG" || true)
  if [[ -n $leaks ]]; then
    fail "own_ac1: the value was written to a file: ${leaks}"
    return 1
  fi
}

# OWN AC1, the traps: the value is not accepted as a positional argument and
# not read from the environment.
own_ac1_value_is_never_an_argument() {
  local box=$1
  prepare "$box"
  local out rc
  if out=$(printf '\n' | PATH="${box}:${PATH}" "$SCRIPT" \
      --project demo-project --name jev-api-key "$VALUE" 2>"${box}/stderr.log"); then
    rc=0
  else
    rc=$?
  fi
  if [[ $rc -eq 0 ]]; then
    fail "own_ac1: a value passed as an argument was accepted"
    return 1
  fi
  if [[ -s ${GCLOUD_ARGV_LOG} ]]; then
    fail "own_ac1: gcloud was called for a refused invocation"
    return 1
  fi
  if grep -qF -- "$VALUE" "${box}/stderr.log" || [[ $out == *"$VALUE"* ]]; then
    fail "own_ac1: the refused argument was echoed back into the output"
    return 1
  fi

  # An environment variable is not an input either: the value comes from stdin,
  # and only from stdin.
  prepare "$box"
  local from_terminal='stdin-only-93aa41'
  if ! out=$(printf '%s\n' "$from_terminal" | env "SECRET_VALUE=${VALUE}" \
      PATH="${box}:${PATH}" "$SCRIPT" --project demo-project --name jev-api-key \
      2>"${box}/stderr.log"); then
    fail "own_ac1: the run with SECRET_VALUE set was refused"
    return 1
  fi
  if [[ $(cat "${GCLOUD_STDIN_LOG}") != "$from_terminal" ]]; then
    fail "own_ac1: the environment variable was used instead of stdin"
    return 1
  fi
  if grep -qF -- 'canary-2f1c9d' "${GCLOUD_ARGV_LOG}"; then
    fail "own_ac1: the environment variable reached gcloud's argv"
    return 1
  fi
}

# OWN AC2: a name that is not one of the four is refused, and so is a name
# whose container does not exist; neither case reads or forwards a value.
own_ac2_refuses_unknown_or_missing_secret() {
  local box=$1
  local out rc

  # Not on the allowed list.
  prepare "$box"
  if out=$(printf '%s\n' "$VALUE" | PATH="${box}:${PATH}" "$SCRIPT" \
      --project demo-project --name not-one-of-the-four 2>"${box}/stderr.log"); then
    rc=0
  else
    rc=$?
  fi
  if [[ $rc -eq 0 ]]; then
    fail "own_ac2: an unknown secret name was accepted"
    return 1
  fi
  if [[ $rc -ne 2 ]]; then
    fail "own_ac2: an unknown secret name exited ${rc}, expected 2"
    return 1
  fi
  if [[ -s ${GCLOUD_ARGV_LOG} ]]; then
    fail "own_ac2: gcloud was called for an unknown secret name"
    return 1
  fi
  if ! grep -qF -- 'not-one-of-the-four' "${box}/stderr.log"; then
    fail "own_ac2: the refusal does not name the secret"
    return 1
  fi
  if grep -qF -- "$VALUE" "${box}/stderr.log" || [[ $out == *"$VALUE"* ]]; then
    fail "own_ac2: the refusal echoed the value"
    return 1
  fi

  # Allowed name, container missing (Terraform has not created it yet).
  prepare "$box"
  export GCLOUD_DESCRIBE_STATUS=1
  if out=$(printf '%s\n' "$VALUE" | PATH="${box}:${PATH}" "$SCRIPT" \
      --project demo-project --name google-oauth-client-secret \
      2>"${box}/stderr.log"); then
    rc=0
  else
    rc=$?
  fi
  if [[ $rc -ne 3 ]]; then
    fail "own_ac2: a missing container exited ${rc}, expected 3"
    return 1
  fi
  if ! grep -qF -- 'secrets describe' "${GCLOUD_ARGV_LOG}"; then
    fail "own_ac2: the container existence was never checked"
    return 1
  fi
  if grep -qF -- 'versions add' "${GCLOUD_ARGV_LOG}"; then
    fail "own_ac2: a version was added although the container does not exist"
    return 1
  fi
  if [[ -s ${GCLOUD_STDIN_LOG} ]]; then
    fail "own_ac2: a value was handed to gcloud although nothing was added"
    return 1
  fi
  if ! grep -qF -- 'google-oauth-client-secret' "${box}/stderr.log"; then
    fail "own_ac2: the refusal does not name the secret"
    return 1
  fi
  unset GCLOUD_DESCRIBE_STATUS
}

run_test() {
  local name=$1
  local box
  box=$(sandbox)
  if ( "$name" "$box" ); then
    printf 'ok - %s\n' "$name"
  else
    printf 'not ok - %s\n' "$name"
    status=1
  fi
  rm -rf "$box"
}

main() {
  if [[ ! -f $SCRIPT ]]; then
    printf 'not ok: %s is missing\n' "$SCRIPT" >&2
    return 1
  fi
  run_test own_ac1_value_never_in_argv_or_output
  run_test own_ac1_value_is_never_an_argument
  run_test own_ac2_refuses_unknown_or_missing_secret
  if [[ $status -ne 0 ]]; then
    printf 'FAILED\n' >&2
    return 1
  fi
  printf 'all tests passed\n'
}

main "$@"
