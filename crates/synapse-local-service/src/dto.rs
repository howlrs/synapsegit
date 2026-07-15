use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HealthResponse {
    pub status: String,
    pub api_version: String,
    pub server_instance: String,
}

impl HealthResponse {
    pub fn new(server_instance: impl Into<String>) -> Self {
        Self {
            status: "ok".into(),
            api_version: "v1".into(),
            server_instance: server_instance.into(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectCapabilities {
    pub read: bool,
    pub creator_import: bool,
    pub human_decision: bool,
    pub fsck: bool,
    pub archive_export: bool,
    pub archive_restore: bool,
}

impl ProjectCapabilities {
    pub const fn creator_workflow() -> Self {
        Self {
            read: true,
            creator_import: true,
            human_decision: true,
            fsck: false,
            archive_export: false,
            archive_restore: false,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectState {
    Ready,
    EmptyRestoreTarget,
    Unavailable,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectSummary {
    pub project_key: String,
    pub display_label: String,
    pub state: ProjectState,
    pub capabilities: ProjectCapabilities,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectList {
    pub projects: Vec<ProjectSummary>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SnapshotContext {
    pub watermark: String,
    pub ref_count: usize,
    pub projection_source_fingerprint: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreatorSessionCounts {
    pub complete: usize,
    pub pending_review: usize,
    pub incomplete: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectionState {
    NotBuilt,
    Current,
    RebuildFailed,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FsckResult {
    pub clean: bool,
    pub objects_seen: usize,
    pub objects_verified: usize,
    pub closure_count: usize,
    pub issue_count: usize,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectStatus {
    pub project: ProjectSummary,
    pub snapshot: SnapshotContext,
    pub creator_session_counts: CreatorSessionCounts,
    pub projection_state: ProjectionState,
    pub last_fsck: Option<FsckResult>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RefRecord {
    pub name: String,
    pub head: String,
    pub updated_event_id: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RefList {
    pub snapshot: SnapshotContext,
    pub refs: Vec<RefRecord>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReflogEntry {
    pub event_id: String,
    pub ref_name: String,
    pub old_head: Option<String>,
    pub new_head: String,
    pub occurred_at_unix_nanos: String,
    pub actor: Option<String>,
    pub message: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReflogPage {
    pub snapshot: SnapshotContext,
    pub entries: Vec<ReflogEntry>,
    pub next_after_event_id: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ReflogQuery {
    pub ref_name: Option<String>,
    pub after_event_id: Option<String>,
    pub limit: usize,
}

impl Default for ReflogQuery {
    fn default() -> Self {
        Self {
            ref_name: None,
            after_event_id: None,
            limit: 100,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CreatorSessionState {
    Complete,
    PendingReview,
    Incomplete,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreatorSessionSummary {
    pub session: String,
    pub state: CreatorSessionState,
    pub proposal_ref: Option<String>,
    pub proposal_head: Option<String>,
    pub decision_ref: Option<String>,
    pub decision_head: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreatorSessionList {
    pub snapshot: SnapshotContext,
    pub sessions: Vec<CreatorSessionSummary>,
}

/// Trusted, already-staged inputs for one creator proposal.
///
/// Repository selection remains a separate catalog key on [`crate::LocalService`].
/// These paths are integration-owned staging paths and are deliberately not
/// serializable transport DTO fields.
#[derive(Debug)]
pub struct BeginCreatorSessionRequest {
    pub session: String,
    pub subject_label: String,
    pub creator_name: String,
    pub original_image: PathBuf,
    pub current_image: PathBuf,
    pub ai_output: PathBuf,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CreatorDecision {
    Adopt,
    Reject,
    Defer,
}

/// Untrusted Human review fields accepted by the transport-neutral service.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreatorDecisionRequest {
    pub review_id: String,
    pub disposition: CreatorDecision,
    pub rationale: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ComparisonEvidence {
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

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TimelineEntry {
    pub oid: String,
    pub stage: String,
    pub kind: String,
    pub entity_id: String,
    pub ordering_time: String,
    pub time_basis: String,
    pub reachable_from: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreatorReport {
    pub snapshot: SnapshotContext,
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
    pub disposition: String,
    pub selected_ai_output: bool,
    pub proposal_attributed_to_agent: String,
    pub ai_output_source: String,
    pub reviewed_by_human: String,
    pub rationale: Option<String>,
    pub original_blob_oid: String,
    pub current_blob_oid: String,
    pub ai_output_blob_oid: String,
    pub comparison: Option<ComparisonEvidence>,
    pub timeline: Vec<TimelineEntry>,
    pub fsck_objects: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum CompleteState {
    #[serde(rename = "complete")]
    Complete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum CommittedState {
    #[serde(rename = "committed")]
    Committed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum PendingReviewState {
    #[serde(rename = "pending_review")]
    PendingReview,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum IncompleteState {
    #[serde(rename = "incomplete")]
    Incomplete,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompleteCreatorSession {
    pub state: CompleteState,
    pub report: CreatorReport,
}

/// Durable publication identifiers returned when the decision committed but
/// a concurrent repository change prevented rebuilding the full report.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreatorDecisionReceipt {
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
    pub byte_identity_outcome: String,
    pub comparison_status: String,
    pub comparison_comparability: String,
    pub comparison_reason_codes: Vec<String>,
    pub ai_activity_oid: String,
    pub decision_feedback_oid: String,
    pub disposition: CreatorDecision,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommittedCreatorSession {
    pub state: CommittedState,
    pub receipt: CreatorDecisionReceipt,
    pub report_available: bool,
    pub inspection_required: bool,
}

/// A decision always returns either the complete rebuilt report or, if the
/// publication committed but report reconstruction failed, its exact durable
/// receipt. The untagged representation preserves the existing complete JSON.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum CreatorDecisionResponse {
    Complete(Box<CompleteCreatorSession>),
    Committed(Box<CommittedCreatorSession>),
}

impl CreatorDecisionResponse {
    pub fn into_complete(self) -> Option<CompleteCreatorSession> {
        match self {
            Self::Complete(complete) => Some(*complete),
            Self::Committed(_) => None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PendingCreatorSession {
    pub state: PendingReviewState,
    pub snapshot: SnapshotContext,
    pub server_instance: String,
    pub review_id: String,
    pub session: String,
    pub project_id: String,
    pub subject_id: String,
    pub proposal_ref: String,
    pub proposal_head: String,
    pub original_blob_oid: String,
    pub current_blob_oid: String,
    pub ai_output_blob_oid: String,
    pub ai_output_source: String,
    pub comparison: ComparisonEvidence,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IncompleteCreatorSession {
    pub state: IncompleteState,
    pub snapshot: SnapshotContext,
    pub session: String,
    pub recovery_supported: bool,
    pub diagnostic: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum CreatorSessionDetail {
    Complete(Box<CompleteCreatorSession>),
    PendingReview(Box<PendingCreatorSession>),
    Incomplete(Box<IncompleteCreatorSession>),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ImageRole {
    Original,
    Current,
    AiOutput,
}

impl ImageRole {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "original" => Some(Self::Original),
            "current" => Some(Self::Current),
            "ai-output" => Some(Self::AiOutput),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ImageMediaType {
    Png,
    Jpeg,
    Gif,
    WebP,
    OctetStream,
}

impl ImageMediaType {
    pub const fn content_type(self) -> &'static str {
        match self {
            Self::Png => "image/png",
            Self::Jpeg => "image/jpeg",
            Self::Gif => "image/gif",
            Self::WebP => "image/webp",
            Self::OctetStream => "application/octet-stream",
        }
    }

    pub const fn is_attachment(self) -> bool {
        matches!(self, Self::OctetStream)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CreatorImage {
    pub blob_oid: String,
    pub media_type: ImageMediaType,
    pub disposition: ImageDisposition,
    pub bytes: Vec<u8>,
}

impl CreatorImage {
    pub const fn content_type(&self) -> &'static str {
        self.media_type.content_type()
    }

    pub const fn is_attachment(&self) -> bool {
        matches!(self.disposition, ImageDisposition::Attachment)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ImageDisposition {
    Inline,
    Attachment,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Problem {
    pub r#type: String,
    pub title: String,
    pub status: u16,
    pub code: String,
    pub detail: String,
    pub request_id: String,
    pub retryable: bool,
}
