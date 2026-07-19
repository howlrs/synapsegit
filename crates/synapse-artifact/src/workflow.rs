//! Trusted, sequential generic artifact workflow.

use super::checkout::checkout_artifact_decision_in_repository;
use super::{
    ArtifactApprovalError, ArtifactApprovalRegistry, ArtifactCheckoutError, ArtifactCheckoutLimits,
    ArtifactDecisionApproval, ArtifactDisposition, ArtifactError, ArtifactLimits,
    ArtifactManifestEntry, ArtifactSourceAttribution, CONTRACT_NAME, CONTRACT_VERSION,
    RegularFileManifest, TrustedArtifactDecisionBinding, map_regular_files,
};
use serde_json::{Map as JsonMap, Value as JsonValue, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::error::Error;
use std::fmt;
use std::path::{Path, PathBuf};
use synapse_application::{
    AdmittedProposalHandle, AiAuthorityProfileConfig, AiExecutionContext, AiExecutor, Application,
    ApplicationError, AuthenticatedSession, AuthenticationFailure, Authenticator,
    DurableProposalBinding, ExecutedAiProposal, ExecutionFailure, HumanAuthorityProfileConfig,
    HumanAuthorityProfileHandle, HumanDecisionCandidate, HumanDecisionPermit, ProjectSelector,
    RegisteredExecutionHandle, RegisteredProject,
};
use synapse_canonical::{CoreError, ObjectKind, Value, canonical_bytes, parse_oid, parse_strict};
use synapse_core::{
    AiCapability, AiSideEffectClass, AuthorizationClock, RefSnapshot, Repository, RepositoryError,
    SystemAuthorizationClock,
};
use synapse_schema::CanonicalTimestamp;
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
    /// Creates trusted configuration while preserving the raw-string API.
    ///
    /// Timestamp validation is deliberately deferred until
    /// [`begin_artifact_proposal`], where it runs before the repository is
    /// opened or any CAS data is written. Both timestamps must use the exact
    /// `YYYY-MM-DDTHH:mm:ss.nnnnnnnnnZ` representation.
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

    /// Creates and validates trusted configuration without opening a repository.
    ///
    /// Prefer this constructor when timestamps originate as strings. It rejects
    /// non-canonical values and reversed Grant expiry at the configuration
    /// boundary while preserving [`Self::new`] for source compatibility.
    pub fn try_new(
        repository: impl Into<PathBuf>,
        project_key: impl Into<String>,
        creator_display_name: impl Into<String>,
        agent_display_name: impl Into<String>,
        recorded_at: impl Into<String>,
        grant_expires_at: impl Into<String>,
    ) -> Result<Self> {
        let config = Self::new(
            repository,
            project_key,
            creator_display_name,
            agent_display_name,
            recorded_at,
            grant_expires_at,
        );
        config.validate()?;
        Ok(config)
    }

    /// Creates trusted configuration from already validated timestamps.
    pub fn new_with_canonical_timestamps(
        repository: impl Into<PathBuf>,
        project_key: impl Into<String>,
        creator_display_name: impl Into<String>,
        agent_display_name: impl Into<String>,
        recorded_at: CanonicalTimestamp,
        grant_expires_at: CanonicalTimestamp,
    ) -> Self {
        Self::new(
            repository,
            project_key,
            creator_display_name,
            agent_display_name,
            recorded_at.as_str(),
            grant_expires_at.as_str(),
        )
    }

    pub fn project_key(&self) -> &str {
        &self.project_key
    }

    /// Return the deterministic trusted project selector for host-owned ACLs.
    /// Browser/request data must never construct project authority from it.
    pub fn project_selector(&self) -> ProjectSelector {
        ProjectSelector::new(entity_id(&self.project_key, "project"))
    }

    pub(crate) fn repository_path(&self) -> &Path {
        &self.repository
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
        let recorded_at = CanonicalTimestamp::parse(&self.recorded_at).map_err(|_| {
            WorkflowError::InvalidArgument(
                "recorded_at must use YYYY-MM-DDTHH:mm:ss.nnnnnnnnnZ and be a valid Gregorian date"
                    .into(),
            )
        })?;
        let grant_expires_at =
            CanonicalTimestamp::parse(&self.grant_expires_at).map_err(|_| {
                WorkflowError::InvalidArgument(
                    "grant_expires_at must use YYYY-MM-DDTHH:mm:ss.nnnnnnnnnZ and be a valid Gregorian date"
                        .into(),
                )
            })?;
        if grant_expires_at < recorded_at {
            return Err(WorkflowError::InvalidArgument(
                "grant_expires_at must be equal to or later than recorded_at".into(),
            ));
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

struct ArtifactProposalData {
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
    context_oid: String,
    activity_oid: String,
    receipt: ArtifactProposalReceipt,
}

impl ArtifactProposalData {
    fn durable_binding(&self) -> DurableProposalBinding {
        DurableProposalBinding::new(
            self.selector.clone(),
            self.proposal_ref.clone(),
            self.proposal_head.clone(),
            self.decision_ref.clone(),
            self.base_head.clone(),
        )
    }
}

/// Opaque prepared Proposal. Immutable objects exist, but no Proposal Ref has
/// been changed. The value is intentionally neither clonable nor serializable.
#[must_use = "a prepared artifact Proposal has not been published"]
pub struct PreparedArtifactProposal {
    application: ArtifactApplication,
    execution: RegisteredExecutionHandle,
    human_profile: HumanAuthorityProfileHandle,
    data: ArtifactProposalData,
    initialize_decision_ref: bool,
}

impl fmt::Debug for PreparedArtifactProposal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("PreparedArtifactProposal(<redacted trusted handle>)")
    }
}

impl PreparedArtifactProposal {
    pub fn receipt(&self) -> &ArtifactProposalReceipt {
        &self.data.receipt
    }

    pub fn durable_binding(&self) -> DurableProposalBinding {
        self.data.durable_binding()
    }

    pub fn context_oid(&self) -> &str {
        &self.data.context_oid
    }

    pub fn activity_oid(&self) -> &str {
        &self.data.activity_oid
    }

    pub fn base_snapshot_oid(&self) -> &str {
        &self.data.base_snapshot
    }

    pub fn proposal_snapshot_oid(&self) -> &str {
        &self.data.proposal_snapshot
    }
}

/// Non-serializable, same-process authority for one generic artifact review.
#[must_use = "dropping the pending artifact leaves its Proposal incomplete"]
pub struct PendingArtifactProposal {
    application: ArtifactApplication,
    human_profile: HumanAuthorityProfileHandle,
    admission: ArtifactProposalAdmission,
    data: ArtifactProposalData,
    state: PendingArtifactState,
}

enum ArtifactProposalAdmission {
    SameProcess(AdmittedProposalHandle),
    Recovered,
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
        &self.data.receipt
    }

    pub const fn state(&self) -> PendingArtifactState {
        self.state
    }

    /// Return a trusted server binding for durable journaling.
    ///
    /// This value contains Ref/head authority inputs and must never be exposed
    /// through the public receipt or accepted from a browser request.
    pub fn durable_binding(&self) -> DurableProposalBinding {
        self.data.durable_binding()
    }

    pub fn context_oid(&self) -> &str {
        &self.data.context_oid
    }

    pub fn activity_oid(&self) -> &str {
        &self.data.activity_oid
    }
}

/// Opaque approval-bound Human Decision ready for its one ordinary Core CAS.
#[must_use = "a prepared artifact Decision has not been published"]
pub struct PreparedArtifactDecision {
    permit: HumanDecisionPermit,
    binding: DurableProposalBinding,
    new_decision_head: String,
    feedback_oid: String,
    disposition: ArtifactDisposition,
    selected_snapshot_oid: String,
    reviewed_manifest_sha256: String,
}

impl fmt::Debug for PreparedArtifactDecision {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("PreparedArtifactDecision(<redacted trusted handle>)")
    }
}

impl PreparedArtifactDecision {
    pub fn durable_binding(&self) -> DurableProposalBinding {
        self.binding.clone()
    }

    pub fn proposal_head(&self) -> &str {
        self.binding.proposal_head()
    }

    pub fn expected_decision_head(&self) -> &str {
        self.binding.decision_head()
    }

    pub fn new_decision_head(&self) -> &str {
        &self.new_decision_head
    }

    pub fn feedback_oid(&self) -> &str {
        &self.feedback_oid
    }

    pub const fn disposition(&self) -> ArtifactDisposition {
        self.disposition
    }

    pub const fn selected_snapshot(&self) -> &'static str {
        disposition_snapshot(self.disposition)
    }

    pub fn selected_snapshot_oid(&self) -> &str {
        &self.selected_snapshot_oid
    }

    pub fn reviewed_artifact_manifest_sha256(&self) -> &str {
        &self.reviewed_manifest_sha256
    }
}

/// Exact trusted publication outcome retained for durable journaling.
pub struct ArtifactDecisionPublication {
    receipt: ArtifactDecisionReceipt,
    binding: DurableProposalBinding,
    new_decision_head: String,
    feedback_oid: String,
    selected_snapshot_oid: String,
}

impl fmt::Debug for ArtifactDecisionPublication {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ArtifactDecisionPublication(<redacted trusted outcome>)")
    }
}

impl ArtifactDecisionPublication {
    pub fn receipt(&self) -> &ArtifactDecisionReceipt {
        &self.receipt
    }

    pub fn durable_binding(&self) -> DurableProposalBinding {
        self.binding.clone()
    }

    pub fn proposal_head(&self) -> &str {
        self.binding.proposal_head()
    }

    pub fn expected_decision_head(&self) -> &str {
        self.binding.decision_head()
    }

    pub fn new_decision_head(&self) -> &str {
        &self.new_decision_head
    }

    pub fn feedback_oid(&self) -> &str {
        &self.feedback_oid
    }

    pub const fn disposition(&self) -> ArtifactDisposition {
        self.receipt.disposition
    }

    pub const fn selected_snapshot(&self) -> &'static str {
        disposition_snapshot(self.receipt.disposition)
    }

    pub fn selected_snapshot_oid(&self) -> &str {
        &self.selected_snapshot_oid
    }

    pub fn reviewed_artifact_manifest_sha256(&self) -> &str {
        &self.receipt.reviewed_artifact_manifest_sha256
    }

    pub fn into_receipt(self) -> ArtifactDecisionReceipt {
        self.receipt
    }
}

#[derive(Clone, Copy)]
enum ProposalBaseKind {
    Initial,
    Sequential,
}

#[derive(Clone, Copy)]
enum RecoveryState<'a> {
    Fresh,
    Prepared(&'a DurableProposalBinding),
    Published(&'a DurableProposalBinding),
}

struct ArtifactAuthorityOids {
    human_actor: String,
    ai_actor: String,
    policy: String,
    grant: String,
}

struct ExistingArtifactBase {
    snapshot: String,
    site_tree: String,
    authority: ArtifactAuthorityOids,
}

/// Prepare immutable objects for a first Proposal without changing any Ref.
pub fn prepare_artifact_proposal(
    config: &TrustedArtifactProjectConfig,
    accepted: &RegularFileManifest,
    proposed: &RegularFileManifest,
    application_context_json: &[u8],
    source_attribution: ArtifactSourceAttribution,
) -> Result<PreparedArtifactProposal> {
    prepare_artifact_proposal_internal(
        config,
        accepted,
        proposed,
        application_context_json,
        source_attribution,
        ProposalBaseKind::Initial,
        RecoveryState::Fresh,
        None,
    )
}

/// Prepare immutable objects for the next Proposal from the exact current
/// completed Decision. The accepted manifest must equal its verified checkout.
pub fn prepare_next_artifact_proposal(
    config: &TrustedArtifactProjectConfig,
    accepted: &RegularFileManifest,
    proposed: &RegularFileManifest,
    application_context_json: &[u8],
    source_attribution: ArtifactSourceAttribution,
) -> Result<PreparedArtifactProposal> {
    prepare_artifact_proposal_internal(
        config,
        accepted,
        proposed,
        application_context_json,
        source_attribution,
        ProposalBaseKind::Sequential,
        RecoveryState::Fresh,
        None,
    )
}

/// Prepare the next Proposal only if the canonical Decision Ref still has the
/// exact trusted head selected by the caller's control-plane transaction.
pub fn prepare_next_artifact_proposal_at_head(
    config: &TrustedArtifactProjectConfig,
    expected_decision_head: &str,
    accepted: &RegularFileManifest,
    proposed: &RegularFileManifest,
    application_context_json: &[u8],
    source_attribution: ArtifactSourceAttribution,
) -> Result<PreparedArtifactProposal> {
    if parse_oid(expected_decision_head).ok() != Some(ObjectKind::Commit) {
        return Err(WorkflowError::InvalidArgument(
            "expected_decision_head must be a Commit OID".into(),
        ));
    }
    prepare_artifact_proposal_internal(
        config,
        accepted,
        proposed,
        application_context_json,
        source_attribution,
        ProposalBaseKind::Sequential,
        RecoveryState::Fresh,
        Some(expected_decision_head),
    )
}

/// Recreate a pre-CAS prepared Proposal from trusted journal facts.
///
/// `binding` and `expected_proposal_manifest_sha256` are trusted control-plane
/// data. A request handler must not populate them from request fields.
#[allow(clippy::too_many_arguments)]
pub fn recover_prepared_artifact_proposal(
    config: &TrustedArtifactProjectConfig,
    binding: &DurableProposalBinding,
    expected_proposal_manifest_sha256: &str,
    accepted: &RegularFileManifest,
    proposed: &RegularFileManifest,
    application_context_json: &[u8],
    source_attribution: ArtifactSourceAttribution,
) -> Result<PreparedArtifactProposal> {
    require_manifest_digest(proposed, expected_proposal_manifest_sha256)?;
    let kind = recovery_base_kind(config, binding)?;
    prepare_artifact_proposal_internal(
        config,
        accepted,
        proposed,
        application_context_json,
        source_attribution,
        kind,
        RecoveryState::Prepared(binding),
        Some(binding.decision_head()),
    )
}

/// Rebind an already-published Proposal after restart without restoring an old
/// permit or `AdmittedProposalHandle`.
#[allow(clippy::too_many_arguments)]
pub fn recover_published_artifact_proposal(
    config: &TrustedArtifactProjectConfig,
    binding: &DurableProposalBinding,
    expected_proposal_manifest_sha256: &str,
) -> Result<PendingArtifactProposal> {
    derive_published_artifact_proposal(config, binding, expected_proposal_manifest_sha256)
}

/// Publish exactly the prepared Proposal using the ordinary Application/Core
/// path. Consuming the opaque handle makes every attempted publication one-shot.
pub fn publish_prepared_artifact_proposal(
    prepared: PreparedArtifactProposal,
) -> Result<PendingArtifactProposal> {
    if prepared.initialize_decision_ref {
        initialize_decision_ref_if_needed(&prepared.data)?;
    }
    let permit = prepared.application.prepare_ai(
        AGENT_CREDENTIAL,
        &prepared.data.selector,
        &prepared.execution,
    )?;
    let publication = prepared
        .application
        .execute_and_publish_ai(AGENT_CREDENTIAL, &permit)?;
    let (decision, admitted_proposal) = publication.into_parts();
    if decision.reflog.ref_name != prepared.data.proposal_ref
        || decision.reflog.old_head.is_some()
        || decision.reflog.new_head != prepared.data.proposal_head
        || decision.activity_oid != prepared.data.activity_oid
        || decision.context_pack_oid != prepared.data.context_oid
    {
        return Err(WorkflowError::Integrity(
            "Creative AI receipt does not match the prepared artifact Proposal".into(),
        ));
    }
    Ok(PendingArtifactProposal {
        application: prepared.application,
        human_profile: prepared.human_profile,
        admission: ArtifactProposalAdmission::SameProcess(admitted_proposal),
        data: prepared.data,
        state: PendingArtifactState::Ready,
    })
}

/// Compatibility wrapper for first-project prepare plus publish.
pub fn begin_artifact_proposal(
    config: &TrustedArtifactProjectConfig,
    accepted: &RegularFileManifest,
    proposed: &RegularFileManifest,
    application_context_json: &[u8],
    source_attribution: ArtifactSourceAttribution,
) -> Result<PendingArtifactProposal> {
    publish_prepared_artifact_proposal(prepare_artifact_proposal(
        config,
        accepted,
        proposed,
        application_context_json,
        source_attribution,
    )?)
}

/// Convenience wrapper for one sequential prepare plus publish.
pub fn begin_next_artifact_proposal(
    config: &TrustedArtifactProjectConfig,
    accepted: &RegularFileManifest,
    proposed: &RegularFileManifest,
    application_context_json: &[u8],
    source_attribution: ArtifactSourceAttribution,
) -> Result<PendingArtifactProposal> {
    publish_prepared_artifact_proposal(prepare_next_artifact_proposal(
        config,
        accepted,
        proposed,
        application_context_json,
        source_attribution,
    )?)
}

/// Convenience wrapper for exact-head sequential prepare plus publish.
pub fn begin_next_artifact_proposal_at_head(
    config: &TrustedArtifactProjectConfig,
    expected_decision_head: &str,
    accepted: &RegularFileManifest,
    proposed: &RegularFileManifest,
    application_context_json: &[u8],
    source_attribution: ArtifactSourceAttribution,
) -> Result<PendingArtifactProposal> {
    publish_prepared_artifact_proposal(prepare_next_artifact_proposal_at_head(
        config,
        expected_decision_head,
        accepted,
        proposed,
        application_context_json,
        source_attribution,
    )?)
}

/// Claim approval, recheck the live binding, create immutable Decision objects,
/// and issue an ordinary one-shot Human permit. No Decision Ref is changed.
pub fn prepare_artifact_decision<A, C>(
    approvals: &ArtifactApprovalRegistry<A, C>,
    credential: &A::Credential,
    approval: &ArtifactDecisionApproval,
    pending: &mut PendingArtifactProposal,
    options: &ArtifactDecisionOptions,
) -> Result<PreparedArtifactDecision>
where
    A: Authenticator,
    C: AuthorizationClock + Send + Sync,
{
    approvals.claim_artifact_decision(credential, approval, pending, options)?;
    let binding = pending.durable_binding();
    pending
        .application
        .verify_recovered_human_decision_binding(&pending.human_profile, &binding)?;

    let rationale = options
        .private_rationale
        .as_deref()
        .unwrap_or_else(|| default_rationale(options.disposition));
    if rationale.is_empty() || rationale.len() > 2_000 || rationale.chars().any(char::is_control) {
        return Err(WorkflowError::InvalidArgument(
            "private_rationale must contain 1..=2000 non-control bytes".into(),
        ));
    }
    let repository = Repository::open(&pending.data.repository_path)?;
    let feedback_oid = put_json(
        &repository,
        feedback_record(
            &pending.data.ids.feedback,
            &pending.data.ids.human,
            &pending.data.ids.subject,
            &pending.data.proposal_head,
            options.disposition,
            rationale,
            &pending.data.recorded_at,
        ),
    )?;
    let selected_snapshot_oid = match options.disposition {
        ArtifactDisposition::AdoptedUnchanged => pending.data.proposal_snapshot.clone(),
        ArtifactDisposition::Rejected | ArtifactDisposition::Deferred => {
            pending.data.base_snapshot.clone()
        }
    };
    let new_decision_head = put_json(
        &repository,
        commit(
            "decision",
            std::slice::from_ref(&pending.data.base_head),
            &selected_snapshot_oid,
            std::slice::from_ref(&feedback_oid),
            &pending.data.ids.human,
            &pending.data.recorded_at,
            "Human reviewed generic artifact Proposal",
        ),
    )?;
    drop(repository);

    let candidate = HumanDecisionCandidate::new(
        new_decision_head.clone(),
        feedback_oid.clone(),
        Some("generic artifact Human Decision"),
    );
    let registration = match &pending.admission {
        ArtifactProposalAdmission::SameProcess(admitted) => pending
            .application
            .register_human_decision(&pending.human_profile, admitted, candidate)?,
        ArtifactProposalAdmission::Recovered => pending
            .application
            .register_recovered_human_decision(&pending.human_profile, &binding, candidate)?,
    };
    let permit = pending.application.prepare_human_decision(
        HUMAN_CREDENTIAL,
        &pending.data.selector,
        &registration,
    )?;
    let reviewed_manifest_sha256 = match options.disposition {
        ArtifactDisposition::AdoptedUnchanged => {
            pending.data.receipt.artifact_manifest_sha256.clone()
        }
        ArtifactDisposition::Rejected | ArtifactDisposition::Deferred => {
            pending.data.base_manifest_sha256.clone()
        }
    };
    pending.state = PendingArtifactState::Deciding;
    Ok(PreparedArtifactDecision {
        permit,
        binding,
        new_decision_head,
        feedback_oid,
        disposition: options.disposition,
        selected_snapshot_oid,
        reviewed_manifest_sha256,
    })
}

/// Consume an approval-created prepared Decision and perform its exact CAS.
pub fn publish_prepared_artifact_decision(
    pending: &mut PendingArtifactProposal,
    prepared: PreparedArtifactDecision,
) -> Result<ArtifactDecisionPublication> {
    if pending.state != PendingArtifactState::Deciding
        || pending.durable_binding() != prepared.binding
    {
        pending.state = PendingArtifactState::OutcomeUnknown;
        return Err(WorkflowError::DecisionUnavailable);
    }
    let receipt = match pending
        .application
        .publish_human_decision(HUMAN_CREDENTIAL, &prepared.permit)
    {
        Ok(receipt) => receipt,
        Err(error) => {
            pending.state = PendingArtifactState::OutcomeUnknown;
            return Err(error.into());
        }
    };
    if receipt.reflog.ref_name != pending.data.decision_ref
        || receipt.reflog.old_head.as_deref() != Some(pending.data.base_head.as_str())
        || receipt.reflog.new_head != prepared.new_decision_head
        || receipt.proposal_commit_oid != pending.data.proposal_head
        || receipt.decision_feedback_oid != prepared.feedback_oid
    {
        pending.state = PendingArtifactState::OutcomeUnknown;
        return Err(WorkflowError::Integrity(
            "Human Decision receipt does not match the prepared artifact lineage".into(),
        ));
    }
    pending.state = PendingArtifactState::Consumed;
    Ok(ArtifactDecisionPublication {
        receipt: ArtifactDecisionReceipt {
            disposition: prepared.disposition,
            reviewed_artifact_manifest_sha256: prepared.reviewed_manifest_sha256,
        },
        binding: prepared.binding,
        new_decision_head: prepared.new_decision_head,
        feedback_oid: prepared.feedback_oid,
        selected_snapshot_oid: prepared.selected_snapshot_oid,
    })
}

/// Compatibility wrapper retaining the approval-required Decision route.
pub fn decide_artifact_proposal<A, C>(
    approvals: &ArtifactApprovalRegistry<A, C>,
    credential: &A::Credential,
    approval: &ArtifactDecisionApproval,
    pending: &mut PendingArtifactProposal,
    options: &ArtifactDecisionOptions,
) -> Result<ArtifactDecisionReceipt>
where
    A: Authenticator,
    C: AuthorizationClock + Send + Sync,
{
    let prepared = prepare_artifact_decision(approvals, credential, approval, pending, options)?;
    Ok(publish_prepared_artifact_decision(pending, prepared)?.into_receipt())
}

#[allow(clippy::too_many_arguments)]
fn prepare_artifact_proposal_internal(
    config: &TrustedArtifactProjectConfig,
    accepted: &RegularFileManifest,
    proposed: &RegularFileManifest,
    application_context_json: &[u8],
    source_attribution: ArtifactSourceAttribution,
    base_kind: ProposalBaseKind,
    recovery: RecoveryState<'_>,
    expected_decision_head: Option<&str>,
) -> Result<PreparedArtifactProposal> {
    config.validate()?;
    let now = SystemAuthorizationClock
        .now_unix_nanos()
        .map_err(WorkflowError::Clock)?;
    let expires = CanonicalTimestamp::parse(&config.grant_expires_at)
        .map_err(|_| WorkflowError::InvalidArgument("grant expiry is invalid".into()))?;
    if now >= expires.unix_nanos() {
        return Err(WorkflowError::AuthorityExpired);
    }
    let canonical_context = canonical_review_context(application_context_json)?;
    let decision_ref = format!("decision/artifact/{}", config.project_key);
    let proposal_namespace = format!("proposal/artifact/{}", config.project_key);
    let stable_ids = WorkflowIds::from_key_and_attempt(&config.project_key, "pending");

    let sequential_base = if matches!(base_kind, ProposalBaseKind::Sequential) {
        if !config.repository.exists() {
            return Err(WorkflowError::ProjectMissing);
        }
        let verifier = Repository::open(&config.repository)?;
        let current_head = verifier
            .refs()
            .get(&decision_ref)?
            .map(|record| record.head)
            .ok_or(WorkflowError::ProjectMissing)?;
        if expected_decision_head.is_some_and(|expected| expected != current_head) {
            return Err(WorkflowError::StaleBase);
        }
        validate_review_availability(
            &verifier,
            &proposal_namespace,
            &decision_ref,
            &current_head,
            recovery,
        )?;
        let existing = verify_existing_artifact_base(
            config,
            accepted,
            &verifier,
            &decision_ref,
            &proposal_namespace,
            &current_head,
        )?;
        Some((current_head, existing))
    } else {
        None
    };
    let repository = Repository::open(&config.repository)?;

    if matches!(base_kind, ProposalBaseKind::Initial)
        && matches!(recovery, RecoveryState::Fresh)
        && !repository.refs().list()?.is_empty()
    {
        return Err(WorkflowError::ProjectExists);
    }

    let base_manifest_sha256 = artifact_manifest_sha256(accepted);
    let (base_head, base_snapshot, authority, context_blob) = match base_kind {
        ProposalBaseKind::Initial => {
            let context_blob = repository.put_blob(canonical_context.as_slice())?.oid;
            let mapped_base = map_regular_files(&repository, accepted)?;
            let human_actor = put_json(
                &repository,
                actor_record(
                    &stable_ids.human,
                    &stable_ids.human,
                    &config.recorded_at,
                    "human",
                    &config.creator_display_name,
                    None,
                ),
            )?;
            let ai_actor = put_json(
                &repository,
                actor_record(
                    &stable_ids.agent,
                    &stable_ids.human,
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
            let proposal_selector = format!("{proposal_namespace}/**");
            let policy = put_json(
                &repository,
                policy_record(
                    &stable_ids.policy,
                    &stable_ids.human,
                    &stable_ids.project,
                    &decision_ref,
                    &proposal_selector,
                    &config.recorded_at,
                ),
            )?;
            let grant = put_json(
                &repository,
                grant_record(
                    &stable_ids.grant,
                    &stable_ids.human,
                    &stable_ids.agent,
                    &stable_ids.project,
                    &proposal_namespace,
                    &config.recorded_at,
                    &config.grant_expires_at,
                ),
            )?;
            let subject = put_json(
                &repository,
                subject_record(
                    &stable_ids.subject,
                    &stable_ids.human,
                    &config.recorded_at,
                    &config.project_key,
                ),
            )?;
            let mut control_entries = JsonMap::new();
            insert_entry(
                &mut control_entries,
                "creator.actor.json",
                "record",
                &human_actor,
            );
            insert_entry(
                &mut control_entries,
                "agent.actor.json",
                "record",
                &ai_actor,
            );
            insert_entry(&mut control_entries, "policy.json", "record", &policy);
            insert_entry(&mut control_entries, "grant.json", "record", &grant);
            insert_entry(&mut control_entries, "subject.json", "record", &subject);
            insert_entry(
                &mut control_entries,
                "review-context.json",
                "blob",
                &context_blob,
            );
            let control_tree = put_json(&repository, manifest_tree(control_entries))?;
            let snapshot = artifact_snapshot(
                &repository,
                &mapped_base.site_tree_oid,
                Some((&control_tree, None)),
            )?;
            let head = put_json(
                &repository,
                commit(
                    "checkpoint",
                    &[],
                    &snapshot,
                    &[],
                    &stable_ids.human,
                    &config.recorded_at,
                    "Generic artifact project initialized",
                ),
            )?;
            (
                head,
                snapshot,
                ArtifactAuthorityOids {
                    human_actor,
                    ai_actor,
                    policy,
                    grant,
                },
                context_blob,
            )
        }
        ProposalBaseKind::Sequential => {
            let (current_head, existing) = sequential_base.ok_or_else(integrity_error)?;
            let live_head = repository
                .refs()
                .get(&decision_ref)?
                .map(|record| record.head)
                .ok_or(WorkflowError::ProjectMissing)?;
            if live_head != current_head {
                return Err(WorkflowError::StaleBase);
            }
            validate_review_availability(
                &repository,
                &proposal_namespace,
                &decision_ref,
                &current_head,
                recovery,
            )?;
            let mapped_base = map_regular_files(&repository, accepted)?;
            if mapped_base.site_tree_oid != existing.site_tree {
                return Err(WorkflowError::AcceptedMismatch);
            }
            let context_blob = repository.put_blob(canonical_context.as_slice())?.oid;
            (
                current_head,
                existing.snapshot,
                existing.authority,
                context_blob,
            )
        }
    };

    let attempt = artifact_attempt_id(&config.project_key, &base_head);
    let ids = WorkflowIds::from_key_and_attempt(&config.project_key, &attempt);
    let proposal_ref = format!("{proposal_namespace}/{attempt}");
    let mapped_proposal = map_regular_files(&repository, proposed)?;
    let context_oid = put_json(
        &repository,
        context_record(
            &ids.context,
            &ids.human,
            &ids.subject,
            &base_head,
            &decision_ref,
            &authority.policy,
            &authority.grant,
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
            &authority.grant,
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
    let selector = ProjectSelector::new(ids.project.clone());
    let data = ArtifactProposalData {
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
        context_oid,
        activity_oid,
        receipt: ArtifactProposalReceipt {
            artifact_manifest_sha256: artifact_manifest_sha256(proposed),
            review_context_sha256: raw_sha256(&canonical_context),
            source_attribution,
        },
    };
    validate_recovery_binding_and_refs(&repository, &data, base_kind, recovery)?;
    install_prepared_application(
        repository,
        data,
        authority,
        matches!(base_kind, ProposalBaseKind::Initial),
    )
}

fn install_prepared_application(
    repository: Repository,
    data: ArtifactProposalData,
    authority: ArtifactAuthorityOids,
    initialize_decision_ref: bool,
) -> Result<PreparedArtifactProposal> {
    let application = Application::new(
        ArtifactAuthenticator {
            agent_id: data.ids.agent.clone(),
            human_id: data.ids.human.clone(),
        },
        PreparedArtifactExecutor {
            proposal_head: data.proposal_head.clone(),
            activity_oid: data.activity_oid.clone(),
        },
        SystemAuthorizationClock,
        PERMIT_TTL_NANOS,
        [RegisteredProject::new(data.selector.clone(), repository)],
    )?;
    application.grant_project_access(&data.selector, data.ids.agent.clone())?;
    application.grant_project_access(&data.selector, data.ids.human.clone())?;
    let ai_profile = application.register_authority_profile(AiAuthorityProfileConfig::new(
        data.selector.clone(),
        data.ids.agent.clone(),
        data.ids.human.clone(),
        data.decision_ref.clone(),
        authority.ai_actor,
        authority.human_actor.clone(),
        data.context_oid.clone(),
        data.proposal_ref.clone(),
        vec![AiCapability::ProposeBranch, AiCapability::ReadContext],
        vec![AiCapability::ProposeBranch, AiCapability::ReadContext],
        AiSideEffectClass::None,
    ))?;
    let human_profile = application.register_human_profile(HumanAuthorityProfileConfig::new(
        data.selector.clone(),
        data.ids.human.clone(),
        data.decision_ref.clone(),
        authority.human_actor,
        authority.policy,
    ))?;
    let execution = application.register_execution(&ai_profile)?;
    Ok(PreparedArtifactProposal {
        application,
        execution,
        human_profile,
        data,
        initialize_decision_ref,
    })
}

fn initialize_decision_ref_if_needed(data: &ArtifactProposalData) -> Result<()> {
    let mut repository = Repository::open(&data.repository_path)?;
    match repository.refs().get(&data.decision_ref)? {
        Some(record) if record.head == data.base_head => return Ok(()),
        Some(_) => return Err(WorkflowError::ProjectExists),
        None => {}
    }
    let observed = SystemAuthorizationClock
        .now_unix_nanos()
        .map_err(WorkflowError::Clock)?;
    let occurred_at = i64::try_from(observed)
        .map_err(|_| WorkflowError::Clock("current time exceeds reflog range".into()))?;
    repository.update_ref(RefUpdate {
        ref_name: &data.decision_ref,
        expected_head: None,
        new_head: &data.base_head,
        metadata: ReflogMetadata {
            occurred_at_unix_nanos: occurred_at,
            actor: Some(&data.ids.human),
            message: Some("initialize generic artifact project"),
        },
    })?;
    Ok(())
}

fn validate_recovery_binding_and_refs(
    repository: &Repository,
    data: &ArtifactProposalData,
    base_kind: ProposalBaseKind,
    recovery: RecoveryState<'_>,
) -> Result<()> {
    let expected = match recovery {
        RecoveryState::Fresh => return Ok(()),
        RecoveryState::Prepared(binding) | RecoveryState::Published(binding) => binding,
    };
    if data.durable_binding() != *expected {
        return Err(WorkflowError::RecoveryMismatch);
    }
    let decision = repository.refs().get(&data.decision_ref)?;
    let decision_valid = match base_kind {
        ProposalBaseKind::Initial => decision
            .as_ref()
            .is_none_or(|record| record.head == data.base_head),
        ProposalBaseKind::Sequential => decision
            .as_ref()
            .is_some_and(|record| record.head == data.base_head),
    };
    if !decision_valid {
        return Err(WorkflowError::RecoveryMismatch);
    }
    let proposal = repository.refs().get(&data.proposal_ref)?;
    let proposal_valid = match recovery {
        RecoveryState::Prepared(_) => proposal.is_none(),
        RecoveryState::Published(_) => proposal
            .as_ref()
            .is_some_and(|record| record.head == data.proposal_head),
        RecoveryState::Fresh => true,
    };
    if !proposal_valid {
        return Err(WorkflowError::RecoveryMismatch);
    }
    Ok(())
}

fn require_manifest_digest(manifest: &RegularFileManifest, expected_sha256: &str) -> Result<()> {
    if expected_sha256.len() != 64
        || !expected_sha256
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        || artifact_manifest_sha256(manifest) != expected_sha256
    {
        return Err(WorkflowError::RecoveryMismatch);
    }
    Ok(())
}

fn derive_published_artifact_proposal(
    config: &TrustedArtifactProjectConfig,
    binding: &DurableProposalBinding,
    expected_manifest_sha256: &str,
) -> Result<PendingArtifactProposal> {
    config.validate()?;
    validate_digest_text(expected_manifest_sha256)?;
    let selector = config.project_selector();
    let decision_ref = format!("decision/artifact/{}", config.project_key);
    let proposal_namespace = format!("proposal/artifact/{}", config.project_key);
    if binding.project() != &selector || binding.decision_ref_name() != decision_ref {
        return Err(WorkflowError::RecoveryMismatch);
    }
    let expected_attempt = artifact_attempt_id(&config.project_key, binding.decision_head());
    if binding.proposal_ref_name() != format!("{proposal_namespace}/{expected_attempt}") {
        return Err(WorkflowError::RecoveryMismatch);
    }

    let repository = Repository::open(&config.repository)?;
    if repository
        .refs()
        .get(binding.decision_ref_name())?
        .is_none_or(|record| record.head != binding.decision_head())
        || repository
            .refs()
            .get(binding.proposal_ref_name())?
            .is_none_or(|record| record.head != binding.proposal_head())
    {
        return Err(WorkflowError::RecoveryMismatch);
    }
    validate_review_availability(
        &repository,
        &proposal_namespace,
        &decision_ref,
        binding.decision_head(),
        RecoveryState::Published(binding),
    )?;

    let ids = WorkflowIds::from_key_and_attempt(&config.project_key, &expected_attempt);
    let base = load_structured_value(&repository, binding.decision_head(), ObjectKind::Commit)?;
    let base_snapshot = value_string(&base, "snapshot")?.to_owned();
    let proposal = load_structured_value(&repository, binding.proposal_head(), ObjectKind::Commit)?;
    let parents = value_array(&proposal, "parents")?;
    let transitions = value_array(&proposal, "transition_refs")?;
    if value_string(&proposal, "commit_kind")? != "checkpoint"
        || parents.len() != 1
        || parents[0].as_str() != Some(binding.decision_head())
        || transitions.len() != 1
        || !value_array(&proposal, "bound_declaration_refs")?.is_empty()
        || value_string(&proposal, "author_ref")? != ids.agent
    {
        return Err(WorkflowError::RecoveryMismatch);
    }
    let recorded_at = value_string(&proposal, "authored_at")?.to_owned();
    CanonicalTimestamp::parse(&recorded_at).map_err(|_| WorkflowError::RecoveryMismatch)?;
    let proposal_snapshot = value_string(&proposal, "snapshot")?.to_owned();
    let snapshot = load_structured_value(&repository, &proposal_snapshot, ObjectKind::Tree)?;
    if tree_entry_names(&snapshot)? != ["activity.json", "base", "context.json", "site"]
        || direct_entry(&snapshot, "base", ObjectKind::Tree)? != base_snapshot
    {
        return Err(WorkflowError::RecoveryMismatch);
    }
    let site = direct_entry(&snapshot, "site", ObjectKind::Tree)?.to_owned();
    let context_oid = direct_entry(&snapshot, "context.json", ObjectKind::Record)?.to_owned();
    let activity_oid = direct_entry(&snapshot, "activity.json", ObjectKind::Record)?.to_owned();
    if transitions[0].as_str() != Some(activity_oid.as_str()) {
        return Err(WorkflowError::RecoveryMismatch);
    }

    let context = load_structured_value(&repository, &context_oid, ObjectKind::Record)?;
    let context_payload = value_field(&context, "payload")?;
    if value_string(&context, "record_type")? != "context_pack"
        || value_string(&context, "entity_id")? != ids.context
        || value_string(&context, "asserted_by")? != ids.human
        || value_string(&context, "recorded_at")? != recorded_at
        || value_string(context_payload, "base_commit")? != binding.decision_head()
        || value_string(context_payload, "base_ref_name")? != binding.decision_ref_name()
        || value_string(context_payload, "expected_ref_head")? != binding.decision_head()
        || value_array(context_payload, "subject_refs")?.len() != 1
        || value_array(context_payload, "subject_refs")?[0].as_str() != Some(ids.subject.as_str())
    {
        return Err(WorkflowError::RecoveryMismatch);
    }
    let policy = value_string(context_payload, "policy_snapshot_ref")?.to_owned();
    let grant = value_string(context_payload, "delegation_grant_ref")?.to_owned();
    let selected_context = value_array(context_payload, "selected_context_refs")?;
    let application_context_blob = selected_context
        .iter()
        .filter_map(Value::as_str)
        .find(|oid| {
            *oid != binding.decision_head() && parse_oid(oid).ok() == Some(ObjectKind::Blob)
        })
        .ok_or(WorkflowError::RecoveryMismatch)?
        .to_owned();
    if selected_context.len() != 2
        || !selected_context
            .iter()
            .any(|value| value.as_str() == Some(binding.decision_head()))
    {
        return Err(WorkflowError::RecoveryMismatch);
    }

    let activity = load_structured_value(&repository, &activity_oid, ObjectKind::Record)?;
    let activity_payload = value_field(&activity, "payload")?;
    if value_string(&activity, "record_type")? != "activity"
        || value_string(&activity, "entity_id")? != ids.activity
        || value_string(&activity, "asserted_by")? != ids.agent
        || value_string(&activity, "recorded_at")? != recorded_at
        || value_string(activity_payload, "activity_kind")? != "ai_run"
        || value_string(activity_payload, "side_effect_class")? != "none"
        || value_string(activity_payload, "summary")?
            != "Recorded caller-supplied bytes as an AI-attributed generic artifact Proposal."
        || value_array(activity_payload, "input_refs")?.len() != 2
        || value_array(activity_payload, "output_refs")?.len() != 1
        || value_array(activity_payload, "actor_refs")?.len() != 2
        || value_array(activity_payload, "subject_refs")?.len() != 1
        || value_array(activity_payload, "subject_refs")?[0].as_str() != Some(ids.subject.as_str())
        || !role_oid_matches(activity_payload, "input_refs", "context", &context_oid)?
        || !role_oid_matches(
            activity_payload,
            "input_refs",
            "application_context",
            &application_context_blob,
        )?
        || !role_oid_matches(activity_payload, "output_refs", "proposal", &site)?
        || !role_actor_matches(activity_payload, "agent", &ids.agent)?
        || !role_actor_matches(activity_payload, "responsible_principal", &ids.human)?
    {
        return Err(WorkflowError::RecoveryMismatch);
    }
    let ai_run = value_field(activity_payload, "ai_run")?;
    if value_string(ai_run, "agent_ref")? != ids.agent
        || value_string(ai_run, "responsible_principal_ref")? != ids.human
        || value_string(ai_run, "context_pack_ref")? != context_oid
        || value_string(ai_run, "delegation_grant_ref")? != grant
        || value_string(ai_run, "status")? != "proposal_ready"
        || value_string(ai_run, "reproducibility_class")? != "not_reproducible"
    {
        return Err(WorkflowError::RecoveryMismatch);
    }
    let authority = verify_protected_authority(
        config,
        &repository,
        &base_snapshot,
        &ids,
        &policy,
        &grant,
        &decision_ref,
        &proposal_namespace,
    )
    .map_err(|error| match error {
        WorkflowError::UnsupportedProfile | WorkflowError::AcceptedMismatch => {
            WorkflowError::RecoveryMismatch
        }
        other => other,
    })?;

    let proposed_manifest = read_site_manifest(&repository, &site, ArtifactLimits::default())?;
    let proposed_digest = artifact_manifest_sha256(&proposed_manifest);
    if proposed_digest != expected_manifest_sha256 {
        return Err(WorkflowError::RecoveryMismatch);
    }
    let base_tree = load_structured_value(&repository, &base_snapshot, ObjectKind::Tree)?;
    let base_site = direct_entry(&base_tree, "site", ObjectKind::Tree)?;
    let base_manifest = read_site_manifest(&repository, base_site, ArtifactLimits::default())?;
    let context_bytes = repository
        .objects()
        .read_verified_blob_limited(
            &application_context_blob,
            u64::try_from(synapse_canonical::DEFAULT_MAX_STRUCTURED_BYTES).unwrap_or(u64::MAX),
        )
        .map_err(|_| WorkflowError::RecoveryMismatch)?
        .ok_or(WorkflowError::RecoveryMismatch)?;
    if canonical_review_context(&context_bytes)? != context_bytes {
        return Err(WorkflowError::RecoveryMismatch);
    }
    let data = ArtifactProposalData {
        selector,
        repository_path: config.repository.clone(),
        ids,
        proposal_ref: binding.proposal_ref_name().to_owned(),
        decision_ref,
        base_head: binding.decision_head().to_owned(),
        proposal_head: binding.proposal_head().to_owned(),
        base_snapshot,
        proposal_snapshot,
        base_manifest_sha256: artifact_manifest_sha256(&base_manifest),
        recorded_at,
        context_oid,
        activity_oid,
        receipt: ArtifactProposalReceipt {
            artifact_manifest_sha256: proposed_digest,
            review_context_sha256: raw_sha256(&context_bytes),
            source_attribution: ArtifactSourceAttribution::CallerSuppliedAiAttributed,
        },
    };
    install_recovered_pending(repository, data, authority, binding)
}

fn install_recovered_pending(
    repository: Repository,
    data: ArtifactProposalData,
    authority: ArtifactAuthorityOids,
    binding: &DurableProposalBinding,
) -> Result<PendingArtifactProposal> {
    let application = Application::new(
        ArtifactAuthenticator {
            agent_id: data.ids.agent.clone(),
            human_id: data.ids.human.clone(),
        },
        PreparedArtifactExecutor {
            proposal_head: data.proposal_head.clone(),
            activity_oid: data.activity_oid.clone(),
        },
        SystemAuthorizationClock,
        PERMIT_TTL_NANOS,
        [RegisteredProject::new(data.selector.clone(), repository)],
    )?;
    application.grant_project_access(&data.selector, data.ids.human.clone())?;
    let human_profile = application.register_human_profile(HumanAuthorityProfileConfig::new(
        data.selector.clone(),
        data.ids.human.clone(),
        data.decision_ref.clone(),
        authority.human_actor,
        authority.policy,
    ))?;
    application.verify_recovered_human_decision_binding(&human_profile, binding)?;
    Ok(PendingArtifactProposal {
        application,
        human_profile,
        admission: ArtifactProposalAdmission::Recovered,
        data,
        state: PendingArtifactState::Ready,
    })
}

fn validate_digest_text(value: &str) -> Result<()> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(WorkflowError::RecoveryMismatch);
    }
    Ok(())
}

fn role_oid_matches(value: &Value, field: &str, role: &str, oid: &str) -> Result<bool> {
    Ok(value_array(value, field)?.iter().any(|entry| {
        entry.get("role").and_then(Value::as_str) == Some(role)
            && entry.get("oid").and_then(Value::as_str) == Some(oid)
    }))
}

fn role_actor_matches(value: &Value, role: &str, actor: &str) -> Result<bool> {
    Ok(value_array(value, "actor_refs")?.iter().any(|entry| {
        entry.get("role").and_then(Value::as_str) == Some(role)
            && entry.get("actor_ref").and_then(Value::as_str) == Some(actor)
    }))
}

fn read_site_manifest(
    repository: &Repository,
    site_oid: &str,
    limits: ArtifactLimits,
) -> Result<RegularFileManifest> {
    struct PendingTree {
        oid: String,
        prefix: String,
        depth: usize,
    }

    let mut pending = vec![PendingTree {
        oid: site_oid.to_owned(),
        prefix: String::new(),
        depth: 0,
    }];
    let mut entries = Vec::new();
    let mut tree_nodes = 0_usize;
    let mut tree_edges = 0_usize;
    let mut total_bytes = 0_u64;
    while let Some(tree) = pending.pop() {
        tree_nodes = tree_nodes.checked_add(1).ok_or_else(integrity_error)?;
        if tree_nodes > 100_000 {
            return Err(WorkflowError::Integrity(
                "artifact recovery tree exceeds its node bound".into(),
            ));
        }
        let value = load_structured_value(repository, &tree.oid, ObjectKind::Tree)?;
        let children = value_field(&value, "entries")?
            .as_object()
            .ok_or_else(integrity_error)?;
        for (segment, entry) in children.iter().rev() {
            tree_edges = tree_edges.checked_add(1).ok_or_else(integrity_error)?;
            if tree_edges > 200_000 {
                return Err(WorkflowError::Integrity(
                    "artifact recovery tree exceeds its edge bound".into(),
                ));
            }
            let depth = tree.depth.checked_add(1).ok_or_else(integrity_error)?;
            if depth > limits.max_depth {
                return Err(WorkflowError::Artifact(ArtifactError::ResourceLimit(
                    "artifact path exceeds max_depth",
                )));
            }
            let path = if tree.prefix.is_empty() {
                segment.clone()
            } else {
                format!("{}/{segment}", tree.prefix)
            };
            if path.len() > limits.max_path_bytes {
                return Err(WorkflowError::Artifact(ArtifactError::ResourceLimit(
                    "artifact path exceeds max_path_bytes",
                )));
            }
            let fields = entry.as_object().ok_or_else(integrity_error)?;
            let kind = fields
                .iter()
                .find_map(|(field, value)| {
                    (field == "entry_kind").then(|| value.as_str()).flatten()
                })
                .ok_or_else(integrity_error)?;
            let oid = fields
                .iter()
                .find_map(|(field, value)| (field == "oid").then(|| value.as_str()).flatten())
                .ok_or_else(integrity_error)?;
            if fields.len() != 2 {
                return Err(integrity_error());
            }
            match kind {
                "tree" if parse_oid(oid)? == ObjectKind::Tree => pending.push(PendingTree {
                    oid: oid.to_owned(),
                    prefix: path,
                    depth,
                }),
                "blob" if parse_oid(oid)? == ObjectKind::Blob => {
                    if entries.len() >= limits.max_files {
                        return Err(WorkflowError::Artifact(ArtifactError::ResourceLimit(
                            "artifact exceeds max_files",
                        )));
                    }
                    let remaining = limits.max_total_bytes.saturating_sub(total_bytes);
                    let read_limit = limits.max_file_bytes.min(remaining).max(1);
                    let bytes = repository
                        .objects()
                        .read_verified_blob_limited(oid, read_limit)
                        .map_err(|_| integrity_error())?
                        .ok_or_else(integrity_error)?;
                    let byte_len = u64::try_from(bytes.len()).map_err(|_| integrity_error())?;
                    if byte_len > limits.max_file_bytes {
                        return Err(WorkflowError::Artifact(ArtifactError::ResourceLimit(
                            "artifact file exceeds max_file_bytes",
                        )));
                    }
                    total_bytes = total_bytes
                        .checked_add(byte_len)
                        .ok_or_else(integrity_error)?;
                    if total_bytes > limits.max_total_bytes {
                        return Err(WorkflowError::Artifact(ArtifactError::ResourceLimit(
                            "artifact exceeds max_total_bytes",
                        )));
                    }
                    entries.push(ArtifactManifestEntry::regular_file(path, bytes));
                }
                _ => return Err(integrity_error()),
            }
        }
    }
    RegularFileManifest::from_entries(entries, limits).map_err(WorkflowError::from)
}

const fn disposition_snapshot(disposition: ArtifactDisposition) -> &'static str {
    match disposition {
        ArtifactDisposition::AdoptedUnchanged => "proposal",
        ArtifactDisposition::Rejected | ArtifactDisposition::Deferred => "base",
    }
}

fn artifact_attempt_id(project_key: &str, decision_head: &str) -> String {
    let mut hash = Sha256::new();
    hash.update(b"synapsegit.generic-artifact-attempt.v1\0");
    hash.update(project_key.as_bytes());
    hash.update(b"\0");
    hash.update(decision_head.as_bytes());
    hex(&hash.finalize()[..16])
}

struct DecisionLineage {
    parent: String,
    snapshot: String,
    proposal_head: String,
    disposition: ArtifactDisposition,
    human_id: String,
}

fn recovery_base_kind(
    config: &TrustedArtifactProjectConfig,
    binding: &DurableProposalBinding,
) -> Result<ProposalBaseKind> {
    config.validate()?;
    if binding.project() != &config.project_selector()
        || binding.decision_ref_name() != format!("decision/artifact/{}", config.project_key)
    {
        return Err(WorkflowError::RecoveryMismatch);
    }
    let repository = Repository::open_existing_read_only(&config.repository)
        .map_err(|_| WorkflowError::ProjectMissing)?;
    let commit = load_structured_value(&repository, binding.decision_head(), ObjectKind::Commit)?;
    match value_string(&commit, "commit_kind")? {
        "checkpoint" => Ok(ProposalBaseKind::Initial),
        "decision" => Ok(ProposalBaseKind::Sequential),
        _ => Err(WorkflowError::RecoveryMismatch),
    }
}

fn validate_review_availability(
    repository: &Repository,
    proposal_namespace: &str,
    decision_ref: &str,
    current_decision_head: &str,
    recovery: RecoveryState<'_>,
) -> Result<()> {
    if let RecoveryState::Prepared(binding) | RecoveryState::Published(binding) = recovery
        && (binding.decision_ref_name() != decision_ref
            || binding.decision_head() != current_decision_head)
    {
        return Err(WorkflowError::RecoveryMismatch);
    }
    let decided = decided_proposal_heads(repository, current_decision_head)?;
    let mut active = repository
        .refs()
        .list()?
        .into_iter()
        .filter(|record| {
            record.name == proposal_namespace
                || record
                    .name
                    .strip_prefix(proposal_namespace)
                    .is_some_and(|suffix| suffix.starts_with('/'))
        })
        .filter(|record| !decided.contains(&record.head))
        .collect::<Vec<_>>();
    active.sort_by(|left, right| left.name.cmp(&right.name));
    match recovery {
        RecoveryState::Fresh | RecoveryState::Prepared(_) if active.is_empty() => Ok(()),
        RecoveryState::Published(binding)
            if active.len() == 1
                && active[0].name == binding.proposal_ref_name()
                && active[0].head == binding.proposal_head() =>
        {
            Ok(())
        }
        RecoveryState::Prepared(_) | RecoveryState::Published(_) => {
            Err(WorkflowError::RecoveryMismatch)
        }
        RecoveryState::Fresh => Err(WorkflowError::ReviewActive),
    }
}

fn decided_proposal_heads(repository: &Repository, head: &str) -> Result<BTreeSet<String>> {
    let mut decided = BTreeSet::new();
    let mut visited = BTreeSet::new();
    let mut cursor = head.to_owned();
    for _ in 0..10_000 {
        if !visited.insert(cursor.clone()) {
            return Err(WorkflowError::Integrity(
                "artifact Decision history contains a cycle".into(),
            ));
        }
        let commit = load_structured_value(repository, &cursor, ObjectKind::Commit)?;
        match value_string(&commit, "commit_kind")? {
            "checkpoint" => {
                if !value_array(&commit, "parents")?.is_empty()
                    || !value_array(&commit, "transition_refs")?.is_empty()
                {
                    return Err(WorkflowError::UnsupportedProfile);
                }
                return Ok(decided);
            }
            "decision" => {
                let parents = value_array(&commit, "parents")?;
                let transitions = value_array(&commit, "transition_refs")?;
                if parents.len() != 1 || transitions.len() != 1 {
                    return Err(WorkflowError::Integrity(
                        "artifact Decision history has invalid cardinality".into(),
                    ));
                }
                let feedback_oid = transitions[0].as_str().ok_or_else(integrity_error)?;
                let feedback = load_structured_value(repository, feedback_oid, ObjectKind::Record)?;
                if value_string(&feedback, "record_type")? != "decision_feedback" {
                    return Err(integrity_error());
                }
                decided.insert(
                    value_string(value_field(&feedback, "payload")?, "proposal_ref")?.to_owned(),
                );
                cursor = parents[0].as_str().ok_or_else(integrity_error)?.to_owned();
            }
            _ => return Err(WorkflowError::UnsupportedProfile),
        }
    }
    Err(WorkflowError::Integrity(
        "artifact Decision history exceeds its traversal bound".into(),
    ))
}

/// Prove that `descendant` reaches `ancestor` only through exact generic
/// artifact Decision steps. A merely well-typed parent Commit is insufficient:
/// every intervening Feedback, Proposal, Context, Activity, base snapshot, and
/// selected snapshot is revalidated. Malformed history fails closed.
pub(crate) fn verify_artifact_decision_descendant(
    config: &TrustedArtifactProjectConfig,
    repository: &Repository,
    refs: &RefSnapshot,
    decision_ref: &str,
    ancestor: &str,
    descendant: &str,
) -> Result<bool> {
    config.validate()?;
    let expected_decision_ref = format!("decision/artifact/{}", config.project_key);
    if decision_ref != expected_decision_ref
        || parse_oid(ancestor).ok() != Some(ObjectKind::Commit)
        || parse_oid(descendant).ok() != Some(ObjectKind::Commit)
    {
        return Err(WorkflowError::InvalidArgument(
            "artifact Decision descendant binding is invalid".into(),
        ));
    }
    if snapshot_ref_head(refs, decision_ref) != Some(descendant) {
        return Err(integrity_error());
    }
    let proposal_namespace = format!("proposal/artifact/{}", config.project_key);
    let stable_ids = WorkflowIds::from_key_and_attempt(&config.project_key, "history-check");
    let authority =
        expected_artifact_authority_oids(config, &stable_ids, decision_ref, &proposal_namespace)?;
    let mut cursor = descendant.to_owned();
    let mut visited = BTreeSet::new();
    let mut seen_ancestor = false;
    for _ in 0..10_000 {
        if !visited.insert(cursor.clone()) {
            return Err(integrity_error());
        }
        if cursor == ancestor {
            seen_ancestor = true;
        }
        match validate_artifact_history_commit(
            config,
            repository,
            refs,
            decision_ref,
            &proposal_namespace,
            &stable_ids,
            &authority,
            &cursor,
        )? {
            Some(parent) => cursor = parent,
            None => return Ok(seen_ancestor),
        }
    }
    Err(WorkflowError::Integrity(
        "artifact Decision history exceeds its traversal bound".into(),
    ))
}

struct ExpectedArtifactAuthorityOids {
    human_actor: String,
    ai_actor: String,
    policy: String,
    grant: String,
    subject: String,
}

fn expected_artifact_authority_oids(
    config: &TrustedArtifactProjectConfig,
    ids: &WorkflowIds,
    decision_ref: &str,
    proposal_namespace: &str,
) -> Result<ExpectedArtifactAuthorityOids> {
    let human_actor = generated_structured_oid(actor_record(
        &ids.human,
        &ids.human,
        &config.recorded_at,
        "human",
        &config.creator_display_name,
        None,
    ))?;
    let ai_actor = generated_structured_oid(actor_record(
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
    ))?;
    let proposal_selector = format!("{proposal_namespace}/**");
    let policy = generated_structured_oid(policy_record(
        &ids.policy,
        &ids.human,
        &ids.project,
        decision_ref,
        &proposal_selector,
        &config.recorded_at,
    ))?;
    let grant = generated_structured_oid(grant_record(
        &ids.grant,
        &ids.human,
        &ids.agent,
        &ids.project,
        proposal_namespace,
        &config.recorded_at,
        &config.grant_expires_at,
    ))?;
    let subject = generated_structured_oid(subject_record(
        &ids.subject,
        &ids.human,
        &config.recorded_at,
        &config.project_key,
    ))?;
    Ok(ExpectedArtifactAuthorityOids {
        human_actor,
        ai_actor,
        policy,
        grant,
        subject,
    })
}

fn generated_structured_oid(value: JsonValue) -> Result<String> {
    let parsed = parse_strict(&serde_json::to_vec(&value)?)?;
    synapse_schema::validate(&parsed).map_err(|_| integrity_error())?;
    synapse_canonical::structured_oid_unchecked(&parsed).map_err(WorkflowError::from)
}

/// Return the exact generic Decision parent, or `None` for the bootstrap
/// checkpoint after validating its fixed root shape.
#[allow(clippy::too_many_arguments)]
fn validate_artifact_history_commit(
    config: &TrustedArtifactProjectConfig,
    repository: &Repository,
    refs: &RefSnapshot,
    decision_ref: &str,
    proposal_namespace: &str,
    stable_ids: &WorkflowIds,
    authority: &ExpectedArtifactAuthorityOids,
    head: &str,
) -> Result<Option<String>> {
    let decision = load_structured_value(repository, head, ObjectKind::Commit)?;
    if !value_array(&decision, "bound_declaration_refs")?.is_empty() {
        return Err(integrity_error());
    }
    if value_string(&decision, "commit_kind")? == "checkpoint" {
        if !value_array(&decision, "parents")?.is_empty()
            || !value_array(&decision, "transition_refs")?.is_empty()
            || value_string(&decision, "author_ref")? != stable_ids.human
        {
            return Err(integrity_error());
        }
        let snapshot_oid = value_string(&decision, "snapshot")?;
        let snapshot = load_structured_value(repository, snapshot_oid, ObjectKind::Tree)?;
        if tree_entry_names(&snapshot)? != ["control", "site"] {
            return Err(integrity_error());
        }
        let control_oid = direct_entry(&snapshot, "control", ObjectKind::Tree)?;
        let site_oid = direct_entry(&snapshot, "site", ObjectKind::Tree)?;
        let control = load_structured_value(repository, control_oid, ObjectKind::Tree)?;
        if tree_entry_names(&control)?
            != [
                "agent.actor.json",
                "creator.actor.json",
                "grant.json",
                "policy.json",
                "review-context.json",
                "subject.json",
            ]
            || direct_entry(&control, "agent.actor.json", ObjectKind::Record)? != authority.ai_actor
            || direct_entry(&control, "creator.actor.json", ObjectKind::Record)?
                != authority.human_actor
            || direct_entry(&control, "grant.json", ObjectKind::Record)? != authority.grant
            || direct_entry(&control, "policy.json", ObjectKind::Record)? != authority.policy
            || direct_entry(&control, "subject.json", ObjectKind::Record)? != authority.subject
        {
            return Err(integrity_error());
        }
        let review_context =
            direct_entry(&control, "review-context.json", ObjectKind::Blob)?.to_owned();
        require_generated_value(
            &control,
            manifest_tree({
                let mut entries = JsonMap::new();
                insert_entry(
                    &mut entries,
                    "agent.actor.json",
                    "record",
                    &authority.ai_actor,
                );
                insert_entry(
                    &mut entries,
                    "creator.actor.json",
                    "record",
                    &authority.human_actor,
                );
                insert_entry(&mut entries, "grant.json", "record", &authority.grant);
                insert_entry(&mut entries, "policy.json", "record", &authority.policy);
                insert_entry(&mut entries, "review-context.json", "blob", &review_context);
                insert_entry(&mut entries, "subject.json", "record", &authority.subject);
                entries
            }),
        )?;
        require_generated_value(
            &snapshot,
            manifest_tree({
                let mut entries = JsonMap::new();
                insert_entry(&mut entries, "control", "tree", control_oid);
                insert_entry(&mut entries, "site", "tree", site_oid);
                entries
            }),
        )?;
        let authored_at = value_string(&decision, "authored_at")?;
        if authored_at != config.recorded_at {
            return Err(integrity_error());
        }
        require_generated_value(
            &decision,
            commit(
                "checkpoint",
                &[],
                snapshot_oid,
                &[],
                &stable_ids.human,
                authored_at,
                "Generic artifact project initialized",
            ),
        )?;
        let verified = verify_protected_authority(
            config,
            repository,
            snapshot_oid,
            stable_ids,
            &authority.policy,
            &authority.grant,
            decision_ref,
            proposal_namespace,
        )?;
        if verified.human_actor != authority.human_actor
            || verified.ai_actor != authority.ai_actor
            || verified.policy != authority.policy
            || verified.grant != authority.grant
        {
            return Err(integrity_error());
        }
        return Ok(None);
    }
    if value_string(&decision, "commit_kind")? != "decision" {
        return Err(integrity_error());
    }
    let parents = value_array(&decision, "parents")?;
    let transitions = value_array(&decision, "transition_refs")?;
    if parents.len() != 1 || transitions.len() != 1 {
        return Err(integrity_error());
    }
    let parent = parents[0].as_str().ok_or_else(integrity_error)?;
    if parse_oid(parent).ok() != Some(ObjectKind::Commit) {
        return Err(integrity_error());
    }
    let human = value_string(&decision, "author_ref")?;
    if human != stable_ids.human {
        return Err(integrity_error());
    }
    let authored_at = value_string(&decision, "authored_at")?;
    let feedback_oid = transitions[0].as_str().ok_or_else(integrity_error)?;
    let feedback = load_structured_value(repository, feedback_oid, ObjectKind::Record)?;
    if value_string(&feedback, "record_type")? != "decision_feedback"
        || value_string(&feedback, "origin")? != "self_declared"
        || value_string(&feedback, "asserted_by")? != human
    {
        return Err(integrity_error());
    }
    let feedback_payload = value_field(&feedback, "payload")?;
    let proposal_head = value_string(feedback_payload, "proposal_ref")?;
    let disposition = match value_string(feedback_payload, "disposition")? {
        "adopted_unchanged" => ArtifactDisposition::AdoptedUnchanged,
        "rejected" => ArtifactDisposition::Rejected,
        "deferred" => ArtifactDisposition::Deferred,
        _ => return Err(integrity_error()),
    };
    let rationale = value_string(feedback_payload, "human_rationale")?;
    if rationale.is_empty() || rationale.len() > 2_000 || rationale.chars().any(char::is_control) {
        return Err(integrity_error());
    }
    let proposal = load_structured_value(repository, proposal_head, ObjectKind::Commit)?;
    let proposal_parents = value_array(&proposal, "parents")?;
    let proposal_transitions = value_array(&proposal, "transition_refs")?;
    if value_string(&proposal, "commit_kind")? != "checkpoint"
        || proposal_parents.len() != 1
        || proposal_parents[0].as_str() != Some(parent)
        || proposal_transitions.len() != 1
        || !value_array(&proposal, "bound_declaration_refs")?.is_empty()
    {
        return Err(integrity_error());
    }
    let attempt = artifact_attempt_id(&config.project_key, parent);
    let ids = WorkflowIds::from_key_and_attempt(&config.project_key, &attempt);
    let proposal_ref = format!("{proposal_namespace}/{attempt}");
    if snapshot_ref_head(refs, &proposal_ref) != Some(proposal_head) {
        return Err(integrity_error());
    }
    let ai = value_string(&proposal, "author_ref")?;
    let proposal_authored_at = value_string(&proposal, "authored_at")?;
    if ai != ids.agent
        || authored_at != proposal_authored_at
        || proposal_authored_at != config.recorded_at
    {
        return Err(integrity_error());
    }
    let activity_oid = proposal_transitions[0]
        .as_str()
        .ok_or_else(integrity_error)?;
    let proposal_snapshot_oid = value_string(&proposal, "snapshot")?;
    let proposal_snapshot =
        load_structured_value(repository, proposal_snapshot_oid, ObjectKind::Tree)?;
    if tree_entry_names(&proposal_snapshot)? != ["activity.json", "base", "context.json", "site"]
        || direct_entry(&proposal_snapshot, "activity.json", ObjectKind::Record)? != activity_oid
    {
        return Err(integrity_error());
    }
    let context_oid = direct_entry(&proposal_snapshot, "context.json", ObjectKind::Record)?;
    let proposal_site = direct_entry(&proposal_snapshot, "site", ObjectKind::Tree)?;
    let parent_commit = load_structured_value(repository, parent, ObjectKind::Commit)?;
    let parent_snapshot = value_string(&parent_commit, "snapshot")?;
    if direct_entry(&proposal_snapshot, "base", ObjectKind::Tree)? != parent_snapshot {
        return Err(integrity_error());
    }
    let expected_selected = match disposition {
        ArtifactDisposition::AdoptedUnchanged => proposal_snapshot_oid,
        ArtifactDisposition::Rejected | ArtifactDisposition::Deferred => parent_snapshot,
    };
    if value_string(&decision, "snapshot")? != expected_selected {
        return Err(integrity_error());
    }

    let context = load_structured_value(repository, context_oid, ObjectKind::Record)?;
    let context_payload = value_field(&context, "payload")?;
    if value_string(&context, "record_type")? != "context_pack"
        || value_string(&context, "origin")? != "tool_recorded"
        || value_string(&context, "asserted_by")? != human
        || value_string(context_payload, "base_commit")? != parent
        || value_string(context_payload, "expected_ref_head")? != parent
        || value_string(context_payload, "base_ref_name")? != decision_ref
        || value_string(context_payload, "policy_snapshot_ref")? != authority.policy
        || value_string(context_payload, "delegation_grant_ref")? != authority.grant
    {
        return Err(integrity_error());
    }
    let selected_context_refs = value_array(context_payload, "selected_context_refs")?;
    if selected_context_refs.len() != 2 {
        return Err(integrity_error());
    }
    let context_blob = selected_context_refs
        .iter()
        .filter_map(Value::as_str)
        .find(|value| *value != parent)
        .filter(|value| parse_oid(value).ok() == Some(ObjectKind::Blob))
        .ok_or_else(integrity_error)?;
    require_generated_value(
        &context,
        context_record(
            &ids.context,
            &ids.human,
            &ids.subject,
            parent,
            decision_ref,
            &authority.policy,
            &authority.grant,
            context_blob,
            proposal_authored_at,
        ),
    )?;
    let activity = load_structured_value(repository, activity_oid, ObjectKind::Record)?;
    require_generated_value(
        &activity,
        activity_record(
            &ids.activity,
            &ids.agent,
            &ids.human,
            &ids.subject,
            context_oid,
            &authority.grant,
            context_blob,
            proposal_site,
            ArtifactSourceAttribution::CallerSuppliedAiAttributed,
            proposal_authored_at,
        ),
    )?;
    require_generated_value(
        &proposal_snapshot,
        manifest_tree({
            let mut entries = JsonMap::new();
            insert_entry(&mut entries, "activity.json", "record", activity_oid);
            insert_entry(&mut entries, "base", "tree", parent_snapshot);
            insert_entry(&mut entries, "context.json", "record", context_oid);
            insert_entry(&mut entries, "site", "tree", proposal_site);
            entries
        }),
    )?;
    require_generated_value(
        &proposal,
        commit(
            "checkpoint",
            &[parent.to_owned()],
            proposal_snapshot_oid,
            &[activity_oid.to_owned()],
            &ids.agent,
            proposal_authored_at,
            "Generic artifact Proposal; canonical Decision unchanged",
        ),
    )?;
    require_generated_value(
        &feedback,
        feedback_record(
            &ids.feedback,
            &ids.human,
            &ids.subject,
            proposal_head,
            disposition,
            rationale,
            proposal_authored_at,
        ),
    )?;
    require_generated_value(
        &decision,
        commit(
            "decision",
            &[parent.to_owned()],
            expected_selected,
            &[feedback_oid.to_owned()],
            &ids.human,
            proposal_authored_at,
            "Human reviewed generic artifact Proposal",
        ),
    )?;
    Ok(Some(parent.to_owned()))
}

fn snapshot_ref_head<'a>(snapshot: &'a RefSnapshot, name: &str) -> Option<&'a str> {
    snapshot
        .refs
        .binary_search_by(|record| record.name.as_str().cmp(name))
        .ok()
        .map(|index| snapshot.refs[index].head.as_str())
}

fn require_generated_value(actual: &Value, expected: JsonValue) -> Result<()> {
    let expected = parse_strict(&serde_json::to_vec(&expected)?)?;
    if actual != &expected {
        return Err(integrity_error());
    }
    Ok(())
}

fn verify_existing_artifact_base(
    config: &TrustedArtifactProjectConfig,
    accepted: &RegularFileManifest,
    repository: &Repository,
    decision_ref: &str,
    proposal_namespace: &str,
    decision_head: &str,
) -> Result<ExistingArtifactBase> {
    let decision = inspect_decision(repository, decision_head)?;
    let proposal_refs = repository
        .refs()
        .list()?
        .into_iter()
        .filter(|record| {
            (record.name == proposal_namespace
                || record
                    .name
                    .strip_prefix(proposal_namespace)
                    .is_some_and(|suffix| suffix.starts_with('/')))
                && record.head == decision.proposal_head
        })
        .collect::<Vec<_>>();
    if proposal_refs.len() != 1 {
        return Err(WorkflowError::Integrity(
            "completed artifact Decision has no unique retained Proposal Ref".into(),
        ));
    }
    let selector = config.project_selector();
    let previous = DurableProposalBinding::new(
        selector,
        proposal_refs[0].name.clone(),
        decision.proposal_head.clone(),
        decision_ref,
        decision.parent.clone(),
    );
    let digest = artifact_manifest_sha256(accepted);
    let checkout = checkout_artifact_decision_in_repository(
        repository,
        &TrustedArtifactDecisionBinding::new(
            &config.repository,
            &config.project_key,
            previous,
            decision_head,
            decision.disposition,
            &digest,
        ),
        ArtifactCheckoutLimits::default(),
    )
    .map_err(|error| {
        if error.code() == "artifact_digest_mismatch" {
            WorkflowError::AcceptedMismatch
        } else {
            WorkflowError::Checkout(error)
        }
    })?;
    if checkout.file_count() != accepted.files.len()
        || accepted.files.iter().any(|(path, bytes)| {
            checkout
                .bytes(path)
                .is_none_or(|checked| checked != bytes.as_ref())
        })
    {
        return Err(WorkflowError::AcceptedMismatch);
    }

    let proposal = load_structured_value(repository, &decision.proposal_head, ObjectKind::Commit)?;
    let proposal_snapshot_oid = value_string(&proposal, "snapshot")?;
    let proposal_snapshot =
        load_structured_value(repository, proposal_snapshot_oid, ObjectKind::Tree)?;
    let context_oid = direct_entry(&proposal_snapshot, "context.json", ObjectKind::Record)?;
    let activity_oid = direct_entry(&proposal_snapshot, "activity.json", ObjectKind::Record)?;
    let context = load_structured_value(repository, context_oid, ObjectKind::Record)?;
    let activity = load_structured_value(repository, activity_oid, ObjectKind::Record)?;
    let context_payload = value_field(&context, "payload")?;
    let policy = value_string(context_payload, "policy_snapshot_ref")?.to_owned();
    let grant = value_string(context_payload, "delegation_grant_ref")?.to_owned();
    let ids = WorkflowIds::from_key_and_attempt(&config.project_key, "profile-check");
    if decision.human_id != ids.human
        || value_string(&proposal, "author_ref")? != ids.agent
        || value_string(&activity, "asserted_by")? != ids.agent
    {
        return Err(WorkflowError::UnsupportedProfile);
    }
    let protected = verify_protected_authority(
        config,
        repository,
        &decision.snapshot,
        &ids,
        &policy,
        &grant,
        decision_ref,
        proposal_namespace,
    )?;
    Ok(ExistingArtifactBase {
        snapshot: decision.snapshot,
        site_tree: direct_entry(
            &load_structured_value(repository, decision_head, ObjectKind::Commit).and_then(
                |value| {
                    let snapshot = value_string(&value, "snapshot")?.to_owned();
                    load_structured_value(repository, &snapshot, ObjectKind::Tree)
                },
            )?,
            "site",
            ObjectKind::Tree,
        )?
        .to_owned(),
        authority: protected,
    })
}

fn inspect_decision(repository: &Repository, decision_head: &str) -> Result<DecisionLineage> {
    let decision = load_structured_value(repository, decision_head, ObjectKind::Commit)?;
    if value_string(&decision, "commit_kind")? != "decision" {
        return Err(WorkflowError::UnsupportedProfile);
    }
    let parents = value_array(&decision, "parents")?;
    let transitions = value_array(&decision, "transition_refs")?;
    if parents.len() != 1 || transitions.len() != 1 {
        return Err(integrity_error());
    }
    let feedback_oid = transitions[0].as_str().ok_or_else(integrity_error)?;
    let feedback = load_structured_value(repository, feedback_oid, ObjectKind::Record)?;
    if value_string(&feedback, "record_type")? != "decision_feedback" {
        return Err(integrity_error());
    }
    let payload = value_field(&feedback, "payload")?;
    let disposition = match value_string(payload, "disposition")? {
        "adopted_unchanged" => ArtifactDisposition::AdoptedUnchanged,
        "rejected" => ArtifactDisposition::Rejected,
        "deferred" => ArtifactDisposition::Deferred,
        _ => return Err(integrity_error()),
    };
    Ok(DecisionLineage {
        parent: parents[0].as_str().ok_or_else(integrity_error)?.to_owned(),
        snapshot: value_string(&decision, "snapshot")?.to_owned(),
        proposal_head: value_string(payload, "proposal_ref")?.to_owned(),
        disposition,
        human_id: value_string(&decision, "author_ref")?.to_owned(),
    })
}

#[allow(clippy::too_many_arguments)]
fn verify_protected_authority(
    config: &TrustedArtifactProjectConfig,
    repository: &Repository,
    selected_snapshot: &str,
    ids: &WorkflowIds,
    expected_policy: &str,
    expected_grant: &str,
    decision_ref: &str,
    proposal_namespace: &str,
) -> Result<ArtifactAuthorityOids> {
    let mut cursor = selected_snapshot.to_owned();
    let mut visited = BTreeSet::new();
    for _ in 0..10_000 {
        if !visited.insert(cursor.clone()) {
            return Err(integrity_error());
        }
        let snapshot = load_structured_value(repository, &cursor, ObjectKind::Tree)?;
        let names = tree_entry_names(&snapshot)?;
        if names == ["control", "site"] {
            let control_oid = direct_entry(&snapshot, "control", ObjectKind::Tree)?;
            let control = load_structured_value(repository, control_oid, ObjectKind::Tree)?;
            if tree_entry_names(&control)?
                != [
                    "agent.actor.json",
                    "creator.actor.json",
                    "grant.json",
                    "policy.json",
                    "review-context.json",
                    "subject.json",
                ]
            {
                return Err(WorkflowError::UnsupportedProfile);
            }
            let human_actor =
                direct_entry(&control, "creator.actor.json", ObjectKind::Record)?.to_owned();
            let ai_actor =
                direct_entry(&control, "agent.actor.json", ObjectKind::Record)?.to_owned();
            let policy = direct_entry(&control, "policy.json", ObjectKind::Record)?.to_owned();
            let grant = direct_entry(&control, "grant.json", ObjectKind::Record)?.to_owned();
            let subject = direct_entry(&control, "subject.json", ObjectKind::Record)?;
            let _ = direct_entry(&control, "review-context.json", ObjectKind::Blob)?;
            if policy != expected_policy || grant != expected_grant {
                return Err(WorkflowError::UnsupportedProfile);
            }
            verify_actor_profile(
                repository,
                &human_actor,
                &ids.human,
                "human",
                &config.creator_display_name,
            )?;
            verify_subject_profile(repository, subject, ids, config)?;
            verify_actor_profile(
                repository,
                &ai_actor,
                &ids.agent,
                "ai_agent",
                &config.agent_display_name,
            )?;
            verify_policy_profile(repository, &policy, ids, decision_ref, proposal_namespace)?;
            verify_grant_profile(repository, &grant, ids, proposal_namespace, config)?;
            return Ok(ArtifactAuthorityOids {
                human_actor,
                ai_actor,
                policy,
                grant,
            });
        }
        if names != ["activity.json", "base", "context.json", "site"] {
            return Err(WorkflowError::UnsupportedProfile);
        }
        cursor = direct_entry(&snapshot, "base", ObjectKind::Tree)?.to_owned();
    }
    Err(WorkflowError::Integrity(
        "protected artifact control chain exceeds its traversal bound".into(),
    ))
}

fn verify_subject_profile(
    repository: &Repository,
    oid: &str,
    ids: &WorkflowIds,
    config: &TrustedArtifactProjectConfig,
) -> Result<()> {
    let subject = load_structured_value(repository, oid, ObjectKind::Record)?;
    let payload = value_field(&subject, "payload")?;
    if value_string(&subject, "record_type")? != "subject"
        || value_string(&subject, "entity_id")? != ids.subject
        || value_string(&subject, "asserted_by")? != ids.human
        || value_string(payload, "subject_kind")? != "digital"
        || value_string(payload, "label")? != config.project_key
    {
        return Err(WorkflowError::UnsupportedProfile);
    }
    Ok(())
}

fn verify_actor_profile(
    repository: &Repository,
    oid: &str,
    entity_id: &str,
    actor_kind: &str,
    display_name: &str,
) -> Result<()> {
    let actor = load_structured_value(repository, oid, ObjectKind::Record)?;
    let payload = value_field(&actor, "payload")?;
    if value_string(&actor, "record_type")? != "actor"
        || value_string(&actor, "entity_id")? != entity_id
        || value_string(payload, "actor_kind")? != actor_kind
        || value_string(payload, "display_name")? != display_name
    {
        return Err(WorkflowError::UnsupportedProfile);
    }
    Ok(())
}

fn verify_policy_profile(
    repository: &Repository,
    oid: &str,
    ids: &WorkflowIds,
    decision_ref: &str,
    proposal_namespace: &str,
) -> Result<()> {
    let policy = load_structured_value(repository, oid, ObjectKind::Record)?;
    let payload = value_field(&policy, "payload")?;
    let expected_selector = format!("{proposal_namespace}/**");
    if value_string(&policy, "record_type")? != "policy"
        || value_string(&policy, "entity_id")? != ids.policy
        || value_string(&policy, "asserted_by")? != ids.human
        || value_string(payload, "default_effect")? != "deny"
        || !value_array(payload, "scope_refs")?
            .iter()
            .any(|value| value.as_str() == Some(ids.project.as_str()))
    {
        return Err(WorkflowError::UnsupportedProfile);
    }
    let rules = value_array(payload, "rules")?;
    let has_proposal = rules.iter().any(|rule| {
        rule.get("effect").and_then(Value::as_str) == Some("allow")
            && rule.get("action").and_then(Value::as_str) == Some("propose")
            && rule.get("resource_selector").and_then(Value::as_str)
                == Some(expected_selector.as_str())
    });
    let has_decision = rules.iter().any(|rule| {
        rule.get("effect").and_then(Value::as_str) == Some("require_human_gate")
            && rule.get("action").and_then(Value::as_str) == Some("publish")
            && rule.get("resource_selector").and_then(Value::as_str) == Some(decision_ref)
            && rule.get("human_gate").and_then(Value::as_str) == Some("before_decision_ref")
    });
    if !has_proposal || !has_decision {
        return Err(WorkflowError::UnsupportedProfile);
    }
    Ok(())
}

fn verify_grant_profile(
    repository: &Repository,
    oid: &str,
    ids: &WorkflowIds,
    proposal_namespace: &str,
    config: &TrustedArtifactProjectConfig,
) -> Result<()> {
    let grant = load_structured_value(repository, oid, ObjectKind::Record)?;
    let payload = value_field(&grant, "payload")?;
    let prefixes = value_array(payload, "writable_ref_prefixes")?;
    if value_string(&grant, "record_type")? != "delegation_grant"
        || value_string(&grant, "entity_id")? != ids.grant
        || value_string(&grant, "asserted_by")? != ids.human
        || value_string(payload, "principal_ref")? != ids.human
        || value_string(payload, "delegate_ref")? != ids.agent
        || value_string(payload, "project_ref")? != ids.project
        || value_string(payload, "expires_at")? != config.grant_expires_at
        || prefixes.len() != 1
        || prefixes[0].as_str() != Some(proposal_namespace)
    {
        return Err(WorkflowError::UnsupportedProfile);
    }
    Ok(())
}

fn tree_entry_names(value: &Value) -> Result<Vec<&str>> {
    let entries = value_field(value, "entries")?
        .as_object()
        .ok_or_else(integrity_error)?;
    Ok(entries.iter().map(|(name, _)| name.as_str()).collect())
}

fn load_structured_value(
    repository: &Repository,
    oid: &str,
    expected: ObjectKind,
) -> Result<Value> {
    if parse_oid(oid)? != expected {
        return Err(integrity_error());
    }
    let object = repository
        .objects()
        .get_verified(oid)
        .map_err(|_| integrity_error())?
        .ok_or_else(integrity_error)?;
    if object.kind() != expected {
        return Err(integrity_error());
    }
    let value = object.structured().cloned().ok_or_else(integrity_error)?;
    synapse_schema::validate(&value).map_err(|_| integrity_error())?;
    Ok(value)
}

fn direct_entry<'a>(value: &'a Value, name: &str, expected: ObjectKind) -> Result<&'a str> {
    let entries = value_field(value, "entries")?
        .as_object()
        .ok_or_else(integrity_error)?;
    let entry = entries
        .iter()
        .find_map(|(candidate, value)| (candidate == name).then_some(value))
        .ok_or_else(integrity_error)?;
    let fields = entry.as_object().ok_or_else(integrity_error)?;
    let kind = fields
        .iter()
        .find_map(|(field, value)| (field == "entry_kind").then(|| value.as_str()).flatten())
        .ok_or_else(integrity_error)?;
    let oid = fields
        .iter()
        .find_map(|(field, value)| (field == "oid").then(|| value.as_str()).flatten())
        .ok_or_else(integrity_error)?;
    if fields.len() != 2 || parse_oid(oid)? != expected || kind != object_kind_name(expected) {
        return Err(integrity_error());
    }
    Ok(oid)
}

fn object_kind_name(kind: ObjectKind) -> &'static str {
    match kind {
        ObjectKind::Blob => "blob",
        ObjectKind::Tree => "tree",
        ObjectKind::Commit => "commit",
        ObjectKind::Record => "record",
    }
}

fn value_field<'a>(value: &'a Value, field: &str) -> Result<&'a Value> {
    value.get(field).ok_or_else(integrity_error)
}

fn value_string<'a>(value: &'a Value, field: &str) -> Result<&'a str> {
    value_field(value, field)?
        .as_str()
        .ok_or_else(integrity_error)
}

fn value_array<'a>(value: &'a Value, field: &str) -> Result<&'a [Value]> {
    value_field(value, field)?
        .as_array()
        .ok_or_else(integrity_error)
}

fn integrity_error() -> WorkflowError {
    WorkflowError::Integrity("generic artifact lineage validation failed".into())
}

pub enum WorkflowError {
    InvalidArgument(String),
    ProjectExists,
    ProjectMissing,
    StaleBase,
    ReviewActive,
    AcceptedMismatch,
    UnsupportedProfile,
    AuthorityExpired,
    RecoveryMismatch,
    DecisionUnavailable,
    Integrity(String),
    Clock(String),
    Artifact(ArtifactError),
    Checkout(ArtifactCheckoutError),
    Approval(ArtifactApprovalError),
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
            Self::ProjectMissing => "artifact_project_missing",
            Self::StaleBase => "stale_base",
            Self::ReviewActive => "artifact_review_active",
            Self::AcceptedMismatch => "artifact_accepted_mismatch",
            Self::UnsupportedProfile => "artifact_profile_unsupported",
            Self::AuthorityExpired => "authorization_denied",
            Self::RecoveryMismatch => "artifact_recovery_mismatch",
            Self::DecisionUnavailable => "artifact_decision_unavailable",
            Self::Integrity(_) => "artifact_integrity_error",
            Self::Clock(_) => "storage_error",
            Self::Artifact(error) => error.code(),
            Self::Checkout(error) => error.code(),
            Self::Approval(error) => error.code(),
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
            Self::ProjectMissing => formatter.write_str("artifact project does not exist"),
            Self::StaleBase => formatter.write_str("artifact Decision base is stale"),
            Self::ReviewActive => formatter.write_str("an artifact review is already active"),
            Self::AcceptedMismatch => {
                formatter.write_str("accepted artifact does not match the current Decision")
            }
            Self::UnsupportedProfile => {
                formatter.write_str("artifact project profile does not support sequential review")
            }
            Self::AuthorityExpired => formatter.write_str("artifact proposal authority expired"),
            Self::RecoveryMismatch => {
                formatter.write_str("trusted artifact recovery facts do not match repository state")
            }
            Self::DecisionUnavailable => formatter.write_str("artifact Decision is unavailable"),
            Self::Artifact(error) => {
                write!(formatter, "artifact operation failed ({})", error.code())
            }
            Self::Checkout(error) => {
                write!(formatter, "artifact checkout failed ({})", error.code())
            }
            Self::Approval(error) => {
                write!(formatter, "artifact approval failed ({})", error.code())
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
            Self::Checkout(error) => Some(error),
            Self::Approval(error) => Some(error),
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

impl From<ArtifactCheckoutError> for WorkflowError {
    fn from(error: ArtifactCheckoutError) -> Self {
        Self::Checkout(error)
    }
}

impl From<ArtifactApprovalError> for WorkflowError {
    fn from(error: ArtifactApprovalError) -> Self {
        Self::Approval(error)
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
    fn from_key_and_attempt(project_key: &str, attempt: &str) -> Self {
        Self {
            human: entity_id(project_key, "human"),
            agent: entity_id(project_key, "agent"),
            project: entity_id(project_key, "project"),
            subject: entity_id(project_key, "subject"),
            policy: entity_id(project_key, "policy"),
            grant: entity_id(project_key, "grant"),
            context: attempt_entity_id(project_key, attempt, "context"),
            activity: attempt_entity_id(project_key, attempt, "activity"),
            feedback: attempt_entity_id(project_key, attempt, "feedback"),
        }
    }
}

fn attempt_entity_id(project_key: &str, attempt: &str, role: &str) -> String {
    let mut scoped_role = String::with_capacity(role.len() + attempt.len() + 1);
    scoped_role.push_str(role);
    scoped_role.push('\0');
    scoped_role.push_str(attempt);
    entity_id(project_key, &scoped_role)
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
            "purpose": "Record bounded sequential generic artifact Proposals.",
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
