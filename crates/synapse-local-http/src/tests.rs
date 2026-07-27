use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::header::{CONTENT_DISPOSITION, CONTENT_TYPE, HOST, ORIGIN};
use axum::http::{Request, StatusCode, header};
use axum::response::Response;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, UNIX_EPOCH};
use synapse_creator::{
    CreatorBeginOptions, CreatorDisposition, CreatorRunOptions, begin_creator_session,
    run_creator_session,
};
use synapse_local_service::{
    CommittedCreatorSession, CommittedState, CreatorDecision, CreatorDecisionReceipt,
    CreatorDecisionResponse, LocalService, OperationKind, OperationResult, OperationState,
};
use tokio::sync::Semaphore;
use tower::ServiceExt;

use crate::app::{
    MAX_CONCURRENT_CREATOR_UPLOADS, build_with_identity, civil_date_from_unix_days,
    monotonic_operation_timestamp,
};
use crate::handlers::{
    MAX_DECISION_JSON_BYTES, decision_success_response, run_blocking_after_acquire,
    valid_operation_id,
};
use crate::security::SecurityPolicy;
use crate::staging::{MAX_CREATOR_FILE_BYTES, StagingDirectory};
use crate::state::{
    AppState, BlockingGates, MAX_ACTIVE_OPERATIONS, MAX_BLOCKING_OPERATIONS,
    MAX_BLOCKING_OPERATIONS_PER_PROJECT, MAX_OPERATION_ENTRIES, OperationRegistry,
    OperationRegistryError,
};
use crate::templates::APP_JS;

static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);

struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new() -> Self {
        let sequence = NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "synapse-local-http-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn test_app() -> (TestDirectory, Router) {
    let directory = TestDirectory::new();
    let repository = directory.0.join("repository");
    fs::create_dir(&repository).unwrap();
    let service = Arc::new(
        LocalService::new([synapse_local_service::ProjectRegistration::new(
            "demo",
            "Demo project",
            repository,
        )])
        .unwrap(),
    );
    let application =
        build_with_identity(service, 43123, "a".repeat(64), "local-test-instance".into());
    (directory, application.into_router())
}

struct CreatorFixture {
    original_oid: String,
    current_oid: String,
}

struct IncompleteFixture {
    proposal_ref: String,
    proposal_head: String,
    decision_ref: String,
    decision_head: String,
}

fn test_app_with_creator(label: &str) -> (TestDirectory, Router, CreatorFixture) {
    let directory = TestDirectory::new();
    let repository = directory.0.join("repository");
    fs::create_dir(&repository).unwrap();
    let original = directory.0.join("original.png");
    let current = directory.0.join("current.svg");
    let ai_output = directory.0.join("ai-output.gif");
    fs::write(&original, b"\x89PNG\r\n\x1a\nhttp-original").unwrap();
    fs::write(
        &current,
        b"<svg xmlns='http://www.w3.org/2000/svg'><rect/></svg>",
    )
    .unwrap();
    fs::write(&ai_output, b"GIF89ahttp-ai-output").unwrap();
    let receipt = run_creator_session(&CreatorRunOptions {
        repository: repository.clone(),
        session: "render-session".into(),
        original_image: original,
        current_image: current,
        ai_output,
        subject_label: "HTTP fixture".into(),
        creator_name: "Test creator".into(),
        disposition: CreatorDisposition::Adopt,
        rationale: Some("Exercise the local HTTP read endpoints.".into()),
    })
    .unwrap();
    let fixture = CreatorFixture {
        original_oid: receipt.original_blob_oid,
        current_oid: receipt.current_blob_oid,
    };
    let service = Arc::new(
        LocalService::new([synapse_local_service::ProjectRegistration::new(
            "demo", label, repository,
        )])
        .unwrap(),
    );
    let application =
        build_with_identity(service, 43123, "a".repeat(64), "local-test-instance".into());
    (directory, application.into_router(), fixture)
}

fn test_app_with_incomplete() -> (TestDirectory, Router, IncompleteFixture) {
    let directory = TestDirectory::new();
    let repository = directory.0.join("repository");
    fs::create_dir(&repository).unwrap();
    let original = directory.0.join("incomplete-original.png");
    let current = directory.0.join("incomplete-current.png");
    let ai_output = directory.0.join("incomplete-ai-output.png");
    fs::write(&original, b"\x89PNG\r\n\x1a\nincomplete-original").unwrap();
    fs::write(&current, b"\x89PNG\r\n\x1a\nincomplete-current").unwrap();
    fs::write(&ai_output, b"\x89PNG\r\n\x1a\nincomplete-ai-output").unwrap();
    let pending = begin_creator_session(&CreatorBeginOptions {
        repository: repository.clone(),
        session: "incomplete-session".into(),
        original_image: original,
        current_image: current,
        ai_output,
        subject_label: "Incomplete HTTP fixture".into(),
        creator_name: "Test creator".into(),
    })
    .unwrap();
    let receipt = pending.receipt().clone();
    let fixture = IncompleteFixture {
        proposal_ref: receipt.proposal_ref,
        proposal_head: receipt.proposal_head,
        decision_ref: receipt.decision_ref,
        decision_head: receipt.base_head,
    };
    drop(pending);

    let service = Arc::new(
        LocalService::new([synapse_local_service::ProjectRegistration::new(
            "demo",
            "Incomplete project",
            repository,
        )])
        .unwrap(),
    );
    let application =
        build_with_identity(service, 43123, "a".repeat(64), "local-test-instance".into());
    (directory, application.into_router(), fixture)
}

fn request(path: &str) -> axum::http::request::Builder {
    Request::builder().uri(path).header(HOST, "127.0.0.1:43123")
}

fn multipart_body(boundary: &str, parts: &[(&str, &str, &[u8])]) -> Vec<u8> {
    let mut body = Vec::new();
    for (name, content_type, bytes) in parts {
        body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
        body.extend_from_slice(
            format!("Content-Disposition: form-data; name=\"{name}\"; filename=\"{name}.bin\"\r\n")
                .as_bytes(),
        );
        body.extend_from_slice(format!("Content-Type: {content_type}\r\n\r\n").as_bytes());
        body.extend_from_slice(bytes);
        body.extend_from_slice(b"\r\n");
    }
    body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    body
}

fn valid_creator_multipart(boundary: &str) -> Vec<u8> {
    multipart_body(
        boundary,
        &[
            ("session", "text/plain; charset=utf-8", b"web-review"),
            (
                "subject_label",
                "text/plain; charset=utf-8",
                b"Web transport fixture",
            ),
            ("creator_name", "text/plain; charset=utf-8", b"HTTP creator"),
            (
                "original_image",
                "application/octet-stream",
                b"\x89PNG\r\n\x1a\nweb-original",
            ),
            (
                "current_image",
                "application/octet-stream",
                b"<svg xmlns='http://www.w3.org/2000/svg'><rect/></svg>",
            ),
            (
                "ai_output",
                "application/octet-stream",
                b"GIF89aweb-ai-output",
            ),
        ],
    )
}

fn unsafe_api_request(
    path: &str,
    content_type: impl AsRef<str>,
    body: Body,
) -> axum::http::Request<Body> {
    request(path)
        .method("POST")
        .header("x-synapse-local-token", "a".repeat(64))
        .header(ORIGIN, "http://127.0.0.1:43123")
        .header("sec-fetch-site", "same-origin")
        .header(CONTENT_TYPE, content_type.as_ref())
        .body(body)
        .unwrap()
}

async fn assert_problem(response: Response, status: StatusCode, code: &str) {
    assert_eq!(response.status(), status);
    assert_eq!(
        response.headers().get(CONTENT_TYPE).unwrap(),
        "application/problem+json"
    );
    let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
    let problem: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(problem["status"], status.as_u16());
    assert_eq!(problem["code"], code);
}

#[test]
fn browser_write_enhancement_preserves_submitter_before_disabling_controls() {
    let prepare = APP_JS
        .find("prepared = formRequest(form, event.submitter);")
        .expect("the request captures the clicked submitter");
    let disable = APP_JS
        .find("setBusy(form, true);")
        .expect("the form disables controls while busy");
    assert!(prepare < disable);
    assert!(APP_JS.contains("data.append(submitter.name, submitter.value)"));
    assert!(APP_JS.contains("event.submitter?.name === \"disposition\""));
    assert!(APP_JS.contains("window.confirm("));
    assert!(APP_JS.contains("form.hidden = false"));
    assert!(APP_JS.contains("data?.state === \"committed\""));
    assert!(APP_JS.contains("showCommittedReceipt(form, data)"));
    assert!(APP_JS.contains("JSON.stringify(receipt, null, 2)"));
    assert!(APP_JS.contains("!committedWithoutReport && form.dataset.successReload"));
    assert!(APP_JS.contains("async function pollOperation"));
    assert!(
        APP_JS
            .contains("operation.state === \"failed\" || operation.state === \"outcome_unknown\"")
    );
    assert!(APP_JS.contains("data?.state === \"queued\""));
    assert!(APP_JS.contains("form.dataset.confirmMaintenance"));
}

#[test]
fn operation_timestamps_are_rfc3339_and_never_regress() {
    assert_eq!(civil_date_from_unix_days(0), (1970, 1, 1));
    assert_eq!(civil_date_from_unix_days(19_205), (2022, 8, 1));

    let later = UNIX_EPOCH + Duration::new(19_205 * 86_400 + 3_661, 123_456_789);
    let earlier = later - Duration::from_secs(600);
    let before_epoch = UNIX_EPOCH - Duration::from_secs(1);
    let advanced = later + Duration::from_secs(5);
    let mut last_timestamp = None;

    let first = monotonic_operation_timestamp(&mut last_timestamp, later).unwrap();
    assert_eq!(first.len(), 30);
    assert_eq!(&first[4..5], "-");
    assert_eq!(&first[10..11], "T");
    assert!(first.ends_with('Z'));
    assert_eq!(
        monotonic_operation_timestamp(&mut last_timestamp, earlier).unwrap(),
        first
    );
    assert_eq!(
        monotonic_operation_timestamp(&mut last_timestamp, before_epoch).unwrap(),
        first
    );
    assert!(monotonic_operation_timestamp(&mut last_timestamp, advanced).unwrap() > first);
}

#[test]
fn operation_registry_bounds_active_jobs() {
    let registry = OperationRegistry::default();
    let mut first_id = None;
    for index in 0..MAX_ACTIVE_OPERATIONS {
        let accepted = registry
            .reserve(OperationKind::Fsck, format!("project-{index}"))
            .unwrap();
        first_id.get_or_insert(accepted.operation_id);
    }
    assert!(matches!(
        registry.reserve(OperationKind::Fsck, "overflow".into()),
        Err(OperationRegistryError::Capacity)
    ));
    let first_id = first_id.unwrap();
    registry.mark_running(&first_id);
    registry.finish(
        &first_id,
        OperationState::Succeeded,
        Some(OperationResult::Fsck(synapse_local_service::FsckResult {
            clean: true,
            objects_seen: 0,
            objects_verified: 0,
            closure_count: 0,
            issue_count: 0,
        })),
        None,
    );
    assert!(
        registry
            .reserve(OperationKind::Fsck, "replacement".into())
            .is_ok()
    );
}

#[test]
fn operation_registry_evicts_by_admission_order_during_clock_regression() {
    let registry = OperationRegistry::default();
    let observed_at = UNIX_EPOCH + Duration::from_secs(2_000_000_000);
    let mut oldest_id = None;
    let mut second_id = None;
    for index in 0..MAX_OPERATION_ENTRIES {
        let regressed_at = observed_at - Duration::from_secs(index as u64);
        let accepted = registry
            .reserve_at(
                OperationKind::Fsck,
                format!("project-{index}"),
                regressed_at,
            )
            .unwrap();
        if index == 0 {
            oldest_id = Some(accepted.operation_id.clone());
        } else if index == 1 {
            second_id = Some(accepted.operation_id.clone());
        }
        registry.finish_at(
            &accepted.operation_id,
            OperationState::Succeeded,
            None,
            None,
            regressed_at,
        );
    }

    let oldest_id = oldest_id.unwrap();
    let second_id = second_id.unwrap();
    assert!(registry.get(&oldest_id).is_some());
    registry
        .reserve_at(OperationKind::Fsck, "replacement".into(), observed_at)
        .unwrap();
    assert!(registry.get(&oldest_id).is_none());
    assert!(registry.get(&second_id).is_some());
}

#[tokio::test]
async fn operation_remains_queued_until_all_blocking_gates_are_acquired() {
    let directory = TestDirectory::new();
    let repository = directory.0.join("repository");
    fs::create_dir(&repository).unwrap();
    let service = Arc::new(
        LocalService::new([synapse_local_service::ProjectRegistration::new(
            "demo",
            "Demo project",
            repository,
        )])
        .unwrap(),
    );
    let blocking = BlockingGates::new(["demo".to_owned()]);
    let operations = OperationRegistry::default();
    let state = AppState {
        service,
        security: SecurityPolicy::new(43123, "a".repeat(64), "local-test-instance".into()),
        blocking: blocking.clone(),
        uploads: Arc::new(Semaphore::new(MAX_CONCURRENT_CREATOR_UPLOADS)),
        operations: operations.clone(),
    };

    let mut project_permits = Vec::new();
    let project_gate = blocking.projects.get("demo").unwrap().clone();
    for _ in 0..MAX_BLOCKING_OPERATIONS_PER_PROJECT {
        project_permits.push(project_gate.clone().acquire_owned().await.unwrap());
    }
    let mut overall_permits = Vec::new();
    for _ in 0..MAX_BLOCKING_OPERATIONS {
        overall_permits.push(blocking.overall.clone().acquire_owned().await.unwrap());
    }

    let accepted = operations
        .reserve(OperationKind::Fsck, "demo".into())
        .unwrap();
    let operation_id = accepted.operation_id;
    let operations_for_start = operations.clone();
    let start_id = operation_id.clone();
    let operations_for_work = operations.clone();
    let work_id = operation_id.clone();
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let worker = tokio::spawn(run_blocking_after_acquire(
        state,
        Some("demo".into()),
        move || {
            operations_for_start.mark_running(&start_id);
            started_tx.send(()).unwrap();
        },
        move |_| {
            Ok(operations_for_work
                .get(&work_id)
                .is_some_and(|status| status.state == OperationState::Running))
        },
    ));

    tokio::task::yield_now().await;
    assert_eq!(
        operations.get(&operation_id).unwrap().state,
        OperationState::Queued
    );
    project_permits.pop();
    tokio::task::yield_now().await;
    assert_eq!(
        operations.get(&operation_id).unwrap().state,
        OperationState::Queued
    );

    overall_permits.pop();
    tokio::time::timeout(Duration::from_secs(5), started_rx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        operations.get(&operation_id).unwrap().state,
        OperationState::Running
    );
    assert!(worker.await.unwrap().unwrap());
}

#[tokio::test]
async fn committed_decision_receipt_is_an_http_200_success_body() {
    let commit = format!("commit:sg-oid-v1:sha256:{}", "a".repeat(64));
    let record = format!("record:sg-oid-v1:sha256:{}", "b".repeat(64));
    let blob = format!("blob:sg-oid-v1:sha256:{}", "c".repeat(64));
    let outcome = CreatorDecisionResponse::Committed(Box::new(CommittedCreatorSession {
        state: CommittedState::Committed,
        receipt: CreatorDecisionReceipt {
            session: "receipt-session".into(),
            project_id: "project-id".into(),
            subject_id: "subject-id".into(),
            creator_id: "creator-id".into(),
            agent_id: "agent-id".into(),
            decision_ref: "decision/creator/receipt-session".into(),
            proposal_ref: "proposal/creator-agent/receipt-session".into(),
            base_head: commit.clone(),
            proposal_head: commit.clone(),
            decision_head: commit.clone(),
            original_blob_oid: blob.clone(),
            current_blob_oid: blob.clone(),
            ai_output_blob_oid: blob.clone(),
            capture_profile_oid: record.clone(),
            original_observation_oid: record.clone(),
            current_observation_oid: record.clone(),
            comparison_tool_id: "comparison-tool".into(),
            comparison_tool_actor_oid: record.clone(),
            comparison_analysis_oid: record.clone(),
            comparison_implementation_oid: blob.clone(),
            comparison_configuration_oid: blob,
            byte_identity_outcome: "different".into(),
            comparison_status: "succeeded".into(),
            comparison_comparability: "partial".into(),
            comparison_reason_codes: Vec::new(),
            ai_activity_oid: record.clone(),
            decision_feedback_oid: record,
            disposition: CreatorDecision::Adopt,
        },
        report_available: false,
        inspection_required: true,
    }));

    let response = decision_success_response(outcome);
    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
    let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(value["state"], "committed");
    assert_eq!(value["receipt"]["decision_head"], commit);
    assert_eq!(value["report_available"], false);
    assert_eq!(value["inspection_required"], true);
}

#[test]
fn attachment_only_media_uses_download_only_blob_urls() {
    let start = APP_JS
        .find("function installAttachmentDownload")
        .expect("the attachment installer exists");
    let end = APP_JS[start..]
        .find("async function loadApiImage")
        .map(|offset| start + offset)
        .expect("the attachment installer has a bounded source section");
    let installer = &APP_JS[start..end];
    assert!(installer.contains("download.hasAttribute(\"download\")"));
    assert!(installer.contains("download.setAttribute(\"href\", objectUrl)"));
    assert!(installer.contains("window.setTimeout("));
    assert!(!installer.contains("image.src = objectUrl"));
    assert!(APP_JS.contains("type === ATTACHMENT_MEDIA_TYPE"));
    assert!(APP_JS.contains("window.addEventListener(\"pagehide\", releaseImageResources)"));
}

#[test]
fn staging_directory_creation_immediately_establishes_raii_ownership() {
    // This intentionally compiles as a synchronous call: there is no await
    // at which cancellation can detach a successful directory creation
    // from the cleanup owner.
    let directory = StagingDirectory::create_sync().unwrap();
    let path = directory.path.clone();
    assert!(path.is_dir());
    #[cfg(unix)]
    assert_eq!(
        <std::fs::Permissions as std::os::unix::fs::PermissionsExt>::mode(
            &fs::metadata(&path).unwrap().permissions(),
        ) & 0o777,
        0o700
    );
    drop(directory);
    assert!(!path.exists());
}

#[tokio::test]
async fn cancelled_staging_create_drops_the_detached_task_output() {
    let parent = TestDirectory::new();
    let path = parent.0.join("detached-staging");
    let operation_path = path.clone();
    let (created_tx, created_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let waiter = tokio::spawn(StagingDirectory::create_with(move || {
        std::fs::create_dir(&operation_path)?;
        let directory = StagingDirectory {
            path: operation_path,
        };
        created_tx.send(()).unwrap();
        release_rx.recv().unwrap();
        Ok(directory)
    }));

    created_rx.await.unwrap();
    assert!(path.is_dir());
    waiter.abort();
    assert!(matches!(waiter.await, Err(error) if error.is_cancelled()));
    release_tx.send(()).unwrap();

    let path_for_poll = path.clone();
    let removed = tokio::task::spawn_blocking(move || {
        for _ in 0..5_000 {
            if !path_for_poll.exists() {
                return true;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        !path_for_poll.exists()
    })
    .await
    .unwrap();
    assert!(removed, "detached staging owner did not clean up {path:?}");
}

#[tokio::test]
async fn health_is_public_but_host_and_proxy_headers_fail_closed() {
    let (_directory, app) = test_app();
    let missing_host = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(missing_host.status(), StatusCode::FORBIDDEN);

    let health = app
        .clone()
        .oneshot(request("/api/v1/health").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(health.status(), StatusCode::OK);
    assert_eq!(health.headers().get("access-control-allow-origin"), None);
    assert_eq!(
        health.headers().get("x-content-type-options").unwrap(),
        "nosniff"
    );

    for proxy_header in ["x-forwarded-host", "x-forwarded-prefix"] {
        let forwarded = app
            .clone()
            .oneshot(
                request("/api/v1/health")
                    .header(proxy_header, "attacker-controlled")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(forwarded.status(), StatusCode::FORBIDDEN, "{proxy_header}");
    }
}

#[tokio::test]
async fn api_requires_the_header_token_and_never_accepts_a_query_token() {
    let (_directory, app) = test_app();
    let missing = app
        .clone()
        .oneshot(request("/api/v1/projects").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(missing.status(), StatusCode::FORBIDDEN);

    let query_only = app
        .clone()
        .oneshot(
            request("/api/v1/projects?token=aaaaaaaa")
                .header("x-synapse-local-token", "a".repeat(64))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(query_only.status(), StatusCode::FORBIDDEN);

    let get_body = app
        .clone()
        .oneshot(
            request("/api/v1/projects")
                .header("x-synapse-local-token", "a".repeat(64))
                .header("content-length", "1")
                .body(Body::from("x"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(get_body.status(), StatusCode::FORBIDDEN);

    let allowed = app
        .oneshot(
            request("/api/v1/projects")
                .header("x-synapse-local-token", "a".repeat(64))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(allowed.status(), StatusCode::OK);
}

#[tokio::test]
async fn incomplete_session_diagnostics_are_read_only_structured_and_rendered() {
    let (directory, app, fixture) = test_app_with_incomplete();
    let diagnostics = app
        .clone()
        .oneshot(
            request("/api/v1/projects/demo/creator-sessions/incomplete-session/diagnostics")
                .header("x-synapse-local-token", "a".repeat(64))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(diagnostics.status(), StatusCode::OK);
    assert_eq!(
        diagnostics.headers().get(CONTENT_TYPE).unwrap(),
        "application/json"
    );
    let diagnostics = to_bytes(diagnostics.into_body(), 64 * 1024).await.unwrap();
    let diagnostic: serde_json::Value = serde_json::from_slice(&diagnostics).unwrap();
    assert_eq!(diagnostic["state"], "incomplete");
    assert_eq!(diagnostic["session"], "incomplete-session");
    assert_eq!(diagnostic["proposal_ref"], fixture.proposal_ref);
    assert_eq!(diagnostic["proposal_head"], fixture.proposal_head);
    assert_eq!(diagnostic["decision_ref"], fixture.decision_ref);
    assert_eq!(diagnostic["decision_head"], fixture.decision_head);
    assert_eq!(diagnostic["automatic_resume_supported"], false);
    assert_eq!(diagnostic["automatic_cleanup_supported"], false);
    assert!(
        diagnostic["recommended_action"]
            .as_str()
            .unwrap()
            .contains("Run fsck")
    );
    assert!(
        !std::str::from_utf8(&diagnostics)
            .unwrap()
            .contains(directory.0.to_str().unwrap())
    );

    let page = app
        .clone()
        .oneshot(
            request("/projects/demo/creator-sessions/incomplete-session")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(page.status(), StatusCode::OK);
    let page = to_bytes(page.into_body(), 2 * 1024 * 1024).await.unwrap();
    let page = std::str::from_utf8(&page).unwrap();
    assert!(page.contains("Creator session diagnostics"));
    assert!(page.contains(&fixture.proposal_ref));
    assert!(page.contains(&fixture.proposal_head));
    assert!(page.contains(&fixture.decision_ref));
    assert!(page.contains(&fixture.decision_head));
    assert!(page.contains("Automatic resume"));
    assert!(page.contains("Automatic cleanup"));
    assert!(!page.contains(directory.0.to_str().unwrap()));

    for session in ["missing-session", "Invalid-Session"] {
        let response = app
            .clone()
            .oneshot(
                request(&format!(
                    "/api/v1/projects/demo/creator-sessions/{session}/diagnostics"
                ))
                .header("x-synapse-local-token", "a".repeat(64))
                .body(Body::empty())
                .unwrap(),
            )
            .await
            .unwrap();
        assert_problem(response, StatusCode::NOT_FOUND, "creator_session_not_found").await;
    }
}

#[tokio::test]
async fn bounded_fsck_is_confirmed_queued_polled_and_reflected_in_project_status() {
    let (_directory, app) = test_app();
    let start = app
        .clone()
        .oneshot(unsafe_api_request(
            "/api/v1/projects/demo/operations/fsck",
            "application/json",
            Body::from(r#"{"confirm_project_key":"demo"}"#),
        ))
        .await
        .unwrap();
    assert_eq!(start.status(), StatusCode::ACCEPTED);
    let start = to_bytes(start.into_body(), 64 * 1024).await.unwrap();
    let accepted: serde_json::Value = serde_json::from_slice(&start).unwrap();
    assert_eq!(accepted["state"], "queued");
    let operation_id = accepted["operation_id"].as_str().unwrap();
    assert!(valid_operation_id(operation_id));
    let poll_path = accepted["poll_path"].as_str().unwrap();
    assert_eq!(poll_path, format!("/api/v1/operations/{operation_id}"));

    let mut terminal = None;
    for _ in 0..2_000 {
        let response = app
            .clone()
            .oneshot(
                request(poll_path)
                    .header("x-synapse-local-token", "a".repeat(64))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
        let status: serde_json::Value = serde_json::from_slice(&body).unwrap();
        if matches!(
            status["state"].as_str(),
            Some("succeeded" | "failed" | "outcome_unknown")
        ) {
            terminal = Some(status);
            break;
        }
        tokio::task::yield_now().await;
    }
    let terminal = terminal.expect("the bounded empty-repository fsck did not finish");
    assert_eq!(terminal["operation_id"], operation_id);
    assert_eq!(terminal["kind"], "fsck");
    assert_eq!(terminal["project_key"], "demo");
    assert_eq!(terminal["state"], "succeeded");
    assert!(terminal["submitted_at"].as_str().unwrap().ends_with('Z'));
    assert!(terminal["completed_at"].as_str().unwrap().ends_with('Z'));
    assert_eq!(terminal["result"]["clean"], true);
    assert_eq!(terminal["result"]["objects_seen"], 0);
    assert_eq!(terminal["result"]["objects_verified"], 0);
    assert_eq!(terminal["result"]["issue_count"], 0);
    assert_eq!(terminal["error"], serde_json::Value::Null);

    let status = app
        .clone()
        .oneshot(
            request("/api/v1/projects/demo/status")
                .header("x-synapse-local-token", "a".repeat(64))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = to_bytes(status.into_body(), 64 * 1024).await.unwrap();
    let status: serde_json::Value = serde_json::from_slice(&status).unwrap();
    assert_eq!(status["project"]["capabilities"]["fsck"], true);
    assert_eq!(status["last_fsck"]["clean"], true);

    let page = app
        .clone()
        .oneshot(request("/projects/demo").body(Body::empty()).unwrap())
        .await
        .unwrap();
    let page = to_bytes(page.into_body(), 2 * 1024 * 1024).await.unwrap();
    let page = std::str::from_utf8(&page).unwrap();
    assert!(page.contains("Repository integrity check"));
    assert!(page.contains("name=\"confirm_project_key\""));
    assert!(page.contains("直近のprocess-local結果: clean"));

    for body in [
        r#"{"confirm_project_key":"other"}"#,
        r#"{"confirm_project_key":"demo","unknown":true}"#,
    ] {
        let rejected = app
            .clone()
            .oneshot(unsafe_api_request(
                "/api/v1/projects/demo/operations/fsck",
                "application/json",
                Body::from(body),
            ))
            .await
            .unwrap();
        assert_problem(rejected, StatusCode::BAD_REQUEST, "local_request_denied").await;
    }

    let lost = app
        .oneshot(
            request(&format!("/api/v1/operations/{}", "f".repeat(64)))
                .header("x-synapse-local-token", "a".repeat(64))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_problem(lost, StatusCode::NOT_FOUND, "operation_state_lost").await;
}

#[tokio::test]
async fn unsafe_routes_require_browser_security_and_known_writes_reject_invalid_bodies() {
    let (_directory, app) = test_app();
    let no_origin = app
        .clone()
        .oneshot(
            request("/api/v1/projects/demo/creator-sessions")
                .method("POST")
                .header("x-synapse-local-token", "a".repeat(64))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(no_origin.status(), StatusCode::FORBIDDEN);

    let protected_write = app
        .clone()
        .oneshot(
            request("/api/v1/projects/demo/creator-sessions")
                .method("POST")
                .header("x-synapse-local-token", "a".repeat(64))
                .header(ORIGIN, "http://127.0.0.1:43123")
                .header("sec-fetch-site", "same-origin")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(protected_write.status(), StatusCode::BAD_REQUEST);

    for forbidden in [
        "/api/v1/objects",
        "/api/v1/update-ref",
        "/api/v1/authority",
        "/api/v1/projects/demo/commits",
    ] {
        let response = app
            .clone()
            .oneshot(
                request(forbidden)
                    .header("x-synapse-local-token", "a".repeat(64))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{forbidden}");
    }
}

#[tokio::test]
async fn creator_multipart_and_decision_complete_the_two_step_transport_workflow() {
    let (_directory, app) = test_app();
    let boundary = "synapse-success-boundary";
    let upload = app
        .clone()
        .oneshot(unsafe_api_request(
            "/api/v1/projects/demo/creator-sessions",
            format!("multipart/form-data; boundary={boundary}"),
            Body::from(valid_creator_multipart(boundary)),
        ))
        .await
        .unwrap();
    assert_eq!(upload.status(), StatusCode::CREATED);
    assert_eq!(
        upload.headers().get(CONTENT_TYPE).unwrap(),
        "application/json"
    );
    let upload_body = to_bytes(upload.into_body(), 2 * 1024 * 1024).await.unwrap();
    let pending: serde_json::Value = serde_json::from_slice(&upload_body).unwrap();
    assert_eq!(pending["state"], "pending_review");
    assert_eq!(pending["session"], "web-review");
    assert_eq!(pending["server_instance"], "local-test-instance");
    assert_eq!(pending["ai_output_source"], "caller_supplied");
    let review_id = pending["review_id"].as_str().unwrap().to_owned();

    let pending_page = app
        .clone()
        .oneshot(
            request("/projects/demo/creator-sessions/web-review")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(pending_page.status(), StatusCode::OK);
    let pending_html = to_bytes(pending_page.into_body(), 2 * 1024 * 1024)
        .await
        .unwrap();
    let pending_html = std::str::from_utf8(&pending_html).unwrap();
    assert_eq!(pending_html.matches("data-synapse-image ").count(), 3);
    assert_eq!(
        pending_html.matches("data-synapse-image-download").count(),
        3
    );
    assert!(pending_html.contains("download=\"web-review-current.bin\" hidden"));
    assert!(pending_html.contains("caller_supplied"));
    assert!(pending_html.contains("name=\"disposition\" value=\"adopt\""));
    assert!(pending_html.contains("name=\"disposition\" value=\"reject\""));
    assert!(pending_html.contains("name=\"disposition\" value=\"defer\""));

    for (role, expected_type, expected_disposition) in [
        ("original", "image/png", "inline"),
        ("current", "application/octet-stream", "attachment"),
        ("ai-output", "image/gif", "inline"),
    ] {
        let image = app
            .clone()
            .oneshot(
                request(&format!(
                    "/api/v1/projects/demo/creator-sessions/web-review/images/{role}"
                ))
                .header("x-synapse-local-token", "a".repeat(64))
                .body(Body::empty())
                .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(image.status(), StatusCode::OK, "{role}");
        assert_eq!(image.headers().get(CONTENT_TYPE).unwrap(), expected_type);
        assert!(
            image
                .headers()
                .get(CONTENT_DISPOSITION)
                .unwrap()
                .to_str()
                .unwrap()
                .starts_with(expected_disposition),
            "{role}"
        );
        assert!(!to_bytes(image.into_body(), 1024).await.unwrap().is_empty());
    }

    let pending_index = app
        .clone()
        .oneshot(request("/").body(Body::empty()).unwrap())
        .await
        .unwrap();
    let pending_index = to_bytes(pending_index.into_body(), 2 * 1024 * 1024)
        .await
        .unwrap();
    let pending_index = std::str::from_utf8(&pending_index).unwrap();
    assert!(pending_index.contains("<dt>レビュー待ち</dt><dd>1</dd>"));

    let decision_body = serde_json::to_vec(&serde_json::json!({
        "review_id": review_id,
        "disposition": "adopt",
        "rationale": "Reviewed through the localhost transport."
    }))
    .unwrap();
    let decision = app
        .clone()
        .oneshot(unsafe_api_request(
            "/api/v1/projects/demo/creator-sessions/web-review/decisions",
            "application/json",
            Body::from(decision_body),
        ))
        .await
        .unwrap();
    assert_eq!(decision.status(), StatusCode::OK);
    let decision_body = to_bytes(decision.into_body(), 2 * 1024 * 1024)
        .await
        .unwrap();
    let complete: serde_json::Value = serde_json::from_slice(&decision_body).unwrap();
    assert_eq!(complete["state"], "complete");
    assert_eq!(complete["report"]["session"], "web-review");
    assert_eq!(complete["report"]["disposition"], "adopt");

    let completed_page = app
        .oneshot(
            request("/projects/demo/creator-sessions/web-review")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(completed_page.status(), StatusCode::OK);
    let completed_html = to_bytes(completed_page.into_body(), 2 * 1024 * 1024)
        .await
        .unwrap();
    let completed_html = std::str::from_utf8(&completed_html).unwrap();
    assert!(completed_html.contains("Disposition"));
    assert!(!completed_html.contains("Human reviewが必要です"));
}

#[tokio::test]
async fn multipart_rejects_duplicate_extra_missing_and_wrong_content_type_fields() {
    let (_directory, app) = test_app();
    let boundary = "synapse-invalid-boundary";
    let content_type = format!("multipart/form-data; boundary={boundary}");
    let cases = [
        multipart_body(
            boundary,
            &[
                ("session", "text/plain; charset=utf-8", b"one"),
                ("session", "text/plain; charset=utf-8", b"two"),
            ],
        ),
        multipart_body(
            boundary,
            &[("unexpected", "text/plain; charset=utf-8", b"value")],
        ),
        multipart_body(
            boundary,
            &[
                ("session", "text/plain; charset=utf-8", b"missing-file"),
                (
                    "subject_label",
                    "text/plain; charset=utf-8",
                    b"Missing fields",
                ),
                ("creator_name", "text/plain; charset=utf-8", b"HTTP creator"),
            ],
        ),
        multipart_body(
            boundary,
            &[("session", "application/octet-stream", b"wrong-type")],
        ),
    ];

    for body in cases {
        let response = app
            .clone()
            .oneshot(unsafe_api_request(
                "/api/v1/projects/demo/creator-sessions",
                &content_type,
                Body::from(body),
            ))
            .await
            .unwrap();
        assert_problem(response, StatusCode::BAD_REQUEST, "local_request_denied").await;
    }

    let extra_content_type = app
        .oneshot(unsafe_api_request(
            "/api/v1/projects/demo/creator-sessions",
            format!("multipart/form-data; boundary={boundary}; charset=utf-8"),
            Body::from(multipart_body(boundary, &[])),
        ))
        .await
        .unwrap();
    assert_problem(
        extra_content_type,
        StatusCode::BAD_REQUEST,
        "local_request_denied",
    )
    .await;
}

#[tokio::test]
async fn multipart_rejects_a_file_larger_than_sixty_four_mib() {
    let (_directory, app) = test_app();
    let boundary = "synapse-oversize-boundary";
    let oversized = vec![b'x'; MAX_CREATOR_FILE_BYTES + 1];
    let body = multipart_body(
        boundary,
        &[("original_image", "application/octet-stream", &oversized)],
    );
    drop(oversized);
    let response = app
        .oneshot(unsafe_api_request(
            "/api/v1/projects/demo/creator-sessions",
            format!("multipart/form-data; boundary={boundary}"),
            Body::from(body),
        ))
        .await
        .unwrap();
    assert_problem(response, StatusCode::PAYLOAD_TOO_LARGE, "resource_limit").await;
}

#[tokio::test]
async fn foreign_origin_is_rejected_before_a_creator_write_is_parsed() {
    let (_directory, app) = test_app();
    let response = app
        .oneshot(
            request("/api/v1/projects/demo/creator-sessions")
                .method("POST")
                .header("x-synapse-local-token", "a".repeat(64))
                .header(ORIGIN, "http://localhost:43123")
                .header("sec-fetch-site", "cross-site")
                .header(
                    CONTENT_TYPE,
                    "multipart/form-data; boundary=foreign-origin-boundary",
                )
                .body(Body::from("not parsed"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_problem(response, StatusCode::FORBIDDEN, "local_request_denied").await;
}

#[tokio::test]
async fn decision_rejects_non_exact_content_type_unknown_fields_and_oversize_json() {
    let (_directory, app) = test_app();
    for (content_type, body, expected_status, expected_code) in [
        (
            "application/json; charset=utf-8",
            Body::from(r#"{"review_id":"abcdefghijklmnopqrstuv","disposition":"defer"}"#),
            StatusCode::BAD_REQUEST,
            "local_request_denied",
        ),
        (
            "application/json",
            Body::from(
                r#"{"review_id":"abcdefghijklmnopqrstuv","disposition":"defer","unknown":true}"#,
            ),
            StatusCode::BAD_REQUEST,
            "local_request_denied",
        ),
        (
            "application/json",
            Body::from(vec![b' '; MAX_DECISION_JSON_BYTES + 1]),
            StatusCode::PAYLOAD_TOO_LARGE,
            "resource_limit",
        ),
    ] {
        let response = app
            .clone()
            .oneshot(unsafe_api_request(
                "/api/v1/projects/demo/creator-sessions/not-pending/decisions",
                content_type,
                body,
            ))
            .await
            .unwrap();
        assert_problem(response, expected_status, expected_code).await;
    }
}

#[tokio::test]
async fn bootstrap_is_non_cacheable_and_contains_only_the_process_token() {
    let (_directory, app) = test_app();
    let response = app
        .oneshot(request("/").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers().get(header::CACHE_CONTROL).unwrap(),
        "no-store"
    );
    assert!(response.headers().get("content-security-policy").is_some());
    let body = to_bytes(response.into_body(), 2 * 1024 * 1024)
        .await
        .unwrap();
    let html = std::str::from_utf8(&body).unwrap();
    assert!(html.contains(&"a".repeat(64)));
    assert!(!html.contains("repository_path"));
}

#[tokio::test]
async fn index_project_and_session_pages_render_with_untrusted_labels_escaped() {
    let injected_label = "Demo <script data-injected>window.pwned=true</script> project";
    let (_directory, app, _fixture) = test_app_with_creator(injected_label);

    for (path, expected_text) in [
        ("/", "制作履歴を、手元で確かめる"),
        ("/projects/demo", "Creator sessions"),
        (
            "/projects/demo/creator-sessions/render-session",
            "Byte identity evidence",
        ),
    ] {
        let response = app
            .clone()
            .oneshot(request(path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK, "{path}");
        assert_eq!(
            response.headers().get(header::CONTENT_TYPE).unwrap(),
            "text/html; charset=utf-8"
        );
        let body = to_bytes(response.into_body(), 2 * 1024 * 1024)
            .await
            .unwrap();
        let html = std::str::from_utf8(&body).unwrap();
        assert!(html.contains(expected_text), "{path}");
        assert!(html.contains("window.pwned=true"), "{path}");
        assert!(!html.contains("<script data-injected>"), "{path}");
    }
}

#[tokio::test]
async fn reflog_query_accepts_declared_fields_and_rejects_unknown_or_duplicate_fields() {
    let (_directory, app) = test_app();

    for path in [
        "/api/v1/projects/demo/reflog",
        "/api/v1/projects/demo/reflog?limit=1",
        "/api/v1/projects/demo/reflog?ref_name=proposal%2Ffixture&after_event_id=0&limit=20",
    ] {
        let response = app
            .clone()
            .oneshot(
                request(path)
                    .header("x-synapse-local-token", "a".repeat(64))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK, "{path}");
    }

    for path in [
        "/api/v1/projects/demo/reflog?unknown=1",
        "/api/v1/projects/demo/reflog?limit=1&limit=2",
        "/api/v1/projects/demo/reflog?ref_name=refs%2Fone&ref_name=refs%2Ftwo",
        "/api/v1/projects/demo/reflog?after_event_id=01",
    ] {
        let response = app
            .clone()
            .oneshot(
                request(path)
                    .header("x-synapse-local-token", "a".repeat(64))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{path}");
        assert_eq!(
            response.headers().get(header::CONTENT_TYPE).unwrap(),
            "application/problem+json"
        );
    }
}

#[tokio::test]
async fn image_endpoint_sets_verified_media_headers_and_unknown_roles_are_not_found() {
    let (_directory, app, fixture) = test_app_with_creator("Demo project");
    let original = app
        .clone()
        .oneshot(
            request("/api/v1/projects/demo/creator-sessions/render-session/images/original")
                .header("x-synapse-local-token", "a".repeat(64))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(original.status(), StatusCode::OK);
    assert_eq!(
        original.headers().get(header::CONTENT_TYPE).unwrap(),
        "image/png"
    );
    assert_eq!(
        original.headers().get(header::CONTENT_DISPOSITION).unwrap(),
        "inline"
    );
    assert_eq!(
        original
            .headers()
            .get("x-synapse-blob-oid")
            .unwrap()
            .to_str()
            .unwrap(),
        fixture.original_oid.as_str()
    );
    let original_body = to_bytes(original.into_body(), 1024).await.unwrap();
    assert!(original_body.starts_with(b"\x89PNG\r\n\x1a\n"));

    let current = app
        .clone()
        .oneshot(
            request("/api/v1/projects/demo/creator-sessions/render-session/images/current")
                .header("x-synapse-local-token", "a".repeat(64))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(current.status(), StatusCode::OK);
    assert_eq!(
        current.headers().get(header::CONTENT_TYPE).unwrap(),
        "application/octet-stream"
    );
    assert_eq!(
        current.headers().get(header::CONTENT_DISPOSITION).unwrap(),
        "attachment; filename=\"render-session-current.bin\""
    );
    assert_eq!(
        current
            .headers()
            .get("x-synapse-blob-oid")
            .unwrap()
            .to_str()
            .unwrap(),
        fixture.current_oid.as_str()
    );

    let invalid_role = app
        .oneshot(
            request("/api/v1/projects/demo/creator-sessions/render-session/images/thumbnail")
                .header("x-synapse-local-token", "a".repeat(64))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(invalid_role.status(), StatusCode::NOT_FOUND);
    assert_eq!(
        invalid_role.headers().get(header::CONTENT_TYPE).unwrap(),
        "application/problem+json"
    );
}

#[tokio::test]
async fn blocking_gates_bound_known_projects_and_route_unknown_projects_through_global_limit() {
    let known = BlockingGates::new(["demo".to_owned()]);
    let first = known.acquire(Some("demo")).await.unwrap();
    let second = known.acquire(Some("demo")).await.unwrap();
    assert_eq!(known.projects["demo"].available_permits(), 0);
    assert!(matches!(
        known.projects["demo"].clone().try_acquire_owned(),
        Err(tokio::sync::TryAcquireError::NoPermits)
    ));
    drop((first, second));
    assert_eq!(
        known.projects["demo"].available_permits(),
        MAX_BLOCKING_OPERATIONS_PER_PROJECT
    );

    let unknown = BlockingGates::new(["demo".to_owned()]);
    let mut permits = Vec::new();
    for _ in 0..MAX_BLOCKING_OPERATIONS {
        permits.push(unknown.acquire(Some("unknown")).await.unwrap());
    }
    assert_eq!(unknown.overall.available_permits(), 0);
    assert!(matches!(
        unknown.overall.clone().try_acquire_owned(),
        Err(tokio::sync::TryAcquireError::NoPermits)
    ));
    drop(permits);
    assert_eq!(unknown.overall.available_permits(), MAX_BLOCKING_OPERATIONS);
}

// Openapi contract route-parity coverage.
//
// Two complementary mechanisms cover every operation declared in
// `api/local/v1/openapi.json`:
//
//   1. Implemented-route parity (`checked`, asserted below): for every
//      operation NOT in `UNIMPLEMENTED_ARCHIVE_OPERATIONS`, a substituted
//      request must return neither 404 nor 405 — proving the route
//      exists and the method is wired, regardless of what status the
//      business logic itself returns.
//   2. Unimplemented-route 404 contract (`UNIMPLEMENTED_ARCHIVE_OPERATIONS`,
//      asserted below): for exactly the 3 archive operations
//      `docs/localhost_application_architecture.md` documents as
//      "remain unimplemented in the browser application" (line 14-15;
//      see also line 467's "the remaining archive portion of slice 7"),
//      the same substituted request must currently return 404. This
//      makes the list self-maintaining: the day someone implements one
//      of these routes, this assertion fails (404 stops being true) and
//      the failure forces that operationId's removal from the skip list
//      into ordinary positive coverage above, rather than the operation
//      silently staying unchecked forever.
//
// `startFsck` and `getOperation` both carry openapi's
// `x-synapse-implementation-slice: 7` tag too, but that tag is a
// compound "slice 7" label spanning the implemented fsck/job foundation
// (`docs/localhost_application_architecture.md` line 3, 7-8: "the
// fsck/job part of slice 7 ... implemented in v0.3.0") and the
// unimplemented archive portion (same doc, line 14-15) without
// distinguishing them. Both routes are genuinely wired
// (`.route("/api/v1/projects/{project_key}/operations/fsck",
// post(api_start_fsck))` and `.route("/api/v1/operations/{operation_id}",
// get(api_operation))`, both above in this file's router construction),
// so this test checks them by `operationId`
// (`UNIMPLEMENTED_ARCHIVE_OPERATIONS`) rather than by the slice tag, and
// includes them in ordinary positive parity coverage below.
//
// Neither mechanism can check the inverse direction: a route registered
// on this axum `Router` but absent from the openapi document is not
// enumerable by reading the `Router` value at runtime (axum exposes no
// route-listing API), so a silent, undocumented router addition would
// not be caught here. That direction would require either a routing
// introspection facility this codebase does not have, or a
// hand-maintained inverse listing that itself could drift; the canary
// test below only proves 404 is distinguishable from a wired route, not
// that every wired route is documented.
const UNIMPLEMENTED_ARCHIVE_OPERATIONS: [&str; 3] =
    ["listArchives", "startArchiveExport", "startArchiveRestore"];

#[tokio::test]
async fn every_documented_openapi_route_matches_its_implementation_status() {
    let spec: serde_json::Value = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../api/local/v1/openapi.json"
    )))
    .expect("api/local/v1/openapi.json is valid JSON");
    let paths = spec["paths"]
        .as_object()
        .expect("openapi document has a paths object");

    // `test_app_with_creator` publishes and adopts `render-session` as a
    // complete session, which the GET routes below substitute in for
    // `{session}` (proven to resolve by the existing read-path tests
    // above). The decisions route instead needs a still-pending review,
    // so a second, undecided session (`web-review`, from
    // `valid_creator_multipart`) is created separately below.
    let (_directory, app, _fixture) = test_app_with_creator("Parity project");

    let boundary = "parity-boundary";
    let begin_response = app
        .clone()
        .oneshot(unsafe_api_request(
            "/api/v1/projects/demo/creator-sessions",
            format!("multipart/form-data; boundary={boundary}"),
            Body::from(valid_creator_multipart(boundary)),
        ))
        .await
        .unwrap();
    assert_eq!(
        begin_response.status(),
        StatusCode::CREATED,
        "fixture setup: beginning a second creator session must succeed"
    );
    let begin_body = to_bytes(begin_response.into_body(), 2 * 1024 * 1024)
        .await
        .unwrap();
    let pending: serde_json::Value = serde_json::from_slice(&begin_body).unwrap();
    let review_id = pending["review_id"].as_str().unwrap().to_owned();

    // A real, currently-registered operation_id for the getOperation
    // substitution below: startFsck reserves the operation
    // synchronously (before the async worker spawns), so it is already
    // visible to an immediate getOperation poll, matching the
    // established pattern in
    // `bounded_fsck_is_confirmed_queued_polled_and_reflected_in_project_status`.
    let fsck_start = app
        .clone()
        .oneshot(unsafe_api_request(
            "/api/v1/projects/demo/operations/fsck",
            "application/json",
            Body::from(r#"{"confirm_project_key":"demo"}"#),
        ))
        .await
        .unwrap();
    assert_eq!(
        fsck_start.status(),
        StatusCode::ACCEPTED,
        "fixture setup: starting fsck for the getOperation substitution must succeed"
    );
    let fsck_start_body = to_bytes(fsck_start.into_body(), 64 * 1024).await.unwrap();
    let fsck_accepted: serde_json::Value = serde_json::from_slice(&fsck_start_body).unwrap();
    let fsck_operation_id = fsck_accepted["operation_id"].as_str().unwrap().to_owned();

    let mut skipped_unimplemented_archive = Vec::new();
    let mut checked = Vec::new();

    for (path_template, methods) in paths {
        let methods = methods
            .as_object()
            .expect("each openapi path entry is an object of methods");
        for (method, operation) in methods {
            let operation_id = operation["operationId"]
                .as_str()
                .expect("every openapi operation declares operationId");

            // The decisions route resolves its target session from the
            // path AND the body's review_id together; every other
            // {session} substitution below only needs a session that
            // resolves at all, so the already-complete `render-session`
            // fixture (proven to resolve by the read-path tests above)
            // is used for those.
            let session_value = if path_template.ends_with("/decisions") {
                "web-review"
            } else {
                "render-session"
            };
            let resolved_path = path_template
                .replace("{projectKey}", "demo")
                .replace("{session}", session_value)
                .replace("{role}", "original")
                .replace("{operationId}", &fsck_operation_id);
            let full_path = format!("/api/v1{resolved_path}");

            if UNIMPLEMENTED_ARCHIVE_OPERATIONS.contains(&operation_id) {
                // Positive assertion, not a mere skip: this operation
                // must currently 404, or the skip list itself is out of
                // date (see the module comment above for why this
                // matters).
                let response = match method.as_str() {
                    "get" => app
                        .clone()
                        .oneshot(
                            request(&full_path)
                                .header("x-synapse-local-token", "a".repeat(64))
                                .body(Body::empty())
                                .unwrap(),
                        )
                        .await
                        .unwrap(),
                    "post" => {
                        let body = serde_json::to_vec(&serde_json::json!({
                            "confirm_project_key": "demo",
                            "archive_name": "nightly"
                        }))
                        .unwrap();
                        app.clone()
                            .oneshot(unsafe_api_request(
                                &full_path,
                                "application/json",
                                Body::from(body),
                            ))
                            .await
                            .unwrap()
                    }
                    other => panic!(
                        "unhandled unimplemented-archive-operation method {other} for \
                         {path_template}; add a substitution branch above"
                    ),
                };
                assert_eq!(
                    response.status(),
                    StatusCode::NOT_FOUND,
                    "{} {full_path} (openapi operationId {operation_id:?}) is listed in \
                     UNIMPLEMENTED_ARCHIVE_OPERATIONS but did not return 404: either it has \
                     been implemented (move it into positive parity coverage above and \
                     remove it from the skip list) or something else changed",
                    method.to_uppercase()
                );
                skipped_unimplemented_archive.push(format!(
                    "{} {path_template} ({operation_id})",
                    method.to_uppercase()
                ));
                continue;
            }

            let response = match method.as_str() {
                "get" => app
                    .clone()
                    .oneshot(
                        request(&full_path)
                            .header("x-synapse-local-token", "a".repeat(64))
                            .body(Body::empty())
                            .unwrap(),
                    )
                    .await
                    .unwrap(),
                "post" if resolved_path.ends_with("/creator-sessions") => {
                    // beginCreatorSession: a fresh session name (not
                    // `render-session` or `web-review`, both already
                    // used above) avoids a duplicate-session business
                    // error standing in for what this assertion checks.
                    let boundary = "parity-route-boundary";
                    let body = multipart_body(
                        boundary,
                        &[
                            (
                                "session",
                                "text/plain; charset=utf-8",
                                b"parity-route-session",
                            ),
                            (
                                "subject_label",
                                "text/plain; charset=utf-8",
                                b"Route parity fixture",
                            ),
                            (
                                "creator_name",
                                "text/plain; charset=utf-8",
                                b"Parity tester",
                            ),
                            (
                                "original_image",
                                "application/octet-stream",
                                b"\x89PNG\r\n\x1a\nparity-original",
                            ),
                            (
                                "current_image",
                                "application/octet-stream",
                                b"<svg xmlns='http://www.w3.org/2000/svg'><rect/></svg>",
                            ),
                            (
                                "ai_output",
                                "application/octet-stream",
                                b"GIF89aparity-ai-output",
                            ),
                        ],
                    );
                    app.clone()
                        .oneshot(unsafe_api_request(
                            &full_path,
                            format!("multipart/form-data; boundary={boundary}"),
                            Body::from(body),
                        ))
                        .await
                        .unwrap()
                }
                "post" if resolved_path.ends_with("/decisions") => {
                    let decision_body = serde_json::to_vec(&serde_json::json!({
                        "review_id": review_id,
                        "disposition": "adopt",
                        "rationale": "Openapi route-parity check."
                    }))
                    .unwrap();
                    app.clone()
                        .oneshot(unsafe_api_request(
                            &full_path,
                            "application/json",
                            Body::from(decision_body),
                        ))
                        .await
                        .unwrap()
                }
                "post" if resolved_path.ends_with("/operations/fsck") => {
                    // startFsck: reuses the confirmation body already
                    // proven valid by the fixture-setup call above; a
                    // second, independent start is fine (each call
                    // reserves its own operation_id).
                    let body = r#"{"confirm_project_key":"demo"}"#;
                    app.clone()
                        .oneshot(unsafe_api_request(
                            &full_path,
                            "application/json",
                            Body::from(body),
                        ))
                        .await
                        .unwrap()
                }
                other => panic!(
                    "unhandled implemented-operation method {other} for {path_template}; \
                     add a substitution branch above"
                ),
            };

            assert_ne!(
                response.status(),
                StatusCode::NOT_FOUND,
                "{} {full_path} (openapi operationId {operation_id:?}) returned 404: the \
                 route is documented but not wired into the router",
                method.to_uppercase()
            );
            assert_ne!(
                response.status(),
                StatusCode::METHOD_NOT_ALLOWED,
                "{} {full_path} (openapi operationId {operation_id:?}) returned 405: the \
                 path exists but this method is not wired",
                method.to_uppercase()
            );
            checked.push(format!(
                "{} {full_path} ({operation_id})",
                method.to_uppercase()
            ));
        }
    }

    // Sanity bounds so a change to openapi.json's paths silently
    // shrinking the enumerated set (e.g. an accidental truncation)
    // would fail loudly instead of this test quietly checking nothing.
    assert_eq!(
        checked.len(),
        13,
        "expected 13 implemented operations, checked: {checked:?}"
    );
    assert_eq!(
        skipped_unimplemented_archive.len(),
        UNIMPLEMENTED_ARCHIVE_OPERATIONS.len(),
        "expected exactly the {} UNIMPLEMENTED_ARCHIVE_OPERATIONS entries, found: \
         {skipped_unimplemented_archive:?}",
        UNIMPLEMENTED_ARCHIVE_OPERATIONS.len()
    );
}

#[tokio::test]
async fn unregistered_api_path_returns_404_not_403_or_405() {
    // Canary for the route-parity test above: confirms an unregistered
    // path is distinguishable as 404 (not swallowed by the security
    // middleware as 403, nor reported as 405) when the request already
    // satisfies the local browser security policy.
    let (_directory, app) = test_app();
    let response = app
        .oneshot(
            request("/api/v1/nonexistent")
                .header("x-synapse-local-token", "a".repeat(64))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}
