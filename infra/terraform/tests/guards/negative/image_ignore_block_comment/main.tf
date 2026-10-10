# Negative fixture for the T-1102b CI image-ignore guard (review F10).
#
# The container image traversal is hidden inside a `/* ... */` block comment, so
# Terraform no longer ignores the image. The guard must strip block comments -
# including across lines - before checking the ignore list, count this service
# as missing the traversal, and fail.
#
# Fed to the guard by the CI self-test, not scanned as part of the real tree.
resource "google_cloud_run_v2_service" "fixture" {
  project             = "mailtinder-test"
  name                = "mt-fixture"
  location            = "us-central1"
  deletion_protection = true

  template {
    containers {
      image = "us-docker.pkg.dev/cloudrun/container/hello"
    }
  }

  lifecycle {
    ignore_changes = [
      /* the deploy pipeline owns the image, so terraform must not act on it:
      template[0].containers[0].image,
      */
      client,
    ]
  }
}
