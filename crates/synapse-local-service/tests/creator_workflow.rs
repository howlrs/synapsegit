use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use synapse_core::Repository;
use synapse_local_service::{
    BeginCreatorSessionRequest, CompleteState, CreatorDecision, CreatorDecisionRequest,
    CreatorSessionDetail, CreatorSessionState, ImageMediaType, ImageRole, LocalService,
    MAX_PENDING_CREATOR_SESSIONS_PER_PROJECT, PendingReviewState, ProjectRegistration,
};
use synapse_sqlite::{RefUpdate, ReflogMetadata};

static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

struct TempDirectory(PathBuf);

impl TempDirectory {
    fn new() -> Self {
        let sequence = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "synapse-local-service-creator-test-{}-{sequence}",
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

    fn join(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for TempDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn service(repository: &Path) -> LocalService {
    LocalService::new([ProjectRegistration::new(
        "project",
        "Creator project",
        repository,
    )])
    .unwrap()
}

fn begin_request(temporary: &TempDirectory, session: &str) -> BeginCreatorSessionRequest {
    let original_image = temporary.join(&format!("{session}-original.png"));
    let current_image = temporary.join(&format!("{session}-current.bin"));
    let ai_output = temporary.join(&format!("{session}-ai.gif"));
    fs::write(&original_image, b"\x89PNG\r\n\x1a\ncreator-original").unwrap();
    fs::write(&current_image, b"creator-current").unwrap();
    fs::write(&ai_output, b"GIF89acreator-ai-output").unwrap();
    BeginCreatorSessionRequest {
        generation_note: None,
        session: session.into(),
        subject_label: "North wall mural".into(),
        creator_name: "Aki".into(),
        original_image,
        current_image,
        ai_output,
    }
}

fn decision(review_id: impl Into<String>, disposition: CreatorDecision) -> CreatorDecisionRequest {
    CreatorDecisionRequest {
        annotations: None,
        review_id: review_id.into(),
        disposition,
        rationale: Some("Reviewed in the local application.".into()),
    }
}

#[test]
fn begin_overlays_ready_state_and_decide_rebuilds_a_complete_report() {
    let temporary = TempDirectory::new();
    let repository = temporary.directory("repository");
    let service = service(&repository);

    let pending = service
        .begin_creator_session(
            "project",
            "server-instance-a",
            begin_request(&temporary, "review-session"),
        )
        .unwrap();
    assert_eq!(pending.state, PendingReviewState::PendingReview);
    assert_eq!(pending.server_instance, "server-instance-a");
    assert_eq!(pending.review_id.len(), 64);
    assert!(
        pending
            .review_id
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
    );
    assert_eq!(pending.ai_output_source, "caller_supplied");
    assert_eq!(pending.comparison.comparability, "partial");
    assert!(pending.snapshot.projection_source_fingerprint.is_none());
    let pending_json = serde_json::to_string(&pending).unwrap();
    assert!(!pending_json.contains(repository.to_str().unwrap()));
    assert!(!pending_json.contains(temporary.0.to_str().unwrap()));

    let sessions = service.list_creator_sessions("project").unwrap();
    assert_eq!(sessions.sessions.len(), 1);
    assert_eq!(
        sessions.sessions[0].state,
        CreatorSessionState::PendingReview
    );
    let status = service.project_status("project").unwrap();
    assert_eq!(status.creator_session_counts.pending_review, 1);
    assert_eq!(status.creator_session_counts.complete, 0);
    assert_eq!(status.creator_session_counts.incomplete, 0);

    let CreatorSessionDetail::PendingReview(refreshed) = service
        .get_creator_session("project", "review-session")
        .unwrap()
    else {
        panic!("ready registry entry was not overlaid as pending_review");
    };
    assert_eq!(refreshed.review_id, pending.review_id);
    assert_eq!(refreshed.proposal_head, pending.proposal_head);

    let diagnostic = service
        .get_creator_session_diagnostic("project", "review-session")
        .unwrap();
    assert_eq!(diagnostic.state, CreatorSessionState::PendingReview);
    assert_eq!(
        diagnostic.proposal_head.as_deref(),
        Some(pending.proposal_head.as_str())
    );
    assert!(diagnostic.decision_head.is_some());
    assert!(!diagnostic.automatic_resume_supported);
    assert!(!diagnostic.automatic_cleanup_supported);
    assert!(
        diagnostic
            .recommended_action
            .contains("running localhost process")
    );

    let original = service
        .get_creator_session_image("project", "review-session", ImageRole::Original)
        .unwrap();
    assert_eq!(original.blob_oid, pending.original_blob_oid);
    assert_eq!(original.media_type, ImageMediaType::Png);
    let ai_output = service
        .get_creator_session_image("project", "review-session", ImageRole::AiOutput)
        .unwrap();
    assert_eq!(ai_output.blob_oid, pending.ai_output_blob_oid);
    assert_eq!(ai_output.media_type, ImageMediaType::Gif);

    let complete = service
        .decide_creator_session(
            "project",
            "review-session",
            "server-instance-a",
            decision(&pending.review_id, CreatorDecision::Adopt),
        )
        .unwrap()
        .into_complete()
        .expect("unchanged repository returns the rebuilt complete report");
    assert_eq!(complete.state, CompleteState::Complete);
    assert_eq!(complete.report.disposition, "adopt");
    assert!(complete.report.selected_ai_output);
    assert!(
        complete
            .report
            .snapshot
            .projection_source_fingerprint
            .as_deref()
            .unwrap()
            .starts_with("projection-source-v1:sha256:")
    );

    let sessions = service.list_creator_sessions("project").unwrap();
    assert_eq!(sessions.sessions[0].state, CreatorSessionState::Complete);
    let status = service.project_status("project").unwrap();
    assert_eq!(status.creator_session_counts.pending_review, 0);
    assert_eq!(status.creator_session_counts.complete, 1);
    let retry = service
        .decide_creator_session(
            "project",
            "review-session",
            "server-instance-a",
            decision(pending.review_id, CreatorDecision::Reject),
        )
        .unwrap_err();
    assert_eq!(retry.code(), "creator_review_state_lost");
}

#[test]
fn rejected_inputs_and_wrong_bindings_leave_the_ready_review_available() {
    let temporary = TempDirectory::new();
    let repository = temporary.directory("repository");
    let other_repository = temporary.directory("other-repository");
    let service = LocalService::new([
        ProjectRegistration::new("project", "Creator project", &repository),
        ProjectRegistration::new("other", "Other project", &other_repository),
    ])
    .unwrap();
    let pending = service
        .begin_creator_session(
            "project",
            "server-instance-a",
            begin_request(&temporary, "bound-session"),
        )
        .unwrap();

    let wrong_server = service
        .decide_creator_session(
            "project",
            "bound-session",
            "server-instance-b",
            decision(&pending.review_id, CreatorDecision::Reject),
        )
        .unwrap_err();
    assert_eq!(wrong_server.code(), "creator_review_state_lost");

    let unknown_review = service
        .decide_creator_session(
            "project",
            "bound-session",
            "server-instance-a",
            decision("a".repeat(64), CreatorDecision::Reject),
        )
        .unwrap_err();
    assert_eq!(unknown_review.code(), "creator_review_state_lost");

    let wrong_session = service
        .decide_creator_session(
            "project",
            "other-session",
            "server-instance-a",
            decision(&pending.review_id, CreatorDecision::Reject),
        )
        .unwrap_err();
    assert_eq!(wrong_session.code(), "creator_review_state_lost");

    let wrong_project = service
        .decide_creator_session(
            "other",
            "bound-session",
            "server-instance-a",
            decision(&pending.review_id, CreatorDecision::Reject),
        )
        .unwrap_err();
    assert_eq!(wrong_project.code(), "creator_review_state_lost");

    let invalid_rationale = service
        .decide_creator_session(
            "project",
            "bound-session",
            "server-instance-a",
            CreatorDecisionRequest {
                annotations: None,
                review_id: pending.review_id.clone(),
                disposition: CreatorDecision::Defer,
                rationale: Some("x".repeat(5_001)),
            },
        )
        .unwrap_err();
    assert_eq!(invalid_rationale.code(), "usage_error");

    let CreatorSessionDetail::PendingReview(still_ready) = service
        .get_creator_session("project", "bound-session")
        .unwrap()
    else {
        panic!("input rejection consumed a ready review");
    };
    assert_eq!(still_ready.review_id, pending.review_id);

    let complete = service
        .decide_creator_session(
            "project",
            "bound-session",
            "server-instance-a",
            decision(pending.review_id, CreatorDecision::Defer),
        )
        .unwrap()
        .into_complete()
        .expect("unchanged repository returns the rebuilt complete report");
    assert_eq!(complete.report.disposition, "defer");
}

#[test]
fn changed_live_heads_make_pending_authority_unavailable_without_retry() {
    let temporary = TempDirectory::new();
    let repository_path = temporary.directory("repository");
    let service = service(&repository_path);
    let pending = service
        .begin_creator_session(
            "project",
            "server-instance-a",
            begin_request(&temporary, "stale-session"),
        )
        .unwrap();
    let summary = service
        .list_creator_sessions("project")
        .unwrap()
        .sessions
        .pop()
        .unwrap();
    let base_head = summary.decision_head.unwrap();

    let mut repository = Repository::open(&repository_path).unwrap();
    repository
        .update_ref(RefUpdate {
            ref_name: "decision/creator/stale-session",
            expected_head: Some(&base_head),
            new_head: &pending.proposal_head,
            metadata: ReflogMetadata::at(500),
        })
        .unwrap();
    drop(repository);

    let CreatorSessionDetail::Incomplete(_) = service
        .get_creator_session("project", "stale-session")
        .unwrap()
    else {
        panic!("changed base head remained exposed as a ready review");
    };
    let image_error = service
        .get_creator_session_image("project", "stale-session", ImageRole::Original)
        .unwrap_err();
    assert_eq!(image_error.code(), "creator_session_incomplete");
    let decision_error = service
        .decide_creator_session(
            "project",
            "stale-session",
            "server-instance-a",
            decision(pending.review_id, CreatorDecision::Adopt),
        )
        .unwrap_err();
    assert_eq!(decision_error.code(), "creator_review_state_lost");
    let problem = decision_error.to_problem(409, "request-stale");
    assert!(
        !serde_json::to_string(&problem)
            .unwrap()
            .contains(repository_path.to_str().unwrap())
    );
}

#[test]
fn failed_staged_input_is_safe_and_releases_its_prepublication_reservation() {
    let temporary = TempDirectory::new();
    let repository_path = temporary.directory("repository");
    let service = service(&repository_path);
    let missing_path = temporary.join("private-missing-image.png");
    let mut request = begin_request(&temporary, "retry-session");
    request.original_image = missing_path.clone();

    let error = service
        .begin_creator_session("project", "server-instance-a", request)
        .unwrap_err();
    assert_eq!(error.code(), "storage_error");
    assert!(
        error
            .diagnostic()
            .is_some_and(|diagnostic| diagnostic.contains(missing_path.to_str().unwrap()))
    );
    assert!(
        !serde_json::to_string(&error.to_problem(500, "request-input"))
            .unwrap()
            .contains(missing_path.to_str().unwrap())
    );

    let pending = service
        .begin_creator_session(
            "project",
            "server-instance-a",
            begin_request(&temporary, "retry-session"),
        )
        .unwrap();
    assert_eq!(pending.state, PendingReviewState::PendingReview);
}

#[test]
fn process_restart_exposes_a_published_proposal_as_incomplete_without_reconstruction() {
    let temporary = TempDirectory::new();
    let repository = temporary.directory("repository");
    let initial_service = service(&repository);
    let pending = initial_service
        .begin_creator_session(
            "project",
            "server-instance-before-restart",
            begin_request(&temporary, "restart-session"),
        )
        .unwrap();
    drop(initial_service);

    let restarted = service(&repository);
    let CreatorSessionDetail::Incomplete(incomplete) = restarted
        .get_creator_session("project", "restart-session")
        .unwrap()
    else {
        panic!("a restarted service reconstructed process-local review authority");
    };
    assert_eq!(incomplete.session, "restart-session");
    assert!(!incomplete.recovery_supported);
    assert_eq!(
        restarted.list_creator_sessions("project").unwrap().sessions[0].state,
        CreatorSessionState::Incomplete
    );

    let error = restarted
        .decide_creator_session(
            "project",
            "restart-session",
            "server-instance-after-restart",
            decision(pending.review_id, CreatorDecision::Adopt),
        )
        .unwrap_err();
    assert_eq!(error.code(), "creator_review_state_lost");
}

#[test]
fn project_capacity_is_reserved_before_a_ninth_proposal_can_publish() {
    let temporary = TempDirectory::new();
    let repository_path = temporary.directory("repository");
    let service = service(&repository_path);
    for index in 0..MAX_PENDING_CREATOR_SESSIONS_PER_PROJECT {
        service
            .begin_creator_session(
                "project",
                "server-instance-a",
                begin_request(&temporary, &format!("pending-{index}")),
            )
            .unwrap();
    }

    let error = service
        .begin_creator_session(
            "project",
            "server-instance-a",
            begin_request(&temporary, "pending-over-limit"),
        )
        .unwrap_err();
    assert_eq!(error.code(), "resource_limit");
    assert_eq!(
        service
            .list_creator_sessions("project")
            .unwrap()
            .sessions
            .len(),
        MAX_PENDING_CREATOR_SESSIONS_PER_PROJECT
    );
    let repository = Repository::open(&repository_path).unwrap();
    assert!(
        repository
            .refs()
            .get("decision/creator/pending-over-limit")
            .unwrap()
            .is_none()
    );
    assert!(
        repository
            .refs()
            .get("proposal/creator-agent/pending-over-limit")
            .unwrap()
            .is_none()
    );
}

#[test]
fn concurrent_begins_publish_as_serialized_project_writer_operations() {
    let temporary = TempDirectory::new();
    let repository_path = temporary.directory("repository");
    let service = Arc::new(service(&repository_path));
    let barrier = Arc::new(std::sync::Barrier::new(3));
    let mut workers = Vec::new();
    for session in ["parallel-one", "parallel-two"] {
        let worker_service = service.clone();
        let worker_barrier = barrier.clone();
        let request = begin_request(&temporary, session);
        workers.push(std::thread::spawn(move || {
            worker_barrier.wait();
            worker_service.begin_creator_session("project", "server-instance-a", request)
        }));
    }
    barrier.wait();
    for worker in workers {
        worker.join().unwrap().unwrap();
    }

    let repository = Repository::open(&repository_path).unwrap();
    let reflog = repository.refs().reflog().unwrap();
    assert_eq!(reflog.len(), 4);
    for pair in reflog.chunks_exact(2) {
        let decision_session = pair[0]
            .ref_name
            .strip_prefix("decision/creator/")
            .expect("begin publishes its decision Ref first");
        let proposal_session = pair[1]
            .ref_name
            .strip_prefix("proposal/creator-agent/")
            .expect("begin publishes its proposal Ref second");
        assert_eq!(decision_session, proposal_session);
    }
    assert!(repository.fsck().unwrap().is_clean());
}

#[test]
fn concurrent_decisions_publish_at_most_once() {
    let temporary = TempDirectory::new();
    let repository = temporary.directory("repository");
    let service = Arc::new(service(&repository));
    let pending = service
        .begin_creator_session(
            "project",
            "server-instance-a",
            begin_request(&temporary, "exclusive-session"),
        )
        .unwrap();
    let first_service = service.clone();
    let first_review = pending.review_id.clone();
    let first = std::thread::spawn(move || {
        first_service.decide_creator_session(
            "project",
            "exclusive-session",
            "server-instance-a",
            decision(first_review, CreatorDecision::Adopt),
        )
    });
    let second_service = service.clone();
    let second_review = pending.review_id;
    let second = std::thread::spawn(move || {
        second_service.decide_creator_session(
            "project",
            "exclusive-session",
            "server-instance-a",
            decision(second_review, CreatorDecision::Reject),
        )
    });
    let results = [first.join().unwrap(), second.join().unwrap()];
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    let losing_code = results
        .iter()
        .find_map(|result| result.as_ref().err())
        .unwrap()
        .code();
    assert!(matches!(
        losing_code,
        "creator_review_busy" | "creator_review_state_lost"
    ));
}

#[test]
fn decision_pin_requests_validate_json_budget_and_restore_pending_after_rejection() {
    use synapse_local_service::{CreatorAnnotations, CreatorImageRole, CreatorPin};
    let temporary = TempDirectory::new();
    let repository = temporary.directory("repository");
    let service = service(&repository);
    let pending = service
        .begin_creator_session(
            "project",
            "server-instance-a",
            begin_request(&temporary, "pin-review"),
        )
        .unwrap();
    let annotations = CreatorAnnotations {
        format: "synapsegit-creator-decision-pins-v1".into(),
        pins: vec![CreatorPin {
            role: CreatorImageRole::AiOutput,
            blob_oid: pending.ai_output_blob_oid.clone(),
            x: 123456,
            y: 654321,
            note: "位置を確認".into(),
        }],
    };
    let mut request = decision(&pending.review_id, CreatorDecision::Defer);
    request.annotations = Some(annotations.clone());
    request.rationale = Some("\"".repeat(4100));
    assert_eq!(
        service
            .decide_creator_session(
                "project",
                "pin-review",
                "server-instance-a",
                request.clone()
            )
            .unwrap_err()
            .code(),
        "usage_error"
    );
    request.rationale = Some("別の理由".into());
    request.annotations.as_mut().unwrap().pins[0].blob_oid = pending.original_blob_oid.clone();
    assert!(
        service
            .decide_creator_session(
                "project",
                "pin-review",
                "server-instance-a",
                request.clone()
            )
            .is_err()
    );
    let CreatorSessionDetail::PendingReview(still_pending) = service
        .get_creator_session("project", "pin-review")
        .unwrap()
    else {
        panic!("validation consumed review");
    };
    assert_eq!(still_pending.review_id, pending.review_id);
    request.annotations = Some(annotations.clone());
    service
        .decide_creator_session("project", "pin-review", "server-instance-a", request)
        .unwrap();
    let CreatorSessionDetail::Complete(complete) = service
        .get_creator_session("project", "pin-review")
        .unwrap()
    else {
        panic!("not complete");
    };
    assert_eq!(complete.report.annotations, Some(annotations));
    assert_eq!(complete.report.rationale.as_deref(), Some("別の理由"));
}

#[test]
fn derivation_confirmation_is_scoped_revalidated_and_consumed() {
    use synapse_local_service::BeginDerivedCreatorSessionRequest;
    let temporary = TempDirectory::new();
    let repository = temporary.directory("derived");
    let service = service(&repository);
    let pending = service
        .begin_creator_session("project", "instance", begin_request(&temporary, "source"))
        .unwrap();
    assert!(
        service
            .prepare_creator_source("project", "source", "instance")
            .is_err()
    );
    let source = service
        .decide_creator_session(
            "project",
            "source",
            "instance",
            decision(&pending.review_id, CreatorDecision::Adopt),
        )
        .unwrap()
        .into_complete()
        .unwrap();
    let preview = service
        .prepare_creator_source("project", "source", "instance")
        .unwrap();
    assert_eq!(preview.creator_name, "Aki");
    assert_eq!(
        preview,
        service
            .prepare_creator_source("project", "source", "instance")
            .unwrap()
    );
    let candidate = temporary.join("new.gif");
    fs::write(&candidate, b"GIF89anew").unwrap();
    let request = || BeginDerivedCreatorSessionRequest {
        confirmation_id: preview.confirmation_id.clone(),
        session: "child".into(),
        creator_name: "New creator".into(),
        subject_label: "New subject".into(),
        ai_output: candidate.clone(),
        generation_note: None,
    };
    let before = Repository::open(&repository)
        .unwrap()
        .refs()
        .snapshot()
        .unwrap();
    for (source, instance) in [("source", "different"), ("other", "instance")] {
        assert!(
            service
                .begin_derived_creator_session("project", source, instance, request())
                .is_err()
        );
        assert_eq!(
            Repository::open(&repository)
                .unwrap()
                .refs()
                .snapshot()
                .unwrap(),
            before
        );
    }
    let mut missing = request();
    missing.ai_output = temporary.join("missing");
    assert!(
        service
            .begin_derived_creator_session("project", "source", "instance", missing)
            .is_err()
    );
    assert_eq!(
        Repository::open(&repository)
            .unwrap()
            .refs()
            .snapshot()
            .unwrap(),
        before
    );
    let child = service
        .begin_derived_creator_session("project", "source", "instance", request())
        .unwrap();
    assert_eq!(child.original_blob_oid, source.report.original_blob_oid);
    assert_eq!(child.current_blob_oid, source.report.current_blob_oid);
    assert_ne!(child.current_blob_oid, source.report.ai_output_blob_oid);
    assert!(
        service
            .begin_derived_creator_session("project", "source", "instance", request())
            .is_err()
    );
}

#[test]
fn public_sidecar_uses_only_fresh_text_and_does_not_write_core() {
    use synapse_local_service::PresentationSidecarRequest;
    let temporary = TempDirectory::new();
    let repository = temporary.directory("public-sidecar");
    let service = service(&repository);
    let mut request = begin_request(&temporary, "source");
    request.generation_note = Some(synapse_local_service::CreatorGenerationNote {
        prompt: "PRIVATE_PROMPT_CANARY".into(),
        ..Default::default()
    });
    let pending = service
        .begin_creator_session("project", "instance", request)
        .unwrap();
    assert!(
        service
            .prepare_presentation_sidecar(
                "project",
                PresentationSidecarRequest {
                    session: "source".into(),
                    ..Default::default()
                }
            )
            .is_err()
    );
    let mut decision = decision(&pending.review_id, CreatorDecision::Reject);
    decision.rationale = Some("PRIVATE_RATIONALE_CANARY".into());
    service
        .decide_creator_session("project", "source", "instance", decision)
        .unwrap();
    let before = Repository::open(&repository)
        .unwrap()
        .refs()
        .snapshot()
        .unwrap();
    let sidecar = service
        .prepare_presentation_sidecar(
            "project",
            PresentationSidecarRequest {
                session: "source".into(),
                title: Some("".into()),
                summary: Some("公開用\n別の文章".into()),
                ..Default::default()
            },
        )
        .unwrap();
    assert!(!sidecar.toml.contains("PRIVATE_"));
    assert!(!sidecar.toml.contains("creator-current"));
    let path = temporary.join("presentation.toml");
    fs::write(&path, &sidecar.toml).unwrap();
    let parsed = synapse_publication::load_presentation(path).unwrap();
    assert_eq!(parsed.title, None);
    assert_eq!(parsed.summary.as_deref(), Some("公開用\n別の文章"));
    assert_eq!(parsed.sessions.len(), 1);
    assert!(
        service
            .prepare_presentation_sidecar(
                "project",
                PresentationSidecarRequest {
                    session: "missing".into(),
                    ..Default::default()
                }
            )
            .is_err()
    );
    assert_eq!(
        Repository::open(&repository)
            .unwrap()
            .refs()
            .snapshot()
            .unwrap(),
        before
    );
}

#[test]
fn derivation_rejects_changed_missing_corrupt_and_tombstoned_source() {
    use synapse_local_service::BeginDerivedCreatorSessionRequest;
    for mutation in ["changed", "missing", "corrupt", "tombstone"] {
        let temporary = TempDirectory::new();
        let repository = temporary.directory(mutation);
        let service = service(&repository);
        let pending = service
            .begin_creator_session("project", "instance", begin_request(&temporary, "source"))
            .unwrap();
        let source = service
            .decide_creator_session(
                "project",
                "source",
                "instance",
                decision(&pending.review_id, CreatorDecision::Adopt),
            )
            .unwrap()
            .into_complete()
            .unwrap();
        let preview = service
            .prepare_creator_source("project", "source", "instance")
            .unwrap();
        let mut storage = Repository::open(&repository).unwrap();
        let digest = source.report.original_blob_oid.rsplit(':').next().unwrap();
        let blob_path = repository
            .join("cas/objects/blob")
            .join(&digest[..2])
            .join(&digest[2..]);
        match mutation {
            "changed" => {
                storage
                    .update_ref(RefUpdate {
                        ref_name: &source.report.decision_ref,
                        expected_head: Some(&source.report.decision_head),
                        new_head: &source.report.base_head,
                        metadata: ReflogMetadata {
                            occurred_at_unix_nanos: 1,
                            actor: None,
                            message: None,
                        },
                    })
                    .unwrap();
            }
            "missing" => fs::remove_file(blob_path).unwrap(),
            "corrupt" => fs::write(blob_path, b"different bytes").unwrap(),
            "tombstone" => {
                let mut tombstone: serde_json::Value = serde_json::from_str(include_str!(
                    "../../../spec/core/v0.1/fixtures/tombstone.json"
                ))
                .unwrap();
                tombstone["payload"]["target_ref"] =
                    serde_json::json!(source.report.original_blob_oid);
                storage
                    .put_object(&serde_json::to_vec(&tombstone).unwrap())
                    .unwrap();
                fs::remove_file(blob_path).unwrap();
            }
            _ => unreachable!(),
        }
        let before = storage.refs().snapshot().unwrap();
        let reflog = storage.refs().reflog().unwrap();
        let request = BeginDerivedCreatorSessionRequest {
            confirmation_id: preview.confirmation_id.clone(),
            session: "child".into(),
            creator_name: "New".into(),
            subject_label: "New".into(),
            ai_output: temporary.join("source-ai.gif"),
            generation_note: None,
        };
        assert!(
            service
                .get_creator_source_image(
                    "project",
                    "source",
                    "instance",
                    &preview.confirmation_id,
                    ImageRole::Original
                )
                .is_err(),
            "{mutation}"
        );
        assert!(
            service
                .begin_derived_creator_session("project", "source", "instance", request)
                .is_err(),
            "{mutation}"
        );
        assert_eq!(storage.refs().snapshot().unwrap(), before, "{mutation}");
        assert_eq!(storage.refs().reflog().unwrap(), reflog, "{mutation}");
    }
}
