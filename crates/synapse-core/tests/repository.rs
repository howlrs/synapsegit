use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use synapse_canonical::blob_oid;
use synapse_cas::ClosureNodeState;
use synapse_core::{Repository, RepositoryError};
use synapse_schema::ingest;
use synapse_sqlite::{RefStoreError, RefUpdate, ReflogMetadata};

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
