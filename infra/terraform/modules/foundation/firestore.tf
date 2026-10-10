# Firestore (S4 1, S6 5). Native mode in var.region; the location is permanent
# once created, so it is never changed or fallen back from.
#
# No backups and no point-in-time recovery: the trial runs with none, so no
# older copy of a wrapped `data_key` survives account deletion (S6 5,
# crypto-shredding, DEL-2; James, 4 October 2026).
resource "google_firestore_database" "default" {
  project                           = var.project_id
  name                              = "(default)"
  location_id                       = var.region
  type                              = "FIRESTORE_NATIVE"
  delete_protection_state           = "DELETE_PROTECTION_ENABLED"
  deletion_policy                   = "ABANDON"
  point_in_time_recovery_enablement = "POINT_IN_TIME_RECOVERY_DISABLED"
}

# TTL backstops (S5 Retention; the sweeper is the control and TTL the backstop).
# Firestore TTL watches one field per collection named by the schema; the
# document shapes are T-301's, so the field name is the shared `expires_at`.
locals {
  ttl_collections = toset([
    "sessions",
    "jobs",
    "needs_attention",
    "rate_limits",
    "classifier_eval",
  ])
}

resource "google_firestore_field" "ttl" {
  for_each = local.ttl_collections

  project    = var.project_id
  database   = google_firestore_database.default.name
  collection = each.value
  field      = "expires_at"

  ttl_config {}

  # An empty `index_config {}` disables *every* single-field index on the
  # field (see the google_firestore_field docs), which takes `expires_at` out
  # of the automatic single-field indexing the sweeper's queries rely on.
  # Declare the default single-field indexes explicitly instead: Firestore
  # serves ascending and descending single-field queries from an order index.
  # The array-contains default is not declared: `expires_at` is a timestamp,
  # never an array (review F2).
  index_config {
    indexes {
      order       = "ASCENDING"
      query_scope = "COLLECTION"
    }

    indexes {
      order       = "DESCENDING"
      query_scope = "COLLECTION"
    }
  }
}

# Firestore has no per-collection IAM, so the least-privilege equivalent is the
# project-level datastore.user role, held by the three runtime identities only.
resource "google_project_iam_member" "datastore_user" {
  for_each = local.runtime_service_accounts

  project = var.project_id
  role    = "roles/datastore.user"
  member  = "serviceAccount:${each.value}"
}

# Vertex AI for the bake-off classifier (S4 2, S4 5.7): api only, no key.
resource "google_project_iam_member" "aiplatform_user" {
  project = var.project_id
  role    = "roles/aiplatform.user"
  member  = "serviceAccount:${local.runtime_service_accounts.api}"
}