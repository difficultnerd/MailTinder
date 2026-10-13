#!/usr/bin/env bash
# T-1116 tests for scripts/infra/bootstrap-project.sh.
#
# A `gcloud` stub sits first on PATH. It records every argument of every call
# (one argument per line, calls separated by a `@@CALL` marker), copies every
# `--condition-from-file` it is handed into a second log, and models whether a
# resource already exists, so the tests assert from the calls the script really
# made - not from its source text - and a second run is exercised against a
# project where everything already exists.
#
#   boot_ac1_dry_run_creates_nothing
#   boot_ac2_second_run_is_noop
#   boot_ac3_refuses_unsafe_contexts
#   boot_ac4_no_key_creation_call
#   boot_ac5_planner_cannot_reach_applier
#   boot_ac6_iam_admin_grant_is_conditioned
#
# and the negative cases an independent security review asked for (SEC-01 to
# SEC-09 in the review on commit 2089917), each asserting on the recorded calls:
#
#   boot_sec01_applier_holds_nothing_in_the_identity_project
#   boot_sec02_no_project_wide_impersonation_grant
#   boot_sec03_applier_cannot_administer_a_bucket
#   boot_sec04_applier_has_no_secret_version_permission
#   boot_sec05_conditions_are_files_the_real_parser_accepts
#   boot_sec06_mismatched_identity_configuration_fails_closed
#   boot_sec07_first_apply_capabilities_are_present
#   boot_sec09_key_detector_catches_an_injected_call
#   boot_sec09_suite_runs_in_ci
#
# plus shellcheck (skipped, loudly, when the tool is not installed).
set -uo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
SCRIPT="${REPO_ROOT}/scripts/infra/bootstrap-project.sh"
MODULES="${REPO_ROOT}/infra/terraform/modules"
CI_WORKFLOW="${REPO_ROOT}/.github/workflows/ci.yml"

SLUG="example-owner/example-repo"
PROJECT_ID="example-project-staging"
IDENTITY_PROJECT_ID="example-identity-staging"
IDENTITY_PROJECT_NUMBER="123456789012"
PLANNER_EMAIL="mt-planner@${IDENTITY_PROJECT_ID}.iam.gserviceaccount.com"
APPLIER_EMAIL="mt-applier@${IDENTITY_PROJECT_ID}.iam.gserviceaccount.com"
RESOURCE_SCOPED_ROLES="roles/iam.serviceAccountUser
roles/iam.workloadIdentityUser"

FAILED=0
TESTS=(
  boot_ac1_dry_run_creates_nothing
  boot_ac2_second_run_is_noop
  boot_ac3_refuses_unsafe_contexts
  boot_ac4_no_key_creation_call
  boot_ac5_planner_cannot_reach_applier
  boot_ac6_iam_admin_grant_is_conditioned
  boot_sec01_applier_holds_nothing_in_the_identity_project
  boot_sec02_no_project_wide_impersonation_grant
  boot_sec03_applier_cannot_administer_a_bucket
  boot_sec04_applier_has_no_secret_version_permission
  boot_sec05_conditions_are_files_the_real_parser_accepts
  boot_sec06_mismatched_identity_configuration_fails_closed
  boot_sec07_first_apply_capabilities_are_present
  boot_sec09_key_detector_catches_an_injected_call
  boot_sec09_suite_runs_in_ci
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

# One line per call, arguments space separated: the shape the tests reason about.
calls() { # <file>
  awk '
    /^@@CALL$/ { if (line != "") print line; line = ""; next }
    { if (line == "") { line = $0 } else { line = line " " $0 } }
    END { if (line != "") print line }
  ' "$1"
}

# Any argument that creates or changes something.
mutating_lines() { # <file>
  grep -xE -- 'create|create-oidc|update|enable|add-iam-policy-binding|delete|set-iam-policy|keys' "$1" || true
}

# A service account key call, in any spelling: the `keys` subcommand of
# `iam service-accounts`, or a role grant that would allow one. The stub records
# one argument per line, so a whole argument is the only thing that can match -
# this is what makes BOOT AC4 fail closed instead of looking for the words on one
# line (SEC-09).
key_creation_calls() { # <file>
  awk '
    /^@@CALL$/ { if (key) { print block }; block = ""; key = 0; next }
    { block = block (block == "" ? "" : " ") $0 }
    $0 == "keys" { key = 1 }
    $0 == "serviceAccountKeys" { key = 1 }
    END { if (key) { print block } }
  ' "$1"
}

# The roles the script's own tables declare: the plan must list exactly these.
spec_roles_from_script() {
  awk '
    index($0, "planner_role_specs() {") == 1 { inside = 1; next }
    index($0, "applier_role_specs() {") == 1 { inside = 1; next }
    inside && /^}/ { inside = 0 }
    inside && /^#/ { next }
    inside
  ' "$SCRIPT" | sed -n 's/^\(roles\/[^|]*\)|.*$/\1/p'
}

custom_role_ids_from_script() {
  awk '
    index($0, "custom_role_specs() {") == 1 { inside = 1; next }
    inside && /^}/ { inside = 0 }
    inside && /^#/ { next }
    inside
  ' "$SCRIPT" | sed -n 's/^\([A-Za-z0-9_]*\)|.*$/\1/p'
}

# Every role the modules grant, whatever resource grants it: the ground truth for
# the split below.
module_roles_from_terraform() {
  sed 's/#.*//' "${MODULES}"/foundation/*.tf "${MODULES}"/runtime/*.tf |
    grep -ohE 'roles/[a-zA-Z0-9._-]+' | sort -u
}

# The roles the modules grant on IAM resources, split by the *resource type* that
# grants them and not by the role string (N-02):
#   `project`  - exactly a `google_project_iam_member`, i.e. grantable at project
#                scope, which is what the applier's projectIamAdmin condition may
#                allow;
#   `resource` - any other `google_*_iam_*` resource: bound on one crypto key, a
#                secret, a service, a repository, a queue or a service account,
#                and bound there by the applier with a resource-level role.
# An oracle that collected role names alone would pass for exactly the reason the
# script did - the shared flaw N-02 named - so it must read the resource type.
iam_roles_from_terraform() { # <project|resource>
  local want="$1"
  awk -v want="$want" '
    /^resource[ \t]+"/ {
      type = $2; gsub(/"/, "", type)
      is_project = (type == "google_project_iam_member")
      inside = (type ~ /^google_.*_iam_/) && ((want == "project") == is_project)
      next
    }
    inside && /^}/ { inside = 0; next }
    inside && $1 == "role" {
      line = $0
      sub(/[ \t]*#.*/, "", line)
      sub(/^[^"]*"/, "", line)
      sub(/"?[ \t]*$/, "", line)
      print line
    }
  ' "${MODULES}"/foundation/*.tf "${MODULES}"/runtime/*.tf | sort -u
}

project_scope_roles_from_terraform() { iam_roles_from_terraform project; }
resource_bound_roles_from_terraform() { iam_roles_from_terraform resource; }

# The three runtime identities, from the two places that name them: the
# foundation module's `runtime_service_accounts` map (account ids `mt-<key>`) and
# the `service_account` argument of the three Cloud Run services. The applier
# must hold `actAs` on these and no others (N-04).
runtime_accounts_from_foundation() {
  awk '
    index($0, "runtime_service_accounts = {") > 0 { inside = 1; next }
    inside && /^}/ { inside = 0; next }
    inside && /^[[:space:]]*[a-zA-Z0-9_-]+[[:space:]]*=/ {
      key = $1; sub(/[[:space:]]*=.*/, "", key); print "mt-" key
    }
  ' "${MODULES}/foundation/service_accounts.tf" | sort -u
}

runtime_accounts_from_services() {
  awk '
    /service_account[ \t]*=[ \t]*var\.service_accounts\["/ {
      line = $0
      sub(/.*service_accounts\["/, "", line)
      sub(/".*/, "", line)
      print "mt-" line
    }
  ' "${MODULES}/runtime/cloud_run.tf" | sort -u
}

# The account ids the script itself binds actAs on.
runtime_accounts_from_script() {
  sed -n 's/^readonly RUNTIME_ACCOUNT_IDS=(\(.*\))$/\1/p' "$SCRIPT" |
    tr ' ' '\n' | grep -v '^$' | sort -u
}

# The roles inside a condition's hasOnly([...]) list.
condition_list_roles() { # <condition file>
  sed -n 's/.*hasOnly(\[\([^]]*\)\]).*/\1/p' "$1" | tr ',' '\n' |
    sed 's/[^A-Za-z0-9._/-]//g' | grep -v '^$'
}

# The condition files split into the two kinds: the one that limits what the
# applier may grant on a service account policy (it holds exactly the two
# impersonation roles) and the ones that limit the project-IAM-admin grant.
# Sets SA_COND_FILE and IAM_COND_FILES, and prints the total count on stdout.
classify_conditions() { # <cond log> <out dir>
  local dir="$2" files file roles
  files="$(split_conditions "$1" "$dir")"
  SA_COND_FILE=""
  IAM_COND_FILES=""
  local scoped
  scoped="$(printf '%s\n' "$RESOURCE_SCOPED_ROLES" | sort -u)"
  while IFS= read -r file; do
    [ -n "$file" ] || continue
    roles="$(condition_list_roles "$file" | sort -u)"
    if [ "$roles" = "$scoped" ]; then
      SA_COND_FILE="$file"
    else
      IAM_COND_FILES="${IAM_COND_FILES}${IAM_COND_FILES:+ }${file}"
    fi
  done <<<"$files"
  printf '%s\n' "$files" | grep -c . || true
}

# The condition files the stub collected, one file per --condition-from-file.
split_conditions() { # <cond log> <out dir>
  local dir="$2"
  mkdir -p "$dir"
  rm -f "${dir}"/cond-*.json
  awk -v dir="$dir" '
    /^@@CONDITION / { n++; file = sprintf("%s/cond-%.3d.json", dir, n); next }
    file != "" { print >file }
  ' "$1"
  find "$dir" -maxdepth 1 -name 'cond-*.json' | sort || true
}

condition_field() { # <condition file> <field>
  sed -n "s/.*\"$2\": \"\([^\"]*\)\".*/\1/p" "$1"
}

# Every call that carries a condition must name a role, and every grant of the
# project-IAM-admin role must carry a condition - no unconditioned IAM-admin
# grant is allowed to exist anywhere in the log.
conditioned_calls_are_iam_admin_grants() { # <file>
  awk -v admin="--role=roles/resourcemanager.projectIamAdmin" '
    function flush() {
      if (has_condition && !(has_admin || has_sa_role)) { bad++ }
      if (has_admin && !has_condition) { bad++ }
      has_condition = 0
      has_admin = 0
      has_sa_role = 0
    }
    /^@@CALL$/ { flush(); next }
    index($0, "--condition-from-file=") == 1 { has_condition = 1 }
    $0 == admin { has_admin = 1 }
    index($0, "--role=projects/") == 1 && index($0, "/roles/mt_applier_service_account_policies") > 0 { has_sa_role = 1 }
    END { flush(); exit (bad > 0 ? 1 : 0) }
  ' "$1"
}

# ---------------------------------------------------------------- the stub

write_stub() {
  mkdir -p "${STUB_DIR}"
  cat >"${STUB_DIR}/gcloud" <<'STUB'
#!/usr/bin/env bash
# A stand-in for gcloud. Every argument of every call is appended to STUB_LOG,
# one per line, with a `@@CALL` marker before each call; STUB_ALL_LOG (optional)
# collects the same for the whole test run, and STUB_CONDITION_LOG collects the
# content of every --condition-from-file. STUB_STATE holds one marker file per
# resource that exists, so a second bootstrap run sees everything as present.
set -u
log="${STUB_LOG:?STUB_LOG must name the call log}"
state="${STUB_STATE:?STUB_STATE must name the state directory}"
conds="${STUB_CONDITION_LOG:?STUB_CONDITION_LOG must name the condition log}"
project_default="${STUB_PROJECT:-example-project}"
identity_default="${STUB_IDENTITY_PROJECT:-example-identity}"

printf '@@CALL\n' >>"$log"
printf '%s\n' "$@" >>"$log"
if [ -n "${STUB_ALL_LOG:-}" ] && [ "$STUB_ALL_LOG" != "$log" ]; then
  printf '@@CALL\n' >>"$STUB_ALL_LOG"
  printf '%s\n' "$@" >>"$STUB_ALL_LOG"
fi
for arg in "$@"; do
  case "$arg" in
    --condition-from-file=*)
      printf '@@CONDITION %s\n' "$arg" >>"$conds"
      cat "${arg#--condition-from-file=}" >>"$conds"
      printf '\n' >>"$conds"
      ;;
  esac
done

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
      "$name")
        printf '%s' "$prev"
        return 0
        ;;
    esac
    prev="$arg"
  done
  printf '%s' ""
}

fmt="$(flag --format "$@")"
project="$(flag --project "$@")"
[ -n "$project" ] || project="$project_default"

case "${1:-}" in
  auth)
    if [ "${2:-}" = "list" ]; then
      printf '%s\n' "${STUB_ACCOUNT:-owner@example.com}"
    fi
    ;;
  billing)
    if [ "${2:-}" = "projects" ] && [ "${3:-}" = "describe" ]; then
      billed="${STUB_BILLING:-True}"
      if [ "${4:-}" = "$identity_default" ]; then
        billed="${STUB_IDENTITY_BILLING:-$billed}"
      fi
      printf '%s\n' "$billed"
    fi
    ;;
  projects)
    case "${2:-}" in
      describe)
        id="${3:-}"
        if [ "${STUB_PROJECT_MISSING:-0}" = "1" ]; then
          printf 'ERROR: (gcloud.projects.describe) Project [%s] not found\n' "$id" >&2
          exit 1
        fi
        if [ "$id" = "$identity_default" ]; then
          number="${STUB_IDENTITY_PROJECT_NUMBER:-123456789012}"
        else
          number="${STUB_PROJECT_NUMBER:-987654321098}"
        fi
        case "$fmt" in
          *projectNumber*) printf '%s\n' "$number" ;;
          *) printf 'projectId: %s\n' "$id" ;;
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
    verb="${3:-}"
    bucket="${4:-}"
    case "$verb" in
      describe)
        if [ -z "$fmt" ]; then
          if exists "bucket_${project}_${bucket}"; then
            printf 'name: %s\n' "$bucket"
          else
            printf 'ERROR: not found\n' >&2
            exit 1
          fi
        else
          case "$fmt" in
            *uniformBucketLevelAccess*) printf '%s\n' "${STUB_BUCKET_UNIFORM:-true}" ;;
            *publicAccessPrevention*) printf '%s\n' "${STUB_BUCKET_PAP:-enforced}" ;;
            *versioning.enabled*) printf '%s\n' "${STUB_BUCKET_VERSIONING:-true}" ;;
            *) printf 'name: %s\n' "$bucket" ;;
          esac
        fi
        ;;
      create) mark "bucket_${project}_${bucket}" ;;
      update) : ;;
      add-iam-policy-binding) printf 'bindings:\n' ;;
    esac
    ;;
  iam)
    case "${2:-}" in
      roles)
        verb="${3:-}"
        name="${4:-}"
        base="${name##*/}"
        case "$verb" in
          describe)
            if exists "role_${project}_${base}"; then
              if [ -n "$fmt" ]; then
                cat "$(key_for "role_${project}_${base}")"
              else
                printf 'title: %s\n' "$base"
              fi
            else
              printf 'ERROR: not found\n' >&2
              exit 1
            fi
            ;;
          create)
            perms="$(flag --permissions "$@")"
            if [ "${STUB_ROLE_DRIFT:-0}" = "1" ] && [ "$base" = "mt_applier_secret_containers" ]; then
              perms="${perms},secretmanager.versions.access"
            fi
            printf '%s\n' "$perms" | tr ',' '\n' | grep -v '^$' >"$(key_for "role_${project}_${base}")"
            ;;
        esac
        ;;
      service-accounts)
        verb="${3:-}"
        name="${4:-}"
        case "$verb" in
          describe)
            if exists "sa_${name}"; then
              printf 'email: %s\n' "$name"
            else
              printf 'ERROR: not found\n' >&2
              exit 1
            fi
            ;;
          create) mark "sa_${name}@${project}.iam.gserviceaccount.com" ;;
          add-iam-policy-binding)
            member="$(flag --member "$@")"
            role="$(flag --role "$@")"
            printf '%s|%s\n' "$role" "$member" >>"$(key_for "sapolicy_${name}")"
            printf 'bindings:\n'
            ;;
          get-iam-policy)
            policy="$(key_for "sapolicy_${name}")"
            roles_file="${policy}.roles"
            members_file="${policy}.members"
            : >"$roles_file"
            : >"$members_file"
            if [ -f "$policy" ]; then
              while IFS='|' read -r role member; do
                [ -n "$role" ] || continue
                printf '%s\n' "$role" >>"$roles_file"
                printf '%s\n' "$member" >>"$members_file"
              done <"$policy"
            fi
            if [ "${STUB_EXTRA_SA_BINDING:-0}" = "1" ]; then
              printf '%s\n' "roles/iam.serviceAccountAdmin" >>"$roles_file"
              printf '%s\n' "serviceAccount:someone@example.com" >>"$members_file"
            fi
            case "$fmt" in
              *bindings.role*) sort -u "$roles_file" ;;
              *bindings.members*) sort -u "$members_file" ;;
              *) printf '{"bindings": []}\n' ;;
            esac
            ;;
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
            verb="${4:-}"
            provider="${5:-}"
            case "$verb" in
              create-oidc)
                mark "provider_${project}_${pool}_${provider}"
                printf '%s\n' "$(flag --issuer-uri "$@")" >"$(key_for "prov_${project}_${pool}_${provider}.issuer")"
                printf '%s\n' "$(flag --attribute-condition "$@")" >"$(key_for "prov_${project}_${pool}_${provider}.condition")"
                printf '%s\n' "$(flag --attribute-mapping "$@")" >"$(key_for "prov_${project}_${pool}_${provider}.mapping")"
                ;;
              list)
                for m in "$state"/provider_"${project}"_"${pool}"_*; do
                  [ -f "$m" ] || continue
                  printf '%s\n' "${m##*/provider_${project}_${pool}_}"
                done
                if [ "${STUB_EXTRA_PROVIDER:-0}" = "1" ]; then
                  printf 'github-extra\n'
                fi
                ;;
              describe)
                if ! exists "provider_${project}_${pool}_${provider}"; then
                  printf 'ERROR: not found\n' >&2
                  exit 1
                fi
                case "$fmt" in
                  *oidc.issuerUri*)
                    printf '%s\n' "${STUB_PROVIDER_ISSUER:-$(cat "$(key_for "prov_${project}_${pool}_${provider}.issuer")")}"
                    ;;
                  *attributeCondition*)
                    printf '%s\n' "${STUB_PROVIDER_CONDITION:-$(cat "$(key_for "prov_${project}_${pool}_${provider}.condition")")}"
                    ;;
                  *attributeMapping*)
                    if [ -n "${STUB_PROVIDER_MAPPING:-}" ]; then
                      printf '%s\n' "$STUB_PROVIDER_MAPPING"
                    else
                      tr ',' '\n' <"$(key_for "prov_${project}_${pool}_${provider}.mapping")" |
                        sed 's/=/ : /'
                    fi
                    ;;
                  *) printf 'name: %s\n' "$provider" ;;
                esac
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
  CONDITIONS="${CASE_DIR}/conditions.log"
  COND_DIR="${CASE_DIR}/conditions"
  OUT="${CASE_DIR}/out"
  ERR="${CASE_DIR}/err"
  mkdir -p "$STATE"
  : >"$LOG"
  : >"$CONDITIONS"
  ACCOUNT="owner@example.com"
  BILLING="True"
  IDENTITY_BILLING="True"
  PROJECT_MISSING="0"
  CASE_ENV="staging"
  CASE_INPUT="/dev/null"
  PROVIDER_CONDITION=""
  PROVIDER_MAPPING=""
  EXTRA_PROVIDER="0"
  EXTRA_SA_BINDING="0"
  ROLE_DRIFT="0"
  BUCKET_VERSIONING="true"
}

bootstrap() { # [extra args...]
  PATH="${STUB_DIR}:${PATH}" \
    STUB_LOG="$LOG" \
    STUB_ALL_LOG="$ALL_LOG" \
    STUB_CONDITION_LOG="$CONDITIONS" \
    STUB_STATE="$STATE" \
    STUB_PROJECT="$PROJECT_ID" \
    STUB_IDENTITY_PROJECT="$IDENTITY_PROJECT_ID" \
    STUB_ACCOUNT="$ACCOUNT" \
    STUB_BILLING="$BILLING" \
    STUB_IDENTITY_BILLING="$IDENTITY_BILLING" \
    STUB_PROJECT_MISSING="$PROJECT_MISSING" \
    STUB_PROVIDER_CONDITION="$PROVIDER_CONDITION" \
    STUB_PROVIDER_MAPPING="$PROVIDER_MAPPING" \
    STUB_EXTRA_PROVIDER="$EXTRA_PROVIDER" \
    STUB_EXTRA_SA_BINDING="$EXTRA_SA_BINDING" \
    STUB_ROLE_DRIFT="$ROLE_DRIFT" \
    STUB_BUCKET_VERSIONING="$BUCKET_VERSIONING" \
    "$SCRIPT" --project "$PROJECT_ID" --identity-project "$IDENTITY_PROJECT_ID" \
    --env "$CASE_ENV" --repo "$SLUG" "$@" \
    >"$OUT" 2>"$ERR" <"$CASE_INPUT"
  return $?
}

# A run's slice of the log, so "this run issued no grant" can be asserted.
log_mark() { wc -l <"$LOG"; }
slice_from() { tail -n "+$(($1 + 1))" "$LOG"; }

sa_email() { # <account id>
  printf '%s@%s.iam.gserviceaccount.com' "$1" "$PROJECT_ID"
}

# The actAs binding the applier must hold on one runtime identity, as the script
# spells it (N-04): on the service account, with no condition.
actas_grant_line() { # <account id>
  printf 'iam service-accounts add-iam-policy-binding %s --project=%s --member=serviceAccount:%s --role=roles/iam.serviceAccountUser --condition=None\n' \
    "$(sa_email "$1")" "$PROJECT_ID" "$APPLIER_EMAIL"
}

# Mark a service account as already existing in the stub's state, using the same
# name-to-file mapping the stub's own key_for uses (`sa_<email>`), so the
# post-first-apply run (N-04) sees the runtime identities as present.
mark_existing_sa() { # <service account email>
  : >"${STATE}/$(printf '%s' "sa_$1" | tr -c 'A-Za-z0-9._-' '_')"
}

impersonation_grants() { # <file>  every call that grants an impersonation role
  calls "$1" | grep -E -- '--role=roles/iam\.(workloadIdentityUser|serviceAccountUser)( |$)' || true
}

grant_calls() { # <file>
  calls "$1" | grep -F -- 'add-iam-policy-binding' || true
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
    "workload identity pool github-plan with provider github-plan in ${IDENTITY_PROJECT_ID}" \
    "workload identity pool github-apply with provider github-apply in ${IDENTITY_PROJECT_ID}" \
    "service account mt-planner in ${IDENTITY_PROJECT_ID}" \
    "service account mt-applier in ${IDENTITY_PROJECT_ID}" \
    "the applier is granted no role at all in ${IDENTITY_PROJECT_ID}" \
    "roles/storage.objectViewer" \
    "roles/storage.objectAdmin" \
    "roles/resourcemanager.projectIamAdmin" \
    "projects/${PROJECT_ID}/roles/mt_applier_secret_containers" \
    "projects/${PROJECT_ID}/roles/mt_applier_service_account_policies" \
    "projects/${PROJECT_ID}/roles/mt_applier_workload_identity" \
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

  # Every role of both tables (the predefined ones and the custom roles) is
  # listed, and the plan lists no other role.
  local plan_roles spec_roles custom_role
  plan_roles="$(grep -E '^  (planner|applier) role ' "$OUT" | awk '{print $3}' | sort -u)"
  spec_roles="$(
    spec_roles_from_script
    while IFS= read -r custom_role; do
      printf 'projects/%s/roles/%s\n' "$PROJECT_ID" "$custom_role"
    done < <(custom_role_ids_from_script)
  )"
  spec_roles="$(printf '%s\n' "$spec_roles" | sort -u)"
  if [ -n "$spec_roles" ] && [ "$plan_roles" = "$spec_roles" ]; then
    ok "the dry run lists every role of both tables, custom roles included"
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
  mark="$(log_mark)"
  run1_mutatives="$(mutating_lines "$LOG" | sort -u)"

  CASE_INPUT="/dev/null"
  bootstrap --yes
  expect_status 0 "$?" "the second run exits 0"

  run2_slice="${CASE_DIR}/run2.log"
  slice_from "$mark" >"$run2_slice"
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
    "provider github-plan in pool github-plan already exists - will be verified" \
    "provider github-apply in pool github-apply already exists - will be verified" \
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

  # (b) billing not linked to the workload project
  setup_case
  BILLING="False"
  bootstrap --yes
  expect_nonzero "$?" "a workload project without billing is refused"
  expect_contains "$OUT" "https://console.cloud.google.com/billing/linkedaccount?project=${PROJECT_ID}" \
    "the refusal prints the console link for linking billing"
  refuse_left_nothing_behind "the billing refusal"

  # (b2) billing not linked to the identity project
  setup_case
  IDENTITY_BILLING="False"
  bootstrap --yes
  expect_nonzero "$?" "an identity project without billing is refused"
  expect_contains "$OUT" "https://console.cloud.google.com/billing/linkedaccount?project=${IDENTITY_PROJECT_ID}" \
    "the refusal prints the console link for the identity project"
  refuse_left_nothing_behind "the identity project billing refusal"

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

  # (f) one project for both roles: no boundary, so the script refuses before it
  # calls gcloud at all.
  setup_case
  PATH="${STUB_DIR}:${PATH}" \
    STUB_LOG="$LOG" STUB_ALL_LOG="$ALL_LOG" STUB_CONDITION_LOG="$CONDITIONS" \
    STUB_STATE="$STATE" STUB_PROJECT="$PROJECT_ID" \
    STUB_IDENTITY_PROJECT="$IDENTITY_PROJECT_ID" \
    "$SCRIPT" --project "$PROJECT_ID" --identity-project "$PROJECT_ID" \
    --env staging --repo "$SLUG" --yes >"$OUT" 2>"$ERR" </dev/null
  expect_nonzero "$?" "an identity project equal to the workload project is refused"
  expect_contains "$ERR" "must differ" "the refusal explains why the two must differ"
  if [ -s "$LOG" ]; then
    fail "the single-project refusal called gcloud before refusing"
  else
    ok "the single-project refusal calls no gcloud command at all"
  fi
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
  keys="$(key_creation_calls "$LOG")"
  if [ -n "$keys" ]; then
    fail "the stub saw a service account key call: $keys"
  else
    ok "the run never asked for a service account key"
  fi

  # Across every call this test file made, including the earlier tests.
  if [ -n "$(key_creation_calls "$ALL_LOG")" ]; then
    fail "the stub saw a service account key call at some point"
  else
    ok "no call in the whole test run was a service account key call"
  fi

  # And the script cannot contain one either: a source check that fails the
  # moment somebody adds one.
  if grep -Eq -- 'service-account[s] key[s]|gcloud ia[m] .*key[s] create' "$SCRIPT"; then
    fail "the script contains a key creation call"
  else
    ok "the script contains no key creation call"
  fi

  # The identities are keyless: both are bound to a workload identity principal.
  expect_log_line "--role=roles/iam.workloadIdentityUser" "the workload identity role is used"
  expect_log_line "--member=principalSet://iam.googleapis.com/projects/${IDENTITY_PROJECT_NUMBER}/locations/global/workloadIdentityPools/github-apply/attribute.environment/staging" \
    "the applier is impersonated through the apply pool"
}

boot_ac5_planner_cannot_reach_applier() {
  setup_case
  bootstrap --yes
  expect_status 0 "$?" "a full run exits 0"

  local expected_applier_member expected_planner_member
  expected_applier_member="--member=principalSet://iam.googleapis.com/projects/${IDENTITY_PROJECT_NUMBER}/locations/global/workloadIdentityPools/github-apply/attribute.environment/staging"
  expected_planner_member="--member=principalSet://iam.googleapis.com/projects/${IDENTITY_PROJECT_NUMBER}/locations/global/workloadIdentityPools/github-plan/attribute.repository/${SLUG}"

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
  # principal cannot reach mt-applier. The plan pool's principalSet may appear in
  # exactly one call, the planner's own impersonation binding.
  local plan_principal_calls
  plan_principal_calls="$(calls "$LOG" | grep -Fc 'github-plan/attribute.repository' || true)"
  if [ "$plan_principal_calls" -eq 1 ] && [ "$(calls "$LOG" | grep -F 'github-plan/attribute.repository')" = \
    "iam service-accounts add-iam-policy-binding ${PLANNER_EMAIL} --project=${IDENTITY_PROJECT_ID} ${expected_planner_member} --role=roles/iam.workloadIdentityUser --condition=None" ]; then
    ok "the plan pool's principalSet appears only in the planner's own impersonation binding"
  else
    fail "the plan pool's principalSet is used in ${plan_principal_calls} calls, not just the planner binding"
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
    "assertion.workflow_ref == '${SLUG}/.github/workflows/terraform.yml@refs/heads/main'"; do
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

  # SEC-08: the condition must not pin job_workflow_ref, which GitHub documents
  # only for jobs that execute a reusable workflow.
  if printf '%s\n' "$conditions" | grep -Fq 'job_workflow_ref'; then
    fail "a provider condition pins job_workflow_ref; an ordinary job does not carry that claim"
  else
    ok "no provider condition pins job_workflow_ref"
  fi

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
  for term in attribute.ref= attribute.environment= attribute.workflow_ref=; do
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
  if [ "$(printf '%s\n' "$mappings" | grep -Fc 'attribute.workflow_ref')" -eq 1 ]; then
    ok "only the apply provider maps the workflow_ref attribute"
  else
    fail "the number of providers mapping workflow_ref is not one"
  fi

  # Every other grant names the service account itself: no direct grant escapes
  # through a pool principal.
  if grep -Fxq -- "--member=serviceAccount:${PLANNER_EMAIL}" "$LOG" &&
    grep -Fxq -- "--member=serviceAccount:${APPLIER_EMAIL}" "$LOG"; then
    ok "the project role grants name the two service accounts, not a pool principal"
  else
    fail "the project role grants do not name both service accounts"
  fi

  # Every grant in the run is made in the workload project. In the identity
  # project nothing is granted to anyone, which is what keeps the applier away
  # from the pools, the providers and its own impersonation policy (SEC-01).
  local outside
  outside="$(calls "$LOG" | grep -F 'projects add-iam-policy-binding' | grep -Fv -- "projects add-iam-policy-binding ${PROJECT_ID}" || true)"
  if [ -z "$outside" ]; then
    ok "every project-level grant is in the workload project, none in the identity project"
  else
    fail "a grant was made in another project: $outside"
  fi
}

boot_ac6_iam_admin_grant_is_conditioned() {
  setup_case
  bootstrap --yes
  expect_status 0 "$?" "a full run exits 0"

  local grants
  grants="$(calls "$LOG" | grep -F -- '--role=roles/resourcemanager.projectIamAdmin' || true)"
  if [ -n "$grants" ] && [ "$(printf '%s\n' "$grants" | grep -Fc -- '--condition-from-file=')" -eq "$(printf '%s\n' "$grants" | grep -c .)" ]; then
    ok "every projectIamAdmin grant carries a condition"
  else
    fail "a projectIamAdmin grant carries no condition: $grants"
  fi

  # SEC-05: the condition is never passed as the comma-delimited dictionary flag.
  if calls "$LOG" | grep -Fq -- '--condition=expression='; then
    fail "a condition was passed with --condition=expression=, which gcloud rejects when the expression contains a comma"
  else
    ok "no condition used the comma-delimited --condition=expression= form"
  fi
  if grep -F -- '--condition=expression=' "$SCRIPT" | grep -vq '^[[:space:]]*#'; then
    fail "the script still builds an inline --condition=expression= argument"
  else
    ok "the script only ever passes conditions through --condition-from-file"
  fi

  if conditioned_calls_are_iam_admin_grants "$LOG"; then
    ok "conditions appear only on the projectIamAdmin grant and the service account policy grant"
  else
    fail "a condition is attached to something other than those two grants"
  fi

  local cond_files
  cond_files="$(split_conditions "$CONDITIONS" "$COND_DIR")"
  if [ -z "$cond_files" ]; then
    fail "no condition file was recorded"
    return
  fi
  ok "$(printf '%s\n' "$cond_files" | grep -c .) condition file(s) recorded"

  # Every condition uses the documented attribute, keeps to the documented list
  # limit of ten roles, and is not joined with `||`.
  local line line_count=0 roles_in over=0
  while IFS= read -r line; do
    [ -n "$line" ] || continue
    line_count=$((line_count + 1))
    if ! grep -Fq 'modifiedGrantsByRole' "$line" || ! grep -Fq 'hasOnly([' "$line"; then
      fail "a condition does not use modifiedGrantsByRole/hasOnly: $(cat "$line")"
    fi
    if grep -Fq '||' "$line"; then
      fail "a condition joins hasOnly() statements with '||': $(cat "$line")"
    fi
    roles_in="$(condition_list_roles "$line" | grep -c .)"
    if [ "$roles_in" -gt 10 ]; then
      over=1
      fail "a condition lists $roles_in roles; hasOnly() takes at most ten: $(cat "$line")"
    fi
  done < <(printf '%s\n' "$cond_files")
  if [ "$over" -eq 0 ] && [ "$line_count" -ge 1 ]; then
    ok "each condition uses modifiedGrantsByRole and a hasOnly() list of at most ten roles"
  fi

  # The union of the project-IAM-admin conditions' lists is exactly the roles the
  # two modules grant on a `google_project_iam_member` resource, which is what
  # "project scope" means here (BOOT AC6, N-02). The impersonation roles and the
  # seven roles the modules bind on one resource are deliberately absent.
  classify_conditions "$CONDITIONS" "$COND_DIR" >/dev/null
  if [ -z "$SA_COND_FILE" ] || [ -z "$IAM_COND_FILES" ]; then
    fail "the two kinds of condition were not both recorded (service account policy: '${SA_COND_FILE}', project IAM admin: '${IAM_COND_FILES}')"
    return
  fi
  local union expected
  union="$(
    for line in $IAM_COND_FILES; do
      condition_list_roles "$line"
    done
  )"
  union="$(printf '%s\n' "$union" | sort -u | grep -v '^$')"
  expected="$(project_scope_roles_from_terraform)"
  if [ -n "$expected" ] && [ "$union" = "$expected" ]; then
    ok "together the project conditions allow exactly the roles the modules grant on a project IAM member"
  else
    fail "the conditions' roles differ from the roles the modules grant on a google_project_iam_member:
$(printf '%s\n' "$union" | sed 's/^/    condition: /')
$(printf '%s\n' "$expected" | sed 's/^/    expected:  /')"
  fi

  # Each of these is bound by the modules on one resource (a crypto key, a
  # secret, a service, a repository, a queue), so it must not be grantable at
  # project scope: that widening was N-02 and the second half of SEC-02.
  local resource_only
  for resource_only in roles/cloudkms.cryptoKeyEncrypterDecrypter \
    roles/secretmanager.secretAccessor roles/run.invoker roles/run.developer \
    roles/artifactregistry.writer roles/cloudtasks.enqueuer \
    roles/cloudtasks.taskDeleter; do
    if printf '%s\n' "$union" | grep -Fxq -- "$resource_only"; then
      fail "the project-scope allowlist holds ${resource_only}, which the modules grant on one resource only (N-02)"
    else
      ok "the project-scope allowlist excludes ${resource_only} (bound per resource)"
    fi
  done

  # The split is exhaustive and nothing is lost by it: every role the modules
  # grant is either project-scope (in the allowlist) or bound on one resource,
  # and the applier binds those per resource with the resource-level roles it
  # holds - so removing them from the allowlist removes no capability the
  # modules need on the first apply (N-02).
  local module_roles resource_bound unaccounted
  module_roles="$(module_roles_from_terraform)"
  resource_bound="$(resource_bound_roles_from_terraform)"
  unaccounted="$(comm -23 <(printf '%s\n' "$module_roles") \
    <(printf '%s\n' "$union" "$resource_bound" | sort -u))"
  if [ -n "$module_roles" ] && [ -z "$unaccounted" ]; then
    ok "every role the modules grant is either in the project-scope allowlist or bound on one resource"
  else
    fail "roles the modules grant that the allowlist and the per-resource binds do not account for:
$(printf '%s\n' "$unaccounted" | sed 's/^/    /')"
  fi

  if printf '%s\n' "$union" | grep -Fxq -- 'roles/resourcemanager.projectIamAdmin'; then
    fail "the conditions let the applier grant itself projectIamAdmin"
  else
    ok "the conditions cannot widen themselves"
  fi

  local forbidden
  for forbidden in roles/owner roles/editor roles/iam.securityAdmin \
    roles/iam.serviceAccountTokenCreator roles/iam.serviceAccountKeyAdmin \
    roles/iam.workloadIdentityPoolAdmin; do
    if grep -Fxq -- "--role=$forbidden" "$LOG"; then
      fail "the script grants $forbidden"
    else
      ok "the script never grants $forbidden"
    fi
  done
}

boot_sec01_applier_holds_nothing_in_the_identity_project() {
  setup_case
  bootstrap --yes
  expect_status 0 "$?" "a full run exits 0"

  # Nothing in the identity project is granted to any member: no pool, provider
  # or bootstrap identity is reachable by the applier, so it cannot rewrite the
  # impersonation policy that admits it, disable or delete an identity, or edit
  # an admission rule (SEC-01).
  local identity_grants
  identity_grants="$(calls "$LOG" | grep -F -- 'add-iam-policy-binding' \
    | grep -F -- "--project=${IDENTITY_PROJECT_ID}" || true)"
  local expected_identity_grants
  expected_identity_grants="$(printf '%s\n%s\n' \
    "iam service-accounts add-iam-policy-binding ${PLANNER_EMAIL} --project=${IDENTITY_PROJECT_ID} --member=principalSet://iam.googleapis.com/projects/${IDENTITY_PROJECT_NUMBER}/locations/global/workloadIdentityPools/github-plan/attribute.repository/${SLUG} --role=roles/iam.workloadIdentityUser --condition=None" \
    "iam service-accounts add-iam-policy-binding ${APPLIER_EMAIL} --project=${IDENTITY_PROJECT_ID} --member=principalSet://iam.googleapis.com/projects/${IDENTITY_PROJECT_NUMBER}/locations/global/workloadIdentityPools/github-apply/attribute.environment/staging --role=roles/iam.workloadIdentityUser --condition=None" | sort)"
  if [ "$(printf '%s\n' "$identity_grants" | sort)" = "$expected_identity_grants" ]; then
    ok "the only bindings in the identity project are the two impersonation bindings"
  else
    fail "the identity project carries other bindings:
$(printf '%s\n' "$identity_grants" | sed 's/^/    /')"
  fi

  local grants
  grants="$(calls "$LOG" | grep -F -- "--member=serviceAccount:${APPLIER_EMAIL}" | grep -F -- 'projects add-iam-policy-binding' || true)"
  if [ -z "$grants" ] || printf '%s\n' "$grants" | grep -Fqv -- "projects add-iam-policy-binding ${PROJECT_ID}"; then
    fail "an applier grant is not in the workload project"
  else
    ok "every applier grant is a workload-project grant"
  fi

  # The applier's identity management is split: creation and policy binding are
  # two separate custom roles, and neither carries a destructive permission.
  local create_perms policy_perms
  create_perms="$(role_permissions_from_log mt_applier_service_accounts_create)"
  policy_perms="$(role_permissions_from_log mt_applier_service_account_policies)"
  if printf '%s\n' "$create_perms" | grep -Fxq 'iam.serviceAccounts.create'; then
    ok "the applier can create a service account"
  else
    fail "the applier cannot create a service account: $create_perms"
  fi
  if printf '%s\n' "$policy_perms" | grep -Fxq 'iam.serviceAccounts.setIamPolicy'; then
    ok "the applier can bind a role on a service account"
  else
    fail "the applier cannot bind a role on a service account"
  fi

  local perm
  for perm in iam.serviceAccounts.delete iam.serviceAccounts.disable \
    iam.serviceAccounts.enable iam.serviceAccounts.undelete \
    iam.serviceAccounts.actAs iam.serviceAccountKeys.create \
    iam.serviceAccountKeys.get iam.serviceAccountApiKeyBindings.create; do
    if printf '%s\n%s\n' "$create_perms" "$policy_perms" | grep -Fxq -- "$perm"; then
      fail "the applier's service account roles include $perm"
    else
      ok "the applier's service account roles exclude $perm"
    fi
  done

  # The policy role is granted with the roles-limiting condition; the creation
  # role is not conditioned (it grants no roles).
  local sa_policy_grant
  sa_policy_grant="$(calls "$LOG" | grep -F -- "/roles/mt_applier_service_account_policies" | grep -F -- 'add-iam-policy-binding' || true)"
  if printf '%s\n' "$sa_policy_grant" | grep -Fq -- '--condition-from-file='; then
    ok "the service account policy role is granted with a condition"
  else
    fail "the service account policy role is granted without a condition"
  fi

  # The custom roles are created in the workload project only.
  local outside_roles
  outside_roles="$(calls "$LOG" | grep -F 'iam roles create' | grep -Fv -- "--project=${PROJECT_ID}" || true)"
  if [ -z "$outside_roles" ]; then
    ok "every custom role is created in the workload project"
  else
    fail "a custom role was created outside the workload project: $outside_roles"
  fi
}

role_permissions_from_log() { # <role id>
  awk -v role="$1" '
    /^@@CALL$/ { block = ""; next }
    { block = block (block == "" ? "" : " ") $0 }
    index(block, "iam roles create " role " ") == 1 {
      n = split(block, parts, " ")
      for (i = 1; i <= n; i++) {
        if (parts[i] ~ /^--permissions=/) {
          sub("--permissions=", "", parts[i])
          gsub(",", "\n", parts[i])
          print parts[i]
        }
      }
    }
  ' "$LOG" | sort -u
}

boot_sec02_no_project_wide_impersonation_grant() {
  setup_case
  bootstrap --yes
  expect_status 0 "$?" "a full run exits 0"

  # The impersonation and actAs roles are only ever bound on a service account,
  # never at project scope: the applier cannot admit the plan pool's principalSet
  # to mt-applier, or an arbitrary member to a runtime identity, by granting a
  # project-wide role (SEC-02).
  local grant role
  while IFS= read -r role; do
    [ -n "$role" ] || continue
    grant="$(impersonation_grants "$LOG" | grep -F -- "--role=${role}" | grep -Fv 'iam service-accounts add-iam-policy-binding' || true)"
    if [ -z "$grant" ]; then
      ok "${role} is never granted at project scope"
    else
      fail "${role} is granted outside a service account policy: $grant"
    fi
  done <<<"$RESOURCE_SCOPED_ROLES"

  # Negative case: no project binding anywhere names the plan pool or the apply
  # pool as a member.
  local pool_member
  pool_member="$(calls "$LOG" | grep -F -- 'projects add-iam-policy-binding' | grep -F -- '--member=principalSet' || true)"
  if [ -z "$pool_member" ]; then
    ok "no project-level binding names a pool principalSet"
  else
    fail "a project-level binding names a pool principalSet: $pool_member"
  fi

  # Negative case: the project-IAM-admin allowlist cannot be used to reach the
  # two identities either, because the two roles are not in it.
  classify_conditions "$CONDITIONS" "$COND_DIR" >/dev/null
  local sa_roles
  sa_roles="$(condition_list_roles "$SA_COND_FILE" | sort -u)"
  if [ "$sa_roles" = "$(printf '%s\n' "$RESOURCE_SCOPED_ROLES" | sort -u)" ]; then
    ok "the service account policy condition allows exactly the two impersonation roles"
  else
    fail "the service account policy condition allows:
$(printf '%s\n' "$sa_roles" | sed 's/^/    /')"
  fi

  local iam_admin_roles
  iam_admin_roles="$(
    for file in $IAM_COND_FILES; do
      condition_list_roles "$file"
    done
  )"
  if printf '%s\n' "$iam_admin_roles" | grep -Fxq -f <(printf '%s\n' "$RESOURCE_SCOPED_ROLES"); then
    fail "the project-IAM-admin allowlist contains an impersonation role"
  else
    ok "the project-IAM-admin allowlist contains no impersonation or actAs role"
  fi
}

boot_sec03_applier_cannot_administer_a_bucket() {
  setup_case
  bootstrap --yes
  expect_status 0 "$?" "a full run exits 0"

  # No storage role is granted at project scope: the only storage grants are the
  # two bucket-level object roles on the state bucket (SEC-03).
  local project_storage
  project_storage="$(calls "$LOG" | grep -F -- 'projects add-iam-policy-binding' | grep -F -- '--role=roles/storage.' || true)"
  if [ -z "$project_storage" ]; then
    ok "no storage role is granted at project scope"
  else
    fail "a storage role is granted on the project: $project_storage"
  fi

  local bucket_storage
  bucket_storage="$(calls "$LOG" | grep -F -- 'storage buckets add-iam-policy-binding' || true)"
  if [ "$(printf '%s\n' "$bucket_storage" | grep -c .)" -eq 2 ] &&
    printf '%s\n' "$bucket_storage" | grep -Fq -- '--role=roles/storage.objectViewer' &&
    printf '%s\n' "$bucket_storage" | grep -Fq -- '--role=roles/storage.objectAdmin'; then
    ok "the only storage grants are objectViewer and objectAdmin on the state bucket"
  else
    fail "the state bucket grants are not the two expected object roles:
$(printf '%s\n' "$bucket_storage" | sed 's/^/    /')"
  fi

  # The Firebase grant is the explicit permission list, not Develop Admin (which
  # carries storage.buckets.setIamPolicy, update and delete).
  local firebase_perms
  firebase_perms="$(role_permissions_from_log mt_applier_firebase)"
  if printf '%s\n' "$firebase_perms" | grep -Fq 'firebase.projects.update' &&
    printf '%s\n' "$firebase_perms" | grep -Fq 'firebasehosting.sites.create'; then
    ok "the applier's firebase role links the project and manages the Hosting site"
  else
    fail "the applier's firebase role lacks firebase.projects.update or the site permission"
  fi
  if printf '%s\n' "$firebase_perms" | grep -F 'storage.' -q; then
    fail "the applier's firebase role carries a storage permission"
  else
    ok "the applier's firebase role carries no storage permission"
  fi

  local forbidden
  for forbidden in roles/firebase.developAdmin roles/firebase.admin roles/storage.admin \
    roles/firebase.developViewer; do
    if grep -Fxq -- "--role=$forbidden" "$LOG"; then
      fail "the script grants $forbidden"
    else
      ok "the script never grants $forbidden"
    fi
  done
}

boot_sec04_applier_has_no_secret_version_permission() {
  setup_case
  bootstrap --yes
  expect_status 0 "$?" "a full run exits 0"

  local secret_perms
  secret_perms="$(role_permissions_from_log mt_applier_secret_containers)"
  if printf '%s\n' "$secret_perms" | grep -Fxq 'secretmanager.secrets.setIamPolicy'; then
    ok "the applier can manage the container's accessor binding"
  else
    fail "the applier cannot manage a secret's IAM binding"
  fi
  if printf '%s\n' "$secret_perms" | grep -Fq 'secretmanager.versions.'; then
    fail "the applier's secret role carries a version permission: $(printf '%s\n' "$secret_perms" | grep 'versions.')"
  else
    ok "the applier's secret role carries no secretmanager.versions.* permission"
  fi

  local forbidden
  for forbidden in roles/secretmanager.admin roles/secretmanager.secretVersionManager \
    roles/secretmanager.secretVersionAdder; do
    if grep -Fxq -- "--role=$forbidden" "$LOG"; then
      fail "the script grants $forbidden"
    else
      ok "the script never grants $forbidden"
    fi
  done

  # Nothing in the run reads or adds a secret version, and nothing reads a value.
  if grep -Fxq -- 'versions' "$LOG" || grep -Fxq -- 'access' "$LOG"; then
    fail "a call used the secret versions subcommand"
  else
    ok "no call touched a secret version or value"
  fi
}

boot_sec05_conditions_are_files_the_real_parser_accepts() {
  setup_case
  bootstrap --yes
  expect_status 0 "$?" "a full run exits 0"

  local cond_files file
  cond_files="$(split_conditions "$CONDITIONS" "$COND_DIR")"
  if [ -z "$cond_files" ]; then
    fail "no condition file was recorded"
    return
  fi

  # The stub copies what the script handed gcloud. Each one must be a JSON object
  # with the three fields, and the expression must be the whole CEL statement
  # (the failure mode SEC-05 named was a plain string that gcloud's comma
  # delimiter sliced into pieces).
  while IFS= read -r file; do
    if ! grep -Eq '^\{.*"expression": ".*", "title": ".*", "description": ".*"\}$' "$file"; then
      fail "a condition file is not the expected JSON object: $(cat "$file")"
      continue
    fi
    local expression
    expression="$(condition_field "$file" expression)"
    if printf '%s' "$expression" | grep -Fq 'modifiedGrantsByRole' && printf '%s' "$expression" | grep -Fq ', '; then
      ok "a condition file carries the whole expression, commas included"
    else
      fail "a condition file's expression is not the expected CEL statement: $expression"
    fi
    printf '%s\n' "$expression" | grep -Fq '||' && fail "a condition joins hasOnly() with '||'"
  done < <(printf '%s\n' "$cond_files")

  # The real SDK parser is the regression test SEC-05 asked for: the arguments
  # the script builds go through gcloud's own condition parser, credential-free.
  if ! command -v python3 >/dev/null 2>&1; then
    printf '  SKIP: python3 is not installed, so the real condition parser could not be run\n'
    return 0
  fi
  local sdk_root pythonpath
  sdk_root=""
  if command -v gcloud >/dev/null 2>&1; then
    sdk_root="$(gcloud info --format='value(installation.sdk_root)' 2>/dev/null || true)"
  fi
  if [ -z "$sdk_root" ]; then
    printf '  SKIP: gcloud is not on PATH, so the real condition parser could not be run (CI installs it)\n'
    return 0
  fi
  pythonpath="${sdk_root}/lib:${sdk_root}/lib/third_party"
  if ! PYTHONPATH="$pythonpath" python3 -c 'import googlecloudsdk.command_lib.iam.iam_util' >/dev/null 2>&1; then
    printf '  SKIP: the gcloud SDK python modules are not importable under %s\n' "$pythonpath"
    return 0
  fi

  while IFS= read -r file; do
    if PYTHONPATH="$pythonpath" python3 "${REPO_ROOT}/scripts/infra/tests/check_condition_file.py" "$file" >/dev/null 2>&1; then
      ok "the real gcloud condition parser accepts $(basename "$file")"
    else
      fail "the real gcloud condition parser rejected $(basename "$file"): $(cat "$file")"
    fi
  done < <(printf '%s\n' "$cond_files")

  # And the inline form the script no longer uses is still the broken one: if
  # this ever stops failing, the file form is merely conservative, not required.
  local inline
  inline="$(condition_field "$(printf '%s\n' "$cond_files" | head -n 1)" expression)"
  if PYTHONPATH="$pythonpath" python3 "${REPO_ROOT}/scripts/infra/tests/check_condition_file.py" --inline \
    "expression=${inline},title=inline,description=inline" >/dev/null 2>&1; then
    printf '  ok: gcloud now accepts the inline form too; the file form is still the safer one\n'
  else
    ok "the inline --condition=expression=...,title=... form is still rejected when the expression contains commas"
  fi
}

boot_sec06_mismatched_identity_configuration_fails_closed() {
  # (a) a provider whose condition is not the intended one
  setup_case
  PROVIDER_CONDITION="assertion.repository == '${SLUG}'"
  bootstrap --yes
  expect_nonzero "$?" "a provider with a different attribute_condition fails the run"
  expect_contains "$ERR" "attribute_condition" "the failure names the attribute_condition"
  expect_contains "$ERR" "Delete and recreate the provider deliberately" \
    "the failure tells the owner to recreate it on purpose"
  expect_no_grants "the mismatched provider run"

  # (a2) a provider whose attribute mapping is not the intended one
  setup_case
  PROVIDER_MAPPING="google.subject=assertion.sub,attribute.repository=assertion.repository"
  bootstrap --yes
  expect_nonzero "$?" "a provider with a different attribute mapping fails the run"
  expect_contains "$ERR" "does not map" "the failure names the attribute that is not mapped"
  expect_no_grants "the mismatched mapping run"

  # (b) an extra provider in the plan pool: an extra admission path
  setup_case
  EXTRA_PROVIDER="1"
  bootstrap --yes
  expect_nonzero "$?" "an extra provider in a pool fails the run"
  expect_contains "$ERR" "admission path" "the failure explains why an extra provider matters"
  expect_no_grants "the extra provider run"

  # (c) an unexpected binding on a bootstrap identity
  setup_case
  EXTRA_SA_BINDING="1"
  bootstrap --yes
  expect_nonzero "$?" "an unexpected binding on a bootstrap identity fails the run"
  expect_contains "$ERR" "bindings this script did not install" \
    "the failure says the binding was not installed by this script"
  expect_no_grants "the unexpected binding run"

  # (d) drift in a custom role's permission list
  setup_case
  ROLE_DRIFT="1"
  bootstrap --yes
  expect_nonzero "$?" "a custom role with an extra permission fails the run"
  expect_contains "$ERR" "does not hold exactly the permissions" \
    "the failure names the permission drift"
  expect_no_grants "the custom role drift run"

  # (e) the state bucket's own security configuration
  setup_case
  BUCKET_VERSIONING="false"
  bootstrap --yes
  expect_nonzero "$?" "a state bucket without versioning fails the run"
  expect_contains "$ERR" "versioning" "the failure names versioning"
  expect_no_grants "the versioning drift run"
}

expect_no_grants() { # <label>
  local grants
  grants="$(grant_calls "$LOG")"
  if [ -n "$grants" ]; then
    fail "$1 granted a privilege before failing closed: $grants"
  else
    ok "$1 granted nothing"
  fi
}

boot_sec07_first_apply_capabilities_are_present() {
  setup_case
  bootstrap --yes
  expect_status 0 "$?" "a full run exits 0"

  local wif_perms firebase_perms secret_perms create_perms
  wif_perms="$(role_permissions_from_log mt_applier_workload_identity)"
  firebase_perms="$(role_permissions_from_log mt_applier_firebase)"
  secret_perms="$(role_permissions_from_log mt_applier_secret_containers)"
  create_perms="$(role_permissions_from_log mt_applier_service_accounts_create)"

  # The resources modules/runtime declares, each with the capability it needs
  # (SEC-07). The applier's predefined roles are checked from the grants.
  local perm
  for perm in iam.googleapis.com/workloadIdentityPools.create \
    iam.googleapis.com/workloadIdentityPoolProviders.create; do
    if printf '%s\n' "$wif_perms" | grep -Fxq -- "$perm"; then
      ok "the applier can $perm"
    else
      fail "the applier cannot $perm, so the runtime module's deploy pool cannot be created"
    fi
  done
  for perm in firebase.projects.update firebasehosting.sites.create; do
    if printf '%s\n' "$firebase_perms" | grep -Fxq -- "$perm"; then
      ok "the applier can $perm"
    else
      fail "the applier cannot $perm, so the first apply cannot link Firebase"
    fi
  done
  for perm in secretmanager.secrets.create secretmanager.secrets.setIamPolicy; do
    if printf '%s\n' "$secret_perms" | grep -Fxq -- "$perm"; then
      ok "the applier can $perm"
    else
      fail "the applier cannot $perm, so the seed secret cannot be created"
    fi
  done
  if printf '%s\n' "$create_perms" | grep -Fxq 'iam.serviceAccounts.create'; then
    ok "the applier can create the runtime service accounts"
  else
    fail "the applier cannot create the runtime service accounts"
  fi

  # And the capability does not come with control of a pool or a provider.
  if printf '%s\n' "$wif_perms" | grep -Fq 'setIamPolicy' || printf '%s\n' "$wif_perms" | grep -Fq 'getIamPolicy'; then
    fail "the pool role carries IAM policy control"
  else
    ok "the pool role carries no IAM policy control over a pool or provider"
  fi

  # The roles the modules bind at project scope are really granted, so the first
  # apply does not stop at the grants the bootstrap was supposed to make.
  local granted
  granted="$(calls "$LOG" | grep -F -- "--member=serviceAccount:${APPLIER_EMAIL}" | grep -F -- 'projects add-iam-policy-binding' || true)"
  for perm in roles/serviceusage.serviceUsageAdmin roles/cloudkms.admin roles/datastore.owner \
    roles/logging.admin roles/artifactregistry.admin roles/run.admin \
    roles/cloudtasks.admin roles/cloudscheduler.admin; do
    if printf '%s\n' "$granted" | grep -Fq -- "--role=${perm}"; then
      ok "the applier holds $perm"
    else
      fail "the applier does not hold $perm"
    fi
  done

  # actAs on the runtime identities (N-04, SEC-07). The three Cloud Run services
  # run as `mt-api`, `mt-unsub` and `mt-worker`, so creating or updating one
  # requires `roles/iam.serviceAccountUser` on that account. The account list is
  # derived from the two modules - not from the script - so a drift fails here.
  local accounts from_foundation from_services
  accounts="$(runtime_accounts_from_script)"
  from_foundation="$(runtime_accounts_from_foundation)"
  from_services="$(runtime_accounts_from_services)"
  if [ -n "$accounts" ] && [ "$accounts" = "$from_foundation" ] && [ "$accounts" = "$from_services" ]; then
    ok "the actAs accounts are exactly the runtime identities the two modules name"
  else
    fail "the actAs accounts differ from the modules:
$(printf '%s\n' "$accounts" | sed 's/^/    script:     /')
$(printf '%s\n' "$from_foundation" | sed 's/^/    foundation: /')
$(printf '%s\n' "$from_services" | sed 's/^/    cloud_run:  /')"
  fi

  # (a) The accounts only exist after the first apply, so on this run (they are
  # absent) the script grants nothing and prints the exact command instead.
  local account command
  for account in $accounts; do
    command="gcloud iam service-accounts add-iam-policy-binding $(sa_email "$account") --project=${PROJECT_ID} --member=serviceAccount:${APPLIER_EMAIL} --role=roles/iam.serviceAccountUser --condition=None"
    expect_contains "$OUT" "$command" "the run before the first apply prints the actAs command for $account"
    if calls "$LOG" | grep -Fq -- "add-iam-policy-binding $(sa_email "$account")"; then
      fail "the run bound actAs on $(sa_email "$account") before that account existed"
    else
      ok "no actAs binding was attempted on $account while it does not exist"
    fi
  done

  # (b) A run after the first apply (the accounts are there) binds the role on
  # exactly those three accounts - member, role and account - reads each one
  # back, and still grants nothing at project scope.
  setup_case
  for account in $accounts; do
    mark_existing_sa "$(sa_email "$account")"
  done
  bootstrap --yes
  expect_status 0 "$?" "a run after the first apply exits 0"

  local expected_actas actual_actas
  expected_actas="$(for account in $accounts; do actas_grant_line "$account"; done | sort)"
  actual_actas="$(calls "$LOG" | grep -F -- 'iam service-accounts add-iam-policy-binding' \
    | grep -F -- "--project=${PROJECT_ID}" | grep -F -- '--role=roles/iam.serviceAccountUser' | sort)"
  if [ -n "$expected_actas" ] && [ "$actual_actas" = "$expected_actas" ]; then
    ok "the applier is bound actAs on exactly the three runtime identities, on the account"
  else
    fail "the actAs bindings differ from the intended ones:
$(printf '%s\n' "$actual_actas" | sed 's/^/    actual:   /')
$(printf '%s\n' "$expected_actas" | sed 's/^/    expected: /')"
  fi
  for account in $accounts; do
    expect_contains "$OUT" "applier actAs on $(sa_email "$account") verified" \
      "the actAs binding on $account is read back"
  done
  if calls "$LOG" | grep -F -- 'projects add-iam-policy-binding' | grep -Fq -- "--role=roles/iam.serviceAccountUser"; then
    fail "roles/iam.serviceAccountUser was granted at project scope"
  else
    ok "roles/iam.serviceAccountUser is never granted at project scope"
  fi
}

boot_sec09_key_detector_catches_an_injected_call() {
  # The check that BOOT AC4 rests on is exercised against a log that really
  # contains an injected key-creation call, so it cannot pass by accident
  # (SEC-09).
  local injected
  setup_case
  injected="${CASE_DIR}/injected.log"
  {
    printf '@@CALL\n'
    printf 'iam\n'
    printf 'service-accounts\n'
    printf 'keys\n'
    printf 'create\n'
    printf 'mt-applier@example.iam.gserviceaccount.com\n'
    printf '@@CALL\n'
    printf 'storage\n'
    printf 'buckets\n'
    printf 'describe\n'
  } >"$injected"
  if [ -n "$(key_creation_calls "$injected")" ]; then
    ok "the key detector fires on an injected keys create call"
  else
    fail "the key detector missed an injected keys create call"
  fi
  if [ -n "$(mutating_lines "$injected")" ]; then
    ok "the mutating-call detector fires on the injected call too"
  else
    fail "the mutating-call detector missed the injected call"
  fi

  # The same detector on this file's own source: no key creation call. The
  # pattern is spelled with a character class so this check does not match the
  # line that holds it.
  if grep -Eq -- 'service-account[s] key[s]|gcloud ia[m] .*key[s] create' "$0"; then
    fail "this test file contains a key creation call"
  else
    ok "this test file contains no key creation call"
  fi
}

boot_sec09_suite_runs_in_ci() {
  # A verification nothing runs is fail-open: the suite must be wired into CI
  # (SEC-09).
  if grep -Eq '^  infra-scripts:' "$CI_WORKFLOW"; then
    ok "ci.yml defines the infra-scripts job"
  else
    fail "ci.yml does not define an infra-scripts job"
  fi
  if grep -Fq -- 'scripts/infra/tests/bootstrap_test.sh' "$CI_WORKFLOW"; then
    ok "ci.yml runs scripts/infra/tests/bootstrap_test.sh"
  else
    fail "ci.yml does not run scripts/infra/tests/bootstrap_test.sh"
  fi
  if grep -Fq -- 'shellcheck' "$CI_WORKFLOW"; then
    ok "ci.yml installs or runs shellcheck"
  else
    fail "ci.yml does not run shellcheck, so the shellcheck assertion would skip in CI"
  fi

  # Running is not enough: the job must be inside the aggregate gate, because
  # branch protection keys on `ci-integrity` and a job outside its `needs` list
  # can fail without blocking a merge (N-01).
  local needs_line
  needs_line="$(awk '
    /^  ci-integrity:/ { inside = 1 }
    inside && /^    needs:/ { print; exit }
  ' "$CI_WORKFLOW")"
  if printf '%s\n' "$needs_line" | grep -Fq 'infra-scripts'; then
    ok "ci-integrity requires the infra-scripts job:${needs_line:+ $needs_line}"
  else
    fail "ci-integrity does not require the infra-scripts job (read '${needs_line}')"
  fi
  if grep -Fq -- 'report infra-scripts' "$CI_WORKFLOW"; then
    ok "ci-integrity reports the infra-scripts result"
  else
    fail "ci-integrity does not report the infra-scripts result"
  fi
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
  # Clean under shellcheck (T-1116 "Tests that must pass"). CI runs it in the
  # shell-scripts job; a missing binary is reported rather than failed.
  if command -v shellcheck >/dev/null 2>&1; then
    if shellcheck "$SCRIPT" "$0"; then
      ok "shellcheck is clean"
    else
      fail "shellcheck reported findings"
    fi
  else
    printf '  SKIP: shellcheck is not installed; run it where it is (it is a required check, and CI installs it)\n' >&2
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
