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

const SCHEMA_VERSION: i64 = 1;
const REVIEW_ID_BYTES: usize = 32;
const REVIEW_ID_HEX_LEN: usize = REVIEW_ID_BYTES * 2;
const MAX_CONTROL_VALUE_BYTES: usize = 2_000;
const MAX_IDEMPOTENCY_KEY_BYTES: usize = 4_096;
const MAX_CANONICAL_REQUEST_BYTES: usize = 1_048_576;
const RANDOM_ID_ATTEMPTS: usize = 8;
const IDEMPOTENCY_DOMAIN: &[u8] = b"synapsegit-artifact-journal-idempotency-v1\0";
const REQUEST_DOMAIN: &[u8] = b"synapsegit-artifact-journal-request-v1\0";

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

#[derive(Debug)]
pub enum JournalError {
    InvalidArgument(String),
    ReviewNotFound,
    ReviewBindingExists,
    ReviewBindingConflict,
    DecisionIntentExists,
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
            Self::DecisionIntentExists => "decision_intent_exists",
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
            Self::DecisionIntentExists => {
                formatter.write_str("a different Decision intent already exists")
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
        if version == 0 {
            transaction.execute_batch(
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
                    ON decision_intents(review_id, idempotency_digest);
                PRAGMA user_version = 1;",
            )?;
        } else if version != SCHEMA_VERSION {
            return Err(JournalError::CorruptData(format!(
                "unsupported schema version {version}"
            )));
        }
        transaction.commit()?;
        Ok(Self { connection })
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
    /// `pending_review` or be relabeled as a retryable failure.
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
        let idempotency_digest = digest(IDEMPOTENCY_DOMAIN, &[request.idempotency_key]);
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

fn validate_intent_request(request: &DecisionIntentRequest<'_>) -> Result<()> {
    if request.idempotency_key.is_empty()
        || request.idempotency_key.len() > MAX_IDEMPOTENCY_KEY_BYTES
    {
        return Err(JournalError::InvalidArgument(format!(
            "idempotency_key must contain 1..={MAX_IDEMPOTENCY_KEY_BYTES} bytes"
        )));
    }
    if request.canonical_request.is_empty()
        || request.canonical_request.len() > MAX_CANONICAL_REQUEST_BYTES
    {
        return Err(JournalError::InvalidArgument(format!(
            "canonical_request must contain 1..={MAX_CANONICAL_REQUEST_BYTES} bytes"
        )));
    }
    for (label, value) in [
        ("candidate_head", request.candidate_head),
        ("feedback_oid", request.feedback_oid),
        ("expected_decision_head", request.expected_decision_head),
    ] {
        validate_control_value(label, value)?;
    }
    Ok(())
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
