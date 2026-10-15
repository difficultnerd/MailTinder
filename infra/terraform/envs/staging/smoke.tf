# The staging smoke identity (T-1105, S10 3.1 "Staging smoke", S11 section 2
# "Extra identity").
#
# Staging only, by construction: this file exists in the staging root and the
# production root never calls it (S4 1, decision T5). `scripts/tf_env_parity.sh`
# ignores it by name - the parity rule is about the modules the two roots call,
# and this file calls none, so it can never give staging a different shape.
#
# `mt-smoke` is the identity `.github/workflows/smoke.yml` impersonates over
# Workload Identity Federation (no key exists, V13.2.1) to run the post-deploy
# smoke binary against staging. Its grants are the smallest set the six checks
# need, and each is on the narrowest resource:
#
#   roles/datastore.user                        project   (Firestore documents)
#   roles/cloudkms.cryptoKeyEncrypterDecrypter  the key   (added below, in
#                                                         main.tf: the key's
#                                                         binding is
#                                                         authoritative)
#   roles/cloudtasks.enqueuer / taskDeleter     the queue
#   roles/iam.serviceAccountUser                mt-tasks-invoker only
#   roles/iam.workloadIdentityUser              the repository
#
# It holds no Secret Manager, Gmail, Drive or Cloud Run role: the smoke binary
# never touches those (`backend/crates/smoke/src/unused.rs`), and the internal
# services are checked *unauthenticated*, so it deliberately cannot invoke them.
#
# Blast radius, stated honestly: `datastore.user` plus this project's KMS key
# lets a token that reaches this identity read and write every document in the
# *staging* project and wrap/unwrap keys there. That project holds synthetic
# data only (S11 6.5: "staging data is synthetic only"), and no production
# resource is reachable from it: the production project has its own state and
# its own identities, and the smoke binding does not exist there at all.

variable "github_repository" {
  type        = string
  default     = "difficultnerd/MailTinder"
  description = "The only repository whose GitHub Actions runs may impersonate the smoke identity (V13.2.2). The same value the runtime module uses for the deployer."
}

locals {
  # The WIF pool the runtime module created, read back from the provider's
  # resource name (`.../workloadIdentityPools/github/providers/github`), so this
  # file never spells the pool out a second time.
  smoke_wif_pool = trimsuffix(module.runtime.wif_provider, "/providers/github")
}

resource "google_service_account" "smoke" {
  project      = var.project_id
  account_id   = "mt-smoke"
  display_name = "MailTinder staging smoke tests (Workload Identity Federation only; no key)"
}

# Read and write Firestore documents: the kms/firestore round trips, and the
# user, mailbox, job and needs_attention documents the UN-02 AC4 check creates
# and then deletes.
resource "google_project_iam_member" "smoke_datastore" {
  project = var.project_id
  role    = "roles/datastore.user"
  member  = "serviceAccount:${google_service_account.smoke.email}"
}

# Cloud Tasks: create the two due-now tasks of the UN-02 AC4 check and delete
# the task the schedule/cancel check created. On the queue resource, never the
# project (V13.2.2).
resource "google_cloud_tasks_queue_iam_member" "smoke_enqueuer" {
  project  = var.project_id
  location = var.region
  name     = basename(module.runtime.queue_id)
  role     = "roles/cloudtasks.enqueuer"
  member   = "serviceAccount:${google_service_account.smoke.email}"
}

resource "google_cloud_tasks_queue_iam_member" "smoke_task_deleter" {
  project  = var.project_id
  location = var.region
  name     = basename(module.runtime.queue_id)
  role     = "roles/cloudtasks.taskDeleter"
  member   = "serviceAccount:${google_service_account.smoke.email}"
}

# Creating a task that runs as `tasks-invoker` means acting as that identity, so
# the smoke account needs `actAs` on exactly that one service account - as the
# api's own binding does (T-1102b edge case, V13.2.1), never on the project.
resource "google_service_account_iam_member" "smoke_acts_as_tasks_invoker" {
  service_account_id = "projects/${var.project_id}/serviceAccounts/${module.foundation.service_accounts["tasks_invoker"]}"
  role               = "roles/iam.serviceAccountUser"
  member             = "serviceAccount:${google_service_account.smoke.email}"
}

# Only a GitHub Actions OIDC token from this repository may impersonate the
# smoke account. The provider's own attribute condition pins the repository, the
# ref (`refs/heads/main`) and the environment (`staging`), so this grant is no
# wider than the deployer's (T-1103, V13.2.2): a token from another environment
# never reaches the pool at all.
resource "google_service_account_iam_member" "smoke_wif" {
  service_account_id = google_service_account.smoke.name
  role               = "roles/iam.workloadIdentityUser"
  member             = "principalSet://iam.googleapis.com/${local.smoke_wif_pool}/attribute.repository/${var.github_repository}"
}
