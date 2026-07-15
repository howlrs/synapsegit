variable "project_id" {
  description = "Existing billing-enabled Google Cloud project for the isolated non-production deployment."
  type        = string

  validation {
    condition     = can(regex("^[a-z][a-z0-9-]{4,28}[a-z0-9]$", var.project_id))
    error_message = "project_id must be a valid 6-30 character Google Cloud project ID."
  }
}

variable "region" {
  description = "Non-production Cloud Run and Artifact Registry region. This is not a production residency decision."
  type        = string
  default     = "asia-northeast1"

  validation {
    condition     = can(regex("^[a-z]+-[a-z]+[0-9]+$", var.region))
    error_message = "region must look like a Google Cloud region such as asia-northeast1."
  }
}

variable "deployer_email" {
  description = "Google user allowed to attach the dedicated runtime service account to the job."
  type        = string
  sensitive   = true

  validation {
    condition     = can(regex("^[^@[:space:]]+@[^@[:space:]]+$", var.deployer_email))
    error_message = "deployer_email must be an email address."
  }
}

variable "job_image" {
  description = "Optional Artifact Registry image pinned by sha256 digest. Leave null for the API/repository bootstrap apply."
  type        = string
  default     = null
  nullable    = true

  validation {
    condition = (
      var.job_image == null ||
      can(regex("^[a-z0-9.-]+-docker\\.pkg\\.dev/[a-z][a-z0-9-]{4,28}[a-z0-9]/[a-z0-9._-]+/[a-z0-9._/-]+@sha256:[0-9a-f]{64}$", var.job_image))
    )
    error_message = "job_image must be null or a complete Artifact Registry URI pinned with @sha256:<64 lowercase hex characters>."
  }
}
