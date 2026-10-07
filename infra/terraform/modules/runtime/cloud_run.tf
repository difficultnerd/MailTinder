# The three Cloud Run services (S4 1, S7 5.12). Terraform owns every setting
# except the container image: the pipeline (T-1104) deploys the image, so the
# lifecycle block ignores it and Terraform never rolls a deploy back.
# `deletion_protection` keeps a mis-fired destroy from taking production down.
#
# Environment contract. The task names MT_* variables; where a service in this
# repository already reads a different name, both are set and the mapping is
# recorded here (the pull request notes it):
#   project   GOOGLE_CLOUD_PROJECT (read by unsub, worker) and MT_PROJECT_ID
#   region    MT_REGION
#   KMS key   MT_KMS_KEY
#   secrets   MT_SECRET_* (_container_ names only: the services read the values
#             through Secret Manager at start-up, T-305, so no value is in state)
#   audience  UNSUB_AUDIENCE / WORKER_AUDIENCE is each service's own URL (S7
#             5.12). A service cannot reference its own `uri` inside its own
#             resource (a cycle), and no other task sets these variables, so the
#             URL is rebuilt deterministically from the project number here:
#             https://<name>-<project-number>.<region>.run.app. The scheduler's
#             OIDC audience and the API's MT_UNSUB_URL carry the same URLs.
#   caller    UNSUB_TASKS_CALLER / WORKER_SCHEDULER_CALLER

# A Cloud Run service's URL is deterministic: name, project number, region. The
# runtime accounts need it as the audience they check (UNSUB_AUDIENCE /
# WORKER_AUDIENCE), so it is derived from the project number rather than from
# the service's own (cyclic) `uri`.
data "google_project" "current" {
  project_id = var.project_id
}

locals {
  unsub_url  = "https://mt-unsub-${data.google_project.current.number}.${var.region}.run.app"
  worker_url = "https://mt-worker-${data.google_project.current.number}.${var.region}.run.app"
}
locals {
  # Variables every service gets. Only non-secret values: the secret *names* go
  # to the service, never a value (ASVS V13.3.1).
  base_env = {
    MT_ENV                    = var.env
    MT_REGION                 = var.region
    MT_PROJECT_ID             = var.project_id
    GOOGLE_CLOUD_PROJECT      = var.project_id
    MT_KMS_KEY                = var.kms_key_id
    MT_SECRET_OAUTH_CLIENT    = var.secret_ids["oauth_client_secret"]
    MT_SECRET_JEV_API_KEY     = var.secret_ids["jev_api_key"]
    MT_SECRET_EMAIL_LOOKUP    = var.secret_ids["email_lookup_hmac"]
    MT_SECRET_LOG_PSEUDONYM   = var.secret_ids["log_pseudonym_hmac"]
    CLASSIFIER_GEMINI_ENABLED = "false"
    CLASSIFIER_JEV_ENABLED    = "false"
  }

  api_env = merge(local.base_env, {
    MT_TASKS_QUEUE      = google_cloud_tasks_queue.unsubscribe.id
    MT_UNSUB_URL        = local.unsub_url
    MT_TASKS_INVOKER_SA = var.service_accounts["tasks_invoker"]
  })

  unsub_env = merge(local.base_env, {
    UNSUB_TASKS_CALLER = var.service_accounts["tasks_invoker"]
    UNSUB_AUDIENCE     = local.unsub_url
  })

  worker_env = merge(local.base_env, {
    WORKER_SCHEDULER_CALLER = var.service_accounts["scheduler_invoker"]
    WORKER_AUDIENCE         = local.worker_url
  })
}

resource "google_cloud_run_v2_service" "api" {
  project             = var.project_id
  name                = "mt-api"
  location            = var.region
  ingress             = "INGRESS_TRAFFIC_ALL"
  deletion_protection = true

  template {
    service_account = var.service_accounts["api"]
    timeout         = "30s"

    scaling {
      min_instance_count = 0
      max_instance_count = var.max_instances
    }

    containers {
      image = var.placeholder_image

      resources {
        limits = {
          cpu    = "1"
          memory = "512Mi"
        }
      }

      dynamic "env" {
        for_each = local.api_env
        content {
          name  = env.key
          value = env.value
        }
      }
    }
  }

  lifecycle {
    ignore_changes = [
      template[0].containers[0].image,
      client,
      client_version,
    ]
  }
}

resource "google_cloud_run_v2_service" "unsub" {
  project             = var.project_id
  name                = "mt-unsub"
  location            = var.region
  ingress             = "INGRESS_TRAFFIC_INTERNAL_ONLY"
  deletion_protection = true

  template {
    service_account = var.service_accounts["unsub"]
    timeout         = "60s"

    scaling {
      min_instance_count = 0
      max_instance_count = var.max_instances
    }

    containers {
      image = var.placeholder_image

      resources {
        limits = {
          cpu    = "1"
          memory = "512Mi"
        }
      }

      dynamic "env" {
        for_each = local.unsub_env
        content {
          name  = env.key
          value = env.value
        }
      }
    }
  }

  lifecycle {
    ignore_changes = [
      template[0].containers[0].image,
      client,
      client_version,
    ]
  }
}

resource "google_cloud_run_v2_service" "worker" {
  project             = var.project_id
  name                = "mt-worker"
  location            = var.region
  ingress             = "INGRESS_TRAFFIC_INTERNAL_ONLY"
  deletion_protection = true

  template {
    service_account = var.service_accounts["worker"]
    timeout         = "300s"

    scaling {
      min_instance_count = 0
      max_instance_count = var.max_instances
    }

    containers {
      image = var.placeholder_image

      resources {
        limits = {
          cpu    = "1"
          memory = "512Mi"
        }
      }

      dynamic "env" {
        for_each = local.worker_env
        content {
          name  = env.key
          value = env.value
        }
      }
    }
  }

  lifecycle {
    ignore_changes = [
      template[0].containers[0].image,
      client,
      client_version,
    ]
  }
}

# `api` is public on purpose: Firebase Hosting rewrites the app's `/api/*`
# requests to it and it must be reachable without auth (S4 1). `unsub` and
# `worker` are internal-ingress and their only invokers are the Cloud Tasks and
# Cloud Scheduler identities, so a caller is authenticated by OIDC alone
# (S7 5.12, V12.3.3, V13.2.1).
resource "google_cloud_run_v2_service_iam_member" "api_public" {
  project  = var.project_id
  location = google_cloud_run_v2_service.api.location
  name     = google_cloud_run_v2_service.api.name
  role     = "roles/run.invoker"
  member   = "allUsers"
}

resource "google_cloud_run_v2_service_iam_member" "unsub_tasks_invoker" {
  project  = var.project_id
  location = google_cloud_run_v2_service.unsub.location
  name     = google_cloud_run_v2_service.unsub.name
  role     = "roles/run.invoker"
  member   = "serviceAccount:${var.service_accounts["tasks_invoker"]}"
}

resource "google_cloud_run_v2_service_iam_member" "worker_scheduler_invoker" {
  project  = var.project_id
  location = google_cloud_run_v2_service.worker.location
  name     = google_cloud_run_v2_service.worker.name
  role     = "roles/run.invoker"
  member   = "serviceAccount:${var.service_accounts["scheduler_invoker"]}"
}