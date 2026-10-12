#!/usr/bin/env bash
#
# T-1118 / ADR 0003 decision 8: typing a secret value is one of the few steps
# that must stay human. This script is the whole of it, and its job is a
# property rather than an action: the VALUE never reaches an argument, an
# environment variable, a log or a file (OWN AC1). It is read hidden from the
# terminal with `read -rs`, piped to `gcloud secrets versions add
# --data-file=-` on standard input, and the variable is unset again.
#
# It never creates a container (OWN AC2). Terraform creates the four containers
# (infra/terraform/README.md step 5), so a typo in the name fails here instead
# of leaving a container that holds a value and has no accessor.
#
#   scripts/infra/set-secret.sh --project <id> --name <secret> [--account <email>]
#
# On success it prints the new version number on stdout and nothing else.
#
# Exit codes: 0 added, 2 refused (bad usage, unknown name, empty value),
# 3 the container does not exist, 1 anything else.

set -euo pipefail

# The four containers Terraform creates (infra/terraform/README.md step 5).
# OWN AC2: any other name is refused; the list is closed on purpose.
readonly -a ALLOWED_NAMES=(
  google-oauth-client-secret
  jev-api-key
  email-lookup-hmac-key
  log-pseudonym-hmac-key
)

usage() {
  printf 'usage: %s --project <id> --name <secret> [--account <email>]\n' "${0##*/}" >&2
  printf 'allowed --name values (the containers Terraform creates):\n' >&2
  printf '  %s\n' "${ALLOWED_NAMES[@]}" >&2
  exit 2
}

project=""
name=""
account=""

while [[ $# -gt 0 ]]; do
  case "$1" in
    --project)
      [[ $# -ge 2 ]] || usage
      project=$2
      shift 2
      ;;
    --name)
      [[ $# -ge 2 ]] || usage
      name=$2
      shift 2
      ;;
    --account)
      [[ $# -ge 2 ]] || usage
      account=$2
      shift 2
      ;;
    *)
      # A value typed as an extra argument is refused, and deliberately not
      # echoed back: printing "$1" here could put a value on the terminal.
      printf 'refusing: unexpected argument; the value is read from the terminal, never passed as an argument\n' >&2
      usage
      ;;
  esac
done

if [[ -z $project || -z $name ]]; then
  usage
fi

allowed=false
for candidate in "${ALLOWED_NAMES[@]}"; do
  if [[ $name == "$candidate" ]]; then
    allowed=true
  fi
done
if [[ $allowed != true ]]; then
  printf 'refusing: %s is not one of the four secret containers Terraform creates\n' "$name" >&2
  printf 'run with no arguments for the list\n' >&2
  exit 2
fi

describe=(secrets describe "$name" --project "$project")
add=(secrets versions add "$name" --project "$project" --data-file=- --format=value\(name\))
if [[ -n $account ]]; then
  describe+=(--account "$account")
  add+=(--account "$account")
fi

# The container must already exist. gcloud's own message goes to stderr and
# names the project and the secret only; no value exists yet at this point.
if ! gcloud "${describe[@]}" >/dev/null; then
  printf 'refusing: no secret container %s in project %s (Terraform creates it; this script never does)\n' "$name" "$project" >&2
  exit 3
fi

# The value. `read -rs` reads one hidden line from standard input: it is never
# an argument and never exported, so it cannot show up in `ps` or in the
# environment of a child process.
printf 'paste the value for %s (input hidden, not echoed): ' "$name" >&2
value=""
IFS= read -rs value || {
  printf '\nrefusing: no value on standard input\n' >&2
  exit 2
}
printf '\n' >&2
if [[ -z $value ]]; then
  unset value
  printf 'refusing: empty value\n' >&2
  exit 2
fi

# `printf` here is the shell builtin, so no process is ever exec'd with the
# value in its argv: the only place it leaves this process is gcloud's stdin.
resource=""
if ! resource=$(printf '%s' "$value" | gcloud "${add[@]}"); then
  unset value
  printf 'adding the version failed; the value was not written anywhere\n' >&2
  exit 1
fi
unset value

version=${resource##*/}
if [[ -z $version ]]; then
  printf 'gcloud returned no version for %s\n' "$name" >&2
  exit 1
fi
printf '%s\n' "$version"
