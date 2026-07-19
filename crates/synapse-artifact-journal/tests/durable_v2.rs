use rusqlite::{Connection, params};
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Barrier};
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};
use synapse_artifact_journal::{
    DecisionCommitIntentRequest, DecisionDisposition, DecisionOutcomeRegistrationOutcome,
    DecisionOutcomeRequest, IntentRegistrationOutcome, ProposalIntentRegistrationOutcome,
    ProposalIntentRequest, ReviewBinding, ReviewRegistrationOutcome, ReviewState, SelectedSnapshot,
    SqliteReviewJournal,
};

const MANIFEST_SHA256: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const OTHER_MANIFEST_SHA256: &str =
    "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const NEW_DECISION_HEAD: &str = "commit:sg-oid-v1:sha256:decision-candidate";
const OTHER_DECISION_HEAD: &str = "commit:sg-oid-v1:sha256:other-decision";
const FEEDBACK_OID: &str = "record:sg-oid-v1:sha256:feedback";
const OTHER_FEEDBACK_OID: &str = "record:sg-oid-v1:sha256:other-feedback";

static NEXT_TEMP: AtomicU64 = AtomicU64::new(1);

struct TempDatabase {
    directory: PathBuf,
    database: PathBuf,
}

impl TempDatabase {
    fn new(label: &str) -> Self {
        let serial = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory = std::env::temp_dir().join(format!(
            "synapsegit-artifact-journal-v2-{label}-{}-{nanos}-{serial}",
            std::process::id()
        ));
        fs::create_dir(&directory).unwrap();
        let database = directory.join("journal.sqlite3");
        Self {
            directory,
            database,
        }
    }
}

impl Drop for TempDatabase {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.directory);
    }
}

fn binding(suffix: &str) -> ReviewBinding {
    ReviewBinding::new(
        format!("project-{suffix}"),
        format!("proposal/artifact/{suffix}"),
        format!("commit:sg-oid-v1:sha256:proposal-{suffix}"),
        format!("decision/artifact/{suffix}"),
        "commit:sg-oid-v1:sha256:base",
    )
    .unwrap()
}

fn proposal_request<'a>(
    key: &'a [u8],
    request: &'a [u8],
    binding: &'a ReviewBinding,
) -> ProposalIntentRequest<'a> {
    ProposalIntentRequest {
        idempotency_key: key,
        canonical_request: request,
        artifact_manifest_sha256: MANIFEST_SHA256,
        binding,
    }
}

fn decision_intent_request<'a>(
    key: &'a [u8],
    request: &'a [u8],
) -> DecisionCommitIntentRequest<'a> {
    DecisionCommitIntentRequest {
        idempotency_key: key,
        canonical_request: request,
        disposition: DecisionDisposition::AdoptedUnchanged,
        selected_snapshot: SelectedSnapshot::Proposal,
        reviewed_artifact_manifest_sha256: MANIFEST_SHA256,
        new_decision_head: NEW_DECISION_HEAD,
        feedback_oid: FEEDBACK_OID,
    }
}

fn decision_outcome(binding: &ReviewBinding) -> DecisionOutcomeRequest<'_> {
    DecisionOutcomeRequest {
        disposition: DecisionDisposition::AdoptedUnchanged,
        selected_snapshot: SelectedSnapshot::Proposal,
        reviewed_artifact_manifest_sha256: MANIFEST_SHA256,
        proposal_head: binding.proposal_head(),
        expected_decision_head: binding.expected_decision_head(),
        new_decision_head: NEW_DECISION_HEAD,
        feedback_oid: FEEDBACK_OID,
    }
}

fn create_published_review(
    journal: &mut SqliteReviewJournal,
    suffix: &str,
) -> (ReviewBinding, synapse_artifact_journal::ReviewId) {
    let binding = binding(suffix);
    let key = format!("proposal-key-{suffix}");
    let canonical_request = format!("{{\"project\":\"{suffix}\"}}");
    let intent = journal
        .register_proposal_intent(proposal_request(
            key.as_bytes(),
            canonical_request.as_bytes(),
            &binding,
        ))
        .unwrap();
    let review = journal
        .commit_proposal_publication(intent.intent().proposal_intent_id(), &binding)
        .unwrap();
    (binding, review.review().review_id().clone())
}

#[test]
fn proposal_review_id_is_created_only_after_exact_publication_commit() {
    let mut journal = SqliteReviewJournal::open_in_memory().unwrap();
    let exact_binding = binding("two-stage");
    let request = proposal_request(
        b"two-stage-key",
        br#"{"operation":"publish"}"#,
        &exact_binding,
    );

    let created = journal.register_proposal_intent(request).unwrap();
    assert_eq!(
        created.outcome(),
        ProposalIntentRegistrationOutcome::Created
    );
    assert!(created.intent().review_id().is_none());
    assert_eq!(
        journal
            .get_proposal_intent_by_idempotency(exact_binding.project_scope(), b"two-stage-key",)
            .unwrap()
            .as_ref(),
        Some(created.intent())
    );
    assert!(
        journal
            .get_proposal_intent_by_idempotency(exact_binding.project_scope(), b"unknown-key",)
            .unwrap()
            .is_none()
    );
    assert!(
        journal
            .get_review_by_binding(&exact_binding)
            .unwrap()
            .is_none()
    );
    assert_eq!(
        journal
            .list_unfinalized_proposal_intents(exact_binding.project_scope())
            .unwrap(),
        vec![created.intent().clone()]
    );

    let replay = journal.register_proposal_intent(request).unwrap();
    assert_eq!(
        replay.outcome(),
        ProposalIntentRegistrationOutcome::Replayed
    );
    assert_eq!(
        replay.intent().proposal_intent_id(),
        created.intent().proposal_intent_id()
    );

    let changed_request = journal
        .register_proposal_intent(proposal_request(
            b"two-stage-key",
            br#"{"operation":"changed"}"#,
            &exact_binding,
        ))
        .unwrap_err();
    assert_eq!(changed_request.code(), "idempotency_conflict");
    let second_key = journal
        .register_proposal_intent(proposal_request(
            b"different-key",
            br#"{"operation":"publish"}"#,
            &exact_binding,
        ))
        .unwrap_err();
    assert_eq!(second_key.code(), "proposal_intent_exists");

    let conflicting_binding = ReviewBinding::new(
        exact_binding.project_scope(),
        exact_binding.proposal_ref_name(),
        exact_binding.proposal_head(),
        "decision/artifact/different",
        exact_binding.expected_decision_head(),
    )
    .unwrap();
    let conflict = journal
        .commit_proposal_publication(created.intent().proposal_intent_id(), &conflicting_binding)
        .unwrap_err();
    assert_eq!(conflict.code(), "proposal_intent_conflict");

    let finalized = journal
        .commit_proposal_publication(created.intent().proposal_intent_id(), &exact_binding)
        .unwrap();
    assert_eq!(finalized.outcome(), ReviewRegistrationOutcome::Created);
    let review_id = finalized.review().review_id().clone();
    assert_eq!(
        journal
            .get_proposal_intent(created.intent().proposal_intent_id())
            .unwrap()
            .review_id(),
        Some(&review_id)
    );
    assert_eq!(
        journal
            .get_review_artifact_manifest_sha256(&review_id)
            .unwrap()
            .as_deref(),
        Some(MANIFEST_SHA256)
    );
    assert!(
        journal
            .list_unfinalized_proposal_intents(exact_binding.project_scope())
            .unwrap()
            .is_empty()
    );

    let response_loss_replay = journal
        .commit_proposal_publication(created.intent().proposal_intent_id(), &exact_binding)
        .unwrap();
    assert_eq!(
        response_loss_replay.outcome(),
        ReviewRegistrationOutcome::Replayed
    );
    assert_eq!(response_loss_replay.review().review_id(), &review_id);
    let already_public = journal.register_proposal_intent(request).unwrap();
    assert_eq!(
        already_public.outcome(),
        ProposalIntentRegistrationOutcome::Replayed
    );
    assert_eq!(already_public.intent().review_id(), Some(&review_id));
}

#[test]
fn decision_outcome_requires_an_exact_intent_and_commits_state_transactionally() {
    let mut journal = SqliteReviewJournal::open_in_memory().unwrap();
    let (binding, review_id) = create_published_review(&mut journal, "decision-exact");
    let intent_request =
        decision_intent_request(b"decision-key", br#"{"disposition":"adopted_unchanged"}"#);
    let created = journal
        .register_decision_commit_intent(&review_id, intent_request)
        .unwrap();
    assert_eq!(created.outcome(), IntentRegistrationOutcome::Created);
    assert_eq!(created.intent().binding(), &binding);
    let replay = journal
        .register_decision_commit_intent(&review_id, intent_request)
        .unwrap();
    assert_eq!(replay.outcome(), IntentRegistrationOutcome::Replayed);
    assert_eq!(replay.intent(), created.intent());

    let changed_request = journal
        .register_decision_commit_intent(
            &review_id,
            decision_intent_request(b"decision-key", br#"{"disposition":"changed"}"#),
        )
        .unwrap_err();
    assert_eq!(changed_request.code(), "idempotency_conflict");
    let second_intent = journal
        .register_decision_commit_intent(
            &review_id,
            decision_intent_request(
                b"different-decision-key",
                br#"{"disposition":"adopted_unchanged"}"#,
            ),
        )
        .unwrap_err();
    assert_eq!(second_intent.code(), "decision_intent_exists");

    let exact = decision_outcome(&binding);
    let mismatches = [
        (
            "disposition",
            DecisionOutcomeRequest {
                disposition: DecisionDisposition::Rejected,
                selected_snapshot: SelectedSnapshot::Base,
                ..exact
            },
        ),
        (
            "manifest",
            DecisionOutcomeRequest {
                reviewed_artifact_manifest_sha256: OTHER_MANIFEST_SHA256,
                ..exact
            },
        ),
        (
            "proposal",
            DecisionOutcomeRequest {
                proposal_head: "commit:sg-oid-v1:sha256:other-proposal",
                ..exact
            },
        ),
        (
            "expected",
            DecisionOutcomeRequest {
                expected_decision_head: "commit:sg-oid-v1:sha256:other-base",
                ..exact
            },
        ),
        (
            "new",
            DecisionOutcomeRequest {
                new_decision_head: OTHER_DECISION_HEAD,
                ..exact
            },
        ),
        (
            "feedback",
            DecisionOutcomeRequest {
                feedback_oid: OTHER_FEEDBACK_OID,
                ..exact
            },
        ),
    ];
    for (label, mismatch) in mismatches {
        let error = journal
            .commit_decision_outcome(&review_id, mismatch)
            .unwrap_err();
        assert_eq!(error.code(), "decision_intent_mismatch", "{label}");
        assert_eq!(
            journal.get_review(&review_id).unwrap().state(),
            ReviewState::PendingReview,
            "{label}"
        );
        assert!(journal.get_decision_outcome(&review_id).unwrap().is_none());
    }

    let committed = journal.commit_decision_outcome(&review_id, exact).unwrap();
    assert_eq!(
        committed.registration(),
        DecisionOutcomeRegistrationOutcome::Created
    );
    assert_eq!(
        journal.get_review(&review_id).unwrap().state(),
        ReviewState::DecisionCommitted
    );
    let replay = journal.commit_decision_outcome(&review_id, exact).unwrap();
    assert_eq!(
        replay.registration(),
        DecisionOutcomeRegistrationOutcome::Replayed
    );
    assert_eq!(replay.outcome(), committed.outcome());
    let conflicting_replay = journal
        .commit_decision_outcome(
            &review_id,
            DecisionOutcomeRequest {
                feedback_oid: OTHER_FEEDBACK_OID,
                ..exact
            },
        )
        .unwrap_err();
    assert_eq!(conflicting_replay.code(), "decision_outcome_conflict");

    let reconciliation = journal.get_review_reconciliation(&review_id).unwrap();
    assert_eq!(
        reconciliation.proposal_artifact_manifest_sha256(),
        Some(MANIFEST_SHA256)
    );
    assert_eq!(
        reconciliation.review().state(),
        ReviewState::DecisionCommitted
    );
    assert_eq!(reconciliation.decision_intent(), Some(created.intent()));
    assert_eq!(reconciliation.decision_outcome(), Some(committed.outcome()));
}

#[test]
fn reopen_reconciles_before_proposal_cas_and_after_unknown_decision_outcome() {
    let temporary = TempDatabase::new("reopen");
    let binding = binding("reopen");
    let proposal_intent_id = {
        let mut journal = SqliteReviewJournal::open(&temporary.database).unwrap();
        journal
            .register_proposal_intent(proposal_request(
                b"reopen-proposal-key",
                br#"{"phase":"before-proposal-cas"}"#,
                &binding,
            ))
            .unwrap()
            .intent()
            .proposal_intent_id()
            .clone()
    };

    let review_id = {
        let mut journal = SqliteReviewJournal::open(&temporary.database).unwrap();
        let pending = journal
            .list_unfinalized_proposal_intents(binding.project_scope())
            .unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].proposal_intent_id(), &proposal_intent_id);
        let review = journal
            .commit_proposal_publication(&proposal_intent_id, &binding)
            .unwrap()
            .into_review();
        let review_id = review.review_id().clone();
        journal
            .register_decision_commit_intent(
                &review_id,
                decision_intent_request(
                    b"reopen-decision-key",
                    br#"{"phase":"before-decision-cas"}"#,
                ),
            )
            .unwrap();
        journal
            .transition_review_state(
                &review_id,
                ReviewState::PendingReview,
                ReviewState::OutcomeUnknown,
            )
            .unwrap();
        review_id
    };

    {
        let mut journal = SqliteReviewJournal::open(&temporary.database).unwrap();
        let before_reconciliation = journal.get_review_reconciliation(&review_id).unwrap();
        assert_eq!(
            before_reconciliation.review().state(),
            ReviewState::OutcomeUnknown
        );
        assert!(before_reconciliation.decision_intent().is_some());
        assert!(before_reconciliation.decision_outcome().is_none());
        journal
            .commit_decision_outcome(&review_id, decision_outcome(&binding))
            .unwrap();
    }

    let mut reopened = SqliteReviewJournal::open(&temporary.database).unwrap();
    let reconciled = reopened.get_review_reconciliation(&review_id).unwrap();
    assert_eq!(reconciled.review().state(), ReviewState::DecisionCommitted);
    assert!(reconciled.decision_intent().is_some());
    assert!(reconciled.decision_outcome().is_some());
}

#[test]
fn exact_external_reconciliation_can_make_an_unknown_uncommitted_intent_retryable() {
    let mut journal = SqliteReviewJournal::open_in_memory().unwrap();
    let (_, review_id) = create_published_review(&mut journal, "retryable-reconciliation");
    let intent = journal
        .register_decision_commit_intent(
            &review_id,
            decision_intent_request(
                b"retryable-reconciliation-key",
                br#"{"phase":"decision-cas-unknown"}"#,
            ),
        )
        .unwrap()
        .into_intent();
    journal
        .transition_review_state(
            &review_id,
            ReviewState::PendingReview,
            ReviewState::OutcomeUnknown,
        )
        .unwrap();

    let reconciled = journal.reconcile_decision_not_committed(&intent).unwrap();
    assert_eq!(reconciled.state(), ReviewState::RetryableFailure);
    let replay = journal.reconcile_decision_not_committed(&intent).unwrap();
    assert_eq!(replay, reconciled);
}

#[test]
fn v2_state_transitions_require_strict_intent_and_exact_committed_outcome() {
    let mut journal = SqliteReviewJournal::open_in_memory().unwrap();
    let (_, review_id) = create_published_review(&mut journal, "guarded-state");

    for forbidden in [ReviewState::OutcomeUnknown, ReviewState::DecisionCommitted] {
        let error = journal
            .transition_review_state(&review_id, ReviewState::PendingReview, forbidden)
            .unwrap_err();
        assert_eq!(error.code(), "review_state_transition_invalid");
        assert_eq!(
            journal.get_review(&review_id).unwrap().state(),
            ReviewState::PendingReview
        );
    }

    journal
        .register_decision_commit_intent(
            &review_id,
            decision_intent_request(b"guarded-state-key", br#"{"decision":"exact"}"#),
        )
        .unwrap();
    journal
        .transition_review_state(
            &review_id,
            ReviewState::PendingReview,
            ReviewState::OutcomeUnknown,
        )
        .unwrap();
    let cannot_claim_commit = journal
        .transition_review_state(
            &review_id,
            ReviewState::OutcomeUnknown,
            ReviewState::DecisionCommitted,
        )
        .unwrap_err();
    assert_eq!(
        cannot_claim_commit.code(),
        "review_state_transition_invalid"
    );
}

#[test]
fn v2_reconciliation_rejects_committed_or_unknown_rows_missing_evidence() {
    let temporary = TempDatabase::new("corrupt-state-evidence");
    let (committed_id, unknown_id) = {
        let mut journal = SqliteReviewJournal::open(&temporary.database).unwrap();
        let (_, committed_id) = create_published_review(&mut journal, "corrupt-committed");
        let (_, unknown_id) = create_published_review(&mut journal, "corrupt-unknown");
        (committed_id, unknown_id)
    };
    let connection = Connection::open(&temporary.database).unwrap();
    connection
        .execute(
            "UPDATE reviews SET state = 'decision_committed' WHERE review_id = ?1",
            [committed_id.as_str()],
        )
        .unwrap();
    connection
        .execute(
            "UPDATE reviews SET state = 'outcome_unknown' WHERE review_id = ?1",
            [unknown_id.as_str()],
        )
        .unwrap();
    drop(connection);

    let mut journal = SqliteReviewJournal::open(&temporary.database).unwrap();
    for review_id in [&committed_id, &unknown_id] {
        let error = journal.get_review_reconciliation(review_id).unwrap_err();
        assert_eq!(error.code(), "journal_corrupt");
    }
}

#[test]
fn adopted_digest_must_match_proposal_while_base_selection_is_checkout_verified() {
    let mut journal = SqliteReviewJournal::open_in_memory().unwrap();
    let (_, adopted_review) = create_published_review(&mut journal, "adopted-digest");
    let mismatch = journal
        .register_decision_commit_intent(
            &adopted_review,
            DecisionCommitIntentRequest {
                reviewed_artifact_manifest_sha256: OTHER_MANIFEST_SHA256,
                ..decision_intent_request(b"adopted-digest-key", br#"{"decision":"adopt"}"#)
            },
        )
        .unwrap_err();
    assert_eq!(mismatch.code(), "decision_intent_mismatch");
    assert!(
        journal
            .get_decision_commit_intent(&adopted_review)
            .unwrap()
            .is_none()
    );

    let (_, rejected_review) = create_published_review(&mut journal, "rejected-base-digest");
    let rejected = journal
        .register_decision_commit_intent(
            &rejected_review,
            DecisionCommitIntentRequest {
                idempotency_key: b"rejected-base-digest-key",
                canonical_request: br#"{"decision":"reject"}"#,
                disposition: DecisionDisposition::Rejected,
                selected_snapshot: SelectedSnapshot::Base,
                reviewed_artifact_manifest_sha256: OTHER_MANIFEST_SHA256,
                new_decision_head: NEW_DECISION_HEAD,
                feedback_oid: FEEDBACK_OID,
            },
        )
        .unwrap();
    assert_eq!(
        rejected.intent().selected_snapshot(),
        SelectedSnapshot::Base
    );
}

#[test]
fn v2_idempotency_digests_are_separated_by_operation_and_scope() {
    let mut journal = SqliteReviewJournal::open_in_memory().unwrap();
    let first_binding = binding("digest-scope-first");
    let second_binding = binding("digest-scope-second");
    let raw_key = b"SAME-RAW-PRIVATE-IDEMPOTENCY-KEY";
    let first = journal
        .register_proposal_intent(proposal_request(
            raw_key,
            br#"{"proposal":"first"}"#,
            &first_binding,
        ))
        .unwrap()
        .into_intent();
    let second = journal
        .register_proposal_intent(proposal_request(
            raw_key,
            br#"{"proposal":"second"}"#,
            &second_binding,
        ))
        .unwrap()
        .into_intent();
    assert_ne!(first.idempotency_digest(), second.idempotency_digest());

    let review = journal
        .commit_proposal_publication(first.proposal_intent_id(), &first_binding)
        .unwrap()
        .into_review();
    let decision = journal
        .register_decision_commit_intent(
            review.review_id(),
            decision_intent_request(raw_key, br#"{"decision":"same-raw-key"}"#),
        )
        .unwrap();
    assert_ne!(
        first.idempotency_digest(),
        decision.intent().idempotency_digest()
    );
}

#[test]
fn concurrent_exact_finalization_creates_one_review_and_one_outcome() {
    const WORKERS: usize = 6;
    let temporary = TempDatabase::new("concurrent");
    let binding = binding("concurrent");
    let proposal_intent_id = {
        let mut journal = SqliteReviewJournal::open(&temporary.database).unwrap();
        journal
            .register_proposal_intent(proposal_request(
                b"concurrent-proposal-key",
                br#"{"phase":"proposal"}"#,
                &binding,
            ))
            .unwrap()
            .intent()
            .proposal_intent_id()
            .clone()
    };

    let barrier = Arc::new(Barrier::new(WORKERS));
    let handles = (0..WORKERS)
        .map(|_| {
            let database = temporary.database.clone();
            let binding = binding.clone();
            let proposal_intent_id = proposal_intent_id.clone();
            let barrier = Arc::clone(&barrier);
            thread::spawn(move || {
                let mut journal = SqliteReviewJournal::open(database).unwrap();
                barrier.wait();
                journal
                    .commit_proposal_publication(&proposal_intent_id, &binding)
                    .unwrap()
            })
        })
        .collect::<Vec<_>>();
    let reviews = handles
        .into_iter()
        .map(|handle| handle.join().unwrap())
        .collect::<Vec<_>>();
    let review_id = reviews[0].review().review_id().clone();
    assert!(
        reviews
            .iter()
            .all(|review| review.review().review_id() == &review_id)
    );
    assert_eq!(
        reviews
            .iter()
            .filter(|review| review.outcome() == ReviewRegistrationOutcome::Created)
            .count(),
        1
    );

    {
        let mut journal = SqliteReviewJournal::open(&temporary.database).unwrap();
        journal
            .register_decision_commit_intent(
                &review_id,
                decision_intent_request(b"concurrent-decision-key", br#"{"phase":"decision"}"#),
            )
            .unwrap();
    }
    let barrier = Arc::new(Barrier::new(WORKERS));
    let handles = (0..WORKERS)
        .map(|_| {
            let database = temporary.database.clone();
            let binding = binding.clone();
            let review_id = review_id.clone();
            let barrier = Arc::clone(&barrier);
            thread::spawn(move || {
                let mut journal = SqliteReviewJournal::open(database).unwrap();
                barrier.wait();
                journal
                    .commit_decision_outcome(&review_id, decision_outcome(&binding))
                    .unwrap()
            })
        })
        .collect::<Vec<_>>();
    let outcomes = handles
        .into_iter()
        .map(|handle| handle.join().unwrap())
        .collect::<Vec<_>>();
    assert!(
        outcomes
            .iter()
            .all(|outcome| outcome.outcome() == outcomes[0].outcome())
    );
    assert_eq!(
        outcomes
            .iter()
            .filter(|outcome| {
                outcome.registration() == DecisionOutcomeRegistrationOutcome::Created
            })
            .count(),
        1
    );
}

#[test]
fn raw_keys_requests_and_authority_canaries_never_reach_sqlite_or_debug() {
    let temporary = TempDatabase::new("privacy");
    let raw_proposal_key = b"RAW-PROPOSAL-IDEMPOTENCY-CANARY-4D19";
    let raw_proposal_request = br#"{"secret":"PRIVATE-PROPOSAL-CONTEXT-CANARY-A71C"}"#;
    let raw_decision_key = b"RAW-DECISION-IDEMPOTENCY-CANARY-5E20";
    let raw_decision_request = br#"{"rationale":"PRIVATE-DECISION-RATIONALE-CANARY-B82D"}"#;
    {
        let mut journal = SqliteReviewJournal::open(&temporary.database).unwrap();
        let binding = binding("privacy");
        let proposal_request = proposal_request(raw_proposal_key, raw_proposal_request, &binding);
        assert!(!format!("{proposal_request:?}").contains("PRIVATE-PROPOSAL"));
        let registered = journal.register_proposal_intent(proposal_request).unwrap();
        assert!(!format!("{registered:?}").contains("project-privacy"));
        let review = journal
            .commit_proposal_publication(registered.intent().proposal_intent_id(), &binding)
            .unwrap();
        let decision_request = decision_intent_request(raw_decision_key, raw_decision_request);
        assert!(!format!("{decision_request:?}").contains("PRIVATE-DECISION"));
        let intent = journal
            .register_decision_commit_intent(review.review().review_id(), decision_request)
            .unwrap();
        let intent_debug = format!("{intent:?}");
        assert!(!intent_debug.contains("commit:"));
        assert!(!intent_debug.contains("record:"));
        assert!(!intent_debug.contains(MANIFEST_SHA256));
        let outcome_request = decision_outcome(&binding);
        assert!(!format!("{outcome_request:?}").contains("commit:"));
        let committed = journal
            .commit_decision_outcome(review.review().review_id(), outcome_request)
            .unwrap();
        assert!(!format!("{committed:?}").contains("commit:"));
    }

    let bytes = fs::read(&temporary.database).unwrap();
    let forbidden: &[&[u8]] = &[
        raw_proposal_key,
        b"PRIVATE-PROPOSAL-CONTEXT-CANARY-A71C",
        raw_decision_key,
        b"PRIVATE-DECISION-RATIONALE-CANARY-B82D",
        b"credential",
        b"permit",
        b"actor_oid",
        b"policy_oid",
        b"grant_oid",
        b"repository_path",
        b"raw_idempotency_key",
        b"rationale",
    ];
    for canary in forbidden {
        assert!(
            !bytes.windows(canary.len()).any(|window| window == *canary),
            "database retained forbidden bytes: {}",
            String::from_utf8_lossy(canary)
        );
    }
}

#[test]
fn v1_database_migrates_transactionally_and_legacy_apis_remain_readable() {
    let temporary = TempDatabase::new("migration");
    let legacy_review_id = "1111111111111111111111111111111111111111111111111111111111111111";
    let legacy_binding = binding("legacy");
    {
        let connection = Connection::open(&temporary.database).unwrap();
        connection
            .execute_batch(
                "PRAGMA foreign_keys = ON;
                 CREATE TABLE reviews (
                    review_id TEXT PRIMARY KEY NOT NULL,
                    project_scope TEXT NOT NULL,
                    proposal_ref_name TEXT NOT NULL,
                    proposal_head TEXT NOT NULL,
                    decision_ref_name TEXT NOT NULL,
                    expected_decision_head TEXT NOT NULL,
                    state TEXT NOT NULL CHECK (state IN (
                        'pending_review', 'decision_committed', 'terminal_denial',
                        'retryable_failure', 'outcome_unknown'
                    )),
                    UNIQUE(project_scope, proposal_ref_name, proposal_head)
                 );
                 CREATE TABLE decision_intents (
                    review_id TEXT PRIMARY KEY NOT NULL
                        REFERENCES reviews(review_id) ON DELETE RESTRICT,
                    idempotency_digest TEXT NOT NULL,
                    request_fingerprint TEXT NOT NULL,
                    candidate_head TEXT NOT NULL,
                    feedback_oid TEXT NOT NULL,
                    expected_decision_head TEXT NOT NULL
                 );
                 CREATE UNIQUE INDEX decision_intents_idempotency
                    ON decision_intents(review_id, idempotency_digest);
                 PRAGMA user_version = 1;",
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO reviews(
                    review_id, project_scope, proposal_ref_name, proposal_head,
                    decision_ref_name, expected_decision_head, state
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'pending_review')",
                params![
                    legacy_review_id,
                    legacy_binding.project_scope(),
                    legacy_binding.proposal_ref_name(),
                    legacy_binding.proposal_head(),
                    legacy_binding.decision_ref_name(),
                    legacy_binding.expected_decision_head(),
                ],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO decision_intents(
                    review_id, idempotency_digest, request_fingerprint,
                    candidate_head, feedback_oid, expected_decision_head
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    legacy_review_id,
                    format!("sha256:{MANIFEST_SHA256}"),
                    format!("sha256:{OTHER_MANIFEST_SHA256}"),
                    NEW_DECISION_HEAD,
                    FEEDBACK_OID,
                    legacy_binding.expected_decision_head(),
                ],
            )
            .unwrap();
    }

    {
        let mut journal = SqliteReviewJournal::open(&temporary.database).unwrap();
        let review_id = synapse_artifact_journal::ReviewId::parse(legacy_review_id).unwrap();
        assert_eq!(
            journal.get_review(&review_id).unwrap().binding(),
            &legacy_binding
        );
        assert_eq!(
            journal
                .get_decision_intent(&review_id)
                .unwrap()
                .unwrap()
                .candidate_head(),
            NEW_DECISION_HEAD
        );
        assert!(
            journal
                .get_decision_commit_intent(&review_id)
                .unwrap()
                .is_none()
        );
        assert!(
            journal
                .get_review_artifact_manifest_sha256(&review_id)
                .unwrap()
                .is_none()
        );
        let legacy_upgrade = journal
            .register_decision_commit_intent(
                &review_id,
                decision_intent_request(
                    b"legacy-upgrade-key",
                    br#"{"disposition":"adopted_unchanged"}"#,
                ),
            )
            .unwrap_err();
        assert_eq!(legacy_upgrade.code(), "decision_intent_upgrade_required");

        let (new_binding, new_review_id) = create_published_review(&mut journal, "post-migration");
        journal
            .register_decision_commit_intent(
                &new_review_id,
                decision_intent_request(
                    b"post-migration-key",
                    br#"{"disposition":"adopted_unchanged"}"#,
                ),
            )
            .unwrap();
        journal
            .commit_decision_outcome(&new_review_id, decision_outcome(&new_binding))
            .unwrap();
    }

    let connection = Connection::open(&temporary.database).unwrap();
    let version: i64 = connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(version, 2);
    let extension_tables: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master
             WHERE type = 'table' AND name IN (
                'proposal_intents', 'decision_commit_intents', 'decision_outcomes'
             )",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(extension_tables, 3);
}

#[test]
fn invalid_semantic_pairs_and_non_advancing_heads_are_rejected_before_storage() {
    let mut journal = SqliteReviewJournal::open_in_memory().unwrap();
    let (_, review_id) = create_published_review(&mut journal, "validation");
    let invalid_semantics = DecisionCommitIntentRequest {
        disposition: DecisionDisposition::Rejected,
        selected_snapshot: SelectedSnapshot::Proposal,
        ..decision_intent_request(b"validation-key", b"{}")
    };
    assert_eq!(
        journal
            .register_decision_commit_intent(&review_id, invalid_semantics)
            .unwrap_err()
            .code(),
        "invalid_argument"
    );
    let non_advancing = DecisionCommitIntentRequest {
        new_decision_head: "commit:sg-oid-v1:sha256:base",
        ..decision_intent_request(b"validation-key", b"{}")
    };
    assert_eq!(
        journal
            .register_decision_commit_intent(&review_id, non_advancing)
            .unwrap_err()
            .code(),
        "invalid_argument"
    );
    assert!(
        journal
            .get_decision_commit_intent(&review_id)
            .unwrap()
            .is_none()
    );
}

#[test]
fn legacy_review_with_strict_intent_cannot_bypass_exact_outcome_commit() {
    let temporary = TempDatabase::new("legacy-strict-hybrid");
    let review_id;
    {
        let mut journal = SqliteReviewJournal::open(&temporary.database).unwrap();
        let review = journal
            .create_or_get_review(binding("legacy-strict-hybrid"))
            .unwrap();
        review_id = review.review().review_id().clone();
        journal
            .register_decision_commit_intent(
                &review_id,
                decision_intent_request(b"legacy-strict-key", b"{}"),
            )
            .unwrap();

        let bypass = journal
            .transition_review_state(
                &review_id,
                ReviewState::PendingReview,
                ReviewState::DecisionCommitted,
            )
            .unwrap_err();
        assert_eq!(bypass.code(), "review_state_transition_invalid");
    }

    // Simulate a database written by the formerly permissive hybrid path.
    let connection = Connection::open(&temporary.database).unwrap();
    connection
        .execute(
            "UPDATE reviews SET state = 'decision_committed' WHERE review_id = ?1",
            [review_id.as_str()],
        )
        .unwrap();
    drop(connection);

    let mut reopened = SqliteReviewJournal::open(&temporary.database).unwrap();
    let reconciliation = reopened.get_review_reconciliation(&review_id).unwrap_err();
    assert_eq!(reconciliation.code(), "journal_corrupt");
    let replay = reopened
        .transition_review_state(
            &review_id,
            ReviewState::DecisionCommitted,
            ReviewState::DecisionCommitted,
        )
        .unwrap_err();
    assert_eq!(replay.code(), "journal_corrupt");
}
