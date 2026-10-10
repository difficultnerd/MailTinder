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