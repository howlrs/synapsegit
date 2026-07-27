use crate::io::{object_field, read_json, string_field};
use crate::records::SCHEMA_VERSION;
use crate::session::{
    COMPARISON_ANALYSIS_ENTRY, COMPARISON_CONFIGURATION_ENTRY, COMPARISON_IMPLEMENTATION_ENTRY,
    COMPARISON_TOOL_ENTRY, CREATOR_FSCK_LIMITS, CREATOR_FSCK_MAX_REF_ROOTS, DECISION_PREFIX,
    PROPOSAL_PREFIX, SessionIds, decision_ref, proposal_ref, related_entity_id, validate_session,
};
use crate::{
    CreatorComparisonReport, CreatorDisposition, CreatorError, CreatorReport, CreatorSessionState,
    CreatorSessionSummary, CreatorSnapshotReport, CreatorTimelineEntry, Result,
};
use serde_json::{Map as JsonMap, Value as JsonValue};
use std::collections::BTreeMap;
use std::path::Path;
use synapse_canonical::ObjectKind;
use synapse_core::{FsckLimits, Repository};
use synapse_observation::{
    BYTE_IDENTITY_ADAPTER_ID, BYTE_IDENTITY_ADAPTER_VERSION, byte_identity_configuration_oid,
    byte_identity_implementation_oid,
};
use synapse_projection::{
    AdapterDeterminism, AnalysisReplayReadiness, ObjectAvailability, ProjectionLimits, RefScope,
    SqliteProjectionStore, TimelineRecordKind, TimelineTimeBasis,
};
use synapse_sqlite::RefSnapshot;

/// Rebuild a creator report from current Refs and CAS.
pub fn creator_report(repository_path: impl AsRef<Path>, session: &str) -> Result<CreatorReport> {
    validate_session(session)?;
    let repository = Repository::open(repository_path)?;
    let snapshot = repository
        .refs()
        .snapshot_limited(CREATOR_FSCK_MAX_REF_ROOTS)?;
    Ok(creator_report_from_snapshot(&repository, &snapshot, session)?.report)
}

/// Rebuild a creator report from exactly the supplied Ref snapshot.
///
/// This is the read boundary used by transports that must return one coherent
/// snapshot watermark and Projection fingerprint. It never captures a second
/// Ref snapshot internally. CAS remains append-only under the local trust
/// model, and the final integrity check is scoped to the supplied heads. This
/// compatibility API prepares a reader and delegates one report to it; batch
/// callers should prepare one [`PreparedCreatorReportReader`] directly.
pub fn creator_report_from_snapshot(
    repository: &Repository,
    snapshot: &RefSnapshot,
    session: &str,
) -> Result<CreatorSnapshotReport> {
    creator_report_from_snapshot_with_limits(repository, snapshot, session, CREATOR_FSCK_LIMITS)
}

pub(crate) fn creator_report_from_snapshot_with_limits(
    repository: &Repository,
    snapshot: &RefSnapshot,
    session: &str,
    fsck_limits: FsckLimits,
) -> Result<CreatorSnapshotReport> {
    PreparedCreatorReportReader::prepare_with_report_and_limits(
        repository,
        snapshot,
        session,
        fsck_limits,
    )
    .map(|(_, report)| report)
}

struct CreatorReportVerification<'source> {
    repository: &'source Repository,
    snapshot: &'source RefSnapshot,
    fsck_objects: usize,
}

struct PreparedCreatorReportSession {
    decision_ref: String,
    proposal_ref: String,
    decision_head: String,
    proposal_head: String,
    ids: SessionIds,
    lineage: ReportLineage,
}

fn verify_creator_report_snapshot<'source>(
    repository: &'source Repository,
    snapshot: &'source RefSnapshot,
    fsck_limits: FsckLimits,
) -> Result<CreatorReportVerification<'source>> {
    let fsck = repository.fsck_snapshot_with_limits(snapshot, fsck_limits)?;
    if !fsck.is_clean() {
        return Err(CreatorError::Integrity(format!(
            "creator report refused {} fsck issue(s)",
            fsck.issues.len()
        )));
    }
    Ok(CreatorReportVerification {
        repository,
        snapshot,
        fsck_objects: fsck.objects_verified,
    })
}

fn prepare_creator_report_session(
    repository: &Repository,
    snapshot: &RefSnapshot,
    session: &str,
) -> Result<PreparedCreatorReportSession> {
    let decision_ref = decision_ref(session);
    let proposal_ref = proposal_ref(session);
    let decision_head = snapshot
        .refs
        .iter()
        .find(|record| record.name == decision_ref)
        .map(|record| record.head.clone());
    let proposal_head = snapshot
        .refs
        .iter()
        .find(|record| record.name == proposal_ref)
        .map(|record| record.head.clone());
    let (decision_head, proposal_head) = match (decision_head, proposal_head) {
        (Some(decision), Some(proposal)) => (decision, proposal),
        (None, None) => return Err(CreatorError::SessionNotFound(session.to_owned())),
        _ => return Err(CreatorError::SessionIncomplete(session.to_owned())),
    };
    if read_json(repository, &decision_head)?
        .get("commit_kind")
        .and_then(JsonValue::as_str)
        != Some("decision")
    {
        return Err(CreatorError::SessionIncomplete(session.to_owned()));
    }
    let ids = load_session_ids(repository, session, &decision_head)?;
    let lineage = validate_report_lineage(repository, &ids, &decision_head, &proposal_head)?;
    Ok(PreparedCreatorReportSession {
        decision_ref,
        proposal_ref,
        decision_head,
        proposal_head,
        ids,
        lineage,
    })
}

/// Opaque report reader prepared from one repository and exact Ref snapshot.
///
/// [`Self::prepare`] performs one bounded full-inventory fsck followed by one
/// bounded in-memory Projection rebuild. Every later [`Self::report`] call
/// reuses both results, so a transport can render several creator sessions
/// without repeating either store-wide operation. The reader remains bound to
/// the borrowed repository and snapshot; callers cannot construct or mutate
/// its private prepared state.
///
/// CAS is assumed to follow the repository's cooperative append-only/no-GC
/// model for the lifetime of the reader. Objects or Tombstones appended later
/// are intentionally visible only to a newly prepared reader.
#[must_use = "a prepared creator report reader has not produced any reports"]
pub struct PreparedCreatorReportReader<'source> {
    repository: &'source Repository,
    snapshot: &'source RefSnapshot,
    projection: SqliteProjectionStore,
    fsck_objects: usize,
    projection_source_fingerprint: String,
}

impl<'source> PreparedCreatorReportReader<'source> {
    /// Run the bounded integrity and Projection preparation used by all
    /// reports returned from this reader.
    pub fn prepare(
        repository: &'source Repository,
        snapshot: &'source RefSnapshot,
    ) -> Result<Self> {
        Self::prepare_with_limits(repository, snapshot, CREATOR_FSCK_LIMITS)
    }

    /// Prepare one reusable reader and render its first session report.
    ///
    /// Unlike calling [`Self::prepare`] followed by [`Self::report`], this
    /// entry point preserves the compatibility API's exact first-session
    /// precedence: session syntax, bounded fsck, session Ref shape and lineage,
    /// then the one eager Projection rebuild and report rendering. Subsequent
    /// [`Self::report`] calls reuse the returned reader's fsck and Projection.
    pub fn prepare_with_report(
        repository: &'source Repository,
        snapshot: &'source RefSnapshot,
        session: &str,
    ) -> Result<(Self, CreatorSnapshotReport)> {
        Self::prepare_with_report_and_limits(repository, snapshot, session, CREATOR_FSCK_LIMITS)
    }

    fn prepare_with_limits(
        repository: &'source Repository,
        snapshot: &'source RefSnapshot,
        fsck_limits: FsckLimits,
    ) -> Result<Self> {
        let verification = verify_creator_report_snapshot(repository, snapshot, fsck_limits)?;
        Self::from_verification(verification)
    }

    fn prepare_with_report_and_limits(
        repository: &'source Repository,
        snapshot: &'source RefSnapshot,
        session: &str,
        fsck_limits: FsckLimits,
    ) -> Result<(Self, CreatorSnapshotReport)> {
        // Preserve the complete legacy order before constructing the opaque
        // reader: argument validation, bounded fsck, session/ref/lineage
        // validation, then Projection construction and rendering.
        validate_session(session)?;
        let verification = verify_creator_report_snapshot(repository, snapshot, fsck_limits)?;
        let prepared_session = prepare_creator_report_session(repository, snapshot, session)?;
        let reader = Self::from_verification(verification)?;
        let report = reader.render_report(session, prepared_session)?;
        Ok((reader, report))
    }

    fn from_verification(verification: CreatorReportVerification<'source>) -> Result<Self> {
        let CreatorReportVerification {
            repository,
            snapshot,
            fsck_objects,
        } = verification;
        let mut projection = SqliteProjectionStore::open_in_memory()?;
        let rebuild = projection.rebuild_with_limits(
            repository.objects(),
            snapshot,
            ProjectionLimits::default(),
        )?;
        Ok(Self {
            repository,
            snapshot,
            projection,
            fsck_objects,
            projection_source_fingerprint: rebuild.metadata.source_fingerprint,
        })
    }

    /// Build one creator-session report from the already prepared snapshot.
    ///
    /// Session-name, missing-session, incomplete-session, lineage, and report
    /// validation retain the same error variants as
    /// [`creator_report_from_snapshot`].
    pub fn report(&self, session: &str) -> Result<CreatorSnapshotReport> {
        validate_session(session)?;
        self.report_validated(session)
    }

    fn report_validated(&self, session: &str) -> Result<CreatorSnapshotReport> {
        let prepared = prepare_creator_report_session(self.repository, self.snapshot, session)?;
        self.render_report(session, prepared)
    }

    fn render_report(
        &self,
        session: &str,
        prepared: PreparedCreatorReportSession,
    ) -> Result<CreatorSnapshotReport> {
        let repository = self.repository;
        let projection = &self.projection;
        let PreparedCreatorReportSession {
            decision_ref,
            proposal_ref,
            decision_head,
            proposal_head,
            ids,
            lineage,
        } = prepared;
        let ReportLineage {
            disposition,
            rationale,
            ai_activity_oid,
            base_head,
            base_snapshot,
            proposal_snapshot,
            decision_snapshot,
            comparison: comparison_pointers,
        } = lineage;

        let report_scope = RefScope::names([decision_ref.clone(), proposal_ref.clone()]);
        let timeline = projection.subject_timeline(&ids.subject, None, &report_scope)?;
        let original_observation = timeline
            .iter()
            .find(|entry| entry.entity_id == ids.original_observation)
            .ok_or_else(|| {
                CreatorError::ReportInvalid("original Observation is absent from timeline".into())
            })?;
        let current_observation = timeline
            .iter()
            .find(|entry| entry.entity_id == ids.current_observation)
            .ok_or_else(|| {
                CreatorError::ReportInvalid("current Observation is absent from timeline".into())
            })?;
        let ai_activity = timeline
            .iter()
            .find(|entry| entry.entity_id == ids.ai_activity)
            .ok_or_else(|| {
                CreatorError::ReportInvalid("AI Activity is absent from timeline".into())
            })?;
        if ai_activity.oid != ai_activity_oid {
            return Err(CreatorError::ReportInvalid(
                "timeline AI Activity does not match the current proposal transition".into(),
            ));
        }
        let original_blob_oid = role_oid(
            object_field(
                &read_json(repository, &original_observation.oid)?,
                "payload",
                "original Observation payload",
            )?,
            "media_refs",
            "primary",
        )?;
        let current_blob_oid = role_oid(
            object_field(
                &read_json(repository, &current_observation.oid)?,
                "payload",
                "current Observation payload",
            )?,
            "media_refs",
            "primary",
        )?;
        let ai_output_blob_oid = role_oid(
            object_field(
                &read_json(repository, &ai_activity.oid)?,
                "payload",
                "AI Activity payload",
            )?,
            "output_refs",
            "proposal",
        )?;
        let comparison = comparison_pointers
            .as_ref()
            .map(|pointers| {
                validate_comparison_report(
                    repository,
                    projection,
                    &report_scope,
                    &ids,
                    pointers,
                    &original_observation.oid,
                    &current_observation.oid,
                    &original_blob_oid,
                    &current_blob_oid,
                    &[decision_ref.as_str(), proposal_ref.as_str()],
                )
            })
            .transpose()?;

        let timeline = timeline
            .into_iter()
            .map(|entry| CreatorTimelineEntry {
                oid: entry.oid,
                stage: timeline_stage(&entry.entity_id, &ids),
                kind: match entry.kind {
                    TimelineRecordKind::Observation => "observation",
                    TimelineRecordKind::Activity => "activity",
                },
                entity_id: entry.entity_id,
                ordering_time: entry.ordering_time,
                time_basis: timeline_time_basis(entry.time_basis),
                reachable_from: entry.reachable_from,
            })
            .collect();

        Ok(CreatorSnapshotReport {
            report: CreatorReport {
                session: session.to_owned(),
                project_id: ids.project,
                subject_id: ids.subject,
                creator_id: ids.creator,
                agent_id: ids.agent,
                decision_ref,
                proposal_ref,
                decision_head,
                proposal_head,
                base_head,
                base_snapshot,
                proposal_snapshot,
                decision_snapshot,
                disposition,
                selected_ai_output: disposition == CreatorDisposition::Adopt,
                rationale,
                original_blob_oid,
                current_blob_oid,
                ai_output_blob_oid,
                comparison,
                timeline,
                fsck_objects: self.fsck_objects,
            },
            projection_source_fingerprint: self.projection_source_fingerprint.clone(),
        })
    }
}

/// Discover creator-owned Ref pairs without rebuilding one Projection per
/// session. Both Refs and a digest-verified decision Commit are required for a
/// Complete summary; all other retained shapes are explicitly incomplete.
pub fn discover_creator_sessions(
    repository: &Repository,
    snapshot: &RefSnapshot,
    max_sessions: usize,
) -> Result<Vec<CreatorSessionSummary>> {
    if max_sessions == 0 {
        return Err(CreatorError::InvalidArgument(
            "max_sessions must be greater than zero".into(),
        ));
    }

    #[derive(Default)]
    struct Heads {
        proposal_ref: Option<String>,
        proposal_head: Option<String>,
        decision_ref: Option<String>,
        decision_head: Option<String>,
    }

    let mut sessions = BTreeMap::<String, Heads>::new();
    for reference in &snapshot.refs {
        let (prefix, is_proposal) = if reference.name.starts_with(&format!("{PROPOSAL_PREFIX}/")) {
            (PROPOSAL_PREFIX, true)
        } else if reference.name.starts_with(&format!("{DECISION_PREFIX}/")) {
            (DECISION_PREFIX, false)
        } else {
            continue;
        };
        let session = reference
            .name
            .strip_prefix(prefix)
            .and_then(|suffix| suffix.strip_prefix('/'))
            .expect("the exact prefix check established a slash suffix");
        validate_session(session).map_err(|_| {
            CreatorError::ReportInvalid(format!(
                "creator Ref {:?} has an invalid session segment",
                reference.name
            ))
        })?;
        if !sessions.contains_key(session) && sessions.len() == max_sessions {
            return Err(CreatorError::ResourceLimit(format!(
                "creator session count exceeds max_sessions {max_sessions}"
            )));
        }
        let heads = sessions.entry(session.to_owned()).or_default();
        if is_proposal {
            heads.proposal_ref = Some(reference.name.clone());
            heads.proposal_head = Some(reference.head.clone());
        } else {
            heads.decision_ref = Some(reference.name.clone());
            heads.decision_head = Some(reference.head.clone());
        }
    }

    sessions
        .into_iter()
        .map(|(session, heads)| {
            let state = match (&heads.proposal_head, &heads.decision_head) {
                (Some(_), Some(decision_head))
                    if read_json(repository, decision_head)?
                        .get("commit_kind")
                        .and_then(JsonValue::as_str)
                        == Some("decision") =>
                {
                    CreatorSessionState::Complete
                }
                _ => CreatorSessionState::Incomplete,
            };
            Ok(CreatorSessionSummary {
                session,
                state,
                proposal_ref: heads.proposal_ref,
                proposal_head: heads.proposal_head,
                decision_ref: heads.decision_ref,
                decision_head: heads.decision_head,
            })
        })
        .collect()
}

struct ReportLineage {
    disposition: CreatorDisposition,
    rationale: Option<String>,
    ai_activity_oid: String,
    base_head: String,
    base_snapshot: String,
    proposal_snapshot: String,
    decision_snapshot: String,
    comparison: Option<ComparisonPointers>,
}

#[derive(Clone, Debug)]
pub(crate) struct ComparisonPointers {
    pub(crate) analysis_oid: String,
    tool_actor_oid: String,
    pub(crate) implementation_oid: String,
    pub(crate) configuration_oid: String,
}

pub(crate) struct BaseSnapshotPointers {
    import_activity_oid: String,
    pub(crate) comparison: Option<ComparisonPointers>,
}

fn load_session_ids(
    repository: &Repository,
    session: &str,
    decision_head: &str,
) -> Result<SessionIds> {
    let decision = read_json(repository, decision_head)?;
    let base_head = single_string_array(&decision, "parents", "creator decision parents")?;
    let base = read_json(repository, base_head)?;
    let base_tree_oid = string_field(&base, "snapshot", "creator base snapshot")?;
    let base_tree = read_json(repository, base_tree_oid)?;
    let entries = base_tree
        .get("entries")
        .and_then(JsonValue::as_object)
        .ok_or_else(|| CreatorError::ReportInvalid("creator base Tree has no entries".into()))?;
    let subject_oid = entries
        .get("subject.json")
        .and_then(JsonValue::as_object)
        .and_then(|entry| entry.get("oid"))
        .and_then(JsonValue::as_str)
        .ok_or_else(|| {
            CreatorError::ReportInvalid(
                "creator base Tree has no subject session-manifest entry".into(),
            )
        })?;
    let subject = read_json(repository, subject_oid)?;
    require_stored_value(
        &subject,
        "record_type",
        "subject",
        "creator Subject record_type",
    )?;
    let manifest = subject
        .get("extensions")
        .and_then(JsonValue::as_object)
        .and_then(|extensions| extensions.get("org.synapsegit.creator-session"))
        .filter(|value| value.is_object())
        .ok_or_else(|| {
            CreatorError::ReportInvalid("creator Subject has no session manifest".into())
        })?;
    require_stored_value(
        manifest,
        "format",
        "synapsegit-creator-session-v1",
        "creator session manifest format",
    )?;
    require_stored_value(
        manifest,
        "session",
        session,
        "creator session manifest name",
    )?;
    let ids = SessionIds {
        creator: manifest_id(manifest, "creator_id")?,
        agent: manifest_id(manifest, "agent_id")?,
        project: manifest_id(manifest, "project_id")?,
        subject: manifest_id(manifest, "subject_id")?,
        series: manifest_id(manifest, "series_id")?,
        original_observation: manifest_id(manifest, "original_observation_id")?,
        current_observation: manifest_id(manifest, "current_observation_id")?,
        import_activity: manifest_id(manifest, "import_activity_id")?,
        policy: manifest_id(manifest, "policy_id")?,
        grant: manifest_id(manifest, "grant_id")?,
        context: manifest_id(manifest, "context_id")?,
        ai_activity: manifest_id(manifest, "ai_activity_id")?,
        feedback: manifest_id(manifest, "feedback_id")?,
    };
    require_stored_value(
        &subject,
        "entity_id",
        &ids.subject,
        "creator Subject entity_id",
    )?;
    Ok(ids)
}

fn manifest_id(manifest: &JsonValue, field: &str) -> Result<String> {
    let value = string_field(manifest, field, "creator session manifest identity")?;
    if !value.starts_with("urn:uuid:") {
        return Err(CreatorError::ReportInvalid(format!(
            "creator session manifest {field} is not an EntityId"
        )));
    }
    Ok(value.to_owned())
}

fn validate_report_lineage(
    repository: &Repository,
    ids: &SessionIds,
    decision_head: &str,
    proposal_head: &str,
) -> Result<ReportLineage> {
    let decision = read_json(repository, decision_head)?;
    require_stored_value(
        &decision,
        "object_type",
        "commit",
        "creator decision object_type",
    )?;
    require_stored_value(
        &decision,
        "commit_kind",
        "decision",
        "creator decision kind",
    )?;
    require_stored_value(
        &decision,
        "author_ref",
        &ids.creator,
        "creator decision author",
    )?;
    require_empty_array(
        &decision,
        "bound_declaration_refs",
        "creator decision bound_declaration_refs",
    )?;
    let base_head = single_string_array(&decision, "parents", "creator decision parents")?;
    let feedback_oid = single_string_array(
        &decision,
        "transition_refs",
        "creator decision transition_refs",
    )?;
    let decision_snapshot = string_field(&decision, "snapshot", "creator decision snapshot")?;

    let base = read_json(repository, base_head)?;
    require_stored_value(&base, "object_type", "commit", "creator base object_type")?;
    require_stored_value(&base, "commit_kind", "checkpoint", "creator base kind")?;
    require_stored_value(&base, "author_ref", &ids.creator, "creator base author")?;
    let base_snapshot = string_field(&base, "snapshot", "creator base snapshot")?;
    let base_pointers = load_base_snapshot_pointers(repository, base_snapshot)?;
    let mut expected_base_transitions = vec![base_pointers.import_activity_oid.as_str()];
    if let Some(comparison) = &base_pointers.comparison {
        expected_base_transitions.push(comparison.analysis_oid.as_str());
    }
    require_string_set(
        &base,
        "transition_refs",
        &expected_base_transitions,
        "creator base transition_refs",
    )?;

    let proposal = read_json(repository, proposal_head)?;
    require_stored_value(
        &proposal,
        "object_type",
        "commit",
        "creator proposal object_type",
    )?;
    require_stored_value(
        &proposal,
        "commit_kind",
        "checkpoint",
        "creator proposal kind",
    )?;
    require_stored_value(
        &proposal,
        "author_ref",
        &ids.agent,
        "creator proposal author",
    )?;
    let proposal_parent = single_string_array(&proposal, "parents", "creator proposal parents")?;
    if proposal_parent != base_head {
        return Err(CreatorError::ReportInvalid(
            "current proposal is not based on the reviewed creator decision parent".into(),
        ));
    }
    let ai_activity_oid = single_string_array(
        &proposal,
        "transition_refs",
        "creator proposal transition_refs",
    )?
    .to_owned();
    let proposal_snapshot = string_field(&proposal, "snapshot", "creator proposal snapshot")?;

    let feedback = read_json(repository, feedback_oid)?;
    require_stored_value(
        &feedback,
        "object_type",
        "record",
        "DecisionFeedback object_type",
    )?;
    require_stored_value(
        &feedback,
        "record_type",
        "decision_feedback",
        "DecisionFeedback record_type",
    )?;
    require_stored_value(
        &feedback,
        "entity_id",
        &ids.feedback,
        "DecisionFeedback entity_id",
    )?;
    require_stored_value(
        &feedback,
        "asserted_by",
        &ids.creator,
        "DecisionFeedback asserted_by",
    )?;
    require_stored_value(
        &feedback,
        "origin",
        "self_declared",
        "DecisionFeedback origin",
    )?;
    let feedback_payload = object_field(&feedback, "payload", "DecisionFeedback payload")?;
    require_stored_value(
        feedback_payload,
        "proposal_ref",
        proposal_head,
        "DecisionFeedback proposal_ref",
    )?;
    let disposition = CreatorDisposition::from_protocol(string_field(
        feedback_payload,
        "disposition",
        "DecisionFeedback disposition",
    )?)?;
    let expected_snapshot = if disposition == CreatorDisposition::Adopt {
        proposal_snapshot
    } else {
        base_snapshot
    };
    if decision_snapshot != expected_snapshot {
        return Err(CreatorError::ReportInvalid(format!(
            "decision snapshot does not match the {disposition:?} disposition"
        )));
    }

    let activity = read_json(repository, &ai_activity_oid)?;
    require_stored_value(
        &activity,
        "object_type",
        "record",
        "AI Activity object_type",
    )?;
    require_stored_value(
        &activity,
        "record_type",
        "activity",
        "AI Activity record_type",
    )?;
    require_stored_value(
        &activity,
        "entity_id",
        &ids.ai_activity,
        "AI Activity entity_id",
    )?;
    require_stored_value(
        &activity,
        "asserted_by",
        &ids.agent,
        "AI Activity asserted_by",
    )?;
    let activity_payload = object_field(&activity, "payload", "AI Activity payload")?;
    require_stored_value(
        activity_payload,
        "activity_kind",
        "ai_run",
        "AI Activity kind",
    )?;
    let ai_run = object_field(activity_payload, "ai_run", "AI Activity ai_run")?;
    require_stored_value(ai_run, "agent_ref", &ids.agent, "AI Activity agent_ref")?;
    require_stored_value(
        ai_run,
        "responsible_principal_ref",
        &ids.creator,
        "AI Activity responsible principal",
    )?;
    require_stored_value(ai_run, "status", "proposal_ready", "AI Activity status")?;

    Ok(ReportLineage {
        disposition,
        rationale: feedback_payload
            .get("human_rationale")
            .and_then(JsonValue::as_str)
            .map(str::to_owned),
        ai_activity_oid,
        base_head: base_head.to_owned(),
        base_snapshot: base_snapshot.to_owned(),
        proposal_snapshot: proposal_snapshot.to_owned(),
        decision_snapshot: decision_snapshot.to_owned(),
        comparison: base_pointers.comparison,
    })
}

pub(crate) fn load_base_snapshot_pointers(
    repository: &Repository,
    base_snapshot: &str,
) -> Result<BaseSnapshotPointers> {
    let tree = read_json(repository, base_snapshot)?;
    require_stored_value(
        &tree,
        "object_type",
        "tree",
        "creator base Tree object_type",
    )?;
    let entries = tree
        .get("entries")
        .and_then(JsonValue::as_object)
        .ok_or_else(|| CreatorError::ReportInvalid("creator base Tree has no entries".into()))?;
    let import_activity_oid = required_tree_entry_oid(
        entries,
        "image-import.activity.json",
        "record",
        "creator image import Activity",
    )?;
    let comparison_parts = [
        optional_tree_entry_oid(entries, COMPARISON_ANALYSIS_ENTRY, "record")?,
        optional_tree_entry_oid(entries, COMPARISON_TOOL_ENTRY, "record")?,
        optional_tree_entry_oid(entries, COMPARISON_IMPLEMENTATION_ENTRY, "blob")?,
        optional_tree_entry_oid(entries, COMPARISON_CONFIGURATION_ENTRY, "blob")?,
    ];
    let comparison = if comparison_parts.iter().all(Option::is_none) {
        None
    } else if comparison_parts.iter().all(Option::is_some) {
        let [
            analysis_oid,
            tool_actor_oid,
            implementation_oid,
            configuration_oid,
        ] = comparison_parts.map(Option::unwrap);
        Some(ComparisonPointers {
            analysis_oid,
            tool_actor_oid,
            implementation_oid,
            configuration_oid,
        })
    } else {
        return Err(CreatorError::ReportInvalid(
            "creator base Tree has incomplete byte-identity evidence entries".into(),
        ));
    };
    Ok(BaseSnapshotPointers {
        import_activity_oid,
        comparison,
    })
}

#[allow(clippy::too_many_arguments)]
fn validate_comparison_report(
    repository: &Repository,
    projection: &SqliteProjectionStore,
    scope: &RefScope,
    ids: &SessionIds,
    pointers: &ComparisonPointers,
    base_observation_oid: &str,
    target_observation_oid: &str,
    base_media_oid: &str,
    target_media_oid: &str,
    expected_refs: &[&str],
) -> Result<CreatorComparisonReport> {
    let tool_id = related_entity_id(&ids.series, "observation-tool");
    let analysis_id = related_entity_id(&ids.series, "byte-identity-analysis");
    if tool_id == ids.agent {
        return Err(CreatorError::ReportInvalid(
            "comparison software tool must be distinct from the AI agent".into(),
        ));
    }
    let tool = read_json(repository, &pointers.tool_actor_oid)?;
    require_stored_value(
        &tool,
        "object_type",
        "record",
        "comparison tool object_type",
    )?;
    require_stored_value(&tool, "record_type", "actor", "comparison tool record_type")?;
    require_stored_value(&tool, "entity_id", &tool_id, "comparison tool entity_id")?;
    require_stored_value(
        &tool,
        "asserted_by",
        &ids.creator,
        "comparison tool asserted_by",
    )?;
    require_stored_value(&tool, "origin", "tool_recorded", "comparison tool origin")?;
    let tool_payload = object_field(&tool, "payload", "comparison tool payload")?;
    require_stored_value(
        tool_payload,
        "actor_kind",
        "software_tool",
        "comparison tool actor_kind",
    )?;

    let lineage = projection.analysis_lineage(&pointers.analysis_oid, scope)?;
    if lineage.entity_id != analysis_id
        || lineage.asserted_by != tool_id
        || lineage.analysis_kind != "byte_identity"
        || lineage.comparison_kind != "temporal_observation"
        || lineage.status != "succeeded"
        || lineage.comparability != "partial"
    {
        return Err(CreatorError::ReportInvalid(
            "byte-identity AnalysisResult identity or conservative status is invalid".into(),
        ));
    }
    if lineage.adapter.id != BYTE_IDENTITY_ADAPTER_ID
        || lineage.adapter.version != BYTE_IDENTITY_ADAPTER_VERSION
        || lineage.adapter.determinism != AdapterDeterminism::Deterministic
        || lineage.adapter.seed.is_some()
    {
        return Err(CreatorError::ReportInvalid(
            "byte-identity AnalysisResult adapter declaration is invalid".into(),
        ));
    }
    let expected_implementation_oid = byte_identity_implementation_oid();
    let expected_configuration_oid = byte_identity_configuration_oid();
    if pointers.implementation_oid != expected_implementation_oid
        || pointers.configuration_oid != expected_configuration_oid
        || lineage.adapter.implementation.oid != expected_implementation_oid
        || lineage.adapter.configuration.oid != expected_configuration_oid
        || lineage.adapter.implementation.kind != ObjectKind::Blob
        || lineage.adapter.configuration.kind != ObjectKind::Blob
        || lineage.adapter.implementation.availability != ObjectAvailability::Present
        || lineage.adapter.configuration.availability != ObjectAvailability::Present
    {
        return Err(CreatorError::ReportInvalid(
            "byte-identity implementation or configuration evidence is invalid".into(),
        ));
    }
    let expected_inputs = [
        (0, "base_observation", base_observation_oid),
        (1, "target_observation", target_observation_oid),
    ];
    if lineage.inputs.len() != expected_inputs.len()
        || !lineage
            .inputs
            .iter()
            .zip(expected_inputs)
            .all(|(actual, (ordinal, role, oid))| {
                actual.ordinal == ordinal
                    && actual.role == role
                    && actual.object.oid == oid
                    && actual.object.kind == ObjectKind::Record
                    && actual.object.availability == ObjectAvailability::Present
            })
        || !lineage.transforms.is_empty()
        || !lineage.derived_blobs.is_empty()
        || !lineage.masks.is_empty()
        || lineage.replay_readiness != AnalysisReplayReadiness::Ready
    {
        return Err(CreatorError::ReportInvalid(
            "byte-identity AnalysisResult ordered lineage is invalid".into(),
        ));
    }
    let mut expected_reachable_from = expected_refs
        .iter()
        .map(|value| (*value).to_owned())
        .collect::<Vec<_>>();
    expected_reachable_from.sort();
    if lineage.reachable_from != expected_reachable_from {
        return Err(CreatorError::ReportInvalid(
            "byte-identity AnalysisResult is not reachable from both creator Refs".into(),
        ));
    }

    let analysis = read_json(repository, &pointers.analysis_oid)?;
    require_object_keys(
        &analysis,
        &[
            "object_type",
            "schema_version",
            "record_type",
            "entity_id",
            "recorded_at",
            "asserted_by",
            "origin",
            "source_refs",
            "payload",
            "extensions",
        ],
        "byte-identity AnalysisResult envelope",
    )?;
    require_stored_value(
        &analysis,
        "object_type",
        "record",
        "byte-identity object_type",
    )?;
    require_stored_value(
        &analysis,
        "schema_version",
        SCHEMA_VERSION,
        "byte-identity schema_version",
    )?;
    require_stored_value(
        &analysis,
        "record_type",
        "analysis_result",
        "byte-identity record_type",
    )?;
    require_stored_value(&analysis, "origin", "tool_recorded", "byte-identity origin")?;
    let source_refs = analysis.get("source_refs").ok_or_else(|| {
        CreatorError::ReportInvalid("byte-identity source_refs are absent".into())
    })?;
    require_role_ref(
        source_refs,
        "base_observation",
        base_observation_oid,
        "byte-identity source_refs",
    )?;
    require_role_ref(
        source_refs,
        "target_observation",
        target_observation_oid,
        "byte-identity source_refs",
    )?;
    if source_refs.as_array().map(Vec::len) != Some(2) {
        return Err(CreatorError::ReportInvalid(
            "byte-identity source_refs must contain exactly two entries".into(),
        ));
    }
    let payload = object_field(&analysis, "payload", "byte-identity payload")?;
    require_object_keys(
        payload,
        &[
            "analysis_kind",
            "comparison_kind",
            "inputs",
            "adapter",
            "status",
            "comparability",
            "reason_codes",
            "derived_blob_refs",
            "metrics",
            "warnings",
            "limitations",
        ],
        "byte-identity payload",
    )?;
    let adapter = object_field(payload, "adapter", "byte-identity adapter")?;
    require_object_keys(
        adapter,
        &[
            "id",
            "version",
            "implementation_digest",
            "configuration_digest",
            "determinism",
        ],
        "byte-identity adapter",
    )?;
    require_string_set(
        payload,
        "reason_codes",
        &[
            "byte_identity_only",
            "capture_profile_imported",
            "capture_time_unknown",
        ],
        "byte-identity reason_codes",
    )?;
    let reason_codes = string_array_field(payload, "reason_codes", "byte-identity reason_codes")?;
    let warnings = string_array_field(payload, "warnings", "byte-identity warnings")?;
    let limitations = string_array_field(payload, "limitations", "byte-identity limitations")?;
    let extensions = object_field(&analysis, "extensions", "byte-identity extensions")?;
    require_object_keys(
        extensions,
        &["org.synapsegit.observation-byte-identity"],
        "byte-identity extensions",
    )?;
    let evidence = extensions
        .get("org.synapsegit.observation-byte-identity")
        .filter(|value| value.is_object())
        .ok_or_else(|| {
            CreatorError::ReportInvalid("byte-identity evidence extension is absent".into())
        })?;
    require_object_keys(
        evidence,
        &["format", "outcome", "base_media_ref", "target_media_ref"],
        "byte-identity evidence",
    )?;
    require_stored_value(
        evidence,
        "format",
        "synapsegit-observation-byte-identity-v1",
        "byte-identity evidence format",
    )?;
    require_stored_value(
        evidence,
        "base_media_ref",
        base_media_oid,
        "byte-identity base media",
    )?;
    require_stored_value(
        evidence,
        "target_media_ref",
        target_media_oid,
        "byte-identity target media",
    )?;
    let outcome = string_field(evidence, "outcome", "byte-identity outcome")?;
    let expected_outcome = if base_media_oid == target_media_oid {
        "identical"
    } else {
        "different"
    };
    if outcome != expected_outcome {
        return Err(CreatorError::ReportInvalid(format!(
            "byte-identity outcome is {outcome:?}, expected {expected_outcome:?}"
        )));
    }
    let expected_warning = if outcome == "identical" {
        "Identical Blob bytes do not establish that the observed physical subject was unchanged."
    } else {
        "Different Blob bytes do not establish visual or physical change."
    };
    if warnings != [expected_warning] {
        return Err(CreatorError::ReportInvalid(
            "byte-identity warning does not preserve the conservative interpretation".into(),
        ));
    }
    if limitations
        != [
            "This adapter compares verified Blob OIDs only and does not decode media, inspect pixels, register viewpoints, or infer appearance or physical change.",
            "The implementation digest covers the semantic Rust source files and crate manifest, not Cargo.lock, transitive dependency sources, compiler, target, operating system, or full runtime environment.",
        ]
    {
        return Err(CreatorError::ReportInvalid(
            "byte-identity limitations do not match the known adapter contract".into(),
        ));
    }
    validate_byte_identity_metric(payload, outcome == "identical")?;

    Ok(CreatorComparisonReport {
        analysis_oid: pointers.analysis_oid.clone(),
        tool_id,
        tool_actor_oid: pointers.tool_actor_oid.clone(),
        adapter_id: lineage.adapter.id,
        adapter_version: lineage.adapter.version,
        implementation_oid: pointers.implementation_oid.clone(),
        configuration_oid: pointers.configuration_oid.clone(),
        status: lineage.status,
        comparability: lineage.comparability,
        outcome: outcome.to_owned(),
        reason_codes,
        warnings,
        base_observation_oid: base_observation_oid.to_owned(),
        target_observation_oid: target_observation_oid.to_owned(),
        base_media_oid: base_media_oid.to_owned(),
        target_media_oid: target_media_oid.to_owned(),
        replay_ready: true,
        reachable_from: lineage.reachable_from,
    })
}

fn require_role_ref(value: &JsonValue, role: &str, expected: &str, label: &str) -> Result<()> {
    let entries = value
        .as_array()
        .ok_or_else(|| CreatorError::ReportInvalid(format!("{label} is invalid")))?;
    let mut matches = entries
        .iter()
        .filter(|entry| entry.get("role").and_then(JsonValue::as_str) == Some(role));
    let actual = matches
        .next()
        .and_then(|entry| entry.get("oid"))
        .and_then(JsonValue::as_str)
        .ok_or_else(|| CreatorError::ReportInvalid(format!("{label} has no {role:?} OID")))?;
    if matches.next().is_some() || actual != expected {
        return Err(CreatorError::ReportInvalid(format!(
            "{label} {role:?} OID does not match the creator session"
        )));
    }
    Ok(())
}

fn string_array_field(value: &JsonValue, field: &str, label: &str) -> Result<Vec<String>> {
    value
        .get(field)
        .and_then(JsonValue::as_array)
        .ok_or_else(|| CreatorError::ReportInvalid(format!("{label} is missing or invalid")))?
        .iter()
        .map(|value| {
            value
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| CreatorError::ReportInvalid(format!("{label} is not a string set")))
        })
        .collect()
}

pub(crate) fn validate_byte_identity_metric(payload: &JsonValue, identical: bool) -> Result<()> {
    let metrics = payload
        .get("metrics")
        .and_then(JsonValue::as_object)
        .ok_or_else(|| CreatorError::ReportInvalid("byte-identity metrics are invalid".into()))?;
    require_object_keys(
        &JsonValue::Object(metrics.clone()),
        &["byte_identical"],
        "byte-identity metrics",
    )?;
    let metric = metrics
        .get("byte_identical")
        .filter(|metric| metric.is_object())
        .ok_or_else(|| {
            CreatorError::ReportInvalid("byte-identity metric is missing or invalid".into())
        })?;
    require_object_keys(
        metric,
        &["mantissa", "scale", "unit"],
        "byte-identity metric",
    )?;
    let expected_mantissa = if identical { "1" } else { "0" };
    require_stored_value(
        metric,
        "mantissa",
        expected_mantissa,
        "byte-identity metric mantissa",
    )?;
    require_stored_value(metric, "unit", "unitless", "byte-identity metric unit")?;
    if metric.get("scale").and_then(JsonValue::as_i64) != Some(0) {
        return Err(CreatorError::ReportInvalid(
            "byte-identity metric scale is invalid".into(),
        ));
    }
    Ok(())
}

fn require_object_keys(value: &JsonValue, expected: &[&str], label: &str) -> Result<()> {
    let object = value
        .as_object()
        .ok_or_else(|| CreatorError::ReportInvalid(format!("{label} is not an object")))?;
    let mut actual = object.keys().map(String::as_str).collect::<Vec<_>>();
    let mut expected = expected.to_vec();
    actual.sort_unstable();
    expected.sort_unstable();
    if actual == expected {
        Ok(())
    } else {
        Err(CreatorError::ReportInvalid(format!(
            "{label} fields do not match the known adapter contract"
        )))
    }
}

fn optional_tree_entry_oid(
    entries: &JsonMap<String, JsonValue>,
    name: &str,
    expected_kind: &str,
) -> Result<Option<String>> {
    let Some(entry) = entries.get(name) else {
        return Ok(None);
    };
    let entry = entry.as_object().ok_or_else(|| {
        CreatorError::ReportInvalid(format!("creator base Tree entry {name:?} is invalid"))
    })?;
    let kind = entry
        .get("entry_kind")
        .and_then(JsonValue::as_str)
        .ok_or_else(|| {
            CreatorError::ReportInvalid(format!(
                "creator base Tree entry {name:?} has no entry_kind"
            ))
        })?;
    if kind != expected_kind {
        return Err(CreatorError::ReportInvalid(format!(
            "creator base Tree entry {name:?} is {kind:?}, expected {expected_kind:?}"
        )));
    }
    let oid = entry
        .get("oid")
        .and_then(JsonValue::as_str)
        .ok_or_else(|| {
            CreatorError::ReportInvalid(format!("creator base Tree entry {name:?} has no OID"))
        })?;
    Ok(Some(oid.to_owned()))
}

fn required_tree_entry_oid(
    entries: &JsonMap<String, JsonValue>,
    name: &str,
    expected_kind: &str,
    label: &str,
) -> Result<String> {
    optional_tree_entry_oid(entries, name, expected_kind)?
        .ok_or_else(|| CreatorError::ReportInvalid(format!("{label} entry is absent")))
}

fn require_stored_value(value: &JsonValue, field: &str, expected: &str, label: &str) -> Result<()> {
    let actual = string_field(value, field, label)?;
    if actual == expected {
        Ok(())
    } else {
        Err(CreatorError::ReportInvalid(format!(
            "{label} is {actual:?}, expected {expected:?}"
        )))
    }
}

fn single_string_array<'a>(value: &'a JsonValue, field: &str, label: &str) -> Result<&'a str> {
    let values = value
        .get(field)
        .and_then(JsonValue::as_array)
        .ok_or_else(|| CreatorError::ReportInvalid(format!("{label} is missing or invalid")))?;
    if values.len() != 1 {
        return Err(CreatorError::ReportInvalid(format!(
            "{label} must contain exactly one value"
        )));
    }
    values[0]
        .as_str()
        .ok_or_else(|| CreatorError::ReportInvalid(format!("{label} value is not a string")))
}

fn require_empty_array(value: &JsonValue, field: &str, label: &str) -> Result<()> {
    let values = value
        .get(field)
        .and_then(JsonValue::as_array)
        .ok_or_else(|| CreatorError::ReportInvalid(format!("{label} is missing or invalid")))?;
    if values.is_empty() {
        Ok(())
    } else {
        Err(CreatorError::ReportInvalid(format!(
            "{label} must be empty"
        )))
    }
}

fn require_string_set(
    value: &JsonValue,
    field: &str,
    expected: &[&str],
    label: &str,
) -> Result<()> {
    let values = value
        .get(field)
        .and_then(JsonValue::as_array)
        .ok_or_else(|| CreatorError::ReportInvalid(format!("{label} is missing or invalid")))?;
    let mut actual = values
        .iter()
        .map(|value| {
            value.as_str().map(str::to_owned).ok_or_else(|| {
                CreatorError::ReportInvalid(format!("{label} contains a non-string value"))
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let mut expected = expected
        .iter()
        .map(|value| (*value).to_owned())
        .collect::<Vec<_>>();
    actual.sort();
    expected.sort();
    if actual == expected {
        Ok(())
    } else {
        Err(CreatorError::ReportInvalid(format!(
            "{label} does not match the creator session contract"
        )))
    }
}

fn role_oid(payload: &JsonValue, field: &str, role: &str) -> Result<String> {
    let entries = payload
        .get(field)
        .and_then(JsonValue::as_array)
        .ok_or_else(|| CreatorError::ReportInvalid(format!("{field} is missing or invalid")))?;
    let mut matches = entries
        .iter()
        .filter(|entry| entry.get("role").and_then(JsonValue::as_str) == Some(role));
    let first = matches
        .next()
        .and_then(|entry| entry.get("oid"))
        .and_then(JsonValue::as_str)
        .ok_or_else(|| CreatorError::ReportInvalid(format!("{field} has no {role:?} OID")))?;
    if matches.next().is_some() {
        return Err(CreatorError::ReportInvalid(format!(
            "{field} contains duplicate {role:?} roles"
        )));
    }
    Ok(first.to_owned())
}

fn timeline_time_basis(basis: TimelineTimeBasis) -> &'static str {
    match basis {
        TimelineTimeBasis::ObservationCaptureInstant => "observation_capture_instant",
        TimelineTimeBasis::ObservationCaptureInterval => "observation_capture_interval",
        TimelineTimeBasis::ObservationRecordedAtFallback => "observation_recorded_at_fallback",
        TimelineTimeBasis::ActivityValidInstant => "activity_valid_instant",
        TimelineTimeBasis::ActivityValidInterval => "activity_valid_interval",
        TimelineTimeBasis::ActivityRecordedAtFallback => "activity_recorded_at_fallback",
    }
}

fn timeline_stage(entity_id: &str, ids: &SessionIds) -> &'static str {
    if entity_id == ids.original_observation {
        "original_observation"
    } else if entity_id == ids.current_observation {
        "current_observation"
    } else if entity_id == ids.import_activity {
        "image_import"
    } else if entity_id == ids.ai_activity {
        "ai_proposal"
    } else {
        "other"
    }
}
