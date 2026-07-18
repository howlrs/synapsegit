use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const PROJECTION_SCHEMA: &str = "org.synapsegit.public-projection";
pub const PROJECTION_SCHEMA_VERSION: u32 = 1;
pub const BUNDLE_SCHEMA: &str = "org.synapsegit.publication-bundle";
pub const BUNDLE_SCHEMA_VERSION: u32 = 1;
pub const RENDERER_PROFILE: &str = "org.synapsegit.publication-renderer";
pub const RENDERER_PROFILE_VERSION: u32 = 1;

#[non_exhaustive]
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OutputTarget {
    Synapse,
    Github,
}

impl OutputTarget {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Synapse => "synapse",
            Self::Github => "github",
        }
    }
}

#[non_exhaustive]
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PublicationVisibility {
    PrivateReview,
    Public,
}

impl PublicationVisibility {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::PrivateReview => "private_review",
            Self::Public => "public",
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PresentationInput {
    pub title: Option<String>,
    pub summary: Option<String>,
    pub creator_display_name: Option<String>,
    pub proposal_agent_display_name: Option<String>,
    #[serde(default)]
    pub sessions: BTreeMap<String, SessionPresentationInput>,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SessionPresentationInput {
    pub title: Option<String>,
    pub public_decision_note: Option<String>,
    pub original_caption: Option<String>,
    pub current_caption: Option<String>,
    pub proposal_caption: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PublicProjection {
    pub schema: SchemaIdentity,
    pub generator: GeneratorIdentity,
    pub publication: PublicationPolicySummary,
    pub source: SourceSummary,
    pub presentation: ProjectionPresentation,
    pub completeness: CompletenessSummary,
    pub sessions: Vec<PublicSession>,
    pub incomplete_sessions: Vec<IncompleteSession>,
    pub limitations: Vec<Limitation>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SchemaIdentity {
    pub name: String,
    pub version: u32,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct GeneratorIdentity {
    pub name: String,
    pub version: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PublicationPolicySummary {
    pub visibility: PublicationVisibility,
    pub state: String,
    pub network_operations: u32,
    pub raw_assets_included: bool,
    pub source_private_rationale_included: bool,
    pub internal_actor_ids_included: bool,
    pub training_use_policy: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SourceSummary {
    pub repository_path_included: bool,
    pub ref_snapshot_sha256: String,
    pub ref_snapshot_digest_origin: ValueOrigin,
    pub projection_source_fingerprint: Option<String>,
    pub fingerprint_origin: Option<ValueOrigin>,
    pub verification_scope: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProjectionPresentation {
    pub title: PresentedText,
    pub summary: PresentedText,
    pub creator_display_name: Option<PresentedText>,
    pub proposal_agent_display_name: Option<PresentedText>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PresentedText {
    pub value: String,
    pub origin: ValueOrigin,
}

#[non_exhaustive]
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ValueOrigin {
    VerifiedFromSynapse,
    ObservedFromSynapse,
    DerivedSummary,
    AuthorSupplied,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CompletenessSummary {
    pub origin: ValueOrigin,
    pub requested_session: Option<String>,
    pub discovered_sessions: usize,
    pub complete_sessions_exported: usize,
    pub incomplete_sessions_retained: usize,
    pub redactions: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PublicSession {
    pub session: String,
    pub title: PresentedText,
    pub state: String,
    pub source_fact_origin: ValueOrigin,
    pub history: Vec<Artifact>,
    pub proposal: ProposalSummary,
    pub human_decision: HumanDecisionSummary,
    pub comparison: Option<ComparisonSummary>,
    pub timeline: Vec<TimelineEntry>,
    pub provenance: ProvenanceSummary,
    pub value: ValueSummary,
    pub limitations: Vec<Limitation>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Artifact {
    pub role: ArtifactRole,
    pub oid: String,
    pub oid_origin: ValueOrigin,
    pub caption: PresentedText,
    pub integrity_assurance: String,
    pub public_rendering: ArtifactRendering,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactRole {
    Original,
    Current,
    AiProposal,
}

impl ArtifactRole {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Original => "Original",
            Self::Current => "Current",
            Self::AiProposal => "AI-attributed proposal",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ArtifactRendering {
    pub state: String,
    pub path: Option<String>,
    pub media_type: Option<String>,
    pub reason: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProposalSummary {
    pub attribution_role: String,
    pub attribution_origin: ValueOrigin,
    pub attribution_scope: String,
    pub attribution_scope_origin: ValueOrigin,
    pub selected_by_human: bool,
    pub retained_when_unselected: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct HumanDecisionSummary {
    pub reviewer_role: String,
    pub disposition: String,
    pub disposition_origin: ValueOrigin,
    pub selected_artifact: ArtifactRole,
    pub public_decision_note: Option<PresentedText>,
    pub source_rationale: RedactedField,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RedactedField {
    pub state: String,
    pub reason: String,
    pub source_policy: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ComparisonSummary {
    pub origin: ValueOrigin,
    pub analysis_kind: String,
    pub status: String,
    pub comparability: String,
    pub outcome: String,
    pub reason_codes: Vec<String>,
    pub warnings: Vec<String>,
    pub replay_ready: bool,
    pub interpretation_limit: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TimelineEntry {
    pub origin: ValueOrigin,
    pub stage: String,
    pub kind: String,
    pub ordering_time: String,
    pub time_basis: String,
    pub oid: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProvenanceSummary {
    pub origin: ValueOrigin,
    pub proposal_ref: String,
    pub decision_ref: String,
    pub base_head: String,
    pub proposal_head: String,
    pub decision_head: String,
    pub projection_source_fingerprint: String,
    pub fsck_objects_verified: usize,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ValueSummary {
    pub origin: ValueOrigin,
    pub decision_retrievable: bool,
    pub proposal_retained: bool,
    pub unselected_alternative_retained: bool,
    pub external_reader_can_distinguish_roles: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct IncompleteSession {
    pub session: String,
    pub state: String,
    pub origin: ValueOrigin,
    pub proposal_present: bool,
    pub decision_present: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Limitation {
    pub code: String,
    pub message: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct BundleManifest {
    pub schema: SchemaIdentity,
    pub generator: GeneratorIdentity,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub renderer_profile: Option<SchemaIdentity>,
    pub target: OutputTarget,
    pub visibility: PublicationVisibility,
    pub publication_state: String,
    pub network_operations: u32,
    pub source_ref_snapshot_sha256: String,
    pub projection_path: String,
    pub story_path: String,
    pub html_path: String,
    pub checksums_path: String,
    pub projection_sha256: String,
    pub review_required_before_external_copy: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ChecksumsDocument {
    pub algorithm: String,
    pub files: Vec<FileChecksum>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Ord, PartialOrd, Serialize)]
pub struct FileChecksum {
    pub path: String,
    pub sha256: String,
    pub byte_len: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VerifiedBundle {
    pub manifest: BundleManifest,
    pub checksums: ChecksumsDocument,
}
