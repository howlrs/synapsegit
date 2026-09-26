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
    assert!(item.reason.as_deref().unwrap().contains("regular file"));
}
