use rusqlite::{TransactionBehavior, params};

use crate::decode::{
    load_decision_commit_intent, load_decision_outcome, load_intent,
    load_proposal_intent_by_review, load_review, validate_proposal_intent_review_link,
};
use crate::error::{JournalError, Result};
use crate::types::{
    CommittedDecisionOutcome, DecisionCommitIntent, DecisionOutcome,
    DecisionOutcomeRegistrationOutcome, DecisionOutcomeRequest, ReviewId, ReviewReconciliation,
    ReviewRecord, ReviewState,
};
use crate::validate::{decision_outcome_matches_intent, validate_decision_outcome_request};

use super::SqliteReviewJournal;

impl SqliteReviewJournal {
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
}
