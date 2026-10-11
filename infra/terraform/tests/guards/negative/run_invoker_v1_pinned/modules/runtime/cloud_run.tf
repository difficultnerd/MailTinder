# Negative fixture for the T-1102b CI Cloud Run IAM guard (review F6).
#
# This binding is shaped like the pinned `unsub_tasks_invoker` binding (same
# label, target service, role and principal), so a guard that matched only the
# v2 resource type would accept it. It uses the v1 family
# (`google_cloud_run_service_iam_member`) on a v2-created service, which the
# guard must reject on the resource-type pin alone.
#
# Placed at a module-shaped path so the file-path pin cannot hide the type bug.
# Fed to the guard by the CI self-test, not scanned as part of the real tree.
resource "google_cloud_run_service_iam_member" "unsub_tasks_invoker" {
  project  = "mailtinder-test"
  location = "us-central1"
  service  = google_cloud_run_v2_service.unsub.name
  role     = "roles/run.invoker"
  member   = "serviceAccount:mt-tasks-invoker@mailtinder-test.iam.gserviceaccount.com"
}
