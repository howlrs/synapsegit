use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use synapse_core::Repository;

use crate::MAX_PROJECTS;

/// One trusted startup-owned project registration.
///
/// The path is configuration input only. It is canonicalized at startup and
/// is never copied into a response DTO.
#[derive(Clone, Debug)]
pub struct ProjectRegistration {
    pub project_key: String,
    pub display_label: String,
    pub repository_path: PathBuf,
}

impl ProjectRegistration {
    pub fn new(
        project_key: impl Into<String>,
        display_label: impl Into<String>,
        repository_path: impl Into<PathBuf>,
    ) -> Self {
        Self {
            project_key: project_key.into(),
            display_label: display_label.into(),
            repository_path: repository_path.into(),
        }
    }
}

/// Safe startup catalog validation failure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CatalogError {
    code: &'static str,
    detail: String,
    diagnostic: Option<String>,
}

impl CatalogError {
    fn new(code: &'static str, detail: impl Into<String>) -> Self {
        Self {
            code,
            detail: detail.into(),
            diagnostic: None,
        }
    }

    fn with_diagnostic(mut self, diagnostic: impl Into<String>) -> Self {
        self.diagnostic = Some(diagnostic.into());
        self
    }

    pub const fn code(&self) -> &'static str {
        self.code
    }

    pub fn detail(&self) -> &str {
        &self.detail
    }

    /// Local-only diagnostic context. Transports must never copy this value
    /// into a response body.
    pub fn diagnostic(&self) -> Option<&str> {
        self.diagnostic.as_deref()
    }
}

impl fmt::Display for CatalogError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.code, self.detail)
    }
}

impl Error for CatalogError {}

#[derive(Clone)]
pub(crate) struct CatalogEntry {
    pub project_key: String,
    pub display_label: String,
    repository_path: PathBuf,
}

impl CatalogEntry {
    pub fn repository_path(&self) -> &Path {
        &self.repository_path
    }

    pub fn open_repository(&self) -> Result<Repository, CatalogError> {
        Repository::open(&self.repository_path).map_err(|error| {
            CatalogError::new(
                "storage_error",
                format!("project {:?} is unavailable", self.project_key),
            )
            .with_diagnostic(error.to_string())
        })
    }
}

pub(crate) struct ProjectCatalog {
    entries: BTreeMap<String, CatalogEntry>,
}

impl ProjectCatalog {
    pub fn build(
        registrations: impl IntoIterator<Item = ProjectRegistration>,
    ) -> Result<Self, CatalogError> {
        let registrations = registrations.into_iter().collect::<Vec<_>>();
        if registrations.is_empty() {
            return Err(CatalogError::new(
                "local_request_denied",
                "at least one project registration is required",
            ));
        }
        if registrations.len() > MAX_PROJECTS {
            return Err(CatalogError::new(
                "resource_limit",
                format!("project catalog exceeds the {MAX_PROJECTS} entry limit"),
            ));
        }

        let mut entries = BTreeMap::new();
        let mut paths = BTreeSet::new();
        for registration in registrations {
            if !is_slug(&registration.project_key) {
                return Err(CatalogError::new(
                    "local_request_denied",
                    "project key must match [a-z][a-z0-9-]{0,63}",
                ));
            }
            let label_length = registration.display_label.chars().count();
            if !(1..=300).contains(&label_length) {
                return Err(CatalogError::new(
                    "local_request_denied",
                    "project display label must contain 1 to 300 Unicode code points",
                ));
            }
            if entries.contains_key(&registration.project_key) {
                return Err(CatalogError::new(
                    "local_request_denied",
                    format!("duplicate project key {:?}", registration.project_key),
                ));
            }
            let canonical_path = canonical_directory(&registration)?;
            if !paths.insert(canonical_path.clone()) {
                return Err(CatalogError::new(
                    "local_request_denied",
                    "duplicate canonical repository path",
                ));
            }

            // Open once at startup so an invalid repository layout or an
            // unusable Ref database fails before a listener can be started.
            Repository::open(&canonical_path).map_err(|error| {
                CatalogError::new(
                    "storage_error",
                    format!(
                        "repository for project {:?} could not be opened",
                        registration.project_key
                    ),
                )
                .with_diagnostic(error.to_string())
            })?;

            entries.insert(
                registration.project_key.clone(),
                CatalogEntry {
                    project_key: registration.project_key,
                    display_label: registration.display_label,
                    repository_path: canonical_path,
                },
            );
        }
        Ok(Self { entries })
    }

    pub fn get(&self, project_key: &str) -> Option<&CatalogEntry> {
        self.entries.get(project_key)
    }

    pub fn values(&self) -> impl Iterator<Item = &CatalogEntry> {
        self.entries.values()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }
}

fn canonical_directory(registration: &ProjectRegistration) -> Result<PathBuf, CatalogError> {
    let metadata = fs::metadata(&registration.repository_path).map_err(|error| {
        CatalogError::new(
            "storage_error",
            format!(
                "repository for project {:?} is unavailable",
                registration.project_key
            ),
        )
        .with_diagnostic(error.to_string())
    })?;
    if !metadata.is_dir() {
        return Err(CatalogError::new(
            "local_request_denied",
            format!(
                "repository for project {:?} is not a directory",
                registration.project_key
            ),
        ));
    }
    fs::canonicalize(&registration.repository_path).map_err(|error| {
        CatalogError::new(
            "storage_error",
            format!(
                "repository for project {:?} could not be canonicalized",
                registration.project_key
            ),
        )
        .with_diagnostic(error.to_string())
    })
}

pub(crate) fn is_slug(value: &str) -> bool {
    let bytes = value.as_bytes();
    (1..=64).contains(&bytes.len())
        && bytes[0].is_ascii_lowercase()
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'-')
}
