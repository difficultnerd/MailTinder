#!/usr/bin/env bash
# T-1116 (BOOT AC1-AC6): bootstrap ONE environment so that every later
# infrastructure change flows through the plan/apply pipeline (T-1117) and the
# owner only reads a summary and approves (ADR 0003).
#
# Usage:
#   scripts/infra/bootstrap-project.sh --project <workload project> \
#     --identity-project <identity project> --env staging|production \
#     --repo <owner/name> [--dry-run] [--yes] \
#     [--i-know-the-log-bucket-lock-is-permanent]
#
#   --dry-run   print the plan and stop: nothing is created, nothing is changed
#   --yes       skip the interactive confirmation (for a re-run after a partial
#               failure; the human user check and the billing checks still run)
#
# TWO projects per environment, and why
# -------------------------------------
# The workload project (--project) holds the state bucket, the resources the two
# Terraform modules manage, and the roles the planner and the applier hold.
# The identity project (--identity-project) holds the two Workload Identity
# pools, their providers and the `mt-planner` and `mt-applier` service accounts.
#
# The applier holds no role of any kind in the identity project. That is the
# bootstrap-identity boundary of ADR 0003 item 5 and T-1116 step 6: an admitted
# apply job cannot rewrite the impersonation policy that admits it, cannot
# disable or delete the planner or the applier, cannot mint a key for either, and
# cannot edit the apply provider's attribute_condition.
#
# In one project that boundary cannot be expressed, which is why the identities
# live elsewhere:
#   - IAM Conditions cannot name service accounts. `resource.name` is not
#     available for IAM resources ("If part of a condition uses an attribute that
#     isn't available for a resource, then that part of the condition is never
#     interpreted as granting access" - IAM Conditions attribute reference), and
#     the resource types that accept `iam.googleapis.com/modifiedGrantsByRole`
#     are projects, folders and organizations only. So a project-level
#     `iam.serviceAccounts.setIamPolicy` is all-or-nothing: it reaches every
#     service account in the project, the two bootstrap identities included.
#   - The modules' `google_service_account_iam_member` resources do need
#     `iam.serviceAccounts.setIamPolicy` on runtime accounts, so the applier must
#     hold it project-wide in the workload project; per-account grants are not
#     possible beforehand because those accounts only exist after the first
#     apply. Only the project boundary is left.
#
# It creates exactly what Terraform cannot create for itself:
#   - in the workload project: the six APIs Terraform needs to start
#     (serviceusage, cloudresourcemanager, iam, iamcredentials, sts, storage;
#     modules/foundation/apis.tf enables the rest) and the state bucket
#     gs://<project>-tfstate: us-central1, uniform bucket-level access, public
#     access prevention, versioning on.
#   - in the identity project: the five APIs the identities need, two Workload
#     Identity pools (`github-plan`, `github-apply`) with one provider each, and
#     the two service accounts with their impersonation bindings.
#
# It never creates a service account key, never reads a secret value and never
# touches Terraform state. It is idempotent: a second run creates nothing (every
# creating call is guarded by a describe probe) and a run after a partial failure
# resumes where it stopped. The grants are re-applied with
# `add-iam-policy-binding`, which is a set operation on the API.
#
# Before it grants anything it reads back what exists and fails closed (SEC-06):
#   - every provider in each pool: the pool must hold exactly its one provider,
#     with exactly the intended issuer, attribute mapping and
#     attribute_condition. A mismatch is not silently accepted: delete and
#     recreate the provider deliberately, then run again.
#   - the two service accounts' IAM policies: they must contain nothing but the
#     workloadIdentityUser binding this script installs.
#   - the state bucket: uniform bucket-level access, public access prevention
#     and versioning must be on.
#
# The two identities, and the role tables a reviewer checks (BOOT AC5, AC6)
# -------------------------------------------------------------------------
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
# Both conditions pin `assertion.workflow_ref`. GitHub documents
# `job_workflow_ref` for jobs that execute a reusable workflow ("For jobs using a
# reusable workflow, the ref path to the reusable workflow"); the plan and apply
# jobs are ordinary jobs in the push-triggered workflow, and `workflow_ref` ("The
# ref path to the workflow") is the claim they carry (SEC-08).
#
# mt-applier in the workload project. Each line is "<role>|<covers>|<note>"; the
# custom roles are the resource-operation-to-permission matrix: every permission
# is listed, nothing is inherited from a broader predefined role.
#
#   roles/serviceusage.serviceUsageAdmin      google_project_service
#   roles/cloudkms.admin                      google_kms_key_ring,
#                                             google_kms_crypto_key,
#                                             google_kms_crypto_key_iam_binding
#   roles/datastore.owner                     google_firestore_database,
#                                             google_firestore_field
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
#   projects/<project>/roles/mt_applier_service_accounts_create
#                                             google_service_account (create only)
#   projects/<project>/roles/mt_applier_service_account_policies
#                                             google_service_account_iam_member
#                                             CONDITIONED: may grant only the two
#                                             impersonation roles
#   projects/<project>/roles/mt_applier_secret_containers
#                                             google_secret_manager_secret,
#                                             google_secret_manager_secret_iam_binding
#                                             container metadata and the accessor
#                                             binding - no secret version permission
#                                             at all, so no value is readable
#                                             directly
#   projects/<project>/roles/mt_applier_firebase
#                                             google_firebase_project,
#                                             google_firebase_hosting_site
#                                             Firebase project link and the Hosting
#                                             site only - no storage bucket
#                                             permission (SEC-03)
#   projects/<project>/roles/mt_applier_workload_identity
#                                             google_iam_workload_identity_pool,
#                                             google_iam_workload_identity_pool_provider
#                                             the runtime module's own pool and
#                                             provider, without IAM control of a pool
#                                             (SEC-07)
#   roles/resourcemanager.projectIamAdmin     google_project_iam_member,
#                                             google_project_iam_audit_config
#                                             CONDITIONED, see below
#
# Deliberately absent from the applier's direct grants, each leaving a capability
# the reviewer would otherwise have to assume (SEC-01, SEC-03, SEC-04, SEC-07):
#   - `iam.serviceAccounts.delete`, `.disable`, `.enable`, `.undelete` and any
#     `iam.serviceAccountKeys.*`/`iam.serviceAccountApiKeyBindings.*`: it cannot
#     destroy or disable an identity and cannot create a key for one.
#   - `secretmanager.versions.*`: it cannot read, add or destroy a secret value.
#   - anything under `storage.`: it cannot administer a bucket, disable the state
#     bucket's versioning, change its IAM or delete it.
#   - `iam.googleapis.com/workloadIdentityPools.setIamPolicy`,
#     `.getIamPolicy`, the same for providers, and `.delete`/`.undelete` on the
#     bootstrap pools is unreachable anyway because they are in the identity
#     project.
#
# Blast radius, stated honestly (S10 10.4, ADR 0003 item 4):
#   - `roles/datastore.owner` can read Firestore documents, and the secret custom
#     role can set the accessor binding on a secret it manages, so the applier
#     reaches secret values *transitively*: it can grant `secretmanager.
#     secretAccessor` on a container it created to a member of its choosing (for
#     example itself) and then read the version. V13.2.2 is assessed on that
#     basis: the applier's direct grants carry no version permission, and the
#     control that keeps the values out of an unapproved run is the GitHub
#     environment approval plus the roles-limiting condition on the binding
#     grant, not the absence of a permission.
#   - in the workload project it can create a service account and a pool, so it
#     can mint a credential for its own workload roles. It cannot use that to
#     reach the identity project, and the second apply would show the plan.
#   - `objectAdmin` on the state bucket is real: it can rewrite state, which is
#     what applying means. The modules' protected resources are guarded at plan
#     time (ADR 0003 item 5).
#
# The project-level grant of roles/resourcemanager.projectIamAdmin carries IAM
# Conditions that limit the roles it may grant to exactly the roles the two
# modules bind *at project scope* (BOOT AC6). Google's IAM Conditions
# documentation, "Set limits on granting roles" (verified 12 Oct 2026):
#   - `iam.googleapis.com/modifiedGrantsByRole` is the documented API attribute
#     for limiting role granting; the resource types that accept it are projects,
#     folders and organizations;
#   - a `hasOnly([...])` list may hold at most 10 roles and must contain only
#     string constants, so a longer list becomes several conditioned bindings on
#     the same member and role (bindings are additive). The roles are never moved
#     into an unconditioned binding, and `hasOnly()` lists are never joined with
#     `||`, which that page explicitly warns against;
#   - roles that can grant roles themselves (anything with `setIamPolicy`) must
#     not appear in a list, or the applier could widen itself;
#   - `roles/iam.workloadIdentityUser` and `roles/iam.serviceAccountUser` are
#     deliberately NOT in the list (SEC-02). A project-wide impersonation grant
#     would let the applier admit the plan pool's principalSet to mt-applier, or
#     admit an arbitrary member to actAs on runtime identities, and the API
#     attribute restricts role names, not members or the target resource. Those
#     two roles are bound only on the service account resources that need them
#     (modules/runtime/deploy_identity.tf, modules/runtime/tasks.tf), through the
#     conditioned mt_applier_service_account_policies role above.
#
# The two IAM conditions this script writes are passed with
# `--condition-from-file`, never with `--condition=expression=...,title=...`
# (SEC-05): gcloud parses that dictionary flag with a comma delimiter, and both
# expressions contain commas, so the inline form raises ArgumentTypeError before
# any request is made and the grants would never be installed.
#
# Never granted to anyone by this script: roles/owner, roles/editor,
# roles/iam.securityAdmin, roles/iam.serviceAccountTokenCreator,
# roles/iam.serviceAccountKeyAdmin, roles/iam.workloadIdentityPoolAdmin.
#
# Deliberately not covered: modules/monitoring is named by neither role table in
# T-1116 step 6, so no monitoring role is granted to either identity here.
# Whether the planner and the applier need one is a separate owner decision and a
# known follow-up - do not add one silently.
#
# Last thing it prints: the exact values to store as GitHub variables.
set -euo pipefail

PROJECT=""
IDENTITY_PROJECT=""
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
# The identity project needs no storage API: it holds no bucket.
readonly BOOTSTRAP_APIS_IDENTITY=(
  serviceusage.googleapis.com
  cloudresourcemanager.googleapis.com
  iam.googleapis.com
  iamcredentials.googleapis.com
  sts.googleapis.com
)

# One run's scratch space for the condition files (SEC-05).
RUN_TMP=""
cleanup() {
  if [ -n "$RUN_TMP" ] && [ -d "$RUN_TMP" ]; then
    rm -rf "$RUN_TMP"
  fi
}
trap cleanup EXIT

usage() {
  cat <<'EOF'
usage: bootstrap-project.sh --project <workload project> \
         --identity-project <identity project> --env staging|production \
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

# A read-only call whose value is needed; never fatal here, the caller decides.
read_value() {
  gcloud "$@" 2>/dev/null || true
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

# The applier's predefined project roles, "<role>|<resource types>|<note>". The
# module-derived custom roles are in custom_role_specs below; the plan and the
# grants are both printed from these lists so the plan cannot drift from what the
# script does.
applier_role_specs() {
  cat <<'EOF'
roles/serviceusage.serviceUsageAdmin|google_project_service|enables the APIs the modules declare
roles/cloudkms.admin|google_kms_key_ring, google_kms_crypto_key, google_kms_crypto_key_iam_binding|creates the keys and binds the crypto roles
roles/datastore.owner|google_firestore_database, google_firestore_field|creates the database and its TTL fields
roles/logging.admin|google_logging_project_bucket_config, google_logging_project_sink, google_logging_project_exclusion|the locked log bucket's configuration
roles/artifactregistry.admin|google_artifact_registry_repository, google_artifact_registry_repository_iam_member|the one repository
roles/run.admin|google_cloud_run_v2_service, google_cloud_run_v2_service_iam_member|the three services and their invoker bindings
roles/cloudtasks.admin|google_cloud_tasks_queue, google_cloud_tasks_queue_iam_member|the queue and its enqueuer
roles/cloudscheduler.admin|google_cloud_scheduler_job|the scheduled jobs
roles/resourcemanager.projectIamAdmin|google_project_iam_member, google_project_iam_audit_config|CONDITIONED: may grant only the roles below
EOF
}

# The applier's custom project roles: "<role id>|<permissions, comma separated>|
# <why>". The permission list is the whole grant - no predefined role is
# inherited - and the test suite asserts that each list keeps out the
# permissions SEC-01, SEC-03, SEC-04 and SEC-07 name.
custom_role_specs() {
  cat <<'EOF'
mt_applier_service_accounts_create|iam.serviceAccounts.create,iam.serviceAccounts.get,iam.serviceAccounts.list,iam.serviceAccounts.update|google_service_account: create the runtime identities, nothing else - no delete, no disable, no actAs, no key
mt_applier_service_account_policies|iam.serviceAccounts.get,iam.serviceAccounts.list,iam.serviceAccounts.getIamPolicy,iam.serviceAccounts.setIamPolicy|google_service_account_iam_member on runtime accounts: CONDITIONED, may grant only the two impersonation roles
mt_applier_secret_containers|secretmanager.secrets.create,secretmanager.secrets.get,secretmanager.secrets.list,secretmanager.secrets.update,secretmanager.secrets.delete,secretmanager.secrets.getIamPolicy,secretmanager.secrets.setIamPolicy|google_secret_manager_secret and its iam_binding: containers only, no secretmanager.versions.* permission at all
mt_applier_firebase|firebase.projects.get,firebase.projects.update,firebasehosting.sites.create,firebasehosting.sites.get,firebasehosting.sites.list,firebasehosting.sites.update,firebasehosting.sites.delete|google_firebase_project and google_firebase_hosting_site: the project link and the one Hosting site, no storage.buckets.* and no storage.objects.*
mt_applier_workload_identity|iam.googleapis.com/workloadIdentityPools.create,iam.googleapis.com/workloadIdentityPools.get,iam.googleapis.com/workloadIdentityPools.list,iam.googleapis.com/workloadIdentityPools.delete,iam.googleapis.com/workloadIdentityPools.undelete,iam.googleapis.com/workloadIdentityPools.update,iam.googleapis.com/workloadIdentityPoolProviders.create,iam.googleapis.com/workloadIdentityPoolProviders.get,iam.googleapis.com/workloadIdentityPoolProviders.list,iam.googleapis.com/workloadIdentityPoolProviders.delete,iam.googleapis.com/workloadIdentityPoolProviders.undelete,iam.googleapis.com/workloadIdentityPoolProviders.update|the runtime module's own pool and provider, without setIamPolicy/getIamPolicy on a pool or provider
EOF
}

# The last line of that table is the only grant that can create a pool; it
# carries no setIamPolicy, so the applier cannot rewrite an admission rule
# through it - and the bootstrap pools are in the identity project, out of its
# reach entirely.

# The roles the two modules bind, which is what the applier's conditional
# projectIamAdmin grant may grant *at project scope* (BOOT AC6). Keep this list
# equal to the roles that appear in infra/terraform/modules/{foundation,runtime}
# /*.tf; the test derives that set from the modules and fails if the two drift.
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

# Impersonation and actAs. The modules bind these two on individual service
# accounts, never on the project, so the applier may bind them too - but only on
# a service account, through mt_applier_service_account_policies, and never at
# project scope (SEC-02). The two lists below are derived from this one, so the
# allowlist and the SA condition can never disagree.
resource_scoped_roles() {
  cat <<'EOF'
roles/iam.serviceAccountUser
roles/iam.workloadIdentityUser
EOF
}

# The roles the applier may grant *at project scope*: the module roles that are
# really project-scoped. Used by the allowlist condition and by the plan.
project_scope_grantable_roles() {
  local role
  while IFS= read -r role; do
    if ! resource_scoped_roles | grep -Fxq -- "$role"; then
      printf '%s\n' "$role"
    fi
  done < <(module_granted_roles)
}

# The applier's roles-limiting conditions, one expression per line. Google's
# documentation caps a hasOnly() list at 10 roles, so a longer list becomes
# several expressions: each binding grants roles/resourcemanager.projectIamAdmin
# to the same member with one slice, and the union of the slices is exactly the
# project-scope grantable role list (bindings are additive).
readonly CONDITION_ROLE_LIMIT=10

roles_limiting_conditions() {
  local role list="" count=0
  while IFS= read -r role; do
    if [ -n "$list" ]; then
      list="${list},"
    fi
    list="${list}'${role}'"
    count=$((count + 1))
    if [ "$count" -ge "$CONDITION_ROLE_LIMIT" ]; then
      printf "api.getAttribute('iam.googleapis.com/modifiedGrantsByRole', []).hasOnly([%s])\n" "$list"
      list=""
      count=0
    fi
  done < <(project_scope_grantable_roles)
  if [ "$count" -gt 0 ]; then
    printf "api.getAttribute('iam.googleapis.com/modifiedGrantsByRole', []).hasOnly([%s])\n" "$list"
  fi
}

# The applier may modify a service account's IAM policy, but only to bind the two
# impersonation roles the modules use. Any other role (a key admin, an
# owner-ish role) fails the request.
service_account_policy_condition() {
  local role list="" first=1
  while IFS= read -r role; do
    if [ "$first" -eq 1 ]; then
      first=0
    else
      list="${list}, "
    fi
    list="${list}'${role}'"
  done < <(resource_scoped_roles)
  printf "api.getAttribute('iam.googleapis.com/modifiedGrantsByRole', []).hasOnly([%s])" "$list"
}

plan_provider_condition() {
  printf "assertion.repository == '%s' && assertion.event_name == 'push' && assertion.ref == 'refs/heads/main' && assertion.workflow_ref == '%s/%s@refs/heads/main'" \
    "$REPO" "$REPO" "$WORKFLOW_PATH"
}

apply_provider_condition() {
  printf "assertion.repository == '%s' && assertion.event_name == 'push' && assertion.ref == 'refs/heads/main' && assertion.environment == '%s' && assertion.workflow_ref == '%s/%s@refs/heads/main'" \
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
  printf 'google.subject=assertion.sub,attribute.repository=assertion.repository,attribute.ref=assertion.ref,attribute.environment=assertion.environment,attribute.workflow_ref=assertion.workflow_ref'
}

print_plan() {
  local role covers note perms why
  printf 'PLAN workload project %s, identity project %s, environment %s\n' \
    "$PROJECT" "$IDENTITY_PROJECT" "$ENVIRONMENT"
  printf '  bootstrap API in %s: %s\n' "$PROJECT" "${BOOTSTRAP_APIS[*]}"
  printf '  bootstrap API in %s: %s\n' "$IDENTITY_PROJECT" "${BOOTSTRAP_APIS_IDENTITY[*]}"
  printf '  state bucket gs://%s-tfstate (location %s, uniform bucket-level access, public access prevention, versioning on)\n' \
    "$PROJECT" "$STATE_LOCATION"
  printf '  workload identity pool %s with provider %s in %s\n' "$PLAN_POOL" "$PLAN_PROVIDER" "$IDENTITY_PROJECT"
  printf '  plan provider condition %s\n' "$(plan_provider_condition)"
  printf '  workload identity pool %s with provider %s in %s\n' "$APPLY_POOL" "$APPLY_PROVIDER" "$IDENTITY_PROJECT"
  printf '  apply provider condition %s\n' "$(apply_provider_condition)"
  printf '  service account %s in %s bound to principalSet github-plan attribute.repository/%s\n' \
    "$PLANNER_ACCOUNT" "$IDENTITY_PROJECT" "$REPO"
  printf '  service account %s in %s bound to principalSet github-apply attribute.environment/%s\n' \
    "$APPLIER_ACCOUNT" "$IDENTITY_PROJECT" "$ENVIRONMENT"
  printf '  the applier is granted no role at all in %s\n' "$IDENTITY_PROJECT"
  printf '  state bucket role roles/storage.objectViewer for %s\n' "$PLANNER_ACCOUNT"
  printf '  state bucket role roles/storage.objectAdmin for %s\n' "$APPLIER_ACCOUNT"
  while IFS='|' read -r role covers note; do
    printf '  planner role %s - covers %s - %s\n' "$role" "$covers" "$note"
  done < <(planner_role_specs)
  while IFS='|' read -r role covers note; do
    printf '  applier role %s - covers %s - %s\n' "$role" "$covers" "$note"
  done < <(applier_role_specs)
  while IFS='|' read -r role perms why; do
    printf '  applier role projects/%s/roles/%s - permissions %s - %s\n' \
      "$PROJECT" "$role" "$perms" "$why"
  done < <(custom_role_specs | grep -v '^#' | grep -v '^$')
  printf '  applier service account policy condition %s\n' "$(service_account_policy_condition)"
  printf '  applier project IAM admin condition %s\n' "$(roles_limiting_conditions)"
  printf '  verified before any grant: each pool holds exactly its one provider, with the intended issuer, mapping and condition; the two service accounts carry nothing but their workloadIdentityUser binding; the state bucket has uniform access, public access prevention and versioning\n'
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
}

# The bucket's security configuration is read back, not assumed (SEC-06): a
# bucket that someone turned into non-uniform access, or with versioning off,
# stops the run instead of being trusted.
verify_state_bucket() {
  local bucket="gs://${PROJECT}-tfstate" uniform pap versioning
  uniform="$(read_value storage buckets describe "$bucket" --project="$PROJECT" \
    --format='value(iamConfiguration.uniformBucketLevelAccess.enabled)')"
  pap="$(read_value storage buckets describe "$bucket" --project="$PROJECT" \
    --format='value(iamConfiguration.publicAccessPrevention)')"
  versioning="$(read_value storage buckets describe "$bucket" --project="$PROJECT" \
    --format='value(versioning.enabled)')"
  if [ "$(printf '%s' "$uniform" | tr '[:upper:]' '[:lower:]')" != "true" ]; then
    die "state bucket ${bucket} does not have uniform bucket-level access (read '${uniform}'); fix it and run again"
  fi
  if [ "$(printf '%s' "$pap" | tr '[:upper:]' '[:lower:]')" != "enforced" ]; then
    die "state bucket ${bucket} does not enforce public access prevention (read '${pap}'); fix it and run again"
  fi
  if [ "$(printf '%s' "$versioning" | tr '[:upper:]' '[:lower:]')" != "true" ]; then
    die "state bucket ${bucket} does not have versioning on (read '${versioning}'); fix it and run again"
  fi
  printf '  state bucket %s verified: uniform access, public access prevention, versioning on\n' "$bucket"
}

# The two object roles the pipeline needs on the state bucket - the planner reads
# state, the applier writes it - and nothing else on that bucket (SEC-03).
grant_state_bucket_roles() {
  local bucket="gs://${PROJECT}-tfstate"
  apply_call storage buckets add-iam-policy-binding "$bucket" --project="$PROJECT" \
    --member="serviceAccount:${PLANNER_EMAIL}" --role=roles/storage.objectViewer
  apply_call storage buckets add-iam-policy-binding "$bucket" --project="$PROJECT" \
    --member="serviceAccount:${APPLIER_EMAIL}" --role=roles/storage.objectAdmin
}

ensure_pool() {
  local pool="$1" display="$2"
  if probe iam workload-identity-pools describe "$pool" \
    --project="$IDENTITY_PROJECT" --location=global; then
    printf '  workload identity pool %s already exists - unchanged\n' "$pool"
  else
    apply_call iam workload-identity-pools create "$pool" \
      --project="$IDENTITY_PROJECT" --location=global --display-name="$display"
    printf '  created workload identity pool %s\n' "$pool"
  fi
}

# A provider that already exists is never silently rewritten: its
# attribute_condition is the security boundary, so a mismatch fails the run
# (SEC-06) and the owner deletes and recreates it deliberately.
ensure_provider() {
  local provider="$1" pool="$2" mapping="$3" condition="$4"
  if probe iam workload-identity-pools providers describe "$provider" \
    --project="$IDENTITY_PROJECT" --location=global --workload-identity-pool="$pool"; then
    printf '  provider %s in pool %s already exists - will be verified\n' "$provider" "$pool"
  else
    apply_call iam workload-identity-pools providers create-oidc "$provider" \
      --project="$IDENTITY_PROJECT" --location=global --workload-identity-pool="$pool" \
      --display-name="GitHub Actions ${pool}" --issuer-uri="$WIF_ISSUER" \
      --attribute-mapping="$mapping" --attribute-condition="$condition"
    printf '  created provider %s in pool %s\n' "$provider" "$pool"
  fi
}

# What is actually installed, read back before a single privilege is granted
# (SEC-06). A pool is scoped as a principalSet, so an extra provider in it is an
# extra admission path: the pool must hold exactly its one provider.
verify_provider() {
  local pool="$1" provider="$2" mapping="$3" condition="$4"
  local -a pairs=()
  local found issuer actual_condition actual_mapping pair key value
  found="$(read_value iam workload-identity-pools providers list \
    --project="$IDENTITY_PROJECT" --location=global --workload-identity-pool="$pool" \
    --format='value(name.basename())' | awk 'NF' | sort | tr '\n' ' ')"
  if [ "$found" != "${provider} " ]; then
    die "pool ${pool} in ${IDENTITY_PROJECT} holds '${found}' - expected only '${provider}'. Every provider in a pool is an admission path for its principalSet; delete the extra provider deliberately and run again"
  fi
  issuer="$(read_value iam workload-identity-pools providers describe "$provider" \
    --project="$IDENTITY_PROJECT" --location=global --workload-identity-pool="$pool" \
    --format='value(oidc.issuerUri)')"
  if [ "$issuer" != "$WIF_ISSUER" ]; then
    die "provider ${provider} in pool ${pool} has issuer '${issuer}', not '${WIF_ISSUER}'; delete and recreate it deliberately and run again"
  fi
  actual_condition="$(read_value iam workload-identity-pools providers describe "$provider" \
    --project="$IDENTITY_PROJECT" --location=global --workload-identity-pool="$pool" \
    --format='value(attributeCondition)')"
  if [ "$actual_condition" != "$condition" ]; then
    die "provider ${provider} in pool ${pool} carries a different attribute_condition:
  installed: ${actual_condition}
  intended:  ${condition}
Delete and recreate the provider deliberately (this is the admission gate), then run again"
  fi
  actual_mapping="$(read_value iam workload-identity-pools providers describe "$provider" \
    --project="$IDENTITY_PROJECT" --location=global --workload-identity-pool="$pool" \
    --format='value(attributeMapping)')"
  IFS=',' read -r -a pairs <<<"$mapping"
  for pair in "${pairs[@]}"; do
    key="${pair%%=*}"
    value="${pair#*=}"
    # "key: value" (YAML), "key=value" (the argument) and "key": "value" (JSON)
    # all satisfy this: the key and its own value, in one pair. Dots in a claim
    # name are escaped so they match a literal dot, not any character.
    if ! printf '%s\n' "$actual_mapping" |
      grep -Eq -- "$(printf '%s' "$key" | sed 's/\./\\./g')['\"]?[[:space:]]*[:=][[:space:]]*['\"]?$(printf '%s' "$value" | sed 's/\./\\./g')['\"]?([,[:space:]]|$)"; then
      die "provider ${provider} in pool ${pool} does not map ${key}=${value}; its attribute mapping is '${actual_mapping}'. Fix it deliberately and run again"
    fi
  done
  printf '  provider %s in pool %s verified: issuer, mapping and attribute_condition are the intended ones\n' \
    "$provider" "$pool"
}

ensure_service_account() {
  local account="$1" display="$2" email="$3"
  if probe iam service-accounts describe "$email" --project="$IDENTITY_PROJECT"; then
    printf '  service account %s already exists - unchanged\n' "$account"
  else
    apply_call iam service-accounts create "$account" \
      --project="$IDENTITY_PROJECT" --display-name="$display"
    printf '  created service account %s\n' "$account"
  fi
}

# A bootstrap identity must carry nothing but the one impersonation binding this
# script installs: another member on it would be a second way in, and that is
# exactly what the applier cannot be trusted to leave alone (ADR 0003 item 5).
verify_service_account_policy() {
  local email="$1" member="$2" roles members
  roles="$(read_value iam service-accounts get-iam-policy "$email" \
    --project="$IDENTITY_PROJECT" --format='value(bindings.role)' | awk 'NF' | sort | tr '\n' ' ')"
  members="$(read_value iam service-accounts get-iam-policy "$email" \
    --project="$IDENTITY_PROJECT" --format='value(bindings.members)' | awk 'NF' | sort | tr '\n' ' ')"
  if [ -z "$roles" ] && [ -z "$members" ]; then
    printf '  service account %s carries no IAM binding yet - unchanged\n' "$email"
    return 0
  fi
  if [ "$roles" != "roles/iam.workloadIdentityUser " ] || [ "$members" != "${member} " ]; then
    die "service account ${email} carries bindings this script did not install:
  roles:   ${roles}
  members: ${members}
Expected: roles/iam.workloadIdentityUser for ${member} only. Remove the extra binding deliberately and run again"
  fi
  printf '  service account %s verified: only %s may impersonate it\n' "$email" "$member"
}

ensure_custom_role() { # <role id> <permissions> <why>
  local role_id="$1" perms="$2" why="$3" full="projects/${PROJECT}/roles/$1"
  if probe iam roles describe "$full" --project="$PROJECT"; then
    printf '  custom role %s already exists - will be verified\n' "$full"
  else
    apply_call iam roles create "$role_id" --project="$PROJECT" \
      --title="$role_id" --description="T-1116 bootstrap: ${why}" \
      --permissions="$perms" --stage=GA
    printf '  created custom role %s\n' "$full"
  fi
}

# The custom role's permission list is the grant; read it back so a hand-edited
# role cannot quietly widen the applier (SEC-06).
verify_custom_role() { # <role id> <permissions>
  local role_id="$1" perms="$2" full="projects/${PROJECT}/roles/$1" installed
  installed="$(read_value iam roles describe "$full" --project="$PROJECT" \
    --format='value(includedPermissions)' | tr -d ' ' | tr ',' '\n' | awk 'NF' | sort | tr '\n' ' ')"
  perms="$(printf '%s' "$perms" | tr ',' '\n' | sort | tr '\n' ' ')"
  if [ "$installed" != "$perms" ]; then
    die "custom role ${full} does not hold exactly the permissions this script grants:
  installed: ${installed}
  intended:  ${perms}
Fix the role deliberately and run again"
  fi
  printf '  custom role %s verified: %s\n' "$full" "$installed"
}

bind_project_role() {
  local member="$1" role="$2"
  apply_call projects add-iam-policy-binding "$PROJECT" \
    --member="$member" --role="$role" --condition=None
}

# SEC-05: the condition goes in a file. The inline form
# (--condition=expression=...,title=...) is parsed as a comma-delimited
# dictionary, and both expressions contain commas, so gcloud rejected them
# before making a request and the restricted grants were never installed.
bind_project_role_conditioned() { # <member> <role> <expression> <title> <description>
  local member="$1" role="$2" expression="$3" title="$4" description="$5" file
  file="${RUN_TMP}/condition-${title}.json"
  write_condition_file "$file" "$expression" "$title" "$description"
  apply_call projects add-iam-policy-binding "$PROJECT" \
    --member="$member" --role="$role" --condition-from-file="$file"
}

json_escape() { # a JSON string body: backslash and double quote are the only two
  printf '%s' "$1" | sed 's/\\/\\\\/g; s/"/\\"/g'
}

write_condition_file() { # <path> <expression> <title> <description>
  local path="$1" expression="$2" title="$3" description="$4"
  printf '{"expression": "%s", "title": "%s", "description": "%s"}\n' \
    "$(json_escape "$expression")" "$(json_escape "$title")" "$(json_escape "$description")" >"$path"
}

bind_service_account_role() {
  local email="$1" member="$2" role="$3"
  apply_call iam service-accounts add-iam-policy-binding "$email" \
    --project="$IDENTITY_PROJECT" --member="$member" --role="$role" --condition=None
}

# The last thing the owner sees: exactly the values T-1117's workflow reads.
print_github_variables() {
  local env_suffix
  env_suffix="$(printf '%s' "$ENVIRONMENT" | tr '[:lower:]' '[:upper:]')"
  cat <<EOF

Store these as GitHub variables (Settings -> Secrets and variables -> Actions).

Repository variables - the post-merge plan job on main has no environment, so it
cannot see an environment-scoped variable:
  GCP_WIF_PLAN_PROVIDER_${env_suffix}=projects/${IDENTITY_PROJECT_NUMBER}/locations/global/workloadIdentityPools/${PLAN_POOL}/providers/${PLAN_PROVIDER}
  GCP_PLANNER_SA_${env_suffix}=${PLANNER_EMAIL}
  TF_STATE_BUCKET_${env_suffix}=${PROJECT}-tfstate
  GCP_PROJECT_ID_${env_suffix}=${PROJECT}

Environment variables - in the '${ENVIRONMENT}' environment, for the apply job:
  GCP_WIF_APPLY_PROVIDER=projects/${IDENTITY_PROJECT_NUMBER}/locations/global/workloadIdentityPools/${APPLY_POOL}/providers/${APPLY_PROVIDER}
  GCP_APPLIER_SA=${APPLIER_EMAIL}

The identity project (${IDENTITY_PROJECT}) holds the pools, the providers and the
two service accounts. The applier has no role there; the workload project
(${PROJECT}) is where both identities have their project roles.

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
    --identity-project)
      [ "$#" -ge 2 ] || die "--identity-project needs a value"
      IDENTITY_PROJECT="$2"
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
[ -n "$IDENTITY_PROJECT" ] || die "--identity-project is required"
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

# The identities cannot be protected in a project where the applier holds project
# roles, so the two arguments must name different projects.
if [ "$IDENTITY_PROJECT" = "$PROJECT" ]; then
  die "--identity-project must differ from --project: the applier holds project roles in ${PROJECT}, so the pools, the providers and the two identities must live in a project where it holds none (ADR 0003 item 5, T-1116 step 6)"
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

# 2. Both projects must exist and both must have billing linked. Linking billing
#    needs a billing admin and is not scripted - the console link is the
#    handover.
check_project() { # <project id>
  local id="$1" billing
  if ! gcloud projects describe "$id" --format='value(projectNumber)' >/dev/null; then
    die "project ${id} does not exist or is not visible to the active account"
  fi
  billing="$(gcloud billing projects describe "$id" --format='value(billingEnabled)' 2>/dev/null || true)"
  if [ "$billing" != "True" ]; then
    printf 'Billing is not linked to %s.\n' "$id"
    printf 'Link it first (needs a billing admin): https://console.cloud.google.com/billing/linkedaccount?project=%s\n' \
      "$id"
    die "billing is not linked for ${id}: add the billing account in the console and run this script again"
  fi
}

check_project "$PROJECT"
check_project "$IDENTITY_PROJECT"

IDENTITY_PROJECT_NUMBER="$(gcloud projects describe "$IDENTITY_PROJECT" --format='value(projectNumber)')"

PLANNER_EMAIL="${PLANNER_ACCOUNT}@${IDENTITY_PROJECT}.iam.gserviceaccount.com"
APPLIER_EMAIL="${APPLIER_ACCOUNT}@${IDENTITY_PROJECT}.iam.gserviceaccount.com"
PLANNER_PRINCIPAL="principalSet://iam.googleapis.com/projects/${IDENTITY_PROJECT_NUMBER}/locations/global/workloadIdentityPools/${PLAN_POOL}/attribute.repository/${REPO}"
APPLIER_PRINCIPAL="principalSet://iam.googleapis.com/projects/${IDENTITY_PROJECT_NUMBER}/locations/global/workloadIdentityPools/${APPLY_POOL}/attribute.environment/${ENVIRONMENT}"

# 3. Print the plan, then wait for the operator (BOOT AC1, AC2).
print_plan
if [ "$DRY_RUN" -eq 1 ]; then
  printf '\ndry run: nothing was created and nothing was changed.\n'
  print_github_variables
  exit 0
fi
if [ "$ASSUME_YES" -eq 0 ]; then
  printf '\nType y to create this in %s and %s: ' "$PROJECT" "$IDENTITY_PROJECT"
  answer=""
  read -r answer || answer=""
  if [ "$answer" != "y" ]; then
    die "aborted by the operator: nothing was created"
  fi
fi

RUN_TMP="$(mktemp -d "${TMPDIR:-/tmp}/bootstrap-project.XXXXXX")"

# 4. Enable only the APIs needed to bootstrap. modules/foundation enables the
#    rest in the workload project.
apply_call services enable "${BOOTSTRAP_APIS[@]}" --project="$PROJECT"
printf '  enabled the bootstrap APIs in %s\n' "$PROJECT"
apply_call services enable "${BOOTSTRAP_APIS_IDENTITY[@]}" --project="$IDENTITY_PROJECT"
printf '  enabled the bootstrap APIs in %s\n' "$IDENTITY_PROJECT"

# 5. The identity project: pools, providers and the two identities. Everything
#    is read back and verified *before* the first privilege is granted anywhere
#    (SEC-06), so a mismatch leaves the project exactly as it was.
ensure_pool "$PLAN_POOL" "GitHub Actions plan (read-only)"
ensure_provider "$PLAN_PROVIDER" "$PLAN_POOL" "$(plan_provider_mapping)" "$(plan_provider_condition)"
ensure_pool "$APPLY_POOL" "GitHub Actions apply"
ensure_provider "$APPLY_PROVIDER" "$APPLY_POOL" "$(apply_provider_mapping)" "$(apply_provider_condition)"

# The admission gates: the plan/apply conditions and mappings GitHub's tokens
# must satisfy.
verify_provider "$PLAN_POOL" "$PLAN_PROVIDER" "$(plan_provider_mapping)" "$(plan_provider_condition)"
verify_provider "$APPLY_POOL" "$APPLY_PROVIDER" "$(apply_provider_mapping)" "$(apply_provider_condition)"

ensure_service_account "$PLANNER_ACCOUNT" "Terraform planner (Workload Identity Federation only; no key)" "$PLANNER_EMAIL"
ensure_service_account "$APPLIER_ACCOUNT" "Terraform applier (Workload Identity Federation only; no key)" "$APPLIER_EMAIL"

verify_service_account_policy "$PLANNER_EMAIL" "$PLANNER_PRINCIPAL"
verify_service_account_policy "$APPLIER_EMAIL" "$APPLIER_PRINCIPAL"

# 6. The workload project's own state: the bucket (created and its security
#    configuration read back) and the custom roles the applier's grants are made
#    of, each read back against the permission list this script intends. Still
#    before the first grant.
ensure_state_bucket
verify_state_bucket

while IFS='|' read -r role_id perms _why; do
  case "$role_id" in '' | '#'*) continue ;; esac
  ensure_custom_role "$role_id" "$perms" "$_why"
  verify_custom_role "$role_id" "$perms"
done < <(custom_role_specs)

# 7. Nothing is created that is not verified: only now are privileges granted.

# The impersonation bindings, in the identity project. The planner member names
# the plan pool's repository attribute; the applier member names the apply
# pool's environment attribute and NEVER the repository (BOOT AC5).
bind_service_account_role "$PLANNER_EMAIL" "$PLANNER_PRINCIPAL" roles/iam.workloadIdentityUser
bind_service_account_role "$APPLIER_EMAIL" "$APPLIER_PRINCIPAL" roles/iam.workloadIdentityUser

# The state bucket's two object roles, and nothing else on that bucket.
grant_state_bucket_roles

# 8. Project roles: narrow viewers for the planner, the predefined and custom
#    roles for the applier, and the two conditioned grants (BOOT AC6, SEC-01,
#    SEC-02). The IAM-admin role is skipped in the loop on purpose: it is granted
#    only with a condition. The applier gets nothing in the identity project, so
#    it cannot reach its own impersonation policy or the admission gates.
while IFS='|' read -r role _covers _note; do
  bind_project_role "serviceAccount:${PLANNER_EMAIL}" "$role"
done < <(planner_role_specs)

while IFS='|' read -r role _covers _note; do
  if [ "$role" = "$IAM_ADMIN_ROLE" ]; then
    continue
  fi
  bind_project_role "serviceAccount:${APPLIER_EMAIL}" "$role"
done < <(applier_role_specs)

while IFS='|' read -r role_id _perms _why; do
  case "$role_id" in '' | '#'*) continue ;; esac
  role="projects/${PROJECT}/roles/${role_id}"
  if [ "$role_id" = "mt_applier_service_account_policies" ]; then
    bind_project_role_conditioned "serviceAccount:${APPLIER_EMAIL}" "$role" \
      "$(service_account_policy_condition)" \
      "applier-service-account-role-allowlist" \
      "The applier may modify a service account policy only to bind the two impersonation roles the modules use (T-1116 SEC-01/SEC-02)"
    continue
  fi
  bind_project_role "serviceAccount:${APPLIER_EMAIL}" "$role"
done < <(custom_role_specs)

condition_index=0
while IFS= read -r condition; do
  condition_index=$((condition_index + 1))
  bind_project_role_conditioned "serviceAccount:${APPLIER_EMAIL}" "$IAM_ADMIN_ROLE" \
    "$condition" "modules-only-role-allowlist-${condition_index}" \
    "The applier may grant only the roles modules/foundation and modules/runtime bind at project scope (T-1116 BOOT AC6, part ${condition_index})"
done < <(roles_limiting_conditions)

printf '\nbootstrap complete for %s (environment %s), identity project %s.\n' \
  "$PROJECT" "$ENVIRONMENT" "$IDENTITY_PROJECT"
print_github_variables
