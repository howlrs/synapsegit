use askama::Template;
use axum::Json;
use axum::body::{Body, to_bytes};
use axum::extract::{
    FromRequest, Multipart, OriginalUri, Path, RawQuery, Request as AxumRequest, State,
};
use axum::http::header::{CONTENT_DISPOSITION, CONTENT_LENGTH, CONTENT_TYPE};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use serde::de::DeserializeOwned;
use synapse_local_service::{
    ArchiveExportRequest, ArchiveRestoreRequest, ArchiveResultKind, CreatorDecisionRequest,
    CreatorDecisionResponse, CreatorImage, HealthResponse, ImageRole, LocalService, OperationKind,
    OperationResult, OperationState, Problem as ServiceProblem, ProjectConfirmation, ReflogQuery,
    ServiceError,
};

use crate::problem::problem_response;
use crate::staging::StagedCreatorUpload;
use crate::state::{AppState, BlockingError, OperationRegistryError};
use crate::templates::{ErrorTemplate, IndexTemplate, ProjectTemplate, SessionTemplate};
use crate::views::{
    ArchiveView, HttpFailure, ProjectCardView, RefView, ReflogView, SessionPageView,
    SessionSummaryView, archive_checksum_preview, archive_state_label, archive_state_tone,
    project_state_label, project_state_tone, session_state_label, session_state_tone,
};

pub(crate) const MAX_DECISION_JSON_BYTES: usize = 8 * 1024;
const MAX_MAINTENANCE_JSON_BYTES: usize = 8 * 1024;

pub(crate) async fn api_health(State(state): State<AppState>) -> Json<HealthResponse> {
    Json(HealthResponse::new(state.security.server_instance()))
}

pub(crate) async fn api_projects(
    State(state): State<AppState>,
) -> Json<synapse_local_service::ProjectList> {
    Json(state.service.list_projects())
}

pub(crate) async fn api_archives(State(state): State<AppState>) -> Response {
    // Archive listing scans a server-owned archive root, not a project
    // repository, so it has no per-project blocking gate key to acquire.
    match run_blocking(state.clone(), None, |service| service.list_archives()).await {
        Ok(archives) => Json(archives).into_response(),
        Err(BlockingError::Service(error)) => failure_response(HttpFailure::service(&state, error)),
        Err(BlockingError::Task) => failure_response(HttpFailure::internal(
            &state,
            "The archive listing task failed.",
        )),
    }
}

pub(crate) async fn api_project_status(
    State(state): State<AppState>,
    Path(project_key): Path<String>,
) -> Response {
    let gate_key = project_key.clone();
    api_blocking(state.clone(), gate_key, move |service| {
        service.project_status(&project_key)
    })
    .await
}

pub(crate) async fn api_project_refs(
    State(state): State<AppState>,
    Path(project_key): Path<String>,
) -> Response {
    let gate_key = project_key.clone();
    api_blocking(state.clone(), gate_key, move |service| {
        service.list_refs(&project_key)
    })
    .await
}

pub(crate) async fn api_project_reflog(
    State(state): State<AppState>,
    Path(project_key): Path<String>,
    RawQuery(raw_query): RawQuery,
) -> Response {
    let query = match parse_query::<ReflogQuery>(raw_query.as_deref()) {
        Ok(query) => query,
        Err(detail) => {
            return failure_response(HttpFailure::request(&state, "local_request_denied", detail));
        }
    };
    let gate_key = project_key.clone();
    api_blocking(state.clone(), gate_key, move |service| {
        service.list_reflog(&project_key, query)
    })
    .await
}

pub(crate) async fn api_creator_sessions(
    State(state): State<AppState>,
    Path(project_key): Path<String>,
) -> Response {
    let gate_key = project_key.clone();
    api_blocking(state.clone(), gate_key, move |service| {
        service.list_creator_sessions(&project_key)
    })
    .await
}

pub(crate) async fn api_begin_creator_session(
    State(state): State<AppState>,
    Path(project_key): Path<String>,
    request: AxumRequest,
) -> Response {
    if !is_exact_multipart_content_type(request.headers()) {
        return failure_response(HttpFailure::request(
            &state,
            "local_request_denied",
            "The request Content-Type must be multipart/form-data with exactly one boundary.",
        ));
    }
    let upload_permit = match state.uploads.clone().acquire_owned().await {
        Ok(permit) => permit,
        Err(_) => {
            return failure_response(HttpFailure::internal(
                &state,
                "The upload concurrency gate is unavailable.",
            ));
        }
    };
    let multipart = match Multipart::from_request(request, &state).await {
        Ok(multipart) => multipart,
        Err(_) => {
            return failure_response(HttpFailure::request(
                &state,
                "local_request_denied",
                "The multipart boundary is invalid.",
            ));
        }
    };
    let staged = match StagedCreatorUpload::from_multipart(multipart).await {
        Ok(staged) => staged,
        Err(error) => return failure_response(error.into_http_failure(&state)),
    };

    let gate_key = project_key.clone();
    let server_instance = state.security.server_instance().to_owned();
    match run_blocking(state.clone(), Some(gate_key), move |service| {
        // Staging is deliberately owned by the detached blocking closure. If
        // the client disconnects while Creator is publishing, the directory
        // remains alive until the service has completed publication and
        // retained the pending review capability.
        let StagedCreatorUpload {
            _directory,
            request,
        } = staged;
        let _upload_permit = upload_permit;
        let result = service.begin_creator_session(&project_key, &server_instance, request);
        drop(_directory);
        result
    })
    .await
    {
        Ok(pending) => (StatusCode::CREATED, Json(pending)).into_response(),
        Err(BlockingError::Service(error)) => failure_response(HttpFailure::service(&state, error)),
        Err(BlockingError::Task) => failure_response(HttpFailure::internal(
            &state,
            "The creator proposal task failed.",
        )),
    }
}

pub(crate) async fn api_decide_creator_session(
    State(state): State<AppState>,
    Path((project_key, session)): Path<(String, String)>,
    request: AxumRequest,
) -> Response {
    if !is_exact_json_content_type(request.headers()) {
        return failure_response(HttpFailure::request(
            &state,
            "local_request_denied",
            "The request Content-Type must be exactly application/json.",
        ));
    }
    let body = match to_bytes(request.into_body(), MAX_DECISION_JSON_BYTES).await {
        Ok(body) => body,
        Err(_) => {
            return failure_response(HttpFailure::limit(
                &state,
                "The decision request exceeds the 8 KiB wire limit.",
            ));
        }
    };
    let decision = match serde_json::from_slice::<CreatorDecisionRequest>(&body) {
        Ok(decision) => decision,
        Err(_) => {
            return failure_response(HttpFailure::request(
                &state,
                "local_request_denied",
                "The decision JSON is invalid or contains an unknown or duplicate field.",
            ));
        }
    };

    let gate_key = project_key.clone();
    let server_instance = state.security.server_instance().to_owned();
    match run_blocking(state.clone(), Some(gate_key), move |service| {
        service.decide_creator_session(&project_key, &session, &server_instance, decision)
    })
    .await
    {
        Ok(outcome) => decision_success_response(outcome),
        Err(BlockingError::Service(error)) => failure_response(HttpFailure::service(&state, error)),
        Err(BlockingError::Task) => failure_response(HttpFailure::internal(
            &state,
            "The creator decision task failed.",
        )),
    }
}

pub(crate) fn decision_success_response(outcome: CreatorDecisionResponse) -> Response {
    Json(outcome).into_response()
}

pub(crate) async fn api_start_fsck(
    State(state): State<AppState>,
    Path(project_key): Path<String>,
    request: AxumRequest,
) -> Response {
    if !is_exact_json_content_type(request.headers()) {
        return failure_response(HttpFailure::request(
            &state,
            "local_request_denied",
            "The request Content-Type must be exactly application/json.",
        ));
    }
    let body = match to_bytes(request.into_body(), MAX_MAINTENANCE_JSON_BYTES).await {
        Ok(body) => body,
        Err(_) => {
            return failure_response(HttpFailure::limit(
                &state,
                "The maintenance request exceeds the 8 KiB wire limit.",
            ));
        }
    };
    let confirmation = match serde_json::from_slice::<ProjectConfirmation>(&body) {
        Ok(confirmation) => confirmation,
        Err(_) => {
            return failure_response(HttpFailure::request(
                &state,
                "local_request_denied",
                "The maintenance JSON is invalid or contains an unknown or duplicate field.",
            ));
        }
    };
    if let Err(error) = state
        .service
        .validate_fsck_confirmation(&project_key, &confirmation)
    {
        return failure_response(HttpFailure::service(&state, error));
    }

    let accepted = match state
        .operations
        .reserve(OperationKind::Fsck, project_key.clone())
    {
        Ok(accepted) => accepted,
        Err(OperationRegistryError::Capacity) => {
            return failure_response(HttpFailure {
                status: StatusCode::TOO_MANY_REQUESTS,
                code: "resource_limit".into(),
                title: "Maintenance capacity reached".into(),
                detail: "The process-local maintenance operation registry is full.".into(),
                request_id: state.security.request_id(),
                retryable: true,
            });
        }
        Err(OperationRegistryError::Entropy | OperationRegistryError::Clock) => {
            return failure_response(HttpFailure::internal(
                &state,
                "The maintenance operation could not be reserved.",
            ));
        }
    };

    let operation_id = accepted.operation_id.clone();
    let worker_state = state.clone();
    tokio::spawn(async move {
        let gate_key = project_key.clone();
        let running_operations = worker_state.operations.clone();
        let running_operation_id = operation_id.clone();
        let outcome = run_blocking_after_acquire(
            worker_state.clone(),
            Some(gate_key),
            move || running_operations.mark_running(&running_operation_id),
            move |service| service.run_maintenance_fsck(&project_key),
        )
        .await;
        match outcome {
            Ok(result) => worker_state.operations.finish(
                &operation_id,
                OperationState::Succeeded,
                Some(OperationResult::Fsck(result)),
                None,
            ),
            Err(BlockingError::Service(error)) => worker_state.operations.finish(
                &operation_id,
                OperationState::Failed,
                None,
                Some(service_operation_problem(&worker_state, error)),
            ),
            Err(BlockingError::Task) => worker_state.operations.finish(
                &operation_id,
                OperationState::OutcomeUnknown,
                None,
                Some(operation_problem(
                    &worker_state,
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "storage_error",
                    "The integrity-check worker stopped before its final state was recorded.",
                    false,
                )),
            ),
        }
    });

    (StatusCode::ACCEPTED, Json(accepted)).into_response()
}

pub(crate) async fn api_start_archive_export(
    State(state): State<AppState>,
    Path(project_key): Path<String>,
    request: AxumRequest,
) -> Response {
    if !is_exact_json_content_type(request.headers()) {
        return failure_response(HttpFailure::request(
            &state,
            "local_request_denied",
            "The request Content-Type must be exactly application/json.",
        ));
    }
    let body = match to_bytes(request.into_body(), MAX_MAINTENANCE_JSON_BYTES).await {
        Ok(body) => body,
        Err(_) => {
            return failure_response(HttpFailure::limit(
                &state,
                "The maintenance request exceeds the 8 KiB wire limit.",
            ));
        }
    };
    let export = match serde_json::from_slice::<ArchiveExportRequest>(&body) {
        Ok(export) => export,
        Err(_) => {
            return failure_response(HttpFailure::request(
                &state,
                "local_request_denied",
                "The maintenance JSON is invalid or contains an unknown or duplicate field.",
            ));
        }
    };
    if let Err(error) = state
        .service
        .validate_archive_export_request(&project_key, &export)
    {
        return failure_response(HttpFailure::service(&state, error));
    }

    let accepted = match state
        .operations
        .reserve(OperationKind::ArchiveExport, project_key.clone())
    {
        Ok(accepted) => accepted,
        Err(OperationRegistryError::Capacity) => {
            return failure_response(HttpFailure {
                status: StatusCode::TOO_MANY_REQUESTS,
                code: "resource_limit".into(),
                title: "Maintenance capacity reached".into(),
                detail: "The process-local maintenance operation registry is full.".into(),
                request_id: state.security.request_id(),
                retryable: true,
            });
        }
        Err(OperationRegistryError::Entropy | OperationRegistryError::Clock) => {
            return failure_response(HttpFailure::internal(
                &state,
                "The maintenance operation could not be reserved.",
            ));
        }
    };

    let operation_id = accepted.operation_id.clone();
    let archive_name = export.archive_name;
    let worker_state = state.clone();
    tokio::spawn(async move {
        let gate_key = project_key.clone();
        let running_operations = worker_state.operations.clone();
        let running_operation_id = operation_id.clone();
        let outcome = run_blocking_after_acquire(
            worker_state.clone(),
            Some(gate_key),
            move || running_operations.mark_running(&running_operation_id),
            move |service| service.run_archive_export(&project_key, &archive_name),
        )
        .await;
        match outcome {
            Ok(result) => {
                debug_assert_eq!(result.result_kind, ArchiveResultKind::Exported);
                worker_state.operations.finish(
                    &operation_id,
                    OperationState::Succeeded,
                    Some(OperationResult::Archive(result)),
                    None,
                );
            }
            Err(BlockingError::Service(error)) => worker_state.operations.finish(
                &operation_id,
                OperationState::Failed,
                None,
                Some(service_operation_problem(&worker_state, error)),
            ),
            Err(BlockingError::Task) => worker_state.operations.finish(
                &operation_id,
                OperationState::OutcomeUnknown,
                None,
                Some(operation_problem(
                    &worker_state,
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "storage_error",
                    "The archive-export worker stopped before its final state was recorded.",
                    false,
                )),
            ),
        }
    });

    (StatusCode::ACCEPTED, Json(accepted)).into_response()
}

pub(crate) async fn api_start_archive_restore(
    State(state): State<AppState>,
    Path(project_key): Path<String>,
    request: AxumRequest,
) -> Response {
    if !is_exact_json_content_type(request.headers()) {
        return failure_response(HttpFailure::request(
            &state,
            "local_request_denied",
            "The request Content-Type must be exactly application/json.",
        ));
    }
    let body = match to_bytes(request.into_body(), MAX_MAINTENANCE_JSON_BYTES).await {
        Ok(body) => body,
        Err(_) => {
            return failure_response(HttpFailure::limit(
                &state,
                "The maintenance request exceeds the 8 KiB wire limit.",
            ));
        }
    };
    let restore = match serde_json::from_slice::<ArchiveRestoreRequest>(&body) {
        Ok(restore) => restore,
        Err(_) => {
            return failure_response(HttpFailure::request(
                &state,
                "local_request_denied",
                "The maintenance JSON is invalid or contains an unknown or duplicate field.",
            ));
        }
    };
    if let Err(error) = state
        .service
        .validate_archive_restore_request(&project_key, &restore)
    {
        return failure_response(HttpFailure::service(&state, error));
    }

    let accepted = match state
        .operations
        .reserve(OperationKind::ArchiveRestore, project_key.clone())
    {
        Ok(accepted) => accepted,
        Err(OperationRegistryError::Capacity) => {
            return failure_response(HttpFailure {
                status: StatusCode::TOO_MANY_REQUESTS,
                code: "resource_limit".into(),
                title: "Maintenance capacity reached".into(),
                detail: "The process-local maintenance operation registry is full.".into(),
                request_id: state.security.request_id(),
                retryable: true,
            });
        }
        Err(OperationRegistryError::Entropy | OperationRegistryError::Clock) => {
            return failure_response(HttpFailure::internal(
                &state,
                "The maintenance operation could not be reserved.",
            ));
        }
    };

    let operation_id = accepted.operation_id.clone();
    let archive_name = restore.archive_name;
    let worker_state = state.clone();
    tokio::spawn(async move {
        let gate_key = project_key.clone();
        let running_operations = worker_state.operations.clone();
        let running_operation_id = operation_id.clone();
        let outcome = run_blocking_after_acquire(
            worker_state.clone(),
            Some(gate_key),
            move || running_operations.mark_running(&running_operation_id),
            move |service| service.run_archive_restore(&project_key, &archive_name),
        )
        .await;
        match outcome {
            Ok(result) => {
                debug_assert_eq!(result.result_kind, ArchiveResultKind::Restored);
                worker_state.operations.finish(
                    &operation_id,
                    OperationState::Succeeded,
                    Some(OperationResult::Archive(result)),
                    None,
                );
            }
            Err(BlockingError::Service(error)) => worker_state.operations.finish(
                &operation_id,
                OperationState::Failed,
                None,
                Some(service_operation_problem(&worker_state, error)),
            ),
            Err(BlockingError::Task) => worker_state.operations.finish(
                &operation_id,
                OperationState::OutcomeUnknown,
                None,
                Some(operation_problem(
                    &worker_state,
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "storage_error",
                    "The archive-restore worker stopped before its final state was recorded.",
                    false,
                )),
            ),
        }
    });

    (StatusCode::ACCEPTED, Json(accepted)).into_response()
}

pub(crate) async fn api_operation(
    State(state): State<AppState>,
    Path(operation_id): Path<String>,
) -> Response {
    if valid_operation_id(&operation_id)
        && let Some(status) = state.operations.get(&operation_id)
    {
        return Json(status).into_response();
    }
    failure_response(HttpFailure {
        status: StatusCode::NOT_FOUND,
        code: "operation_state_lost".into(),
        title: "Operation state unavailable".into(),
        detail: "The process-local maintenance operation state is unavailable.".into(),
        request_id: state.security.request_id(),
        retryable: false,
    })
}

pub(crate) fn valid_operation_id(operation_id: &str) -> bool {
    (22..=128).contains(&operation_id.len())
        && operation_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

fn single_content_type(headers: &HeaderMap) -> Option<&str> {
    let mut values = headers.get_all(CONTENT_TYPE).iter();
    let value = values.next()?;
    if values.next().is_some() {
        return None;
    }
    value.to_str().ok()
}

fn is_exact_multipart_content_type(headers: &HeaderMap) -> bool {
    let Some(value) = single_content_type(headers) else {
        return false;
    };
    let Ok(media_type) = value.parse::<mime::Mime>() else {
        return false;
    };
    if media_type.type_() != mime::MULTIPART || media_type.subtype() != mime::FORM_DATA {
        return false;
    }
    let mut parameters = media_type.params();
    matches!(
        parameters.next(),
        Some((name, value)) if name == mime::BOUNDARY && !value.as_str().is_empty()
    ) && parameters.next().is_none()
}

fn is_exact_json_content_type(headers: &HeaderMap) -> bool {
    single_content_type(headers)
        .and_then(|value| value.parse::<mime::Mime>().ok())
        .is_some_and(|media_type| media_type == mime::APPLICATION_JSON)
}

pub(crate) fn has_exact_part_content_type(headers: &HeaderMap, expected: &mime::Mime) -> bool {
    single_content_type(headers)
        .and_then(|value| value.parse::<mime::Mime>().ok())
        .is_some_and(|media_type| media_type == *expected)
}

pub(crate) async fn api_creator_session(
    State(state): State<AppState>,
    Path((project_key, session)): Path<(String, String)>,
) -> Response {
    let gate_key = project_key.clone();
    api_blocking(state.clone(), gate_key, move |service| {
        service.get_creator_session(&project_key, &session)
    })
    .await
}

pub(crate) async fn api_creator_session_diagnostics(
    State(state): State<AppState>,
    Path((project_key, session)): Path<(String, String)>,
) -> Response {
    let gate_key = project_key.clone();
    api_blocking(state.clone(), gate_key, move |service| {
        service.get_creator_session_diagnostic(&project_key, &session)
    })
    .await
}

pub(crate) async fn api_creator_image(
    State(state): State<AppState>,
    Path((project_key, session, role)): Path<(String, String, String)>,
) -> Response {
    let Some(role) = ImageRole::parse(&role) else {
        return failure_response(HttpFailure::not_found(
            &state,
            "The requested creator image role was not found.",
        ));
    };
    let role_name = match role {
        ImageRole::Original => "original",
        ImageRole::Current => "current",
        ImageRole::AiOutput => "ai-output",
    };
    let session_for_read = session.clone();
    let gate_key = project_key.clone();
    match run_blocking(state.clone(), Some(gate_key), move |service| {
        service.get_creator_session_image(&project_key, &session_for_read, role)
    })
    .await
    {
        Ok(image) => image_response(image, &session, role_name),
        Err(BlockingError::Service(error)) => failure_response(HttpFailure::service(&state, error)),
        Err(BlockingError::Task) => failure_response(HttpFailure::internal(
            &state,
            "The creator image read task failed.",
        )),
    }
}

async fn api_blocking<T, F>(state: AppState, project_key: String, operation: F) -> Response
where
    T: serde::Serialize + Send + 'static,
    F: FnOnce(&LocalService) -> Result<T, ServiceError> + Send + 'static,
{
    match run_blocking(state.clone(), Some(project_key), operation).await {
        Ok(value) => Json(value).into_response(),
        Err(BlockingError::Service(error)) => failure_response(HttpFailure::service(&state, error)),
        Err(BlockingError::Task) => {
            failure_response(HttpFailure::internal(&state, "The local read task failed."))
        }
    }
}

async fn run_blocking<T, F>(
    state: AppState,
    project_key: Option<String>,
    operation: F,
) -> Result<T, BlockingError>
where
    T: Send + 'static,
    F: FnOnce(&LocalService) -> Result<T, ServiceError> + Send + 'static,
{
    run_blocking_after_acquire(state, project_key, || {}, operation).await
}

pub(crate) async fn run_blocking_after_acquire<T, S, F>(
    state: AppState,
    project_key: Option<String>,
    on_started: S,
    operation: F,
) -> Result<T, BlockingError>
where
    T: Send + 'static,
    S: FnOnce() + Send + 'static,
    F: FnOnce(&LocalService) -> Result<T, ServiceError> + Send + 'static,
{
    let permit = state.blocking.acquire(project_key.as_deref()).await?;
    let service = state.service;
    // Queued maintenance operations transition only after both gates are
    // held, immediately before their synchronous work is dispatched.
    on_started();
    tokio::task::spawn_blocking(move || {
        // The permit deliberately lives in the blocking closure. Dropping the
        // handler future detaches the blocking task but cannot release either
        // gate before the synchronous Repository/SQLite operation finishes.
        let _permit = permit;
        operation(&service)
    })
    .await
    .map_err(|_| BlockingError::Task)?
    .map_err(BlockingError::Service)
}

fn parse_query<T: DeserializeOwned>(query: Option<&str>) -> Result<T, &'static str> {
    serde_urlencoded::from_str(query.unwrap_or_default())
        .map_err(|_| "The request query is invalid or contains an unknown field.")
}

fn image_response(image: CreatorImage, session: &str, role: &str) -> Response {
    let content_type = image.media_type.content_type();
    let disposition = if image.media_type.is_attachment() {
        format!("attachment; filename=\"{session}-{role}.bin\"")
    } else {
        "inline".to_owned()
    };
    let byte_len = image.bytes.len();
    let mut response = Response::new(Body::from(image.bytes));
    *response.status_mut() = StatusCode::OK;
    let headers = response.headers_mut();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static(content_type));
    headers.insert(
        CONTENT_LENGTH,
        HeaderValue::from_str(&byte_len.to_string())
            .expect("a decimal usize is a valid header value"),
    );
    headers.insert(
        CONTENT_DISPOSITION,
        HeaderValue::from_str(&disposition)
            .expect("validated slugs produce a valid Content-Disposition"),
    );
    headers.insert(
        "x-synapse-blob-oid",
        HeaderValue::from_str(&image.blob_oid).expect("a validated OID is a valid header value"),
    );
    response
}

pub(crate) async fn index_page(State(state): State<AppState>) -> Response {
    let projects = state.service.list_projects();
    let mut cards = Vec::with_capacity(projects.projects.len());
    for project in projects.projects {
        let project_key = project.project_key;
        let gate_key = project_key.clone();
        let status = match run_blocking(state.clone(), Some(gate_key), move |service| {
            service.project_status(&project_key)
        })
        .await
        {
            Ok(status) => status,
            Err(BlockingError::Service(error)) => {
                return page_failure(&state, HttpFailure::service(&state, error));
            }
            Err(BlockingError::Task) => {
                return page_failure(
                    &state,
                    HttpFailure::internal(&state, "The project dashboard could not be built."),
                );
            }
        };
        let project = status.project;
        cards.push(ProjectCardView {
            key: project.project_key,
            label: project.display_label,
            state_label: project_state_label(project.state),
            tone: project_state_tone(project.state),
            ref_count: status.snapshot.ref_count,
            complete_sessions: status.creator_session_counts.complete,
            pending_sessions: status.creator_session_counts.pending_review,
            incomplete_sessions: status.creator_session_counts.incomplete,
        });
    }

    // Archive listing degrades independently of the project dashboard: an
    // archive-root read failure (for example a resource_limit from too many
    // root entries) must not turn the whole project dashboard into a page
    // failure, since archives are server-wide state unrelated to any one
    // project's availability. A failure renders as an inline notice in the
    // archives section instead.
    let (archives, archives_error) =
        match run_blocking(state.clone(), None, |service| service.list_archives()).await {
            Ok(list) => (
                list.archives
                    .into_iter()
                    .map(|archive| ArchiveView {
                        archive_name: archive.archive_name,
                        state_label: archive_state_label(archive.state),
                        tone: archive_state_tone(archive.state),
                        checksum_preview: archive_checksum_preview(
                            archive.manifest_checksum.as_deref(),
                        ),
                    })
                    .collect::<Vec<_>>(),
                None,
            ),
            Err(BlockingError::Service(error)) => {
                (Vec::new(), Some(HttpFailure::service(&state, error).detail))
            }
            Err(BlockingError::Task) => (
                Vec::new(),
                Some("The archive listing task failed.".to_owned()),
            ),
        };

    render_template(
        &state,
        IndexTemplate {
            page_title: "プロジェクト",
            token: state.security.token(),
            projects: &cards,
            archives: &archives,
            archives_error: archives_error.as_deref(),
        },
    )
}

pub(crate) async fn project_page(
    State(state): State<AppState>,
    Path(project_key): Path<String>,
) -> Response {
    let key = project_key.clone();
    let dashboard = match run_dashboard(state.clone(), project_key).await {
        Ok(dashboard) => dashboard,
        Err(DashboardError::Service(error)) => {
            return page_failure(&state, HttpFailure::service(&state, error));
        }
        Err(DashboardError::Changed) => {
            return page_failure(
                &state,
                HttpFailure {
                    status: StatusCode::CONFLICT,
                    code: "ref_conflict".into(),
                    title: "Project changed during read".into(),
                    detail:
                        "The project changed while the dashboard was being built. Reload the page."
                            .into(),
                    request_id: state.security.request_id(),
                    retryable: true,
                },
            );
        }
        Err(DashboardError::Task) => {
            return page_failure(
                &state,
                HttpFailure::internal(&state, "The project dashboard read task failed."),
            );
        }
    };
    let refs = dashboard
        .refs
        .refs
        .into_iter()
        .map(|reference| RefView {
            name: reference.name,
            head: reference.head,
            event_id: reference.updated_event_id,
        })
        .collect::<Vec<_>>();
    let reflog = dashboard
        .reflog
        .entries
        .into_iter()
        .map(|entry| ReflogView {
            event_id: entry.event_id,
            ref_name: entry.ref_name,
            new_head: entry.new_head,
            message: entry.message.unwrap_or_else(|| "メッセージなし".into()),
        })
        .collect::<Vec<_>>();
    let sessions = dashboard
        .sessions
        .sessions
        .into_iter()
        .map(|session| SessionSummaryView {
            session: session.session,
            state_label: session_state_label(session.state),
            tone: session_state_tone(session.state),
            proposal_head: session.proposal_head.unwrap_or_else(|| "—".into()),
            decision_head: session.decision_head.unwrap_or_else(|| "—".into()),
        })
        .collect::<Vec<_>>();
    let fsck_supported = dashboard.status.project.capabilities.fsck;
    let archive_export_supported = dashboard.status.project.capabilities.archive_export;
    let has_last_fsck = dashboard.status.last_fsck.is_some();
    let last_fsck_clean = dashboard
        .status
        .last_fsck
        .as_ref()
        .is_some_and(|result| result.clean);
    let last_fsck_objects = dashboard
        .status
        .last_fsck
        .as_ref()
        .map_or(0, |result| result.objects_verified);
    let last_fsck_issues = dashboard
        .status
        .last_fsck
        .as_ref()
        .map_or(0, |result| result.issue_count);
    let project_label = dashboard.status.project.display_label;
    let watermark = dashboard.status.snapshot.watermark;
    render_template(
        &state,
        ProjectTemplate {
            page_title: &project_label,
            token: state.security.token(),
            project_key: &key,
            project_label: &project_label,
            watermark: &watermark,
            complete_sessions: dashboard.status.creator_session_counts.complete,
            pending_sessions: dashboard.status.creator_session_counts.pending_review,
            incomplete_sessions: dashboard.status.creator_session_counts.incomplete,
            fsck_supported,
            archive_export_supported,
            has_last_fsck,
            last_fsck_clean,
            last_fsck_objects,
            last_fsck_issues,
            refs: &refs,
            reflog: &reflog,
            sessions: &sessions,
        },
    )
}

struct Dashboard {
    status: synapse_local_service::ProjectStatus,
    refs: synapse_local_service::RefList,
    reflog: synapse_local_service::ReflogPage,
    sessions: synapse_local_service::CreatorSessionList,
}

async fn run_dashboard(state: AppState, project_key: String) -> Result<Dashboard, DashboardError> {
    let gate_key = project_key.clone();
    run_blocking(state, Some(gate_key), move |service| {
        for _ in 0..3 {
            let status = service.project_status(&project_key)?;
            let refs = service.list_refs(&project_key)?;
            let after = refs
                .refs
                .iter()
                .filter_map(|reference| reference.updated_event_id.parse::<i64>().ok())
                .max()
                .map(|last| last.saturating_sub(20).max(0).to_string());
            let reflog = service.list_reflog(
                &project_key,
                ReflogQuery {
                    after_event_id: after,
                    limit: 20,
                    ..ReflogQuery::default()
                },
            )?;
            let sessions = service.list_creator_sessions(&project_key)?;
            let watermark = &status.snapshot.watermark;
            if refs.snapshot.watermark == *watermark
                && reflog.snapshot.watermark == *watermark
                && sessions.snapshot.watermark == *watermark
            {
                return Ok(Some(Dashboard {
                    status,
                    refs,
                    reflog,
                    sessions,
                }));
            }
        }
        Ok(None)
    })
    .await
    .map_err(|error| match error {
        BlockingError::Service(error) => DashboardError::Service(error),
        BlockingError::Task => DashboardError::Task,
    })?
    .ok_or(DashboardError::Changed)
}

enum DashboardError {
    Service(ServiceError),
    Changed,
    Task,
}

pub(crate) async fn session_page(
    State(state): State<AppState>,
    Path((project_key, session)): Path<(String, String)>,
) -> Response {
    let project_key_for_read = project_key.clone();
    let session_for_read = session.clone();
    let gate_key = project_key.clone();
    let (project_label, detail, diagnostic) =
        match run_blocking(state.clone(), Some(gate_key), move |service| {
            let project = service.project_status(&project_key_for_read)?.project;
            let (detail, diagnostic) = service
                .get_creator_session_with_diagnostic(&project.project_key, &session_for_read)?;
            Ok((project.display_label, detail, diagnostic))
        })
        .await
        {
            Ok(value) => value,
            Err(BlockingError::Service(error)) => {
                return page_failure(&state, HttpFailure::service(&state, error));
            }
            Err(BlockingError::Task) => {
                return page_failure(
                    &state,
                    HttpFailure::internal(&state, "The creator session page could not be built."),
                );
            }
        };

    let view = SessionPageView::new(&project_key, &project_label, &session, detail, diagnostic);
    render_template(
        &state,
        SessionTemplate {
            page_title: &session,
            token: state.security.token(),
            project_key: &project_key,
            project_label: &project_label,
            session: &session,
            complete: view.complete,
            pending: view.pending,
            show_evidence: view.show_evidence,
            state_label: &view.state_label,
            state_tone: &view.state_tone,
            state_description: &view.state_description,
            ai_output_source: &view.ai_output_source,
            review_id: &view.review_id,
            decision_url: &view.decision_url,
            disposition: &view.disposition,
            selected: &view.selected,
            fsck_objects: view.fsck_objects,
            images: &view.images,
            has_comparison: view.has_comparison,
            comparison_outcome: &view.comparison_outcome,
            comparison_warning: &view.comparison_warning,
            comparison_status: &view.comparison_status,
            comparison_comparability: &view.comparison_comparability,
            comparison_adapter: &view.comparison_adapter,
            comparison_replay: &view.comparison_replay,
            timeline: &view.timeline,
            diagnostic: &view.diagnostic,
            diagnostic_proposal_ref: &view.diagnostic_proposal_ref,
            diagnostic_proposal_head: &view.diagnostic_proposal_head,
            diagnostic_decision_ref: &view.diagnostic_decision_ref,
            diagnostic_decision_head: &view.diagnostic_decision_head,
        },
    )
}

pub(crate) async fn not_found(
    State(state): State<AppState>,
    OriginalUri(uri): OriginalUri,
) -> Response {
    let failure = HttpFailure::not_found(&state, "The requested local resource was not found.");
    if uri.path().starts_with("/api/v1") {
        failure_response(failure)
    } else {
        page_failure(&state, failure)
    }
}

pub(crate) async fn method_not_allowed(
    State(state): State<AppState>,
    OriginalUri(uri): OriginalUri,
) -> Response {
    let failure = HttpFailure {
        status: StatusCode::METHOD_NOT_ALLOWED,
        code: "local_request_denied".into(),
        title: "Method not allowed".into(),
        detail: "This local resource does not support the requested method.".into(),
        request_id: state.security.request_id(),
        retryable: false,
    };
    if uri.path().starts_with("/api/v1") {
        failure_response(failure)
    } else {
        page_failure(&state, failure)
    }
}

fn service_operation_problem(state: &AppState, error: ServiceError) -> ServiceProblem {
    let failure = HttpFailure::service(state, error);
    ServiceProblem {
        r#type: format!("urn:synapsegit:error:{}", failure.code),
        title: failure.title,
        status: failure.status.as_u16(),
        code: failure.code,
        detail: failure.detail,
        request_id: failure.request_id,
        retryable: failure.retryable,
    }
}

fn operation_problem(
    state: &AppState,
    status: StatusCode,
    code: &str,
    detail: &str,
    retryable: bool,
) -> ServiceProblem {
    ServiceProblem {
        r#type: format!("urn:synapsegit:error:{code}"),
        title: status
            .canonical_reason()
            .unwrap_or("Local application error")
            .into(),
        status: status.as_u16(),
        code: code.into(),
        detail: detail.into(),
        request_id: state.security.request_id(),
        retryable,
    }
}

fn failure_response(failure: HttpFailure) -> Response {
    problem_response(
        failure.status,
        &failure.code,
        &failure.title,
        &failure.detail,
        failure.request_id,
        failure.retryable,
    )
}

fn page_failure(state: &AppState, failure: HttpFailure) -> Response {
    let status_text = failure.status.as_u16().to_string();
    let template = ErrorTemplate {
        page_title: &failure.title,
        token: state.security.token(),
        status: &status_text,
        title: &failure.title,
        detail: &failure.detail,
        request_id: &failure.request_id,
    };
    match template.render() {
        Ok(html) => (failure.status, Html(html)).into_response(),
        Err(_) => failure_response(failure),
    }
}

fn render_template(state: &AppState, template: impl Template) -> Response {
    match template.render() {
        Ok(html) => Html(html).into_response(),
        Err(_) => page_failure(
            state,
            HttpFailure::internal(state, "The local page could not be rendered."),
        ),
    }
}
