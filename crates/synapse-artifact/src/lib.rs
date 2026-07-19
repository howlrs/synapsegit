//! Provider-neutral, bounded regular-file artifact mapping for SynapseGit.

#![forbid(unsafe_code)]

use serde::{Deserialize, Serialize};
use serde_json::{Map as JsonMap, Value as JsonValue, json};
use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;
use std::io::Cursor;
use synapse_core::{Repository, RepositoryError};
use unicode_normalization::UnicodeNormalization;

mod approval;
mod checkout;
mod durable;
mod workflow;

pub use approval::{ArtifactApprovalError, ArtifactApprovalRegistry, ArtifactDecisionApproval};
pub use checkout::{
    ArtifactCheckoutError, ArtifactCheckoutLimits, CheckedOutArtifact, CheckoutResult,
    TrustedArtifactDecisionBinding, checkout_artifact_decision,
};
pub use durable::{
    ArtifactReviewId, DurableArtifactCheckoutState, DurableArtifactDecisionStatus,
    DurableArtifactError, DurableArtifactProposalRecovery, DurableArtifactResult,
    DurableArtifactReviewState, DurableArtifactReviewStatus, DurableArtifactSelectedSnapshot,
    DurablePendingArtifactReview, PreparedDurableArtifactDecision, PreparedDurableArtifactProposal,
    PublishedDurableArtifactDecision, PublishedDurableArtifactProposal,
    ReconciledDurableArtifactReview, commit_published_durable_artifact_decision,
    commit_published_durable_artifact_proposal, get_durable_artifact_review_status,
    prepare_durable_artifact_decision, prepare_durable_artifact_proposal,
    publish_prepared_durable_artifact_decision, publish_prepared_durable_artifact_proposal,
    reconcile_durable_artifact_review, recover_durable_artifact_proposal,
    recover_durable_artifact_review,
};
pub use synapse_schema::{
    CanonicalTimestamp, CanonicalTimestampError, CanonicalTimestampErrorKind, ScaledInteger,
    ScaledIntegerError, ScaledIntegerErrorKind, Unit,
};
pub use workflow::{
    ArtifactDecisionOptions, ArtifactDecisionPublication, ArtifactDecisionReceipt,
    ArtifactProposalReceipt, PendingArtifactProposal, PendingArtifactState,
    PreparedArtifactDecision, PreparedArtifactProposal, TrustedArtifactProjectConfig,
    WorkflowError, artifact_manifest_sha256, begin_artifact_proposal, begin_next_artifact_proposal,
    begin_next_artifact_proposal_at_head, decide_artifact_proposal, prepare_artifact_decision,
    prepare_artifact_proposal, prepare_next_artifact_proposal,
    prepare_next_artifact_proposal_at_head, publish_prepared_artifact_decision,
    publish_prepared_artifact_proposal, recover_prepared_artifact_proposal,
    recover_published_artifact_proposal, review_context_sha256,
};

pub const CONTRACT_NAME: &str = "synapsegit.generic-artifact";
pub const CONTRACT_VERSION: u32 = 1;
pub const PATH_PROFILE: &str = "relative-nfc-portable-v1";

/// Versioned capabilities safe to expose to a sibling application.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactCapabilitiesV1 {
    pub contract: String,
    pub contract_version: u32,
    pub path_profile: String,
    pub supported_entry_kinds: Vec<String>,
    pub supported_dispositions: Vec<String>,
    pub source_attributions: Vec<String>,
    pub mapper_writes_refs: bool,
}

pub fn capabilities_v1() -> ArtifactCapabilitiesV1 {
    ArtifactCapabilitiesV1 {
        contract: CONTRACT_NAME.into(),
        contract_version: CONTRACT_VERSION,
        path_profile: PATH_PROFILE.into(),
        supported_entry_kinds: vec!["regular_file".into()],
        supported_dispositions: vec![
            "adopted_unchanged".into(),
            "rejected".into(),
            "deferred".into(),
        ],
        source_attributions: vec!["caller_supplied_ai_attributed".into()],
        mapper_writes_refs: false,
    }
}

/// Attribution metadata. It is not proof of model execution.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactSourceAttribution {
    CallerSuppliedAiAttributed,
}

impl ArtifactSourceAttribution {
    pub const fn execution_verified(self) -> bool {
        false
    }
}

/// The only Human dispositions supported by the v1 generic contract.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactDisposition {
    AdoptedUnchanged,
    Rejected,
    Deferred,
}

/// Host entry classification supplied by a trusted collector.
///
/// Directories are implicit in regular-file paths. Every non-regular kind is
/// representable so the mapper can reject it before writing any object.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ArtifactEntryKind {
    RegularFile,
    Directory,
    Symlink,
    Socket,
    Fifo,
    Device,
    Other,
}

#[derive(Clone, Eq, PartialEq)]
pub struct ArtifactManifestEntry {
    path: String,
    kind: ArtifactEntryKind,
    bytes: Box<[u8]>,
}

impl fmt::Debug for ArtifactManifestEntry {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ArtifactManifestEntry")
            .field("path", &"<redacted>")
            .field("kind", &self.kind)
            .field("byte_len", &self.bytes.len())
            .finish()
    }
}

impl ArtifactManifestEntry {
    pub fn regular_file(path: impl Into<String>, bytes: impl Into<Box<[u8]>>) -> Self {
        Self {
            path: path.into(),
            kind: ArtifactEntryKind::RegularFile,
            bytes: bytes.into(),
        }
    }

    pub fn unsupported(path: impl Into<String>, kind: ArtifactEntryKind) -> Self {
        Self {
            path: path.into(),
            kind,
            bytes: Box::new([]),
        }
    }

    pub fn path(&self) -> &str {
        &self.path
    }

    pub const fn kind(&self) -> ArtifactEntryKind {
        self.kind
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ArtifactLimits {
    pub max_files: usize,
    pub max_file_bytes: u64,
    pub max_total_bytes: u64,
    pub max_path_bytes: usize,
    pub max_depth: usize,
}

impl Default for ArtifactLimits {
    fn default() -> Self {
        Self {
            max_files: 10_000,
            max_file_bytes: 64 * 1024 * 1024,
            max_total_bytes: 512 * 1024 * 1024,
            max_path_bytes: 1_024,
            max_depth: 32,
        }
    }
}

/// Validated manifest. Construction performs no CAS or Ref mutation.
#[derive(Clone, Eq, PartialEq)]
pub struct RegularFileManifest {
    files: BTreeMap<String, Box<[u8]>>,
    total_bytes: u64,
}

impl fmt::Debug for RegularFileManifest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RegularFileManifest")
            .field("file_count", &self.files.len())
            .field("total_bytes", &self.total_bytes)
            .field("paths", &"<redacted>")
            .field("contents", &"<redacted>")
            .finish()
    }
}

impl RegularFileManifest {
    pub fn from_entries(
        entries: impl IntoIterator<Item = ArtifactManifestEntry>,
        limits: ArtifactLimits,
    ) -> Result<Self> {
        validate_limits(limits)?;
        let mut files = BTreeMap::new();
        let mut total_bytes = 0_u64;
        let mut child_spellings: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();

        for entry in entries {
            if entry.kind != ArtifactEntryKind::RegularFile {
                return Err(ArtifactError::UnsupportedEntryKind {
                    path: entry.path,
                    kind: entry.kind,
                });
            }
            let components = validate_path(&entry.path, limits)?;
            let normalized_path = components.join("/");
            if files.contains_key(&normalized_path) {
                return Err(ArtifactError::DuplicatePath(normalized_path));
            }
            let byte_len = u64::try_from(entry.bytes.len())
                .map_err(|_| ArtifactError::ResourceLimit("file size exceeds u64"))?;
            if byte_len > limits.max_file_bytes {
                return Err(ArtifactError::ResourceLimit(
                    "regular file exceeds max_file_bytes",
                ));
            }
            total_bytes = total_bytes
                .checked_add(byte_len)
                .ok_or(ArtifactError::ResourceLimit("aggregate bytes overflow"))?;
            if total_bytes > limits.max_total_bytes {
                return Err(ArtifactError::ResourceLimit(
                    "manifest exceeds max_total_bytes",
                ));
            }

            for index in 0..components.len() {
                let parent = components[..index].join("/");
                let spelling = &components[index];
                let collision_key = lowercase_key(spelling);
                if let Some(existing) = child_spellings
                    .entry(parent)
                    .or_default()
                    .insert(collision_key, spelling.clone())
                    .filter(|existing| existing != spelling)
                {
                    return Err(ArtifactError::PathCollision {
                        first: existing,
                        second: spelling.clone(),
                    });
                }
            }
            files.insert(normalized_path, entry.bytes);
            if files.len() > limits.max_files {
                return Err(ArtifactError::ResourceLimit("manifest exceeds max_files"));
            }
        }

        let paths = files.keys().cloned().collect::<BTreeSet<_>>();
        for path in &paths {
            let components = path.split('/').collect::<Vec<_>>();
            for prefix_len in 1..components.len() {
                let prefix = components[..prefix_len].join("/");
                if paths.contains(&prefix) {
                    return Err(ArtifactError::FileDirectoryConflict {
                        file: prefix,
                        descendant: path.clone(),
                    });
                }
            }
        }

        Ok(Self { files, total_bytes })
    }

    pub fn file_count(&self) -> usize {
        self.files.len()
    }

    pub const fn total_bytes(&self) -> u64 {
        self.total_bytes
    }

    pub fn paths(&self) -> impl ExactSizeIterator<Item = &str> {
        self.files.keys().map(String::as_str)
    }
}

#[derive(Clone, Eq, PartialEq)]
pub struct MappedArtifactFile {
    pub normalized_path: String,
    pub blob_oid: String,
    pub byte_len: u64,
}

impl fmt::Debug for MappedArtifactFile {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MappedArtifactFile")
            .field("normalized_path", &"<redacted>")
            .field("blob_oid", &"<redacted>")
            .field("byte_len", &self.byte_len)
            .finish()
    }
}

/// Server-side mapper receipt. It is not the public Proposal receipt.
#[derive(Clone, Eq, PartialEq)]
pub struct MappedArtifactTree {
    pub contract: &'static str,
    pub contract_version: u32,
    pub path_profile: &'static str,
    pub site_tree_oid: String,
    pub files: Vec<MappedArtifactFile>,
    pub total_bytes: u64,
}

impl fmt::Debug for MappedArtifactTree {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MappedArtifactTree")
            .field("contract", &self.contract)
            .field("contract_version", &self.contract_version)
            .field("path_profile", &self.path_profile)
            .field("site_tree_oid", &"<redacted>")
            .field("file_count", &self.files.len())
            .field("total_bytes", &self.total_bytes)
            .finish()
    }
}

/// Map a fully validated manifest to nested Core Blob and ManifestTree objects.
///
/// This function never updates a Ref. CAS writes are append-only; an I/O error
/// can leave harmless unreachable objects but cannot expose a partial Tree.
pub fn map_regular_files(
    repository: &Repository,
    manifest: &RegularFileManifest,
) -> Result<MappedArtifactTree> {
    let mut root = DirectoryNode::default();
    let mut mapped = Vec::with_capacity(manifest.files.len());
    for (path, bytes) in &manifest.files {
        let stored = repository.put_blob(Cursor::new(bytes.as_ref()))?;
        root.insert(path, &stored.oid)?;
        mapped.push(MappedArtifactFile {
            normalized_path: path.clone(),
            blob_oid: stored.oid,
            byte_len: u64::try_from(bytes.len())
                .map_err(|_| ArtifactError::ResourceLimit("file size exceeds u64"))?,
        });
    }
    let site_tree_oid = root.store(repository)?;
    Ok(MappedArtifactTree {
        contract: CONTRACT_NAME,
        contract_version: CONTRACT_VERSION,
        path_profile: PATH_PROFILE,
        site_tree_oid,
        files: mapped,
        total_bytes: manifest.total_bytes,
    })
}

#[derive(Default)]
struct DirectoryNode {
    blobs: BTreeMap<String, String>,
    directories: BTreeMap<String, DirectoryNode>,
}

impl DirectoryNode {
    fn insert(&mut self, path: &str, blob_oid: &str) -> Result<()> {
        let mut components = path.split('/');
        let first = components
            .next()
            .ok_or_else(|| ArtifactError::InvalidPath(path.into()))?;
        let rest = components.collect::<Vec<_>>();
        if rest.is_empty() {
            self.blobs.insert(first.into(), blob_oid.into());
        } else {
            self.directories
                .entry(first.into())
                .or_default()
                .insert(&rest.join("/"), blob_oid)?;
        }
        Ok(())
    }

    fn store(&self, repository: &Repository) -> Result<String> {
        let mut entries = JsonMap::new();
        for (name, oid) in &self.blobs {
            entries.insert(name.clone(), json!({"entry_kind": "blob", "oid": oid}));
        }
        for (name, directory) in &self.directories {
            let oid = directory.store(repository)?;
            entries.insert(name.clone(), json!({"entry_kind": "tree", "oid": oid}));
        }
        let value = json!({
            "object_type": "tree",
            "schema_version": "0.1.0",
            "entries": JsonValue::Object(entries),
            "extensions": {},
        });
        Ok(repository.put_object(&serde_json::to_vec(&value)?)?.oid)
    }
}

pub enum ArtifactError {
    InvalidLimits(&'static str),
    InvalidPath(String),
    PathNotNfc(String),
    UnsupportedEntryKind {
        path: String,
        kind: ArtifactEntryKind,
    },
    DuplicatePath(String),
    PathCollision {
        first: String,
        second: String,
    },
    FileDirectoryConflict {
        file: String,
        descendant: String,
    },
    ResourceLimit(&'static str),
    Repository(RepositoryError),
    Json(serde_json::Error),
}

impl fmt::Debug for ArtifactError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ArtifactError")
            .field("code", &self.code())
            .field("detail", &"<redacted>")
            .finish()
    }
}

impl ArtifactError {
    pub fn code(&self) -> &str {
        match self {
            Self::InvalidLimits(_) => "artifact_limits_invalid",
            Self::InvalidPath(_) | Self::PathNotNfc(_) => "artifact_path_invalid",
            Self::UnsupportedEntryKind { .. } => "artifact_entry_unsupported",
            Self::DuplicatePath(_)
            | Self::PathCollision { .. }
            | Self::FileDirectoryConflict { .. } => "artifact_path_collision",
            Self::ResourceLimit(_) => "resource_limit",
            Self::Repository(error) => error.code(),
            Self::Json(_) => "schema_invalid",
        }
    }
}

impl fmt::Display for ArtifactError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidLimits(message) | Self::ResourceLimit(message) => {
                formatter.write_str(message)
            }
            Self::InvalidPath(_) => formatter.write_str("artifact path is invalid"),
            Self::PathNotNfc(_) => formatter.write_str("artifact path is not NFC"),
            Self::UnsupportedEntryKind { kind, .. } => {
                write!(formatter, "artifact entry kind is unsupported: {kind:?}")
            }
            Self::DuplicatePath(_) => formatter.write_str("artifact path is duplicated"),
            Self::PathCollision { .. } => formatter.write_str("artifact path spellings collide"),
            Self::FileDirectoryConflict { .. } => {
                formatter.write_str("artifact file conflicts with a descendant path")
            }
            Self::Repository(error) => {
                write!(
                    formatter,
                    "artifact repository operation failed ({})",
                    error.code()
                )
            }
            Self::Json(_) => formatter.write_str("artifact JSON serialization failed"),
        }
    }
}

impl Error for ArtifactError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Repository(error) => Some(error),
            Self::Json(error) => Some(error),
            _ => None,
        }
    }
}

impl From<RepositoryError> for ArtifactError {
    fn from(error: RepositoryError) -> Self {
        Self::Repository(error)
    }
}

impl From<serde_json::Error> for ArtifactError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

pub type Result<T> = std::result::Result<T, ArtifactError>;

fn validate_limits(limits: ArtifactLimits) -> Result<()> {
    if limits.max_files == 0 {
        return Err(ArtifactError::InvalidLimits("max_files must be positive"));
    }
    if limits.max_file_bytes == 0 || limits.max_total_bytes == 0 {
        return Err(ArtifactError::InvalidLimits("byte limits must be positive"));
    }
    if limits.max_path_bytes == 0 || limits.max_depth == 0 {
        return Err(ArtifactError::InvalidLimits("path limits must be positive"));
    }
    Ok(())
}

fn validate_path(path: &str, limits: ArtifactLimits) -> Result<Vec<String>> {
    if path.is_empty()
        || path.starts_with('/')
        || path.ends_with('/')
        || path.contains('\\')
        || path
            .bytes()
            .any(|byte| byte == 0 || byte.is_ascii_control())
        || looks_like_windows_absolute(path)
    {
        return Err(ArtifactError::InvalidPath(path.into()));
    }
    if path.len() > limits.max_path_bytes {
        return Err(ArtifactError::ResourceLimit(
            "artifact path exceeds max_path_bytes",
        ));
    }
    let components = path.split('/').map(str::to_owned).collect::<Vec<_>>();
    if components.len() > limits.max_depth {
        return Err(ArtifactError::ResourceLimit(
            "artifact path exceeds max_depth",
        ));
    }
    if components
        .iter()
        .any(|component| component.is_empty() || component == "." || component == "..")
    {
        return Err(ArtifactError::InvalidPath(path.into()));
    }
    if components
        .iter()
        .any(|component| component.nfc().collect::<String>() != *component)
    {
        return Err(ArtifactError::PathNotNfc(path.into()));
    }
    if components
        .iter()
        .any(|component| !portable_component(component))
    {
        return Err(ArtifactError::InvalidPath(path.into()));
    }
    Ok(components)
}

fn looks_like_windows_absolute(path: &str) -> bool {
    let bytes = path.as_bytes();
    bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':'
}

fn lowercase_key(value: &str) -> String {
    value.chars().flat_map(char::to_lowercase).collect()
}

fn portable_component(component: &str) -> bool {
    if component.ends_with(['.', ' '])
        || component
            .chars()
            .any(|character| matches!(character, '<' | '>' | ':' | '"' | '|' | '?' | '*'))
        || component.chars().any(is_bidi_control)
    {
        return false;
    }
    let stem = component.split('.').next().unwrap_or(component);
    let uppercase = stem.to_ascii_uppercase();
    !matches!(uppercase.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        && !matches!(
            uppercase.as_bytes(),
            [b'C', b'O', b'M', b'1'..=b'9'] | [b'L', b'P', b'T', b'1'..=b'9']
        )
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
