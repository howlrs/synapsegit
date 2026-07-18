use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use synapse_canonical::{canonical_bytes, parse_strict};
use synapse_creator::{
    CreatorBeginOptions, CreatorDisposition, CreatorRunOptions, begin_creator_session,
    run_creator_session,
};
use synapse_publication::{
    BundleManifest, ChecksumsDocument, DEFAULT_MAX_SESSIONS, ExportOptions, OutputTarget,
    PresentationInput, ProjectionOptions, PublicationError, PublicationVisibility,
    SessionPresentationInput, ValueOrigin, build_public_projection, export_bundle, verify_bundle,
};

static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

struct TempDirectory(PathBuf);

impl TempDirectory {
    fn new() -> Self {
        let sequence = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "synapse-publication-test-{}-{sequence}",
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

fn create_input_fixture(root: &Path) -> (PathBuf, PathBuf, PathBuf) {
    let source = root.join("inputs");
    fs::create_dir(&source).unwrap();
    let original = source.join("original.bin");
    let current = source.join("current.bin");
    let proposal = source.join("proposal.bin");
    fs::write(&original, b"RAW_ORIGINAL_SECRET_91d6").unwrap();
    fs::write(&current, b"RAW_CURRENT_SECRET_82e5").unwrap();
    fs::write(&proposal, b"RAW_PROPOSAL_SECRET_73f4").unwrap();
    (original, current, proposal)
}

fn create_three_decision_fixture(root: &Path) {
    let (original, current, proposal) = create_input_fixture(root);

    for (session, disposition) in [
        ("adopt-story", CreatorDisposition::Adopt),
        ("defer-story", CreatorDisposition::Defer),
        ("reject-story", CreatorDisposition::Reject),
    ] {
        run_creator_session(&CreatorRunOptions {
            repository: root.join("repo"),
            session: session.into(),
            original_image: original.clone(),
            current_image: current.clone(),
            ai_output: proposal.clone(),
            subject_label: "private.person+projection@example.invalid".into(),
            creator_name: "private.person+projection@example.invalid".into(),
            disposition,
            rationale: Some(format!(
                "PRIVATE_RATIONALE_{session}_TOKEN_5c14 GH_TOKEN=secret"
            )),
        })
        .unwrap();
    }
}

fn create_incomplete_fixture(root: &Path) {
    let (original, current, proposal) = create_input_fixture(root);
    drop(
        begin_creator_session(&CreatorBeginOptions {
            repository: root.join("repo"),
            session: "pending-story".into(),
            original_image: original,
            current_image: current,
            ai_output: proposal,
            subject_label: "private.pending@example.invalid".into(),
            creator_name: "Private Pending Creator".into(),
        })
        .unwrap(),
    );
}

fn projection_options(repository: PathBuf) -> ProjectionOptions {
    let mut presentation = PresentationInput {
        title: Some("A <script>alert('title')</script> history".into()),
        summary: Some("Public summary: [remote](javascript:alert(1))".into()),
        creator_display_name: Some("Public creator".into()),
        proposal_agent_display_name: None,
        sessions: BTreeMap::new(),
    };
    presentation.sessions.insert(
        "reject-story".into(),
        SessionPresentationInput {
            title: Some("A useful rejected direction".into()),
            public_decision_note: Some(
                "This is separately supplied public context, not the private rationale.".into(),
            ),
            original_caption: Some("First recorded state".into()),
            current_caption: None,
            proposal_caption: Some("Exploration retained for future readers".into()),
        },
    );
    ProjectionOptions {
        repository,
        session: None,
        visibility: PublicationVisibility::Public,
        presentation,
        max_sessions: 20,
    }
}

fn export(root: &TempDirectory, name: &str, target: OutputTarget) -> PathBuf {
    let destination = root.join(name);
    export_bundle(&ExportOptions {
        projection: projection_options(root.join("repo")),
        destination: destination.clone(),
        target,
    })
    .unwrap();
    destination
}

#[test]
fn exports_adopt_reject_and_defer_without_private_or_raw_source_material() {
    let temporary = TempDirectory::new();
    create_three_decision_fixture(&temporary.0);
    let repository_before = snapshot_tree(&temporary.join("repo"));

    let bundle = export(&temporary, "github-bundle", OutputTarget::Github);
    let verified = verify_bundle(&bundle).unwrap();
    assert_eq!(verified.manifest.target, OutputTarget::Github);
    assert_eq!(verified.manifest.visibility, PublicationVisibility::Public);
    assert_eq!(verified.manifest.network_operations, 0);

    let projection: serde_json::Value =
        serde_json::from_slice(&fs::read(bundle.join("projection.json")).unwrap()).unwrap();
    let dispositions = projection["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|session| session["human_decision"]["disposition"].as_str().unwrap())
        .collect::<BTreeSet<_>>();
    assert_eq!(dispositions, BTreeSet::from(["adopt", "defer", "reject"]));
    assert_eq!(projection["publication"]["raw_assets_included"], false);
    assert_eq!(
        projection["publication"]["source_private_rationale_included"],
        false
    );
    assert_eq!(
        projection["publication"]["training_use_policy"],
        "prohibited"
    );

    let all_bundle_bytes = bundle_bytes(&bundle);
    for secret in [
        "PRIVATE_RATIONALE_",
        "GH_TOKEN=secret",
        "private.person+projection@example.invalid",
        "RAW_ORIGINAL_SECRET_91d6",
        "RAW_CURRENT_SECRET_82e5",
        "RAW_PROPOSAL_SECRET_73f4",
        temporary.join("repo").to_str().unwrap(),
    ] {
        assert!(
            !all_bundle_bytes.contains(secret),
            "bundle leaked canary {secret:?}"
        );
    }
    assert!(all_bundle_bytes.contains("separately supplied public context"));
    assert_eq!(snapshot_tree(&temporary.join("repo")), repository_before);
}

#[test]
fn renders_untrusted_presentation_as_text_without_active_html_or_markdown() {
    let temporary = TempDirectory::new();
    create_three_decision_fixture(&temporary.0);
    let bundle = export(&temporary, "escaped-bundle", OutputTarget::Github);
    let html = fs::read_to_string(bundle.join("index.html")).unwrap();
    let story = fs::read_to_string(bundle.join("story.md")).unwrap();

    assert!(html.contains("&lt;script&gt;alert(&#39;title&#39;)&lt;/script&gt;"));
    assert!(!html.contains("<script"));
    assert!(!html.contains("href=\"javascript:"));
    assert!(!html.contains("<iframe"));
    assert!(!html.contains("<object"));
    assert!(!html.contains("<embed"));
    assert!(!html.contains("<form"));
    assert!(story.contains("\\<script\\>alert"));
    assert!(story.contains("\\[remote\\]\\(javascript:alert\\(1\\)\\)"));
    assert!(!story.contains("[remote](javascript:"));
}

#[test]
fn provider_targets_share_identical_projection_and_human_story() {
    let temporary = TempDirectory::new();
    create_three_decision_fixture(&temporary.0);
    let synapse = export(&temporary, "synapse-bundle", OutputTarget::Synapse);
    let github = export(&temporary, "github-bundle", OutputTarget::Github);

    for path in ["projection.json", "story.md", "index.html"] {
        assert_eq!(
            fs::read(synapse.join(path)).unwrap(),
            fs::read(github.join(path)).unwrap()
        );
    }
    assert_ne!(
        fs::read(synapse.join("manifest.json")).unwrap(),
        fs::read(github.join("manifest.json")).unwrap()
    );
    let projection = fs::read(synapse.join("projection.json")).unwrap();
    assert_eq!(
        canonical_bytes(&parse_strict(&projection).unwrap()).unwrap(),
        projection
    );
}

#[test]
fn repeated_exports_are_byte_deterministic_and_never_replace() {
    let temporary = TempDirectory::new();
    create_three_decision_fixture(&temporary.0);
    let first = export(&temporary, "first", OutputTarget::Github);
    let second = export(&temporary, "second", OutputTarget::Github);
    assert_eq!(snapshot_tree(&first), snapshot_tree(&second));

    let sentinel = temporary.join("occupied");
    fs::create_dir(&sentinel).unwrap();
    fs::write(sentinel.join("keep.txt"), b"keep").unwrap();
    let error = export_bundle(&ExportOptions {
        projection: projection_options(temporary.join("repo")),
        destination: sentinel.clone(),
        target: OutputTarget::Github,
    })
    .unwrap_err();
    assert!(matches!(error, PublicationError::DestinationExists(_)));
    assert_eq!(fs::read(sentinel.join("keep.txt")).unwrap(), b"keep");
}

#[test]
fn verification_rejects_a_checksummed_human_view_that_does_not_render_from_projection() {
    let temporary = TempDirectory::new();
    create_three_decision_fixture(&temporary.0);
    let bundle = export(&temporary, "bundle", OutputTarget::Github);
    let replacement = b"# Internally checksummed but semantically unrelated\n";
    fs::write(bundle.join("story.md"), replacement).unwrap();
    fs::write(bundle.join("target/README.md"), replacement).unwrap();
    reconcile_bundle_checksums(&bundle);

    let error = verify_bundle(bundle).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("do not render from projection.json")
    );
}

#[test]
fn verification_rejects_consistent_bundle_with_source_rationale_payload() {
    let temporary = TempDirectory::new();
    create_three_decision_fixture(&temporary.0);
    let bundle = export(&temporary, "bundle", OutputTarget::Github);
    let private_canary = "PRIVATE_SOURCE_RATIONALE_CANARY_7f10";

    let mut projection: serde_json::Value =
        serde_json::from_slice(&fs::read(bundle.join("projection.json")).unwrap()).unwrap();
    projection["sessions"][0]["human_decision"]["source_rationale"]["reason"] =
        serde_json::Value::String(private_canary.into());
    let projection_bytes = canonical_json(&projection);
    fs::write(bundle.join("projection.json"), &projection_bytes).unwrap();
    fs::write(bundle.join("target/projection.json"), &projection_bytes).unwrap();

    let mut manifest: BundleManifest =
        serde_json::from_slice(&fs::read(bundle.join("manifest.json")).unwrap()).unwrap();
    manifest.projection_sha256 = sha256_hex(&projection_bytes);
    write_manifest_and_reconcile_checksums(&bundle, &manifest);

    assert_eq!(
        fs::read(bundle.join("story.md")).unwrap(),
        fs::read(bundle.join("target/README.md")).unwrap()
    );
    assert_eq!(
        fs::read(bundle.join("index.html")).unwrap(),
        fs::read(bundle.join("target/index.html")).unwrap()
    );
    assert_eq!(
        fs::read(bundle.join("projection.json")).unwrap(),
        fs::read(bundle.join("target/projection.json")).unwrap()
    );

    let error = verify_bundle(bundle).unwrap_err();
    assert!(matches!(error, PublicationError::InvalidBundle(_)));
}

#[test]
fn verification_accepts_legacy_v1_bundle_without_renderer_profile() {
    let temporary = TempDirectory::new();
    create_three_decision_fixture(&temporary.0);
    let bundle = export(&temporary, "legacy-v1-bundle", OutputTarget::Github);

    let mut manifest: BundleManifest =
        serde_json::from_slice(&fs::read(bundle.join("manifest.json")).unwrap()).unwrap();
    manifest.renderer_profile = None;
    write_manifest_and_reconcile_checksums(&bundle, &manifest);

    let verified = verify_bundle(bundle).unwrap();
    assert!(verified.manifest.renderer_profile.is_none());
}

#[test]
fn verification_rejects_unknown_renderer_profile_name_or_version() {
    let temporary = TempDirectory::new();
    create_three_decision_fixture(&temporary.0);

    for (bundle_name, unknown_name) in [
        ("unknown-renderer-name", true),
        ("unknown-renderer-version", false),
    ] {
        let bundle = export(&temporary, bundle_name, OutputTarget::Github);
        let mut manifest: BundleManifest =
            serde_json::from_slice(&fs::read(bundle.join("manifest.json")).unwrap()).unwrap();
        let profile = manifest.renderer_profile.as_mut().unwrap();
        if unknown_name {
            profile.name = "org.synapsegit.unknown-publication-renderer".into();
        } else {
            profile.version = profile.version.checked_add(1).unwrap();
        }
        write_manifest_and_reconcile_checksums(&bundle, &manifest);

        let error = verify_bundle(bundle).unwrap_err();
        match error {
            PublicationError::InvalidBundle(message) => assert!(
                message.contains("renderer profile"),
                "unexpected renderer error: {message}"
            ),
            other => panic!("unexpected renderer verification error: {other}"),
        }
    }
}

#[test]
fn incomplete_only_projection_marks_source_facts_unverified() {
    let temporary = TempDirectory::new();
    create_incomplete_fixture(&temporary.0);

    let mut options = ProjectionOptions::new(temporary.join("repo"));
    options.session = Some("pending-story".into());
    let projection = build_public_projection(&options).unwrap();

    assert!(projection.sessions.is_empty());
    assert_eq!(projection.incomplete_sessions.len(), 1);
    assert_ne!(
        projection.incomplete_sessions[0].origin,
        ValueOrigin::VerifiedFromSynapse
    );
    assert!(
        projection
            .source
            .verification_scope
            .to_ascii_lowercase()
            .contains("unverified"),
        "incomplete-only verification scope must disclose unverified CAS lineage: {}",
        projection.source.verification_scope
    );
}

#[test]
fn rejects_max_sessions_above_hard_ceiling() {
    let temporary = TempDirectory::new();
    let repository = temporary.join("repo");
    drop(synapse_core::Repository::open(&repository).unwrap());

    let mut options = ProjectionOptions::new(repository);
    options.max_sessions = DEFAULT_MAX_SESSIONS + 1;
    let error = build_public_projection(&options).unwrap_err();
    match error {
        PublicationError::InvalidArgument(message) => assert!(
            message.contains("max_sessions") && message.contains(&DEFAULT_MAX_SESSIONS.to_string()),
            "unexpected hard-ceiling message: {message}"
        ),
        other => panic!("unexpected max_sessions error: {other}"),
    }
}

#[test]
fn rejects_output_inside_source_before_opening_it() {
    let temporary = TempDirectory::new();
    create_three_decision_fixture(&temporary.0);
    let repository = temporary.join("repo");
    let before = snapshot_tree(&repository);
    let error = export_bundle(&ExportOptions {
        projection: projection_options(repository.clone()),
        destination: repository.join("public"),
        target: OutputTarget::Github,
    })
    .unwrap_err();
    assert!(matches!(error, PublicationError::UnsafePath(_)));
    assert!(!repository.join("public").exists());
    assert_eq!(snapshot_tree(&repository), before);
}

#[test]
fn fails_closed_on_uncheckpointed_sqlite_sidecar_without_partial_output() {
    let temporary = TempDirectory::new();
    create_three_decision_fixture(&temporary.0);
    let repository = temporary.join("repo");
    let wal = repository.join("refs.sqlite3-wal");
    fs::write(&wal, b"UNTOUCHED_WAL_CANARY").unwrap();
    let destination = temporary.join("bundle");

    let error = export_bundle(&ExportOptions {
        projection: projection_options(repository),
        destination: destination.clone(),
        target: OutputTarget::Github,
    })
    .unwrap_err();
    assert_eq!(error.code(), "read_only_source_busy");
    assert_eq!(fs::read(wal).unwrap(), b"UNTOUCHED_WAL_CANARY");
    assert!(!destination.exists());
}

#[cfg(unix)]
#[test]
fn rejects_symlink_destination_parent_without_touching_target() {
    use std::os::unix::fs::symlink;

    let temporary = TempDirectory::new();
    create_three_decision_fixture(&temporary.0);
    let real_parent = temporary.join("real-parent");
    fs::create_dir(&real_parent).unwrap();
    fs::write(real_parent.join("sentinel"), b"unchanged").unwrap();
    let linked_parent = temporary.join("linked-parent");
    symlink(&real_parent, &linked_parent).unwrap();

    let error = export_bundle(&ExportOptions {
        projection: projection_options(temporary.join("repo")),
        destination: linked_parent.join("bundle"),
        target: OutputTarget::Github,
    })
    .unwrap_err();
    assert!(matches!(error, PublicationError::UnsafePath(_)));
    assert_eq!(
        fs::read(real_parent.join("sentinel")).unwrap(),
        b"unchanged"
    );
    assert!(!real_parent.join("bundle").exists());
}

#[cfg(unix)]
#[test]
fn rejects_symlinked_source_cas_root_without_publishing_destination() {
    use std::os::unix::fs::symlink;

    let temporary = TempDirectory::new();
    create_three_decision_fixture(&temporary.0);
    let repository = temporary.join("repo");
    let real_cas = temporary.join("real-cas");
    fs::rename(repository.join("cas"), &real_cas).unwrap();
    symlink(&real_cas, repository.join("cas")).unwrap();
    let destination = temporary.join("bundle");

    let error = export_bundle(&ExportOptions {
        projection: projection_options(repository),
        destination: destination.clone(),
        target: OutputTarget::Github,
    })
    .unwrap_err();
    assert_eq!(error.code(), "repository_error");
    assert!(!destination.exists());
}

fn snapshot_tree(root: &Path) -> BTreeMap<String, Vec<u8>> {
    fn walk(root: &Path, directory: &Path, output: &mut BTreeMap<String, Vec<u8>>) {
        let mut entries = fs::read_dir(directory)
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let path = entry.path();
            let relative = path
                .strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            let metadata = fs::symlink_metadata(&path).unwrap();
            if metadata.is_dir() {
                output.insert(format!("{relative}/"), Vec::new());
                walk(root, &path, output);
            } else if metadata.is_file() {
                output.insert(relative, fs::read(path).unwrap());
            } else {
                output.insert(format!("{relative}@special"), Vec::new());
            }
        }
    }

    let mut output = BTreeMap::new();
    walk(root, root, &mut output);
    output
}

fn bundle_bytes(root: &Path) -> String {
    snapshot_tree(root)
        .into_values()
        .flat_map(|bytes| bytes.into_iter())
        .map(char::from)
        .collect()
}

fn canonical_json(value: &impl serde::Serialize) -> Vec<u8> {
    let ordinary = serde_json::to_vec(value).unwrap();
    canonical_bytes(&parse_strict(&ordinary).unwrap()).unwrap()
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<Vec<_>>()
        .join("")
}

fn write_manifest_and_reconcile_checksums(bundle: &Path, manifest: &BundleManifest) {
    fs::write(bundle.join("manifest.json"), canonical_json(manifest)).unwrap();
    reconcile_bundle_checksums(bundle);
}

fn reconcile_bundle_checksums(bundle: &Path) {
    let mut checksums: ChecksumsDocument =
        serde_json::from_slice(&fs::read(bundle.join("checksums.json")).unwrap()).unwrap();
    for entry in &mut checksums.files {
        let bytes = fs::read(bundle.join(&entry.path)).unwrap();
        entry.byte_len = bytes.len() as u64;
        entry.sha256 = sha256_hex(&bytes);
    }
    fs::write(bundle.join("checksums.json"), canonical_json(&checksums)).unwrap();
}
