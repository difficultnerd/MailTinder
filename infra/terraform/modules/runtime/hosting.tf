# Firebase Hosting (S4 1). The site id is stable (`mailtinder-<env>`) so the
# Flutter web build and `firebase.json` (T-006) can name it before the site
# exists. Headers and rewrites live in `firebase.json` and are deployed by the
# pipeline (T-1104); Terraform only creates the project link and the site.
# Firebase has no GA resource yet, so these use the `google-beta` provider.
resource "google_firebase_project" "default" {
  provider = google-beta
  project  = var.project_id
}

resource "google_firebase_hosting_site" "app" {
  provider = google-beta
  project  = var.project_id
  site_id  = "mailtinder-${var.env}"

  depends_on = [google_firebase_project.default]
}