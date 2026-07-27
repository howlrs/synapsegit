use sha2::{Digest, Sha256};

use crate::error::{JournalError, Result};
use crate::types::{
    DecisionCommitIntent, DecisionCommitIntentRequest, DecisionDisposition, DecisionIntentRequest,
    DecisionOutcome, DecisionOutcomeRequest, ProposalIntentRequest, ReviewRecord, SelectedSnapshot,
};

const MAX_CONTROL_VALUE_BYTES: usize = 2_000;
const MAX_IDEMPOTENCY_KEY_BYTES: usize = 4_096;
const MAX_CANONICAL_REQUEST_BYTES: usize = 1_048_576;

pub(crate) fn validate_intent_request(request: &DecisionIntentRequest<'_>) -> Result<()> {
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

pub(crate) fn validate_proposal_intent_request(request: &ProposalIntentRequest<'_>) -> Result<()> {
    validate_request_bytes(request.idempotency_key, request.canonical_request)?;
    if !valid_artifact_manifest_sha256(request.artifact_manifest_sha256) {
        return Err(JournalError::InvalidArgument(
            "artifact_manifest_sha256 must be 64 lowercase hexadecimal characters".into(),
        ));
    }
    request.binding.validate()
}

pub(crate) fn validate_decision_commit_intent_request(
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

pub(crate) fn validate_decision_outcome_request(
    request: &DecisionOutcomeRequest<'_>,
) -> Result<()> {
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

pub(crate) fn validate_request_bytes(
    idempotency_key: &[u8],
    canonical_request: &[u8],
) -> Result<()> {
    validate_idempotency_key(idempotency_key)?;
    if canonical_request.is_empty() || canonical_request.len() > MAX_CANONICAL_REQUEST_BYTES {
        return Err(JournalError::InvalidArgument(format!(
            "canonical_request must contain 1..={MAX_CANONICAL_REQUEST_BYTES} bytes"
        )));
    }
    Ok(())
}

pub(crate) fn validate_idempotency_key(idempotency_key: &[u8]) -> Result<()> {
    if idempotency_key.is_empty() || idempotency_key.len() > MAX_IDEMPOTENCY_KEY_BYTES {
        return Err(JournalError::InvalidArgument(format!(
            "idempotency_key must contain 1..={MAX_IDEMPOTENCY_KEY_BYTES} bytes"
        )));
    }
    Ok(())
}

pub(crate) fn validate_decision_semantics(
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

pub(crate) fn validate_stored_decision_commit_intent(intent: &DecisionCommitIntent) -> Result<()> {
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

pub(crate) fn validate_stored_decision_outcome(outcome: &DecisionOutcome) -> Result<()> {
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

pub(crate) fn decision_outcome_matches_intent(
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

pub(crate) fn validate_control_value(label: &str, value: &str) -> Result<()> {
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

pub(crate) fn digest(domain: &[u8], parts: &[&[u8]]) -> String {
    let mut hash = Sha256::new();
    hash.update(domain);
    for part in parts {
        hash.update(u64::try_from(part.len()).unwrap_or(u64::MAX).to_be_bytes());
        hash.update(part);
    }
    format!("sha256:{}", hex(&hash.finalize()))
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(char::from(DIGITS[usize::from(byte >> 4)]));
        output.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
    }
    output
}

pub(crate) fn valid_sha256(value: &str) -> bool {
    value.len() == 71
        && value.starts_with("sha256:")
        && value[7..]
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

pub(crate) fn valid_artifact_manifest_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

pub(crate) fn is_constraint(error: &rusqlite::Error) -> bool {
    matches!(
        error,
        rusqlite::Error::SqliteFailure(details, _)
            if details.code == rusqlite::ErrorCode::ConstraintViolation
    )
}
