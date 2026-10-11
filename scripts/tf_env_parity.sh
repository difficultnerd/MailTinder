#!/bin/bash
# Staging and production are built from the same Terraform modules (T-1103,
# decision T5). The two roots may differ only in the *values* they pass to a
# module; they may never call a different module, and neither may fork one.
#
# This compares the sorted `source = "..."` lines of every .tf file in
# infra/terraform/envs/prod with the same for infra/terraform/envs/staging and
# fails on any difference, so a staging-only fork of a module (or a root that
# stopped calling one) fails the `terraform` CI job.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ENVS="$REPO_ROOT/infra/terraform/envs"

status=0

# An empty or missing root must fail rather than diff two empty lists.
for env in prod staging; do
  if ! compgen -G "$ENVS/$env/*.tf" >/dev/null; then
    echo "FAIL no Terraform files in infra/terraform/envs/$env"
    status=1
  fi
done
[ "$status" -eq 0 ] || exit 1

# The sorted module sources of a root, with trailing comments and whitespace
# stripped so formatting is not a difference.
module_sources() {
  grep -hE '^[[:space:]]*source[[:space:]]*=' "$ENVS/$1"/*.tf |
    sed -E 's/[[:space:]]*(#|\/\/).*$//; s/[[:space:]]+$//' |
    sort
}

prod_sources="$(module_sources prod)"
staging_sources="$(module_sources staging)"

if [ -z "$prod_sources" ]; then
  echo "FAIL infra/terraform/envs/prod calls no module (no 'source =' line found)"
  exit 1
fi

if ! diff_output="$(diff <(printf '%s\n' "$prod_sources") <(printf '%s\n' "$staging_sources"))"; then
  echo "FAIL staging and production do not call the same modules (T-1103)."
  echo "Only module *variables* may differ between the roots; pass a variable"
  echo "instead of forking a module. diff prod -> staging:"
  printf '%s\n' "$diff_output"
  exit 1
fi

echo "OK   staging and production call the same modules ($(printf '%s\n' "$prod_sources" | wc -l | tr -d ' ') module source(s))"
