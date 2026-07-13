use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use synapse_core::Repository;
use synapse_creator::{
    CreatorDisposition, CreatorError, CreatorRunOptions, creator_report, run_creator_session,
};
use synapse_sqlite::{RefUpdate, ReflogMetadata};

static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

struct TempDirectory(PathBuf);

impl TempDirectory {
    fn new() -> Self {
        let sequence = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "synapse-creator-test-{}-{sequence}",
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

fn options(
    temporary: &TempDirectory,
    repository: &Path,
    session: &str,
    disposition: CreatorDisposition,
) -> CreatorRunOptions {
    let original = temporary.join(format!("{session}-original.png"));
    let current = temporary.join(format!("{session}-current.png"));
    let proposal = temporary.join(format!("{session}-proposal.png"));
    fs::write(&original, format!("original image for {session}")).unwrap();
    fs::write(&current, format!("current image for {session}")).unwrap();
    fs::write(&proposal, format!("AI proposal for {session}")).unwrap();
    CreatorRunOptions {
        repository: repository.to_owned(),
        session: session.to_owned(),
        original_image: original,
        current_image: current,
        ai_output: proposal,
        subject_label: "North wall mural".into(),
        creator_name: "Aki".into(),
        disposition,
        rationale: Some(format!("Creator chose {}.", disposition.as_cli_str())),
    }
}

#[test]
fn creator_workflow_uses_ai_and_human_routes_and_survives_restore() {
    let temporary = TempDirectory::new();
    let repository_path = temporary.join("repo");
    let receipt = run_creator_session(&options(
        &temporary,
        &repository_path,
        "mural-1",
        CreatorDisposition::Adopt,
    ))
    .unwrap();
    assert_ne!(receipt.base_head, receipt.proposal_head);
    assert_ne!(receipt.base_head, receipt.decision_head);
    assert_eq!(receipt.disposition, CreatorDisposition::Adopt);

    let report = creator_report(&repository_path, "mural-1").unwrap();
    assert_eq!(report.disposition, CreatorDisposition::Adopt);
    assert_eq!(report.decision_head, receipt.decision_head);
    assert_eq!(report.proposal_head, receipt.proposal_head);
    assert_eq!(report.original_blob_oid, receipt.original_blob_oid);
    assert_eq!(report.current_blob_oid, receipt.current_blob_oid);
    assert_eq!(report.ai_output_blob_oid, receipt.ai_output_blob_oid);
    assert_eq!(report.timeline.len(), 4);
    assert!(
        report
            .timeline
            .iter()
            .all(|entry| entry.time_basis.ends_with("recorded_at_fallback"))
    );
    assert_eq!(
        report
            .timeline
            .iter()
            .map(|entry| entry.stage)
            .collect::<Vec<_>>(),
        [
            "original_observation",
            "current_observation",
            "image_import",
            "ai_proposal"
        ]
    );
    assert!(
        report
            .timeline
            .windows(2)
            .all(|pair| pair[0].ordering_time < pair[1].ordering_time)
    );
    assert!(report.selected_ai_output);
    assert_eq!(report.decision_snapshot, report.proposal_snapshot);
    assert_eq!(
        report
            .timeline
            .iter()
            .filter(|entry| entry.kind == "observation")
            .count(),
        2
    );
    let repository = Repository::open(&repository_path).unwrap();
    for observation_oid in [
        &receipt.original_observation_oid,
        &receipt.current_observation_oid,
    ] {
        let bytes = repository
            .objects()
            .read_raw(observation_oid)
            .unwrap()
            .unwrap();
        let observation: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(observation["payload"]["capture_time"]["kind"], "unknown");
        assert_eq!(
            observation["payload"]["capture_profile_ref"],
            receipt.capture_profile_oid
        );
    }
    let capture_profile: serde_json::Value = serde_json::from_slice(
        &repository
            .objects()
            .read_raw(&receipt.capture_profile_oid)
            .unwrap()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(capture_profile["record_type"], "capture_profile");
    assert_eq!(capture_profile["payload"]["profile_level"], "imported");
    assert_eq!(
        capture_profile["payload"]["allowed_claims"],
        serde_json::json!(["reference_only"])
    );
    assert_eq!(
        capture_profile["payload"]["required_conditions"],
        serde_json::json!([])
    );
    let feedback: serde_json::Value = serde_json::from_slice(
        &repository
            .objects()
            .read_raw(&receipt.decision_feedback_oid)
            .unwrap()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        feedback["payload"]["reason_codes"],
        serde_json::json!(["unspecified"])
    );
    assert_eq!(feedback["payload"]["visibility"], "private");
    assert_eq!(feedback["payload"]["training_use_policy"], "prohibited");
    for activity_oid in report
        .timeline
        .iter()
        .filter(|entry| entry.kind == "activity")
        .map(|entry| &entry.oid)
    {
        let activity: serde_json::Value = serde_json::from_slice(
            &repository
                .objects()
                .read_raw(activity_oid)
                .unwrap()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(
            activity["payload"]["before_observation_refs"],
            serde_json::json!([])
        );
        assert_eq!(
            activity["payload"]["after_observation_refs"],
            serde_json::json!([])
        );
    }
    drop(repository);
    assert_eq!(
        report
            .timeline
            .iter()
            .filter(|entry| entry.kind == "activity")
            .count(),
        2
    );

    let archive = temporary.join("archive");
    let restored = temporary.join("restored");
    Repository::open(&repository_path)
        .unwrap()
        .export_archive(&archive)
        .unwrap();
    Repository::restore_archive(&archive, &restored).unwrap();
    let restored_report = creator_report(&restored, "mural-1").unwrap();
    assert_eq!(restored_report, report);
}

#[test]
fn reject_and_defer_keep_ai_provenance_separate_from_human_selection() {
    let temporary = TempDirectory::new();
    let repository_path = temporary.join("repo");
    for (session, disposition) in [
        ("rejected", CreatorDisposition::Reject),
        ("deferred", CreatorDisposition::Defer),
    ] {
        let receipt =
            run_creator_session(&options(&temporary, &repository_path, session, disposition))
                .unwrap();
        let report = creator_report(&repository_path, session).unwrap();
        assert_eq!(report.disposition, disposition);
        assert_eq!(report.agent_id, receipt.agent_id);
        assert_eq!(report.creator_id, receipt.creator_id);
        assert_eq!(report.ai_output_blob_oid, receipt.ai_output_blob_oid);
        assert_eq!(report.timeline.len(), 4);
        assert!(!report.selected_ai_output);
        assert_eq!(report.decision_snapshot, report.base_snapshot);
    }
}

#[test]
fn creator_session_is_create_only() {
    let temporary = TempDirectory::new();
    let repository_path = temporary.join("repo");
    let session_options = options(
        &temporary,
        &repository_path,
        "same-session",
        CreatorDisposition::Adopt,
    );
    let receipt = run_creator_session(&session_options).unwrap();
    fs::remove_file(&session_options.original_image).unwrap();
    fs::remove_file(&session_options.current_image).unwrap();
    fs::remove_file(&session_options.ai_output).unwrap();
    let error = run_creator_session(&session_options).unwrap_err();
    assert!(matches!(error, CreatorError::SessionExists(session) if session == "same-session"));

    let mut repository = Repository::open(&repository_path).unwrap();
    repository
        .update_ref(RefUpdate {
            ref_name: "decision/creator/incomplete-session",
            expected_head: None,
            new_head: &receipt.base_head,
            metadata: ReflogMetadata::at(1),
        })
        .unwrap();
    drop(repository);
    let incomplete = options(
        &temporary,
        &repository_path,
        "incomplete-session",
        CreatorDisposition::Adopt,
    );
    fs::remove_file(&incomplete.original_image).unwrap();
    let error = run_creator_session(&incomplete).unwrap_err();
    assert!(
        matches!(error, CreatorError::SessionIncomplete(session) if session == "incomplete-session")
    );
}

#[test]
fn report_requires_both_creator_refs() {
    let temporary = TempDirectory::new();
    let repository_path = temporary.join("repo");
    let error = creator_report(&repository_path, "missing-session").unwrap_err();
    assert!(
        matches!(error, CreatorError::SessionNotFound(session) if session == "missing-session")
    );

    let receipt = run_creator_session(&options(
        &temporary,
        &repository_path,
        "complete-session",
        CreatorDisposition::Adopt,
    ))
    .unwrap();
    let mut repository = Repository::open(&repository_path).unwrap();
    repository
        .update_ref(RefUpdate {
            ref_name: "decision/creator/report-incomplete",
            expected_head: None,
            new_head: &receipt.base_head,
            metadata: ReflogMetadata::at(1),
        })
        .unwrap();
    drop(repository);
    let error = creator_report(&repository_path, "report-incomplete").unwrap_err();
    assert!(
        matches!(error, CreatorError::SessionIncomplete(session) if session == "report-incomplete")
    );

    let mut repository = Repository::open(&repository_path).unwrap();
    repository
        .update_ref(RefUpdate {
            ref_name: "decision/creator/report-both-incomplete",
            expected_head: None,
            new_head: &receipt.base_head,
            metadata: ReflogMetadata::at(2),
        })
        .unwrap();
    repository
        .update_ref(RefUpdate {
            ref_name: "proposal/creator-agent/report-both-incomplete",
            expected_head: None,
            new_head: &receipt.proposal_head,
            metadata: ReflogMetadata::at(3),
        })
        .unwrap();
    drop(repository);
    let error = creator_report(&repository_path, "report-both-incomplete").unwrap_err();
    assert!(
        matches!(error, CreatorError::SessionIncomplete(session) if session == "report-both-incomplete")
    );
}

#[test]
fn report_rejects_a_current_proposal_that_does_not_match_human_feedback() {
    let temporary = TempDirectory::new();
    let repository_path = temporary.join("repo");
    let first = run_creator_session(&options(
        &temporary,
        &repository_path,
        "first-session",
        CreatorDisposition::Adopt,
    ))
    .unwrap();
    let second = run_creator_session(&options(
        &temporary,
        &repository_path,
        "second-session",
        CreatorDisposition::Adopt,
    ))
    .unwrap();
    let mut repository = Repository::open(&repository_path).unwrap();
    repository
        .update_ref(RefUpdate {
            ref_name: &first.proposal_ref,
            expected_head: Some(&first.proposal_head),
            new_head: &second.proposal_head,
            metadata: ReflogMetadata::at(1),
        })
        .unwrap();
    assert!(repository.fsck().unwrap().is_clean());
    drop(repository);

    let error = creator_report(&repository_path, "first-session").unwrap_err();
    assert!(matches!(error, CreatorError::ReportInvalid(_)));
}
