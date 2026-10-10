# Project APIs. Each is enabled with disable_on_destroy = false so a
# `terraform destroy` never turns an API off underneath a live environment.
locals {
  apis = toset([
    "run",
    "cloudtasks",
    "cloudscheduler",
    "firestore",
    "cloudkms",
    "secretmanager",
    "aiplatform",
    "logging",
    "monitoring",
    "artifactregistry",
    "iam",
    "iamcredentials",
    "sts",
    "firebase",
    "firebasehosting",
    "gmail",
    "drive",
    "cloudbilling",
    "billingbudgets",
  ])
}

resource "google_project_service" "enabled" {
  for_each = local.apis

  project            = var.project_id
  service            = "${each.key}.googleapis.com"
  disable_on_destroy = false
}