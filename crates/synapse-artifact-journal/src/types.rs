use std::fmt;

use crate::error::{JournalError, Result};
use crate::validate::{hex, validate_control_value};

/// Public-safe random locator for one durable review.
///
/// This identifier is not a credential, capability, or authorization proof.
#[derive(Clone, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ReviewId(String);

impl ReviewId {
    pub fn parse(value: impl Into<String>) -> Result<Self> {
        let value = value.into();
        if value.len() != REVIEW_ID_HEX_LEN
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(JournalError::InvalidArgument(
                "review_id must be 64 lowercase hexadecimal characters".into(),
            ));
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub(crate) fn generate() -> Result<Self> {
        let mut random = [0_u8; REVIEW_ID_BYTES];
        getrandom::fill(&mut random).map_err(|error| JournalError::Random(error.to_string()))?;
        Ok(Self(hex(&random)))
    }
}

impl fmt::Debug for ReviewId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ReviewId(<opaque>)")
    }
}

impl fmt::Display for ReviewId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// Server-owned durable bindings for one admitted Proposal awaiting Decision.
///
/// These identifiers are journal data, not authority. A caller must revalidate
/// them against its authenticated project configuration and live repository.
#[derive(Clone, Eq, PartialEq)]
pub struct ReviewBinding {
    pub(crate) project_scope: String,
    pub(crate) proposal_ref_name: String,
    pub(crate) proposal_head: String,
    pub(crate) decision_ref_name: String,
    pub(crate) expected_decision_head: String,
}

impl fmt::Debug for ReviewBinding {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ReviewBinding(<redacted server binding>)")
    }
}

impl ReviewBinding {
    pub fn new(
        project_scope: impl Into<String>,
        proposal_ref_name: impl Into<String>,
        proposal_head: impl Into<String>,
        decision_ref_name: impl Into<String>,
        expected_decision_head: impl Into<String>,
    ) -> Result<Self> {
        let binding = Self {
            project_scope: project_scope.into(),
            proposal_ref_name: proposal_ref_name.into(),
            proposal_head: proposal_head.into(),
            decision_ref_name: decision_ref_name.into(),
            expected_decision_head: expected_decision_head.into(),
        };
        binding.validate()?;
        Ok(binding)
    }

    pub fn project_scope(&self) -> &str {
        &self.project_scope
    }

    pub fn proposal_ref_name(&self) -> &str {
        &self.proposal_ref_name
    }

    pub fn proposal_head(&self) -> &str {
        &self.proposal_head
    }

    pub fn decision_ref_name(&self) -> &str {
        &self.decision_ref_name
    }

    pub fn expected_decision_head(&self) -> &str {
        &self.expected_decision_head
    }

    pub(crate) fn validate(&self) -> Result<()> {
        for (label, value) in [
            ("project_scope", self.project_scope.as_str()),
            ("proposal_ref_name", self.proposal_ref_name.as_str()),
            ("proposal_head", self.proposal_head.as_str()),
            ("decision_ref_name", self.decision_ref_name.as_str()),
            (
                "expected_decision_head",
                self.expected_decision_head.as_str(),
            ),
        ] {
            validate_control_value(label, value)?;
        }
        if self
            .proposal_ref_name
            .strip_prefix("proposal/")
            .is_none_or(str::is_empty)
        {
            return Err(JournalError::InvalidArgument(
                "proposal_ref_name must use the proposal/* namespace".into(),
            ));
        }
        if self
            .decision_ref_name
            .strip_prefix("decision/")
            .is_none_or(str::is_empty)
        {
            return Err(JournalError::InvalidArgument(
                "decision_ref_name must use the decision/* namespace".into(),
            ));
        }
        Ok(())
    }
}

/// Durable, bounded outcome known by the journal.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum ReviewState {
    PendingReview,
    DecisionCommitted,
    TerminalDenial,
    RetryableFailure,
    OutcomeUnknown,
}

impl ReviewState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::PendingReview => "pending_review",
            Self::DecisionCommitted => "decision_committed",
            Self::TerminalDenial => "terminal_denial",
            Self::RetryableFailure => "retryable_failure",
            Self::OutcomeUnknown => "outcome_unknown",
        }
    }

    pub(crate) fn parse(value: &str) -> Result<Self> {
        match value {
            "pending_review" => Ok(Self::PendingReview),
            "decision_committed" => Ok(Self::DecisionCommitted),
            "terminal_denial" => Ok(Self::TerminalDenial),
            "retryable_failure" => Ok(Self::RetryableFailure),
            "outcome_unknown" => Ok(Self::OutcomeUnknown),
            _ => Err(JournalError::CorruptData(format!(
                "unknown review state {value:?}"
            ))),
        }
    }
}

/// One durable review row.
#[derive(Clone, Eq, PartialEq)]
pub struct ReviewRecord {
    pub(crate) review_id: ReviewId,
    pub(crate) binding: ReviewBinding,
    pub(crate) state: ReviewState,
}

impl fmt::Debug for ReviewRecord {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ReviewRecord")
            .field("review_id", &self.review_id)
            .field("binding", &"<redacted server binding>")
            .field("state", &self.state)
            .finish()
    }
}

impl ReviewRecord {
    pub fn review_id(&self) -> &ReviewId {
        &self.review_id
    }

    pub fn binding(&self) -> &ReviewBinding {
        &self.binding
    }

    pub const fn state(&self) -> ReviewState {
        self.state
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReviewRegistrationOutcome {
    Created,
    Replayed,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RegisteredReview {
    pub(crate) review: ReviewRecord,
    pub(crate) outcome: ReviewRegistrationOutcome,
}

impl RegisteredReview {
    pub fn review(&self) -> &ReviewRecord {
        &self.review
    }

    pub const fn outcome(&self) -> ReviewRegistrationOutcome {
        self.outcome
    }

    pub fn into_review(self) -> ReviewRecord {
        self.review
    }
}

/// Private server-side locator for a durable Proposal publication intent.
///
/// Unlike [`ReviewId`], this identifier is never a public receipt. It exists so
/// a trusted orchestrator can reconcile a crash around Proposal CAS without
/// allocating a public review locator before publication is verified.
#[derive(Clone, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ProposalIntentId(String);

impl ProposalIntentId {
    pub fn parse(value: impl Into<String>) -> Result<Self> {
        let value = value.into();
        if value.len() != REVIEW_ID_HEX_LEN
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(JournalError::InvalidArgument(
                "proposal_intent_id must be 64 lowercase hexadecimal characters".into(),
            ));
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub(crate) fn generate() -> Result<Self> {
        let mut random = [0_u8; REVIEW_ID_BYTES];
        getrandom::fill(&mut random).map_err(|error| JournalError::Random(error.to_string()))?;
        Ok(Self(hex(&random)))
    }
}

impl fmt::Debug for ProposalIntentId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ProposalIntentId(<private opaque>)")
    }
}

/// Caller data persisted before attempting Proposal publication.
///
/// `canonical_request` must already be canonicalized by the application. Raw
/// request bytes and the idempotency key are hashed in memory and never sent to
/// SQLite. `binding` is trusted server data describing the exact planned CAS.
#[derive(Clone, Copy)]
pub struct ProposalIntentRequest<'a> {
    pub idempotency_key: &'a [u8],
    pub canonical_request: &'a [u8],
    pub artifact_manifest_sha256: &'a str,
    pub binding: &'a ReviewBinding,
}

impl fmt::Debug for ProposalIntentRequest<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ProposalIntentRequest(<redacted request and binding>)")
    }
}

/// Durable private Proposal intent, optionally linked to a published review.
#[derive(Clone, Eq, PartialEq)]
pub struct ProposalIntent {
    pub(crate) proposal_intent_id: ProposalIntentId,
    pub(crate) idempotency_digest: String,
    pub(crate) request_fingerprint: String,
    pub(crate) artifact_manifest_sha256: String,
    pub(crate) binding: ReviewBinding,
    pub(crate) review_id: Option<ReviewId>,
}

impl fmt::Debug for ProposalIntent {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProposalIntent")
            .field("proposal_intent_id", &self.proposal_intent_id)
            .field("binding", &"<redacted server binding>")
            .field("finalized", &self.review_id.is_some())
            .finish()
    }
}

impl ProposalIntent {
    pub fn proposal_intent_id(&self) -> &ProposalIntentId {
        &self.proposal_intent_id
    }

    pub fn idempotency_digest(&self) -> &str {
        &self.idempotency_digest
    }

    pub fn request_fingerprint(&self) -> &str {
        &self.request_fingerprint
    }

    pub fn artifact_manifest_sha256(&self) -> &str {
        &self.artifact_manifest_sha256
    }

    pub fn binding(&self) -> &ReviewBinding {
        &self.binding
    }

    pub fn review_id(&self) -> Option<&ReviewId> {
        self.review_id.as_ref()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProposalIntentRegistrationOutcome {
    Created,
    Replayed,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RegisteredProposalIntent {
    pub(crate) intent: ProposalIntent,
    pub(crate) outcome: ProposalIntentRegistrationOutcome,
}

impl RegisteredProposalIntent {
    pub fn intent(&self) -> &ProposalIntent {
        &self.intent
    }

    pub const fn outcome(&self) -> ProposalIntentRegistrationOutcome {
        self.outcome
    }

    pub fn into_intent(self) -> ProposalIntent {
        self.intent
    }
}

/// Supported canonical disposition recorded by the durable journal profile.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum DecisionDisposition {
    AdoptedUnchanged,
    Rejected,
    Deferred,
}

impl DecisionDisposition {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::AdoptedUnchanged => "adopted_unchanged",
            Self::Rejected => "rejected",
            Self::Deferred => "deferred",
        }
    }

    pub(crate) fn parse(value: &str) -> Result<Self> {
        match value {
            "adopted_unchanged" => Ok(Self::AdoptedUnchanged),
            "rejected" => Ok(Self::Rejected),
            "deferred" => Ok(Self::Deferred),
            _ => Err(JournalError::CorruptData(format!(
                "unknown Decision disposition {value:?}"
            ))),
        }
    }
}

/// Snapshot selected by a canonical Decision.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum SelectedSnapshot {
    Base,
    Proposal,
}

impl SelectedSnapshot {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Base => "base",
            Self::Proposal => "proposal",
        }
    }

    pub(crate) fn parse(value: &str) -> Result<Self> {
        match value {
            "base" => Ok(Self::Base),
            "proposal" => Ok(Self::Proposal),
            _ => Err(JournalError::CorruptData(format!(
                "unknown selected snapshot {value:?}"
            ))),
        }
    }
}

/// Immutable data for one exact Decision publication attempt.
///
/// The journal loads and stores the full [`ReviewBinding`] itself. The caller
/// supplies only the new immutable objects and semantic receipt fields. For an
/// adopted Proposal, the reviewed digest must equal the linked Proposal intent
/// digest. Rejected/deferred Decisions select the canonical base; the trusted
/// orchestrator must verify that digest during full checkout because this
/// transport-neutral journal does not inspect repository base objects.
#[derive(Clone, Copy)]
pub struct DecisionCommitIntentRequest<'a> {
    pub idempotency_key: &'a [u8],
    pub canonical_request: &'a [u8],
    pub disposition: DecisionDisposition,
    pub selected_snapshot: SelectedSnapshot,
    pub reviewed_artifact_manifest_sha256: &'a str,
    pub new_decision_head: &'a str,
    pub feedback_oid: &'a str,
}

impl fmt::Debug for DecisionCommitIntentRequest<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("DecisionCommitIntentRequest(<redacted request and bindings>)")
    }
}

/// Fully bound v2 Decision intent used for restart reconciliation.
#[derive(Clone, Eq, PartialEq)]
pub struct DecisionCommitIntent {
    pub(crate) review_id: ReviewId,
    pub(crate) idempotency_digest: String,
    pub(crate) request_fingerprint: String,
    pub(crate) binding: ReviewBinding,
    pub(crate) disposition: DecisionDisposition,
    pub(crate) selected_snapshot: SelectedSnapshot,
    pub(crate) reviewed_artifact_manifest_sha256: String,
    pub(crate) new_decision_head: String,
    pub(crate) feedback_oid: String,
}

impl fmt::Debug for DecisionCommitIntent {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DecisionCommitIntent")
            .field("review_id", &self.review_id)
            .field("binding", &"<redacted server binding>")
            .field("disposition", &self.disposition)
            .field("selected_snapshot", &self.selected_snapshot)
            .finish_non_exhaustive()
    }
}

impl DecisionCommitIntent {
    pub fn review_id(&self) -> &ReviewId {
        &self.review_id
    }

    pub fn idempotency_digest(&self) -> &str {
        &self.idempotency_digest
    }

    pub fn request_fingerprint(&self) -> &str {
        &self.request_fingerprint
    }

    pub fn binding(&self) -> &ReviewBinding {
        &self.binding
    }

    pub const fn disposition(&self) -> DecisionDisposition {
        self.disposition
    }

    pub const fn selected_snapshot(&self) -> SelectedSnapshot {
        self.selected_snapshot
    }

    pub fn reviewed_artifact_manifest_sha256(&self) -> &str {
        &self.reviewed_artifact_manifest_sha256
    }

    pub fn expected_decision_head(&self) -> &str {
        self.binding.expected_decision_head()
    }

    pub fn new_decision_head(&self) -> &str {
        &self.new_decision_head
    }

    pub fn feedback_oid(&self) -> &str {
        &self.feedback_oid
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RegisteredDecisionCommitIntent {
    pub(crate) intent: DecisionCommitIntent,
    pub(crate) outcome: IntentRegistrationOutcome,
}

impl RegisteredDecisionCommitIntent {
    pub fn intent(&self) -> &DecisionCommitIntent {
        &self.intent
    }

    pub const fn outcome(&self) -> IntentRegistrationOutcome {
        self.outcome
    }

    pub fn into_intent(self) -> DecisionCommitIntent {
        self.intent
    }
}

/// Caller data for one Decision intent.
///
/// The raw idempotency key and canonical request bytes are hashed in memory and
/// are never supplied to SQLite. The request fingerprint also binds every
/// persisted candidate field so a key cannot replay with changed OIDs or heads.
#[derive(Clone, Copy)]
pub struct DecisionIntentRequest<'a> {
    pub idempotency_key: &'a [u8],
    pub canonical_request: &'a [u8],
    pub candidate_head: &'a str,
    pub feedback_oid: &'a str,
    pub expected_decision_head: &'a str,
}

impl fmt::Debug for DecisionIntentRequest<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("DecisionIntentRequest(<redacted request and bindings>)")
    }
}

/// Persisted digest and immutable candidate bindings for one Decision attempt.
#[derive(Clone, Eq, PartialEq)]
pub struct DecisionIntent {
    pub(crate) review_id: ReviewId,
    pub(crate) idempotency_digest: String,
    pub(crate) request_fingerprint: String,
    pub(crate) candidate_head: String,
    pub(crate) feedback_oid: String,
    pub(crate) expected_decision_head: String,
}

impl fmt::Debug for DecisionIntent {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("DecisionIntent(<redacted digests and bindings>)")
    }
}

impl DecisionIntent {
    pub fn review_id(&self) -> &ReviewId {
        &self.review_id
    }

    pub fn idempotency_digest(&self) -> &str {
        &self.idempotency_digest
    }

    pub fn request_fingerprint(&self) -> &str {
        &self.request_fingerprint
    }

    pub fn candidate_head(&self) -> &str {
        &self.candidate_head
    }

    pub fn feedback_oid(&self) -> &str {
        &self.feedback_oid
    }

    pub fn expected_decision_head(&self) -> &str {
        &self.expected_decision_head
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IntentRegistrationOutcome {
    Created,
    Replayed,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RegisteredDecisionIntent {
    pub(crate) intent: DecisionIntent,
    pub(crate) outcome: IntentRegistrationOutcome,
}

impl RegisteredDecisionIntent {
    pub fn intent(&self) -> &DecisionIntent {
        &self.intent
    }

    pub const fn outcome(&self) -> IntentRegistrationOutcome {
        self.outcome
    }

    pub fn into_intent(self) -> DecisionIntent {
        self.intent
    }
}

/// Caller-verified Core Decision receipt fields to persist atomically.
///
/// This value is evidence supplied by a trusted orchestrator after it has
/// validated the real Core receipt. The journal checks it against its stored
/// intent and binding but does not itself inspect Core or claim admission.
#[derive(Clone, Copy)]
pub struct DecisionOutcomeRequest<'a> {
    pub disposition: DecisionDisposition,
    pub selected_snapshot: SelectedSnapshot,
    pub reviewed_artifact_manifest_sha256: &'a str,
    pub proposal_head: &'a str,
    pub expected_decision_head: &'a str,
    pub new_decision_head: &'a str,
    pub feedback_oid: &'a str,
}

impl fmt::Debug for DecisionOutcomeRequest<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("DecisionOutcomeRequest(<redacted verified receipt>)")
    }
}

/// Durable exact Decision outcome, recorded only with `decision_committed`.
#[derive(Clone, Eq, PartialEq)]
pub struct DecisionOutcome {
    pub(crate) review_id: ReviewId,
    pub(crate) disposition: DecisionDisposition,
    pub(crate) selected_snapshot: SelectedSnapshot,
    pub(crate) reviewed_artifact_manifest_sha256: String,
    pub(crate) proposal_head: String,
    pub(crate) expected_decision_head: String,
    pub(crate) new_decision_head: String,
    pub(crate) feedback_oid: String,
}

impl fmt::Debug for DecisionOutcome {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DecisionOutcome")
            .field("review_id", &self.review_id)
            .field("disposition", &self.disposition)
            .field("selected_snapshot", &self.selected_snapshot)
            .finish_non_exhaustive()
    }
}

impl DecisionOutcome {
    pub fn review_id(&self) -> &ReviewId {
        &self.review_id
    }

    pub const fn disposition(&self) -> DecisionDisposition {
        self.disposition
    }

    pub const fn selected_snapshot(&self) -> SelectedSnapshot {
        self.selected_snapshot
    }

    pub fn reviewed_artifact_manifest_sha256(&self) -> &str {
        &self.reviewed_artifact_manifest_sha256
    }

    pub fn proposal_head(&self) -> &str {
        &self.proposal_head
    }

    pub fn expected_decision_head(&self) -> &str {
        &self.expected_decision_head
    }

    pub fn new_decision_head(&self) -> &str {
        &self.new_decision_head
    }

    pub fn feedback_oid(&self) -> &str {
        &self.feedback_oid
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DecisionOutcomeRegistrationOutcome {
    Created,
    Replayed,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CommittedDecisionOutcome {
    pub(crate) outcome: DecisionOutcome,
    pub(crate) registration: DecisionOutcomeRegistrationOutcome,
}

impl CommittedDecisionOutcome {
    pub fn outcome(&self) -> &DecisionOutcome {
        &self.outcome
    }

    pub const fn registration(&self) -> DecisionOutcomeRegistrationOutcome {
        self.registration
    }

    pub fn into_outcome(self) -> DecisionOutcome {
        self.outcome
    }
}

/// Consistent restart view for one published review.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReviewReconciliation {
    pub(crate) review: ReviewRecord,
    pub(crate) proposal_artifact_manifest_sha256: Option<String>,
    pub(crate) decision_intent: Option<DecisionCommitIntent>,
    pub(crate) decision_outcome: Option<DecisionOutcome>,
}

impl ReviewReconciliation {
    pub fn review(&self) -> &ReviewRecord {
        &self.review
    }

    pub fn proposal_artifact_manifest_sha256(&self) -> Option<&str> {
        self.proposal_artifact_manifest_sha256.as_deref()
    }

    pub fn decision_intent(&self) -> Option<&DecisionCommitIntent> {
        self.decision_intent.as_ref()
    }

    pub fn decision_outcome(&self) -> Option<&DecisionOutcome> {
        self.decision_outcome.as_ref()
    }
}

pub(crate) const REVIEW_ID_BYTES: usize = 32;
pub(crate) const REVIEW_ID_HEX_LEN: usize = REVIEW_ID_BYTES * 2;
pub(crate) const RANDOM_ID_ATTEMPTS: usize = 8;
