# Plan-time tests for the runtime module (T-1102b step 7). Every run uses
# mock_provider, so the suite needs no credentials and no network.
#
# The runs inspect the resources the module *declares*; like the foundation
# suite they cannot prove the absence of a resource type. The absence
# properties the task claims (no additional Cloud Run invoker, no
# roles/iam.serviceAccountUser on the project, no deployer KMS/Secret Manager/
# Firestore/IAM role, no primitive role, no `allUsers`/`allAuthenticatedUsers`
# outside `api_public`) are additionally enforced by the `terraform` CI job's
# role allowlist, actAs-scope guard, allUsers guard and image-ignore guard,
# which scan every `*.tf` file under `modules/` and `envs/`. In particular
# `ignore_changes` on the container image cannot be read from an expression, so
# its presence on every service is enforced by the CI image-ignore guard, and
# the run below checks the image Terraform is told to use.
mock_provider "google" {}
mock_provider "google-beta" {}

# The project number fixes each service's deterministic URL (used as the
# UNSUB_AUDIENCE / WORKER_AUDIENCE the services check).
override_data {
  target = data.google_project.current
  values = {
    number = "123456789012"
  }
}

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

  # A fake client id: the module takes the real one as an input (M1), so the
  # suite must supply one. It is a fixture, not a real identifier.
  google_oauth_client_id = "test-client-id.apps.googleusercontent.com"

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
      google_cloud_scheduler_job.sweep.http_target[0].uri == "${one([for e in google_cloud_run_v2_service.worker.template[0].containers[0].env : e.value if e.name == "WORKER_AUDIENCE"])}/internal/v1/sweep"
    )
    error_message = "the sweep must POST the worker's /internal/v1/sweep (S7 5.12)"
  }

  assert {
    condition = (
      google_cloud_scheduler_job.sweep.http_target[0].oidc_token[0].service_account_email == var.service_accounts["scheduler_invoker"] &&
      google_cloud_scheduler_job.sweep.http_target[0].oidc_token[0].audience == one([for e in google_cloud_run_v2_service.worker.template[0].containers[0].env : e.value if e.name == "WORKER_AUDIENCE"])
    )
    error_message = "the sweep's OIDC audience must equal the worker's WORKER_AUDIENCE, or worker rejects every sweep (S7 5.12)"
  }
}

run "internal_services_carry_their_own_oidc_audience" {
  command = plan

  assert {
    condition = anytrue([
      for e in google_cloud_run_v2_service.unsub.template[0].containers[0].env :
      e.name == "UNSUB_AUDIENCE" && e.value == "https://mt-unsub-123456789012.us-central1.run.app"
    ])
    error_message = "unsub must be given UNSUB_AUDIENCE, its own deterministic URL, or it refuses to start (S7 5.12)"
  }

  # M1: unsub also reads UNSUB_BASE_URL (parsed as a URL) and
  # GOOGLE_OAUTH_CLIENT_ID at start-up. Without them it fails to boot, so the
  # service carries both. UNSUB_BASE_URL is set from the same local as
  # UNSUB_AUDIENCE, so the audience Cloud Tasks signs for and the base URL the
  # service builds links against cannot diverge.
  assert {
    condition = anytrue([
      for e in google_cloud_run_v2_service.unsub.template[0].containers[0].env :
      e.name == "UNSUB_BASE_URL" && e.value == "https://mt-unsub-123456789012.us-central1.run.app"
    ])
    error_message = "unsub must be given UNSUB_BASE_URL, its own deterministic URL, or it refuses to start (M1)"
  }

  assert {
    condition = one([
      for e in google_cloud_run_v2_service.unsub.template[0].containers[0].env :
      e.value if e.name == "UNSUB_BASE_URL"
      ]) == one([
      for e in google_cloud_run_v2_service.unsub.template[0].containers[0].env :
      e.value if e.name == "UNSUB_AUDIENCE"
    ])
    error_message = "unsub's UNSUB_BASE_URL must equal its UNSUB_AUDIENCE (one source per URL), or the audience and the emitted links disagree (M1, S7 5.12)"
  }

  assert {
    condition = anytrue([
      for e in google_cloud_run_v2_service.unsub.template[0].containers[0].env :
      e.name == "GOOGLE_OAUTH_CLIENT_ID" && e.value == var.google_oauth_client_id
    ])
    error_message = "unsub must be given GOOGLE_OAUTH_CLIENT_ID from the module input, or it refuses to start (M1)"
  }

  assert {
    condition = anytrue([
      for e in google_cloud_run_v2_service.worker.template[0].containers[0].env :
      e.name == "WORKER_AUDIENCE" && e.value == "https://mt-worker-123456789012.us-central1.run.app"
    ])
    error_message = "worker must be given WORKER_AUDIENCE, its own deterministic URL, or it refuses to start (S7 5.12)"
  }

  assert {
    condition = one([
      for e in google_cloud_run_v2_service.api.template[0].containers[0].env :
      e.value if e.name == "MT_UNSUB_URL"
      ]) == one([
      for e in google_cloud_run_v2_service.unsub.template[0].containers[0].env :
      e.value if e.name == "UNSUB_AUDIENCE"
    ])
    error_message = "api's MT_UNSUB_URL must equal unsub's UNSUB_AUDIENCE (one source per URL), or every unsubscribe task 401s (S7 5.12)"
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
    condition     = google_iam_workload_identity_pool_provider.github.attribute_condition == "assertion.repository == 'difficultnerd/MailTinder' && assertion.ref == 'refs/heads/main' && assertion.environment == 'production'"
    error_message = "the WIF condition must pin exactly the MailTinder repository, main and the production environment (V13.2.2)"
  }

  assert {
    condition = (
      google_iam_workload_identity_pool_provider.github.attribute_mapping["google.subject"] == "assertion.sub" &&
      google_iam_workload_identity_pool_provider.github.attribute_mapping["attribute.repository"] == "assertion.repository" &&
      google_iam_workload_identity_pool_provider.github.attribute_mapping["attribute.ref"] == "assertion.ref" &&
      google_iam_workload_identity_pool_provider.github.attribute_mapping["attribute.environment"] == "assertion.environment"
    )
    error_message = "the WIF provider must map subject, repository, ref and environment"
  }

  assert {
    condition     = google_iam_workload_identity_pool_provider.github.oidc[0].issuer_uri == "https://token.actions.githubusercontent.com"
    error_message = "the WIF provider must trust the GitHub Actions OIDC issuer"
  }
}

# Staging is a separate project built from this same module (T-1103). Its
# deploy identity must pin ITS OWN GitHub environment: the staging root sets
# `deploy_environment = "staging"` and the staging deploy job names
# `environment: staging` (T-1104), or the WIF condition below rejects every
# staging token. Production keeps the `production` default above.
run "staging_environment_is_its_own_pin" {
  command = plan

  variables {
    env                = "staging"
    deploy_environment = "staging"
  }

  assert {
    condition     = google_iam_workload_identity_pool_provider.github.attribute_condition == "assertion.repository == 'difficultnerd/MailTinder' && assertion.ref == 'refs/heads/main' && assertion.environment == 'staging'"
    error_message = "the staging root must set deploy_environment = \"staging\" and its deploy job must name environment: staging, or the staging token is rejected (T-1103, T-1104)"
  }

  assert {
    condition     = google_iam_workload_identity_pool_provider.github.attribute_mapping["attribute.environment"] == "assertion.environment"
    error_message = "the WIF provider must map the environment claim in staging as well as production"
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