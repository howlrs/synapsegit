//! Import-inbox manifest v1 and a manifest-last candidate writer.
//!
//! The localhost service reads candidates that a producer placed under an
//! inbox root; `synapse inbox put` is the reference producer.  Both sides use
//! the types and limits below, so a candidate written here is exactly what the
//! service accepts.  Writing a candidate never opens a repository, creates a
//! Proposal, or records a decision: a person still reviews and decides in the
//! localhost UI.

use crate::{CreatorError, CreatorGenerationNote, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

/// The manifest `version` value accepted by this contract.
pub const IMPORT_INBOX_MANIFEST_VERSION: &str = "synapsegit-import-inbox-v1";
/// The manifest leaf name inside a candidate directory.
pub const IMPORT_INBOX_MANIFEST_NAME: &str = "manifest.json";
/// Largest accepted manifest.
pub const IMPORT_INBOX_MANIFEST_MAX_BYTES: u64 = 64 * 1024;
/// Largest accepted image file.
pub const IMPORT_INBOX_FILE_MAX_BYTES: u64 = 64 * 1024 * 1024;
/// Largest accepted `metadata.subject_label`, in UTF-8 bytes.
pub const IMPORT_INBOX_SUBJECT_MAX_BYTES: usize = 500;
/// Largest accepted `metadata.creator_name`, in UTF-8 bytes.
pub const IMPORT_INBOX_CREATOR_MAX_BYTES: usize = 300;
/// Leaf names written by [`put_import_inbox_candidate`].  The manifest format
/// itself accepts any single leaf name.
pub const IMPORT_INBOX_ORIGINAL_NAME: &str = "original";
pub const IMPORT_INBOX_CURRENT_NAME: &str = "current";
pub const IMPORT_INBOX_AI_OUTPUT_NAME: &str = "ai-output";

/// One strict `synapsegit-import-inbox-v1` manifest.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(remote = "Self", deny_unknown_fields)]
pub struct ImportInboxManifest {
    pub version: String,
    pub original: ImportInboxFile,
    pub current: ImportInboxFile,
    pub ai_output: ImportInboxFile,
    pub metadata: ImportInboxMetadata,
}

json_object_serde!(ImportInboxManifest);

/// One file entry of an inbox manifest.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(remote = "Self", deny_unknown_fields)]
pub struct ImportInboxFile {
    pub name: String,
    pub size: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
}

json_object_serde!(ImportInboxFile);

/// Caller-supplied display metadata of an inbox candidate.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(remote = "Self", deny_unknown_fields)]
pub struct ImportInboxMetadata {
    pub subject_label: String,
    pub creator_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generation_note: Option<CreatorGenerationNote>,
}

json_object_serde!(ImportInboxMetadata);

/// Why an inbox manifest is not acceptable.  The messages are shown by the
/// localhost service and are part of its user-facing behavior.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ImportInboxManifestError {
    UnsupportedVersion,
    InvalidFile,
    InvalidSha256,
    DisplayNames,
    GenerationNote,
}

impl fmt::Display for ImportInboxManifestError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::UnsupportedVersion => "The inbox manifest version is unsupported.",
            Self::InvalidFile => "Each inbox manifest file name must name one bounded file.",
            Self::InvalidSha256 => "An inbox SHA-256 must be 64 hexadecimal characters.",
            Self::DisplayNames => "The inbox metadata display names exceed their limits.",
            Self::GenerationNote => "The inbox generation note exceeds its limits.",
        })
    }
}

impl std::error::Error for ImportInboxManifestError {}

impl ImportInboxManifest {
    /// Check the version, file entries, display names, and generation note.
    pub fn validate(&self) -> std::result::Result<(), ImportInboxManifestError> {
        if self.version != IMPORT_INBOX_MANIFEST_VERSION {
            return Err(ImportInboxManifestError::UnsupportedVersion);
        }
        for file in [&self.original, &self.current, &self.ai_output] {
            file.validate()?;
        }
        if !display_name_is_valid(&self.metadata.subject_label, IMPORT_INBOX_SUBJECT_MAX_BYTES)
            || !display_name_is_valid(&self.metadata.creator_name, IMPORT_INBOX_CREATOR_MAX_BYTES)
        {
            return Err(ImportInboxManifestError::DisplayNames);
        }
        if let Some(note) = &self.metadata.generation_note {
            note.validate()
                .map_err(|_| ImportInboxManifestError::GenerationNote)?;
        }
        Ok(())
    }
}

impl ImportInboxFile {
    fn validate(&self) -> std::result::Result<(), ImportInboxManifestError> {
        // The v1 schema allows one leaf name without NUL, `/`, or `\`.
        if self.name.is_empty()
            || self.name.len() > 255
            || self.name.contains(['\0', '/', '\\'])
            || self.name == "."
            || self.name == ".."
            || self.size > IMPORT_INBOX_FILE_MAX_BYTES
        {
            return Err(ImportInboxManifestError::InvalidFile);
        }
        if let Some(hash) = &self.sha256
            && (hash.len() != 64 || !hash.bytes().all(|byte| byte.is_ascii_hexdigit()))
        {
            return Err(ImportInboxManifestError::InvalidSha256);
        }
        Ok(())
    }
}

fn display_name_is_valid(value: &str, limit: usize) -> bool {
    !value.is_empty() && value.len() <= limit
}

/// Whether `value` is a valid inbox candidate slug, `[a-z][a-z0-9-]{0,63}`.
pub fn is_import_inbox_slug(value: &str) -> bool {
    let bytes = value.as_bytes();
    (1..=64).contains(&bytes.len())
        && bytes[0].is_ascii_lowercase()
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'-')
}

/// Canonical session suggestion shared by the localhost Inbox view and direct
/// Inbox decision clients. Long slugs retain a readable prefix plus a digest
/// suffix so distinct candidates stay distinct within the session grammar.
pub fn suggested_import_inbox_session(slug: &str) -> Result<String> {
    if !is_import_inbox_slug(slug) {
        return Err(CreatorError::InvalidArgument(
            "inbox slug must match [a-z][a-z0-9-]{0,63}".into(),
        ));
    }
    let session = format!("inbox-{slug}");
    if session.len() <= 64 {
        return Ok(session);
    }
    let digest = Sha256::digest(slug.as_bytes());
    Ok(format!(
        "inbox-{}-{:02x}{:02x}{:02x}{:02x}",
        slug[..49].trim_end_matches('-'),
        digest[0],
        digest[1],
        digest[2],
        digest[3]
    ))
}

/// Inputs for [`put_import_inbox_candidate`].
#[derive(Clone, Copy, Debug)]
pub struct ImportInboxCandidate<'a> {
    /// An existing inbox root; it is never created.
    pub inbox_root: &'a Path,
    pub slug: &'a str,
    pub original: &'a Path,
    pub current: &'a Path,
    pub ai_output: &'a Path,
    pub subject_label: &'a str,
    pub creator_name: &'a str,
    pub generation_note: Option<&'a CreatorGenerationNote>,
}

/// A published candidate: its directory and the manifest written into it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ImportInboxReceipt {
    pub directory: PathBuf,
    pub manifest: ImportInboxManifest,
}

/// Exact bytes and metadata admitted from a manifest-last Inbox candidate.
/// The source Inbox is never consulted again after this value is returned.
#[derive(Debug)]
pub struct RetainedInboxCandidate {
    pub manifest: ImportInboxManifest,
    pub original: PathBuf,
    pub current: PathBuf,
    pub ai_output: PathBuf,
    directory: PathBuf,
}

/// Private retained copies of three caller input files.
#[derive(Debug)]
pub struct RetainedCreatorInputs {
    pub original: PathBuf,
    pub current: PathBuf,
    pub ai_output: PathBuf,
    directory: PathBuf,
}

impl Drop for RetainedCreatorInputs {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.directory);
    }
}

impl RetainedCreatorInputs {
    pub fn metadata_warnings(&self) -> Result<Vec<crate::ImageMetadataWarning>> {
        Ok([
            ("original", &self.original),
            ("current", &self.current),
            ("ai_output", &self.ai_output),
        ]
        .into_iter()
        .map(|(role, path)| crate::image_metadata_warning(role, path))
        .collect())
    }
}

pub fn retain_creator_input_files(
    original: &Path,
    current: &Path,
    ai_output: &Path,
) -> Result<RetainedCreatorInputs> {
    let directory = retained_staging_directory()?;
    let result = (|| {
        let original_copy = directory.join("original");
        let current_copy = directory.join("current");
        let output_copy = directory.join("ai-output");
        copy_creator_input(original, &original_copy, "original")?;
        copy_creator_input(current, &current_copy, "current")?;
        copy_creator_input(ai_output, &output_copy, "ai-output")?;
        Ok(RetainedCreatorInputs {
            original: original_copy,
            current: current_copy,
            ai_output: output_copy,
            directory: directory.clone(),
        })
    })();
    if result.is_err() {
        let _ = fs::remove_dir_all(&directory);
    }
    result
}

impl Drop for RetainedInboxCandidate {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.directory);
    }
}

/// Verify a manifest-last candidate and retain the exact verified bytes before
/// any repository mutation. Callers must use the returned paths, never Inbox.
#[cfg(unix)]
pub fn retain_import_inbox_candidate(root: &Path, slug: &str) -> Result<RetainedInboxCandidate> {
    if !is_import_inbox_slug(slug) {
        return Err(CreatorError::InvalidArgument(
            "inbox slug must match [a-z][a-z0-9-]{0,63}".into(),
        ));
    }
    use rustix::fs::{CWD, Mode, OFlags, openat};
    let root_file = File::from(
        openat(
            CWD,
            root,
            OFlags::RDONLY
                | OFlags::DIRECTORY
                | OFlags::NOFOLLOW
                | OFlags::NONBLOCK
                | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(|error| {
            CreatorError::InvalidArgument(format!("inbox root cannot be opened safely: {error}"))
        })?,
    );
    let directory = File::from(
        openat(
            &root_file,
            slug,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(|e| {
            CreatorError::InvalidArgument(format!("inbox candidate cannot be opened safely: {e}"))
        })?,
    );
    let manifest_bytes = read_inbox_leaf(
        &directory,
        IMPORT_INBOX_MANIFEST_NAME,
        IMPORT_INBOX_MANIFEST_MAX_BYTES,
    )?;
    let manifest: ImportInboxManifest = serde_json::from_slice(&manifest_bytes).map_err(|_| {
        CreatorError::InvalidArgument("inbox manifest is not valid strict JSON".into())
    })?;
    manifest
        .validate()
        .map_err(|e| CreatorError::InvalidArgument(e.to_string()))?;
    let staging = retained_staging_directory()?;
    let result = (|| {
        let original = staging.join("original");
        let current = staging.join("current");
        let ai_output = staging.join("ai-output");
        retain_inbox_leaf(&directory, &manifest.original, &original)?;
        retain_inbox_leaf(&directory, &manifest.current, &current)?;
        retain_inbox_leaf(&directory, &manifest.ai_output, &ai_output)?;
        Ok(RetainedInboxCandidate {
            manifest,
            original,
            current,
            ai_output,
            directory: staging.clone(),
        })
    })();
    if result.is_err() {
        let _ = fs::remove_dir_all(&staging);
    }
    result
}

#[cfg(not(unix))]
pub fn retain_import_inbox_candidate(_root: &Path, _slug: &str) -> Result<RetainedInboxCandidate> {
    Err(CreatorError::InvalidArgument(
        "safe Inbox retention is unsupported on this platform".into(),
    ))
}

#[cfg(unix)]
fn read_inbox_leaf(directory: &File, name: &str, maximum: u64) -> Result<Vec<u8>> {
    use rustix::fs::{Mode, OFlags, openat};
    let mut file = File::from(
        openat(
            directory,
            name,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(|e| {
            CreatorError::InvalidArgument(format!("inbox file cannot be opened safely: {e}"))
        })?,
    );
    let metadata = file
        .metadata()
        .map_err(|e| CreatorError::io("inspect inbox file", Path::new(name), e))?;
    if !metadata.is_file() || metadata.len() > maximum {
        return Err(CreatorError::InvalidArgument(
            "inbox file is not a bounded regular file".into(),
        ));
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    Read::by_ref(&mut file)
        .take(metadata.len() + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| CreatorError::io("read inbox file", Path::new(name), e))?;
    if bytes.len() as u64 != metadata.len() {
        return Err(CreatorError::InvalidArgument(
            "inbox file changed while it was read".into(),
        ));
    }
    Ok(bytes)
}

#[cfg(unix)]
fn retain_inbox_leaf(
    directory: &File,
    expected: &ImportInboxFile,
    destination: &Path,
) -> Result<()> {
    let bytes = read_inbox_leaf(directory, &expected.name, IMPORT_INBOX_FILE_MAX_BYTES)?;
    if bytes.len() as u64 != expected.size {
        return Err(CreatorError::InvalidArgument(
            "inbox file does not match manifest size".into(),
        ));
    }
    let actual = Sha256::digest(&bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    if expected
        .sha256
        .as_deref()
        .is_none_or(|hash| !actual.eq_ignore_ascii_case(hash))
    {
        return Err(CreatorError::InvalidArgument(
            "inbox file does not match manifest SHA-256".into(),
        ));
    }
    fs::write(destination, bytes)
        .map_err(|e| CreatorError::io("stage retained inbox file", destination, e))
}

fn retained_staging_directory() -> Result<PathBuf> {
    for _ in 0..32 {
        let mut nonce = [0_u8; 16];
        getrandom::fill(&mut nonce).map_err(|error| {
            CreatorError::ResourceLimit(format!(
                "could not allocate retained inbox staging: {error}"
            ))
        })?;
        let suffix = nonce
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        let path = std::env::temp_dir().join(format!("synapsegit-inbox-{suffix}"));
        let mut builder = fs::DirBuilder::new();
        #[cfg(unix)]
        std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
        match builder.create(&path) {
            Ok(()) => return Ok(path),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(CreatorError::io("create retained inbox staging", &path, e)),
        }
    }
    Err(CreatorError::ResourceLimit(
        "could not allocate retained inbox staging".into(),
    ))
}

/// Write one candidate below `inbox_root/slug` without replacing anything.
///
/// The three files and the manifest are written and flushed inside a hidden
/// staging directory whose name is not a slug, which the localhost service
/// ignores.  The staging directory is then renamed to `slug` without
/// replacement, so the candidate appears complete with its manifest or not at
/// all, and two writers of one slug cannot both succeed.  Each input is read
/// once while it is copied, measured, and hashed.  Inputs are the caller's
/// explicit paths, so symbolic links are followed, but each must resolve to a
/// regular file of at most [`IMPORT_INBOX_FILE_MAX_BYTES`].
pub fn put_import_inbox_candidate(
    candidate: &ImportInboxCandidate<'_>,
) -> Result<ImportInboxReceipt> {
    put_import_inbox_candidate_with_metadata_review(candidate, |_| {})
}

/// Retain the three supplied bytes in the hidden Inbox staging directory,
/// invoke `review` on metadata read from those copies, then publish manifest
/// last. The callback runs before the candidate becomes visible.
pub fn put_import_inbox_candidate_with_metadata_review<F>(
    candidate: &ImportInboxCandidate<'_>,
    mut review: F,
) -> Result<ImportInboxReceipt>
where
    F: FnMut(&[crate::ImageMetadataWarning]),
{
    if !is_import_inbox_slug(candidate.slug) {
        return Err(CreatorError::InvalidArgument(
            "inbox slug must match [a-z][a-z0-9-]{0,63}".into(),
        ));
    }
    if !display_name_is_valid(candidate.subject_label, IMPORT_INBOX_SUBJECT_MAX_BYTES) {
        return Err(CreatorError::InvalidArgument(format!(
            "--subject must be 1 to {IMPORT_INBOX_SUBJECT_MAX_BYTES} UTF-8 bytes"
        )));
    }
    if !display_name_is_valid(candidate.creator_name, IMPORT_INBOX_CREATOR_MAX_BYTES) {
        return Err(CreatorError::InvalidArgument(format!(
            "--creator must be 1 to {IMPORT_INBOX_CREATOR_MAX_BYTES} UTF-8 bytes"
        )));
    }
    if let Some(note) = candidate.generation_note {
        note.validate()?;
    }
    match fs::metadata(candidate.inbox_root) {
        Ok(metadata) if metadata.is_dir() => {}
        Ok(_) => {
            return Err(CreatorError::InvalidArgument(format!(
                "inbox directory {} is not a directory",
                candidate.inbox_root.display()
            )));
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Err(CreatorError::InvalidArgument(format!(
                "inbox directory {} does not exist; create it first",
                candidate.inbox_root.display()
            )));
        }
        Err(error) => {
            return Err(CreatorError::io(
                "inspect inbox directory",
                candidate.inbox_root,
                error,
            ));
        }
    }
    let inputs = [
        (candidate.original, IMPORT_INBOX_ORIGINAL_NAME),
        (candidate.current, IMPORT_INBOX_CURRENT_NAME),
        (candidate.ai_output, IMPORT_INBOX_AI_OUTPUT_NAME),
    ];
    for (path, _) in inputs {
        check_input(path)?;
    }
    let destination = candidate.inbox_root.join(candidate.slug);
    if fs::symlink_metadata(&destination).is_ok() {
        return Err(CreatorError::InboxCandidateExists(
            candidate.slug.to_owned(),
        ));
    }

    let staging = Staging::create(candidate.inbox_root, candidate.slug)?;
    let mut files = Vec::with_capacity(inputs.len());
    for (path, name) in inputs {
        files.push(copy_hashed(path, &staging.path.join(name), name, false)?);
    }
    let [original, current, ai_output]: [ImportInboxFile; 3] =
        files.try_into().expect("three inbox files");
    let warnings = [
        ("original", staging.path.join(IMPORT_INBOX_ORIGINAL_NAME)),
        ("current", staging.path.join(IMPORT_INBOX_CURRENT_NAME)),
        ("ai_output", staging.path.join(IMPORT_INBOX_AI_OUTPUT_NAME)),
    ]
    .into_iter()
    .map(|(role, path)| crate::image_metadata_warning(role, &path))
    .collect::<Vec<_>>();
    review(&warnings);
    let manifest = ImportInboxManifest {
        version: IMPORT_INBOX_MANIFEST_VERSION.to_owned(),
        original,
        current,
        ai_output,
        metadata: ImportInboxMetadata {
            subject_label: candidate.subject_label.to_owned(),
            creator_name: candidate.creator_name.to_owned(),
            generation_note: candidate.generation_note.cloned(),
        },
    };
    // `review` intentionally runs before publication. Re-read the private
    // leaves through no-follow descriptors afterwards so a callback cannot
    // make the manifest describe bytes different from those we publish.
    validate_staged_manifest_files(&staging.path, &manifest)?;
    manifest
        .validate()
        .map_err(|error| CreatorError::InvalidArgument(error.to_string()))?;
    let mut bytes = serde_json::to_vec_pretty(&manifest).map_err(CreatorError::Json)?;
    bytes.push(b'\n');
    if bytes.len() as u64 > IMPORT_INBOX_MANIFEST_MAX_BYTES {
        return Err(CreatorError::ResourceLimit(
            "the inbox manifest would exceed 64 KiB".into(),
        ));
    }
    let manifest_path = staging.path.join(IMPORT_INBOX_MANIFEST_NAME);
    write_new_file(&manifest_path, &bytes)?;
    sync_directory(&staging.path)?;
    rename_directory_no_replace(&staging.path, &destination, candidate.slug)?;
    staging.disarm();
    sync_directory(candidate.inbox_root)?;
    Ok(ImportInboxReceipt {
        directory: destination,
        manifest,
    })
}

#[cfg(unix)]
fn validate_staged_manifest_files(staging: &Path, manifest: &ImportInboxManifest) -> Result<()> {
    use rustix::fs::{CWD, Mode, OFlags, openat};
    let directory = File::from(
        openat(
            CWD,
            staging,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(|error| {
            CreatorError::InvalidArgument(format!(
                "inbox staging directory cannot be opened safely: {error}"
            ))
        })?,
    );
    for expected in [&manifest.original, &manifest.current, &manifest.ai_output] {
        let bytes = read_inbox_leaf(&directory, &expected.name, IMPORT_INBOX_FILE_MAX_BYTES)?;
        if bytes.len() as u64 != expected.size {
            return Err(CreatorError::InvalidArgument(
                "staged inbox file does not match manifest size".into(),
            ));
        }
        let digest = Sha256::digest(&bytes);
        let actual = hex(&digest);
        if expected
            .sha256
            .as_deref()
            .is_none_or(|hash| !actual.eq_ignore_ascii_case(hash))
        {
            return Err(CreatorError::InvalidArgument(
                "staged inbox file does not match manifest SHA-256".into(),
            ));
        }
    }
    Ok(())
}

#[cfg(not(unix))]
fn validate_staged_manifest_files(staging: &Path, manifest: &ImportInboxManifest) -> Result<()> {
    for expected in [&manifest.original, &manifest.current, &manifest.ai_output] {
        let path = staging.join(&expected.name);
        let bytes = fs::read(&path)
            .map_err(|error| CreatorError::io("read inbox staging file", &path, error))?;
        if bytes.len() as u64 != expected.size {
            return Err(CreatorError::InvalidArgument(
                "staged inbox file does not match manifest size".into(),
            ));
        }
        let digest = Sha256::digest(&bytes);
        if expected
            .sha256
            .as_deref()
            .is_none_or(|hash| !hex(&digest).eq_ignore_ascii_case(hash))
        {
            return Err(CreatorError::InvalidArgument(
                "staged inbox file does not match manifest SHA-256".into(),
            ));
        }
    }
    Ok(())
}

fn check_input(path: &Path) -> Result<()> {
    let metadata =
        fs::metadata(path).map_err(|error| CreatorError::io("inspect inbox input", path, error))?;
    if !metadata.is_file() {
        return Err(CreatorError::InvalidArgument(format!(
            "inbox input {} must be a regular file",
            path.display()
        )));
    }
    if metadata.len() > IMPORT_INBOX_FILE_MAX_BYTES {
        return Err(input_too_large(path, false));
    }
    Ok(())
}

fn input_too_large(path: &Path, creator_input: bool) -> CreatorError {
    CreatorError::ResourceLimit(format!(
        "{} input {} exceeds the 64 MiB limit",
        if creator_input { "creator" } else { "inbox" },
        path.display()
    ))
}

/// Copy `source` to a new file while measuring and hashing the same bytes.
fn copy_creator_input(source: &Path, destination: &Path, name: &str) -> Result<ImportInboxFile> {
    copy_hashed(source, destination, name, true)
}

fn copy_hashed(
    source: &Path,
    destination: &Path,
    name: &str,
    creator_input: bool,
) -> Result<ImportInboxFile> {
    let operation = if creator_input {
        "creator input"
    } else {
        "inbox input"
    };
    let mut input = open_input(source).map_err(|error| {
        CreatorError::io(
            if creator_input {
                "open creator input"
            } else {
                "open inbox input"
            },
            source,
            error,
        )
    })?;
    let metadata = input.metadata().map_err(|error| {
        CreatorError::io(
            if creator_input {
                "inspect creator input"
            } else {
                "inspect inbox input"
            },
            source,
            error,
        )
    })?;
    if !metadata.is_file() {
        return Err(CreatorError::InvalidArgument(format!(
            "{operation} {} must be a regular file",
            source.display()
        )));
    }
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)
        .map_err(|error| CreatorError::io("create inbox file", destination, error))?;
    let mut hasher = Sha256::new();
    let mut size = 0_u64;
    let mut buffer = vec![0_u8; 64 * 1024];
    loop {
        let read = input.read(&mut buffer).map_err(|error| {
            CreatorError::io(
                if creator_input {
                    "read creator input"
                } else {
                    "read inbox input"
                },
                source,
                error,
            )
        })?;
        if read == 0 {
            break;
        }
        size += read as u64;
        if size > IMPORT_INBOX_FILE_MAX_BYTES {
            return Err(input_too_large(source, creator_input));
        }
        hasher.update(&buffer[..read]);
        output
            .write_all(&buffer[..read])
            .map_err(|error| CreatorError::io("write inbox file", destination, error))?;
    }
    output
        .sync_all()
        .map_err(|error| CreatorError::io("sync inbox file", destination, error))?;
    Ok(ImportInboxFile {
        name: name.to_owned(),
        size,
        sha256: Some(hex(&hasher.finalize())),
    })
}

/// Open without blocking, so an input replaced by a FIFO after the metadata
/// check cannot park the process before `fstat` rejects it.  Reads of a
/// regular file are unaffected by `O_NONBLOCK`.
#[cfg(unix)]
fn open_input(path: &Path) -> io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt;
    OpenOptions::new()
        .read(true)
        .custom_flags(rustix::fs::OFlags::NONBLOCK.bits() as i32)
        .open(path)
}

#[cfg(not(unix))]
fn open_input(path: &Path) -> io::Result<File> {
    File::open(path)
}

fn write_new_file(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| CreatorError::io("create inbox manifest", path, error))?;
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(|error| CreatorError::io("write inbox manifest", path, error))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// A hidden staging directory that is removed unless it was published.
struct Staging {
    path: PathBuf,
    published: bool,
}

impl Staging {
    fn create(root: &Path, slug: &str) -> Result<Self> {
        let mut nonce = [0_u8; 8];
        getrandom::fill(&mut nonce)
            .map_err(|error| CreatorError::Random(format!("inbox staging name: {error}")))?;
        // A leading dot keeps the name outside the slug grammar, so the
        // service never lists an unfinished candidate.
        let path = root.join(format!(".{slug}.partial-{}", hex(&nonce)));
        let mut builder = fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder
            .create(&path)
            .map_err(|error| CreatorError::io("create inbox staging directory", &path, error))?;
        Ok(Self {
            path,
            published: false,
        })
    }

    fn disarm(mut self) {
        self.published = true;
    }
}

impl Drop for Staging {
    fn drop(&mut self) {
        if !self.published {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}

#[cfg(unix)]
fn sync_directory(path: &Path) -> Result<()> {
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| CreatorError::io("sync inbox directory", path, error))
}

#[cfg(not(unix))]
fn sync_directory(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(any(
    target_os = "linux",
    target_os = "android",
    target_vendor = "apple",
    target_os = "redox"
))]
fn rename_directory_no_replace(source: &Path, destination: &Path, slug: &str) -> Result<()> {
    use rustix::fs::{CWD, RenameFlags, renameat_with};
    use rustix::io::Errno;

    match renameat_with(CWD, source, CWD, destination, RenameFlags::NOREPLACE) {
        Ok(()) => Ok(()),
        Err(Errno::EXIST | Errno::NOTEMPTY) => {
            Err(CreatorError::InboxCandidateExists(slug.to_owned()))
        }
        Err(Errno::INVAL | Errno::NOSYS | Errno::OPNOTSUPP) => Err(CreatorError::io(
            "publish inbox candidate without replacement",
            destination,
            io::Error::new(
                io::ErrorKind::Unsupported,
                "the inbox file system does not support renaming a directory without replacement",
            ),
        )),
        Err(error) => Err(CreatorError::io(
            "publish inbox candidate without replacement",
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
fn rename_directory_no_replace(_source: &Path, destination: &Path, _slug: &str) -> Result<()> {
    Err(CreatorError::io(
        "publish inbox candidate without replacement",
        destination,
        io::Error::new(
            io::ErrorKind::Unsupported,
            "this platform has no supported atomic directory no-replace primitive",
        ),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Barrier};

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(label: &str) -> Self {
            let mut nonce = [0_u8; 8];
            getrandom::fill(&mut nonce).unwrap();
            let path = std::env::temp_dir().join(format!("synapse-inbox-{label}-{}", hex(&nonce)));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    struct Fixture {
        directory: TempDir,
        inbox: PathBuf,
        original: PathBuf,
        current: PathBuf,
        ai_output: PathBuf,
    }

    fn fixture(label: &str) -> Fixture {
        let directory = TempDir::new(label);
        let inbox = directory.0.join("inbox");
        fs::create_dir(&inbox).unwrap();
        let write = |name: &str, bytes: &[u8]| {
            let path = directory.0.join(name);
            fs::write(&path, bytes).unwrap();
            path
        };
        Fixture {
            original: write("in-original.png", b"original bytes"),
            current: write("in-current.png", b"current bytes"),
            ai_output: write("in-output.png", b"candidate bytes"),
            inbox,
            directory,
        }
    }

    fn candidate<'a>(fixture: &'a Fixture, slug: &'a str) -> ImportInboxCandidate<'a> {
        ImportInboxCandidate {
            inbox_root: &fixture.inbox,
            slug,
            original: &fixture.original,
            current: &fixture.current,
            ai_output: &fixture.ai_output,
            subject_label: "Coastal mural",
            creator_name: "Creator",
            generation_note: None,
        }
    }

    fn entries(path: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(path)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn metadata_review_precedes_manifest_publication_and_binds_staged_bytes() {
        let fixture = fixture("metadata-review");
        let mut tiff = b"II*\0\x08\0\0\0".to_vec();
        tiff.extend_from_slice(&1u16.to_le_bytes());
        tiff.extend_from_slice(&0x8825u16.to_le_bytes());
        tiff.extend_from_slice(&4u16.to_le_bytes());
        tiff.extend_from_slice(&1u32.to_le_bytes());
        tiff.extend_from_slice(&32u32.to_le_bytes());
        tiff.extend_from_slice(&0u32.to_le_bytes());
        tiff.resize(40, 0);
        let mut original = vec![0xff, 0xd8, 0xff, 0xe1];
        original.extend_from_slice(&((tiff.len() + 8) as u16).to_be_bytes());
        original.extend_from_slice(b"Exif\0\0");
        original.extend_from_slice(&tiff);
        original.extend_from_slice(&[0xff, 0xd9]);
        fs::write(&fixture.original, &original).unwrap();
        let receipt = put_import_inbox_candidate_with_metadata_review(
            &candidate(&fixture, "gps"),
            |warnings| {
                assert!(!fixture.inbox.join("gps").exists());
                assert!(
                    warnings
                        .iter()
                        .any(|warning| warning.check == crate::ImageMetadataCheck::GpsFound)
                );
                fs::write(&fixture.original, b"changed-after-copy").unwrap();
            },
        )
        .unwrap();
        assert_eq!(
            fs::read(receipt.directory.join("original")).unwrap(),
            original
        );
        assert!(receipt.directory.join("manifest.json").is_file());
    }

    #[cfg(unix)]
    #[test]
    fn metadata_review_cannot_publish_a_mutated_staged_leaf() {
        let fixture = fixture("staged-leaf-mutation");
        let source_before = fs::read(&fixture.original).unwrap();
        let error = put_import_inbox_candidate_with_metadata_review(
            &candidate(&fixture, "candidate"),
            |_| {
                let staging = fs::read_dir(&fixture.inbox)
                    .unwrap()
                    .map(|entry| entry.unwrap().path())
                    .find(|path| {
                        path.file_name()
                            .and_then(|name| name.to_str())
                            .is_some_and(|name| name.starts_with(".candidate.partial-"))
                    })
                    .expect("hidden staging directory exists during review");
                fs::write(staging.join(IMPORT_INBOX_ORIGINAL_NAME), b"tampered").unwrap();
            },
        )
        .unwrap_err();

        assert!(matches!(error, CreatorError::InvalidArgument(_)));
        assert!(!fixture.inbox.join("candidate").exists());
        assert_eq!(fs::read(&fixture.original).unwrap(), source_before);
    }

    #[test]
    fn put_publishes_three_fixed_files_and_a_valid_manifest() {
        let fixture = fixture("put");
        let note = CreatorGenerationNote {
            tool: "tool".into(),
            prompt: "prompt".into(),
            ..CreatorGenerationNote::default()
        };
        let receipt = put_import_inbox_candidate(&ImportInboxCandidate {
            generation_note: Some(&note),
            ..candidate(&fixture, "candidate-1")
        })
        .unwrap();
        assert_eq!(receipt.directory, fixture.inbox.join("candidate-1"));
        assert_eq!(entries(&fixture.inbox), ["candidate-1"]);
        assert_eq!(
            entries(&receipt.directory),
            ["ai-output", "current", "manifest.json", "original"]
        );
        let stored: ImportInboxManifest = serde_json::from_slice(
            &fs::read(receipt.directory.join(IMPORT_INBOX_MANIFEST_NAME)).unwrap(),
        )
        .unwrap();
        stored.validate().unwrap();
        assert_eq!(stored, receipt.manifest);
        assert_eq!(stored.original.name, "original");
        assert_eq!(stored.ai_output.name, "ai-output");
        assert_eq!(stored.current.size, b"current bytes".len() as u64);
        assert_eq!(
            stored.ai_output.sha256.as_deref(),
            Some(hex(&Sha256::digest(b"candidate bytes")).as_str())
        );
        assert_eq!(
            fs::read(receipt.directory.join("ai-output")).unwrap(),
            b"candidate bytes"
        );
        // Omitted note fields are written as empty strings, the same meaning
        // as the creator-run note format.
        let raw: serde_json::Value = serde_json::from_slice(
            &fs::read(receipt.directory.join(IMPORT_INBOX_MANIFEST_NAME)).unwrap(),
        )
        .unwrap();
        assert_eq!(raw["metadata"]["generation_note"]["model"], "");
        assert_eq!(raw["version"], IMPORT_INBOX_MANIFEST_VERSION);
    }

    #[test]
    fn manifest_parts_are_read_only_from_json_objects() {
        let file = |name: &str| serde_json::json!({"name": name, "size": 1});
        let valid = serde_json::json!({
            "version": IMPORT_INBOX_MANIFEST_VERSION,
            "original": file("original"),
            "current": file("current"),
            "ai_output": file("ai-output"),
            "metadata": {
                "subject_label": "Subject",
                "creator_name": "Creator",
                "generation_note": {"tool": "T", "model": "M", "prompt": "P", "intent": "I"}
            }
        });
        let manifest: ImportInboxManifest = serde_json::from_value(valid.clone()).unwrap();
        manifest.validate().unwrap();
        assert_eq!(serde_json::to_value(&manifest).unwrap(), valid);

        // Each array lists the values in field order, which serde's derived
        // struct deserialization would otherwise accept positionally.
        let mut top_level = Vec::new();
        for key in ["version", "original", "current", "ai_output", "metadata"] {
            top_level.push(valid[key].clone());
        }
        let mut positional = vec![serde_json::Value::Array(top_level)];
        for (pointer, value) in [
            ("/original", serde_json::json!(["original", 1])),
            ("/metadata", serde_json::json!(["Subject", "Creator"])),
            (
                "/metadata/generation_note",
                serde_json::json!(["T", "M", "P", "I"]),
            ),
        ] {
            let mut manifest = valid.clone();
            *manifest.pointer_mut(pointer).unwrap() = value;
            positional.push(manifest);
        }
        for manifest in positional {
            assert!(
                serde_json::from_value::<ImportInboxManifest>(manifest.clone()).is_err(),
                "{manifest}"
            );
        }
    }

    #[test]
    fn existing_slug_is_refused_without_changes() {
        let fixture = fixture("exists");
        put_import_inbox_candidate(&candidate(&fixture, "same")).unwrap();
        let before = fs::read(fixture.inbox.join("same/manifest.json")).unwrap();
        let error = put_import_inbox_candidate(&ImportInboxCandidate {
            subject_label: "Other",
            ..candidate(&fixture, "same")
        })
        .unwrap_err();
        assert_eq!(error.code(), "inbox_candidate_exists");
        assert_eq!(
            fs::read(fixture.inbox.join("same/manifest.json")).unwrap(),
            before
        );
        assert_eq!(entries(&fixture.inbox), ["same"]);

        // A dangling symbolic link with the slug name is an existing entry.
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(fixture.inbox.join("nowhere"), fixture.inbox.join("link"))
                .unwrap();
            let error = put_import_inbox_candidate(&candidate(&fixture, "link")).unwrap_err();
            assert_eq!(error.code(), "inbox_candidate_exists");
            fs::remove_file(fixture.inbox.join("link")).unwrap();
        }

        // An empty directory with the slug name is also never replaced.
        fs::create_dir(fixture.inbox.join("empty")).unwrap();
        let error = put_import_inbox_candidate(&candidate(&fixture, "empty")).unwrap_err();
        assert_eq!(error.code(), "inbox_candidate_exists");
        assert!(entries(&fixture.inbox.join("empty")).is_empty());
    }

    #[test]
    fn invalid_requests_write_nothing() {
        let fixture = fixture("invalid");
        let directory = fixture.directory.0.join("in-directory");
        fs::create_dir(&directory).unwrap();
        let missing = fixture.directory.0.join("missing");
        let long_subject = "s".repeat(IMPORT_INBOX_SUBJECT_MAX_BYTES + 1);
        let long_creator = "c".repeat(IMPORT_INBOX_CREATOR_MAX_BYTES + 1);
        let not_a_directory = fixture.original.clone();
        let too_long_slug = "a".repeat(65);
        let cases: Vec<(ImportInboxCandidate<'_>, &str)> = vec![
            (candidate(&fixture, "Invalid"), "usage_error"),
            (candidate(&fixture, "-x"), "usage_error"),
            (candidate(&fixture, &too_long_slug), "usage_error"),
            (
                ImportInboxCandidate {
                    subject_label: "",
                    ..candidate(&fixture, "a")
                },
                "usage_error",
            ),
            (
                ImportInboxCandidate {
                    subject_label: &long_subject,
                    ..candidate(&fixture, "a")
                },
                "usage_error",
            ),
            (
                ImportInboxCandidate {
                    creator_name: &long_creator,
                    ..candidate(&fixture, "a")
                },
                "usage_error",
            ),
            (
                ImportInboxCandidate {
                    inbox_root: &missing,
                    ..candidate(&fixture, "a")
                },
                "usage_error",
            ),
            (
                ImportInboxCandidate {
                    inbox_root: &not_a_directory,
                    ..candidate(&fixture, "a")
                },
                "usage_error",
            ),
            (
                ImportInboxCandidate {
                    current: &directory,
                    ..candidate(&fixture, "a")
                },
                "usage_error",
            ),
            (
                ImportInboxCandidate {
                    ai_output: &missing,
                    ..candidate(&fixture, "a")
                },
                "storage_error",
            ),
        ];
        for (request, code) in cases {
            let error = put_import_inbox_candidate(&request).unwrap_err();
            assert_eq!(error.code(), code, "{error}");
        }
        assert!(!missing.exists());
        assert!(entries(&fixture.inbox).is_empty());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_fifo_input_is_refused_before_it_is_opened() {
        let fixture = fixture("fifo");
        let fifo = fixture.directory.0.join("fifo");
        rustix::fs::mknodat(
            rustix::fs::CWD,
            &fifo,
            rustix::fs::FileType::Fifo,
            rustix::fs::Mode::from_raw_mode(0o600),
            0,
        )
        .unwrap();
        let error = put_import_inbox_candidate(&ImportInboxCandidate {
            original: &fifo,
            ..candidate(&fixture, "fifo")
        })
        .unwrap_err();
        assert_eq!(error.code(), "usage_error");
        assert!(entries(&fixture.inbox).is_empty());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_fifo_inbox_root_is_refused_without_blocking() {
        let fixture = fixture("fifo-root");
        let fifo = fixture.directory.0.join("fifo-root");
        rustix::fs::mknodat(
            rustix::fs::CWD,
            &fifo,
            rustix::fs::FileType::Fifo,
            rustix::fs::Mode::from_raw_mode(0o600),
            0,
        )
        .unwrap();

        let error = retain_import_inbox_candidate(&fifo, "candidate").unwrap_err();
        assert_eq!(error.code(), "usage_error");
    }

    #[test]
    fn the_file_limit_is_inclusive() {
        let fixture = fixture("limit");
        let at_limit = fixture.directory.0.join("at-limit");
        File::create(&at_limit)
            .unwrap()
            .set_len(IMPORT_INBOX_FILE_MAX_BYTES)
            .unwrap();
        let over_limit = fixture.directory.0.join("over-limit");
        File::create(&over_limit)
            .unwrap()
            .set_len(IMPORT_INBOX_FILE_MAX_BYTES + 1)
            .unwrap();
        let receipt = put_import_inbox_candidate(&ImportInboxCandidate {
            ai_output: &at_limit,
            ..candidate(&fixture, "at-limit")
        })
        .unwrap();
        assert_eq!(receipt.manifest.ai_output.size, IMPORT_INBOX_FILE_MAX_BYTES);
        let error = put_import_inbox_candidate(&ImportInboxCandidate {
            ai_output: &over_limit,
            ..candidate(&fixture, "over-limit")
        })
        .unwrap_err();
        assert_eq!(error.code(), "resource_limit");
        assert_eq!(entries(&fixture.inbox), ["at-limit"]);
    }

    #[test]
    fn a_leftover_staging_directory_does_not_block_a_retry() {
        let fixture = fixture("leftover");
        let leftover = fixture.inbox.join(".retry.partial-0000000000000000");
        fs::create_dir(&leftover).unwrap();
        fs::write(leftover.join("original"), b"half written").unwrap();
        put_import_inbox_candidate(&candidate(&fixture, "retry")).unwrap();
        assert!(fixture.inbox.join("retry/manifest.json").is_file());
        assert!(leftover.is_dir());
    }

    #[cfg(unix)]
    #[test]
    fn a_failed_copy_removes_its_staging_directory() {
        use std::os::unix::fs::PermissionsExt;
        let fixture = fixture("cleanup");
        // The metadata check passes, then opening the input fails after the
        // staging directory exists.
        fs::set_permissions(&fixture.current, fs::Permissions::from_mode(0o000)).unwrap();
        if File::open(&fixture.current).is_ok() {
            // Permission bits do not restrict this user (for example root).
            return;
        }
        let error = put_import_inbox_candidate(&candidate(&fixture, "cleanup")).unwrap_err();
        assert_eq!(error.code(), "storage_error");
        assert!(entries(&fixture.inbox).is_empty());
    }

    #[test]
    fn concurrent_writers_of_one_slug_publish_once() {
        let fixture = Arc::new(fixture("race"));
        let barrier = Arc::new(Barrier::new(2));
        let handles: Vec<_> = (0..2)
            .map(|_| {
                let fixture = Arc::clone(&fixture);
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    let request = candidate(&fixture, "race");
                    barrier.wait();
                    put_import_inbox_candidate(&request).map(|_| ())
                })
            })
            .collect();
        let results: Vec<_> = handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect();
        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        for result in results {
            if let Err(error) = result {
                assert_eq!(error.code(), "inbox_candidate_exists");
            }
        }
        // The losing writer removes its staging directory.
        assert_eq!(entries(&fixture.inbox), ["race"]);
    }

    #[test]
    fn manifest_validation_matches_the_v1_schema_limits() {
        let fixture = fixture("validate");
        let manifest = put_import_inbox_candidate(&candidate(&fixture, "valid"))
            .unwrap()
            .manifest;
        let with = |change: &dyn Fn(&mut ImportInboxManifest)| {
            let mut copy = manifest.clone();
            change(&mut copy);
            copy.validate()
        };
        assert_eq!(with(&|_| {}), Ok(()));
        assert_eq!(
            with(&|m| m.version = "synapsegit-import-inbox-v2".into()),
            Err(ImportInboxManifestError::UnsupportedVersion)
        );
        for name in ["", ".", "..", "a/b", "a\\b", "a\0b"] {
            assert_eq!(
                with(&|m| m.original.name = name.into()),
                Err(ImportInboxManifestError::InvalidFile),
                "{name:?}"
            );
        }
        assert_eq!(
            with(&|m| m.current.size = IMPORT_INBOX_FILE_MAX_BYTES + 1),
            Err(ImportInboxManifestError::InvalidFile)
        );
        assert_eq!(
            with(&|m| m.ai_output.sha256 = Some("xyz".into())),
            Err(ImportInboxManifestError::InvalidSha256)
        );
        assert_eq!(
            with(&|m| m.metadata.creator_name = String::new()),
            Err(ImportInboxManifestError::DisplayNames)
        );
        assert_eq!(
            with(&|m| {
                m.metadata.generation_note = Some(CreatorGenerationNote {
                    tool: "t".repeat(301),
                    ..CreatorGenerationNote::default()
                })
            }),
            Err(ImportInboxManifestError::GenerationNote)
        );
    }
}
