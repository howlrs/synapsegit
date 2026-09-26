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

#[test]
fn version_reports_the_package_version() {
    let output = run(&["--version"]);
    assert_success(&output);
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        format!("synapse {}\n", env!("CARGO_PKG_VERSION"))
    );
    assert!(output.stderr.is_empty());
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
fn export_to_bare_relative_destination_succeeds_and_restores() {
    let temporary = TempDirectory::new();
    let repository_path = temporary.join("repo");
    let repository = Repository::open(&repository_path).unwrap();
    load_fixture_store(&repository);
    drop(repository);

    let export = Command::new(env!("CARGO_BIN_EXE_synapse"))
        .current_dir(&temporary.0)
        .args(["export", repository_path.to_str().unwrap(), "out"])
        .output()
        .unwrap();
    assert_success(&export);
    assert_eq!(String::from_utf8(export.stdout).unwrap(), "exported out\n");
    assert!(temporary.join("out/manifest.json").is_file());

    let restore = Command::new(env!("CARGO_BIN_EXE_synapse"))
        .current_dir(&temporary.0)
        .args(["restore", "out", "restored"])
        .output()
        .unwrap();
    assert_success(&restore);
    assert!(temporary.join("restored/refs.sqlite3").is_file());
}

#[test]
fn existing_repository_commands_reject_missing_paths_without_creating_them() {
    let temporary = TempDirectory::new();
    let missing = temporary.join("missing/typo");
    let input = temporary.join("input.json");
    fs::write(&input, b"{}").unwrap();
    let missing_text = missing.to_str().unwrap();
    let input_text = input.to_str().unwrap();

    let commands = [
        vec!["refs", missing_text],
        vec!["fsck", missing_text],
        vec!["export", missing_text, "archive"],
        vec!["put-blob", missing_text, input_text],
        vec!["put-record", missing_text, input_text],
        vec!["build-tree", missing_text, input_text],
        vec!["commit", missing_text, input_text],
        vec!["put-object", missing_text, input_text],
        vec![
            "update-ref",
            missing_text,
            "proposal/agent/test",
            "-",
            PROPOSAL_HEAD,
        ],
        vec!["creator-report", missing_text, "session"],
    ];
    for command in commands {
        let output = run(&command);
        assert!(
            !output.status.success(),
            "command unexpectedly succeeded: {command:?}"
        );
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("repository_not_found"),
            "command={command:?}, stderr={}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            !missing.exists(),
            "command created missing repository path: {command:?}"
        );
    }
}

#[test]
fn init_refuses_a_nonempty_nonrepository_directory() {
    let temporary = TempDirectory::new();
    let repository = temporary.join("repository");
    fs::create_dir(&repository).unwrap();
    fs::write(repository.join("keep.txt"), b"keep").unwrap();

    let output = run(&["init", repository.to_str().unwrap()]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("repository_not_empty"));
    assert_eq!(fs::read(repository.join("keep.txt")).unwrap(), b"keep");
    assert!(!repository.join("cas").exists());
    assert!(!repository.join("refs.sqlite3").exists());
}

#[test]
fn existing_repository_commands_preserve_deep_cas_layout_failures() {
    let temporary = TempDirectory::new();
    let repository_path = temporary.join("repository");
    Repository::open(&repository_path).unwrap();
    fs::remove_dir(repository_path.join("cas/objects/blob")).unwrap();

    let output = run(&["fsck", repository_path.to_str().unwrap()]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("schema_invalid"));
    assert!(!repository_path.join("cas/objects/blob").exists());
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

#[test]
fn creator_report_prints_escaped_three_blob_reuse_provenance() {
    use synapse_creator::{
        CreatorBeginOptions, CreatorDecisionOptions, CreatorDisposition, begin_creator_session,
        begin_creator_session_with_reuse_source, creator_reuse_source_from_snapshot,
        decide_creator_session,
    };

    let temporary = TempDirectory::new();
    let repository = temporary.join("reuse-report");
    let original = temporary.join("original.png");
    let current = temporary.join("current.png");
    let proposal = temporary.join("proposal.png");
    fs::write(&original, b"original image").unwrap();
    fs::write(&current, b"current image").unwrap();
    fs::write(&proposal, b"proposal image").unwrap();
    let options = |session: &str| CreatorBeginOptions {
        repository: repository.clone(),
        session: session.into(),
        original_image: original.clone(),
        current_image: current.clone(),
        ai_output: proposal.clone(),
        subject_label: "CLI reuse report".into(),
        creator_name: "CLI tester".into(),
    };

    let mut source_pending = begin_creator_session(&options("deferred-source")).unwrap();
    decide_creator_session(
        &mut source_pending,
        &CreatorDecisionOptions {
            disposition: CreatorDisposition::Defer,
            rationale: Some("A fresh review is needed.".into()),
        },
    )
    .unwrap();
    let repository_handle = Repository::open(&repository).unwrap();
    let snapshot = repository_handle.refs().snapshot().unwrap();
    let source =
        creator_reuse_source_from_snapshot(&repository_handle, &snapshot, "deferred-source")
            .unwrap();

    let mut reused_pending =
        begin_creator_session_with_reuse_source(&options("reused-session"), &source).unwrap();
    decide_creator_session(
        &mut reused_pending,
        &CreatorDecisionOptions {
            disposition: CreatorDisposition::Adopt,
            rationale: None,
        },
    )
    .unwrap();

    let report = run(&[
        "creator-report",
        repository.to_str().unwrap(),
        "reused-session",
    ]);
    assert_success(&report);
    let report = String::from_utf8(report.stdout).unwrap();
    assert!(
        report.contains("reused_three_blob_source_format=\"synapsegit-creator-reuse-source-v1\"")
    );
    assert!(report.contains("reused_three_blob_source_kind=\"deferred_rereview\""));
    assert!(report.contains("reused_three_blob_source_session=\"deferred-source\""));
    assert!(report.contains(&format!(
        "reused_three_blob_source_proposal_head={:?}",
        source.proposal_head
    )));
    assert!(report.contains(&format!(
        "reused_three_blob_source_decision_head={:?}",
        source.decision_head
    )));
    for (label, oid) in [
        ("original", source.original_blob_oid),
        ("current", source.current_blob_oid),
        ("ai_output", source.ai_output_blob_oid),
    ] {
        assert!(report.contains(&format!(
            "reused_three_blob_source_{label}_blob_oid={oid:?}"
        )));
    }
}

#[test]
fn creator_report_prints_private_user_declared_notes_separately_and_escaped() {
    use synapse_creator::{
        CreatorBeginOptions, CreatorDecisionOptions, CreatorDisposition, CreatorGenerationNote,
        begin_creator_session_with_note, decide_creator_session,
    };
    let temporary = TempDirectory::new();
    let image = temporary.join("image");
    fs::write(&image, b"opaque image").unwrap();
    let repository = temporary.join("repo");
    let mut pending = begin_creator_session_with_note(
        &CreatorBeginOptions {
            repository: repository.clone(),
            session: "note-report".into(),
            original_image: image.clone(),
            current_image: image.clone(),
            ai_output: image,
            subject_label: "作品".into(),
            creator_name: "Creator".into(),
        },
        Some(&CreatorGenerationNote {
            prompt: "日本語\nPRIVATE_NOTE\u{1b}[31m".into(),
            ..Default::default()
        }),
    )
    .unwrap();
    decide_creator_session(
        &mut pending,
        &CreatorDecisionOptions {
            disposition: CreatorDisposition::Reject,
            rationale: Some("別の判断理由".into()),
        },
    )
    .unwrap();
    let report = run(&[
        "creator-report",
        repository.to_str().unwrap(),
        "note-report",
    ]);
    assert_success(&report);
    let text = String::from_utf8(report.stdout).unwrap();
    assert!(text.contains("generation_note_user_declared="));
    assert!(text.contains("日本語\\nPRIVATE_NOTE\\u{1b}[31m"));
    assert!(text.contains("rationale=\"別の判断理由\""));
    assert!(!text.contains('\u{1b}'));
}

fn json_stdout(output: &Output) -> serde_json::Value {
    assert_success(output);
    serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "creator-report --format json did not print a single parseable JSON document: {error}\nstdout={}",
            String::from_utf8_lossy(&output.stdout)
        )
    })
}

#[test]
fn creator_report_format_json_default_and_text_output_are_unchanged() {
    let temporary = TempDirectory::new();
    let repository_path = temporary.join("repo");
    let original = temporary.join("original.png");
    let current = temporary.join("current.png");
    let proposal = temporary.join("proposal.png");
    fs::write(&original, b"json original image").unwrap();
    fs::write(&current, b"json current image").unwrap();
    fs::write(&proposal, b"json proposal image").unwrap();

    let created = run(&[
        "creator-run",
        repository_path.to_str().unwrap(),
        "json-default",
        original.to_str().unwrap(),
        current.to_str().unwrap(),
        proposal.to_str().unwrap(),
        "--subject",
        "JSON default subject",
        "--creator",
        "Aki",
        "--decision",
        "adopt",
        "--rationale",
        "adopted for the json contract test",
    ]);
    assert_success(&created);

    // The existing line-oriented text output stays byte-identical whether
    // --format is omitted or explicitly "text".
    let default_output = run(&[
        "creator-report",
        repository_path.to_str().unwrap(),
        "json-default",
    ]);
    assert_success(&default_output);
    let explicit_text_output = run(&[
        "creator-report",
        repository_path.to_str().unwrap(),
        "json-default",
        "--format",
        "text",
    ]);
    assert_success(&explicit_text_output);
    assert_eq!(default_output.stdout, explicit_text_output.stdout);

    let json_output = run(&[
        "creator-report",
        repository_path.to_str().unwrap(),
        "json-default",
        "--format",
        "json",
    ]);
    let document = json_stdout(&json_output);
    assert_eq!(
        document["format"],
        serde_json::json!("synapsegit-cli-creator-report-v1")
    );
    assert_eq!(document["scope"], serde_json::json!("private_local"));
    assert_eq!(
        document["ai_output_source"],
        serde_json::json!("caller_supplied")
    );
    assert_eq!(document["disposition"], serde_json::json!("adopt"));
    assert_eq!(document["selected_ai_output"], serde_json::json!(true));
    assert_eq!(
        document["rationale"],
        serde_json::json!("adopted for the json contract test")
    );
    // No optional evidence was ever recorded for this session: every
    // optional slot must say so explicitly rather than being silently
    // omitted or defaulted to some other absence-shaped value.
    assert_eq!(
        document["source"]["availability"],
        serde_json::json!("absent")
    );
    assert_eq!(
        document["reuse_source"]["availability"],
        serde_json::json!("absent")
    );
    assert_eq!(
        document["generation_note"]["availability"],
        serde_json::json!("absent")
    );
    assert_eq!(
        document["decision_pins"]["availability"],
        serde_json::json!("absent")
    );
    assert_eq!(
        document["comparison"]["availability"],
        serde_json::json!("present")
    );
    assert_eq!(
        document["comparison"]["outcome"],
        serde_json::json!("different")
    );
    assert!(!document["timeline"].as_array().unwrap().is_empty());
    assert!(document["fsck"]["clean"] == serde_json::json!(true));

    // Usage errors (unknown/duplicate option, bad value, missing value) stay
    // on the existing CliError::Usage path/exit code, and never print JSON.
    for bad_arguments in [
        vec![
            "creator-report",
            repository_path.to_str().unwrap(),
            "json-default",
            "--format",
            "yaml",
        ],
        vec![
            "creator-report",
            repository_path.to_str().unwrap(),
            "json-default",
            "--format",
        ],
        vec![
            "creator-report",
            repository_path.to_str().unwrap(),
            "json-default",
            "--bogus",
            "json",
        ],
        vec![
            "creator-report",
            repository_path.to_str().unwrap(),
            "json-default",
            "--format",
            "json",
            "--format",
            "text",
        ],
    ] {
        let output = run(&bad_arguments);
        assert!(
            !output.status.success(),
            "unexpectedly succeeded: {bad_arguments:?}"
        );
        assert!(
            output.stdout.is_empty(),
            "usage error leaked stdout: {bad_arguments:?}"
        );
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("usage_error"),
            "command={bad_arguments:?}, stderr={}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    // Verification failure (unknown session) must never print partial JSON.
    let missing_session = run(&[
        "creator-report",
        repository_path.to_str().unwrap(),
        "no-such-session",
        "--format",
        "json",
    ]);
    assert!(!missing_session.status.success());
    assert!(missing_session.stdout.is_empty());
    assert!(String::from_utf8_lossy(&missing_session.stderr).contains("creator_session_not_found"));
}

#[test]
fn creator_report_format_json_represents_every_disposition_and_json_round_trips_archive() {
    let temporary = TempDirectory::new();
    let repository_path = temporary.join("repo");
    let archive_path = temporary.join("archive");
    let restored_path = temporary.join("restored");
    let original = temporary.join("original.png");
    let current = temporary.join("current.png");
    let proposal = temporary.join("proposal.png");
    fs::write(&original, b"disposition original image").unwrap();
    fs::write(&current, b"disposition current image").unwrap();
    fs::write(&proposal, b"disposition proposal image").unwrap();

    for (session, decision, expected_disposition, expected_selected) in [
        ("adopt-session", "adopt", "adopt", true),
        ("reject-session", "reject", "reject", false),
        ("defer-session", "defer", "defer", false),
    ] {
        let created = run(&[
            "creator-run",
            repository_path.to_str().unwrap(),
            session,
            original.to_str().unwrap(),
            current.to_str().unwrap(),
            proposal.to_str().unwrap(),
            "--subject",
            "Disposition subject",
            "--creator",
            "Aki",
            "--decision",
            decision,
        ]);
        assert_success(&created);

        let json_output = run(&[
            "creator-report",
            repository_path.to_str().unwrap(),
            session,
            "--format",
            "json",
        ]);
        let document = json_stdout(&json_output);
        assert_eq!(
            document["disposition"],
            serde_json::json!(expected_disposition)
        );
        assert_eq!(
            document["selected_ai_output"],
            serde_json::json!(expected_selected)
        );
        assert_eq!(document["session"], serde_json::json!(session));
    }

    // Archive export/restore before/after must yield the identical JSON
    // document (decision, image refs, provenance and comparison) for the
    // same logical history.
    let before = json_stdout(&run(&[
        "creator-report",
        repository_path.to_str().unwrap(),
        "adopt-session",
        "--format",
        "json",
    ]));

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

    let after = json_stdout(&run(&[
        "creator-report",
        restored_path.to_str().unwrap(),
        "adopt-session",
        "--format",
        "json",
    ]));
    assert_eq!(before, after);
    assert_eq!(before["disposition"], serde_json::json!("adopt"));
    assert_eq!(before["blobs"], after["blobs"]);
    assert_eq!(before["comparison"], after["comparison"]);
    assert_eq!(before["source"], after["source"]);
    assert_eq!(before["reuse_source"], after["reuse_source"]);
}

#[test]
fn creator_report_format_json_represents_generation_note_pins_derived_and_reuse_provenance() {
    use synapse_creator::{
        ANNOTATIONS_FORMAT, CreatorAnnotations, CreatorBeginOptions, CreatorDecisionOptions,
        CreatorDisposition, CreatorGenerationNote, CreatorImageRole, CreatorPin,
        CreatorSourceBinding, begin_creator_session_with_note,
        begin_creator_session_with_reuse_source, begin_creator_session_with_source,
        creator_reuse_source_from_snapshot, decide_creator_session,
        decide_creator_session_with_annotations,
    };

    let temporary = TempDirectory::new();
    let repository = temporary.join("repo");
    let original = temporary.join("original.png");
    let current = temporary.join("current.png");
    let proposal = temporary.join("proposal.png");
    fs::write(&original, b"pins original image").unwrap();
    fs::write(&current, b"pins current image").unwrap();
    fs::write(&proposal, b"pins proposal image").unwrap();

    let options = |session: &str| CreatorBeginOptions {
        repository: repository.clone(),
        session: session.into(),
        original_image: original.clone(),
        current_image: current.clone(),
        ai_output: proposal.clone(),
        subject_label: "Pins and provenance subject".into(),
        creator_name: "Aki".into(),
    };

    // Generation note + decision pins present.
    let note = CreatorGenerationNote {
        tool: "paint-tool".into(),
        model: "diffusion-mini".into(),
        prompt: "日本語プロンプト\t\"quoted\"\nline two".into(),
        intent: "restore the mural".into(),
    };
    let mut pinned =
        begin_creator_session_with_note(&options("note-and-pins"), Some(&note)).unwrap();
    let original_oid = pinned.receipt().original_blob_oid.clone();
    let annotations = CreatorAnnotations {
        format: ANNOTATIONS_FORMAT.into(),
        pins: vec![CreatorPin {
            role: CreatorImageRole::Original,
            blob_oid: original_oid,
            x: 10,
            y: 20,
            note: "見てください".into(),
        }],
    };
    decide_creator_session_with_annotations(
        &mut pinned,
        &CreatorDecisionOptions {
            disposition: CreatorDisposition::Adopt,
            rationale: Some("見た目\t確認済み\n次へ".into()),
        },
        Some(&annotations),
    )
    .unwrap();

    let note_document = json_stdout(&run(&[
        "creator-report",
        repository.to_str().unwrap(),
        "note-and-pins",
        "--format",
        "json",
    ]));
    assert_eq!(
        note_document["generation_note"]["availability"],
        serde_json::json!("present")
    );
    assert_eq!(
        note_document["generation_note"]["prompt"],
        serde_json::json!("日本語プロンプト\t\"quoted\"\nline two")
    );
    assert_eq!(
        note_document["generation_note"]["intent"],
        serde_json::json!("restore the mural")
    );
    assert_eq!(
        note_document["decision_pins"]["availability"],
        serde_json::json!("present")
    );
    assert_eq!(
        note_document["decision_pins"]["format"],
        serde_json::json!(ANNOTATIONS_FORMAT)
    );
    let pins = note_document["decision_pins"]["pins"].as_array().unwrap();
    assert_eq!(pins.len(), 1);
    assert_eq!(pins[0]["role"], serde_json::json!("original"));
    assert_eq!(pins[0]["note"], serde_json::json!("見てください"));
    assert_eq!(
        note_document["rationale"],
        serde_json::json!("見た目\t確認済み\n次へ")
    );

    // Derived source: begin a fresh session bound to a prior complete report.
    let mut base = synapse_creator::begin_creator_session(&options("derived-base")).unwrap();
    let base_receipt = decide_creator_session(
        &mut base,
        &CreatorDecisionOptions {
            disposition: CreatorDisposition::Adopt,
            rationale: Some("base adopted".into()),
        },
    )
    .unwrap();
    let base_report = synapse_creator::creator_report(&repository, "derived-base").unwrap();
    let source = CreatorSourceBinding::from_report(&base_report);
    let mut derived =
        begin_creator_session_with_source(&options("derived-session"), None, &source).unwrap();
    decide_creator_session(
        &mut derived,
        &CreatorDecisionOptions {
            disposition: CreatorDisposition::Adopt,
            rationale: None,
        },
    )
    .unwrap();
    let derived_document = json_stdout(&run(&[
        "creator-report",
        repository.to_str().unwrap(),
        "derived-session",
        "--format",
        "json",
    ]));
    assert_eq!(
        derived_document["source"]["availability"],
        serde_json::json!("present")
    );
    assert_eq!(
        derived_document["source"]["session"],
        serde_json::json!("derived-base")
    );
    assert_eq!(
        derived_document["source"]["proposal_head"],
        serde_json::json!(base_receipt.proposal_head)
    );
    assert_eq!(
        derived_document["reuse_source"]["availability"],
        serde_json::json!("absent")
    );
    assert_eq!(derived_document["source_depth"], serde_json::json!(1));

    // Three-image reuse provenance: a session that reuses all three blobs
    // from a deferred prior review.
    let mut deferred =
        synapse_creator::begin_creator_session(&options("reuse-deferred-source")).unwrap();
    decide_creator_session(
        &mut deferred,
        &CreatorDecisionOptions {
            disposition: CreatorDisposition::Defer,
            rationale: Some("needs a fresh review".into()),
        },
    )
    .unwrap();
    let repository_handle = synapse_core::Repository::open_existing(&repository).unwrap();
    let snapshot = repository_handle.refs().snapshot().unwrap();
    let reuse_source =
        creator_reuse_source_from_snapshot(&repository_handle, &snapshot, "reuse-deferred-source")
            .unwrap();
    let mut reused =
        begin_creator_session_with_reuse_source(&options("reuse-session"), &reuse_source).unwrap();
    decide_creator_session(
        &mut reused,
        &CreatorDecisionOptions {
            disposition: CreatorDisposition::Adopt,
            rationale: None,
        },
    )
    .unwrap();
    let reuse_document = json_stdout(&run(&[
        "creator-report",
        repository.to_str().unwrap(),
        "reuse-session",
        "--format",
        "json",
    ]));
    assert_eq!(
        reuse_document["reuse_source"]["availability"],
        serde_json::json!("present")
    );
    assert_eq!(
        reuse_document["reuse_source"]["kind"],
        serde_json::json!("deferred_rereview")
    );
    assert_eq!(
        reuse_document["reuse_source"]["session"],
        serde_json::json!("reuse-deferred-source")
    );
    assert_eq!(
        reuse_document["reuse_source"]["original_blob_oid"],
        serde_json::json!(reuse_source.original_blob_oid)
    );
    assert_eq!(
        reuse_document["source"]["availability"],
        serde_json::json!("absent")
    );
}
