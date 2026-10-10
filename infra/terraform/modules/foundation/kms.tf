# Cloud KMS (S6 5). Two symmetric keys, both in var.region:
#   data-key-kek  wraps every user's `data_key`; exactly three holders
#   system-fields encrypts data that exists before a user; api only
#
# The bindings are authoritative for their role on their key, so no extra
# member can be added out of band (a `google_project_iam_member` with a KMS
# role would be non-authoritative and is never used here).

resource "google_kms_key_ring" "mailtinder" {
  project  = var.project_id
  name     = "mailtinder"
  location = var.region
}

resource "google_kms_crypto_key" "data_key_kek" {
  name            = "data-key-kek"
  key_ring        = google_kms_key_ring.mailtinder.id
  purpose         = "ENCRYPT_DECRYPT"
  rotation_period = "${var.kms_rotation_days * 86400}s"

  version_template {
    algorithm        = "GOOGLE_SYMMETRIC_ENCRYPTION"
    protection_level = "SOFTWARE"
  }

  lifecycle {
    prevent_destroy = true
  }
}

resource "google_kms_crypto_key_iam_binding" "data_key_kek" {
  crypto_key_id = google_kms_crypto_key.data_key_kek.id
  role          = "roles/cloudkms.cryptoKeyEncrypterDecrypter"

  # Exactly api, unsub and worker (S4 2, 3 October 2026).
  members = [
    "serviceAccount:${local.runtime_service_accounts.api}",
    "serviceAccount:${local.runtime_service_accounts.unsub}",
    "serviceAccount:${local.runtime_service_accounts.worker}",
  ]
}

resource "google_kms_crypto_key" "system_fields" {
  name            = "system-fields"
  key_ring        = google_kms_key_ring.mailtinder.id
  purpose         = "ENCRYPT_DECRYPT"
  rotation_period = "${var.kms_rotation_days * 86400}s"

  version_template {
    algorithm        = "GOOGLE_SYMMETRIC_ENCRYPTION"
    protection_level = "SOFTWARE"
  }

  lifecycle {
    prevent_destroy = true
  }
}

resource "google_kms_crypto_key_iam_binding" "system_fields" {
  crypto_key_id = google_kms_crypto_key.system_fields.id
  role          = "roles/cloudkms.cryptoKeyEncrypterDecrypter"

  # api only (S6 5, 4 October 2026). James's project owner role covers the
  # admin tool (T-507), so it is not listed here.
  members = [
    "serviceAccount:${local.runtime_service_accounts.api}",
  ]
}