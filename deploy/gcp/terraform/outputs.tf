output "artifact_registry_image_prefix" {
  description = "Prefix used when building the SynapseGit CLI smoke image."
  value       = "${var.region}-docker.pkg.dev/${var.project_id}/${google_artifact_registry_repository.containers.repository_id}/synapsegit-cli-smoke"
}

output "runtime_service_account" {
  description = "No-project-role service identity attached to the smoke job."
  value       = google_service_account.cli_smoke.email
}

output "build_service_account" {
  description = "Least-privilege service identity used by Cloud Build."
  value       = google_service_account.container_builder.email
}

output "cloud_build_source_bucket" {
  description = "Private short-lived staging bucket used for Cloud Build source archives."
  value       = google_storage_bucket.cloud_build_source.name
}

output "job_name" {
  description = "Cloud Run Job name after the digest-pinned deployment apply."
  value       = var.job_image == null ? null : google_cloud_run_v2_job.cli_smoke[0].name
}
