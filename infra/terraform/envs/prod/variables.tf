variable "project_id" {
  type        = string
  description = "Production project id (created once by James, see ../../README.md)."
}

variable "region" {
  type        = string
  default     = "us-central1"
  description = "Single region for every regional service (S4 1). Do not change for production."
}

variable "lock_log_bucket" {
  type        = bool
  default     = true
  description = "Lock the 90-day log bucket. Locking cannot be undone; production keeps it true."
}