# Negative fixture for the T-1102b CI Cloud Run IAM guard (review F6).
#
# A second, root-level resource labelled `api_public` grants the public invoker
# on the internal `unsub` service. The guard pins each binding to its module
# file, target service, role and principal, so it must reject this.
#
# Fed to the guard by the CI self-test, not scanned as part of the real tree.
resource "google_cloud_run_v2_service_iam_member" "api_public" {
  project  = "mailtinder-test"
  location = "us-central1"
  name     = google_cloud_run_v2_service.unsub.name
  role     = "roles/run.invoker"
  member   = "allUsers"
}
