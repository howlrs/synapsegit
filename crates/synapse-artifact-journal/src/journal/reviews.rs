use rusqlite::{TransactionBehavior, params};

use crate::decode::{
    decode_review, load_decision_commit_intent, load_decision_outcome,
    load_proposal_intent_by_review, load_review, load_review_by_proposal, review_tuple,
    state_transition_allowed,
};
use crate::error::{JournalError, Result};
use crate::types::{
    RANDOM_ID_ATTEMPTS, RegisteredReview, ReviewBinding, ReviewId, ReviewRecord,
    ReviewRegistrationOutcome, ReviewState,
};
use crate::validate::is_constraint;

use super::SqliteReviewJournal;

impl SqliteReviewJournal {
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
}
