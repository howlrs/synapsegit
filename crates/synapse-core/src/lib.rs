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
use std::collections::HashSet;
use std::error::Error;
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use synapse_canonical::{CoreError, ErrorCode, ObjectKind, parse_oid};
use synapse_cas::{
    ClosureIssueKind, FileObjectStore, FsckReport, GraphLimits, PutResult, StoreError, StoreLimits,
    fsck, verify_closure,
};
use synapse_schema::{ingest, ingest_claimed};
use synapse_sqlite::{
    RefArchive, RefRecord, RefSnapshot, RefStoreError, RefUpdate, ReflogEntry, SqliteRefStore,
    ValidationError,
};

const ARCHIVE_FORMAT: &str = "synapsegit-core-archive-v0.1";
const MANIFEST_FILE: &str = "manifest.json";
const MANIFEST_CHECKSUM_FILE: &str = "manifest.sha256";
const MAX_MANIFEST_BYTES: u64 = 64 * 1024 * 1024;

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
}

impl Repository {
    pub fn open(root: impl AsRef<Path>) -> Result<Self> {
        Self::open_with_limits(root, StoreLimits::default(), GraphLimits::default())
    }

    pub fn open_with_limits(
        root: impl AsRef<Path>,
        store_limits: StoreLimits,
        graph_limits: GraphLimits,
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
        validate_head(&self.objects, head, self.graph_limits)
    }

    pub fn update_ref(&mut self, update: RefUpdate<'_>) -> Result<ReflogEntry> {
        let objects = &self.objects;
        let limits = self.graph_limits;
        let validator = |head: &str| validate_head(objects, head, limits);
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

    pub fn export_archive(&mut self, destination: impl AsRef<Path>) -> Result<()> {
        let destination = destination.as_ref();
        if destination.exists() {
            return Err(RepositoryError::ArchiveDestinationExists(
                destination.to_path_buf(),
            ));
        }
        // Refs and their complete reflog are snapshotted first. Objects are
        // immutable and never removed, so concurrent writers can only add data
        // after this point; they cannot make this archived Ref history depend
        // on an omitted newer object.
        let refs = self.refs.export_archive()?;
        let mut validated_heads = HashSet::new();
        for record in &refs.snapshot.refs {
            if validated_heads.insert(record.head.as_str()) {
                self.validate_head(&record.head).map_err(|error| {
                    RepositoryError::ArchiveInvalid(format!(
                        "Ref {:?} is not exportable: {error}",
                        record.name
                    ))
                })?;
            }
        }
        for event in &refs.reflog {
            if validated_heads.insert(event.new_head.as_str()) {
                self.validate_head(&event.new_head).map_err(|error| {
                    RepositoryError::ArchiveInvalid(format!(
                        "reflog event {} for Ref {:?} is not exportable: {error}",
                        event.id, event.ref_name
                    ))
                })?;
            }
        }
        let parent = destination.parent().unwrap_or_else(|| Path::new("."));
        fs::create_dir_all(parent)
            .map_err(|error| RepositoryError::io("create archive parent", parent, error))?;
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|error| RepositoryError::Clock(format!("system clock error: {error}")))?
            .as_nanos();
        let file_name = destination
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("archive");
        let staging_path = parent.join(format!(".{file_name}.tmp-{}-{nonce}", std::process::id()));
        let mut staging = StagingDirectory::create(&staging_path)?;
        let objects_directory = staging_path.join("objects");
        fs::create_dir(&objects_directory).map_err(|error| {
            RepositoryError::io("create archive object directory", &objects_directory, error)
        })?;

        let mut object_rows = Vec::new();
        for (index, oid) in self.objects.list_oids()?.into_iter().enumerate() {
            let relative_path = format!("objects/{index:08}");
            let output_path = staging_path.join(&relative_path);
            let file = OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&output_path)
                .map_err(|error| {
                    RepositoryError::io("create archive object", &output_path, error)
                })?;
            let mut writer = HashingWriter::new(file);
            let info = self
                .objects
                .copy_verified_to(&oid, &mut writer)?
                .ok_or_else(|| {
                    RepositoryError::ArchiveInvalid(format!(
                        "object disappeared during export: {oid}"
                    ))
                })?;
            let (byte_length, sha256) = writer.finish(&output_path)?;
            if byte_length != info.byte_len {
                return Err(RepositoryError::ArchiveInvalid(format!(
                    "object changed length during export: {oid}"
                )));
            }
            object_rows.push(ArchiveObject {
                oid,
                path: relative_path,
                byte_length,
                sha256,
            });
        }

        let manifest = ArchiveManifest::from_parts(object_rows, refs);
        let manifest_bytes = serde_json::to_vec_pretty(&manifest)?;
        write_new_synced(&staging_path.join(MANIFEST_FILE), &manifest_bytes)?;
        let checksum = format!("{}\n", sha256_hex(&manifest_bytes));
        write_new_synced(
            &staging_path.join(MANIFEST_CHECKSUM_FILE),
            checksum.as_bytes(),
        )?;
        sync_directory(&objects_directory)?;
        sync_directory(&staging_path)?;
        fs::rename(&staging_path, destination)
            .map_err(|error| RepositoryError::io("publish archive", destination, error))?;
        sync_directory(parent)?;
        staging.disarm();
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
        let validator = |head: &str| validate_head(objects, head, limits);
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
) -> std::result::Result<(), ValidationError> {
    let report = verify_closure(objects, head, limits)
        .map_err(|error| ValidationError::new(store_error_code(&error), error.to_string()))?;
    if report.is_complete() {
        return Ok(());
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
}

impl HashingWriter {
    fn new(file: File) -> Self {
        Self {
            file,
            digest: Sha256::new(),
            byte_length: 0,
        }
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
        let count = self.file.write(buffer)?;
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
