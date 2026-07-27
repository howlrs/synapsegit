use std::error::Error;
use std::fmt;

use crate::types::ReviewState;

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
