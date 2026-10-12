variable "project_id" {
  type        = string
  description = "Google Cloud project that holds the production foundation (S4 1: one project per environment)."
}

variable "region" {
  type        = string
  default     = "us-central1"
  description = "Single region for every regional service (S4 1: Cloud Run, Cloud Tasks, Firestore, KMS, Secret Manager, Vertex AI, log buckets)."
}

variable "env" {
  type        = string
  description = "Environment name; production and staging are separate projects built from this module (S4 1)."

  validation {
    condition     = contains(["prod", "staging"], var.env)
    error_message = "env is prod or staging"
  }
}

variable "lock_log_bucket" {
  type        = bool
  default     = true
  description = "Lock the 90-day log bucket so its retention cannot be reduced. Locking is irreversible; staging sets false (T-1103)."
}

variable "kms_rotation_days" {
  type        = number
  default     = 365
  description = "KMS key rotation period in days (S6 5: yearly, automatic)."
}

variable "additional_data_key_holders" {
  type        = list(string)
  default     = []
  description = <<-EOT
    Extra members to add to the `data-key-kek` binding, as full IAM members
    (`serviceAccount:...`). Empty everywhere but staging: production keeps
    exactly api, unsub and worker (S4 2, S6 5). The staging root passes its
    smoke identity (T-1105), so staging has four holders of a key that protects
    synthetic data only. The binding stays authoritative - this list is what
    lets the extra holder be granted *in band*, instead of a second
    `google_kms_crypto_key_iam_member` fighting the binding on every apply.
  EOT
}