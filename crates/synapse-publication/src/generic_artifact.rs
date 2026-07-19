use super::{
    PublicationError, canonical_json_bytes, collect_bundle_files, publish_files_atomically,
    read_regular_file, reject_symlink_components, require_real_directory, sha256,
    validate_bundle_relative_path,
};
use crate::generic_artifact_render::render_generic_artifact_views;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};
use synapse_artifact::{
    ArtifactCheckoutLimits, ArtifactDisposition, TrustedArtifactDecisionBinding,
    checkout_artifact_decision,
};
use synapse_canonical::{canonical_bytes, parse_strict};

pub const GENERIC_ARTIFACT_PUBLICATION_PROFILE: &str =
    "org.synapsegit.generic-artifact-publication";
pub const GENERIC_ARTIFACT_PUBLICATION_PROFILE_VERSION: u32 = 1;
pub const GENERIC_ARTIFACT_PROJECTION_SCHEMA: &str =
    "org.synapsegit.generic-artifact-public-projection";
pub const GENERIC_ARTIFACT_PROJECTION_SCHEMA_VERSION: u32 = 1;
pub const GENERIC_ARTIFACT_BUNDLE_SCHEMA: &str =
    "org.synapsegit.generic-artifact-publication-bundle";
pub const GENERIC_ARTIFACT_BUNDLE_SCHEMA_VERSION: u32 = 1;
pub const GENERIC_ARTIFACT_CHECKSUMS_SCHEMA: &str =
    "org.synapsegit.generic-artifact-publication-checksums";
pub const GENERIC_ARTIFACT_CHECKSUMS_SCHEMA_VERSION: u32 = 1;
pub const GENERIC_ARTIFACT_RENDERER_PROFILE: &str =
    "org.synapsegit.generic-artifact-publication-renderer";
pub const GENERIC_ARTIFACT_RENDERER_PROFILE_VERSION: u32 = 1;
pub const LP_STUDIO_PUBLIC_TARGET_SCHEMA: &str = "org.synapsegit.lp-studio.public-target";
pub const LP_STUDIO_PUBLIC_TARGET_SCHEMA_VERSION: u32 = 1;

const GENERATOR_NAME: &str = "synapse-publication";
const LP_STUDIO_PRODUCT: &str = "synapsegit-lp-studio";
const LP_STUDIO_API_VERSION: &str = "v1";
const LP_STUDIO_API_SCHEMA_VERSION: &str = "1";
const LP_STUDIO_TARGET_SCHEMA_VERSION: u32 = 1;
const GENERIC_ARTIFACT_CONTRACT: &str = "synapsegit.generic-artifact";
const GENERIC_ARTIFACT_CONTRACT_VERSION: u32 = 1;
const MANIFEST_DIGEST_PROFILE: &str = "synapsegit-generic-artifact-manifest-v1";
const MANIFEST_DIGEST_PROFILE_VERSION: u32 = 1;
const MAX_PUBLIC_TARGET_BYTES: usize = 16 * 1024;
const MAX_SAFE_VERSION_LABEL_BYTES: usize = 64;
const MAX_TARGET_ID_BYTES: usize = 128;
const MAX_PUBLIC_LABEL_CHARACTERS: usize = 300;
const COMPLETE_VERIFICATION_SCOPE: &str = "The selected generic artifact site was reconstructed from one trusted Decision binding, its canonical Decision lineage was checked, every selected regular file was read and validated, and the application manifest digest was recomputed before projection.";
const PENDING_VERIFICATION_SCOPE: &str = "Pending is a bounded trusted status input only. No completed Human Decision, selected site, repository lineage, or artifact bytes were verified for this projection.";
const INCOMPLETE_VERIFICATION_SCOPE: &str = "Incomplete is a bounded trusted status input only. It is not evidence of a completed Human Decision and carries no selected site or portable authority.";
const ACCEPTED_SITE_IDENTITY_LIMIT: &str = "This SHA-256 binds the normalized regular-file paths and bytes under the generic-artifact manifest profile. It does not prove authorship, rights, truth, semantic equivalence, visual equivalence, or physical change.";

pub type GenericArtifactPublicationResult<T> =
    std::result::Result<T, GenericArtifactPublicationError>;

#[non_exhaustive]
#[derive(Debug)]
pub enum GenericArtifactPublicationError {
    InvalidInput(String),
    SourceVerification { code: &'static str },
    InvalidBundle(String),
    Publication(PublicationError),
}

impl GenericArtifactPublicationError {
    pub fn code(&self) -> &str {
        match self {
            Self::InvalidInput(_) => "generic_artifact_input_invalid",
            Self::SourceVerification { code } => code,
            Self::InvalidBundle(_) => "generic_artifact_bundle_invalid",
            Self::Publication(error) => error.code(),
        }
    }
}

impl fmt::Display for GenericArtifactPublicationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidInput(message) | Self::InvalidBundle(message) => {
                formatter.write_str(message)
            }
            Self::SourceVerification { code } => {
                write!(
                    formatter,
                    "generic artifact source verification failed ({code})"
                )
            }
            Self::Publication(error) => error.fmt(formatter),
        }
    }
}

impl Error for GenericArtifactPublicationError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Publication(error) => Some(error),
            _ => None,
        }
    }
}

impl From<PublicationError> for GenericArtifactPublicationError {
    fn from(error: PublicationError) -> Self {
        Self::Publication(error)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GenericArtifactSchemaIdentity {
    pub name: String,
    pub version: u32,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LpStudioPublicContractV1 {
    product: String,
    product_version: String,
    api_version: String,
    api_schema_version: String,
    target_schema_version: u32,
}

impl LpStudioPublicContractV1 {
    pub fn product_version(&self) -> &str {
        &self.product_version
    }

    pub fn api_version(&self) -> &str {
        &self.api_version
    }

    pub fn api_schema_version(&self) -> &str {
        &self.api_schema_version
    }

    pub const fn target_schema_version(&self) -> u32 {
        self.target_schema_version
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PublicTargetKindV1 {
    Page,
    Block,
    Element,
    Text,
    Point,
    Region,
}

impl PublicTargetKindV1 {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Page => "page",
            Self::Block => "block",
            Self::Element => "element",
            Self::Text => "text",
            Self::Point => "point",
            Self::Region => "region",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PublicTargetCaptureSourceV1 {
    Accepted,
    Proposal,
}

impl PublicTargetCaptureSourceV1 {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Accepted => "accepted",
            Self::Proposal => "proposal",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PublicTargetDescriptorV1 {
    target_id: String,
    kind: PublicTargetKindV1,
    label: String,
    capture_source: PublicTargetCaptureSourceV1,
}

impl PublicTargetDescriptorV1 {
    pub fn target_id(&self) -> &str {
        &self.target_id
    }

    pub const fn kind(&self) -> PublicTargetKindV1 {
        self.kind
    }

    pub fn label(&self) -> &str {
        &self.label
    }

    pub const fn capture_source(&self) -> PublicTargetCaptureSourceV1 {
        self.capture_source
    }
}

/// Reviewed, intentionally small LP Studio sidecar accepted by the public
/// projection profile.
///
/// This is not LP Studio's private `TargetV1`. DOM anchors, text quotes,
/// geometry, page paths, prompts, provider responses, private rationale, raw
/// bytes, repository paths, and Synapse authority identifiers have no field in
/// this type. Review remains a trusted application responsibility; parsing the
/// sidecar is validation, not proof that review occurred.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewedPublicTargetV1 {
    schema: GenericArtifactSchemaIdentity,
    lp_studio: LpStudioPublicContractV1,
    target: PublicTargetDescriptorV1,
}

impl ReviewedPublicTargetV1 {
    pub fn new(
        product_version: impl Into<String>,
        target_id: impl Into<String>,
        kind: PublicTargetKindV1,
        label: impl Into<String>,
        capture_source: PublicTargetCaptureSourceV1,
    ) -> GenericArtifactPublicationResult<Self> {
        let target = Self {
            schema: schema_identity(
                LP_STUDIO_PUBLIC_TARGET_SCHEMA,
                LP_STUDIO_PUBLIC_TARGET_SCHEMA_VERSION,
            ),
            lp_studio: LpStudioPublicContractV1 {
                product: LP_STUDIO_PRODUCT.into(),
                product_version: product_version.into(),
                api_version: LP_STUDIO_API_VERSION.into(),
                api_schema_version: LP_STUDIO_API_SCHEMA_VERSION.into(),
                target_schema_version: LP_STUDIO_TARGET_SCHEMA_VERSION,
            },
            target: PublicTargetDescriptorV1 {
                target_id: target_id.into(),
                kind,
                label: label.into(),
                capture_source,
            },
        };
        validate_public_target(&target)?;
        Ok(target)
    }

    pub fn lp_studio(&self) -> &LpStudioPublicContractV1 {
        &self.lp_studio
    }

    pub fn target(&self) -> &PublicTargetDescriptorV1 {
        &self.target
    }
}

/// Strictly parses one bounded reviewed PublicTarget sidecar.
pub fn parse_reviewed_public_target_v1(
    input: &[u8],
) -> GenericArtifactPublicationResult<ReviewedPublicTargetV1> {
    if input.len() > MAX_PUBLIC_TARGET_BYTES {
        return Err(invalid_input("reviewed PublicTarget sidecar is too large"));
    }
    let parsed = parse_strict(input)
        .map_err(|_| invalid_input("reviewed PublicTarget sidecar is not strict JSON"))?;
    let canonical = canonical_bytes(&parsed)
        .map_err(|_| invalid_input("reviewed PublicTarget sidecar is not canonical-safe"))?;
    let target: ReviewedPublicTargetV1 = serde_json::from_slice(&canonical)
        .map_err(|_| invalid_input("reviewed PublicTarget sidecar has an unsupported shape"))?;
    validate_public_target(&target)?;
    Ok(target)
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GenericArtifactOutcomeState {
    Complete,
    Pending,
    Incomplete,
}

impl GenericArtifactOutcomeState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Complete => "complete",
            Self::Pending => "pending",
            Self::Incomplete => "incomplete",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GenericArtifactStatusReason {
    PendingReview,
    RetryableFailure,
    OutcomeUnknown,
    TerminalDenial,
}

impl GenericArtifactStatusReason {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::PendingReview => "pending_review",
            Self::RetryableFailure => "retryable_failure",
            Self::OutcomeUnknown => "outcome_unknown",
            Self::TerminalDenial => "terminal_denial",
        }
    }
}

/// Bounded trusted status input for projections that have no completed
/// Decision binding.
///
/// This type deliberately contains no digest, repository locator, Ref, OID,
/// credential, permit, or serializable authority.
#[derive(Clone, Eq, PartialEq)]
pub struct TrustedGenericArtifactStatus {
    state: GenericArtifactOutcomeState,
    reason: GenericArtifactStatusReason,
}

impl TrustedGenericArtifactStatus {
    pub const fn pending() -> Self {
        Self {
            state: GenericArtifactOutcomeState::Pending,
            reason: GenericArtifactStatusReason::PendingReview,
        }
    }

    pub fn incomplete(
        reason: GenericArtifactStatusReason,
    ) -> GenericArtifactPublicationResult<Self> {
        if reason == GenericArtifactStatusReason::PendingReview {
            return Err(invalid_input(
                "pending_review cannot be represented as incomplete",
            ));
        }
        Ok(Self {
            state: GenericArtifactOutcomeState::Incomplete,
            reason,
        })
    }

    pub const fn state(&self) -> GenericArtifactOutcomeState {
        self.state
    }

    pub const fn reason(&self) -> GenericArtifactStatusReason {
        self.reason
    }
}

impl fmt::Debug for TrustedGenericArtifactStatus {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TrustedGenericArtifactStatus")
            .field("state", &self.state)
            .field("reason", &self.reason)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GenericArtifactVisibility {
    PrivateReview,
    Public,
}

impl GenericArtifactVisibility {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::PrivateReview => "private_review",
            Self::Public => "public",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GenericArtifactOutputTarget {
    Synapse,
    Github,
}

impl GenericArtifactOutputTarget {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Synapse => "synapse",
            Self::Github => "github",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GenericArtifactGeneratorIdentity {
    pub name: String,
    pub version: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GenericArtifactPublicationPolicyV1 {
    pub visibility: GenericArtifactVisibility,
    pub state: String,
    pub network_operations: u32,
    pub git_operations: u32,
    pub raw_site_bytes_included: bool,
    pub raw_site_paths_included: bool,
    pub private_rationale_included: bool,
    pub prompt_included: bool,
    pub provider_response_included: bool,
    pub repository_path_included: bool,
    pub internal_authority_ids_included: bool,
    pub training_use_policy: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GenericArtifactContractSummaryV1 {
    pub lp_studio: LpStudioPublicContractV1,
    pub generic_artifact: GenericArtifactSchemaIdentity,
    pub public_target: GenericArtifactSchemaIdentity,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GenericArtifactFactOrigin {
    VerifiedFromSynapse,
    BoundedTrustedStatusInput,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GenericArtifactHumanDisposition {
    AdoptedUnchanged,
    Rejected,
    Deferred,
}

impl GenericArtifactHumanDisposition {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::AdoptedUnchanged => "adopted_unchanged",
            Self::Rejected => "rejected",
            Self::Deferred => "deferred",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GenericArtifactSelectedSnapshot {
    Proposal,
    Base,
}

impl GenericArtifactSelectedSnapshot {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Proposal => "proposal",
            Self::Base => "base",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GenericArtifactAttributionV1 {
    pub mode: String,
    pub execution_verified: bool,
    pub origin: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GenericArtifactAcceptedSiteBindingV1 {
    pub profile: GenericArtifactSchemaIdentity,
    pub manifest_sha256: String,
    pub file_count: u64,
    pub total_bytes: u64,
    pub public_core_oid_included: bool,
    pub identity_limit: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GenericArtifactOutcomeV1 {
    pub state: GenericArtifactOutcomeState,
    pub fact_origin: GenericArtifactFactOrigin,
    pub verification_scope: String,
    pub source_attribution: GenericArtifactAttributionV1,
    pub human_disposition: Option<GenericArtifactHumanDisposition>,
    pub selected_snapshot: Option<GenericArtifactSelectedSnapshot>,
    pub accepted_site: Option<GenericArtifactAcceptedSiteBindingV1>,
    pub status_reason: Option<GenericArtifactStatusReason>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GenericArtifactLimitationV1 {
    pub code: String,
    pub message: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GenericArtifactPublicProjectionV1 {
    pub schema: GenericArtifactSchemaIdentity,
    pub profile: GenericArtifactSchemaIdentity,
    pub generator: GenericArtifactGeneratorIdentity,
    pub publication: GenericArtifactPublicationPolicyV1,
    pub contracts: GenericArtifactContractSummaryV1,
    pub public_target: ReviewedPublicTargetV1,
    pub outcome: GenericArtifactOutcomeV1,
    pub limitations: Vec<GenericArtifactLimitationV1>,
}

/// Build a complete projection only after the selected site and canonical
/// Human Decision lineage pass the generic artifact checkout boundary.
pub fn build_generic_artifact_complete_projection(
    public_target: &ReviewedPublicTargetV1,
    binding: &TrustedArtifactDecisionBinding,
    limits: ArtifactCheckoutLimits,
    visibility: GenericArtifactVisibility,
) -> GenericArtifactPublicationResult<GenericArtifactPublicProjectionV1> {
    validate_public_target(public_target)?;
    let checkout = checkout_artifact_decision(binding, limits).map_err(|error| {
        GenericArtifactPublicationError::SourceVerification { code: error.code() }
    })?;
    let (human_disposition, selected_snapshot) = match checkout.disposition() {
        ArtifactDisposition::AdoptedUnchanged => (
            GenericArtifactHumanDisposition::AdoptedUnchanged,
            GenericArtifactSelectedSnapshot::Proposal,
        ),
        ArtifactDisposition::Rejected => (
            GenericArtifactHumanDisposition::Rejected,
            GenericArtifactSelectedSnapshot::Base,
        ),
        ArtifactDisposition::Deferred => (
            GenericArtifactHumanDisposition::Deferred,
            GenericArtifactSelectedSnapshot::Base,
        ),
    };
    if checkout.selected_snapshot() != selected_snapshot.as_str() {
        return Err(GenericArtifactPublicationError::SourceVerification {
            code: "artifact_lineage_invalid",
        });
    }
    let file_count = u64::try_from(checkout.file_count()).map_err(|_| {
        GenericArtifactPublicationError::SourceVerification {
            code: "resource_limit",
        }
    })?;
    let projection = projection(
        public_target,
        visibility,
        GenericArtifactOutcomeV1 {
            state: GenericArtifactOutcomeState::Complete,
            fact_origin: GenericArtifactFactOrigin::VerifiedFromSynapse,
            verification_scope: COMPLETE_VERIFICATION_SCOPE.into(),
            source_attribution: attribution(),
            human_disposition: Some(human_disposition),
            selected_snapshot: Some(selected_snapshot),
            accepted_site: Some(GenericArtifactAcceptedSiteBindingV1 {
                profile: schema_identity(MANIFEST_DIGEST_PROFILE, MANIFEST_DIGEST_PROFILE_VERSION),
                manifest_sha256: checkout.manifest_sha256().to_owned(),
                file_count,
                total_bytes: checkout.total_bytes(),
                public_core_oid_included: false,
                identity_limit: ACCEPTED_SITE_IDENTITY_LIMIT.into(),
            }),
            status_reason: None,
        },
    );
    validate_generic_artifact_projection(&projection)?;
    Ok(projection)
}

/// Build a pending or incomplete projection from non-authority status input.
pub fn build_generic_artifact_status_projection(
    public_target: &ReviewedPublicTargetV1,
    status: &TrustedGenericArtifactStatus,
    visibility: GenericArtifactVisibility,
) -> GenericArtifactPublicationResult<GenericArtifactPublicProjectionV1> {
    validate_public_target(public_target)?;
    let verification_scope = match status.state {
        GenericArtifactOutcomeState::Pending => PENDING_VERIFICATION_SCOPE,
        GenericArtifactOutcomeState::Incomplete => INCOMPLETE_VERIFICATION_SCOPE,
        GenericArtifactOutcomeState::Complete => {
            return Err(invalid_input(
                "complete outcome requires a trusted Decision binding",
            ));
        }
    };
    let projection = projection(
        public_target,
        visibility,
        GenericArtifactOutcomeV1 {
            state: status.state,
            fact_origin: GenericArtifactFactOrigin::BoundedTrustedStatusInput,
            verification_scope: verification_scope.into(),
            source_attribution: attribution(),
            human_disposition: None,
            selected_snapshot: None,
            accepted_site: None,
            status_reason: Some(status.reason),
        },
    );
    validate_generic_artifact_projection(&projection)?;
    Ok(projection)
}

fn projection(
    public_target: &ReviewedPublicTargetV1,
    visibility: GenericArtifactVisibility,
    outcome: GenericArtifactOutcomeV1,
) -> GenericArtifactPublicProjectionV1 {
    GenericArtifactPublicProjectionV1 {
        schema: schema_identity(
            GENERIC_ARTIFACT_PROJECTION_SCHEMA,
            GENERIC_ARTIFACT_PROJECTION_SCHEMA_VERSION,
        ),
        profile: schema_identity(
            GENERIC_ARTIFACT_PUBLICATION_PROFILE,
            GENERIC_ARTIFACT_PUBLICATION_PROFILE_VERSION,
        ),
        generator: generator(),
        publication: GenericArtifactPublicationPolicyV1 {
            visibility,
            state: "local_preview".into(),
            network_operations: 0,
            git_operations: 0,
            raw_site_bytes_included: false,
            raw_site_paths_included: false,
            private_rationale_included: false,
            prompt_included: false,
            provider_response_included: false,
            repository_path_included: false,
            internal_authority_ids_included: false,
            training_use_policy: "prohibited".into(),
        },
        contracts: GenericArtifactContractSummaryV1 {
            lp_studio: public_target.lp_studio.clone(),
            generic_artifact: schema_identity(
                GENERIC_ARTIFACT_CONTRACT,
                GENERIC_ARTIFACT_CONTRACT_VERSION,
            ),
            public_target: schema_identity(
                LP_STUDIO_PUBLIC_TARGET_SCHEMA,
                LP_STUDIO_PUBLIC_TARGET_SCHEMA_VERSION,
            ),
        },
        public_target: public_target.clone(),
        limitations: limitations(outcome.state),
        outcome,
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GenericArtifactBundleManifestV1 {
    pub schema: GenericArtifactSchemaIdentity,
    pub profile: GenericArtifactSchemaIdentity,
    pub projection_schema: GenericArtifactSchemaIdentity,
    pub renderer_profile: GenericArtifactSchemaIdentity,
    pub generator: GenericArtifactGeneratorIdentity,
    pub target: GenericArtifactOutputTarget,
    pub visibility: GenericArtifactVisibility,
    pub outcome_state: GenericArtifactOutcomeState,
    pub publication_state: String,
    pub network_operations: u32,
    pub git_operations: u32,
    pub projection_path: String,
    pub story_path: String,
    pub html_path: String,
    pub checksums_path: String,
    pub projection_sha256: String,
    pub accepted_site_manifest_sha256: Option<String>,
    pub review_required_before_external_copy: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Ord, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GenericArtifactFileChecksumV1 {
    pub path: String,
    pub sha256: String,
    pub byte_len: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GenericArtifactChecksumsV1 {
    pub schema: GenericArtifactSchemaIdentity,
    pub algorithm: String,
    pub files: Vec<GenericArtifactFileChecksumV1>,
}

#[derive(Clone, Debug)]
pub struct GenericArtifactBundleOptions<'a> {
    pub projection: &'a GenericArtifactPublicProjectionV1,
    pub destination: PathBuf,
    pub target: GenericArtifactOutputTarget,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GenericArtifactExportReceipt {
    pub destination: PathBuf,
    pub target: GenericArtifactOutputTarget,
    pub visibility: GenericArtifactVisibility,
    pub outcome_state: GenericArtifactOutcomeState,
    pub projection_sha256: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VerifiedGenericArtifactBundle {
    pub manifest: GenericArtifactBundleManifestV1,
    pub checksums: GenericArtifactChecksumsV1,
    pub projection: GenericArtifactPublicProjectionV1,
}

/// Export a previously detached public projection to an atomic local bundle.
/// This phase has no repository, network, Git, or credential input.
pub fn export_generic_artifact_bundle(
    options: &GenericArtifactBundleOptions<'_>,
) -> GenericArtifactPublicationResult<GenericArtifactExportReceipt> {
    validate_generic_artifact_projection(options.projection)?;
    let destination = validate_generic_destination(&options.destination)?;
    let projection_bytes = canonical_generic_json(options.projection)?;
    let projection_sha256 = sha256(&projection_bytes);
    let (story_bytes, html_bytes) = render_generic_artifact_views(options.projection);
    let accepted_site_manifest_sha256 = options
        .projection
        .outcome
        .accepted_site
        .as_ref()
        .map(|binding| binding.manifest_sha256.clone());
    let manifest = GenericArtifactBundleManifestV1 {
        schema: schema_identity(
            GENERIC_ARTIFACT_BUNDLE_SCHEMA,
            GENERIC_ARTIFACT_BUNDLE_SCHEMA_VERSION,
        ),
        profile: schema_identity(
            GENERIC_ARTIFACT_PUBLICATION_PROFILE,
            GENERIC_ARTIFACT_PUBLICATION_PROFILE_VERSION,
        ),
        projection_schema: options.projection.schema.clone(),
        renderer_profile: schema_identity(
            GENERIC_ARTIFACT_RENDERER_PROFILE,
            GENERIC_ARTIFACT_RENDERER_PROFILE_VERSION,
        ),
        generator: generator(),
        target: options.target,
        visibility: options.projection.publication.visibility,
        outcome_state: options.projection.outcome.state,
        publication_state: "local_preview".into(),
        network_operations: 0,
        git_operations: 0,
        projection_path: "projection.json".into(),
        story_path: "story.md".into(),
        html_path: "index.html".into(),
        checksums_path: "checksums.json".into(),
        projection_sha256: projection_sha256.clone(),
        accepted_site_manifest_sha256,
        review_required_before_external_copy: options.projection.publication.visibility
            != GenericArtifactVisibility::Public,
    };
    let mut files = BTreeMap::<String, Vec<u8>>::new();
    files.insert("projection.json".into(), projection_bytes.clone());
    files.insert("story.md".into(), story_bytes.clone());
    files.insert("index.html".into(), html_bytes.clone());
    files.insert("manifest.json".into(), canonical_generic_json(&manifest)?);
    for spec in generic_target_file_specs(options.target) {
        files.insert(
            spec.path.into(),
            generic_target_content(spec.content, &projection_bytes, &story_bytes, &html_bytes)
                .to_vec(),
        );
    }
    let checksums = GenericArtifactChecksumsV1 {
        schema: schema_identity(
            GENERIC_ARTIFACT_CHECKSUMS_SCHEMA,
            GENERIC_ARTIFACT_CHECKSUMS_SCHEMA_VERSION,
        ),
        algorithm: "sha256".into(),
        files: files
            .iter()
            .map(|(path, bytes)| GenericArtifactFileChecksumV1 {
                path: path.clone(),
                sha256: sha256(bytes),
                byte_len: bytes.len() as u64,
            })
            .collect(),
    };
    files.insert("checksums.json".into(), canonical_generic_json(&checksums)?);
    publish_files_atomically(&destination, &files)?;
    Ok(GenericArtifactExportReceipt {
        destination,
        target: options.target,
        visibility: options.projection.publication.visibility,
        outcome_state: options.projection.outcome.state,
        projection_sha256,
    })
}

/// Verify only the explicitly versioned generic-artifact bundle profile.
/// Existing creator publication bundle v1 dispatch remains separate.
pub fn verify_generic_artifact_bundle(
    root: impl AsRef<Path>,
) -> GenericArtifactPublicationResult<VerifiedGenericArtifactBundle> {
    let root = root.as_ref();
    require_real_directory(root, "generic artifact publication bundle")?;
    let manifest_bytes = read_regular_file(&root.join("manifest.json"))?;
    let checksums_bytes = read_regular_file(&root.join("checksums.json"))?;
    let manifest: GenericArtifactBundleManifestV1 = serde_json::from_slice(&manifest_bytes)
        .map_err(|_| invalid_bundle("manifest.json has an unsupported shape"))?;
    let checksums: GenericArtifactChecksumsV1 = serde_json::from_slice(&checksums_bytes)
        .map_err(|_| invalid_bundle("checksums.json has an unsupported shape"))?;
    if canonical_generic_json(&manifest)? != manifest_bytes
        || canonical_generic_json(&checksums)? != checksums_bytes
    {
        return Err(invalid_bundle(
            "manifest.json and checksums.json must be canonical Synapse JSON",
        ));
    }
    resolve_generic_profile(&manifest.profile)?;
    resolve_generic_renderer(&manifest.renderer_profile)?;
    if manifest.schema
        != schema_identity(
            GENERIC_ARTIFACT_BUNDLE_SCHEMA,
            GENERIC_ARTIFACT_BUNDLE_SCHEMA_VERSION,
        )
        || manifest.projection_schema
            != schema_identity(
                GENERIC_ARTIFACT_PROJECTION_SCHEMA,
                GENERIC_ARTIFACT_PROJECTION_SCHEMA_VERSION,
            )
    {
        return Err(invalid_bundle(
            "unsupported generic artifact bundle or projection schema",
        ));
    }
    if manifest.projection_path != "projection.json"
        || manifest.story_path != "story.md"
        || manifest.html_path != "index.html"
        || manifest.checksums_path != "checksums.json"
        || manifest.publication_state != "local_preview"
        || manifest.network_operations != 0
        || manifest.git_operations != 0
    {
        return Err(invalid_bundle(
            "manifest is outside the fixed local-only generic artifact profile",
        ));
    }
    if checksums.schema
        != schema_identity(
            GENERIC_ARTIFACT_CHECKSUMS_SCHEMA,
            GENERIC_ARTIFACT_CHECKSUMS_SCHEMA_VERSION,
        )
        || checksums.algorithm != "sha256"
    {
        return Err(invalid_bundle("unsupported checksum profile"));
    }

    let actual_files = collect_bundle_files(root)?;
    let mut expected_files = BTreeSet::from(["checksums.json".to_owned()]);
    let required_checksummed_files = generic_target_checksummed_paths(manifest.target);
    let mut checksummed_files = BTreeSet::new();
    let mut previous = None::<&str>;
    for entry in &checksums.files {
        validate_bundle_relative_path(&entry.path)?;
        if previous.is_some_and(|value| value >= entry.path.as_str()) {
            return Err(invalid_bundle(
                "checksum entries must be unique and strictly sorted",
            ));
        }
        previous = Some(&entry.path);
        let bytes = read_regular_file(&root.join(&entry.path))?;
        if bytes.len() as u64 != entry.byte_len || sha256(&bytes) != entry.sha256 {
            return Err(invalid_bundle("generic artifact bundle checksum mismatch"));
        }
        expected_files.insert(entry.path.clone());
        checksummed_files.insert(entry.path.clone());
    }
    if checksummed_files != required_checksummed_files || actual_files != expected_files {
        return Err(invalid_bundle(
            "bundle inventory does not match the selected generic artifact target profile",
        ));
    }

    let projection_bytes = read_regular_file(&root.join("projection.json"))?;
    if sha256(&projection_bytes) != manifest.projection_sha256 {
        return Err(invalid_bundle(
            "manifest projection digest does not match projection.json",
        ));
    }
    let projection: GenericArtifactPublicProjectionV1 =
        serde_json::from_slice(&projection_bytes)
            .map_err(|_| invalid_bundle("projection.json has an unsupported shape"))?;
    if canonical_generic_json(&projection)? != projection_bytes {
        return Err(invalid_bundle(
            "projection.json is not canonical Synapse JSON",
        ));
    }
    validate_generic_artifact_projection(&projection)
        .map_err(|_| invalid_bundle("projection violates the generic artifact safe profile"))?;
    let accepted_digest = projection
        .outcome
        .accepted_site
        .as_ref()
        .map(|binding| binding.manifest_sha256.as_str());
    if manifest.projection_schema != projection.schema
        || manifest.generator != projection.generator
        || manifest.visibility != projection.publication.visibility
        || manifest.outcome_state != projection.outcome.state
        || manifest.accepted_site_manifest_sha256.as_deref() != accepted_digest
        || manifest.review_required_before_external_copy
            != (manifest.visibility != GenericArtifactVisibility::Public)
    {
        return Err(invalid_bundle(
            "manifest and projection describe different generic artifact facts",
        ));
    }
    let story_bytes = read_regular_file(&root.join("story.md"))?;
    let html_bytes = read_regular_file(&root.join("index.html"))?;
    let (expected_story, expected_html) = render_generic_artifact_views(&projection);
    if story_bytes != expected_story || html_bytes != expected_html {
        return Err(invalid_bundle(
            "Human views do not render from generic artifact projection.json",
        ));
    }
    for spec in generic_target_file_specs(manifest.target) {
        let expected =
            generic_target_content(spec.content, &projection_bytes, &story_bytes, &html_bytes);
        if read_regular_file(&root.join(spec.path))? != expected {
            return Err(invalid_bundle(
                "generic artifact target copy differs from the provider-neutral bundle",
            ));
        }
    }
    Ok(VerifiedGenericArtifactBundle {
        manifest,
        checksums,
        projection,
    })
}

pub fn validate_generic_artifact_projection(
    projection: &GenericArtifactPublicProjectionV1,
) -> GenericArtifactPublicationResult<()> {
    if projection.schema
        != schema_identity(
            GENERIC_ARTIFACT_PROJECTION_SCHEMA,
            GENERIC_ARTIFACT_PROJECTION_SCHEMA_VERSION,
        )
    {
        return Err(invalid_input(
            "unsupported generic artifact projection schema",
        ));
    }
    resolve_generic_profile(&projection.profile)?;
    if projection.generator.name != GENERATOR_NAME
        || !safe_version_label(&projection.generator.version)
    {
        return Err(invalid_input("unsupported generic artifact generator"));
    }
    validate_public_target(&projection.public_target)?;
    if projection.contracts.lp_studio != projection.public_target.lp_studio
        || projection.contracts.generic_artifact
            != schema_identity(GENERIC_ARTIFACT_CONTRACT, GENERIC_ARTIFACT_CONTRACT_VERSION)
        || projection.contracts.public_target
            != schema_identity(
                LP_STUDIO_PUBLIC_TARGET_SCHEMA,
                LP_STUDIO_PUBLIC_TARGET_SCHEMA_VERSION,
            )
        || projection.publication.state != "local_preview"
        || projection.publication.network_operations != 0
        || projection.publication.git_operations != 0
        || projection.publication.raw_site_bytes_included
        || projection.publication.raw_site_paths_included
        || projection.publication.private_rationale_included
        || projection.publication.prompt_included
        || projection.publication.provider_response_included
        || projection.publication.repository_path_included
        || projection.publication.internal_authority_ids_included
        || projection.publication.training_use_policy != "prohibited"
        || projection.outcome.source_attribution
            != (GenericArtifactAttributionV1 {
                mode: "caller_supplied_ai_attributed".into(),
                execution_verified: false,
                origin: "frozen_generic_artifact_contract_v1".into(),
            })
    {
        return Err(invalid_input(
            "projection is outside the generic artifact safe disclosure profile",
        ));
    }

    match projection.outcome.state {
        GenericArtifactOutcomeState::Complete => {
            let (Some(disposition), Some(snapshot), Some(site)) = (
                projection.outcome.human_disposition,
                projection.outcome.selected_snapshot,
                projection.outcome.accepted_site.as_ref(),
            ) else {
                return Err(invalid_input("complete outcome is missing verified facts"));
            };
            let expected_snapshot = match disposition {
                GenericArtifactHumanDisposition::AdoptedUnchanged => {
                    GenericArtifactSelectedSnapshot::Proposal
                }
                GenericArtifactHumanDisposition::Rejected
                | GenericArtifactHumanDisposition::Deferred => {
                    GenericArtifactSelectedSnapshot::Base
                }
            };
            if projection.outcome.fact_origin != GenericArtifactFactOrigin::VerifiedFromSynapse
                || projection.outcome.verification_scope != COMPLETE_VERIFICATION_SCOPE
                || projection.outcome.status_reason.is_some()
                || snapshot != expected_snapshot
                || site.profile
                    != schema_identity(MANIFEST_DIGEST_PROFILE, MANIFEST_DIGEST_PROFILE_VERSION)
                || !is_lower_sha256(&site.manifest_sha256)
                || site.public_core_oid_included
                || site.identity_limit != ACCEPTED_SITE_IDENTITY_LIMIT
            {
                return Err(invalid_input(
                    "complete outcome violates verified site semantics",
                ));
            }
        }
        GenericArtifactOutcomeState::Pending => {
            if projection.outcome.fact_origin
                != GenericArtifactFactOrigin::BoundedTrustedStatusInput
                || projection.outcome.verification_scope != PENDING_VERIFICATION_SCOPE
                || projection.outcome.human_disposition.is_some()
                || projection.outcome.selected_snapshot.is_some()
                || projection.outcome.accepted_site.is_some()
                || projection.outcome.status_reason
                    != Some(GenericArtifactStatusReason::PendingReview)
            {
                return Err(invalid_input(
                    "pending outcome must not claim completed authority",
                ));
            }
        }
        GenericArtifactOutcomeState::Incomplete => {
            if projection.outcome.fact_origin
                != GenericArtifactFactOrigin::BoundedTrustedStatusInput
                || projection.outcome.verification_scope != INCOMPLETE_VERIFICATION_SCOPE
                || projection.outcome.human_disposition.is_some()
                || projection.outcome.selected_snapshot.is_some()
                || projection.outcome.accepted_site.is_some()
                || !matches!(
                    projection.outcome.status_reason,
                    Some(
                        GenericArtifactStatusReason::RetryableFailure
                            | GenericArtifactStatusReason::OutcomeUnknown
                            | GenericArtifactStatusReason::TerminalDenial
                    )
                )
            {
                return Err(invalid_input(
                    "incomplete outcome must not claim completed authority",
                ));
            }
        }
    }
    if projection.limitations != limitations(projection.outcome.state) {
        return Err(invalid_input(
            "projection limitations do not match the selected profile",
        ));
    }
    Ok(())
}

fn validate_public_target(
    public_target: &ReviewedPublicTargetV1,
) -> GenericArtifactPublicationResult<()> {
    if public_target.schema
        != schema_identity(
            LP_STUDIO_PUBLIC_TARGET_SCHEMA,
            LP_STUDIO_PUBLIC_TARGET_SCHEMA_VERSION,
        )
        || public_target.lp_studio.product != LP_STUDIO_PRODUCT
        || public_target.lp_studio.api_version != LP_STUDIO_API_VERSION
        || public_target.lp_studio.api_schema_version != LP_STUDIO_API_SCHEMA_VERSION
        || public_target.lp_studio.target_schema_version != LP_STUDIO_TARGET_SCHEMA_VERSION
        || !safe_version_label(&public_target.lp_studio.product_version)
        || !safe_target_id(&public_target.target.target_id)
        || public_target.target.label.is_empty()
        || public_target.target.label.chars().count() > MAX_PUBLIC_LABEL_CHARACTERS
        || public_target
            .target
            .label
            .chars()
            .any(|character| character.is_control() || is_bidi_control(character))
    {
        return Err(invalid_input(
            "reviewed PublicTarget sidecar violates the fixed v1 allowlist",
        ));
    }
    Ok(())
}

fn safe_version_label(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_SAFE_VERSION_LABEL_BYTES
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_' | b'+'))
}

fn safe_target_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_TARGET_ID_BYTES
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_' | b':'))
}

fn is_bidi_control(character: char) -> bool {
    matches!(
        character,
        '\u{061c}'
            | '\u{200e}'
            | '\u{200f}'
            | '\u{202a}'..='\u{202e}'
            | '\u{2066}'..='\u{2069}'
    )
}

fn attribution() -> GenericArtifactAttributionV1 {
    GenericArtifactAttributionV1 {
        mode: "caller_supplied_ai_attributed".into(),
        execution_verified: false,
        origin: "frozen_generic_artifact_contract_v1".into(),
    }
}

fn limitations(state: GenericArtifactOutcomeState) -> Vec<GenericArtifactLimitationV1> {
    let mut values = vec![
        generic_limitation(
            "byte_identity_only",
            "The accepted-site digest verifies normalized regular-file path and byte identity only; it does not prove authorship, rights, truth, semantic equivalence, visual equivalence, or physical change.",
        ),
        generic_limitation(
            "caller_supplied_attribution",
            "The Proposal is caller-supplied AI-attributed output. SynapseGit did not execute or verify a model invocation and exposes no prompt or provider response.",
        ),
        generic_limitation(
            "public_target_sidecar_only",
            "Target metadata comes only from the reviewed PublicTarget v1 allowlist. Private Target evidence such as DOM paths, text quotes, geometry, and page paths is structurally omitted.",
        ),
        generic_limitation(
            "raw_site_omitted",
            "Site file paths and raw bytes are verified for a complete outcome but are never copied into this publication profile.",
        ),
        generic_limitation(
            "private_control_data_omitted",
            "Private rationale, application context, repository location, and internal Actor, Policy, Grant, Ref, Commit, and Tree identifiers are not represented.",
        ),
        generic_limitation(
            "local_bundle_only",
            "This projection and bundle perform no Git, GitHub API, hosted Synapse, upload, or other network operation and contain no remote publication receipt.",
        ),
        generic_limitation(
            "bundle_not_signed",
            "Checksums detect bundle damage but are not an identity signature or proof of who reviewed or published the bundle.",
        ),
        generic_limitation(
            "training_prohibited",
            "Machine-readable output is provided for inspection and interoperability, not as permission to train on the content.",
        ),
    ];
    if state != GenericArtifactOutcomeState::Complete {
        values.push(generic_limitation(
            "status_not_authority",
            "Pending and incomplete status is bounded trusted display input only. It is not a Decision receipt, permit, selected-site proof, or portable authority.",
        ));
    }
    values
}

fn generic_limitation(code: &str, message: &str) -> GenericArtifactLimitationV1 {
    GenericArtifactLimitationV1 {
        code: code.into(),
        message: message.into(),
    }
}

#[derive(Clone, Copy)]
enum GenericTargetContent {
    Projection,
    Story,
    Html,
}

#[derive(Clone, Copy)]
struct GenericTargetFileSpec {
    path: &'static str,
    content: GenericTargetContent,
}

const GENERIC_GITHUB_TARGET_FILES: &[GenericTargetFileSpec] = &[
    GenericTargetFileSpec {
        path: "target/README.md",
        content: GenericTargetContent::Story,
    },
    GenericTargetFileSpec {
        path: "target/index.html",
        content: GenericTargetContent::Html,
    },
    GenericTargetFileSpec {
        path: "target/projection.json",
        content: GenericTargetContent::Projection,
    },
];
const GENERIC_SYNAPSE_TARGET_FILES: &[GenericTargetFileSpec] = &[GenericTargetFileSpec {
    path: "target/generic-artifact-public-projection.json",
    content: GenericTargetContent::Projection,
}];

fn generic_target_file_specs(
    target: GenericArtifactOutputTarget,
) -> &'static [GenericTargetFileSpec] {
    match target {
        GenericArtifactOutputTarget::Synapse => GENERIC_SYNAPSE_TARGET_FILES,
        GenericArtifactOutputTarget::Github => GENERIC_GITHUB_TARGET_FILES,
    }
}

fn generic_target_checksummed_paths(target: GenericArtifactOutputTarget) -> BTreeSet<String> {
    ["index.html", "manifest.json", "projection.json", "story.md"]
        .into_iter()
        .chain(
            generic_target_file_specs(target)
                .iter()
                .map(|spec| spec.path),
        )
        .map(str::to_owned)
        .collect()
}

fn generic_target_content<'a>(
    content: GenericTargetContent,
    projection: &'a [u8],
    story: &'a [u8],
    html: &'a [u8],
) -> &'a [u8] {
    match content {
        GenericTargetContent::Projection => projection,
        GenericTargetContent::Story => story,
        GenericTargetContent::Html => html,
    }
}

fn resolve_generic_profile(
    profile: &GenericArtifactSchemaIdentity,
) -> GenericArtifactPublicationResult<()> {
    if profile
        == &schema_identity(
            GENERIC_ARTIFACT_PUBLICATION_PROFILE,
            GENERIC_ARTIFACT_PUBLICATION_PROFILE_VERSION,
        )
    {
        Ok(())
    } else {
        Err(invalid_bundle(
            "unsupported generic artifact publication profile",
        ))
    }
}

fn resolve_generic_renderer(
    renderer: &GenericArtifactSchemaIdentity,
) -> GenericArtifactPublicationResult<()> {
    if renderer
        == &schema_identity(
            GENERIC_ARTIFACT_RENDERER_PROFILE,
            GENERIC_ARTIFACT_RENDERER_PROFILE_VERSION,
        )
    {
        Ok(())
    } else {
        Err(invalid_bundle(
            "unsupported generic artifact renderer profile",
        ))
    }
}

fn validate_generic_destination(path: &Path) -> GenericArtifactPublicationResult<PathBuf> {
    match fs::symlink_metadata(path) {
        Ok(_) => {
            return Err(PublicationError::DestinationExists(path.to_path_buf()).into());
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(PublicationError::io(
                "inspect generic artifact publication destination",
                path,
                error,
            )
            .into());
        }
    }
    if path.as_os_str().is_empty()
        || path.file_name().is_none()
        || path
            .components()
            .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(PublicationError::UnsafePath(
            "generic artifact publication destination must name a new directory without '..'"
                .into(),
        )
        .into());
    }
    let parent = path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    reject_symlink_components(parent)?;
    require_real_directory(parent, "generic artifact publication parent")?;
    let parent = fs::canonicalize(parent).map_err(|error| {
        PublicationError::io(
            "canonicalize generic artifact publication parent",
            parent,
            error,
        )
    })?;
    Ok(parent.join(
        path.file_name()
            .expect("generic artifact destination file name was validated"),
    ))
}

fn canonical_generic_json<T: Serialize>(value: &T) -> GenericArtifactPublicationResult<Vec<u8>> {
    canonical_json_bytes(value).map_err(Into::into)
}

fn generator() -> GenericArtifactGeneratorIdentity {
    GenericArtifactGeneratorIdentity {
        name: GENERATOR_NAME.into(),
        version: env!("CARGO_PKG_VERSION").into(),
    }
}

fn schema_identity(name: &str, version: u32) -> GenericArtifactSchemaIdentity {
    GenericArtifactSchemaIdentity {
        name: name.into(),
        version,
    }
}

fn is_lower_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn invalid_input(message: impl Into<String>) -> GenericArtifactPublicationError {
    GenericArtifactPublicationError::InvalidInput(message.into())
}

fn invalid_bundle(message: impl Into<String>) -> GenericArtifactPublicationError {
    GenericArtifactPublicationError::InvalidBundle(message.into())
}
