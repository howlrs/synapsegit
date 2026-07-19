//! Durable, transport-neutral review journaling for artifact applications.
//!
//! This crate stores locators, server-owned Ref bindings, state, and digests. It
//! deliberately does not authenticate, authorize, reconstruct permits, inspect
//! a SynapseGit repository, or claim that Core admitted an operation.

#![forbid(unsafe_code)]

use rusqlite::{Connection, ErrorCode, OptionalExtension, TransactionBehavior, params};
use sha2::{Digest, Sha256};
use std::error::Error;
use std::fmt;
use std::path::Path;
use std::time::Duration;

const SCHEMA_VERSION: i64 = 2;
const REVIEW_ID_BYTES: usize = 32;
const REVIEW_ID_HEX_LEN: usize = REVIEW_ID_BYTES * 2;
const MAX_CONTROL_VALUE_BYTES: usize = 2_000;
const MAX_IDEMPOTENCY_KEY_BYTES: usize = 4_096;
const MAX_CANONICAL_REQUEST_BYTES: usize = 1_048_576;
const RANDOM_ID_ATTEMPTS: usize = 8;
const LEGACY_IDEMPOTENCY_DOMAIN: &[u8] = b"synapsegit-artifact-journal-idempotency-v1\0";
const PROPOSAL_IDEMPOTENCY_DOMAIN: &[u8] = b"synapsegit-artifact-journal-proposal-idempotency-v2\0";
const DECISION_COMMIT_IDEMPOTENCY_DOMAIN: &[u8] =
    b"synapsegit-artifact-journal-decision-commit-idempotency-v2\0";
const REQUEST_DOMAIN: &[u8] = b"synapsegit-artifact-journal-request-v1\0";
const PROPOSAL_REQUEST_DOMAIN: &[u8] = b"synapsegit-artifact-journal-proposal-request-v2\0";
const DECISION_COMMIT_REQUEST_DOMAIN: &[u8] =
    b"synapsegit-artifact-journal-decision-commit-request-v2\0";

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

    fn generate() -> Result<Self> {
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
    project_scope: String,
    proposal_ref_name: String,
    proposal_head: String,
    decision_ref_name: String,
    expected_decision_head: String,
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

    fn validate(&self) -> Result<()> {
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

    fn parse(value: &str) -> Result<Self> {
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
    review_id: ReviewId,
    binding: ReviewBinding,
    state: ReviewState,
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
    review: ReviewRecord,
    outcome: ReviewRegistrationOutcome,
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

    fn generate() -> Result<Self> {
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
    proposal_intent_id: ProposalIntentId,
    idempotency_digest: String,
    request_fingerprint: String,
    artifact_manifest_sha256: String,
    binding: ReviewBinding,
    review_id: Option<ReviewId>,
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
    intent: ProposalIntent,
    outcome: ProposalIntentRegistrationOutcome,
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

    fn parse(value: &str) -> Result<Self> {
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

    fn parse(value: &str) -> Result<Self> {
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
    review_id: ReviewId,
    idempotency_digest: String,
    request_fingerprint: String,
    binding: ReviewBinding,
    disposition: DecisionDisposition,
    selected_snapshot: SelectedSnapshot,
    reviewed_artifact_manifest_sha256: String,
    new_decision_head: String,
    feedback_oid: String,
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
    intent: DecisionCommitIntent,
    outcome: IntentRegistrationOutcome,
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
    review_id: ReviewId,
    idempotency_digest: String,
    request_fingerprint: String,
    candidate_head: String,
    feedback_oid: String,
    expected_decision_head: String,
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
    intent: DecisionIntent,
    outcome: IntentRegistrationOutcome,
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
    review_id: ReviewId,
    disposition: DecisionDisposition,
    selected_snapshot: SelectedSnapshot,
    reviewed_artifact_manifest_sha256: String,
    proposal_head: String,
    expected_decision_head: String,
    new_decision_head: String,
    feedback_oid: String,
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
    outcome: DecisionOutcome,
    registration: DecisionOutcomeRegistrationOutcome,
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
    review: ReviewRecord,
    proposal_artifact_manifest_sha256: Option<String>,
    decision_intent: Option<DecisionCommitIntent>,
    decision_outcome: Option<DecisionOutcome>,
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

#[derive(Debug)]
pub enum JournalError {
    InvalidArgument(String),
    ReviewNotFound,
    ReviewBindingExists,
    ReviewBindingConflict,
    ProposalIntentNotFound,
    ProposalIntentExists,
    ProposalIntentConflict,
    DecisionIntentExists,
    LegacyDecisionIntent,
    DecisionIntentMismatch,
    DecisionOutcomeConflict,
    IdempotencyConflict,
    StateConflict {
        expected: ReviewState,
        actual: ReviewState,
    },
    InvalidStateTransition {
        from: ReviewState,
        to: ReviewState,
    },
    Random(String),
    CorruptData(String),
    Storage(rusqlite::Error),
}

impl JournalError {
    pub const fn code(&self) -> &'static str {
        match self {
            Self::InvalidArgument(_) => "invalid_argument",
            Self::ReviewNotFound => "review_not_found",
            Self::ReviewBindingExists => "review_binding_exists",
            Self::ReviewBindingConflict => "review_binding_conflict",
            Self::ProposalIntentNotFound => "proposal_intent_not_found",
            Self::ProposalIntentExists => "proposal_intent_exists",
            Self::ProposalIntentConflict => "proposal_intent_conflict",
            Self::DecisionIntentExists => "decision_intent_exists",
            Self::LegacyDecisionIntent => "decision_intent_upgrade_required",
            Self::DecisionIntentMismatch => "decision_intent_mismatch",
            Self::DecisionOutcomeConflict => "decision_outcome_conflict",
            Self::IdempotencyConflict => "idempotency_conflict",
            Self::StateConflict { .. } => "review_state_conflict",
            Self::InvalidStateTransition { .. } => "review_state_transition_invalid",
            Self::Random(_) | Self::Storage(_) => "storage_error",
            Self::CorruptData(_) => "journal_corrupt",
        }
    }
}

impl fmt::Display for JournalError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidArgument(message) => formatter.write_str(message),
            Self::ReviewNotFound => formatter.write_str("review not found"),
            Self::ReviewBindingExists => formatter.write_str("review binding already exists"),
            Self::ReviewBindingConflict => {
                formatter.write_str("Proposal binding already has different Decision bindings")
            }
            Self::ProposalIntentNotFound => formatter.write_str("Proposal intent not found"),
            Self::ProposalIntentExists => {
                formatter.write_str("a different Proposal intent already exists")
            }
            Self::ProposalIntentConflict => {
                formatter.write_str("Proposal publication does not match its durable intent")
            }
            Self::DecisionIntentExists => {
                formatter.write_str("a different Decision intent already exists")
            }
            Self::LegacyDecisionIntent => formatter
                .write_str("the existing Decision intent predates exact v2 outcome bindings"),
            Self::DecisionIntentMismatch => {
                formatter.write_str("Decision outcome does not match its durable intent")
            }
            Self::DecisionOutcomeConflict => {
                formatter.write_str("a different Decision outcome is already committed")
            }
            Self::IdempotencyConflict => {
                formatter.write_str("idempotency key was reused for a different Decision intent")
            }
            Self::StateConflict { expected, actual } => write!(
                formatter,
                "review state conflict: expected {}, actual {}",
                expected.as_str(),
                actual.as_str()
            ),
            Self::InvalidStateTransition { from, to } => write!(
                formatter,
                "review state cannot transition from {} to {}",
                from.as_str(),
                to.as_str()
            ),
            Self::Random(message) => write!(formatter, "random source failed: {message}"),
            Self::CorruptData(message) => write!(formatter, "journal data is corrupt: {message}"),
            Self::Storage(error) => error.fmt(formatter),
        }
    }
}

impl Error for JournalError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Storage(error) => Some(error),
            _ => None,
        }
    }
}

impl From<rusqlite::Error> for JournalError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Storage(error)
    }
}

pub type Result<T> = std::result::Result<T, JournalError>;

/// SQLite-backed review journal. This type is storage, never authority.
pub struct SqliteReviewJournal {
    connection: Connection,
}

impl fmt::Debug for SqliteReviewJournal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SqliteReviewJournal")
            .finish_non_exhaustive()
    }
}

fn create_schema_v2(connection: &Connection) -> Result<()> {
    connection.execute_batch(
        "CREATE TABLE reviews (
            review_id TEXT PRIMARY KEY NOT NULL,
            project_scope TEXT NOT NULL,
            proposal_ref_name TEXT NOT NULL,
            proposal_head TEXT NOT NULL,
            decision_ref_name TEXT NOT NULL,
            expected_decision_head TEXT NOT NULL,
            state TEXT NOT NULL CHECK (state IN (
                'pending_review',
                'decision_committed',
                'terminal_denial',
                'retryable_failure',
                'outcome_unknown'
            )),
            UNIQUE(project_scope, proposal_ref_name, proposal_head)
        );
        CREATE TABLE decision_intents (
            review_id TEXT PRIMARY KEY NOT NULL
                REFERENCES reviews(review_id) ON DELETE RESTRICT,
            idempotency_digest TEXT NOT NULL,
            request_fingerprint TEXT NOT NULL,
            candidate_head TEXT NOT NULL,
            feedback_oid TEXT NOT NULL,
            expected_decision_head TEXT NOT NULL
        );
        CREATE UNIQUE INDEX decision_intents_idempotency
            ON decision_intents(review_id, idempotency_digest);",
    )?;
    create_v2_extension_tables(connection)?;
    connection.execute_batch("PRAGMA user_version = 2;")?;
    Ok(())
}

fn migrate_schema_v1_to_v2(connection: &Connection) -> Result<()> {
    create_v2_extension_tables(connection)?;
    connection.execute_batch("PRAGMA user_version = 2;")?;
    Ok(())
}

fn create_v2_extension_tables(connection: &Connection) -> Result<()> {
    connection.execute_batch(
        "CREATE TABLE proposal_intents (
            proposal_intent_id TEXT PRIMARY KEY NOT NULL
                CHECK (
                    length(proposal_intent_id) = 64
                    AND proposal_intent_id NOT GLOB '*[^0-9a-f]*'
                ),
            idempotency_digest TEXT NOT NULL,
            request_fingerprint TEXT NOT NULL,
            artifact_manifest_sha256 TEXT NOT NULL CHECK (
                length(artifact_manifest_sha256) = 64
                AND artifact_manifest_sha256 NOT GLOB '*[^0-9a-f]*'
            ),
            project_scope TEXT NOT NULL,
            proposal_ref_name TEXT NOT NULL,
            proposal_head TEXT NOT NULL,
            decision_ref_name TEXT NOT NULL,
            expected_decision_head TEXT NOT NULL,
            review_id TEXT UNIQUE
                REFERENCES reviews(review_id) ON DELETE RESTRICT,
            UNIQUE(project_scope, idempotency_digest),
            UNIQUE(project_scope, proposal_ref_name, proposal_head)
        );
        CREATE INDEX proposal_intents_unfinalized
            ON proposal_intents(project_scope, proposal_intent_id)
            WHERE review_id IS NULL;
        CREATE TABLE decision_commit_intents (
            review_id TEXT PRIMARY KEY NOT NULL
                REFERENCES decision_intents(review_id) ON DELETE RESTRICT,
            project_scope TEXT NOT NULL,
            proposal_ref_name TEXT NOT NULL,
            proposal_head TEXT NOT NULL,
            decision_ref_name TEXT NOT NULL,
            disposition TEXT NOT NULL CHECK (disposition IN (
                'adopted_unchanged', 'rejected', 'deferred'
            )),
            selected_snapshot TEXT NOT NULL CHECK (selected_snapshot IN (
                'base', 'proposal'
            )),
            reviewed_artifact_manifest_sha256 TEXT NOT NULL CHECK (
                length(reviewed_artifact_manifest_sha256) = 64
                AND reviewed_artifact_manifest_sha256 NOT GLOB '*[^0-9a-f]*'
            ),
            CHECK (
                (disposition = 'adopted_unchanged' AND selected_snapshot = 'proposal')
                OR (disposition IN ('rejected', 'deferred') AND selected_snapshot = 'base')
            )
        );
        CREATE TABLE decision_outcomes (
            review_id TEXT PRIMARY KEY NOT NULL
                REFERENCES decision_commit_intents(review_id) ON DELETE RESTRICT,
            disposition TEXT NOT NULL CHECK (disposition IN (
                'adopted_unchanged', 'rejected', 'deferred'
            )),
            selected_snapshot TEXT NOT NULL CHECK (selected_snapshot IN (
                'base', 'proposal'
            )),
            reviewed_artifact_manifest_sha256 TEXT NOT NULL CHECK (
                length(reviewed_artifact_manifest_sha256) = 64
                AND reviewed_artifact_manifest_sha256 NOT GLOB '*[^0-9a-f]*'
            ),
            proposal_head TEXT NOT NULL,
            expected_decision_head TEXT NOT NULL,
            new_decision_head TEXT NOT NULL,
            feedback_oid TEXT NOT NULL,
            CHECK (expected_decision_head <> new_decision_head),
            CHECK (
                (disposition = 'adopted_unchanged' AND selected_snapshot = 'proposal')
                OR (disposition IN ('rejected', 'deferred') AND selected_snapshot = 'base')
            )
        );",
    )?;
    Ok(())
}

impl SqliteReviewJournal {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        Self::initialize(Connection::open(path)?)
    }

    pub fn open_in_memory() -> Result<Self> {
        Self::initialize(Connection::open_in_memory()?)
    }

    fn initialize(mut connection: Connection) -> Result<Self> {
        connection.busy_timeout(Duration::from_secs(5))?;
        connection.execute_batch("PRAGMA foreign_keys = ON;")?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let version =
            transaction.query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))?;
        match version {
            0 => create_schema_v2(&transaction)?,
            1 => migrate_schema_v1_to_v2(&transaction)?,
            SCHEMA_VERSION => {}
            _ => {
                return Err(JournalError::CorruptData(format!(
                    "unsupported schema version {version}"
                )));
            }
        }
        transaction.commit()?;
        Ok(Self { connection })
    }

    /// Durably register the exact private command before Proposal CAS.
    ///
    /// No [`ReviewId`] is generated by this operation. An exact retry in the
    /// same project scope returns the original private intent; reuse of the key
    /// or Proposal identity with changed data fails closed.
    pub fn register_proposal_intent(
        &mut self,
        request: ProposalIntentRequest<'_>,
    ) -> Result<RegisteredProposalIntent> {
        validate_proposal_intent_request(&request)?;
        let idempotency_digest = digest(
            PROPOSAL_IDEMPOTENCY_DOMAIN,
            &[
                request.binding.project_scope().as_bytes(),
                request.idempotency_key,
            ],
        );
        let request_fingerprint = digest(
            PROPOSAL_REQUEST_DOMAIN,
            &[
                request.canonical_request,
                request.artifact_manifest_sha256.as_bytes(),
                request.binding.project_scope().as_bytes(),
                request.binding.proposal_ref_name().as_bytes(),
                request.binding.proposal_head().as_bytes(),
                request.binding.decision_ref_name().as_bytes(),
                request.binding.expected_decision_head().as_bytes(),
            ],
        );
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;

        if let Some(existing) = load_proposal_intent_by_idempotency(
            &transaction,
            request.binding.project_scope(),
            &idempotency_digest,
        )? {
            if existing.binding == *request.binding
                && existing.request_fingerprint == request_fingerprint
            {
                validate_proposal_intent_review_link(&transaction, &existing)?;
                return Ok(RegisteredProposalIntent {
                    intent: existing,
                    outcome: ProposalIntentRegistrationOutcome::Replayed,
                });
            }
            return Err(JournalError::IdempotencyConflict);
        }
        if let Some(existing) = load_proposal_intent_by_proposal(&transaction, request.binding)? {
            return if existing.binding == *request.binding {
                Err(JournalError::ProposalIntentExists)
            } else {
                Err(JournalError::ReviewBindingConflict)
            };
        }
        if let Some(review) = load_review_by_proposal(&transaction, request.binding)? {
            return if review.binding() == request.binding {
                Err(JournalError::ReviewBindingExists)
            } else {
                Err(JournalError::ReviewBindingConflict)
            };
        }

        for _ in 0..RANDOM_ID_ATTEMPTS {
            let proposal_intent_id = ProposalIntentId::generate()?;
            let inserted = transaction.execute(
                "INSERT OR IGNORE INTO proposal_intents(
                    proposal_intent_id, idempotency_digest, request_fingerprint,
                    artifact_manifest_sha256,
                    project_scope, proposal_ref_name, proposal_head,
                    decision_ref_name, expected_decision_head, review_id
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, NULL)",
                params![
                    proposal_intent_id.as_str(),
                    &idempotency_digest,
                    &request_fingerprint,
                    request.artifact_manifest_sha256,
                    request.binding.project_scope(),
                    request.binding.proposal_ref_name(),
                    request.binding.proposal_head(),
                    request.binding.decision_ref_name(),
                    request.binding.expected_decision_head(),
                ],
            )?;
            if inserted == 1 {
                let intent = ProposalIntent {
                    proposal_intent_id,
                    idempotency_digest,
                    request_fingerprint,
                    artifact_manifest_sha256: request.artifact_manifest_sha256.to_owned(),
                    binding: request.binding.clone(),
                    review_id: None,
                };
                transaction.commit()?;
                return Ok(RegisteredProposalIntent {
                    intent,
                    outcome: ProposalIntentRegistrationOutcome::Created,
                });
            }
            if let Some(existing) = load_proposal_intent_by_idempotency(
                &transaction,
                request.binding.project_scope(),
                &idempotency_digest,
            )? {
                if existing.binding == *request.binding
                    && existing.request_fingerprint == request_fingerprint
                {
                    validate_proposal_intent_review_link(&transaction, &existing)?;
                    return Ok(RegisteredProposalIntent {
                        intent: existing,
                        outcome: ProposalIntentRegistrationOutcome::Replayed,
                    });
                }
                return Err(JournalError::IdempotencyConflict);
            }
            if let Some(existing) = load_proposal_intent_by_proposal(&transaction, request.binding)?
            {
                return if existing.binding == *request.binding {
                    Err(JournalError::ProposalIntentExists)
                } else {
                    Err(JournalError::ReviewBindingConflict)
                };
            }
        }
        Err(JournalError::Random(
            "could not allocate a unique private Proposal intent identifier".into(),
        ))
    }

    /// Finalize a private intent after the caller verifies Proposal publication.
    ///
    /// The supplied binding must exactly equal the stored command. The public
    /// [`ReviewId`] and pending review row are created atomically, and an exact
    /// response-loss retry returns the same locator.
    pub fn commit_proposal_publication(
        &mut self,
        proposal_intent_id: &ProposalIntentId,
        published_binding: &ReviewBinding,
    ) -> Result<RegisteredReview> {
        published_binding.validate()?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let intent = load_proposal_intent(&transaction, proposal_intent_id)?
            .ok_or(JournalError::ProposalIntentNotFound)?;
        if intent.binding() != published_binding {
            return Err(JournalError::ProposalIntentConflict);
        }
        if let Some(review_id) = intent.review_id() {
            let review = load_review(&transaction, review_id)?.ok_or_else(|| {
                JournalError::CorruptData(
                    "finalized Proposal intent references a missing review".into(),
                )
            })?;
            if review.binding() != published_binding {
                return Err(JournalError::CorruptData(
                    "finalized Proposal intent and review binding differ".into(),
                ));
            }
            return Ok(RegisteredReview {
                review,
                outcome: ReviewRegistrationOutcome::Replayed,
            });
        }

        if let Some(existing) = load_review_by_proposal(&transaction, published_binding)? {
            if existing.binding() != published_binding {
                return Err(JournalError::ReviewBindingConflict);
            }
            attach_review_to_proposal_intent(
                &transaction,
                proposal_intent_id,
                existing.review_id(),
            )?;
            transaction.commit()?;
            return Ok(RegisteredReview {
                review: existing,
                outcome: ReviewRegistrationOutcome::Replayed,
            });
        }

        for _ in 0..RANDOM_ID_ATTEMPTS {
            let review_id = ReviewId::generate()?;
            let inserted = transaction.execute(
                "INSERT OR IGNORE INTO reviews(
                    review_id, project_scope, proposal_ref_name, proposal_head,
                    decision_ref_name, expected_decision_head, state
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'pending_review')",
                params![
                    review_id.as_str(),
                    published_binding.project_scope(),
                    published_binding.proposal_ref_name(),
                    published_binding.proposal_head(),
                    published_binding.decision_ref_name(),
                    published_binding.expected_decision_head(),
                ],
            )?;
            if inserted == 1 {
                attach_review_to_proposal_intent(&transaction, proposal_intent_id, &review_id)?;
                let review = ReviewRecord {
                    review_id,
                    binding: published_binding.clone(),
                    state: ReviewState::PendingReview,
                };
                transaction.commit()?;
                return Ok(RegisteredReview {
                    review,
                    outcome: ReviewRegistrationOutcome::Created,
                });
            }
            if let Some(existing) = load_review_by_proposal(&transaction, published_binding)? {
                if existing.binding() != published_binding {
                    return Err(JournalError::ReviewBindingConflict);
                }
                attach_review_to_proposal_intent(
                    &transaction,
                    proposal_intent_id,
                    existing.review_id(),
                )?;
                transaction.commit()?;
                return Ok(RegisteredReview {
                    review: existing,
                    outcome: ReviewRegistrationOutcome::Replayed,
                });
            }
        }
        Err(JournalError::Random(
            "could not allocate a unique review identifier".into(),
        ))
    }

    pub fn get_proposal_intent(
        &self,
        proposal_intent_id: &ProposalIntentId,
    ) -> Result<ProposalIntent> {
        let intent = load_proposal_intent(&self.connection, proposal_intent_id)?
            .ok_or(JournalError::ProposalIntentNotFound)?;
        validate_proposal_intent_review_link(&self.connection, &intent)?;
        Ok(intent)
    }

    /// Find a Proposal intent by its private idempotency key within one trusted
    /// project scope.
    ///
    /// The raw key is hashed before SQLite is queried and is never persisted.
    /// Callers must authenticate and authorize `project_scope` before using the
    /// result outside trusted restart orchestration.
    pub fn get_proposal_intent_by_idempotency(
        &self,
        project_scope: &str,
        idempotency_key: &[u8],
    ) -> Result<Option<ProposalIntent>> {
        validate_control_value("project_scope", project_scope)?;
        validate_idempotency_key(idempotency_key)?;
        let idempotency_digest = digest(
            PROPOSAL_IDEMPOTENCY_DOMAIN,
            &[project_scope.as_bytes(), idempotency_key],
        );
        let intent = load_proposal_intent_by_idempotency(
            &self.connection,
            project_scope,
            &idempotency_digest,
        )?;
        if let Some(intent) = intent.as_ref() {
            validate_proposal_intent_review_link(&self.connection, intent)?;
        }
        Ok(intent)
    }

    /// Find an exact private intent from trusted server-owned binding data.
    pub fn get_proposal_intent_by_binding(
        &self,
        binding: &ReviewBinding,
    ) -> Result<Option<ProposalIntent>> {
        binding.validate()?;
        let Some(intent) = load_proposal_intent_by_proposal(&self.connection, binding)? else {
            return Ok(None);
        };
        validate_proposal_intent_review_link(&self.connection, &intent)?;
        Ok((intent.binding() == binding).then_some(intent))
    }

    /// Read the public Proposal manifest digest linked to a durable review.
    ///
    /// Compatibility reviews created through the schema-v1 API have no
    /// Proposal intent and therefore return `None`.
    pub fn get_review_artifact_manifest_sha256(
        &self,
        review_id: &ReviewId,
    ) -> Result<Option<String>> {
        if !review_exists(&self.connection, review_id)? {
            return Err(JournalError::ReviewNotFound);
        }
        let intent = load_proposal_intent_by_review(&self.connection, review_id)?;
        if let Some(intent) = intent.as_ref() {
            validate_proposal_intent_review_link(&self.connection, intent)?;
        }
        Ok(intent.map(|intent| intent.artifact_manifest_sha256))
    }

    /// List private intents still awaiting publication reconciliation.
    ///
    /// This is an internal worker API. The caller must authenticate and scope
    /// `project_scope` before exposing any result outside trusted orchestration.
    pub fn list_unfinalized_proposal_intents(
        &self,
        project_scope: &str,
    ) -> Result<Vec<ProposalIntent>> {
        validate_control_value("project_scope", project_scope)?;
        let mut statement = self.connection.prepare(
            "SELECT proposal_intent_id, idempotency_digest, request_fingerprint,
                    artifact_manifest_sha256,
                    project_scope, proposal_ref_name, proposal_head,
                    decision_ref_name, expected_decision_head, review_id
             FROM proposal_intents
             WHERE project_scope = ?1 AND review_id IS NULL
             ORDER BY proposal_intent_id",
        )?;
        let stored = statement.query_map([project_scope], proposal_intent_tuple)?;
        let mut intents = Vec::new();
        for row in stored {
            intents.push(decode_proposal_intent(row?)?);
        }
        Ok(intents)
    }

    /// Create a pending review after the caller's Proposal publication succeeds.
    ///
    /// This compatibility method rejects an existing binding. Callers that may
    /// lose the create response should use [`Self::create_or_get_review`] so an
    /// exact retry returns the original opaque locator.
    pub fn create_review(&mut self, binding: ReviewBinding) -> Result<ReviewRecord> {
        match self.create_or_get_review(binding)? {
            RegisteredReview {
                review,
                outcome: ReviewRegistrationOutcome::Created,
            } => Ok(review),
            RegisteredReview {
                outcome: ReviewRegistrationOutcome::Replayed,
                ..
            } => Err(JournalError::ReviewBindingExists),
        }
    }

    /// Create a review or replay the exact previously stored binding.
    ///
    /// The Proposal identity `(project, Ref, head)` is unique. Replaying every
    /// exact binding returns the existing `ReviewId`; changing its Decision Ref
    /// or expected head is a binding conflict rather than a new review.
    pub fn create_or_get_review(&mut self, binding: ReviewBinding) -> Result<RegisteredReview> {
        binding.validate()?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(existing) = load_review_by_proposal(&transaction, &binding)? {
            if existing.binding == binding {
                return Ok(RegisteredReview {
                    review: existing,
                    outcome: ReviewRegistrationOutcome::Replayed,
                });
            }
            return Err(JournalError::ReviewBindingConflict);
        }
        for _ in 0..RANDOM_ID_ATTEMPTS {
            let review_id = ReviewId::generate()?;
            let inserted = transaction.execute(
                "INSERT OR IGNORE INTO reviews(
                    review_id, project_scope, proposal_ref_name, proposal_head,
                    decision_ref_name, expected_decision_head, state
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'pending_review')",
                params![
                    review_id.as_str(),
                    binding.project_scope(),
                    binding.proposal_ref_name(),
                    binding.proposal_head(),
                    binding.decision_ref_name(),
                    binding.expected_decision_head(),
                ],
            );
            match inserted {
                Ok(1) => {
                    transaction.commit()?;
                    return Ok(RegisteredReview {
                        review: ReviewRecord {
                            review_id,
                            binding,
                            state: ReviewState::PendingReview,
                        },
                        outcome: ReviewRegistrationOutcome::Created,
                    });
                }
                Ok(0) => {
                    if let Some(existing) = load_review_by_proposal(&transaction, &binding)? {
                        if existing.binding == binding {
                            return Ok(RegisteredReview {
                                review: existing,
                                outcome: ReviewRegistrationOutcome::Replayed,
                            });
                        }
                        return Err(JournalError::ReviewBindingConflict);
                    }
                    continue;
                }
                Ok(_) => unreachable!("one review insert changed more than one row"),
                Err(error) if is_constraint(&error) => {
                    if let Some(existing) = load_review_by_proposal(&transaction, &binding)? {
                        if existing.binding == binding {
                            return Ok(RegisteredReview {
                                review: existing,
                                outcome: ReviewRegistrationOutcome::Replayed,
                            });
                        }
                        return Err(JournalError::ReviewBindingConflict);
                    }
                    continue;
                }
                Err(error) => return Err(error.into()),
            }
        }
        Err(JournalError::Random(
            "could not allocate a unique review identifier".into(),
        ))
    }

    pub fn get_review(&self, review_id: &ReviewId) -> Result<ReviewRecord> {
        load_review(&self.connection, review_id)?.ok_or(JournalError::ReviewNotFound)
    }

    /// Find an exact server-owned binding after project authorization.
    pub fn get_review_by_binding(&self, binding: &ReviewBinding) -> Result<Option<ReviewRecord>> {
        binding.validate()?;
        let Some(record) = load_review_by_proposal(&self.connection, binding)? else {
            return Ok(None);
        };
        Ok((record.binding() == binding).then_some(record))
    }

    /// Compare and set a durable state without interpreting repository authority.
    ///
    /// Repeating the same state is idempotent and returns the existing record.
    /// Committed and terminal-denial states are terminal. `outcome_unknown` may
    /// be reconciled only to a terminal state; it cannot blindly return to
    /// `pending_review` or be relabeled as a retryable failure. For v2 reviews
    /// linked to a Proposal intent, and for any legacy review upgraded with a
    /// strict Decision intent, `outcome_unknown` requires that intent and
    /// `decision_committed` is reserved for [`Self::commit_decision_outcome`].
    /// Legacy rows without strict v2 state retain the v1 transition behavior.
    pub fn transition_review_state(
        &mut self,
        review_id: &ReviewId,
        expected: ReviewState,
        next: ReviewState,
    ) -> Result<ReviewRecord> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current = load_review(&transaction, review_id)?.ok_or(JournalError::ReviewNotFound)?;
        let actual = current.state;
        if actual != expected {
            return Err(JournalError::StateConflict { expected, actual });
        }
        let proposal_linked = load_proposal_intent_by_review(&transaction, review_id)?.is_some();
        let strict_intent = load_decision_commit_intent(&transaction, review_id)?;
        let strict_outcome = load_decision_outcome(&transaction, review_id)?;
        if proposal_linked || strict_intent.is_some() || strict_outcome.is_some() {
            match next {
                ReviewState::OutcomeUnknown if strict_intent.is_none() => {
                    return if actual == ReviewState::OutcomeUnknown {
                        Err(JournalError::CorruptData(
                            "strict outcome_unknown review has no Decision intent".into(),
                        ))
                    } else {
                        Err(JournalError::InvalidStateTransition {
                            from: actual,
                            to: next,
                        })
                    };
                }
                ReviewState::DecisionCommitted if actual != ReviewState::DecisionCommitted => {
                    return Err(JournalError::InvalidStateTransition {
                        from: actual,
                        to: next,
                    });
                }
                ReviewState::DecisionCommitted
                    if strict_intent.is_none() || strict_outcome.is_none() =>
                {
                    return Err(JournalError::CorruptData(
                        "strict decision_committed review lacks its intent or outcome".into(),
                    ));
                }
                _ => {}
            }
        }
        if actual == next {
            return Ok(current);
        }
        if !state_transition_allowed(actual, next) {
            return Err(JournalError::InvalidStateTransition {
                from: actual,
                to: next,
            });
        }
        let changed = transaction.execute(
            "UPDATE reviews SET state = ?1 WHERE review_id = ?2 AND state = ?3",
            params![next.as_str(), review_id.as_str(), expected.as_str()],
        )?;
        if changed != 1 {
            return Err(JournalError::StateConflict { expected, actual });
        }
        let stored = transaction.query_row(
            "SELECT review_id, project_scope, proposal_ref_name, proposal_head,
                    decision_ref_name, expected_decision_head, state
             FROM reviews WHERE review_id = ?1",
            [review_id.as_str()],
            review_tuple,
        )?;
        let record = decode_review(stored)?;
        transaction.commit()?;
        Ok(record)
    }

    /// Persist or replay one idempotent Decision intent.
    pub fn register_decision_intent(
        &mut self,
        review_id: &ReviewId,
        request: DecisionIntentRequest<'_>,
    ) -> Result<RegisteredDecisionIntent> {
        validate_intent_request(&request)?;
        let idempotency_digest = digest(LEGACY_IDEMPOTENCY_DOMAIN, &[request.idempotency_key]);
        let request_fingerprint = digest(
            REQUEST_DOMAIN,
            &[
                request.canonical_request,
                request.candidate_head.as_bytes(),
                request.feedback_oid.as_bytes(),
                request.expected_decision_head.as_bytes(),
            ],
        );
        let candidate = DecisionIntent {
            review_id: review_id.clone(),
            idempotency_digest,
            request_fingerprint,
            candidate_head: request.candidate_head.to_owned(),
            feedback_oid: request.feedback_oid.to_owned(),
            expected_decision_head: request.expected_decision_head.to_owned(),
        };

        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let review = load_review(&transaction, review_id)?.ok_or(JournalError::ReviewNotFound)?;
        if let Some(existing) = load_intent(&transaction, review_id)? {
            if existing.idempotency_digest == candidate.idempotency_digest {
                if existing == candidate {
                    return Ok(RegisteredDecisionIntent {
                        intent: existing,
                        outcome: IntentRegistrationOutcome::Replayed,
                    });
                }
                return Err(JournalError::IdempotencyConflict);
            }
            return Err(JournalError::DecisionIntentExists);
        }
        if request.expected_decision_head != review.binding.expected_decision_head() {
            return Err(JournalError::ReviewBindingConflict);
        }
        if !matches!(
            review.state,
            ReviewState::PendingReview | ReviewState::RetryableFailure
        ) {
            return Err(JournalError::StateConflict {
                expected: ReviewState::PendingReview,
                actual: review.state,
            });
        }
        transaction.execute(
            "INSERT INTO decision_intents(
                review_id, idempotency_digest, request_fingerprint,
                candidate_head, feedback_oid, expected_decision_head
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                candidate.review_id.as_str(),
                &candidate.idempotency_digest,
                &candidate.request_fingerprint,
                &candidate.candidate_head,
                &candidate.feedback_oid,
                &candidate.expected_decision_head,
            ],
        )?;
        transaction.commit()?;
        Ok(RegisteredDecisionIntent {
            intent: candidate,
            outcome: IntentRegistrationOutcome::Created,
        })
    }

    /// Persist or replay one fully bound v2 Decision commit intent.
    ///
    /// The full [`ReviewBinding`] is copied from the journal rather than caller
    /// input. This produces the exact durable command needed to reconcile a
    /// restart before or after Decision CAS.
    pub fn register_decision_commit_intent(
        &mut self,
        review_id: &ReviewId,
        request: DecisionCommitIntentRequest<'_>,
    ) -> Result<RegisteredDecisionCommitIntent> {
        validate_decision_commit_intent_request(&request)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let review = load_review(&transaction, review_id)?.ok_or(JournalError::ReviewNotFound)?;
        if request.disposition == DecisionDisposition::AdoptedUnchanged
            && load_proposal_intent_by_review(&transaction, review_id)?.is_some_and(|proposal| {
                proposal.artifact_manifest_sha256() != request.reviewed_artifact_manifest_sha256
            })
        {
            return Err(JournalError::DecisionIntentMismatch);
        }
        let idempotency_digest = digest(
            DECISION_COMMIT_IDEMPOTENCY_DOMAIN,
            &[
                review.binding.project_scope().as_bytes(),
                review_id.as_str().as_bytes(),
                request.idempotency_key,
            ],
        );
        if request.new_decision_head == review.binding.expected_decision_head() {
            return Err(JournalError::InvalidArgument(
                "new_decision_head must differ from expected_decision_head".into(),
            ));
        }
        let request_fingerprint = digest(
            DECISION_COMMIT_REQUEST_DOMAIN,
            &[
                request.canonical_request,
                review.binding.project_scope().as_bytes(),
                review.binding.proposal_ref_name().as_bytes(),
                review.binding.proposal_head().as_bytes(),
                review.binding.decision_ref_name().as_bytes(),
                review.binding.expected_decision_head().as_bytes(),
                request.disposition.as_str().as_bytes(),
                request.selected_snapshot.as_str().as_bytes(),
                request.reviewed_artifact_manifest_sha256.as_bytes(),
                request.new_decision_head.as_bytes(),
                request.feedback_oid.as_bytes(),
            ],
        );
        let candidate = DecisionCommitIntent {
            review_id: review_id.clone(),
            idempotency_digest,
            request_fingerprint,
            binding: review.binding.clone(),
            disposition: request.disposition,
            selected_snapshot: request.selected_snapshot,
            reviewed_artifact_manifest_sha256: request.reviewed_artifact_manifest_sha256.to_owned(),
            new_decision_head: request.new_decision_head.to_owned(),
            feedback_oid: request.feedback_oid.to_owned(),
        };

        if load_intent(&transaction, review_id)?.is_some() {
            let Some(existing) = load_decision_commit_intent(&transaction, review_id)? else {
                return Err(JournalError::LegacyDecisionIntent);
            };
            if existing.idempotency_digest == candidate.idempotency_digest {
                if existing == candidate {
                    return Ok(RegisteredDecisionCommitIntent {
                        intent: existing,
                        outcome: IntentRegistrationOutcome::Replayed,
                    });
                }
                return Err(JournalError::IdempotencyConflict);
            }
            return Err(JournalError::DecisionIntentExists);
        }
        if !matches!(
            review.state,
            ReviewState::PendingReview | ReviewState::RetryableFailure
        ) {
            return Err(JournalError::StateConflict {
                expected: ReviewState::PendingReview,
                actual: review.state,
            });
        }

        transaction.execute(
            "INSERT INTO decision_intents(
                review_id, idempotency_digest, request_fingerprint,
                candidate_head, feedback_oid, expected_decision_head
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                candidate.review_id.as_str(),
                &candidate.idempotency_digest,
                &candidate.request_fingerprint,
                &candidate.new_decision_head,
                &candidate.feedback_oid,
                candidate.binding.expected_decision_head(),
            ],
        )?;
        transaction.execute(
            "INSERT INTO decision_commit_intents(
                review_id, project_scope, proposal_ref_name, proposal_head,
                decision_ref_name, disposition, selected_snapshot,
                reviewed_artifact_manifest_sha256
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                candidate.review_id.as_str(),
                candidate.binding.project_scope(),
                candidate.binding.proposal_ref_name(),
                candidate.binding.proposal_head(),
                candidate.binding.decision_ref_name(),
                candidate.disposition.as_str(),
                candidate.selected_snapshot.as_str(),
                &candidate.reviewed_artifact_manifest_sha256,
            ],
        )?;
        transaction.commit()?;
        Ok(RegisteredDecisionCommitIntent {
            intent: candidate,
            outcome: IntentRegistrationOutcome::Created,
        })
    }

    /// Read the strict v2 intent, returning `None` for no intent or a legacy v1
    /// intent that lacks exact outcome bindings.
    pub fn get_decision_commit_intent(
        &self,
        review_id: &ReviewId,
    ) -> Result<Option<DecisionCommitIntent>> {
        let review =
            load_review(&self.connection, review_id)?.ok_or(JournalError::ReviewNotFound)?;
        let intent = load_decision_commit_intent(&self.connection, review_id)?;
        if intent
            .as_ref()
            .is_some_and(|intent| intent.binding() != review.binding())
        {
            return Err(JournalError::CorruptData(
                "Decision intent and review binding differ".into(),
            ));
        }
        Ok(intent)
    }

    /// Record an externally reconciled proof that Decision CAS did not commit.
    ///
    /// The trusted caller must first compare the live Ref/reflog with this exact
    /// stored v2 intent. The journal performs no Core verification itself. This
    /// narrow API is the only way `outcome_unknown` can become retryable; the
    /// generic state transition intentionally continues to reject that move.
    pub fn reconcile_decision_not_committed(
        &mut self,
        intent: &DecisionCommitIntent,
    ) -> Result<ReviewRecord> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let review =
            load_review(&transaction, intent.review_id())?.ok_or(JournalError::ReviewNotFound)?;
        let stored = load_decision_commit_intent(&transaction, intent.review_id())?
            .ok_or(JournalError::DecisionIntentMismatch)?;
        if &stored != intent || stored.binding() != review.binding() {
            return Err(JournalError::DecisionIntentMismatch);
        }
        if load_decision_outcome(&transaction, intent.review_id())?.is_some() {
            return Err(JournalError::StateConflict {
                expected: ReviewState::OutcomeUnknown,
                actual: review.state,
            });
        }
        match review.state {
            ReviewState::PendingReview | ReviewState::RetryableFailure => Ok(review),
            ReviewState::OutcomeUnknown => {
                let changed = transaction.execute(
                    "UPDATE reviews SET state = 'retryable_failure'
                     WHERE review_id = ?1 AND state = 'outcome_unknown'",
                    [intent.review_id().as_str()],
                )?;
                if changed != 1 {
                    return Err(JournalError::StateConflict {
                        expected: ReviewState::OutcomeUnknown,
                        actual: load_review(&transaction, intent.review_id())?
                            .ok_or(JournalError::ReviewNotFound)?
                            .state,
                    });
                }
                let reconciled = ReviewRecord {
                    review_id: review.review_id,
                    binding: review.binding,
                    state: ReviewState::RetryableFailure,
                };
                transaction.commit()?;
                Ok(reconciled)
            }
            ReviewState::DecisionCommitted | ReviewState::TerminalDenial => {
                Err(JournalError::StateConflict {
                    expected: ReviewState::OutcomeUnknown,
                    actual: review.state,
                })
            }
        }
    }

    /// Atomically persist a caller-verified receipt and mark the review
    /// `decision_committed`.
    ///
    /// Every field must exactly match both the stored v2 Decision intent and
    /// current durable Review binding. The caller remains responsible for Core
    /// receipt verification. Exact retries replay the same outcome; changed
    /// retries fail without modifying either row.
    pub fn commit_decision_outcome(
        &mut self,
        review_id: &ReviewId,
        request: DecisionOutcomeRequest<'_>,
    ) -> Result<CommittedDecisionOutcome> {
        validate_decision_outcome_request(&request)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let review = load_review(&transaction, review_id)?.ok_or(JournalError::ReviewNotFound)?;
        let candidate = DecisionOutcome {
            review_id: review_id.clone(),
            disposition: request.disposition,
            selected_snapshot: request.selected_snapshot,
            reviewed_artifact_manifest_sha256: request.reviewed_artifact_manifest_sha256.to_owned(),
            proposal_head: request.proposal_head.to_owned(),
            expected_decision_head: request.expected_decision_head.to_owned(),
            new_decision_head: request.new_decision_head.to_owned(),
            feedback_oid: request.feedback_oid.to_owned(),
        };
        let strict_intent = load_decision_commit_intent(&transaction, review_id)?;

        if let Some(existing) = load_decision_outcome(&transaction, review_id)? {
            if review.state != ReviewState::DecisionCommitted {
                return Err(JournalError::CorruptData(
                    "Decision outcome exists without decision_committed state".into(),
                ));
            }
            if existing == candidate {
                let intent = strict_intent.as_ref().ok_or_else(|| {
                    JournalError::CorruptData(
                        "Decision outcome exists without its strict v2 intent".into(),
                    )
                })?;
                if !decision_outcome_matches_intent(intent, &review, &existing) {
                    return Err(JournalError::CorruptData(
                        "Decision outcome, intent, and review binding differ".into(),
                    ));
                }
                return Ok(CommittedDecisionOutcome {
                    outcome: existing,
                    registration: DecisionOutcomeRegistrationOutcome::Replayed,
                });
            }
            return Err(JournalError::DecisionOutcomeConflict);
        }

        let Some(intent) = strict_intent else {
            return if load_intent(&transaction, review_id)?.is_some() {
                Err(JournalError::LegacyDecisionIntent)
            } else {
                Err(JournalError::DecisionIntentMismatch)
            };
        };
        if !decision_outcome_matches_intent(&intent, &review, &candidate) {
            return Err(JournalError::DecisionIntentMismatch);
        }
        if !matches!(
            review.state,
            ReviewState::PendingReview
                | ReviewState::RetryableFailure
                | ReviewState::OutcomeUnknown
        ) {
            return Err(JournalError::StateConflict {
                expected: ReviewState::PendingReview,
                actual: review.state,
            });
        }

        transaction.execute(
            "INSERT INTO decision_outcomes(
                review_id, disposition, selected_snapshot,
                reviewed_artifact_manifest_sha256, proposal_head,
                expected_decision_head, new_decision_head, feedback_oid
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                candidate.review_id.as_str(),
                candidate.disposition.as_str(),
                candidate.selected_snapshot.as_str(),
                &candidate.reviewed_artifact_manifest_sha256,
                &candidate.proposal_head,
                &candidate.expected_decision_head,
                &candidate.new_decision_head,
                &candidate.feedback_oid,
            ],
        )?;
        let changed = transaction.execute(
            "UPDATE reviews SET state = 'decision_committed'
             WHERE review_id = ?1 AND state = ?2",
            params![review_id.as_str(), review.state.as_str()],
        )?;
        if changed != 1 {
            return Err(JournalError::StateConflict {
                expected: review.state,
                actual: load_review(&transaction, review_id)?
                    .ok_or(JournalError::ReviewNotFound)?
                    .state,
            });
        }
        transaction.commit()?;
        Ok(CommittedDecisionOutcome {
            outcome: candidate,
            registration: DecisionOutcomeRegistrationOutcome::Created,
        })
    }

    pub fn get_decision_outcome(&self, review_id: &ReviewId) -> Result<Option<DecisionOutcome>> {
        let review =
            load_review(&self.connection, review_id)?.ok_or(JournalError::ReviewNotFound)?;
        let outcome = load_decision_outcome(&self.connection, review_id)?;
        if let Some(outcome) = outcome.as_ref() {
            let intent =
                load_decision_commit_intent(&self.connection, review_id)?.ok_or_else(|| {
                    JournalError::CorruptData(
                        "Decision outcome exists without its strict v2 intent".into(),
                    )
                })?;
            if review.state() != ReviewState::DecisionCommitted
                || !decision_outcome_matches_intent(&intent, &review, outcome)
            {
                return Err(JournalError::CorruptData(
                    "Decision outcome, intent, state, and review binding differ".into(),
                ));
            }
        }
        Ok(outcome)
    }

    /// Return a transactionally consistent restart/reconciliation view.
    pub fn get_review_reconciliation(
        &mut self,
        review_id: &ReviewId,
    ) -> Result<ReviewReconciliation> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Deferred)?;
        let review = load_review(&transaction, review_id)?.ok_or(JournalError::ReviewNotFound)?;
        let proposal_intent = load_proposal_intent_by_review(&transaction, review_id)?;
        if let Some(intent) = proposal_intent.as_ref() {
            validate_proposal_intent_review_link(&transaction, intent)?;
        }
        let proposal_linked = proposal_intent.is_some();
        let proposal_artifact_manifest_sha256 =
            proposal_intent.map(|intent| intent.artifact_manifest_sha256);
        let decision_intent = load_decision_commit_intent(&transaction, review_id)?;
        let decision_outcome = load_decision_outcome(&transaction, review_id)?;
        if decision_intent
            .as_ref()
            .is_some_and(|intent| intent.binding() != review.binding())
        {
            return Err(JournalError::CorruptData(
                "Decision intent and review binding differ".into(),
            ));
        }
        if decision_outcome.is_some() && review.state != ReviewState::DecisionCommitted {
            return Err(JournalError::CorruptData(
                "Decision outcome exists without decision_committed state".into(),
            ));
        }
        if decision_outcome.is_some() && decision_intent.is_none() {
            return Err(JournalError::CorruptData(
                "Decision outcome exists without its strict v2 intent".into(),
            ));
        }
        if (proposal_linked || decision_intent.is_some() || decision_outcome.is_some())
            && review.state == ReviewState::DecisionCommitted
            && (decision_intent.is_none() || decision_outcome.is_none())
        {
            return Err(JournalError::CorruptData(
                "strict decision_committed review lacks its intent or outcome".into(),
            ));
        }
        if proposal_linked
            && review.state == ReviewState::OutcomeUnknown
            && decision_intent.is_none()
        {
            return Err(JournalError::CorruptData(
                "v2 outcome_unknown review has no strict Decision intent".into(),
            ));
        }
        if let (Some(intent), Some(outcome)) = (&decision_intent, &decision_outcome)
            && !decision_outcome_matches_intent(intent, &review, outcome)
        {
            return Err(JournalError::CorruptData(
                "Decision outcome, intent, and review binding differ".into(),
            ));
        }
        transaction.commit()?;
        Ok(ReviewReconciliation {
            review,
            proposal_artifact_manifest_sha256,
            decision_intent,
            decision_outcome,
        })
    }

    pub fn get_decision_intent(&self, review_id: &ReviewId) -> Result<Option<DecisionIntent>> {
        if !review_exists(&self.connection, review_id)? {
            return Err(JournalError::ReviewNotFound);
        }
        load_intent(&self.connection, review_id)
    }
}

type StoredReview = (String, String, String, String, String, String, String);

fn review_tuple(row: &rusqlite::Row<'_>) -> rusqlite::Result<StoredReview> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
        row.get(6)?,
    ))
}

fn decode_review(stored: StoredReview) -> Result<ReviewRecord> {
    let (review_id, project, proposal_ref, proposal_head, decision_ref, decision_head, state) =
        stored;
    Ok(ReviewRecord {
        review_id: ReviewId::parse(review_id)
            .map_err(|error| JournalError::CorruptData(error.to_string()))?,
        binding: ReviewBinding::new(
            project,
            proposal_ref,
            proposal_head,
            decision_ref,
            decision_head,
        )
        .map_err(|error| JournalError::CorruptData(error.to_string()))?,
        state: ReviewState::parse(&state)?,
    })
}

fn load_review(connection: &Connection, review_id: &ReviewId) -> Result<Option<ReviewRecord>> {
    connection
        .query_row(
            "SELECT review_id, project_scope, proposal_ref_name, proposal_head,
                    decision_ref_name, expected_decision_head, state
             FROM reviews WHERE review_id = ?1",
            [review_id.as_str()],
            review_tuple,
        )
        .optional()?
        .map(decode_review)
        .transpose()
}

fn load_review_by_proposal(
    connection: &Connection,
    binding: &ReviewBinding,
) -> Result<Option<ReviewRecord>> {
    connection
        .query_row(
            "SELECT review_id, project_scope, proposal_ref_name, proposal_head,
                    decision_ref_name, expected_decision_head, state
             FROM reviews
             WHERE project_scope = ?1 AND proposal_ref_name = ?2 AND proposal_head = ?3",
            params![
                binding.project_scope(),
                binding.proposal_ref_name(),
                binding.proposal_head(),
            ],
            review_tuple,
        )
        .optional()?
        .map(decode_review)
        .transpose()
}

type StoredProposalIntent = (
    String,
    String,
    String,
    String,
    String,
    String,
    String,
    String,
    String,
    Option<String>,
);

fn proposal_intent_tuple(row: &rusqlite::Row<'_>) -> rusqlite::Result<StoredProposalIntent> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
        row.get(6)?,
        row.get(7)?,
        row.get(8)?,
        row.get(9)?,
    ))
}

fn decode_proposal_intent(stored: StoredProposalIntent) -> Result<ProposalIntent> {
    let (
        proposal_intent_id,
        idempotency_digest,
        request_fingerprint,
        artifact_manifest_sha256,
        project_scope,
        proposal_ref_name,
        proposal_head,
        decision_ref_name,
        expected_decision_head,
        review_id,
    ) = stored;
    for (label, value) in [
        ("idempotency_digest", idempotency_digest.as_str()),
        ("request_fingerprint", request_fingerprint.as_str()),
    ] {
        if !valid_sha256(value) {
            return Err(JournalError::CorruptData(format!(
                "Proposal intent {label} is not a SHA-256 digest"
            )));
        }
    }
    if !valid_artifact_manifest_sha256(&artifact_manifest_sha256) {
        return Err(JournalError::CorruptData(
            "Proposal intent manifest digest is not lowercase SHA-256".into(),
        ));
    }
    Ok(ProposalIntent {
        proposal_intent_id: ProposalIntentId::parse(proposal_intent_id)
            .map_err(|error| JournalError::CorruptData(error.to_string()))?,
        idempotency_digest,
        request_fingerprint,
        artifact_manifest_sha256,
        binding: ReviewBinding::new(
            project_scope,
            proposal_ref_name,
            proposal_head,
            decision_ref_name,
            expected_decision_head,
        )
        .map_err(|error| JournalError::CorruptData(error.to_string()))?,
        review_id: review_id
            .map(ReviewId::parse)
            .transpose()
            .map_err(|error| JournalError::CorruptData(error.to_string()))?,
    })
}

fn load_proposal_intent(
    connection: &Connection,
    proposal_intent_id: &ProposalIntentId,
) -> Result<Option<ProposalIntent>> {
    connection
        .query_row(
            "SELECT proposal_intent_id, idempotency_digest, request_fingerprint,
                    artifact_manifest_sha256,
                    project_scope, proposal_ref_name, proposal_head,
                    decision_ref_name, expected_decision_head, review_id
             FROM proposal_intents WHERE proposal_intent_id = ?1",
            [proposal_intent_id.as_str()],
            proposal_intent_tuple,
        )
        .optional()?
        .map(decode_proposal_intent)
        .transpose()
}

fn load_proposal_intent_by_idempotency(
    connection: &Connection,
    project_scope: &str,
    idempotency_digest: &str,
) -> Result<Option<ProposalIntent>> {
    connection
        .query_row(
            "SELECT proposal_intent_id, idempotency_digest, request_fingerprint,
                    artifact_manifest_sha256,
                    project_scope, proposal_ref_name, proposal_head,
                    decision_ref_name, expected_decision_head, review_id
             FROM proposal_intents
             WHERE project_scope = ?1 AND idempotency_digest = ?2",
            params![project_scope, idempotency_digest],
            proposal_intent_tuple,
        )
        .optional()?
        .map(decode_proposal_intent)
        .transpose()
}

fn load_proposal_intent_by_proposal(
    connection: &Connection,
    binding: &ReviewBinding,
) -> Result<Option<ProposalIntent>> {
    connection
        .query_row(
            "SELECT proposal_intent_id, idempotency_digest, request_fingerprint,
                    artifact_manifest_sha256,
                    project_scope, proposal_ref_name, proposal_head,
                    decision_ref_name, expected_decision_head, review_id
             FROM proposal_intents
             WHERE project_scope = ?1 AND proposal_ref_name = ?2 AND proposal_head = ?3",
            params![
                binding.project_scope(),
                binding.proposal_ref_name(),
                binding.proposal_head(),
            ],
            proposal_intent_tuple,
        )
        .optional()?
        .map(decode_proposal_intent)
        .transpose()
}

fn load_proposal_intent_by_review(
    connection: &Connection,
    review_id: &ReviewId,
) -> Result<Option<ProposalIntent>> {
    connection
        .query_row(
            "SELECT proposal_intent_id, idempotency_digest, request_fingerprint,
                    artifact_manifest_sha256,
                    project_scope, proposal_ref_name, proposal_head,
                    decision_ref_name, expected_decision_head, review_id
             FROM proposal_intents WHERE review_id = ?1",
            [review_id.as_str()],
            proposal_intent_tuple,
        )
        .optional()?
        .map(decode_proposal_intent)
        .transpose()
}

fn attach_review_to_proposal_intent(
    connection: &Connection,
    proposal_intent_id: &ProposalIntentId,
    review_id: &ReviewId,
) -> Result<()> {
    let changed = connection.execute(
        "UPDATE proposal_intents SET review_id = ?1
         WHERE proposal_intent_id = ?2 AND review_id IS NULL",
        params![review_id.as_str(), proposal_intent_id.as_str()],
    )?;
    if changed == 1 {
        return Ok(());
    }
    let existing = load_proposal_intent(connection, proposal_intent_id)?
        .ok_or(JournalError::ProposalIntentNotFound)?;
    if existing.review_id() == Some(review_id) {
        Ok(())
    } else {
        Err(JournalError::ProposalIntentConflict)
    }
}

fn validate_proposal_intent_review_link(
    connection: &Connection,
    intent: &ProposalIntent,
) -> Result<()> {
    let Some(review_id) = intent.review_id() else {
        return Ok(());
    };
    let review = load_review(connection, review_id)?.ok_or_else(|| {
        JournalError::CorruptData("finalized Proposal intent references a missing review".into())
    })?;
    if review.binding() != intent.binding() {
        return Err(JournalError::CorruptData(
            "finalized Proposal intent and review binding differ".into(),
        ));
    }
    Ok(())
}

fn review_exists(connection: &Connection, review_id: &ReviewId) -> Result<bool> {
    Ok(connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM reviews WHERE review_id = ?1)",
        [review_id.as_str()],
        |row| row.get(0),
    )?)
}

fn state_transition_allowed(from: ReviewState, to: ReviewState) -> bool {
    match from {
        ReviewState::PendingReview => true,
        ReviewState::RetryableFailure => matches!(
            to,
            ReviewState::DecisionCommitted
                | ReviewState::TerminalDenial
                | ReviewState::OutcomeUnknown
        ),
        ReviewState::OutcomeUnknown => {
            matches!(
                to,
                ReviewState::DecisionCommitted | ReviewState::TerminalDenial
            )
        }
        ReviewState::DecisionCommitted | ReviewState::TerminalDenial => false,
    }
}

fn load_intent(connection: &Connection, review_id: &ReviewId) -> Result<Option<DecisionIntent>> {
    let stored = connection
        .query_row(
            "SELECT idempotency_digest, request_fingerprint, candidate_head,
                    feedback_oid, expected_decision_head
             FROM decision_intents WHERE review_id = ?1",
            [review_id.as_str()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                ))
            },
        )
        .optional()?;
    stored
        .map(
            |(idempotency_digest, request_fingerprint, candidate, feedback, expected)| {
                for (label, digest_value) in [
                    ("idempotency_digest", idempotency_digest.as_str()),
                    ("request_fingerprint", request_fingerprint.as_str()),
                ] {
                    if !valid_sha256(digest_value) {
                        return Err(JournalError::CorruptData(format!(
                            "{label} is not a SHA-256 digest"
                        )));
                    }
                }
                for (label, value) in [
                    ("candidate_head", candidate.as_str()),
                    ("feedback_oid", feedback.as_str()),
                    ("expected_decision_head", expected.as_str()),
                ] {
                    validate_control_value(label, value)
                        .map_err(|error| JournalError::CorruptData(error.to_string()))?;
                }
                Ok(DecisionIntent {
                    review_id: review_id.clone(),
                    idempotency_digest,
                    request_fingerprint,
                    candidate_head: candidate,
                    feedback_oid: feedback,
                    expected_decision_head: expected,
                })
            },
        )
        .transpose()
}

fn load_decision_commit_intent(
    connection: &Connection,
    review_id: &ReviewId,
) -> Result<Option<DecisionCommitIntent>> {
    let stored = connection
        .query_row(
            "SELECT legacy.idempotency_digest, legacy.request_fingerprint,
                    legacy.candidate_head, legacy.feedback_oid,
                    legacy.expected_decision_head,
                    exact.project_scope, exact.proposal_ref_name,
                    exact.proposal_head, exact.decision_ref_name,
                    exact.disposition, exact.selected_snapshot,
                    exact.reviewed_artifact_manifest_sha256
             FROM decision_intents AS legacy
             JOIN decision_commit_intents AS exact
               ON exact.review_id = legacy.review_id
             WHERE legacy.review_id = ?1",
            [review_id.as_str()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, String>(7)?,
                    row.get::<_, String>(8)?,
                    row.get::<_, String>(9)?,
                    row.get::<_, String>(10)?,
                    row.get::<_, String>(11)?,
                ))
            },
        )
        .optional()?;
    stored
        .map(
            |(
                idempotency_digest,
                request_fingerprint,
                new_decision_head,
                feedback_oid,
                expected_decision_head,
                project_scope,
                proposal_ref_name,
                proposal_head,
                decision_ref_name,
                disposition,
                selected_snapshot,
                reviewed_artifact_manifest_sha256,
            )| {
                for (label, value) in [
                    ("idempotency_digest", idempotency_digest.as_str()),
                    ("request_fingerprint", request_fingerprint.as_str()),
                ] {
                    if !valid_sha256(value) {
                        return Err(JournalError::CorruptData(format!(
                            "Decision commit intent {label} is not a SHA-256 digest"
                        )));
                    }
                }
                let binding = ReviewBinding::new(
                    project_scope,
                    proposal_ref_name,
                    proposal_head,
                    decision_ref_name,
                    expected_decision_head,
                )
                .map_err(|error| JournalError::CorruptData(error.to_string()))?;
                let intent = DecisionCommitIntent {
                    review_id: review_id.clone(),
                    idempotency_digest,
                    request_fingerprint,
                    binding,
                    disposition: DecisionDisposition::parse(&disposition)?,
                    selected_snapshot: SelectedSnapshot::parse(&selected_snapshot)?,
                    reviewed_artifact_manifest_sha256,
                    new_decision_head,
                    feedback_oid,
                };
                validate_stored_decision_commit_intent(&intent)?;
                Ok(intent)
            },
        )
        .transpose()
}

fn load_decision_outcome(
    connection: &Connection,
    review_id: &ReviewId,
) -> Result<Option<DecisionOutcome>> {
    let stored = connection
        .query_row(
            "SELECT disposition, selected_snapshot,
                    reviewed_artifact_manifest_sha256, proposal_head,
                    expected_decision_head, new_decision_head, feedback_oid
             FROM decision_outcomes WHERE review_id = ?1",
            [review_id.as_str()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                ))
            },
        )
        .optional()?;
    stored
        .map(
            |(
                disposition,
                selected_snapshot,
                reviewed_artifact_manifest_sha256,
                proposal_head,
                expected_decision_head,
                new_decision_head,
                feedback_oid,
            )| {
                let outcome = DecisionOutcome {
                    review_id: review_id.clone(),
                    disposition: DecisionDisposition::parse(&disposition)?,
                    selected_snapshot: SelectedSnapshot::parse(&selected_snapshot)?,
                    reviewed_artifact_manifest_sha256,
                    proposal_head,
                    expected_decision_head,
                    new_decision_head,
                    feedback_oid,
                };
                validate_stored_decision_outcome(&outcome)?;
                Ok(outcome)
            },
        )
        .transpose()
}

fn validate_intent_request(request: &DecisionIntentRequest<'_>) -> Result<()> {
    validate_request_bytes(request.idempotency_key, request.canonical_request)?;
    for (label, value) in [
        ("candidate_head", request.candidate_head),
        ("feedback_oid", request.feedback_oid),
        ("expected_decision_head", request.expected_decision_head),
    ] {
        validate_control_value(label, value)?;
    }
    Ok(())
}

fn validate_proposal_intent_request(request: &ProposalIntentRequest<'_>) -> Result<()> {
    validate_request_bytes(request.idempotency_key, request.canonical_request)?;
    if !valid_artifact_manifest_sha256(request.artifact_manifest_sha256) {
        return Err(JournalError::InvalidArgument(
            "artifact_manifest_sha256 must be 64 lowercase hexadecimal characters".into(),
        ));
    }
    request.binding.validate()
}

fn validate_decision_commit_intent_request(
    request: &DecisionCommitIntentRequest<'_>,
) -> Result<()> {
    validate_request_bytes(request.idempotency_key, request.canonical_request)?;
    validate_decision_semantics(request.disposition, request.selected_snapshot)?;
    if !valid_artifact_manifest_sha256(request.reviewed_artifact_manifest_sha256) {
        return Err(JournalError::InvalidArgument(
            "reviewed_artifact_manifest_sha256 must be 64 lowercase hexadecimal characters".into(),
        ));
    }
    for (label, value) in [
        ("new_decision_head", request.new_decision_head),
        ("feedback_oid", request.feedback_oid),
    ] {
        validate_control_value(label, value)?;
    }
    Ok(())
}

fn validate_decision_outcome_request(request: &DecisionOutcomeRequest<'_>) -> Result<()> {
    validate_decision_semantics(request.disposition, request.selected_snapshot)?;
    if !valid_artifact_manifest_sha256(request.reviewed_artifact_manifest_sha256) {
        return Err(JournalError::InvalidArgument(
            "reviewed_artifact_manifest_sha256 must be 64 lowercase hexadecimal characters".into(),
        ));
    }
    for (label, value) in [
        ("proposal_head", request.proposal_head),
        ("expected_decision_head", request.expected_decision_head),
        ("new_decision_head", request.new_decision_head),
        ("feedback_oid", request.feedback_oid),
    ] {
        validate_control_value(label, value)?;
    }
    if request.expected_decision_head == request.new_decision_head {
        return Err(JournalError::InvalidArgument(
            "new_decision_head must differ from expected_decision_head".into(),
        ));
    }
    Ok(())
}

fn validate_request_bytes(idempotency_key: &[u8], canonical_request: &[u8]) -> Result<()> {
    validate_idempotency_key(idempotency_key)?;
    if canonical_request.is_empty() || canonical_request.len() > MAX_CANONICAL_REQUEST_BYTES {
        return Err(JournalError::InvalidArgument(format!(
            "canonical_request must contain 1..={MAX_CANONICAL_REQUEST_BYTES} bytes"
        )));
    }
    Ok(())
}

fn validate_idempotency_key(idempotency_key: &[u8]) -> Result<()> {
    if idempotency_key.is_empty() || idempotency_key.len() > MAX_IDEMPOTENCY_KEY_BYTES {
        return Err(JournalError::InvalidArgument(format!(
            "idempotency_key must contain 1..={MAX_IDEMPOTENCY_KEY_BYTES} bytes"
        )));
    }
    Ok(())
}

fn validate_decision_semantics(
    disposition: DecisionDisposition,
    selected_snapshot: SelectedSnapshot,
) -> Result<()> {
    let valid = matches!(
        (disposition, selected_snapshot),
        (
            DecisionDisposition::AdoptedUnchanged,
            SelectedSnapshot::Proposal
        ) | (DecisionDisposition::Rejected, SelectedSnapshot::Base)
            | (DecisionDisposition::Deferred, SelectedSnapshot::Base)
    );
    if valid {
        Ok(())
    } else {
        Err(JournalError::InvalidArgument(
            "selected_snapshot does not match the Decision disposition".into(),
        ))
    }
}

fn validate_stored_decision_commit_intent(intent: &DecisionCommitIntent) -> Result<()> {
    validate_decision_semantics(intent.disposition, intent.selected_snapshot)
        .map_err(|error| JournalError::CorruptData(error.to_string()))?;
    if !valid_artifact_manifest_sha256(&intent.reviewed_artifact_manifest_sha256) {
        return Err(JournalError::CorruptData(
            "Decision intent manifest digest is not lowercase SHA-256".into(),
        ));
    }
    for (label, value) in [
        ("new_decision_head", intent.new_decision_head.as_str()),
        ("feedback_oid", intent.feedback_oid.as_str()),
    ] {
        validate_control_value(label, value)
            .map_err(|error| JournalError::CorruptData(error.to_string()))?;
    }
    if intent.new_decision_head == intent.binding.expected_decision_head {
        return Err(JournalError::CorruptData(
            "Decision intent does not advance the expected head".into(),
        ));
    }
    Ok(())
}

fn validate_stored_decision_outcome(outcome: &DecisionOutcome) -> Result<()> {
    validate_decision_semantics(outcome.disposition, outcome.selected_snapshot)
        .map_err(|error| JournalError::CorruptData(error.to_string()))?;
    if !valid_artifact_manifest_sha256(&outcome.reviewed_artifact_manifest_sha256) {
        return Err(JournalError::CorruptData(
            "Decision outcome manifest digest is not lowercase SHA-256".into(),
        ));
    }
    for (label, value) in [
        ("proposal_head", outcome.proposal_head.as_str()),
        (
            "expected_decision_head",
            outcome.expected_decision_head.as_str(),
        ),
        ("new_decision_head", outcome.new_decision_head.as_str()),
        ("feedback_oid", outcome.feedback_oid.as_str()),
    ] {
        validate_control_value(label, value)
            .map_err(|error| JournalError::CorruptData(error.to_string()))?;
    }
    if outcome.expected_decision_head == outcome.new_decision_head {
        return Err(JournalError::CorruptData(
            "Decision outcome does not advance the expected head".into(),
        ));
    }
    Ok(())
}

fn decision_outcome_matches_intent(
    intent: &DecisionCommitIntent,
    review: &ReviewRecord,
    outcome: &DecisionOutcome,
) -> bool {
    intent.binding() == review.binding()
        && intent.disposition() == outcome.disposition
        && intent.selected_snapshot() == outcome.selected_snapshot
        && intent.reviewed_artifact_manifest_sha256() == outcome.reviewed_artifact_manifest_sha256
        && intent.binding().proposal_head() == outcome.proposal_head
        && intent.expected_decision_head() == outcome.expected_decision_head
        && intent.new_decision_head() == outcome.new_decision_head
        && intent.feedback_oid() == outcome.feedback_oid
}

fn validate_control_value(label: &str, value: &str) -> Result<()> {
    if value.is_empty()
        || value.len() > MAX_CONTROL_VALUE_BYTES
        || value.chars().any(char::is_control)
    {
        return Err(JournalError::InvalidArgument(format!(
            "{label} must contain 1..={MAX_CONTROL_VALUE_BYTES} non-control UTF-8 bytes"
        )));
    }
    Ok(())
}

fn digest(domain: &[u8], parts: &[&[u8]]) -> String {
    let mut hash = Sha256::new();
    hash.update(domain);
    for part in parts {
        hash.update(u64::try_from(part.len()).unwrap_or(u64::MAX).to_be_bytes());
        hash.update(part);
    }
    format!("sha256:{}", hex(&hash.finalize()))
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

fn valid_sha256(value: &str) -> bool {
    value.len() == 71
        && value.starts_with("sha256:")
        && value[7..]
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn valid_artifact_manifest_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn is_constraint(error: &rusqlite::Error) -> bool {
    matches!(
        error,
        rusqlite::Error::SqliteFailure(details, _)
            if details.code == ErrorCode::ConstraintViolation
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::ErrorKind;
    use std::path::PathBuf;
    use std::sync::{Arc, Barrier};
    use std::thread;

    struct TempDirectory(PathBuf);

    impl TempDirectory {
        fn new() -> Self {
            for _ in 0..RANDOM_ID_ATTEMPTS {
                let suffix = ReviewId::generate().unwrap();
                let path = std::env::temp_dir().join(format!(
                    "synapsegit-artifact-journal-test-{}-{}",
                    std::process::id(),
                    suffix.as_str()
                ));
                match fs::create_dir(&path) {
                    Ok(()) => return Self(path),
                    Err(error) if error.kind() == ErrorKind::AlreadyExists => continue,
                    Err(error) => panic!("failed to create test directory: {error}"),
                }
            }
            panic!("failed to allocate a collision-resistant test directory")
        }

        fn database(&self) -> PathBuf {
            self.0.join("review-journal.sqlite3")
        }
    }

    impl Drop for TempDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn binding(suffix: &str) -> ReviewBinding {
        ReviewBinding::new(
            format!("project-{suffix}"),
            format!("proposal/agent/{suffix}"),
            format!("commit:sg-oid-v1:sha256:proposal-{suffix}"),
            format!("decision/main/{suffix}"),
            "commit:sg-oid-v1:sha256:base",
        )
        .unwrap()
    }

    fn intent<'a>(key: &'a [u8], request: &'a [u8]) -> DecisionIntentRequest<'a> {
        DecisionIntentRequest {
            idempotency_key: key,
            canonical_request: request,
            candidate_head: "commit:sg-oid-v1:sha256:candidate",
            feedback_oid: "record:sg-oid-v1:sha256:feedback",
            expected_decision_head: "commit:sg-oid-v1:sha256:base",
        }
    }

    #[test]
    fn random_ids_and_binding_uniqueness_are_enforced() {
        let mut journal = SqliteReviewJournal::open_in_memory().unwrap();
        let first_binding = binding("one");
        let first = journal.create_review(first_binding.clone()).unwrap();
        let second = journal.create_review(binding("two")).unwrap();
        assert_ne!(first.review_id(), second.review_id());
        assert_eq!(first.review_id().as_str().len(), REVIEW_ID_HEX_LEN);
        assert_eq!(format!("{:?}", first.review_id()), "ReviewId(<opaque>)");
        let error = journal.create_review(first_binding).unwrap_err();
        assert_eq!(error.code(), "review_binding_exists");
        let count: i64 = journal
            .connection
            .query_row("SELECT COUNT(*) FROM reviews", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 2);
    }

    #[test]
    fn binding_ref_names_require_nonempty_namespaced_tails() {
        for (proposal, decision) in [
            ("proposal", "decision/main"),
            ("proposal/", "decision/main"),
            ("proposal/main", "decision"),
            ("proposal/main", "decision/"),
        ] {
            let error = ReviewBinding::new(
                "project",
                proposal,
                "commit:sg-oid-v1:sha256:proposal",
                decision,
                "commit:sg-oid-v1:sha256:base",
            )
            .unwrap_err();
            assert_eq!(error.code(), "invalid_argument");
        }
    }

    #[test]
    fn create_or_get_recovers_the_same_locator_and_debug_redacts_bindings() {
        let mut journal = SqliteReviewJournal::open_in_memory().unwrap();
        let exact_binding = binding("response-loss-canary");
        let created = journal.create_or_get_review(exact_binding.clone()).unwrap();
        assert_eq!(created.outcome(), ReviewRegistrationOutcome::Created);
        let replayed = journal.create_or_get_review(exact_binding.clone()).unwrap();
        assert_eq!(replayed.outcome(), ReviewRegistrationOutcome::Replayed);
        assert_eq!(created.review().review_id(), replayed.review().review_id());
        assert_eq!(
            journal
                .get_review_by_binding(&exact_binding)
                .unwrap()
                .unwrap()
                .review_id(),
            created.review().review_id()
        );

        for debug in [
            format!("{exact_binding:?}"),
            format!("{:?}", replayed.review()),
        ] {
            assert!(!debug.contains("response-loss-canary"));
            assert!(!debug.contains("proposal/"));
            assert!(!debug.contains("decision/"));
            assert!(!debug.contains("commit:"));
        }

        let conflicting = ReviewBinding::new(
            exact_binding.project_scope(),
            exact_binding.proposal_ref_name(),
            exact_binding.proposal_head(),
            "decision/different",
            "commit:sg-oid-v1:sha256:different-base",
        )
        .unwrap();
        let error = journal.create_or_get_review(conflicting).unwrap_err();
        assert_eq!(error.code(), "review_binding_conflict");
        let count: i64 = journal
            .connection
            .query_row("SELECT COUNT(*) FROM reviews", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn concurrent_create_or_get_returns_one_durable_locator() {
        const WORKERS: usize = 6;
        let temporary = TempDirectory::new();
        let database = temporary.database();
        let barrier = Arc::new(Barrier::new(WORKERS));
        let handles = (0..WORKERS)
            .map(|_| {
                let database = database.clone();
                let barrier = Arc::clone(&barrier);
                thread::spawn(move || {
                    let mut journal = SqliteReviewJournal::open(database).unwrap();
                    barrier.wait();
                    journal.create_or_get_review(binding("concurrent")).unwrap()
                })
            })
            .collect::<Vec<_>>();
        let results = handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect::<Vec<_>>();

        let locator = results[0].review().review_id().clone();
        assert!(
            results
                .iter()
                .all(|result| result.review().review_id() == &locator)
        );
        assert_eq!(
            results
                .iter()
                .filter(|result| result.outcome() == ReviewRegistrationOutcome::Created)
                .count(),
            1
        );
    }

    #[test]
    fn reopen_preserves_binding_intent_and_state() {
        let temporary = TempDirectory::new();
        let database = temporary.database();
        let review_id = {
            let mut journal = SqliteReviewJournal::open(&database).unwrap();
            let review = journal.create_review(binding("reopen")).unwrap();
            let review_id = review.review_id().clone();
            let registered = journal
                .register_decision_intent(
                    &review_id,
                    intent(b"reopen-key", br#"{"disposition":"adopt"}"#),
                )
                .unwrap();
            assert_eq!(registered.outcome(), IntentRegistrationOutcome::Created);
            journal
                .transition_review_state(
                    &review_id,
                    ReviewState::PendingReview,
                    ReviewState::OutcomeUnknown,
                )
                .unwrap();
            review_id
        };

        let journal = SqliteReviewJournal::open(&database).unwrap();
        let review = journal.get_review(&review_id).unwrap();
        assert_eq!(review.binding(), &binding("reopen"));
        assert_eq!(review.state(), ReviewState::OutcomeUnknown);
        let stored = journal.get_decision_intent(&review_id).unwrap().unwrap();
        assert_eq!(stored.candidate_head(), "commit:sg-oid-v1:sha256:candidate");
        assert!(valid_sha256(stored.idempotency_digest()));
        assert!(valid_sha256(stored.request_fingerprint()));
    }

    #[test]
    fn same_key_and_fingerprint_replay_without_a_second_intent() {
        let mut journal = SqliteReviewJournal::open_in_memory().unwrap();
        let review = journal.create_review(binding("replay")).unwrap();
        let request = br#"{"disposition":"reject"}"#;
        let first = journal
            .register_decision_intent(review.review_id(), intent(b"stable-key", request))
            .unwrap();
        let replay = journal
            .register_decision_intent(review.review_id(), intent(b"stable-key", request))
            .unwrap();
        assert_eq!(first.outcome(), IntentRegistrationOutcome::Created);
        assert_eq!(replay.outcome(), IntentRegistrationOutcome::Replayed);
        assert_eq!(first.intent(), replay.intent());
        let count: i64 = journal
            .connection
            .query_row("SELECT COUNT(*) FROM decision_intents", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn same_key_with_changed_request_or_candidate_is_an_idempotency_conflict() {
        let mut journal = SqliteReviewJournal::open_in_memory().unwrap();
        let review = journal.create_review(binding("conflict")).unwrap();
        journal
            .register_decision_intent(
                review.review_id(),
                intent(b"same-key", br#"{"disposition":"adopt"}"#),
            )
            .unwrap();
        let changed_request = journal
            .register_decision_intent(
                review.review_id(),
                intent(b"same-key", br#"{"disposition":"reject"}"#),
            )
            .unwrap_err();
        assert_eq!(changed_request.code(), "idempotency_conflict");

        let changed_candidate = DecisionIntentRequest {
            candidate_head: "commit:sg-oid-v1:sha256:different",
            ..intent(b"same-key", br#"{"disposition":"adopt"}"#)
        };
        let error = journal
            .register_decision_intent(review.review_id(), changed_candidate)
            .unwrap_err();
        assert_eq!(error.code(), "idempotency_conflict");
    }

    #[test]
    fn privacy_sensitive_inputs_are_hashed_or_absent_from_the_database() {
        let temporary = TempDirectory::new();
        let database = temporary.database();
        let raw_key = b"RAW-IDEMPOTENCY-KEY-CANARY-91D4";
        let canonical_request =
            br#"{"disposition":"defer","rationale":"PRIVATE-RATIONALE-CANARY-7A2E"}"#;
        let request_debug = format!("{:?}", intent(raw_key, canonical_request));
        assert!(!request_debug.contains("RAW-IDEMPOTENCY"));
        assert!(!request_debug.contains("PRIVATE-RATIONALE"));
        {
            let mut journal = SqliteReviewJournal::open(&database).unwrap();
            let review = journal.create_review(binding("privacy")).unwrap();
            let registered = journal
                .register_decision_intent(review.review_id(), intent(raw_key, canonical_request))
                .unwrap();
            let intent_debug = format!("{registered:?}");
            assert!(!intent_debug.contains("candidate"));
            assert!(!intent_debug.contains("feedback"));
            assert!(!intent_debug.contains("sha256:"));
        }

        let bytes = fs::read(&database).unwrap();
        let forbidden = [
            raw_key.as_slice(),
            b"PRIVATE-RATIONALE-CANARY-7A2E".as_slice(),
            b"credential".as_slice(),
            b"permit".as_slice(),
            b"actor_oid".as_slice(),
            b"policy_oid".as_slice(),
            b"grant_oid".as_slice(),
            b"repository_path".as_slice(),
            b"raw_idempotency_key".as_slice(),
            b"rationale".as_slice(),
        ];
        for canary in forbidden {
            assert!(
                !bytes.windows(canary.len()).any(|window| window == canary),
                "database retained forbidden bytes: {}",
                String::from_utf8_lossy(canary)
            );
        }
    }

    #[test]
    fn state_transition_is_compare_and_set_and_all_states_round_trip() {
        let mut journal = SqliteReviewJournal::open_in_memory().unwrap();
        for (index, state) in [
            ReviewState::DecisionCommitted,
            ReviewState::TerminalDenial,
            ReviewState::RetryableFailure,
            ReviewState::OutcomeUnknown,
        ]
        .into_iter()
        .enumerate()
        {
            let review = journal
                .create_review(binding(&format!("state-{index}")))
                .unwrap();
            let changed = journal
                .transition_review_state(review.review_id(), ReviewState::PendingReview, state)
                .unwrap();
            assert_eq!(changed.state(), state);
            let idempotent = journal
                .transition_review_state(review.review_id(), state, state)
                .unwrap();
            assert_eq!(idempotent, changed);
            let error = journal
                .transition_review_state(review.review_id(), ReviewState::PendingReview, state)
                .unwrap_err();
            assert_eq!(error.code(), "review_state_conflict");
        }
    }

    #[test]
    fn terminal_and_unknown_states_cannot_regress_to_pending() {
        let mut journal = SqliteReviewJournal::open_in_memory().unwrap();
        for (index, terminal) in [ReviewState::DecisionCommitted, ReviewState::TerminalDenial]
            .into_iter()
            .enumerate()
        {
            let review = journal
                .create_review(binding(&format!("terminal-{index}")))
                .unwrap();
            journal
                .transition_review_state(review.review_id(), ReviewState::PendingReview, terminal)
                .unwrap();
            let error = journal
                .transition_review_state(review.review_id(), terminal, ReviewState::PendingReview)
                .unwrap_err();
            assert_eq!(error.code(), "review_state_transition_invalid");
            assert_eq!(
                journal.get_review(review.review_id()).unwrap().state(),
                terminal
            );
        }

        let unknown = journal.create_review(binding("unknown-state")).unwrap();
        journal
            .transition_review_state(
                unknown.review_id(),
                ReviewState::PendingReview,
                ReviewState::OutcomeUnknown,
            )
            .unwrap();
        for regressed in [ReviewState::PendingReview, ReviewState::RetryableFailure] {
            let error = journal
                .transition_review_state(
                    unknown.review_id(),
                    ReviewState::OutcomeUnknown,
                    regressed,
                )
                .unwrap_err();
            assert_eq!(error.code(), "review_state_transition_invalid");
        }
        let reconciled = journal
            .transition_review_state(
                unknown.review_id(),
                ReviewState::OutcomeUnknown,
                ReviewState::DecisionCommitted,
            )
            .unwrap();
        assert_eq!(reconciled.state(), ReviewState::DecisionCommitted);

        let retryable = journal.create_review(binding("retryable-state")).unwrap();
        journal
            .transition_review_state(
                retryable.review_id(),
                ReviewState::PendingReview,
                ReviewState::RetryableFailure,
            )
            .unwrap();
        let error = journal
            .transition_review_state(
                retryable.review_id(),
                ReviewState::RetryableFailure,
                ReviewState::PendingReview,
            )
            .unwrap_err();
        assert_eq!(error.code(), "review_state_transition_invalid");
    }

    #[test]
    fn a_new_intent_requires_a_decisionable_state_and_the_bound_base() {
        let mut journal = SqliteReviewJournal::open_in_memory().unwrap();
        let terminal = journal.create_review(binding("terminal-intent")).unwrap();
        journal
            .transition_review_state(
                terminal.review_id(),
                ReviewState::PendingReview,
                ReviewState::TerminalDenial,
            )
            .unwrap();
        let error = journal
            .register_decision_intent(
                terminal.review_id(),
                intent(b"terminal-key", br#"{"disposition":"reject"}"#),
            )
            .unwrap_err();
        assert_eq!(error.code(), "review_state_conflict");

        let pending = journal.create_review(binding("bound-intent")).unwrap();
        let wrong_base = DecisionIntentRequest {
            expected_decision_head: "commit:sg-oid-v1:sha256:wrong-base",
            ..intent(b"bound-key", br#"{"disposition":"adopt"}"#)
        };
        let error = journal
            .register_decision_intent(pending.review_id(), wrong_base)
            .unwrap_err();
        assert_eq!(error.code(), "review_binding_conflict");
        assert!(
            journal
                .get_decision_intent(pending.review_id())
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn an_existing_exact_intent_can_be_recovered_after_terminal_commit() {
        let mut journal = SqliteReviewJournal::open_in_memory().unwrap();
        let review = journal.create_review(binding("terminal-replay")).unwrap();
        let request = intent(b"replay-key", br#"{"disposition":"adopt"}"#);
        journal
            .register_decision_intent(review.review_id(), request)
            .unwrap();
        journal
            .transition_review_state(
                review.review_id(),
                ReviewState::PendingReview,
                ReviewState::DecisionCommitted,
            )
            .unwrap();

        let replay = journal
            .register_decision_intent(review.review_id(), request)
            .unwrap();
        assert_eq!(replay.outcome(), IntentRegistrationOutcome::Replayed);
    }
}
