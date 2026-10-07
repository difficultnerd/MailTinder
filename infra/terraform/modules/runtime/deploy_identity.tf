# The GitHub Actions deploy identity (V13.2.1, V13.2.2). GitHub presents an
# OIDC token; Workload Identity Federation exchanges it for short-lived
# credentials for `mt-deployer`. No service account key exists anywhere, and the
# provider's condition admits only one repository and one ref.
resource "google_iam_workload_identity_pool" "github" {
  project                   = var.project_id
  workload_identity_pool_id = "github"
  display_name              = "GitHub Actions"
  description               = "Deploy identity for ${var.github_repository} (V13.2.2)."
}

resource "google_iam_workload_identity_pool_provider" "github" {
  project                            = var.project_id
  workload_identity_pool_id          = google_iam_workload_identity_pool.github.workload_identity_pool_id
  workload_identity_pool_provider_id = "github"
  display_name                       = "GitHub Actions OIDC"

  attribute_mapping = {
    "google.subject"       = "assertion.sub"
    "attribute.repository" = "assertion.repository"
    "attribute.ref"        = "assertion.ref"
  }

  attribute_condition = "assertion.repository == '${var.github_repository}' && assertion.ref == '${var.deploy_ref}'"

  oidc {
    issuer_uri = "https://token.actions.githubusercontent.com"
  }
}

resource "google_service_account" "deployer" {
  project      = var.project_id
  account_id   = "mt-deployer"
  display_name = "GitHub Actions deployer (Workload Identity Federation only; no key)"
}

# Only a token from `attribute.repository = var.github_repository` may
# impersonate the deployer.
resource "google_service_account_iam_member" "deployer_wif" {
  service_account_id = google_service_account.deployer.name
  role               = "roles/iam.workloadIdentityUser"
  member             = "principalSet://iam.googleapis.com/${google_iam_workload_identity_pool.github.name}/attribute.repository/${var.github_repository}"
}

# Deploy images to the three services (resource level, not the project).
resource "google_cloud_run_v2_service_iam_member" "deployer_run_developer" {
  for_each = {
    api    = google_cloud_run_v2_service.api.name
    unsub  = google_cloud_run_v2_service.unsub.name
    worker = google_cloud_run_v2_service.worker.name
  }

  project  = var.project_id
  location = var.region
  name     = each.value
  role     = "roles/run.developer"
  member   = "serviceAccount:${google_service_account.deployer.email}"
}

# Deploying a Cloud Run revision means acting as the revision's runtime
# identity, so the deployer needs `actAs` on exactly those three accounts -
# never on the project (T-1102b edge case, V13.2.1).
resource "google_service_account_iam_member" "deployer_acts_as_runtime" {
  for_each = toset(["api", "unsub", "worker"])

  service_account_id = "projects/${var.project_id}/serviceAccounts/${var.service_accounts[each.key]}"
  role               = "roles/iam.serviceAccountUser"
  member             = "serviceAccount:${google_service_account.deployer.email}"
}

# Push images to the one Artifact Registry repository, nothing wider.
resource "google_artifact_registry_repository_iam_member" "deployer_writer" {
  project    = var.project_id
  location   = var.region
  repository = "mailtinder"
  role       = "roles/artifactregistry.writer"
  member     = "serviceAccount:${google_service_account.deployer.email}"
}

# Deploy the Hosting site. There is no narrower role than admin (T-1102b).
resource "google_project_iam_member" "deployer_hosting_admin" {
  project = var.project_id
  role    = "roles/firebasehosting.admin"
  member  = "serviceAccount:${google_service_account.deployer.email}"
}

# The complete set of roles the deployer holds. Nothing for KMS, Secret
# Manager, Firestore or IAM administration (V13.2.2); the run and actAs roles
# are per service, not per project.
locals {
  deployer_roles = toset([
    "roles/run.developer",
    "roles/iam.serviceAccountUser",
    "roles/artifactregistry.writer",
    "roles/firebasehosting.admin",
    "roles/iam.workloadIdentityUser",
  ])
}