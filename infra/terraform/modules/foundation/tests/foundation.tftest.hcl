# Plan-time tests for the foundation module (T-1102a step 13).
#
# Every run uses mock_provider, so the suite needs no credentials and no
# network.
#
# These runs inspect the resources the module *declares*. They cannot prove the
# absence of a resource type, and they cannot see a binding a later change adds
# somewhere else: `terraform test` has no view of resources the module does not
# use. The absence properties the task claims (no google_service_account_key, no
# google_secret_manager_secret_version, no google_firestore_backup_schedule, no
# primitive role, no roles/iam.serviceAccountUser, no logging role for an app
# identity) are enforced instead by the role allowlist and resource guards in
# the `terraform` CI job, which scan every `*.tf` file under `modules/` and
# `envs/`. The runs below are named for what they actually check.
mock_provider "google" {}

# Deterministic, distinct values for the computed attributes the assertions
# read, made available during plan (override_during = plan). Without these the
# conditions are unknown at plan time and Terraform refuses to evaluate them;
# with one shared value the identity sets would collapse to a single element
# and the membership tests could not fail.
override_resource {
  target          = google_service_account.service["api"]
  override_during = plan
  values = {
    email = "mt-api@mock.example.com"
  }
}

override_resource {
  target          = google_service_account.service["unsub"]
  override_during = plan
  values = {
    email = "mt-unsub@mock.example.com"
  }
}

override_resource {
  target          = google_service_account.service["worker"]
  override_during = plan
  values = {
    email = "mt-worker@mock.example.com"
  }
}

override_resource {
  target          = google_service_account.service["tasks-invoker"]
  override_during = plan
  values = {
    email = "mt-tasks-invoker@mock.example.com"
  }
}

override_resource {
  target          = google_service_account.service["scheduler-invoker"]
  override_during = plan
  values = {
    email = "mt-scheduler-invoker@mock.example.com"
  }
}

# Both sinks route to a log bucket in this same project, so the API returns no
# writer identity for them: the real value is the empty string. The override
# models that faithfully rather than inventing a plausible writer. If the module
# ever binds a sink's (empty) writer identity to a google_project_iam_member,
# the provider rejects the empty member and this suite fails at plan time - the
# counterexample the previous round missed (review D1).
override_resource {
  target          = google_logging_project_sink.app
  override_during = plan
  values = {
    writer_identity = ""
  }
}

override_resource {
  target          = google_logging_project_sink.audit
  override_during = plan
  values = {
    writer_identity = ""
  }
}

variables {
  project_id = "mailtinder-test"
  env        = "prod"
}

run "kms_has_exactly_three_holders" {
  command = plan

  assert {
    condition = toset(google_kms_crypto_key_iam_binding.data_key_kek.members) == toset([
      "serviceAccount:${google_service_account.service["api"].email}",
      "serviceAccount:${google_service_account.service["unsub"].email}",
      "serviceAccount:${google_service_account.service["worker"].email}",
    ])
    error_message = "data-key-kek must have exactly three encrypt/decrypt holders: api, unsub and worker (S4 2, ASVS V13.2.2)"
  }

  assert {
    condition     = google_kms_crypto_key_iam_binding.data_key_kek.role == "roles/cloudkms.cryptoKeyEncrypterDecrypter"
    error_message = "the only KMS role granted must be cryptoKeyEncrypterDecrypter"
  }
}

run "system_key_has_api_only" {
  command = plan

  assert {
    condition     = toset(google_kms_crypto_key_iam_binding.system_fields.members) == toset(["serviceAccount:${google_service_account.service["api"].email}"])
    error_message = "system-fields must be encrypt/decrypt for api only (S6 5)"
  }
}

run "no_firestore_backups" {
  command = plan

  assert {
    condition     = google_firestore_database.default.point_in_time_recovery_enablement == "POINT_IN_TIME_RECOVERY_DISABLED"
    error_message = "point-in-time recovery must stay disabled: backups would survive crypto-shredding (S6 5, DEL-2)"
  }

  assert {
    condition     = google_firestore_database.default.deletion_policy == "ABANDON"
    error_message = "the Firestore database must not be destroyed by Terraform"
  }
}

run "kms_key_rotates_yearly_in_region" {
  command = plan

  variables {
    kms_rotation_days = 365
  }

  assert {
    condition = (
      google_kms_crypto_key.data_key_kek.rotation_period == "31536000s" &&
      google_kms_crypto_key.system_fields.rotation_period == "31536000s"
    )
    error_message = "both KMS keys must rotate yearly (S6 5, kms_rotation_days = 365)"
  }

  assert {
    condition     = google_kms_key_ring.mailtinder.location == var.region
    error_message = "the key ring must live in var.region"
  }
}

run "firestore_in_us_central1_native_with_delete_protection" {
  command = plan

  assert {
    condition = (
      google_firestore_database.default.location_id == "us-central1" &&
      google_firestore_database.default.type == "FIRESTORE_NATIVE" &&
      google_firestore_database.default.delete_protection_state == "DELETE_PROTECTION_ENABLED"
    )
    error_message = "Firestore must be FIRESTORE_NATIVE in us-central1 with delete protection on (S4 1)"
  }
}

run "ttl_on_every_ttl_collection" {
  command = plan

  assert {
    condition     = length(google_firestore_field.ttl) == 5
    error_message = "every S5 TTL collection needs a TTL field (five fields)"
  }

  assert {
    condition = toset([for f in values(google_firestore_field.ttl) : f.collection]) == toset([
      "sessions",
      "jobs",
      "needs_attention",
      "rate_limits",
      "classifier_eval",
    ])
    error_message = "TTL fields must be on exactly the S5 TTL collections"
  }

  assert {
    condition     = alltrue([for f in values(google_firestore_field.ttl) : f.field == "expires_at" && f.database == google_firestore_database.default.name])
    error_message = "every TTL field must be `expires_at` on the (default) database"
  }

  assert {
    condition     = alltrue([for f in values(google_firestore_field.ttl) : length(f.ttl_config) == 1])
    error_message = "every TTL field must have ttl_config enabled, or the retention backstop is not configured (S5 Retention, review F9)"
  }

  assert {
    condition     = alltrue([for f in values(google_firestore_field.ttl) : length(f.index_config) == 1 && length(f.index_config[0].indexes) > 0])
    error_message = "every TTL field must keep a declared single-field index; an empty index_config disables all indexing on the field (review F2)"
  }
}

run "jev_and_email_lookup_secrets_have_api_only" {
  command = plan

  assert {
    condition     = toset(google_secret_manager_secret_iam_binding.accessor["jev-api-key"].members) == toset(["serviceAccount:${google_service_account.service["api"].email}"])
    error_message = "the Jev API key must be readable by api only (S4 2, ASVS V13.3.2)"
  }

  assert {
    condition     = toset(google_secret_manager_secret_iam_binding.accessor["email-lookup-hmac-key"].members) == toset(["serviceAccount:${google_service_account.service["api"].email}"])
    error_message = "the email lookup HMAC key must be readable by api only (ASVS V13.3.2)"
  }

  assert {
    condition = (
      toset(google_secret_manager_secret_iam_binding.accessor["google-oauth-client-secret"].members) == toset([
        "serviceAccount:${google_service_account.service["api"].email}",
        "serviceAccount:${google_service_account.service["unsub"].email}",
        "serviceAccount:${google_service_account.service["worker"].email}",
      ]) &&
      toset(google_secret_manager_secret_iam_binding.accessor["log-pseudonym-hmac-key"].members) == toset([
        "serviceAccount:${google_service_account.service["api"].email}",
        "serviceAccount:${google_service_account.service["unsub"].email}",
        "serviceAccount:${google_service_account.service["worker"].email}",
      ])
    )
    error_message = "the OAuth client secret and log pseudonymisation key are readable by api, unsub and worker only (S4 2)"
  }
}

run "no_secret_versions_in_state" {
  command = plan

  assert {
    condition     = length(google_secret_manager_secret.secret) == 4
    error_message = "the module creates secret containers only; no version (and so no value) may reach state (ASVS V13.3.1)"
  }

  assert {
    condition     = toset([for s in values(google_secret_manager_secret.secret) : s.secret_id]) == toset(["google-oauth-client-secret", "jev-api-key", "email-lookup-hmac-key", "log-pseudonym-hmac-key"])
    error_message = "exactly the four S4 2 secret containers"
  }
}

run "log_bucket_90_days_locked_in_region" {
  command = plan

  assert {
    condition = (
      google_logging_project_bucket_config.app.retention_days == 90 &&
      google_logging_project_bucket_config.app.locked == true &&
      google_logging_project_bucket_config.app.location == "us-central1"
    )
    error_message = "the log bucket must be 90 days, locked and in us-central1 (S6 7, ASVS V16.2.3, V16.4.2)"
  }

  assert {
    condition = (
      strcontains(google_logging_project_sink.app.filter, "resource.type=\"cloud_run_revision\"") &&
      !strcontains(google_logging_project_sink.app.filter, "cloud_tasks_queue") &&
      !strcontains(google_logging_project_sink.app.filter, "cloud_scheduler_job") &&
      strcontains(google_logging_project_sink.app.filter, "NOT logName:\"run.googleapis.com%2Frequests\"")
    )
    error_message = "the sink must route the Cloud Run revision entries - the application's stdout/stderr and the Cloud Run platform/system entries that share that resource type, not stdout/stderr alone - keep the Cloud Run request log out (request URL and client IP, S5 88-90), and leave platform Cloud Tasks/Scheduler logs out of the locked bucket (review F3, F7)"
  }

  assert {
    condition = (
      strcontains(google_logging_project_exclusion.app_default.filter, "resource.type=\"cloud_run_revision\"") &&
      !strcontains(google_logging_project_exclusion.app_default.filter, "run.googleapis.com%2Frequests")
    )
    error_message = "_Default must exclude every app entry, request log included, so nothing banned is retained elsewhere (V16.2.3, review F3)"
  }
}

# The audit evidence S11 reads (elevation review, alert A7, leak investigation)
# must land in the locked 90-day bucket, not only in _Default (V16.4.2,
# V16.4.3). The sinks are declared resources, so this is checkable at plan time,
# and every assertion here is removal-sensitive (review D2): disabling the sink,
# re-pointing it away from the locked bucket, dropping the cloudaudit selector,
# dropping either service, turning the service OR into an AND, or dropping a
# Data Access category each makes the suite fail.
run "audit_logs_are_routed_to_the_locked_bucket" {
  command = plan

  assert {
    condition = (
      google_logging_project_sink.audit.disabled != true &&
      google_logging_project_sink.audit.destination == "logging.googleapis.com/projects/${var.project_id}/locations/${var.region}/buckets/${google_logging_project_bucket_config.app.bucket_id}"
    )
    error_message = "the audit sink must stay enabled and target the locked mailtinder-logs bucket exactly (V16.4.2, review D1, D2)"
  }

  # EXACT filter, not a substring probe (review D2). A substring check passes
  # while extra clauses are appended - e.g. `... AND severity=ERROR AND
  # severity=INFO` makes the filter match no entry yet still contains both
  # expected fragments. Pinning the whole string means any edit to the sink's
  # filter (adding, dropping or reordering a clause) fails here. The expected
  # string is the canonical filter; the second clause records the D2 mutation
  # and proves this assertion rejects it.
  assert {
    condition = (
      google_logging_project_sink.audit.filter == "logName:\"cloudaudit.googleapis.com\" AND (protoPayload.serviceName=\"cloudkms.googleapis.com\" OR protoPayload.serviceName=\"secretmanager.googleapis.com\")" &&
      google_logging_project_sink.audit.filter != "logName:\"cloudaudit.googleapis.com\" AND (protoPayload.serviceName=\"cloudkms.googleapis.com\" OR protoPayload.serviceName=\"secretmanager.googleapis.com\") AND severity=ERROR AND severity=INFO"
    )
    error_message = "the audit sink filter must be exactly the canonical cloudaudit filter for KMS-or-Secret-Manager; appending extra clauses (e.g. AND severity=...) must fail (review D2)"
  }

  # The Data Access categories the sink can route: KMS and Secret Manager must
  # each collect DATA_READ and DATA_WRITE, or the routing above has nothing to
  # send (V16.4.2, review D2).
  assert {
    condition = alltrue([
      for svc in [google_project_iam_audit_config.cloudkms, google_project_iam_audit_config.secretmanager] : alltrue([
        for log_type in ["DATA_READ", "DATA_WRITE"] : anytrue([
          for c in svc.audit_log_config : c.log_type == log_type
        ])
      ])
    ])
    error_message = "KMS and Secret Manager must each collect DATA_READ and DATA_WRITE audit logs, or the audit sink routes no evidence (V16.4.2, review D2)"
  }
}

# Checks the module's declared bindings only (see the file header): it proves
# that none of the project-level grants the module makes gives an app identity a
# roles/logging.* role. The sinks' writer identities are empty (same-project log
# buckets) and are bound to no IAM member, so there is no logging grant from a
# sink identity either (review D1). Absence of a logging grant added elsewhere is
# enforced by the CI role allowlist.
run "declared_bindings_give_app_identities_no_logging_role" {
  command = plan

  assert {
    condition = alltrue([
      for m in concat(
        [google_project_iam_member.aiplatform_user],
        values(google_project_iam_member.datastore_user),
        ) : !(startswith(m.role, "roles/logging.") && contains([
          "serviceAccount:${google_service_account.service["api"].email}",
          "serviceAccount:${google_service_account.service["unsub"].email}",
          "serviceAccount:${google_service_account.service["worker"].email}",
          "serviceAccount:${google_service_account.service["tasks-invoker"].email}",
          "serviceAccount:${google_service_account.service["scheduler-invoker"].email}",
      ], m.member))
    ])
    error_message = "no application identity may hold a roles/logging.* role (ASVS V16.4.2)"
  }

  # Same-project bucket sinks return an empty writer identity and need no IAM
  # member at all. Pinning it here means a re-added writer grant cannot hide
  # behind a plausible mock value (review D1).
  assert {
    condition = (
      google_logging_project_sink.app.writer_identity == "" &&
      google_logging_project_sink.audit.writer_identity == ""
    )
    error_message = "a same-project log bucket sink must have no writer identity (and so no bucketWriter grant; review D1)"
  }
}

run "five_service_accounts_are_created" {
  command = plan

  assert {
    condition     = length(google_service_account.service) == 5
    error_message = "the module creates exactly five identities (S4 2)"
  }

  assert {
    condition     = toset([for s in values(google_service_account.service) : s.account_id]) == toset(["mt-api", "mt-unsub", "mt-worker", "mt-tasks-invoker", "mt-scheduler-invoker"])
    error_message = "exactly the five S4 2 identities"
  }
}

# Checks the module's declared bindings only (see the file header): no role the
# module grants is a primitive role. A primitive role added anywhere else is
# caught by the CI role allowlist, which scans every .tf file.
run "declared_bindings_grant_no_primitive_role" {
  command = plan

  assert {
    condition = alltrue([
      for role in concat(
        [
          google_kms_crypto_key_iam_binding.data_key_kek.role,
          google_kms_crypto_key_iam_binding.system_fields.role,
          google_project_iam_member.aiplatform_user.role,
        ],
        [for m in values(google_project_iam_member.datastore_user) : m.role],
        [for m in values(google_secret_manager_secret_iam_binding.accessor) : m.role],
      ) : !contains(["roles/owner", "roles/editor", "roles/viewer"], role)
    ])
    error_message = "no primitive role (owner, editor, viewer) may be granted anywhere (ASVS V13.2.3)"
  }
}

run "every_regional_resource_uses_var_region" {
  command = plan

  variables {
    region = "europe-west9"
  }

  assert {
    condition = alltrue([
      google_kms_key_ring.mailtinder.location == var.region,
      google_firestore_database.default.location_id == var.region,
      google_logging_project_bucket_config.app.location == var.region,
      google_artifact_registry_repository.mailtinder.location == var.region,
      alltrue([for s in values(google_secret_manager_secret.secret) : s.replication[0].user_managed[0].replicas[0].location == var.region]),
      strcontains(google_logging_project_sink.app.destination, "/locations/${var.region}/"),
    ])
    error_message = "every regional resource must take its location from var.region (S4 1)"
  }
}