# The unsubscribe Cloud Tasks queue (S7 5.12 API-INT-1). The API enqueues one
# task per unsubscribe job; Cloud Tasks calls the internal `unsub` service with
# an OIDC token, and a retryable 503 is retried with backoff.
resource "google_cloud_tasks_queue" "unsubscribe" {
  project  = var.project_id
  name     = "unsubscribe"
  location = var.region

  retry_config {
    max_attempts = 4
    min_backoff  = "30s"
    max_backoff  = "300s"
  }

  rate_limits {
    max_dispatches_per_second = 5
    max_concurrent_dispatches = 10
  }
}

# `api` may enqueue and delete tasks on this queue and no other: the binding is
# on the queue resource, not the project (V13.2.2).
resource "google_cloud_tasks_queue_iam_member" "api_enqueuer" {
  project  = var.project_id
  location = google_cloud_tasks_queue.unsubscribe.location
  name     = google_cloud_tasks_queue.unsubscribe.name
  role     = "roles/cloudtasks.enqueuer"
  member   = "serviceAccount:${var.service_accounts["api"]}"
}

resource "google_cloud_tasks_queue_iam_member" "api_task_deleter" {
  project  = var.project_id
  location = google_cloud_tasks_queue.unsubscribe.location
  name     = google_cloud_tasks_queue.unsubscribe.name
  role     = "roles/cloudtasks.taskDeleter"
  member   = "serviceAccount:${var.service_accounts["api"]}"
}

# To create a task that runs as `tasks-invoker`, `api` must be allowed to act as
# that one identity. `actAs` is granted on a single service account, never on
# the project (T-1102b edge case, V13.2.1).
resource "google_service_account_iam_member" "api_acts_as_tasks_invoker" {
  service_account_id = "projects/${var.project_id}/serviceAccounts/${var.service_accounts["tasks_invoker"]}"
  role               = "roles/iam.serviceAccountUser"
  member             = "serviceAccount:${var.service_accounts["api"]}"
}