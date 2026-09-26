use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use synapse_local_service::{ImageRole, LocalService, ProjectRegistration};

static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

struct TempDirectory(PathBuf);

impl TempDirectory {
    fn new() -> Self {
        let id = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "synapse-local-inbox-test-{}-{id}",
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

#[test]
fn manifest_last_listing_and_staging_freeze_verified_bytes() {
    let temporary = TempDirectory::new();
    let repository = temporary.directory("repository");
    let inbox = temporary.directory("inbox");
    let candidate = inbox.join("first");
    fs::create_dir(&candidate).unwrap();
    fs::write(candidate.join("original"), b"original-a").unwrap();
    fs::write(candidate.join("current"), b"current-a").unwrap();
    fs::write(candidate.join("ai-output"), b"output-a").unwrap();
    let mut roots = BTreeMap::new();
    roots.insert("project".to_owned(), inbox);
    let service = LocalService::new([ProjectRegistration::new("project", "Project", repository)])
        .unwrap()
        .with_import_roots(roots)
        .unwrap();
    assert!(
        service
            .list_import_inbox("project")
            .unwrap()
            .items
            .is_empty()
    );
    fs::write(candidate.join("manifest.json"), r#"{"version":"synapsegit-import-inbox-v1","original":{"name":"original","size":10},"current":{"name":"current","size":9},"ai_output":{"name":"ai-output","size":8},"metadata":{"subject_label":"Subject","creator_name":"Creator"}}"#).unwrap();
    let items = service.list_import_inbox("project").unwrap();
    assert_eq!(items.items.len(), 1);
    assert!(items.items[0].ready);
    let staged = service.stage_import_inbox("project", "first").unwrap();
    fs::write(candidate.join("original"), b"replaced!!").unwrap();
    assert_eq!(
        service
            .staged_import_image("project", &staged.stage_id, ImageRole::Original)
            .unwrap()
            .bytes,
        b"original-a"
    );
    let original_oid = service
        .staged_import_image("project", &staged.stage_id, ImageRole::Original)
        .unwrap()
        .blob_oid;
    let current_oid = service
        .staged_import_image("project", &staged.stage_id, ImageRole::Current)
        .unwrap()
        .blob_oid;
    assert_ne!(original_oid, current_oid);
}

#[cfg(unix)]
#[test]
fn manifest_symlink_is_rejected() {
    use std::os::unix::fs::symlink;
    let temporary = TempDirectory::new();
    let repository = temporary.directory("repository");
    let inbox = temporary.directory("inbox");
    let candidate = inbox.join("first");
    fs::create_dir(&candidate).unwrap();
    symlink("/etc/passwd", candidate.join("manifest.json")).unwrap();
    let mut roots = BTreeMap::new();
    roots.insert("project".to_owned(), inbox);
    let service = LocalService::new([ProjectRegistration::new("project", "Project", repository)])
        .unwrap()
        .with_import_roots(roots)
        .unwrap();
    let item = &service.list_import_inbox("project").unwrap().items[0];
    assert!(!item.ready);
    assert!(
        item.reason
            .as_deref()
            .unwrap()
            .contains("without following links")
    );
}

#[test]
fn import_root_is_rejected_when_it_overlaps_any_registered_repository() {
    let temporary = TempDirectory::new();
    let repository_a = temporary.directory("repository-a");
    let repository_b = temporary.directory("repository-b");
    let mut roots = BTreeMap::new();
    roots.insert("a".to_owned(), repository_b.clone());
    let error = LocalService::new([
        ProjectRegistration::new("a", "A", repository_a),
        ProjectRegistration::new("b", "B", repository_b),
    ])
    .unwrap()
    .with_import_roots(roots)
    .unwrap_err();
    assert_eq!(error.code(), "local_request_denied");
}

#[test]
fn cancelling_a_stage_releases_it_immediately() {
    let temporary = TempDirectory::new();
    let repository = temporary.directory("repository");
    let inbox = temporary.directory("inbox");
    let candidate = inbox.join("first");
    fs::create_dir(&candidate).unwrap();
    for (name, bytes) in [
        ("original", b"one".as_slice()),
        ("current", b"two".as_slice()),
        ("ai-output", b"three".as_slice()),
    ] {
        fs::write(candidate.join(name), bytes).unwrap();
    }
    fs::write(candidate.join("manifest.json"), r#"{"version":"synapsegit-import-inbox-v1","original":{"name":"original","size":3},"current":{"name":"current","size":3},"ai_output":{"name":"ai-output","size":5},"metadata":{"subject_label":"Subject","creator_name":"Creator"}}"#).unwrap();
    let mut roots = BTreeMap::new();
    roots.insert("project".to_owned(), inbox);
    let service = LocalService::new([ProjectRegistration::new("project", "Project", repository)])
        .unwrap()
        .with_import_roots(roots)
        .unwrap();
    let stage = service.stage_import_inbox("project", "first").unwrap();
    service
        .cancel_staged_import_inbox("project", &stage.stage_id)
        .unwrap();
    assert!(
        service
            .staged_import_image("project", &stage.stage_id, ImageRole::Original)
            .is_err()
    );
}

#[cfg(unix)]
#[test]
fn fifo_leaf_is_rejected_without_blocking() {
    use std::os::unix::fs::FileTypeExt;
    let temporary = TempDirectory::new();
    let repository = temporary.directory("repository");
    let inbox = temporary.directory("inbox");
    let candidate = inbox.join("first");
    fs::create_dir(&candidate).unwrap();
    let fifo = candidate.join("original");
    assert!(
        std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .unwrap()
            .success()
    );
    assert!(fs::symlink_metadata(&fifo).unwrap().file_type().is_fifo());
    fs::write(candidate.join("current"), b"two").unwrap();
    fs::write(candidate.join("ai-output"), b"three").unwrap();
    fs::write(candidate.join("manifest.json"), r#"{"version":"synapsegit-import-inbox-v1","original":{"name":"original","size":3},"current":{"name":"current","size":3},"ai_output":{"name":"ai-output","size":5},"metadata":{"subject_label":"Subject","creator_name":"Creator"}}"#).unwrap();
    let mut roots = BTreeMap::new();
    roots.insert("project".to_owned(), inbox);
    let service = LocalService::new([ProjectRegistration::new("project", "Project", repository)])
        .unwrap()
        .with_import_roots(roots)
        .unwrap();
    assert!(service.stage_import_inbox("project", "first").is_err());
}

#[test]
fn missing_hash_mismatch_and_declared_oversize_files_are_rejected() {
    let temporary = TempDirectory::new();
    let repository = temporary.directory("repository");
    let inbox = temporary.directory("inbox");
    let candidate = inbox.join("first");
    fs::create_dir(&candidate).unwrap();
    fs::write(candidate.join("original"), b"one").unwrap();
    fs::write(candidate.join("current"), b"two").unwrap();
    fs::write(candidate.join("ai-output"), b"three").unwrap();
    let mut roots = BTreeMap::new();
    roots.insert("project".to_owned(), inbox);
    let service = LocalService::new([ProjectRegistration::new("project", "Project", repository)])
        .unwrap()
        .with_import_roots(roots)
        .unwrap();

    fs::write(candidate.join("manifest.json"), r#"{"version":"synapsegit-import-inbox-v1","original":{"name":"missing","size":3},"current":{"name":"current","size":3},"ai_output":{"name":"ai-output","size":5},"metadata":{"subject_label":"Subject","creator_name":"Creator"}}"#).unwrap();
    assert!(service.stage_import_inbox("project", "first").is_err());

    fs::write(candidate.join("manifest.json"), r#"{"version":"synapsegit-import-inbox-v1","original":{"name":"original","size":3,"sha256":"0000000000000000000000000000000000000000000000000000000000000000"},"current":{"name":"current","size":3},"ai_output":{"name":"ai-output","size":5},"metadata":{"subject_label":"Subject","creator_name":"Creator"}}"#).unwrap();
    assert!(service.stage_import_inbox("project", "first").is_err());

    fs::write(candidate.join("manifest.json"), r#"{"version":"synapsegit-import-inbox-v1","original":{"name":"original","size":67108865},"current":{"name":"current","size":3},"ai_output":{"name":"ai-output","size":5},"metadata":{"subject_label":"Subject","creator_name":"Creator"}}"#).unwrap();
    assert!(service.stage_import_inbox("project", "first").is_err());
}

#[test]
fn dropping_service_removes_private_staging_directory() {
    let temporary = TempDirectory::new();
    let repository = temporary.directory("repository");
    let inbox = temporary.directory("inbox");
    let candidate = inbox.join("first");
    fs::create_dir(&candidate).unwrap();
    for (name, bytes) in [
        ("original", b"one".as_slice()),
        ("current", b"two".as_slice()),
        ("ai-output", b"three".as_slice()),
    ] {
        fs::write(candidate.join(name), bytes).unwrap();
    }
    fs::write(candidate.join("manifest.json"), r#"{"version":"synapsegit-import-inbox-v1","original":{"name":"original","size":3},"current":{"name":"current","size":3},"ai_output":{"name":"ai-output","size":5},"metadata":{"subject_label":"Subject","creator_name":"Creator"}}"#).unwrap();
    let stage_path;
    {
        let mut roots = BTreeMap::new();
        roots.insert("project".to_owned(), inbox);
        let service =
            LocalService::new([ProjectRegistration::new("project", "Project", repository)])
                .unwrap()
                .with_import_roots(roots)
                .unwrap();
        let stage = service.stage_import_inbox("project", "first").unwrap();
        stage_path = std::env::temp_dir().join(format!("synapse-local-inbox-{}", stage.stage_id));
        assert!(stage_path.is_dir());
    }
    assert!(!stage_path.exists());
}

#[test]
fn staging_ninth_candidate_hits_the_process_limit_before_copying_it() {
    let temporary = TempDirectory::new();
    let repository = temporary.directory("repository");
    let inbox = temporary.directory("inbox");
    for index in 0..9 {
        let candidate = inbox.join(format!("candidate-{index}"));
        fs::create_dir(&candidate).unwrap();
        for (name, bytes) in [
            ("original", b"one".as_slice()),
            ("current", b"two".as_slice()),
            ("ai-output", b"three".as_slice()),
        ] {
            fs::write(candidate.join(name), bytes).unwrap();
        }
        fs::write(candidate.join("manifest.json"), r#"{"version":"synapsegit-import-inbox-v1","original":{"name":"original","size":3},"current":{"name":"current","size":3},"ai_output":{"name":"ai-output","size":5},"metadata":{"subject_label":"Subject","creator_name":"Creator"}}"#).unwrap();
    }
    let mut roots = BTreeMap::new();
    roots.insert("project".to_owned(), inbox);
    let service = LocalService::new([ProjectRegistration::new("project", "Project", repository)])
        .unwrap()
        .with_import_roots(roots)
        .unwrap();
    for index in 0..8 {
        service
            .stage_import_inbox("project", &format!("candidate-{index}"))
            .unwrap();
    }
    let error = service
        .stage_import_inbox("project", "candidate-8")
        .unwrap_err();
    assert_eq!(error.code(), "resource_limit");
}

#[test]
fn listing_skips_only_manifest_last_directories_and_reports_other_slug_entries() {
    let temporary = TempDirectory::new();
    let repository = temporary.directory("repository");
    let inbox = temporary.directory("inbox");
    fs::create_dir(inbox.join("writing")).unwrap();
    fs::write(inbox.join("plain-file"), b"not a candidate directory").unwrap();
    let mut roots = BTreeMap::new();
    roots.insert("project".to_owned(), inbox);
    let service = LocalService::new([ProjectRegistration::new("project", "Project", repository)])
        .unwrap()
        .with_import_roots(roots)
        .unwrap();
    let items = service.list_import_inbox("project").unwrap().items;
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].slug, "plain-file");
    assert!(!items[0].ready);
    assert!(
        items[0]
            .reason
            .as_deref()
            .unwrap()
            .contains("regular directory")
    );
}

#[cfg(unix)]
#[test]
fn listing_reports_candidate_directory_symlinks_as_rejected() {
    use std::os::unix::fs::symlink;
    let temporary = TempDirectory::new();
    let repository = temporary.directory("repository");
    let inbox = temporary.directory("inbox");
    let target = temporary.directory("outside");
    symlink(target, inbox.join("linked")).unwrap();
    let mut roots = BTreeMap::new();
    roots.insert("project".to_owned(), inbox);
    let service = LocalService::new([ProjectRegistration::new("project", "Project", repository)])
        .unwrap()
        .with_import_roots(roots)
        .unwrap();
    let items = service.list_import_inbox("project").unwrap().items;
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].slug, "linked");
    assert!(!items[0].ready);
    assert!(
        items[0]
            .reason
            .as_deref()
            .unwrap()
            .contains("regular directory")
    );
}

#[test]
fn manifest_leaf_names_reject_escape_and_platform_separators() {
    let temporary = TempDirectory::new();
    let repository = temporary.directory("repository");
    let inbox = temporary.directory("inbox");
    let candidate = inbox.join("first");
    fs::create_dir(&candidate).unwrap();
    fs::write(candidate.join("current"), b"two").unwrap();
    fs::write(candidate.join("ai-output"), b"three").unwrap();
    let mut roots = BTreeMap::new();
    roots.insert("project".to_owned(), inbox);
    let service = LocalService::new([ProjectRegistration::new("project", "Project", repository)])
        .unwrap()
        .with_import_roots(roots)
        .unwrap();
    for name in [".", "..", r"original\\other", "original\u{0}"] {
        let manifest = format!(
            r#"{{"version":"synapsegit-import-inbox-v1","original":{{"name":"{name}","size":3}},"current":{{"name":"current","size":3}},"ai_output":{{"name":"ai-output","size":5}},"metadata":{{"subject_label":"Subject","creator_name":"Creator"}}}}"#
        );
        fs::write(candidate.join("manifest.json"), manifest).unwrap();
        assert!(
            service.stage_import_inbox("project", "first").is_err(),
            "{name:?}"
        );
    }
}

#[test]
fn archive_root_rejects_existing_import_root_regardless_of_builder_order() {
    let temporary = TempDirectory::new();
    let repository = temporary.directory("repository");
    let shared = temporary.directory("shared");
    let mut roots = BTreeMap::new();
    roots.insert("project".to_owned(), shared.clone());
    let service = LocalService::new([ProjectRegistration::new("project", "Project", repository)])
        .unwrap()
        .with_import_roots(roots)
        .unwrap();
    assert_eq!(
        service.with_archive_root(shared).unwrap_err().code(),
        "local_request_denied"
    );
}

#[test]
fn archive_root_canonicalizes_relative_aliases_before_overlap_checking() {
    let temporary = TempDirectory::new();
    let repository = temporary.directory("repository");
    let mut roots = BTreeMap::new();
    roots.insert("project".to_owned(), std::env::current_dir().unwrap());
    let service = LocalService::new([ProjectRegistration::new("project", "Project", repository)])
        .unwrap()
        .with_import_roots(roots)
        .unwrap();
    assert_eq!(
        service
            .with_archive_root(PathBuf::from("."))
            .unwrap_err()
            .code(),
        "local_request_denied"
    );
}
