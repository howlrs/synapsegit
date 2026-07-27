use std::error::Error;
use std::fmt;
use std::io;
use std::path::PathBuf;
use synapse_application::ApplicationError;
use synapse_core::RepositoryError;
use synapse_observation::ObservationAnalysisError;
use synapse_projection::ProjectionError;
use synapse_sqlite::RefStoreError;

/// Errors from the Pilot orchestration boundary.
#[derive(Debug)]
pub enum CreatorError {
    InvalidArgument(String),
    ResourceLimit(String),
    SessionExists(String),
    SessionIncomplete(String),
    SessionNotFound(String),
    Io {
        operation: &'static str,
        path: PathBuf,
        source: io::Error,
    },
    Clock(String),
    Random(String),
    Repository(RepositoryError),
    Application(ApplicationError),
    Observation(ObservationAnalysisError),
    Projection(ProjectionError),
    Json(serde_json::Error),
    Integrity(String),
    ReportInvalid(String),
}

impl CreatorError {
    pub fn code(&self) -> &str {
        match self {
            Self::InvalidArgument(_) => "usage_error",
            Self::ResourceLimit(_) => "resource_limit",
            Self::SessionExists(_) => "creator_session_exists",
            Self::SessionIncomplete(_) => "creator_session_incomplete",
            Self::SessionNotFound(_) => "creator_session_not_found",
            Self::Io { .. } | Self::Clock(_) | Self::Random(_) => "storage_error",
            Self::Repository(error) => error.code(),
            Self::Application(error) => error.code(),
            Self::Observation(error) => error.code(),
            Self::Projection(error) => error.code(),
            Self::Json(_) | Self::ReportInvalid(_) => "creator_report_invalid",
            Self::Integrity(_) => "fsck_failed",
        }
    }

    pub(crate) fn io(operation: &'static str, path: impl Into<PathBuf>, source: io::Error) -> Self {
        Self::Io {
            operation,
            path: path.into(),
            source,
        }
    }
}

impl fmt::Display for CreatorError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidArgument(message) => formatter.write_str(message),
            Self::ResourceLimit(message) => formatter.write_str(message),
            Self::SessionExists(session) => {
                write!(formatter, "creator session {session:?} already exists")
            }
            Self::SessionIncomplete(session) => write!(
                formatter,
                "creator session {session:?} is incomplete and requires diagnosis or a new name"
            ),
            Self::SessionNotFound(session) => {
                write!(formatter, "creator session {session:?} was not found")
            }
            Self::Io {
                operation,
                path,
                source,
            } => write!(formatter, "{operation} {}: {source}", path.display()),
            Self::Clock(message) => formatter.write_str(message),
            Self::Random(message) => formatter.write_str(message),
            Self::Repository(error) => error.fmt(formatter),
            Self::Application(error) => error.fmt(formatter),
            Self::Observation(error) => error.fmt(formatter),
            Self::Projection(error) => error.fmt(formatter),
            Self::Json(error) => write!(formatter, "invalid stored creator JSON: {error}"),
            Self::Integrity(message) | Self::ReportInvalid(message) => formatter.write_str(message),
        }
    }
}

impl Error for CreatorError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Repository(error) => Some(error),
            Self::Application(error) => Some(error),
            Self::Observation(error) => Some(error),
            Self::Projection(error) => Some(error),
            Self::Json(error) => Some(error),
            _ => None,
        }
    }
}

impl From<RepositoryError> for CreatorError {
    fn from(error: RepositoryError) -> Self {
        Self::Repository(error)
    }
}

impl From<RefStoreError> for CreatorError {
    fn from(error: RefStoreError) -> Self {
        Self::Repository(error.into())
    }
}

impl From<ApplicationError> for CreatorError {
    fn from(error: ApplicationError) -> Self {
        Self::Application(error)
    }
}

impl From<ObservationAnalysisError> for CreatorError {
    fn from(error: ObservationAnalysisError) -> Self {
        Self::Observation(error)
    }
}

impl From<ProjectionError> for CreatorError {
    fn from(error: ProjectionError) -> Self {
        Self::Projection(error)
    }
}

impl From<serde_json::Error> for CreatorError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

pub type Result<T> = std::result::Result<T, CreatorError>;
