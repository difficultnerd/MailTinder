# Artifact Registry: the Docker repository CI and Cloud Run read images from.
# Container scanning is deliberately not enabled here: CLAUDE.md keeps it out of
# the core (S10 12, Waits on S11), so containerscanning.googleapis.com is not
# among the APIs this module enables.
resource "google_artifact_registry_repository" "mailtinder" {
  project       = var.project_id
  location      = var.region
  repository_id = "mailtinder"
  description   = "MailTinder container images"
  format        = "DOCKER"
}