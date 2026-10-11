# Secret Manager containers only (S4 2, S6 5, step 6). No version resource
# exists anywhere in this module: values never enter Terraform state
# (ASVS V13.3.1). James adds the values with
#   printf '%s' "$VALUE" | gcloud secrets versions add <name> --data-file=-
# after `apply` (see ../../README.md).
locals {
  secrets = {
    # The container id is what the services look up at start-up
    # (`SecretName::secret_id()`, `ports/src/secrets.rs`), so it must match
    # the code exactly (review F3).
    google-oauth-client-secret = ["api", "unsub", "worker"]
    jev-api-key                = ["api"]
    email-lookup-hmac-key      = ["api"]
    log-pseudonym-hmac-key     = ["api", "unsub", "worker"]
  }
}

resource "google_secret_manager_secret" "secret" {
  for_each = local.secrets

  project   = var.project_id
  secret_id = each.key

  replication {
    user_managed {
      replicas {
        location = var.region
      }
    }
  }
}

# Authoritative binding per secret, so no extra accessor can be added out of
# band (ASVS V13.3.2, S4 2).
resource "google_secret_manager_secret_iam_binding" "accessor" {
  for_each = local.secrets

  project   = var.project_id
  secret_id = google_secret_manager_secret.secret[each.key].secret_id
  role      = "roles/secretmanager.secretAccessor"

  members = [
    for name in each.value : "serviceAccount:${local.runtime_service_accounts[name]}"
  ]
}