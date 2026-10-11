# Remote state (S4 1: one state per environment, in a GCS bucket).
#
# Staging has its OWN bucket, gs://<staging-project>-tfstate: the production
# bucket is never reused (T-1103 edge case), so a staging destroy can never
# touch production state. James creates the bucket once (see ../../README.md)
# and then either replaces the placeholder below or, preferably, leaves it out
# and passes it at init time:
#
#   terraform init -backend-config="bucket=<staging-project>-tfstate"
#
terraform {
  backend "gcs" {
    bucket = "REPLACE-WITH-STAGING-PROJECT-tfstate"
    prefix = "staging"
  }
}
