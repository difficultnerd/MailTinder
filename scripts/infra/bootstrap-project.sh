#!/usr/bin/env bash
# T-1116 (BOOT AC1-AC6): bootstrap ONE Google Cloud project so that every later
# infrastructure change flows through the plan/apply pipeline (T-1117) and the
# owner only reads a summary and approves (ADR 0003).
#
# Usage:
#   scripts/infra/bootstrap-project.sh --project <id> --env staging|production \
#     --repo <owner/name> [--dry-run] [--yes] \
#     [--i-know-the-log-bucket-lock-is-permanent]
#
#   --dry-run   print the plan and stop: nothing is created, nothing is changed
#   --yes       skip the interactive confirmation (for a re-run after a partial
#               failure; the human user check and the billing check still run)
#
# It creates exactly what Terraform cannot create for itself:
#   - the six APIs Terraform needs to start: serviceusage, cloudresourcemanager,
#     iam, iamcredentials, sts, storage. Everything else (run, firestore, kms,
#     secretmanager, artifactregistry, logging, cloudtasks, cloudscheduler,
#     aiplatform, ...) is enabled by modules/foundation/apis.tf.
#   - the state bucket gs://<project>-tfstate: us-central1, uniform bucket-level
#     access, public access prevention, versioning on.
#   - two Workload Identity pools, `github-plan` and `github-apply`, one provider
#     each. A principalSet is scoped to a pool, not to a provider, so one pool
#     with two providers could not keep the planner away from the applier.
#   - the two service accounts, `mt-planner` and `mt-applier`, and their bindings.
#
# It never creates a service account key, never reads a secret value and never
# touches Terraform state. It is idempotent: a second run creates nothing (every
# creating call is guarded by a describe probe) and a run after a partial failure
# resumes where it stopped. The grants are re-applied with
# `add-iam-policy-binding`, which is a set operation on the API: re-adding a
# member that already has the role changes nothing.
#
# The two identities, and the role tables a reviewer checks (BOOT AC5, AC6)
# -------------------------------------------------------------------------
#
# mt-planner is bound ONLY to
#   principalSet://.../github-plan/attribute.repository/<repo>
# so any principal the plan provider admits can impersonate it. The plan
# provider's condition admits only a `push` to `refs/heads/main` of <repo> that
# runs the pinned workflow, so no pull request and no other repository can.
#
# mt-applier is bound ONLY to
#   principalSet://.../github-apply/attribute.environment/<env>
# (never to `attribute.repository`). A principalSet member matches ONE attribute,
# so the ref and workflow constraints are NOT repeated in the member: they are
# enforced by the github-apply provider's attribute_condition, which admits
# nothing but a `push` to `refs/heads/main` of <repo>, from the named GitHub
# environment, running the pinned workflow. The applier is the most powerful
# identity in the system; these two lines together are its gate.
#
# mt-applier project roles, each mapped to the resource in modules/foundation or
# modules/runtime that needs it. The resources do not exist yet at bootstrap
# time, so these grants are project-scoped; the state bucket is the one grant
# that can be resource-scoped here, and it is.
#
#   roles/serviceusage.serviceUsageAdmin      google_project_service
#   roles/cloudkms.admin                      google_kms_key_ring,
#                                             google_kms_crypto_key,
#                                             google_kms_crypto_key_iam_binding
#   roles/datastore.owner                     google_firestore_database,
#                                             google_firestore_field
#   roles/secretmanager.admin                 google_secret_manager_secret,
#                                             google_secret_manager_secret_iam_binding
#                                             (containers only: no value is read)
#   roles/iam.serviceAccountAdmin             google_service_account,
#                                             google_service_account_iam_member
#                                             (the runtime accounts and mt-deployer;
#                                             NOT the planner or the applier)
#   roles/logging.admin                       google_logging_project_bucket_config,
#                                             google_logging_project_sink,
#                                             google_logging_project_exclusion
#   roles/artifactregistry.admin              google_artifact_registry_repository,
#                                             google_artifact_registry_repository_iam_member
#   roles/run.admin                           google_cloud_run_v2_service,
#                                             google_cloud_run_v2_service_iam_member
#   roles/cloudtasks.admin                    google_cloud_tasks_queue,
#                                             google_cloud_tasks_queue_iam_member
#   roles/cloudscheduler.admin                google_cloud_scheduler_job
#   roles/firebase.developAdmin               google_firebase_project,
#                                             google_firebase_hosting_site
#   roles/resourcemanager.projectIamAdmin     google_project_iam_member,
#                                             google_project_iam_audit_config
#                                             CONDITIONED, see below
#
# It gets NO rights at all on the state bucket's identity pools, on the providers
# or on the planner and applier service accounts themselves, so it cannot widen
# its own access (ADR 0003 item 4). `roles/iam.serviceAccountUser` (actAs) on
# mt-api/mt-unsub/mt-worker, which a Cloud Run deploy needs, is a resource-level
# grant the modules make - it cannot be made here because those accounts are
# created by the first apply.
#
# The project-level grant of roles/resourcemanager.projectIamAdmin carries IAM
# Conditions that limit the roles it may grant to exactly the roles the two
# modules bind (BOOT AC6). Google's IAM Conditions documentation, "Set limits on
# granting roles" (verified 12 Oct 2026):
#   - `iam.googleapis.com/modifiedGrantsByRole` is the documented API attribute
#     for limiting role granting, and the resource types that accept it are
#     projects, folders and organizations;
#   - a `hasOnly([...])` list may hold at most 10 roles and must contain only
#     string constants, so the twelve roles the modules bind become two
#     conditioned bindings on the same member and role (bindings are additive).
#     The roles are never moved into an unconditioned binding, and the two
#     `hasOnly()` lists are never joined with `||`, which that page explicitly
#     warns against;
#   - roles that can grant roles themselves (anything with `setIamPolicy`) must
#     not appear in a list, or the applier could widen itself. The lists are the
#     modules' runtime roles only, never `resourcemanager.projectIamAdmin`.
#
# Never granted to anyone by this script: roles/owner, roles/editor,
# roles/iam.securityAdmin, roles/iam.serviceAccountTokenCreator.
#
# Deliberately not covered: modules/monitoring is named by neither role table in
# T-1116 step 6, so no monitoring role is granted to either identity here.
# Whether the planner and the applier need one is a separate owner decision and a
# known follow-up - do not add one silently.
#
# Last thing it prints: the exact values to store as GitHub variables (BOOT AC1
# through AC6 in docs/backlog/T-1116-bootstrap-project-script.md).
set -euo pipefail

PROJECT=""
ENVIRONMENT=""
REPO=""
DRY_RUN=0
ASSUME_YES=0
LOCK_ACK=0

readonly PLAN_POOL="github-plan"
readonly APPLY_POOL="github-apply"
readonly PLAN_PROVIDER="github-plan"
readonly APPLY_PROVIDER="github-apply"
readonly PLANNER_ACCOUNT="mt-planner"
readonly APPLIER_ACCOUNT="mt-applier"
readonly WORKFLOW_PATH=".github/workflows/terraform.yml"
readonly IAM_ADMIN_ROLE="roles/resourcemanager.projectIamAdmin"
readonly WIF_ISSUER="https://token.actions.githubusercontent.com"
readonly STATE_LOCATION="us-central1"
readonly BOOTSTRAP_APIS=(
  serviceusage.googleapis.com
  cloudresourcemanager.googleapis.com
  iam.googleapis.com
  iamcredentials.googleapis.com
  sts.googleapis.com
  storage.googleapis.com
)

usage() {
  cat <<'EOF'
usage: bootstrap-project.sh --project <id> --env staging|production \
         --repo <owner/name> [--dry-run] [--yes] \
         [--i-know-the-log-bucket-lock-is-permanent]
EOF
}

die() {
  printf 'ERROR: %s\n' "$*" >&2
  exit 1
}

# Run one mutating gcloud call. Under --dry-run nothing is ever executed, so a
# later edit cannot make a dry run create something by accident.
apply_call() {
  if [ "$DRY_RUN" -eq 1 ]; then
    return 0
  fi
  if ! gcloud "$@"; then
    die "gcloud $* failed; fix the problem and run this script again - it resumes"
  fi
}

# A read-only probe: success means the resource exists.
probe() {
  gcloud "$@" >/dev/null 2>&1
}

# Planner roles: narrow per-service viewers instead of roles/viewer. Each line is
# "<role>|<resource types it covers>|<note>". Roles that can read data-plane
# contents are marked DATA PLANE and recorded for the reviewer (T-1116 step 6):
# check each one with `gcloud iam roles describe <role>` at bootstrap and read
# the DATA PLANE list again with the plan, because a viewer that can read
# contents is a real answer, not a formality.
planner_role_specs() {
  cat <<'EOF'
roles/cloudkms.viewer|google_kms_key_ring, google_kms_crypto_key|metadata only: cannot decrypt
roles/datastore.viewer|google_firestore_database, google_firestore_field|DATA PLANE: can read documents
roles/run.viewer|google_cloud_run_v2_service|reads service config only
roles/secretmanager.viewer|google_secret_manager_secret|metadata only: cannot read versions
roles/iam.securityReviewer|google_project_iam_member, google_project_iam_audit_config|reads the IAM policy, grants nothing
roles/serviceusage.serviceUsageViewer|google_project_service|cannot enable or disable an API
roles/artifactregistry.reader|google_artifact_registry_repository|DATA PLANE: can pull images
roles/logging.viewer|google_logging_project_bucket_config, google_logging_project_sink, google_logging_project_exclusion|DATA PLANE: can read log entries
roles/cloudtasks.viewer|google_cloud_tasks_queue|reads queue config only
roles/cloudscheduler.viewer|google_cloud_scheduler_job|reads job config only
roles/firebase.viewer|google_firebase_project, google_firebase_hosting_site|reads project and site config only
EOF
}

# Applier roles: one line per role, "<role>|<resource types it covers>|<note>".
# The header table above carries the same mapping for the reviewer; the plan and
# the grants are both printed from this list so the plan cannot drift from what
# the script does.
applier_role_specs() {
  cat <<'EOF'
roles/serviceusage.serviceUsageAdmin|google_project_service|enables the APIs the modules declare
roles/cloudkms.admin|google_kms_key_ring, google_kms_crypto_key, google_kms_crypto_key_iam_binding|creates the keys and binds the crypto roles
roles/datastore.owner|google_firestore_database, google_firestore_field|creates the database and its TTL fields
roles/secretmanager.admin|google_secret_manager_secret, google_secret_manager_secret_iam_binding|containers and accessors only: no value is ever read
roles/iam.serviceAccountAdmin|google_service_account, google_service_account_iam_member|the runtime accounts and mt-deployer, never the planner or the applier
roles/logging.admin|google_logging_project_bucket_config, google_logging_project_sink, google_logging_project_exclusion|the locked log bucket's configuration
roles/artifactregistry.admin|google_artifact_registry_repository, google_artifact_registry_repository_iam_member|the one repository
roles/run.admin|google_cloud_run_v2_service, google_cloud_run_v2_service_iam_member|the three services and their invoker bindings
roles/cloudtasks.admin|google_cloud_tasks_queue, google_cloud_tasks_queue_iam_member|the queue and its enqueuer
roles/cloudscheduler.admin|google_cloud_scheduler_job|the scheduled jobs
roles/firebase.developAdmin|google_firebase_project, google_firebase_hosting_site|enables Firebase and manages the Hosting site
roles/resourcemanager.projectIamAdmin|google_project_iam_member, google_project_iam_audit_config|CONDITIONED: may grant only the roles below
EOF
}

# The roles the two modules bind, which is exactly what the applier's conditional
# projectIamAdmin grant may grant (BOOT AC6). Keep this list equal to the roles
# that appear in infra/terraform/modules/{foundation,runtime}/*.tf; the test
# derives that set from the modules and fails if the two drift.
module_granted_roles() {
  cat <<'EOF'
roles/aiplatform.user
roles/artifactregistry.writer
roles/cloudkms.cryptoKeyEncrypterDecrypter
roles/cloudtasks.enqueuer
roles/cloudtasks.taskDeleter
roles/datastore.user
roles/firebasehosting.admin
roles/iam.serviceAccountUser
roles/iam.workloadIdentityUser
roles/run.developer
roles/run.invoker
roles/secretmanager.secretAccessor
EOF
}

# The applier's roles-limiting conditions, one expression per line. Google's
# documentation caps a hasOnly() list at 10 roles, so a longer list becomes
# several expressions: each binding grants roles/resourcemanager.projectIamAdmin
# to the same member with one slice, and the union of the slices is exactly the
# module role list (bindings are additive).
readonly CONDITION_ROLE_LIMIT=10

roles_limiting_conditions() {
  local role list="" count=0
  while IFS= read -r role; do
    if [ -n "$list" ]; then
      list="${list},"
    fi
    list="${list}\"${role}\""
    count=$((count + 1))
    if [ "$count" -ge "$CONDITION_ROLE_LIMIT" ]; then
      printf "api.getAttribute('iam.googleapis.com/modifiedGrantsByRole', []).hasOnly([%s])\n" "$list"
      list=""
      count=0
    fi
  done < <(module_granted_roles)
  if [ "$count" -gt 0 ]; then
    printf "api.getAttribute('iam.googleapis.com/modifiedGrantsByRole', []).hasOnly([%s])\n" "$list"
  fi
}

plan_provider_condition() {
  printf "assertion.repository == '%s' && assertion.event_name == 'push' && assertion.ref == 'refs/heads/main' && assertion.job_workflow_ref == '%s/%s@refs/heads/main'" \
    "$REPO" "$REPO" "$WORKFLOW_PATH"
}

apply_provider_condition() {
  printf "assertion.repository == '%s' && assertion.event_name == 'push' && assertion.ref == 'refs/heads/main' && assertion.environment == '%s' && assertion.job_workflow_ref == '%s/%s@refs/heads/main'" \
    "$REPO" "$ENVIRONMENT" "$REPO" "$WORKFLOW_PATH"
}

# The plan provider maps the repository only: the planner member keys off
# attribute.repository, and nothing in the plan job needs a ref, an environment
# or a workflow attribute.
plan_provider_mapping() {
  printf 'google.subject=assertion.sub,attribute.repository=assertion.repository'
}

# Only the apply provider maps the environment, the ref and the workflow: the
# applier's principalSet member names the environment attribute and the other two
# are enforced by the attribute_condition.
apply_provider_mapping() {
  printf 'google.subject=assertion.sub,attribute.repository=assertion.repository,attribute.ref=assertion.ref,attribute.environment=assertion.environment,attribute.job_workflow_ref=assertion.job_workflow_ref'
}

print_plan() {
  local role covers note
  printf 'PLAN project %s environment %s\n' "$PROJECT" "$ENVIRONMENT"
  printf '  bootstrap API %s\n' "${BOOTSTRAP_APIS[@]}"
  printf '  state bucket gs://%s-tfstate (location %s, uniform bucket-level access, public access prevention, versioning on)\n' \
    "$PROJECT" "$STATE_LOCATION"
  printf '  workload identity pool %s with provider %s\n' "$PLAN_POOL" "$PLAN_PROVIDER"
  printf '  plan provider condition %s\n' "$(plan_provider_condition)"
  printf '  workload identity pool %s with provider %s\n' "$APPLY_POOL" "$APPLY_PROVIDER"
  printf '  apply provider condition %s\n' "$(apply_provider_condition)"
  printf '  service account %s bound to principalSet github-plan attribute.repository/%s\n' \
    "$PLANNER_ACCOUNT" "$REPO"
  printf '  service account %s bound to principalSet github-apply attribute.environment/%s\n' \
    "$APPLIER_ACCOUNT" "$ENVIRONMENT"
  printf '  state bucket role roles/storage.objectViewer for %s\n' "$PLANNER_ACCOUNT"
  printf '  state bucket role roles/storage.objectAdmin for %s\n' "$APPLIER_ACCOUNT"
  while IFS='|' read -r role covers note; do
    printf '  planner role %s - covers %s - %s\n' "$role" "$covers" "$note"
  done < <(planner_role_specs)
  while IFS='|' read -r role covers note; do
    printf '  applier role %s - covers %s - %s\n' "$role" "$covers" "$note"
  done < <(applier_role_specs)
  printf '  applier project IAM admin condition %s\n' "$(roles_limiting_conditions)"
  printf '  never: service account keys, secret values, Terraform state\n'
}

# Created only when the describe probe says it is missing, so a second run (or a
# run after a partial failure) reports it as existing and changes nothing.
ensure_state_bucket() {
  local bucket="gs://${PROJECT}-tfstate"
  if probe storage buckets describe "$bucket" --project="$PROJECT"; then
    printf '  state bucket %s already exists - unchanged\n' "$bucket"
  else
    apply_call storage buckets create "$bucket" --project="$PROJECT" \
      --location="$STATE_LOCATION" --uniform-bucket-level-access \
      --public-access-prevention
    printf '  created state bucket %s\n' "$bucket"
  fi
  apply_call storage buckets update "$bucket" --project="$PROJECT" --versioning
  apply_call storage buckets update "$bucket" --project="$PROJECT" \
    --uniform-bucket-level-access --public-access-prevention
  apply_call storage buckets add-iam-policy-binding "$bucket" --project="$PROJECT" \
    --member="serviceAccount:${PLANNER_EMAIL}" --role=roles/storage.objectViewer
  apply_call storage buckets add-iam-policy-binding "$bucket" --project="$PROJECT" \
    --member="serviceAccount:${APPLIER_EMAIL}" --role=roles/storage.objectAdmin
}

ensure_pool() {
  local pool="$1" display="$2"
  if probe iam workload-identity-pools describe "$pool" \
    --project="$PROJECT" --location=global; then
    printf '  workload identity pool %s already exists - unchanged\n' "$pool"
  else
    apply_call iam workload-identity-pools create "$pool" \
      --project="$PROJECT" --location=global --display-name="$display"
    printf '  created workload identity pool %s\n' "$pool"
  fi
}

# A provider that already exists is left exactly as it is: its attribute_condition
# is the security boundary, and this script does not silently rewrite it. Changing
# a condition is a deliberate delete-and-recreate, reviewed as such.
ensure_provider() {
  local provider="$1" pool="$2" mapping="$3" condition="$4"
  if probe iam workload-identity-pools providers describe "$provider" \
    --project="$PROJECT" --location=global --workload-identity-pool="$pool"; then
    printf '  provider %s in pool %s already exists - left as is (its attribute_condition is not re-applied)\n' \
      "$provider" "$pool"
  else
    apply_call iam workload-identity-pools providers create-oidc "$provider" \
      --project="$PROJECT" --location=global --workload-identity-pool="$pool" \
      --display-name="GitHub Actions ${pool}" --issuer-uri="$WIF_ISSUER" \
      --attribute-mapping="$mapping" --attribute-condition="$condition"
    printf '  created provider %s in pool %s\n' "$provider" "$pool"
  fi
}

ensure_service_account() {
  local account="$1" display="$2" email="$3"
  if probe iam service-accounts describe "$email" --project="$PROJECT"; then
    printf '  service account %s already exists - unchanged\n' "$account"
  else
    apply_call iam service-accounts create "$account" \
      --project="$PROJECT" --display-name="$display"
    printf '  created service account %s\n' "$account"
  fi
}

bind_project_role() {
  local member="$1" role="$2"
  apply_call projects add-iam-policy-binding "$PROJECT" \
    --member="$member" --role="$role" --condition=None
}

bind_service_account_role() {
  local email="$1" member="$2" role="$3"
  apply_call iam service-accounts add-iam-policy-binding "$email" \
    --project="$PROJECT" --member="$member" --role="$role" --condition=None
}

# The last thing the owner sees: exactly the values T-1117's workflow reads.
print_github_variables() {
  local env_suffix
  env_suffix="$(printf '%s' "$ENVIRONMENT" | tr '[:lower:]' '[:upper:]')"
  cat <<EOF

Store these as GitHub variables (Settings -> Secrets and variables -> Actions).

Repository variables - the post-merge plan job on main has no environment, so it
cannot see an environment-scoped variable:
  GCP_WIF_PLAN_PROVIDER_${env_suffix}=projects/${PROJECT_NUMBER}/locations/global/workloadIdentityPools/${PLAN_POOL}/providers/${PLAN_PROVIDER}
  GCP_PLANNER_SA_${env_suffix}=${PLANNER_EMAIL}
  TF_STATE_BUCKET_${env_suffix}=${PROJECT}-tfstate
  GCP_PROJECT_ID_${env_suffix}=${PROJECT}

Environment variables - in the '${ENVIRONMENT}' environment, for the apply job:
  GCP_WIF_APPLY_PROVIDER=projects/${PROJECT_NUMBER}/locations/global/workloadIdentityPools/${APPLY_POOL}/providers/${APPLY_PROVIDER}
  GCP_APPLIER_SA=${APPLIER_EMAIL}

Repository variable reminder (ADR 0003 item 7): set TF_APPLY_ENABLED=false now.
Set it to true only after 'Require review from Code Owners' is on for main with
CODEOWNERS covering .github/workflows/**, infra/** and scripts/infra/**, so no
plan can apply until then.
EOF
}

while [ "$#" -gt 0 ]; do
  case "$1" in
    --project)
      [ "$#" -ge 2 ] || die "--project needs a value"
      PROJECT="$2"
      shift 2
      ;;
    --env)
      [ "$#" -ge 2 ] || die "--env needs a value"
      ENVIRONMENT="$2"
      shift 2
      ;;
    --repo)
      [ "$#" -ge 2 ] || die "--repo needs a value"
      REPO="$2"
      shift 2
      ;;
    --dry-run)
      DRY_RUN=1
      shift
      ;;
    --yes)
      ASSUME_YES=1
      shift
      ;;
    --i-know-the-log-bucket-lock-is-permanent)
      LOCK_ACK=1
      shift
      ;;
    -h | --help)
      usage
      exit 0
      ;;
    *)
      die "unknown argument: $1 (see --help)"
      ;;
  esac
done

[ -n "$PROJECT" ] || die "--project is required"
[ -n "$REPO" ] || die "--repo is required"
case "$ENVIRONMENT" in
  staging | production) ;;
  *) die "--env must be staging or production, not '${ENVIRONMENT}'" ;;
esac
case "$REPO" in
  */*) ;;
  *) die "--repo must be owner/name, not '${REPO}'" ;;
esac

# Refuse to run unsafely, before anything is created (BOOT AC3). Production's log
# bucket lock is permanent, so the owner has to say so in the command itself.
if [ "$ENVIRONMENT" = "production" ] && [ "$LOCK_ACK" -eq 0 ]; then
  die "production needs --i-know-the-log-bucket-lock-is-permanent: the log bucket lock cannot be undone"
fi

# 1. The active account must be a human user. A service account here would mean
#    an automated credential is creating the identities that later hold the
#    pipeline's power.
account="$(gcloud auth list --filter=status:ACTIVE --format='value(account)')"
if [ -z "$account" ]; then
  die "no active gcloud account: run 'gcloud auth login' with your own user account"
fi
case "$account" in
  *gserviceaccount.com)
    die "the active gcloud account is a service account; bootstrap must run as a human user (gcloud auth login)"
    ;;
esac

# 2. The project must exist and billing must be linked. Linking billing needs a
#    billing admin and is not scripted - the console link is the handover.
if ! PROJECT_NUMBER="$(gcloud projects describe "$PROJECT" --format='value(projectNumber)')"; then
  die "project ${PROJECT} does not exist or is not visible to the active account"
fi
billing="$(gcloud billing projects describe "$PROJECT" --format='value(billingEnabled)' 2>/dev/null || true)"
if [ "$billing" != "True" ]; then
  printf 'Billing is not linked to %s.\n' "$PROJECT"
  printf 'Link it first (needs a billing admin): https://console.cloud.google.com/billing/linkedaccount?project=%s\n' \
    "$PROJECT"
  die "billing is not linked: add the billing account in the console and run this script again"
fi

PLANNER_EMAIL="${PLANNER_ACCOUNT}@${PROJECT}.iam.gserviceaccount.com"
APPLIER_EMAIL="${APPLIER_ACCOUNT}@${PROJECT}.iam.gserviceaccount.com"
PLANNER_PRINCIPAL="principalSet://iam.googleapis.com/projects/${PROJECT_NUMBER}/locations/global/workloadIdentityPools/${PLAN_POOL}/attribute.repository/${REPO}"
APPLIER_PRINCIPAL="principalSet://iam.googleapis.com/projects/${PROJECT_NUMBER}/locations/global/workloadIdentityPools/${APPLY_POOL}/attribute.environment/${ENVIRONMENT}"

# 3. Print the plan, then wait for the operator (BOOT AC1, AC2).
print_plan
if [ "$DRY_RUN" -eq 1 ]; then
  printf '\ndry run: nothing was created and nothing was changed.\n'
  print_github_variables
  exit 0
fi
if [ "$ASSUME_YES" -eq 0 ]; then
  printf '\nType y to create this in %s: ' "$PROJECT"
  answer=""
  read -r answer || answer=""
  if [ "$answer" != "y" ]; then
    die "aborted by the operator: nothing was created"
  fi
fi

# 4. Enable only the APIs Terraform needs to start (modules/foundation enables
#    the rest).
apply_call services enable "${BOOTSTRAP_APIS[@]}" --project="$PROJECT"
printf '  enabled the bootstrap APIs\n'

# 5. State bucket, pools, providers, service accounts (each guarded by a probe).
ensure_state_bucket
ensure_pool "$PLAN_POOL" "GitHub Actions plan (read-only)"
ensure_provider "$PLAN_PROVIDER" "$PLAN_POOL" "$(plan_provider_mapping)" "$(plan_provider_condition)"
ensure_pool "$APPLY_POOL" "GitHub Actions apply"
ensure_provider "$APPLY_PROVIDER" "$APPLY_POOL" "$(apply_provider_mapping)" "$(apply_provider_condition)"
ensure_service_account "$PLANNER_ACCOUNT" "Terraform planner (Workload Identity Federation only; no key)" "$PLANNER_EMAIL"
ensure_service_account "$APPLIER_ACCOUNT" "Terraform applier (Workload Identity Federation only; no key)" "$APPLIER_EMAIL"

# 6. Impersonation bindings. The planner member names the plan pool's repository
#    attribute; the applier member names the apply pool's environment attribute
#    and NEVER the repository (BOOT AC5).
bind_service_account_role "$PLANNER_EMAIL" "$PLANNER_PRINCIPAL" roles/iam.workloadIdentityUser
bind_service_account_role "$APPLIER_EMAIL" "$APPLIER_PRINCIPAL" roles/iam.workloadIdentityUser

# 7. Project roles: narrow viewers for the planner, the module-derived list for
#    the applier, and the conditioned IAM-admin grants (BOOT AC6). The IAM-admin
#    role is skipped in the loop on purpose: it is granted only with a condition,
#    once per slice of the module role list.
while IFS='|' read -r role _covers _note; do
  bind_project_role "serviceAccount:${PLANNER_EMAIL}" "$role"
done < <(planner_role_specs)
while IFS='|' read -r role _covers _note; do
  if [ "$role" = "$IAM_ADMIN_ROLE" ]; then
    continue
  fi
  bind_project_role "serviceAccount:${APPLIER_EMAIL}" "$role"
done < <(applier_role_specs)
condition_index=0
while IFS= read -r condition; do
  condition_index=$((condition_index + 1))
  apply_call projects add-iam-policy-binding "$PROJECT" \
    --member="serviceAccount:${APPLIER_EMAIL}" \
    --role="$IAM_ADMIN_ROLE" \
    --condition="expression=${condition},title=modules-only-role-allowlist-${condition_index},description=The applier may grant only the roles modules/foundation and modules/runtime bind (T-1116 BOOT AC6, part ${condition_index})"
done < <(roles_limiting_conditions)

printf '\nbootstrap complete for %s (environment %s).\n' "$PROJECT" "$ENVIRONMENT"
print_github_variables
