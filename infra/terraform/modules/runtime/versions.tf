# The runtime module (T-1102b). Two providers: `google` for Cloud Run, Cloud
# Tasks, Scheduler and IAM, and `google-beta` for Firebase Hosting, which has
# no GA resource yet.
terraform {
  required_version = ">= 1.9"

  required_providers {
    google = {
      source  = "hashicorp/google"
      version = "~> 7.0"
    }
    google-beta = {
      source  = "hashicorp/google-beta"
      version = "~> 7.0"
    }
  }
}