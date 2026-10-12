#!/usr/bin/env bash
# T-1116 tests for scripts/infra/bootstrap-project.sh.
#
# A `gcloud` stub sits first on PATH. It records every argument of every call
# (one argument per line, calls separated by a `@@CALL` marker) and models
# whether a resource already exists, so BOOT AC1-AC6 are asserted from the calls
# the script really made - not from its source text - and a second run is
# exercised against a project where everything already exists.
#
#   boot_ac1_dry_run_creates_nothing
#   boot_ac2_second_run_is_noop
#   boot_ac3_refuses_unsafe_contexts
#   boot_ac4_no_key_creation_call
#   boot_ac5_planner_cannot_reach_applier
#   boot_ac6_iam_admin_grant_is_conditioned
#
# plus shellcheck (skipped, loudly, when the tool is not installed).
set -uo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
SCRIPT="${REPO_ROOT}/scripts/infra/bootstrap-project.sh"
MODULES="${REPO_ROOT}/infra/terraform/modules"

SLUG="example-owner/example-repo"
PROJECT_ID="example-project-staging"
PROJECT_NUMBER="987654321098"
PLANNER_EMAIL="mt-planner@${PROJECT_ID}.iam.gserviceaccount.com"
APPLIER_EMAIL="mt-applier@${PROJECT_ID}.iam.gserviceaccount.com"

FAILED=0
TESTS=(
  boot_ac1_dry_run_creates_nothing
  boot_ac2_second_run_is_noop
  boot_ac3_refuses_unsafe_contexts
  boot_ac4_no_key_creation_call
  boot_ac5_planner_cannot_reach_applier
  boot_ac6_iam_admin_grant_is_conditioned
)

# ---------------------------------------------------------------- assertions

fail() {
  FAILED=$((FAILED + 1))
  printf '  FAIL: %s\n' "$*" >&2
}

ok() {
  printf '  ok: %s\n' "$*"
}

expect_status() { # <expected> <actual> <label>
  if [ "$2" -ne "$1" ]; then
    fail "$3: expected exit $1, got $2"
  else
    ok "$3"
  fi
}

expect_nonzero() { # <actual> <label>
  if [ "$1" -eq 0 ]; then
    fail "$2: expected a non-zero exit, got 0"
  else
    ok "$2"
  fi
}

expect_contains() { # <file> <text> <label>
  if grep -Fq -- "$2" "$1"; then
    ok "$3"
  else
    fail "$3: '$2' not found in $1"
  fi
}

expect_regex() { # <file> <pattern> <label>
  if grep -Eq -- "$2" "$1"; then
    ok "$3"
  else
    fail "$3: /$2/ does not match in $1"
  fi
}

expect_not_contains() { # <file> <text> <label>
  if grep -Fq -- "$2" "$1"; then
    fail "$3: '$2' must not appear in $1"
  else
    ok "$3"
  fi
}

expect_log_line() { # <exact argument line> <label>  (the call log is line-based)
  if grep -Fxq -- "$1" "$LOG"; then
    ok "$2"
  else
    fail "$2: no call passed the argument '$1'"
  fi
}

# Any argument that creates or changes something.
mutating_lines() { # <file>
  grep -xE -- 'create|create-oidc|update|enable|add-iam-policy-binding|delete|set-iam-policy|keys' "$1" || true
}

# ---------------------------------------------------------------- the stub

write_stub() {
  mkdir -p "${STUB_DIR}"
  cat >"${STUB_DIR}/gcloud" <<'STUB'
#!/usr/bin/env bash
# A stand-in for gcloud. Every argument of every call is appended to STUB_LOG,
# one per line, with a `@@CALL` marker before each call; STUB_ALL_LOG (optional)
# collects the same for the whole test run. STUB_STATE holds one marker file per
# resource that exists, so a second bootstrap run sees everything as present.
set -u
log="${STUB_LOG:?STUB_LOG must name the call log}"
state="${STUB_STATE:?STUB_STATE must name the state directory}"
project_default="${STUB_PROJECT:-example-project}"

printf '@@CALL\n' >>"$log"
printf '%s\n' "$@" >>"$log"
if [ -n "${STUB_ALL_LOG:-}" ] && [ "$STUB_ALL_LOG" != "$log" ]; then
  printf '@@CALL\n' >>"$STUB_ALL_LOG"
  printf '%s\n' "$@" >>"$STUB_ALL_LOG"
fi

key_for() {
  printf '%s/%s' "$state" "$(printf '%s' "$1" | tr -c 'A-Za-z0-9._-' '_')"
}
exists() { [ -f "$(key_for "$1")" ]; }
mark() { : >"$(key_for "$1")"; }

flag() { # <--flag> <args...> -> the value of --flag=value or --flag value
  local name="$1"
  shift
  local prev="" arg
  for arg in "$@"; do
    case "$arg" in
      "$name"=*) printf '%s' "${arg#"$name"=}"; return 0 ;;
      "$name") printf '%s' "$prev"; return 0 ;;
    esac
    prev="$arg"
  done
  printf '%s' ""
}

project="$(flag --project "$@")"
[ -n "$project" ] || project="$project_default"

case "${1:-}" in
  auth)
    if [ "${2:-}" = "list" ]; then
      printf '%s\n' "${STUB_ACCOUNT:-owner@example.com}"
    fi
    ;;
  billing)
    if [ "${2:-}" = "projects" ]; then
      printf '%s\n' "${STUB_BILLING:-True}"
    fi
    ;;
  projects)
    case "${2:-}" in
      describe)
        if [ "${STUB_PROJECT_MISSING:-0}" = "1" ]; then
          printf 'ERROR: (gcloud.projects.describe) Project [%s] not found\n' "$project" >&2
          exit 1
        fi
        case "$*" in
          *projectNumber*) printf '%s\n' "${STUB_PROJECT_NUMBER:-987654321098}" ;;
          *) printf 'projectId: %s\n' "$project" ;;
        esac
        ;;
      add-iam-policy-binding) printf 'bindings:\n' ;;
      get-iam-policy) printf '{"bindings": []}\n' ;;
    esac
    ;;
  services)
    if [ "${2:-}" = "enable" ]; then
      printf 'Operation finished successfully.\n'
    fi
    ;;
  storage)
    # gcloud storage buckets <verb> gs://<bucket>
    verb="${3:-}"
    bucket="${4:-}"
    case "$verb" in
      describe)
        if exists "bucket_$bucket"; then
          printf 'name: %s\n' "$bucket"
        else
          printf 'ERROR: not found\n' >&2
          exit 1
        fi
        ;;
      create) mark "bucket_$bucket" ;;
      update) : ;;
      add-iam-policy-binding) printf 'bindings:\n' ;;
    esac
    ;;
  iam)
    case "${2:-}" in
      service-accounts)
        verb="${3:-}"
        name="${4:-}"
        case "$verb" in
          describe)
            if exists "sa_$name"; then
              printf 'email: %s\n' "$name"
            else
              printf 'ERROR: not found\n' >&2
              exit 1
            fi
            ;;
          create) mark "sa_${name}@${project}.iam.gserviceaccount.com" ;;
          add-iam-policy-binding) printf 'bindings:\n' ;;
        esac
        ;;
      workload-identity-pools)
        case "${3:-}" in
          create) mark "pool_${project}_${4:-}" ;;
          describe)
            if ! exists "pool_${project}_${4:-}"; then
              printf 'ERROR: not found\n' >&2
              exit 1
            fi
            ;;
          providers)
            pool="$(flag --workload-identity-pool "$@")"
            case "${4:-}" in
              create-oidc) mark "provider_${project}_${pool}_${5:-}" ;;
              describe)
                if ! exists "provider_${project}_${pool}_${5:-}"; then
                  printf 'ERROR: not found\n' >&2
                  exit 1
                fi
                ;;
            esac
            ;;
        esac
        ;;
    esac
    ;;
esac
exit 0
STUB
  chmod +x "${STUB_DIR}/gcloud"
}

# ------------------------------------------------------------- test harness

setup_case() {
  CASE_DIR="$(mktemp -d "${TEST_TMP}/case.XXXXXX")"
  STATE="${CASE_DIR}/state"
  LOG="${CASE_DIR}/calls.log"
  OUT="${CASE_DIR}/out"
  ERR="${CASE_DIR}/err"
  mkdir -p "$STATE"
  : >"$LOG"
  ACCOUNT="owner@example.com"
  BILLING="True"
  PROJECT_MISSING="0"
  CASE_ENV="staging"
  CASE_INPUT="/dev/null"
}

bootstrap() { # [extra args...]
  PATH="${STUB_DIR}:${PATH}" \
    STUB_LOG="$LOG" \
    STUB_ALL_LOG="$ALL_LOG" \
    STUB_STATE="$STATE" \
    STUB_PROJECT="$PROJECT_ID" \
    STUB_ACCOUNT="$ACCOUNT" \
    STUB_BILLING="$BILLING" \
    STUB_PROJECT_MISSING="$PROJECT_MISSING" \
    "$SCRIPT" --project "$PROJECT_ID" --env "$CASE_ENV" --repo "$SLUG" "$@" \
    >"$OUT" 2>"$ERR" <"$CASE_INPUT"
  return $?
}

# The roles the script's own tables declare: the plan must list exactly these.
spec_roles_from_script() {
  awk '
    index($0, "planner_role_specs() {") == 1 { inside = 1; next }
    index($0, "applier_role_specs() {") == 1 { inside = 1; next }
    inside && /^}/ { inside = 0 }
    inside
  ' "$SCRIPT" | sed -n 's/^\(roles\/[^|]*\)|.*$/\1/p' | sort -u
}

# The roles the modules actually bind: the ground truth for BOOT AC6.
module_roles_from_terraform() {
  sed 's/#.*//' "${MODULES}"/foundation/*.tf "${MODULES}"/runtime/*.tf |
    grep -ohE 'roles/[a-zA-Z0-9._-]+' | sort -u
}

# The roles inside a condition's hasOnly([...]) list.
condition_list_roles() { # <condition argument line>
  printf '%s\n' "$1" | sed -n 's/.*hasOnly(\[\([^]]*\)\]).*/\1/p' | tr ',' '\n' |
    sed -n 's/^[[:space:]]*"\(roles\/[^"]*\)"[[:space:]]*$/\1/p'
}

# Every call that carries a condition must be a projectIamAdmin grant, and every
# projectIamAdmin grant must carry one - no unconditioned IAM-admin grant is
# allowed to exist anywhere in the log.
conditioned_calls_are_iam_admin_grants() {
  awk '
    function flush() {
      if (has_condition && !has_admin) { bad++ }
      if (has_admin && !has_condition) { bad++ }
      has_condition = 0
      has_admin = 0
    }
    /^@@CALL$/ { flush(); next }
    index($0, "--condition=expression=") == 1 { has_condition = 1 }
    $0 == "--role=roles/resourcemanager.projectIamAdmin" { has_admin = 1 }
    END { flush(); exit (bad > 0 ? 1 : 0) }
  ' "$LOG"
}

# ------------------------------------------------------------------- tests

boot_ac1_dry_run_creates_nothing() {
  setup_case
  bootstrap --dry-run
  expect_status 0 "$?" "dry run exits 0"

  local changed
  changed="$(mutating_lines "$LOG")"
  if [ -n "$changed" ]; then
    fail "dry run issued a mutating call: $(printf '%s' "$changed" | tr '\n' ' ')"
  else
    ok "dry run issued no creating, updating, enabling or granting call"
  fi

  local token
  for token in \
    "gs://${PROJECT_ID}-tfstate" \
    "workload identity pool github-plan with provider github-plan" \
    "workload identity pool github-apply with provider github-apply" \
    "service account mt-planner" \
    "service account mt-applier" \
    "roles/storage.objectViewer" \
    "roles/storage.objectAdmin" \
    "roles/resourcemanager.projectIamAdmin" \
    "never: service account keys"; do
    expect_contains "$OUT" "$token" "the plan names '$token'"
  done

  local api
  for api in serviceusage.googleapis.com cloudresourcemanager.googleapis.com \
    iam.googleapis.com iamcredentials.googleapis.com sts.googleapis.com \
    storage.googleapis.com; do
    expect_contains "$OUT" "$api" "the plan names the $api bootstrap API"
  done

  local token
  for token in plan apply; do
    expect_regex "$OUT" "^  ${token} provider condition .*${SLUG}.*refs/heads/main" \
      "the plan prints the ${token} provider condition"
  done

  # Every role of both tables is listed, and the plan lists no other role.
  local plan_roles spec_roles
  plan_roles="$(grep -E '^  (planner|applier) role ' "$OUT" | awk '{print $3}' | sort -u)"
  spec_roles="$(spec_roles_from_script)"
  if [ -n "$spec_roles" ] && [ "$plan_roles" = "$spec_roles" ]; then
    ok "the dry run lists every role of both tables"
  else
    fail "the dry run's role list differs from the tables:
$(printf '%s\n' "$plan_roles" | sed 's/^/    plan: /')
$(printf '%s\n' "$spec_roles" | sed 's/^/   table: /')"
  fi

  # The planner's viewers named by the task itself are really there.
  local planner_role
  for planner_role in roles/cloudkms.viewer roles/datastore.viewer roles/run.viewer \
    roles/secretmanager.viewer roles/iam.securityReviewer \
    roles/serviceusage.serviceUsageViewer roles/artifactregistry.reader \
    roles/logging.viewer roles/cloudtasks.viewer roles/cloudscheduler.viewer; do
    expect_contains "$OUT" "$planner_role" "the plan lists $planner_role for the planner"
  done
}

boot_ac2_second_run_is_noop() {
  setup_case
  printf 'y\n' >"${CASE_DIR}/input"
  CASE_INPUT="${CASE_DIR}/input"
  bootstrap
  expect_status 0 "$?" "the first run exits 0 after 'y'"
  expect_contains "$OUT" "created state bucket" "the first run creates the state bucket"

  local mark run1_mutatives run2_slice run2_mutatives extra
  mark="$(wc -l <"$LOG")"
  run1_mutatives="$(mutating_lines "$LOG" | sort -u)"

  CASE_INPUT="/dev/null"
  bootstrap --yes
  expect_status 0 "$?" "the second run exits 0"

  run2_slice="${CASE_DIR}/run2.log"
  tail -n "+$((mark + 1))" "$LOG" >"$run2_slice"
  if mutating_lines "$run2_slice" | grep -Fxq -e create -e create-oidc; then
    fail "the second run created something"
  else
    ok "the second run creates nothing"
  fi

  run2_mutatives="$(mutating_lines "$run2_slice" | sort -u)"
  extra="$(comm -13 <(printf '%s\n' "$run1_mutatives") <(printf '%s\n' "$run2_mutatives"))"
  if [ -z "$extra" ]; then
    ok "the second run introduces no change the first run had not already made"
  else
    fail "the second run issued a new call: $(printf '%s' "$extra" | tr '\n' ' ')"
  fi

  local token
  for token in \
    "state bucket gs://${PROJECT_ID}-tfstate already exists - unchanged" \
    "workload identity pool github-plan already exists - unchanged" \
    "workload identity pool github-apply already exists - unchanged" \
    "provider github-plan in pool github-plan already exists" \
    "provider github-apply in pool github-apply already exists" \
    "service account mt-planner already exists - unchanged" \
    "service account mt-applier already exists - unchanged"; do
    expect_contains "$OUT" "$token" "the second run reports: $token"
  done
  expect_not_contains "$OUT" "created state bucket" "the second run creates no state bucket"
}

boot_ac3_refuses_unsafe_contexts() {
  # (a) a service account as the active gcloud account
  setup_case
  ACCOUNT="ci@example-project.iam.gserviceaccount.com"
  bootstrap --yes
  expect_nonzero "$?" "a service account account is refused"
  expect_contains "$ERR" "human user" "the refusal names the human-user requirement"
  refuse_left_nothing_behind "the service account refusal"

  # (b) billing not linked
  setup_case
  BILLING="False"
  bootstrap --yes
  expect_nonzero "$?" "a project without billing is refused"
  expect_contains "$OUT" "https://console.cloud.google.com/billing/linkedaccount?project=${PROJECT_ID}" \
    "the refusal prints the console link for linking billing"
  refuse_left_nothing_behind "the billing refusal"

  # (c) production without the explicit acknowledgement
  setup_case
  CASE_ENV="production"
  bootstrap --yes
  expect_nonzero "$?" "production without --i-know-the-log-bucket-lock-is-permanent is refused"
  if [ -s "$LOG" ]; then
    fail "the production refusal called gcloud before refusing"
  else
    ok "the production refusal calls no gcloud command at all"
  fi

  # (d) the project does not exist
  setup_case
  PROJECT_MISSING="1"
  bootstrap --yes
  expect_nonzero "$?" "a project that does not exist is refused"
  expect_contains "$ERR" "does not exist" "the refusal says the project is not there"
  refuse_left_nothing_behind "the missing project refusal"

  # (e) nobody confirmed
  setup_case
  bootstrap
  expect_nonzero "$?" "an unanswered confirmation aborts"
  refuse_left_nothing_behind "the unanswered confirmation"
}

refuse_left_nothing_behind() { # <label>
  local changed
  changed="$(mutating_lines "$LOG")"
  if [ -n "$changed" ]; then
    fail "$1 issued a mutating call: $(printf '%s' "$changed" | tr '\n' ' ')"
  else
    ok "$1 created nothing"
  fi
}

boot_ac4_no_key_creation_call() {
  setup_case
  bootstrap --yes
  expect_status 0 "$?" "a full run exits 0"

  local keys
  keys="$(grep -FxE -- 'keys' "$LOG" || true)"
  if [ -n "$keys" ]; then
    fail "the stub saw a service account key call"
  else
    ok "the run never asked for a service account key"
  fi

  # Across every call this test file made, including the earlier tests.
  if grep -Fq -- 'keys create' "$ALL_LOG"; then
    fail "the stub saw 'keys create' at some point"
  else
    ok "no call in the whole test run was 'keys create'"
  fi

  # And the script cannot contain one either: a source check that fails the
  # moment somebody adds one.
  if grep -Eq -- 'service-accounts keys|keys create' "$SCRIPT"; then
    fail "the script contains a key creation call"
  else
    ok "the script contains no key creation call"
  fi

  # The identities are keyless: both are bound to a workload identity principal.
  expect_log_line "--role=roles/iam.workloadIdentityUser" "the workload identity role is used"
  expect_log_line "--member=principalSet://iam.googleapis.com/projects/${PROJECT_NUMBER}/locations/global/workloadIdentityPools/github-apply/attribute.environment/staging" \
    "the applier is impersonated through the apply pool"
}

boot_ac5_planner_cannot_reach_applier() {
  setup_case
  bootstrap --yes
  expect_status 0 "$?" "a full run exits 0"

  local expected_applier_member expected_planner_member
  expected_applier_member="--member=principalSet://iam.googleapis.com/projects/${PROJECT_NUMBER}/locations/global/workloadIdentityPools/github-apply/attribute.environment/staging"
  expected_planner_member="--member=principalSet://iam.googleapis.com/projects/${PROJECT_NUMBER}/locations/global/workloadIdentityPools/github-plan/attribute.repository/${SLUG}"

  # The two pools are separate, and each binds exactly one member.
  local apply_members plan_members
  apply_members="$(grep -F -- '--member=' "$LOG" | grep -F 'github-apply' || true)"
  plan_members="$(grep -F -- '--member=' "$LOG" | grep -F 'github-plan' || true)"
  if [ "$apply_members" = "$expected_applier_member" ]; then
    ok "the applier is bound only to the apply pool's environment attribute"
  else
    fail "the applier's workload identity member is '$apply_members'"
  fi
  if [ "$plan_members" = "$expected_planner_member" ]; then
    ok "the planner is bound only to the plan pool's repository attribute"
  else
    fail "the planner's workload identity member is '$plan_members'"
  fi

  # No member anywhere names the applier and the repository together: a planner
  # principal cannot reach mt-applier.
  if grep -F -- '--member=' "$LOG" | grep -Fq 'attribute.repository'; then
    local applier_repository
    applier_repository="$(grep -F -- '--member=' "$LOG" | grep -F 'attribute.repository' | grep -F "$APPLIER_EMAIL" || true)"
    if [ -n "$applier_repository" ]; then
      fail "an applier binding names the repository attribute: $applier_repository"
    else
      ok "no applier binding names the repository attribute"
    fi
  else
    ok "no binding names the repository attribute with the applier"
  fi

  # Two impersonation bindings in total, one per service account.
  local wif_bindings
  wif_bindings="$(grep -Fxc -- '--role=roles/iam.workloadIdentityUser' "$LOG")"
  if [ "$wif_bindings" -eq 2 ]; then
    ok "exactly two workload identity bindings exist, one per service account"
  else
    fail "expected two workload identity bindings, saw ${wif_bindings}"
  fi

  # The apply provider's condition pins the main branch, the environment and the
  # pinned workflow; the plan provider's condition pins no environment at all.
  local conditions apply_condition plan_condition
  conditions="$(grep -F -- '--attribute-condition=' "$LOG")"
  apply_condition="$(printf '%s\n' "$conditions" | grep -F "assertion.environment == 'staging'" || true)"
  plan_condition="$(printf '%s\n' "$conditions" | grep -Fv "assertion.environment" || true)"

  if [ "$(printf '%s\n' "$conditions" | grep -Fc -- "assertion.environment == 'staging'")" -eq 1 ]; then
    ok "only one provider (the apply provider) names the environment attribute"
  else
    fail "more than one provider condition names the environment"
  fi

  local term
  for term in \
    "assertion.repository == '${SLUG}'" \
    "assertion.event_name == 'push'" \
    "assertion.ref == 'refs/heads/main'" \
    "assertion.job_workflow_ref == '${SLUG}/.github/workflows/terraform.yml@refs/heads/main'"; do
    if printf '%s\n' "$apply_condition" | grep -Fq -- "$term"; then
      ok "the apply provider condition contains: $term"
    else
      fail "the apply provider condition is missing: $term"
    fi
    if printf '%s\n' "$plan_condition" | grep -Fq -- "$term"; then
      ok "the plan provider condition contains: $term"
    else
      fail "the plan provider condition is missing: $term"
    fi
  done

  if printf '%s\n' "$conditions" | grep -Fq 'pull_request'; then
    fail "a provider condition admits pull requests"
  else
    ok "no provider condition admits a pull request"
  fi

  # The plan provider maps the repository only: the environment, the ref and the
  # workflow attributes are the apply provider's alone.
  local mappings without_env
  mappings="$(grep -F -- '--attribute-mapping=' "$LOG" | sort -u)"
  without_env="$(printf '%s\n' "$mappings" | grep -Fv 'attribute.environment' || true)"
  if [ "$(printf '%s\n' "$mappings" | grep -c .)" -eq 2 ] && [ "$(printf '%s\n' "$without_env" | grep -c .)" -eq 1 ]; then
    ok "only the apply provider maps the environment attribute"
  else
    fail "the provider attribute mappings are not one plan and one apply mapping"
  fi
  for term in attribute.ref= attribute.environment= attribute.job_workflow_ref=; do
    if printf '%s\n' "$without_env" | grep -Fq -- "$term"; then
      fail "the plan provider mapping contains $term"
    else
      ok "the plan provider mapping does not contain $term"
    fi
  done
  if printf '%s\n' "$without_env" | grep -Fq 'attribute.repository=assertion.repository'; then
    ok "the plan provider maps attribute.repository"
  else
    fail "the plan provider does not map attribute.repository"
  fi
  if [ "$(printf '%s\n' "$mappings" | grep -Fc 'attribute.job_workflow_ref')" -eq 1 ]; then
    ok "only the apply provider maps the job_workflow_ref attribute"
  else
    fail "the number of providers mapping job_workflow_ref is not one"
  fi

  # Every other grant names the service account itself: no direct grant escapes
  # through a pool principal.
  if grep -Fxq -- "--member=serviceAccount:${PLANNER_EMAIL}" "$LOG" &&
    grep -Fxq -- "--member=serviceAccount:${APPLIER_EMAIL}" "$LOG"; then
    ok "the project role grants name the two service accounts, not a pool principal"
  else
    fail "the project role grants do not name both service accounts"
  fi
}

boot_ac6_iam_admin_grant_is_conditioned() {
  setup_case
  bootstrap --yes
  expect_status 0 "$?" "a full run exits 0"

  local conditions grants union modules
  conditions="${CASE_DIR}/conditions.txt"
  grep -F -- '--condition=expression=' "$LOG" >"$conditions" || true
  grants="$(grep -Fxc -- '--role=roles/resourcemanager.projectIamAdmin' "$LOG")"

  if [ "$grants" -ge 1 ] && [ "$grants" -eq "$(grep -c . "$conditions")" ]; then
    ok "every projectIamAdmin grant carries a condition ($grants binding(s))"
  else
    fail "expected one condition per projectIamAdmin grant; saw $grants grant(s) and $(grep -c . "$conditions") condition(s)"
  fi

  # No call carries a condition without granting projectIamAdmin, and none grants
  # projectIamAdmin without a condition.
  if conditioned_calls_are_iam_admin_grants; then
    ok "conditions appear only on the projectIamAdmin grant"
  else
    fail "a condition is attached to something other than the projectIamAdmin grant"
  fi

  # Every condition uses the documented attribute, keeps to the documented list
  # limit of ten roles, and is not joined with `||`.
  local line over=0 roles_in line_count
  line_count="$(grep -c . "$conditions")"
  while IFS= read -r line; do
    if ! printf '%s' "$line" | grep -Fq 'modifiedGrantsByRole' ||
      ! printf '%s' "$line" | grep -Fq 'hasOnly(['; then
      fail "a condition does not use modifiedGrantsByRole/hasOnly: $line"
    fi
    if printf '%s' "$line" | grep -Fq '||'; then
      fail "a condition joins hasOnly() statements with '||': $line"
    fi
    roles_in="$(condition_list_roles "$line" | grep -c .)"
    if [ "$roles_in" -gt 10 ]; then
      over=1
      fail "a condition lists $roles_in roles; hasOnly() takes at most ten: $line"
    fi
  done <"$conditions"
  if [ "$over" -eq 0 ] && [ "$line_count" -ge 1 ]; then
    ok "each of the $line_count condition(s) uses modifiedGrantsByRole and a hasOnly() list of at most ten roles"
  fi

  # The union of the lists is exactly the roles the two modules bind.
  union="$(while IFS= read -r line; do condition_list_roles "$line"; done <"$conditions" | sort -u)"
  modules="$(module_roles_from_terraform)"
  if [ -n "$modules" ] && [ "$union" = "$modules" ]; then
    ok "together the conditions allow exactly the roles modules/foundation and modules/runtime bind"
  else
    fail "the conditions' roles differ from the modules':
$(printf '%s\n' "$union" | sed 's/^/    condition: /')
$(printf '%s\n' "$modules" | sed 's/^/      modules: /')"
  fi

  if printf '%s\n' "$union" | grep -Fq 'roles/resourcemanager.projectIamAdmin'; then
    fail "the conditions let the applier grant itself projectIamAdmin"
  else
    ok "the conditions cannot widen themselves"
  fi

  if printf '%s\n' "$union" | grep -Fq 'roles/iam.serviceAccountTokenCreator'; then
    fail "the conditions allow roles/iam.serviceAccountTokenCreator"
  else
    ok "the conditions allow no token-creator role"
  fi

  local forbidden
  for forbidden in roles/owner roles/editor roles/iam.securityAdmin roles/iam.serviceAccountTokenCreator; do
    if grep -Fxq -- "--role=$forbidden" "$LOG"; then
      fail "the script grants $forbidden"
    else
      ok "the script never grants $forbidden"
    fi
  done
}

# ------------------------------------------------------------------ runner

check_sources() {
  printf '\n== sources\n'
  local file
  for file in "$SCRIPT" "$0"; do
    if bash -n "$file"; then
      ok "bash parses $file"
    else
      fail "bash cannot parse $file"
    fi
  done
  # Clean under shellcheck (T-1116 "Tests that must pass"). CI does not run the
  # tool for scripts/, so a missing binary is reported rather than failed.
  if command -v shellcheck >/dev/null 2>&1; then
    if shellcheck "$SCRIPT" "$0"; then
      ok "shellcheck is clean"
    else
      fail "shellcheck reported findings"
    fi
  else
    printf '  SKIP: shellcheck is not installed; run it where it is (it is a required check)\n' >&2
  fi
}

main() {
  TEST_TMP="$(mktemp -d "${TMPDIR:-/tmp}/bootstrap-test.XXXXXX")"
  STUB_DIR="${TEST_TMP}/bin"
  ALL_LOG="${TEST_TMP}/all-calls.log"
  : >"$ALL_LOG"
  write_stub

  check_sources

  local test status_before
  for test in "${TESTS[@]}"; do
    printf '\n== %s\n' "$test"
    status_before="$FAILED"
    "$test"
    if [ "$FAILED" -eq "$status_before" ]; then
      printf 'ok %s\n' "$test"
    else
      printf 'not ok %s\n' "$test"
    fi
  done

  printf '\n%s test(s) reported %s failure(s)\n' "${#TESTS[@]}" "$FAILED"
  if [ "$FAILED" -gt 0 ]; then
    return 1
  fi
  printf 'all bootstrap tests passed\n'
  return 0
}

main "$@"
