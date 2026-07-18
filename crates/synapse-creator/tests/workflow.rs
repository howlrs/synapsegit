use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use synapse_core::Repository;
use synapse_creator::{
    AnalysisComparability, AnalysisStatus, ByteIdentityOutcome, CreatorBeginOptions,
    CreatorDecisionOptions, CreatorDisposition, CreatorError, CreatorPendingDecisionState,
    CreatorRunOptions, CreatorSessionState, PreparedCreatorReportReader, begin_creator_session,
    creator_report, creator_report_from_snapshot, decide_creator_session,
    discover_creator_sessions, run_creator_session,
};
use synapse_projection::{ProjectionLimits, SqliteProjectionStore};
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

fn begin_options(options: &CreatorRunOptions) -> CreatorBeginOptions {
    CreatorBeginOptions {
        repository: options.repository.clone(),
        session: options.session.clone(),
        original_image: options.original_image.clone(),
        current_image: options.current_image.clone(),
        ai_output: options.ai_output.clone(),
        subject_label: options.subject_label.clone(),
        creator_name: options.creator_name.clone(),
    }
}

fn stored_object_path(repository: &Path, oid: &str) -> PathBuf {
    let kind = oid.split(':').next().unwrap();
    let digest = oid.rsplit(':').next().unwrap();
    repository
        .join("cas")
        .join("objects")
        .join(kind)
        .join(&digest[..2])
        .join(&digest[2..])
}

#[test]
fn creator_workflow_pauses_with_same_process_authority_until_human_decision() {
    let temporary = TempDirectory::new();
    let repository_path = temporary.join("repo");
    let run = options(
        &temporary,
        &repository_path,
        "pending-review",
        CreatorDisposition::Adopt,
    );
    let mut pending = begin_creator_session(&begin_options(&run)).unwrap();
    let receipt = pending.receipt().clone();
    assert_eq!(pending.decision_state(), CreatorPendingDecisionState::Ready);
    let debug = format!("{pending:?}");
    assert!(!debug.contains(repository_path.to_str().unwrap()));
    assert!(!debug.contains(&receipt.creator_id));
    assert!(!debug.contains("AdmittedProposalHandle"));

    let repository = Repository::open(&repository_path).unwrap();
    assert_eq!(
        repository
            .refs()
            .get(&receipt.decision_ref)
            .unwrap()
            .unwrap()
            .head,
        receipt.base_head
    );
    assert_eq!(
        repository
            .refs()
            .get(&receipt.proposal_ref)
            .unwrap()
            .unwrap()
            .head,
        receipt.proposal_head
    );
    let snapshot = repository.refs().snapshot().unwrap();
    let sessions = discover_creator_sessions(&repository, &snapshot, 10).unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].state, CreatorSessionState::Incomplete);
    drop(repository);
    assert!(matches!(
        creator_report(&repository_path, "pending-review"),
        Err(CreatorError::SessionIncomplete(_))
    ));

    let pending_snapshot = Repository::open(&repository_path)
        .unwrap()
        .refs()
        .snapshot()
        .unwrap();
    assert!(matches!(
        decide_creator_session(
            &mut pending,
            &CreatorDecisionOptions {
                disposition: CreatorDisposition::Reject,
                rationale: Some("x".repeat(5_001)),
            }
        ),
        Err(CreatorError::InvalidArgument(_))
    ));
    assert_eq!(
        Repository::open(&repository_path)
            .unwrap()
            .refs()
            .snapshot()
            .unwrap(),
        pending_snapshot
    );
    assert_eq!(pending.decision_state(), CreatorPendingDecisionState::Ready);

    let completed = decide_creator_session(
        &mut pending,
        &CreatorDecisionOptions {
            disposition: CreatorDisposition::Adopt,
            rationale: Some("Reviewed in the two-step creator workflow.".into()),
        },
    )
    .unwrap();
    assert_eq!(completed.proposal_head, receipt.proposal_head);
    assert_eq!(
        creator_report(&repository_path, "pending-review")
            .unwrap()
            .decision_head,
        completed.decision_head
    );
    assert_eq!(pending.completed_receipt(), Some(&completed));
    assert_eq!(
        pending.decision_state(),
        CreatorPendingDecisionState::Consumed
    );

    let snapshot_before_retry = Repository::open(&repository_path)
        .unwrap()
        .refs()
        .snapshot()
        .unwrap();
    assert!(matches!(
        decide_creator_session(
            &mut pending,
            &CreatorDecisionOptions {
                disposition: CreatorDisposition::Reject,
                rationale: None,
            }
        ),
        Err(CreatorError::SessionExists(session)) if session == "pending-review"
    ));
    let snapshot_after_retry = Repository::open(&repository_path)
        .unwrap()
        .refs()
        .snapshot()
        .unwrap();
    assert_eq!(snapshot_after_retry, snapshot_before_retry);
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

    let snapshot_repository = Repository::open(&repository_path).unwrap();
    let snapshot = snapshot_repository.refs().snapshot().unwrap();
    let snapshot_report =
        creator_report_from_snapshot(&snapshot_repository, &snapshot, "mural-1").unwrap();
    assert_eq!(snapshot_report.report, report);
    assert!(
        snapshot_report
            .projection_source_fingerprint
            .starts_with("projection-source-v1:sha256:")
    );
    let mut independent_projection = SqliteProjectionStore::open_in_memory().unwrap();
    let independent_rebuild = independent_projection
        .rebuild_with_limits(
            snapshot_repository.objects(),
            &snapshot,
            ProjectionLimits::default(),
        )
        .unwrap();
    assert_eq!(
        snapshot_report.projection_source_fingerprint,
        independent_rebuild.metadata.source_fingerprint
    );
    assert_eq!(report.decision_head, receipt.decision_head);
    assert_eq!(report.proposal_head, receipt.proposal_head);
    assert_eq!(report.original_blob_oid, receipt.original_blob_oid);
    assert_eq!(report.current_blob_oid, receipt.current_blob_oid);
    assert_eq!(report.ai_output_blob_oid, receipt.ai_output_blob_oid);
    assert_eq!(
        receipt.byte_identity_outcome,
        ByteIdentityOutcome::Different
    );
    assert_eq!(receipt.comparison_status, AnalysisStatus::Succeeded);
    assert_eq!(
        receipt.comparison_comparability,
        AnalysisComparability::Partial
    );
    assert_eq!(
        receipt.comparison_reason_codes,
        [
            "byte_identity_only",
            "capture_profile_imported",
            "capture_time_unknown"
        ]
    );
    let comparison = report.comparison.as_ref().unwrap();
    assert_eq!(comparison.analysis_oid, receipt.comparison_analysis_oid);
    assert_eq!(comparison.tool_id, receipt.comparison_tool_id);
    assert_eq!(comparison.tool_actor_oid, receipt.comparison_tool_actor_oid);
    assert_eq!(
        comparison.implementation_oid,
        receipt.comparison_implementation_oid
    );
    assert_eq!(
        comparison.configuration_oid,
        receipt.comparison_configuration_oid
    );
    assert_ne!(comparison.tool_id, report.agent_id);
    assert_eq!(comparison.status, "succeeded");
    assert_eq!(comparison.comparability, "partial");
    assert_eq!(comparison.outcome, "different");
    assert_eq!(
        comparison.base_observation_oid,
        receipt.original_observation_oid
    );
    assert_eq!(
        comparison.target_observation_oid,
        receipt.current_observation_oid
    );
    assert_eq!(comparison.base_media_oid, receipt.original_blob_oid);
    assert_eq!(comparison.target_media_oid, receipt.current_blob_oid);
    assert!(comparison.replay_ready);
    let mut expected_comparison_refs =
        vec![receipt.decision_ref.clone(), receipt.proposal_ref.clone()];
    expected_comparison_refs.sort();
    assert_eq!(comparison.reachable_from, expected_comparison_refs);
    assert_eq!(
        comparison.warnings,
        ["Different Blob bytes do not establish visual or physical change."]
    );
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
    let comparison_tool: serde_json::Value = serde_json::from_slice(
        &repository
            .objects()
            .read_raw(&receipt.comparison_tool_actor_oid)
            .unwrap()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(comparison_tool["record_type"], "actor");
    assert_eq!(comparison_tool["payload"]["actor_kind"], "software_tool");
    assert_eq!(comparison_tool["entity_id"], receipt.comparison_tool_id);
    assert_eq!(comparison_tool["asserted_by"], receipt.creator_id);
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
fn identical_imports_are_reported_only_as_byte_identity() {
    let temporary = TempDirectory::new();
    let repository_path = temporary.join("repo");
    let session_options = options(
        &temporary,
        &repository_path,
        "identical-files",
        CreatorDisposition::Adopt,
    );
    let original = fs::read(&session_options.original_image).unwrap();
    fs::write(&session_options.current_image, original).unwrap();

    let receipt = run_creator_session(&session_options).unwrap();
    assert_eq!(receipt.original_blob_oid, receipt.current_blob_oid);
    assert_eq!(
        receipt.byte_identity_outcome,
        ByteIdentityOutcome::Identical
    );
    let report = creator_report(&repository_path, "identical-files").unwrap();
    let comparison = report.comparison.unwrap();
    assert_eq!(comparison.outcome, "identical");
    assert_eq!(comparison.comparability, "partial");
    assert_eq!(
        comparison.warnings,
        ["Identical Blob bytes do not establish that the observed physical subject was unchanged."]
    );
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
        let comparison = report.comparison.unwrap();
        assert_eq!(comparison.analysis_oid, receipt.comparison_analysis_oid);
        assert_eq!(comparison.outcome, "different");
        let mut expected_refs = vec![receipt.decision_ref, receipt.proposal_ref];
        expected_refs.sort();
        assert_eq!(comparison.reachable_from, expected_refs);
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

    let repository = Repository::open(&repository_path).unwrap();
    let snapshot = repository.refs().snapshot().unwrap();
    let discovered = discover_creator_sessions(&repository, &snapshot, 10).unwrap();
    let overflow = discover_creator_sessions(&repository, &snapshot, 2)
        .expect_err("session discovery must enforce its response bound");
    assert_eq!(overflow.code(), "resource_limit");
    assert_eq!(
        discovered
            .iter()
            .map(|summary| (summary.session.as_str(), summary.state))
            .collect::<Vec<_>>(),
        [
            ("complete-session", CreatorSessionState::Complete),
            ("report-both-incomplete", CreatorSessionState::Incomplete),
            ("report-incomplete", CreatorSessionState::Incomplete),
        ]
    );
}

#[test]
fn invalid_report_session_does_not_create_a_repository() {
    let temporary = TempDirectory::new();
    let repository_path = temporary.join("must-not-exist");
    let error = creator_report(&repository_path, "INVALID").unwrap_err();
    assert!(matches!(error, CreatorError::InvalidArgument(_)));
    assert!(!repository_path.exists());
}

#[test]
fn caller_supplied_snapshot_remains_stable_after_later_ref_updates() {
    let temporary = TempDirectory::new();
    let repository_path = temporary.join("repo");
    run_creator_session(&options(
        &temporary,
        &repository_path,
        "first-snapshot",
        CreatorDisposition::Adopt,
    ))
    .unwrap();
    let repository = Repository::open(&repository_path).unwrap();
    let first_snapshot = repository.refs().snapshot().unwrap();
    let first =
        creator_report_from_snapshot(&repository, &first_snapshot, "first-snapshot").unwrap();
    drop(repository);

    run_creator_session(&options(
        &temporary,
        &repository_path,
        "later-session",
        CreatorDisposition::Reject,
    ))
    .unwrap();
    let repository = Repository::open(&repository_path).unwrap();
    let still_first =
        creator_report_from_snapshot(&repository, &first_snapshot, "first-snapshot").unwrap();
    let current_snapshot = repository.refs().snapshot().unwrap();
    let current =
        creator_report_from_snapshot(&repository, &current_snapshot, "first-snapshot").unwrap();

    assert_eq!(
        still_first.projection_source_fingerprint,
        first.projection_source_fingerprint
    );
    assert_eq!(still_first.report.decision_head, first.report.decision_head);
    assert_eq!(still_first.report.proposal_head, first.report.proposal_head);
    assert_eq!(still_first.report.timeline, first.report.timeline);
    assert_ne!(
        still_first.projection_source_fingerprint,
        current.projection_source_fingerprint
    );
}

#[test]
fn prepared_report_reader_reuses_one_fsck_and_projection_across_sessions() {
    let temporary = TempDirectory::new();
    let repository_path = temporary.join("repo");
    run_creator_session(&options(
        &temporary,
        &repository_path,
        "batch-first",
        CreatorDisposition::Adopt,
    ))
    .unwrap();
    run_creator_session(&options(
        &temporary,
        &repository_path,
        "batch-second",
        CreatorDisposition::Reject,
    ))
    .unwrap();

    let repository = Repository::open(&repository_path).unwrap();
    let snapshot = repository.refs().snapshot().unwrap();
    let object_count_before = repository.objects().list_oids().unwrap().len();
    let (reader, first) =
        PreparedCreatorReportReader::prepare_with_report(&repository, &snapshot, "batch-first")
            .unwrap();

    // A late ordinary orphan changes the bounded fsck inventory but not the
    // fixed Ref projection. A fresh single-report reader observes it, while
    // the prepared batch reader retains the one verified object count.
    repository
        .put_blob(&b"late fsck inventory object"[..])
        .unwrap();
    assert_eq!(
        repository.objects().list_oids().unwrap().len(),
        object_count_before + 1
    );
    let fresh_before_erasure =
        creator_report_from_snapshot(&repository, &snapshot, "batch-second").unwrap();
    assert_eq!(
        fresh_before_erasure.report.fsck_objects,
        first.report.fsck_objects + 1
    );
    assert_eq!(
        fresh_before_erasure.projection_source_fingerprint,
        first.projection_source_fingerprint
    );

    // Add a new corrupt orphan Record without replacing or removing prepared
    // source data. Both full fsck and the Projection Tombstone inventory scan
    // reject it, so either operation being repeated per report would make the
    // cached reader fail here.
    let corrupt_oid = format!("record:sg-oid-v1:sha256:{}", "f".repeat(64));
    let corrupt_path = stored_object_path(&repository_path, &corrupt_oid);
    assert!(!corrupt_path.exists());
    fs::create_dir_all(corrupt_path.parent().unwrap()).unwrap();
    fs::write(&corrupt_path, br#"{"object_type":"record"}"#).unwrap();

    let second = reader.report("batch-second").unwrap();
    let first_again = reader.report("batch-first").unwrap();
    assert_eq!(first.report.disposition, CreatorDisposition::Adopt);
    assert_eq!(second.report.disposition, CreatorDisposition::Reject);
    assert_eq!(first_again.report, first.report);
    assert_eq!(second.report.fsck_objects, first.report.fsck_objects);
    assert_eq!(
        second.projection_source_fingerprint,
        first.projection_source_fingerprint
    );

    let error = match PreparedCreatorReportReader::prepare(&repository, &snapshot) {
        Ok(_) => panic!("fresh reader accepted a corrupt Record inventory"),
        Err(error) => error,
    };
    assert_eq!(error.code(), "oid_mismatch");
}

#[test]
fn legacy_snapshot_report_preserves_fsck_session_and_projection_error_order() {
    let temporary = TempDirectory::new();
    let repository_path = temporary.join("repo");
    run_creator_session(&options(
        &temporary,
        &repository_path,
        "projection-order",
        CreatorDisposition::Adopt,
    ))
    .unwrap();
    let repository = Repository::open(&repository_path).unwrap();
    let mut invalid_projection_snapshot = repository.refs().snapshot().unwrap();
    assert!(invalid_projection_snapshot.refs.len() >= 2);
    invalid_projection_snapshot.refs[1].updated_event_id =
        invalid_projection_snapshot.refs[0].updated_event_id;

    let missing = creator_report_from_snapshot(
        &repository,
        &invalid_projection_snapshot,
        "missing-but-valid-name",
    )
    .unwrap_err();
    assert!(
        matches!(missing, CreatorError::SessionNotFound(session) if session == "missing-but-valid-name")
    );

    let existing = creator_report_from_snapshot(
        &repository,
        &invalid_projection_snapshot,
        "projection-order",
    )
    .unwrap_err();
    assert_eq!(existing.code(), "projection_source_invalid");

    // The compatibility API has always run bounded fsck before looking up the
    // session. Keep that observable order as the prepared reader evolves.
    let corrupt_oid = format!("record:sg-oid-v1:sha256:{}", "e".repeat(64));
    let corrupt_path = stored_object_path(&repository_path, &corrupt_oid);
    assert!(!corrupt_path.exists());
    fs::create_dir_all(corrupt_path.parent().unwrap()).unwrap();
    fs::write(&corrupt_path, br#"{"object_type":"record"}"#).unwrap();

    let fsck_error = creator_report_from_snapshot(
        &repository,
        &invalid_projection_snapshot,
        "missing-but-valid-name",
    )
    .unwrap_err();
    assert_eq!(fsck_error.code(), "oid_mismatch");
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
