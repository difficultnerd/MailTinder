# Plan-time tests for the runtime module (T-1102b step 7). Every run uses
# mock_provider, so the suite needs no credentials and no network.
#
# The runs inspect the resources the module *declares*; like the foundation
# suite they cannot prove the absence of a resource type. The absence
# properties the task claims (no additional Cloud Run invoker, no
# roles/iam.serviceAccountUser on the project, no deployer KMS/Secret Manager/
# Firestore/IAM role, no primitive role) are additionally enforced by the
# `terraform` CI job's role allowlist and actAs-scope guard, which scan every
# `*.tf` file under `modules/` and `envs/`. In particular `ignore_changes` on
# the container image cannot be read from an expression, so its presence on
# every service is enforced by the CI guard, and the run below checks the image
# Terraform is told to use.
mock_provider "google" {}
mock_provider "google-beta" {}

# Deterministic service URLs, made available during plan (override_during =
# plan). Without them the scheduler target and the OIDC audiences are unknown at
# plan time and Terraform refuses to evaluate the conditions.
override_resource {
  target          = google_cloud_run_v2_service.api
  override_during = plan
  values = {
    uri = "https://mt-api-mock.a.run.app"
  }
}

override_resource {
  target          = google_cloud_run_v2_service.unsub
  override_during = plan
  values = {
    uri = "https://mt-unsub-mock.a.run.app"
  }
}

override_resource {
  target          = google_cloud_run_v2_service.worker
  override_during = plan
  values = {
    uri = "https://mt-worker-mock.a.run.app"
  }
}

variables {
  project_id = "mailtinder-test"
  env        = "prod"
  kms_key_id = "projects/mailtinder-test/locations/us-central1/keyRings/mailtinder/cryptoKeys/data-key-kek"

  service_accounts = {
    api               = "mt-api@mailtinder-test.iam.gserviceaccount.com"
    unsub             = "mt-unsub@mailtinder-test.iam.gserviceaccount.com"
    worker            = "mt-worker@mailtinder-test.iam.gserviceaccount.com"
    tasks_invoker     = "mt-tasks-invoker@mailtinder-test.iam.gserviceaccount.com"
    scheduler_invoker = "mt-scheduler-invoker@mailtinder-test.iam.gserviceaccount.com"
  }

  secret_ids = {
    oauth_client_secret = "oauth-client-secret"
    jev_api_key         = "jev-api-key"
    email_lookup_hmac   = "email-lookup-hmac-key"
    log_pseudonym_hmac  = "log-pseudonym-hmac-key"
  }
}

run "only_api_is_public" {
  command = plan

  assert {
    condition = (
      google_cloud_run_v2_service.api.ingress == "INGRESS_TRAFFIC_ALL" &&
      google_cloud_run_v2_service.unsub.ingress == "INGRESS_TRAFFIC_INTERNAL_ONLY" &&
      google_cloud_run_v2_service.worker.ingress == "INGRESS_TRAFFIC_INTERNAL_ONLY"
    )
    error_message = "only api may be public; unsub and worker must have internal ingress (V12.3.3, V13.2.1)"
  }

  assert {
    condition = (
      google_cloud_run_v2_service_iam_member.api_public.member == "allUsers" &&
      google_cloud_run_v2_service_iam_member.api_public.role == "roles/run.invoker" &&
      google_cloud_run_v2_service_iam_member.api_public.name == google_cloud_run_v2_service.api.name
    )
    error_message = "api must be the service with the allUsers run.invoker binding (S4 1)"
  }

  assert {
    condition = (
      google_cloud_run_v2_service_iam_member.unsub_tasks_invoker.member != "allUsers" &&
      google_cloud_run_v2_service_iam_member.worker_scheduler_invoker.member != "allUsers"
    )
    error_message = "unsub and worker must not be invokable by allUsers (V13.2.1)"
  }
}

run "unsub_invoker_is_tasks_invoker_only" {
  command = plan

  assert {
    condition = toset([google_cloud_run_v2_service_iam_member.unsub_tasks_invoker.member]) == toset([
      "serviceAccount:${var.service_accounts["tasks_invoker"]}",
    ])
    error_message = "unsub's only invoker must be the tasks-invoker identity (S7 5.12)"
  }

  assert {
    condition = (
      google_cloud_run_v2_service_iam_member.unsub_tasks_invoker.role == "roles/run.invoker" &&
      google_cloud_run_v2_service_iam_member.unsub_tasks_invoker.name == google_cloud_run_v2_service.unsub.name
    )
    error_message = "the tasks-invoker binding must be roles/run.invoker on unsub only"
  }
}

run "worker_invoker_is_scheduler_invoker_only" {
  command = plan

  assert {
    condition = toset([google_cloud_run_v2_service_iam_member.worker_scheduler_invoker.member]) == toset([
      "serviceAccount:${var.service_accounts["scheduler_invoker"]}",
    ])
    error_message = "worker's only invoker must be the scheduler-invoker identity (S7 5.12)"
  }

  assert {
    condition = (
      google_cloud_run_v2_service_iam_member.worker_scheduler_invoker.role == "roles/run.invoker" &&
      google_cloud_run_v2_service_iam_member.worker_scheduler_invoker.name == google_cloud_run_v2_service.worker.name
    )
    error_message = "the scheduler-invoker binding must be roles/run.invoker on worker only"
  }
}

run "queue_retry_matches_s7" {
  command = plan

  assert {
    condition = (
      google_cloud_tasks_queue.unsubscribe.retry_config[0].max_attempts == 4 &&
      google_cloud_tasks_queue.unsubscribe.retry_config[0].min_backoff == "30s" &&
      google_cloud_tasks_queue.unsubscribe.retry_config[0].max_backoff == "300s"
    )
    error_message = "the queue must retry 4 times with 30s-300s backoff (S7 5.12 API-INT-1)"
  }

  assert {
    condition     = google_cloud_tasks_queue.unsubscribe.name == "unsubscribe"
    error_message = "the queue must be the single `unsubscribe` queue"
  }
}

run "api_can_enqueue_and_delete_on_one_queue_only" {
  command = plan

  assert {
    condition = (
      google_cloud_tasks_queue_iam_member.api_enqueuer.role == "roles/cloudtasks.enqueuer" &&
      google_cloud_tasks_queue_iam_member.api_enqueuer.member == "serviceAccount:${var.service_accounts["api"]}" &&
      google_cloud_tasks_queue_iam_member.api_enqueuer.name == google_cloud_tasks_queue.unsubscribe.name
    )
    error_message = "api must hold cloudtasks.enqueuer on the unsubscribe queue only (V13.2.2)"
  }

  assert {
    condition = (
      google_cloud_tasks_queue_iam_member.api_task_deleter.role == "roles/cloudtasks.taskDeleter" &&
      google_cloud_tasks_queue_iam_member.api_task_deleter.member == "serviceAccount:${var.service_accounts["api"]}" &&
      google_cloud_tasks_queue_iam_member.api_task_deleter.name == google_cloud_tasks_queue.unsubscribe.name
    )
    error_message = "api must hold cloudtasks.taskDeleter on the unsubscribe queue only (V13.2.2)"
  }

  assert {
    condition = (
      google_service_account_iam_member.api_acts_as_tasks_invoker.role == "roles/iam.serviceAccountUser" &&
      google_service_account_iam_member.api_acts_as_tasks_invoker.member == "serviceAccount:${var.service_accounts["api"]}" &&
      strcontains(google_service_account_iam_member.api_acts_as_tasks_invoker.service_account_id, var.service_accounts["tasks_invoker"])
    )
    error_message = "api may actAs tasks-invoker only, on the single service account (T-1102b edge case)"
  }
}

run "sweep_every_15_minutes_with_oidc" {
  command = plan

  assert {
    condition = (
      google_cloud_scheduler_job.sweep.schedule == "*/15 * * * *" &&
      google_cloud_scheduler_job.sweep.time_zone == "Etc/UTC" &&
      google_cloud_scheduler_job.sweep.retry_config[0].retry_count == 1
    )
    error_message = "the sweep must run every 15 minutes in UTC with one retry (S7 5.12 API-INT-2)"
  }

  assert {
    condition = (
      google_cloud_scheduler_job.sweep.http_target[0].http_method == "POST" &&
      google_cloud_scheduler_job.sweep.http_target[0].uri == "${google_cloud_run_v2_service.worker.uri}/internal/v1/sweep"
    )
    error_message = "the sweep must POST the worker's /internal/v1/sweep (S7 5.12)"
  }

  assert {
    condition = (
      google_cloud_scheduler_job.sweep.http_target[0].oidc_token[0].service_account_email == var.service_accounts["scheduler_invoker"] &&
      google_cloud_scheduler_job.sweep.http_target[0].oidc_token[0].audience == google_cloud_run_v2_service.worker.uri
    )
    error_message = "the sweep must call as scheduler-invoker with worker's URL as the OIDC audience (S7 5.12)"
  }
}

run "deployer_has_no_kms_secret_or_firestore_role" {
  command = plan

  assert {
    condition = toset(concat(
      [for m in values(google_cloud_run_v2_service_iam_member.deployer_run_developer) : m.role],
      [for m in values(google_service_account_iam_member.deployer_acts_as_runtime) : m.role],
      [google_artifact_registry_repository_iam_member.deployer_writer.role],
      [google_project_iam_member.deployer_hosting_admin.role],
      [google_service_account_iam_member.deployer_wif.role],
      )) == toset([
      "roles/run.developer",
      "roles/iam.serviceAccountUser",
      "roles/artifactregistry.writer",
      "roles/firebasehosting.admin",
      "roles/iam.workloadIdentityUser",
    ])
    error_message = "the deployer must hold exactly run.developer, iam.serviceAccountUser, artifactregistry.writer, firebasehosting.admin and iam.workloadIdentityUser (V13.2.2)"
  }

  assert {
    condition = alltrue([
      for role in concat(
        [for m in values(google_cloud_run_v2_service_iam_member.deployer_run_developer) : m.role],
        [for m in values(google_service_account_iam_member.deployer_acts_as_runtime) : m.role],
        [google_artifact_registry_repository_iam_member.deployer_writer.role],
        [google_project_iam_member.deployer_hosting_admin.role],
        [google_service_account_iam_member.deployer_wif.role],
        ) : !(
        startswith(role, "roles/cloudkms.") ||
        startswith(role, "roles/secretmanager.") ||
        startswith(role, "roles/datastore.") ||
        startswith(role, "roles/firestore.") ||
        startswith(role, "roles/iam.serviceAccountTokenCreator") ||
        contains(["roles/owner", "roles/editor", "roles/viewer"], role)
      )
    ])
    error_message = "the deployer must hold no KMS, Secret Manager, Firestore, IAM-admin or primitive role (V13.2.2)"
  }
}

run "wif_condition_pins_repository_and_ref" {
  command = plan

  assert {
    condition     = google_iam_workload_identity_pool_provider.github.attribute_condition == "assertion.repository == 'difficultnerd/MailTinder' && assertion.ref == 'refs/heads/main'"
    error_message = "the WIF condition must pin exactly the MailTinder repository and main (V13.2.2)"
  }

  assert {
    condition = (
      google_iam_workload_identity_pool_provider.github.attribute_mapping["google.subject"] == "assertion.sub" &&
      google_iam_workload_identity_pool_provider.github.attribute_mapping["attribute.repository"] == "assertion.repository" &&
      google_iam_workload_identity_pool_provider.github.attribute_mapping["attribute.ref"] == "assertion.ref"
    )
    error_message = "the WIF provider must map subject, repository and ref"
  }

  assert {
    condition     = google_iam_workload_identity_pool_provider.github.oidc[0].issuer_uri == "https://token.actions.githubusercontent.com"
    error_message = "the WIF provider must trust the GitHub Actions OIDC issuer"
  }
}

run "max_instances_three" {
  command = plan

  assert {
    condition = alltrue([
      for s in [
        google_cloud_run_v2_service.api,
        google_cloud_run_v2_service.unsub,
        google_cloud_run_v2_service.worker,
      ] : s.template[0].scaling[0].max_instance_count == 3 && s.template[0].scaling[0].min_instance_count == 0
    ])
    error_message = "every service must scale 0..3 (S7 6 [ASSUMES] 3)"
  }
}

run "image_ignored_by_terraform" {
  command = plan

  assert {
    condition = alltrue([
      for s in [
        google_cloud_run_v2_service.api,
        google_cloud_run_v2_service.unsub,
        google_cloud_run_v2_service.worker,
      ] : s.template[0].containers[0].image == var.placeholder_image
    ])
    error_message = "Terraform must only ever reference the placeholder image; the pipeline owns the real one and the ignore_changes list that keeps it is enforced by the CI guard (T-1102b edge case)"
  }
}