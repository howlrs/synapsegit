//! Restart-durable orchestration for generic artifact reviews.
//!
//! The journal stores facts, never authority. Every public `ReviewId` lookup is
//! authenticated and project-authorized before SQLite is consulted, and every
//! restart path rebuilds fresh application authority from trusted project
//! configuration plus the live repository.

use crate::{
    ArtifactApprovalRegistry, ArtifactCheckoutLimits, ArtifactDecisionApproval,
    ArtifactDecisionOptions, ArtifactDecisionPublication, ArtifactDisposition,
    ArtifactProposalReceipt, ArtifactSourceAttribution, CheckedOutArtifact,
    PendingArtifactProposal, PendingArtifactState, PreparedArtifactDecision,
    PreparedArtifactProposal, RegularFileManifest, TrustedArtifactDecisionBinding,
    TrustedArtifactProjectConfig, WorkflowError, artifact_manifest_sha256,
    checkout_artifact_decision, prepare_artifact_decision, prepare_artifact_proposal,
    publish_prepared_artifact_decision, publish_prepared_artifact_proposal,
    recover_prepared_artifact_proposal, recover_published_artifact_proposal, review_context_sha256,
};
use std::error::Error;
use std::fmt;
use synapse_application::{Authenticator, DurableProposalBinding};
use synapse_artifact_journal::{
    DecisionCommitIntent, DecisionCommitIntentRequest, DecisionDisposition, DecisionOutcome,
    DecisionOutcomeRequest, JournalError, ProposalIntentId, ProposalIntentRequest, ReviewBinding,
    ReviewId, ReviewReconciliation, ReviewState, SelectedSnapshot, SqliteReviewJournal,
};
use synapse_core::{AuthorizationClock, Repository};

const PROPOSAL_REQUEST_DOMAIN: &[u8] = b"synapsegit.durable-artifact-proposal-request.v1\0";
const DECISION_REQUEST_DOMAIN: &[u8] = b"synapsegit.durable-artifact-decision-request.v1\0";
const MAX_IDEMPOTENCY_KEY_BYTES: usize = 4_096;

/// Public-safe opaque locator for one durable artifact review.
///
/// This value is not a credential or bearer capability. Authorization is
/// performed again on every function that resolves it through the journal.
#[derive(Clone, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ArtifactReviewId(ReviewId);

impl ArtifactReviewId {
    pub fn parse(value: impl Into<String>) -> DurableArtifactResult<Self> {
        ReviewId::parse(value)
            .map(Self)
            .map_err(DurableArtifactError::from_journal)
    }

    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }

    fn from_journal(value: &ReviewId) -> Self {
        Self(value.clone())
    }

    fn journal_id(&self) -> &ReviewId {
        &self.0
    }
}

impl fmt::Debug for ArtifactReviewId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ArtifactReviewId(<opaque>)")
    }
}

impl fmt::Display for ArtifactReviewId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Bounded, public-safe durable review state.
///
/// This is a getter-only application value, not a wire DTO. Embeddings must
/// map it into their frozen schema-specific transport type explicitly.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DurableArtifactReviewState {
    PendingReview,
    DecisionCommitted,
    TerminalDenial,
    RetryableFailure,
    OutcomeUnknown,
}

/// Public-safe selected snapshot label.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DurableArtifactSelectedSnapshot {
    Base,
    Proposal,
}

/// Public-safe exact Decision facts. No Ref, OID, path, rationale, or authority
/// data is present.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DurableArtifactDecisionStatus {
    disposition: ArtifactDisposition,
    selected_snapshot: DurableArtifactSelectedSnapshot,
    reviewed_artifact_manifest_sha256: String,
}

impl DurableArtifactDecisionStatus {
    pub const fn disposition(&self) -> ArtifactDisposition {
        self.disposition
    }

    pub const fn selected_snapshot(&self) -> DurableArtifactSelectedSnapshot {
        self.selected_snapshot
    }

    pub fn reviewed_artifact_manifest_sha256(&self) -> &str {
        &self.reviewed_artifact_manifest_sha256
    }
}

/// Public-safe query result for one durable review.
/// Getter-only application value; this deliberately does not implement Serde
/// and must not be mistaken for the frozen public review-status wire schema.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DurableArtifactReviewStatus {
    review_id: ArtifactReviewId,
    state: DurableArtifactReviewState,
    proposal_artifact_manifest_sha256: String,
    decision: Option<DurableArtifactDecisionStatus>,
}

impl DurableArtifactReviewStatus {
    pub fn review_id(&self) -> &ArtifactReviewId {
        &self.review_id
    }

    pub const fn state(&self) -> DurableArtifactReviewState {
        self.state
    }

    pub fn proposal_artifact_manifest_sha256(&self) -> &str {
        &self.proposal_artifact_manifest_sha256
    }

    pub fn decision(&self) -> Option<&DurableArtifactDecisionStatus> {
        self.decision.as_ref()
    }
}

/// Stable, redacted durable-orchestration failure.
#[derive(Clone, Eq, PartialEq)]
pub struct DurableArtifactError {
    code: String,
    message: &'static str,
}

impl DurableArtifactError {
    pub fn code(&self) -> &str {
        &self.code
    }

    fn new(code: impl Into<String>, message: &'static str) -> Self {
        Self {
            code: code.into(),
            message,
        }
    }

    fn invalid_argument() -> Self {
        Self::new("invalid_argument", "durable artifact request is invalid")
    }

    fn integrity() -> Self {
        Self::new(
            "artifact_durable_integrity_error",
            "durable artifact state failed integrity validation",
        )
    }

    fn review_not_found() -> Self {
        Self::new("artifact_review_not_found", "artifact review was not found")
    }

    fn outcome_unknown() -> Self {
        Self::new(
            "artifact_outcome_unknown",
            "artifact publication outcome is not safely retryable",
        )
    }

    fn from_workflow(error: WorkflowError) -> Self {
        Self::new(error.code(), "durable artifact workflow failed")
    }

    fn from_journal(error: JournalError) -> Self {
        match error {
            JournalError::ReviewNotFound | JournalError::ProposalIntentNotFound => {
                Self::review_not_found()
            }
            other => Self::new(other.code(), "durable artifact journal failed"),
        }
    }
}

impl fmt::Debug for DurableArtifactError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DurableArtifactError")
            .field("code", &self.code)
            .field("detail", &"<redacted>")
            .finish()
    }
}

impl fmt::Display for DurableArtifactError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.message)
    }
}

impl Error for DurableArtifactError {}

pub type DurableArtifactResult<T> = std::result::Result<T, DurableArtifactError>;

/// Immutable Proposal objects plus its private pre-CAS journal intent.
#[must_use = "a prepared durable Proposal has not been published"]
pub struct PreparedDurableArtifactProposal {
    prepared: PreparedArtifactProposal,
    proposal_intent_id: ProposalIntentId,
    binding: ReviewBinding,
}

impl fmt::Debug for PreparedDurableArtifactProposal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("PreparedDurableArtifactProposal(<redacted trusted handle>)")
    }
}

/// Proposal CAS is verified, but the public ReviewId may not yet be committed.
#[must_use = "a published durable Proposal still needs journal finalization"]
pub struct PublishedDurableArtifactProposal {
    pending: PendingArtifactProposal,
    proposal_intent_id: ProposalIntentId,
    binding: ReviewBinding,
}

impl fmt::Debug for PublishedDurableArtifactProposal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("PublishedDurableArtifactProposal(<redacted trusted outcome>)")
    }
}

/// Fresh process-local authority for one journal-backed pending review.
#[must_use = "a pending durable review has not received a Decision"]
pub struct DurablePendingArtifactReview {
    review_id: ArtifactReviewId,
    pending: PendingArtifactProposal,
}

impl DurablePendingArtifactReview {
    pub fn review_id(&self) -> &ArtifactReviewId {
        &self.review_id
    }

    pub fn proposal_receipt(&self) -> &ArtifactProposalReceipt {
        self.pending.receipt()
    }

    pub const fn state(&self) -> PendingArtifactState {
        self.pending.state()
    }

    /// Borrow the process-local pending value for approval issuance.
    pub fn pending(&self) -> &PendingArtifactProposal {
        &self.pending
    }
}

impl fmt::Debug for DurablePendingArtifactReview {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DurablePendingArtifactReview")
            .field("review_id", &self.review_id)
            .field("state", &self.pending.state())
            .finish()
    }
}

/// Result of reconciling a private Proposal intent after restart.
pub enum DurableArtifactProposalRecovery {
    Prepared(PreparedDurableArtifactProposal),
    Pending(DurablePendingArtifactReview),
}

impl fmt::Debug for DurableArtifactProposalRecovery {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Prepared(_) => formatter.write_str("DurableArtifactProposalRecovery::Prepared"),
            Self::Pending(review) => formatter
                .debug_tuple("DurableArtifactProposalRecovery::Pending")
                .field(review.review_id())
                .finish(),
        }
    }
}

/// Approval-bound Decision objects plus their exact durable journal intent.
#[must_use = "a prepared durable Decision has not been published"]
pub struct PreparedDurableArtifactDecision {
    review_id: ArtifactReviewId,
    prepared: PreparedArtifactDecision,
}

impl fmt::Debug for PreparedDurableArtifactDecision {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("PreparedDurableArtifactDecision(<redacted trusted handle>)")
    }
}

/// Verified Decision CAS awaiting exact outcome persistence.
#[must_use = "a published durable Decision still needs outcome persistence"]
pub struct PublishedDurableArtifactDecision {
    review_id: ArtifactReviewId,
    publication: ArtifactDecisionPublication,
}

impl fmt::Debug for PublishedDurableArtifactDecision {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("PublishedDurableArtifactDecision(<redacted trusted outcome>)")
    }
}

/// Reconciliation result. A committed Decision at the current canonical head
/// includes a full verified checkout for an Accepted-pointer update. A
/// committed review superseded by a verified sequential descendant retains
/// status but intentionally returns no historical checkout bytes.
pub struct ReconciledDurableArtifactReview {
    status: DurableArtifactReviewStatus,
    checkout_state: DurableArtifactCheckoutState,
    checked_out_artifact: Option<CheckedOutArtifact>,
}

/// Whether an exact committed checkout is available at the current Decision
/// head. Historical committed status remains queryable after a verified
/// sequential descendant supersedes it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DurableArtifactCheckoutState {
    Unavailable,
    Current,
    Superseded,
}

impl ReconciledDurableArtifactReview {
    pub fn status(&self) -> &DurableArtifactReviewStatus {
        &self.status
    }

    pub fn checked_out_artifact(&self) -> Option<&CheckedOutArtifact> {
        self.checked_out_artifact.as_ref()
    }

    pub const fn checkout_state(&self) -> DurableArtifactCheckoutState {
        self.checkout_state
    }

    pub fn into_checked_out_artifact(self) -> Option<CheckedOutArtifact> {
        self.checked_out_artifact
    }
}

impl fmt::Debug for ReconciledDurableArtifactReview {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ReconciledDurableArtifactReview")
            .field("status", &self.status)
            .field("checkout_state", &self.checkout_state)
            .field(
                "checked_out_artifact",
                &self.checked_out_artifact.as_ref().map(|_| "<verified>"),
            )
            .finish()
    }
}

/// Prepare immutable Proposal objects and durably register a private intent.
/// No public ReviewId is allocated and no Proposal Ref is changed.
#[allow(clippy::too_many_arguments)]
pub fn prepare_durable_artifact_proposal(
    journal: &mut SqliteReviewJournal,
    config: &TrustedArtifactProjectConfig,
    accepted: &RegularFileManifest,
    proposed: &RegularFileManifest,
    application_context_json: &[u8],
    source_attribution: ArtifactSourceAttribution,
    idempotency_key: &[u8],
) -> DurableArtifactResult<PreparedDurableArtifactProposal> {
    validate_idempotency_key(idempotency_key)?;
    let canonical_request = proposal_canonical_request(
        accepted,
        proposed,
        application_context_json,
        source_attribution,
    )?;
    let prepared = prepare_artifact_proposal(
        config,
        accepted,
        proposed,
        application_context_json,
        source_attribution,
    )
    .map_err(DurableArtifactError::from_workflow)?;
    let binding = review_binding(&prepared.durable_binding())?;
    let registered = journal
        .register_proposal_intent(ProposalIntentRequest {
            idempotency_key,
            canonical_request: &canonical_request,
            artifact_manifest_sha256: prepared.receipt().artifact_manifest_sha256(),
            binding: &binding,
        })
        .map_err(DurableArtifactError::from_journal)?;
    if registered.intent().review_id().is_some() {
        return Err(DurableArtifactError::new(
            "artifact_review_already_published",
            "artifact Proposal is already published",
        ));
    }
    Ok(PreparedDurableArtifactProposal {
        prepared,
        proposal_intent_id: registered.intent().proposal_intent_id().clone(),
        binding,
    })
}

/// Perform the ordinary Proposal CAS while retaining its private intent.
pub fn publish_prepared_durable_artifact_proposal(
    prepared: PreparedDurableArtifactProposal,
) -> DurableArtifactResult<PublishedDurableArtifactProposal> {
    let pending = publish_prepared_artifact_proposal(prepared.prepared)
        .map_err(DurableArtifactError::from_workflow)?;
    if review_binding(&pending.durable_binding())? != prepared.binding {
        return Err(DurableArtifactError::integrity());
    }
    Ok(PublishedDurableArtifactProposal {
        pending,
        proposal_intent_id: prepared.proposal_intent_id,
        binding: prepared.binding,
    })
}

/// Allocate the public ReviewId only after Proposal publication was verified.
pub fn commit_published_durable_artifact_proposal(
    journal: &mut SqliteReviewJournal,
    published: PublishedDurableArtifactProposal,
) -> DurableArtifactResult<DurablePendingArtifactReview> {
    if review_binding(&published.pending.durable_binding())? != published.binding {
        return Err(DurableArtifactError::integrity());
    }
    let registered = journal
        .commit_proposal_publication(&published.proposal_intent_id, &published.binding)
        .map_err(DurableArtifactError::from_journal)?;
    Ok(DurablePendingArtifactReview {
        review_id: ArtifactReviewId::from_journal(registered.review().review_id()),
        pending: published.pending,
    })
}

/// Reconcile the private Proposal intent selected by an idempotency key.
///
/// Before CAS this returns a freshly reconstructed prepared handle. After the
/// exact CAS it verifies the live repository, finalizes (or replays) the same
/// public ReviewId, and returns fresh pending authority. Authentication and
/// trusted-project ACL checks run before the private intent lookup.
#[allow(clippy::too_many_arguments)]
pub fn recover_durable_artifact_proposal<A, C>(
    journal: &mut SqliteReviewJournal,
    approvals: &ArtifactApprovalRegistry<A, C>,
    credential: &A::Credential,
    config: &TrustedArtifactProjectConfig,
    accepted: &RegularFileManifest,
    proposed: &RegularFileManifest,
    application_context_json: &[u8],
    source_attribution: ArtifactSourceAttribution,
    idempotency_key: &[u8],
) -> DurableArtifactResult<DurableArtifactProposalRecovery>
where
    A: Authenticator,
    C: AuthorizationClock + Send + Sync,
{
    approvals
        .authorize_project(credential, &config.project_selector())
        .map_err(|error| {
            DurableArtifactError::new(error.code(), "artifact review access failed")
        })?;
    validate_idempotency_key(idempotency_key)?;
    let canonical_request = proposal_canonical_request(
        accepted,
        proposed,
        application_context_json,
        source_attribution,
    )?;
    let project = config.project_selector();
    let intent = journal
        .get_proposal_intent_by_idempotency(project.as_str(), idempotency_key)
        .map_err(DurableArtifactError::from_journal)?
        .ok_or_else(DurableArtifactError::review_not_found)?;
    let binding = durable_binding(config, intent.binding())?;

    // Re-registering recomputes the request fingerprint and rejects reuse of
    // the same key with changed manifests/context before recovery proceeds.
    journal
        .register_proposal_intent(ProposalIntentRequest {
            idempotency_key,
            canonical_request: &canonical_request,
            artifact_manifest_sha256: artifact_manifest_sha256(proposed).as_str(),
            binding: intent.binding(),
        })
        .map_err(DurableArtifactError::from_journal)?;

    match inspect_cas(
        config,
        intent.binding().proposal_ref_name(),
        None,
        intent.binding().proposal_head(),
    )? {
        CasEvidence::NotCommitted => {
            if intent.review_id().is_some() {
                return Err(DurableArtifactError::integrity());
            }
            let prepared = recover_prepared_artifact_proposal(
                config,
                &binding,
                intent.artifact_manifest_sha256(),
                accepted,
                proposed,
                application_context_json,
                source_attribution,
            )
            .map_err(DurableArtifactError::from_workflow)?;
            Ok(DurableArtifactProposalRecovery::Prepared(
                PreparedDurableArtifactProposal {
                    prepared,
                    proposal_intent_id: intent.proposal_intent_id().clone(),
                    binding: intent.binding().clone(),
                },
            ))
        }
        CasEvidence::Committed => {
            let pending = recover_published_artifact_proposal(
                config,
                &binding,
                intent.artifact_manifest_sha256(),
            )
            .map_err(DurableArtifactError::from_workflow)?;
            let registered = journal
                .commit_proposal_publication(intent.proposal_intent_id(), intent.binding())
                .map_err(DurableArtifactError::from_journal)?;
            Ok(DurableArtifactProposalRecovery::Pending(
                DurablePendingArtifactReview {
                    review_id: ArtifactReviewId::from_journal(registered.review().review_id()),
                    pending,
                },
            ))
        }
        CasEvidence::StaleNotCommitted => Err(DurableArtifactError::new(
            "ref_conflict",
            "artifact Proposal Ref advanced before publication",
        )),
        CasEvidence::CommittedSuperseded | CasEvidence::Ambiguous => {
            Err(DurableArtifactError::outcome_unknown())
        }
    }
}

/// Authenticate and authorize the trusted project before resolving ReviewId.
pub fn get_durable_artifact_review_status<A, C>(
    approvals: &ArtifactApprovalRegistry<A, C>,
    credential: &A::Credential,
    journal: &mut SqliteReviewJournal,
    config: &TrustedArtifactProjectConfig,
    review_id: &ArtifactReviewId,
) -> DurableArtifactResult<DurableArtifactReviewStatus>
where
    A: Authenticator,
    C: AuthorizationClock + Send + Sync,
{
    approvals
        .authorize_project(credential, &config.project_selector())
        .map_err(|error| {
            DurableArtifactError::new(error.code(), "artifact review access failed")
        })?;
    let reconciliation = authorized_reconciliation(journal, config, review_id)?;
    status_from_reconciliation(&reconciliation)
}

/// Reconstruct fresh pending authority after process restart.
///
/// Old permits and admitted handles are neither accepted nor restored. The
/// workflow derives and validates the published Proposal directly from the
/// trusted binding and live canonical graph.
pub fn recover_durable_artifact_review<A, C>(
    approvals: &ArtifactApprovalRegistry<A, C>,
    credential: &A::Credential,
    journal: &mut SqliteReviewJournal,
    config: &TrustedArtifactProjectConfig,
    review_id: &ArtifactReviewId,
) -> DurableArtifactResult<DurablePendingArtifactReview>
where
    A: Authenticator,
    C: AuthorizationClock + Send + Sync,
{
    approvals
        .authorize_project(credential, &config.project_selector())
        .map_err(|error| {
            DurableArtifactError::new(error.code(), "artifact review access failed")
        })?;
    let reconciliation = authorized_reconciliation(journal, config, review_id)?;
    if !matches!(
        reconciliation.review().state(),
        ReviewState::PendingReview | ReviewState::RetryableFailure
    ) {
        return Err(DurableArtifactError::new(
            reconciliation.review().state().as_str(),
            "artifact review cannot be recovered for a new Decision",
        ));
    }
    let digest = reconciliation
        .proposal_artifact_manifest_sha256()
        .ok_or_else(DurableArtifactError::integrity)?;
    let binding = durable_binding(config, reconciliation.review().binding())?;
    let pending = recover_published_artifact_proposal(config, &binding, digest)
        .map_err(DurableArtifactError::from_workflow)?;
    Ok(DurablePendingArtifactReview {
        review_id: review_id.clone(),
        pending,
    })
}

/// Claim one host approval, prepare immutable Decision objects, and durably
/// register their exact pre-CAS intent.
#[allow(clippy::too_many_arguments)]
pub fn prepare_durable_artifact_decision<A, C>(
    journal: &mut SqliteReviewJournal,
    approvals: &ArtifactApprovalRegistry<A, C>,
    credential: &A::Credential,
    approval: &ArtifactDecisionApproval,
    review: &mut DurablePendingArtifactReview,
    options: &ArtifactDecisionOptions,
    idempotency_key: &[u8],
) -> DurableArtifactResult<PreparedDurableArtifactDecision>
where
    A: Authenticator,
    C: AuthorizationClock + Send + Sync,
{
    let binding = review.pending.durable_binding();
    approvals
        .authorize_project(credential, binding.project())
        .map_err(|error| {
            DurableArtifactError::new(error.code(), "artifact review access failed")
        })?;
    let reconciliation = journal
        .get_review_reconciliation(review.review_id.journal_id())
        .map_err(DurableArtifactError::from_journal)?;
    if reconciliation.review().binding() != &review_binding(&binding)? {
        return Err(DurableArtifactError::review_not_found());
    }
    if !matches!(
        reconciliation.review().state(),
        ReviewState::PendingReview | ReviewState::RetryableFailure
    ) {
        return Err(DurableArtifactError::new(
            reconciliation.review().state().as_str(),
            "artifact review is not decisionable",
        ));
    }
    validate_idempotency_key(idempotency_key)?;
    let canonical_request = decision_canonical_request(options)?;
    let prepared = prepare_artifact_decision(
        approvals,
        credential,
        approval,
        &mut review.pending,
        options,
    )
    .map_err(DurableArtifactError::from_workflow)?;
    let registered = journal
        .register_decision_commit_intent(
            review.review_id.journal_id(),
            DecisionCommitIntentRequest {
                idempotency_key,
                canonical_request: &canonical_request,
                disposition: journal_disposition(prepared.disposition()),
                selected_snapshot: journal_selected_snapshot(prepared.disposition()),
                reviewed_artifact_manifest_sha256: prepared.reviewed_artifact_manifest_sha256(),
                new_decision_head: prepared.new_decision_head(),
                feedback_oid: prepared.feedback_oid(),
            },
        )
        .map_err(DurableArtifactError::from_journal)?;
    if !prepared_matches_intent(&prepared, registered.intent()) {
        return Err(DurableArtifactError::integrity());
    }
    Ok(PreparedDurableArtifactDecision {
        review_id: review.review_id.clone(),
        prepared,
    })
}

/// Mark the durable attempt `outcome_unknown` before performing its one CAS,
/// then publish through the ordinary Core path. A crash on either side of CAS
/// is therefore reconcilable without blind replay.
pub fn publish_prepared_durable_artifact_decision(
    journal: &mut SqliteReviewJournal,
    review: &mut DurablePendingArtifactReview,
    prepared: PreparedDurableArtifactDecision,
) -> DurableArtifactResult<PublishedDurableArtifactDecision> {
    if review.review_id != prepared.review_id {
        return Err(DurableArtifactError::integrity());
    }
    let reconciliation = journal
        .get_review_reconciliation(review.review_id.journal_id())
        .map_err(DurableArtifactError::from_journal)?;
    let intent = reconciliation
        .decision_intent()
        .ok_or_else(DurableArtifactError::integrity)?;
    if !prepared_matches_intent(&prepared.prepared, intent) {
        return Err(DurableArtifactError::integrity());
    }
    match reconciliation.review().state() {
        ReviewState::PendingReview | ReviewState::RetryableFailure => {
            journal
                .transition_review_state(
                    review.review_id.journal_id(),
                    reconciliation.review().state(),
                    ReviewState::OutcomeUnknown,
                )
                .map_err(DurableArtifactError::from_journal)?;
        }
        ReviewState::OutcomeUnknown => return Err(DurableArtifactError::outcome_unknown()),
        state => {
            return Err(DurableArtifactError::new(
                state.as_str(),
                "artifact review is not decisionable",
            ));
        }
    }
    let publication = publish_prepared_artifact_decision(&mut review.pending, prepared.prepared)
        .map_err(DurableArtifactError::from_workflow)?;
    if !publication_matches_intent(&publication, intent) {
        return Err(DurableArtifactError::integrity());
    }
    Ok(PublishedDurableArtifactDecision {
        review_id: review.review_id.clone(),
        publication,
    })
}

/// Verify the exact live Decision checkout, atomically persist its outcome,
/// and return bytes suitable for the embedding application's Accepted update.
pub fn commit_published_durable_artifact_decision(
    journal: &mut SqliteReviewJournal,
    config: &TrustedArtifactProjectConfig,
    published: PublishedDurableArtifactDecision,
    limits: ArtifactCheckoutLimits,
) -> DurableArtifactResult<ReconciledDurableArtifactReview> {
    let intent = journal
        .get_decision_commit_intent(published.review_id.journal_id())
        .map_err(DurableArtifactError::from_journal)?
        .ok_or_else(DurableArtifactError::integrity)?;
    if !publication_matches_intent(&published.publication, &intent) {
        return Err(DurableArtifactError::integrity());
    }
    let binding = durable_binding(config, intent.binding())?;
    let checkout = checkout_artifact_decision(
        &TrustedArtifactDecisionBinding::new(
            config.repository_path(),
            config.project_key(),
            binding,
            published.publication.new_decision_head(),
            published.publication.disposition(),
            published.publication.reviewed_artifact_manifest_sha256(),
        ),
        limits,
    )
    .map_err(|error| DurableArtifactError::new(error.code(), "artifact checkout failed"))?;
    journal
        .commit_decision_outcome(
            published.review_id.journal_id(),
            outcome_request_from_publication(&published.publication),
        )
        .map_err(DurableArtifactError::from_journal)?;
    let reconciliation = journal
        .get_review_reconciliation(published.review_id.journal_id())
        .map_err(DurableArtifactError::from_journal)?;
    Ok(ReconciledDurableArtifactReview {
        status: status_from_reconciliation(&reconciliation)?,
        checkout_state: DurableArtifactCheckoutState::Current,
        checked_out_artifact: Some(checkout),
    })
}

/// Authenticate, authorize, and reconcile an uncertain Decision from one
/// consistent Ref snapshot and a bounded complete-history reflog aggregate.
pub fn reconcile_durable_artifact_review<A, C>(
    approvals: &ArtifactApprovalRegistry<A, C>,
    credential: &A::Credential,
    journal: &mut SqliteReviewJournal,
    config: &TrustedArtifactProjectConfig,
    review_id: &ArtifactReviewId,
    limits: ArtifactCheckoutLimits,
) -> DurableArtifactResult<ReconciledDurableArtifactReview>
where
    A: Authenticator,
    C: AuthorizationClock + Send + Sync,
{
    approvals
        .authorize_project(credential, &config.project_selector())
        .map_err(|error| {
            DurableArtifactError::new(error.code(), "artifact review access failed")
        })?;
    let mut reconciliation = authorized_reconciliation(journal, config, review_id)?;

    if reconciliation.review().state() == ReviewState::DecisionCommitted {
        let outcome = reconciliation
            .decision_outcome()
            .ok_or_else(DurableArtifactError::integrity)?;
        return match decision_head_relation(
            config,
            reconciliation.review().binding().decision_ref_name(),
            outcome.new_decision_head(),
        )? {
            DecisionHeadRelation::Current => {
                let checkout = checkout_from_outcome(
                    config,
                    reconciliation.review().binding(),
                    outcome,
                    limits,
                )?;
                Ok(ReconciledDurableArtifactReview {
                    status: status_from_reconciliation(&reconciliation)?,
                    checkout_state: DurableArtifactCheckoutState::Current,
                    checked_out_artifact: Some(checkout),
                })
            }
            DecisionHeadRelation::Descendant => Ok(ReconciledDurableArtifactReview {
                status: status_from_reconciliation(&reconciliation)?,
                checkout_state: DurableArtifactCheckoutState::Superseded,
                checked_out_artifact: None,
            }),
            DecisionHeadRelation::Unrelated => Err(DurableArtifactError::integrity()),
        };
    }
    if reconciliation.review().state() == ReviewState::TerminalDenial {
        return Ok(ReconciledDurableArtifactReview {
            status: status_from_reconciliation(&reconciliation)?,
            checkout_state: DurableArtifactCheckoutState::Unavailable,
            checked_out_artifact: None,
        });
    }

    let Some(intent) = reconciliation.decision_intent().cloned() else {
        return Ok(ReconciledDurableArtifactReview {
            status: status_from_reconciliation(&reconciliation)?,
            checkout_state: DurableArtifactCheckoutState::Unavailable,
            checked_out_artifact: None,
        });
    };
    match inspect_decision_cas(config, &intent, limits)? {
        CasEvidence::NotCommitted => {
            if reconciliation.review().state() == ReviewState::OutcomeUnknown {
                journal
                    .reconcile_decision_not_committed(&intent)
                    .map_err(DurableArtifactError::from_journal)?;
                reconciliation = authorized_reconciliation(journal, config, review_id)?;
            }
            Ok(ReconciledDurableArtifactReview {
                status: status_from_reconciliation(&reconciliation)?,
                checkout_state: DurableArtifactCheckoutState::Unavailable,
                checked_out_artifact: None,
            })
        }
        CasEvidence::Committed => {
            let checkout = checkout_from_intent(config, &intent, limits)?;
            journal
                .commit_decision_outcome(
                    review_id.journal_id(),
                    outcome_request_from_intent(&intent),
                )
                .map_err(DurableArtifactError::from_journal)?;
            reconciliation = authorized_reconciliation(journal, config, review_id)?;
            Ok(ReconciledDurableArtifactReview {
                status: status_from_reconciliation(&reconciliation)?,
                checkout_state: DurableArtifactCheckoutState::Current,
                checked_out_artifact: Some(checkout),
            })
        }
        CasEvidence::CommittedSuperseded => {
            journal
                .commit_decision_outcome(
                    review_id.journal_id(),
                    outcome_request_from_intent(&intent),
                )
                .map_err(DurableArtifactError::from_journal)?;
            reconciliation = authorized_reconciliation(journal, config, review_id)?;
            Ok(ReconciledDurableArtifactReview {
                status: status_from_reconciliation(&reconciliation)?,
                checkout_state: DurableArtifactCheckoutState::Superseded,
                checked_out_artifact: None,
            })
        }
        CasEvidence::StaleNotCommitted => {
            journal
                .transition_review_state(
                    review_id.journal_id(),
                    reconciliation.review().state(),
                    ReviewState::TerminalDenial,
                )
                .map_err(DurableArtifactError::from_journal)?;
            reconciliation = authorized_reconciliation(journal, config, review_id)?;
            Ok(ReconciledDurableArtifactReview {
                status: status_from_reconciliation(&reconciliation)?,
                checkout_state: DurableArtifactCheckoutState::Unavailable,
                checked_out_artifact: None,
            })
        }
        CasEvidence::Ambiguous => {
            if matches!(
                reconciliation.review().state(),
                ReviewState::PendingReview | ReviewState::RetryableFailure
            ) {
                journal
                    .transition_review_state(
                        review_id.journal_id(),
                        reconciliation.review().state(),
                        ReviewState::OutcomeUnknown,
                    )
                    .map_err(DurableArtifactError::from_journal)?;
                reconciliation = authorized_reconciliation(journal, config, review_id)?;
            }
            Ok(ReconciledDurableArtifactReview {
                status: status_from_reconciliation(&reconciliation)?,
                checkout_state: DurableArtifactCheckoutState::Unavailable,
                checked_out_artifact: None,
            })
        }
    }
}

fn authorized_reconciliation(
    journal: &mut SqliteReviewJournal,
    config: &TrustedArtifactProjectConfig,
    review_id: &ArtifactReviewId,
) -> DurableArtifactResult<ReviewReconciliation> {
    let reconciliation = journal
        .get_review_reconciliation(review_id.journal_id())
        .map_err(DurableArtifactError::from_journal)?;
    if durable_binding(config, reconciliation.review().binding()).is_err() {
        // A caller authorized only for `config` must not learn whether the
        // supplied locator belongs to a different project.
        return Err(DurableArtifactError::review_not_found());
    }
    Ok(reconciliation)
}

fn status_from_reconciliation(
    reconciliation: &ReviewReconciliation,
) -> DurableArtifactResult<DurableArtifactReviewStatus> {
    let proposal_artifact_manifest_sha256 = reconciliation
        .proposal_artifact_manifest_sha256()
        .ok_or_else(DurableArtifactError::integrity)?
        .to_owned();
    let decision = reconciliation
        .decision_outcome()
        .map(|outcome| DurableArtifactDecisionStatus {
            disposition: artifact_disposition(outcome.disposition()),
            selected_snapshot: match outcome.selected_snapshot() {
                SelectedSnapshot::Base => DurableArtifactSelectedSnapshot::Base,
                SelectedSnapshot::Proposal => DurableArtifactSelectedSnapshot::Proposal,
            },
            reviewed_artifact_manifest_sha256: outcome
                .reviewed_artifact_manifest_sha256()
                .to_owned(),
        });
    if reconciliation.review().state() == ReviewState::DecisionCommitted && decision.is_none() {
        return Err(DurableArtifactError::integrity());
    }
    Ok(DurableArtifactReviewStatus {
        review_id: ArtifactReviewId::from_journal(reconciliation.review().review_id()),
        state: artifact_review_state(reconciliation.review().state()),
        proposal_artifact_manifest_sha256,
        decision,
    })
}

fn review_binding(binding: &DurableProposalBinding) -> DurableArtifactResult<ReviewBinding> {
    ReviewBinding::new(
        binding.project().as_str(),
        binding.proposal_ref_name(),
        binding.proposal_head(),
        binding.decision_ref_name(),
        binding.decision_head(),
    )
    .map_err(DurableArtifactError::from_journal)
}

fn durable_binding(
    config: &TrustedArtifactProjectConfig,
    binding: &ReviewBinding,
) -> DurableArtifactResult<DurableProposalBinding> {
    let project = config.project_selector();
    let expected_decision_ref = format!("decision/artifact/{}", config.project_key());
    let proposal_prefix = format!("proposal/artifact/{}", config.project_key());
    if binding.project_scope() != project.as_str()
        || binding.decision_ref_name() != expected_decision_ref
        || (binding.proposal_ref_name() != proposal_prefix
            && !binding
                .proposal_ref_name()
                .strip_prefix(&proposal_prefix)
                .is_some_and(|suffix| suffix.starts_with('/')))
    {
        return Err(DurableArtifactError::integrity());
    }
    Ok(DurableProposalBinding::new(
        project,
        binding.proposal_ref_name(),
        binding.proposal_head(),
        binding.decision_ref_name(),
        binding.expected_decision_head(),
    ))
}

fn proposal_canonical_request(
    accepted: &RegularFileManifest,
    proposed: &RegularFileManifest,
    application_context_json: &[u8],
    source_attribution: ArtifactSourceAttribution,
) -> DurableArtifactResult<Vec<u8>> {
    let context_sha256 = review_context_sha256(application_context_json)
        .map_err(DurableArtifactError::from_workflow)?;
    let source = match source_attribution {
        ArtifactSourceAttribution::CallerSuppliedAiAttributed => {
            b"caller_supplied_ai_attributed".as_slice()
        }
    };
    canonical_fields(
        PROPOSAL_REQUEST_DOMAIN,
        &[
            artifact_manifest_sha256(accepted).as_bytes(),
            artifact_manifest_sha256(proposed).as_bytes(),
            context_sha256.as_bytes(),
            source,
        ],
    )
}

fn decision_canonical_request(options: &ArtifactDecisionOptions) -> DurableArtifactResult<Vec<u8>> {
    let disposition = match options.disposition {
        ArtifactDisposition::AdoptedUnchanged => b"adopted_unchanged".as_slice(),
        ArtifactDisposition::Rejected => b"rejected".as_slice(),
        ArtifactDisposition::Deferred => b"deferred".as_slice(),
    };
    let rationale_tag = if options.private_rationale.is_some() {
        b"some".as_slice()
    } else {
        b"none".as_slice()
    };
    canonical_fields(
        DECISION_REQUEST_DOMAIN,
        &[
            disposition,
            rationale_tag,
            options
                .private_rationale
                .as_deref()
                .unwrap_or("")
                .as_bytes(),
        ],
    )
}

fn canonical_fields(domain: &[u8], fields: &[&[u8]]) -> DurableArtifactResult<Vec<u8>> {
    let mut encoded = Vec::from(domain);
    for field in fields {
        let length =
            u64::try_from(field.len()).map_err(|_| DurableArtifactError::invalid_argument())?;
        encoded.extend_from_slice(&length.to_be_bytes());
        encoded.extend_from_slice(field);
    }
    Ok(encoded)
}

fn validate_idempotency_key(idempotency_key: &[u8]) -> DurableArtifactResult<()> {
    if idempotency_key.is_empty() || idempotency_key.len() > MAX_IDEMPOTENCY_KEY_BYTES {
        return Err(DurableArtifactError::invalid_argument());
    }
    Ok(())
}

fn journal_disposition(disposition: ArtifactDisposition) -> DecisionDisposition {
    match disposition {
        ArtifactDisposition::AdoptedUnchanged => DecisionDisposition::AdoptedUnchanged,
        ArtifactDisposition::Rejected => DecisionDisposition::Rejected,
        ArtifactDisposition::Deferred => DecisionDisposition::Deferred,
    }
}

fn artifact_disposition(disposition: DecisionDisposition) -> ArtifactDisposition {
    match disposition {
        DecisionDisposition::AdoptedUnchanged => ArtifactDisposition::AdoptedUnchanged,
        DecisionDisposition::Rejected => ArtifactDisposition::Rejected,
        DecisionDisposition::Deferred => ArtifactDisposition::Deferred,
    }
}

fn journal_selected_snapshot(disposition: ArtifactDisposition) -> SelectedSnapshot {
    match disposition {
        ArtifactDisposition::AdoptedUnchanged => SelectedSnapshot::Proposal,
        ArtifactDisposition::Rejected | ArtifactDisposition::Deferred => SelectedSnapshot::Base,
    }
}

fn artifact_review_state(state: ReviewState) -> DurableArtifactReviewState {
    match state {
        ReviewState::PendingReview => DurableArtifactReviewState::PendingReview,
        ReviewState::DecisionCommitted => DurableArtifactReviewState::DecisionCommitted,
        ReviewState::TerminalDenial => DurableArtifactReviewState::TerminalDenial,
        ReviewState::RetryableFailure => DurableArtifactReviewState::RetryableFailure,
        ReviewState::OutcomeUnknown => DurableArtifactReviewState::OutcomeUnknown,
    }
}

fn prepared_matches_intent(
    prepared: &PreparedArtifactDecision,
    intent: &DecisionCommitIntent,
) -> bool {
    review_binding(&prepared.durable_binding()).is_ok_and(|binding| binding == *intent.binding())
        && prepared.disposition() == artifact_disposition(intent.disposition())
        && journal_selected_snapshot(prepared.disposition()) == intent.selected_snapshot()
        && prepared.reviewed_artifact_manifest_sha256()
            == intent.reviewed_artifact_manifest_sha256()
        && prepared.new_decision_head() == intent.new_decision_head()
        && prepared.feedback_oid() == intent.feedback_oid()
}

fn publication_matches_intent(
    publication: &ArtifactDecisionPublication,
    intent: &DecisionCommitIntent,
) -> bool {
    review_binding(&publication.durable_binding()).is_ok_and(|binding| binding == *intent.binding())
        && publication.disposition() == artifact_disposition(intent.disposition())
        && journal_selected_snapshot(publication.disposition()) == intent.selected_snapshot()
        && publication.reviewed_artifact_manifest_sha256()
            == intent.reviewed_artifact_manifest_sha256()
        && publication.new_decision_head() == intent.new_decision_head()
        && publication.feedback_oid() == intent.feedback_oid()
}

fn outcome_request_from_publication(
    publication: &ArtifactDecisionPublication,
) -> DecisionOutcomeRequest<'_> {
    DecisionOutcomeRequest {
        disposition: journal_disposition(publication.disposition()),
        selected_snapshot: journal_selected_snapshot(publication.disposition()),
        reviewed_artifact_manifest_sha256: publication.reviewed_artifact_manifest_sha256(),
        proposal_head: publication.proposal_head(),
        expected_decision_head: publication.expected_decision_head(),
        new_decision_head: publication.new_decision_head(),
        feedback_oid: publication.feedback_oid(),
    }
}

fn outcome_request_from_intent(intent: &DecisionCommitIntent) -> DecisionOutcomeRequest<'_> {
    DecisionOutcomeRequest {
        disposition: intent.disposition(),
        selected_snapshot: intent.selected_snapshot(),
        reviewed_artifact_manifest_sha256: intent.reviewed_artifact_manifest_sha256(),
        proposal_head: intent.binding().proposal_head(),
        expected_decision_head: intent.expected_decision_head(),
        new_decision_head: intent.new_decision_head(),
        feedback_oid: intent.feedback_oid(),
    }
}

fn checkout_from_intent(
    config: &TrustedArtifactProjectConfig,
    intent: &DecisionCommitIntent,
    limits: ArtifactCheckoutLimits,
) -> DurableArtifactResult<CheckedOutArtifact> {
    let binding = durable_binding(config, intent.binding())?;
    checkout_artifact_decision(
        &TrustedArtifactDecisionBinding::new(
            config.repository_path(),
            config.project_key(),
            binding,
            intent.new_decision_head(),
            artifact_disposition(intent.disposition()),
            intent.reviewed_artifact_manifest_sha256(),
        ),
        limits,
    )
    .map_err(|error| DurableArtifactError::new(error.code(), "artifact checkout failed"))
}

fn checkout_from_outcome(
    config: &TrustedArtifactProjectConfig,
    review_binding: &ReviewBinding,
    outcome: &DecisionOutcome,
    limits: ArtifactCheckoutLimits,
) -> DurableArtifactResult<CheckedOutArtifact> {
    let binding = durable_binding(config, review_binding)?;
    checkout_artifact_decision(
        &TrustedArtifactDecisionBinding::new(
            config.repository_path(),
            config.project_key(),
            binding,
            outcome.new_decision_head(),
            artifact_disposition(outcome.disposition()),
            outcome.reviewed_artifact_manifest_sha256(),
        ),
        limits,
    )
    .map_err(|error| DurableArtifactError::new(error.code(), "artifact checkout failed"))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CasEvidence {
    NotCommitted,
    Committed,
    CommittedSuperseded,
    StaleNotCommitted,
    Ambiguous,
}

fn inspect_cas(
    config: &TrustedArtifactProjectConfig,
    ref_name: &str,
    expected_old_head: Option<&str>,
    candidate_head: &str,
) -> DurableArtifactResult<CasEvidence> {
    let repository = Repository::open_existing_read_only(config.repository_path())
        .map_err(|error| DurableArtifactError::new(error.code(), "artifact repository failed"))?;
    let evidence = repository
        .refs()
        .read_ref_transition_evidence(ref_name, expected_old_head, candidate_head)
        .map_err(|error| DurableArtifactError::new(error.code(), "artifact Ref read failed"))?;
    let current_head = evidence
        .snapshot
        .refs
        .iter()
        .find(|record| record.name == ref_name)
        .map(|record| record.head.as_str());
    let last_is_exact = evidence.latest_entry.as_ref().is_some_and(|entry| {
        entry.old_head.as_deref() == expected_old_head && entry.new_head == candidate_head
    });

    if current_head == Some(candidate_head)
        && evidence.candidate_touch_count == 1
        && evidence.exact_transition_count == 1
        && last_is_exact
    {
        return Ok(CasEvidence::Committed);
    }
    if evidence.candidate_touch_count == 0 {
        if current_head == expected_old_head
            && (expected_old_head.is_some() || evidence.latest_entry.is_none())
        {
            return Ok(CasEvidence::NotCommitted);
        }
        return Ok(CasEvidence::StaleNotCommitted);
    }
    Ok(CasEvidence::Ambiguous)
}

fn inspect_decision_cas(
    config: &TrustedArtifactProjectConfig,
    intent: &DecisionCommitIntent,
    limits: ArtifactCheckoutLimits,
) -> DurableArtifactResult<CasEvidence> {
    let repository = Repository::open_existing_read_only(config.repository_path())
        .map_err(|error| DurableArtifactError::new(error.code(), "artifact repository failed"))?;
    let ref_name = intent.binding().decision_ref_name();
    let candidate_head = intent.new_decision_head();
    let expected_old_head = intent.expected_decision_head();
    let evidence = repository
        .refs()
        .read_ref_transition_evidence(ref_name, Some(expected_old_head), candidate_head)
        .map_err(|error| DurableArtifactError::new(error.code(), "artifact Ref read failed"))?;
    let current_head = evidence
        .snapshot
        .refs
        .iter()
        .find(|record| record.name == ref_name)
        .map(|record| record.head.as_str());
    let latest_is_exact = evidence.latest_entry.as_ref().is_some_and(|entry| {
        entry.old_head.as_deref() == Some(expected_old_head) && entry.new_head == candidate_head
    });

    if current_head == Some(candidate_head)
        && evidence.candidate_touch_count == 1
        && evidence.exact_transition_count == 1
        && latest_is_exact
    {
        return Ok(CasEvidence::Committed);
    }
    if evidence.candidate_touch_count == 0 {
        return if current_head == Some(expected_old_head) {
            Ok(CasEvidence::NotCommitted)
        } else {
            Ok(CasEvidence::StaleNotCommitted)
        };
    }

    if evidence.candidate_touch_count == 2
        && evidence.exact_transition_count == 1
        && current_head.is_some_and(|head| head != candidate_head)
    {
        let current_head = current_head.expect("a historical candidate has a current Ref head");
        if crate::workflow::verify_artifact_decision_descendant(
            config,
            &repository,
            &evidence.snapshot,
            ref_name,
            candidate_head,
            current_head,
        )
        .map_err(DurableArtifactError::from_workflow)?
        {
            let binding = durable_binding(config, intent.binding())?;
            crate::checkout::verify_historical_artifact_decision_in_repository(
                &repository,
                &evidence.snapshot,
                &TrustedArtifactDecisionBinding::new(
                    config.repository_path(),
                    config.project_key(),
                    binding,
                    candidate_head,
                    artifact_disposition(intent.disposition()),
                    intent.reviewed_artifact_manifest_sha256(),
                ),
                limits,
            )
            .map_err(|error| {
                DurableArtifactError::new(error.code(), "historical artifact verification failed")
            })?;
            return Ok(CasEvidence::CommittedSuperseded);
        }
    }
    Ok(CasEvidence::Ambiguous)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DecisionHeadRelation {
    Current,
    Descendant,
    Unrelated,
}

fn decision_head_relation(
    config: &TrustedArtifactProjectConfig,
    decision_ref_name: &str,
    committed_head: &str,
) -> DurableArtifactResult<DecisionHeadRelation> {
    let repository = Repository::open_existing_read_only(config.repository_path())
        .map_err(|error| DurableArtifactError::new(error.code(), "artifact repository failed"))?;
    let evidence = repository
        .refs()
        .read_ref_transition_evidence(decision_ref_name, None, committed_head)
        .map_err(|error| DurableArtifactError::new(error.code(), "artifact Ref read failed"))?;
    let Some(current_head) = evidence
        .snapshot
        .refs
        .iter()
        .find(|record| record.name == decision_ref_name)
        .map(|record| record.head.clone())
    else {
        return Ok(DecisionHeadRelation::Unrelated);
    };
    if current_head == committed_head {
        return Ok(DecisionHeadRelation::Current);
    }
    if crate::workflow::verify_artifact_decision_descendant(
        config,
        &repository,
        &evidence.snapshot,
        decision_ref_name,
        committed_head,
        &current_head,
    )
    .map_err(DurableArtifactError::from_workflow)?
    {
        Ok(DecisionHeadRelation::Descendant)
    } else {
        Ok(DecisionHeadRelation::Unrelated)
    }
}
