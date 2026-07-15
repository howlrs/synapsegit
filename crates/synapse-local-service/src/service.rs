use sha2::{Digest, Sha256};
use std::error::Error;
use std::fmt::{self, Write as _};
use synapse_core::Repository;
use synapse_creator::{
    CreatorComparisonReport as CoreComparisonReport, CreatorError,
    CreatorReport as CoreCreatorReport, CreatorSessionState as CoreCreatorSessionState,
    CreatorSnapshotReport, CreatorTimelineEntry as CoreTimelineEntry, creator_report_from_snapshot,
    discover_creator_sessions,
};
use synapse_sqlite::{
    MAX_REF_SNAPSHOT_ENTRIES, MAX_REFLOG_PAGE_ENTRIES, RefSnapshot, RefStoreError,
    ReflogEntry as CoreReflogEntry,
};

use crate::CatalogError;
use crate::catalog::{CatalogEntry, ProjectCatalog, ProjectRegistration, is_slug};
use crate::dto::*;

pub const MAX_PROJECTS: usize = 1_000;
pub const MAX_REFS: usize = MAX_REF_SNAPSHOT_ENTRIES;
pub const MAX_CREATOR_SESSIONS: usize = 50_000;
pub const IMAGE_RESPONSE_MAX_BYTES: u64 = 64 * 1024 * 1024;

/// A safe application-facing failure. Nested repository paths, SQL details,
/// and raw dependency errors are intentionally not retained.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ServiceError {
    code: String,
    detail: String,
    retryable: bool,
    diagnostic: Option<String>,
}

impl ServiceError {
    fn new(code: impl Into<String>, detail: impl Into<String>, retryable: bool) -> Self {
        Self {
            code: code.into(),
            detail: detail.into(),
            retryable,
            diagnostic: None,
        }
    }

    fn with_diagnostic(mut self, diagnostic: impl Into<String>) -> Self {
        self.diagnostic = Some(diagnostic.into());
        self
    }

    fn project_not_found() -> Self {
        Self::new(
            "project_not_found",
            "The requested project was not found.",
            false,
        )
    }

    fn session_not_found() -> Self {
        Self::new(
            "creator_session_not_found",
            "The requested creator session was not found.",
            false,
        )
    }

    fn session_incomplete() -> Self {
        Self::new(
            "creator_session_incomplete",
            "The creator session is incomplete and cannot serve this resource.",
            false,
        )
    }

    fn storage() -> Self {
        Self::new(
            "storage_error",
            "The local project could not be read.",
            true,
        )
    }

    pub fn code(&self) -> &str {
        &self.code
    }

    pub fn detail(&self) -> &str {
        &self.detail
    }

    pub const fn retryable(&self) -> bool {
        self.retryable
    }

    /// Local-only diagnostic context retained for process logging. It can
    /// contain repository paths or nested storage details and must never be
    /// copied into a response.
    pub fn diagnostic(&self) -> Option<&str> {
        self.diagnostic.as_deref()
    }

    pub fn to_problem(&self, status: u16, request_id: impl Into<String>) -> Problem {
        Problem {
            r#type: format!("urn:synapsegit:error:{}", self.code),
            title: problem_title(&self.code).into(),
            status,
            code: self.code.clone(),
            detail: self.detail.clone(),
            request_id: request_id.into(),
            retryable: self.retryable,
        }
    }
}

impl fmt::Display for ServiceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.code, self.detail)
    }
}

impl Error for ServiceError {}

impl From<CatalogError> for ServiceError {
    fn from(error: CatalogError) -> Self {
        let diagnostic = error.diagnostic().map(str::to_owned);
        let mut service_error = Self::new(
            error.code(),
            error.detail(),
            error.code() == "storage_error",
        );
        service_error.diagnostic = diagnostic;
        service_error
    }
}

/// Exact startup-owned read facade.
///
/// Only canonical paths are retained, and a fresh [`Repository`] is opened
/// inside each call. Therefore the non-`Sync` SQLite connection never becomes
/// part of shared server state.
pub struct LocalService {
    catalog: ProjectCatalog,
}

impl fmt::Debug for LocalService {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LocalService")
            .field("project_count", &self.catalog.len())
            .finish_non_exhaustive()
    }
}

impl LocalService {
    pub fn new(
        registrations: impl IntoIterator<Item = ProjectRegistration>,
    ) -> Result<Self, CatalogError> {
        Ok(Self {
            catalog: ProjectCatalog::build(registrations)?,
        })
    }

    pub fn health(&self, server_instance: impl Into<String>) -> HealthResponse {
        HealthResponse::new(server_instance)
    }

    pub fn list_projects(&self) -> ProjectList {
        ProjectList {
            projects: self.catalog.values().map(project_summary).collect(),
        }
    }

    pub fn project_status(&self, project_key: &str) -> Result<ProjectStatus, ServiceError> {
        let entry = self.entry(project_key)?;
        let repository = open_repository(entry)?;
        let snapshot = capture_snapshot(&repository)?;
        let sessions = discover_sessions(&repository, &snapshot)?;
        let mut counts = CreatorSessionCounts {
            complete: 0,
            pending_review: 0,
            incomplete: 0,
        };
        for session in sessions {
            match session.state {
                CreatorSessionState::Complete => counts.complete += 1,
                CreatorSessionState::PendingReview => counts.pending_review += 1,
                CreatorSessionState::Incomplete => counts.incomplete += 1,
            }
        }
        Ok(ProjectStatus {
            project: project_summary(entry),
            snapshot: snapshot_context(&snapshot, None),
            creator_session_counts: counts,
            projection_state: ProjectionState::NotBuilt,
            last_fsck: None,
        })
    }

    pub fn list_refs(&self, project_key: &str) -> Result<RefList, ServiceError> {
        let repository = self.open_repository(project_key)?;
        let snapshot = capture_snapshot(&repository)?;
        let context = snapshot_context(&snapshot, None);
        let refs = snapshot
            .refs
            .into_iter()
            .map(|reference| RefRecord {
                name: reference.name,
                head: reference.head,
                updated_event_id: reference.updated_event_id.to_string(),
            })
            .collect();
        Ok(RefList {
            snapshot: context,
            refs,
        })
    }

    pub fn list_reflog(
        &self,
        project_key: &str,
        query: ReflogQuery,
    ) -> Result<ReflogPage, ServiceError> {
        validate_reflog_query(&query)?;
        let after_event_id = query
            .after_event_id
            .as_deref()
            .map(parse_event_id)
            .transpose()?;
        let repository = self.open_repository(project_key)?;
        let page = repository
            .refs()
            .read_reflog_page(query.ref_name.as_deref(), after_event_id, query.limit)
            .map_err(ref_store_error)?;
        let context = snapshot_context(&page.snapshot, None);
        Ok(ReflogPage {
            snapshot: context,
            entries: page.entries.into_iter().map(reflog_entry).collect(),
            next_after_event_id: page.next_after_event_id.map(|value| value.to_string()),
        })
    }

    pub fn list_creator_sessions(
        &self,
        project_key: &str,
    ) -> Result<CreatorSessionList, ServiceError> {
        let repository = self.open_repository(project_key)?;
        let snapshot = capture_snapshot(&repository)?;
        let sessions = discover_sessions(&repository, &snapshot)?;
        Ok(CreatorSessionList {
            snapshot: snapshot_context(&snapshot, None),
            sessions,
        })
    }

    pub fn get_creator_session(
        &self,
        project_key: &str,
        session: &str,
    ) -> Result<CreatorSessionDetail, ServiceError> {
        if !is_slug(session) {
            return Err(ServiceError::session_not_found());
        }
        let repository = self.open_repository(project_key)?;
        let snapshot = capture_snapshot(&repository)?;
        match creator_report_from_snapshot(&repository, &snapshot, session) {
            Ok(snapshot_report) => Ok(CreatorSessionDetail::Complete(Box::new(complete_session(
                &snapshot,
                snapshot_report,
            )))),
            Err(CreatorError::SessionIncomplete(_)) => Ok(CreatorSessionDetail::Incomplete(
                Box::new(incomplete_session(&snapshot, session)),
            )),
            Err(CreatorError::SessionNotFound(_)) => Err(ServiceError::session_not_found()),
            Err(error) => Err(creator_error(error)),
        }
    }

    pub fn get_creator_session_image(
        &self,
        project_key: &str,
        session: &str,
        role: ImageRole,
    ) -> Result<CreatorImage, ServiceError> {
        if !is_slug(session) {
            return Err(ServiceError::session_not_found());
        }
        let repository = self.open_repository(project_key)?;
        let snapshot = capture_snapshot(&repository)?;
        let snapshot_report = match creator_report_from_snapshot(&repository, &snapshot, session) {
            Ok(report) => report,
            Err(CreatorError::SessionIncomplete(_)) => {
                return Err(ServiceError::session_incomplete());
            }
            Err(CreatorError::SessionNotFound(_)) => {
                return Err(ServiceError::session_not_found());
            }
            Err(error) => return Err(creator_error(error)),
        };
        let blob_oid = match role {
            ImageRole::Original => snapshot_report.report.original_blob_oid,
            ImageRole::Current => snapshot_report.report.current_blob_oid,
            ImageRole::AiOutput => snapshot_report.report.ai_output_blob_oid,
        };
        let bytes = repository
            .objects()
            .read_verified_blob_limited(&blob_oid, IMAGE_RESPONSE_MAX_BYTES)
            .map_err(|error| {
                let diagnostic = error.to_string();
                let service_error = match error.code() {
                    Some(code) if code.as_str() == "resource_limit" => ServiceError::new(
                        code.as_str(),
                        "The creator image exceeds the 64 MiB response limit.",
                        false,
                    ),
                    Some(code) => ServiceError::new(
                        code.as_str(),
                        "The creator image failed verified storage validation.",
                        false,
                    ),
                    None => ServiceError::storage(),
                };
                service_error.with_diagnostic(diagnostic)
            })?
            .ok_or_else(|| {
                ServiceError::new(
                    "creator_report_invalid",
                    "The creator image is absent from verified storage.",
                    false,
                )
            })?;
        let media_type = classify_image_media_type(&bytes);
        Ok(CreatorImage {
            blob_oid,
            media_type,
            disposition: if media_type.is_attachment() {
                ImageDisposition::Attachment
            } else {
                ImageDisposition::Inline
            },
            bytes,
        })
    }

    fn entry(&self, project_key: &str) -> Result<&CatalogEntry, ServiceError> {
        self.catalog
            .get(project_key)
            .ok_or_else(ServiceError::project_not_found)
    }

    fn open_repository(&self, project_key: &str) -> Result<Repository, ServiceError> {
        open_repository(self.entry(project_key)?)
    }
}

pub fn snapshot_watermark(snapshot: &RefSnapshot) -> String {
    let mut hasher = Sha256::new();
    hash_field(&mut hasher, b"synapse-local-ref-snapshot-v1");
    for reference in &snapshot.refs {
        hash_field(&mut hasher, b"ref");
        hash_field(&mut hasher, reference.name.as_bytes());
        hash_field(&mut hasher, reference.head.as_bytes());
        hash_field(&mut hasher, &reference.updated_event_id.to_be_bytes());
    }
    let digest = hasher.finalize();
    let mut hex = String::with_capacity(64);
    for byte in digest {
        write!(&mut hex, "{byte:02x}").expect("writing to String cannot fail");
    }
    format!("sha256:{hex}")
}

pub fn classify_image_media_type(bytes: &[u8]) -> ImageMediaType {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        ImageMediaType::Png
    } else if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        ImageMediaType::Jpeg
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        ImageMediaType::Gif
    } else if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        ImageMediaType::WebP
    } else {
        ImageMediaType::OctetStream
    }
}

fn project_summary(entry: &CatalogEntry) -> ProjectSummary {
    ProjectSummary {
        project_key: entry.project_key.clone(),
        display_label: entry.display_label.clone(),
        state: ProjectState::Ready,
        capabilities: ProjectCapabilities::slice_two(),
    }
}

fn open_repository(entry: &CatalogEntry) -> Result<Repository, ServiceError> {
    entry.open_repository().map_err(ServiceError::from)
}

fn capture_snapshot(repository: &Repository) -> Result<RefSnapshot, ServiceError> {
    repository
        .refs()
        .snapshot_limited(MAX_REFS)
        .map_err(ref_store_error)
}

fn snapshot_context(
    snapshot: &RefSnapshot,
    projection_source_fingerprint: Option<String>,
) -> SnapshotContext {
    SnapshotContext {
        watermark: snapshot_watermark(snapshot),
        ref_count: snapshot.refs.len(),
        projection_source_fingerprint,
    }
}

fn discover_sessions(
    repository: &Repository,
    snapshot: &RefSnapshot,
) -> Result<Vec<CreatorSessionSummary>, ServiceError> {
    discover_creator_sessions(repository, snapshot, MAX_CREATOR_SESSIONS)
        .map_err(creator_error)?
        .into_iter()
        .map(|session| {
            Ok(CreatorSessionSummary {
                session: session.session,
                state: match session.state {
                    CoreCreatorSessionState::Complete => CreatorSessionState::Complete,
                    CoreCreatorSessionState::Incomplete => CreatorSessionState::Incomplete,
                },
                proposal_ref: session.proposal_ref,
                proposal_head: session.proposal_head,
                decision_ref: session.decision_ref,
                decision_head: session.decision_head,
            })
        })
        .collect()
}

fn complete_session(
    snapshot: &RefSnapshot,
    snapshot_report: CreatorSnapshotReport,
) -> CompleteCreatorSession {
    let CreatorSnapshotReport {
        report,
        projection_source_fingerprint,
    } = snapshot_report;
    CompleteCreatorSession {
        state: CompleteState::Complete,
        report: creator_report(
            snapshot_context(snapshot, Some(projection_source_fingerprint)),
            report,
        ),
    }
}

fn incomplete_session(snapshot: &RefSnapshot, session: &str) -> IncompleteCreatorSession {
    IncompleteCreatorSession {
        state: IncompleteState::Incomplete,
        snapshot: snapshot_context(snapshot, None),
        session: session.to_owned(),
        recovery_supported: false,
        diagnostic: "The current creator Refs do not form a complete validated session. Automatic resume, cleanup, and history mutation are not supported.".into(),
    }
}

fn creator_report(snapshot: SnapshotContext, report: CoreCreatorReport) -> CreatorReport {
    let proposal_attributed_to_agent = report.agent_id.clone();
    let reviewed_by_human = report.creator_id.clone();
    CreatorReport {
        snapshot,
        session: report.session,
        project_id: report.project_id,
        subject_id: report.subject_id,
        creator_id: report.creator_id,
        agent_id: report.agent_id,
        decision_ref: report.decision_ref,
        proposal_ref: report.proposal_ref,
        decision_head: report.decision_head,
        proposal_head: report.proposal_head,
        base_head: report.base_head,
        base_snapshot: report.base_snapshot,
        proposal_snapshot: report.proposal_snapshot,
        decision_snapshot: report.decision_snapshot,
        disposition: report.disposition.as_cli_str().into(),
        selected_ai_output: report.selected_ai_output,
        proposal_attributed_to_agent,
        ai_output_source: "caller_supplied".into(),
        reviewed_by_human,
        rationale: report.rationale,
        original_blob_oid: report.original_blob_oid,
        current_blob_oid: report.current_blob_oid,
        ai_output_blob_oid: report.ai_output_blob_oid,
        comparison: report.comparison.map(comparison_evidence),
        timeline: report.timeline.into_iter().map(timeline_entry).collect(),
        fsck_objects: report.fsck_objects,
    }
}

fn comparison_evidence(report: CoreComparisonReport) -> ComparisonEvidence {
    ComparisonEvidence {
        analysis_oid: report.analysis_oid,
        tool_id: report.tool_id,
        tool_actor_oid: report.tool_actor_oid,
        adapter_id: report.adapter_id,
        adapter_version: report.adapter_version,
        implementation_oid: report.implementation_oid,
        configuration_oid: report.configuration_oid,
        status: report.status,
        comparability: report.comparability,
        outcome: report.outcome,
        reason_codes: report.reason_codes,
        warnings: report.warnings,
        base_observation_oid: report.base_observation_oid,
        target_observation_oid: report.target_observation_oid,
        base_media_oid: report.base_media_oid,
        target_media_oid: report.target_media_oid,
        replay_ready: report.replay_ready,
        reachable_from: report.reachable_from,
    }
}

fn timeline_entry(entry: CoreTimelineEntry) -> TimelineEntry {
    TimelineEntry {
        oid: entry.oid,
        stage: entry.stage.into(),
        kind: entry.kind.into(),
        entity_id: entry.entity_id,
        ordering_time: entry.ordering_time,
        time_basis: entry.time_basis.into(),
        reachable_from: entry.reachable_from,
    }
}

fn reflog_entry(entry: CoreReflogEntry) -> ReflogEntry {
    ReflogEntry {
        event_id: entry.id.to_string(),
        ref_name: entry.ref_name,
        old_head: entry.old_head,
        new_head: entry.new_head,
        occurred_at_unix_nanos: entry.occurred_at_unix_nanos.to_string(),
        actor: entry.actor,
        message: entry.message,
    }
}

fn validate_reflog_query(query: &ReflogQuery) -> Result<(), ServiceError> {
    if !(1..=MAX_REFLOG_PAGE_ENTRIES).contains(&query.limit) {
        return Err(ServiceError::new(
            "resource_limit",
            format!("The reflog page limit must be between 1 and {MAX_REFLOG_PAGE_ENTRIES}."),
            false,
        ));
    }
    if query
        .ref_name
        .as_ref()
        .is_some_and(|name| name.is_empty() || name.len() > 500)
    {
        return Err(ServiceError::new(
            "local_request_denied",
            "The reflog Ref filter must contain 1 to 500 UTF-8 bytes.",
            false,
        ));
    }
    Ok(())
}

fn parse_event_id(value: &str) -> Result<i64, ServiceError> {
    if value.is_empty()
        || (value.len() > 1 && value.starts_with('0'))
        || !value.bytes().all(|byte| byte.is_ascii_digit())
    {
        return Err(ServiceError::new(
            "local_request_denied",
            "after_event_id must be a canonical non-negative decimal integer.",
            false,
        ));
    }
    value.parse().map_err(|_| {
        ServiceError::new(
            "local_request_denied",
            "after_event_id is outside the supported range.",
            false,
        )
    })
}

fn ref_store_error(error: RefStoreError) -> ServiceError {
    let diagnostic = error.to_string();
    let code = error.code();
    let detail = match code {
        "resource_limit" => "The requested read exceeds a configured resource limit.",
        "path_segment_invalid" | "schema_invalid" => "The read request is invalid.",
        _ => "The local Ref store could not be read.",
    };
    ServiceError::new(code, detail, code == "storage_error").with_diagnostic(diagnostic)
}

fn creator_error(error: CreatorError) -> ServiceError {
    let diagnostic = error.to_string();
    let code = error.code().to_owned();
    let detail = match code.as_str() {
        "creator_session_not_found" => "The requested creator session was not found.",
        "creator_session_incomplete" => "The creator session is incomplete.",
        "resource_limit" => "Creator session discovery exceeded its resource limit.",
        "creator_report_invalid" => "The creator session report could not be validated.",
        "fsck_failed" => "Creator session integrity validation failed.",
        _ => "The creator session could not be read.",
    };
    let retryable = code == "storage_error";
    ServiceError::new(code, detail, retryable).with_diagnostic(diagnostic)
}

fn hash_field(hasher: &mut Sha256, bytes: &[u8]) {
    hasher.update((bytes.len() as u64).to_be_bytes());
    hasher.update(bytes);
}

fn problem_title(code: &str) -> &'static str {
    match code {
        "project_not_found" => "Project not found",
        "creator_session_not_found" => "Creator session not found",
        "creator_session_incomplete" => "Creator session incomplete",
        "creator_report_invalid" => "Creator report invalid",
        "fsck_failed" => "Integrity check failed",
        "resource_limit" => "Resource limit exceeded",
        "local_request_denied" => "Local request denied",
        _ => "Local read failed",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_signatures_are_conservative() {
        assert_eq!(
            classify_image_media_type(b"\x89PNG\r\n\x1a\nrest"),
            ImageMediaType::Png
        );
        assert_eq!(
            classify_image_media_type(&[0xff, 0xd8, 0xff, 0xe0]),
            ImageMediaType::Jpeg
        );
        assert_eq!(classify_image_media_type(b"GIF89a"), ImageMediaType::Gif);
        assert_eq!(
            classify_image_media_type(b"RIFF\0\0\0\0WEBPdata"),
            ImageMediaType::WebP
        );
        assert_eq!(
            classify_image_media_type(b"<svg xmlns='http://www.w3.org/2000/svg'/>"),
            ImageMediaType::OctetStream
        );
    }

    #[test]
    fn event_ids_require_canonical_decimal_form() {
        assert_eq!(parse_event_id("0").unwrap(), 0);
        assert_eq!(parse_event_id("9223372036854775807").unwrap(), i64::MAX);
        for invalid in ["", "00", "01", "-1", "+1", "1 ", "9223372036854775808"] {
            assert_eq!(
                parse_event_id(invalid).unwrap_err().code(),
                "local_request_denied"
            );
        }
    }

    #[test]
    fn service_shared_state_is_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<LocalService>();
    }
}
