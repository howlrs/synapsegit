use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Barrier};
use std::thread;

use synapse_canonical::{ErrorCode, ResourceLimits};
use synapse_cas::{
    ClosureIssueKind, ClosureNodeState, FileObjectStore, FsckIssueKind, GraphLimits, ObjectKind,
    ObjectState, PutDisposition, ReferenceRole, StoreError, StoreLimits, fsck_all, verify_closure,
};

static NEXT_TEMP_DIRECTORY: AtomicU64 = AtomicU64::new(0);

struct TempDirectory {
    path: PathBuf,
}

impl TempDirectory {
    fn new(label: &str) -> Self {
        loop {
            let sequence = NEXT_TEMP_DIRECTORY.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "synapse-cas-{label}-{}-{sequence}",
                std::process::id()
            ));
            match fs::create_dir(&path) {
                Ok(()) => return Self { path },
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => panic!(
                    "create temporary test directory {}: {error}",
                    path.display()
                ),
            }
        }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn open_store(label: &str) -> (TempDirectory, FileObjectStore) {
    let temporary = TempDirectory::new(label);
    let store = FileObjectStore::open(temporary.path()).expect("open filesystem CAS");
    (temporary, store)
}

fn fake_oid(kind: &str, digit: &str) -> String {
    format!("{kind}:sg-oid-v1:sha256:{}", digit.repeat(64))
}

fn object_path(root: &Path, oid: &str) -> PathBuf {
    let family = oid.split(':').next().expect("OID has a family");
    let digest = oid.rsplit(':').next().expect("OID has a digest");
    root.join("objects")
        .join(family)
        .join(&digest[..2])
        .join(&digest[2..])
}

fn assert_error_code(error: &StoreError, expected: ErrorCode) {
    assert_eq!(error.code(), Some(expected), "unexpected error: {error}");
}

fn put_record(store: &FileObjectStore, name: &str) -> String {
    let input = format!(r#"{{"object_type":"record","name":"{name}"}}"#);
    store
        .put_structured_unchecked(input.as_bytes())
        .expect("put fixture Record")
        .oid
}

fn put_tree(store: &FileObjectStore, entries: &[(&str, &str, &str)]) -> String {
    let entries = entries
        .iter()
        .map(|(segment, kind, oid)| {
            format!(r#""{segment}":{{"entry_kind":"{kind}","oid":"{oid}"}}"#)
        })
        .collect::<Vec<_>>()
        .join(",");
    let input = format!(r#"{{"object_type":"tree","entries":{{{entries}}}}}"#);
    store
        .put_structured_unchecked(input.as_bytes())
        .expect("put fixture Tree")
        .oid
}

fn quoted_oids(oids: &[&str]) -> String {
    oids.iter()
        .map(|oid| format!(r#""{oid}""#))
        .collect::<Vec<_>>()
        .join(",")
}

fn put_commit(
    store: &FileObjectStore,
    parents: &[&str],
    snapshot: &str,
    transition_refs: &[&str],
    bound_declaration_refs: &[&str],
) -> String {
    let input = format!(
        concat!(
            r#"{{"object_type":"commit","parents":[{}],"snapshot":"{}","#,
            r#""transition_refs":[{}],"bound_declaration_refs":[{}]}}"#
        ),
        quoted_oids(parents),
        snapshot,
        quoted_oids(transition_refs),
        quoted_oids(bound_declaration_refs),
    );
    store
        .put_structured_unchecked(input.as_bytes())
        .expect("put fixture Commit")
        .oid
}

struct CompleteFixture {
    blob: String,
    record: String,
    nested_tree: String,
    root_tree: String,
    parent_commit: String,
    tip_commit: String,
}

impl CompleteFixture {
    fn object_count(&self) -> usize {
        6
    }
}

fn install_complete_fixture(store: &FileObjectStore) -> CompleteFixture {
    let blob = store
        .put_blob(b"fixture blob".as_slice())
        .expect("put fixture Blob")
        .oid;
    let record = put_record(store, "transition");
    let nested_tree = put_tree(store, &[]);
    let root_tree = put_tree(
        store,
        &[
            ("asset", "blob", blob.as_str()),
            ("nested", "tree", nested_tree.as_str()),
        ],
    );
    let parent_commit = put_commit(store, &[], &root_tree, &[], &[]);
    let tip_commit = put_commit(
        store,
        &[parent_commit.as_str()],
        &root_tree,
        &[record.as_str()],
        &[],
    );
    CompleteFixture {
        blob,
        record,
        nested_tree,
        root_tree,
        parent_commit,
        tip_commit,
    }
}

#[test]
fn blob_and_structured_puts_are_idempotent_and_store_canonical_bytes() {
    let (_temporary, store) = open_store("idempotent");

    let first_blob = store.put_blob(b"hello CAS".as_slice()).unwrap();
    let second_blob = store.put_blob(b"hello CAS".as_slice()).unwrap();
    assert_eq!(first_blob.oid, second_blob.oid);
    assert_eq!(first_blob.kind, ObjectKind::Blob);
    assert_eq!(first_blob.byte_len, 9);
    assert_eq!(first_blob.disposition, PutDisposition::Created);
    assert_eq!(second_blob.disposition, PutDisposition::AlreadyPresent);

    let first_record = store
        .put_structured_unchecked(br#"{ "value": 1, "object_type": "record" }"#)
        .unwrap();
    let second_record = store
        .put_structured_unchecked(br#"{"object_type":"record","value":1}"#)
        .unwrap();
    assert_eq!(first_record.oid, second_record.oid);
    assert_eq!(first_record.kind, ObjectKind::Record);
    assert_eq!(first_record.disposition, PutDisposition::Created);
    assert_eq!(second_record.disposition, PutDisposition::AlreadyPresent);
    assert_eq!(
        store.read_raw(&first_record.oid).unwrap().unwrap(),
        br#"{"object_type":"record","value":1}"#
    );

    let listed = store.list_oids().unwrap();
    assert_eq!(
        listed.len(),
        2,
        "idempotent puts must not add inventory rows"
    );
    assert!(listed.contains(&first_blob.oid));
    assert!(listed.contains(&first_record.oid));
}

#[test]
fn claimed_oid_mismatches_are_rejected_without_publication() {
    let (temporary, store) = open_store("claimed-mismatch");

    let blob_error = store
        .put_blob_claimed(&fake_oid("blob", "0"), b"different".as_slice())
        .unwrap_err();
    assert_error_code(&blob_error, ErrorCode::OidMismatch);

    let wrong_family_error = store
        .put_blob_claimed(&fake_oid("record", "0"), b"different".as_slice())
        .unwrap_err();
    assert_error_code(&wrong_family_error, ErrorCode::OidMismatch);

    let record_error = store
        .put_structured_claimed_unchecked(
            &fake_oid("record", "0"),
            br#"{"object_type":"record","value":1}"#,
        )
        .unwrap_err();
    assert_error_code(&record_error, ErrorCode::OidMismatch);

    assert!(store.list_oids().unwrap().is_empty());
    assert_eq!(
        fs::read_dir(temporary.path().join("tmp")).unwrap().count(),
        0,
        "failed claimed puts must clean staged files"
    );
}

#[test]
fn raw_restore_requires_exact_canonical_structured_bytes() {
    let (_source_directory, source) = open_store("restore-source");
    let source_result = source
        .put_structured_unchecked(br#"{"value":1,"object_type":"record"}"#)
        .unwrap();
    let canonical = source.read_raw(&source_result.oid).unwrap().unwrap();

    let (_target_directory, target) = open_store("restore-target");
    let noncanonical_error = target
        .put_verified_raw(
            &source_result.oid,
            br#"{ "object_type": "record", "value": 1 }"#,
        )
        .unwrap_err();
    assert_error_code(&noncanonical_error, ErrorCode::SchemaInvalid);
    assert!(target.get_verified(&source_result.oid).unwrap().is_none());

    let restored = target
        .put_verified_raw(&source_result.oid, &canonical)
        .unwrap();
    assert_eq!(restored.disposition, PutDisposition::Created);
    assert_eq!(
        target.read_raw(&source_result.oid).unwrap(),
        Some(canonical)
    );
}

#[test]
fn configured_blob_and_structured_size_limits_are_enforced() {
    let temporary = TempDirectory::new("size-limits");
    let limits = StoreLimits {
        structured: ResourceLimits {
            max_input_bytes: 64,
            max_canonical_bytes: 64,
            ..ResourceLimits::default()
        },
        max_blob_bytes: 3,
        io_buffer_bytes: 2,
    };
    let store = FileObjectStore::open_with_limits(temporary.path(), limits).unwrap();

    assert!(store.put_blob(b"abc".as_slice()).is_ok());
    let blob_error = store.put_blob(b"abcd".as_slice()).unwrap_err();
    assert_error_code(&blob_error, ErrorCode::ResourceLimit);

    let input_error = store
        .put_structured_unchecked(
            br#"{"object_type":"record","payload":"this input intentionally exceeds sixty-four bytes"}"#,
        )
        .unwrap_err();
    assert_error_code(&input_error, ErrorCode::ResourceLimit);

    let output_directory = TempDirectory::new("canonical-output-limit");
    let output_limited = FileObjectStore::open_with_limits(
        output_directory.path(),
        StoreLimits {
            structured: ResourceLimits {
                max_input_bytes: 128,
                max_canonical_bytes: 16,
                ..ResourceLimits::default()
            },
            max_blob_bytes: 8,
            io_buffer_bytes: 2,
        },
    )
    .unwrap();
    let output_error = output_limited
        .put_structured_unchecked(br#"{"object_type":"record"}"#)
        .unwrap_err();
    assert_error_code(&output_error, ErrorCode::ResourceLimit);

    let invalid_directory = TempDirectory::new("invalid-buffer-limit");
    let invalid_limits = FileObjectStore::open_with_limits(
        invalid_directory.path(),
        StoreLimits {
            io_buffer_bytes: 0,
            ..StoreLimits::default()
        },
    )
    .unwrap_err();
    assert_error_code(&invalid_limits, ErrorCode::ResourceLimit);
}

#[test]
fn list_read_get_and_object_state_cover_present_missing_and_invalid_oids() {
    let (_temporary, store) = open_store("read-apis");
    let blob = store.put_blob(b"read me".as_slice()).unwrap();
    let record = store
        .put_structured_unchecked(br#"{"object_type":"record","label":"value"}"#)
        .unwrap();

    let mut expected = vec![blob.oid.clone(), record.oid.clone()];
    expected.sort_unstable();
    assert_eq!(store.list_oids().unwrap(), expected);
    assert_eq!(store.read_raw(&blob.oid).unwrap().unwrap(), b"read me");

    let verified_blob = store.get_verified(&blob.oid).unwrap().unwrap();
    assert_eq!(verified_blob.info().kind, ObjectKind::Blob);
    assert_eq!(verified_blob.byte_len(), 7);
    assert!(verified_blob.structured().is_none());

    let verified_record = store.get_verified(&record.oid).unwrap().unwrap();
    assert_eq!(verified_record.kind(), ObjectKind::Record);
    assert_eq!(
        verified_record
            .structured()
            .and_then(|value| value.get("label"))
            .and_then(|value| value.as_str()),
        Some("value")
    );

    match store.object_state(&record.oid).unwrap() {
        ObjectState::Present(info) => {
            assert_eq!(info.oid, record.oid);
            assert_eq!(info.kind, ObjectKind::Record);
        }
        other => panic!("expected present Record, got {other:?}"),
    }

    let missing = fake_oid("blob", "f");
    assert!(store.read_raw(&missing).unwrap().is_none());
    assert!(store.get_verified(&missing).unwrap().is_none());
    assert!(matches!(
        store.object_state(&missing).unwrap(),
        ObjectState::Missing
    ));

    let invalid_error = store.read_raw("not-an-oid").unwrap_err();
    assert_error_code(&invalid_error, ErrorCode::SchemaInvalid);
}

#[test]
fn concurrent_same_object_publication_creates_exactly_one_object() {
    const WRITERS: usize = 16;

    let temporary = TempDirectory::new("concurrent-publication");
    let store = Arc::new(FileObjectStore::open(temporary.path()).unwrap());
    let barrier = Arc::new(Barrier::new(WRITERS));
    let handles = (0..WRITERS)
        .map(|_| {
            let store = Arc::clone(&store);
            let barrier = Arc::clone(&barrier);
            thread::spawn(move || {
                barrier.wait();
                store.put_blob(b"one concurrently published value".as_slice())
            })
        })
        .collect::<Vec<_>>();

    let results = handles
        .into_iter()
        .map(|handle| {
            handle
                .join()
                .expect("writer panicked")
                .expect("writer failed")
        })
        .collect::<Vec<_>>();
    let expected_oid = results[0].oid.clone();
    assert!(results.iter().all(|result| result.oid == expected_oid));
    assert_eq!(
        results
            .iter()
            .filter(|result| result.disposition == PutDisposition::Created)
            .count(),
        1
    );
    assert_eq!(
        results
            .iter()
            .filter(|result| result.disposition == PutDisposition::AlreadyPresent)
            .count(),
        WRITERS - 1
    );
    assert_eq!(store.list_oids().unwrap(), vec![expected_oid.clone()]);
    assert_eq!(
        store.read_raw(&expected_oid).unwrap().unwrap(),
        b"one concurrently published value"
    );
    assert_eq!(
        fs::read_dir(temporary.path().join("tmp")).unwrap().count(),
        0
    );
}

#[test]
fn fixture_graph_has_a_complete_commit_closure() {
    let (_temporary, store) = open_store("complete-closure");
    let fixture = install_complete_fixture(&store);

    let report = verify_closure(&store, &fixture.tip_commit, GraphLimits::default()).unwrap();
    assert!(report.is_complete(), "closure issues: {:?}", report.issues);
    assert!(!report.truncated);
    assert_eq!(report.nodes.len(), fixture.object_count());
    assert_eq!(report.edges.len(), 6);
    for oid in [
        &fixture.tip_commit,
        &fixture.parent_commit,
        &fixture.root_tree,
        &fixture.nested_tree,
        &fixture.blob,
        &fixture.record,
    ] {
        assert!(matches!(
            report.nodes.get(oid).map(|node| &node.state),
            Some(ClosureNodeState::Present { .. })
        ));
    }
    assert!(report.edges.iter().any(|edge| {
        edge.source == fixture.tip_commit
            && edge.target == fixture.parent_commit
            && edge.role == ReferenceRole::CommitParent { index: 0 }
    }));
    assert!(report.edges.iter().any(|edge| {
        edge.source == fixture.tip_commit
            && edge.target == fixture.record
            && edge.role == ReferenceRole::CommitTransition { index: 0 }
    }));
    assert!(report.edges.iter().any(|edge| {
        edge.source == fixture.root_tree
            && edge.target == fixture.blob
            && edge.role
                == ReferenceRole::TreeEntry {
                    segment: "asset".to_owned(),
                }
    }));
}

#[test]
fn fixture_graph_reports_a_missing_referenced_object_with_context() {
    let (_temporary, store) = open_store("missing-closure");
    let missing_blob = fake_oid("blob", "e");
    let tree = put_tree(&store, &[("missing", "blob", missing_blob.as_str())]);
    let commit = put_commit(&store, &[], &tree, &[], &[]);

    let report = verify_closure(&store, &commit, GraphLimits::default()).unwrap();
    assert!(!report.is_complete());
    assert!(matches!(
        report.nodes.get(&missing_blob).map(|node| &node.state),
        Some(ClosureNodeState::Missing {
            kind: ObjectKind::Blob
        })
    ));
    assert!(report.issues.iter().any(|issue| {
        issue.oid == missing_blob
            && issue.referenced_by.as_deref() == Some(tree.as_str())
            && issue.role
                == Some(ReferenceRole::TreeEntry {
                    segment: "missing".to_owned(),
                })
            && matches!(issue.kind, ClosureIssueKind::Missing)
    }));
}

#[test]
fn fixture_graph_reports_reference_type_mismatches_before_reading_target() {
    let (_temporary, store) = open_store("type-mismatch-closure");
    let record = put_record(&store, "not-a-blob");
    let tree = put_tree(&store, &[("typed-as-blob", "blob", record.as_str())]);
    let commit = put_commit(&store, &[], &tree, &[], &[]);

    let report = verify_closure(&store, &commit, GraphLimits::default()).unwrap();
    assert!(!report.is_complete());
    assert!(!report.nodes.contains_key(&record));
    assert!(report.issues.iter().any(|issue| {
        issue.oid == record
            && issue.referenced_by.as_deref() == Some(tree.as_str())
            && matches!(
                issue.kind,
                ClosureIssueKind::ReferenceTypeMismatch {
                    expected: ObjectKind::Blob,
                    actual: ObjectKind::Record,
                }
            )
    }));
}

#[test]
fn fsck_is_clean_for_a_complete_fixture_graph() {
    let (_temporary, store) = open_store("fsck-clean");
    let fixture = install_complete_fixture(&store);

    let report = fsck_all(&store, GraphLimits::default()).unwrap();
    assert!(report.is_clean(), "fsck issues: {:?}", report.issues);
    assert_eq!(report.objects_seen, fixture.object_count());
    assert_eq!(report.objects_verified, fixture.object_count());
    assert_eq!(report.closures.len(), 2, "both stored Commits are roots");
    assert!(report.closures.iter().all(|closure| closure.is_complete()));
}

#[test]
fn fsck_and_reads_report_digest_corruption() {
    let (temporary, store) = open_store("fsck-corrupt");
    let fixture = install_complete_fixture(&store);
    fs::write(
        object_path(temporary.path(), &fixture.blob),
        b"tampered bytes",
    )
    .unwrap();

    let read_error = store.get_verified(&fixture.blob).unwrap_err();
    assert!(matches!(
        &read_error,
        StoreError::CorruptObject { oid, .. } if oid == &fixture.blob
    ));
    assert_error_code(&read_error, ErrorCode::OidMismatch);
    assert!(matches!(
        store.object_state(&fixture.blob).unwrap(),
        ObjectState::Corrupt {
            kind: ObjectKind::Blob,
            ..
        }
    ));

    let report = fsck_all(&store, GraphLimits::default()).unwrap();
    assert!(!report.is_clean());
    assert_eq!(report.objects_seen, fixture.object_count());
    assert_eq!(report.objects_verified, fixture.object_count() - 1);
    assert!(report.issues.iter().any(|issue| matches!(
        &issue.kind,
        FsckIssueKind::CorruptObject { oid, .. } if oid == &fixture.blob
    )));
    assert!(report.issues.iter().any(|issue| matches!(
        &issue.kind,
        FsckIssueKind::Closure(closure_issue)
            if closure_issue.oid == fixture.blob
                && matches!(closure_issue.kind, ClosureIssueKind::Corrupt { .. })
    )));
}

#[test]
fn fsck_reports_invalid_layout_while_list_rejects_it() {
    let (temporary, store) = open_store("invalid-layout");
    let blob = store.put_blob(b"valid object".as_slice()).unwrap();
    fs::write(
        temporary.path().join("objects").join("unknown-family"),
        b"not an object family",
    )
    .unwrap();

    let list_error = store.list_oids().unwrap_err();
    assert!(matches!(&list_error, StoreError::InvalidStoreLayout { .. }));
    assert_error_code(&list_error, ErrorCode::SchemaInvalid);

    let report = fsck_all(&store, GraphLimits::default()).unwrap();
    assert!(!report.is_clean());
    assert_eq!(report.objects_seen, 1);
    assert_eq!(report.objects_verified, 1);
    assert!(report.issues.iter().any(|issue| matches!(
        &issue.kind,
        FsckIssueKind::InvalidStorePath { path, .. }
            if path.ends_with("objects/unknown-family")
    )));
    assert_eq!(store.read_raw(&blob.oid).unwrap().unwrap(), b"valid object");
}

#[test]
fn opening_a_store_rejects_reserved_paths_that_are_files() {
    let temporary = TempDirectory::new("reserved-path-file");
    fs::write(temporary.path().join("objects"), b"not a directory").unwrap();

    let error = FileObjectStore::open(temporary.path()).unwrap_err();
    assert!(matches!(
        &error,
        StoreError::InvalidStoreLayout { path, .. } if path.ends_with("objects")
    ));
    assert_error_code(&error, ErrorCode::SchemaInvalid);
}
