# Negative fixture for the T-1102b CI scalar-role guard (review F5).
#
# The role value starts with a quoted `roles/...` literal, so a prefix check
# accepts it, but it is a conditional expression that can evaluate to any role,
# including a primitive one. The guard must reject the whole expression.
#
# This file is not part of any module and is not scanned by the real-tree guards
# (which read modules/ and envs/ only); the CI self-test feeds it to the guard
# expecting a failure.
variable "review_role" {
  type = string
}

resource "google_project_iam_member" "conditional_role" {
  project = "mailtinder-test"
  role    = "roles/datastore.user" == "" ? var.review_role : var.review_role
  member  = "serviceAccount:mt-api@mailtinder-test.iam.gserviceaccount.com"
}
