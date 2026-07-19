use serde_json::Value as JsonValue;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};
use synapse_artifact::{
    ArtifactEntryKind, ArtifactError, ArtifactLimits, ArtifactManifestEntry,
    ArtifactSourceAttribution, RegularFileManifest, capabilities_v1, map_regular_files,
};
use synapse_core::Repository;

static NEXT_TEMP: AtomicU64 = AtomicU64::new(1);

struct TempRepository {
    path: PathBuf,
}

impl TempRepository {
    fn new(label: &str) -> Self {
        let serial = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "synapse-artifact-{label}-{}-{nanos}-{serial}",
            std::process::id()
        ));
        Self { path }
    }

    fn open(&self) -> Repository {
        Repository::open(&self.path).unwrap()
    }
}

impl Drop for TempRepository {
    fn drop(&mut self) {
        if self.path.starts_with(std::env::temp_dir()) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}

fn entry(path: &str, bytes: &[u8]) -> ArtifactManifestEntry {
    ArtifactManifestEntry::regular_file(path, bytes.to_vec())
}

#[test]
fn capabilities_match_the_frozen_v1_fixture() {
    let fixture = fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../spec/application/generic-artifact/v1/capabilities.json"),
    )
    .unwrap();
    let expected: JsonValue = serde_json::from_str(&fixture).unwrap();
    let actual = serde_json::to_value(capabilities_v1()).unwrap();
    assert_eq!(actual, expected);
}

#[test]
fn mapping_is_independent_of_input_order_and_repository_location() {
    let limits = ArtifactLimits::default();
    let forward = RegularFileManifest::from_entries(
        [
            entry("index.html", b"<!doctype html><title>LP</title>"),
            entry("assets/css/site.css", b"body { color: #123; }"),
            entry("assets/logo.bin", &[0, 1, 2, 255]),
        ],
        limits,
    )
    .unwrap();
    let reverse = RegularFileManifest::from_entries(
        [
            entry("assets/logo.bin", &[0, 1, 2, 255]),
            entry("assets/css/site.css", b"body { color: #123; }"),
            entry("index.html", b"<!doctype html><title>LP</title>"),
        ],
        limits,
    )
    .unwrap();
    let first_temp = TempRepository::new("first");
    let second_temp = TempRepository::new("second");
    let first = map_regular_files(&first_temp.open(), &forward).unwrap();
    let second = map_regular_files(&second_temp.open(), &reverse).unwrap();

    assert_eq!(first.site_tree_oid, second.site_tree_oid);
    assert_eq!(first.files, second.files);
    assert_eq!(first.total_bytes, second.total_bytes);
}

#[test]
fn mapping_builds_nested_manifest_trees_without_updating_refs() {
    let temp = TempRepository::new("nested");
    let repository = temp.open();
    let manifest = RegularFileManifest::from_entries(
        [
            entry("index.html", b"home"),
            entry("assets/css/site.css", b"css"),
        ],
        ArtifactLimits::default(),
    )
    .unwrap();
    let mapped = map_regular_files(&repository, &manifest).unwrap();

    assert!(repository.refs().list().unwrap().is_empty());
    let root = repository
        .objects()
        .get_verified(&mapped.site_tree_oid)
        .unwrap()
        .unwrap();
    let entries = root.structured().unwrap().get("entries").unwrap();
    assert_eq!(
        entries
            .get("index.html")
            .unwrap()
            .get("entry_kind")
            .unwrap()
            .as_str(),
        Some("blob")
    );
    assert_eq!(
        entries
            .get("assets")
            .unwrap()
            .get("entry_kind")
            .unwrap()
            .as_str(),
        Some("tree")
    );
}

#[test]
fn rejects_non_regular_entries_before_a_manifest_can_be_mapped() {
    for kind in [
        ArtifactEntryKind::Directory,
        ArtifactEntryKind::Symlink,
        ArtifactEntryKind::Socket,
        ArtifactEntryKind::Fifo,
        ArtifactEntryKind::Device,
        ArtifactEntryKind::Other,
    ] {
        let error = RegularFileManifest::from_entries(
            [ArtifactManifestEntry::unsupported("unsafe", kind)],
            ArtifactLimits::default(),
        )
        .unwrap_err();
        assert!(matches!(
            error,
            ArtifactError::UnsupportedEntryKind { kind: actual, .. } if actual == kind
        ));
    }
}

#[test]
fn rejects_unsafe_and_non_normalized_paths() {
    for path in [
        "",
        "/etc/passwd",
        "../outside",
        "a/../outside",
        "a/./b",
        "a//b",
        "a/",
        "C:/secret",
        "assets\\secret",
        "line\nfeed",
        "assets/trailing.",
        "assets/trailing ",
        "assets/name:stream",
        "assets/a?.txt",
        "assets/CON",
        "assets/com1.txt",
        "assets/Lpt9.log",
        "assets/COM¹",
        "assets/com².txt",
        "assets/Com³.tar.gz",
        "assets/LPT¹",
        "assets/lpt².log",
        "assets/Lpt³.data",
        "assets/bidi\u{202e}spoof.txt",
    ] {
        let error =
            RegularFileManifest::from_entries([entry(path, b"x")], ArtifactLimits::default())
                .unwrap_err();
        assert_eq!(error.code(), "artifact_path_invalid", "path={path:?}");
    }

    let decomposed = "assets/cafe\u{301}.txt";
    let error =
        RegularFileManifest::from_entries([entry(decomposed, b"x")], ArtifactLimits::default())
            .unwrap_err();
    assert!(matches!(error, ArtifactError::PathNotNfc(_)));
}

#[test]
fn rejects_duplicate_case_and_file_directory_collisions() {
    let duplicate = RegularFileManifest::from_entries(
        [entry("a.txt", b"1"), entry("a.txt", b"2")],
        ArtifactLimits::default(),
    )
    .unwrap_err();
    assert!(matches!(duplicate, ArtifactError::DuplicatePath(_)));

    let case = RegularFileManifest::from_entries(
        [entry("Assets/a.txt", b"1"), entry("assets/b.txt", b"2")],
        ArtifactLimits::default(),
    )
    .unwrap_err();
    assert!(matches!(case, ArtifactError::PathCollision { .. }));

    let prefix = RegularFileManifest::from_entries(
        [entry("assets", b"file"), entry("assets/site.css", b"css")],
        ArtifactLimits::default(),
    )
    .unwrap_err();
    assert!(matches!(
        prefix,
        ArtifactError::FileDirectoryConflict { .. }
    ));
}

#[test]
fn enforces_each_resource_limit() {
    let base = ArtifactLimits {
        max_files: 2,
        max_file_bytes: 4,
        max_total_bytes: 6,
        max_path_bytes: 12,
        max_depth: 2,
    };
    let cases = [
        (
            vec![entry("a", b"1"), entry("b", b"2"), entry("c", b"3")],
            base,
        ),
        (vec![entry("a", b"12345")], base),
        (vec![entry("a", b"1234"), entry("b", b"1234")], base),
        (vec![entry("path-is-too-long", b"1")], base),
        (vec![entry("a/b/c", b"1")], base),
    ];
    for (entries, limits) in cases {
        let error = RegularFileManifest::from_entries(entries, limits).unwrap_err();
        assert_eq!(error.code(), "resource_limit");
    }
}

#[test]
fn caller_supplied_attribution_is_never_verified_execution() {
    assert!(!ArtifactSourceAttribution::CallerSuppliedAiAttributed.execution_verified());
    assert!(serde_json::from_str::<ArtifactSourceAttribution>("\"trusted_executor\"").is_err());
}

#[test]
fn debug_and_display_do_not_expose_paths_contents_or_object_ids() {
    let manifest_entry = ArtifactManifestEntry::regular_file(
        "SECRET-PATH-CANARY.txt",
        b"SECRET-CONTENT-CANARY".to_vec(),
    );
    let entry_debug = format!("{manifest_entry:?}");
    assert!(!entry_debug.contains("SECRET-PATH"));
    assert!(!entry_debug.contains("SECRET-CONTENT"));

    let manifest =
        RegularFileManifest::from_entries([manifest_entry], ArtifactLimits::default()).unwrap();
    let manifest_debug = format!("{manifest:?}");
    assert!(!manifest_debug.contains("SECRET-PATH"));
    assert!(!manifest_debug.contains("SECRET-CONTENT"));

    let temporary = TempRepository::new("redacted-debug");
    let mapped = map_regular_files(&temporary.open(), &manifest).unwrap();
    let mapped_debug = format!("{mapped:?}");
    assert!(!mapped_debug.contains("SECRET-PATH"));
    assert!(!mapped_debug.contains("sg-oid"));

    let error = RegularFileManifest::from_entries(
        [entry("../SECRET-ERROR-PATH", b"x")],
        ArtifactLimits::default(),
    )
    .unwrap_err();
    assert!(!format!("{error:?}").contains("SECRET-ERROR"));
    assert!(!error.to_string().contains("SECRET-ERROR"));
}
