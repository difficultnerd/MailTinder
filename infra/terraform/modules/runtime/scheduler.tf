# The sweep, every 15 minutes (S7 5.12 API-INT-2). Cloud Scheduler calls the
# internal `worker` service with an OIDC token whose audience is the service's
# own URL, so `worker` authenticates the caller from the token alone.
#
# The URL comes from `local.worker_url` (the same source `WORKER_AUDIENCE` is
# built from), not from `google_cloud_run_v2_service.worker.uri`. Cloud Run
# services have more than one URL form, so the audience the sweep sends must be
# byte-identical to the audience the service checks or every sweep gets a 401.
resource "google_cloud_scheduler_job" "sweep" {
  project   = var.project_id
  region    = var.region
  name      = "sweep"
  schedule  = "*/15 * * * *"
  time_zone = "Etc/UTC"

  http_target {
    uri         = "${local.worker_url}/internal/v1/sweep"
    http_method = "POST"
    body        = base64encode("{}")

    headers = {
      "Content-Type" = "application/json"
    }

    oidc_token {
      service_account_email = var.service_accounts["scheduler_invoker"]
      audience              = local.worker_url
    }
  }

  retry_config {
    retry_count = 1
  }
}