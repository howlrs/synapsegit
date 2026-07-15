use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use synapse_core::Repository;
use synapse_creator::{CreatorDisposition, CreatorRunOptions, run_creator_session};
use synapse_local_service::{
    CompleteState, CreatorSessionDetail, CreatorSessionState, ImageMediaType, ImageRole,
    IncompleteState, LocalService, ProjectRegistration, ProjectionState, ReflogQuery,
};
use synapse_sqlite::{RefUpdate, ReflogMetadata};

static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

struct TempDirectory(PathBuf);

impl TempDirectory {
    fn new() -> Self {
        let sequence = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "synapse-local-service-test-{}-{sequence}",
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

fn registration(key: &str, label: &str, path: &Path) -> ProjectRegistration {
    ProjectRegistration::new(key, label, path)
}

fn creator_options(
    temporary: &TempDirectory,
    repository: &Path,
    session: &str,
) -> CreatorRunOptions {
    let original_image = temporary.join(&format!("{session}-original.png"));
    let current_image = temporary.join(&format!("{session}-current.svg"));
    let ai_output = temporary.join(&format!("{session}-ai.gif"));
    fs::write(&original_image, b"\x89PNG\r\n\x1a\ncreator-original").unwrap();
    fs::write(
        &current_image,
        b"<svg xmlns='http://www.w3.org/2000/svg'><rect/></svg>",
    )
    .unwrap();
    fs::write(&ai_output, b"GIF89acreator-ai-output").unwrap();
    CreatorRunOptions {
        repository: repository.to_owned(),
        session: session.into(),
        original_image,
        current_image,
        ai_output,
        subject_label: "North wall mural".into(),
        creator_name: "Aki".into(),
        disposition: CreatorDisposition::Adopt,
        rationale: Some("Keep the caller-supplied proposal.".into()),
    }
}

#[test]
fn catalog_is_exact_sorted_and_never_serializes_paths() {
    let temporary = TempDirectory::new();
    let alpha = temporary.directory("alpha-private-repository");
    let beta = temporary.directory("beta-private-repository");
    let service = LocalService::new([
        registration("beta", "Beta project", &beta),
        registration("alpha", "Alpha project", &alpha),
    ])
    .unwrap();

    let projects = service.list_projects();
    assert_eq!(
        projects
            .projects
            .iter()
            .map(|project| project.project_key.as_str())
            .collect::<Vec<_>>(),
        ["alpha", "beta"]
    );
    assert!(projects.projects.iter().all(|project| {
        project.capabilities.read
            && !project.capabilities.creator_import
            && !project.capabilities.human_decision
            && !project.capabilities.fsck
            && !project.capabilities.archive_export
            && !project.capabilities.archive_restore
    }));
    let json = serde_json::to_string(&projects).unwrap();
    let debug = format!("{service:?}");
    assert!(!json.contains(alpha.to_str().unwrap()));
    assert!(!json.contains(beta.to_str().unwrap()));
    assert!(!debug.contains(alpha.to_str().unwrap()));
    assert!(!debug.contains(beta.to_str().unwrap()));

    let unknown = service.project_status("unknown").unwrap_err();
    let malformed = service.project_status("../alpha").unwrap_err();
    assert_eq!(unknown, malformed);
    assert_eq!(unknown.code(), "project_not_found");
}

#[test]
fn catalog_allows_duplicate_labels_but_rejects_duplicate_or_invalid_configuration() {
    let temporary = TempDirectory::new();
    let first = temporary.directory("first");
    let second = temporary.directory("second");

    let duplicate_key = LocalService::new([
        registration("same", "One", &first),
        registration("same", "Two", &second),
    ])
    .unwrap_err();
    assert_eq!(duplicate_key.code(), "local_request_denied");

    let duplicate_labels = LocalService::new([
        registration("one", "Same label", &first),
        registration("two", "Same label", &second),
    ])
    .unwrap();
    assert_eq!(duplicate_labels.list_projects().projects.len(), 2);

    let duplicate_path = LocalService::new([
        registration("one", "One", &first),
        registration("two", "Two", &first.join(".")),
    ])
    .unwrap_err();
    assert_eq!(duplicate_path.code(), "local_request_denied");

    let invalid_slug = LocalService::new([registration("Bad", "Bad", &first)]).unwrap_err();
    assert_eq!(invalid_slug.code(), "local_request_denied");

    let long_label = "画".repeat(301);
    let invalid_label = LocalService::new([registration("long", &long_label, &first)]).unwrap_err();
    assert_eq!(invalid_label.code(), "local_request_denied");
}

#[test]
fn empty_project_reads_share_a_stable_bounded_snapshot_shape() {
    let temporary = TempDirectory::new();
    let repository = temporary.directory("repository");
    let service = LocalService::new([registration("project", "Project", &repository)]).unwrap();

    let status = service.project_status("project").unwrap();
    let refs = service.list_refs("project").unwrap();
    let reflog = service
        .list_reflog("project", ReflogQuery::default())
        .unwrap();
    let sessions = service.list_creator_sessions("project").unwrap();

    assert_eq!(status.snapshot, refs.snapshot);
    assert_eq!(status.snapshot, reflog.snapshot);
    assert_eq!(status.snapshot, sessions.snapshot);
    assert!(status.snapshot.watermark.starts_with("sha256:"));
    assert_eq!(status.snapshot.watermark.len(), 71);
    assert_eq!(status.snapshot.ref_count, 0);
    assert_eq!(status.snapshot.projection_source_fingerprint, None);
    assert_eq!(status.projection_state, ProjectionState::NotBuilt);
    assert_eq!(status.last_fsck, None);
    assert_eq!(status.creator_session_counts.complete, 0);
    assert_eq!(status.creator_session_counts.pending_review, 0);
    assert_eq!(status.creator_session_counts.incomplete, 0);
    assert!(refs.refs.is_empty());
    assert!(reflog.entries.is_empty());
    assert_eq!(reflog.next_after_event_id, None);
    assert!(sessions.sessions.is_empty());

    let status_json = serde_json::to_value(status).unwrap();
    assert_eq!(status_json["last_fsck"], Value::Null);
    assert_eq!(
        status_json["snapshot"]["projection_source_fingerprint"],
        Value::Null
    );
}

#[test]
fn facade_lists_pages_and_reports_from_coherent_snapshots() {
    let temporary = TempDirectory::new();
    let repository = temporary.directory("repository");
    let receipt = run_creator_session(&creator_options(
        &temporary,
        &repository,
        "complete-session",
    ))
    .unwrap();
    let service = LocalService::new([registration("project", "Project", &repository)]).unwrap();

    let refs = service.list_refs("project").unwrap();
    assert!(refs.refs.windows(2).all(|pair| pair[0].name < pair[1].name));
    assert!(
        refs.refs
            .iter()
            .all(|reference| reference.updated_event_id.parse::<i64>().is_ok())
    );

    let first_page = service
        .list_reflog(
            "project",
            ReflogQuery {
                limit: 1,
                ..ReflogQuery::default()
            },
        )
        .unwrap();
    assert_eq!(first_page.snapshot, refs.snapshot);
    assert_eq!(first_page.entries.len(), 1);
    let cursor = first_page.next_after_event_id.clone().unwrap();
    let second_page = service
        .list_reflog(
            "project",
            ReflogQuery {
                after_event_id: Some(cursor),
                limit: 500,
                ..ReflogQuery::default()
            },
        )
        .unwrap();
    assert_eq!(second_page.snapshot, refs.snapshot);
    assert!(!second_page.entries.is_empty());
    assert!(second_page.entries.iter().all(|entry| {
        entry.event_id.parse::<i64>().is_ok() && entry.occurred_at_unix_nanos.parse::<i64>().is_ok()
    }));

    let sessions = service.list_creator_sessions("project").unwrap();
    assert_eq!(sessions.snapshot, refs.snapshot);
    assert_eq!(sessions.sessions.len(), 1);
    assert_eq!(sessions.sessions[0].session, "complete-session");
    assert_eq!(sessions.sessions[0].state, CreatorSessionState::Complete);

    let status = service.project_status("project").unwrap();
    assert_eq!(status.snapshot, refs.snapshot);
    assert_eq!(status.creator_session_counts.complete, 1);
    assert_eq!(status.creator_session_counts.incomplete, 0);
    assert_eq!(status.last_fsck, None);

    let detail = service
        .get_creator_session("project", "complete-session")
        .unwrap();
    let CreatorSessionDetail::Complete(detail) = detail else {
        panic!("complete creator session was not returned as complete");
    };
    assert_eq!(detail.state, CompleteState::Complete);
    assert_eq!(detail.report.snapshot.watermark, refs.snapshot.watermark);
    assert!(
        detail
            .report
            .snapshot
            .projection_source_fingerprint
            .as_deref()
            .unwrap()
            .starts_with("projection-source-v1:sha256:")
    );
    assert_eq!(detail.report.agent_id, receipt.agent_id);
    assert_eq!(detail.report.proposal_attributed_to_agent, receipt.agent_id);
    assert_eq!(detail.report.creator_id, receipt.creator_id);
    assert_eq!(detail.report.reviewed_by_human, receipt.creator_id);
    assert_eq!(detail.report.ai_output_source, "caller_supplied");
    assert_eq!(detail.report.disposition, "adopt");
    let comparison = detail.report.comparison.unwrap();
    assert_eq!(comparison.comparability, "partial");
    assert_eq!(comparison.reason_codes[0], "byte_identity_only");
    assert!(!comparison.warnings.is_empty());
}

#[test]
fn both_refs_with_a_non_decision_head_remain_incomplete() {
    let temporary = TempDirectory::new();
    let repository_path = temporary.directory("repository");
    let receipt = run_creator_session(&creator_options(
        &temporary,
        &repository_path,
        "source-session",
    ))
    .unwrap();
    let mut repository = Repository::open(&repository_path).unwrap();
    repository
        .update_ref(RefUpdate {
            ref_name: "decision/creator/incomplete-session",
            expected_head: None,
            new_head: &receipt.base_head,
            metadata: ReflogMetadata::at(100),
        })
        .unwrap();
    repository
        .update_ref(RefUpdate {
            ref_name: "proposal/creator-agent/incomplete-session",
            expected_head: None,
            new_head: &receipt.proposal_head,
            metadata: ReflogMetadata::at(101),
        })
        .unwrap();
    repository
        .update_ref(RefUpdate {
            ref_name: "decision/creator/one-ref-session",
            expected_head: None,
            new_head: &receipt.base_head,
            metadata: ReflogMetadata::at(102),
        })
        .unwrap();
    drop(repository);

    let service =
        LocalService::new([registration("project", "Project", &repository_path)]).unwrap();
    let sessions = service.list_creator_sessions("project").unwrap();
    let incomplete = sessions
        .sessions
        .iter()
        .find(|session| session.session == "incomplete-session")
        .unwrap();
    assert_eq!(incomplete.state, CreatorSessionState::Incomplete);
    assert!(incomplete.proposal_head.is_some());
    assert!(incomplete.decision_head.is_some());
    let one_ref = sessions
        .sessions
        .iter()
        .find(|session| session.session == "one-ref-session")
        .unwrap();
    assert_eq!(one_ref.state, CreatorSessionState::Incomplete);
    assert!(one_ref.proposal_head.is_none());
    assert!(one_ref.decision_head.is_some());

    let detail = service
        .get_creator_session("project", "incomplete-session")
        .unwrap();
    let CreatorSessionDetail::Incomplete(detail) = detail else {
        panic!("incomplete creator session was not returned as incomplete");
    };
    assert_eq!(detail.state, IncompleteState::Incomplete);
    assert!(!detail.recovery_supported);
    assert_eq!(detail.snapshot.projection_source_fingerprint, None);

    let image_error = service
        .get_creator_session_image("project", "incomplete-session", ImageRole::Original)
        .unwrap_err();
    assert_eq!(image_error.code(), "creator_session_incomplete");
}

#[test]
fn images_are_role_resolved_verified_and_unknown_media_is_attachment_only() {
    let temporary = TempDirectory::new();
    let repository = temporary.directory("repository");
    let receipt =
        run_creator_session(&creator_options(&temporary, &repository, "image-session")).unwrap();
    let service = LocalService::new([registration("project", "Project", &repository)]).unwrap();

    let original = service
        .get_creator_session_image("project", "image-session", ImageRole::Original)
        .unwrap();
    assert_eq!(original.blob_oid, receipt.original_blob_oid);
    assert_eq!(original.media_type, ImageMediaType::Png);
    assert_eq!(original.media_type.content_type(), "image/png");
    assert!(!original.media_type.is_attachment());
    assert!(!original.is_attachment());
    assert!(original.bytes.starts_with(b"\x89PNG\r\n\x1a\n"));

    let current = service
        .get_creator_session_image("project", "image-session", ImageRole::Current)
        .unwrap();
    assert_eq!(current.blob_oid, receipt.current_blob_oid);
    assert_eq!(current.media_type, ImageMediaType::OctetStream);
    assert_eq!(
        current.media_type.content_type(),
        "application/octet-stream"
    );
    assert!(current.media_type.is_attachment());
    assert!(current.is_attachment());
    assert!(current.bytes.starts_with(b"<svg"));

    let ai_output = service
        .get_creator_session_image("project", "image-session", ImageRole::AiOutput)
        .unwrap();
    assert_eq!(ai_output.blob_oid, receipt.ai_output_blob_oid);
    assert_eq!(ai_output.media_type, ImageMediaType::Gif);
    assert!(!ai_output.media_type.is_attachment());
}

#[test]
fn invalid_reflog_inputs_fail_closed() {
    let temporary = TempDirectory::new();
    let repository = temporary.directory("repository");
    let service = LocalService::new([registration("project", "Project", &repository)]).unwrap();

    for after_event_id in ["", "01", "-1", "9223372036854775808"] {
        let error = service
            .list_reflog(
                "project",
                ReflogQuery {
                    after_event_id: Some(after_event_id.into()),
                    ..ReflogQuery::default()
                },
            )
            .unwrap_err();
        assert_eq!(error.code(), "local_request_denied");
    }
    let error = service
        .list_reflog(
            "project",
            ReflogQuery {
                limit: 501,
                ..ReflogQuery::default()
            },
        )
        .unwrap_err();
    assert_eq!(error.code(), "resource_limit");
}

#[test]
fn service_errors_create_safe_problem_dtos() {
    let temporary = TempDirectory::new();
    let repository = temporary.directory("repository-private-path");
    let service = LocalService::new([registration("project", "Project", &repository)]).unwrap();
    let error = service.project_status("missing").unwrap_err();
    let problem = error.to_problem(404, "request-1");
    let json = serde_json::to_string(&problem).unwrap();
    assert_eq!(problem.r#type, "urn:synapsegit:error:project_not_found");
    assert_eq!(problem.code, "project_not_found");
    assert_eq!(problem.status, 404);
    assert!(!problem.retryable);
    assert!(!json.contains(repository.to_str().unwrap()));

    fs::remove_dir_all(&repository).unwrap();
    fs::write(&repository, b"not a directory").unwrap();
    let storage_error = service.project_status("project").unwrap_err();
    assert_eq!(storage_error.code(), "storage_error");
    assert!(
        storage_error
            .diagnostic()
            .is_some_and(|diagnostic| diagnostic.contains(repository.to_str().unwrap()))
    );
    let storage_problem = storage_error.to_problem(500, "request-2");
    let storage_json = serde_json::to_string(&storage_problem).unwrap();
    assert!(!storage_json.contains(repository.to_str().unwrap()));
}
