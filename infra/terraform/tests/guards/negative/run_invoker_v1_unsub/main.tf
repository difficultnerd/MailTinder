# Negative fixture for the T-1102b CI Cloud Run IAM guard (review F6).
#
# The v1 Cloud Run IAM resource family (`google_cloud_run_service_iam_member`)
# is still supported and can grant roles/run.invoker on a service created by the
# v2 resource. A guard that inspects only `google_cloud_run_v2_service_iam_*`
# ignores this binding: invoker on the internal `unsub` service, under a fresh
# label, for another runtime identity. The guard must fail on any run.invoker
# grant that is not one of the four pinned bindings.
#
# Fed to the guard by the CI self-test, not scanned as part of the real tree.
resource "google_cloud_run_service_iam_member" "sneaky_invoker" {
  project  = "mailtinder-test"
  location = "us-central1"
  service  = google_cloud_run_v2_service.unsub.name
  role     = "roles/run.invoker"
  member   = "serviceAccount:mt-worker@mailtinder-test.iam.gserviceaccount.com"
}
