use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use synapse_core::{FsckLimits, Repository, TombstoneScanLimits};
use synapse_local_service::{
    ARCHIVE_LIST_LIMITS, ArchiveList, ArchiveResult, ArchiveResultKind, ArchiveState, FsckResult,
    LocalService, MAINTENANCE_FSCK_LIMITS, OperationAccepted, OperationKind, OperationResult,
    OperationState, OperationStatus, ProjectConfirmation, ProjectRegistration,
};

static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

struct TempDirectory(PathBuf);

impl TempDirectory {
    fn new() -> Self {
        let sequence = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "synapse-local-service-maintenance-test-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }

    fn directory(&self, name: &str) -> PathBuf {
        let path = self.0.join(name);
        fs::create_dir(&path).unwrap();
        path
    }
}

impl Drop for TempDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn service_for(repository: &Path) -> LocalService {
    LocalService::new([ProjectRegistration::new("project", "Project", repository)]).unwrap()
}

fn service_with_archive_root(repository: &Path, archive_root: &Path) -> LocalService {
    LocalService::new([ProjectRegistration::new("project", "Project", repository)])
        .unwrap()
        .with_archive_root(archive_root.to_path_buf())
}

fn blob_path(repository: &Path, oid: &str) -> PathBuf {
    let digest = oid.rsplit(':').next().unwrap();
    repository
        .join("cas")
        .join("objects")
        .join("blob")
        .join(&digest[..2])
        .join(&digest[2..])
}

#[test]
fn maintenance_profile_confirmation_and_capabilities_are_server_fixed() {
    assert_eq!(
        MAINTENANCE_FSCK_LIMITS,
        FsckLimits {
            max_ref_roots: 100_000,
            max_objects: 100_000,
            max_object_bytes: 1024_u64 * 1024 * 1024 * 1024,
            max_closure_nodes: 1_000_000,
            max_closure_edges: 10_000_000,
            tombstone_scan: TombstoneScanLimits {
                max_record_objects: 100_000,
                max_record_bytes: 1024_u64 * 1024 * 1024,
            },
        }
    );

    let temporary = TempDirectory::new();
    let repository = temporary.directory("repository");
    let service = service_for(&repository);
    service
        .validate_fsck_confirmation(
            "project",
            &ProjectConfirmation {
                confirm_project_key: "project".into(),
            },
        )
        .unwrap();

    let mismatch = service
        .validate_fsck_confirmation(
            "project",
            &ProjectConfirmation {
                confirm_project_key: "other-project".into(),
            },
        )
        .unwrap_err();
    assert_eq!(mismatch.code(), "local_request_denied");
    assert!(mismatch.diagnostic().is_none());

    let unknown = service
        .validate_fsck_confirmation(
            "missing",
            &ProjectConfirmation {
                confirm_project_key: "missing".into(),
            },
        )
        .unwrap_err();
    assert_eq!(unknown.code(), "project_not_found");

    let capabilities = &service.list_projects().projects[0].capabilities;
    assert!(capabilities.fsck);
    assert!(!capabilities.archive_export);
    assert!(!capabilities.archive_restore);
}

#[test]
fn empty_fsck_completes_cleanly_and_updates_process_local_status() {
    let temporary = TempDirectory::new();
    let repository = temporary.directory("repository");
    let service = service_for(&repository);

    assert_eq!(service.project_status("project").unwrap().last_fsck, None);
    let result = service.run_maintenance_fsck("project").unwrap();
    assert_eq!(
        result,
        FsckResult {
            clean: true,
            objects_seen: 0,
            objects_verified: 0,
            closure_count: 0,
            issue_count: 0,
        }
    );
    assert_eq!(
        service.project_status("project").unwrap().last_fsck,
        Some(result)
    );
}

#[test]
fn dirty_fsck_is_a_completed_count_only_result() {
    let temporary = TempDirectory::new();
    let repository_path = temporary.directory("private-repository");
    let repository = Repository::open(&repository_path).unwrap();
    let stored = repository.put_blob(&b"verified blob"[..]).unwrap();
    let object_path = blob_path(&repository_path, &stored.oid);
    let service = service_for(&repository_path);
    fs::write(&object_path, b"corrupted blob").unwrap();

    let result = service.run_maintenance_fsck("project").unwrap();
    assert!(!result.clean);
    assert_eq!(result.objects_seen, 1);
    assert_eq!(result.objects_verified, 0);
    assert_eq!(result.closure_count, 0);
    assert_eq!(result.issue_count, 1);
    assert_eq!(
        service.project_status("project").unwrap().last_fsck,
        Some(result.clone())
    );

    let response_json = serde_json::to_string(&result).unwrap();
    assert!(!response_json.contains(&stored.oid));
    assert!(!response_json.contains(repository_path.to_str().unwrap()));
}

#[test]
fn nested_fsck_failures_only_expose_paths_as_diagnostics() {
    let temporary = TempDirectory::new();
    let repository_path = temporary.directory("private-repository");
    let repository = Repository::open(&repository_path).unwrap();
    let stored = repository.put_blob(&b"verified blob"[..]).unwrap();
    let object_path = blob_path(&repository_path, &stored.oid);
    let service = service_for(&repository_path);
    fs::remove_file(&object_path).unwrap();
    fs::create_dir(&object_path).unwrap();

    let error = service.run_maintenance_fsck("project").unwrap_err();
    let digest = stored.oid.rsplit(':').next().unwrap();
    assert_eq!(error.code(), "fsck_failed");
    assert!(!error.detail().contains(digest));
    let diagnostic = error
        .diagnostic()
        .expect("nested error is retained locally");
    let relative_object_path = object_path
        .strip_prefix(repository_path.join("cas"))
        .unwrap()
        .to_str()
        .unwrap();
    assert!(
        diagnostic.contains(relative_object_path),
        "unexpected diagnostic: {diagnostic}"
    );
    let problem_json = serde_json::to_string(&error.to_problem(500, "request-1")).unwrap();
    assert!(!problem_json.contains(digest));
    assert!(!problem_json.contains(repository_path.to_str().unwrap()));
}

#[test]
fn maintenance_operation_dtos_serialize_to_the_openapi_shape() {
    let confirmation = ProjectConfirmation {
        confirm_project_key: "project".into(),
    };
    assert_eq!(
        serde_json::to_value(&confirmation).unwrap(),
        json!({"confirm_project_key": "project"})
    );
    assert!(
        serde_json::from_value::<ProjectConfirmation>(json!({
            "confirm_project_key": "project",
            "unexpected": true
        }))
        .is_err()
    );

    let accepted = OperationAccepted {
        operation_id: "abcdefghijklmnopqrstuv".into(),
        state: OperationState::Queued,
        poll_path: "/api/v1/operations/abcdefghijklmnopqrstuv".into(),
    };
    assert_eq!(
        serde_json::to_value(&accepted).unwrap(),
        json!({
            "operation_id": "abcdefghijklmnopqrstuv",
            "state": "queued",
            "poll_path": "/api/v1/operations/abcdefghijklmnopqrstuv"
        })
    );

    let fsck = FsckResult {
        clean: false,
        objects_seen: 3,
        objects_verified: 2,
        closure_count: 1,
        issue_count: 1,
    };
    let status = OperationStatus {
        operation_id: "abcdefghijklmnopqrstuv".into(),
        kind: OperationKind::Fsck,
        project_key: "project".into(),
        state: OperationState::Succeeded,
        submitted_at: "2026-07-16T00:00:00Z".into(),
        completed_at: Some("2026-07-16T00:00:01Z".into()),
        result: Some(OperationResult::Fsck(fsck)),
        error: None,
    };
    let status_json = serde_json::to_value(&status).unwrap();
    assert_eq!(status_json["kind"], "fsck");
    assert_eq!(status_json["state"], "succeeded");
    assert_eq!(status_json["completed_at"], "2026-07-16T00:00:01Z");
    assert_eq!(status_json["result"]["clean"], Value::Bool(false));
    assert_eq!(status_json["error"], Value::Null);
    assert_eq!(
        serde_json::from_value::<OperationStatus>(status_json).unwrap(),
        status
    );

    let archive_status = OperationStatus {
        operation_id: "zyxwvutsrqponmlkjihgfe".into(),
        kind: OperationKind::ArchiveRestore,
        project_key: "project".into(),
        state: OperationState::Queued,
        submitted_at: "2026-07-16T00:00:00Z".into(),
        completed_at: None,
        result: Some(OperationResult::Archive(ArchiveResult {
            archive_name: "nightly".into(),
            result_kind: ArchiveResultKind::Restored,
            report_equivalence_required: true,
        })),
        error: None,
    };
    let archive_json = serde_json::to_value(&archive_status).unwrap();
    assert_eq!(archive_json["kind"], "archive_restore");
    assert_eq!(archive_json["completed_at"], Value::Null);
    assert_eq!(archive_json["result"]["result_kind"], "restored");
    assert_eq!(archive_json["error"], Value::Null);
}

#[test]
fn archive_list_limits_are_server_fixed() {
    // Server-fixed work ceiling, mirrored the same way as
    // `maintenance_profile_confirmation_and_capabilities_are_server_fixed`
    // pins `MAINTENANCE_FSCK_LIMITS`: this locks the exact profile so a
    // silent change is caught by this test rather than only observed at
    // runtime.
    assert_eq!(ARCHIVE_LIST_LIMITS.max_root_entries, 100_000);
    assert_eq!(ARCHIVE_LIST_LIMITS.inspection.max_objects, 100_000);
    assert_eq!(
        ARCHIVE_LIST_LIMITS.inspection.max_object_bytes,
        1024_u64 * 1024 * 1024 * 1024
    );
}

#[test]
fn list_archives_is_empty_without_a_configured_archive_root() {
    let temporary = TempDirectory::new();
    let repository = temporary.directory("repository");
    let service = service_for(&repository);
    assert_eq!(
        service.list_archives().unwrap(),
        ArchiveList {
            archives: Vec::new()
        }
    );
}

#[test]
fn list_archives_is_empty_for_an_empty_archive_root() {
    let temporary = TempDirectory::new();
    let repository = temporary.directory("repository");
    let archive_root = temporary.directory("archives");
    let service = service_with_archive_root(&repository, &archive_root);
    assert_eq!(
        service.list_archives().unwrap(),
        ArchiveList {
            archives: Vec::new()
        }
    );
}

fn export_named_archive(source_repository: &Path, archive_root: &Path, name: &str) -> String {
    let mut repository = Repository::open(source_repository).unwrap();
    repository
        .put_blob(&b"archive listing fixture blob"[..])
        .unwrap();
    let destination = archive_root.join(name);
    repository.export_archive(&destination).unwrap();
    let checksum_bytes = fs::read(destination.join("manifest.sha256")).unwrap();
    let checksum_text = String::from_utf8(checksum_bytes).unwrap();
    checksum_text.trim().to_owned()
}

#[test]
fn list_archives_reports_valid_invalid_and_staging_or_unknown_in_name_order() {
    let temporary = TempDirectory::new();
    let repository = temporary.directory("repository");
    let source = temporary.directory("archive-source");
    let archive_root = temporary.directory("archives");

    // `zzz-valid`: a real exported archive, expected `valid` with its
    // manifest checksum.
    let expected_checksum = export_named_archive(&source, &archive_root, "zzz-valid");

    // `bbb-invalid`: an exported archive whose manifest is then tampered,
    // expected `invalid`.
    export_named_archive(&source, &archive_root, "bbb-invalid");
    let bbb_manifest = archive_root.join("bbb-invalid").join("manifest.json");
    let mut bytes = fs::read(&bbb_manifest).unwrap();
    bytes.push(b'\n');
    fs::write(&bbb_manifest, bytes).unwrap();

    // `mmm-staging`: a slug directory with no manifest at all yet, expected
    // `staging_or_unknown`.
    fs::create_dir(archive_root.join("mmm-staging")).unwrap();

    // A dot-prefixed directory is never a valid slug and must not appear.
    fs::create_dir(archive_root.join(".tmp-export-hidden")).unwrap();
    // A non-slug (uppercase) directory name must not appear either.
    fs::create_dir(archive_root.join("Not-A-Slug")).unwrap();
    // A stray regular file directly under the root must not appear.
    fs::write(archive_root.join("readme.txt"), b"not an archive").unwrap();

    let service = service_with_archive_root(&repository, &archive_root);
    let list = service.list_archives().unwrap();

    assert_eq!(
        list.archives
            .iter()
            .map(|entry| entry.archive_name.as_str())
            .collect::<Vec<_>>(),
        vec!["bbb-invalid", "mmm-staging", "zzz-valid"],
        "archives must be sorted by archive_name ascending"
    );

    let bbb = &list.archives[0];
    assert_eq!(bbb.state, ArchiveState::Invalid);
    assert_eq!(bbb.manifest_checksum, None);

    let mmm = &list.archives[1];
    assert_eq!(mmm.state, ArchiveState::StagingOrUnknown);
    assert_eq!(mmm.manifest_checksum, None);

    let zzz = &list.archives[2];
    assert_eq!(zzz.state, ArchiveState::Valid);
    assert_eq!(
        zzz.manifest_checksum.as_deref(),
        Some(expected_checksum.as_str())
    );

    let response_json = serde_json::to_string(&list).unwrap();
    assert!(
        !response_json.contains(archive_root.to_str().unwrap()),
        "archive listing responses must never leak filesystem paths"
    );
}

#[test]
fn list_archives_fails_closed_when_the_inspection_profile_is_exhausted() {
    let temporary = TempDirectory::new();
    let repository = temporary.directory("repository");
    let source = temporary.directory("archive-source");
    let archive_root = temporary.directory("archives");
    export_named_archive(&source, &archive_root, "over-limit");

    let service = service_with_archive_root(&repository, &archive_root);
    let error = service
        .list_archives_with_limits(synapse_local_service::ArchiveListLimits {
            max_root_entries: ARCHIVE_LIST_LIMITS.max_root_entries,
            inspection: synapse_core::ArchiveInspectionLimits {
                max_objects: 0,
                max_object_bytes: 1,
            },
        })
        .unwrap_err();
    assert_eq!(error.code(), "resource_limit");
}

#[test]
fn list_archives_enforces_the_cumulative_object_limit() {
    let temporary = TempDirectory::new();
    let repository = temporary.directory("repository");
    let source = temporary.directory("archive-source");
    let archive_root = temporary.directory("archives");
    export_named_archive(&source, &archive_root, "aaa");
    export_named_archive(&source, &archive_root, "bbb");

    let service = service_with_archive_root(&repository, &archive_root);
    let error = service
        .list_archives_with_limits(synapse_local_service::ArchiveListLimits {
            max_root_entries: ARCHIVE_LIST_LIMITS.max_root_entries,
            inspection: synapse_core::ArchiveInspectionLimits {
                max_objects: 1,
                max_object_bytes: ARCHIVE_LIST_LIMITS.inspection.max_object_bytes,
            },
        })
        .unwrap_err();
    assert_eq!(error.code(), "resource_limit");
}

#[test]
fn list_archives_admits_the_exact_cumulative_inventory_limit() {
    let temporary = TempDirectory::new();
    let repository = temporary.directory("repository");
    let source = temporary.directory("archive-source");
    let empty_source = temporary.directory("empty-archive-source");
    let archive_root = temporary.directory("archives");
    export_named_archive(&source, &archive_root, "aaa");
    export_named_archive(&source, &archive_root, "bbb");
    let mut empty_repository = Repository::open(&empty_source).unwrap();
    empty_repository
        .export_archive(archive_root.join("zzz-empty"))
        .unwrap();

    let service = service_with_archive_root(&repository, &archive_root);
    let list = service
        .list_archives_with_limits(synapse_local_service::ArchiveListLimits {
            max_root_entries: ARCHIVE_LIST_LIMITS.max_root_entries,
            inspection: synapse_core::ArchiveInspectionLimits {
                max_objects: 2,
                max_object_bytes: (b"archive listing fixture blob".len() * 2) as u64,
            },
        })
        .unwrap();
    assert_eq!(list.archives.len(), 3);
    assert!(
        list.archives
            .iter()
            .all(|archive| archive.state == ArchiveState::Valid)
    );
}

#[test]
fn list_archives_enforces_the_cumulative_object_byte_limit() {
    let temporary = TempDirectory::new();
    let repository = temporary.directory("repository");
    let source = temporary.directory("archive-source");
    let archive_root = temporary.directory("archives");
    export_named_archive(&source, &archive_root, "aaa");
    export_named_archive(&source, &archive_root, "bbb");

    let service = service_with_archive_root(&repository, &archive_root);
    let error = service
        .list_archives_with_limits(synapse_local_service::ArchiveListLimits {
            max_root_entries: ARCHIVE_LIST_LIMITS.max_root_entries,
            inspection: synapse_core::ArchiveInspectionLimits {
                max_objects: 2,
                max_object_bytes: b"archive listing fixture blob".len() as u64,
            },
        })
        .unwrap_err();
    assert_eq!(error.code(), "resource_limit");
}

#[test]
fn list_archives_charges_an_invalid_archive_reserved_inventory() {
    let temporary = TempDirectory::new();
    let repository = temporary.directory("repository");
    let source = temporary.directory("archive-source");
    let archive_root = temporary.directory("archives");
    export_named_archive(&source, &archive_root, "aaa-invalid");
    export_named_archive(&source, &archive_root, "bbb-valid");
    fs::write(
        archive_root.join("aaa-invalid/objects/00000000"),
        b"wrong length",
    )
    .unwrap();

    let service = service_with_archive_root(&repository, &archive_root);
    let error = service
        .list_archives_with_limits(synapse_local_service::ArchiveListLimits {
            max_root_entries: ARCHIVE_LIST_LIMITS.max_root_entries,
            inspection: synapse_core::ArchiveInspectionLimits {
                max_objects: 1,
                max_object_bytes: ARCHIVE_LIST_LIMITS.inspection.max_object_bytes,
            },
        })
        .unwrap_err();
    assert_eq!(error.code(), "resource_limit");
}

#[test]
fn list_archives_fails_closed_when_root_entries_exceed_the_limit() {
    let temporary = TempDirectory::new();
    let repository = temporary.directory("repository");
    let archive_root = temporary.directory("archives");
    fs::create_dir(archive_root.join("aaa")).unwrap();
    fs::create_dir(archive_root.join("bbb")).unwrap();

    let service = service_with_archive_root(&repository, &archive_root);
    let error = service
        .list_archives_with_limits(synapse_local_service::ArchiveListLimits {
            max_root_entries: 1,
            inspection: ARCHIVE_LIST_LIMITS.inspection,
        })
        .unwrap_err();
    assert_eq!(error.code(), "resource_limit");
}

#[test]
fn list_archives_root_entry_limit_counts_every_raw_entry_deterministically() {
    // The root-entry limit must count every raw `readdir` entry (slug or
    // not), not only entries accepted as candidate archives. Otherwise
    // whether a request over budget fails or succeeds would depend on
    // readdir order whenever accepted and excluded entries coexist near the
    // limit. Exercise this with 2 slug directories and 2 dot-prefixed
    // (non-slug) directories against a limit of 3: total raw entries (4)
    // exceed the limit regardless of order, so this must always fail
    // closed.
    let temporary = TempDirectory::new();
    let repository = temporary.directory("repository");
    let archive_root = temporary.directory("archives");
    fs::create_dir(archive_root.join("aaa")).unwrap();
    fs::create_dir(archive_root.join("bbb")).unwrap();
    fs::create_dir(archive_root.join(".staging-one")).unwrap();
    fs::create_dir(archive_root.join(".staging-two")).unwrap();

    let service = service_with_archive_root(&repository, &archive_root);
    let error = service
        .list_archives_with_limits(synapse_local_service::ArchiveListLimits {
            max_root_entries: 3,
            inspection: ARCHIVE_LIST_LIMITS.inspection,
        })
        .unwrap_err();
    assert_eq!(error.code(), "resource_limit");
}

#[test]
fn list_archives_root_entry_limit_admits_the_exact_raw_entry_count() {
    // The inverse of the above: when the raw entry count (including
    // non-slug entries) is exactly at the limit, the request must succeed.
    let temporary = TempDirectory::new();
    let repository = temporary.directory("repository");
    let archive_root = temporary.directory("archives");
    fs::create_dir(archive_root.join("aaa")).unwrap();
    fs::create_dir(archive_root.join("bbb")).unwrap();
    fs::create_dir(archive_root.join(".staging-one")).unwrap();
    fs::create_dir(archive_root.join(".staging-two")).unwrap();

    let service = service_with_archive_root(&repository, &archive_root);
    let list = service
        .list_archives_with_limits(synapse_local_service::ArchiveListLimits {
            max_root_entries: 4,
            inspection: ARCHIVE_LIST_LIMITS.inspection,
        })
        .unwrap();
    assert_eq!(
        list.archives
            .iter()
            .map(|entry| entry.archive_name.as_str())
            .collect::<Vec<_>>(),
        vec!["aaa", "bbb"]
    );
}

#[test]
fn list_archives_excludes_a_slug_named_plain_file() {
    let temporary = TempDirectory::new();
    let repository = temporary.directory("repository");
    let archive_root = temporary.directory("archives");
    // A slug-named regular file directly under the root must never be
    // listed, even though its name alone would pass `is_slug`.
    fs::write(archive_root.join("backup"), b"not a directory").unwrap();

    let service = service_with_archive_root(&repository, &archive_root);
    let list = service.list_archives().unwrap();
    assert_eq!(
        list,
        ArchiveList {
            archives: Vec::new()
        }
    );
}

#[cfg(unix)]
#[test]
fn list_archives_does_not_follow_a_slug_named_symlink() {
    let temporary = TempDirectory::new();
    let repository = temporary.directory("repository");
    let source = temporary.directory("archive-source");
    let archive_root = temporary.directory("archives");
    let real_archive_dir = temporary.directory("outside-archives");
    export_named_archive(&source, &real_archive_dir, "real-archive");

    // A slug-named symlink under the root, even one pointing at a real,
    // valid exported archive directory, must not be followed or admitted
    // as a candidate archive.
    std::os::unix::fs::symlink(
        real_archive_dir.join("real-archive"),
        archive_root.join("linked-archive"),
    )
    .unwrap();

    let service = service_with_archive_root(&repository, &archive_root);
    let list = service.list_archives().unwrap();
    assert_eq!(
        list,
        ArchiveList {
            archives: Vec::new()
        }
    );
}
