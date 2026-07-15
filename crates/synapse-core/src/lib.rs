//! Integrated local SynapseGit Core repository.
//!
//! This crate is the production boundary joining strict schema ingestion,
//! immutable filesystem CAS, graph closure verification, transactional Refs,
//! and a self-verifying directory archive.

#![forbid(unsafe_code)]

mod authorization;
mod human_decision;

pub use authorization::{
    AiCapability, AiExecutionAuthority, AiGeneratedProposal, AiPreflightDecision, AiProposalUpdate,
    AiPublicationTarget, AiSideEffectClass, AuthorizationClock, AuthorizationDecision,
    CreativeAiRuntime, SystemAuthorizationClock,
};
pub use human_decision::{
    DecisionDisposition, HumanDecisionAuthority, HumanDecisionReceipt, HumanDecisionRuntime,
    HumanDecisionUpdate,
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::cell::RefCell;
use std::collections::HashSet;
use std::error::Error;
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use synapse_canonical::{CoreError, ErrorCode, ObjectKind, parse_oid};
pub use synapse_cas::TombstoneScanLimits;
use synapse_cas::{
    ClosureIssueKind, ClosureReport, FileObjectStore, FsckIssue, FsckIssueKind, FsckReport,
    GraphLimits, PreparedClosureVerifier, PutResult, StoreError, StoreLimits, fsck,
};
use synapse_schema::{ingest, ingest_claimed};
use synapse_sqlite::{
    RefArchive, RefRecord, RefStoreError, RefUpdate, ReflogEntry, SqliteRefStore, ValidationError,
    validate_commit_oid,
};
pub use synapse_sqlite::{RefArchiveExportLimits, RefSnapshot};

const ARCHIVE_FORMAT: &str = "synapsegit-core-archive-v0.1";
const MANIFEST_FILE: &str = "manifest.json";
const MANIFEST_CHECKSUM_FILE: &str = "manifest.sha256";
const MAX_MANIFEST_BYTES: u64 = 64 * 1024 * 1024;
/// Default maximum number of immutable objects copied by one archive export.
pub const DEFAULT_MAX_ARCHIVE_OBJECTS: usize = 100_000;
/// Default maximum cumulative raw object bytes copied by one archive export.
pub const DEFAULT_MAX_ARCHIVE_OBJECT_BYTES: u64 = 1024 * 1024 * 1024 * 1024;
/// Default maximum cumulative nodes visited while validating distinct heads.
pub const DEFAULT_MAX_ARCHIVE_HEAD_VALIDATION_NODES: usize = 1_000_000;
/// Default maximum cumulative edges visited while validating distinct heads.
pub const DEFAULT_MAX_ARCHIVE_HEAD_VALIDATION_EDGES: usize = 10_000_000;
/// Default maximum current Ref roots checked by one bounded fsck operation.
pub const DEFAULT_MAX_FSCK_REF_ROOTS: usize = 100_000;
/// Default maximum complete CAS inventory size checked by bounded fsck.
pub const DEFAULT_MAX_FSCK_OBJECTS: usize = 100_000;
/// Default maximum cumulative inventoried raw object bytes checked by bounded fsck.
pub const DEFAULT_MAX_FSCK_OBJECT_BYTES: u64 = 1024 * 1024 * 1024 * 1024;
/// Default maximum cumulative closure nodes visited across distinct Ref heads.
pub const DEFAULT_MAX_FSCK_CLOSURE_NODES: usize = 1_000_000;
/// Default maximum cumulative closure edges visited across distinct Ref heads.
pub const DEFAULT_MAX_FSCK_CLOSURE_EDGES: usize = 10_000_000;

/// Hard resource limits for one bounded repository consistency check.
///
/// `max_ref_roots` is applied to the complete supplied Ref snapshot before
/// duplicate heads are removed. Closure node and edge limits are cumulative
/// across the resulting distinct heads and therefore bound actual traversal
/// work. Object count and byte limits cover the complete CAS inventory,
/// including unreachable objects.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FsckLimits {
    /// Maximum number of Ref records in the checked snapshot.
    pub max_ref_roots: usize,
    /// Maximum number of objects in the complete CAS inventory.
    pub max_objects: usize,
    /// Maximum cumulative regular-file bytes in the complete CAS inventory.
    pub max_object_bytes: u64,
    /// Maximum cumulative closure nodes visited across distinct heads.
    pub max_closure_nodes: usize,
    /// Maximum cumulative closure edges visited across distinct heads.
    pub max_closure_edges: usize,
    /// Limits for the one shared Tombstone Record catalog used by all heads.
    pub tombstone_scan: TombstoneScanLimits,
}

impl Default for FsckLimits {
    fn default() -> Self {
        Self {
            max_ref_roots: DEFAULT_MAX_FSCK_REF_ROOTS,
            max_objects: DEFAULT_MAX_FSCK_OBJECTS,
            max_object_bytes: DEFAULT_MAX_FSCK_OBJECT_BYTES,
            max_closure_nodes: DEFAULT_MAX_FSCK_CLOSURE_NODES,
            max_closure_edges: DEFAULT_MAX_FSCK_CLOSURE_EDGES,
            tombstone_scan: TombstoneScanLimits::default(),
        }
    }
}

/// Resource limits for one local directory archive export.
///
/// Object count and byte limits are inclusive and cover every exported CAS
/// object, including objects that are not reachable from a current Ref. These
/// are caller-controlled deployment limits; [`Default`] supplies the local
/// profile values rather than immutable protocol hard ceilings.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ArchiveExportLimits {
    /// Maximum complete CAS inventory size.
    pub max_objects: usize,
    /// Maximum cumulative raw bytes copied from all inventoried objects.
    pub max_object_bytes: u64,
    /// Maximum nodes visited across all distinct current and historical heads.
    /// Shared closure nodes are charged again when another head re-traverses
    /// them, so this bounds actual validation work rather than unique objects.
    pub max_head_validation_nodes: usize,
    /// Maximum edges visited across all distinct current and historical heads.
    /// Shared closure edges are charged once per traversal.
    pub max_head_validation_edges: usize,
    /// Limits for the one shared Tombstone Record catalog used by head checks.
    pub tombstone_scan: TombstoneScanLimits,
    /// Limits for the consistent SQLite Ref/reflog snapshot.
    pub ref_archive: RefArchiveExportLimits,
}

impl Default for ArchiveExportLimits {
    fn default() -> Self {
        Self {
            max_objects: DEFAULT_MAX_ARCHIVE_OBJECTS,
            max_object_bytes: DEFAULT_MAX_ARCHIVE_OBJECT_BYTES,
            max_head_validation_nodes: DEFAULT_MAX_ARCHIVE_HEAD_VALIDATION_NODES,
            max_head_validation_edges: DEFAULT_MAX_ARCHIVE_HEAD_VALIDATION_EDGES,
            tombstone_scan: TombstoneScanLimits::default(),
            ref_archive: RefArchiveExportLimits::default(),
        }
    }
}

#[derive(Debug)]
pub enum RepositoryError {
    Core(CoreError),
    Store(StoreError),
    RefStore(RefStoreError),
    Io {
        operation: &'static str,
        path: PathBuf,
        source: io::Error,
    },
    ArchiveInvalid(String),
    ArchiveDestinationExists(PathBuf),
    RepositoryNotEmpty,
    Json(serde_json::Error),
    Clock(String),
}

impl RepositoryError {
    pub fn code(&self) -> &str {
        match self {
            Self::Core(error) => error.code().as_str(),
            Self::Store(error) => error.code().map_or("storage_error", ErrorCode::as_str),
            Self::RefStore(error) => error.code(),
            Self::Io { .. } => "storage_error",
            Self::ArchiveInvalid(_) | Self::ArchiveDestinationExists(_) => "archive_invalid",
            Self::RepositoryNotEmpty => "archive_not_empty",
            Self::Json(_) => "archive_invalid",
            Self::Clock(_) => "storage_error",
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

impl fmt::Display for RepositoryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Core(error) => error.fmt(formatter),
            Self::Store(error) => error.fmt(formatter),
            Self::RefStore(error) => error.fmt(formatter),
            Self::Io {
                operation,
                path,
                source,
            } => write!(formatter, "{operation} {}: {source}", path.display()),
            Self::ArchiveInvalid(message) => write!(formatter, "invalid archive: {message}"),
            Self::ArchiveDestinationExists(path) => {
                write!(
                    formatter,
                    "archive destination already exists: {}",
                    path.display()
                )
            }
            Self::RepositoryNotEmpty => {
                formatter.write_str("archive restore requires an empty repository")
            }
            Self::Json(error) => write!(formatter, "archive JSON error: {error}"),
            Self::Clock(message) => formatter.write_str(message),
        }
    }
}

impl Error for RepositoryError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Core(error) => Some(error),
            Self::Store(error) => Some(error),
            Self::RefStore(error) => Some(error),
            Self::Io { source, .. } => Some(source),
            Self::Json(error) => Some(error),
            _ => None,
        }
    }
}

impl From<CoreError> for RepositoryError {
    fn from(error: CoreError) -> Self {
        Self::Core(error)
    }
}

impl From<StoreError> for RepositoryError {
    fn from(error: StoreError) -> Self {
        Self::Store(error)
    }
}

impl From<RefStoreError> for RepositoryError {
    fn from(error: RefStoreError) -> Self {
        Self::RefStore(error)
    }
}

impl From<serde_json::Error> for RepositoryError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

pub type Result<T> = std::result::Result<T, RepositoryError>;

pub struct Repository {
    root: PathBuf,
    objects: FileObjectStore,
    refs: SqliteRefStore,
    graph_limits: GraphLimits,
    tombstone_scan_limits: TombstoneScanLimits,
}

impl Repository {
    pub fn open(root: impl AsRef<Path>) -> Result<Self> {
        Self::open_with_limits(root, StoreLimits::default(), GraphLimits::default())
    }

    /// Open a repository with a service-owned bound for publication-time
    /// Tombstone discovery while retaining the default object and graph limits.
    pub fn open_with_tombstone_scan_limits(
        root: impl AsRef<Path>,
        tombstone_scan_limits: TombstoneScanLimits,
    ) -> Result<Self> {
        Self::open_with_validation_limits(
            root,
            StoreLimits::default(),
            GraphLimits::default(),
            tombstone_scan_limits,
        )
    }

    pub fn open_with_limits(
        root: impl AsRef<Path>,
        store_limits: StoreLimits,
        graph_limits: GraphLimits,
    ) -> Result<Self> {
        Self::open_with_validation_limits(
            root,
            store_limits,
            graph_limits,
            TombstoneScanLimits::default(),
        )
    }

    /// Open a repository with explicit storage, traversal, and
    /// publication-time Tombstone scan limits.
    pub fn open_with_validation_limits(
        root: impl AsRef<Path>,
        store_limits: StoreLimits,
        graph_limits: GraphLimits,
        tombstone_scan_limits: TombstoneScanLimits,
    ) -> Result<Self> {
        let requested = root.as_ref();
        fs::create_dir_all(requested)
            .map_err(|error| RepositoryError::io("create repository", requested, error))?;
        let root = fs::canonicalize(requested)
            .map_err(|error| RepositoryError::io("canonicalize repository", requested, error))?;
        let objects = FileObjectStore::open_with_limits(root.join("cas"), store_limits)?;
        let refs = SqliteRefStore::open(root.join("refs.sqlite3"))?;
        Ok(Self {
            root,
            objects,
            refs,
            graph_limits,
            tombstone_scan_limits,
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn objects(&self) -> &FileObjectStore {
        &self.objects
    }

    pub fn refs(&self) -> &SqliteRefStore {
        &self.refs
    }

    pub fn put_blob(&self, reader: impl Read) -> Result<PutResult> {
        Ok(self.objects.put_blob(reader)?)
    }

    pub fn put_blob_claimed(&self, claimed_oid: &str, reader: impl Read) -> Result<PutResult> {
        Ok(self.objects.put_blob_claimed(claimed_oid, reader)?)
    }

    /// Validate a concrete structured object before publishing canonical bytes.
    pub fn put_object(&self, input: &[u8]) -> Result<PutResult> {
        let validated = ingest(input)?;
        Ok(self
            .objects
            .put_verified_raw(validated.oid(), validated.canonical_bytes())?)
    }

    /// As [`Self::put_object`], requiring one structured object family.
    pub fn put_object_as(&self, expected: ObjectKind, input: &[u8]) -> Result<PutResult> {
        if !expected.is_structured() {
            return Err(CoreError::new(
                ErrorCode::SchemaInvalid,
                "put_object_as requires a structured object kind",
            )
            .into());
        }
        let validated = ingest(input)?;
        let actual = parse_oid(validated.oid())?;
        if actual != expected {
            return Err(CoreError::new(
                ErrorCode::ReferenceTypeMismatch,
                format!(
                    "expected {}, received {}",
                    expected.prefix(),
                    actual.prefix()
                ),
            )
            .into());
        }
        Ok(self
            .objects
            .put_verified_raw(validated.oid(), validated.canonical_bytes())?)
    }

    /// Validate a concrete structured object and its transport-supplied OID.
    pub fn put_object_claimed(&self, claimed_oid: &str, input: &[u8]) -> Result<PutResult> {
        let validated = ingest_claimed(claimed_oid, input)?;
        Ok(self
            .objects
            .put_verified_raw(validated.oid(), validated.canonical_bytes())?)
    }

    /// Claimed-OID ingestion requiring one structured object family.
    pub fn put_object_claimed_as(
        &self,
        expected: ObjectKind,
        claimed_oid: &str,
        input: &[u8],
    ) -> Result<PutResult> {
        if !expected.is_structured() {
            return Err(CoreError::new(
                ErrorCode::SchemaInvalid,
                "put_object_claimed_as requires a structured object kind",
            )
            .into());
        }
        if parse_oid(claimed_oid)? != expected {
            return Err(CoreError::new(
                ErrorCode::ReferenceTypeMismatch,
                format!("claimed OID is not a {} OID", expected.prefix()),
            )
            .into());
        }
        let validated = ingest_claimed(claimed_oid, input)?;
        Ok(self
            .objects
            .put_verified_raw(validated.oid(), validated.canonical_bytes())?)
    }

    pub fn validate_head(&self, head: &str) -> std::result::Result<(), ValidationError> {
        validate_head(
            &self.objects,
            head,
            self.graph_limits,
            self.tombstone_scan_limits,
        )
    }

    pub fn update_ref(&mut self, update: RefUpdate<'_>) -> Result<ReflogEntry> {
        let objects = &self.objects;
        let limits = self.graph_limits;
        let tombstone_scan_limits = self.tombstone_scan_limits;
        // SqliteRefStore performs lexical Ref/head/metadata validation before
        // invoking this closure, so malformed requests cannot force the
        // bounded-but-potentially-large Tombstone inventory scan.
        let validator = |head: &str| validate_head(objects, head, limits, tombstone_scan_limits);
        Ok(self.refs.compare_and_swap(update, &validator)?)
    }

    pub fn fsck(&self) -> Result<FsckReport> {
        let heads = self
            .refs
            .list()?
            .into_iter()
            .map(|record| record.head)
            .collect::<Vec<_>>();
        Ok(fsck(&self.objects, &heads, self.graph_limits)?)
    }

    /// Check the complete CAS inventory and all heads in one bounded current
    /// Ref snapshot.
    ///
    /// This is the service-safe counterpart to [`Self::fsck`]. The legacy
    /// method remains unbounded for CLI compatibility.
    pub fn fsck_with_limits(&self, limits: FsckLimits) -> Result<FsckReport> {
        validate_fsck_limits(limits)?;
        let snapshot = self.refs.snapshot_limited(limits.max_ref_roots)?;
        self.fsck_snapshot_with_limits(&snapshot, limits)
    }

    /// Check the complete CAS inventory using exactly the heads in `snapshot`.
    ///
    /// The supplied snapshot is never replaced with a newer database snapshot.
    /// Its Ref count is charged before duplicate heads are removed; closure work
    /// is then charged once for each distinct head across the whole operation.
    pub fn fsck_snapshot_with_limits(
        &self,
        snapshot: &RefSnapshot,
        limits: FsckLimits,
    ) -> Result<FsckReport> {
        validate_fsck_limits(limits)?;
        if snapshot.refs.len() > limits.max_ref_roots {
            return Err(resource_limit(format!(
                "fsck Ref snapshot exceeds max_ref_roots {}",
                limits.max_ref_roots
            )));
        }

        let inventory = self.objects.list_oids_limited(limits.max_objects)?;
        let objects_seen = inventory.len();
        let mut objects_verified = 0_usize;
        let mut inventoried_object_bytes = 0_u64;
        let mut stored_commits = Vec::new();
        let mut issues = Vec::new();

        for oid in &inventory {
            if parse_oid(oid)? == ObjectKind::Commit {
                stored_commits.push(oid.clone());
            }
            let Some(byte_len) = self.objects.stored_object_byte_len(oid)? else {
                issues.push(FsckIssue {
                    kind: FsckIssueKind::MissingScannedObject { oid: oid.clone() },
                });
                continue;
            };
            inventoried_object_bytes =
                inventoried_object_bytes
                    .checked_add(byte_len)
                    .ok_or_else(|| {
                        resource_limit("fsck inventoried object byte total overflowed u64")
                    })?;
            if inventoried_object_bytes > limits.max_object_bytes {
                return Err(resource_limit(format!(
                    "fsck inventoried object bytes exceed max_object_bytes {}",
                    limits.max_object_bytes
                )));
            }
            match self.objects.get_verified(oid) {
                Ok(Some(object)) => {
                    if object.byte_len() != byte_len {
                        return Err(RepositoryError::io(
                            "verify stable fsck object length",
                            PathBuf::from(oid),
                            io::Error::other("object length changed during the integrity check"),
                        ));
                    }
                    objects_verified = objects_verified.checked_add(1).ok_or_else(|| {
                        resource_limit("fsck verified object count overflowed usize")
                    })?;
                }
                Ok(None) => issues.push(FsckIssue {
                    kind: FsckIssueKind::MissingScannedObject { oid: oid.clone() },
                }),
                Err(StoreError::CorruptObject { detail, .. }) => issues.push(FsckIssue {
                    kind: FsckIssueKind::CorruptObject {
                        oid: oid.clone(),
                        detail,
                    },
                }),
                Err(error) if error.code() == Some(ErrorCode::ResourceLimit) => {
                    return Err(error.into());
                }
                Err(error) => issues.push(FsckIssue {
                    kind: FsckIssueKind::ReadFailure {
                        oid: oid.clone(),
                        detail: error.to_string(),
                    },
                }),
            }
        }

        let verifier =
            PreparedClosureVerifier::new(&self.objects, self.graph_limits, limits.tombstone_scan)?;
        let mut distinct_heads = HashSet::new();
        let mut closure_nodes = 0_usize;
        let mut closure_edges = 0_usize;
        let mut closures = Vec::new();

        let roots = if snapshot.refs.is_empty() {
            stored_commits
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>()
        } else {
            snapshot
                .refs
                .iter()
                .map(|record| record.head.as_str())
                .collect::<Vec<_>>()
        };
        for head in roots {
            if !distinct_heads.insert(head) {
                continue;
            }
            let remaining_nodes = limits
                .max_closure_nodes
                .checked_sub(closure_nodes)
                .ok_or_else(|| resource_limit("fsck closure node accounting underflowed"))?;
            let remaining_edges = limits
                .max_closure_edges
                .checked_sub(closure_edges)
                .ok_or_else(|| resource_limit("fsck closure edge accounting underflowed"))?;
            let closure = verifier.verify_uncached_with_work_limits(
                head,
                remaining_nodes,
                remaining_edges,
            )?;
            if closure.truncated
                || closure
                    .issues
                    .iter()
                    .any(|issue| matches!(issue.kind, ClosureIssueKind::ResourceLimit { .. }))
            {
                return Err(resource_limit(format!(
                    "fsck closure traversal exceeded cumulative limits for {head}"
                )));
            }
            closure_nodes = checked_add_fsck_work(
                closure_nodes,
                closure.nodes.len(),
                limits.max_closure_nodes,
                "closure nodes",
            )?;
            closure_edges = checked_add_fsck_work(
                closure_edges,
                closure.edges.len(),
                limits.max_closure_edges,
                "closure edges",
            )?;
            issues.extend(closure.issues.iter().cloned().map(|issue| FsckIssue {
                kind: FsckIssueKind::Closure(issue),
            }));
            closures.push(closure);
        }

        Ok(FsckReport {
            objects_seen,
            objects_verified,
            closures,
            issues,
        })
    }

    pub fn export_archive(&mut self, destination: impl AsRef<Path>) -> Result<()> {
        self.export_archive_with_limits(destination, ArchiveExportLimits::default())
    }

    pub fn export_archive_with_limits(
        &mut self,
        destination: impl AsRef<Path>,
        limits: ArchiveExportLimits,
    ) -> Result<()> {
        let destination = destination.as_ref();
        if destination.exists() {
            return Err(RepositoryError::ArchiveDestinationExists(
                destination.to_path_buf(),
            ));
        }
        if limits.max_objects == 0 {
            return Err(resource_limit(
                "archive max_objects must be greater than zero",
            ));
        }
        if limits.max_object_bytes == 0 {
            return Err(resource_limit(
                "archive max_object_bytes must be greater than zero",
            ));
        }
        if limits.max_head_validation_nodes == 0 {
            return Err(resource_limit(
                "archive max_head_validation_nodes must be greater than zero",
            ));
        }
        if limits.max_head_validation_edges == 0 {
            return Err(resource_limit(
                "archive max_head_validation_edges must be greater than zero",
            ));
        }
        // Refs and their complete reflog are snapshotted first. Objects are
        // immutable and never removed, so concurrent writers can only add data
        // after this point; they cannot make this archived Ref history depend
        // on an omitted newer object.
        let refs = self.refs.export_archive_with_limits(limits.ref_archive)?;
        let verifier =
            PreparedClosureVerifier::new(&self.objects, self.graph_limits, limits.tombstone_scan)
                .map_err(|error| {
                archive_store_error(error, "Tombstone catalog is not exportable".to_owned())
            })?;
        let mut validated_heads = HashSet::new();
        let mut validated_head_nodes = 0_usize;
        let mut validated_head_edges = 0_usize;
        let mut validate_distinct_head = |head: &str, context: String| -> Result<()> {
            let remaining_nodes = limits
                .max_head_validation_nodes
                .checked_sub(validated_head_nodes)
                .expect("archive head node work stays within its configured limit");
            let remaining_edges = limits
                .max_head_validation_edges
                .checked_sub(validated_head_edges)
                .expect("archive head edge work stays within its configured limit");
            let (nodes, edges) =
                validate_archive_head(&verifier, head, context, remaining_nodes, remaining_edges)?;
            validated_head_nodes = validated_head_nodes.checked_add(nodes).ok_or_else(|| {
                resource_limit("archive head validation node total overflowed usize")
            })?;
            validated_head_edges = validated_head_edges.checked_add(edges).ok_or_else(|| {
                resource_limit("archive head validation edge total overflowed usize")
            })?;
            Ok(())
        };
        for record in &refs.snapshot.refs {
            if validated_heads.insert(record.head.as_str()) {
                validate_distinct_head(
                    &record.head,
                    format!("Ref {:?} is not exportable", record.name),
                )?;
            }
        }
        for event in &refs.reflog {
            if validated_heads.insert(event.new_head.as_str()) {
                validate_distinct_head(
                    &event.new_head,
                    format!(
                        "reflog event {} for Ref {:?} is not exportable",
                        event.id, event.ref_name
                    ),
                )?;
            }
        }
        let parent = destination.parent().unwrap_or_else(|| Path::new("."));
        fs::create_dir_all(parent)
            .map_err(|error| RepositoryError::io("create archive parent", parent, error))?;
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|error| RepositoryError::Clock(format!("system clock error: {error}")))?
            .as_nanos();
        let staging_path = archive_staging_path(destination, nonce);
        let mut staging = StagingDirectory::create(&staging_path)?;
        let objects_directory = staging_path.join("objects");
        fs::create_dir(&objects_directory).map_err(|error| {
            RepositoryError::io("create archive object directory", &objects_directory, error)
        })?;

        let mut object_rows = Vec::new();
        let mut total_object_bytes = 0_u64;
        for (index, oid) in self
            .objects
            .list_oids_limited(limits.max_objects)?
            .into_iter()
            .enumerate()
        {
            let relative_path = format!("objects/{index:08}");
            let output_path = staging_path.join(&relative_path);
            let file = OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&output_path)
                .map_err(|error| {
                    RepositoryError::io("create archive object", &output_path, error)
                })?;
            let remaining_object_bytes = limits
                .max_object_bytes
                .checked_sub(total_object_bytes)
                .expect("archive byte total never exceeds its configured limit");
            let mut writer = HashingWriter::new(file, remaining_object_bytes);
            let copy_result = self.objects.copy_verified_to(&oid, &mut writer);
            if writer.limit_exceeded() {
                return Err(resource_limit(format!(
                    "archive object bytes exceed max_object_bytes {}",
                    limits.max_object_bytes
                )));
            }
            let info = match copy_result {
                Ok(Some(info)) => info,
                Ok(None) => {
                    return Err(RepositoryError::ArchiveInvalid(format!(
                        "object disappeared during export: {oid}"
                    )));
                }
                Err(error) => return Err(error.into()),
            };
            let (byte_length, sha256) = writer.finish(&output_path)?;
            if byte_length != info.byte_len {
                return Err(RepositoryError::ArchiveInvalid(format!(
                    "object changed length during export: {oid}"
                )));
            }
            total_object_bytes = total_object_bytes
                .checked_add(byte_length)
                .ok_or_else(|| resource_limit("archive object byte total overflowed u64"))?;
            object_rows.push(ArchiveObject {
                oid,
                path: relative_path,
                byte_length,
                sha256,
            });
        }
        let manifest = ArchiveManifest::from_parts(object_rows, refs);
        let manifest_path = staging_path.join(MANIFEST_FILE);
        let manifest_file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&manifest_path)
            .map_err(|error| {
                RepositoryError::io("create archive manifest", &manifest_path, error)
            })?;
        let mut manifest_writer = HashingWriter::new(manifest_file, MAX_MANIFEST_BYTES);
        let serialization = serde_json::to_writer_pretty(&mut manifest_writer, &manifest);
        if manifest_writer.limit_exceeded() {
            return Err(resource_limit(format!(
                "archive manifest exceeds the {MAX_MANIFEST_BYTES} byte restore limit"
            )));
        }
        if let Err(error) = serialization {
            if error.is_io() {
                let kind = error.io_error_kind().unwrap_or(io::ErrorKind::Other);
                return Err(RepositoryError::io(
                    "write archive manifest",
                    &manifest_path,
                    io::Error::new(kind, error),
                ));
            }
            return Err(error.into());
        }
        let (_, manifest_sha256) = manifest_writer.finish(&manifest_path)?;
        let checksum = format!("{manifest_sha256}\n");
        write_new_synced(
            &staging_path.join(MANIFEST_CHECKSUM_FILE),
            checksum.as_bytes(),
        )?;
        sync_directory(&objects_directory)?;
        sync_directory(&staging_path)?;
        rename_directory_no_replace(&staging_path, destination)?;
        staging.disarm();
        sync_directory(parent)?;
        Ok(())
    }

    pub fn restore_archive(
        archive: impl AsRef<Path>,
        repository: impl AsRef<Path>,
    ) -> Result<Self> {
        let mut result = Self::open(repository)?;
        result.restore_from(archive)?;
        Ok(result)
    }

    pub fn restore_from(&mut self, archive: impl AsRef<Path>) -> Result<()> {
        if !self.refs.snapshot()?.is_empty() || !self.refs.reflog()?.is_empty() {
            return Err(RepositoryError::RepositoryNotEmpty);
        }
        let existing_oids = self.objects.list_oids()?;

        let archive = archive.as_ref();
        let manifest_path = archive.join(MANIFEST_FILE);
        let manifest_bytes = read_limited(&manifest_path, MAX_MANIFEST_BYTES)?;
        let checksum_path = archive.join(MANIFEST_CHECKSUM_FILE);
        let checksum_bytes = read_limited(&checksum_path, 256)?;
        if checksum_bytes.len() != 65
            || checksum_bytes[64] != b'\n'
            || !checksum_bytes[..64]
                .iter()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
        {
            return Err(RepositoryError::ArchiveInvalid(
                "manifest checksum must be 64 lowercase hex characters plus newline".into(),
            ));
        }
        let claimed_checksum = std::str::from_utf8(&checksum_bytes[..64]).map_err(|_| {
            RepositoryError::ArchiveInvalid("manifest checksum is not ASCII".into())
        })?;
        let expected_checksum = sha256_hex(&manifest_bytes);
        if claimed_checksum != expected_checksum {
            return Err(RepositoryError::ArchiveInvalid(format!(
                "manifest checksum mismatch: claimed {claimed_checksum:?}, expected {expected_checksum}"
            )));
        }
        let manifest: ArchiveManifest = serde_json::from_slice(&manifest_bytes)?;
        manifest.validate()?;
        let archived_oids = manifest
            .objects
            .iter()
            .map(|object| object.oid.as_str())
            .collect::<HashSet<_>>();
        if existing_oids
            .iter()
            .any(|oid| !archived_oids.contains(oid.as_str()))
        {
            return Err(RepositoryError::RepositoryNotEmpty);
        }

        for object in &manifest.objects {
            let path = archive.join(&object.path);
            match parse_oid(&object.oid)? {
                ObjectKind::Blob => {
                    let (file, byte_length) =
                        open_regular_limited(&path, self.object_limit(&object.oid)?)?;
                    if byte_length != object.byte_length {
                        return Err(RepositoryError::ArchiveInvalid(format!(
                            "{} length mismatch",
                            object.path
                        )));
                    }
                    let result = self.objects.put_blob_claimed(&object.oid, file)?;
                    if result.byte_len != object.byte_length {
                        return Err(RepositoryError::ArchiveInvalid(format!(
                            "{} changed length during restore",
                            object.path
                        )));
                    }
                }
                _ => {
                    let bytes = read_limited(&path, self.object_limit(&object.oid)?)?;
                    if bytes.len() as u64 != object.byte_length {
                        return Err(RepositoryError::ArchiveInvalid(format!(
                            "{} length mismatch",
                            object.path
                        )));
                    }
                    let digest = sha256_hex(&bytes);
                    if digest != object.sha256 {
                        return Err(RepositoryError::ArchiveInvalid(format!(
                            "{} checksum mismatch",
                            object.path
                        )));
                    }
                    let validated = ingest_claimed(&object.oid, &bytes)?;
                    if validated.canonical_bytes() != bytes {
                        return Err(RepositoryError::ArchiveInvalid(format!(
                            "{} is not canonical structured JSON",
                            object.path
                        )));
                    }
                    self.objects.put_verified_raw(&object.oid, &bytes)?;
                }
            }
        }

        let stored = self.objects.list_oids()?;
        let expected = manifest
            .objects
            .iter()
            .map(|object| object.oid.clone())
            .collect::<Vec<_>>();
        if stored != expected {
            return Err(RepositoryError::ArchiveInvalid(
                "restored object inventory differs from manifest".into(),
            ));
        }

        let ref_archive = manifest.into_ref_archive();
        let objects = &self.objects;
        let limits = self.graph_limits;
        let tombstone_scan_limits = self.tombstone_scan_limits;
        let verifier = RefCell::new(None);
        let validator = |head: &str| {
            let mut verifier = verifier.borrow_mut();
            if verifier.is_none() {
                *verifier = Some(
                    PreparedClosureVerifier::new(objects, limits, tombstone_scan_limits).map_err(
                        |error| ValidationError::new(store_error_code(&error), error.to_string()),
                    )?,
                );
            }
            validate_prepared_head(
                verifier
                    .as_ref()
                    .expect("the archive verifier is initialized above"),
                head,
            )
        };
        self.refs.restore_archive(&ref_archive, &validator)?;
        Ok(())
    }

    fn object_limit(&self, oid: &str) -> Result<u64> {
        Ok(match parse_oid(oid)? {
            ObjectKind::Blob => self.objects.limits().max_blob_bytes,
            _ => self.objects.limits().structured.max_input_bytes as u64,
        })
    }
}

fn validate_head(
    objects: &FileObjectStore,
    head: &str,
    limits: GraphLimits,
    tombstone_scan_limits: TombstoneScanLimits,
) -> std::result::Result<(), ValidationError> {
    validate_commit_oid(head)
        .map_err(|error| ValidationError::new(error.code(), error.to_string()))?;
    let verifier = PreparedClosureVerifier::new(objects, limits, tombstone_scan_limits)
        .map_err(|error| ValidationError::new(store_error_code(&error), error.to_string()))?;
    validate_prepared_head(&verifier, head)
}

fn validate_prepared_head(
    verifier: &PreparedClosureVerifier<'_, FileObjectStore>,
    head: &str,
) -> std::result::Result<(), ValidationError> {
    let report = verifier
        .verify_uncached(head)
        .map_err(|error| ValidationError::new(store_error_code(&error), error.to_string()))?;
    validate_closure_report(&report)
}

fn validate_archive_head(
    verifier: &PreparedClosureVerifier<'_, FileObjectStore>,
    head: &str,
    context: String,
    max_objects: usize,
    max_edges: usize,
) -> Result<(usize, usize)> {
    // Keep operational StoreError values intact: export callers must observe
    // resource_limit or storage_error rather than archive_invalid.
    let report = verifier
        .verify_uncached_with_work_limits(head, max_objects, max_edges)
        .map_err(|error| archive_store_error(error, context.clone()))?;
    validate_closure_report(&report)
        .map_err(|error| archive_head_error(&error, format!("{context}: {error}")))?;
    Ok((report.nodes.len(), report.edges.len()))
}

fn validate_closure_report(report: &ClosureReport) -> std::result::Result<(), ValidationError> {
    if report.is_complete() {
        return Ok(());
    }
    if report.truncated
        || report
            .issues
            .iter()
            .any(|issue| matches!(&issue.kind, ClosureIssueKind::ResourceLimit { .. }))
    {
        return Err(ValidationError::new(
            ErrorCode::ResourceLimit.as_str(),
            format!(
                "closure traversal exceeded its configured limits for {}",
                report.root
            ),
        ));
    }
    let Some(issue) = report.issues.first() else {
        return Err(ValidationError::new(
            "resource_limit",
            "closure traversal was truncated",
        ));
    };
    let code = match &issue.kind {
        ClosureIssueKind::Missing => "closure_missing",
        ClosureIssueKind::Corrupt { .. } | ClosureIssueKind::ReadFailure { .. } => "oid_mismatch",
        ClosureIssueKind::ReferenceTypeMismatch { .. } => "reference_type_mismatch",
        ClosureIssueKind::ReferenceSemanticMismatch { .. } => "reference_type_mismatch",
        ClosureIssueKind::InvalidObject { .. } | ClosureIssueKind::InvalidReference { .. } => {
            "schema_invalid"
        }
        ClosureIssueKind::Cycle { .. } => "schema_invalid",
        ClosureIssueKind::ResourceLimit { .. } => "resource_limit",
    };
    Err(ValidationError::new(
        code,
        format!("closure issue for {}: {:?}", issue.oid, issue.kind),
    ))
}

fn store_error_code(error: &StoreError) -> &'static str {
    error.code().map_or("storage_error", ErrorCode::as_str)
}

fn validate_fsck_limits(limits: FsckLimits) -> Result<()> {
    for (name, value) in [
        ("max_ref_roots", limits.max_ref_roots),
        ("max_objects", limits.max_objects),
        ("max_closure_nodes", limits.max_closure_nodes),
        ("max_closure_edges", limits.max_closure_edges),
        (
            "tombstone_scan.max_record_objects",
            limits.tombstone_scan.max_record_objects,
        ),
    ] {
        if value == 0 {
            return Err(resource_limit(format!(
                "fsck {name} must be greater than zero"
            )));
        }
    }
    if limits.max_object_bytes == 0 {
        return Err(resource_limit(
            "fsck max_object_bytes must be greater than zero",
        ));
    }
    if limits.tombstone_scan.max_record_bytes == 0 {
        return Err(resource_limit(
            "fsck tombstone_scan.max_record_bytes must be greater than zero",
        ));
    }
    Ok(())
}

fn checked_add_fsck_work(
    current: usize,
    additional: usize,
    limit: usize,
    resource: &str,
) -> Result<usize> {
    let total = current
        .checked_add(additional)
        .ok_or_else(|| resource_limit(format!("fsck {resource} overflowed usize")))?;
    if total > limit {
        return Err(resource_limit(format!(
            "fsck {resource} exceed cumulative limit {limit}"
        )));
    }
    Ok(total)
}

fn resource_limit(message: impl Into<String>) -> RepositoryError {
    CoreError::new(ErrorCode::ResourceLimit, message).into()
}

fn archive_head_error(error: &ValidationError, message: String) -> RepositoryError {
    if error.code() == ErrorCode::ResourceLimit.as_str() {
        resource_limit(message)
    } else {
        RepositoryError::ArchiveInvalid(message)
    }
}

fn archive_store_error(error: StoreError, context: String) -> RepositoryError {
    if error
        .code()
        .is_none_or(|code| code == ErrorCode::ResourceLimit)
    {
        RepositoryError::Store(error)
    } else {
        RepositoryError::ArchiveInvalid(format!("{context}: {error}"))
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ArchiveManifest {
    format: String,
    objects: Vec<ArchiveObject>,
    refs: Vec<ArchiveRef>,
    reflog: Vec<ArchiveReflog>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ArchiveObject {
    oid: String,
    path: String,
    byte_length: u64,
    sha256: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ArchiveRef {
    name: String,
    head: String,
    updated_event_id: i64,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ArchiveReflog {
    id: i64,
    ref_name: String,
    old_head: Option<String>,
    new_head: String,
    occurred_at_unix_nanos: i64,
    actor: Option<String>,
    message: Option<String>,
}

impl ArchiveManifest {
    fn from_parts(objects: Vec<ArchiveObject>, refs: RefArchive) -> Self {
        Self {
            format: ARCHIVE_FORMAT.to_owned(),
            objects,
            refs: refs
                .snapshot
                .refs
                .into_iter()
                .map(|record| ArchiveRef {
                    name: record.name,
                    head: record.head,
                    updated_event_id: record.updated_event_id,
                })
                .collect(),
            reflog: refs
                .reflog
                .into_iter()
                .map(|entry| ArchiveReflog {
                    id: entry.id,
                    ref_name: entry.ref_name,
                    old_head: entry.old_head,
                    new_head: entry.new_head,
                    occurred_at_unix_nanos: entry.occurred_at_unix_nanos,
                    actor: entry.actor,
                    message: entry.message,
                })
                .collect(),
        }
    }

    fn validate(&self) -> Result<()> {
        if self.format != ARCHIVE_FORMAT {
            return Err(RepositoryError::ArchiveInvalid(format!(
                "unsupported format {:?}",
                self.format
            )));
        }
        let mut oids = HashSet::with_capacity(self.objects.len());
        let mut paths = HashSet::with_capacity(self.objects.len());
        let mut previous_oid: Option<&str> = None;
        for (index, object) in self.objects.iter().enumerate() {
            let kind = parse_oid(&object.oid)?;
            let expected_path = format!("objects/{index:08}");
            if object.path != expected_path {
                return Err(RepositoryError::ArchiveInvalid(format!(
                    "object path {:?} must be {:?}",
                    object.path, expected_path
                )));
            }
            if !oids.insert(object.oid.as_str()) || !paths.insert(object.path.as_str()) {
                return Err(RepositoryError::ArchiveInvalid(
                    "duplicate object OID or path".into(),
                ));
            }
            if previous_oid.is_some_and(|previous| previous >= object.oid.as_str()) {
                return Err(RepositoryError::ArchiveInvalid(
                    "object rows are not in strict OID order".into(),
                ));
            }
            if object.sha256.len() != 64
                || !object
                    .sha256
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            {
                return Err(RepositoryError::ArchiveInvalid(format!(
                    "invalid object checksum for {}",
                    object.oid
                )));
            }
            if kind == ObjectKind::Blob
                && object.oid.rsplit(':').next() != Some(object.sha256.as_str())
            {
                return Err(RepositoryError::ArchiveInvalid(format!(
                    "Blob checksum does not match its OID: {}",
                    object.oid
                )));
            }
            previous_oid = Some(&object.oid);
        }
        Ok(())
    }

    fn into_ref_archive(self) -> RefArchive {
        RefArchive {
            snapshot: RefSnapshot {
                refs: self
                    .refs
                    .into_iter()
                    .map(|record| RefRecord {
                        name: record.name,
                        head: record.head,
                        updated_event_id: record.updated_event_id,
                    })
                    .collect(),
            },
            reflog: self
                .reflog
                .into_iter()
                .map(|entry| ReflogEntry {
                    id: entry.id,
                    ref_name: entry.ref_name,
                    old_head: entry.old_head,
                    new_head: entry.new_head,
                    occurred_at_unix_nanos: entry.occurred_at_unix_nanos,
                    actor: entry.actor,
                    message: entry.message,
                })
                .collect(),
        }
    }
}

fn write_new_synced(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(path)
        .map_err(|error| RepositoryError::io("create archive file", path, error))?;
    file.write_all(bytes)
        .map_err(|error| RepositoryError::io("write archive file", path, error))?;
    file.flush()
        .map_err(|error| RepositoryError::io("flush archive file", path, error))?;
    file.sync_all()
        .map_err(|error| RepositoryError::io("sync archive file", path, error))?;
    Ok(())
}

fn read_limited(path: &Path, limit: u64) -> Result<Vec<u8>> {
    let (file, _) = open_regular_limited(path, limit)?;
    let mut bytes = Vec::new();
    file.take(limit.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|error| RepositoryError::io("read archive file", path, error))?;
    if bytes.len() as u64 > limit {
        return Err(RepositoryError::ArchiveInvalid(format!(
            "{} exceeds the {limit} byte limit",
            path.display()
        )));
    }
    Ok(bytes)
}

fn open_regular_limited(path: &Path, limit: u64) -> Result<(File, u64)> {
    let path_metadata = fs::symlink_metadata(path)
        .map_err(|error| RepositoryError::io("inspect archive file", path, error))?;
    if path_metadata.file_type().is_symlink() || !path_metadata.is_file() {
        return Err(RepositoryError::ArchiveInvalid(format!(
            "{} is not a regular file",
            path.display()
        )));
    }
    if path_metadata.len() > limit {
        return Err(RepositoryError::ArchiveInvalid(format!(
            "{} exceeds the {limit} byte limit",
            path.display()
        )));
    }
    let file =
        File::open(path).map_err(|error| RepositoryError::io("open archive file", path, error))?;
    let opened_metadata = file
        .metadata()
        .map_err(|error| RepositoryError::io("inspect opened archive file", path, error))?;
    if !opened_metadata.is_file() || opened_metadata.len() > limit {
        return Err(RepositoryError::ArchiveInvalid(format!(
            "{} changed while it was being opened",
            path.display()
        )));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if path_metadata.dev() != opened_metadata.dev()
            || path_metadata.ino() != opened_metadata.ino()
        {
            return Err(RepositoryError::ArchiveInvalid(format!(
                "{} changed while it was being opened",
                path.display()
            )));
        }
    }
    Ok((file, opened_metadata.len()))
}

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

struct HashingWriter {
    file: File,
    digest: Sha256,
    byte_length: u64,
    max_byte_length: u64,
    limit_exceeded: bool,
}

impl HashingWriter {
    fn new(file: File, max_byte_length: u64) -> Self {
        Self {
            file,
            digest: Sha256::new(),
            byte_length: 0,
            max_byte_length,
            limit_exceeded: false,
        }
    }

    const fn limit_exceeded(&self) -> bool {
        self.limit_exceeded
    }

    fn finish(mut self, path: &Path) -> Result<(u64, String)> {
        self.file
            .flush()
            .map_err(|error| RepositoryError::io("flush archive object", path, error))?;
        self.file
            .sync_all()
            .map_err(|error| RepositoryError::io("sync archive object", path, error))?;
        Ok((self.byte_length, format!("{:x}", self.digest.finalize())))
    }
}

impl Write for HashingWriter {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        let remaining = self
            .max_byte_length
            .checked_sub(self.byte_length)
            .expect("archive writer byte length stays within its limit");
        if remaining == 0 && !buffer.is_empty() {
            self.limit_exceeded = true;
            return Err(io::Error::other("archive byte limit exceeded"));
        }
        let allowed = usize::try_from(remaining)
            .unwrap_or(usize::MAX)
            .min(buffer.len());
        let count = self.file.write(&buffer[..allowed])?;
        self.digest.update(&buffer[..count]);
        self.byte_length = self
            .byte_length
            .checked_add(count as u64)
            .ok_or_else(|| io::Error::other("archive byte length overflow"))?;
        Ok(count)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

struct StagingDirectory {
    path: PathBuf,
    armed: bool,
}

impl StagingDirectory {
    fn create(path: &Path) -> Result<Self> {
        fs::create_dir(path).map_err(|error| {
            RepositoryError::io("create archive staging directory", path, error)
        })?;
        Ok(Self {
            path: path.to_path_buf(),
            armed: true,
        })
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for StagingDirectory {
    fn drop(&mut self) {
        if self.armed {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}

fn archive_staging_path(destination: &Path, nonce: u128) -> PathBuf {
    let parent = destination.parent().unwrap_or_else(|| Path::new("."));
    let mut name = OsString::from(".");
    name.push(
        destination
            .file_name()
            .unwrap_or_else(|| OsStr::new("archive")),
    );
    name.push(format!(".tmp-{}-{nonce}", std::process::id()));
    parent.join(name)
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
        Err(Errno::EXIST) => Err(RepositoryError::ArchiveDestinationExists(
            destination.to_path_buf(),
        )),
        Err(error) => Err(RepositoryError::io(
            "publish archive without replacement",
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
    Err(RepositoryError::io(
        "publish archive without replacement",
        destination,
        io::Error::new(
            io::ErrorKind::Unsupported,
            "this platform has no supported atomic directory no-replace primitive",
        ),
    ))
}

#[cfg(unix)]
fn sync_directory(path: &Path) -> Result<()> {
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| RepositoryError::io("sync directory", path, error))
}

#[cfg(not(unix))]
fn sync_directory(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod archive_tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_MANIFEST_TEST: AtomicU64 = AtomicU64::new(0);

    fn unique_test_path(label: &str) -> PathBuf {
        let sequence = NEXT_MANIFEST_TEST.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "synapse-core-{label}-{}-{sequence}",
            std::process::id()
        ))
    }

    #[test]
    fn manifest_serialization_stops_at_the_writer_byte_limit() {
        let path = unique_test_path("manifest-limit");
        let file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&path)
            .unwrap();
        let manifest = ArchiveManifest {
            format: ARCHIVE_FORMAT.to_owned(),
            objects: Vec::new(),
            refs: Vec::new(),
            reflog: vec![ArchiveReflog {
                id: 1,
                ref_name: "proposal/agent/manifest-limit".to_owned(),
                old_head: None,
                new_head: "commit:sg-oid-v1:sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                    .to_owned(),
                occurred_at_unix_nanos: 1,
                actor: None,
                message: Some("x".repeat(1_024)),
            }],
        };
        let mut writer = HashingWriter::new(file, 128);

        let error = serde_json::to_writer_pretty(&mut writer, &manifest).unwrap_err();
        assert!(error.is_io());
        assert!(writer.limit_exceeded());
        drop(writer);
        assert_eq!(fs::metadata(&path).unwrap().len(), 128);
        fs::remove_file(path).unwrap();
    }

    #[cfg(any(
        target_os = "linux",
        target_os = "android",
        target_vendor = "apple",
        target_os = "redox"
    ))]
    #[test]
    fn archive_publication_never_replaces_an_existing_directory() {
        let root = unique_test_path("archive-no-replace");
        let source = root.join("staging");
        let destination = root.join("destination");
        fs::create_dir(&root).unwrap();
        fs::create_dir(&source).unwrap();
        fs::write(source.join("manifest.json"), b"staged").unwrap();
        fs::create_dir(&destination).unwrap();

        let error = rename_directory_no_replace(&source, &destination).unwrap_err();
        assert_eq!(error.code(), "archive_invalid");
        assert!(source.join("manifest.json").is_file());
        assert_eq!(fs::read_dir(&destination).unwrap().count(), 0);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn closure_resource_limits_take_precedence_over_earlier_missing_issues() {
        let root = "commit:sg-oid-v1:sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let report = ClosureReport {
            root: root.to_owned(),
            nodes: Default::default(),
            edges: Vec::new(),
            issues: vec![
                synapse_cas::ClosureIssue {
                    oid: "blob:sg-oid-v1:sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
                        .to_owned(),
                    referenced_by: Some(root.to_owned()),
                    role: None,
                    kind: ClosureIssueKind::Missing,
                },
                synapse_cas::ClosureIssue {
                    oid: root.to_owned(),
                    referenced_by: None,
                    role: None,
                    kind: ClosureIssueKind::ResourceLimit {
                        resource: "objects",
                        limit: 1,
                    },
                },
            ],
            truncated: true,
        };

        let error = validate_closure_report(&report).unwrap_err();
        assert_eq!(error.code(), "resource_limit");
    }

    #[test]
    fn archive_head_store_errors_preserve_only_operational_codes() {
        let semantic = archive_store_error(
            CoreError::new(ErrorCode::SchemaInvalid, "invalid head").into(),
            "Ref is not exportable".to_owned(),
        );
        assert_eq!(semantic.code(), "archive_invalid");

        let limited = archive_store_error(
            CoreError::new(ErrorCode::ResourceLimit, "read limit").into(),
            "Ref is not exportable".to_owned(),
        );
        assert_eq!(limited.code(), "resource_limit");

        let io_error = archive_store_error(
            StoreError::Io {
                operation: "read object",
                path: PathBuf::from("object"),
                source: io::Error::other("temporary failure"),
            },
            "Ref is not exportable".to_owned(),
        );
        assert_eq!(io_error.code(), "storage_error");
    }
}
