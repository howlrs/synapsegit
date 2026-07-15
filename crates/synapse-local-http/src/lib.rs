//! Loopback-only HTTP and UI transport for the SynapseGit local application.

#![forbid(unsafe_code)]

mod problem;
mod security;

use askama::Template;
use axum::body::{Body, to_bytes};
use axum::extract::{
    DefaultBodyLimit, FromRequest, Multipart, OriginalUri, Path, RawQuery, Request as AxumRequest,
    State,
};
use axum::http::header::{CONTENT_DISPOSITION, CONTENT_LENGTH, CONTENT_TYPE};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::middleware;
use axum::response::{Html, IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use problem::problem_response;
use security::{SecurityPolicy, enforce_local_request};
use serde::de::DeserializeOwned;
use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;
use std::io;
use std::path::PathBuf;
use std::sync::Arc;
use synapse_local_service::{
    BeginCreatorSessionRequest, CreatorDecisionRequest, CreatorImage, CreatorReport,
    CreatorSessionDetail, CreatorSessionState, HealthResponse, ImageRole, LocalService,
    ProjectState, ReflogQuery, ServiceError,
};
use tokio::io::AsyncWriteExt;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

const APP_CSS: &str = include_str!("../assets/app.css");
const APP_JS: &str = include_str!("../assets/app.js");
const MAX_BLOCKING_OPERATIONS: usize = 8;
const MAX_BLOCKING_OPERATIONS_PER_PROJECT: usize = 2;
const MAX_CONCURRENT_CREATOR_UPLOADS: usize = 2;
const MAX_CREATOR_FILE_BYTES: usize = 64 * 1024 * 1024;
const MAX_CREATOR_FILE_AGGREGATE_BYTES: usize = 3 * MAX_CREATOR_FILE_BYTES;
// The file aggregate is the contractual payload. This small fixed allowance
// bounds the multipart framing, the three short text fields, and part headers.
const MAX_CREATOR_MULTIPART_WIRE_BYTES: usize = MAX_CREATOR_FILE_AGGREGATE_BYTES + 1024 * 1024;
const MAX_DECISION_JSON_BYTES: usize = 8 * 1024;
const MAX_SESSION_BYTES: usize = 64;
const MAX_SUBJECT_LABEL_BYTES: usize = 500;
const MAX_CREATOR_NAME_BYTES: usize = 300;

#[derive(Debug)]
pub struct StartupError {
    detail: String,
}

impl StartupError {
    fn new(detail: impl Into<String>) -> Self {
        Self {
            detail: detail.into(),
        }
    }
}

impl fmt::Display for StartupError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.detail)
    }
}

impl Error for StartupError {}

pub struct LocalHttpApplication {
    router: Router,
    origin: String,
}

impl LocalHttpApplication {
    pub fn origin(&self) -> &str {
        &self.origin
    }

    pub fn into_router(self) -> Router {
        self.router
    }
}

#[derive(Clone)]
struct AppState {
    service: Arc<LocalService>,
    security: SecurityPolicy,
    blocking: BlockingGates,
    uploads: Arc<Semaphore>,
}

#[derive(Clone)]
struct BlockingGates {
    overall: Arc<Semaphore>,
    projects: Arc<BTreeMap<String, Arc<Semaphore>>>,
}

impl BlockingGates {
    fn new(project_keys: impl IntoIterator<Item = String>) -> Self {
        let projects = project_keys
            .into_iter()
            .map(|key| {
                (
                    key,
                    Arc::new(Semaphore::new(MAX_BLOCKING_OPERATIONS_PER_PROJECT)),
                )
            })
            .collect();
        Self {
            overall: Arc::new(Semaphore::new(MAX_BLOCKING_OPERATIONS)),
            projects: Arc::new(projects),
        }
    }

    async fn acquire(&self, project_key: Option<&str>) -> Result<BlockingPermit, BlockingError> {
        // Acquire the narrower gate first so callers queued for one busy
        // project cannot consume all global capacity while they wait.
        let project = match project_key.and_then(|key| self.projects.get(key)) {
            Some(gate) => Some(
                gate.clone()
                    .acquire_owned()
                    .await
                    .map_err(|_| BlockingError::Task)?,
            ),
            None => None,
        };
        let overall = self
            .overall
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| BlockingError::Task)?;
        Ok(BlockingPermit {
            _overall: overall,
            _project: project,
        })
    }
}

struct BlockingPermit {
    _overall: OwnedSemaphorePermit,
    _project: Option<OwnedSemaphorePermit>,
}

pub fn build_local_application(
    service: Arc<LocalService>,
    port: u16,
) -> Result<LocalHttpApplication, StartupError> {
    if port == 0 {
        return Err(StartupError::new(
            "the HTTP application requires the listener's resolved non-zero port",
        ));
    }
    let token = random_hex(32)?;
    let server_instance = format!("local-{}", random_hex(16)?);
    Ok(build_with_identity(service, port, token, server_instance))
}

fn build_with_identity(
    service: Arc<LocalService>,
    port: u16,
    token: String,
    server_instance: String,
) -> LocalHttpApplication {
    let security = SecurityPolicy::new(port, token, server_instance);
    let origin = security.canonical_origin().to_owned();
    let blocking = BlockingGates::new(
        service
            .list_projects()
            .projects
            .into_iter()
            .map(|project| project.project_key),
    );
    let state = AppState {
        service,
        security: security.clone(),
        blocking,
        uploads: Arc::new(Semaphore::new(MAX_CONCURRENT_CREATOR_UPLOADS)),
    };

    let router = Router::new()
        .route("/", get(index_page))
        .route("/projects/{project_key}", get(project_page))
        .route(
            "/projects/{project_key}/creator-sessions/{session}",
            get(session_page),
        )
        .route("/assets/app.css", get(css_asset))
        .route("/assets/app.js", get(js_asset))
        .route("/api/v1/health", get(api_health))
        .route("/api/v1/projects", get(api_projects))
        .route(
            "/api/v1/projects/{project_key}/status",
            get(api_project_status),
        )
        .route("/api/v1/projects/{project_key}/refs", get(api_project_refs))
        .route(
            "/api/v1/projects/{project_key}/reflog",
            get(api_project_reflog),
        )
        .route(
            "/api/v1/projects/{project_key}/creator-sessions",
            get(api_creator_sessions)
                .post(api_begin_creator_session)
                .layer(DefaultBodyLimit::max(MAX_CREATOR_MULTIPART_WIRE_BYTES)),
        )
        .route(
            "/api/v1/projects/{project_key}/creator-sessions/{session}",
            get(api_creator_session),
        )
        .route(
            "/api/v1/projects/{project_key}/creator-sessions/{session}/images/{role}",
            get(api_creator_image),
        )
        .route(
            "/api/v1/projects/{project_key}/creator-sessions/{session}/decisions",
            axum::routing::post(api_decide_creator_session),
        )
        .fallback(not_found)
        .method_not_allowed_fallback(method_not_allowed)
        .with_state(state)
        .layer(middleware::from_fn_with_state(
            security,
            enforce_local_request,
        ));

    LocalHttpApplication { router, origin }
}

fn random_hex(byte_count: usize) -> Result<String, StartupError> {
    let mut bytes = vec![0_u8; byte_count];
    getrandom::fill(&mut bytes)
        .map_err(|_| StartupError::new("operating-system randomness is unavailable"))?;
    let mut output = String::with_capacity(byte_count * 2);
    for byte in bytes {
        use fmt::Write as _;
        write!(&mut output, "{byte:02x}")
            .expect("writing a hexadecimal byte to String cannot fail");
    }
    Ok(output)
}

async fn api_health(State(state): State<AppState>) -> Json<HealthResponse> {
    Json(HealthResponse::new(state.security.server_instance()))
}

async fn api_projects(State(state): State<AppState>) -> Json<synapse_local_service::ProjectList> {
    Json(state.service.list_projects())
}

async fn api_project_status(
    State(state): State<AppState>,
    Path(project_key): Path<String>,
) -> Response {
    let gate_key = project_key.clone();
    api_blocking(state.clone(), gate_key, move |service| {
        service.project_status(&project_key)
    })
    .await
}

async fn api_project_refs(
    State(state): State<AppState>,
    Path(project_key): Path<String>,
) -> Response {
    let gate_key = project_key.clone();
    api_blocking(state.clone(), gate_key, move |service| {
        service.list_refs(&project_key)
    })
    .await
}

async fn api_project_reflog(
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

async fn api_creator_sessions(
    State(state): State<AppState>,
    Path(project_key): Path<String>,
) -> Response {
    let gate_key = project_key.clone();
    api_blocking(state.clone(), gate_key, move |service| {
        service.list_creator_sessions(&project_key)
    })
    .await
}

async fn api_begin_creator_session(
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

async fn api_decide_creator_session(
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
        Ok(complete) => Json(complete).into_response(),
        Err(BlockingError::Service(error)) => failure_response(HttpFailure::service(&state, error)),
        Err(BlockingError::Task) => failure_response(HttpFailure::internal(
            &state,
            "The creator decision task failed.",
        )),
    }
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

fn has_exact_part_content_type(headers: &HeaderMap, expected: &mime::Mime) -> bool {
    single_content_type(headers)
        .and_then(|value| value.parse::<mime::Mime>().ok())
        .is_some_and(|media_type| media_type == *expected)
}

struct StagingDirectory {
    path: PathBuf,
}

impl StagingDirectory {
    async fn create() -> io::Result<Self> {
        Self::create_with(Self::create_sync).await
    }

    async fn create_with<F>(operation: F) -> io::Result<Self>
    where
        F: FnOnce() -> io::Result<Self> + Send + 'static,
    {
        // Dropping this JoinHandle detaches the short task, but its returned
        // StagingDirectory is still dropped when the task completes. Thus a
        // cancellation cannot separate successful creation from its owner.
        tokio::task::spawn_blocking(operation)
            .await
            .map_err(io::Error::other)?
    }

    fn create_sync() -> io::Result<Self> {
        for _ in 0..8 {
            let suffix = random_hex(16).map_err(io::Error::other)?;
            let path = std::env::temp_dir().join(format!("synapse-local-upload-{suffix}"));
            let mut builder = std::fs::DirBuilder::new();
            #[cfg(unix)]
            std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
            match builder.create(&path) {
                Ok(()) => {
                    let directory = Self { path };
                    #[cfg(unix)]
                    std::fs::set_permissions(
                        &directory.path,
                        <std::fs::Permissions as std::os::unix::fs::PermissionsExt>::from_mode(
                            0o700,
                        ),
                    )?;
                    return Ok(directory);
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error),
            }
        }
        Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "could not allocate a unique upload directory",
        ))
    }

    fn file(&self, name: &str) -> PathBuf {
        self.path.join(name)
    }
}

impl Drop for StagingDirectory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

struct StagedCreatorUpload {
    _directory: StagingDirectory,
    request: BeginCreatorSessionRequest,
}

impl StagedCreatorUpload {
    async fn from_multipart(mut multipart: Multipart) -> Result<Self, UploadFailure> {
        let directory = StagingDirectory::create()
            .await
            .map_err(|_| UploadFailure::Storage)?;
        let mut session = None;
        let mut subject_label = None;
        let mut creator_name = None;
        let mut original_image = None;
        let mut current_image = None;
        let mut ai_output = None;
        let mut aggregate_file_bytes = 0_usize;
        let mut part_count = 0_usize;

        while let Some(field) = multipart
            .next_field()
            .await
            .map_err(UploadFailure::multipart)?
        {
            part_count += 1;
            if part_count > 6 {
                return Err(UploadFailure::Request(
                    "The multipart request must contain exactly six fields.",
                ));
            }
            let name = field.name().ok_or(UploadFailure::Request(
                "Every multipart field must have a name.",
            ))?;
            match name {
                "session" if session.is_none() => {
                    session = Some(read_text_part(field, MAX_SESSION_BYTES).await?);
                }
                "subject_label" if subject_label.is_none() => {
                    subject_label = Some(read_text_part(field, MAX_SUBJECT_LABEL_BYTES).await?);
                }
                "creator_name" if creator_name.is_none() => {
                    creator_name = Some(read_text_part(field, MAX_CREATOR_NAME_BYTES).await?);
                }
                "original_image" if original_image.is_none() => {
                    let path = directory.file("original-image.bin");
                    write_file_part(field, &path, &mut aggregate_file_bytes).await?;
                    original_image = Some(path);
                }
                "current_image" if current_image.is_none() => {
                    let path = directory.file("current-image.bin");
                    write_file_part(field, &path, &mut aggregate_file_bytes).await?;
                    current_image = Some(path);
                }
                "ai_output" if ai_output.is_none() => {
                    let path = directory.file("ai-output.bin");
                    write_file_part(field, &path, &mut aggregate_file_bytes).await?;
                    ai_output = Some(path);
                }
                "session" | "subject_label" | "creator_name" | "original_image"
                | "current_image" | "ai_output" => {
                    return Err(UploadFailure::Request(
                        "Multipart fields must not be duplicated.",
                    ));
                }
                _ => {
                    return Err(UploadFailure::Request(
                        "The multipart request contains an unknown field.",
                    ));
                }
            }
        }

        let session = session.ok_or(UploadFailure::Request(
            "The multipart request is missing a required field.",
        ))?;
        let subject_label = subject_label.ok_or(UploadFailure::Request(
            "The multipart request is missing a required field.",
        ))?;
        let creator_name = creator_name.ok_or(UploadFailure::Request(
            "The multipart request is missing a required field.",
        ))?;
        let original_image = original_image.ok_or(UploadFailure::Request(
            "The multipart request is missing a required field.",
        ))?;
        let current_image = current_image.ok_or(UploadFailure::Request(
            "The multipart request is missing a required field.",
        ))?;
        let ai_output = ai_output.ok_or(UploadFailure::Request(
            "The multipart request is missing a required field.",
        ))?;

        if !is_session_slug(&session) || subject_label.is_empty() || creator_name.is_empty() {
            return Err(UploadFailure::Request(
                "The multipart text fields do not satisfy the API schema.",
            ));
        }

        Ok(Self {
            _directory: directory,
            request: BeginCreatorSessionRequest {
                session,
                subject_label,
                creator_name,
                original_image,
                current_image,
                ai_output,
            },
        })
    }
}

async fn read_text_part(
    mut field: axum::extract::multipart::Field<'_>,
    max_bytes: usize,
) -> Result<String, UploadFailure> {
    if !has_exact_part_content_type(field.headers(), &mime::TEXT_PLAIN_UTF_8) {
        return Err(UploadFailure::Request(
            "Multipart text fields must use text/plain; charset=utf-8.",
        ));
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = field.chunk().await.map_err(UploadFailure::multipart)? {
        let next_len = bytes
            .len()
            .checked_add(chunk.len())
            .ok_or(UploadFailure::Request(
                "A multipart text field exceeds its UTF-8 byte limit.",
            ))?;
        if next_len > max_bytes {
            return Err(UploadFailure::Request(
                "A multipart text field exceeds its UTF-8 byte limit.",
            ));
        }
        bytes.extend_from_slice(&chunk);
    }
    String::from_utf8(bytes)
        .map_err(|_| UploadFailure::Request("Multipart text fields must contain valid UTF-8."))
}

async fn write_file_part(
    mut field: axum::extract::multipart::Field<'_>,
    path: &PathBuf,
    aggregate_file_bytes: &mut usize,
) -> Result<(), UploadFailure> {
    if !has_exact_part_content_type(field.headers(), &mime::APPLICATION_OCTET_STREAM) {
        return Err(UploadFailure::Request(
            "Multipart file fields must use application/octet-stream.",
        ));
    }
    let mut options = tokio::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options
        .open(path)
        .await
        .map_err(|_| UploadFailure::Storage)?;
    #[cfg(unix)]
    tokio::fs::set_permissions(
        path,
        <std::fs::Permissions as std::os::unix::fs::PermissionsExt>::from_mode(0o600),
    )
    .await
    .map_err(|_| UploadFailure::Storage)?;

    let mut file_bytes = 0_usize;
    while let Some(chunk) = field.chunk().await.map_err(UploadFailure::multipart)? {
        file_bytes = file_bytes
            .checked_add(chunk.len())
            .ok_or(UploadFailure::Limit(
                "A creator file exceeds the 64 MiB limit.",
            ))?;
        let next_aggregate =
            aggregate_file_bytes
                .checked_add(chunk.len())
                .ok_or(UploadFailure::Limit(
                    "The creator files exceed the 192 MiB aggregate limit.",
                ))?;
        if file_bytes > MAX_CREATOR_FILE_BYTES {
            return Err(UploadFailure::Limit(
                "A creator file exceeds the 64 MiB limit.",
            ));
        }
        if next_aggregate > MAX_CREATOR_FILE_AGGREGATE_BYTES {
            return Err(UploadFailure::Limit(
                "The creator files exceed the 192 MiB aggregate limit.",
            ));
        }
        file.write_all(&chunk)
            .await
            .map_err(|_| UploadFailure::Storage)?;
        *aggregate_file_bytes = next_aggregate;
    }
    file.flush().await.map_err(|_| UploadFailure::Storage)
}

fn is_session_slug(value: &str) -> bool {
    let bytes = value.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= MAX_SESSION_BYTES
        && bytes[0].is_ascii_lowercase()
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'-')
}

enum UploadFailure {
    Request(&'static str),
    Limit(&'static str),
    Storage,
}

impl UploadFailure {
    fn multipart(error: axum::extract::multipart::MultipartError) -> Self {
        match error.status() {
            StatusCode::PAYLOAD_TOO_LARGE => {
                Self::Limit("The multipart request exceeds the configured wire or file limit.")
            }
            StatusCode::BAD_REQUEST => Self::Request("The multipart request is malformed."),
            _ => Self::Storage,
        }
    }

    fn into_http_failure(self, state: &AppState) -> HttpFailure {
        match self {
            Self::Request(detail) => HttpFailure::request(state, "local_request_denied", detail),
            Self::Limit(detail) => HttpFailure::limit(state, detail),
            Self::Storage => {
                HttpFailure::storage(state, "The upload staging area could not be written.")
            }
        }
    }
}

async fn api_creator_session(
    State(state): State<AppState>,
    Path((project_key, session)): Path<(String, String)>,
) -> Response {
    let gate_key = project_key.clone();
    api_blocking(state.clone(), gate_key, move |service| {
        service.get_creator_session(&project_key, &session)
    })
    .await
}

async fn api_creator_image(
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
    let permit = state.blocking.acquire(project_key.as_deref()).await?;
    let service = state.service;
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

#[derive(Debug)]
enum BlockingError {
    Service(ServiceError),
    Task,
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

async fn index_page(State(state): State<AppState>) -> Response {
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
    render_template(
        &state,
        IndexTemplate {
            page_title: "プロジェクト",
            token: state.security.token(),
            projects: &cards,
        },
    )
}

async fn project_page(State(state): State<AppState>, Path(project_key): Path<String>) -> Response {
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

async fn session_page(
    State(state): State<AppState>,
    Path((project_key, session)): Path<(String, String)>,
) -> Response {
    let project_key_for_read = project_key.clone();
    let session_for_read = session.clone();
    let gate_key = project_key.clone();
    let (project_label, detail) =
        match run_blocking(state.clone(), Some(gate_key), move |service| {
            let project = service.project_status(&project_key_for_read)?.project;
            let detail = service.get_creator_session(&project.project_key, &session_for_read)?;
            Ok((project.display_label, detail))
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

    let view = SessionPageView::new(&project_key, &project_label, &session, detail);
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
        },
    )
}

async fn css_asset() -> Response {
    ([(CONTENT_TYPE, "text/css; charset=utf-8")], APP_CSS).into_response()
}

async fn js_asset() -> Response {
    ([(CONTENT_TYPE, "text/javascript; charset=utf-8")], APP_JS).into_response()
}

async fn not_found(State(state): State<AppState>, OriginalUri(uri): OriginalUri) -> Response {
    let failure = HttpFailure::not_found(&state, "The requested local resource was not found.");
    if uri.path().starts_with("/api/v1") {
        failure_response(failure)
    } else {
        page_failure(&state, failure)
    }
}

async fn method_not_allowed(
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

#[derive(Clone)]
struct HttpFailure {
    status: StatusCode,
    code: String,
    title: String,
    detail: String,
    request_id: String,
    retryable: bool,
}

impl HttpFailure {
    fn service(state: &AppState, error: ServiceError) -> Self {
        let status = match error.code() {
            "project_not_found" | "creator_session_not_found" | "creator_review_state_lost" => {
                StatusCode::NOT_FOUND
            }
            "local_request_denied" | "usage_error" | "path_segment_invalid" => {
                StatusCode::BAD_REQUEST
            }
            "resource_limit" => StatusCode::PAYLOAD_TOO_LARGE,
            "creator_session_exists"
            | "creator_session_incomplete"
            | "creator_review_busy"
            | "creator_outcome_unknown"
            | "ref_conflict"
            | "stale_base" => StatusCode::CONFLICT,
            "creator_report_invalid" | "fsck_failed" => StatusCode::UNPROCESSABLE_ENTITY,
            "service_unavailable" | "storage_error" => StatusCode::SERVICE_UNAVAILABLE,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        };
        Self {
            status,
            code: error.code().to_owned(),
            title: status
                .canonical_reason()
                .unwrap_or("Local application error")
                .to_owned(),
            detail: error.detail().to_owned(),
            request_id: state.security.request_id(),
            retryable: error.retryable(),
        }
    }

    fn request(state: &AppState, code: &str, detail: &str) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            code: code.into(),
            title: "Invalid local request".into(),
            detail: detail.into(),
            request_id: state.security.request_id(),
            retryable: false,
        }
    }

    fn not_found(state: &AppState, detail: &str) -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            code: "local_request_denied".into(),
            title: "Not found".into(),
            detail: detail.into(),
            request_id: state.security.request_id(),
            retryable: false,
        }
    }

    fn limit(state: &AppState, detail: &str) -> Self {
        Self {
            status: StatusCode::PAYLOAD_TOO_LARGE,
            code: "resource_limit".into(),
            title: "Payload too large".into(),
            detail: detail.into(),
            request_id: state.security.request_id(),
            retryable: false,
        }
    }

    fn storage(state: &AppState, detail: &str) -> Self {
        Self {
            status: StatusCode::SERVICE_UNAVAILABLE,
            code: "storage_error".into(),
            title: "Storage unavailable".into(),
            detail: detail.into(),
            request_id: state.security.request_id(),
            retryable: true,
        }
    }

    fn internal(state: &AppState, detail: &str) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            code: "storage_error".into(),
            title: "Local application error".into(),
            detail: detail.into(),
            request_id: state.security.request_id(),
            retryable: true,
        }
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

fn project_state_label(state: ProjectState) -> &'static str {
    match state {
        ProjectState::Ready => "利用可能",
        ProjectState::EmptyRestoreTarget => "空の復元先",
        ProjectState::Unavailable => "利用不可",
    }
}

fn project_state_tone(state: ProjectState) -> &'static str {
    match state {
        ProjectState::Ready => "success",
        ProjectState::EmptyRestoreTarget => "info",
        ProjectState::Unavailable => "danger",
    }
}

fn session_state_label(state: CreatorSessionState) -> &'static str {
    match state {
        CreatorSessionState::Complete => "完了",
        CreatorSessionState::PendingReview => "レビュー待ち",
        CreatorSessionState::Incomplete => "未完了",
    }
}

fn session_state_tone(state: CreatorSessionState) -> &'static str {
    match state {
        CreatorSessionState::Complete => "success",
        CreatorSessionState::PendingReview => "info",
        CreatorSessionState::Incomplete => "warning",
    }
}

struct SessionPageView {
    complete: bool,
    pending: bool,
    show_evidence: bool,
    state_label: String,
    state_tone: String,
    state_description: String,
    ai_output_source: String,
    review_id: String,
    decision_url: String,
    disposition: String,
    selected: String,
    fsck_objects: usize,
    images: Vec<ImageView>,
    has_comparison: bool,
    comparison_outcome: String,
    comparison_warning: String,
    comparison_status: String,
    comparison_comparability: String,
    comparison_adapter: String,
    comparison_replay: String,
    timeline: Vec<TimelineView>,
    diagnostic: String,
}

impl SessionPageView {
    fn new(
        project_key: &str,
        _project_label: &str,
        session: &str,
        detail: CreatorSessionDetail,
    ) -> Self {
        match detail {
            CreatorSessionDetail::Complete(detail) => {
                Self::complete(project_key, session, detail.report)
            }
            CreatorSessionDetail::PendingReview(detail) => {
                let detail = *detail;
                let images = Self::images(
                    project_key,
                    session,
                    &detail.original_blob_oid,
                    &detail.current_blob_oid,
                    &detail.ai_output_blob_oid,
                );
                let comparison = detail.comparison;
                Self {
                    complete: false,
                    pending: true,
                    show_evidence: true,
                    state_label: "レビュー待ち".into(),
                    state_tone: "info".into(),
                    state_description: "このprocess内でHuman reviewを待っています。".into(),
                    ai_output_source: detail.ai_output_source,
                    review_id: detail.review_id,
                    decision_url: format!(
                        "/api/v1/projects/{project_key}/creator-sessions/{session}/decisions"
                    ),
                    disposition: "—".into(),
                    selected: "—".into(),
                    fsck_objects: 0,
                    images,
                    has_comparison: true,
                    comparison_outcome: comparison.outcome,
                    comparison_warning: comparison.warnings.join(" "),
                    comparison_status: comparison.status,
                    comparison_comparability: comparison.comparability,
                    comparison_adapter: format!(
                        "{} {}",
                        comparison.adapter_id, comparison.adapter_version
                    ),
                    comparison_replay: if comparison.replay_ready {
                        "はい"
                    } else {
                        "いいえ"
                    }
                    .into(),
                    timeline: Vec::new(),
                    diagnostic: String::new(),
                }
            }
            CreatorSessionDetail::Incomplete(incomplete) => Self {
                complete: false,
                pending: false,
                show_evidence: false,
                state_label: "未完了".into(),
                state_tone: "warning".into(),
                state_description: "現在のRefsは完了したCreator sessionを構成していません。".into(),
                ai_output_source: String::new(),
                review_id: String::new(),
                decision_url: String::new(),
                disposition: "—".into(),
                selected: "—".into(),
                fsck_objects: 0,
                images: Vec::new(),
                has_comparison: false,
                comparison_outcome: String::new(),
                comparison_warning: String::new(),
                comparison_status: String::new(),
                comparison_comparability: String::new(),
                comparison_adapter: String::new(),
                comparison_replay: String::new(),
                timeline: Vec::new(),
                diagnostic: incomplete.diagnostic,
            },
        }
    }

    fn complete(project_key: &str, session: &str, report: CreatorReport) -> Self {
        let images = Self::images(
            project_key,
            session,
            &report.original_blob_oid,
            &report.current_blob_oid,
            &report.ai_output_blob_oid,
        );
        let timeline = report
            .timeline
            .into_iter()
            .map(|entry| TimelineView {
                oid: entry.oid,
                stage: entry.stage,
                kind: entry.kind,
                ordering_time: entry.ordering_time,
                time_basis: entry.time_basis,
            })
            .collect();
        let (
            has_comparison,
            comparison_outcome,
            comparison_warning,
            comparison_status,
            comparison_comparability,
            comparison_adapter,
            comparison_replay,
        ) = if let Some(comparison) = report.comparison {
            (
                true,
                comparison.outcome,
                comparison.warnings.join(" "),
                comparison.status,
                comparison.comparability,
                format!("{} {}", comparison.adapter_id, comparison.adapter_version),
                if comparison.replay_ready {
                    "はい"
                } else {
                    "いいえ"
                }
                .into(),
            )
        } else {
            (
                false,
                String::new(),
                String::new(),
                String::new(),
                String::new(),
                String::new(),
                String::new(),
            )
        };
        Self {
            complete: true,
            pending: false,
            show_evidence: true,
            state_label: "完了".into(),
            state_tone: "success".into(),
            state_description: "現在のRefsとCASから検証済みレポートを再構築しました。".into(),
            ai_output_source: report.ai_output_source,
            review_id: String::new(),
            decision_url: String::new(),
            disposition: report.disposition,
            selected: if report.selected_ai_output {
                "はい".into()
            } else {
                "いいえ".into()
            },
            fsck_objects: report.fsck_objects,
            images,
            has_comparison,
            comparison_outcome,
            comparison_warning,
            comparison_status,
            comparison_comparability,
            comparison_adapter,
            comparison_replay,
            timeline,
            diagnostic: String::new(),
        }
    }

    fn images(
        project_key: &str,
        session: &str,
        original_oid: &str,
        current_oid: &str,
        ai_output_oid: &str,
    ) -> Vec<ImageView> {
        let image_base =
            format!("/api/v1/projects/{project_key}/creator-sessions/{session}/images");
        vec![
            ImageView {
                label: "Original".into(),
                alt: "取り込まれたoriginal画像".into(),
                url: format!("{image_base}/original"),
                oid: original_oid.into(),
                download_name: format!("{session}-original.bin"),
            },
            ImageView {
                label: "Current".into(),
                alt: "取り込まれたcurrent画像".into(),
                url: format!("{image_base}/current"),
                oid: current_oid.into(),
                download_name: format!("{session}-current.bin"),
            },
            ImageView {
                label: "AI output".into(),
                alt: "caller supplied AI output".into(),
                url: format!("{image_base}/ai-output"),
                oid: ai_output_oid.into(),
                download_name: format!("{session}-ai-output.bin"),
            },
        ]
    }
}

struct ProjectCardView {
    key: String,
    label: String,
    state_label: &'static str,
    tone: &'static str,
    ref_count: usize,
    complete_sessions: usize,
    pending_sessions: usize,
    incomplete_sessions: usize,
}

struct RefView {
    name: String,
    head: String,
    event_id: String,
}

struct ReflogView {
    event_id: String,
    ref_name: String,
    new_head: String,
    message: String,
}

struct SessionSummaryView {
    session: String,
    state_label: &'static str,
    tone: &'static str,
    proposal_head: String,
    decision_head: String,
}

struct ImageView {
    label: String,
    alt: String,
    url: String,
    oid: String,
    download_name: String,
}

struct TimelineView {
    oid: String,
    stage: String,
    kind: String,
    ordering_time: String,
    time_basis: String,
}

#[derive(Template)]
#[template(path = "index.html")]
struct IndexTemplate<'a> {
    page_title: &'a str,
    token: &'a str,
    projects: &'a [ProjectCardView],
}

#[derive(Template)]
#[template(path = "project.html")]
struct ProjectTemplate<'a> {
    page_title: &'a str,
    token: &'a str,
    project_key: &'a str,
    project_label: &'a str,
    watermark: &'a str,
    complete_sessions: usize,
    pending_sessions: usize,
    incomplete_sessions: usize,
    refs: &'a [RefView],
    reflog: &'a [ReflogView],
    sessions: &'a [SessionSummaryView],
}

#[derive(Template)]
#[template(path = "session.html")]
struct SessionTemplate<'a> {
    page_title: &'a str,
    token: &'a str,
    project_key: &'a str,
    project_label: &'a str,
    session: &'a str,
    complete: bool,
    pending: bool,
    show_evidence: bool,
    state_label: &'a str,
    state_tone: &'a str,
    state_description: &'a str,
    ai_output_source: &'a str,
    review_id: &'a str,
    decision_url: &'a str,
    disposition: &'a str,
    selected: &'a str,
    fsck_objects: usize,
    images: &'a [ImageView],
    has_comparison: bool,
    comparison_outcome: &'a str,
    comparison_warning: &'a str,
    comparison_status: &'a str,
    comparison_comparability: &'a str,
    comparison_adapter: &'a str,
    comparison_replay: &'a str,
    timeline: &'a [TimelineView],
    diagnostic: &'a str,
}

#[derive(Template)]
#[template(path = "error.html")]
struct ErrorTemplate<'a> {
    page_title: &'a str,
    token: &'a str,
    status: &'a str,
    title: &'a str,
    detail: &'a str,
    request_id: &'a str,
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;
    use axum::http::header::{HOST, ORIGIN};
    use axum::http::{Request, header};
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};
    use synapse_creator::{CreatorDisposition, CreatorRunOptions, run_creator_session};
    use tower::ServiceExt;

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

    fn request(path: &str) -> axum::http::request::Builder {
        Request::builder().uri(path).header(HOST, "127.0.0.1:43123")
    }

    fn multipart_body(boundary: &str, parts: &[(&str, &str, &[u8])]) -> Vec<u8> {
        let mut body = Vec::new();
        for (name, content_type, bytes) in parts {
            body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
            body.extend_from_slice(
                format!(
                    "Content-Disposition: form-data; name=\"{name}\"; filename=\"{name}.bin\"\r\n"
                )
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
}
