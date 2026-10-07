# Interface of the runtime module (T-1102b).
#
# `service_accounts`, `kms_key_id` and `secret_ids` are the outputs of the
# foundation module (T-1102a): this module wires those identities into the
# three Cloud Run services and their queue/schedule, and creates the GitHub
# Actions deploy identity.

variable "project_id" {
  type        = string
  description = "Google Cloud project that holds the production runtime (S4 1: one project per environment)."
}

variable "region" {
  type        = string
  default     = "us-central1"
  description = "Single region for every regional service (S4 1). Cloud Run, Cloud Tasks and Scheduler all take their location from it."
}

variable "env" {
  type        = string
  description = "Environment name; production and staging are separate projects built from this module (S4 1)."

  validation {
    condition     = contains(["prod", "staging"], var.env)
    error_message = "env is prod or staging"
  }
}

variable "service_accounts" {
  type        = map(string)
  description = <<-EOT
    Service account emails by short name, from the foundation module
    (`module.foundation.service_accounts`). Keys: api, unsub, worker,
    tasks_invoker, scheduler_invoker (S4 2).
  EOT
}

variable "kms_key_id" {
  type        = string
  description = "Key id of `data-key-kek`, the key that wraps every user's data_key (foundation output). Passed to the services as MT_KMS_KEY; the key itself is not a secret."
}

variable "secret_ids" {
  type        = map(string)
  description = <<-EOT
    Secret Manager container ids by short name, from the foundation module
    (`module.foundation.secret_ids`). Keys: oauth_client_secret, jev_api_key,
    email_lookup_hmac, log_pseudonym_hmac. Only the resource *names* are passed
    to the services; the values are read through Secret Manager at start-up
    (T-305), so no secret value ever reaches Terraform state or an env var.
  EOT
}

variable "google_oauth_client_id" {
  type        = string
  description = <<-EOT
    The Google OAuth client id the unsub service presents for the Gmail
    identity flow; it reads it as GOOGLE_OAUTH_CLIENT_ID at start-up and
    refuses to start without a non-empty value. It is deliberately an input
    with no default: a missing or blank value fails the plan loudly instead of
    deploying a service that cannot boot. Each environment has its own client
    id (staging's differs from production's, T-1103).
  EOT

  validation {
    condition     = length(trimspace(var.google_oauth_client_id)) > 0
    error_message = "google_oauth_client_id must be a non-empty Google OAuth client id (unsub refuses to start without it)."
  }
}

variable "github_repository" {
  type        = string
  default     = "difficultnerd/MailTinder"
  description = "The only repository whose GitHub Actions runs may impersonate the deployer (V13.2.2)."
}

variable "deploy_ref" {
  type        = string
  default     = "refs/heads/main"
  description = "The only ref whose GitHub Actions runs may impersonate the deployer. Production deploys additionally need the GitHub `production` environment approval (T-1104)."
}

variable "deploy_environment" {
  type        = string
  default     = "production"
  description = <<-EOT
    The only GitHub Actions environment whose OIDC token may impersonate the
    deployer. GitHub only puts an `environment` claim in the token when a job
    names one, so this WIF condition requires the token to carry the
    `production` claim. The approval itself is GitHub's: the `production`
    environment is configured with a required reviewer (T-1104), and the claim
    this condition demands is only as trustworthy as that environment's
    configuration - GitHub withholds the claim until the reviewer approves, and
    GCP then rejects any token without it. Staging is a separate project built
    from this same module (T-1103) and must set its own value (`"staging"`);
    its deploy job must then name `environment: staging` (T-1104), or the
    provider condition rejects the token. The production root keeps the
    `production` default.
  EOT
}

variable "max_instances" {
  type        = number
  default     = 3
  description = "Cloud Run max instance count per service (S7 6 [ASSUMES] 3: the effective per-instance rate limit is three times the per-instance figure)."
}

variable "placeholder_image" {
  type        = string
  default     = "us-docker.pkg.dev/cloudrun/container/hello"
  description = "Image every service starts with. Terraform ignores later changes to it: the pipeline (T-1104) owns the deployed image."
}