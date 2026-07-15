use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use synapse_canonical::blob_oid;
use synapse_cas::{
    ClosureNodeState, GraphLimits, StoreLimits, TombstoneScanLimits, verify_closure,
};
use synapse_core::{ArchiveExportLimits, RefArchiveExportLimits, Repository, RepositoryError};
use synapse_schema::ingest;
use synapse_sqlite::{
    RefArchive, RefRecord, RefSnapshot, RefStoreError, RefUpdate, ReflogEntry, ReflogMetadata,
    SqliteRefStore,
};

static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

struct TempDirectory(PathBuf);

impl TempDirectory {
    fn new(label: &str) -> Self {
        let sequence = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "synapse-core-{label}-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }

    fn join(&self, path: impl AsRef<Path>) -> PathBuf {
        self.0.join(path)
    }
}

impl Drop for TempDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn fixture_directory() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../spec/core/v0.1/fixtures")
}

fn fixture(name: &str) -> Vec<u8> {
    fs::read(fixture_directory().join(name)).unwrap()
}

fn load_fixture_store(repository: &Repository) {
    let directory = fixture_directory();
    let mut paths = fs::read_dir(&directory)
        .unwrap()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "json")
                && path.file_name().is_some_and(|name| name != "golden.json")
        })
        .collect::<Vec<_>>();
    paths.sort();
    for path in paths {
        repository.put_object(&fs::read(path).unwrap()).unwrap();
    }
    repository
        .put_blob(fs::File::open(directory.join("proposal.txt")).unwrap())
        .unwrap();
}

fn oid(name: &str) -> String {
    ingest(&fixture(name)).unwrap().oid().to_owned()
}

fn assert_no_archive_staging(temporary: &TempDirectory, destination_name: &str) {
    let prefix = format!(".{destination_name}.tmp-");
    let remaining = fs::read_dir(&temporary.0)
        .unwrap()
        .filter_map(Result::ok)
        .map(|entry| entry.file_name())
        .filter(|name| name.to_string_lossy().starts_with(&prefix))
        .collect::<Vec<_>>();
    assert!(
        remaining.is_empty(),
        "archive staging remains: {remaining:?}"
    );
}

#[test]
fn validated_store_ref_fsck_export_and_empty_restore_round_trip() {
    let temporary = TempDirectory::new("round-trip");
    let repository_path = temporary.join("source");
    let archive_path = temporary.join("archive");
    let restored_path = temporary.join("restored");
    let mut repository = Repository::open(&repository_path).unwrap();
    load_fixture_store(&repository);

    let proposal = oid("proposal-commit.json");
    repository
        .update_ref(RefUpdate {
            ref_name: "proposal/agent/run-1",
            expected_head: None,
            new_head: &proposal,
            metadata: ReflogMetadata {
                occurred_at_unix_nanos: 1_000,
                actor: Some("urn:uuid:00000000-0000-4000-8000-000000000002"),
                message: Some("fixture proposal"),
            },
        })
        .unwrap();

    let before_oids = repository.objects().list_oids().unwrap();
    let before_refs = repository.refs().snapshot().unwrap();
    let before_reflog = repository.refs().reflog().unwrap();
    let report = repository.fsck().unwrap();
    assert!(report.is_clean(), "{:?}", report.issues);

    repository.export_archive(&archive_path).unwrap();
    let restored = Repository::restore_archive(&archive_path, &restored_path).unwrap();
    assert_eq!(restored.objects().list_oids().unwrap(), before_oids);
    assert_eq!(restored.refs().snapshot().unwrap(), before_refs);
    assert_eq!(restored.refs().reflog().unwrap(), before_reflog);
    assert!(restored.fsck().unwrap().is_clean());
}

#[test]
fn stale_ref_update_does_not_change_head_or_reflog() {
    let temporary = TempDirectory::new("conflict");
    let mut repository = Repository::open(temporary.join("repo")).unwrap();
    load_fixture_store(&repository);
    let base = oid("base-commit.json");
    let proposal = oid("proposal-commit.json");
    repository
        .update_ref(RefUpdate {
            ref_name: "proposal/agent/run-1",
            expected_head: None,
            new_head: &proposal,
            metadata: ReflogMetadata::at(1),
        })
        .unwrap();
    let snapshot = repository.refs().snapshot().unwrap();
    let reflog = repository.refs().reflog().unwrap();

    let error = repository
        .update_ref(RefUpdate {
            ref_name: "proposal/agent/run-1",
            expected_head: Some(&base),
            new_head: &proposal,
            metadata: ReflogMetadata::at(2),
        })
        .unwrap_err();
    assert_eq!(error.code(), "ref_conflict");
    assert!(matches!(
        error,
        RepositoryError::RefStore(RefStoreError::RefConflict { .. })
    ));
    assert_eq!(repository.refs().snapshot().unwrap(), snapshot);
    assert_eq!(repository.refs().reflog().unwrap(), reflog);
}

#[test]
fn ref_update_rejects_a_missing_commit_closure_without_mutation() {
    let temporary = TempDirectory::new("missing");
    let mut repository = Repository::open(temporary.join("repo")).unwrap();
    let proposal_bytes = fixture("proposal-commit.json");
    let proposal = repository.put_object(&proposal_bytes).unwrap().oid;
    let error = repository
        .update_ref(RefUpdate {
            ref_name: "proposal/agent/run-1",
            expected_head: None,
            new_head: &proposal,
            metadata: ReflogMetadata::at(1),
        })
        .unwrap_err();
    assert_eq!(error.code(), "closure_missing");
    assert!(repository.refs().snapshot().unwrap().is_empty());
    assert!(repository.refs().reflog().unwrap().is_empty());
}

#[test]
fn ref_update_rejects_a_tombstoned_missing_commit_root_without_mutation() {
    let temporary = TempDirectory::new("missing-tombstoned-root");
    let mut repository = Repository::open(temporary.join("repo")).unwrap();
    let missing =
        "commit:sg-oid-v1:sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    let tombstone = String::from_utf8(fixture("tombstone.json"))
        .unwrap()
        .replace(
            "blob:sg-oid-v1:sha256:0601e763908d07ad94396b6eea24f1b4f3da3b3250c643a90beef19bb12f798b",
            missing,
        );
    repository.put_object(tombstone.as_bytes()).unwrap();
    let snapshot = repository.refs().snapshot().unwrap();
    let reflog = repository.refs().reflog().unwrap();

    let error = repository
        .update_ref(RefUpdate {
            ref_name: "proposal/agent/run-1",
            expected_head: None,
            new_head: missing,
            metadata: ReflogMetadata::at(1),
        })
        .unwrap_err();
    assert_eq!(error.code(), "closure_missing");
    assert_eq!(repository.refs().snapshot().unwrap(), snapshot);
    assert_eq!(repository.refs().reflog().unwrap(), reflog);
}

#[test]
fn failed_restore_publishes_no_refs_and_can_resume_after_archive_repair() {
    let temporary = TempDirectory::new("tamper");
    let mut source = Repository::open(temporary.join("source")).unwrap();
    load_fixture_store(&source);
    let proposal = oid("proposal-commit.json");
    source
        .update_ref(RefUpdate {
            ref_name: "proposal/agent/run-1",
            expected_head: None,
            new_head: &proposal,
            metadata: ReflogMetadata::at(1),
        })
        .unwrap();
    let archive = temporary.join("archive");
    source.export_archive(&archive).unwrap();

    let damaged_object = archive.join("objects/00000001");
    let original = fs::read(&damaged_object).unwrap();
    let mut bytes = original.clone();
    bytes[0] ^= 1;
    fs::write(&damaged_object, bytes).unwrap();

    let restored_path = temporary.join("restored");
    let error = match Repository::restore_archive(&archive, &restored_path) {
        Ok(_) => panic!("tampered archive must fail"),
        Err(error) => error,
    };
    assert!(matches!(error.code(), "archive_invalid" | "oid_mismatch"));
    let mut restored = Repository::open(restored_path).unwrap();
    assert!(restored.refs().snapshot().unwrap().is_empty());
    assert!(restored.refs().reflog().unwrap().is_empty());
    assert!(!restored.objects().list_oids().unwrap().is_empty());

    fs::write(&damaged_object, original).unwrap();
    restored.restore_from(&archive).unwrap();
    assert!(!restored.refs().snapshot().unwrap().is_empty());
}

#[test]
fn export_rejects_missing_historical_reflog_head_without_publishing_destination() {
    let temporary = TempDirectory::new("missing-historical-archive-head");
    let repository_path = temporary.join("source");
    let destination = temporary.join("archive");
    let mut repository = Repository::open(&repository_path).unwrap();
    load_fixture_store(&repository);
    let current_head = oid("proposal-commit.json");
    let missing_head =
        "commit:sg-oid-v1:sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    // Construct an otherwise valid Ref archive through the public SQLite API:
    // the current Ref target is present, but an earlier reflog target is not.
    // This state can occur when an archive has been repaired incompletely.
    let archive = RefArchive {
        snapshot: RefSnapshot {
            refs: vec![RefRecord {
                name: "proposal/agent/run-1".to_owned(),
                head: current_head.clone(),
                updated_event_id: 2,
            }],
        },
        reflog: vec![
            ReflogEntry {
                id: 1,
                ref_name: "proposal/agent/run-1".to_owned(),
                old_head: None,
                new_head: missing_head.to_owned(),
                occurred_at_unix_nanos: 1,
                actor: None,
                message: None,
            },
            ReflogEntry {
                id: 2,
                ref_name: "proposal/agent/run-1".to_owned(),
                old_head: Some(missing_head.to_owned()),
                new_head: current_head,
                occurred_at_unix_nanos: 2,
                actor: None,
                message: None,
            },
        ],
    };
    let mut refs = SqliteRefStore::open(repository_path.join("refs.sqlite3")).unwrap();
    refs.restore_archive(&archive, &|_: &str| Ok(())).unwrap();
    drop(refs);

    let error = repository.export_archive(&destination).unwrap_err();
    assert_eq!(error.code(), "archive_invalid");
    assert!(!destination.exists());
}

#[test]
fn bounded_export_enforces_inclusive_object_count_and_cleans_staging() {
    let temporary = TempDirectory::new("archive-object-count-limit");
    let mut repository = Repository::open(temporary.join("source")).unwrap();
    load_fixture_store(&repository);
    let object_count = repository.objects().list_oids().unwrap().len();

    let exact_destination = temporary.join("exact-archive");
    repository
        .export_archive_with_limits(
            &exact_destination,
            ArchiveExportLimits {
                max_objects: object_count,
                ..ArchiveExportLimits::default()
            },
        )
        .unwrap();
    assert!(exact_destination.is_dir());

    let limited_destination = temporary.join("limited-archive");
    let error = repository
        .export_archive_with_limits(
            &limited_destination,
            ArchiveExportLimits {
                max_objects: object_count - 1,
                ..ArchiveExportLimits::default()
            },
        )
        .unwrap_err();
    assert_eq!(error.code(), "resource_limit");
    assert!(!limited_destination.exists());
    assert_no_archive_staging(&temporary, "limited-archive");

    for limits in [
        ArchiveExportLimits {
            max_objects: 0,
            ..ArchiveExportLimits::default()
        },
        ArchiveExportLimits {
            max_object_bytes: 0,
            ..ArchiveExportLimits::default()
        },
        ArchiveExportLimits {
            max_head_validation_nodes: 0,
            ..ArchiveExportLimits::default()
        },
        ArchiveExportLimits {
            max_head_validation_edges: 0,
            ..ArchiveExportLimits::default()
        },
    ] {
        let error = repository
            .export_archive_with_limits(temporary.join("invalid-limit"), limits)
            .unwrap_err();
        assert_eq!(error.code(), "resource_limit");
        assert!(!temporary.join("invalid-limit").exists());
    }
}

#[test]
fn bounded_export_enforces_inclusive_total_bytes_without_partial_publication() {
    let temporary = TempDirectory::new("archive-object-byte-limit");
    let mut repository = Repository::open(temporary.join("source")).unwrap();
    repository.put_blob(b"abc".as_slice()).unwrap();
    repository.put_blob(b"defg".as_slice()).unwrap();

    let exact_destination = temporary.join("exact-bytes");
    repository
        .export_archive_with_limits(
            &exact_destination,
            ArchiveExportLimits {
                max_objects: 2,
                max_object_bytes: 7,
                ..ArchiveExportLimits::default()
            },
        )
        .unwrap();
    Repository::restore_archive(&exact_destination, temporary.join("restored")).unwrap();

    let limited_destination = temporary.join("limited-bytes");
    let error = repository
        .export_archive_with_limits(
            &limited_destination,
            ArchiveExportLimits {
                max_objects: 2,
                max_object_bytes: 6,
                ..ArchiveExportLimits::default()
            },
        )
        .unwrap_err();
    assert_eq!(error.code(), "resource_limit");
    assert!(!limited_destination.exists());
    assert_no_archive_staging(&temporary, "limited-bytes");
}

#[test]
fn bounded_export_limits_the_shared_tombstone_inventory_scan() {
    let temporary = TempDirectory::new("archive-tombstone-scan-limit");
    let mut repository = Repository::open(temporary.join("source")).unwrap();
    load_fixture_store(&repository);
    let destination = temporary.join("archive");

    let error = repository
        .export_archive_with_limits(
            &destination,
            ArchiveExportLimits {
                tombstone_scan: TombstoneScanLimits {
                    max_record_objects: 1,
                    max_record_bytes: u64::MAX,
                },
                ..ArchiveExportLimits::default()
            },
        )
        .unwrap_err();
    assert_eq!(error.code(), "resource_limit");
    assert!(!destination.exists());
    assert_no_archive_staging(&temporary, "archive");
}

#[test]
fn bounded_export_preserves_a_closure_read_resource_limit() {
    let temporary = TempDirectory::new("archive-closure-read-limit");
    let repository_path = temporary.join("source");
    let proposal = oid("proposal-commit.json");
    {
        let mut repository = Repository::open(&repository_path).unwrap();
        load_fixture_store(&repository);
        repository
            .update_ref(RefUpdate {
                ref_name: "proposal/limited-reader",
                expected_head: None,
                new_head: &proposal,
                metadata: ReflogMetadata::at(1),
            })
            .unwrap();
    }

    let store_limits = StoreLimits {
        max_blob_bytes: fs::metadata(fixture_directory().join("proposal.txt"))
            .unwrap()
            .len()
            - 1,
        ..StoreLimits::default()
    };
    let mut repository =
        Repository::open_with_limits(&repository_path, store_limits, GraphLimits::default())
            .unwrap();
    let destination = temporary.join("archive");

    let error = repository.export_archive(&destination).unwrap_err();
    assert_eq!(error.code(), "resource_limit");
    assert!(!destination.exists());
    assert_no_archive_staging(&temporary, "archive");
}

#[test]
fn bounded_export_caps_cumulative_work_across_distinct_heads() {
    let temporary = TempDirectory::new("archive-head-work-limit");
    let mut repository = Repository::open(temporary.join("source")).unwrap();
    load_fixture_store(&repository);
    let base = oid("base-commit.json");
    let proposal = oid("proposal-commit.json");

    for (index, (ref_name, head)) in [
        ("proposal/archive-base", base.as_str()),
        ("proposal/archive-tip", proposal.as_str()),
    ]
    .into_iter()
    .enumerate()
    {
        repository
            .update_ref(RefUpdate {
                ref_name,
                expected_head: None,
                new_head: head,
                metadata: ReflogMetadata::at(index as i64 + 1),
            })
            .unwrap();
    }

    let base_report = verify_closure(repository.objects(), &base, GraphLimits::default()).unwrap();
    let proposal_report =
        verify_closure(repository.objects(), &proposal, GraphLimits::default()).unwrap();
    assert!(base_report.is_complete());
    assert!(proposal_report.is_complete());
    let total_nodes = base_report.nodes.len() + proposal_report.nodes.len();
    let total_edges = base_report.edges.len() + proposal_report.edges.len();

    let exact = ArchiveExportLimits {
        max_head_validation_nodes: total_nodes,
        max_head_validation_edges: total_edges,
        ..ArchiveExportLimits::default()
    };
    let exact_destination = temporary.join("exact-head-work");
    repository
        .export_archive_with_limits(&exact_destination, exact)
        .unwrap();
    Repository::restore_archive(&exact_destination, temporary.join("restored-head-work")).unwrap();

    for (destination_name, limits) in [
        (
            "limited-head-nodes",
            ArchiveExportLimits {
                max_head_validation_nodes: total_nodes - 1,
                ..exact
            },
        ),
        (
            "limited-head-edges",
            ArchiveExportLimits {
                max_head_validation_edges: total_edges - 1,
                ..exact
            },
        ),
    ] {
        let destination = temporary.join(destination_name);
        let error = repository
            .export_archive_with_limits(&destination, limits)
            .unwrap_err();
        assert_eq!(error.code(), "resource_limit");
        assert!(!destination.exists());
        assert_no_archive_staging(&temporary, destination_name);
    }
}

#[test]
fn bounded_export_rejects_an_oversized_ref_snapshot_before_publication() {
    let temporary = TempDirectory::new("archive-ref-snapshot-limit");
    let mut repository = Repository::open(temporary.join("source")).unwrap();
    load_fixture_store(&repository);
    let proposal = oid("proposal-commit.json");
    for (index, ref_name) in ["proposal/one", "proposal/two"].into_iter().enumerate() {
        repository
            .update_ref(RefUpdate {
                ref_name,
                expected_head: None,
                new_head: &proposal,
                metadata: ReflogMetadata::at(index as i64 + 1),
            })
            .unwrap();
    }
    let destination = temporary.join("archive");

    let error = repository
        .export_archive_with_limits(
            &destination,
            ArchiveExportLimits {
                ref_archive: RefArchiveExportLimits {
                    max_refs: 1,
                    ..RefArchiveExportLimits::default()
                },
                ..ArchiveExportLimits::default()
            },
        )
        .unwrap_err();
    assert_eq!(error.code(), "resource_limit");
    assert!(!destination.exists());
    assert_no_archive_staging(&temporary, "archive");
}

#[test]
fn tombstone_preserves_a_historical_closure_after_blob_erasure() {
    let temporary = TempDirectory::new("tombstone");
    let mut repository = Repository::open(temporary.join("repo")).unwrap();
    let directory = fixture_directory();
    for entry in fs::read_dir(&directory).unwrap().filter_map(Result::ok) {
        let path = entry.path();
        if path
            .extension()
            .is_some_and(|extension| extension == "json")
            && path.file_name().is_some_and(|name| name != "golden.json")
        {
            repository.put_object(&fs::read(path).unwrap()).unwrap();
        }
    }

    let erased_blob = blob_oid(&fixture("proposal.txt"));
    assert!(
        repository
            .objects()
            .read_raw(&erased_blob)
            .unwrap()
            .is_none()
    );
    let proposal = oid("proposal-commit.json");
    repository
        .update_ref(RefUpdate {
            ref_name: "proposal/agent/run-erased",
            expected_head: None,
            new_head: &proposal,
            metadata: ReflogMetadata::at(1),
        })
        .unwrap();
    let report = repository.fsck().unwrap();
    assert!(report.is_clean(), "{:?}", report.issues);
    let closure = report
        .closures
        .iter()
        .find(|closure| closure.root == proposal)
        .unwrap();
    assert!(matches!(
        closure.nodes.get(&erased_blob).map(|node| &node.state),
        Some(ClosureNodeState::Tombstoned { .. })
    ));
}
