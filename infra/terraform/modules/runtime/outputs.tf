# Outputs of the runtime module (T-1102b), exactly the interface the task names.

output "api_url" {
  description = "Cloud Run URL of the public API service (S4 1)."
  value       = google_cloud_run_v2_service.api.uri
}

output "unsub_url" {
  description = "Cloud Run URL of the internal unsubscribe service; also the OIDC audience Cloud Tasks signs for (S7 5.12)."
  value       = google_cloud_run_v2_service.unsub.uri
}

output "worker_url" {
  description = "Cloud Run URL of the internal worker service; also the OIDC audience Cloud Scheduler signs for (S7 5.12)."
  value       = google_cloud_run_v2_service.worker.uri
}

output "queue_id" {
  description = "Cloud Tasks queue the API enqueues unsubscribe jobs on (S7 5.12)."
  value       = google_cloud_tasks_queue.unsubscribe.id
}

output "hosting_site_id" {
  description = "Firebase Hosting site id the Flutter web build is deployed to (S4 1)."
  value       = google_firebase_hosting_site.app.site_id
}

output "wif_provider" {
  description = "Workload Identity Federation provider GitHub Actions authenticates against (V13.2.2)."
  value       = google_iam_workload_identity_pool_provider.github.name
}

output "deployer_email" {
  description = "The deploy service account the pipeline impersonates; it has no key (V13.2.1)."
  value       = google_service_account.deployer.email
}