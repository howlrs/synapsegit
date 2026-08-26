use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::middleware;
use axum::routing::get;
use std::error::Error;
use std::fmt;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use synapse_local_service::LocalService;
use tokio::sync::Semaphore;

use crate::handlers::{
    api_archives, api_begin_creator_session, api_creator_image, api_creator_session,
    api_creator_session_diagnostics, api_creator_sessions, api_decide_creator_session, api_health,
    api_operation, api_project_reflog, api_project_refs, api_project_status, api_projects,
    api_start_archive_export, api_start_fsck, index_page, method_not_allowed, not_found,
    project_page, session_page,
};
use crate::security::{SecurityPolicy, enforce_local_request};
use crate::staging::MAX_CREATOR_FILE_AGGREGATE_BYTES;
use crate::state::{AppState, BlockingGates, OperationRegistry};
use crate::templates::{css_asset, js_asset};

pub(crate) const MAX_CONCURRENT_CREATOR_UPLOADS: usize = 2;
// The file aggregate is the contractual payload. This small fixed allowance
// bounds the multipart framing, the three short text fields, and part headers.
const MAX_CREATOR_MULTIPART_WIRE_BYTES: usize = MAX_CREATOR_FILE_AGGREGATE_BYTES + 1024 * 1024;

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

pub(crate) fn build_with_identity(
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
        operations: OperationRegistry::default(),
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
            "/api/v1/projects/{project_key}/creator-sessions/{session}/diagnostics",
            get(api_creator_session_diagnostics),
        )
        .route(
            "/api/v1/projects/{project_key}/creator-sessions/{session}/images/{role}",
            get(api_creator_image),
        )
        .route(
            "/api/v1/projects/{project_key}/creator-sessions/{session}/decisions",
            axum::routing::post(api_decide_creator_session),
        )
        .route(
            "/api/v1/projects/{project_key}/operations/fsck",
            axum::routing::post(api_start_fsck),
        )
        .route(
            "/api/v1/projects/{project_key}/archive-exports",
            axum::routing::post(api_start_archive_export),
        )
        .route("/api/v1/operations/{operation_id}", get(api_operation))
        .route("/api/v1/archives", get(api_archives))
        .fallback(not_found)
        .method_not_allowed_fallback(method_not_allowed)
        .with_state(state)
        .layer(middleware::from_fn_with_state(
            security,
            enforce_local_request,
        ));

    LocalHttpApplication { router, origin }
}

pub(crate) fn random_hex(byte_count: usize) -> Result<String, StartupError> {
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

pub(crate) fn monotonic_operation_timestamp(
    last_timestamp: &mut Option<Duration>,
    observed_at: SystemTime,
) -> Option<String> {
    let observed = observed_at
        .duration_since(UNIX_EPOCH)
        .ok()
        .filter(|duration| format_operation_timestamp(*duration).is_some());
    let logical = match (*last_timestamp, observed) {
        (Some(previous), Some(observed)) => previous.max(observed),
        (Some(previous), None) => previous,
        (None, Some(observed)) => observed,
        (None, None) => return None,
    };
    let timestamp = format_operation_timestamp(logical)?;
    *last_timestamp = Some(logical);
    Some(timestamp)
}

fn format_operation_timestamp(duration: Duration) -> Option<String> {
    let unix_seconds = i64::try_from(duration.as_secs()).ok()?;
    let days = unix_seconds.div_euclid(86_400);
    let second_of_day = unix_seconds.rem_euclid(86_400);
    let (year, month, day) = civil_date_from_unix_days(days);
    if !(0..=9999).contains(&year) {
        return None;
    }
    let hour = second_of_day / 3_600;
    let minute = (second_of_day % 3_600) / 60;
    let second = second_of_day % 60;
    Some(format!(
        "{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{:09}Z",
        duration.subsec_nanos()
    ))
}

// Howard Hinnant's civil-from-days transform, with day zero at 1970-01-01.
pub(crate) fn civil_date_from_unix_days(days: i64) -> (i64, i64, i64) {
    let shifted = days + 719_468;
    let era = if shifted >= 0 {
        shifted
    } else {
        shifted - 146_096
    } / 146_097;
    let day_of_era = shifted - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    if month <= 2 {
        year += 1;
    }
    (year, month, day)
}
