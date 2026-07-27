use rusqlite::{Connection, OptionalExtension, params};

use crate::error::{JournalError, Result};
use crate::types::{
    DecisionCommitIntent, DecisionDisposition, DecisionIntent, DecisionOutcome, ProposalIntent,
    ProposalIntentId, ReviewBinding, ReviewId, ReviewRecord, ReviewState, SelectedSnapshot,
};
use crate::validate::{
    valid_artifact_manifest_sha256, valid_sha256, validate_control_value,
    validate_stored_decision_commit_intent, validate_stored_decision_outcome,
};

type StoredReview = (String, String, String, String, String, String, String);

pub(crate) fn review_tuple(row: &rusqlite::Row<'_>) -> rusqlite::Result<StoredReview> {
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

pub(crate) fn decode_review(stored: StoredReview) -> Result<ReviewRecord> {
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

pub(crate) fn load_review(
    connection: &Connection,
    review_id: &ReviewId,
) -> Result<Option<ReviewRecord>> {
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

pub(crate) fn load_review_by_proposal(
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

pub(crate) fn proposal_intent_tuple(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<StoredProposalIntent> {
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

pub(crate) fn decode_proposal_intent(stored: StoredProposalIntent) -> Result<ProposalIntent> {
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

pub(crate) fn load_proposal_intent(
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

pub(crate) fn load_proposal_intent_by_idempotency(
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

pub(crate) fn load_proposal_intent_by_proposal(
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

pub(crate) fn load_proposal_intent_by_review(
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

pub(crate) fn attach_review_to_proposal_intent(
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

pub(crate) fn validate_proposal_intent_review_link(
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

pub(crate) fn review_exists(connection: &Connection, review_id: &ReviewId) -> Result<bool> {
    Ok(connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM reviews WHERE review_id = ?1)",
        [review_id.as_str()],
        |row| row.get(0),
    )?)
}

pub(crate) fn state_transition_allowed(from: ReviewState, to: ReviewState) -> bool {
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

pub(crate) fn load_intent(
    connection: &Connection,
    review_id: &ReviewId,
) -> Result<Option<DecisionIntent>> {
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

pub(crate) fn load_decision_commit_intent(
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

pub(crate) fn load_decision_outcome(
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
