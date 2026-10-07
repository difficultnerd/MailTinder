# Remote state (S4 1: one state per environment, in a GCS bucket).
#
# The bucket name is never a real value in this repository. James creates
# gs://<project>-tfstate once (see ../../README.md) and then either replaces the
# placeholder below or, preferably, leaves it out and passes it at init time:
#
#   terraform init -backend-config="bucket=<project>-tfstate"
#
terraform {
  backend "gcs" {
    bucket = "REPLACE-WITH-PROJECT-tfstate"
    prefix = "prod"
  }
}