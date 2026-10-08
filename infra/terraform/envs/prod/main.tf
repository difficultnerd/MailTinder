# Runtime/foundation modules are supplied by T-1102a/T-1102b and T-1103.
# Inputs are deliberately supplied at apply time; no account or email is stored.
terraform {
  required_version = ">= 1.7.0"
  required_providers {
    google = {
      source  = "hashicorp/google"
      version = "~> 6.0"
    }
  }
}
provider "google" {
  project = var.project_id
  region  = "us-central1"
}
variable "project_id" {
  type = string
}
variable "billing_account" {
  type = string
}
variable "alert_email" {
  type      = string
  sensitive = true
}
variable "app_url" {
  type = string
}
module "monitoring" {
  source          = "../../modules/monitoring"
  project_id      = var.project_id
  billing_account = var.billing_account
  alert_email     = var.alert_email
  app_url         = var.app_url
  enable_alerts   = true
}
