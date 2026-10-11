# Negative fixture for the T-1102b CI image-ignore guard (review F10).
#
# The container image traversal in the ignore list is commented out with a `//`
# line comment, so Terraform no longer ignores the image and every apply would
# roll production back to the placeholder. The guard must strip `//` comments
# before checking, count this service as missing the traversal, and fail.
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
      // template[0].containers[0].image,
      client,
    ]
  }
}
