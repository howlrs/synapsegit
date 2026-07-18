//! Provider-neutral, read-only projections of SynapseGit creator history.
//!
//! This crate is deliberately outside the Core protocol. It consumes one
//! coherent Ref snapshot, emits reviewable local files, and performs no
//! network or Git operation. Source-private rationale and raw asset bytes are
//! not copied into the public projection.

#![forbid(unsafe_code)]

mod model;
mod render;

pub use model::*;

use render::{render_html, render_story};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use synapse_canonical::{ObjectKind, canonical_bytes, parse_oid, parse_strict};
use synapse_core::{Repository, RepositoryError};
use synapse_creator::{
    CREATOR_FSCK_MAX_OBJECTS, CREATOR_FSCK_MAX_REF_ROOTS, CreatorError, CreatorReport,
    CreatorSessionState, creator_report_from_snapshot, discover_creator_sessions,
};

pub const DEFAULT_MAX_SESSIONS: usize = 100;
pub const MAX_PUBLICATION_SESSIONS: usize = 100;
const MAX_PRESENTATION_BYTES: u64 = 64 * 1024;
const MAX_BUNDLE_FILE_BYTES: u64 = 16 * 1024 * 1024;
const MAX_BUNDLE_DIRECTORY_ENTRIES: usize = 16;
const MAX_TITLE_BYTES: usize = 300;
const MAX_SUMMARY_BYTES: usize = 8 * 1024;
const MAX_PUBLIC_NOTE_BYTES: usize = 5 * 1024;
const MAX_CAPTION_BYTES: usize = 1_024;
const GENERATOR_NAME: &str = "synapse-publication";
const COMPLETE_VERIFICATION_SCOPE: &str =
    "One coherent read-only Ref snapshot plus digest-verified reachable CAS objects";
const INCOMPLETE_VERIFICATION_SCOPE: &str = "Deterministic Ref snapshot only; reachable CAS closure remains unverified because no complete creator report was available";
const SOURCE_RATIONALE_REASON: &str =
    "A verified public visibility grant is unavailable at the CreatorReport boundary";
const ARTIFACT_INTEGRITY_ASSURANCE: &str =
    "The OID matched locally stored bytes during snapshot-scoped integrity validation";
const ARTIFACT_OMISSION_REASON: &str =
    "Raw asset bytes are not copied by the M0/M1 safe publication profile";
const COMPARISON_INTERPRETATION_LIMIT: &str = "This compares primary Blob bytes only; it is not a pixel, semantic, or physical-change analysis.";

static NEXT_STAGING: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug)]
pub struct ProjectionOptions {
    pub repository: PathBuf,
    pub session: Option<String>,
    pub visibility: PublicationVisibility,
    pub presentation: PresentationInput,
    pub max_sessions: usize,
}

impl ProjectionOptions {
    pub fn new(repository: impl Into<PathBuf>) -> Self {
        Self {
            repository: repository.into(),
            session: None,
            visibility: PublicationVisibility::PrivateReview,
            presentation: PresentationInput::default(),
            max_sessions: DEFAULT_MAX_SESSIONS,
        }
    }
}

#[derive(Clone, Debug)]
pub struct ExportOptions {
    pub projection: ProjectionOptions,
    pub destination: PathBuf,
    pub target: OutputTarget,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExportReceipt {
    pub destination: PathBuf,
    pub target: OutputTarget,
    pub visibility: PublicationVisibility,
    pub sessions_exported: usize,
    pub incomplete_sessions: usize,
    pub projection_sha256: String,
}

#[derive(Debug)]
pub enum PublicationError {
    InvalidArgument(String),
    DestinationExists(PathBuf),
    UnsafePath(String),
    Repository(RepositoryError),
    Creator(CreatorError),
    Canonical(synapse_canonical::CoreError),
    Json(serde_json::Error),
    Toml(toml::de::Error),
    Io {
        operation: &'static str,
        path: PathBuf,
        source: io::Error,
    },
    InvalidBundle(String),
}

impl PublicationError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::InvalidArgument(_) => "usage_error",
            Self::DestinationExists(_) => "destination_exists",
            Self::UnsafePath(_) => "unsafe_path",
            Self::Repository(error) => match error.code() {
                "read_only_source_busy" => "read_only_source_busy",
                _ => "repository_error",
            },
            Self::Creator(_) => "creator_report_error",
            Self::Canonical(_) | Self::Json(_) | Self::Toml(_) => "projection_invalid",
            Self::Io { .. } => "storage_error",
            Self::InvalidBundle(_) => "bundle_invalid",
        }
    }

    fn io(operation: &'static str, path: impl Into<PathBuf>, source: io::Error) -> Self {
        Self::Io {
            operation,
            path: path.into(),
            source,
        }
    }
}

impl fmt::Display for PublicationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidArgument(message)
            | Self::UnsafePath(message)
            | Self::InvalidBundle(message) => formatter.write_str(message),
            Self::DestinationExists(path) => write!(
                formatter,
                "refusing to replace existing publication destination {}",
                path.display()
            ),
            Self::Repository(error) if error.code() == "read_only_source_busy" => formatter
                .write_str(
                    "read-only source is busy; stop repository writers and retry after SQLite checkpoints",
                ),
            Self::Repository(error) => error.fmt(formatter),
            Self::Creator(error) => error.fmt(formatter),
            Self::Canonical(error) => error.fmt(formatter),
            Self::Json(error) => write!(formatter, "publication JSON error: {error}"),
            Self::Toml(error) => write!(formatter, "presentation TOML error: {error}"),
            Self::Io {
                operation,
                path,
                source,
            } => write!(formatter, "{operation} {}: {source}", path.display()),
        }
    }
}

impl Error for PublicationError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Repository(error) => Some(error),
            Self::Creator(error) => Some(error),
            Self::Canonical(error) => Some(error),
            Self::Json(error) => Some(error),
            Self::Toml(error) => Some(error),
            Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

impl From<RepositoryError> for PublicationError {
    fn from(value: RepositoryError) -> Self {
        Self::Repository(value)
    }
}

impl From<CreatorError> for PublicationError {
    fn from(value: CreatorError) -> Self {
        Self::Creator(value)
    }
}

impl From<synapse_canonical::CoreError> for PublicationError {
    fn from(value: synapse_canonical::CoreError) -> Self {
        Self::Canonical(value)
    }
}

impl From<serde_json::Error> for PublicationError {
    fn from(value: serde_json::Error) -> Self {
        Self::Json(value)
    }
}

impl From<toml::de::Error> for PublicationError {
    fn from(value: toml::de::Error) -> Self {
        Self::Toml(value)
    }
}

pub type Result<T> = std::result::Result<T, PublicationError>;

/// Parse a bounded, regular, non-symlink `presentation.toml` sidecar.
///
/// Every value in the sidecar is publication metadata supplied by the author;
/// it is never treated as verified source history.
pub fn load_presentation(path: impl AsRef<Path>) -> Result<PresentationInput> {
    let path = path.as_ref();
    let parent = path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    reject_symlink_components(parent)?;
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| PublicationError::io("inspect presentation sidecar", path, error))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(PublicationError::UnsafePath(format!(
            "presentation sidecar must be a regular file, not a symlink: {}",
            path.display()
        )));
    }
    if metadata.len() > MAX_PRESENTATION_BYTES {
        return Err(PublicationError::InvalidArgument(format!(
            "presentation sidecar is larger than {MAX_PRESENTATION_BYTES} bytes"
        )));
    }
    let bytes = fs::read(path)
        .map_err(|error| PublicationError::io("read presentation sidecar", path, error))?;
    if bytes.len() as u64 > MAX_PRESENTATION_BYTES {
        return Err(PublicationError::InvalidArgument(format!(
            "presentation sidecar is larger than {MAX_PRESENTATION_BYTES} bytes"
        )));
    }
    let text = std::str::from_utf8(&bytes).map_err(|_| {
        PublicationError::InvalidArgument("presentation sidecar is not valid UTF-8".into())
    })?;
    let presentation: PresentationInput = toml::from_str(text)?;
    validate_presentation(&presentation)?;
    Ok(presentation)
}

/// Build a provider-neutral projection from exactly one read-only Ref snapshot.
pub fn build_public_projection(options: &ProjectionOptions) -> Result<PublicProjection> {
    if options.max_sessions == 0 {
        return Err(PublicationError::InvalidArgument(
            "max_sessions must be greater than zero".into(),
        ));
    }
    if options.max_sessions > MAX_PUBLICATION_SESSIONS {
        return Err(PublicationError::InvalidArgument(format!(
            "max_sessions must not exceed the publication ceiling of {MAX_PUBLICATION_SESSIONS}"
        )));
    }
    validate_presentation(&options.presentation)?;
    validate_existing_repository_path(&options.repository)?;

    let repository = Repository::open_existing_read_only(&options.repository)?;
    let snapshot = repository
        .refs()
        .snapshot_limited(CREATOR_FSCK_MAX_REF_ROOTS)
        .map_err(RepositoryError::from)?;
    let ref_snapshot_sha256 = ref_snapshot_sha256(snapshot.refs.iter().map(|record| {
        (
            record.name.as_str(),
            record.head.as_str(),
            record.updated_event_id,
        )
    }));
    let discovered = discover_creator_sessions(&repository, &snapshot, options.max_sessions)?;

    let discovered_names = discovered
        .iter()
        .map(|summary| summary.session.as_str())
        .collect::<BTreeSet<_>>();
    for session in options.presentation.sessions.keys() {
        if !discovered_names.contains(session.as_str()) {
            return Err(PublicationError::InvalidArgument(format!(
                "presentation metadata names unknown session {session:?}"
            )));
        }
    }

    let selected = if let Some(requested) = &options.session {
        let summary = discovered
            .iter()
            .find(|summary| &summary.session == requested)
            .ok_or_else(|| {
                PublicationError::InvalidArgument(format!(
                    "creator session {requested:?} was not found"
                ))
            })?;
        vec![summary]
    } else {
        discovered.iter().collect::<Vec<_>>()
    };

    let mut sessions = Vec::new();
    let mut incomplete_sessions = Vec::new();
    let mut projection_source_fingerprint = None::<String>;
    for summary in selected {
        match summary.state {
            CreatorSessionState::Complete => {
                let snapshot_report =
                    creator_report_from_snapshot(&repository, &snapshot, &summary.session)?;
                if let Some(existing) = &projection_source_fingerprint {
                    if existing != &snapshot_report.projection_source_fingerprint {
                        return Err(PublicationError::InvalidBundle(
                            "reports built from one Ref snapshot returned different projection fingerprints"
                                .into(),
                        ));
                    }
                } else {
                    projection_source_fingerprint =
                        Some(snapshot_report.projection_source_fingerprint.clone());
                }
                sessions.push(map_session(
                    snapshot_report.report,
                    snapshot_report.projection_source_fingerprint,
                    options.presentation.sessions.get(&summary.session),
                ));
            }
            CreatorSessionState::Incomplete => incomplete_sessions.push(IncompleteSession {
                session: summary.session.clone(),
                state: "incomplete".into(),
                origin: ValueOrigin::ObservedFromSynapse,
                proposal_present: summary.proposal_ref.is_some(),
                decision_present: summary.decision_ref.is_some(),
            }),
        }
    }

    let title = options.presentation.title.as_ref().map_or_else(
        || PresentedText {
            value: "SynapseGit creative history".into(),
            origin: ValueOrigin::DerivedSummary,
        },
        |value| PresentedText {
            value: value.clone(),
            origin: ValueOrigin::AuthorSupplied,
        },
    );
    let summary = options.presentation.summary.as_ref().map_or_else(
        || PresentedText {
            value: format!(
                "A reviewable history of {} complete creator session(s), preserving AI-attributed proposals and Human decisions without publishing raw source assets.",
                sessions.len()
            ),
            origin: ValueOrigin::DerivedSummary,
        },
        |value| PresentedText {
            value: value.clone(),
            origin: ValueOrigin::AuthorSupplied,
        },
    );
    let creator_display_name = author_text(options.presentation.creator_display_name.as_ref());
    let proposal_agent_display_name =
        author_text(options.presentation.proposal_agent_display_name.as_ref());

    let has_verified_reports = projection_source_fingerprint.is_some();
    let projection = PublicProjection {
        schema: schema(PROJECTION_SCHEMA, PROJECTION_SCHEMA_VERSION),
        generator: generator(),
        publication: PublicationPolicySummary {
            visibility: options.visibility,
            state: "local_preview".into(),
            network_operations: 0,
            raw_assets_included: false,
            source_private_rationale_included: false,
            internal_actor_ids_included: false,
            training_use_policy: "prohibited".into(),
        },
        source: SourceSummary {
            repository_path_included: false,
            ref_snapshot_sha256,
            ref_snapshot_digest_origin: ValueOrigin::DerivedSummary,
            fingerprint_origin: projection_source_fingerprint
                .as_ref()
                .map(|_| ValueOrigin::VerifiedFromSynapse),
            projection_source_fingerprint,
            verification_scope: if has_verified_reports {
                COMPLETE_VERIFICATION_SCOPE
            } else {
                INCOMPLETE_VERIFICATION_SCOPE
            }
            .into(),
        },
        presentation: ProjectionPresentation {
            title,
            summary,
            creator_display_name,
            proposal_agent_display_name,
        },
        completeness: CompletenessSummary {
            origin: ValueOrigin::DerivedSummary,
            requested_session: options.session.clone(),
            discovered_sessions: discovered.len(),
            complete_sessions_exported: sessions.len(),
            incomplete_sessions_retained: incomplete_sessions.len(),
            redactions: vec![
                "source_human_rationale".into(),
                "internal_actor_ids".into(),
                "repository_path".into(),
                "raw_asset_bytes".into(),
            ],
        },
        sessions,
        incomplete_sessions,
        limitations: projection_limitations(has_verified_reports),
    };
    validate_public_projection(&projection)?;
    Ok(projection)
}

/// Generate a deterministic local bundle through a staged, atomic no-replace
/// directory publication. Path safety is checked before the source is opened.
pub fn export_bundle(options: &ExportOptions) -> Result<ExportReceipt> {
    let destination = validate_export_paths(&options.projection.repository, &options.destination)?;
    let projection = build_public_projection(&options.projection)?;
    let projection_bytes = canonical_json_bytes(&projection)?;
    let projection_sha256 = sha256(&projection_bytes);
    let (story_bytes, html_bytes) = render_views(ResolvedRendererProfile::V1, &projection);

    let manifest = BundleManifest {
        schema: schema(BUNDLE_SCHEMA, BUNDLE_SCHEMA_VERSION),
        generator: generator(),
        renderer_profile: Some(renderer_profile()),
        target: options.target,
        visibility: options.projection.visibility,
        publication_state: "local_preview".into(),
        network_operations: 0,
        source_ref_snapshot_sha256: projection.source.ref_snapshot_sha256.clone(),
        projection_path: "projection.json".into(),
        story_path: "story.md".into(),
        html_path: "index.html".into(),
        checksums_path: "checksums.json".into(),
        projection_sha256: projection_sha256.clone(),
        review_required_before_external_copy: options.projection.visibility
            != PublicationVisibility::Public,
    };

    let mut files = BTreeMap::<String, Vec<u8>>::new();
    files.insert("projection.json".into(), projection_bytes.clone());
    files.insert("story.md".into(), story_bytes.clone());
    files.insert("index.html".into(), html_bytes.clone());
    files.insert("manifest.json".into(), canonical_json_bytes(&manifest)?);
    for spec in target_file_specs(options.target) {
        files.insert(
            spec.path.into(),
            target_content(spec.content, &projection_bytes, &story_bytes, &html_bytes).to_vec(),
        );
    }
    let checksums = ChecksumsDocument {
        algorithm: "sha256".into(),
        files: files
            .iter()
            .map(|(path, bytes)| FileChecksum {
                path: path.clone(),
                sha256: sha256(bytes),
                byte_len: bytes.len() as u64,
            })
            .collect(),
    };
    files.insert("checksums.json".into(), canonical_json_bytes(&checksums)?);
    publish_files_atomically(&destination, &files)?;

    Ok(ExportReceipt {
        destination,
        target: options.target,
        visibility: options.projection.visibility,
        sessions_exported: projection.sessions.len(),
        incomplete_sessions: projection.incomplete_sessions.len(),
        projection_sha256,
    })
}

/// Validate a bundle's fixed inventory, checksums, schemas, and semantic links.
pub fn verify_bundle(root: impl AsRef<Path>) -> Result<VerifiedBundle> {
    let root = root.as_ref();
    require_real_directory(root, "publication bundle")?;
    let manifest_bytes = read_regular_file(&root.join("manifest.json"))?;
    let checksums_bytes = read_regular_file(&root.join("checksums.json"))?;
    let manifest: BundleManifest = serde_json::from_slice(&manifest_bytes)?;
    let checksums: ChecksumsDocument = serde_json::from_slice(&checksums_bytes)?;
    if canonical_json_bytes(&manifest)? != manifest_bytes
        || canonical_json_bytes(&checksums)? != checksums_bytes
    {
        return Err(PublicationError::InvalidBundle(
            "manifest.json and checksums.json must be canonical Synapse JSON".into(),
        ));
    }
    if manifest.schema != schema(BUNDLE_SCHEMA, BUNDLE_SCHEMA_VERSION) {
        return Err(PublicationError::InvalidBundle(
            "unsupported publication bundle schema".into(),
        ));
    }
    let resolved_renderer = resolve_renderer_profile(manifest.renderer_profile.as_ref())?;
    if manifest.projection_path != "projection.json"
        || manifest.story_path != "story.md"
        || manifest.html_path != "index.html"
        || manifest.checksums_path != "checksums.json"
    {
        return Err(PublicationError::InvalidBundle(
            "manifest uses an unsupported bundle path".into(),
        ));
    }
    if manifest.network_operations != 0 || manifest.publication_state != "local_preview" {
        return Err(PublicationError::InvalidBundle(
            "manifest does not describe a local-only preview".into(),
        ));
    }
    if checksums.algorithm != "sha256" {
        return Err(PublicationError::InvalidBundle(
            "checksums algorithm must be sha256".into(),
        ));
    }

    let actual_files = collect_bundle_files(root)?;
    let mut expected_files = BTreeSet::from(["checksums.json".to_owned()]);
    let required_checksummed_files = target_checksummed_paths(manifest.target);
    let mut checksummed_files = BTreeSet::new();
    let mut previous = None::<&str>;
    for entry in &checksums.files {
        validate_bundle_relative_path(&entry.path)?;
        if previous.is_some_and(|value| value >= entry.path.as_str()) {
            return Err(PublicationError::InvalidBundle(
                "checksum entries must be unique and strictly sorted".into(),
            ));
        }
        previous = Some(&entry.path);
        let bytes = read_regular_file(&root.join(&entry.path))?;
        if bytes.len() as u64 != entry.byte_len || sha256(&bytes) != entry.sha256 {
            return Err(PublicationError::InvalidBundle(format!(
                "checksum mismatch for {:?}",
                entry.path
            )));
        }
        expected_files.insert(entry.path.clone());
        checksummed_files.insert(entry.path.clone());
    }
    if checksummed_files != required_checksummed_files {
        return Err(PublicationError::InvalidBundle(
            "bundle checksum inventory does not match the selected target profile".into(),
        ));
    }
    if actual_files != expected_files {
        return Err(PublicationError::InvalidBundle(
            "bundle contains an unlisted or missing file".into(),
        ));
    }

    let projection_bytes = read_regular_file(&root.join(&manifest.projection_path))?;
    if sha256(&projection_bytes) != manifest.projection_sha256 {
        return Err(PublicationError::InvalidBundle(
            "manifest projection digest does not match projection.json".into(),
        ));
    }
    let projection: PublicProjection = serde_json::from_slice(&projection_bytes)?;
    if projection.schema != schema(PROJECTION_SCHEMA, PROJECTION_SCHEMA_VERSION) {
        return Err(PublicationError::InvalidBundle(
            "unsupported public projection schema".into(),
        ));
    }
    let canonical_projection = canonical_json_bytes(&projection)?;
    if canonical_projection != projection_bytes {
        return Err(PublicationError::InvalidBundle(
            "projection.json is not canonical Synapse JSON".into(),
        ));
    }
    validate_public_projection(&projection)?;
    if projection.source.ref_snapshot_sha256 != manifest.source_ref_snapshot_sha256
        || projection.publication.visibility != manifest.visibility
        || projection.generator != manifest.generator
    {
        return Err(PublicationError::InvalidBundle(
            "manifest and projection describe different source, visibility, or generator".into(),
        ));
    }
    if manifest.generator.name != GENERATOR_NAME
        || projection.publication.state != "local_preview"
        || projection.publication.network_operations != 0
        || projection.publication.raw_assets_included
        || projection.publication.source_private_rationale_included
        || projection.publication.internal_actor_ids_included
        || projection.publication.training_use_policy != "prohibited"
        || manifest.review_required_before_external_copy
            != (manifest.visibility != PublicationVisibility::Public)
    {
        return Err(PublicationError::InvalidBundle(
            "bundle is outside the supported safe local publication profile".into(),
        ));
    }
    let story_bytes = read_regular_file(&root.join(&manifest.story_path))?;
    let html_bytes = read_regular_file(&root.join(&manifest.html_path))?;
    let (expected_story, expected_html) = render_views(resolved_renderer, &projection);
    if story_bytes != expected_story || html_bytes != expected_html {
        return Err(PublicationError::InvalidBundle(
            "Human views do not render from projection.json".into(),
        ));
    }
    for spec in target_file_specs(manifest.target) {
        let expected = target_content(spec.content, &projection_bytes, &story_bytes, &html_bytes);
        if read_regular_file(&root.join(spec.path))? != expected {
            return Err(PublicationError::InvalidBundle(
                "target copy differs from the provider-neutral bundle".into(),
            ));
        }
    }
    Ok(VerifiedBundle {
        manifest,
        checksums,
    })
}

#[derive(Clone, Copy)]
enum TargetContent {
    Projection,
    Story,
    Html,
}

#[derive(Clone, Copy)]
struct TargetFileSpec {
    path: &'static str,
    content: TargetContent,
}

const GITHUB_TARGET_FILES: &[TargetFileSpec] = &[
    TargetFileSpec {
        path: "target/README.md",
        content: TargetContent::Story,
    },
    TargetFileSpec {
        path: "target/index.html",
        content: TargetContent::Html,
    },
    TargetFileSpec {
        path: "target/projection.json",
        content: TargetContent::Projection,
    },
];
const SYNAPSE_TARGET_FILES: &[TargetFileSpec] = &[TargetFileSpec {
    path: "target/public-projection.json",
    content: TargetContent::Projection,
}];

fn target_file_specs(target: OutputTarget) -> &'static [TargetFileSpec] {
    match target {
        OutputTarget::Github => GITHUB_TARGET_FILES,
        OutputTarget::Synapse => SYNAPSE_TARGET_FILES,
    }
}

fn target_checksummed_paths(target: OutputTarget) -> BTreeSet<String> {
    ["index.html", "manifest.json", "projection.json", "story.md"]
        .into_iter()
        .chain(target_file_specs(target).iter().map(|spec| spec.path))
        .map(str::to_owned)
        .collect()
}

fn target_content<'a>(
    content: TargetContent,
    projection: &'a [u8],
    story: &'a [u8],
    html: &'a [u8],
) -> &'a [u8] {
    match content {
        TargetContent::Projection => projection,
        TargetContent::Story => story,
        TargetContent::Html => html,
    }
}

fn renderer_profile() -> SchemaIdentity {
    schema(RENDERER_PROFILE, RENDERER_PROFILE_VERSION)
}

#[derive(Clone, Copy)]
enum ResolvedRendererProfile {
    V1,
}

fn resolve_renderer_profile(profile: Option<&SchemaIdentity>) -> Result<ResolvedRendererProfile> {
    match profile {
        // Bundles created before the profile field was added used this exact
        // v1 renderer under bundle schema v1.
        None => Ok(ResolvedRendererProfile::V1),
        Some(profile) if profile == &renderer_profile() => Ok(ResolvedRendererProfile::V1),
        Some(_) => Err(PublicationError::InvalidBundle(
            "unsupported publication renderer profile".into(),
        )),
    }
}

fn render_views(
    profile: ResolvedRendererProfile,
    projection: &PublicProjection,
) -> (Vec<u8>, Vec<u8>) {
    // Profile v1 is frozen. A future renderer must use a new profile version
    // and retain an explicit verifier dispatch for bundles created with v1.
    match profile {
        ResolvedRendererProfile::V1 => (
            render_story(projection).into_bytes(),
            render_html(projection).into_bytes(),
        ),
    }
}

fn map_session(
    report: CreatorReport,
    fingerprint: String,
    presentation: Option<&SessionPresentationInput>,
) -> PublicSession {
    let presentation = presentation.cloned().unwrap_or_default();
    let session_title = presentation.title.map_or_else(
        || PresentedText {
            value: format!("Session {}", report.session),
            origin: ValueOrigin::DerivedSummary,
        },
        |value| PresentedText {
            value,
            origin: ValueOrigin::AuthorSupplied,
        },
    );
    let history = vec![
        artifact(
            ArtifactRole::Original,
            report.original_blob_oid.clone(),
            presentation.original_caption,
            "Recorded original source",
        ),
        artifact(
            ArtifactRole::Current,
            report.current_blob_oid.clone(),
            presentation.current_caption,
            "Recorded current state",
        ),
        artifact(
            ArtifactRole::AiProposal,
            report.ai_output_blob_oid.clone(),
            presentation.proposal_caption,
            "AI-attributed proposal",
        ),
    ];
    let comparison = report.comparison.map(|comparison| ComparisonSummary {
        origin: ValueOrigin::VerifiedFromSynapse,
        analysis_kind: "primary_blob_byte_identity".into(),
        status: comparison.status,
        comparability: comparison.comparability,
        outcome: comparison.outcome,
        reason_codes: comparison.reason_codes,
        warnings: comparison.warnings,
        replay_ready: comparison.replay_ready,
        interpretation_limit: COMPARISON_INTERPRETATION_LIMIT.into(),
    });
    let timeline = report
        .timeline
        .into_iter()
        .map(|entry| TimelineEntry {
            origin: ValueOrigin::VerifiedFromSynapse,
            stage: entry.stage.into(),
            kind: entry.kind.into(),
            ordering_time: entry.ordering_time,
            time_basis: entry.time_basis.into(),
            oid: entry.oid,
        })
        .collect();
    let selected_artifact = if report.selected_ai_output {
        ArtifactRole::AiProposal
    } else {
        ArtifactRole::Current
    };
    let disposition = report.disposition.as_cli_str().to_owned();
    let public_decision_note = presentation
        .public_decision_note
        .map(|value| PresentedText {
            value,
            origin: ValueOrigin::AuthorSupplied,
        });

    PublicSession {
        session: report.session,
        title: session_title,
        state: "complete".into(),
        source_fact_origin: ValueOrigin::VerifiedFromSynapse,
        history,
        proposal: ProposalSummary {
            attribution_role: "ai_agent".into(),
            attribution_origin: ValueOrigin::VerifiedFromSynapse,
            attribution_scope: "Caller-supplied output recorded by the workflow as AI-attributed; no model invocation is independently verified"
                .into(),
            attribution_scope_origin: ValueOrigin::DerivedSummary,
            selected_by_human: report.selected_ai_output,
            retained_when_unselected: true,
        },
        human_decision: HumanDecisionSummary {
            reviewer_role: "human_reviewer".into(),
            disposition,
            disposition_origin: ValueOrigin::VerifiedFromSynapse,
            selected_artifact,
            public_decision_note,
            source_rationale: RedactedField {
                state: "redacted".into(),
                reason: SOURCE_RATIONALE_REASON.into(),
                source_policy: "conservative_private".into(),
            },
        },
        comparison,
        timeline,
        provenance: ProvenanceSummary {
            origin: ValueOrigin::VerifiedFromSynapse,
            proposal_ref: report.proposal_ref,
            decision_ref: report.decision_ref,
            base_head: report.base_head,
            proposal_head: report.proposal_head,
            decision_head: report.decision_head,
            projection_source_fingerprint: fingerprint,
            fsck_objects_verified: report.fsck_objects,
        },
        value: ValueSummary {
            origin: ValueOrigin::DerivedSummary,
            decision_retrievable: true,
            proposal_retained: true,
            unselected_alternative_retained: true,
            external_reader_can_distinguish_roles: true,
        },
        limitations: session_limitations(),
    }
}

fn artifact(
    role: ArtifactRole,
    oid: String,
    caption: Option<String>,
    default_caption: &str,
) -> Artifact {
    Artifact {
        role,
        oid,
        oid_origin: ValueOrigin::VerifiedFromSynapse,
        caption: caption.map_or_else(
            || PresentedText {
                value: default_caption.into(),
                origin: ValueOrigin::DerivedSummary,
            },
            |value| PresentedText {
                value,
                origin: ValueOrigin::AuthorSupplied,
            },
        ),
        integrity_assurance: ARTIFACT_INTEGRITY_ASSURANCE.into(),
        public_rendering: ArtifactRendering {
            state: "omitted_by_policy".into(),
            path: None,
            media_type: None,
            reason: ARTIFACT_OMISSION_REASON.into(),
        },
    }
}

fn projection_limitations(has_verified_reports: bool) -> Vec<Limitation> {
    let mut limitations = vec![
        limitation(
            "byte_identity_only",
            "SynapseGit verifies stored bytes and graph relations; it does not prove authorship, truth, rights, permission, or physical change.",
        ),
        limitation(
            "raw_assets_omitted",
            "Original, current, and proposal bytes are omitted by default to avoid leaking metadata, active content, or unrelated private material.",
        ),
        limitation(
            "private_rationale_redacted",
            "The source CreatorReport does not expose a verified feedback visibility policy. Its rationale is therefore withheld; only a separately supplied public decision note may appear.",
        ),
        limitation(
            "attribution_is_scoped",
            "The proposal is AI-attributed by the recorded workflow. This bundle does not verify that a model generated the supplied bytes or identify a model invocation.",
        ),
        limitation(
            "training_prohibited",
            "Machine-readable output is provided for inspection and interoperability, not as permission to train on the content.",
        ),
        limitation(
            "local_bundle_only",
            "This export performs no Git, GitHub, Synapse service, upload, or other network operation and contains no remote publication receipt.",
        ),
        limitation(
            "identifier_correlation",
            "Artifact OIDs and technical Ref/Commit identifiers can correlate this view with another copy of the same history; review them before external publication.",
        ),
        limitation(
            "bundle_not_signed",
            "Checksums detect accidental bundle damage but are not an identity signature or proof of who published the bundle.",
        ),
    ];
    if !has_verified_reports {
        limitations.push(limitation(
            "projection_fingerprint_unavailable",
            "No complete creator report was available, so only the deterministic Ref snapshot digest is present.",
        ));
    }
    limitations
}

fn session_limitations() -> Vec<Limitation> {
    vec![
        limitation(
            "source_rationale_not_public",
            "The recorded source rationale was not copied. A public decision note, when present, is separate author-supplied text.",
        ),
        limitation(
            "artifact_preview_unavailable",
            "This first safe renderer identifies artifacts by role and OID but does not include their raw bytes or thumbnails.",
        ),
    ]
}

fn validate_public_projection(projection: &PublicProjection) -> Result<()> {
    if projection.schema != schema(PROJECTION_SCHEMA, PROJECTION_SCHEMA_VERSION)
        || projection.generator.name != GENERATOR_NAME
        || !safe_version_label(&projection.generator.version)
    {
        return invalid_public_profile("projection identity is unsupported");
    }
    if projection.publication.state != "local_preview"
        || projection.publication.network_operations != 0
        || projection.publication.raw_assets_included
        || projection.publication.source_private_rationale_included
        || projection.publication.internal_actor_ids_included
        || projection.publication.training_use_policy != "prohibited"
    {
        return invalid_public_profile("projection publication policy is outside the safe profile");
    }
    if projection.source.repository_path_included
        || !is_lower_sha256(&projection.source.ref_snapshot_sha256)
        || projection.source.ref_snapshot_digest_origin != ValueOrigin::DerivedSummary
    {
        return invalid_public_profile("projection source summary is outside the safe profile");
    }

    validate_projection_presentation(&projection.presentation, projection.sessions.len())?;

    let complete = projection.sessions.len();
    let incomplete = projection.incomplete_sessions.len();
    let selected = complete
        .checked_add(incomplete)
        .ok_or_else(|| profile_error("projection session count overflow"))?;
    if selected > MAX_PUBLICATION_SESSIONS
        || projection.completeness.origin != ValueOrigin::DerivedSummary
        || projection.completeness.complete_sessions_exported != complete
        || projection.completeness.incomplete_sessions_retained != incomplete
        || projection.completeness.discovered_sessions > MAX_PUBLICATION_SESSIONS
        || projection.completeness.redactions
            != [
                "source_human_rationale",
                "internal_actor_ids",
                "repository_path",
                "raw_asset_bytes",
            ]
    {
        return invalid_public_profile("projection completeness summary is inconsistent");
    }

    match projection.completeness.requested_session.as_deref() {
        Some(requested)
            if !valid_session_name(requested)
                || selected != 1
                || projection.completeness.discovered_sessions == 0 =>
        {
            return invalid_public_profile("requested-session completeness is inconsistent");
        }
        Some(_) => {}
        None if projection.completeness.discovered_sessions != selected => {
            return invalid_public_profile("all-session completeness is inconsistent");
        }
        None => {}
    }

    let has_verified_reports = !projection.sessions.is_empty();
    if projection.limitations != projection_limitations(has_verified_reports) {
        return invalid_public_profile("projection disclosure statements are not the v1 profile");
    }
    match (
        has_verified_reports,
        projection.source.projection_source_fingerprint.as_deref(),
        projection.source.fingerprint_origin,
        projection.source.verification_scope.as_str(),
    ) {
        (
            true,
            Some(fingerprint),
            Some(ValueOrigin::VerifiedFromSynapse),
            COMPLETE_VERIFICATION_SCOPE,
        ) if valid_projection_fingerprint(fingerprint) => {}
        (false, None, None, INCOMPLETE_VERIFICATION_SCOPE) => {}
        _ => {
            return invalid_public_profile(
                "projection verification scope overstates or misstates source validation",
            );
        }
    }

    let mut names = BTreeSet::new();
    for session in &projection.sessions {
        if !names.insert(session.session.as_str()) {
            return invalid_public_profile("projection contains duplicate session names");
        }
        validate_public_session(
            session,
            projection
                .source
                .projection_source_fingerprint
                .as_deref()
                .expect("complete projections have a validated fingerprint"),
        )?;
    }
    for session in &projection.incomplete_sessions {
        if !names.insert(session.session.as_str())
            || !valid_session_name(&session.session)
            || session.state != "incomplete"
            || session.origin != ValueOrigin::ObservedFromSynapse
            || (!session.proposal_present && !session.decision_present)
        {
            return invalid_public_profile("incomplete session summary is inconsistent");
        }
    }
    if let Some(requested) = projection.completeness.requested_session.as_deref()
        && !names.contains(requested)
    {
        return invalid_public_profile("requested session does not match the exported session");
    }
    Ok(())
}

fn validate_projection_presentation(
    presentation: &ProjectionPresentation,
    complete_sessions: usize,
) -> Result<()> {
    validate_presented_text(
        "projection title",
        &presentation.title,
        MAX_TITLE_BYTES,
        false,
        Some("SynapseGit creative history"),
    )?;
    let default_summary = format!(
        "A reviewable history of {complete_sessions} complete creator session(s), preserving AI-attributed proposals and Human decisions without publishing raw source assets."
    );
    validate_presented_text(
        "projection summary",
        &presentation.summary,
        MAX_SUMMARY_BYTES,
        true,
        Some(&default_summary),
    )?;
    for (field, value) in [
        ("creator display name", &presentation.creator_display_name),
        (
            "proposal agent display name",
            &presentation.proposal_agent_display_name,
        ),
    ] {
        if let Some(value) = value {
            validate_author_text(field, value, MAX_TITLE_BYTES, false)?;
        }
    }
    Ok(())
}

fn validate_public_session(session: &PublicSession, source_fingerprint: &str) -> Result<()> {
    if !valid_session_name(&session.session)
        || session.state != "complete"
        || session.source_fact_origin != ValueOrigin::VerifiedFromSynapse
        || session.limitations != session_limitations()
    {
        return invalid_public_profile("complete session is outside the v1 profile");
    }
    validate_presented_text(
        "session title",
        &session.title,
        MAX_TITLE_BYTES,
        false,
        Some(&format!("Session {}", session.session)),
    )?;

    let expected_roles = [
        (ArtifactRole::Original, "Recorded original source"),
        (ArtifactRole::Current, "Recorded current state"),
        (ArtifactRole::AiProposal, "AI-attributed proposal"),
    ];
    if session.history.len() != expected_roles.len() {
        return invalid_public_profile("complete session must contain three artifact roles");
    }
    for (artifact, (role, default_caption)) in session.history.iter().zip(expected_roles) {
        if artifact.role != role
            || artifact.oid_origin != ValueOrigin::VerifiedFromSynapse
            || parse_oid(&artifact.oid).ok() != Some(ObjectKind::Blob)
            || artifact.integrity_assurance != ARTIFACT_INTEGRITY_ASSURANCE
            || artifact.public_rendering.state != "omitted_by_policy"
            || artifact.public_rendering.path.is_some()
            || artifact.public_rendering.media_type.is_some()
            || artifact.public_rendering.reason != ARTIFACT_OMISSION_REASON
        {
            return invalid_public_profile("artifact disclosure is outside the safe v1 profile");
        }
        validate_presented_text(
            "artifact caption",
            &artifact.caption,
            MAX_CAPTION_BYTES,
            false,
            Some(default_caption),
        )?;
    }

    let selected_ai = session.proposal.selected_by_human;
    if session.proposal.attribution_role != "ai_agent"
        || session.proposal.attribution_origin != ValueOrigin::VerifiedFromSynapse
        || session.proposal.attribution_scope
            != "Caller-supplied output recorded by the workflow as AI-attributed; no model invocation is independently verified"
        || session.proposal.attribution_scope_origin != ValueOrigin::DerivedSummary
        || !session.proposal.retained_when_unselected
        || session.human_decision.reviewer_role != "human_reviewer"
        || session.human_decision.disposition_origin != ValueOrigin::VerifiedFromSynapse
        || session.human_decision.source_rationale.state != "redacted"
        || session.human_decision.source_rationale.reason != SOURCE_RATIONALE_REASON
        || session.human_decision.source_rationale.source_policy != "conservative_private"
    {
        return invalid_public_profile("proposal or Human-decision disclosure is invalid");
    }
    let expected_selected = match session.human_decision.disposition.as_str() {
        "adopt" => (true, ArtifactRole::AiProposal),
        "reject" | "defer" => (false, ArtifactRole::Current),
        _ => return invalid_public_profile("Human disposition is unsupported"),
    };
    if (selected_ai, session.human_decision.selected_artifact) != expected_selected {
        return invalid_public_profile("Human disposition and selected artifact disagree");
    }
    if let Some(note) = &session.human_decision.public_decision_note {
        validate_author_text("public decision note", note, MAX_PUBLIC_NOTE_BYTES, true)?;
    }

    if let Some(comparison) = &session.comparison {
        validate_comparison(comparison)?;
    }
    for entry in &session.timeline {
        if entry.origin != ValueOrigin::VerifiedFromSynapse
            || !matches!(
                entry.stage.as_str(),
                "original_observation"
                    | "current_observation"
                    | "image_import"
                    | "ai_proposal"
                    | "other"
            )
            || !matches!(entry.kind.as_str(), "observation" | "activity")
            || !matches!(
                entry.time_basis.as_str(),
                "observation_capture_instant"
                    | "observation_capture_interval"
                    | "observation_recorded_at_fallback"
                    | "activity_valid_instant"
                    | "activity_valid_interval"
                    | "activity_recorded_at_fallback"
            )
            || !is_canonical_timestamp(&entry.ordering_time)
            || parse_oid(&entry.oid).ok() != Some(ObjectKind::Record)
        {
            return invalid_public_profile("timeline entry is outside the verified source profile");
        }
    }
    if session.timeline.len() > CREATOR_FSCK_MAX_OBJECTS {
        return invalid_public_profile("timeline exceeds the publication work ceiling");
    }

    let provenance = &session.provenance;
    if provenance.origin != ValueOrigin::VerifiedFromSynapse
        || provenance.proposal_ref != format!("proposal/creator-agent/{}", session.session)
        || provenance.decision_ref != format!("decision/creator/{}", session.session)
        || parse_oid(&provenance.base_head).ok() != Some(ObjectKind::Commit)
        || parse_oid(&provenance.proposal_head).ok() != Some(ObjectKind::Commit)
        || parse_oid(&provenance.decision_head).ok() != Some(ObjectKind::Commit)
        || provenance.projection_source_fingerprint != source_fingerprint
        || provenance.fsck_objects_verified == 0
        || provenance.fsck_objects_verified > CREATOR_FSCK_MAX_OBJECTS
    {
        return invalid_public_profile(
            "technical provenance is outside the verified source profile",
        );
    }
    if session.value.origin != ValueOrigin::DerivedSummary
        || !session.value.decision_retrievable
        || !session.value.proposal_retained
        || !session.value.unselected_alternative_retained
        || !session.value.external_reader_can_distinguish_roles
    {
        return invalid_public_profile("session value summary is inconsistent");
    }
    Ok(())
}

fn validate_comparison(comparison: &ComparisonSummary) -> Result<()> {
    let expected_warning = match comparison.outcome.as_str() {
        "identical" => {
            "Identical Blob bytes do not establish that the observed physical subject was unchanged."
        }
        "different" => "Different Blob bytes do not establish visual or physical change.",
        _ => return invalid_public_profile("comparison outcome is unsupported"),
    };
    let reason_codes = comparison
        .reason_codes
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    if comparison.origin != ValueOrigin::VerifiedFromSynapse
        || comparison.analysis_kind != "primary_blob_byte_identity"
        || comparison.status != "succeeded"
        || comparison.comparability != "partial"
        || reason_codes
            != BTreeSet::from([
                "byte_identity_only",
                "capture_profile_imported",
                "capture_time_unknown",
            ])
        || comparison.reason_codes.len() != 3
        || comparison.warnings != [expected_warning]
        || !comparison.replay_ready
        || comparison.interpretation_limit != COMPARISON_INTERPRETATION_LIMIT
    {
        return invalid_public_profile("comparison is outside the conservative byte profile");
    }
    Ok(())
}

fn validate_presented_text(
    field: &str,
    text: &PresentedText,
    max_bytes: usize,
    multiline: bool,
    derived_default: Option<&str>,
) -> Result<()> {
    match text.origin {
        ValueOrigin::AuthorSupplied => validate_author_text(field, text, max_bytes, multiline),
        ValueOrigin::DerivedSummary if derived_default == Some(text.value.as_str()) => Ok(()),
        _ => invalid_public_profile("presented text has an invalid origin or derived value"),
    }
}

fn validate_author_text(
    field: &str,
    text: &PresentedText,
    max_bytes: usize,
    multiline: bool,
) -> Result<()> {
    if text.origin != ValueOrigin::AuthorSupplied {
        return invalid_public_profile("public author text is not marked author_supplied");
    }
    validate_optional_text(field, Some(&text.value), max_bytes, multiline)
        .map_err(|_| profile_error("public author text violates its bounded UTF-8 profile"))
}

fn valid_session_name(session: &str) -> bool {
    !session.is_empty()
        && session.len() <= 64
        && session.as_bytes()[0].is_ascii_lowercase()
        && session
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

fn valid_projection_fingerprint(value: &str) -> bool {
    value
        .strip_prefix("projection-source-v1:sha256:")
        .is_some_and(is_lower_sha256)
}

fn is_lower_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn safe_version_label(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'+'))
}

fn is_canonical_timestamp(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 30
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes[10] == b'T'
        && bytes[13] == b':'
        && bytes[16] == b':'
        && bytes[19] == b'.'
        && bytes[29] == b'Z'
        && bytes.iter().enumerate().all(|(index, byte)| {
            matches!(index, 4 | 7 | 10 | 13 | 16 | 19 | 29) || byte.is_ascii_digit()
        })
}

fn profile_error(message: impl Into<String>) -> PublicationError {
    PublicationError::InvalidBundle(message.into())
}

fn invalid_public_profile<T>(message: impl Into<String>) -> Result<T> {
    Err(profile_error(message))
}

fn validate_presentation(input: &PresentationInput) -> Result<()> {
    validate_optional_text("title", input.title.as_deref(), MAX_TITLE_BYTES, false)?;
    validate_optional_text("summary", input.summary.as_deref(), MAX_SUMMARY_BYTES, true)?;
    validate_optional_text(
        "creator_display_name",
        input.creator_display_name.as_deref(),
        MAX_TITLE_BYTES,
        false,
    )?;
    validate_optional_text(
        "proposal_agent_display_name",
        input.proposal_agent_display_name.as_deref(),
        MAX_TITLE_BYTES,
        false,
    )?;
    for (session, value) in &input.sessions {
        if !valid_session_name(session) {
            return Err(PublicationError::InvalidArgument(format!(
                "presentation session key {session:?} must match [a-z][a-z0-9-]{{0,63}}"
            )));
        }
        validate_optional_text(
            "session title",
            value.title.as_deref(),
            MAX_TITLE_BYTES,
            false,
        )?;
        validate_optional_text(
            "public_decision_note",
            value.public_decision_note.as_deref(),
            MAX_PUBLIC_NOTE_BYTES,
            true,
        )?;
        for (name, text) in [
            ("original_caption", value.original_caption.as_deref()),
            ("current_caption", value.current_caption.as_deref()),
            ("proposal_caption", value.proposal_caption.as_deref()),
        ] {
            validate_optional_text(name, text, MAX_CAPTION_BYTES, false)?;
        }
    }
    Ok(())
}

fn validate_optional_text(
    field: &str,
    value: Option<&str>,
    max_bytes: usize,
    multiline: bool,
) -> Result<()> {
    let Some(value) = value else {
        return Ok(());
    };
    if value.is_empty() || value.len() > max_bytes {
        return Err(PublicationError::InvalidArgument(format!(
            "{field} must contain 1 to {max_bytes} UTF-8 bytes"
        )));
    }
    if value.chars().any(|character| {
        (character.is_control()
            && !((multiline && matches!(character, '\n' | '\r')) || character == '\t'))
            || matches!(
                character,
                '\u{200b}'
                    ..='\u{200f}'
                        | '\u{202a}'
                            ..='\u{202e}'
                                | '\u{2060}'
                                    | '\u{2066}'..='\u{2069}'
                                    | '\u{feff}'
            )
    }) {
        return Err(PublicationError::InvalidArgument(format!(
            "{field} contains a forbidden control or direction-format character"
        )));
    }
    if !multiline && value.contains(['\n', '\r']) {
        return Err(PublicationError::InvalidArgument(format!(
            "{field} must be a single line"
        )));
    }
    Ok(())
}

fn validate_existing_repository_path(path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| PublicationError::io("inspect source repository", path, error))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(PublicationError::UnsafePath(format!(
            "source repository must be an existing real directory: {}",
            path.display()
        )));
    }
    Ok(())
}

fn validate_export_paths(source: &Path, destination: &Path) -> Result<PathBuf> {
    validate_existing_repository_path(source)?;
    match fs::symlink_metadata(destination) {
        Ok(_) => {
            return Err(PublicationError::DestinationExists(
                destination.to_path_buf(),
            ));
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(PublicationError::io(
                "inspect publication destination",
                destination,
                error,
            ));
        }
    }
    if destination.as_os_str().is_empty() || destination.file_name().is_none() {
        return Err(PublicationError::UnsafePath(
            "publication destination must name a new directory".into(),
        ));
    }
    if destination
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(PublicationError::UnsafePath(
            "publication destination must not contain '..'".into(),
        ));
    }
    let parent = destination
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    reject_symlink_components(parent)?;
    require_real_directory(parent, "publication parent")?;
    let source = fs::canonicalize(source)
        .map_err(|error| PublicationError::io("canonicalize source repository", source, error))?;
    let parent = fs::canonicalize(parent)
        .map_err(|error| PublicationError::io("canonicalize publication parent", parent, error))?;
    let destination = parent.join(
        destination
            .file_name()
            .expect("destination file name was validated"),
    );
    if destination.starts_with(&source) {
        return Err(PublicationError::UnsafePath(
            "publication destination must not be the source repository or one of its descendants"
                .into(),
        ));
    }
    Ok(destination)
}

fn reject_symlink_components(path: &Path) -> Result<()> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|error| PublicationError::io("read current directory", ".", error))?
            .join(path)
    };
    let mut current = PathBuf::new();
    for component in absolute.components() {
        current.push(component.as_os_str());
        if matches!(component, Component::RootDir | Component::Prefix(_)) {
            continue;
        }
        let metadata = fs::symlink_metadata(&current).map_err(|error| {
            PublicationError::io("inspect publication parent component", &current, error)
        })?;
        if metadata.file_type().is_symlink() {
            return Err(PublicationError::UnsafePath(format!(
                "publication parent contains a symlink component: {}",
                current.display()
            )));
        }
    }
    Ok(())
}

fn publish_files_atomically(destination: &Path, files: &BTreeMap<String, Vec<u8>>) -> Result<()> {
    let parent = destination
        .parent()
        .expect("validated destination has a parent");
    let nonce = NEXT_STAGING.fetch_add(1, Ordering::Relaxed);
    let staging = staging_path(destination, nonce);
    fs::create_dir(&staging).map_err(|error| {
        PublicationError::io("create publication staging directory", &staging, error)
    })?;
    let mut guard = StagingGuard {
        path: staging.clone(),
        armed: true,
    };
    for (relative, bytes) in files {
        validate_bundle_relative_path(relative)?;
        if bytes.len() as u64 > MAX_BUNDLE_FILE_BYTES {
            return Err(PublicationError::InvalidBundle(format!(
                "generated bundle file {relative:?} exceeds the {MAX_BUNDLE_FILE_BYTES} byte limit"
            )));
        }
        let path = staging.join(relative);
        if let Some(directory) = path.parent() {
            ensure_staging_directory(&staging, directory)?;
        }
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&path)
            .map_err(|error| {
                PublicationError::io("create publication bundle file", &path, error)
            })?;
        file.write_all(bytes)
            .map_err(|error| PublicationError::io("write publication bundle file", &path, error))?;
        file.sync_all()
            .map_err(|error| PublicationError::io("sync publication bundle file", &path, error))?;
    }
    sync_tree_directories(&staging)?;
    rename_directory_no_replace(&staging, destination)?;
    guard.armed = false;
    sync_directory(parent)?;
    Ok(())
}

fn ensure_staging_directory(staging: &Path, directory: &Path) -> Result<()> {
    if directory == staging {
        return Ok(());
    }
    if directory != staging.join("target") {
        return Err(PublicationError::InvalidBundle(
            "bundle writer requested an unsupported directory".into(),
        ));
    }
    match fs::create_dir(directory) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            require_real_directory(directory, "publication staging directory")
        }
        Err(error) => Err(PublicationError::io(
            "create publication bundle directory",
            directory,
            error,
        )),
    }
}

struct StagingGuard {
    path: PathBuf,
    armed: bool,
}

impl Drop for StagingGuard {
    fn drop(&mut self) {
        if self.armed {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}

fn staging_path(destination: &Path, nonce: u64) -> PathBuf {
    let mut name = OsString::from(".");
    name.push(
        destination
            .file_name()
            .unwrap_or_else(|| OsStr::new("publication")),
    );
    name.push(format!(".tmp-{}-{nonce}", std::process::id()));
    destination
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(name)
}

#[cfg(any(
    target_os = "linux",
    target_os = "android",
    target_vendor = "apple",
    target_os = "redox"
))]
fn rename_directory_no_replace(source: &Path, destination: &Path) -> Result<()> {
    use rustix::fs::{CWD, RenameFlags, renameat_with};
    use rustix::io::Errno;

    match renameat_with(CWD, source, CWD, destination, RenameFlags::NOREPLACE) {
        Ok(()) => Ok(()),
        Err(Errno::EXIST) => Err(PublicationError::DestinationExists(
            destination.to_path_buf(),
        )),
        Err(error) => Err(PublicationError::io(
            "publish bundle without replacement",
            destination,
            io::Error::from_raw_os_error(error.raw_os_error()),
        )),
    }
}

#[cfg(not(any(
    target_os = "linux",
    target_os = "android",
    target_vendor = "apple",
    target_os = "redox"
)))]
fn rename_directory_no_replace(_source: &Path, destination: &Path) -> Result<()> {
    Err(PublicationError::io(
        "publish bundle without replacement",
        destination,
        io::Error::new(
            io::ErrorKind::Unsupported,
            "this platform has no supported atomic directory no-replace primitive",
        ),
    ))
}

fn sync_tree_directories(root: &Path) -> Result<()> {
    let target = root.join("target");
    if target.is_dir() {
        sync_directory(&target)?;
    }
    sync_directory(root)
}

#[cfg(unix)]
fn sync_directory(path: &Path) -> Result<()> {
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| PublicationError::io("sync publication directory", path, error))
}

#[cfg(not(unix))]
fn sync_directory(_path: &Path) -> Result<()> {
    Ok(())
}

fn require_real_directory(path: &Path, label: &str) -> Result<()> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| PublicationError::io("inspect directory", path, error))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(PublicationError::UnsafePath(format!(
            "{label} must be a real directory: {}",
            path.display()
        )));
    }
    Ok(())
}

fn read_regular_file(path: &Path) -> Result<Vec<u8>> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| PublicationError::io("inspect bundle file", path, error))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(PublicationError::InvalidBundle(format!(
            "bundle path is not a regular file: {}",
            path.display()
        )));
    }
    if metadata.len() > MAX_BUNDLE_FILE_BYTES {
        return Err(PublicationError::InvalidBundle(format!(
            "bundle file exceeds the {MAX_BUNDLE_FILE_BYTES} byte limit: {}",
            path.display()
        )));
    }
    let bytes =
        fs::read(path).map_err(|error| PublicationError::io("read bundle file", path, error))?;
    if bytes.len() as u64 > MAX_BUNDLE_FILE_BYTES {
        return Err(PublicationError::InvalidBundle(format!(
            "bundle file grew beyond the {MAX_BUNDLE_FILE_BYTES} byte limit: {}",
            path.display()
        )));
    }
    Ok(bytes)
}

fn collect_bundle_files(root: &Path) -> Result<BTreeSet<String>> {
    let mut files = BTreeSet::new();
    collect_bundle_files_from(root, root, &mut files)?;
    Ok(files)
}

fn collect_bundle_files_from(
    root: &Path,
    directory: &Path,
    files: &mut BTreeSet<String>,
) -> Result<()> {
    let mut entries = Vec::new();
    for entry in fs::read_dir(directory)
        .map_err(|error| PublicationError::io("list bundle directory", directory, error))?
    {
        let entry = entry
            .map_err(|error| PublicationError::io("list bundle directory", directory, error))?;
        if entries.len() == MAX_BUNDLE_DIRECTORY_ENTRIES {
            return Err(PublicationError::InvalidBundle(format!(
                "bundle directory has more than {MAX_BUNDLE_DIRECTORY_ENTRIES} entries: {}",
                directory.display()
            )));
        }
        entries.push(entry);
    }
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path)
            .map_err(|error| PublicationError::io("inspect bundle inventory", &path, error))?;
        if metadata.file_type().is_symlink() {
            return Err(PublicationError::InvalidBundle(format!(
                "bundle inventory contains a symlink: {}",
                path.display()
            )));
        }
        if metadata.is_dir() {
            let relative = path.strip_prefix(root).expect("walk is rooted in bundle");
            if relative != Path::new("target") {
                return Err(PublicationError::InvalidBundle(format!(
                    "bundle contains an unsupported directory: {}",
                    path.display()
                )));
            }
            collect_bundle_files_from(root, &path, files)?;
        } else if metadata.is_file() {
            let relative = path.strip_prefix(root).expect("walk is rooted in bundle");
            let relative = path_to_slash(relative)?;
            validate_bundle_relative_path(&relative)?;
            files.insert(relative);
        } else {
            return Err(PublicationError::InvalidBundle(format!(
                "bundle inventory contains a non-file entry: {}",
                path.display()
            )));
        }
    }
    Ok(())
}

fn path_to_slash(path: &Path) -> Result<String> {
    let mut parts = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(value) => parts.push(value.to_str().ok_or_else(|| {
                PublicationError::InvalidBundle("bundle path is not valid UTF-8".into())
            })?),
            _ => {
                return Err(PublicationError::InvalidBundle(
                    "bundle path is not relative and normalized".into(),
                ));
            }
        }
    }
    Ok(parts.join("/"))
}

fn validate_bundle_relative_path(path: &str) -> Result<()> {
    if path.is_empty()
        || path.starts_with('/')
        || path.contains('\\')
        || path
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return Err(PublicationError::InvalidBundle(format!(
            "unsafe bundle path {path:?}"
        )));
    }
    if !path
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'.' | b'-' | b'_'))
    {
        return Err(PublicationError::InvalidBundle(format!(
            "bundle path is outside the fixed ASCII profile: {path:?}"
        )));
    }
    Ok(())
}

fn canonical_json_bytes<T: Serialize>(value: &T) -> Result<Vec<u8>> {
    let ordinary = serde_json::to_vec(value)?;
    let parsed = parse_strict(&ordinary)?;
    Ok(canonical_bytes(&parsed)?)
}

fn ref_snapshot_sha256<'a>(records: impl IntoIterator<Item = (&'a str, &'a str, i64)>) -> String {
    let mut digest = Sha256::new();
    digest.update(b"synapsegit-public-ref-snapshot-v1\0");
    for (name, head, updated_event_id) in records {
        update_length_prefixed(&mut digest, name.as_bytes());
        update_length_prefixed(&mut digest, head.as_bytes());
        digest.update(updated_event_id.to_be_bytes());
    }
    lower_hex(digest.finalize())
}

fn update_length_prefixed(digest: &mut Sha256, value: &[u8]) {
    digest.update((value.len() as u64).to_be_bytes());
    digest.update(value);
}

fn sha256(bytes: &[u8]) -> String {
    lower_hex(Sha256::digest(bytes))
}

fn lower_hex(bytes: impl AsRef<[u8]>) -> String {
    let bytes = bytes.as_ref();
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        write!(output, "{byte:02x}").expect("writing to String cannot fail");
    }
    output
}

fn schema(name: &str, version: u32) -> SchemaIdentity {
    SchemaIdentity {
        name: name.into(),
        version,
    }
}

fn generator() -> GeneratorIdentity {
    GeneratorIdentity {
        name: GENERATOR_NAME.into(),
        version: env!("CARGO_PKG_VERSION").into(),
    }
}

fn limitation(code: &str, message: &str) -> Limitation {
    Limitation {
        code: code.into(),
        message: message.into(),
    }
}

fn author_text(value: Option<&String>) -> Option<PresentedText> {
    value.map(|value| PresentedText {
        value: value.clone(),
        origin: ValueOrigin::AuthorSupplied,
    })
}
