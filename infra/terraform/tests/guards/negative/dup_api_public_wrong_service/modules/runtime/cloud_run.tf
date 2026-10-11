# Negative fixture for the T-1102b CI Cloud Run IAM guard (review F6).
#
# Same duplicate public invoker on the internal `unsub` service, but placed at
# the module path so the file-path pin alone would not catch it: the guard must
# fail on the target-service (and principal) pin instead. Fed to the guard by
# the CI self-test.
resource "google_cloud_run_v2_service_iam_member" "api_public" {
  project  = "mailtinder-test"
  location = "us-central1"
  name     = google_cloud_run_v2_service.unsub.name
  role     = "roles/run.invoker"
  member   = "allUsers"
}
