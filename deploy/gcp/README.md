# GCP non-production CLI smoke deployment

This directory contains the first deployable cloud artifact for the currently
implemented SynapseGit code: a **private, one-shot Cloud Run Job** that runs the
local CLI against disposable files and exits.

It is a deployment and packaging smoke test only. It is not the public cloud
service described in [`docs/cloud_service_architecture.md`](../../docs/cloud_service_architecture.md):

- it has no HTTP endpoint or public ingress;
- it does not persist its temporary filesystem;
- it has no GCS/PostgreSQL authority adapter, OIDC, or tenant boundary;
- it grants its runtime service account no project role; and
- it must not be used to enable the Human Decision cloud endpoint.

## Managed resources

The Terraform root in [`terraform/main.tf`](./terraform/main.tf) manages only:

- the Artifact Registry, Cloud Build, IAM, Logging, and Cloud Run APIs;
- one private Artifact Registry Docker repository;
- one private source-staging bucket whose objects expire after seven days;
- one dedicated build service account with source-read, repository-write, and
  log-write access only;
- one dedicated runtime service account with no project-level role; and
- optionally, one digest-pinned Cloud Run Job.

The Google Cloud project and billing link are deliberate bootstrap resources
and are not destroyed by this Terraform root. Use an isolated, billing-enabled
non-production project. Do not reuse a project that hosts another application.

`asia-northeast1` is the development default because this initial deployment is
being operated from Japan. It is not the production residency or DR-region
decision.

## Prerequisites

- Google Cloud CLI authenticated as the intended deployer
- Terraform 1.15.8
- a billing-enabled isolated development project
- permission to enable APIs, manage Artifact Registry and Cloud Run Jobs, and
  act as the dedicated runtime service account

Terraform and the Google provider are pinned in `versions.tf` and
`.terraform.lock.hcl`. Do not commit `terraform.tfvars`, plans, or state.
Before this setup becomes shared or production infrastructure, migrate state
to a restricted, versioned remote backend in an administration boundary.

## 1. Guard the target account and project

Set shell values explicitly. Every `gcloud` command below also supplies the
account and project so an unrelated active CLI configuration is not used.

```bash
export ACCOUNT='deployer@example.com'
export PROJECT_ID='replace-with-isolated-dev-project'
export REGION='asia-northeast1'

gcloud auth list --filter="account=${ACCOUNT}" \
  --format='value(account,status)'
gcloud billing projects describe "${PROJECT_ID}" \
  --account="${ACCOUNT}" \
  --format='value(billingEnabled,billingAccountName)'
```

Stop if the account is not authenticated or `billingEnabled` is not `True`.
If linking a new project returns `Cloud billing quota exceeded`, request a
billing-project quota increase or have the owner explicitly choose an obsolete
project to detach. Do not detach or reuse an existing application project as
an automated workaround.

## 2. Bootstrap APIs, registry, and runtime identity

From `deploy/gcp/terraform`:

```bash
cp terraform.tfvars.example terraform.tfvars
```

Set `project_id`, `region`, and `deployer_email` in the ignored file. Leave
`job_image` unset for the first apply.

```bash
terraform init
terraform fmt -check -recursive
terraform validate
terraform plan -input=false -out=bootstrap.tfplan
terraform apply bootstrap.tfplan
```

Review the plan before applying it. The bootstrap apply creates no running
compute and no public endpoint.

## 3. Build and push one immutable image tag

From the repository root:

```bash
export REVISION="$(git rev-parse --short=12 HEAD)"
export IMAGE_PREFIX="${REGION}-docker.pkg.dev/${PROJECT_ID}/synapsegit/synapsegit-cli-smoke"
export IMAGE_TAG="${IMAGE_PREFIX}:${REVISION}"

gcloud builds submit . \
  --config=cloudbuild.yaml \
  --substitutions="_IMAGE=${IMAGE_TAG}" \
  --gcs-source-staging-dir="gs://${PROJECT_ID}-cloud-build-source/source" \
  --project="${PROJECT_ID}" \
  --account="${ACCOUNT}" \
  --region="${REGION}"
```

Artifact Registry tags are immutable, so use a new source revision tag for
each build. Resolve the registry digest after the build:

```bash
export DIGEST="$(gcloud artifacts docker images describe "${IMAGE_TAG}" \
  --project="${PROJECT_ID}" \
  --account="${ACCOUNT}" \
  --format='value(image_summary.digest)')"
export IMAGE_DIGEST="${IMAGE_PREFIX}@${DIGEST}"

test -n "${DIGEST}"
```

## 4. Deploy by digest

Set the untracked `job_image` value in `terraform.tfvars` to
`IMAGE_DIGEST`, then review and apply the second plan:

```bash
terraform plan -input=false -out=job.tfplan
terraform apply job.tfplan
```

Terraform rejects mutable tags for `job_image`.

## 5. Execute and verify

```bash
gcloud run jobs execute synapsegit-cli-smoke \
  --wait \
  --region="${REGION}" \
  --project="${PROJECT_ID}" \
  --account="${ACCOUNT}"

gcloud run jobs logs read synapsegit-cli-smoke \
  --limit=100 \
  --region="${REGION}" \
  --project="${PROJECT_ID}" \
  --account="${ACCOUNT}"
```

Success requires the job to exit with status zero after `creator-run`,
`creator-report`, and `fsck`. The temporary repository is intentionally lost
when the task exits; persistence belongs to the later provider-neutral object
and PostgreSQL authority implementation.

## Cleanup

For this isolated development root, review `terraform plan -destroy` before
running `terraform destroy`. Destroying the root removes the job, runtime
identity, and registry; enabled APIs are deliberately left enabled. It does not
delete the project or alter its billing link.
