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