# Negative fixture for the T-1102b CI image-ignore guard (review F10).
#
# The two `locals` below hold the strings "/*" and "*/" - they are not comments.
# A comment stripper that does not track quoted strings treats the "/*" as the
# start of a block comment and the "*/" as its end, discarding the API service
# between them; the guard then sees only two services and passes even though API
# no longer ignores the container image. A string-aware stripper counts all
# three services and rejects this fixture because `api` is missing the traversal.
#
# Fed to the guard by the CI self-test, not scanned as part of the real tree.
locals {
  harmless_open = "/* this is a string, not a comment"
}

resource "google_cloud_run_v2_service" "api" {
  project             = "mailtinder-test"
  name                = "mt-api"
  location            = "us-central1"
  deletion_protection = true

  template {
    containers {
      image = "us-docker.pkg.dev/cloudrun/container/hello"
    }
  }

  lifecycle {
    ignore_changes = [
      client,
    ]
  }
}

locals {
  harmless_close = "*/ this is a string, not a comment"
}

resource "google_cloud_run_v2_service" "unsub" {
  project             = "mailtinder-test"
  name                = "mt-unsub"
  location            = "us-central1"
  deletion_protection = true

  template {
    containers {
      image = "us-docker.pkg.dev/cloudrun/container/hello"
    }
  }

  lifecycle {
    ignore_changes = [
      template[0].containers[0].image,
      client,
    ]
  }
}

resource "google_cloud_run_v2_service" "worker" {
  project             = "mailtinder-test"
  name                = "mt-worker"
  location            = "us-central1"
  deletion_protection = true

  template {
    containers {
      image = "us-docker.pkg.dev/cloudrun/container/hello"
    }
  }

  lifecycle {
    ignore_changes = [
      template[0].containers[0].image,
      client,
    ]
  }
}
