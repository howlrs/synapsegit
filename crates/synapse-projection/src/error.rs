use std::error::Error;
use std::fmt;
use synapse_cas::StoreError;

#[derive(Debug)]
pub enum ProjectionError {
    Storage(rusqlite::Error),
    ObjectStore(StoreError),
    InvalidSnapshot(String),
    InvalidSource(String),
    ResourceLimit(String),
    UnsupportedSchemaVersion { found: String },
    CorruptProjection(String),
    UnknownRef(String),
    ObservationNotIndexed(String),
    AnalysisNotIndexed(String),
    AnalysisNotReachable(String),
}

impl ProjectionError {
    pub const fn code(&self) -> &'static str {
        match self {
            Self::Storage(_) | Self::ObjectStore(_) | Self::CorruptProjection(_) => "storage_error",
            Self::InvalidSnapshot(_) | Self::InvalidSource(_) => "projection_source_invalid",
            Self::ResourceLimit(_) => "resource_limit",
            Self::UnsupportedSchemaVersion { .. } => "projection_schema_unsupported",
            Self::UnknownRef(_) => "projection_ref_unknown",
            Self::ObservationNotIndexed(_) => "projection_observation_unknown",
            Self::AnalysisNotIndexed(_) => "projection_analysis_unknown",
            Self::AnalysisNotReachable(_) => "projection_analysis_not_reachable",
        }
    }
}

impl fmt::Display for ProjectionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Storage(error) => write!(formatter, "projection SQLite error: {error}"),
            Self::ObjectStore(error) => write!(formatter, "projection object-store error: {error}"),
            Self::InvalidSnapshot(message) => write!(formatter, "invalid Ref snapshot: {message}"),
            Self::InvalidSource(message) => {
                write!(formatter, "invalid projection source: {message}")
            }
            Self::ResourceLimit(message) => {
                write!(formatter, "projection resource limit: {message}")
            }
            Self::UnsupportedSchemaVersion { found } => {
                write!(formatter, "unsupported projection schema version {found:?}")
            }
            Self::CorruptProjection(message) => write!(formatter, "corrupt projection: {message}"),
            Self::UnknownRef(name) => write!(formatter, "Ref {name:?} is not in the projection"),
            Self::ObservationNotIndexed(oid) => {
                write!(formatter, "Observation {oid} is not indexed")
            }
            Self::AnalysisNotIndexed(oid) => {
                write!(formatter, "AnalysisResult {oid} is not indexed")
            }
            Self::AnalysisNotReachable(oid) => write!(
                formatter,
                "AnalysisResult {oid} is not reachable from the selected Refs"
            ),
        }
    }
}

impl Error for ProjectionError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Storage(error) => Some(error),
            Self::ObjectStore(error) => Some(error),
            _ => None,
        }
    }
}

impl From<rusqlite::Error> for ProjectionError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Storage(error)
    }
}

impl From<StoreError> for ProjectionError {
    fn from(error: StoreError) -> Self {
        Self::ObjectStore(error)
    }
}

pub type Result<T> = std::result::Result<T, ProjectionError>;
