# The monitoring module (T-1107). Cloud Monitoring alert policies and uptime
# checks, Cloud Logging metrics and Cloud Billing budgets all come from the
# `google` provider; no beta resource is used.
terraform {
  required_version = ">= 1.9"

  required_providers {
    google = {
      source  = "hashicorp/google"
      version = "~> 7.0"
    }
  }
}
