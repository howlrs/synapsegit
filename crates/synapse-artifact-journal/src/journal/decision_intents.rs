use rusqlite::{TransactionBehavior, params};

use crate::decode::{
    load_decision_commit_intent, load_intent, load_proposal_intent_by_review, load_review,
    review_exists,
};
use crate::error::{JournalError, Result};
use crate::types::{
    DecisionCommitIntent, DecisionCommitIntentRequest, DecisionDisposition, DecisionIntent,
    DecisionIntentRequest, IntentRegistrationOutcome, RegisteredDecisionCommitIntent,
    RegisteredDecisionIntent, ReviewId, ReviewState,
};
use crate::validate::{digest, validate_decision_commit_intent_request, validate_intent_request};

use super::SqliteReviewJournal;

const LEGACY_IDEMPOTENCY_DOMAIN: &[u8] = b"synapsegit-artifact-journal-idempotency-v1\0";
const REQUEST_DOMAIN: &[u8] = b"synapsegit-artifact-journal-request-v1\0";
const DECISION_COMMIT_IDEMPOTENCY_DOMAIN: &[u8] =
    b"synapsegit-artifact-journal-decision-commit-idempotency-v2\0";
const DECISION_COMMIT_REQUEST_DOMAIN: &[u8] =
    b"synapsegit-artifact-journal-decision-commit-request-v2\0";

impl SqliteReviewJournal {
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
    /// The full [`ReviewBinding`](crate::ReviewBinding) is copied from the journal rather than caller
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

    pub fn get_decision_intent(&self, review_id: &ReviewId) -> Result<Option<DecisionIntent>> {
        if !review_exists(&self.connection, review_id)? {
            return Err(JournalError::ReviewNotFound);
        }
        load_intent(&self.connection, review_id)
    }
}
