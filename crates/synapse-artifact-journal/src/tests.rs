use std::fs;
use std::io::ErrorKind;
use std::path::PathBuf;
use std::sync::{Arc, Barrier};
use std::thread;

use crate::journal::SqliteReviewJournal;
use crate::types::{
    DecisionIntentRequest, IntentRegistrationOutcome, RANDOM_ID_ATTEMPTS, REVIEW_ID_HEX_LEN,
    ReviewBinding, ReviewId, ReviewRegistrationOutcome, ReviewState,
};
use crate::validate::valid_sha256;

struct TempDirectory(PathBuf);

impl TempDirectory {
    fn new() -> Self {
        for _ in 0..RANDOM_ID_ATTEMPTS {
            let suffix = ReviewId::generate().unwrap();
            let path = std::env::temp_dir().join(format!(
                "synapsegit-artifact-journal-test-{}-{}",
                std::process::id(),
                suffix.as_str()
            ));
            match fs::create_dir(&path) {
                Ok(()) => return Self(path),
                Err(error) if error.kind() == ErrorKind::AlreadyExists => continue,
                Err(error) => panic!("failed to create test directory: {error}"),
            }
        }
        panic!("failed to allocate a collision-resistant test directory")
    }

    fn database(&self) -> PathBuf {
        self.0.join("review-journal.sqlite3")
    }
}

impl Drop for TempDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn binding(suffix: &str) -> ReviewBinding {
    ReviewBinding::new(
        format!("project-{suffix}"),
        format!("proposal/agent/{suffix}"),
        format!("commit:sg-oid-v1:sha256:proposal-{suffix}"),
        format!("decision/main/{suffix}"),
        "commit:sg-oid-v1:sha256:base",
    )
    .unwrap()
}

fn intent<'a>(key: &'a [u8], request: &'a [u8]) -> DecisionIntentRequest<'a> {
    DecisionIntentRequest {
        idempotency_key: key,
        canonical_request: request,
        candidate_head: "commit:sg-oid-v1:sha256:candidate",
        feedback_oid: "record:sg-oid-v1:sha256:feedback",
        expected_decision_head: "commit:sg-oid-v1:sha256:base",
    }
}

#[test]
fn random_ids_and_binding_uniqueness_are_enforced() {
    let mut journal = SqliteReviewJournal::open_in_memory().unwrap();
    let first_binding = binding("one");
    let first = journal.create_review(first_binding.clone()).unwrap();
    let second = journal.create_review(binding("two")).unwrap();
    assert_ne!(first.review_id(), second.review_id());
    assert_eq!(first.review_id().as_str().len(), REVIEW_ID_HEX_LEN);
    assert_eq!(format!("{:?}", first.review_id()), "ReviewId(<opaque>)");
    let error = journal.create_review(first_binding).unwrap_err();
    assert_eq!(error.code(), "review_binding_exists");
    let count: i64 = journal
        .connection
        .query_row("SELECT COUNT(*) FROM reviews", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 2);
}

#[test]
fn binding_ref_names_require_nonempty_namespaced_tails() {
    for (proposal, decision) in [
        ("proposal", "decision/main"),
        ("proposal/", "decision/main"),
        ("proposal/main", "decision"),
        ("proposal/main", "decision/"),
    ] {
        let error = ReviewBinding::new(
            "project",
            proposal,
            "commit:sg-oid-v1:sha256:proposal",
            decision,
            "commit:sg-oid-v1:sha256:base",
        )
        .unwrap_err();
        assert_eq!(error.code(), "invalid_argument");
    }
}

#[test]
fn create_or_get_recovers_the_same_locator_and_debug_redacts_bindings() {
    let mut journal = SqliteReviewJournal::open_in_memory().unwrap();
    let exact_binding = binding("response-loss-canary");
    let created = journal.create_or_get_review(exact_binding.clone()).unwrap();
    assert_eq!(created.outcome(), ReviewRegistrationOutcome::Created);
    let replayed = journal.create_or_get_review(exact_binding.clone()).unwrap();
    assert_eq!(replayed.outcome(), ReviewRegistrationOutcome::Replayed);
    assert_eq!(created.review().review_id(), replayed.review().review_id());
    assert_eq!(
        journal
            .get_review_by_binding(&exact_binding)
            .unwrap()
            .unwrap()
            .review_id(),
        created.review().review_id()
    );

    for debug in [
        format!("{exact_binding:?}"),
        format!("{:?}", replayed.review()),
    ] {
        assert!(!debug.contains("response-loss-canary"));
        assert!(!debug.contains("proposal/"));
        assert!(!debug.contains("decision/"));
        assert!(!debug.contains("commit:"));
    }

    let conflicting = ReviewBinding::new(
        exact_binding.project_scope(),
        exact_binding.proposal_ref_name(),
        exact_binding.proposal_head(),
        "decision/different",
        "commit:sg-oid-v1:sha256:different-base",
    )
    .unwrap();
    let error = journal.create_or_get_review(conflicting).unwrap_err();
    assert_eq!(error.code(), "review_binding_conflict");
    let count: i64 = journal
        .connection
        .query_row("SELECT COUNT(*) FROM reviews", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 1);
}

#[test]
fn concurrent_create_or_get_returns_one_durable_locator() {
    const WORKERS: usize = 6;
    let temporary = TempDirectory::new();
    let database = temporary.database();
    let barrier = Arc::new(Barrier::new(WORKERS));
    let handles = (0..WORKERS)
        .map(|_| {
            let database = database.clone();
            let barrier = Arc::clone(&barrier);
            thread::spawn(move || {
                let mut journal = SqliteReviewJournal::open(database).unwrap();
                barrier.wait();
                journal.create_or_get_review(binding("concurrent")).unwrap()
            })
        })
        .collect::<Vec<_>>();
    let results = handles
        .into_iter()
        .map(|handle| handle.join().unwrap())
        .collect::<Vec<_>>();

    let locator = results[0].review().review_id().clone();
    assert!(
        results
            .iter()
            .all(|result| result.review().review_id() == &locator)
    );
    assert_eq!(
        results
            .iter()
            .filter(|result| result.outcome() == ReviewRegistrationOutcome::Created)
            .count(),
        1
    );
}

#[test]
fn reopen_preserves_binding_intent_and_state() {
    let temporary = TempDirectory::new();
    let database = temporary.database();
    let review_id = {
        let mut journal = SqliteReviewJournal::open(&database).unwrap();
        let review = journal.create_review(binding("reopen")).unwrap();
        let review_id = review.review_id().clone();
        let registered = journal
            .register_decision_intent(
                &review_id,
                intent(b"reopen-key", br#"{"disposition":"adopt"}"#),
            )
            .unwrap();
        assert_eq!(registered.outcome(), IntentRegistrationOutcome::Created);
        journal
            .transition_review_state(
                &review_id,
                ReviewState::PendingReview,
                ReviewState::OutcomeUnknown,
            )
            .unwrap();
        review_id
    };

    let journal = SqliteReviewJournal::open(&database).unwrap();
    let review = journal.get_review(&review_id).unwrap();
    assert_eq!(review.binding(), &binding("reopen"));
    assert_eq!(review.state(), ReviewState::OutcomeUnknown);
    let stored = journal.get_decision_intent(&review_id).unwrap().unwrap();
    assert_eq!(stored.candidate_head(), "commit:sg-oid-v1:sha256:candidate");
    assert!(valid_sha256(stored.idempotency_digest()));
    assert!(valid_sha256(stored.request_fingerprint()));
}

#[test]
fn same_key_and_fingerprint_replay_without_a_second_intent() {
    let mut journal = SqliteReviewJournal::open_in_memory().unwrap();
    let review = journal.create_review(binding("replay")).unwrap();
    let request = br#"{"disposition":"reject"}"#;
    let first = journal
        .register_decision_intent(review.review_id(), intent(b"stable-key", request))
        .unwrap();
    let replay = journal
        .register_decision_intent(review.review_id(), intent(b"stable-key", request))
        .unwrap();
    assert_eq!(first.outcome(), IntentRegistrationOutcome::Created);
    assert_eq!(replay.outcome(), IntentRegistrationOutcome::Replayed);
    assert_eq!(first.intent(), replay.intent());
    let count: i64 = journal
        .connection
        .query_row("SELECT COUNT(*) FROM decision_intents", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(count, 1);
}

#[test]
fn same_key_with_changed_request_or_candidate_is_an_idempotency_conflict() {
    let mut journal = SqliteReviewJournal::open_in_memory().unwrap();
    let review = journal.create_review(binding("conflict")).unwrap();
    journal
        .register_decision_intent(
            review.review_id(),
            intent(b"same-key", br#"{"disposition":"adopt"}"#),
        )
        .unwrap();
    let changed_request = journal
        .register_decision_intent(
            review.review_id(),
            intent(b"same-key", br#"{"disposition":"reject"}"#),
        )
        .unwrap_err();
    assert_eq!(changed_request.code(), "idempotency_conflict");

    let changed_candidate = DecisionIntentRequest {
        candidate_head: "commit:sg-oid-v1:sha256:different",
        ..intent(b"same-key", br#"{"disposition":"adopt"}"#)
    };
    let error = journal
        .register_decision_intent(review.review_id(), changed_candidate)
        .unwrap_err();
    assert_eq!(error.code(), "idempotency_conflict");
}

#[test]
fn privacy_sensitive_inputs_are_hashed_or_absent_from_the_database() {
    let temporary = TempDirectory::new();
    let database = temporary.database();
    let raw_key = b"RAW-IDEMPOTENCY-KEY-CANARY-91D4";
    let canonical_request =
        br#"{"disposition":"defer","rationale":"PRIVATE-RATIONALE-CANARY-7A2E"}"#;
    let request_debug = format!("{:?}", intent(raw_key, canonical_request));
    assert!(!request_debug.contains("RAW-IDEMPOTENCY"));
    assert!(!request_debug.contains("PRIVATE-RATIONALE"));
    {
        let mut journal = SqliteReviewJournal::open(&database).unwrap();
        let review = journal.create_review(binding("privacy")).unwrap();
        let registered = journal
            .register_decision_intent(review.review_id(), intent(raw_key, canonical_request))
            .unwrap();
        let intent_debug = format!("{registered:?}");
        assert!(!intent_debug.contains("candidate"));
        assert!(!intent_debug.contains("feedback"));
        assert!(!intent_debug.contains("sha256:"));
    }

    let bytes = fs::read(&database).unwrap();
    let forbidden = [
        raw_key.as_slice(),
        b"PRIVATE-RATIONALE-CANARY-7A2E".as_slice(),
        b"credential".as_slice(),
        b"permit".as_slice(),
        b"actor_oid".as_slice(),
        b"policy_oid".as_slice(),
        b"grant_oid".as_slice(),
        b"repository_path".as_slice(),
        b"raw_idempotency_key".as_slice(),
        b"rationale".as_slice(),
    ];
    for canary in forbidden {
        assert!(
            !bytes.windows(canary.len()).any(|window| window == canary),
            "database retained forbidden bytes: {}",
            String::from_utf8_lossy(canary)
        );
    }
}

#[test]
fn state_transition_is_compare_and_set_and_all_states_round_trip() {
    let mut journal = SqliteReviewJournal::open_in_memory().unwrap();
    for (index, state) in [
        ReviewState::DecisionCommitted,
        ReviewState::TerminalDenial,
        ReviewState::RetryableFailure,
        ReviewState::OutcomeUnknown,
    ]
    .into_iter()
    .enumerate()
    {
        let review = journal
            .create_review(binding(&format!("state-{index}")))
            .unwrap();
        let changed = journal
            .transition_review_state(review.review_id(), ReviewState::PendingReview, state)
            .unwrap();
        assert_eq!(changed.state(), state);
        let idempotent = journal
            .transition_review_state(review.review_id(), state, state)
            .unwrap();
        assert_eq!(idempotent, changed);
        let error = journal
            .transition_review_state(review.review_id(), ReviewState::PendingReview, state)
            .unwrap_err();
        assert_eq!(error.code(), "review_state_conflict");
    }
}

#[test]
fn terminal_and_unknown_states_cannot_regress_to_pending() {
    let mut journal = SqliteReviewJournal::open_in_memory().unwrap();
    for (index, terminal) in [ReviewState::DecisionCommitted, ReviewState::TerminalDenial]
        .into_iter()
        .enumerate()
    {
        let review = journal
            .create_review(binding(&format!("terminal-{index}")))
            .unwrap();
        journal
            .transition_review_state(review.review_id(), ReviewState::PendingReview, terminal)
            .unwrap();
        let error = journal
            .transition_review_state(review.review_id(), terminal, ReviewState::PendingReview)
            .unwrap_err();
        assert_eq!(error.code(), "review_state_transition_invalid");
        assert_eq!(
            journal.get_review(review.review_id()).unwrap().state(),
            terminal
        );
    }

    let unknown = journal.create_review(binding("unknown-state")).unwrap();
    journal
        .transition_review_state(
            unknown.review_id(),
            ReviewState::PendingReview,
            ReviewState::OutcomeUnknown,
        )
        .unwrap();
    for regressed in [ReviewState::PendingReview, ReviewState::RetryableFailure] {
        let error = journal
            .transition_review_state(unknown.review_id(), ReviewState::OutcomeUnknown, regressed)
            .unwrap_err();
        assert_eq!(error.code(), "review_state_transition_invalid");
    }
    let reconciled = journal
        .transition_review_state(
            unknown.review_id(),
            ReviewState::OutcomeUnknown,
            ReviewState::DecisionCommitted,
        )
        .unwrap();
    assert_eq!(reconciled.state(), ReviewState::DecisionCommitted);

    let retryable = journal.create_review(binding("retryable-state")).unwrap();
    journal
        .transition_review_state(
            retryable.review_id(),
            ReviewState::PendingReview,
            ReviewState::RetryableFailure,
        )
        .unwrap();
    let error = journal
        .transition_review_state(
            retryable.review_id(),
            ReviewState::RetryableFailure,
            ReviewState::PendingReview,
        )
        .unwrap_err();
    assert_eq!(error.code(), "review_state_transition_invalid");
}

#[test]
fn a_new_intent_requires_a_decisionable_state_and_the_bound_base() {
    let mut journal = SqliteReviewJournal::open_in_memory().unwrap();
    let terminal = journal.create_review(binding("terminal-intent")).unwrap();
    journal
        .transition_review_state(
            terminal.review_id(),
            ReviewState::PendingReview,
            ReviewState::TerminalDenial,
        )
        .unwrap();
    let error = journal
        .register_decision_intent(
            terminal.review_id(),
            intent(b"terminal-key", br#"{"disposition":"reject"}"#),
        )
        .unwrap_err();
    assert_eq!(error.code(), "review_state_conflict");

    let pending = journal.create_review(binding("bound-intent")).unwrap();
    let wrong_base = DecisionIntentRequest {
        expected_decision_head: "commit:sg-oid-v1:sha256:wrong-base",
        ..intent(b"bound-key", br#"{"disposition":"adopt"}"#)
    };
    let error = journal
        .register_decision_intent(pending.review_id(), wrong_base)
        .unwrap_err();
    assert_eq!(error.code(), "review_binding_conflict");
    assert!(
        journal
            .get_decision_intent(pending.review_id())
            .unwrap()
            .is_none()
    );
}

#[test]
fn an_existing_exact_intent_can_be_recovered_after_terminal_commit() {
    let mut journal = SqliteReviewJournal::open_in_memory().unwrap();
    let review = journal.create_review(binding("terminal-replay")).unwrap();
    let request = intent(b"replay-key", br#"{"disposition":"adopt"}"#);
    journal
        .register_decision_intent(review.review_id(), request)
        .unwrap();
    journal
        .transition_review_state(
            review.review_id(),
            ReviewState::PendingReview,
            ReviewState::DecisionCommitted,
        )
        .unwrap();

    let replay = journal
        .register_decision_intent(review.review_id(), request)
        .unwrap();
    assert_eq!(replay.outcome(), IntentRegistrationOutcome::Replayed);
}
