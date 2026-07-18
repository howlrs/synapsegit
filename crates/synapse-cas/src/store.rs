use sha2::{Digest, Sha256};
use std::error::Error;
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use synapse_canonical::{
    CoreError, ErrorCode, ObjectKind, ResourceLimits, Value, canonical_bytes_with_limits,
    parse_oid, parse_strict_with_limits, structured_oid_unchecked_with_limits, verify_blob_oid,
    verify_claimed_oid_unchecked_with_limits,
};

const OBJECTS_DIRECTORY: &str = "objects";
const TEMP_DIRECTORY: &str = "tmp";
const DEFAULT_IO_BUFFER_BYTES: usize = 64 * 1024;
const DEFAULT_MAX_BLOB_BYTES: u64 = 512 * 1024 * 1024;

/// Deployment limits for filesystem object ingestion and verification.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StoreLimits {
    /// Strict JSON and canonical-output limits for structured objects.
    pub structured: ResourceLimits,
    /// Maximum original byte length accepted for a Blob.
    pub max_blob_bytes: u64,
    /// Fixed-size buffer used while hashing and comparing Blob streams.
    pub io_buffer_bytes: usize,
}

impl Default for StoreLimits {
    fn default() -> Self {
        Self {
            structured: ResourceLimits::default(),
            max_blob_bytes: DEFAULT_MAX_BLOB_BYTES,
            io_buffer_bytes: DEFAULT_IO_BUFFER_BYTES,
        }
    }
}

/// Filesystem CAS failures. Protocol-boundary failures retain their stable
/// [`ErrorCode`] through [`StoreError::code`].
#[derive(Debug)]
pub enum StoreError {
    /// A mutating operation was attempted through a read-only store handle.
    ReadOnly,
    Core(CoreError),
    Io {
        operation: &'static str,
        path: PathBuf,
        source: io::Error,
    },
    CorruptObject {
        oid: String,
        detail: String,
    },
    InvalidStoreLayout {
        path: PathBuf,
        detail: String,
    },
}

impl StoreError {
    /// Stable protocol code when this failure has one.
    pub fn code(&self) -> Option<ErrorCode> {
        match self {
            Self::ReadOnly => None,
            Self::Core(error) => Some(error.code()),
            Self::CorruptObject { .. } => Some(ErrorCode::OidMismatch),
            Self::InvalidStoreLayout { .. } => Some(ErrorCode::SchemaInvalid),
            Self::Io { .. } => None,
        }
    }

    pub fn corrupt_oid(&self) -> Option<&str> {
        match self {
            Self::CorruptObject { oid, .. } => Some(oid),
            _ => None,
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

impl fmt::Display for StoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ReadOnly => formatter.write_str("filesystem object store is read-only"),
            Self::Core(error) => error.fmt(formatter),
            Self::Io {
                operation,
                path,
                source,
            } => write!(formatter, "{operation} {}: {source}", path.display()),
            Self::CorruptObject { oid, detail } => {
                write!(formatter, "corrupt object {oid}: {detail}")
            }
            Self::InvalidStoreLayout { path, detail } => {
                write!(formatter, "invalid store path {}: {detail}", path.display())
            }
        }
    }
}

impl Error for StoreError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::ReadOnly => None,
            Self::Core(error) => Some(error),
            Self::Io { source, .. } => Some(source),
            Self::CorruptObject { .. } | Self::InvalidStoreLayout { .. } => None,
        }
    }
}

impl From<CoreError> for StoreError {
    fn from(value: CoreError) -> Self {
        Self::Core(value)
    }
}

/// Whether a create-if-absent write created the OID path or found identical
/// bytes already present there.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PutDisposition {
    Created,
    AlreadyPresent,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PutResult {
    pub oid: String,
    pub kind: ObjectKind,
    pub byte_len: u64,
    pub disposition: PutDisposition,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ObjectInfo {
    pub oid: String,
    pub kind: ObjectKind,
    pub byte_len: u64,
}

/// Verified object metadata plus the parsed body for structured objects.
/// Blobs are verified incrementally and therefore do not materialize their body
/// in this value.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VerifiedObject {
    info: ObjectInfo,
    structured: Option<Value>,
}

impl VerifiedObject {
    pub fn info(&self) -> &ObjectInfo {
        &self.info
    }

    pub fn oid(&self) -> &str {
        &self.info.oid
    }

    pub const fn kind(&self) -> ObjectKind {
        self.info.kind
    }

    pub const fn byte_len(&self) -> u64 {
        self.info.byte_len
    }

    pub fn structured(&self) -> Option<&Value> {
        self.structured.as_ref()
    }

    #[cfg(test)]
    pub(crate) fn test_structured(oid: &str, kind: ObjectKind, value: Value) -> Self {
        Self::test_structured_with_len(oid, kind, value, 0)
    }

    #[cfg(test)]
    pub(crate) fn test_structured_with_len(
        oid: &str,
        kind: ObjectKind,
        value: Value,
        byte_len: u64,
    ) -> Self {
        Self {
            info: ObjectInfo {
                oid: oid.to_owned(),
                kind,
                byte_len,
            },
            structured: Some(value),
        }
    }
}

/// Result of a verified object-state lookup.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ObjectState {
    Present(ObjectInfo),
    Missing,
    Corrupt { kind: ObjectKind, detail: String },
}

/// Read boundary used by graph closure verification. Implementations must
/// return only digest-verified objects from `get_verified`.
pub trait ObjectStore {
    fn get_verified(&self, oid: &str) -> Result<Option<VerifiedObject>, StoreError>;
    fn list_oids(&self) -> Result<Vec<String>, StoreError>;
}

/// Object-store inventory boundary whose implementation bounds enumeration
/// work before it allocates or returns more than `max_objects` matching OIDs.
///
/// This is deliberately additive instead of a default method on
/// [`ObjectStore`]: adapting an already-materialized `list_oids` result would
/// not provide a real work or memory bound. Stores opt in only when they can
/// uphold this contract.
pub trait BoundedObjectStore: ObjectStore {
    fn list_oids_by_kind_limited(
        &self,
        kind: ObjectKind,
        max_objects: usize,
    ) -> Result<Vec<String>, StoreError>;
}

/// Local create-if-absent filesystem content-addressed store.
///
/// Publication uses a fully flushed temporary file followed by an atomic hard
/// link into the private OID layout. A hard link has no replace mode: concurrent
/// writers can create the name at most once, while losers byte-compare against
/// the winner before reporting [`PutDisposition::AlreadyPresent`].
#[derive(Debug)]
pub struct FileObjectStore {
    root: PathBuf,
    limits: StoreLimits,
    next_temp: AtomicU64,
    read_only: bool,
}

impl FileObjectStore {
    pub fn open(root: impl AsRef<Path>) -> Result<Self, StoreError> {
        Self::open_with_limits(root, StoreLimits::default())
    }

    pub fn open_with_limits(
        root: impl AsRef<Path>,
        limits: StoreLimits,
    ) -> Result<Self, StoreError> {
        validate_limits(limits)?;
        let requested_root = root.as_ref();
        fs::create_dir_all(requested_root)
            .map_err(|error| StoreError::io("create store root", requested_root, error))?;
        let root = fs::canonicalize(requested_root)
            .map_err(|error| StoreError::io("canonicalize store root", requested_root, error))?;
        if let Some(parent) = root.parent() {
            sync_directory(parent)?;
        }

        ensure_real_directory(&root.join(OBJECTS_DIRECTORY))?;
        ensure_real_directory(&root.join(TEMP_DIRECTORY))?;
        for kind in [
            ObjectKind::Blob,
            ObjectKind::Record,
            ObjectKind::Tree,
            ObjectKind::Commit,
        ] {
            ensure_real_directory(&root.join(OBJECTS_DIRECTORY).join(kind.prefix()))?;
        }

        Ok(Self {
            root,
            limits,
            next_temp: AtomicU64::new(0),
            read_only: false,
        })
    }

    /// Open an existing filesystem CAS without creating or syncing any path.
    ///
    /// The root and the complete fixed directory layout must already exist.
    /// Every mutation method on the returned handle fails with
    /// [`StoreError::ReadOnly`] before consuming or validating write input.
    pub fn open_existing_read_only(root: impl AsRef<Path>) -> Result<Self, StoreError> {
        let limits = StoreLimits::default();
        validate_limits(limits)?;
        let requested_root = root.as_ref();
        require_existing_real_directory(requested_root)?;
        let root = fs::canonicalize(requested_root).map_err(|error| {
            StoreError::io("canonicalize existing store root", requested_root, error)
        })?;
        require_existing_real_directory(&root.join(OBJECTS_DIRECTORY))?;
        require_existing_real_directory(&root.join(TEMP_DIRECTORY))?;
        for kind in [
            ObjectKind::Blob,
            ObjectKind::Record,
            ObjectKind::Tree,
            ObjectKind::Commit,
        ] {
            require_existing_real_directory(&root.join(OBJECTS_DIRECTORY).join(kind.prefix()))?;
        }

        Ok(Self {
            root,
            limits,
            next_temp: AtomicU64::new(0),
            read_only: true,
        })
    }

    pub const fn limits(&self) -> StoreLimits {
        self.limits
    }

    /// Stream a Blob through a bounded buffer, calculating SHA-256 while the
    /// original bytes are staged on the target filesystem.
    pub fn put_blob(&self, reader: impl Read) -> Result<PutResult, StoreError> {
        self.ensure_writable()?;
        self.put_blob_inner(None, reader)
    }

    /// As [`Self::put_blob`], while also checking a transport-supplied OID.
    pub fn put_blob_claimed(
        &self,
        claimed_oid: &str,
        reader: impl Read,
    ) -> Result<PutResult, StoreError> {
        self.ensure_writable()?;
        let kind = parse_oid(claimed_oid)?;
        if kind != ObjectKind::Blob {
            return Err(CoreError::new(
                ErrorCode::OidMismatch,
                format!("claimed OID {claimed_oid} is not a Blob OID"),
            )
            .into());
        }
        self.put_blob_inner(Some(claimed_oid), reader)
    }

    /// Parse restricted JSON, calculate its unchecked structured OID, and store
    /// canonical bytes. Concrete schema and semantic validation remain a caller
    /// precondition, matching `synapse-canonical`'s `*_unchecked` boundary.
    pub fn put_structured_unchecked(&self, input: &[u8]) -> Result<PutResult, StoreError> {
        self.ensure_writable()?;
        self.put_structured_inner(None, input, false)
    }

    /// Canonicalize an unchecked structured object while verifying its claimed
    /// OID. Concrete schema and semantic validation remain a caller precondition.
    pub fn put_structured_claimed_unchecked(
        &self,
        claimed_oid: &str,
        input: &[u8],
    ) -> Result<PutResult, StoreError> {
        self.ensure_writable()?;
        parse_oid(claimed_oid)?;
        self.put_structured_inner(Some(claimed_oid), input, false)
    }

    /// Restore already-canonical object bytes under a claimed OID. Every byte is
    /// rehashed. Structured bytes must already be in canonical form; this method
    /// never silently rewrites an archive entry during restore.
    pub fn put_verified_raw(
        &self,
        claimed_oid: &str,
        bytes: &[u8],
    ) -> Result<PutResult, StoreError> {
        self.ensure_writable()?;
        match parse_oid(claimed_oid)? {
            ObjectKind::Blob => self.put_blob_claimed(claimed_oid, bytes),
            _ => self.put_structured_inner(Some(claimed_oid), bytes, true),
        }
    }

    /// Return raw bytes only after recalculating and verifying the requested OID.
    /// Blob callers that need bounded memory should prefer [`Self::get_verified`]
    /// for validation followed by their own archive streaming policy.
    pub fn read_raw(&self, oid: &str) -> Result<Option<Vec<u8>>, StoreError> {
        let (kind, path) = self.validated_object_path(oid)?;
        let Some(metadata) = regular_file_metadata_if_present(&path, oid)? else {
            return Ok(None);
        };
        let limit = match kind {
            ObjectKind::Blob => self.limits.max_blob_bytes,
            _ => self.limits.structured.max_input_bytes as u64,
        };
        let bytes = read_file_limited(&path, metadata.len(), limit)?;
        self.verify_raw_bytes(oid, kind, &bytes)
            .map_err(|error| stored_object_error(oid, error))?;
        Ok(Some(bytes))
    }

    /// Return one digest-verified Blob without reading or allocating beyond a
    /// caller-supplied response limit.
    ///
    /// Unlike [`Self::read_raw`], this boundary rejects structured object OIDs
    /// and applies the tighter caller limit before allocation. The file reader
    /// also observes at most one byte beyond the limit so concurrent growth
    /// fails closed.
    pub fn read_verified_blob_limited(
        &self,
        oid: &str,
        max_bytes: u64,
    ) -> Result<Option<Vec<u8>>, StoreError> {
        if max_bytes == 0 {
            return Err(resource_limit(
                "verified Blob max_bytes must be greater than zero",
            ));
        }
        let (kind, path) = self.validated_object_path(oid)?;
        if kind != ObjectKind::Blob {
            return Err(CoreError::new(
                ErrorCode::ReferenceTypeMismatch,
                "verified Blob read requires a Blob OID",
            )
            .into());
        }
        let Some(metadata) = regular_file_metadata_if_present(&path, oid)? else {
            return Ok(None);
        };
        let effective_limit = max_bytes.min(self.limits.max_blob_bytes);
        let bytes = read_blob_file_exact_limited(&path, oid, metadata.len(), effective_limit)?;
        verify_blob_oid(oid, &bytes).map_err(|error| stored_object_error(oid, error))?;
        Ok(Some(bytes))
    }

    /// Verify an object while copying its raw stored bytes to a caller-owned
    /// stream. Blobs remain bounded-memory; structured objects retain their
    /// configured parse/canonicalization bound.
    pub fn copy_verified_to(
        &self,
        oid: &str,
        writer: &mut impl Write,
    ) -> Result<Option<ObjectInfo>, StoreError> {
        let (kind, path) = self.validated_object_path(oid)?;
        let Some(metadata) = regular_file_metadata_if_present(&path, oid)? else {
            return Ok(None);
        };
        let byte_len = metadata.len();
        if kind == ObjectKind::Blob {
            copy_verified_blob(
                &path,
                oid,
                self.limits.max_blob_bytes,
                self.limits.io_buffer_bytes,
                writer,
            )?;
        } else {
            let bytes = read_file_limited(
                &path,
                byte_len,
                self.limits.structured.max_input_bytes as u64,
            )?;
            self.verify_raw_bytes(oid, kind, &bytes)
                .map_err(|error| stored_object_error(oid, error))?;
            writer
                .write_all(&bytes)
                .map_err(|error| StoreError::io("write exported object", &path, error))?;
        }
        Ok(Some(ObjectInfo {
            oid: oid.to_owned(),
            kind,
            byte_len,
        }))
    }

    pub fn get_verified(&self, oid: &str) -> Result<Option<VerifiedObject>, StoreError> {
        <Self as ObjectStore>::get_verified(self, oid)
    }

    pub fn list_oids(&self) -> Result<Vec<String>, StoreError> {
        <Self as ObjectStore>::list_oids(self)
    }

    /// List the complete CAS inventory without retaining or returning more
    /// than `max_objects` OIDs.
    ///
    /// Unlike composing the per-kind bounded scans, this also bounds and
    /// validates the object-family directory itself. At most one object beyond
    /// the configured limit is examined to detect overflow.
    pub fn list_oids_limited(&self, max_objects: usize) -> Result<Vec<String>, StoreError> {
        scan_inventory_limited(&self.root, max_objects)
    }

    /// Return the current regular-file length for one syntactically valid OID.
    ///
    /// This is a resource-accounting boundary, not an integrity check. Callers
    /// must still use [`Self::get_verified`] before trusting the object. The
    /// store's cooperative append-only/no-removal contract must remain in force
    /// between this metadata read and the subsequent verified read.
    pub fn stored_object_byte_len(&self, oid: &str) -> Result<Option<u64>, StoreError> {
        let (_, path) = self.validated_object_path(oid)?;
        Ok(regular_file_metadata_if_present(&path, oid)?.map(|metadata| metadata.len()))
    }

    /// List one object kind without materializing the other CAS families.
    /// Returned OIDs and retained filesystem leaf entries are bounded by
    /// `max_objects`; at most one additional leaf is examined to detect
    /// overflow, and digest-prefix directory work has a fixed 256-entry cap.
    pub fn list_oids_by_kind_limited(
        &self,
        kind: ObjectKind,
        max_objects: usize,
    ) -> Result<Vec<String>, StoreError> {
        <Self as BoundedObjectStore>::list_oids_by_kind_limited(self, kind, max_objects)
    }

    pub fn object_state(&self, oid: &str) -> Result<ObjectState, StoreError> {
        let kind = parse_oid(oid)?;
        match self.get_verified(oid) {
            Ok(Some(object)) => Ok(ObjectState::Present(object.info)),
            Ok(None) => Ok(ObjectState::Missing),
            Err(StoreError::CorruptObject { detail, .. }) => {
                Ok(ObjectState::Corrupt { kind, detail })
            }
            Err(error) => Err(error),
        }
    }

    fn put_blob_inner(
        &self,
        claimed_oid: Option<&str>,
        mut reader: impl Read,
    ) -> Result<PutResult, StoreError> {
        self.ensure_writable()?;
        let mut staged = self.create_staged_file()?;
        let mut digest = Sha256::new();
        let mut byte_len = 0_u64;
        let mut buffer = vec![0_u8; self.limits.io_buffer_bytes];

        loop {
            let count = reader
                .read(&mut buffer)
                .map_err(|error| StoreError::io("read Blob input", &staged.path, error))?;
            if count == 0 {
                break;
            }
            byte_len = byte_len
                .checked_add(count as u64)
                .ok_or_else(|| resource_limit("Blob length overflowed the supported u64 range"))?;
            if byte_len > self.limits.max_blob_bytes {
                return Err(resource_limit(format!(
                    "Blob is larger than the {} byte limit",
                    self.limits.max_blob_bytes
                )));
            }
            digest.update(&buffer[..count]);
            staged
                .file
                .write_all(&buffer[..count])
                .map_err(|error| StoreError::io("write staged Blob", &staged.path, error))?;
        }

        staged
            .file
            .flush()
            .map_err(|error| StoreError::io("flush staged Blob", &staged.path, error))?;
        staged
            .file
            .sync_all()
            .map_err(|error| StoreError::io("sync staged Blob", &staged.path, error))?;
        let oid = format!("blob:sg-oid-v1:sha256:{}", lower_hex(digest.finalize()));
        if let Some(claimed_oid) = claimed_oid
            && claimed_oid != oid
        {
            return Err(CoreError::new(
                ErrorCode::OidMismatch,
                format!("claimed {claimed_oid}, expected {oid}"),
            )
            .into());
        }
        self.publish_staged(staged, oid, ObjectKind::Blob, byte_len)
    }

    fn put_structured_inner(
        &self,
        claimed_oid: Option<&str>,
        input: &[u8],
        require_canonical_input: bool,
    ) -> Result<PutResult, StoreError> {
        self.ensure_writable()?;
        let value = parse_strict_with_limits(input, self.limits.structured)?;
        let canonical = canonical_bytes_with_limits(&value, self.limits.structured)?;
        if require_canonical_input && input != canonical {
            return Err(CoreError::new(
                ErrorCode::SchemaInvalid,
                "structured restore input is not canonical JSON",
            )
            .into());
        }
        let oid = structured_oid_unchecked_with_limits(&value, self.limits.structured)?;
        if let Some(claimed_oid) = claimed_oid
            && claimed_oid != oid
        {
            return Err(CoreError::new(
                ErrorCode::OidMismatch,
                format!("claimed {claimed_oid}, expected {oid}"),
            )
            .into());
        }

        let mut staged = self.create_staged_file()?;
        staged.file.write_all(&canonical).map_err(|error| {
            StoreError::io("write staged structured object", &staged.path, error)
        })?;
        staged.file.flush().map_err(|error| {
            StoreError::io("flush staged structured object", &staged.path, error)
        })?;
        staged.file.sync_all().map_err(|error| {
            StoreError::io("sync staged structured object", &staged.path, error)
        })?;
        self.publish_staged(
            staged,
            oid.clone(),
            parse_oid(&oid)?,
            canonical.len() as u64,
        )
    }

    fn publish_staged(
        &self,
        staged: StagedFile,
        oid: String,
        kind: ObjectKind,
        byte_len: u64,
    ) -> Result<PutResult, StoreError> {
        self.ensure_writable()?;
        let target = self.object_path_for_valid_oid(&oid, kind);
        let parent = target
            .parent()
            .expect("private object path always has a parent");
        ensure_real_directory(parent)?;

        let disposition = match fs::hard_link(&staged.path, &target) {
            Ok(()) => {
                sync_directory(parent)?;
                PutDisposition::Created
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                compare_regular_files(&staged.path, &target, self.limits.io_buffer_bytes, &oid)?;
                PutDisposition::AlreadyPresent
            }
            Err(error) => return Err(StoreError::io("publish object", &target, error)),
        };

        Ok(PutResult {
            oid,
            kind,
            byte_len,
            disposition,
        })
    }

    fn create_staged_file(&self) -> Result<StagedFile, StoreError> {
        self.ensure_writable()?;
        let directory = self.root.join(TEMP_DIRECTORY);
        for _ in 0..1024 {
            let counter = self.next_temp.fetch_add(1, Ordering::Relaxed);
            let path = directory.join(format!("{}-{counter}.tmp", std::process::id()));
            match OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(file) => return Ok(StagedFile { path, file }),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(StoreError::io("create temporary object", path, error)),
            }
        }
        Err(StoreError::io(
            "create temporary object",
            directory,
            io::Error::new(
                io::ErrorKind::AlreadyExists,
                "temporary name attempts exhausted",
            ),
        ))
    }

    fn validated_object_path(&self, oid: &str) -> Result<(ObjectKind, PathBuf), StoreError> {
        let kind = parse_oid(oid)?;
        Ok((kind, self.object_path_for_valid_oid(oid, kind)))
    }

    fn object_path_for_valid_oid(&self, oid: &str, kind: ObjectKind) -> PathBuf {
        let digest = oid
            .rsplit(':')
            .next()
            .expect("parse_oid already established a digest");
        self.root
            .join(OBJECTS_DIRECTORY)
            .join(kind.prefix())
            .join(&digest[..2])
            .join(&digest[2..])
    }

    fn verify_raw_bytes(
        &self,
        oid: &str,
        kind: ObjectKind,
        bytes: &[u8],
    ) -> Result<Option<Value>, CoreError> {
        if kind == ObjectKind::Blob {
            verify_blob_oid(oid, bytes)?;
            return Ok(None);
        }
        let value = parse_strict_with_limits(bytes, self.limits.structured)?;
        let canonical = canonical_bytes_with_limits(&value, self.limits.structured)?;
        if canonical != bytes {
            return Err(CoreError::new(
                ErrorCode::OidMismatch,
                "stored structured bytes are not canonical JSON",
            ));
        }
        verify_claimed_oid_unchecked_with_limits(oid, &value, self.limits.structured)?;
        Ok(Some(value))
    }

    fn ensure_writable(&self) -> Result<(), StoreError> {
        if self.read_only {
            Err(StoreError::ReadOnly)
        } else {
            Ok(())
        }
    }

    pub(crate) fn inventory(&self) -> Result<StoreInventory, StoreError> {
        scan_inventory(&self.root)
    }
}

impl ObjectStore for FileObjectStore {
    fn get_verified(&self, oid: &str) -> Result<Option<VerifiedObject>, StoreError> {
        let (kind, path) = self.validated_object_path(oid)?;
        let Some(metadata) = regular_file_metadata_if_present(&path, oid)? else {
            return Ok(None);
        };
        let byte_len = metadata.len();
        let structured = if kind == ObjectKind::Blob {
            verify_blob_file(
                &path,
                oid,
                self.limits.max_blob_bytes,
                self.limits.io_buffer_bytes,
            )?;
            None
        } else {
            let bytes = read_file_limited(
                &path,
                byte_len,
                self.limits.structured.max_input_bytes as u64,
            )?;
            self.verify_raw_bytes(oid, kind, &bytes)
                .map_err(|error| stored_object_error(oid, error))?
        };
        Ok(Some(VerifiedObject {
            info: ObjectInfo {
                oid: oid.to_owned(),
                kind,
                byte_len,
            },
            structured,
        }))
    }

    fn list_oids(&self) -> Result<Vec<String>, StoreError> {
        let inventory = self.inventory()?;
        if let Some(invalid) = inventory.invalid_paths.into_iter().next() {
            return Err(StoreError::InvalidStoreLayout {
                path: invalid.path,
                detail: invalid.detail,
            });
        }
        Ok(inventory.oids)
    }
}

impl BoundedObjectStore for FileObjectStore {
    fn list_oids_by_kind_limited(
        &self,
        kind: ObjectKind,
        max_objects: usize,
    ) -> Result<Vec<String>, StoreError> {
        scan_kind_inventory_limited(&self.root, kind, max_objects)
    }
}

#[derive(Debug)]
struct StagedFile {
    path: PathBuf,
    file: File,
}

impl Drop for StagedFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

#[derive(Debug)]
pub(crate) struct InvalidStorePath {
    pub path: PathBuf,
    pub detail: String,
}

#[derive(Debug)]
pub(crate) struct StoreInventory {
    pub oids: Vec<String>,
    pub invalid_paths: Vec<InvalidStorePath>,
}

fn scan_inventory(root: &Path) -> Result<StoreInventory, StoreError> {
    let objects = root.join(OBJECTS_DIRECTORY);
    let mut oids = Vec::new();
    let mut invalid_paths = Vec::new();
    let entries = fs::read_dir(&objects)
        .map_err(|error| StoreError::io("scan object directory", &objects, error))?;
    for entry in entries {
        let entry = entry.map_err(|error| StoreError::io("read object entry", &objects, error))?;
        let path = entry.path();
        let Some(family) = entry.file_name().to_str().map(str::to_owned) else {
            invalid_paths.push(invalid_path(root, &path, "object family is not UTF-8"));
            continue;
        };
        let kind = match family.as_str() {
            "blob" => ObjectKind::Blob,
            "record" => ObjectKind::Record,
            "tree" => ObjectKind::Tree,
            "commit" => ObjectKind::Commit,
            _ => {
                invalid_paths.push(invalid_path(root, &path, "unknown object family"));
                continue;
            }
        };
        let metadata = fs::symlink_metadata(&path)
            .map_err(|error| StoreError::io("inspect object family", &path, error))?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            invalid_paths.push(invalid_path(
                root,
                &path,
                "object family is not a real directory",
            ));
            continue;
        }
        scan_family(root, &path, kind, &mut oids, &mut invalid_paths)?;
    }
    oids.sort_unstable();
    oids.dedup();
    invalid_paths.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(StoreInventory {
        oids,
        invalid_paths,
    })
}

/// Scan every object family while retaining at most `max_objects` OIDs across
/// the complete CAS. The private layout has exactly four family directories,
/// so an extra family is rejected before attacker-controlled directory work can
/// grow without a bound.
fn scan_inventory_limited(root: &Path, max_objects: usize) -> Result<Vec<String>, StoreError> {
    if max_objects == 0 {
        return Err(resource_limit(
            "object inventory max_objects must be greater than zero",
        ));
    }

    let objects = root.join(OBJECTS_DIRECTORY);
    let families = read_directory_at_most(&objects, 4, "read object family entry", || {
        StoreError::InvalidStoreLayout {
            path: PathBuf::from(OBJECTS_DIRECTORY),
            detail: "object directory contains more than four family entries".to_owned(),
        }
    })?;
    let mut family_names = families
        .into_iter()
        .map(|entry| {
            entry.file_name().into_string().map_err(|_| {
                let path = entry.path();
                StoreError::InvalidStoreLayout {
                    path: path.strip_prefix(root).unwrap_or(&path).to_path_buf(),
                    detail: "object family is not UTF-8".to_owned(),
                }
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    family_names.sort_unstable();
    let expected_families = ["blob", "commit", "record", "tree"];
    if family_names
        .iter()
        .map(String::as_str)
        .ne(expected_families)
    {
        return Err(StoreError::InvalidStoreLayout {
            path: PathBuf::from(OBJECTS_DIRECTORY),
            detail: "object directory must contain exactly blob, commit, record, and tree families"
                .to_owned(),
        });
    }

    let mut oids = Vec::new();
    for kind in [
        ObjectKind::Blob,
        ObjectKind::Record,
        ObjectKind::Tree,
        ObjectKind::Commit,
    ] {
        let remaining = max_objects - oids.len();
        // The internal scan accepts zero so later empty families can still be
        // validated after an earlier family exactly exhausts the global limit.
        // A non-empty later family examines one extra leaf and rejects.
        let mut family_oids =
            scan_kind_inventory_at_most(root, kind, remaining).map_err(|error| {
                if error.code() == Some(ErrorCode::ResourceLimit) {
                    resource_limit(format!(
                        "object inventory exceeds max_objects {max_objects}"
                    ))
                } else {
                    error
                }
            })?;
        oids.append(&mut family_oids);
    }
    oids.sort_unstable();
    Ok(oids)
}

/// Scan one object family without first materializing the complete CAS
/// inventory. A valid SHA-256 layout has at most 256 prefix directories, so
/// directory work is bounded by `max_objects` plus that fixed overhead.
fn scan_kind_inventory_limited(
    root: &Path,
    kind: ObjectKind,
    max_objects: usize,
) -> Result<Vec<String>, StoreError> {
    if max_objects == 0 {
        return Err(resource_limit(format!(
            "{} inventory max_objects must be greater than zero",
            kind.prefix()
        )));
    }

    scan_kind_inventory_at_most(root, kind, max_objects)
}

fn scan_kind_inventory_at_most(
    root: &Path,
    kind: ObjectKind,
    max_objects: usize,
) -> Result<Vec<String>, StoreError> {
    let family_path = root.join(OBJECTS_DIRECTORY).join(kind.prefix());
    let metadata = fs::symlink_metadata(&family_path)
        .map_err(|error| StoreError::io("inspect object family", &family_path, error))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(StoreError::InvalidStoreLayout {
            path: family_path
                .strip_prefix(root)
                .unwrap_or(&family_path)
                .to_path_buf(),
            detail: "object family is not a real directory".to_owned(),
        });
    }

    // The canonical SHA-256 layout admits exactly 256 possible two-hex prefix
    // names. Read one extra entry, but never an attacker-sized prefix Vec.
    let mut prefixes =
        read_directory_at_most(&family_path, 256, "read object prefix entry", || {
            StoreError::InvalidStoreLayout {
                path: family_path
                    .strip_prefix(root)
                    .unwrap_or(&family_path)
                    .to_path_buf(),
                detail: "object family contains more than 256 digest prefix entries".to_owned(),
            }
        })?;
    prefixes.sort_by_key(fs::DirEntry::file_name);

    let mut examined_objects = 0_usize;
    let mut oids = Vec::new();
    for entry in prefixes {
        let prefix_path = entry.path();
        let prefix = entry.file_name();
        let Some(prefix) = prefix.to_str() else {
            return Err(StoreError::InvalidStoreLayout {
                path: prefix_path
                    .strip_prefix(root)
                    .unwrap_or(&prefix_path)
                    .to_path_buf(),
                detail: "digest prefix is not UTF-8".to_owned(),
            });
        };
        let metadata = fs::symlink_metadata(&prefix_path)
            .map_err(|error| StoreError::io("inspect digest prefix", &prefix_path, error))?;
        if prefix.len() != 2
            || !prefix.bytes().all(is_lower_hex)
            || metadata.file_type().is_symlink()
            || !metadata.is_dir()
        {
            return Err(StoreError::InvalidStoreLayout {
                path: prefix_path
                    .strip_prefix(root)
                    .unwrap_or(&prefix_path)
                    .to_path_buf(),
                detail: "digest prefix must be a two-hex-character real directory".to_owned(),
            });
        }

        let remaining = max_objects - examined_objects;
        let mut objects =
            read_directory_at_most(&prefix_path, remaining, "read digest entry", || {
                inventory_object_limit(kind, max_objects)
            })?;
        examined_objects = examined_objects
            .checked_add(objects.len())
            .ok_or_else(|| inventory_object_limit(kind, max_objects))?;
        objects.sort_by_key(fs::DirEntry::file_name);
        for object in objects {
            let path = object.path();
            let suffix = object.file_name();
            let Some(suffix) = suffix.to_str() else {
                return Err(StoreError::InvalidStoreLayout {
                    path: path.strip_prefix(root).unwrap_or(&path).to_path_buf(),
                    detail: "digest suffix is not UTF-8".to_owned(),
                });
            };
            let metadata = fs::symlink_metadata(&path)
                .map_err(|error| StoreError::io("inspect object file", &path, error))?;
            if suffix.len() != 62
                || !suffix.bytes().all(is_lower_hex)
                || metadata.file_type().is_symlink()
                || !metadata.is_file()
            {
                return Err(StoreError::InvalidStoreLayout {
                    path: path.strip_prefix(root).unwrap_or(&path).to_path_buf(),
                    detail:
                        "object must be a regular file named by the remaining 62 digest hex characters"
                            .to_owned(),
                });
            }
            let oid = format!("{}:sg-oid-v1:sha256:{prefix}{suffix}", kind.prefix());
            parse_oid(&oid)?;
            oids.push(oid);
        }
    }
    oids.sort_unstable();
    oids.dedup();
    Ok(oids)
}

fn read_directory_at_most<F>(
    path: &Path,
    max_entries: usize,
    operation: &'static str,
    exceeded: F,
) -> Result<Vec<fs::DirEntry>, StoreError>
where
    F: FnOnce() -> StoreError,
{
    let entries = fs::read_dir(path).map_err(|error| StoreError::io(operation, path, error))?;
    let mut result = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| StoreError::io(operation, path, error))?;
        if result.len() == max_entries {
            return Err(exceeded());
        }
        result.push(entry);
    }
    Ok(result)
}

fn scan_family(
    root: &Path,
    family_path: &Path,
    kind: ObjectKind,
    oids: &mut Vec<String>,
    invalid_paths: &mut Vec<InvalidStorePath>,
) -> Result<(), StoreError> {
    let entries = fs::read_dir(family_path)
        .map_err(|error| StoreError::io("scan object family", family_path, error))?;
    for entry in entries {
        let entry = entry
            .map_err(|error| StoreError::io("read object prefix entry", family_path, error))?;
        let prefix_path = entry.path();
        let prefix = entry.file_name();
        let Some(prefix) = prefix.to_str() else {
            invalid_paths.push(invalid_path(
                root,
                &prefix_path,
                "digest prefix is not UTF-8",
            ));
            continue;
        };
        let metadata = fs::symlink_metadata(&prefix_path)
            .map_err(|error| StoreError::io("inspect digest prefix", &prefix_path, error))?;
        if prefix.len() != 2
            || !prefix.bytes().all(is_lower_hex)
            || metadata.file_type().is_symlink()
            || !metadata.is_dir()
        {
            invalid_paths.push(invalid_path(
                root,
                &prefix_path,
                "digest prefix must be a two-hex-character real directory",
            ));
            continue;
        }
        scan_prefix(root, &prefix_path, prefix, kind, oids, invalid_paths)?;
    }
    Ok(())
}

fn scan_prefix(
    root: &Path,
    prefix_path: &Path,
    prefix: &str,
    kind: ObjectKind,
    oids: &mut Vec<String>,
    invalid_paths: &mut Vec<InvalidStorePath>,
) -> Result<(), StoreError> {
    let entries = fs::read_dir(prefix_path)
        .map_err(|error| StoreError::io("scan digest prefix", prefix_path, error))?;
    for entry in entries {
        let entry =
            entry.map_err(|error| StoreError::io("read digest entry", prefix_path, error))?;
        let path = entry.path();
        let suffix = entry.file_name();
        let Some(suffix) = suffix.to_str() else {
            invalid_paths.push(invalid_path(root, &path, "digest suffix is not UTF-8"));
            continue;
        };
        let metadata = fs::symlink_metadata(&path)
            .map_err(|error| StoreError::io("inspect object file", &path, error))?;
        if suffix.len() != 62
            || !suffix.bytes().all(is_lower_hex)
            || metadata.file_type().is_symlink()
            || !metadata.is_file()
        {
            invalid_paths.push(invalid_path(
                root,
                &path,
                "object must be a regular file named by the remaining 62 digest hex characters",
            ));
            continue;
        }
        let oid = format!("{}:sg-oid-v1:sha256:{prefix}{suffix}", kind.prefix());
        if parse_oid(&oid).is_err() {
            invalid_paths.push(invalid_path(
                root,
                &path,
                "object path does not form a valid OID",
            ));
        } else {
            oids.push(oid);
        }
    }
    Ok(())
}

fn invalid_path(root: &Path, path: &Path, detail: impl Into<String>) -> InvalidStorePath {
    InvalidStorePath {
        path: path.strip_prefix(root).unwrap_or(path).to_path_buf(),
        detail: detail.into(),
    }
}

fn validate_limits(limits: StoreLimits) -> Result<(), StoreError> {
    if limits.max_blob_bytes == 0 {
        return Err(resource_limit("max_blob_bytes must be greater than zero"));
    }
    if limits.io_buffer_bytes == 0 {
        return Err(resource_limit("io_buffer_bytes must be greater than zero"));
    }
    // Exercise canonical's limit validator without duplicating its hard ceiling.
    parse_strict_with_limits(b"null", limits.structured)?;
    Ok(())
}

fn resource_limit(message: impl Into<String>) -> StoreError {
    CoreError::new(ErrorCode::ResourceLimit, message).into()
}

fn inventory_object_limit(kind: ObjectKind, max_objects: usize) -> StoreError {
    resource_limit(format!(
        "{} inventory exceeds max_objects {max_objects}",
        kind.prefix()
    ))
}

fn stored_object_error(oid: &str, error: CoreError) -> StoreError {
    if error.code() == ErrorCode::ResourceLimit {
        StoreError::Core(error)
    } else {
        StoreError::CorruptObject {
            oid: oid.to_owned(),
            detail: error.to_string(),
        }
    }
}

fn ensure_real_directory(path: &Path) -> Result<(), StoreError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return Err(StoreError::InvalidStoreLayout {
                    path: path.to_path_buf(),
                    detail: "expected a real directory, not a file or symlink".to_owned(),
                });
            }
            Ok(())
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => match fs::create_dir(path) {
            Ok(()) => {
                if let Some(parent) = path.parent() {
                    sync_directory(parent)?;
                }
                Ok(())
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                ensure_real_directory(path)
            }
            Err(error) => Err(StoreError::io("create directory", path, error)),
        },
        Err(error) => Err(StoreError::io("inspect directory", path, error)),
    }
}

fn require_existing_real_directory(path: &Path) -> Result<(), StoreError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if !metadata.file_type().is_symlink() && metadata.is_dir() => Ok(()),
        Ok(_) => Err(StoreError::InvalidStoreLayout {
            path: path.to_path_buf(),
            detail: "expected an existing real directory, not a file or symlink".to_owned(),
        }),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            Err(StoreError::InvalidStoreLayout {
                path: path.to_path_buf(),
                detail: "required directory is missing".to_owned(),
            })
        }
        Err(error) => Err(StoreError::io("inspect existing directory", path, error)),
    }
}

fn regular_file_metadata_if_present(
    path: &Path,
    oid: &str,
) -> Result<Option<fs::Metadata>, StoreError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() || !metadata.is_file() {
                return Err(StoreError::CorruptObject {
                    oid: oid.to_owned(),
                    detail: "OID path is not a regular file".to_owned(),
                });
            }
            Ok(Some(metadata))
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(StoreError::io("inspect object", path, error)),
    }
}

fn read_file_limited(path: &Path, metadata_len: u64, limit: u64) -> Result<Vec<u8>, StoreError> {
    if metadata_len > limit {
        return Err(resource_limit(format!(
            "stored object is {metadata_len} bytes; verification limit is {limit}"
        )));
    }
    let capacity = usize::try_from(metadata_len)
        .map_err(|_| resource_limit("stored object does not fit in addressable memory"))?;
    let file = File::open(path).map_err(|error| StoreError::io("open object", path, error))?;
    let read_bound = limit.checked_add(1).unwrap_or(limit);
    let mut reader = BufReader::new(file).take(read_bound);
    let mut bytes = Vec::with_capacity(capacity);
    reader
        .read_to_end(&mut bytes)
        .map_err(|error| StoreError::io("read object", path, error))?;
    if bytes.len() as u64 > limit {
        return Err(resource_limit(format!(
            "stored object grew beyond the {limit} byte verification limit"
        )));
    }
    Ok(bytes)
}

fn read_blob_file_exact_limited(
    path: &Path,
    oid: &str,
    metadata_len: u64,
    limit: u64,
) -> Result<Vec<u8>, StoreError> {
    if metadata_len > limit {
        return Err(resource_limit(format!(
            "stored Blob is {metadata_len} bytes; verification limit is {limit}"
        )));
    }
    let length = usize::try_from(metadata_len)
        .map_err(|_| resource_limit("stored Blob does not fit in addressable memory"))?;
    let mut file = File::open(path).map_err(|error| StoreError::io("open Blob", path, error))?;
    let mut bytes = vec![0_u8; length];
    if let Err(error) = file.read_exact(&mut bytes) {
        if error.kind() == io::ErrorKind::UnexpectedEof {
            return Err(StoreError::CorruptObject {
                oid: oid.to_owned(),
                detail: "Blob length shrank while it was being read".to_owned(),
            });
        }
        return Err(StoreError::io("read Blob", path, error));
    }
    let mut extra = [0_u8; 1];
    match file.read(&mut extra) {
        Ok(0) => Ok(bytes),
        Ok(_) => Err(StoreError::CorruptObject {
            oid: oid.to_owned(),
            detail: format!(
                "Blob length grew after metadata inspection (verification limit {limit})"
            ),
        }),
        Err(error) => Err(StoreError::io("probe Blob length", path, error)),
    }
}

fn verify_blob_file(
    path: &Path,
    oid: &str,
    max_blob_bytes: u64,
    buffer_bytes: usize,
) -> Result<(), StoreError> {
    let file = File::open(path).map_err(|error| StoreError::io("open Blob", path, error))?;
    let mut reader = BufReader::with_capacity(buffer_bytes, file);
    let mut digest = Sha256::new();
    let mut total = 0_u64;
    let mut buffer = vec![0_u8; buffer_bytes];
    loop {
        let count = reader
            .read(&mut buffer)
            .map_err(|error| StoreError::io("read Blob", path, error))?;
        if count == 0 {
            break;
        }
        total = total
            .checked_add(count as u64)
            .ok_or_else(|| resource_limit("stored Blob length overflowed u64"))?;
        if total > max_blob_bytes {
            return Err(resource_limit(format!(
                "stored Blob is larger than the {max_blob_bytes} byte verification limit"
            )));
        }
        digest.update(&buffer[..count]);
    }
    let actual = format!("blob:sg-oid-v1:sha256:{}", lower_hex(digest.finalize()));
    if actual != oid {
        return Err(StoreError::CorruptObject {
            oid: oid.to_owned(),
            detail: format!("OID digest mismatch; content calculates to {actual}"),
        });
    }
    Ok(())
}

fn copy_verified_blob(
    path: &Path,
    oid: &str,
    max_blob_bytes: u64,
    buffer_bytes: usize,
    writer: &mut impl Write,
) -> Result<(), StoreError> {
    let file = File::open(path).map_err(|error| StoreError::io("open Blob", path, error))?;
    let mut reader = BufReader::with_capacity(buffer_bytes, file);
    let mut digest = Sha256::new();
    let mut total = 0_u64;
    let mut buffer = vec![0_u8; buffer_bytes];
    loop {
        let count = reader
            .read(&mut buffer)
            .map_err(|error| StoreError::io("read Blob", path, error))?;
        if count == 0 {
            break;
        }
        total = total
            .checked_add(count as u64)
            .ok_or_else(|| resource_limit("stored Blob length overflowed u64"))?;
        if total > max_blob_bytes {
            return Err(resource_limit(format!(
                "stored Blob is larger than the {max_blob_bytes} byte verification limit"
            )));
        }
        digest.update(&buffer[..count]);
        writer
            .write_all(&buffer[..count])
            .map_err(|error| StoreError::io("write exported Blob", path, error))?;
    }
    let actual = format!("blob:sg-oid-v1:sha256:{}", lower_hex(digest.finalize()));
    if actual != oid {
        return Err(StoreError::CorruptObject {
            oid: oid.to_owned(),
            detail: format!("OID digest mismatch; content calculates to {actual}"),
        });
    }
    Ok(())
}

fn compare_regular_files(
    staged: &Path,
    existing: &Path,
    buffer_bytes: usize,
    oid: &str,
) -> Result<(), StoreError> {
    let metadata = fs::symlink_metadata(existing)
        .map_err(|error| StoreError::io("inspect existing object", existing, error))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(StoreError::CorruptObject {
            oid: oid.to_owned(),
            detail: "existing OID path is not a regular file".to_owned(),
        });
    }
    let staged_metadata = fs::metadata(staged)
        .map_err(|error| StoreError::io("inspect staged object", staged, error))?;
    if staged_metadata.len() != metadata.len() {
        return Err(StoreError::CorruptObject {
            oid: oid.to_owned(),
            detail: "existing OID path contains different bytes".to_owned(),
        });
    }

    let mut left = BufReader::with_capacity(
        buffer_bytes,
        File::open(staged).map_err(|error| StoreError::io("open staged object", staged, error))?,
    );
    let mut right = BufReader::with_capacity(
        buffer_bytes,
        File::open(existing)
            .map_err(|error| StoreError::io("open existing object", existing, error))?,
    );
    let mut left_buffer = vec![0_u8; buffer_bytes];
    let mut right_buffer = vec![0_u8; buffer_bytes];
    loop {
        let left_count = left
            .read(&mut left_buffer)
            .map_err(|error| StoreError::io("read staged object", staged, error))?;
        let right_count = right
            .read(&mut right_buffer)
            .map_err(|error| StoreError::io("read existing object", existing, error))?;
        if left_count != right_count || left_buffer[..left_count] != right_buffer[..right_count] {
            return Err(StoreError::CorruptObject {
                oid: oid.to_owned(),
                detail: "existing OID path contains different bytes".to_owned(),
            });
        }
        if left_count == 0 {
            return Ok(());
        }
    }
}

fn is_lower_hex(byte: u8) -> bool {
    byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)
}

fn lower_hex(bytes: impl IntoIterator<Item = u8>) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let bytes = bytes.into_iter();
    let mut output = String::with_capacity(bytes.size_hint().0.saturating_mul(2));
    for byte in bytes {
        output.push(DIGITS[usize::from(byte >> 4)] as char);
        output.push(DIGITS[usize::from(byte & 0x0f)] as char);
    }
    output
}

#[cfg(unix)]
fn sync_directory(path: &Path) -> Result<(), StoreError> {
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| StoreError::io("sync object directory", path, error))
}

#[cfg(not(unix))]
fn sync_directory(_path: &Path) -> Result<(), StoreError> {
    Ok(())
}
