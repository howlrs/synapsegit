mod support;

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};
use support::{HOST_CREDENTIAL, approval_registry, approved_decide};
use synapse_application::{DurableProposalBinding, ProjectSelector};
use synapse_artifact::{
    ArtifactCheckoutLimits, ArtifactDecisionOptions, ArtifactDisposition, ArtifactLimits,
    ArtifactManifestEntry, ArtifactSourceAttribution, RegularFileManifest,
    TrustedArtifactDecisionBinding, TrustedArtifactProjectConfig, artifact_manifest_sha256,
    begin_artifact_proposal, begin_next_artifact_proposal, checkout_artifact_decision,
    prepare_artifact_decision, prepare_artifact_proposal, prepare_next_artifact_proposal,
    prepare_next_artifact_proposal_at_head, publish_prepared_artifact_decision,
    publish_prepared_artifact_proposal, recover_prepared_artifact_proposal,
    recover_published_artifact_proposal,
};
use synapse_core::Repository;
use synapse_sqlite::{RefUpdate, ReflogMetadata};

static NEXT_TEMP: AtomicU64 = AtomicU64::new(1);

struct TempProject(PathBuf);

impl TempProject {
    fn new(label: &str) -> Self {
        let serial = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        Self(std::env::temp_dir().join(format!(
            "synapse-artifact-sequential-{label}-{}-{nanos}-{serial}",
            std::process::id()
        )))
    }

    fn config(&self, key: &str) -> TrustedArtifactProjectConfig {
        TrustedArtifactProjectConfig::new(
            &self.0,
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
        if self.0.starts_with(std::env::temp_dir()) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
}

fn manifest(label: &str) -> RegularFileManifest {
    RegularFileManifest::from_entries(
        [ArtifactManifestEntry::regular_file(
            "index.html",
            format!("<title>{label}</title>").into_bytes(),
        )],
        ArtifactLimits::default(),
    )
    .unwrap()
}

fn options(disposition: ArtifactDisposition) -> ArtifactDecisionOptions {
    ArtifactDecisionOptions {
        disposition,
        private_rationale: None,
    }
}

fn repo_state(path: &PathBuf) -> (synapse_core::RefSnapshot, Vec<synapse_sqlite::ReflogEntry>) {
    let repository = Repository::open(path).unwrap();
    (
        repository.refs().snapshot().unwrap(),
        repository.refs().reflog().unwrap(),
    )
}

#[test]
fn sequential_adopt_reject_and_defer_chains_use_the_exact_selected_base() {
    for (label, first, second) in [
        (
            "adopt-adopt",
            ArtifactDisposition::AdoptedUnchanged,
            ArtifactDisposition::AdoptedUnchanged,
        ),
        (
            "reject-adopt",
            ArtifactDisposition::Rejected,
            ArtifactDisposition::AdoptedUnchanged,
        ),
        (
            "defer-reject",
            ArtifactDisposition::Deferred,
            ArtifactDisposition::Rejected,
        ),
    ] {
        let temp = TempProject::new(label);
        let config = temp.config(label);
        let initial = manifest("accepted-v0");
        let first_proposal = manifest("proposal-v1");
        let mut first_pending = begin_artifact_proposal(
            &config,
            &initial,
            &first_proposal,
            br#"{"turn":1}"#,
            ArtifactSourceAttribution::CallerSuppliedAiAttributed,
        )
        .unwrap();
        let first_ref = first_pending
            .durable_binding()
            .proposal_ref_name()
            .to_owned();
        approved_decide(&mut first_pending, &options(first)).unwrap();
        drop(first_pending);

        let accepted = if first == ArtifactDisposition::AdoptedUnchanged {
            &first_proposal
        } else {
            &initial
        };
        let second_proposal = manifest("proposal-v2");
        let mut second_pending = begin_next_artifact_proposal(
            &config,
            accepted,
            &second_proposal,
            br#"{"turn":2}"#,
            ArtifactSourceAttribution::CallerSuppliedAiAttributed,
        )
        .unwrap();
        let second_binding = second_pending.durable_binding();
        assert_ne!(first_ref, second_binding.proposal_ref_name());
        assert_eq!(
            Repository::open(&temp.0)
                .unwrap()
                .refs()
                .list()
                .unwrap()
                .len(),
            3
        );

        let receipt = approved_decide(&mut second_pending, &options(second)).unwrap();
        let expected = if second == ArtifactDisposition::AdoptedUnchanged {
            artifact_manifest_sha256(&second_proposal)
        } else {
            artifact_manifest_sha256(accepted)
        };
        assert_eq!(receipt.reviewed_artifact_manifest_sha256(), expected);
        drop(second_pending);
        let decision_head = Repository::open(&temp.0)
            .unwrap()
            .refs()
            .get(&format!("decision/artifact/{label}"))
            .unwrap()
            .unwrap()
            .head;
        let checkout = checkout_artifact_decision(
            &TrustedArtifactDecisionBinding::new(
                &temp.0,
                label,
                second_binding,
                decision_head,
                second,
                &expected,
            ),
            ArtifactCheckoutLimits::default(),
        )
        .unwrap();
        assert_eq!(checkout.manifest_sha256(), expected);
        assert!(
            Repository::open(&temp.0)
                .unwrap()
                .fsck()
                .unwrap()
                .is_clean()
        );
    }
}

#[test]
fn active_duplicate_and_same_base_parallel_publish_are_ref_atomic() {
    let temp = TempProject::new("parallel");
    let config = temp.config("parallel");
    let initial = manifest("v0");
    let first_proposal = manifest("v1");
    let mut first = begin_artifact_proposal(
        &config,
        &initial,
        &first_proposal,
        b"{}",
        ArtifactSourceAttribution::CallerSuppliedAiAttributed,
    )
    .unwrap();
    approved_decide(&mut first, &options(ArtifactDisposition::AdoptedUnchanged)).unwrap();
    drop(first);

    let left = prepare_next_artifact_proposal(
        &config,
        &first_proposal,
        &manifest("left"),
        br#"{"candidate":"left"}"#,
        ArtifactSourceAttribution::CallerSuppliedAiAttributed,
    )
    .unwrap();
    let right = prepare_next_artifact_proposal(
        &config,
        &first_proposal,
        &manifest("right"),
        br#"{"candidate":"right"}"#,
        ArtifactSourceAttribution::CallerSuppliedAiAttributed,
    )
    .unwrap();
    assert_eq!(
        left.durable_binding().proposal_ref_name(),
        right.durable_binding().proposal_ref_name()
    );
    let _winner = publish_prepared_artifact_proposal(left).unwrap();
    let committed = repo_state(&temp.0);
    let loser = publish_prepared_artifact_proposal(right).unwrap_err();
    assert_eq!(loser.code(), "ref_conflict");
    assert_eq!(repo_state(&temp.0), committed);

    let duplicate = prepare_next_artifact_proposal(
        &config,
        &first_proposal,
        &manifest("duplicate"),
        b"{}",
        ArtifactSourceAttribution::CallerSuppliedAiAttributed,
    )
    .unwrap_err();
    assert_eq!(duplicate.code(), "artifact_review_active");
    assert_eq!(repo_state(&temp.0), committed);
}

#[test]
fn staged_proposal_and_decision_survive_restart_without_restoring_old_permits() {
    let temp = TempProject::new("restart");
    let config = temp.config("restart");
    let accepted = manifest("accepted");
    let proposed = manifest("proposed");
    let prepared = prepare_artifact_proposal(
        &config,
        &accepted,
        &proposed,
        br#"{"restart":true}"#,
        ArtifactSourceAttribution::CallerSuppliedAiAttributed,
    )
    .unwrap();
    let binding = prepared.durable_binding();
    let digest = prepared.receipt().artifact_manifest_sha256().to_owned();
    drop(prepared);

    let replay = recover_prepared_artifact_proposal(
        &config,
        &binding,
        &digest,
        &accepted,
        &proposed,
        br#"{"restart":true}"#,
        ArtifactSourceAttribution::CallerSuppliedAiAttributed,
    )
    .unwrap();
    let published = publish_prepared_artifact_proposal(replay).unwrap();
    let published_binding = published.durable_binding();
    drop(published);

    let mut recovered =
        recover_published_artifact_proposal(&config, &published_binding, &digest).unwrap();
    let decision = options(ArtifactDisposition::AdoptedUnchanged);
    let (registry, _, _) = approval_registry(&recovered);
    let approval = registry
        .issue_artifact_decision(HOST_CREDENTIAL, &recovered, &decision)
        .unwrap();
    let prepared_decision = prepare_artifact_decision(
        &registry,
        HOST_CREDENTIAL,
        &approval,
        &mut recovered,
        &decision,
    )
    .unwrap();
    assert_eq!(
        prepared_decision.proposal_head(),
        published_binding.proposal_head()
    );
    assert_eq!(
        prepared_decision.expected_decision_head(),
        published_binding.decision_head()
    );
    let outcome = publish_prepared_artifact_decision(&mut recovered, prepared_decision).unwrap();
    assert_eq!(outcome.disposition(), ArtifactDisposition::AdoptedUnchanged);
    assert_eq!(outcome.reviewed_artifact_manifest_sha256(), digest);
}

#[test]
fn accepted_mismatch_never_changes_refs_or_reflog() {
    let temp = TempProject::new("mismatch");
    let config = temp.config("mismatch");
    let initial = manifest("v0");
    let proposal = manifest("v1");
    let mut pending = begin_artifact_proposal(
        &config,
        &initial,
        &proposal,
        b"{}",
        ArtifactSourceAttribution::CallerSuppliedAiAttributed,
    )
    .unwrap();
    approved_decide(
        &mut pending,
        &options(ArtifactDisposition::AdoptedUnchanged),
    )
    .unwrap();
    drop(pending);
    let before = repo_state(&temp.0);
    let error = prepare_next_artifact_proposal(
        &config,
        &manifest("wrong-accepted"),
        &manifest("v2"),
        b"{}",
        ArtifactSourceAttribution::CallerSuppliedAiAttributed,
    )
    .unwrap_err();
    assert_eq!(error.code(), "artifact_accepted_mismatch");
    assert_eq!(repo_state(&temp.0), before);
}

#[test]
fn published_recovery_rejects_wrong_journal_and_moved_refs_without_mutation() {
    let temp = TempProject::new("recovery-negative");
    let config = temp.config("recovery-negative");
    let accepted = manifest("accepted");
    let proposed = manifest("proposed");
    let pending = begin_artifact_proposal(
        &config,
        &accepted,
        &proposed,
        b"{}",
        ArtifactSourceAttribution::CallerSuppliedAiAttributed,
    )
    .unwrap();
    let binding = pending.durable_binding();
    let digest = pending.receipt().artifact_manifest_sha256().to_owned();
    drop(pending);
    let before = repo_state(&temp.0);

    let wrong_digest =
        recover_published_artifact_proposal(&config, &binding, &"0".repeat(64)).unwrap_err();
    assert_eq!(wrong_digest.code(), "artifact_recovery_mismatch");
    assert_eq!(repo_state(&temp.0), before);

    let foreign = DurableProposalBinding::new(
        ProjectSelector::new("urn:uuid:00000000-0000-4000-8000-000000000000"),
        binding.proposal_ref_name(),
        binding.proposal_head(),
        binding.decision_ref_name(),
        binding.decision_head(),
    );
    let foreign_error =
        recover_published_artifact_proposal(&config, &foreign, &digest).unwrap_err();
    assert_eq!(foreign_error.code(), "artifact_recovery_mismatch");
    assert_eq!(repo_state(&temp.0), before);

    let mut repository = Repository::open(&temp.0).unwrap();
    repository
        .update_ref(RefUpdate {
            ref_name: binding.proposal_ref_name(),
            expected_head: Some(binding.proposal_head()),
            new_head: binding.decision_head(),
            metadata: ReflogMetadata {
                occurred_at_unix_nanos: i64::MAX - 1,
                actor: Some("external-test-writer"),
                message: Some("move Proposal before recovery"),
            },
        })
        .unwrap();
    drop(repository);
    let moved = repo_state(&temp.0);
    let moved_error = recover_published_artifact_proposal(&config, &binding, &digest).unwrap_err();
    assert_eq!(moved_error.code(), "artifact_recovery_mismatch");
    assert_eq!(repo_state(&temp.0), moved);

    let decision_temp = TempProject::new("recovery-moved-decision");
    let decision_config = decision_temp.config("recovery-moved-decision");
    let decision_pending = begin_artifact_proposal(
        &decision_config,
        &accepted,
        &proposed,
        b"{}",
        ArtifactSourceAttribution::CallerSuppliedAiAttributed,
    )
    .unwrap();
    let decision_binding = decision_pending.durable_binding();
    let decision_digest = decision_pending
        .receipt()
        .artifact_manifest_sha256()
        .to_owned();
    drop(decision_pending);
    let mut repository = Repository::open(&decision_temp.0).unwrap();
    repository
        .update_ref(RefUpdate {
            ref_name: decision_binding.decision_ref_name(),
            expected_head: Some(decision_binding.decision_head()),
            new_head: decision_binding.proposal_head(),
            metadata: ReflogMetadata {
                occurred_at_unix_nanos: i64::MAX - 1,
                actor: Some("external-test-writer"),
                message: Some("move Decision before recovery"),
            },
        })
        .unwrap();
    drop(repository);
    let moved_decision = repo_state(&decision_temp.0);
    let error =
        recover_published_artifact_proposal(&decision_config, &decision_binding, &decision_digest)
            .unwrap_err();
    assert_eq!(error.code(), "artifact_recovery_mismatch");
    assert_eq!(repo_state(&decision_temp.0), moved_decision);
}

#[test]
fn expired_or_mismatched_authority_is_rejected_before_ref_mutation() {
    let expired = TempProject::new("expired");
    let expired_config = TrustedArtifactProjectConfig::new(
        &expired.0,
        "expired",
        "Artifact Creator",
        "Application-owned AI",
        "2020-01-01T00:00:00.000000000Z",
        "2021-01-01T00:00:00.000000000Z",
    );
    let error = prepare_artifact_proposal(
        &expired_config,
        &manifest("v0"),
        &manifest("v1"),
        b"{}",
        ArtifactSourceAttribution::CallerSuppliedAiAttributed,
    )
    .unwrap_err();
    assert_eq!(error.code(), "authorization_denied");
    assert!(!expired.0.exists());

    let temp = TempProject::new("authority-mismatch");
    let config = temp.config("authority-mismatch");
    let initial = manifest("v0");
    let proposed = manifest("v1");
    let mut pending = begin_artifact_proposal(
        &config,
        &initial,
        &proposed,
        b"{}",
        ArtifactSourceAttribution::CallerSuppliedAiAttributed,
    )
    .unwrap();
    approved_decide(
        &mut pending,
        &options(ArtifactDisposition::AdoptedUnchanged),
    )
    .unwrap();
    drop(pending);
    let mismatched = TrustedArtifactProjectConfig::new(
        &temp.0,
        "authority-mismatch",
        "Artifact Creator",
        "Application-owned AI",
        "2026-07-19T00:00:00.000000000Z",
        "2098-01-01T00:00:00.000000000Z",
    );
    let before = repo_state(&temp.0);
    let error = prepare_next_artifact_proposal(
        &mismatched,
        &proposed,
        &manifest("v2"),
        b"{}",
        ArtifactSourceAttribution::CallerSuppliedAiAttributed,
    )
    .unwrap_err();
    assert_eq!(error.code(), "artifact_profile_unsupported");
    assert_eq!(repo_state(&temp.0), before);
}

#[test]
fn exact_expected_decision_head_rejects_stale_intent_without_mutation() {
    let temp = TempProject::new("stale-head");
    let config = temp.config("stale-head");
    let initial = manifest("v0");
    let proposed = manifest("v1");
    let mut pending = begin_artifact_proposal(
        &config,
        &initial,
        &proposed,
        b"{}",
        ArtifactSourceAttribution::CallerSuppliedAiAttributed,
    )
    .unwrap();
    let stale_head = pending.durable_binding().decision_head().to_owned();
    approved_decide(
        &mut pending,
        &options(ArtifactDisposition::AdoptedUnchanged),
    )
    .unwrap();
    drop(pending);
    let before = repo_state(&temp.0);
    let error = prepare_next_artifact_proposal_at_head(
        &config,
        &stale_head,
        &proposed,
        &manifest("v2"),
        b"{}",
        ArtifactSourceAttribution::CallerSuppliedAiAttributed,
    )
    .unwrap_err();
    assert_eq!(error.code(), "stale_base");
    assert_eq!(repo_state(&temp.0), before);

    let current_head = Repository::open(&temp.0)
        .unwrap()
        .refs()
        .get("decision/artifact/stale-head")
        .unwrap()
        .unwrap()
        .head;
    let prepared = prepare_next_artifact_proposal_at_head(
        &config,
        &current_head,
        &proposed,
        &manifest("v2"),
        b"{}",
        ArtifactSourceAttribution::CallerSuppliedAiAttributed,
    )
    .unwrap();
    let mut repository = Repository::open(&temp.0).unwrap();
    repository
        .update_ref(RefUpdate {
            ref_name: "decision/artifact/stale-head",
            expected_head: Some(&current_head),
            new_head: &stale_head,
            metadata: ReflogMetadata {
                occurred_at_unix_nanos: i64::MAX - 1,
                actor: Some("external-test-writer"),
                message: Some("move Decision after prepare"),
            },
        })
        .unwrap();
    drop(repository);
    let after_external_move = repo_state(&temp.0);
    let publish_error = publish_prepared_artifact_proposal(prepared).unwrap_err();
    assert_eq!(publish_error.code(), "stale_base");
    assert_eq!(repo_state(&temp.0), after_external_move);
}

#[test]
fn published_recovery_rejects_a_malformed_proposal_graph_without_ref_mutation() {
    let temp = TempProject::new("recovery-malformed");
    let config = temp.config("recovery-malformed");
    let pending = begin_artifact_proposal(
        &config,
        &manifest("v0"),
        &manifest("v1"),
        b"{}",
        ArtifactSourceAttribution::CallerSuppliedAiAttributed,
    )
    .unwrap();
    let binding = pending.durable_binding();
    let digest = pending.receipt().artifact_manifest_sha256().to_owned();
    drop(pending);

    let object_digest = binding.proposal_head().rsplit(':').next().unwrap();
    let object_path = temp
        .0
        .join("cas/objects/commit")
        .join(&object_digest[..2])
        .join(&object_digest[2..]);
    fs::write(object_path, b"{}".as_slice()).unwrap();
    let before = repo_state(&temp.0);
    let error = recover_published_artifact_proposal(&config, &binding, &digest).unwrap_err();
    assert!(matches!(
        error.code(),
        "artifact_integrity_error" | "oid_mismatch" | "schema_invalid"
    ));
    assert_eq!(repo_state(&temp.0), before);
}
