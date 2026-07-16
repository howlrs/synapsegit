use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use synapse_core::{FsckLimits, Repository, TombstoneScanLimits};
use synapse_local_service::{
    ArchiveResult, ArchiveResultKind, FsckResult, LocalService, MAINTENANCE_FSCK_LIMITS,
    OperationAccepted, OperationKind, OperationResult, OperationState, OperationStatus,
    ProjectConfirmation, ProjectRegistration,
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
