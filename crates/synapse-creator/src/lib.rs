//! Local creator-facing orchestration for one SynapseGit Stage 0 workflow.
//!
//! This crate turns image files and a human disposition into Core objects
//! without caller-authored JSON. It is intentionally a synchronous,
//! single-process Pilot boundary: images remain opaque Blobs, the AI output is
//! supplied by a trusted local integration, and publication still passes
//! through [`synapse_application`] AI and Human admission routes.

#![forbid(unsafe_code)]

use serde_json::{Map as JsonMap, Value as JsonValue, json};
use sha2::{Digest, Sha256};
use std::error::Error;
use std::fmt;
use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use synapse_application::{
    AiAuthorityProfileConfig, AiExecutionContext, AiExecutor, Application, ApplicationError,
    AuthenticatedSession, AuthenticationFailure, Authenticator, ExecutedAiProposal,
    ExecutionFailure, HumanAuthorityProfileConfig, HumanDecisionCandidate, ProjectSelector,
    RegisteredProject,
};
use synapse_canonical::{canonical_bytes, parse_strict};
use synapse_cas::{GraphLimits, fsck};
use synapse_core::{
    AiCapability, AiSideEffectClass, Repository, RepositoryError, SystemAuthorizationClock,
};
use synapse_projection::{
    ProjectionError, ProjectionLimits, RefScope, SqliteProjectionStore, TimelineRecordKind,
    TimelineTimeBasis,
};
use synapse_sqlite::{RefStoreError, RefUpdate, ReflogMetadata};

const SCHEMA_VERSION: &str = "0.1.0";
const DECISION_PREFIX: &str = "decision/creator";
const PROPOSAL_PREFIX: &str = "proposal/creator-agent";
const PILOT_PERMIT_TTL_NANOS: i128 = 60_000_000_000;
const PILOT_MAX_OUTPUT_BYTES: i64 = 1_073_741_824;
const AGENT_CREDENTIAL: &str = "local-creator-agent";
const HUMAN_CREDENTIAL: &str = "local-creator-human";

/// Human outcomes supported by the narrow Stage 0 decision route.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CreatorDisposition {
    Adopt,
    Reject,
    Defer,
}

impl CreatorDisposition {
    pub fn parse(value: &str) -> Result<Self> {
        match value {
            "adopt" => Ok(Self::Adopt),
            "reject" => Ok(Self::Reject),
            "defer" => Ok(Self::Defer),
            _ => Err(CreatorError::InvalidArgument(
                "decision must be one of adopt, reject, or defer".into(),
            )),
        }
    }

    pub const fn as_cli_str(self) -> &'static str {
        match self {
            Self::Adopt => "adopt",
            Self::Reject => "reject",
            Self::Defer => "defer",
        }
    }

    pub const fn as_protocol_str(self) -> &'static str {
        match self {
            Self::Adopt => "adopted_unchanged",
            Self::Reject => "rejected",
            Self::Defer => "deferred",
        }
    }

    fn from_protocol(value: &str) -> Result<Self> {
        match value {
            "adopted_unchanged" => Ok(Self::Adopt),
            "rejected" => Ok(Self::Reject),
            "deferred" => Ok(Self::Defer),
            _ => Err(CreatorError::ReportInvalid(format!(
                "unsupported creator disposition {value:?}"
            ))),
        }
    }

    const fn reason_code(self) -> &'static str {
        "unspecified"
    }

    const fn default_rationale(self) -> &'static str {
        match self {
            Self::Adopt => "The creator adopted the AI proposal unchanged.",
            Self::Reject => "The creator rejected the AI proposal.",
            Self::Defer => "The creator deferred the AI proposal for later review.",
        }
    }
}

/// Inputs for one new creator session.
#[derive(Clone, Debug)]
pub struct CreatorRunOptions {
    pub repository: PathBuf,
    pub session: String,
    pub original_image: PathBuf,
    pub current_image: PathBuf,
    pub ai_output: PathBuf,
    pub subject_label: String,
    pub creator_name: String,
    pub disposition: CreatorDisposition,
    pub rationale: Option<String>,
}

/// Stable identifiers produced by a completed creator session.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CreatorRunReceipt {
    pub session: String,
    pub project_id: String,
    pub subject_id: String,
    pub creator_id: String,
    pub agent_id: String,
    pub decision_ref: String,
    pub proposal_ref: String,
    pub base_head: String,
    pub proposal_head: String,
    pub decision_head: String,
    pub original_blob_oid: String,
    pub current_blob_oid: String,
    pub ai_output_blob_oid: String,
    pub original_observation_oid: String,
    pub current_observation_oid: String,
    pub ai_activity_oid: String,
    pub decision_feedback_oid: String,
    pub disposition: CreatorDisposition,
}

/// One report timeline row rebuilt from current authoritative Refs and CAS.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CreatorTimelineEntry {
    pub oid: String,
    pub stage: &'static str,
    pub kind: &'static str,
    pub entity_id: String,
    pub ordering_time: String,
    pub time_basis: &'static str,
    pub reachable_from: Vec<String>,
}

/// Creator-readable process report. The ProjectionStore used to build it is
/// disposable and is never an authorization or recovery source.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CreatorReport {
    pub session: String,
    pub project_id: String,
    pub subject_id: String,
    pub creator_id: String,
    pub agent_id: String,
    pub decision_ref: String,
    pub proposal_ref: String,
    pub decision_head: String,
    pub proposal_head: String,
    pub base_head: String,
    pub base_snapshot: String,
    pub proposal_snapshot: String,
    pub decision_snapshot: String,
    pub disposition: CreatorDisposition,
    pub selected_ai_output: bool,
    pub rationale: Option<String>,
    pub original_blob_oid: String,
    pub current_blob_oid: String,
    pub ai_output_blob_oid: String,
    pub timeline: Vec<CreatorTimelineEntry>,
    pub fsck_objects: usize,
}

/// Errors from the Pilot orchestration boundary.
#[derive(Debug)]
pub enum CreatorError {
    InvalidArgument(String),
    SessionExists(String),
    SessionIncomplete(String),
    SessionNotFound(String),
    Io { path: PathBuf, source: io::Error },
    Clock(String),
    Random(String),
    Repository(RepositoryError),
    Application(ApplicationError),
    Projection(ProjectionError),
    Json(serde_json::Error),
    Integrity(String),
    ReportInvalid(String),
}

impl CreatorError {
    pub fn code(&self) -> &str {
        match self {
            Self::InvalidArgument(_) => "usage_error",
            Self::SessionExists(_) => "creator_session_exists",
            Self::SessionIncomplete(_) => "creator_session_incomplete",
            Self::SessionNotFound(_) => "creator_session_not_found",
            Self::Io { .. } | Self::Clock(_) | Self::Random(_) => "storage_error",
            Self::Repository(error) => error.code(),
            Self::Application(error) => error.code(),
            Self::Projection(error) => error.code(),
            Self::Json(_) | Self::ReportInvalid(_) => "creator_report_invalid",
            Self::Integrity(_) => "fsck_failed",
        }
    }
}

impl fmt::Display for CreatorError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidArgument(message) => formatter.write_str(message),
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
            Self::Io { path, source } => write!(formatter, "{}: {source}", path.display()),
            Self::Clock(message) => formatter.write_str(message),
            Self::Random(message) => formatter.write_str(message),
            Self::Repository(error) => error.fmt(formatter),
            Self::Application(error) => error.fmt(formatter),
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

/// Create one complete local creator session.
///
/// Both target Refs must be absent. CAS writes before the base Ref publication
/// are harmless immutable orphans. A failure after publication may leave an
/// incomplete or already-complete live session which this create-only Pilot
/// will not overwrite; callers must inspect it or choose a new session name.
pub fn run_creator_session(options: &CreatorRunOptions) -> Result<CreatorRunReceipt> {
    validate_metadata(options)?;
    let decision_ref = decision_ref(&options.session);
    let proposal_ref = proposal_ref(&options.session);
    let mut repository = Repository::open(&options.repository)?;
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
    let preflight = repository.fsck()?;
    if !preflight.is_clean() {
        return Err(CreatorError::Integrity(format!(
            "creator session refused an existing repository with {} fsck issue(s)",
            preflight.issues.len()
        )));
    }
    validate_input_files(options)?;

    let original_blob_oid = put_file(&repository, &options.original_image)?;
    let current_blob_oid = put_file(&repository, &options.current_image)?;
    let ai_output_blob_oid = put_file(&repository, &options.ai_output)?;
    let mut recording_clock = RecordingClock::default();
    let base_recorded_at = recording_clock.tick()?;
    let ids = SessionIds::fresh()?;
    let original_recorded_at = recording_clock.tick()?;
    let current_recorded_at = recording_clock.tick()?;
    let import_recorded_at = recording_clock.tick()?;

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
            &base_recorded_at.timestamp,
            &options.subject_label,
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
        ),
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
    insert_entry(&mut base_entries, "policy.json", "record", &policy_oid);
    insert_entry(&mut base_entries, "grant.json", "record", &grant_oid);
    insert_entry(&mut base_entries, "subject.json", "record", &subject_oid);
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
    let base_head = put_json(
        &repository,
        commit(
            "checkpoint",
            &[],
            &base_tree_oid,
            slice(&import_activity_oid),
            &ids.creator,
            &import_recorded_at.timestamp,
            "Creator images imported and observed",
        ),
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

    let rationale = options
        .rationale
        .as_deref()
        .unwrap_or_else(|| options.disposition.default_rationale());
    let decision_recorded_at = recording_clock.tick()?;
    let decision_feedback_oid = put_json(
        &repository,
        feedback_record(
            &ids.feedback,
            &ids.creator,
            &ids.subject,
            &proposal_head,
            options.disposition,
            rationale,
            &decision_recorded_at.timestamp,
        ),
    )?;
    let selected_tree = if options.disposition == CreatorDisposition::Adopt {
        &proposal_tree_oid
    } else {
        &base_tree_oid
    };
    let decision_head = put_json(
        &repository,
        commit(
            "decision",
            slice(&base_head),
            selected_tree,
            slice(&decision_feedback_oid),
            &ids.creator,
            &decision_recorded_at.timestamp,
            "Creator reviewed AI proposal",
        ),
    )?;

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
    let execution = application.register_execution(&ai_profile)?;
    let ai_permit = application.prepare_ai(AGENT_CREDENTIAL, &selector, &execution)?;
    let ai_receipt = application.execute_and_publish_ai(AGENT_CREDENTIAL, &ai_permit)?;
    let (_, admitted_proposal) = ai_receipt.into_parts();

    let human_profile = application.register_human_profile(HumanAuthorityProfileConfig::new(
        selector.clone(),
        ids.creator.clone(),
        decision_ref.clone(),
        creator_actor_oid,
        policy_oid,
    ))?;
    let human_candidate = HumanDecisionCandidate::new(
        decision_head.clone(),
        decision_feedback_oid.clone(),
        Some("creator Pilot human decision"),
    );
    let human_registration =
        application.register_human_decision(&human_profile, &admitted_proposal, human_candidate)?;
    let human_permit =
        application.prepare_human_decision(HUMAN_CREDENTIAL, &selector, &human_registration)?;
    let human_receipt = application.publish_human_decision(HUMAN_CREDENTIAL, &human_permit)?;
    if human_receipt.reflog.new_head != decision_head
        || human_receipt.proposal_commit_oid != proposal_head
        || human_receipt.decision_feedback_oid != decision_feedback_oid
    {
        return Err(CreatorError::Integrity(
            "application receipts do not match the prepared creator lineage".into(),
        ));
    }
    drop(application);

    let repository = Repository::open(&options.repository)?;
    let fsck = repository.fsck()?;
    if !fsck.is_clean() {
        return Err(CreatorError::Integrity(format!(
            "creator session completed with {} fsck issue(s)",
            fsck.issues.len()
        )));
    }

    Ok(CreatorRunReceipt {
        session: options.session.clone(),
        project_id: ids.project,
        subject_id: ids.subject,
        creator_id: ids.creator,
        agent_id: ids.agent,
        decision_ref,
        proposal_ref,
        base_head,
        proposal_head,
        decision_head,
        original_blob_oid,
        current_blob_oid,
        ai_output_blob_oid,
        original_observation_oid,
        current_observation_oid,
        ai_activity_oid,
        decision_feedback_oid,
        disposition: options.disposition,
    })
}

/// Rebuild a creator report from current Refs and CAS.
pub fn creator_report(repository_path: impl AsRef<Path>, session: &str) -> Result<CreatorReport> {
    validate_session(session)?;
    let repository = Repository::open(repository_path)?;
    let decision_ref = decision_ref(session);
    let proposal_ref = proposal_ref(session);
    let snapshot = repository.refs().snapshot()?;
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
    if read_json(&repository, &decision_head)?
        .get("commit_kind")
        .and_then(JsonValue::as_str)
        != Some("decision")
    {
        return Err(CreatorError::SessionIncomplete(session.to_owned()));
    }
    let ids = load_session_ids(&repository, session, &decision_head)?;

    let lineage = validate_report_lineage(&repository, &ids, &decision_head, &proposal_head)?;
    let disposition = lineage.disposition;
    let rationale = lineage.rationale;

    let mut projection = SqliteProjectionStore::open_in_memory()?;
    projection.rebuild_with_limits(repository.objects(), &snapshot, ProjectionLimits::default())?;
    let timeline = projection.subject_timeline(
        &ids.subject,
        None,
        &RefScope::names([decision_ref.clone(), proposal_ref.clone()]),
    )?;
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
        .ok_or_else(|| CreatorError::ReportInvalid("AI Activity is absent from timeline".into()))?;
    if ai_activity.oid != lineage.ai_activity_oid {
        return Err(CreatorError::ReportInvalid(
            "timeline AI Activity does not match the current proposal transition".into(),
        ));
    }
    let original_blob_oid = role_oid(
        object_field(
            &read_json(&repository, &original_observation.oid)?,
            "payload",
            "original Observation payload",
        )?,
        "media_refs",
        "primary",
    )?;
    let current_blob_oid = role_oid(
        object_field(
            &read_json(&repository, &current_observation.oid)?,
            "payload",
            "current Observation payload",
        )?,
        "media_refs",
        "primary",
    )?;
    let ai_output_blob_oid = role_oid(
        object_field(
            &read_json(&repository, &ai_activity.oid)?,
            "payload",
            "AI Activity payload",
        )?,
        "output_refs",
        "proposal",
    )?;

    let heads = snapshot
        .refs
        .iter()
        .map(|record| record.head.clone())
        .collect::<Vec<_>>();
    let fsck = fsck(repository.objects(), &heads, GraphLimits::default())
        .map_err(RepositoryError::from)?;
    if !fsck.is_clean() {
        return Err(CreatorError::Integrity(format!(
            "creator report refused {} fsck issue(s)",
            fsck.issues.len()
        )));
    }
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

    Ok(CreatorReport {
        session: session.to_owned(),
        project_id: ids.project,
        subject_id: ids.subject,
        creator_id: ids.creator,
        agent_id: ids.agent,
        decision_ref,
        proposal_ref,
        decision_head,
        proposal_head,
        base_head: lineage.base_head,
        base_snapshot: lineage.base_snapshot,
        proposal_snapshot: lineage.proposal_snapshot,
        decision_snapshot: lineage.decision_snapshot,
        disposition,
        selected_ai_output: disposition == CreatorDisposition::Adopt,
        rationale,
        original_blob_oid,
        current_blob_oid,
        ai_output_blob_oid,
        timeline,
        fsck_objects: fsck.objects_verified,
    })
}

fn validate_metadata(options: &CreatorRunOptions) -> Result<()> {
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

fn validate_input_files(options: &CreatorRunOptions) -> Result<()> {
    for path in [
        &options.original_image,
        &options.current_image,
        &options.ai_output,
    ] {
        File::open(path).map_err(|source| CreatorError::Io {
            path: path.clone(),
            source,
        })?;
    }
    Ok(())
}

fn validate_session(session: &str) -> Result<()> {
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

fn decision_ref(session: &str) -> String {
    format!("{DECISION_PREFIX}/{session}")
}

fn proposal_ref(session: &str) -> String {
    format!("{PROPOSAL_PREFIX}/{session}")
}

#[derive(Clone)]
struct SessionIds {
    creator: String,
    agent: String,
    project: String,
    subject: String,
    series: String,
    original_observation: String,
    current_observation: String,
    import_activity: String,
    policy: String,
    grant: String,
    context: String,
    ai_activity: String,
    feedback: String,
}

impl SessionIds {
    fn fresh() -> Result<Self> {
        let mut seed = [0_u8; 32];
        getrandom::fill(&mut seed).map_err(|error| {
            CreatorError::Random(format!("operating-system random source failed: {error}"))
        })?;
        Ok(Self::from_seed(&seed))
    }

    fn from_seed(seed: &[u8; 32]) -> Self {
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

fn entity_id(seed: &[u8; 32], role: &str) -> String {
    let mut hash = Sha256::new();
    hash.update(b"synapsegit-creator-entity-v1\0");
    hash.update(seed);
    hash.update(b"\0");
    hash.update(role.as_bytes());
    let mut bytes: [u8; 32] = hash.finalize().into();
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

struct ProtocolTime {
    timestamp: String,
    unix_nanos: i64,
    seconds: i64,
    subsec_nanos: u32,
}

impl ProtocolTime {
    fn after_seconds(&self, delta: i64) -> Result<String> {
        let seconds = self
            .seconds
            .checked_add(delta)
            .ok_or_else(|| CreatorError::Clock("protocol timestamp overflow".into()))?;
        format_timestamp(seconds, self.subsec_nanos)
    }
}

#[derive(Default)]
struct RecordingClock {
    last_unix_nanos: Option<i128>,
}

impl RecordingClock {
    fn tick(&mut self) -> Result<ProtocolTime> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|error| CreatorError::Clock(format!("system clock error: {error}")))?;
        let observed = i128::try_from(now.as_nanos())
            .map_err(|_| CreatorError::Clock("system time exceeds i128 nanoseconds".into()))?;
        let logical = self
            .last_unix_nanos
            .and_then(|last| last.checked_add(1))
            .map_or(observed, |next| observed.max(next));
        self.last_unix_nanos = Some(logical);
        let unix_nanos = i64::try_from(logical).map_err(|_| {
            CreatorError::Clock("system time exceeds reflog nanosecond range".into())
        })?;
        let seconds = unix_nanos.div_euclid(1_000_000_000);
        let subsec_nanos = u32::try_from(unix_nanos.rem_euclid(1_000_000_000))
            .expect("nanosecond remainder is within u32");
        Ok(ProtocolTime {
            timestamp: format_timestamp(seconds, subsec_nanos)?,
            unix_nanos,
            seconds,
            subsec_nanos,
        })
    }
}

fn format_timestamp(seconds: i64, nanos: u32) -> Result<String> {
    if seconds < 0 {
        return Err(CreatorError::Clock(
            "creator Pilot requires a system clock after the Unix epoch".into(),
        ));
    }
    let days = seconds.div_euclid(86_400);
    let seconds_of_day = seconds.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    if !(0..=9_999).contains(&year) {
        return Err(CreatorError::Clock(
            "system time is outside the four-digit protocol year range".into(),
        ));
    }
    let hour = seconds_of_day / 3_600;
    let minute = (seconds_of_day % 3_600) / 60;
    let second = seconds_of_day % 60;
    Ok(format!(
        "{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{nanos:09}Z"
    ))
}

fn civil_from_days(days_since_epoch: i64) -> (i64, i64, i64) {
    let z = days_since_epoch + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let day_of_era = z - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    year += i64::from(month <= 2);
    (year, month, day)
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

fn put_file(repository: &Repository, path: &Path) -> Result<String> {
    let file = File::open(path).map_err(|source| CreatorError::Io {
        path: path.to_owned(),
        source,
    })?;
    Ok(repository.put_blob(file)?.oid)
}

fn put_json(repository: &Repository, value: JsonValue) -> Result<String> {
    Ok(repository.put_object(&serde_json::to_vec(&value)?)?.oid)
}

fn read_json(repository: &Repository, oid: &str) -> Result<JsonValue> {
    let bytes = repository
        .objects()
        .read_raw(oid)
        .map_err(RepositoryError::from)?
        .ok_or_else(|| CreatorError::ReportInvalid(format!("stored object is missing: {oid}")))?;
    Ok(serde_json::from_slice(&bytes)?)
}

fn object_field<'a>(value: &'a JsonValue, key: &str, label: &str) -> Result<&'a JsonValue> {
    value
        .get(key)
        .filter(|value| value.is_object())
        .ok_or_else(|| CreatorError::ReportInvalid(format!("{label} is missing or invalid")))
}

fn string_field<'a>(value: &'a JsonValue, key: &str, label: &str) -> Result<&'a str> {
    value
        .get(key)
        .and_then(JsonValue::as_str)
        .ok_or_else(|| CreatorError::ReportInvalid(format!("{label} is missing or invalid")))
}

struct ReportLineage {
    disposition: CreatorDisposition,
    rationale: Option<String>,
    ai_activity_oid: String,
    base_head: String,
    base_snapshot: String,
    proposal_snapshot: String,
    decision_snapshot: String,
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
    let base_snapshot = string_field(&base, "snapshot", "creator base snapshot")?;

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
            "decision snapshot does not match the {:?} disposition",
            disposition
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
    })
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

fn canonical_set(mut values: Vec<JsonValue>) -> Vec<JsonValue> {
    values.sort_by_cached_key(|value| {
        let json = serde_json::to_vec(value).expect("JsonValue serialization cannot fail");
        let parsed = parse_strict(&json).expect("internal set member is strict JSON");
        canonical_bytes(&parsed).expect("internal set member fits canonical limits")
    });
    values
}

fn slice(value: &String) -> &[String] {
    std::slice::from_ref(value)
}

fn insert_entry(entries: &mut JsonMap<String, JsonValue>, name: &str, kind: &str, oid: &str) {
    entries.insert(name.to_owned(), json!({ "entry_kind": kind, "oid": oid }));
}

fn envelope(
    record_type: &str,
    entity_id: &str,
    recorded_at: &str,
    asserted_by: &str,
    origin: &str,
    payload: JsonValue,
) -> JsonValue {
    json!({
        "object_type": "record",
        "schema_version": SCHEMA_VERSION,
        "record_type": record_type,
        "entity_id": entity_id,
        "recorded_at": recorded_at,
        "asserted_by": asserted_by,
        "origin": origin,
        "source_refs": [],
        "payload": payload,
        "extensions": {}
    })
}

fn actor_record(
    entity_id: &str,
    asserted_by: &str,
    recorded_at: &str,
    actor_kind: &str,
    display_name: &str,
) -> JsonValue {
    envelope(
        "actor",
        entity_id,
        recorded_at,
        asserted_by,
        "self_declared",
        json!({
            "actor_kind": actor_kind,
            "display_name": display_name
        }),
    )
}

fn ai_actor_record(entity_id: &str, asserted_by: &str, recorded_at: &str) -> JsonValue {
    envelope(
        "actor",
        entity_id,
        recorded_at,
        asserted_by,
        "tool_recorded",
        json!({
            "actor_kind": "ai_agent",
            "display_name": "Local creator integration",
            "ai_profile": {
                "provider": "local",
                "model_id": "creator-supplied-output",
                "model_version": "stage0-pilot-v1",
                "capabilities": canonical_set(vec![json!("propose_branch"), json!("read_context")])
            },
            "description": "Records a caller-supplied AI output through the authenticated Pilot boundary."
        }),
    )
}

fn policy_record(
    entity_id: &str,
    creator_id: &str,
    project_id: &str,
    decision_ref: &str,
    proposal_ref: &str,
    recorded_at: &str,
) -> JsonValue {
    envelope(
        "policy",
        entity_id,
        recorded_at,
        creator_id,
        "self_declared",
        json!({
            "scope_refs": canonical_set(vec![json!(project_id)]),
            "rules": [
                {
                    "rule_id": "allow-context-read",
                    "effect": "allow",
                    "action": "read",
                    "resource_selector": "project/**"
                },
                {
                    "rule_id": "allow-session-proposal",
                    "effect": "allow",
                    "action": "propose",
                    "resource_selector": proposal_ref
                },
                {
                    "rule_id": "gate-session-decision",
                    "effect": "require_human_gate",
                    "action": "publish",
                    "resource_selector": decision_ref,
                    "human_gate": "before_decision_ref"
                }
            ],
            "default_effect": "deny",
            "notes": "Local single-creator Stage 0 Pilot policy."
        }),
    )
}

#[allow(clippy::too_many_arguments)]
fn grant_record(
    entity_id: &str,
    creator_id: &str,
    agent_id: &str,
    project_id: &str,
    proposal_ref: &str,
    recorded_at: &str,
    active_at: &str,
    expires_at: &str,
) -> JsonValue {
    let mut record = envelope(
        "delegation_grant",
        entity_id,
        recorded_at,
        creator_id,
        "self_declared",
        json!({
            "principal_ref": creator_id,
            "delegate_ref": agent_id,
            "project_ref": project_id,
            "purpose": "Record one bounded creator-facing AI proposal.",
            "capabilities": canonical_set(vec![json!("propose_branch"), json!("read_context")]),
            "resource_selectors": canonical_set(vec![json!("project/**")]),
            "writable_ref_prefixes": canonical_set(vec![json!(proposal_ref)]),
            "data_classes": canonical_set(vec![json!("internal")]),
            "allowed_egress": [],
            "may_delegate": false,
            "max_child_depth": 0,
            "max_output_bytes": PILOT_MAX_OUTPUT_BYTES,
            "required_human_gates": canonical_set(vec![json!("before_decision_ref"), json!("before_release_ref")]),
            "expires_at": expires_at
        }),
    );
    record
        .as_object_mut()
        .expect("record envelope is an object")
        .insert(
            "valid_time".into(),
            json!({ "kind": "instant", "at": active_at }),
        );
    record
}

fn subject_record(session: &str, ids: &SessionIds, recorded_at: &str, label: &str) -> JsonValue {
    let mut record = envelope(
        "subject",
        &ids.subject,
        recorded_at,
        &ids.creator,
        "self_declared",
        json!({
            "subject_kind": "hybrid",
            "label": label,
            "description": "Creator subject tracked by the local Stage 0 Pilot.",
            "relation_refs": [],
            "spatial_frame_refs": []
        }),
    );
    record
        .get_mut("extensions")
        .and_then(JsonValue::as_object_mut)
        .expect("record extensions is an object")
        .insert(
            "org.synapsegit.creator-session".into(),
            json!({
                "format": "synapsegit-creator-session-v1",
                "session": session,
                "project_id": ids.project,
                "creator_id": ids.creator,
                "agent_id": ids.agent,
                "subject_id": ids.subject,
                "series_id": ids.series,
                "original_observation_id": ids.original_observation,
                "current_observation_id": ids.current_observation,
                "import_activity_id": ids.import_activity,
                "policy_id": ids.policy,
                "grant_id": ids.grant,
                "context_id": ids.context,
                "ai_activity_id": ids.ai_activity,
                "feedback_id": ids.feedback
            }),
        );
    record
}

fn observation_record(
    entity_id: &str,
    creator_id: &str,
    subject_id: &str,
    series_id: &str,
    timestamp: &str,
    image_oid: &str,
) -> JsonValue {
    envelope(
        "observation",
        entity_id,
        timestamp,
        creator_id,
        "imported",
        json!({
            "subject_ref": subject_id,
            "series_ref": series_id,
            "capture_time": {
                "kind": "unknown",
                "reason": "Imported file; capture time was not supplied or independently verified."
            },
            "media_refs": canonical_set(vec![json!({ "role": "primary", "oid": image_oid })]),
            "calibration_refs": [],
            "protocol_deviations": ["Capture time and capture metadata were not supplied or independently verified."],
            "environment_refs": [],
            "missing_regions": []
        }),
    )
}

#[allow(clippy::too_many_arguments)]
fn import_activity_record(
    entity_id: &str,
    creator_id: &str,
    subject_id: &str,
    timestamp: &str,
    original_blob: &str,
    current_blob: &str,
) -> JsonValue {
    let mut record = envelope(
        "activity",
        entity_id,
        timestamp,
        creator_id,
        "tool_recorded",
        json!({
            "activity_kind": "import",
            "actor_refs": canonical_set(vec![json!({ "role": "creator", "actor_ref": creator_id })]),
            "subject_refs": canonical_set(vec![json!(subject_id)]),
            "input_refs": [],
            "output_refs": canonical_set(vec![
                json!({ "role": "current", "oid": current_blob }),
                json!({ "role": "original", "oid": original_blob })
            ]),
            "before_observation_refs": [],
            "after_observation_refs": [],
            "reversibility": "reversible",
            "summary": "Imported original and current images without interpreting their pixels.",
            "side_effect_class": "project_internal"
        }),
    );
    record
        .as_object_mut()
        .expect("record envelope is an object")
        .insert(
            "valid_time".into(),
            json!({
                "kind": "unknown",
                "reason": "The local Pilot did not receive an external import-event timestamp."
            }),
        );
    record
}

#[allow(clippy::too_many_arguments)]
fn context_record(
    entity_id: &str,
    creator_id: &str,
    subject_id: &str,
    base_head: &str,
    decision_ref: &str,
    policy_oid: &str,
    grant_oid: &str,
    recorded_at: &str,
) -> JsonValue {
    envelope(
        "context_pack",
        entity_id,
        recorded_at,
        creator_id,
        "tool_recorded",
        json!({
            "base_commit": base_head,
            "base_ref_name": decision_ref,
            "expected_ref_head": base_head,
            "subject_refs": canonical_set(vec![json!(subject_id)]),
            "selected_context_refs": canonical_set(vec![json!(base_head)]),
            "must_preserve_constraints": ["Preserve creator ownership of the canonical decision Ref."],
            "allowed_transformations": canonical_set(vec![json!("image_proposal")]),
            "unresolved_questions": [],
            "policy_snapshot_ref": policy_oid,
            "delegation_grant_ref": grant_oid,
            "data_classification": "internal",
            "retrieval_method": "creator session base Commit"
        }),
    )
}

#[allow(clippy::too_many_arguments)]
fn ai_activity_record(
    entity_id: &str,
    agent_id: &str,
    creator_id: &str,
    subject_id: &str,
    timestamp: &str,
    context_oid: &str,
    grant_oid: &str,
    current_blob: &str,
    output_blob: &str,
) -> JsonValue {
    let mut record = envelope(
        "activity",
        entity_id,
        timestamp,
        agent_id,
        "tool_recorded",
        json!({
            "activity_kind": "ai_run",
            "actor_refs": canonical_set(vec![
                json!({ "role": "agent", "actor_ref": agent_id }),
                json!({ "role": "responsible_principal", "actor_ref": creator_id })
            ]),
            "subject_refs": canonical_set(vec![json!(subject_id)]),
            "input_refs": canonical_set(vec![
                json!({ "role": "context", "oid": context_oid }),
                json!({ "role": "source_image", "oid": current_blob })
            ]),
            "output_refs": canonical_set(vec![json!({ "role": "proposal", "oid": output_blob })]),
            "before_observation_refs": [],
            "after_observation_refs": [],
            "reversibility": "reversible",
            "summary": "Recorded a creator-supplied AI image proposal.",
            "side_effect_class": "none",
            "ai_run": {
                "agent_ref": agent_id,
                "responsible_principal_ref": creator_id,
                "context_pack_ref": context_oid,
                "delegation_grant_ref": grant_oid,
                "requested_capabilities": canonical_set(vec![json!("propose_branch"), json!("read_context")]),
                "required_human_gates": canonical_set(vec![json!("before_decision_ref"), json!("before_release_ref")]),
                "status": "proposal_ready",
                "reproducibility_class": "not_reproducible"
            }
        }),
    );
    record
        .as_object_mut()
        .expect("record envelope is an object")
        .insert(
            "valid_time".into(),
            json!({
                "kind": "unknown",
                "reason": "The caller-supplied AI output had no independently verified execution timestamp."
            }),
        );
    record
}

fn feedback_record(
    entity_id: &str,
    creator_id: &str,
    subject_id: &str,
    proposal_head: &str,
    disposition: CreatorDisposition,
    rationale: &str,
    recorded_at: &str,
) -> JsonValue {
    envelope(
        "decision_feedback",
        entity_id,
        recorded_at,
        creator_id,
        "self_declared",
        json!({
            "proposal_ref": proposal_head,
            "disposition": disposition.as_protocol_str(),
            "reason_codes": canonical_set(vec![json!(disposition.reason_code())]),
            "human_rationale": rationale,
            "applies_to_subjects": canonical_set(vec![json!(subject_id)]),
            "visibility": "private",
            "training_use_policy": "prohibited"
        }),
    )
}

fn manifest_tree(entries: JsonMap<String, JsonValue>) -> JsonValue {
    json!({
        "object_type": "tree",
        "schema_version": SCHEMA_VERSION,
        "entries": entries,
        "extensions": {}
    })
}

fn commit(
    kind: &str,
    parents: &[String],
    snapshot: &str,
    transitions: &[String],
    author: &str,
    authored_at: &str,
    message: &str,
) -> JsonValue {
    json!({
        "object_type": "commit",
        "schema_version": SCHEMA_VERSION,
        "commit_kind": kind,
        "parents": parents,
        "snapshot": snapshot,
        "transition_refs": canonical_set(transitions.iter().map(|value| json!(value)).collect()),
        "bound_declaration_refs": [],
        "author_ref": author,
        "authored_at": authored_at,
        "message": message,
        "extensions": {}
    })
}

#[cfg(test)]
mod tests {
    use super::{SessionIds, civil_from_days, entity_id, format_timestamp};

    #[test]
    fn timestamp_conversion_matches_epoch_and_leap_day() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(
            format_timestamp(0, 0).unwrap(),
            "1970-01-01T00:00:00.000000000Z"
        );
        assert_eq!(
            format_timestamp(951_782_400, 123).unwrap(),
            "2000-02-29T00:00:00.000000123Z"
        );
    }

    #[test]
    fn entity_ids_are_stable_uuid_v4_values() {
        let seed = [7; 32];
        let first = entity_id(&seed, "subject");
        assert_eq!(first, entity_id(&seed, "subject"));
        assert_ne!(first, entity_id(&seed, "creator"));
        assert!(first.starts_with("urn:uuid:"));
        assert_eq!(first.as_bytes()[23], b'4');
        assert!(matches!(first.as_bytes()[28], b'8' | b'9' | b'a' | b'b'));

        assert_ne!(
            SessionIds::fresh().unwrap().subject,
            SessionIds::fresh().unwrap().subject
        );
    }
}
