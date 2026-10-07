# Service accounts. One identity per service, no keys are ever created
# `google_service_account_key` must not appear anywhere in this module: every
# caller authenticates with its own Google identity and short-lived OIDC tokens
# (S4 2, ASVS V13.2.1, V13.2.3).
locals {
  service_accounts = {
    api               = "MailTinder api (Cloud Run)"
    unsub             = "MailTinder unsubscribe (Cloud Run)"
    worker            = "MailTinder worker (Cloud Run)"
    tasks-invoker     = "Cloud Tasks invoker; OIDC identity for calls to unsub"
    scheduler-invoker = "Cloud Scheduler invoker; OIDC identity for calls to worker"
  }
}

resource "google_service_account" "service" {
  for_each = local.service_accounts

  project      = var.project_id
  account_id   = "mt-${each.key}"
  display_name = each.value
}

# The three runtime identities, by short name, for the IAM bindings below.
locals {
  runtime_service_accounts = {
    api    = google_service_account.service["api"].email
    unsub  = google_service_account.service["unsub"].email
    worker = google_service_account.service["worker"].email
  }
}