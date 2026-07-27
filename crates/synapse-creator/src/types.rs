use crate::{AnalysisComparability, AnalysisStatus, ByteIdentityOutcome, CreatorError, Result};
use std::path::PathBuf;

/// Human outcomes supported by the narrow Stage 0 decision route.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CreatorDisposition {
    Adopt,
    Reject,
    Defer,
}

impl CreatorDisposition {
    pub fn parse(value: &str) -> Result<Self> {
        match value {
            "adopt" => Ok(Self::Adopt),
            "reject" => Ok(Self::Reject),
            "defer" => Ok(Self::Defer),
            _ => Err(CreatorError::InvalidArgument(
                "decision must be one of adopt, reject, or defer".into(),
            )),
        }
    }

    pub const fn as_cli_str(self) -> &'static str {
        match self {
            Self::Adopt => "adopt",
            Self::Reject => "reject",
            Self::Defer => "defer",
        }
    }

    pub const fn as_protocol_str(self) -> &'static str {
        match self {
            Self::Adopt => "adopted_unchanged",
            Self::Reject => "rejected",
            Self::Defer => "deferred",
        }
    }

    pub(crate) fn from_protocol(value: &str) -> Result<Self> {
        match value {
            "adopted_unchanged" => Ok(Self::Adopt),
            "rejected" => Ok(Self::Reject),
            "deferred" => Ok(Self::Defer),
            _ => Err(CreatorError::ReportInvalid(format!(
                "unsupported creator disposition {value:?}"
            ))),
        }
    }

    pub(crate) const fn reason_code(self) -> &'static str {
        "unspecified"
    }

    pub(crate) const fn default_rationale(self) -> &'static str {
        match self {
            Self::Adopt => "The creator adopted the AI proposal unchanged.",
            Self::Reject => "The creator rejected the AI proposal.",
            Self::Defer => "The creator deferred the AI proposal for later review.",
        }
    }
}

/// Inputs for one new creator session.
#[derive(Clone, Debug)]
pub struct CreatorRunOptions {
    pub repository: PathBuf,
    pub session: String,
    pub original_image: PathBuf,
    pub current_image: PathBuf,
    pub ai_output: PathBuf,
    pub subject_label: String,
    pub creator_name: String,
    pub disposition: CreatorDisposition,
    pub rationale: Option<String>,
}

/// Inputs needed to publish a creator proposal for later Human review.
///
/// The file paths belong to the trusted local integration. Browser and other
/// request boundaries must stage uploaded bytes and must never accept a
/// repository path from an untrusted caller.
#[derive(Clone, Debug)]
pub struct CreatorBeginOptions {
    pub repository: PathBuf,
    pub session: String,
    pub original_image: PathBuf,
    pub current_image: PathBuf,
    pub ai_output: PathBuf,
    pub subject_label: String,
    pub creator_name: String,
}

/// Human input accepted after the exact proposal has been admitted.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CreatorDecisionOptions {
    pub disposition: CreatorDisposition,
    pub rationale: Option<String>,
}

/// Stable, non-authoritative identifiers for a proposal awaiting review.
///
/// This receipt is safe to render, but it is not sufficient to publish a
/// Human decision. Publication also requires the opaque same-process pending
/// value returned by [`begin_creator_session`](crate::begin_creator_session).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CreatorPendingReceipt {
    pub session: String,
    pub project_id: String,
    pub subject_id: String,
    pub creator_id: String,
    pub agent_id: String,
    pub decision_ref: String,
    pub proposal_ref: String,
    pub base_head: String,
    pub proposal_head: String,
    pub original_blob_oid: String,
    pub current_blob_oid: String,
    pub ai_output_blob_oid: String,
    pub capture_profile_oid: String,
    pub original_observation_oid: String,
    pub current_observation_oid: String,
    pub comparison: CreatorComparisonReport,
    pub ai_activity_oid: String,
}

/// Observable lifecycle of the opaque Human-decision capability.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CreatorPendingDecisionState {
    Ready,
    Deciding,
    Consumed,
    OutcomeUnknown,
}

/// Stable identifiers produced by a completed creator session.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CreatorRunReceipt {
    pub session: String,
    pub project_id: String,
    pub subject_id: String,
    pub creator_id: String,
    pub agent_id: String,
    pub decision_ref: String,
    pub proposal_ref: String,
    pub base_head: String,
    pub proposal_head: String,
    pub decision_head: String,
    pub original_blob_oid: String,
    pub current_blob_oid: String,
    pub ai_output_blob_oid: String,
    pub capture_profile_oid: String,
    pub original_observation_oid: String,
    pub current_observation_oid: String,
    pub comparison_tool_id: String,
    pub comparison_tool_actor_oid: String,
    pub comparison_analysis_oid: String,
    pub comparison_implementation_oid: String,
    pub comparison_configuration_oid: String,
    pub byte_identity_outcome: ByteIdentityOutcome,
    pub comparison_status: AnalysisStatus,
    pub comparison_comparability: AnalysisComparability,
    pub comparison_reason_codes: Vec<String>,
    pub ai_activity_oid: String,
    pub decision_feedback_oid: String,
    pub disposition: CreatorDisposition,
}

/// Conservative byte-identity evidence rebuilt from the current creator Refs.
/// This is not a visual or physical-change judgment.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CreatorComparisonReport {
    pub analysis_oid: String,
    pub tool_id: String,
    pub tool_actor_oid: String,
    pub adapter_id: String,
    pub adapter_version: String,
    pub implementation_oid: String,
    pub configuration_oid: String,
    pub status: String,
    pub comparability: String,
    pub outcome: String,
    pub reason_codes: Vec<String>,
    pub warnings: Vec<String>,
    pub base_observation_oid: String,
    pub target_observation_oid: String,
    pub base_media_oid: String,
    pub target_media_oid: String,
    pub replay_ready: bool,
    pub reachable_from: Vec<String>,
}

/// One report timeline row rebuilt from current authoritative Refs and CAS.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CreatorTimelineEntry {
    pub oid: String,
    pub stage: &'static str,
    pub kind: &'static str,
    pub entity_id: String,
    pub ordering_time: String,
    pub time_basis: &'static str,
    pub reachable_from: Vec<String>,
}

/// Creator-readable process report. The ProjectionStore used to build it is
/// disposable and is never an authorization or recovery source.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CreatorReport {
    pub session: String,
    pub project_id: String,
    pub subject_id: String,
    pub creator_id: String,
    pub agent_id: String,
    pub decision_ref: String,
    pub proposal_ref: String,
    pub decision_head: String,
    pub proposal_head: String,
    pub base_head: String,
    pub base_snapshot: String,
    pub proposal_snapshot: String,
    pub decision_snapshot: String,
    pub disposition: CreatorDisposition,
    pub selected_ai_output: bool,
    pub rationale: Option<String>,
    pub original_blob_oid: String,
    pub current_blob_oid: String,
    pub ai_output_blob_oid: String,
    /// `None` when the reachable base Tree has no byte-identity evidence
    /// entries. This preserves legacy-shaped sessions without inferring an
    /// outcome or proving when they were created.
    pub comparison: Option<CreatorComparisonReport>,
    pub timeline: Vec<CreatorTimelineEntry>,
    pub fsck_objects: usize,
}

/// A creator report and the exact Projection fingerprint built from one
/// caller-supplied Ref snapshot.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CreatorSnapshotReport {
    pub report: CreatorReport,
    pub projection_source_fingerprint: String,
}

/// Lightweight creator-session state derived from exact creator Ref
/// namespaces. A Complete summary is still fully revalidated when its detail
/// report is requested.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CreatorSessionState {
    Complete,
    Incomplete,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CreatorSessionSummary {
    pub session: String,
    pub state: CreatorSessionState,
    pub proposal_ref: Option<String>,
    pub proposal_head: Option<String>,
    pub decision_ref: Option<String>,
    pub decision_head: Option<String>,
}
