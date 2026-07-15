use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Barrier};
use std::thread;
use synapse_core::Repository;

static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);
const PROPOSAL_HEAD: &str =
    "commit:sg-oid-v1:sha256:21f1e5825721dafad3847c6b0f7d2143f46288fe72082da4dedae62c0db82b00";
const BASE_HEAD: &str =
    "commit:sg-oid-v1:sha256:0b1370e0f6eb296f698c650740cb9fd9e0fbaa4cb86707c4967d034d7a27ca13";

struct TempDirectory(PathBuf);

impl TempDirectory {
    fn new() -> Self {
        let sequence = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "synapse-cli-test-{}-{sequence}",
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

fn run(arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_synapse"))
        .args(arguments)
        .output()
        .unwrap()
}

fn run_owned(arguments: Vec<String>) -> Output {
    Command::new(env!("CARGO_BIN_EXE_synapse"))
        .args(arguments)
        .output()
        .unwrap()
}

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "stdout={}\nstderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn fixture_directory() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../spec/core/v0.1/fixtures")
}

fn load_fixture_store(repository: &Repository) {
    let fixture_directory = fixture_directory();
    for entry in fs::read_dir(&fixture_directory)
        .unwrap()
        .filter_map(Result::ok)
    {
        let path = entry.path();
        if path
            .extension()
            .is_some_and(|extension| extension == "json")
            && path.file_name().is_some_and(|name| name != "golden.json")
        {
            repository.put_object(&fs::read(path).unwrap()).unwrap();
        }
    }
    repository
        .put_blob(fs::File::open(fixture_directory.join("proposal.txt")).unwrap())
        .unwrap();
}

#[test]
fn command_line_drives_ref_fsck_export_and_restore() {
    let temporary = TempDirectory::new();
    let repository_path = temporary.join("repo");
    let archive_path = temporary.join("archive");
    let restored_path = temporary.join("restored");
    let repository = Repository::open(&repository_path).unwrap();
    load_fixture_store(&repository);
    drop(repository);

    let repository_text = repository_path.to_str().unwrap();
    let update = run(&[
        "update-ref",
        repository_text,
        "proposal/agent/cli",
        "-",
        PROPOSAL_HEAD,
        "--message",
        "CLI acceptance",
    ]);
    assert_success(&update);

    let fsck = run(&["fsck", repository_text]);
    assert_success(&fsck);
    assert!(String::from_utf8_lossy(&fsck.stdout).contains("issues=0"));

    let export = run(&["export", repository_text, archive_path.to_str().unwrap()]);
    assert_success(&export);
    let restore = run(&[
        "restore",
        archive_path.to_str().unwrap(),
        restored_path.to_str().unwrap(),
    ]);
    assert_success(&restore);
    let refs = run(&["refs", restored_path.to_str().unwrap()]);
    assert_success(&refs);
    let refs = String::from_utf8(refs.stdout).unwrap();
    assert!(refs.contains("proposal/agent/cli"));
    assert!(refs.contains(PROPOSAL_HEAD));
}

#[test]
fn concurrent_cli_exports_restore_consistent_ref_update_prefixes() {
    const ROUNDS: usize = 16;
    const REF_NAME: &str = "proposal/agent/export-race";

    let temporary = TempDirectory::new();
    let repository_path = temporary.join("repo");
    let repository = Repository::open(&repository_path).unwrap();
    load_fixture_store(&repository);
    drop(repository);
    let repository_text = repository_path.to_str().unwrap().to_owned();

    let initial = run(&[
        "update-ref",
        &repository_text,
        REF_NAME,
        "-",
        BASE_HEAD,
        "--message",
        "initial",
    ]);
    assert_success(&initial);

    let mut expected_head = BASE_HEAD;
    for round in 0..ROUNDS {
        let new_head = if expected_head == BASE_HEAD {
            PROPOSAL_HEAD
        } else {
            BASE_HEAD
        };
        let archive_path = temporary.join(format!("archive-{round}"));
        let restored_path = temporary.join(format!("restored-{round}"));
        let before_reflog_len = round + 1;
        let barrier = Arc::new(Barrier::new(2));

        let export_barrier = Arc::clone(&barrier);
        let export_arguments = vec![
            "export".to_owned(),
            repository_text.clone(),
            archive_path.to_str().unwrap().to_owned(),
        ];
        let export = thread::spawn(move || {
            export_barrier.wait();
            run_owned(export_arguments)
        });

        let update_barrier = Arc::clone(&barrier);
        let update_arguments = vec![
            "update-ref".to_owned(),
            repository_text.clone(),
            REF_NAME.to_owned(),
            expected_head.to_owned(),
            new_head.to_owned(),
            "--message".to_owned(),
            format!("round-{round}"),
        ];
        let update = thread::spawn(move || {
            update_barrier.wait();
            run_owned(update_arguments)
        });

        let export = export.join().expect("export thread panicked");
        let update = update.join().expect("update thread panicked");
        assert_success(&export);
        assert_success(&update);

        let restored = Repository::restore_archive(&archive_path, &restored_path).unwrap();
        assert!(restored.fsck().unwrap().is_clean());
        let restored_ref = restored.refs().get(REF_NAME).unwrap().unwrap();
        assert!(
            restored_ref.head == expected_head || restored_ref.head == new_head,
            "archive observed unexpected head {}",
            restored_ref.head
        );
        let restored_reflog = restored.refs().reflog().unwrap();
        assert!(
            restored_reflog.len() == before_reflog_len
                || restored_reflog.len() == before_reflog_len + 1
        );
        let restored_last = restored_reflog.last().unwrap();
        assert_eq!(restored_last.new_head, restored_ref.head);
        assert_eq!(restored_last.id, restored_ref.updated_event_id);

        let source = Repository::open(&repository_path).unwrap();
        assert_eq!(source.refs().get(REF_NAME).unwrap().unwrap().head, new_head);
        let source_reflog = source.refs().reflog().unwrap();
        assert_eq!(
            restored_reflog.as_slice(),
            &source_reflog[..restored_reflog.len()],
            "archive reflog must be one consistent prefix"
        );
        expected_head = new_head;
    }
}

#[test]
fn creator_cli_builds_reports_and_restores_one_human_gated_session() {
    let temporary = TempDirectory::new();
    let repository_path = temporary.join("creator-repo");
    let archive_path = temporary.join("creator-archive");
    let restored_path = temporary.join("creator-restored");
    let original_path = temporary.join("original.png");
    let current_path = temporary.join("current.png");
    let proposal_path = temporary.join("proposal.png");
    fs::write(&original_path, b"creator original image").unwrap();
    fs::write(&current_path, b"creator current image").unwrap();
    fs::write(&proposal_path, b"creator AI proposal image").unwrap();

    let created = run(&[
        "creator-run",
        repository_path.to_str().unwrap(),
        "north-wall",
        original_path.to_str().unwrap(),
        current_path.to_str().unwrap(),
        proposal_path.to_str().unwrap(),
        "--subject",
        "North wall mural",
        "--creator",
        "Aki",
        "--decision",
        "adopt",
        "--rationale",
        "The proposal fits the intended palette.",
    ]);
    assert_success(&created);
    let created = String::from_utf8(created.stdout).unwrap();
    assert!(created.contains("proposal_attributed_to_agent=urn:uuid:"));
    assert!(created.contains("ai_output_source=caller_supplied"));
    assert!(created.contains("reviewed_by_human=urn:uuid:"));
    assert!(created.contains("selected=true"));
    assert!(created.contains("disposition=adopt"));
    assert!(created.contains("comparison_analysis=record:sg-oid-v1:sha256:"));
    assert!(created.contains("comparison_adapter=synapsegit.observation.byte-identity@1"));
    assert!(created.contains("comparison_status=succeeded"));
    assert!(created.contains("comparison_comparability=partial"));
    assert!(created.contains("byte_identity=different"));
    assert!(created.contains(
        "comparison_reason_codes=byte_identity_only,capture_profile_imported,capture_time_unknown"
    ));
    assert!(created.contains("comparison_replay_ready=true"));
    assert!(!created.contains("changed=false"));
    assert!(created.contains("fsck=clean"));
    assert!(created.contains("timeline=4"));

    let report = run(&[
        "creator-report",
        repository_path.to_str().unwrap(),
        "north-wall",
    ]);
    assert_success(&report);
    let report = String::from_utf8(report.stdout).unwrap();

    assert_success(&run(&[
        "export",
        repository_path.to_str().unwrap(),
        archive_path.to_str().unwrap(),
    ]));
    assert_success(&run(&[
        "restore",
        archive_path.to_str().unwrap(),
        restored_path.to_str().unwrap(),
    ]));
    let restored_report = run(&[
        "creator-report",
        restored_path.to_str().unwrap(),
        "north-wall",
    ]);
    assert_success(&restored_report);
    assert_eq!(String::from_utf8(restored_report.stdout).unwrap(), report);

    let rejected_path = temporary.join("creator-rejected");
    let rejected = run(&[
        "creator-run",
        rejected_path.to_str().unwrap(),
        "rejected-wall",
        original_path.to_str().unwrap(),
        current_path.to_str().unwrap(),
        proposal_path.to_str().unwrap(),
        "--subject",
        "Rejected wall proposal",
        "--creator",
        "Aki",
        "--decision",
        "reject",
    ]);
    assert_success(&rejected);
    let rejected = String::from_utf8(rejected.stdout).unwrap();
    assert!(rejected.contains("disposition=reject"));
    assert!(rejected.contains("selected=false"));

    let invalid = run(&[
        "creator-run",
        temporary.join("invalid").to_str().unwrap(),
        "invalid-decision",
        original_path.to_str().unwrap(),
        current_path.to_str().unwrap(),
        proposal_path.to_str().unwrap(),
        "--subject",
        "Invalid decision fixture",
        "--creator",
        "Aki",
        "--decision",
        "maybe",
    ]);
    assert!(!invalid.status.success());
    let invalid = String::from_utf8(invalid.stderr).unwrap();
    assert!(invalid.contains("usage_error: decision must be one of"));
    assert!(invalid.contains("Usage:"));
}
