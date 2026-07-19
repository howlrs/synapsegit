mod support;

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};
use support::approved_decide;
use synapse_artifact::{
    ArtifactDecisionOptions, ArtifactDisposition, ArtifactLimits, ArtifactManifestEntry,
    ArtifactSourceAttribution, PendingArtifactState, RegularFileManifest,
    TrustedArtifactProjectConfig, WorkflowError, begin_artifact_proposal,
};
use synapse_core::Repository;

static NEXT_TEMP: AtomicU64 = AtomicU64::new(1);

struct TempProject {
    path: PathBuf,
}

impl TempProject {
    fn new(label: &str) -> Self {
        let serial = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        Self {
            path: std::env::temp_dir().join(format!(
                "synapse-artifact-workflow-{label}-{}-{nanos}-{serial}",
                std::process::id()
            )),
        }
    }

    fn config(&self, key: &str) -> TrustedArtifactProjectConfig {
        TrustedArtifactProjectConfig::new(
            &self.path,
            key,
            "Artifact Creator",
            "Application-owned AI",
            "2026-07-19T00:00:00.000000000Z",
            "2099-01-01T00:00:00.000000000Z",
        )
    }
}

impl Drop for TempProject {
    fn drop(&mut self) {
        if self.path.starts_with(std::env::temp_dir()) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}

fn manifest(label: &str) -> RegularFileManifest {
    RegularFileManifest::from_entries(
        [
            ArtifactManifestEntry::regular_file(
                "index.html",
                format!("<!doctype html><title>{label}</title>").into_bytes(),
            ),
            ArtifactManifestEntry::regular_file(
                "assets/site.css",
                format!("/* {label} */ body {{ color: #123456; }}").into_bytes(),
            ),
        ],
        ArtifactLimits::default(),
    )
    .unwrap()
}

fn begin(temp: &TempProject, key: &str) -> synapse_artifact::PendingArtifactProposal {
    begin_artifact_proposal(
        &temp.config(key),
        &manifest("accepted"),
        &manifest("proposed"),
        br#"{"request":{"kind":"copy_change"},"selection":{"block_id":"hero"}}"#,
        ArtifactSourceAttribution::CallerSuppliedAiAttributed,
    )
    .unwrap()
}

#[test]
fn same_process_workflow_publishes_proposal_and_adopted_decision() {
    let temp = TempProject::new("adopt");
    let mut pending = begin(&temp, "adopt-flow");

    assert_eq!(pending.state(), PendingArtifactState::Ready);
    assert_eq!(pending.receipt().contract(), "synapsegit.generic-artifact");
    assert_eq!(pending.receipt().contract_version(), 1);
    assert!(!pending.receipt().execution_verified());
    assert_eq!(pending.receipt().artifact_manifest_sha256().len(), 64);
    assert_eq!(pending.receipt().review_context_sha256().len(), 64);

    let binding = pending.durable_binding();
    assert!(binding.project().as_str().starts_with("urn:uuid:"));
    assert!(
        binding
            .proposal_ref_name()
            .starts_with("proposal/artifact/adopt-flow/")
    );
    assert_eq!(binding.decision_ref_name(), "decision/artifact/adopt-flow");
    let debug_binding = format!("{binding:?}");
    assert!(debug_binding.contains("redacted"));
    assert!(!debug_binding.contains("proposal/artifact"));

    let proposed_digest = pending.receipt().artifact_manifest_sha256().to_owned();
    let receipt = approved_decide(
        &mut pending,
        &ArtifactDecisionOptions {
            disposition: ArtifactDisposition::AdoptedUnchanged,
            private_rationale: Some("Creator accepts this exact proposal.".into()),
        },
    )
    .unwrap();

    assert_eq!(pending.state(), PendingArtifactState::Consumed);
    assert_eq!(receipt.disposition(), ArtifactDisposition::AdoptedUnchanged);
    assert_eq!(receipt.selected_snapshot(), "proposal");
    assert_eq!(receipt.reviewed_artifact_manifest_sha256(), proposed_digest);

    let repository = Repository::open(&temp.path).unwrap();
    assert!(repository.fsck().unwrap().is_clean());
    assert_eq!(repository.refs().list().unwrap().len(), 2);
    assert_ne!(
        repository
            .refs()
            .get("decision/artifact/adopt-flow")
            .unwrap()
            .unwrap()
            .head,
        binding.decision_head()
    );
}

#[test]
fn rejecting_or_deferring_selects_the_identical_base_manifest() {
    let rejected_temp = TempProject::new("reject");
    let deferred_temp = TempProject::new("defer");
    let mut rejected = begin(&rejected_temp, "reject-flow");
    let mut deferred = begin(&deferred_temp, "defer-flow");

    let rejected_receipt = approved_decide(
        &mut rejected,
        &ArtifactDecisionOptions {
            disposition: ArtifactDisposition::Rejected,
            private_rationale: None,
        },
    )
    .unwrap();
    let deferred_receipt = approved_decide(
        &mut deferred,
        &ArtifactDecisionOptions {
            disposition: ArtifactDisposition::Deferred,
            private_rationale: None,
        },
    )
    .unwrap();

    assert_eq!(rejected_receipt.selected_snapshot(), "base");
    assert_eq!(deferred_receipt.selected_snapshot(), "base");
    assert_eq!(
        rejected_receipt.reviewed_artifact_manifest_sha256(),
        deferred_receipt.reviewed_artifact_manifest_sha256()
    );
    assert_ne!(
        rejected_receipt.reviewed_artifact_manifest_sha256(),
        rejected.receipt().artifact_manifest_sha256()
    );
    assert!(
        Repository::open(&rejected_temp.path)
            .unwrap()
            .fsck()
            .unwrap()
            .is_clean()
    );
    assert!(
        Repository::open(&deferred_temp.path)
            .unwrap()
            .fsck()
            .unwrap()
            .is_clean()
    );
}

#[test]
fn authority_is_one_shot_and_an_initialized_repository_is_not_reused() {
    let temp = TempProject::new("one-shot");
    let mut pending = begin(&temp, "one-shot-flow");
    approved_decide(
        &mut pending,
        &ArtifactDecisionOptions {
            disposition: ArtifactDisposition::Rejected,
            private_rationale: None,
        },
    )
    .unwrap();

    let second_decision = approved_decide(
        &mut pending,
        &ArtifactDecisionOptions {
            disposition: ArtifactDisposition::Rejected,
            private_rationale: None,
        },
    )
    .unwrap_err();
    assert_eq!(second_decision.code(), "artifact_approval_invalid");

    let second_proposal = begin_artifact_proposal(
        &temp.config("another-flow"),
        &manifest("accepted"),
        &manifest("proposed"),
        b"{}",
        ArtifactSourceAttribution::CallerSuppliedAiAttributed,
    )
    .unwrap_err();
    assert!(matches!(second_proposal, WorkflowError::ProjectExists));
}

#[test]
fn strict_context_validation_happens_before_repository_creation() {
    let temp = TempProject::new("invalid-context");
    let error = begin_artifact_proposal(
        &temp.config("invalid-context"),
        &manifest("accepted"),
        &manifest("proposed"),
        br#"{"selection":"hero","selection":"footer"}"#,
        ArtifactSourceAttribution::CallerSuppliedAiAttributed,
    )
    .unwrap_err();

    assert_eq!(error.code(), "duplicate_key");
    assert!(!temp.path.exists());
}

#[test]
fn invalid_project_ref_segments_fail_before_repository_creation() {
    for project_key in ["Uppercase", ".leading-dot", "contains:colon"] {
        let temp = TempProject::new("invalid-project-key");
        let error = begin_artifact_proposal(
            &temp.config(project_key),
            &manifest("accepted"),
            &manifest("proposed"),
            b"{}",
            ArtifactSourceAttribution::CallerSuppliedAiAttributed,
        )
        .unwrap_err();

        assert_eq!(error.code(), "invalid_argument", "{project_key}");
        assert!(!temp.path.exists(), "{project_key}");
    }
}

#[test]
fn decision_options_and_context_errors_redact_private_input() {
    let options = ArtifactDecisionOptions {
        disposition: ArtifactDisposition::Rejected,
        private_rationale: Some("PRIVATE-RATIONALE-CANARY".into()),
    };
    assert!(!format!("{options:?}").contains("PRIVATE-RATIONALE"));

    let temp = TempProject::new("redacted-context-error");
    let error = begin_artifact_proposal(
        &temp.config("redacted-context-error"),
        &manifest("accepted"),
        &manifest("proposed"),
        br#"{"SECRET-CONTEXT-CANARY":1.5}"#,
        ArtifactSourceAttribution::CallerSuppliedAiAttributed,
    )
    .unwrap_err();
    assert!(!format!("{error:?}").contains("SECRET-CONTEXT"));
    assert!(!error.to_string().contains("SECRET-CONTEXT"));
    assert!(!temp.path.exists());
}
