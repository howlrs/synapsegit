//! Durable, transport-neutral review journaling for artifact applications.
//!
//! This crate stores locators, server-owned Ref bindings, state, and digests. It
//! deliberately does not authenticate, authorize, reconstruct permits, inspect
//! a SynapseGit repository, or claim that Core admitted an operation.

#![forbid(unsafe_code)]

mod decode;
mod error;
mod journal;
mod schema;
mod types;
mod validate;

#[cfg(test)]
mod tests;

pub use error::{JournalError, Result};
pub use journal::SqliteReviewJournal;
pub use types::{
    CommittedDecisionOutcome, DecisionCommitIntent, DecisionCommitIntentRequest,
    DecisionDisposition, DecisionIntent, DecisionIntentRequest, DecisionOutcome,
    DecisionOutcomeRegistrationOutcome, DecisionOutcomeRequest, IntentRegistrationOutcome,
    ProposalIntent, ProposalIntentId, ProposalIntentRegistrationOutcome, ProposalIntentRequest,
    RegisteredDecisionCommitIntent, RegisteredDecisionIntent, RegisteredProposalIntent,
    RegisteredReview, ReviewBinding, ReviewId, ReviewReconciliation, ReviewRecord,
    ReviewRegistrationOutcome, ReviewState, SelectedSnapshot,
};
