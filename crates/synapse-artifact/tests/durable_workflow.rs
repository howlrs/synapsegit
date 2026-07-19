mod support;

use serde_json::{Value as JsonValue, json};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};
use support::{
    HOST_ACTOR, HOST_CREDENTIAL, TestApprovalRegistry, approval_registry, approved_decide,
};
use synapse_artifact::{
    ArtifactApprovalRegistry, ArtifactCheckoutLimits, ArtifactDecisionOptions, ArtifactDisposition,
    ArtifactLimits, ArtifactManifestEntry, ArtifactReviewId, ArtifactSourceAttribution,
    DurableArtifactCheckoutState, DurableArtifactProposalRecovery, DurableArtifactReviewState,
    RegularFileManifest, TrustedArtifactProjectConfig, begin_next_artifact_proposal,
    commit_published_durable_artifact_decision, commit_published_durable_artifact_proposal,
    get_durable_artifact_review_status, prepare_durable_artifact_decision,
    prepare_durable_artifact_proposal, publish_prepared_durable_artifact_decision,
    publish_prepared_durable_artifact_proposal, reconcile_durable_artifact_review,
    recover_durable_artifact_proposal, recover_durable_artifact_review,
};
use synapse_artifact_journal::{ReviewId, ReviewState, SqliteReviewJournal};
use synapse_core::Repository;
use synapse_sqlite::{RefUpdate, ReflogMetadata};

static NEXT_TEMP: AtomicU64 = AtomicU64::new(1);

struct TempReview {
    root: PathBuf,
    repository: PathBuf,
    journal: PathBuf,
}

impl TempReview {
    fn new(label: &str) -> Self {
        let serial = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "synapse-artifact-durable-{label}-{}-{nanos}-{serial}",
            std::process::id()
        ));
        fs::create_dir(&root).unwrap();
        Self {
            repository: root.join("repository"),
            journal: root.join("review-journal.sqlite3"),
            root,
        }
    }

    fn config(&self, key: &str) -> TrustedArtifactProjectConfig {
        TrustedArtifactProjectConfig::new(
            &self.repository,
            key,
            "Artifact Creator",
            "Application-owned AI",
            "2026-07-19T00:00:00.000000000Z",
            "2099-01-01T00:00:00.000000000Z",
        )
    }
}

impl Drop for TempReview {
    fn drop(&mut self) {
        if self.root.starts_with(std::env::temp_dir()) {
            let _ = fs::remove_dir_all(&self.root);
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
                "assets/app.js",
                format!("console.log({label:?});").into_bytes(),
            ),
        ],
        ArtifactLimits::default(),
    )
    .unwrap()
}

fn decision(disposition: ArtifactDisposition, rationale: &str) -> ArtifactDecisionOptions {
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

fn repository_state(path: &Path) -> RepositoryState {
    let repository = Repository::open(path).unwrap();
    RepositoryState {
        refs: repository.refs().snapshot().unwrap(),
        reflog: repository.refs().reflog().unwrap(),
        objects: repository.objects().list_oids().unwrap(),
    }
}

fn object_json(repository: &Repository, oid: &str) -> JsonValue {
    let bytes = repository
        .objects()
        .read_raw(oid)
        .unwrap_or_else(|error| panic!("read {oid}: {error}"))
        .unwrap_or_else(|| panic!("missing object {oid}"));
    serde_json::from_slice(&bytes).unwrap_or_else(|error| panic!("parse {oid}: {error}"))
}

fn store_changed_object(
    repository: &Repository,
    oid: &str,
    label: &str,
    change: impl FnOnce(&mut JsonValue),
) -> String {
    let mut value = object_json(repository, oid);
    change(&mut value);
    if value.get("object_type").and_then(JsonValue::as_str) == Some("commit") {
        value["message"] = json!(format!("external {label}"));
    }
    repository
        .put_object(&serde_json::to_vec(&value).unwrap())
        .unwrap_or_else(|error| panic!("store external {label}: {error}"))
        .oid
}

fn cas_object_path(repository: &Path, oid: &str) -> PathBuf {
    let family = oid.split(':').next().unwrap();
    let digest = oid.rsplit(':').next().unwrap();
    repository
        .join("cas/objects")
        .join(family)
        .join(&digest[..2])
        .join(&digest[2..])
}

fn reopen(path: &Path) -> SqliteReviewJournal {
    SqliteReviewJournal::open(path).unwrap()
}

fn fresh_registry(config: &TrustedArtifactProjectConfig) -> TestApprovalRegistry {
    let registry = ArtifactApprovalRegistry::new(
        support::TestHostAuthenticator::default(),
        support::TestClock::default(),
        60_000_000_000,
    )
    .unwrap();
    registry
        .grant_project_access(&config.project_selector(), HOST_ACTOR)
        .unwrap();
    registry
}

#[test]
fn proposal_faults_before_and_after_cas_recover_one_public_review_id() {
    let temp = TempReview::new("proposal-faults");
    let config = temp.config("durable-proposal-faults");
    let accepted = manifest("accepted");
    let proposed = manifest("proposed");
    let context = br#"{"session":"PRIVATE-CONTEXT-CANARY"}"#;
    let key = b"PRIVATE-PROPOSAL-IDEMPOTENCY-CANARY";

    let mut journal = reopen(&temp.journal);
    let prepared = prepare_durable_artifact_proposal(
        &mut journal,
        &config,
        &accepted,
        &proposed,
        context,
        ArtifactSourceAttribution::CallerSuppliedAiAttributed,
        key,
    )
    .unwrap();
    assert!(repository_state(&temp.repository).refs.is_empty());

    // Process loss before Proposal CAS: only immutable objects and a private
    // intent survive. No public ReviewId has been allocated.
    drop(prepared);
    drop(journal);
    let mut journal = reopen(&temp.journal);
    let recovery_registry = fresh_registry(&config);
    let recovered_prepared = match recover_durable_artifact_proposal(
        &mut journal,
        &recovery_registry,
        HOST_CREDENTIAL,
        &config,
        &accepted,
        &proposed,
        context,
        ArtifactSourceAttribution::CallerSuppliedAiAttributed,
        key,
    )
    .unwrap()
    {
        DurableArtifactProposalRecovery::Prepared(prepared) => prepared,
        DurableArtifactProposalRecovery::Pending(_) => panic!("Proposal CAS was not attempted"),
    };
    let published = publish_prepared_durable_artifact_proposal(recovered_prepared).unwrap();
    let after_proposal_cas = repository_state(&temp.repository);

    // Process loss after Proposal CAS but before ReviewId persistence is
    // reconciled from the exact live Ref/reflog and canonical graph.
    drop(published);
    drop(journal);
    let mut journal = reopen(&temp.journal);
    let pending = match recover_durable_artifact_proposal(
        &mut journal,
        &recovery_registry,
        HOST_CREDENTIAL,
        &config,
        &accepted,
        &proposed,
        context,
        ArtifactSourceAttribution::CallerSuppliedAiAttributed,
        key,
    )
    .unwrap()
    {
        DurableArtifactProposalRecovery::Pending(pending) => pending,
        DurableArtifactProposalRecovery::Prepared(_) => panic!("Proposal CAS was committed"),
    };
    let review_id = pending.review_id().clone();
    assert_eq!(repository_state(&temp.repository), after_proposal_cas);

    // Response loss after ReviewId persistence replays the same locator and
    // creates no object or reflog event.
    drop(pending);
    drop(journal);
    let mut journal = reopen(&temp.journal);
    let replay = match recover_durable_artifact_proposal(
        &mut journal,
        &recovery_registry,
        HOST_CREDENTIAL,
        &config,
        &accepted,
        &proposed,
        context,
        ArtifactSourceAttribution::CallerSuppliedAiAttributed,
        key,
    )
    .unwrap()
    {
        DurableArtifactProposalRecovery::Pending(pending) => pending,
        DurableArtifactProposalRecovery::Prepared(_) => panic!("published replay regressed"),
    };
    assert_eq!(replay.review_id(), &review_id);
    assert_eq!(repository_state(&temp.repository), after_proposal_cas);

    let changed_request = recover_durable_artifact_proposal(
        &mut journal,
        &recovery_registry,
        HOST_CREDENTIAL,
        &config,
        &accepted,
        &manifest("changed-proposal"),
        context,
        ArtifactSourceAttribution::CallerSuppliedAiAttributed,
        key,
    )
    .unwrap_err();
    assert_eq!(changed_request.code(), "idempotency_conflict");
}

#[test]
fn review_lookup_authenticates_and_authorizes_before_review_id_resolution() {
    let temp = TempReview::new("anti-oracle");
    let config = temp.config("durable-anti-oracle");
    let accepted = manifest("accepted");
    let proposed = manifest("proposed");
    let mut journal = reopen(&temp.journal);
    let prepared = prepare_durable_artifact_proposal(
        &mut journal,
        &config,
        &accepted,
        &proposed,
        b"{}",
        ArtifactSourceAttribution::CallerSuppliedAiAttributed,
        b"anti-oracle-proposal-key",
    )
    .unwrap();
    let published = publish_prepared_durable_artifact_proposal(prepared).unwrap();
    let pending = commit_published_durable_artifact_proposal(&mut journal, published).unwrap();
    let real_id = pending.review_id().clone();
    let unknown_id = ArtifactReviewId::parse("0".repeat(64)).unwrap();

    let nonmember: TestApprovalRegistry = ArtifactApprovalRegistry::new(
        support::TestHostAuthenticator::default(),
        support::TestClock::default(),
        60_000_000_000,
    )
    .unwrap();
    for review_id in [&real_id, &unknown_id] {
        let denied = get_durable_artifact_review_status(
            &nonmember,
            HOST_CREDENTIAL,
            &mut journal,
            &config,
            review_id,
        )
        .unwrap_err();
        assert_eq!(denied.code(), "project_access_denied");
    }
    for key in [
        b"anti-oracle-proposal-key".as_slice(),
        b"unknown-proposal-key".as_slice(),
    ] {
        let denied = recover_durable_artifact_proposal(
            &mut journal,
            &nonmember,
            HOST_CREDENTIAL,
            &config,
            &accepted,
            &proposed,
            b"{}",
            ArtifactSourceAttribution::CallerSuppliedAiAttributed,
            key,
        )
        .unwrap_err();
        assert_eq!(denied.code(), "project_access_denied");
    }
    let unauthenticated = get_durable_artifact_review_status(
        &nonmember,
        "INVALID-CREDENTIAL-CANARY",
        &mut journal,
        &config,
        &real_id,
    )
    .unwrap_err();
    assert_eq!(unauthenticated.code(), "authentication_failed");

    let (member, _, _) = approval_registry(pending.pending());
    let missing = get_durable_artifact_review_status(
        &member,
        HOST_CREDENTIAL,
        &mut journal,
        &config,
        &unknown_id,
    )
    .unwrap_err();
    assert_eq!(missing.code(), "artifact_review_not_found");
    let status = get_durable_artifact_review_status(
        &member,
        HOST_CREDENTIAL,
        &mut journal,
        &config,
        &real_id,
    )
    .unwrap();
    assert_eq!(status.state(), DurableArtifactReviewState::PendingReview);

    for rendered in [
        format!("{unauthenticated:?}"),
        unauthenticated.to_string(),
        format!("{status:?}"),
    ] {
        assert!(!rendered.contains("INVALID-CREDENTIAL"));
        assert!(!rendered.contains("PRIVATE-CONTEXT"));
        assert!(!rendered.contains(&temp.repository.display().to_string()));
        assert!(!rendered.contains("proposal/artifact/"));
        assert!(!rendered.contains("sg-oid-v1"));
    }
}

#[test]
fn restart_before_decision_cas_replays_objects_and_unknown_after_cas_reconciles() {
    let temp = TempReview::new("decision-reconcile");
    let config = temp.config("durable-decision-reconcile");
    let accepted = manifest("accepted");
    let proposed = manifest("proposed");
    let mut journal = reopen(&temp.journal);
    let prepared = prepare_durable_artifact_proposal(
        &mut journal,
        &config,
        &accepted,
        &proposed,
        b"{}",
        ArtifactSourceAttribution::CallerSuppliedAiAttributed,
        b"decision-reconcile-proposal-key",
    )
    .unwrap();
    let published = publish_prepared_durable_artifact_proposal(prepared).unwrap();
    let mut pending = commit_published_durable_artifact_proposal(&mut journal, published).unwrap();
    let review_id = pending.review_id().clone();
    let options = decision(
        ArtifactDisposition::AdoptedUnchanged,
        "PRIVATE-DECISION-RATIONALE-CANARY",
    );
    let decision_key = b"PRIVATE-DECISION-IDEMPOTENCY-CANARY";
    let (old_registry, _, _) = approval_registry(pending.pending());
    let approval = old_registry
        .issue_artifact_decision(HOST_CREDENTIAL, pending.pending(), &options)
        .unwrap();
    let prepared_decision = prepare_durable_artifact_decision(
        &mut journal,
        &old_registry,
        HOST_CREDENTIAL,
        &approval,
        &mut pending,
        &options,
        decision_key,
    )
    .unwrap();
    let after_first_prepare = repository_state(&temp.repository);

    // The old permit and pending handle are discarded. Fresh recovery and a
    // fresh approval reproduce the same CAS objects without extra writes.
    drop(prepared_decision);
    drop(pending);
    drop(journal);
    drop(old_registry);
    let mut journal = reopen(&temp.journal);
    let registry = fresh_registry(&config);
    let mut recovered = recover_durable_artifact_review(
        &registry,
        HOST_CREDENTIAL,
        &mut journal,
        &config,
        &review_id,
    )
    .unwrap();
    let retry_approval = registry
        .issue_artifact_decision(HOST_CREDENTIAL, recovered.pending(), &options)
        .unwrap();
    let retry_prepared = prepare_durable_artifact_decision(
        &mut journal,
        &registry,
        HOST_CREDENTIAL,
        &retry_approval,
        &mut recovered,
        &options,
        decision_key,
    )
    .unwrap();
    assert_eq!(repository_state(&temp.repository), after_first_prepare);

    let published_decision =
        publish_prepared_durable_artifact_decision(&mut journal, &mut recovered, retry_prepared)
            .unwrap();
    let after_decision_cas = repository_state(&temp.repository);
    drop(published_decision);
    let unknown = get_durable_artifact_review_status(
        &registry,
        HOST_CREDENTIAL,
        &mut journal,
        &config,
        &review_id,
    )
    .unwrap();
    assert_eq!(unknown.state(), DurableArtifactReviewState::OutcomeUnknown);

    // Receipt persistence and response delivery can be completed after reopen
    // from journal intent + live exact CAS evidence.
    drop(recovered);
    drop(journal);
    let mut journal = reopen(&temp.journal);
    let reconciled = reconcile_durable_artifact_review(
        &registry,
        HOST_CREDENTIAL,
        &mut journal,
        &config,
        &review_id,
        ArtifactCheckoutLimits::default(),
    )
    .unwrap();
    assert_eq!(
        reconciled.status().state(),
        DurableArtifactReviewState::DecisionCommitted
    );
    let checkout = reconciled.checked_out_artifact().unwrap();
    assert_eq!(
        checkout.disposition(),
        ArtifactDisposition::AdoptedUnchanged
    );
    let expected_index = b"<!doctype html><title>proposed</title>";
    assert_eq!(
        checkout.bytes("index.html"),
        Some(expected_index.as_slice())
    );
    assert_eq!(repository_state(&temp.repository), after_decision_cas);

    let replay = reconcile_durable_artifact_review(
        &registry,
        HOST_CREDENTIAL,
        &mut journal,
        &config,
        &review_id,
        ArtifactCheckoutLimits::default(),
    )
    .unwrap();
    assert!(replay.checked_out_artifact().is_some());
    assert_eq!(repository_state(&temp.repository), after_decision_cas);
    let no_second_disposition = recover_durable_artifact_review(
        &registry,
        HOST_CREDENTIAL,
        &mut journal,
        &config,
        &review_id,
    )
    .unwrap_err();
    assert_eq!(no_second_disposition.code(), "decision_committed");

    let rendered = format!("{:?}", reconciled.status());
    assert!(!rendered.contains("PRIVATE-DECISION"));
    assert!(!rendered.contains(&temp.repository.display().to_string()));
    assert!(!rendered.contains("proposal/artifact/"));
    assert!(!rendered.contains("sg-oid-v1"));

    // A later canonical Decision keeps the first review committed but makes
    // its checkout historical. Reconciliation proves the complete Decision
    // chain and never returns stale Accepted bytes.
    let next = manifest("proposed-next");
    let mut next_pending = begin_next_artifact_proposal(
        &config,
        &proposed,
        &next,
        br#"{"turn":2}"#,
        ArtifactSourceAttribution::CallerSuppliedAiAttributed,
    )
    .unwrap();
    approved_decide(
        &mut next_pending,
        &decision(ArtifactDisposition::Deferred, "defer the second proposal"),
    )
    .unwrap();
    drop(next_pending);
    let superseded = reconcile_durable_artifact_review(
        &registry,
        HOST_CREDENTIAL,
        &mut journal,
        &config,
        &review_id,
        ArtifactCheckoutLimits::default(),
    )
    .unwrap();
    assert_eq!(
        superseded.status().state(),
        DurableArtifactReviewState::DecisionCommitted
    );
    assert_eq!(
        superseded.checkout_state(),
        DurableArtifactCheckoutState::Superseded
    );
    assert!(superseded.checked_out_artifact().is_none());
}

#[test]
fn decision_cas_then_later_canonical_decision_reconciles_as_superseded() {
    let temp = TempReview::new("decision-cas-then-superseded");
    let config = temp.config("durable-decision-cas-then-superseded");
    let accepted = manifest("accepted");
    let proposed = manifest("proposed");
    let mut journal = reopen(&temp.journal);
    let prepared = prepare_durable_artifact_proposal(
        &mut journal,
        &config,
        &accepted,
        &proposed,
        b"{}",
        ArtifactSourceAttribution::CallerSuppliedAiAttributed,
        b"cas-then-superseded-proposal-key",
    )
    .unwrap();
    let published = publish_prepared_durable_artifact_proposal(prepared).unwrap();
    let mut pending = commit_published_durable_artifact_proposal(&mut journal, published).unwrap();
    let review_id = pending.review_id().clone();
    let options = decision(
        ArtifactDisposition::AdoptedUnchanged,
        "adopt before the response is lost",
    );
    let (registry, _, _) = approval_registry(pending.pending());
    let approval = registry
        .issue_artifact_decision(HOST_CREDENTIAL, pending.pending(), &options)
        .unwrap();
    let prepared_decision = prepare_durable_artifact_decision(
        &mut journal,
        &registry,
        HOST_CREDENTIAL,
        &approval,
        &mut pending,
        &options,
        b"cas-then-superseded-decision-key",
    )
    .unwrap();
    let published_decision =
        publish_prepared_durable_artifact_decision(&mut journal, &mut pending, prepared_decision)
            .unwrap();
    drop(published_decision);
    drop(pending);

    // The exact H -> C CAS committed, but its outcome row and response were
    // lost. A later ordinary C -> D Decision advances the canonical history.
    let next = manifest("later-proposal");
    let mut next_pending = begin_next_artifact_proposal(
        &config,
        &proposed,
        &next,
        br#"{"turn":"later"}"#,
        ArtifactSourceAttribution::CallerSuppliedAiAttributed,
    )
    .unwrap();
    approved_decide(
        &mut next_pending,
        &decision(ArtifactDisposition::Deferred, "defer the later proposal"),
    )
    .unwrap();
    drop(next_pending);
    let after_later_decision = repository_state(&temp.repository);

    drop(journal);
    let mut journal = reopen(&temp.journal);
    let restarted_registry = fresh_registry(&config);
    let reconciled = reconcile_durable_artifact_review(
        &restarted_registry,
        HOST_CREDENTIAL,
        &mut journal,
        &config,
        &review_id,
        ArtifactCheckoutLimits::default(),
    )
    .unwrap();
    assert_eq!(
        reconciled.status().state(),
        DurableArtifactReviewState::DecisionCommitted
    );
    assert_eq!(
        reconciled.status().decision().unwrap().disposition(),
        ArtifactDisposition::AdoptedUnchanged
    );
    assert_eq!(
        reconciled.checkout_state(),
        DurableArtifactCheckoutState::Superseded
    );
    assert!(reconciled.checked_out_artifact().is_none());
    assert_eq!(repository_state(&temp.repository), after_later_decision);

    let replay = reconcile_durable_artifact_review(
        &restarted_registry,
        HOST_CREDENTIAL,
        &mut journal,
        &config,
        &review_id,
        ArtifactCheckoutLimits::default(),
    )
    .unwrap();
    assert_eq!(
        replay.checkout_state(),
        DurableArtifactCheckoutState::Superseded
    );
    assert!(replay.checked_out_artifact().is_none());
    assert_eq!(repository_state(&temp.repository), after_later_decision);
}

#[test]
fn superseded_unknown_decision_still_verifies_historical_selected_bytes() {
    let temp = TempReview::new("superseded-corrupt-selected-bytes");
    let config = temp.config("durable-superseded-corrupt-selected-bytes");
    let accepted = manifest("accepted");
    let proposed = manifest("proposed");
    let mut journal = reopen(&temp.journal);
    let prepared = prepare_durable_artifact_proposal(
        &mut journal,
        &config,
        &accepted,
        &proposed,
        b"{}",
        ArtifactSourceAttribution::CallerSuppliedAiAttributed,
        b"superseded-corrupt-proposal-key",
    )
    .unwrap();
    let published = publish_prepared_durable_artifact_proposal(prepared).unwrap();
    let mut pending = commit_published_durable_artifact_proposal(&mut journal, published).unwrap();
    let review_id = pending.review_id().clone();
    let journal_review_id = ReviewId::parse(review_id.as_str()).unwrap();
    let options = decision(ArtifactDisposition::AdoptedUnchanged, "adopt before crash");
    let (registry, _, _) = approval_registry(pending.pending());
    let approval = registry
        .issue_artifact_decision(HOST_CREDENTIAL, pending.pending(), &options)
        .unwrap();
    let prepared_decision = prepare_durable_artifact_decision(
        &mut journal,
        &registry,
        HOST_CREDENTIAL,
        &approval,
        &mut pending,
        &options,
        b"superseded-corrupt-decision-key",
    )
    .unwrap();
    let published_decision =
        publish_prepared_durable_artifact_decision(&mut journal, &mut pending, prepared_decision)
            .unwrap();
    drop(published_decision);
    drop(pending);

    let intent = journal
        .get_decision_commit_intent(&journal_review_id)
        .unwrap()
        .unwrap();
    let repository = Repository::open(&temp.repository).unwrap();
    let candidate = object_json(&repository, intent.new_decision_head());
    let feedback = object_json(
        &repository,
        candidate["transition_refs"][0].as_str().unwrap(),
    );
    let proposal = object_json(
        &repository,
        feedback["payload"]["proposal_ref"].as_str().unwrap(),
    );
    let proposal_snapshot = object_json(&repository, proposal["snapshot"].as_str().unwrap());
    let site = object_json(
        &repository,
        proposal_snapshot["entries"]["site"]["oid"]
            .as_str()
            .unwrap(),
    );
    let selected_blob = site["entries"]["index.html"]["oid"]
        .as_str()
        .unwrap()
        .to_owned();
    drop(repository);

    let next = manifest("later-proposal");
    let mut next_pending = begin_next_artifact_proposal(
        &config,
        &proposed,
        &next,
        br#"{"turn":"later"}"#,
        ArtifactSourceAttribution::CallerSuppliedAiAttributed,
    )
    .unwrap();
    approved_decide(
        &mut next_pending,
        &decision(ArtifactDisposition::Deferred, "defer the later proposal"),
    )
    .unwrap();
    drop(next_pending);
    fs::write(
        cas_object_path(&temp.repository, &selected_blob),
        b"corrupt historical selected bytes",
    )
    .unwrap();

    drop(journal);
    let mut journal = reopen(&temp.journal);
    let restarted_registry = fresh_registry(&config);
    let error = reconcile_durable_artifact_review(
        &restarted_registry,
        HOST_CREDENTIAL,
        &mut journal,
        &config,
        &review_id,
        ArtifactCheckoutLimits::default(),
    )
    .unwrap_err();
    assert_eq!(error.code(), "oid_mismatch");
    let status = get_durable_artifact_review_status(
        &restarted_registry,
        HOST_CREDENTIAL,
        &mut journal,
        &config,
        &review_id,
    )
    .unwrap();
    assert_eq!(status.state(), DurableArtifactReviewState::OutcomeUnknown);
    assert!(
        journal
            .get_decision_outcome(&journal_review_id)
            .unwrap()
            .is_none()
    );
}

#[test]
fn stale_proposal_candidate_is_not_finalized_as_a_public_review() {
    let temp = TempReview::new("stale-proposal");
    let config = temp.config("durable-stale-proposal");
    let accepted = manifest("accepted");
    let proposed = manifest("proposed");
    let key = b"stale-proposal-key";
    let mut journal = reopen(&temp.journal);
    let prepared = prepare_durable_artifact_proposal(
        &mut journal,
        &config,
        &accepted,
        &proposed,
        b"{}",
        ArtifactSourceAttribution::CallerSuppliedAiAttributed,
        key,
    )
    .unwrap();
    drop(prepared);
    let intent = journal
        .get_proposal_intent_by_idempotency(config.project_selector().as_str(), key)
        .unwrap()
        .unwrap();
    let mut repository = Repository::open(&temp.repository).unwrap();
    let competing = store_changed_object(
        &repository,
        intent.binding().proposal_head(),
        "competing Proposal",
        |_| {},
    );
    repository
        .update_ref(RefUpdate {
            ref_name: intent.binding().proposal_ref_name(),
            expected_head: None,
            new_head: &competing,
            metadata: ReflogMetadata {
                occurred_at_unix_nanos: i64::MAX - 10,
                actor: Some("external-test-writer"),
                message: Some("publish competing Proposal"),
            },
        })
        .unwrap();
    drop(repository);

    let registry = fresh_registry(&config);
    let error = recover_durable_artifact_proposal(
        &mut journal,
        &registry,
        HOST_CREDENTIAL,
        &config,
        &accepted,
        &proposed,
        b"{}",
        ArtifactSourceAttribution::CallerSuppliedAiAttributed,
        key,
    )
    .unwrap_err();
    assert_eq!(error.code(), "ref_conflict");
    assert!(
        journal
            .get_proposal_intent(intent.proposal_intent_id())
            .unwrap()
            .review_id()
            .is_none()
    );
}

#[test]
fn competing_decision_with_a_different_disposition_becomes_terminal_denial() {
    let temp = TempReview::new("stale-decision");
    let config = temp.config("durable-stale-decision");
    let accepted = manifest("accepted");
    let proposed = manifest("proposed");
    let mut journal = reopen(&temp.journal);
    let prepared = prepare_durable_artifact_proposal(
        &mut journal,
        &config,
        &accepted,
        &proposed,
        b"{}",
        ArtifactSourceAttribution::CallerSuppliedAiAttributed,
        b"stale-decision-proposal-key",
    )
    .unwrap();
    let published = publish_prepared_durable_artifact_proposal(prepared).unwrap();
    let mut pending = commit_published_durable_artifact_proposal(&mut journal, published).unwrap();
    let review_id = pending.review_id().clone();
    let journal_review_id = ReviewId::parse(review_id.as_str()).unwrap();
    let options = decision(ArtifactDisposition::Deferred, "defer this proposal");
    let (registry, _, _) = approval_registry(pending.pending());
    let approval = registry
        .issue_artifact_decision(HOST_CREDENTIAL, pending.pending(), &options)
        .unwrap();
    let prepared_decision = prepare_durable_artifact_decision(
        &mut journal,
        &registry,
        HOST_CREDENTIAL,
        &approval,
        &mut pending,
        &options,
        b"stale-decision-key",
    )
    .unwrap();
    let intent = journal
        .get_decision_commit_intent(&journal_review_id)
        .unwrap()
        .unwrap();
    let mut repository = Repository::open(&temp.repository).unwrap();
    let competing_feedback = store_changed_object(
        &repository,
        intent.feedback_oid(),
        "Rejected Feedback",
        |feedback| feedback["payload"]["disposition"] = json!("rejected"),
    );
    let competing_decision = store_changed_object(
        &repository,
        intent.new_decision_head(),
        "Rejected Decision",
        |decision| decision["transition_refs"] = json!([competing_feedback]),
    );
    repository
        .update_ref(RefUpdate {
            ref_name: intent.binding().decision_ref_name(),
            expected_head: Some(intent.expected_decision_head()),
            new_head: &competing_decision,
            metadata: ReflogMetadata {
                occurred_at_unix_nanos: i64::MAX - 9,
                actor: Some("external-test-writer"),
                message: Some("publish competing Rejected Decision"),
            },
        })
        .unwrap();
    drop(repository);

    let publication_error =
        publish_prepared_durable_artifact_decision(&mut journal, &mut pending, prepared_decision)
            .unwrap_err();
    assert_eq!(publication_error.code(), "ref_conflict");
    drop(pending);
    let reconciled = reconcile_durable_artifact_review(
        &registry,
        HOST_CREDENTIAL,
        &mut journal,
        &config,
        &review_id,
        ArtifactCheckoutLimits::default(),
    )
    .unwrap();
    assert_eq!(
        reconciled.status().state(),
        DurableArtifactReviewState::TerminalDenial
    );
    assert_eq!(
        reconciled.checkout_state(),
        DurableArtifactCheckoutState::Unavailable
    );
    assert!(reconciled.checked_out_artifact().is_none());
}

#[test]
fn non_decision_commit_cannot_spoof_a_canonical_descendant() {
    let temp = TempReview::new("spoofed-descendant");
    let config = temp.config("durable-spoofed-descendant");
    let accepted = manifest("accepted");
    let proposed = manifest("proposed");
    let mut journal = reopen(&temp.journal);
    let prepared = prepare_durable_artifact_proposal(
        &mut journal,
        &config,
        &accepted,
        &proposed,
        b"{}",
        ArtifactSourceAttribution::CallerSuppliedAiAttributed,
        b"spoofed-descendant-proposal-key",
    )
    .unwrap();
    let published = publish_prepared_durable_artifact_proposal(prepared).unwrap();
    let mut pending = commit_published_durable_artifact_proposal(&mut journal, published).unwrap();
    let review_id = pending.review_id().clone();
    let options = decision(ArtifactDisposition::AdoptedUnchanged, "adopt the proposal");
    let (registry, _, _) = approval_registry(pending.pending());
    let approval = registry
        .issue_artifact_decision(HOST_CREDENTIAL, pending.pending(), &options)
        .unwrap();
    let prepared_decision = prepare_durable_artifact_decision(
        &mut journal,
        &registry,
        HOST_CREDENTIAL,
        &approval,
        &mut pending,
        &options,
        b"spoofed-descendant-decision-key",
    )
    .unwrap();
    let published_decision =
        publish_prepared_durable_artifact_decision(&mut journal, &mut pending, prepared_decision)
            .unwrap();
    drop(pending);
    let committed = commit_published_durable_artifact_decision(
        &mut journal,
        &config,
        published_decision,
        ArtifactCheckoutLimits::default(),
    )
    .unwrap();
    assert_eq!(
        committed.checkout_state(),
        DurableArtifactCheckoutState::Current
    );
    drop(committed);
    let journal_review_id = ReviewId::parse(review_id.as_str()).unwrap();
    let outcome = journal
        .get_decision_outcome(&journal_review_id)
        .unwrap()
        .unwrap();
    let old_head = outcome.new_decision_head().to_owned();
    assert_eq!(outcome.review_id(), &journal_review_id);
    let decision_ref = format!("decision/artifact/{}", config.project_key());
    let mut repository = Repository::open(&temp.repository).unwrap();
    let spoof = store_changed_object(&repository, &old_head, "non-Decision successor", |commit| {
        commit["commit_kind"] = json!("checkpoint");
        commit["parents"] = json!([old_head]);
        commit["transition_refs"] = json!([]);
    });
    repository
        .update_ref(RefUpdate {
            ref_name: &decision_ref,
            expected_head: Some(&old_head),
            new_head: &spoof,
            metadata: ReflogMetadata {
                occurred_at_unix_nanos: i64::MAX - 8,
                actor: Some("external-test-writer"),
                message: Some("advance Decision Ref to non-Decision Commit"),
            },
        })
        .unwrap();
    drop(repository);

    let error = reconcile_durable_artifact_review(
        &registry,
        HOST_CREDENTIAL,
        &mut journal,
        &config,
        &review_id,
        ArtifactCheckoutLimits::default(),
    )
    .unwrap_err();
    assert!(
        matches!(
            error.code(),
            "artifact_integrity_error" | "artifact_durable_integrity_error"
        ),
        "{}",
        error.code()
    );
}

#[test]
fn unknown_before_decision_cas_becomes_retryable_and_uses_fresh_authority() {
    let temp = TempReview::new("decision-before-cas");
    let config = temp.config("durable-decision-before-cas");
    let accepted = manifest("accepted");
    let proposed = manifest("proposed");
    let mut journal = reopen(&temp.journal);
    let prepared = prepare_durable_artifact_proposal(
        &mut journal,
        &config,
        &accepted,
        &proposed,
        b"{}",
        ArtifactSourceAttribution::CallerSuppliedAiAttributed,
        b"before-cas-proposal-key",
    )
    .unwrap();
    let published = publish_prepared_durable_artifact_proposal(prepared).unwrap();
    let mut pending = commit_published_durable_artifact_proposal(&mut journal, published).unwrap();
    let review_id = pending.review_id().clone();
    let options = decision(ArtifactDisposition::Deferred, "defer for later");
    let (old_registry, _, _) = approval_registry(pending.pending());
    let approval = old_registry
        .issue_artifact_decision(HOST_CREDENTIAL, pending.pending(), &options)
        .unwrap();
    let prepared_decision = prepare_durable_artifact_decision(
        &mut journal,
        &old_registry,
        HOST_CREDENTIAL,
        &approval,
        &mut pending,
        &options,
        b"before-cas-decision-key",
    )
    .unwrap();
    let journal_review_id = ReviewId::parse(review_id.as_str()).unwrap();
    journal
        .transition_review_state(
            &journal_review_id,
            ReviewState::PendingReview,
            ReviewState::OutcomeUnknown,
        )
        .unwrap();

    // Crash after durable attempt arming and before Decision CAS.
    drop(prepared_decision);
    drop(pending);
    drop(journal);
    drop(old_registry);
    let mut journal = reopen(&temp.journal);
    let registry = fresh_registry(&config);
    let reconciled = reconcile_durable_artifact_review(
        &registry,
        HOST_CREDENTIAL,
        &mut journal,
        &config,
        &review_id,
        ArtifactCheckoutLimits::default(),
    )
    .unwrap();
    assert_eq!(
        reconciled.status().state(),
        DurableArtifactReviewState::RetryableFailure
    );
    assert!(reconciled.checked_out_artifact().is_none());

    let recovered = recover_durable_artifact_review(
        &registry,
        HOST_CREDENTIAL,
        &mut journal,
        &config,
        &review_id,
    )
    .unwrap();
    assert_eq!(
        recovered.state(),
        synapse_artifact::PendingArtifactState::Ready
    );
}

#[test]
fn acl_profile_and_expiry_changes_fail_closed_during_restart_recovery() {
    let temp = TempReview::new("recovery-security");
    let config = temp.config("durable-recovery-security");
    let accepted = manifest("accepted");
    let proposed = manifest("proposed");
    let mut journal = reopen(&temp.journal);
    let prepared = prepare_durable_artifact_proposal(
        &mut journal,
        &config,
        &accepted,
        &proposed,
        b"{}",
        ArtifactSourceAttribution::CallerSuppliedAiAttributed,
        b"recovery-security-key",
    )
    .unwrap();
    let published = publish_prepared_durable_artifact_proposal(prepared).unwrap();
    let pending = commit_published_durable_artifact_proposal(&mut journal, published).unwrap();
    let review_id = pending.review_id().clone();
    let (registry, _, _) = approval_registry(pending.pending());
    registry
        .revoke_project_access(pending.pending().durable_binding().project(), HOST_ACTOR)
        .unwrap();
    let denied = recover_durable_artifact_review(
        &registry,
        HOST_CREDENTIAL,
        &mut journal,
        &config,
        &review_id,
    )
    .unwrap_err();
    assert_eq!(denied.code(), "project_access_denied");
    registry
        .grant_project_access(pending.pending().durable_binding().project(), HOST_ACTOR)
        .unwrap();

    let changed_profile = TrustedArtifactProjectConfig::new(
        &temp.repository,
        "durable-recovery-security",
        "Changed Creator Profile",
        "Application-owned AI",
        "2026-07-19T00:00:00.000000000Z",
        "2099-01-01T00:00:00.000000000Z",
    );
    let profile_error = recover_durable_artifact_review(
        &registry,
        HOST_CREDENTIAL,
        &mut journal,
        &changed_profile,
        &review_id,
    )
    .unwrap_err();
    assert!(
        matches!(
            profile_error.code(),
            "artifact_recovery_mismatch"
                | "authorization_denied"
                | "artifact_integrity_error"
                | "artifact_profile_unsupported"
        ),
        "{}",
        profile_error.code()
    );

    let expired_profile = TrustedArtifactProjectConfig::new(
        &temp.repository,
        "durable-recovery-security",
        "Artifact Creator",
        "Application-owned AI",
        "2020-01-01T00:00:00.000000000Z",
        "2020-01-02T00:00:00.000000000Z",
    );
    let expiry_error = recover_durable_artifact_review(
        &registry,
        HOST_CREDENTIAL,
        &mut journal,
        &expired_profile,
        &review_id,
    )
    .unwrap_err();
    assert_ne!(expiry_error.code(), "pending_review");
}
