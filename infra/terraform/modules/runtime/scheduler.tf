# The sweep, every 15 minutes (S7 5.12 API-INT-2). Cloud Scheduler calls the
# internal `worker` service with an OIDC token whose audience is the service's
# own URL, so `worker` authenticates the caller from the token alone.
resource "google_cloud_scheduler_job" "sweep" {
  project   = var.project_id
  region    = var.region
  name      = "sweep"
  schedule  = "*/15 * * * *"
  time_zone = "Etc/UTC"

  http_target {
    uri         = "${google_cloud_run_v2_service.worker.uri}/internal/v1/sweep"
    http_method = "POST"
    body        = base64encode("{}")

    headers = {
      "Content-Type" = "application/json"
    }

    oidc_token {
      service_account_email = var.service_accounts["scheduler_invoker"]
      audience              = google_cloud_run_v2_service.worker.uri
    }
  }

  retry_config {
    retry_count = 1
  }
}