locals {
  labels = {
    application = "synapsegit"
    component   = "cli-smoke"
    environment = "development"
    managed_by  = "terraform"
  }

  required_services = toset([
    "artifactregistry.googleapis.com",
    "cloudbuild.googleapis.com",
    "iam.googleapis.com",
    "logging.googleapis.com",
    "run.googleapis.com",
    "storage.googleapis.com",
  ])
}

data "google_project" "current" {
  project_id = var.project_id
}

resource "google_project_service" "required" {
  for_each = local.required_services

  project            = var.project_id
  service            = each.value
  disable_on_destroy = false
}

resource "google_artifact_registry_repository" "containers" {
  project       = var.project_id
  location      = var.region
  repository_id = "synapsegit"
  description   = "Private non-production SynapseGit OCI images"
  format        = "DOCKER"
  labels        = local.labels

  docker_config {
    immutable_tags = true
  }

  depends_on = [google_project_service.required]
}

resource "google_service_account" "cli_smoke" {
  project      = var.project_id
  account_id   = "synapsegit-cli-smoke"
  display_name = "SynapseGit CLI smoke runtime"
  description  = "Dedicated no-project-role identity for the private non-production Cloud Run Job."

  depends_on = [google_project_service.required]
}

resource "google_service_account" "container_builder" {
  project      = var.project_id
  account_id   = "synapsegit-build"
  display_name = "SynapseGit container builder"
  description  = "Dedicated Cloud Build identity for building and pushing the non-production CLI smoke image."

  depends_on = [google_project_service.required]
}

resource "google_storage_bucket" "cloud_build_source" {
  project                     = var.project_id
  name                        = "${var.project_id}-cloud-build-source"
  location                    = var.region
  force_destroy               = false
  uniform_bucket_level_access = true
  public_access_prevention    = "enforced"
  labels                      = local.labels

  lifecycle_rule {
    condition {
      age = 7
    }

    action {
      type = "Delete"
    }
  }

  depends_on = [google_project_service.required]
}

resource "google_service_account_iam_member" "deployer_can_attach_runtime" {
  service_account_id = google_service_account.cli_smoke.name
  role               = "roles/iam.serviceAccountUser"
  member             = "user:${var.deployer_email}"
}

resource "google_service_account_iam_member" "deployer_can_attach_builder" {
  service_account_id = google_service_account.container_builder.name
  role               = "roles/iam.serviceAccountUser"
  member             = "user:${var.deployer_email}"
}

resource "google_service_account_iam_member" "cloud_build_can_issue_builder_tokens" {
  service_account_id = google_service_account.container_builder.name
  role               = "roles/iam.serviceAccountTokenCreator"
  member             = "serviceAccount:service-${data.google_project.current.number}@gcp-sa-cloudbuild.iam.gserviceaccount.com"
}

resource "google_storage_bucket_iam_member" "builder_reads_source" {
  bucket = google_storage_bucket.cloud_build_source.name
  role   = "roles/storage.objectViewer"
  member = google_service_account.container_builder.member
}

resource "google_artifact_registry_repository_iam_member" "builder_pushes_images" {
  project    = var.project_id
  location   = google_artifact_registry_repository.containers.location
  repository = google_artifact_registry_repository.containers.repository_id
  role       = "roles/artifactregistry.writer"
  member     = google_service_account.container_builder.member
}

resource "google_project_iam_member" "builder_writes_logs" {
  project = var.project_id
  role    = "roles/logging.logWriter"
  member  = google_service_account.container_builder.member
}

resource "google_cloud_run_v2_job" "cli_smoke" {
  count = var.job_image == null ? 0 : 1

  project             = var.project_id
  name                = "synapsegit-cli-smoke"
  location            = var.region
  deletion_protection = false
  labels              = local.labels

  template {
    task_count  = 1
    parallelism = 1

    template {
      service_account = google_service_account.cli_smoke.email
      max_retries     = 0
      timeout         = "600s"

      containers {
        name  = "synapsegit-cli-smoke"
        image = var.job_image

        resources {
          limits = {
            cpu    = "1"
            memory = "512Mi"
          }
        }
      }
    }
  }

  depends_on = [
    google_artifact_registry_repository.containers,
    google_service_account_iam_member.deployer_can_attach_runtime,
  ]
}
