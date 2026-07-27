use crate::fsck::{prospective_fsck, reserve_fsck_capacity};
use crate::io::{put_file, put_json, read_json};
use crate::records::{
    actor_record, ai_activity_record, ai_actor_record, commit, context_record, feedback_record,
    grant_record, import_activity_record, imported_capture_profile_record, manifest_tree,
    observation_record, observation_tool_actor_record, policy_record, subject_record,
};
use crate::time::RecordingClock;
use crate::{
    AnalysisComparability, AnalysisStatus, ByteIdentityOutcome, CreatorBeginOptions,
    CreatorComparisonReport, CreatorDecisionOptions, CreatorDisposition, CreatorError,
    CreatorPendingDecisionState, CreatorPendingReceipt, CreatorRunOptions, CreatorRunReceipt,
    Result,
};
use serde_json::{Map as JsonMap, Value as JsonValue, json};
use sha2::{Digest, Sha256};
use std::fmt;
use std::fs::File;
use std::path::{Path, PathBuf};
use synapse_application::{
    AdmittedProposalHandle, AiAuthorityProfileConfig, AiExecutionContext, AiExecutor, Application,
    AuthenticatedSession, AuthenticationFailure, Authenticator, ExecutedAiProposal,
    ExecutionFailure, HumanAuthorityProfileConfig, HumanAuthorityProfileHandle,
    HumanDecisionCandidate, ProjectSelector, RegisteredProject,
};
use synapse_core::{
    AiCapability, AiSideEffectClass, FsckLimits, Repository, SystemAuthorizationClock,
    TombstoneScanLimits,
};
use synapse_observation::{
    BYTE_IDENTITY_ADAPTER_ID, BYTE_IDENTITY_ADAPTER_VERSION, ByteIdentityComparisonRequest,
    record_byte_identity_comparison,
};
use synapse_sqlite::{RefUpdate, ReflogMetadata};

pub(crate) const DECISION_PREFIX: &str = "decision/creator";
pub(crate) const PROPOSAL_PREFIX: &str = "proposal/creator-agent";
const PILOT_PERMIT_TTL_NANOS: i128 = 60_000_000_000;
pub(crate) const PILOT_MAX_OUTPUT_BYTES: i64 = 1_073_741_824;
const AGENT_CREDENTIAL: &str = "local-creator-agent";
const HUMAN_CREDENTIAL: &str = "local-creator-human";
pub(crate) const COMPARISON_TOOL_ENTRY: &str = "byte-identity.tool.actor.json";
pub(crate) const COMPARISON_ANALYSIS_ENTRY: &str = "original-current.byte-identity.analysis.json";
pub(crate) const COMPARISON_IMPLEMENTATION_ENTRY: &str = "byte-identity.implementation";
pub(crate) const COMPARISON_CONFIGURATION_ENTRY: &str = "byte-identity.configuration";
/// Maximum Ref records retained by one creator integrity check.
pub const CREATOR_FSCK_MAX_REF_ROOTS: usize = 10_000;
/// Maximum complete CAS inventory retained by one creator integrity check.
pub const CREATOR_FSCK_MAX_OBJECTS: usize = 25_000;
/// Maximum cumulative raw CAS bytes read by the inventory verification phase.
pub const CREATOR_FSCK_MAX_OBJECT_BYTES: u64 = 4 * 1024 * 1024 * 1024;
/// Maximum cumulative closure nodes visited across distinct current heads.
pub const CREATOR_FSCK_MAX_CLOSURE_NODES: usize = 250_000;
/// Maximum cumulative closure edges visited across distinct current heads.
pub const CREATOR_FSCK_MAX_CLOSURE_EDGES: usize = 2_500_000;
const CREATOR_FSCK_MAX_TOMBSTONE_RECORDS: usize = 25_000;
const CREATOR_FSCK_MAX_TOMBSTONE_BYTES: u64 = 512 * 1024 * 1024;
pub(crate) const CREATOR_MAX_INPUT_FILE_BYTES: u64 = 64 * 1024 * 1024;
const CREATOR_MAX_INPUT_AGGREGATE_BYTES: u64 = 3 * CREATOR_MAX_INPUT_FILE_BYTES;
/// Number of simultaneously retained localhost reviews whose decisions are
/// covered by creator begin's repository-capacity reservation.
pub const CREATOR_RESERVED_PENDING_DECISIONS: usize = 8;
// begin writes three bounded Blobs plus a fixed-schema graph. The reservation
// is deliberately larger than the current graph and is checked again against
// the exact prospective Ref snapshot before publication.
pub(crate) const CREATOR_BEGIN_RESERVE: CreatorFsckReserve = CreatorFsckReserve {
    ref_roots: 2,
    objects: 32,
    object_bytes: CREATOR_MAX_INPUT_AGGREGATE_BYTES + 8 * 1024 * 1024,
    closure_nodes: 64,
    closure_edges: 256,
    tombstone_records: 24,
    tombstone_bytes: 4 * 1024 * 1024,
};
// A Human decision adds exactly one DecisionFeedback Record and one decision
// Commit. begin reserves a fixed pool of these units so successful proposals
// cannot consume the space needed by the localhost pending-review slots.
pub(crate) const CREATOR_DECISION_RESERVE: CreatorFsckReserve = CreatorFsckReserve {
    ref_roots: 0,
    objects: 2,
    object_bytes: 128 * 1024,
    // DecisionFeedback binds the proposal Commit, so the prospective decision
    // closure re-traverses the fixed proposal-only graph as well as adding the
    // two new objects. Keep a conservative margin over that schema-fixed work.
    closure_nodes: 16,
    closure_edges: 64,
    tombstone_records: 1,
    tombstone_bytes: 64 * 1024,
};
pub(crate) const CREATOR_PENDING_DECISION_POOL_RESERVE: CreatorFsckReserve = CreatorFsckReserve {
    ref_roots: 0,
    objects: CREATOR_DECISION_RESERVE.objects * CREATOR_RESERVED_PENDING_DECISIONS,
    object_bytes: CREATOR_DECISION_RESERVE.object_bytes * CREATOR_RESERVED_PENDING_DECISIONS as u64,
    closure_nodes: CREATOR_DECISION_RESERVE.closure_nodes * CREATOR_RESERVED_PENDING_DECISIONS,
    closure_edges: CREATOR_DECISION_RESERVE.closure_edges * CREATOR_RESERVED_PENDING_DECISIONS,
    tombstone_records: CREATOR_DECISION_RESERVE.tombstone_records
        * CREATOR_RESERVED_PENDING_DECISIONS,
    tombstone_bytes: CREATOR_DECISION_RESERVE.tombstone_bytes
        * CREATOR_RESERVED_PENDING_DECISIONS as u64,
};
pub(crate) const CREATOR_FSCK_LIMITS: FsckLimits = FsckLimits {
    max_ref_roots: CREATOR_FSCK_MAX_REF_ROOTS,
    max_objects: CREATOR_FSCK_MAX_OBJECTS,
    max_object_bytes: CREATOR_FSCK_MAX_OBJECT_BYTES,
    max_closure_nodes: CREATOR_FSCK_MAX_CLOSURE_NODES,
    max_closure_edges: CREATOR_FSCK_MAX_CLOSURE_EDGES,
    tombstone_scan: TombstoneScanLimits {
        max_record_objects: CREATOR_FSCK_MAX_TOMBSTONE_RECORDS,
        max_record_bytes: CREATOR_FSCK_MAX_TOMBSTONE_BYTES,
    },
};

#[derive(Clone, Copy)]
pub(crate) struct CreatorFsckReserve {
    pub(crate) ref_roots: usize,
    pub(crate) objects: usize,
    pub(crate) object_bytes: u64,
    pub(crate) closure_nodes: usize,
    pub(crate) closure_edges: usize,
    pub(crate) tombstone_records: usize,
    pub(crate) tombstone_bytes: u64,
}

type PilotApplication = Application<PilotAuthenticator, PreparedExecutor, SystemAuthorizationClock>;

/// Opaque, same-process authority needed to publish one Human decision.
///
/// This value is intentionally non-Clone and non-serializable. Persisting its
/// visible identifiers does not recreate the admitted-proposal capability
/// held by the exact [`Application`] instance.
#[must_use = "dropping pending creator authority leaves the published proposal incomplete"]
pub struct PendingCreatorSession {
    application: PilotApplication,
    admitted_proposal: AdmittedProposalHandle,
    human_profile: HumanAuthorityProfileHandle,
    selector: ProjectSelector,
    repository_path: PathBuf,
    ids: SessionIds,
    receipt: CreatorPendingReceipt,
    base_tree_oid: String,
    proposal_tree_oid: String,
    byte_identity_outcome: ByteIdentityOutcome,
    comparison_status: AnalysisStatus,
    comparison_comparability: AnalysisComparability,
    recording_clock: RecordingClock,
    decision_state: PendingDecisionState,
}

enum PendingDecisionState {
    Ready,
    Deciding,
    Consumed(Box<CreatorRunReceipt>),
    OutcomeUnknown,
}

impl fmt::Debug for PendingCreatorSession {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PendingCreatorSession")
            .field("session", &self.receipt.session)
            .field("proposal_head", &self.receipt.proposal_head)
            .field("decision_state", &self.decision_state.label())
            .finish_non_exhaustive()
    }
}

impl PendingCreatorSession {
    pub fn receipt(&self) -> &CreatorPendingReceipt {
        &self.receipt
    }

    /// Return the committed receipt even when a later integrity check failed.
    pub fn completed_receipt(&self) -> Option<&CreatorRunReceipt> {
        match &self.decision_state {
            PendingDecisionState::Consumed(receipt) => Some(receipt),
            _ => None,
        }
    }

    /// Report whether a caller may safely attempt a Human decision.
    pub const fn decision_state(&self) -> CreatorPendingDecisionState {
        match &self.decision_state {
            PendingDecisionState::Ready => CreatorPendingDecisionState::Ready,
            PendingDecisionState::Deciding => CreatorPendingDecisionState::Deciding,
            PendingDecisionState::Consumed(_) => CreatorPendingDecisionState::Consumed,
            PendingDecisionState::OutcomeUnknown => CreatorPendingDecisionState::OutcomeUnknown,
        }
    }
}

impl PendingDecisionState {
    const fn label(&self) -> &'static str {
        match self {
            Self::Ready => "ready",
            Self::Deciding => "deciding",
            Self::Consumed(_) => "consumed",
            Self::OutcomeUnknown => "outcome_unknown",
        }
    }
}

/// Publish one local creator proposal and retain its exact Human-review authority.
///
/// Both target Refs must be absent. CAS writes before the base Ref publication
/// are harmless immutable orphans. A failure after publication may leave an
/// incomplete or already-complete live session which this create-only Pilot
/// will not overwrite; callers must inspect it or choose a new session name.
pub fn begin_creator_session(options: &CreatorBeginOptions) -> Result<PendingCreatorSession> {
    begin_creator_session_with_limits(options, CREATOR_FSCK_LIMITS)
}

pub(crate) fn begin_creator_session_with_limits(
    options: &CreatorBeginOptions,
    fsck_limits: FsckLimits,
) -> Result<PendingCreatorSession> {
    validate_begin_metadata(options)?;
    let pending_decision_capacity_limits = reserve_fsck_capacity(
        fsck_limits,
        CREATOR_PENDING_DECISION_POOL_RESERVE,
        "pending decision pool",
    )?;
    let begin_admission_limits = reserve_fsck_capacity(
        pending_decision_capacity_limits,
        CREATOR_BEGIN_RESERVE,
        "begin admission",
    )?;
    let decision_ref = decision_ref(&options.session);
    let proposal_ref = proposal_ref(&options.session);
    let mut repository = Repository::open_with_tombstone_scan_limits(
        &options.repository,
        fsck_limits.tombstone_scan,
    )?;
    let existing_decision = repository.refs().get(&decision_ref)?;
    let existing_proposal = repository.refs().get(&proposal_ref)?;
    if existing_decision.is_some() || existing_proposal.is_some() {
        let complete = match (&existing_decision, &existing_proposal) {
            (Some(decision), Some(_)) => {
                read_json(&repository, &decision.head)?
                    .get("commit_kind")
                    .and_then(JsonValue::as_str)
                    == Some("decision")
            }
            _ => false,
        };
        return Err(if complete {
            CreatorError::SessionExists(options.session.clone())
        } else {
            CreatorError::SessionIncomplete(options.session.clone())
        });
    }
    let preflight = repository.fsck_with_limits(begin_admission_limits)?;
    if !preflight.is_clean() {
        return Err(CreatorError::Integrity(format!(
            "creator session refused an existing repository with {} fsck issue(s)",
            preflight.issues.len()
        )));
    }
    validate_input_files(
        &options.original_image,
        &options.current_image,
        &options.ai_output,
    )?;

    let original_blob_oid = put_file(&repository, &options.original_image)?;
    let current_blob_oid = put_file(&repository, &options.current_image)?;
    let ai_output_blob_oid = put_file(&repository, &options.ai_output)?;
    let mut recording_clock = RecordingClock::default();
    let base_recorded_at = recording_clock.tick()?;
    let ids = SessionIds::fresh()?;
    let original_recorded_at = recording_clock.tick()?;
    let current_recorded_at = recording_clock.tick()?;
    let comparison_recorded_at = recording_clock.tick()?;
    let import_recorded_at = recording_clock.tick()?;
    let capture_profile_id = related_entity_id(&ids.series, "capture-profile");
    let comparison_tool_id = related_entity_id(&ids.series, "observation-tool");
    let comparison_analysis_id = related_entity_id(&ids.series, "byte-identity-analysis");

    let creator_actor_oid = put_json(
        &repository,
        actor_record(
            &ids.creator,
            &ids.creator,
            &base_recorded_at.timestamp,
            "human",
            &options.creator_name,
        ),
    )?;
    let ai_actor_oid = put_json(
        &repository,
        ai_actor_record(&ids.agent, &ids.creator, &base_recorded_at.timestamp),
    )?;
    let policy_oid = put_json(
        &repository,
        policy_record(
            &ids.policy,
            &ids.creator,
            &ids.project,
            &decision_ref,
            &proposal_ref,
            &base_recorded_at.timestamp,
        ),
    )?;
    let grant_oid = put_json(
        &repository,
        grant_record(
            &ids.grant,
            &ids.creator,
            &ids.agent,
            &ids.project,
            &proposal_ref,
            &base_recorded_at.timestamp,
            &base_recorded_at.timestamp,
            &base_recorded_at.after_seconds(86_400)?,
        ),
    )?;
    let subject_oid = put_json(
        &repository,
        subject_record(
            &options.session,
            &ids,
            &capture_profile_id,
            &base_recorded_at.timestamp,
            &options.subject_label,
        ),
    )?;
    let capture_profile_oid = put_json(
        &repository,
        imported_capture_profile_record(
            &capture_profile_id,
            &ids.creator,
            &base_recorded_at.timestamp,
        ),
    )?;
    let original_observation_oid = put_json(
        &repository,
        observation_record(
            &ids.original_observation,
            &ids.creator,
            &ids.subject,
            &ids.series,
            &original_recorded_at.timestamp,
            &original_blob_oid,
            &capture_profile_oid,
        ),
    )?;
    let current_observation_oid = put_json(
        &repository,
        observation_record(
            &ids.current_observation,
            &ids.creator,
            &ids.subject,
            &ids.series,
            &current_recorded_at.timestamp,
            &current_blob_oid,
            &capture_profile_oid,
        ),
    )?;
    let comparison_tool_actor_oid = put_json(
        &repository,
        observation_tool_actor_record(
            &comparison_tool_id,
            &ids.creator,
            &base_recorded_at.timestamp,
        ),
    )?;
    let comparison = record_byte_identity_comparison(
        &repository,
        &ByteIdentityComparisonRequest {
            base_observation_oid: original_observation_oid.clone(),
            target_observation_oid: current_observation_oid.clone(),
            analysis_entity_id: comparison_analysis_id,
            asserted_by: comparison_tool_id.clone(),
            recorded_at: comparison_recorded_at.timestamp.clone(),
        },
    )?;
    let import_activity_oid = put_json(
        &repository,
        import_activity_record(
            &ids.import_activity,
            &ids.creator,
            &ids.subject,
            &import_recorded_at.timestamp,
            &original_blob_oid,
            &current_blob_oid,
        ),
    )?;

    let mut base_entries = JsonMap::new();
    insert_entry(
        &mut base_entries,
        "creator.actor.json",
        "record",
        &creator_actor_oid,
    );
    insert_entry(
        &mut base_entries,
        "agent.actor.json",
        "record",
        &ai_actor_oid,
    );
    insert_entry(
        &mut base_entries,
        COMPARISON_TOOL_ENTRY,
        "record",
        &comparison_tool_actor_oid,
    );
    insert_entry(&mut base_entries, "policy.json", "record", &policy_oid);
    insert_entry(&mut base_entries, "grant.json", "record", &grant_oid);
    insert_entry(&mut base_entries, "subject.json", "record", &subject_oid);
    insert_entry(
        &mut base_entries,
        "capture-profile.json",
        "record",
        &capture_profile_oid,
    );
    insert_entry(
        &mut base_entries,
        "original.observation.json",
        "record",
        &original_observation_oid,
    );
    insert_entry(
        &mut base_entries,
        "current.observation.json",
        "record",
        &current_observation_oid,
    );
    insert_entry(
        &mut base_entries,
        "image-import.activity.json",
        "record",
        &import_activity_oid,
    );
    insert_entry(
        &mut base_entries,
        COMPARISON_ANALYSIS_ENTRY,
        "record",
        &comparison.analysis_oid,
    );
    insert_entry(
        &mut base_entries,
        COMPARISON_IMPLEMENTATION_ENTRY,
        "blob",
        &comparison.implementation_oid,
    );
    insert_entry(
        &mut base_entries,
        COMPARISON_CONFIGURATION_ENTRY,
        "blob",
        &comparison.configuration_oid,
    );
    insert_entry(
        &mut base_entries,
        "original.image",
        "blob",
        &original_blob_oid,
    );
    insert_entry(
        &mut base_entries,
        "current.image",
        "blob",
        &current_blob_oid,
    );
    let base_tree_oid = put_json(&repository, manifest_tree(base_entries.clone()))?;
    let base_transitions = vec![import_activity_oid.clone(), comparison.analysis_oid.clone()];
    let base_head = put_json(
        &repository,
        commit(
            "checkpoint",
            &[],
            &base_tree_oid,
            &base_transitions,
            &ids.creator,
            &import_recorded_at.timestamp,
            "Creator images imported and observed",
        ),
    )?;
    let ai_recorded_at = recording_clock.tick()?;
    let context_oid = put_json(
        &repository,
        context_record(
            &ids.context,
            &ids.creator,
            &ids.subject,
            &base_head,
            &decision_ref,
            &policy_oid,
            &grant_oid,
            &ai_recorded_at.timestamp,
        ),
    )?;
    let ai_activity_oid = put_json(
        &repository,
        ai_activity_record(
            &ids.ai_activity,
            &ids.agent,
            &ids.creator,
            &ids.subject,
            &ai_recorded_at.timestamp,
            &context_oid,
            &grant_oid,
            &current_blob_oid,
            &ai_output_blob_oid,
        ),
    )?;
    let mut proposal_entries = base_entries;
    insert_entry(
        &mut proposal_entries,
        "ai.context.json",
        "record",
        &context_oid,
    );
    insert_entry(
        &mut proposal_entries,
        "ai-run.activity.json",
        "record",
        &ai_activity_oid,
    );
    insert_entry(
        &mut proposal_entries,
        "ai-proposal.image",
        "blob",
        &ai_output_blob_oid,
    );
    let proposal_tree_oid = put_json(&repository, manifest_tree(proposal_entries))?;
    let proposal_head = put_json(
        &repository,
        commit(
            "checkpoint",
            slice(&base_head),
            &proposal_tree_oid,
            slice(&ai_activity_oid),
            &ids.agent,
            &ai_recorded_at.timestamp,
            "Caller-supplied output recorded as an AI proposal; canonical decision unchanged",
        ),
    )?;
    let base_media_oid = comparison.base_media_oid.clone().ok_or_else(|| {
        CreatorError::Integrity("creator byte-identity base media is absent".into())
    })?;
    let target_media_oid = comparison.target_media_oid.clone().ok_or_else(|| {
        CreatorError::Integrity("creator byte-identity target media is absent".into())
    })?;

    // Verify the exact state that the two create-only Ref publications will
    // expose. All creator CAS writes are complete at this point, so a
    // successful check also proves that begin preserves the reserved Human
    // decision headroom before it mutates either Ref.
    let prospective_snapshot = repository
        .refs()
        .snapshot_limited(pending_decision_capacity_limits.max_ref_roots)?;
    prospective_fsck(
        &repository,
        prospective_snapshot,
        &[(&decision_ref, &base_head), (&proposal_ref, &proposal_head)],
        pending_decision_capacity_limits,
        "begin",
    )?;

    repository.update_ref(RefUpdate {
        ref_name: &decision_ref,
        expected_head: None,
        new_head: &base_head,
        metadata: ReflogMetadata {
            occurred_at_unix_nanos: import_recorded_at.unix_nanos,
            actor: Some(&ids.creator),
            message: Some("initialize creator session"),
        },
    })?;

    let selector = ProjectSelector::new(ids.project.clone());
    let application = Application::new(
        PilotAuthenticator {
            agent_id: ids.agent.clone(),
            human_id: ids.creator.clone(),
        },
        PreparedExecutor {
            proposal_head: proposal_head.clone(),
            activity_oid: ai_activity_oid.clone(),
        },
        SystemAuthorizationClock,
        PILOT_PERMIT_TTL_NANOS,
        [RegisteredProject::new(selector.clone(), repository)],
    )?;
    application.grant_project_access(&selector, ids.agent.clone())?;
    application.grant_project_access(&selector, ids.creator.clone())?;
    let ai_profile = application.register_authority_profile(AiAuthorityProfileConfig::new(
        selector.clone(),
        ids.agent.clone(),
        ids.creator.clone(),
        decision_ref.clone(),
        ai_actor_oid,
        creator_actor_oid.clone(),
        context_oid,
        proposal_ref.clone(),
        vec![AiCapability::ProposeBranch, AiCapability::ReadContext],
        vec![AiCapability::ProposeBranch, AiCapability::ReadContext],
        AiSideEffectClass::None,
    ))?;
    let human_profile = application.register_human_profile(HumanAuthorityProfileConfig::new(
        selector.clone(),
        ids.creator.clone(),
        decision_ref.clone(),
        creator_actor_oid.clone(),
        policy_oid.clone(),
    ))?;
    let execution = application.register_execution(&ai_profile)?;
    let ai_permit = application.prepare_ai(AGENT_CREDENTIAL, &selector, &execution)?;
    let ai_receipt = application.execute_and_publish_ai(AGENT_CREDENTIAL, &ai_permit)?;
    let (ai_decision, admitted_proposal) = ai_receipt.into_parts();
    let published_proposal_ref = ai_decision.reflog.ref_name;
    let published_proposal_head = ai_decision.reflog.new_head;
    let published_ai_activity_oid = ai_decision.activity_oid;

    let warning = match comparison.outcome {
        ByteIdentityOutcome::Identical => {
            "Identical Blob bytes do not establish that the observed physical subject was unchanged."
        }
        ByteIdentityOutcome::Different => {
            "Different Blob bytes do not establish visual or physical change."
        }
        ByteIdentityOutcome::NotCompared => {
            "Byte identity was not compared because the ordered Observation inputs were incompatible."
        }
    };
    let mut reachable_from = vec![decision_ref.clone(), published_proposal_ref.clone()];
    reachable_from.sort();
    let pending_receipt = CreatorPendingReceipt {
        session: options.session.clone(),
        project_id: ids.project.clone(),
        subject_id: ids.subject.clone(),
        creator_id: ids.creator.clone(),
        agent_id: ids.agent.clone(),
        decision_ref: decision_ref.clone(),
        proposal_ref: published_proposal_ref,
        base_head: base_head.clone(),
        proposal_head: published_proposal_head,
        original_blob_oid: original_blob_oid.clone(),
        current_blob_oid: current_blob_oid.clone(),
        ai_output_blob_oid: ai_output_blob_oid.clone(),
        capture_profile_oid: capture_profile_oid.clone(),
        original_observation_oid: original_observation_oid.clone(),
        current_observation_oid: current_observation_oid.clone(),
        comparison: CreatorComparisonReport {
            analysis_oid: comparison.analysis_oid,
            tool_id: comparison_tool_id,
            tool_actor_oid: comparison_tool_actor_oid,
            adapter_id: BYTE_IDENTITY_ADAPTER_ID.into(),
            adapter_version: BYTE_IDENTITY_ADAPTER_VERSION.into(),
            implementation_oid: comparison.implementation_oid,
            configuration_oid: comparison.configuration_oid,
            status: comparison.status.as_str().into(),
            comparability: comparison.comparability.as_str().into(),
            outcome: comparison.outcome.as_str().into(),
            reason_codes: comparison.reason_codes,
            warnings: vec![warning.into()],
            base_observation_oid: comparison.base_observation_oid,
            target_observation_oid: comparison.target_observation_oid,
            base_media_oid,
            target_media_oid,
            replay_ready: true,
            reachable_from,
        },
        ai_activity_oid: published_ai_activity_oid,
    };
    let pending = PendingCreatorSession {
        application,
        admitted_proposal,
        human_profile,
        selector,
        repository_path: options.repository.clone(),
        ids,
        receipt: pending_receipt.clone(),
        base_tree_oid,
        proposal_tree_oid,
        byte_identity_outcome: comparison.outcome,
        comparison_status: comparison.status,
        comparison_comparability: comparison.comparability,
        recording_clock,
        decision_state: PendingDecisionState::Ready,
    };
    Ok(pending)
}

/// Publish one Human decision through the exact application instance that
/// admitted the pending proposal.
///
/// A publication error is outcome-ambiguous to the caller. The pending value
/// then refuses replay; callers must inspect the current Refs/report instead
/// of retrying blindly. After a successful publication, the committed receipt
/// remains available through [`PendingCreatorSession::completed_receipt`] even
/// if the final repository integrity check fails.
pub fn decide_creator_session(
    pending: &mut PendingCreatorSession,
    decision: &CreatorDecisionOptions,
) -> Result<CreatorRunReceipt> {
    decide_creator_session_with_limits(pending, decision, CREATOR_FSCK_LIMITS)
}

pub(crate) fn decide_creator_session_with_limits(
    pending: &mut PendingCreatorSession,
    decision: &CreatorDecisionOptions,
    fsck_limits: FsckLimits,
) -> Result<CreatorRunReceipt> {
    validate_decision_metadata(decision)?;
    let decision_admission_limits =
        reserve_fsck_capacity(fsck_limits, CREATOR_DECISION_RESERVE, "decision admission")?;
    match &pending.decision_state {
        PendingDecisionState::Ready => {}
        PendingDecisionState::Consumed(_) => {
            return Err(CreatorError::SessionExists(pending.receipt.session.clone()));
        }
        PendingDecisionState::Deciding => {
            pending.decision_state = PendingDecisionState::OutcomeUnknown;
            return Err(CreatorError::SessionIncomplete(
                pending.receipt.session.clone(),
            ));
        }
        PendingDecisionState::OutcomeUnknown => {
            return Err(CreatorError::SessionIncomplete(
                pending.receipt.session.clone(),
            ));
        }
    }

    let rationale = decision
        .rationale
        .as_deref()
        .unwrap_or_else(|| decision.disposition.default_rationale());
    let decision_recorded_at = pending.recording_clock.tick()?;
    let repository = Repository::open_with_tombstone_scan_limits(
        &pending.repository_path,
        fsck_limits.tombstone_scan,
    )?;
    let preflight = repository.fsck_with_limits(decision_admission_limits)?;
    if !preflight.is_clean() {
        return Err(CreatorError::Integrity(format!(
            "creator decision refused a repository with {} fsck issue(s)",
            preflight.issues.len()
        )));
    }
    let decision_feedback_oid = put_json(
        &repository,
        feedback_record(
            &pending.ids.feedback,
            &pending.ids.creator,
            &pending.ids.subject,
            &pending.receipt.proposal_head,
            decision.disposition,
            rationale,
            &decision_recorded_at.timestamp,
        ),
    )?;
    let selected_tree = if decision.disposition == CreatorDisposition::Adopt {
        &pending.proposal_tree_oid
    } else {
        &pending.base_tree_oid
    };
    let decision_head = put_json(
        &repository,
        commit(
            "decision",
            slice(&pending.receipt.base_head),
            selected_tree,
            slice(&decision_feedback_oid),
            &pending.ids.creator,
            &decision_recorded_at.timestamp,
            "Creator reviewed AI proposal",
        ),
    )?;

    let prospective_snapshot = repository
        .refs()
        .snapshot_limited(fsck_limits.max_ref_roots)?;
    prospective_fsck(
        &repository,
        prospective_snapshot,
        &[(
            pending.receipt.decision_ref.as_str(),
            decision_head.as_str(),
        )],
        fsck_limits,
        "decision",
    )?;
    drop(repository);

    let human_candidate = HumanDecisionCandidate::new(
        decision_head.clone(),
        decision_feedback_oid.clone(),
        Some("creator Pilot human decision"),
    );
    let human_registration = pending.application.register_human_decision(
        &pending.human_profile,
        &pending.admitted_proposal,
        human_candidate,
    )?;
    let human_permit = pending.application.prepare_human_decision(
        HUMAN_CREDENTIAL,
        &pending.selector,
        &human_registration,
    )?;
    pending.decision_state = PendingDecisionState::Deciding;
    let human_receipt = match pending
        .application
        .publish_human_decision(HUMAN_CREDENTIAL, &human_permit)
    {
        Ok(receipt) => receipt,
        Err(error) => {
            pending.decision_state = PendingDecisionState::OutcomeUnknown;
            return Err(error.into());
        }
    };
    let receipt_matches_prepared_lineage = human_receipt.reflog.ref_name
        == pending.receipt.decision_ref
        && human_receipt.reflog.old_head.as_deref() == Some(pending.receipt.base_head.as_str())
        && human_receipt.reflog.new_head == decision_head
        && human_receipt.proposal_commit_oid == pending.receipt.proposal_head
        && human_receipt.decision_feedback_oid == decision_feedback_oid;

    let comparison = &pending.receipt.comparison;
    let completed = CreatorRunReceipt {
        session: pending.receipt.session.clone(),
        project_id: pending.receipt.project_id.clone(),
        subject_id: pending.receipt.subject_id.clone(),
        creator_id: pending.receipt.creator_id.clone(),
        agent_id: pending.receipt.agent_id.clone(),
        decision_ref: human_receipt.reflog.ref_name,
        proposal_ref: pending.receipt.proposal_ref.clone(),
        base_head: pending.receipt.base_head.clone(),
        proposal_head: human_receipt.proposal_commit_oid,
        decision_head: human_receipt.reflog.new_head,
        original_blob_oid: pending.receipt.original_blob_oid.clone(),
        current_blob_oid: pending.receipt.current_blob_oid.clone(),
        ai_output_blob_oid: pending.receipt.ai_output_blob_oid.clone(),
        capture_profile_oid: pending.receipt.capture_profile_oid.clone(),
        original_observation_oid: pending.receipt.original_observation_oid.clone(),
        current_observation_oid: pending.receipt.current_observation_oid.clone(),
        comparison_tool_id: comparison.tool_id.clone(),
        comparison_tool_actor_oid: comparison.tool_actor_oid.clone(),
        comparison_analysis_oid: comparison.analysis_oid.clone(),
        comparison_implementation_oid: comparison.implementation_oid.clone(),
        comparison_configuration_oid: comparison.configuration_oid.clone(),
        byte_identity_outcome: pending.byte_identity_outcome,
        comparison_status: pending.comparison_status,
        comparison_comparability: pending.comparison_comparability,
        comparison_reason_codes: comparison.reason_codes.clone(),
        ai_activity_oid: pending.receipt.ai_activity_oid.clone(),
        decision_feedback_oid: human_receipt.decision_feedback_oid,
        disposition: decision.disposition,
    };
    if !receipt_matches_prepared_lineage {
        pending.decision_state = PendingDecisionState::OutcomeUnknown;
        return Err(CreatorError::Integrity(
            "application receipts do not match the prepared creator lineage".into(),
        ));
    }
    pending.decision_state = PendingDecisionState::Consumed(Box::new(completed.clone()));

    let repository = Repository::open(&pending.repository_path)?;
    let fsck = repository.fsck_with_limits(fsck_limits)?;
    if !fsck.is_clean() {
        return Err(CreatorError::Integrity(format!(
            "creator session completed with {} fsck issue(s)",
            fsck.issues.len()
        )));
    }
    Ok(completed)
}

/// Create one complete local creator session.
///
/// This compatibility wrapper preserves the original CLI contract while the
/// localhost application can pause between proposal admission and review.
pub fn run_creator_session(options: &CreatorRunOptions) -> Result<CreatorRunReceipt> {
    let decision = CreatorDecisionOptions {
        disposition: options.disposition,
        rationale: options.rationale.clone(),
    };
    validate_decision_metadata(&decision)?;
    let begin = CreatorBeginOptions {
        repository: options.repository.clone(),
        session: options.session.clone(),
        original_image: options.original_image.clone(),
        current_image: options.current_image.clone(),
        ai_output: options.ai_output.clone(),
        subject_label: options.subject_label.clone(),
        creator_name: options.creator_name.clone(),
    };
    let mut pending = begin_creator_session(&begin)?;
    match decide_creator_session(&mut pending, &decision) {
        Ok(receipt) => Ok(receipt),
        Err(_) if pending.completed_receipt().is_some() => Ok(pending
            .completed_receipt()
            .expect("completed receipt was just observed")
            .clone()),
        Err(error) => Err(error),
    }
}

pub(crate) fn validate_begin_metadata(options: &CreatorBeginOptions) -> Result<()> {
    validate_session(&options.session)?;
    if options.subject_label.is_empty() || options.subject_label.len() > 500 {
        return Err(CreatorError::InvalidArgument(
            "subject label must contain 1 to 500 UTF-8 bytes".into(),
        ));
    }
    if options.creator_name.is_empty() || options.creator_name.len() > 300 {
        return Err(CreatorError::InvalidArgument(
            "creator name must contain 1 to 300 UTF-8 bytes".into(),
        ));
    }
    Ok(())
}

fn validate_decision_metadata(options: &CreatorDecisionOptions) -> Result<()> {
    if options
        .rationale
        .as_ref()
        .is_some_and(|value| value.len() > 5_000)
    {
        return Err(CreatorError::InvalidArgument(
            "rationale exceeds 5000 UTF-8 bytes".into(),
        ));
    }
    Ok(())
}

fn validate_input_files(original: &Path, current: &Path, ai_output: &Path) -> Result<()> {
    let mut aggregate_bytes = 0_u64;
    for path in [original, current, ai_output] {
        let file = File::open(path)
            .map_err(|source| CreatorError::io("open creator input file", path, source))?;
        let bytes = file
            .metadata()
            .map_err(|source| CreatorError::io("inspect opened creator input file", path, source))?
            .len();
        if bytes > CREATOR_MAX_INPUT_FILE_BYTES {
            return Err(CreatorError::ResourceLimit(format!(
                "creator input file exceeds {CREATOR_MAX_INPUT_FILE_BYTES} bytes"
            )));
        }
        aggregate_bytes = aggregate_bytes.checked_add(bytes).ok_or_else(|| {
            CreatorError::ResourceLimit("creator input byte total overflowed u64".into())
        })?;
        if aggregate_bytes > CREATOR_MAX_INPUT_AGGREGATE_BYTES {
            return Err(CreatorError::ResourceLimit(format!(
                "creator input files exceed {CREATOR_MAX_INPUT_AGGREGATE_BYTES} aggregate bytes"
            )));
        }
    }
    Ok(())
}

pub(crate) fn validate_session(session: &str) -> Result<()> {
    if session.is_empty()
        || session.len() > 64
        || !session.as_bytes()[0].is_ascii_lowercase()
        || !session
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    {
        return Err(CreatorError::InvalidArgument(
            "session must match [a-z][a-z0-9-]{0,63}".into(),
        ));
    }
    Ok(())
}

pub(crate) fn decision_ref(session: &str) -> String {
    format!("{DECISION_PREFIX}/{session}")
}

pub(crate) fn proposal_ref(session: &str) -> String {
    format!("{PROPOSAL_PREFIX}/{session}")
}

#[derive(Clone)]
pub(crate) struct SessionIds {
    pub(crate) creator: String,
    pub(crate) agent: String,
    pub(crate) project: String,
    pub(crate) subject: String,
    pub(crate) series: String,
    pub(crate) original_observation: String,
    pub(crate) current_observation: String,
    pub(crate) import_activity: String,
    pub(crate) policy: String,
    pub(crate) grant: String,
    pub(crate) context: String,
    pub(crate) ai_activity: String,
    pub(crate) feedback: String,
}

impl SessionIds {
    pub(crate) fn fresh() -> Result<Self> {
        let mut seed = [0_u8; 32];
        getrandom::fill(&mut seed).map_err(|error| {
            CreatorError::Random(format!("operating-system random source failed: {error}"))
        })?;
        Ok(Self::from_seed(&seed))
    }

    pub(crate) fn from_seed(seed: &[u8; 32]) -> Self {
        Self {
            creator: entity_id(seed, "creator"),
            agent: entity_id(seed, "agent"),
            project: entity_id(seed, "project"),
            subject: entity_id(seed, "subject"),
            series: entity_id(seed, "series"),
            original_observation: entity_id(seed, "original-observation"),
            current_observation: entity_id(seed, "current-observation"),
            import_activity: entity_id(seed, "import-activity"),
            policy: entity_id(seed, "policy"),
            grant: entity_id(seed, "grant"),
            context: entity_id(seed, "context"),
            ai_activity: entity_id(seed, "ai-activity"),
            feedback: entity_id(seed, "feedback"),
        }
    }
}

pub(crate) fn entity_id(seed: &[u8; 32], role: &str) -> String {
    let mut hash = Sha256::new();
    hash.update(b"synapsegit-creator-entity-v1\0");
    hash.update(seed);
    hash.update(b"\0");
    hash.update(role.as_bytes());
    uuid_entity_id(hash.finalize().into())
}

pub(crate) fn related_entity_id(scope: &str, role: &str) -> String {
    let mut hash = Sha256::new();
    hash.update(b"synapsegit-creator-related-entity-v1\0");
    hash.update(scope.as_bytes());
    hash.update(b"\0");
    hash.update(role.as_bytes());
    uuid_entity_id(hash.finalize().into())
}

fn uuid_entity_id(mut bytes: [u8; 32]) -> String {
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    format!(
        "urn:uuid:{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0],
        bytes[1],
        bytes[2],
        bytes[3],
        bytes[4],
        bytes[5],
        bytes[6],
        bytes[7],
        bytes[8],
        bytes[9],
        bytes[10],
        bytes[11],
        bytes[12],
        bytes[13],
        bytes[14],
        bytes[15]
    )
}

#[derive(Clone)]
struct PilotAuthenticator {
    agent_id: String,
    human_id: String,
}

impl Authenticator for PilotAuthenticator {
    type Credential = str;

    fn authenticate(
        &self,
        credential: &Self::Credential,
    ) -> std::result::Result<AuthenticatedSession, AuthenticationFailure> {
        match credential {
            AGENT_CREDENTIAL => AuthenticatedSession::new(&self.agent_id, "creator-agent-session"),
            HUMAN_CREDENTIAL => AuthenticatedSession::new(&self.human_id, "creator-human-session"),
            _ => Err(AuthenticationFailure),
        }
    }
}

#[derive(Clone)]
struct PreparedExecutor {
    proposal_head: String,
    activity_oid: String,
}

impl AiExecutor for PreparedExecutor {
    fn execute(
        &self,
        _context: &AiExecutionContext,
    ) -> std::result::Result<ExecutedAiProposal, ExecutionFailure> {
        Ok(ExecutedAiProposal::new(
            self.proposal_head.clone(),
            self.activity_oid.clone(),
            Some("creator Pilot AI proposal"),
        ))
    }
}

fn slice(value: &String) -> &[String] {
    std::slice::from_ref(value)
}

pub(crate) fn insert_entry(
    entries: &mut JsonMap<String, JsonValue>,
    name: &str,
    kind: &str,
    oid: &str,
) {
    entries.insert(name.to_owned(), json!({ "entry_kind": kind, "oid": oid }));
}
