mod support;

use serde_json::{Value as JsonValue, json};
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};
use support::{
    HOST_ACTOR, HOST_CREDENTIAL, TestApprovalRegistry, TestClock, TestHostAuthenticator,
    approval_registry,
};
use synapse_artifact::{
    ArtifactApprovalRegistry, ArtifactDecisionOptions, ArtifactDisposition, ArtifactLimits,
    ArtifactManifestEntry, ArtifactSourceAttribution, PendingArtifactProposal, RegularFileManifest,
    TrustedArtifactProjectConfig, begin_artifact_proposal, decide_artifact_proposal,
};
use synapse_core::Repository;
use synapse_sqlite::{RefUpdate, ReflogMetadata};

static NEXT_TEMP: AtomicU64 = AtomicU64::new(1);

struct TempProject {
    path: PathBuf,
}

impl TempProject {
    fn new(label: &str) -> Self {
        let serial = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("test clock after epoch")
            .as_nanos();
        Self {
            path: std::env::temp_dir().join(format!(
                "synapse-artifact-approval-{label}-{}-{nanos}-{serial}",
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
        [ArtifactManifestEntry::regular_file(
            "index.html",
            format!("<!doctype html><title>{label}</title>").into_bytes(),
        )],
        ArtifactLimits::default(),
    )
    .unwrap()
}

fn begin(temp: &TempProject, key: &str) -> PendingArtifactProposal {
    begin_artifact_proposal(
        &temp.config(key),
        &manifest("accepted"),
        &manifest("proposed"),
        b"{}",
        ArtifactSourceAttribution::CallerSuppliedAiAttributed,
    )
    .unwrap()
}

fn options(disposition: ArtifactDisposition, rationale: &str) -> ArtifactDecisionOptions {
    ArtifactDecisionOptions {
        disposition,
        private_rationale: Some(rationale.into()),
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct RepositoryState {
    refs: synapse_core::RefSnapshot,
    reflog: Vec<synapse_sqlite::ReflogEntry>,
    objects: Vec<String>,
}

fn repository_state(repository: &Repository) -> RepositoryState {
    RepositoryState {
        refs: repository.refs().snapshot().unwrap(),
        reflog: repository.refs().reflog().unwrap(),
        objects: repository.objects().list_oids().unwrap(),
    }
}

fn object_json(repository: &Repository, oid: &str) -> JsonValue {
    serde_json::from_slice(
        &repository
            .objects()
            .read_raw(oid)
            .unwrap()
            .unwrap_or_else(|| panic!("missing test object")),
    )
    .unwrap()
}

fn successor_commit(repository: &Repository, parent: &str, label: &str) -> String {
    let mut commit = object_json(repository, parent);
    commit["parents"] = json!([parent]);
    commit["message"] = json!(format!("external {label} advance"));
    repository
        .put_object(&serde_json::to_vec(&commit).unwrap())
        .unwrap()
        .oid
}

#[test]
fn valid_host_approval_publishes_once_and_replay_is_mutation_free() {
    let temp = TempProject::new("valid-replay");
    let mut pending = begin(&temp, "approval-valid-replay");
    let decision = options(
        ArtifactDisposition::AdoptedUnchanged,
        "PRIVATE-RATIONALE-CANARY-valid",
    );
    let (registry, _, _) = approval_registry(&pending);
    let approval = registry
        .issue_artifact_decision(HOST_CREDENTIAL, &pending, &decision)
        .unwrap();
    let approval_debug = format!("{approval:?}");
    assert_eq!(approval_debug, "ArtifactDecisionApproval(<opaque>)");
    assert!(!approval_debug.contains(HOST_CREDENTIAL));
    assert!(!approval_debug.contains("proposal/"));

    let receipt = decide_artifact_proposal(
        &registry,
        HOST_CREDENTIAL,
        &approval,
        &mut pending,
        &decision,
    )
    .unwrap();
    assert_eq!(receipt.disposition(), ArtifactDisposition::AdoptedUnchanged);

    let repository = Repository::open(&temp.path).unwrap();
    let committed = repository_state(&repository);
    let replay = decide_artifact_proposal(
        &registry,
        HOST_CREDENTIAL,
        &approval,
        &mut pending,
        &decision,
    )
    .unwrap_err();
    assert_eq!(replay.code(), "artifact_approval_invalid");
    assert_eq!(repository_state(&repository), committed);
}

#[test]
fn authentication_precedes_foreign_handle_lookup_and_errors_are_redacted() {
    let temp = TempProject::new("auth-order");
    let mut pending = begin(&temp, "approval-auth-order");
    let decision = options(
        ArtifactDisposition::Rejected,
        "PRIVATE-RATIONALE-CANARY-auth-order",
    );
    let (issuer, _, _) = approval_registry(&pending);
    let approval = issuer
        .issue_artifact_decision(HOST_CREDENTIAL, &pending, &decision)
        .unwrap();

    let foreign_authenticator = TestHostAuthenticator::default();
    let foreign = ArtifactApprovalRegistry::new(
        foreign_authenticator.clone(),
        TestClock::default(),
        60_000_000_000,
    )
    .unwrap();
    foreign
        .grant_project_access(pending.durable_binding().project(), HOST_ACTOR)
        .unwrap();
    let repository = Repository::open(&temp.path).unwrap();
    let before = repository_state(&repository);

    let unauthenticated = decide_artifact_proposal(
        &foreign,
        "INVALID-CREDENTIAL-CANARY",
        &approval,
        &mut pending,
        &decision,
    )
    .unwrap_err();
    assert_eq!(unauthenticated.code(), "authentication_failed");
    assert_eq!(foreign_authenticator.calls(), 1);

    let foreign_handle = decide_artifact_proposal(
        &foreign,
        HOST_CREDENTIAL,
        &approval,
        &mut pending,
        &decision,
    )
    .unwrap_err();
    assert_eq!(foreign_handle.code(), "artifact_approval_invalid");
    assert_eq!(foreign_authenticator.calls(), 2);
    assert_eq!(repository_state(&repository), before);

    for rendered in [
        format!("{unauthenticated:?}"),
        unauthenticated.to_string(),
        format!("{foreign_handle:?}"),
        foreign_handle.to_string(),
    ] {
        assert!(!rendered.contains("INVALID-CREDENTIAL"));
        assert!(!rendered.contains("PRIVATE-RATIONALE"));
        assert!(!rendered.contains(&temp.path.display().to_string()));
        assert!(!rendered.contains("proposal/"));
    }
}

#[test]
fn foreign_registry_handle_with_same_serial_does_not_burn_local_approval() {
    let temp = TempProject::new("foreign-same-serial");
    let mut pending = begin(&temp, "approval-foreign-same-serial");
    let decision = options(
        ArtifactDisposition::AdoptedUnchanged,
        "PRIVATE-RATIONALE-CANARY-same-serial",
    );
    let (local, _, _) = approval_registry(&pending);
    let (foreign, _, _) = approval_registry(&pending);
    let local_approval = local
        .issue_artifact_decision(HOST_CREDENTIAL, &pending, &decision)
        .unwrap();
    let foreign_approval = foreign
        .issue_artifact_decision(HOST_CREDENTIAL, &pending, &decision)
        .unwrap();

    let wrong_registry = decide_artifact_proposal(
        &local,
        HOST_CREDENTIAL,
        &foreign_approval,
        &mut pending,
        &decision,
    )
    .unwrap_err();
    assert_eq!(wrong_registry.code(), "artifact_approval_invalid");

    decide_artifact_proposal(
        &local,
        HOST_CREDENTIAL,
        &local_approval,
        &mut pending,
        &decision,
    )
    .unwrap();
}

#[test]
fn approval_is_bound_to_exact_pending_review_and_decision_intent() {
    let first_temp = TempProject::new("intent-first");
    let second_temp = TempProject::new("intent-second");
    let mut first = begin(&first_temp, "approval-intent-first");
    let mut second = begin(&second_temp, "approval-intent-second");
    let adopted = options(
        ArtifactDisposition::AdoptedUnchanged,
        "PRIVATE-RATIONALE-CANARY-intent",
    );
    let rejected = options(
        ArtifactDisposition::Rejected,
        "PRIVATE-RATIONALE-CANARY-intent",
    );
    let (registry, _, _) = approval_registry(&first);
    registry
        .grant_project_access(second.durable_binding().project(), HOST_ACTOR)
        .unwrap();
    let first_repository = Repository::open(&first_temp.path).unwrap();
    let second_repository = Repository::open(&second_temp.path).unwrap();

    let wrong_intent_approval = registry
        .issue_artifact_decision(HOST_CREDENTIAL, &first, &adopted)
        .unwrap();
    let first_before = repository_state(&first_repository);
    let wrong_intent = decide_artifact_proposal(
        &registry,
        HOST_CREDENTIAL,
        &wrong_intent_approval,
        &mut first,
        &rejected,
    )
    .unwrap_err();
    assert_eq!(wrong_intent.code(), "artifact_approval_invalid");
    assert_eq!(repository_state(&first_repository), first_before);

    let wrong_review_approval = registry
        .issue_artifact_decision(HOST_CREDENTIAL, &first, &adopted)
        .unwrap();
    let second_before = repository_state(&second_repository);
    let wrong_review = decide_artifact_proposal(
        &registry,
        HOST_CREDENTIAL,
        &wrong_review_approval,
        &mut second,
        &adopted,
    )
    .unwrap_err();
    assert_eq!(wrong_review.code(), "artifact_approval_invalid");
    assert_eq!(repository_state(&second_repository), second_before);
}

#[test]
fn expiry_revocation_and_clock_failure_burn_without_repository_mutation() {
    for case in ["expired", "revoked", "clock-failure"] {
        let temp = TempProject::new(case);
        let mut pending = begin(&temp, &format!("approval-{case}"));
        let decision = options(
            ArtifactDisposition::Deferred,
            "PRIVATE-RATIONALE-CANARY-expiry",
        );
        let (registry, authenticator, clock) = approval_registry(&pending);
        let approval = registry
            .issue_artifact_decision(HOST_CREDENTIAL, &pending, &decision)
            .unwrap();
        match case {
            "expired" => clock.set(i128::MAX - 1),
            "revoked" => registry
                .revoke_project_access(pending.durable_binding().project(), HOST_ACTOR)
                .unwrap(),
            "clock-failure" => clock.fail(),
            _ => unreachable!(),
        }
        let repository = Repository::open(&temp.path).unwrap();
        let before = repository_state(&repository);
        let error = decide_artifact_proposal(
            &registry,
            HOST_CREDENTIAL,
            &approval,
            &mut pending,
            &decision,
        )
        .unwrap_err();
        assert_eq!(
            error.code(),
            if case == "clock-failure" {
                "service_unavailable"
            } else if case == "revoked" {
                "project_access_denied"
            } else {
                "artifact_approval_invalid"
            },
            "{case}"
        );
        assert_eq!(repository_state(&repository), before, "{case}");
        if case == "clock-failure" {
            clock.set(1_900_000_000_000_000_000);
            let retry = decide_artifact_proposal(
                &registry,
                HOST_CREDENTIAL,
                &approval,
                &mut pending,
                &decision,
            )
            .unwrap_err();
            assert_eq!(retry.code(), "artifact_approval_invalid");
            assert_eq!(authenticator.calls(), 3);
            assert_eq!(repository_state(&repository), before, "{case}-retry");
        }
    }
}

#[test]
fn stale_proposal_or_decision_is_rejected_before_candidate_cas_writes() {
    for moved in ["proposal", "decision"] {
        let temp = TempProject::new(&format!("stale-{moved}"));
        let mut pending = begin(&temp, &format!("approval-stale-{moved}"));
        let decision = options(
            ArtifactDisposition::AdoptedUnchanged,
            "PRIVATE-RATIONALE-CANARY-stale",
        );
        let (registry, _, _) = approval_registry(&pending);
        let approval = registry
            .issue_artifact_decision(HOST_CREDENTIAL, &pending, &decision)
            .unwrap();
        let binding = pending.durable_binding();
        let mut repository = Repository::open(&temp.path).unwrap();
        let (ref_name, expected_head) = if moved == "proposal" {
            (binding.proposal_ref_name(), binding.proposal_head())
        } else {
            (binding.decision_ref_name(), binding.decision_head())
        };
        let advanced = successor_commit(&repository, expected_head, moved);
        repository
            .update_ref(RefUpdate {
                ref_name,
                expected_head: Some(expected_head),
                new_head: &advanced,
                metadata: ReflogMetadata {
                    occurred_at_unix_nanos: i64::MAX - 1,
                    actor: Some("external-test-writer"),
                    message: Some("external approval-staleness test"),
                },
            })
            .unwrap();
        let after_external_advance = repository_state(&repository);

        let error = decide_artifact_proposal(
            &registry,
            HOST_CREDENTIAL,
            &approval,
            &mut pending,
            &decision,
        )
        .unwrap_err();
        assert_eq!(
            error.code(),
            if moved == "proposal" {
                "ref_conflict"
            } else {
                "stale_base"
            }
        );
        assert_eq!(
            repository_state(&repository),
            after_external_advance,
            "{moved}"
        );
    }
}

#[test]
fn approval_issuance_requires_authenticated_project_membership() {
    let temp = TempProject::new("membership");
    let pending = begin(&temp, "approval-membership");
    let decision = options(ArtifactDisposition::Rejected, "membership rationale");
    let registry: TestApprovalRegistry = ArtifactApprovalRegistry::new(
        TestHostAuthenticator::default(),
        TestClock::default(),
        60_000_000_000,
    )
    .unwrap();

    let denied = registry
        .issue_artifact_decision(HOST_CREDENTIAL, &pending, &decision)
        .unwrap_err();
    assert_eq!(denied.code(), "project_access_denied");
    let unauthenticated = registry
        .issue_artifact_decision("bad credential", &pending, &decision)
        .unwrap_err();
    assert_eq!(unauthenticated.code(), "authentication_failed");
}

#[test]
fn issuance_checks_project_access_before_pending_state_options_and_clock() {
    let temp = TempProject::new("issuance-order");
    let mut pending = begin(&temp, "approval-issuance-order");
    let decision = options(
        ArtifactDisposition::Rejected,
        "PRIVATE-RATIONALE-CANARY-issuance-order",
    );
    let (member_registry, _, _) = approval_registry(&pending);
    let approval = member_registry
        .issue_artifact_decision(HOST_CREDENTIAL, &pending, &decision)
        .unwrap();
    decide_artifact_proposal(
        &member_registry,
        HOST_CREDENTIAL,
        &approval,
        &mut pending,
        &decision,
    )
    .unwrap();

    let authenticator = TestHostAuthenticator::default();
    let clock = TestClock::default();
    clock.fail();
    let nonmember: TestApprovalRegistry =
        ArtifactApprovalRegistry::new(authenticator.clone(), clock, 60_000_000_000).unwrap();
    let invalid_options = ArtifactDecisionOptions {
        disposition: ArtifactDisposition::Deferred,
        private_rationale: Some(String::new()),
    };
    let denied = nonmember
        .issue_artifact_decision(HOST_CREDENTIAL, &pending, &invalid_options)
        .unwrap_err();
    assert_eq!(denied.code(), "project_access_denied");
    assert_eq!(authenticator.calls(), 1);
}

#[test]
fn claim_checks_project_access_before_pending_state_options_and_clock() {
    let temp = TempProject::new("claim-order");
    let mut pending = begin(&temp, "approval-claim-order");
    let decision = options(
        ArtifactDisposition::Rejected,
        "PRIVATE-RATIONALE-CANARY-claim-order",
    );
    let (registry, _, clock) = approval_registry(&pending);
    let consume_pending = registry
        .issue_artifact_decision(HOST_CREDENTIAL, &pending, &decision)
        .unwrap();
    let denied_later = registry
        .issue_artifact_decision(HOST_CREDENTIAL, &pending, &decision)
        .unwrap();
    decide_artifact_proposal(
        &registry,
        HOST_CREDENTIAL,
        &consume_pending,
        &mut pending,
        &decision,
    )
    .unwrap();
    registry
        .revoke_project_access(pending.durable_binding().project(), HOST_ACTOR)
        .unwrap();
    clock.fail();
    let invalid_options = ArtifactDecisionOptions {
        disposition: ArtifactDisposition::Deferred,
        private_rationale: Some("invalid\ncontrol".into()),
    };

    let denied = decide_artifact_proposal(
        &registry,
        HOST_CREDENTIAL,
        &denied_later,
        &mut pending,
        &invalid_options,
    )
    .unwrap_err();
    assert_eq!(denied.code(), "project_access_denied");
}

#[test]
fn issuance_rejects_invalid_present_rationale_before_reading_clock() {
    let temp = TempProject::new("invalid-rationale");
    let pending = begin(&temp, "approval-invalid-rationale");
    let (registry, _, clock) = approval_registry(&pending);
    clock.fail();

    for rationale in [String::new(), "x".repeat(2_001), "invalid\ncontrol".into()] {
        let invalid_options = ArtifactDecisionOptions {
            disposition: ArtifactDisposition::Deferred,
            private_rationale: Some(rationale),
        };
        let invalid = registry
            .issue_artifact_decision(HOST_CREDENTIAL, &pending, &invalid_options)
            .unwrap_err();
        assert_eq!(invalid.code(), "artifact_approval_invalid");
    }

    let valid = registry
        .issue_artifact_decision(
            HOST_CREDENTIAL,
            &pending,
            &options(ArtifactDisposition::Deferred, "valid rationale"),
        )
        .unwrap_err();
    assert_eq!(valid.code(), "service_unavailable");
}
