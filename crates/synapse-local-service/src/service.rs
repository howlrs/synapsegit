use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::error::Error;
use std::fmt::{self, Write as _};
use std::mem;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Mutex, MutexGuard};
use synapse_core::{Repository, RepositoryError};
use synapse_creator::{
    CreatorBeginOptions, CreatorComparisonReport as CoreComparisonReport, CreatorDecisionOptions,
    CreatorDisposition as CoreCreatorDisposition, CreatorError, CreatorPendingDecisionState,
    CreatorPendingReceipt as CorePendingReceipt, CreatorReport as CoreCreatorReport,
    CreatorRunReceipt as CoreRunReceipt, CreatorSessionState as CoreCreatorSessionState,
    CreatorSnapshotReport, CreatorTimelineEntry as CoreTimelineEntry,
    PendingCreatorSession as CorePendingCreatorSession, begin_creator_session as core_begin,
    creator_report_from_snapshot, decide_creator_session as core_decide, discover_creator_sessions,
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
pub const MAX_PENDING_CREATOR_SESSIONS: usize = 64;
pub const MAX_PENDING_CREATOR_SESSIONS_PER_PROJECT: usize = 8;

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

    fn review_busy() -> Self {
        Self::new(
            "creator_review_busy",
            "The creator review is already being decided.",
            true,
        )
    }

    fn review_state_lost() -> Self {
        Self::new(
            "creator_review_state_lost",
            "The creator review is not available in this server process.",
            false,
        )
    }

    fn outcome_unknown() -> Self {
        Self::new(
            "creator_outcome_unknown",
            "The creator publication outcome is unknown and will not be retried automatically.",
            false,
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

#[derive(Default)]
struct PendingRegistry {
    entries: BTreeMap<String, PendingEntry>,
}

struct PendingEntry {
    project_key: String,
    session: String,
    server_instance: String,
    proposal_head: Option<String>,
    state: PendingEntryState,
}

enum PendingEntryState {
    Reserved,
    Ready(Box<ReadyPendingState>),
    Deciding,
    OutcomeUnknown,
}

struct ReadyPendingState {
    pending: CorePendingCreatorSession,
    receipt: CorePendingReceipt,
}

#[derive(Clone)]
struct ReadyPending {
    review_id: String,
    server_instance: String,
    receipt: CorePendingReceipt,
}

impl PendingRegistry {
    fn reserve(
        &mut self,
        review_id: String,
        project_key: &str,
        session: &str,
        server_instance: &str,
    ) -> Result<(), ServiceError> {
        if self.entries.len() >= MAX_PENDING_CREATOR_SESSIONS {
            return Err(ServiceError::new(
                "resource_limit",
                format!(
                    "The process already retains {MAX_PENDING_CREATOR_SESSIONS} creator reviews."
                ),
                false,
            ));
        }
        if self
            .entries
            .values()
            .filter(|entry| entry.project_key == project_key)
            .count()
            >= MAX_PENDING_CREATOR_SESSIONS_PER_PROJECT
        {
            return Err(ServiceError::new(
                "resource_limit",
                format!(
                    "The project already retains {MAX_PENDING_CREATOR_SESSIONS_PER_PROJECT} creator reviews."
                ),
                false,
            ));
        }
        if let Some(existing) = self
            .entries
            .values()
            .find(|entry| entry.project_key == project_key && entry.session == session)
        {
            return Err(match &existing.state {
                PendingEntryState::Reserved | PendingEntryState::Deciding => {
                    ServiceError::review_busy()
                }
                PendingEntryState::Ready(_) => ServiceError::new(
                    "creator_session_exists",
                    "The creator session already has a pending review.",
                    false,
                ),
                PendingEntryState::OutcomeUnknown => ServiceError::review_state_lost(),
            });
        }
        if self.entries.contains_key(&review_id) {
            return Err(ServiceError::new(
                "service_unavailable",
                "A creator review identifier could not be allocated.",
                true,
            ));
        }
        self.entries.insert(
            review_id,
            PendingEntry {
                project_key: project_key.to_owned(),
                session: session.to_owned(),
                server_instance: server_instance.to_owned(),
                proposal_head: None,
                state: PendingEntryState::Reserved,
            },
        );
        Ok(())
    }
}

/// Exact startup-owned localhost facade.
///
/// Only canonical catalog paths are retained. Read calls open fresh
/// repositories; pending writes retain the creator-owned application instance,
/// whose repository connection remains behind its own synchronization boundary.
pub struct LocalService {
    catalog: ProjectCatalog,
    pending: Mutex<PendingRegistry>,
}

impl fmt::Debug for LocalService {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LocalService")
            .field("project_count", &self.catalog.len())
            .field("pending_count", &lock_registry(&self.pending).entries.len())
            .finish_non_exhaustive()
    }
}

impl LocalService {
    pub fn new(
        registrations: impl IntoIterator<Item = ProjectRegistration>,
    ) -> Result<Self, CatalogError> {
        Ok(Self {
            catalog: ProjectCatalog::build(registrations)?,
            pending: Mutex::new(PendingRegistry::default()),
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
        let sessions = self.sessions_with_pending(&repository, &snapshot, project_key)?;
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

    /// Publish one proposal using catalog-fixed repository authority and retain
    /// the exact same-process Human review capability before returning.
    pub fn begin_creator_session(
        &self,
        project_key: &str,
        server_instance: &str,
        request: BeginCreatorSessionRequest,
    ) -> Result<PendingCreatorSession, ServiceError> {
        validate_begin_request(server_instance, &request)?;
        let repository_path = self.entry(project_key)?.repository_path().to_owned();
        let review_id = self.reserve_pending(project_key, &request.session, server_instance)?;

        let repository = match Repository::open(&repository_path).map_err(repository_error) {
            Ok(repository) => repository,
            Err(error) => {
                self.remove_reserved(&review_id);
                return Err(error);
            }
        };
        let before = match capture_snapshot(&repository) {
            Ok(snapshot) => snapshot,
            Err(error) => {
                self.remove_reserved(&review_id);
                return Err(error);
            }
        };
        let session = request.session.clone();
        let options = CreatorBeginOptions {
            repository: repository_path,
            session: request.session,
            original_image: request.original_image,
            current_image: request.current_image,
            ai_output: request.ai_output,
            subject_label: request.subject_label,
            creator_name: request.creator_name,
        };
        let outcome = catch_unwind(AssertUnwindSafe(|| core_begin(&options)));
        let (pending, receipt) = match outcome {
            Ok(Ok(pending)) => {
                let receipt = pending.receipt().clone();
                (pending, receipt)
            }
            Ok(Err(error)) => {
                self.settle_failed_begin(&review_id, project_key, &session, &before);
                return Err(creator_error(error));
            }
            Err(_) => {
                self.mark_outcome_unknown(&review_id);
                return Err(ServiceError::outcome_unknown()
                    .with_diagnostic("creator proposal publication panicked"));
            }
        };
        if !valid_pending_receipt(&receipt, &session) {
            self.mark_outcome_unknown(&review_id);
            return Err(ServiceError::outcome_unknown()
                .with_diagnostic("creator pending receipt did not match the reserved session"));
        }
        self.fill_ready(&review_id, pending, receipt)?;

        let snapshot = capture_snapshot(&repository)?;
        self.ready_pending(project_key, &snapshot)?
            .into_iter()
            .find(|pending| pending.review_id == review_id)
            .map(|pending| pending_session(&snapshot, pending))
            .ok_or_else(ServiceError::outcome_unknown)
    }

    /// Publish one server-fixed Human decision through the exact pending
    /// application instance named by `review_id`.
    pub fn decide_creator_session(
        &self,
        project_key: &str,
        session: &str,
        server_instance: &str,
        request: CreatorDecisionRequest,
    ) -> Result<CompleteCreatorSession, ServiceError> {
        validate_decision_request(session, &request)?;
        self.entry(project_key)?;
        let repository = self.open_repository(project_key)?;
        let snapshot = capture_snapshot(&repository)?;
        let (mut pending, pending_receipt) = self.claim_ready(
            project_key,
            session,
            server_instance,
            &request.review_id,
            &snapshot,
        )?;
        let decision = CreatorDecisionOptions {
            disposition: core_disposition(request.disposition),
            rationale: request.rationale,
        };
        let outcome = catch_unwind(AssertUnwindSafe(|| core_decide(&mut pending, &decision)));
        let run_receipt = match outcome {
            Ok(Ok(receipt)) => receipt,
            Ok(Err(error)) => match pending.decision_state() {
                CreatorPendingDecisionState::Ready => {
                    if self.restore_ready_after_error(
                        &request.review_id,
                        project_key,
                        pending,
                        pending_receipt,
                    ) {
                        return Err(creator_error(error));
                    }
                    return Err(ServiceError::outcome_unknown().with_diagnostic(
                        "creator decision failed before publication, but live Refs changed",
                    ));
                }
                CreatorPendingDecisionState::Consumed => {
                    let Some(completed) = pending.completed_receipt().cloned() else {
                        self.mark_outcome_unknown(&request.review_id);
                        return Err(ServiceError::outcome_unknown().with_diagnostic(
                            "creator decision reported consumed state without a receipt",
                        ));
                    };
                    return self.finish_decision(
                        project_key,
                        session,
                        &request.review_id,
                        &completed,
                        &pending_receipt,
                    );
                }
                CreatorPendingDecisionState::Deciding
                | CreatorPendingDecisionState::OutcomeUnknown => {
                    self.mark_outcome_unknown(&request.review_id);
                    return Err(ServiceError::outcome_unknown().with_diagnostic(error.to_string()));
                }
            },
            Err(_) => {
                self.mark_outcome_unknown(&request.review_id);
                return Err(ServiceError::outcome_unknown()
                    .with_diagnostic("creator Human decision publication panicked"));
            }
        };
        self.finish_decision(
            project_key,
            session,
            &request.review_id,
            &run_receipt,
            &pending_receipt,
        )
    }

    pub fn list_creator_sessions(
        &self,
        project_key: &str,
    ) -> Result<CreatorSessionList, ServiceError> {
        let repository = self.open_repository(project_key)?;
        let snapshot = capture_snapshot(&repository)?;
        let sessions = self.sessions_with_pending(&repository, &snapshot, project_key)?;
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
        if let Some(pending) = self
            .ready_pending(project_key, &snapshot)?
            .into_iter()
            .find(|pending| pending.receipt.session == session)
        {
            return Ok(CreatorSessionDetail::PendingReview(Box::new(
                pending_session(&snapshot, pending),
            )));
        }
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
        if let Some(pending) = self
            .ready_pending(project_key, &snapshot)?
            .into_iter()
            .find(|pending| pending.receipt.session == session)
        {
            let blob_oid = pending_blob_oid(&repository, &pending.receipt, role)?;
            return load_creator_image(&repository, blob_oid);
        }
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
        load_creator_image(&repository, blob_oid)
    }

    fn sessions_with_pending(
        &self,
        repository: &Repository,
        snapshot: &RefSnapshot,
        project_key: &str,
    ) -> Result<Vec<CreatorSessionSummary>, ServiceError> {
        let sessions = discover_sessions(repository, snapshot)?;
        Ok(overlay_pending_sessions(
            sessions,
            self.ready_pending(project_key, snapshot)?,
        ))
    }

    fn reserve_pending(
        &self,
        project_key: &str,
        session: &str,
        server_instance: &str,
    ) -> Result<String, ServiceError> {
        let review_id = random_review_id()?;
        lock_registry(&self.pending).reserve(
            review_id.clone(),
            project_key,
            session,
            server_instance,
        )?;
        Ok(review_id)
    }

    fn remove_reserved(&self, review_id: &str) {
        let mut registry = lock_registry(&self.pending);
        if registry
            .entries
            .get(review_id)
            .is_some_and(|entry| matches!(entry.state, PendingEntryState::Reserved))
        {
            registry.entries.remove(review_id);
        }
    }

    fn mark_outcome_unknown(&self, review_id: &str) {
        if let Some(entry) = lock_registry(&self.pending).entries.get_mut(review_id) {
            entry.state = PendingEntryState::OutcomeUnknown;
        }
    }

    fn settle_failed_begin(
        &self,
        review_id: &str,
        project_key: &str,
        session: &str,
        before: &RefSnapshot,
    ) {
        let unchanged = self
            .open_repository(project_key)
            .and_then(|repository| capture_snapshot(&repository))
            .is_ok_and(|after| session_heads(before, session) == session_heads(&after, session));
        if unchanged {
            self.remove_reserved(review_id);
        } else {
            self.mark_outcome_unknown(review_id);
        }
    }

    fn fill_ready(
        &self,
        review_id: &str,
        pending: CorePendingCreatorSession,
        receipt: CorePendingReceipt,
    ) -> Result<(), ServiceError> {
        let mut registry = lock_registry(&self.pending);
        let Some(entry) = registry.entries.get_mut(review_id) else {
            return Err(ServiceError::outcome_unknown()
                .with_diagnostic("reserved creator review entry disappeared"));
        };
        if !matches!(entry.state, PendingEntryState::Reserved) {
            entry.state = PendingEntryState::OutcomeUnknown;
            return Err(ServiceError::outcome_unknown()
                .with_diagnostic("reserved creator review entry changed state"));
        }
        entry.proposal_head = Some(receipt.proposal_head.clone());
        entry.state = PendingEntryState::Ready(Box::new(ReadyPendingState { pending, receipt }));
        Ok(())
    }

    fn ready_pending(
        &self,
        project_key: &str,
        snapshot: &RefSnapshot,
    ) -> Result<Vec<ReadyPending>, ServiceError> {
        let mut registry = lock_registry(&self.pending);
        let mut ready = Vec::new();
        for (review_id, entry) in &mut registry.entries {
            if entry.project_key != project_key {
                continue;
            }
            let receipt = match &entry.state {
                PendingEntryState::Ready(ready) => ready.receipt.clone(),
                _ => continue,
            };
            if entry.proposal_head.as_deref() == Some(receipt.proposal_head.as_str())
                && pending_heads_match(snapshot, &receipt)
            {
                ready.push(ReadyPending {
                    review_id: review_id.clone(),
                    server_instance: entry.server_instance.clone(),
                    receipt,
                });
            } else {
                entry.state = PendingEntryState::OutcomeUnknown;
            }
        }
        Ok(ready)
    }

    fn claim_ready(
        &self,
        project_key: &str,
        session: &str,
        server_instance: &str,
        review_id: &str,
        snapshot: &RefSnapshot,
    ) -> Result<(CorePendingCreatorSession, CorePendingReceipt), ServiceError> {
        let mut registry = lock_registry(&self.pending);
        let Some(entry) = registry.entries.get_mut(review_id) else {
            return Err(ServiceError::review_state_lost());
        };
        if entry.project_key != project_key
            || entry.session != session
            || entry.server_instance != server_instance
        {
            return Err(ServiceError::review_state_lost());
        }
        match &entry.state {
            PendingEntryState::Reserved | PendingEntryState::OutcomeUnknown => {
                return Err(ServiceError::review_state_lost());
            }
            PendingEntryState::Deciding => return Err(ServiceError::review_busy()),
            PendingEntryState::Ready(ready)
                if entry.proposal_head.as_deref() != Some(ready.receipt.proposal_head.as_str())
                    || !pending_heads_match(snapshot, &ready.receipt) =>
            {
                entry.state = PendingEntryState::OutcomeUnknown;
                return Err(ServiceError::review_state_lost());
            }
            PendingEntryState::Ready(_) => {}
        }
        match mem::replace(&mut entry.state, PendingEntryState::Deciding) {
            PendingEntryState::Ready(ready) => Ok((ready.pending, ready.receipt)),
            _ => unreachable!("ready creator review changed while registry was locked"),
        }
    }

    fn restore_ready_after_error(
        &self,
        review_id: &str,
        project_key: &str,
        pending: CorePendingCreatorSession,
        receipt: CorePendingReceipt,
    ) -> bool {
        let heads_match = self
            .open_repository(project_key)
            .and_then(|repository| capture_snapshot(&repository))
            .is_ok_and(|snapshot| pending_heads_match(&snapshot, &receipt));
        let mut registry = lock_registry(&self.pending);
        let Some(entry) = registry.entries.get_mut(review_id) else {
            return false;
        };
        if heads_match && matches!(entry.state, PendingEntryState::Deciding) {
            entry.state =
                PendingEntryState::Ready(Box::new(ReadyPendingState { pending, receipt }));
            true
        } else {
            entry.state = PendingEntryState::OutcomeUnknown;
            false
        }
    }

    fn finish_decision(
        &self,
        project_key: &str,
        session: &str,
        review_id: &str,
        run_receipt: &CoreRunReceipt,
        pending_receipt: &CorePendingReceipt,
    ) -> Result<CompleteCreatorSession, ServiceError> {
        let repository = match self.open_repository(project_key) {
            Ok(repository) => repository,
            Err(error) => {
                self.mark_outcome_unknown(review_id);
                return Err(error);
            }
        };
        let snapshot = match capture_snapshot(&repository) {
            Ok(snapshot) => snapshot,
            Err(error) => {
                self.mark_outcome_unknown(review_id);
                return Err(error);
            }
        };
        let snapshot_report = match creator_report_from_snapshot(&repository, &snapshot, session) {
            Ok(report) if completion_matches(run_receipt, pending_receipt, &report) => report,
            Ok(_) => {
                self.mark_outcome_unknown(review_id);
                return Err(ServiceError::outcome_unknown().with_diagnostic(
                    "completed creator report did not match the publication receipt",
                ));
            }
            Err(error) => {
                self.mark_outcome_unknown(review_id);
                return Err(creator_error(error));
            }
        };
        self.consume_deciding(review_id)?;
        Ok(complete_session(&snapshot, snapshot_report))
    }

    fn consume_deciding(&self, review_id: &str) -> Result<(), ServiceError> {
        let mut registry = lock_registry(&self.pending);
        if registry
            .entries
            .get(review_id)
            .is_some_and(|entry| matches!(entry.state, PendingEntryState::Deciding))
        {
            registry.entries.remove(review_id);
            Ok(())
        } else {
            if let Some(entry) = registry.entries.get_mut(review_id) {
                entry.state = PendingEntryState::OutcomeUnknown;
            }
            Err(ServiceError::outcome_unknown()
                .with_diagnostic("deciding creator review entry changed before consumption"))
        }
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

fn lock_registry(registry: &Mutex<PendingRegistry>) -> MutexGuard<'_, PendingRegistry> {
    match registry.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

fn validate_begin_request(
    server_instance: &str,
    request: &BeginCreatorSessionRequest,
) -> Result<(), ServiceError> {
    if !is_slug(&request.session) {
        return Err(ServiceError::new(
            "usage_error",
            "The creator session name is invalid.",
            false,
        ));
    }
    if request.subject_label.is_empty() || request.subject_label.len() > 500 {
        return Err(ServiceError::new(
            "usage_error",
            "The subject label must contain 1 to 500 UTF-8 bytes.",
            false,
        ));
    }
    if request.creator_name.is_empty() || request.creator_name.len() > 300 {
        return Err(ServiceError::new(
            "usage_error",
            "The creator name must contain 1 to 300 UTF-8 bytes.",
            false,
        ));
    }
    if server_instance.is_empty()
        || server_instance.len() > 300
        || server_instance.chars().any(char::is_control)
    {
        return Err(ServiceError::new(
            "local_request_denied",
            "The trusted server instance binding is invalid.",
            false,
        ));
    }
    Ok(())
}

fn validate_decision_request(
    session: &str,
    request: &CreatorDecisionRequest,
) -> Result<(), ServiceError> {
    if !is_slug(session) {
        return Err(ServiceError::session_not_found());
    }
    if !(22..=128).contains(&request.review_id.len())
        || !request
            .review_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
    {
        return Err(ServiceError::new(
            "local_request_denied",
            "The creator review identifier is invalid.",
            false,
        ));
    }
    if request
        .rationale
        .as_ref()
        .is_some_and(|rationale| rationale.len() > 5_000)
    {
        return Err(ServiceError::new(
            "usage_error",
            "The Human rationale exceeds 5000 UTF-8 bytes.",
            false,
        ));
    }
    Ok(())
}

fn random_review_id() -> Result<String, ServiceError> {
    let mut bytes = [0_u8; 32];
    getrandom::fill(&mut bytes).map_err(|error| {
        ServiceError::new(
            "service_unavailable",
            "A creator review identifier could not be allocated.",
            true,
        )
        .with_diagnostic(format!("operating-system random source failed: {error}"))
    })?;
    let mut review_id = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        write!(&mut review_id, "{byte:02x}").expect("writing to String cannot fail");
    }
    Ok(review_id)
}

fn valid_pending_receipt(receipt: &CorePendingReceipt, session: &str) -> bool {
    receipt.session == session
        && receipt.decision_ref == format!("decision/creator/{session}")
        && receipt.proposal_ref == format!("proposal/creator-agent/{session}")
        && !receipt.base_head.is_empty()
        && !receipt.proposal_head.is_empty()
}

fn snapshot_head<'a>(snapshot: &'a RefSnapshot, name: &str) -> Option<&'a str> {
    snapshot
        .refs
        .iter()
        .find(|reference| reference.name == name)
        .map(|reference| reference.head.as_str())
}

fn pending_heads_match(snapshot: &RefSnapshot, receipt: &CorePendingReceipt) -> bool {
    snapshot_head(snapshot, &receipt.decision_ref) == Some(receipt.base_head.as_str())
        && snapshot_head(snapshot, &receipt.proposal_ref) == Some(receipt.proposal_head.as_str())
}

type RefVersion = Option<(String, i64)>;

fn session_heads(snapshot: &RefSnapshot, session: &str) -> (RefVersion, RefVersion) {
    let decision_ref = format!("decision/creator/{session}");
    let proposal_ref = format!("proposal/creator-agent/{session}");
    let version = |name: &str| {
        snapshot
            .refs
            .iter()
            .find(|reference| reference.name == name)
            .map(|reference| (reference.head.clone(), reference.updated_event_id))
    };
    (version(&decision_ref), version(&proposal_ref))
}

fn overlay_pending_sessions(
    mut sessions: Vec<CreatorSessionSummary>,
    pending: Vec<ReadyPending>,
) -> Vec<CreatorSessionSummary> {
    for pending in pending {
        let summary = CreatorSessionSummary {
            session: pending.receipt.session.clone(),
            state: CreatorSessionState::PendingReview,
            proposal_ref: Some(pending.receipt.proposal_ref.clone()),
            proposal_head: Some(pending.receipt.proposal_head.clone()),
            decision_ref: Some(pending.receipt.decision_ref.clone()),
            decision_head: Some(pending.receipt.base_head.clone()),
        };
        if let Some(existing) = sessions
            .iter_mut()
            .find(|session| session.session == summary.session)
        {
            *existing = summary;
        } else {
            sessions.push(summary);
        }
    }
    sessions.sort_by(|left, right| left.session.cmp(&right.session));
    sessions
}

fn pending_session(snapshot: &RefSnapshot, pending: ReadyPending) -> PendingCreatorSession {
    let receipt = pending.receipt;
    PendingCreatorSession {
        state: PendingReviewState::PendingReview,
        snapshot: snapshot_context(snapshot, None),
        server_instance: pending.server_instance,
        review_id: pending.review_id,
        session: receipt.session,
        project_id: receipt.project_id,
        subject_id: receipt.subject_id,
        proposal_ref: receipt.proposal_ref,
        proposal_head: receipt.proposal_head,
        original_blob_oid: receipt.original_blob_oid,
        current_blob_oid: receipt.current_blob_oid,
        ai_output_blob_oid: receipt.ai_output_blob_oid,
        ai_output_source: "caller_supplied".into(),
        comparison: comparison_evidence(receipt.comparison),
    }
}

fn core_disposition(decision: CreatorDecision) -> CoreCreatorDisposition {
    match decision {
        CreatorDecision::Adopt => CoreCreatorDisposition::Adopt,
        CreatorDecision::Reject => CoreCreatorDisposition::Reject,
        CreatorDecision::Defer => CoreCreatorDisposition::Defer,
    }
}

fn completion_matches(
    receipt: &CoreRunReceipt,
    pending: &CorePendingReceipt,
    snapshot_report: &CreatorSnapshotReport,
) -> bool {
    let report = &snapshot_report.report;
    receipt.session == pending.session
        && receipt.session == report.session
        && receipt.project_id == pending.project_id
        && receipt.project_id == report.project_id
        && receipt.subject_id == pending.subject_id
        && receipt.subject_id == report.subject_id
        && receipt.creator_id == pending.creator_id
        && receipt.creator_id == report.creator_id
        && receipt.agent_id == pending.agent_id
        && receipt.agent_id == report.agent_id
        && receipt.decision_ref == pending.decision_ref
        && receipt.decision_ref == report.decision_ref
        && receipt.proposal_ref == pending.proposal_ref
        && receipt.proposal_ref == report.proposal_ref
        && receipt.base_head == pending.base_head
        && receipt.base_head == report.base_head
        && receipt.proposal_head == pending.proposal_head
        && receipt.proposal_head == report.proposal_head
        && receipt.decision_head == report.decision_head
        && receipt.original_blob_oid == pending.original_blob_oid
        && receipt.original_blob_oid == report.original_blob_oid
        && receipt.current_blob_oid == pending.current_blob_oid
        && receipt.current_blob_oid == report.current_blob_oid
        && receipt.ai_output_blob_oid == pending.ai_output_blob_oid
        && receipt.ai_output_blob_oid == report.ai_output_blob_oid
        && receipt.capture_profile_oid == pending.capture_profile_oid
        && receipt.original_observation_oid == pending.original_observation_oid
        && receipt.current_observation_oid == pending.current_observation_oid
        && receipt.comparison_tool_id == pending.comparison.tool_id
        && receipt.comparison_tool_actor_oid == pending.comparison.tool_actor_oid
        && receipt.comparison_analysis_oid == pending.comparison.analysis_oid
        && receipt.comparison_implementation_oid == pending.comparison.implementation_oid
        && receipt.comparison_configuration_oid == pending.comparison.configuration_oid
        && receipt.ai_activity_oid == pending.ai_activity_oid
        && report.comparison.as_ref().is_some_and(|comparison| {
            comparison.analysis_oid == receipt.comparison_analysis_oid
                && comparison.tool_id == receipt.comparison_tool_id
                && comparison.tool_actor_oid == receipt.comparison_tool_actor_oid
                && comparison.implementation_oid == receipt.comparison_implementation_oid
                && comparison.configuration_oid == receipt.comparison_configuration_oid
        })
        && receipt.disposition == report.disposition
}

fn pending_blob_oid(
    repository: &Repository,
    receipt: &CorePendingReceipt,
    role: ImageRole,
) -> Result<String, ServiceError> {
    let (head, entry_name, expected_oid) = match role {
        ImageRole::Original => (
            &receipt.base_head,
            "original.image",
            &receipt.original_blob_oid,
        ),
        ImageRole::Current => (
            &receipt.base_head,
            "current.image",
            &receipt.current_blob_oid,
        ),
        ImageRole::AiOutput => (
            &receipt.proposal_head,
            "ai-proposal.image",
            &receipt.ai_output_blob_oid,
        ),
    };
    verify_pending_tree_entry(repository, head, entry_name, expected_oid)?;
    Ok(expected_oid.clone())
}

fn verify_pending_tree_entry(
    repository: &Repository,
    head: &str,
    entry_name: &str,
    expected_oid: &str,
) -> Result<(), ServiceError> {
    let commit = read_pending_json(repository, head)?;
    let tree_oid = commit
        .get("snapshot")
        .and_then(serde_json::Value::as_str)
        .filter(|_| {
            commit
                .get("object_type")
                .and_then(serde_json::Value::as_str)
                == Some("commit")
        })
        .ok_or_else(pending_lineage_invalid)?;
    let tree = read_pending_json(repository, tree_oid)?;
    let entry = tree
        .get("entries")
        .and_then(serde_json::Value::as_object)
        .and_then(|entries| entries.get(entry_name))
        .and_then(serde_json::Value::as_object)
        .filter(|_| tree.get("object_type").and_then(serde_json::Value::as_str) == Some("tree"))
        .ok_or_else(pending_lineage_invalid)?;
    if entry.get("entry_kind").and_then(serde_json::Value::as_str) != Some("blob")
        || entry.get("oid").and_then(serde_json::Value::as_str) != Some(expected_oid)
    {
        return Err(pending_lineage_invalid());
    }
    Ok(())
}

fn read_pending_json(
    repository: &Repository,
    oid: &str,
) -> Result<serde_json::Value, ServiceError> {
    let bytes = repository
        .objects()
        .read_raw(oid)
        .map_err(|error| pending_lineage_invalid().with_diagnostic(error.to_string()))?
        .ok_or_else(pending_lineage_invalid)?;
    serde_json::from_slice(&bytes)
        .map_err(|error| pending_lineage_invalid().with_diagnostic(error.to_string()))
}

fn pending_lineage_invalid() -> ServiceError {
    ServiceError::new(
        "creator_report_invalid",
        "The pending creator image is not reachable from its retained proposal lineage.",
        false,
    )
}

fn load_creator_image(
    repository: &Repository,
    blob_oid: String,
) -> Result<CreatorImage, ServiceError> {
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

fn repository_error(error: RepositoryError) -> ServiceError {
    let code = error.code().to_owned();
    let retryable = code == "storage_error";
    ServiceError::new(
        code,
        "The local project could not be opened for a creator operation.",
        retryable,
    )
    .with_diagnostic(error.to_string())
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
        capabilities: ProjectCapabilities::creator_workflow(),
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
        "usage_error" => "The creator request is invalid.",
        "creator_session_exists" => "The creator session already exists.",
        "creator_session_not_found" => "The requested creator session was not found.",
        "creator_session_incomplete" => "The creator session is incomplete.",
        "resource_limit" => "The creator operation exceeded a configured resource limit.",
        "creator_report_invalid" => "The creator session report could not be validated.",
        "fsck_failed" => "Creator session integrity validation failed.",
        "ref_conflict" | "stale_base" => "The creator session changed before publication.",
        "authentication_required"
        | "project_access_denied"
        | "execution_permit_invalid"
        | "execution_failed"
        | "configuration_invalid" => "The creator application denied the operation.",
        "service_unavailable" | "storage_error" => {
            "The creator operation could not access local service state."
        }
        _ => "The creator operation failed.",
    };
    let retryable = matches!(code.as_str(), "storage_error" | "service_unavailable");
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
        "creator_session_exists" => "Creator session already exists",
        "creator_session_incomplete" => "Creator session incomplete",
        "creator_report_invalid" => "Creator report invalid",
        "creator_review_busy" => "Creator review busy",
        "creator_review_state_lost" => "Creator review state lost",
        "creator_outcome_unknown" => "Creator outcome unknown",
        "fsck_failed" => "Integrity check failed",
        "resource_limit" => "Resource limit exceeded",
        "usage_error" => "Invalid creator request",
        "local_request_denied" => "Local request denied",
        "service_unavailable" => "Local service unavailable",
        "storage_error" => "Local storage failed",
        _ => "Local operation failed",
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

    #[test]
    fn pending_registry_enforces_project_and_process_capacity() {
        let mut registry = PendingRegistry::default();
        for project in 0..8 {
            for session in 0..MAX_PENDING_CREATOR_SESSIONS_PER_PROJECT {
                registry
                    .reserve(
                        format!("review-{project}-{session}"),
                        &format!("project-{project}"),
                        &format!("session-{session}"),
                        "server-instance",
                    )
                    .unwrap();
            }
            if project == 0 {
                let error = registry
                    .reserve(
                        "project-over-limit".into(),
                        "project-0",
                        "session-over-limit",
                        "server-instance",
                    )
                    .unwrap_err();
                assert_eq!(error.code(), "resource_limit");
                assert_eq!(registry.entries.len(), 8);
            }
        }
        assert_eq!(registry.entries.len(), MAX_PENDING_CREATOR_SESSIONS);
        let error = registry
            .reserve(
                "process-over-limit".into(),
                "project-over-limit",
                "session",
                "server-instance",
            )
            .unwrap_err();
        assert_eq!(error.code(), "resource_limit");
        assert_eq!(registry.entries.len(), MAX_PENDING_CREATOR_SESSIONS);
    }
}
