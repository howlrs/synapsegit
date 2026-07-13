use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Barrier, Mutex};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rusqlite::Connection;
use synapse_sqlite::{
    RefArchive, RefArchiveExportLimits, RefPrecondition, RefSnapshot, RefStoreError, RefUpdate,
    ReflogEntry, ReflogMetadata, SqliteRefStore, ValidationError, validate_commit_oid,
    validate_ref_name,
};

static NEXT_TEMP_DIRECTORY: AtomicU64 = AtomicU64::new(0);

struct TestDirectory {
    path: PathBuf,
}

impl TestDirectory {
    fn new(label: &str) -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock must be after the Unix epoch")
            .as_nanos();
        let sequence = NEXT_TEMP_DIRECTORY.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "synapse-sqlite-{label}-{}-{nonce}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&path)
            .unwrap_or_else(|error| panic!("create test directory {}: {error}", path.display()));
        Self { path }
    }

    fn database_path(&self) -> PathBuf {
        self.path.join("refs.sqlite3")
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        if let Err(error) = fs::remove_dir_all(&self.path) {
            if self.path.exists() {
                eprintln!(
                    "failed to remove test directory {}: {error}",
                    self.path.display()
                );
            }
        }
    }
}

fn commit_oid(digit: char) -> String {
    assert!(digit.is_ascii_hexdigit() && !digit.is_ascii_uppercase());
    format!("commit:sg-oid-v1:sha256:{}", digit.to_string().repeat(64))
}

fn allow_all(_: &str) -> std::result::Result<(), ValidationError> {
    Ok(())
}

fn update<'a>(
    ref_name: &'a str,
    expected_head: Option<&'a str>,
    new_head: &'a str,
    occurred_at_unix_nanos: i64,
) -> RefUpdate<'a> {
    RefUpdate {
        ref_name,
        expected_head,
        new_head,
        metadata: ReflogMetadata::at(occurred_at_unix_nanos),
    }
}

fn create_sample_archive() -> RefArchive {
    let head_a = commit_oid('a');
    let head_b = commit_oid('b');
    let head_c = commit_oid('c');
    let mut store = SqliteRefStore::open_in_memory().unwrap();

    store
        .compare_and_swap(
            RefUpdate {
                ref_name: "proposal/archive",
                expected_head: None,
                new_head: &head_a,
                metadata: ReflogMetadata {
                    occurred_at_unix_nanos: 10,
                    actor: Some("archiver"),
                    message: Some("create proposal"),
                },
            },
            &allow_all,
        )
        .unwrap();
    store
        .compare_and_swap(update("release/stable", None, &head_b, 20), &allow_all)
        .unwrap();
    store
        .compare_and_swap(
            update("proposal/archive", Some(&head_a), &head_c, 30),
            &allow_all,
        )
        .unwrap();

    store.export_archive().unwrap()
}

fn assert_archive_invalid(archive: &RefArchive) {
    let validation_calls = AtomicUsize::new(0);
    let validator = |_: &str| {
        validation_calls.fetch_add(1, Ordering::SeqCst);
        Ok(())
    };
    let mut store = SqliteRefStore::open_in_memory().unwrap();
    let error = store
        .restore_archive(archive, &validator)
        .expect_err("invalid archive must be rejected");

    assert!(
        matches!(error, RefStoreError::ArchiveInvalid { .. }),
        "unexpected error: {error}"
    );
    assert_eq!(error.code(), "archive_invalid");
    assert_eq!(validation_calls.load(Ordering::SeqCst), 0);
    assert!(store.snapshot().unwrap().is_empty());
    assert!(store.reflog().unwrap().is_empty());
}

#[test]
fn validates_exact_ref_name_and_commit_oid_profiles() {
    for name in [
        "proposal/a",
        "decision/main",
        "release/2026.07",
        "observed/camera_1:raw",
        "material-events/site-1/event.2",
    ] {
        validate_ref_name(name).unwrap_or_else(|error| panic!("{name}: {error}"));
    }

    let segment_128 = format!("a{}", "z".repeat(127));
    validate_ref_name(&format!("proposal/{segment_128}")).unwrap();
    let exactly_500 = format!(
        "proposal/{}/{}/{}/{}",
        "a".repeat(128),
        "b".repeat(128),
        "c".repeat(128),
        "d".repeat(104)
    );
    assert_eq!(exactly_500.len(), 500);
    validate_ref_name(&exactly_500).unwrap();

    let mut invalid_names = vec![
        "".to_owned(),
        "proposal".to_owned(),
        "unknown/main".to_owned(),
        "Proposal/main".to_owned(),
        "proposal/".to_owned(),
        "proposal//main".to_owned(),
        "proposal/-main".to_owned(),
        "proposal/_main".to_owned(),
        "proposal/Main".to_owned(),
        "proposal/main@remote".to_owned(),
        "proposal/naïve".to_owned(),
        format!("proposal/a{}", "z".repeat(128)),
    ];
    invalid_names.push(format!("{exactly_500}x"));
    for name in invalid_names {
        let error = validate_ref_name(&name).expect_err("invalid Ref name must fail");
        assert!(matches!(error, RefStoreError::InvalidRefName { .. }));
        assert_eq!(error.code(), "path_segment_invalid");
    }

    validate_commit_oid(&commit_oid('0')).unwrap();
    validate_commit_oid(&commit_oid('f')).unwrap();
    for oid in [
        format!("blob:sg-oid-v1:sha256:{}", "a".repeat(64)),
        format!("commit:sg-oid-v1:sha256:{}", "a".repeat(63)),
        format!("commit:sg-oid-v1:sha256:{}", "a".repeat(65)),
        format!("commit:sg-oid-v1:sha256:{}", "A".repeat(64)),
        format!("commit:sg-oid-v1:sha256:{}g", "a".repeat(63)),
        format!("commit:sg-oid-v1:sha256:{}\n", "a".repeat(64)),
    ] {
        let error = validate_commit_oid(&oid).expect_err("invalid Commit OID must fail");
        assert!(matches!(error, RefStoreError::InvalidCommitOid { .. }));
        assert_eq!(error.code(), "oid_mismatch");
    }
}

#[test]
fn compare_and_swap_rejects_lexically_invalid_inputs_without_validation_or_mutation() {
    let valid_head = commit_oid('a');
    let invalid_head = format!("commit:sg-oid-v1:sha256:{}", "A".repeat(64));
    let validation_calls = AtomicUsize::new(0);
    let validator = |_: &str| {
        validation_calls.fetch_add(1, Ordering::SeqCst);
        Ok(())
    };
    let mut store = SqliteRefStore::open_in_memory().unwrap();

    let invalid_name = store
        .compare_and_swap(update("proposal/Upper", None, &valid_head, 1), &validator)
        .expect_err("invalid Ref name must be rejected");
    assert!(matches!(invalid_name, RefStoreError::InvalidRefName { .. }));

    let invalid_expected = store
        .compare_and_swap(
            update("proposal/valid", Some(&invalid_head), &valid_head, 2),
            &validator,
        )
        .expect_err("invalid expected Commit OID must be rejected");
    assert!(matches!(
        invalid_expected,
        RefStoreError::InvalidCommitOid { .. }
    ));

    let invalid_new = store
        .compare_and_swap(update("proposal/valid", None, &invalid_head, 3), &validator)
        .expect_err("invalid new Commit OID must be rejected");
    assert!(matches!(
        invalid_new,
        RefStoreError::InvalidCommitOid { .. }
    ));

    assert_eq!(validation_calls.load(Ordering::SeqCst), 0);
    assert!(store.snapshot().unwrap().is_empty());
    assert!(store.reflog().unwrap().is_empty());
}

#[test]
fn additional_preconditions_reject_invalid_names_and_oids_before_target_validation() {
    let current_head = commit_oid('a');
    let next_head = commit_oid('b');
    let invalid_head = format!("commit:sg-oid-v1:sha256:{}", "A".repeat(64));
    let mut store = SqliteRefStore::open_in_memory().unwrap();
    store
        .compare_and_swap(
            update("proposal/precondition-input", None, &current_head, 1),
            &allow_all,
        )
        .unwrap();
    let snapshot_before = store.snapshot().unwrap();
    let reflog_before = store.reflog().unwrap();
    let validation_calls = AtomicUsize::new(0);
    let validator = |_: &str| {
        validation_calls.fetch_add(1, Ordering::SeqCst);
        Ok(())
    };

    let invalid_name = store
        .compare_and_swap_with_preconditions(
            update(
                "proposal/precondition-input",
                Some(&current_head),
                &next_head,
                2,
            ),
            &[RefPrecondition {
                ref_name: "decision/Upper",
                expected_head: None,
            }],
            &validator,
        )
        .expect_err("an invalid precondition Ref name must be rejected");
    assert!(matches!(invalid_name, RefStoreError::InvalidRefName { .. }));

    let invalid_expected_head = store
        .compare_and_swap_with_preconditions(
            update(
                "proposal/precondition-input",
                Some(&current_head),
                &next_head,
                3,
            ),
            &[RefPrecondition {
                ref_name: "decision/valid",
                expected_head: Some(&invalid_head),
            }],
            &validator,
        )
        .expect_err("an invalid precondition Commit OID must be rejected");
    assert!(matches!(
        invalid_expected_head,
        RefStoreError::InvalidCommitOid { .. }
    ));

    assert_eq!(validation_calls.load(Ordering::SeqCst), 0);
    assert_eq!(store.snapshot().unwrap(), snapshot_before);
    assert_eq!(store.reflog().unwrap(), reflog_before);
}

#[test]
fn failed_additional_precondition_is_structured_and_leaves_state_unchanged() {
    let dependency_head = commit_oid('a');
    let target_head = commit_oid('b');
    let stale_dependency_head = commit_oid('c');
    let proposed_head = commit_oid('d');
    let mut store = SqliteRefStore::open_in_memory().unwrap();
    store
        .compare_and_swap(
            update("decision/dependency", None, &dependency_head, 1),
            &allow_all,
        )
        .unwrap();
    store
        .compare_and_swap(
            update("proposal/guarded", None, &target_head, 2),
            &allow_all,
        )
        .unwrap();
    let snapshot_before = store.snapshot().unwrap();
    let reflog_before = store.reflog().unwrap();

    let error = store
        .compare_and_swap_with_preconditions(
            update("proposal/guarded", Some(&target_head), &proposed_head, 3),
            &[RefPrecondition {
                ref_name: "decision/dependency",
                expected_head: Some(&stale_dependency_head),
            }],
            &allow_all,
        )
        .expect_err("a stale additional Ref precondition must fail");
    assert_eq!(error.code(), "ref_conflict");
    match error {
        RefStoreError::PreconditionFailed {
            ref_name,
            expected_head,
            actual_head,
        } => {
            assert_eq!(ref_name, "decision/dependency");
            assert_eq!(
                expected_head.as_deref(),
                Some(stale_dependency_head.as_str())
            );
            assert_eq!(actual_head.as_deref(), Some(dependency_head.as_str()));
        }
        other => panic!("unexpected error: {other}"),
    }

    assert_eq!(store.snapshot().unwrap(), snapshot_before);
    assert_eq!(store.reflog().unwrap(), reflog_before);
}

#[test]
fn matching_additional_preconditions_allow_the_requested_update() {
    let dependency_head = commit_oid('a');
    let target_head = commit_oid('b');
    let proposed_head = commit_oid('c');
    let mut store = SqliteRefStore::open_in_memory().unwrap();
    store
        .compare_and_swap(
            update("decision/dependency", None, &dependency_head, 1),
            &allow_all,
        )
        .unwrap();
    store
        .compare_and_swap(
            update("proposal/guarded", None, &target_head, 2),
            &allow_all,
        )
        .unwrap();

    let entry = store
        .compare_and_swap_with_preconditions(
            update("proposal/guarded", Some(&target_head), &proposed_head, 3),
            &[
                RefPrecondition {
                    ref_name: "decision/dependency",
                    expected_head: Some(&dependency_head),
                },
                RefPrecondition {
                    ref_name: "observed/must-not-exist",
                    expected_head: None,
                },
            ],
            &allow_all,
        )
        .unwrap();

    assert_eq!(entry.old_head.as_deref(), Some(target_head.as_str()));
    assert_eq!(entry.new_head, proposed_head);
    assert_eq!(
        store.get("decision/dependency").unwrap().unwrap().head,
        dependency_head
    );
    assert!(store.get("observed/must-not-exist").unwrap().is_none());
    assert_eq!(
        store.get("proposal/guarded").unwrap().unwrap().head,
        proposed_head
    );
    assert_eq!(
        store.reflog_for_ref("decision/dependency").unwrap().len(),
        1
    );
    assert_eq!(store.reflog_for_ref("proposal/guarded").unwrap().len(), 2);
}

#[test]
fn transaction_guard_runs_after_target_validation_and_before_ref_preconditions() {
    let dependency_head = commit_oid('a');
    let stale_dependency_head = commit_oid('b');
    let target_head = commit_oid('c');
    let proposed_head = commit_oid('d');
    let mut store = SqliteRefStore::open_in_memory().unwrap();
    store
        .compare_and_swap(
            update("decision/guard-order", None, &dependency_head, 1),
            &allow_all,
        )
        .unwrap();
    store
        .compare_and_swap(
            update("proposal/guard-order", None, &target_head, 2),
            &allow_all,
        )
        .unwrap();
    let snapshot_before = store.snapshot().unwrap();
    let reflog_before = store.reflog().unwrap();
    let call_order = Mutex::new(Vec::new());
    let validator = |_: &str| {
        call_order.lock().unwrap().push("target-validator");
        Ok(())
    };
    let guard = || {
        call_order.lock().unwrap().push("transaction-guard");
        Err(ValidationError::new(
            "authorization_denied",
            "capability expired while waiting for the SQLite writer lock",
        ))
    };

    let error = store
        .compare_and_swap_with_preconditions_and_guard(
            update(
                "proposal/guard-order",
                Some(&target_head),
                &proposed_head,
                3,
            ),
            &[RefPrecondition {
                ref_name: "decision/guard-order",
                expected_head: Some(&stale_dependency_head),
            }],
            &validator,
            &guard,
        )
        .expect_err("transaction guard must reject before the stale precondition is read");

    match error {
        RefStoreError::Validation(error) => {
            assert_eq!(error.code(), "authorization_denied");
            assert!(error.message().contains("writer lock"));
        }
        other => panic!("unexpected error: {other}"),
    }
    assert_eq!(
        call_order.into_inner().unwrap(),
        vec!["target-validator", "transaction-guard"]
    );
    assert_eq!(store.snapshot().unwrap(), snapshot_before);
    assert_eq!(store.reflog().unwrap(), reflog_before);
}

#[test]
fn transaction_guard_runs_while_the_immediate_writer_lock_is_held() {
    let directory = TestDirectory::new("transaction-guard-lock");
    let database = directory.database_path();
    let current_head = commit_oid('a');
    let next_head = commit_oid('b');
    let mut store = SqliteRefStore::open(&database).unwrap();
    store
        .compare_and_swap(
            update("proposal/guard-lock", None, &current_head, 1),
            &allow_all,
        )
        .unwrap();
    let guard_calls = AtomicUsize::new(0);
    let guard = || {
        guard_calls.fetch_add(1, Ordering::SeqCst);
        let mut contender = rusqlite::Connection::open(&database).unwrap();
        contender.busy_timeout(Duration::ZERO).unwrap();
        let lock_error =
            match contender.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate) {
                Ok(_) => panic!("another immediate writer must be excluded while the guard runs"),
                Err(error) => error,
            };
        assert_eq!(
            lock_error.sqlite_error_code(),
            Some(rusqlite::ErrorCode::DatabaseBusy)
        );
        Ok(())
    };

    let entry = store
        .compare_and_swap_with_preconditions_and_guard(
            update("proposal/guard-lock", Some(&current_head), &next_head, 2),
            &[],
            &allow_all,
            &guard,
        )
        .unwrap();

    assert_eq!(guard_calls.load(Ordering::SeqCst), 1);
    assert_eq!(entry.old_head.as_deref(), Some(current_head.as_str()));
    assert_eq!(entry.new_head, next_head);
    assert_eq!(store.reflog().unwrap().len(), 2);
}

#[test]
fn create_and_update_are_compare_and_swap_transactions() {
    let head_a = commit_oid('a');
    let head_b = commit_oid('b');
    let mut store = SqliteRefStore::open_in_memory().unwrap();

    let created = store
        .compare_and_swap(
            RefUpdate {
                ref_name: "decision/main",
                expected_head: None,
                new_head: &head_a,
                metadata: ReflogMetadata {
                    occurred_at_unix_nanos: 101,
                    actor: Some("human:alice"),
                    message: Some("initialize decision history"),
                },
            },
            &allow_all,
        )
        .unwrap();
    assert_eq!(created.id, 1);
    assert_eq!(created.old_head, None);
    assert_eq!(created.new_head, head_a);
    assert_eq!(created.actor.as_deref(), Some("human:alice"));
    assert_eq!(
        created.message.as_deref(),
        Some("initialize decision history")
    );

    let record = store.get("decision/main").unwrap().unwrap();
    assert_eq!(record.head, head_a);
    assert_eq!(record.updated_event_id, created.id);

    let advanced = store
        .compare_and_swap(
            update("decision/main", Some(&head_a), &head_b, 202),
            &allow_all,
        )
        .unwrap();
    assert_eq!(advanced.id, 2);
    assert_eq!(advanced.old_head.as_deref(), Some(head_a.as_str()));
    assert_eq!(advanced.new_head, head_b);

    let record = store.get("decision/main").unwrap().unwrap();
    assert_eq!(record.head, head_b);
    assert_eq!(record.updated_event_id, advanced.id);
    assert_eq!(
        store.reflog_for_ref("decision/main").unwrap(),
        vec![created, advanced]
    );
}

#[test]
fn stale_conflict_leaves_head_snapshot_and_reflog_unchanged() {
    let head_a = commit_oid('a');
    let head_b = commit_oid('b');
    let stale_proposal = commit_oid('c');
    let mut store = SqliteRefStore::open_in_memory().unwrap();
    store
        .compare_and_swap(update("proposal/stale", None, &head_a, 1), &allow_all)
        .unwrap();
    store
        .compare_and_swap(
            update("proposal/stale", Some(&head_a), &head_b, 2),
            &allow_all,
        )
        .unwrap();
    let snapshot_before = store.snapshot().unwrap();
    let reflog_before = store.reflog().unwrap();

    let error = store
        .compare_and_swap(
            update("proposal/stale", Some(&head_a), &stale_proposal, 3),
            &allow_all,
        )
        .expect_err("a stale expected head must conflict");
    match error {
        RefStoreError::RefConflict {
            ref_name,
            expected_head,
            actual_head,
        } => {
            assert_eq!(ref_name, "proposal/stale");
            assert_eq!(expected_head.as_deref(), Some(head_a.as_str()));
            assert_eq!(actual_head.as_deref(), Some(head_b.as_str()));
        }
        other => panic!("unexpected error: {other}"),
    }

    assert_eq!(store.snapshot().unwrap(), snapshot_before);
    assert_eq!(store.reflog().unwrap(), reflog_before);
    assert_eq!(store.get("proposal/stale").unwrap().unwrap().head, head_b);
}

#[test]
fn target_validator_failure_leaves_existing_state_unchanged() {
    let head_a = commit_oid('a');
    let rejected_head = commit_oid('e');
    let mut store = SqliteRefStore::open_in_memory().unwrap();
    store
        .compare_and_swap(update("observed/site-1", None, &head_a, 1), &allow_all)
        .unwrap();
    let snapshot_before = store.snapshot().unwrap();
    let reflog_before = store.reflog().unwrap();
    let validation_calls = AtomicUsize::new(0);
    let reject_missing_closure = |head: &str| {
        validation_calls.fetch_add(1, Ordering::SeqCst);
        Err(ValidationError::new(
            "closure_missing",
            format!("required object below {head} is unavailable"),
        ))
    };

    let error = store
        .compare_and_swap(
            update("observed/site-1", Some(&head_a), &rejected_head, 2),
            &reject_missing_closure,
        )
        .expect_err("closure validation must reject the update");
    match error {
        RefStoreError::Validation(error) => {
            assert_eq!(error.code(), "closure_missing");
            assert!(error.message().contains(&rejected_head));
        }
        other => panic!("unexpected error: {other}"),
    }

    assert_eq!(validation_calls.load(Ordering::SeqCst), 1);
    assert_eq!(store.snapshot().unwrap(), snapshot_before);
    assert_eq!(store.reflog().unwrap(), reflog_before);
}

#[test]
fn ref_state_and_reflog_persist_across_reopen() {
    let directory = TestDirectory::new("reopen");
    let database = directory.database_path();
    let head_a = commit_oid('a');
    let head_b = commit_oid('b');
    let expected_snapshot;
    let expected_reflog;

    {
        let mut store = SqliteRefStore::open(&database).unwrap();
        store
            .compare_and_swap(update("release/persistent", None, &head_a, 100), &allow_all)
            .unwrap();
        store
            .compare_and_swap(
                update("release/persistent", Some(&head_a), &head_b, 200),
                &allow_all,
            )
            .unwrap();
        expected_snapshot = store.snapshot().unwrap();
        expected_reflog = store.reflog().unwrap();
    }

    let reopened = SqliteRefStore::open(&database).unwrap();
    assert_eq!(reopened.snapshot().unwrap(), expected_snapshot);
    assert_eq!(reopened.reflog().unwrap(), expected_reflog);
    assert_eq!(
        reopened.get("release/persistent").unwrap().unwrap().head,
        head_b
    );
}

#[test]
fn concurrent_connections_racing_the_same_expected_head_have_one_winner() {
    let directory = TestDirectory::new("race");
    let database = directory.database_path();
    let original_head = commit_oid('1');
    let contender_a = commit_oid('a');
    let contender_b = commit_oid('b');

    {
        let mut store = SqliteRefStore::open(&database).unwrap();
        store
            .compare_and_swap(update("decision/race", None, &original_head, 1), &allow_all)
            .unwrap();
    }

    let barrier = Arc::new(Barrier::new(3));
    let mut handles = Vec::new();
    for (timestamp, proposed_head) in [(2, contender_a.clone()), (3, contender_b.clone())] {
        let database = database.clone();
        let barrier = Arc::clone(&barrier);
        let expected_head = original_head.clone();
        handles.push(thread::spawn(move || {
            let mut store = SqliteRefStore::open(database).unwrap();
            barrier.wait();
            let result = store.compare_and_swap(
                update(
                    "decision/race",
                    Some(&expected_head),
                    &proposed_head,
                    timestamp,
                ),
                &allow_all,
            );
            match result {
                Ok(entry) => (proposed_head, true, entry.new_head),
                Err(RefStoreError::RefConflict { actual_head, .. }) => (
                    proposed_head,
                    false,
                    actual_head.expect("the winning head must be visible to the loser"),
                ),
                Err(error) => panic!("unexpected racing update error: {error}"),
            }
        }));
    }

    barrier.wait();
    let outcomes: Vec<_> = handles
        .into_iter()
        .map(|handle| handle.join().expect("writer thread must not panic"))
        .collect();
    let winners: Vec<_> = outcomes
        .iter()
        .filter(|(_, won, _)| *won)
        .map(|(proposed, _, _)| proposed.clone())
        .collect();
    assert_eq!(winners.len(), 1, "outcomes: {outcomes:?}");
    let winner = &winners[0];
    let loser_observation = outcomes
        .iter()
        .find(|(_, won, _)| !won)
        .expect("exactly one contender must lose");
    assert_eq!(&loser_observation.2, winner);

    let store = SqliteRefStore::open(&database).unwrap();
    assert_eq!(store.get("decision/race").unwrap().unwrap().head, *winner);
    let reflog = store.reflog_for_ref("decision/race").unwrap();
    assert_eq!(reflog.len(), 2);
    assert_eq!(reflog[0].new_head, original_head);
    assert_eq!(reflog[1].old_head.as_deref(), Some(original_head.as_str()));
    assert_eq!(reflog[1].new_head, *winner);
}

#[test]
fn racing_precondition_change_and_guarded_update_are_serializable() {
    #[derive(Debug)]
    enum GuardedOutcome {
        Updated,
        PreconditionFailed {
            ref_name: String,
            expected_head: Option<String>,
            actual_head: Option<String>,
        },
    }

    let directory = TestDirectory::new("precondition-race");
    let database = directory.database_path();
    let dependency_head = commit_oid('a');
    let next_dependency_head = commit_oid('b');
    let target_head = commit_oid('c');
    let proposed_head = commit_oid('d');

    {
        let mut store = SqliteRefStore::open(&database).unwrap();
        store
            .compare_and_swap(
                update("decision/dependency-race", None, &dependency_head, 1),
                &allow_all,
            )
            .unwrap();
        store
            .compare_and_swap(
                update("proposal/guarded-race", None, &target_head, 2),
                &allow_all,
            )
            .unwrap();
    }

    let barrier = Arc::new(Barrier::new(3));

    let dependency_writer = {
        let database = database.clone();
        let barrier = Arc::clone(&barrier);
        let dependency_head = dependency_head.clone();
        let next_dependency_head = next_dependency_head.clone();
        thread::spawn(move || {
            let mut store = SqliteRefStore::open(database).unwrap();
            barrier.wait();
            store
                .compare_and_swap(
                    update(
                        "decision/dependency-race",
                        Some(&dependency_head),
                        &next_dependency_head,
                        3,
                    ),
                    &allow_all,
                )
                .unwrap();
        })
    };

    let guarded_writer = {
        let database = database.clone();
        let barrier = Arc::clone(&barrier);
        let dependency_head = dependency_head.clone();
        let target_head = target_head.clone();
        let proposed_head = proposed_head.clone();
        thread::spawn(move || {
            let mut store = SqliteRefStore::open(database).unwrap();
            barrier.wait();
            match store.compare_and_swap_with_preconditions(
                update(
                    "proposal/guarded-race",
                    Some(&target_head),
                    &proposed_head,
                    4,
                ),
                &[RefPrecondition {
                    ref_name: "decision/dependency-race",
                    expected_head: Some(&dependency_head),
                }],
                &allow_all,
            ) {
                Ok(_) => GuardedOutcome::Updated,
                Err(RefStoreError::PreconditionFailed {
                    ref_name,
                    expected_head,
                    actual_head,
                }) => GuardedOutcome::PreconditionFailed {
                    ref_name,
                    expected_head,
                    actual_head,
                },
                Err(error) => panic!("unexpected guarded update error: {error}"),
            }
        })
    };

    barrier.wait();
    dependency_writer
        .join()
        .expect("dependency writer must not panic");
    let guarded_outcome = guarded_writer
        .join()
        .expect("guarded writer must not panic");

    let store = SqliteRefStore::open(&database).unwrap();
    assert_eq!(
        store.get("decision/dependency-race").unwrap().unwrap().head,
        next_dependency_head
    );
    match guarded_outcome {
        GuardedOutcome::Updated => {
            assert_eq!(
                store.get("proposal/guarded-race").unwrap().unwrap().head,
                proposed_head
            );
            assert_eq!(store.reflog().unwrap().len(), 4);
        }
        GuardedOutcome::PreconditionFailed {
            ref_name,
            expected_head,
            actual_head,
        } => {
            assert_eq!(ref_name, "decision/dependency-race");
            assert_eq!(expected_head.as_deref(), Some(dependency_head.as_str()));
            assert_eq!(actual_head.as_deref(), Some(next_dependency_head.as_str()));
            assert_eq!(
                store.get("proposal/guarded-race").unwrap().unwrap().head,
                target_head
            );
            assert_eq!(store.reflog().unwrap().len(), 3);
        }
    }
}

#[test]
fn snapshot_and_reflog_orders_are_deterministic() {
    let head_a = commit_oid('a');
    let head_b = commit_oid('b');
    let head_c = commit_oid('c');
    let head_d = commit_oid('d');
    let head_e = commit_oid('e');
    let mut store = SqliteRefStore::open_in_memory().unwrap();

    for (name, head, timestamp) in [
        ("release/z", &head_a, 10),
        ("proposal/b", &head_b, 20),
        ("decision/main", &head_c, 30),
        ("proposal/a", &head_d, 40),
    ] {
        store
            .compare_and_swap(update(name, None, head, timestamp), &allow_all)
            .unwrap();
    }
    store
        .compare_and_swap(update("proposal/b", Some(&head_b), &head_e, 50), &allow_all)
        .unwrap();

    let snapshot = store.snapshot().unwrap();
    assert_eq!(
        snapshot
            .refs
            .iter()
            .map(|record| record.name.as_str())
            .collect::<Vec<_>>(),
        vec!["decision/main", "proposal/a", "proposal/b", "release/z"]
    );
    assert_eq!(
        snapshot
            .refs
            .iter()
            .map(|record| record.updated_event_id)
            .collect::<Vec<_>>(),
        vec![3, 4, 5, 1]
    );
    assert_eq!(store.list().unwrap(), snapshot.refs);

    let reflog = store.reflog().unwrap();
    assert_eq!(
        reflog.iter().map(|entry| entry.id).collect::<Vec<_>>(),
        vec![1, 2, 3, 4, 5]
    );
    assert_eq!(
        store
            .reflog_for_ref("proposal/b")
            .unwrap()
            .iter()
            .map(|entry| entry.id)
            .collect::<Vec<_>>(),
        vec![2, 5]
    );

    let archive = store.export_archive().unwrap();
    assert_eq!(archive.snapshot, snapshot);
    assert_eq!(archive.reflog, reflog);
}

#[test]
fn ref_archive_export_limits_are_global_inclusive_and_read_only() {
    let head_a = commit_oid('a');
    let head_b = commit_oid('b');
    let head_c = commit_oid('c');
    let mut store = SqliteRefStore::open_in_memory().unwrap();
    store
        .compare_and_swap(
            RefUpdate {
                ref_name: "proposal/archive",
                expected_head: None,
                new_head: &head_a,
                metadata: ReflogMetadata {
                    occurred_at_unix_nanos: 1,
                    actor: Some("archiver"),
                    message: Some("create proposal"),
                },
            },
            &allow_all,
        )
        .unwrap();
    store
        .compare_and_swap(update("release/stable", None, &head_b, 2), &allow_all)
        .unwrap();
    store
        .compare_and_swap(
            RefUpdate {
                ref_name: "proposal/archive",
                expected_head: Some(&head_a),
                new_head: &head_c,
                metadata: ReflogMetadata {
                    occurred_at_unix_nanos: 3,
                    actor: None,
                    message: Some("advance"),
                },
            },
            &allow_all,
        )
        .unwrap();

    let expected = store.export_archive().unwrap();
    let text_bytes = expected
        .snapshot
        .refs
        .iter()
        .map(|record| record.name.len())
        .chain(expected.reflog.iter().map(|entry| {
            entry.ref_name.len()
                + entry.actor.as_ref().map_or(0, String::len)
                + entry.message.as_ref().map_or(0, String::len)
        }))
        .sum::<usize>() as u64;
    let exact = RefArchiveExportLimits {
        max_refs: expected.snapshot.refs.len(),
        max_reflog_entries: expected.reflog.len(),
        max_text_bytes: text_bytes,
    };
    assert_eq!(store.export_archive_with_limits(exact).unwrap(), expected);

    for limits in [
        RefArchiveExportLimits {
            max_refs: exact.max_refs - 1,
            ..exact
        },
        RefArchiveExportLimits {
            max_reflog_entries: exact.max_reflog_entries - 1,
            ..exact
        },
        RefArchiveExportLimits {
            max_text_bytes: exact.max_text_bytes - 1,
            ..exact
        },
        RefArchiveExportLimits {
            max_refs: 0,
            ..exact
        },
        RefArchiveExportLimits {
            max_reflog_entries: 0,
            ..exact
        },
        RefArchiveExportLimits {
            max_text_bytes: 0,
            ..exact
        },
    ] {
        let error = store.export_archive_with_limits(limits).unwrap_err();
        assert_eq!(error.code(), "resource_limit");
        assert!(matches!(error, RefStoreError::Validation(_)));
        assert_eq!(store.export_archive().unwrap(), expected);
    }
}

#[test]
fn bounded_ref_archive_export_rejects_sql_level_semantic_corruption() {
    for corrupt_head in [false, true] {
        let label = if corrupt_head {
            "bounded-export-invalid-head"
        } else {
            "bounded-export-invalid-name"
        };
        let directory = TestDirectory::new(label);
        let database = directory.database_path();
        {
            let mut store = SqliteRefStore::open(&database).unwrap();
            store
                .compare_and_swap(
                    update("proposal/archive", None, &commit_oid('a'), 1),
                    &allow_all,
                )
                .unwrap();
        }
        let connection = Connection::open(&database).unwrap();
        if corrupt_head {
            let invalid_head = format!("commit:sg-oid-v1:sha256:{}", "z".repeat(64));
            connection
                .execute("UPDATE refs SET head = ?1", [&invalid_head])
                .unwrap();
            connection
                .execute("UPDATE ref_events SET new_head = ?1", [&invalid_head])
                .unwrap();
        } else {
            connection
                .execute("UPDATE refs SET name = 'x'", [])
                .unwrap();
            connection
                .execute("UPDATE ref_events SET ref_name = 'x'", [])
                .unwrap();
        }
        drop(connection);

        let mut store = SqliteRefStore::open(&database).unwrap();
        let error = store
            .export_archive_with_limits(RefArchiveExportLimits::default())
            .unwrap_err();
        assert_eq!(error.code(), "archive_invalid");
        assert!(matches!(error, RefStoreError::ArchiveInvalid { .. }));
    }
}

#[test]
fn archive_round_trip_into_empty_store_preserves_snapshot_reflog_and_ids() {
    let archive = create_sample_archive();
    let expected_heads: BTreeSet<_> = archive
        .reflog
        .iter()
        .map(|entry| entry.new_head.clone())
        .collect();
    let validated_heads = Mutex::new(Vec::new());
    let validator = |head: &str| {
        validated_heads.lock().unwrap().push(head.to_owned());
        Ok(())
    };
    let mut restored = SqliteRefStore::open_in_memory().unwrap();

    restored.restore_archive(&archive, &validator).unwrap();
    assert_eq!(restored.export_archive().unwrap(), archive);
    let validated_heads = validated_heads.into_inner().unwrap();
    assert_eq!(
        validated_heads,
        expected_heads.into_iter().collect::<Vec<_>>(),
        "each distinct archived head must be validated exactly once in deterministic order"
    );

    let next_head = commit_oid('f');
    let current = restored.get("release/stable").unwrap().unwrap();
    let next_event = restored
        .compare_and_swap(
            update("release/stable", Some(&current.head), &next_head, 40),
            &allow_all,
        )
        .unwrap();
    assert_eq!(
        next_event.id,
        archive
            .reflog
            .last()
            .expect("sample archive is nonempty")
            .id
            + 1
    );
}

#[test]
fn empty_archive_restores_as_an_empty_store() {
    let mut source = SqliteRefStore::open_in_memory().unwrap();
    let archive = source.export_archive().unwrap();
    assert_eq!(archive, RefArchive::default());

    let validation_calls = AtomicUsize::new(0);
    let validator = |_: &str| {
        validation_calls.fetch_add(1, Ordering::SeqCst);
        Ok(())
    };
    let mut restored = SqliteRefStore::open_in_memory().unwrap();
    restored.restore_archive(&archive, &validator).unwrap();

    assert_eq!(validation_calls.load(Ordering::SeqCst), 0);
    assert_eq!(restored.export_archive().unwrap(), archive);
}

#[test]
fn restore_rejects_broken_or_incomplete_reflog_chains_without_mutation() {
    let valid = create_sample_archive();
    let unrelated_head = commit_oid('f');

    let mut broken_old_head = valid.clone();
    broken_old_head.reflog[2].old_head = Some(unrelated_head.clone());

    let mut wrong_final_head = valid.clone();
    wrong_final_head
        .snapshot
        .refs
        .iter_mut()
        .find(|record| record.name == "proposal/archive")
        .unwrap()
        .head = unrelated_head.clone();

    let mut wrong_final_event = valid.clone();
    wrong_final_event
        .snapshot
        .refs
        .iter_mut()
        .find(|record| record.name == "proposal/archive")
        .unwrap()
        .updated_event_id = 1;

    let mut orphan_reflog_chain = valid.clone();
    orphan_reflog_chain.reflog.push(ReflogEntry {
        id: 4,
        ref_name: "observed/orphan".to_owned(),
        old_head: None,
        new_head: unrelated_head,
        occurred_at_unix_nanos: 40,
        actor: None,
        message: None,
    });

    let mut duplicate_event_id = valid.clone();
    duplicate_event_id.reflog[2].id = duplicate_event_id.reflog[1].id;

    for archive in [
        broken_old_head,
        wrong_final_head,
        wrong_final_event,
        orphan_reflog_chain,
        duplicate_event_id,
    ] {
        assert_archive_invalid(&archive);
    }
}

#[test]
fn restore_validation_failure_is_atomic() {
    let archive = create_sample_archive();
    let rejected_head = commit_oid('b');
    let mut restored = SqliteRefStore::open_in_memory().unwrap();
    let validator = |head: &str| {
        if head == rejected_head {
            Err(ValidationError::new(
                "closure_missing",
                "archive closure is incomplete",
            ))
        } else {
            Ok(())
        }
    };

    let error = restored
        .restore_archive(&archive, &validator)
        .expect_err("a rejected archived head must abort restore");
    assert_eq!(error.code(), "closure_missing");
    assert!(matches!(error, RefStoreError::Validation(_)));
    assert_eq!(restored.snapshot().unwrap(), RefSnapshot::default());
    assert!(restored.reflog().unwrap().is_empty());
}

#[test]
fn restore_rejects_a_nonempty_store_without_changing_it() {
    let archive = create_sample_archive();
    let existing_head = commit_oid('d');
    let mut target = SqliteRefStore::open_in_memory().unwrap();
    target
        .compare_and_swap(
            update("material-events/live", None, &existing_head, 999),
            &allow_all,
        )
        .unwrap();
    let before = target.export_archive().unwrap();

    let error = target
        .restore_archive(&archive, &allow_all)
        .expect_err("restore into a nonempty store must fail");
    assert!(matches!(error, RefStoreError::ArchiveNotEmpty));
    assert_eq!(error.code(), "archive_invalid");
    assert_eq!(target.export_archive().unwrap(), before);
}
