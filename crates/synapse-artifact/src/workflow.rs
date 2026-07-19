//! Trusted, one-proposal generic artifact workflow.

use super::{
    ArtifactDisposition, ArtifactError, ArtifactSourceAttribution, CONTRACT_NAME, CONTRACT_VERSION,
    RegularFileManifest, map_regular_files,
};
use serde_json::{Map as JsonMap, Value as JsonValue, json};
use sha2::{Digest, Sha256};
use std::error::Error;
use std::fmt;
use std::path::PathBuf;
use synapse_application::{
    AdmittedProposalHandle, AiAuthorityProfileConfig, AiExecutionContext, AiExecutor, Application,
    ApplicationError, AuthenticatedSession, AuthenticationFailure, Authenticator,
    DurableProposalBinding, ExecutedAiProposal, ExecutionFailure, HumanAuthorityProfileConfig,
    HumanAuthorityProfileHandle, HumanDecisionCandidate, ProjectSelector, RegisteredProject,
};
use synapse_canonical::{CoreError, canonical_bytes, parse_strict};
use synapse_core::{
    AiCapability, AiSideEffectClass, AuthorizationClock, Repository, RepositoryError,
    SystemAuthorizationClock,
};
use synapse_sqlite::{RefStoreError, RefUpdate, ReflogMetadata};

const SCHEMA_VERSION: &str = "0.1.0";
const AGENT_CREDENTIAL: &str = "generic-artifact-agent";
const HUMAN_CREDENTIAL: &str = "generic-artifact-human";
const PERMIT_TTL_NANOS: i128 = 60_000_000_000;
const MAX_OUTPUT_BYTES: i64 = 1_073_741_824;

/// Trusted configuration. A browser-facing request must never construct it.
#[derive(Clone)]
pub struct TrustedArtifactProjectConfig {
    repository: PathBuf,
    project_key: String,
    creator_display_name: String,
    agent_display_name: String,
    recorded_at: String,
    grant_expires_at: String,
}

impl TrustedArtifactProjectConfig {
    pub fn new(
        repository: impl Into<PathBuf>,
        project_key: impl Into<String>,
        creator_display_name: impl Into<String>,
        agent_display_name: impl Into<String>,
        recorded_at: impl Into<String>,
        grant_expires_at: impl Into<String>,
    ) -> Self {
        Self {
            repository: repository.into(),
            project_key: project_key.into(),
            creator_display_name: creator_display_name.into(),
            agent_display_name: agent_display_name.into(),
            recorded_at: recorded_at.into(),
            grant_expires_at: grant_expires_at.into(),
        }
    }

    pub fn project_key(&self) -> &str {
        &self.project_key
    }

    fn validate(&self) -> Result<()> {
        let mut project_key = self.project_key.bytes();
        if self.project_key.len() > 128
            || !project_key
                .next()
                .is_some_and(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
            || !project_key.all(|byte| {
                byte.is_ascii_lowercase()
                    || byte.is_ascii_digit()
                    || matches!(byte, b'-' | b'_' | b'.')
            })
        {
            return Err(WorkflowError::InvalidArgument(
                "project_key must match [a-z0-9][a-z0-9._-]{0,127}".into(),
            ));
        }
        for (label, value, max) in [
            (
                "creator_display_name",
                self.creator_display_name.as_str(),
                200,
            ),
            ("agent_display_name", self.agent_display_name.as_str(), 200),
            ("recorded_at", self.recorded_at.as_str(), 64),
            ("grant_expires_at", self.grant_expires_at.as_str(), 64),
        ] {
            if value.is_empty() || value.len() > max || value.chars().any(char::is_control) {
                return Err(WorkflowError::InvalidArgument(format!(
                    "{label} is empty, too long, or contains a control character"
                )));
            }
        }
        Ok(())
    }
}

impl fmt::Debug for TrustedArtifactProjectConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TrustedArtifactProjectConfig")
            .field("project_key", &self.project_key)
            .field("repository", &"<redacted>")
            .finish_non_exhaustive()
    }
}

/// Public-safe facts produced after Proposal admission.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ArtifactProposalReceipt {
    artifact_manifest_sha256: String,
    review_context_sha256: String,
    source_attribution: ArtifactSourceAttribution,
}

impl ArtifactProposalReceipt {
    pub const fn contract(&self) -> &'static str {
        CONTRACT_NAME
    }

    pub const fn contract_version(&self) -> u32 {
        CONTRACT_VERSION
    }

    pub fn artifact_manifest_sha256(&self) -> &str {
        &self.artifact_manifest_sha256
    }

    pub fn review_context_sha256(&self) -> &str {
        &self.review_context_sha256
    }

    pub const fn source_attribution(&self) -> ArtifactSourceAttribution {
        self.source_attribution
    }

    pub const fn execution_verified(&self) -> bool {
        self.source_attribution.execution_verified()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ArtifactDecisionReceipt {
    disposition: ArtifactDisposition,
    reviewed_artifact_manifest_sha256: String,
}

impl ArtifactDecisionReceipt {
    pub const fn contract(&self) -> &'static str {
        CONTRACT_NAME
    }

    pub const fn contract_version(&self) -> u32 {
        CONTRACT_VERSION
    }

    pub const fn disposition(&self) -> ArtifactDisposition {
        self.disposition
    }

    pub fn reviewed_artifact_manifest_sha256(&self) -> &str {
        &self.reviewed_artifact_manifest_sha256
    }

    pub const fn selected_snapshot(&self) -> &'static str {
        match self.disposition {
            ArtifactDisposition::AdoptedUnchanged => "proposal",
            ArtifactDisposition::Rejected | ArtifactDisposition::Deferred => "base",
        }
    }
}

#[derive(Clone, Eq, PartialEq)]
pub struct ArtifactDecisionOptions {
    pub disposition: ArtifactDisposition,
    pub private_rationale: Option<String>,
}

impl fmt::Debug for ArtifactDecisionOptions {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ArtifactDecisionOptions")
            .field("disposition", &self.disposition)
            .field("private_rationale", &"<redacted>")
            .finish()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PendingArtifactState {
    Ready,
    Deciding,
    Consumed,
    OutcomeUnknown,
}

type ArtifactApplication =
    Application<ArtifactAuthenticator, PreparedArtifactExecutor, SystemAuthorizationClock>;

/// Non-serializable, same-process authority for one generic artifact review.
#[must_use = "dropping the pending artifact leaves its Proposal incomplete"]
pub struct PendingArtifactProposal {
    application: ArtifactApplication,
    admitted_proposal: AdmittedProposalHandle,
    human_profile: HumanAuthorityProfileHandle,
    selector: ProjectSelector,
    repository_path: PathBuf,
    ids: WorkflowIds,
    proposal_ref: String,
    decision_ref: String,
    base_head: String,
    proposal_head: String,
    base_snapshot: String,
    proposal_snapshot: String,
    base_manifest_sha256: String,
    recorded_at: String,
    receipt: ArtifactProposalReceipt,
    state: PendingArtifactState,
}

impl fmt::Debug for PendingArtifactProposal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PendingArtifactProposal")
            .field("state", &self.state)
            .field("binding", &"<redacted>")
            .finish_non_exhaustive()
    }
}

impl PendingArtifactProposal {
    pub fn receipt(&self) -> &ArtifactProposalReceipt {
        &self.receipt
    }

    pub const fn state(&self) -> PendingArtifactState {
        self.state
    }

    /// Return a trusted server binding for durable journaling.
    ///
    /// This value contains Ref/head authority inputs and must never be exposed
    /// through the public receipt or accepted from a browser request.
    pub fn durable_binding(&self) -> DurableProposalBinding {
        DurableProposalBinding::new(
            self.selector.clone(),
            self.proposal_ref.clone(),
            self.proposal_head.clone(),
            self.decision_ref.clone(),
            self.base_head.clone(),
        )
    }
}

/// Bootstrap one empty repository, publish one generic Proposal, and retain
/// its exact same-process Human review authority.
pub fn begin_artifact_proposal(
    config: &TrustedArtifactProjectConfig,
    accepted: &RegularFileManifest,
    proposed: &RegularFileManifest,
    application_context_json: &[u8],
    source_attribution: ArtifactSourceAttribution,
) -> Result<PendingArtifactProposal> {
    config.validate()?;
    let canonical_context = canonical_review_context(application_context_json)?;
    let mut repository = Repository::open(&config.repository)?;
    if !repository.refs().list()?.is_empty() {
        return Err(WorkflowError::ProjectExists);
    }
    let ids = WorkflowIds::from_key(&config.project_key);
    let decision_ref = format!("decision/artifact/{}", config.project_key);
    let proposal_ref = format!("proposal/artifact/{}", config.project_key);
    let base_manifest_sha256 = artifact_manifest_sha256(accepted);
    let mapped_base = map_regular_files(&repository, accepted)?;
    let mapped_proposal = map_regular_files(&repository, proposed)?;
    let context_blob = repository.put_blob(canonical_context.as_slice())?.oid;

    let human_actor_oid = put_json(
        &repository,
        actor_record(
            &ids.human,
            &ids.human,
            &config.recorded_at,
            "human",
            &config.creator_display_name,
            None,
        ),
    )?;
    let ai_actor_oid = put_json(
        &repository,
        actor_record(
            &ids.agent,
            &ids.human,
            &config.recorded_at,
            "ai_agent",
            &config.agent_display_name,
            Some(json!({
                "provider": "application-owned",
                "model_id": "caller-supplied-output",
                "model_version": "generic-artifact-v1",
                "capabilities": canonical_set(vec![json!("propose_branch"), json!("read_context")])
            })),
        ),
    )?;
    let policy_oid = put_json(
        &repository,
        policy_record(
            &ids.policy,
            &ids.human,
            &ids.project,
            &decision_ref,
            &proposal_ref,
            &config.recorded_at,
        ),
    )?;
    let grant_oid = put_json(
        &repository,
        grant_record(
            &ids.grant,
            &ids.human,
            &ids.agent,
            &ids.project,
            &proposal_ref,
            &config.recorded_at,
            &config.grant_expires_at,
        ),
    )?;
    let subject_oid = put_json(
        &repository,
        subject_record(
            &ids.subject,
            &ids.human,
            &config.recorded_at,
            &config.project_key,
        ),
    )?;

    let mut control_entries = JsonMap::new();
    insert_entry(
        &mut control_entries,
        "creator.actor.json",
        "record",
        &human_actor_oid,
    );
    insert_entry(
        &mut control_entries,
        "agent.actor.json",
        "record",
        &ai_actor_oid,
    );
    insert_entry(&mut control_entries, "policy.json", "record", &policy_oid);
    insert_entry(&mut control_entries, "grant.json", "record", &grant_oid);
    insert_entry(&mut control_entries, "subject.json", "record", &subject_oid);
    insert_entry(
        &mut control_entries,
        "review-context.json",
        "blob",
        &context_blob,
    );
    let control_tree = put_json(&repository, manifest_tree(control_entries))?;
    let base_snapshot = artifact_snapshot(
        &repository,
        &mapped_base.site_tree_oid,
        Some((&control_tree, None)),
    )?;
    let base_head = put_json(
        &repository,
        commit(
            "checkpoint",
            &[],
            &base_snapshot,
            &[],
            &ids.human,
            &config.recorded_at,
            "Generic artifact project initialized",
        ),
    )?;
    let context_oid = put_json(
        &repository,
        context_record(
            &ids.context,
            &ids.human,
            &ids.subject,
            &base_head,
            &decision_ref,
            &policy_oid,
            &grant_oid,
            &context_blob,
            &config.recorded_at,
        ),
    )?;
    let activity_oid = put_json(
        &repository,
        activity_record(
            &ids.activity,
            &ids.agent,
            &ids.human,
            &ids.subject,
            &context_oid,
            &grant_oid,
            &context_blob,
            &mapped_proposal.site_tree_oid,
            source_attribution,
            &config.recorded_at,
        ),
    )?;
    let proposal_snapshot = artifact_snapshot(
        &repository,
        &mapped_proposal.site_tree_oid,
        Some((&base_snapshot, Some((&context_oid, &activity_oid)))),
    )?;
    let proposal_head = put_json(
        &repository,
        commit(
            "checkpoint",
            std::slice::from_ref(&base_head),
            &proposal_snapshot,
            std::slice::from_ref(&activity_oid),
            &ids.agent,
            &config.recorded_at,
            "Generic artifact Proposal; canonical Decision unchanged",
        ),
    )?;

    let observed = SystemAuthorizationClock
        .now_unix_nanos()
        .map_err(WorkflowError::Clock)?;
    let occurred_at = i64::try_from(observed)
        .map_err(|_| WorkflowError::Clock("current time exceeds reflog range".into()))?;
    repository.update_ref(RefUpdate {
        ref_name: &decision_ref,
        expected_head: None,
        new_head: &base_head,
        metadata: ReflogMetadata {
            occurred_at_unix_nanos: occurred_at,
            actor: Some(&ids.human),
            message: Some("initialize generic artifact project"),
        },
    })?;

    let selector = ProjectSelector::new(ids.project.clone());
    let application = Application::new(
        ArtifactAuthenticator {
            agent_id: ids.agent.clone(),
            human_id: ids.human.clone(),
        },
        PreparedArtifactExecutor {
            proposal_head: proposal_head.clone(),
            activity_oid: activity_oid.clone(),
        },
        SystemAuthorizationClock,
        PERMIT_TTL_NANOS,
        [RegisteredProject::new(selector.clone(), repository)],
    )?;
    application.grant_project_access(&selector, ids.agent.clone())?;
    application.grant_project_access(&selector, ids.human.clone())?;
    let ai_profile = application.register_authority_profile(AiAuthorityProfileConfig::new(
        selector.clone(),
        ids.agent.clone(),
        ids.human.clone(),
        decision_ref.clone(),
        ai_actor_oid,
        human_actor_oid.clone(),
        context_oid,
        proposal_ref.clone(),
        vec![AiCapability::ProposeBranch, AiCapability::ReadContext],
        vec![AiCapability::ProposeBranch, AiCapability::ReadContext],
        AiSideEffectClass::None,
    ))?;
    let human_profile = application.register_human_profile(HumanAuthorityProfileConfig::new(
        selector.clone(),
        ids.human.clone(),
        decision_ref.clone(),
        human_actor_oid,
        policy_oid,
    ))?;
    let execution = application.register_execution(&ai_profile)?;
    let permit = application.prepare_ai(AGENT_CREDENTIAL, &selector, &execution)?;
    let publication = application.execute_and_publish_ai(AGENT_CREDENTIAL, &permit)?;
    let (decision, admitted_proposal) = publication.into_parts();
    if decision.reflog.ref_name != proposal_ref
        || decision.reflog.new_head != proposal_head
        || decision.activity_oid != activity_oid
    {
        return Err(WorkflowError::Integrity(
            "Creative AI receipt does not match the prepared artifact Proposal".into(),
        ));
    }

    Ok(PendingArtifactProposal {
        application,
        admitted_proposal,
        human_profile,
        selector,
        repository_path: config.repository.clone(),
        ids,
        proposal_ref,
        decision_ref,
        base_head,
        proposal_head,
        base_snapshot,
        proposal_snapshot,
        base_manifest_sha256,
        recorded_at: config.recorded_at.clone(),
        receipt: ArtifactProposalReceipt {
            artifact_manifest_sha256: artifact_manifest_sha256(proposed),
            review_context_sha256: raw_sha256(&canonical_context),
            source_attribution,
        },
        state: PendingArtifactState::Ready,
    })
}

pub fn decide_artifact_proposal(
    pending: &mut PendingArtifactProposal,
    options: &ArtifactDecisionOptions,
) -> Result<ArtifactDecisionReceipt> {
    if pending.state != PendingArtifactState::Ready {
        return Err(WorkflowError::DecisionUnavailable);
    }
    let rationale = options
        .private_rationale
        .as_deref()
        .unwrap_or_else(|| default_rationale(options.disposition));
    if rationale.is_empty() || rationale.len() > 2_000 || rationale.chars().any(char::is_control) {
        return Err(WorkflowError::InvalidArgument(
            "private_rationale must contain 1..=2000 non-control bytes".into(),
        ));
    }
    let repository = Repository::open(&pending.repository_path)?;
    let feedback_oid = put_json(
        &repository,
        feedback_record(
            &pending.ids.feedback,
            &pending.ids.human,
            &pending.ids.subject,
            &pending.proposal_head,
            options.disposition,
            rationale,
            &pending.recorded_at,
        ),
    )?;
    let selected_snapshot = match options.disposition {
        ArtifactDisposition::AdoptedUnchanged => &pending.proposal_snapshot,
        ArtifactDisposition::Rejected | ArtifactDisposition::Deferred => &pending.base_snapshot,
    };
    let decision_head = put_json(
        &repository,
        commit(
            "decision",
            std::slice::from_ref(&pending.base_head),
            selected_snapshot,
            std::slice::from_ref(&feedback_oid),
            &pending.ids.human,
            &pending.recorded_at,
            "Human reviewed generic artifact Proposal",
        ),
    )?;
    drop(repository);

    let registration = pending.application.register_human_decision(
        &pending.human_profile,
        &pending.admitted_proposal,
        HumanDecisionCandidate::new(
            decision_head.clone(),
            feedback_oid.clone(),
            Some("generic artifact Human Decision"),
        ),
    )?;
    let permit = pending.application.prepare_human_decision(
        HUMAN_CREDENTIAL,
        &pending.selector,
        &registration,
    )?;
    pending.state = PendingArtifactState::Deciding;
    let receipt = match pending
        .application
        .publish_human_decision(HUMAN_CREDENTIAL, &permit)
    {
        Ok(receipt) => receipt,
        Err(error) => {
            pending.state = PendingArtifactState::OutcomeUnknown;
            return Err(error.into());
        }
    };
    if receipt.reflog.ref_name != pending.decision_ref
        || receipt.reflog.old_head.as_deref() != Some(pending.base_head.as_str())
        || receipt.reflog.new_head != decision_head
        || receipt.proposal_commit_oid != pending.proposal_head
        || receipt.decision_feedback_oid != feedback_oid
    {
        pending.state = PendingArtifactState::OutcomeUnknown;
        return Err(WorkflowError::Integrity(
            "Human Decision receipt does not match the prepared artifact lineage".into(),
        ));
    }
    pending.state = PendingArtifactState::Consumed;
    let reviewed_digest = match options.disposition {
        ArtifactDisposition::AdoptedUnchanged => pending.receipt.artifact_manifest_sha256.clone(),
        ArtifactDisposition::Rejected | ArtifactDisposition::Deferred => {
            pending.base_manifest_sha256.clone()
        }
    };
    Ok(ArtifactDecisionReceipt {
        disposition: options.disposition,
        reviewed_artifact_manifest_sha256: reviewed_digest,
    })
}

pub enum WorkflowError {
    InvalidArgument(String),
    ProjectExists,
    DecisionUnavailable,
    Integrity(String),
    Clock(String),
    Artifact(ArtifactError),
    Application(ApplicationError),
    Repository(RepositoryError),
    RefStore(RefStoreError),
    Canonical(CoreError),
    Json(serde_json::Error),
}

impl fmt::Debug for WorkflowError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("WorkflowError")
            .field("code", &self.code())
            .field("detail", &"<redacted>")
            .finish()
    }
}

impl WorkflowError {
    pub fn code(&self) -> &str {
        match self {
            Self::InvalidArgument(_) => "invalid_argument",
            Self::ProjectExists => "artifact_project_exists",
            Self::DecisionUnavailable => "artifact_decision_unavailable",
            Self::Integrity(_) => "artifact_integrity_error",
            Self::Clock(_) => "storage_error",
            Self::Artifact(error) => error.code(),
            Self::Application(error) => error.code(),
            Self::Repository(error) => error.code(),
            Self::RefStore(error) => error.code(),
            Self::Canonical(error) => error.code().as_str(),
            Self::Json(_) => "schema_invalid",
        }
    }
}

impl fmt::Display for WorkflowError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidArgument(message) | Self::Integrity(message) | Self::Clock(message) => {
                formatter.write_str(message)
            }
            Self::ProjectExists => formatter.write_str("artifact project already exists"),
            Self::DecisionUnavailable => formatter.write_str("artifact Decision is unavailable"),
            Self::Artifact(error) => {
                write!(formatter, "artifact operation failed ({})", error.code())
            }
            Self::Application(error) => {
                write!(formatter, "application operation failed ({})", error.code())
            }
            Self::Repository(error) => {
                write!(formatter, "repository operation failed ({})", error.code())
            }
            Self::RefStore(error) => {
                write!(formatter, "Ref operation failed ({})", error.code())
            }
            Self::Canonical(error) => {
                write!(
                    formatter,
                    "review context rejected ({})",
                    error.code().as_str()
                )
            }
            Self::Json(_) => formatter.write_str("internal JSON serialization failed"),
        }
    }
}

impl Error for WorkflowError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Artifact(error) => Some(error),
            Self::Application(error) => Some(error),
            Self::Repository(error) => Some(error),
            Self::RefStore(error) => Some(error),
            Self::Canonical(error) => Some(error),
            Self::Json(error) => Some(error),
            _ => None,
        }
    }
}

impl From<ArtifactError> for WorkflowError {
    fn from(error: ArtifactError) -> Self {
        Self::Artifact(error)
    }
}

impl From<ApplicationError> for WorkflowError {
    fn from(error: ApplicationError) -> Self {
        Self::Application(error)
    }
}

impl From<RepositoryError> for WorkflowError {
    fn from(error: RepositoryError) -> Self {
        Self::Repository(error)
    }
}

impl From<RefStoreError> for WorkflowError {
    fn from(error: RefStoreError) -> Self {
        Self::RefStore(error)
    }
}

impl From<CoreError> for WorkflowError {
    fn from(error: CoreError) -> Self {
        Self::Canonical(error)
    }
}

impl From<serde_json::Error> for WorkflowError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

pub type Result<T> = std::result::Result<T, WorkflowError>;

#[derive(Clone)]
struct ArtifactAuthenticator {
    agent_id: String,
    human_id: String,
}

impl Authenticator for ArtifactAuthenticator {
    type Credential = str;

    fn authenticate(
        &self,
        credential: &Self::Credential,
    ) -> std::result::Result<AuthenticatedSession, AuthenticationFailure> {
        match credential {
            AGENT_CREDENTIAL => AuthenticatedSession::new(&self.agent_id, "artifact-agent-session"),
            HUMAN_CREDENTIAL => AuthenticatedSession::new(&self.human_id, "artifact-human-session"),
            _ => Err(AuthenticationFailure),
        }
    }
}

#[derive(Clone)]
struct PreparedArtifactExecutor {
    proposal_head: String,
    activity_oid: String,
}

impl AiExecutor for PreparedArtifactExecutor {
    fn execute(
        &self,
        _context: &AiExecutionContext,
    ) -> std::result::Result<ExecutedAiProposal, ExecutionFailure> {
        Ok(ExecutedAiProposal::new(
            self.proposal_head.clone(),
            self.activity_oid.clone(),
            Some("generic artifact Proposal"),
        ))
    }
}

struct WorkflowIds {
    human: String,
    agent: String,
    project: String,
    subject: String,
    policy: String,
    grant: String,
    context: String,
    activity: String,
    feedback: String,
}

impl WorkflowIds {
    fn from_key(project_key: &str) -> Self {
        Self {
            human: entity_id(project_key, "human"),
            agent: entity_id(project_key, "agent"),
            project: entity_id(project_key, "project"),
            subject: entity_id(project_key, "subject"),
            policy: entity_id(project_key, "policy"),
            grant: entity_id(project_key, "grant"),
            context: entity_id(project_key, "context"),
            activity: entity_id(project_key, "activity"),
            feedback: entity_id(project_key, "feedback"),
        }
    }
}

fn entity_id(project_key: &str, role: &str) -> String {
    let mut hash = Sha256::new();
    hash.update(b"synapsegit-generic-artifact-entity-v1\0");
    hash.update(project_key.as_bytes());
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

fn put_json(repository: &Repository, value: JsonValue) -> Result<String> {
    Ok(repository.put_object(&serde_json::to_vec(&value)?)?.oid)
}

fn artifact_snapshot(
    repository: &Repository,
    site_tree: &str,
    control: Option<(&str, Option<(&str, &str)>)>,
) -> Result<String> {
    let mut entries = JsonMap::new();
    insert_entry(&mut entries, "site", "tree", site_tree);
    if let Some((base_or_control, proposal_records)) = control {
        let name = if proposal_records.is_some() {
            "base"
        } else {
            "control"
        };
        insert_entry(&mut entries, name, "tree", base_or_control);
        if let Some((context, activity)) = proposal_records {
            insert_entry(&mut entries, "context.json", "record", context);
            insert_entry(&mut entries, "activity.json", "record", activity);
        }
    }
    put_json(repository, manifest_tree(entries))
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
    ai_profile: Option<JsonValue>,
) -> JsonValue {
    let mut payload = json!({
        "actor_kind": actor_kind,
        "display_name": display_name,
    });
    if let Some(profile) = ai_profile {
        payload
            .as_object_mut()
            .expect("actor payload is an object")
            .insert("ai_profile".into(), profile);
    }
    envelope(
        "actor",
        entity_id,
        recorded_at,
        asserted_by,
        if actor_kind == "human" {
            "self_declared"
        } else {
            "tool_recorded"
        },
        payload,
    )
}

fn policy_record(
    entity_id: &str,
    human: &str,
    project: &str,
    decision_ref: &str,
    proposal_ref: &str,
    recorded_at: &str,
) -> JsonValue {
    envelope(
        "policy",
        entity_id,
        recorded_at,
        human,
        "self_declared",
        json!({
            "scope_refs": canonical_set(vec![json!(project)]),
            "rules": [
                {"rule_id":"allow-context-read","effect":"allow","action":"read","resource_selector":"project/**"},
                {"rule_id":"allow-artifact-proposal","effect":"allow","action":"propose","resource_selector":proposal_ref},
                {"rule_id":"gate-artifact-decision","effect":"require_human_gate","action":"publish","resource_selector":decision_ref,"human_gate":"before_decision_ref"}
            ],
            "default_effect": "deny"
        }),
    )
}

fn grant_record(
    entity_id: &str,
    human: &str,
    agent: &str,
    project: &str,
    proposal_ref: &str,
    recorded_at: &str,
    expires_at: &str,
) -> JsonValue {
    envelope(
        "delegation_grant",
        entity_id,
        recorded_at,
        human,
        "self_declared",
        json!({
            "principal_ref": human,
            "delegate_ref": agent,
            "project_ref": project,
            "purpose": "Record one bounded generic artifact Proposal.",
            "capabilities": canonical_set(vec![json!("propose_branch"), json!("read_context")]),
            "resource_selectors": canonical_set(vec![json!("project/**")]),
            "writable_ref_prefixes": canonical_set(vec![json!(proposal_ref)]),
            "data_classes": canonical_set(vec![json!("internal")]),
            "allowed_egress": [],
            "may_delegate": false,
            "max_child_depth": 0,
            "max_output_bytes": MAX_OUTPUT_BYTES,
            "required_human_gates": canonical_set(vec![json!("before_decision_ref"), json!("before_release_ref")]),
            "expires_at": expires_at
        }),
    )
}

fn subject_record(entity_id: &str, human: &str, recorded_at: &str, label: &str) -> JsonValue {
    envelope(
        "subject",
        entity_id,
        recorded_at,
        human,
        "self_declared",
        json!({
            "subject_kind": "digital",
            "label": label,
            "relation_refs": [],
            "spatial_frame_refs": []
        }),
    )
}

#[allow(clippy::too_many_arguments)]
fn context_record(
    entity_id: &str,
    human: &str,
    subject: &str,
    base_head: &str,
    decision_ref: &str,
    policy_oid: &str,
    grant_oid: &str,
    application_context_blob: &str,
    recorded_at: &str,
) -> JsonValue {
    envelope(
        "context_pack",
        entity_id,
        recorded_at,
        human,
        "tool_recorded",
        json!({
            "base_commit": base_head,
            "base_ref_name": decision_ref,
            "expected_ref_head": base_head,
            "subject_refs": canonical_set(vec![json!(subject)]),
            "selected_context_refs": canonical_set(vec![json!(base_head), json!(application_context_blob)]),
            "must_preserve_constraints": ["Preserve the accepted artifact base and protected controls."],
            "allowed_transformations": canonical_set(vec![json!("file_tree_proposal")]),
            "unresolved_questions": [],
            "policy_snapshot_ref": policy_oid,
            "delegation_grant_ref": grant_oid,
            "data_classification": "internal",
            "retrieval_method": "application-owned redacted context"
        }),
    )
}

#[allow(clippy::too_many_arguments)]
fn activity_record(
    entity_id: &str,
    agent: &str,
    human: &str,
    subject: &str,
    context_oid: &str,
    grant_oid: &str,
    context_blob: &str,
    output_tree: &str,
    attribution: ArtifactSourceAttribution,
    recorded_at: &str,
) -> JsonValue {
    let (summary, reproducibility) = match attribution {
        ArtifactSourceAttribution::CallerSuppliedAiAttributed => (
            "Recorded caller-supplied bytes as an AI-attributed generic artifact Proposal.",
            "not_reproducible",
        ),
    };
    let mut value = envelope(
        "activity",
        entity_id,
        recorded_at,
        agent,
        "tool_recorded",
        json!({
            "activity_kind": "ai_run",
            "actor_refs": canonical_set(vec![
                json!({"role":"agent","actor_ref":agent}),
                json!({"role":"responsible_principal","actor_ref":human})
            ]),
            "subject_refs": canonical_set(vec![json!(subject)]),
            "input_refs": canonical_set(vec![
                json!({"role":"context","oid":context_oid}),
                json!({"role":"application_context","oid":context_blob})
            ]),
            "output_refs": canonical_set(vec![json!({"role":"proposal","oid":output_tree})]),
            "before_observation_refs": [],
            "after_observation_refs": [],
            "reversibility": "reversible",
            "summary": summary,
            "side_effect_class": "none",
            "ai_run": {
                "agent_ref": agent,
                "responsible_principal_ref": human,
                "context_pack_ref": context_oid,
                "delegation_grant_ref": grant_oid,
                "requested_capabilities": canonical_set(vec![json!("propose_branch"), json!("read_context")]),
                "required_human_gates": canonical_set(vec![json!("before_decision_ref"), json!("before_release_ref")]),
                "status": "proposal_ready",
                "reproducibility_class": reproducibility
            }
        }),
    );
    value
        .as_object_mut()
        .expect("activity envelope is an object")
        .insert(
            "valid_time".into(),
            json!({"kind":"instant","at":recorded_at}),
        );
    value
}

fn feedback_record(
    entity_id: &str,
    human: &str,
    subject: &str,
    proposal_head: &str,
    disposition: ArtifactDisposition,
    rationale: &str,
    recorded_at: &str,
) -> JsonValue {
    envelope(
        "decision_feedback",
        entity_id,
        recorded_at,
        human,
        "self_declared",
        json!({
            "proposal_ref": proposal_head,
            "disposition": disposition_protocol(disposition),
            "reason_codes": ["unspecified"],
            "human_rationale": rationale,
            "applies_to_subjects": canonical_set(vec![json!(subject)]),
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

fn insert_entry(entries: &mut JsonMap<String, JsonValue>, name: &str, kind: &str, oid: &str) {
    entries.insert(name.into(), json!({"entry_kind":kind,"oid":oid}));
}

fn canonical_set(mut values: Vec<JsonValue>) -> Vec<JsonValue> {
    values.sort_by_cached_key(|value| {
        let bytes = serde_json::to_vec(value).expect("internal JSON serialization succeeds");
        let parsed = parse_strict(&bytes).expect("internal set member is strict JSON");
        canonical_bytes(&parsed).expect("internal set member fits canonical limits")
    });
    values
}

fn disposition_protocol(disposition: ArtifactDisposition) -> &'static str {
    match disposition {
        ArtifactDisposition::AdoptedUnchanged => "adopted_unchanged",
        ArtifactDisposition::Rejected => "rejected",
        ArtifactDisposition::Deferred => "deferred",
    }
}

fn default_rationale(disposition: ArtifactDisposition) -> &'static str {
    match disposition {
        ArtifactDisposition::AdoptedUnchanged => "The creator adopted the artifact unchanged.",
        ArtifactDisposition::Rejected => "The creator rejected the artifact.",
        ArtifactDisposition::Deferred => "The creator deferred the artifact.",
    }
}

/// Hash one validated manifest using the frozen generic-artifact v1 profile.
pub fn artifact_manifest_sha256(manifest: &RegularFileManifest) -> String {
    let mut hash = Sha256::new();
    hash.update(b"synapsegit-generic-artifact-manifest-v1\0");
    for (path, bytes) in &manifest.files {
        hash.update(u64::try_from(path.len()).unwrap_or(u64::MAX).to_be_bytes());
        hash.update(path.as_bytes());
        hash.update(u64::try_from(bytes.len()).unwrap_or(u64::MAX).to_be_bytes());
        hash.update(bytes);
    }
    hex(&hash.finalize())
}

/// Strictly canonicalize and hash the public-safe application review context.
pub fn review_context_sha256(application_context_json: &[u8]) -> Result<String> {
    Ok(raw_sha256(&canonical_review_context(
        application_context_json,
    )?))
}

fn canonical_review_context(application_context_json: &[u8]) -> Result<Vec<u8>> {
    let parsed = parse_strict(application_context_json)?;
    Ok(canonical_bytes(&parsed)?)
}

fn raw_sha256(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(char::from(DIGITS[usize::from(byte >> 4)]));
        output.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
    }
    output
}
