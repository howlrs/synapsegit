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
        .unwrap();
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
        .unwrap();
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
