//! SQLite-backed mutable Refs and reflog for SynapseGit Core.
//!
//! Immutable objects remain in the caller's content-addressed store. Before a
//! Ref transaction begins, [`RefTargetValidator`] verifies that the proposed
//! head is a stored Commit and that its required closure is complete.

#![forbid(unsafe_code)]

use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;
use std::path::Path;
use std::time::Duration;

/// On-disk schema version owned by this crate.
pub const REF_STORE_SCHEMA_VERSION: i64 = 1;

const MAX_ACTOR_BYTES: usize = 1_024;
const MAX_MESSAGE_BYTES: usize = 16 * 1_024;
const COMMIT_OID_PREFIX: &str = "commit:sg-oid-v1:sha256:";

/// A Ref at one consistent snapshot.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RefRecord {
    pub name: String,
    pub head: String,
    pub updated_event_id: i64,
}

/// A deterministic Ref snapshot, ordered lexicographically by Ref name.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RefSnapshot {
    pub refs: Vec<RefRecord>,
}

impl RefSnapshot {
    pub fn is_empty(&self) -> bool {
        self.refs.is_empty()
    }

    pub fn len(&self) -> usize {
        self.refs.len()
    }
}

/// One committed Ref transition.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReflogEntry {
    pub id: i64,
    pub ref_name: String,
    pub old_head: Option<String>,
    pub new_head: String,
    pub occurred_at_unix_nanos: i64,
    pub actor: Option<String>,
    pub message: Option<String>,
}

/// Complete SQLite-owned state needed by an archive export/restore.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RefArchive {
    pub snapshot: RefSnapshot,
    /// The complete reflog, ordered by ascending event ID.
    pub reflog: Vec<ReflogEntry>,
}

/// Caller-supplied metadata recorded with a successful update.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReflogMetadata<'a> {
    pub occurred_at_unix_nanos: i64,
    pub actor: Option<&'a str>,
    pub message: Option<&'a str>,
}

impl ReflogMetadata<'_> {
    pub const fn at(occurred_at_unix_nanos: i64) -> Self {
        Self {
            occurred_at_unix_nanos,
            actor: None,
            message: None,
        }
    }
}

/// One requested compare-and-swap operation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RefUpdate<'a> {
    pub ref_name: &'a str,
    /// `None` is valid only for creation and never matches an existing Ref.
    pub expected_head: Option<&'a str>,
    pub new_head: &'a str,
    pub metadata: ReflogMetadata<'a>,
}

/// An additional Ref state that must still hold when an update is committed.
///
/// Preconditions are independent of the Ref being updated. `None` requires
/// the named Ref to be absent, while `Some(head)` requires an exact Commit OID
/// match. All preconditions are checked inside the same immediate transaction
/// as the requested update.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RefPrecondition<'a> {
    pub ref_name: &'a str,
    pub expected_head: Option<&'a str>,
}

/// A semantic/object-store validation failure returned by the integration
/// layer. The code can preserve protocol codes such as `closure_missing`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ValidationError {
    code: String,
    message: String,
}

impl ValidationError {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }

    pub fn code(&self) -> &str {
        &self.code
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for ValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.code, self.message)
    }
}

impl Error for ValidationError {}

/// Verifies a new head against the immutable object store.
///
/// Implementations must verify both that `new_head` is a valid stored Commit
/// and that its required reference closure is complete. It is deliberately
/// called before SQLite obtains a write transaction.
pub trait RefTargetValidator {
    fn validate_new_head(&self, new_head: &str) -> std::result::Result<(), ValidationError>;
}

impl<F> RefTargetValidator for F
where
    F: Fn(&str) -> std::result::Result<(), ValidationError>,
{
    fn validate_new_head(&self, new_head: &str) -> std::result::Result<(), ValidationError> {
        self(new_head)
    }
}

/// Revalidates an update while SQLite holds its immediate write transaction.
///
/// The guard is called exactly once, immediately after `BEGIN IMMEDIATE` and
/// before any Ref preconditions are read. This lets an integration layer
/// recheck short-lived authorization or capability state after waiting for the
/// SQLite writer lock. Implementations should be fast because they run while
/// every other writer is excluded.
pub trait RefTransactionGuard {
    fn validate_transaction(&self) -> std::result::Result<(), ValidationError>;
}

impl<F> RefTransactionGuard for F
where
    F: Fn() -> std::result::Result<(), ValidationError>,
{
    fn validate_transaction(&self) -> std::result::Result<(), ValidationError> {
        self()
    }
}

fn allow_transaction() -> std::result::Result<(), ValidationError> {
    Ok(())
}

/// Failures at the RefStore boundary.
#[derive(Debug)]
pub enum RefStoreError {
    InvalidRefName {
        value: String,
    },
    InvalidCommitOid {
        value: String,
    },
    InvalidMetadata {
        message: String,
    },
    Validation(ValidationError),
    RefConflict {
        ref_name: String,
        expected_head: Option<String>,
        actual_head: Option<String>,
    },
    PreconditionFailed {
        ref_name: String,
        expected_head: Option<String>,
        actual_head: Option<String>,
    },
    ArchiveNotEmpty,
    ArchiveInvalid {
        message: String,
    },
    UnsupportedSchemaVersion {
        found: i64,
    },
    Storage(rusqlite::Error),
}

impl RefStoreError {
    /// A stable protocol-facing code where one exists.
    pub fn code(&self) -> &str {
        match self {
            Self::InvalidRefName { .. } => "path_segment_invalid",
            Self::InvalidCommitOid { .. } => "oid_mismatch",
            Self::InvalidMetadata { .. } => "schema_invalid",
            Self::Validation(error) => error.code(),
            Self::RefConflict { .. } | Self::PreconditionFailed { .. } => "ref_conflict",
            Self::ArchiveNotEmpty | Self::ArchiveInvalid { .. } => "archive_invalid",
            Self::UnsupportedSchemaVersion { .. } | Self::Storage(_) => "storage_error",
        }
    }
}

impl fmt::Display for RefStoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidRefName { value } => write!(formatter, "invalid Ref name: {value:?}"),
            Self::InvalidCommitOid { value } => write!(formatter, "invalid Commit OID: {value:?}"),
            Self::InvalidMetadata { message } => formatter.write_str(message),
            Self::Validation(error) => write!(formatter, "head validation failed: {error}"),
            Self::RefConflict {
                ref_name,
                expected_head,
                actual_head,
            } => write!(
                formatter,
                "Ref {ref_name:?} conflict: expected {expected_head:?}, actual {actual_head:?}"
            ),
            Self::PreconditionFailed {
                ref_name,
                expected_head,
                actual_head,
            } => write!(
                formatter,
                "Ref precondition {ref_name:?} failed: expected {expected_head:?}, actual {actual_head:?}"
            ),
            Self::ArchiveNotEmpty => {
                formatter.write_str("archive restore requires an empty RefStore")
            }
            Self::ArchiveInvalid { message } => write!(formatter, "invalid Ref archive: {message}"),
            Self::UnsupportedSchemaVersion { found } => {
                write!(formatter, "unsupported RefStore schema version {found}")
            }
            Self::Storage(error) => write!(formatter, "SQLite RefStore error: {error}"),
        }
    }
}

impl Error for RefStoreError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Validation(error) => Some(error),
            Self::Storage(error) => Some(error),
            _ => None,
        }
    }
}

impl From<rusqlite::Error> for RefStoreError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Storage(error)
    }
}

pub type Result<T> = std::result::Result<T, RefStoreError>;

/// One SQLite connection to the local RefStore.
///
/// Open a separate instance per writer thread/process. SQLite's immediate
/// transactions serialize competing CAS operations across those connections.
pub struct SqliteRefStore {
    connection: Connection,
}

impl SqliteRefStore {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let connection = Connection::open(path)?;
        Self::initialize(connection)
    }

    pub fn open_in_memory() -> Result<Self> {
        let connection = Connection::open_in_memory()?;
        Self::initialize(connection)
    }

    fn initialize(mut connection: Connection) -> Result<Self> {
        connection.busy_timeout(Duration::from_secs(10))?;
        connection.pragma_update(None, "foreign_keys", "ON")?;
        connection.pragma_update(None, "journal_mode", "WAL")?;
        connection.pragma_update(None, "synchronous", "FULL")?;

        connection.execute_batch(
            "CREATE TABLE IF NOT EXISTS synapse_ref_meta (
                key TEXT PRIMARY KEY NOT NULL,
                value INTEGER NOT NULL
            ) STRICT;",
        )?;

        let existing_version = connection
            .query_row(
                "SELECT value FROM synapse_ref_meta WHERE key = 'schema_version'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .optional()?;
        if let Some(found) = existing_version {
            if found != REF_STORE_SCHEMA_VERSION {
                return Err(RefStoreError::UnsupportedSchemaVersion { found });
            }
        }

        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        transaction.execute(
            "INSERT OR IGNORE INTO synapse_ref_meta(key, value) VALUES ('schema_version', ?1)",
            [REF_STORE_SCHEMA_VERSION],
        )?;
        let found = transaction.query_row(
            "SELECT value FROM synapse_ref_meta WHERE key = 'schema_version'",
            [],
            |row| row.get::<_, i64>(0),
        )?;
        if found != REF_STORE_SCHEMA_VERSION {
            return Err(RefStoreError::UnsupportedSchemaVersion { found });
        }

        transaction.execute_batch(
            "CREATE TABLE IF NOT EXISTS ref_events (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                ref_name TEXT NOT NULL CHECK(length(ref_name) BETWEEN 1 AND 500),
                old_head TEXT,
                new_head TEXT NOT NULL,
                occurred_at_unix_nanos INTEGER NOT NULL,
                actor TEXT CHECK(actor IS NULL OR length(CAST(actor AS BLOB)) <= 1024),
                message TEXT CHECK(message IS NULL OR length(CAST(message AS BLOB)) <= 16384),
                CHECK(old_head IS NULL OR (
                    length(old_head) = 88 AND
                    substr(old_head, 1, 24) = 'commit:sg-oid-v1:sha256:'
                )),
                CHECK(length(new_head) = 88 AND
                    substr(new_head, 1, 24) = 'commit:sg-oid-v1:sha256:')
            ) STRICT;

            CREATE INDEX IF NOT EXISTS ref_events_ref_id
                ON ref_events(ref_name, id);

            CREATE TABLE IF NOT EXISTS refs (
                name TEXT PRIMARY KEY NOT NULL CHECK(length(name) BETWEEN 1 AND 500),
                head TEXT NOT NULL CHECK(
                    length(head) = 88 AND
                    substr(head, 1, 24) = 'commit:sg-oid-v1:sha256:'
                ),
                updated_event_id INTEGER NOT NULL UNIQUE,
                FOREIGN KEY(updated_event_id) REFERENCES ref_events(id)
                    ON UPDATE RESTRICT ON DELETE RESTRICT
            ) STRICT;",
        )?;
        transaction.commit()?;

        Ok(Self { connection })
    }

    /// Return one Ref, validating the requested name before querying SQLite.
    pub fn get(&self, name: &str) -> Result<Option<RefRecord>> {
        validate_ref_name(name)?;
        self.connection
            .query_row(
                "SELECT name, head, updated_event_id FROM refs WHERE name = ?1",
                [name],
                ref_record_from_row,
            )
            .optional()
            .map_err(Into::into)
    }

    /// List all Refs in deterministic Ref-name order.
    pub fn list(&self) -> Result<Vec<RefRecord>> {
        Ok(load_snapshot(&self.connection)?.refs)
    }

    /// Capture all current Refs in deterministic Ref-name order.
    pub fn snapshot(&self) -> Result<RefSnapshot> {
        load_snapshot(&self.connection)
    }

    /// Retrieve the complete reflog in ascending event-ID order.
    pub fn reflog(&self) -> Result<Vec<ReflogEntry>> {
        load_reflog(&self.connection, None)
    }

    /// Retrieve one Ref's reflog in ascending event-ID order.
    pub fn reflog_for_ref(&self, ref_name: &str) -> Result<Vec<ReflogEntry>> {
        validate_ref_name(ref_name)?;
        load_reflog(&self.connection, Some(ref_name))
    }

    /// Read the snapshot and complete reflog from one consistent transaction.
    pub fn export_archive(&mut self) -> Result<RefArchive> {
        let transaction = self.connection.transaction()?;
        let archive = RefArchive {
            snapshot: load_snapshot(&transaction)?,
            reflog: load_reflog(&transaction, None)?,
        };
        transaction.commit()?;
        Ok(archive)
    }

    /// Atomically compare, append the reflog, and advance a Ref.
    ///
    /// Target validation happens first. Once the immediate transaction starts,
    /// a missing Ref matches only `expected_head = None`, while an existing Ref
    /// matches only the exact expected Commit OID. A conflict rolls back the
    /// inserted event before it becomes visible.
    pub fn compare_and_swap<V>(
        &mut self,
        update: RefUpdate<'_>,
        validator: &V,
    ) -> Result<ReflogEntry>
    where
        V: RefTargetValidator + ?Sized,
    {
        self.compare_and_swap_with_preconditions(update, &[], validator)
    }

    /// Atomically verify additional Ref states, compare the update target,
    /// append the reflog, and advance the target Ref.
    ///
    /// Target validation happens before SQLite obtains a write transaction.
    /// Once the immediate transaction starts, every precondition is compared
    /// with the transaction's current snapshot before the update target is
    /// compared. Any mismatch leaves both the Ref set and reflog unchanged.
    pub fn compare_and_swap_with_preconditions<V>(
        &mut self,
        update: RefUpdate<'_>,
        preconditions: &[RefPrecondition<'_>],
        validator: &V,
    ) -> Result<ReflogEntry>
    where
        V: RefTargetValidator + ?Sized,
    {
        self.compare_and_swap_with_preconditions_and_guard(
            update,
            preconditions,
            validator,
            &allow_transaction,
        )
    }

    /// Atomically revalidate transaction-scoped state, verify additional Ref
    /// states, compare the update target, append the reflog, and advance it.
    ///
    /// Lexical input checks and target validation happen before SQLite obtains
    /// a write transaction. After `BEGIN IMMEDIATE`, `transaction_guard` runs
    /// before any Ref precondition or target state is read. A guard failure is
    /// returned as [`RefStoreError::Validation`] and leaves both the Ref set and
    /// reflog unchanged.
    pub fn compare_and_swap_with_preconditions_and_guard<V, G>(
        &mut self,
        update: RefUpdate<'_>,
        preconditions: &[RefPrecondition<'_>],
        validator: &V,
        transaction_guard: &G,
    ) -> Result<ReflogEntry>
    where
        V: RefTargetValidator + ?Sized,
        G: RefTransactionGuard + ?Sized,
    {
        validate_ref_name(update.ref_name)?;
        if let Some(expected_head) = update.expected_head {
            validate_commit_oid(expected_head)?;
        }
        validate_commit_oid(update.new_head)?;
        validate_metadata(update.metadata.actor, update.metadata.message)?;
        for precondition in preconditions {
            validate_ref_name(precondition.ref_name)?;
            if let Some(expected_head) = precondition.expected_head {
                validate_commit_oid(expected_head)?;
            }
        }
        validator
            .validate_new_head(update.new_head)
            .map_err(RefStoreError::Validation)?;

        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        transaction_guard
            .validate_transaction()
            .map_err(RefStoreError::Validation)?;
        for precondition in preconditions {
            let actual_head = query_head(&transaction, precondition.ref_name)?;
            if actual_head.as_deref() != precondition.expected_head {
                return Err(RefStoreError::PreconditionFailed {
                    ref_name: precondition.ref_name.to_owned(),
                    expected_head: precondition.expected_head.map(str::to_owned),
                    actual_head,
                });
            }
        }
        let current_head = query_head(&transaction, update.ref_name)?;
        if current_head.as_deref() != update.expected_head {
            return Err(RefStoreError::RefConflict {
                ref_name: update.ref_name.to_owned(),
                expected_head: update.expected_head.map(str::to_owned),
                actual_head: current_head,
            });
        }

        transaction.execute(
            "INSERT INTO ref_events(
                ref_name, old_head, new_head, occurred_at_unix_nanos, actor, message
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                update.ref_name,
                current_head.as_deref(),
                update.new_head,
                update.metadata.occurred_at_unix_nanos,
                update.metadata.actor,
                update.metadata.message,
            ],
        )?;
        let event_id = transaction.last_insert_rowid();

        let changed = if let Some(expected_head) = update.expected_head {
            transaction.execute(
                "UPDATE refs
                 SET head = ?1, updated_event_id = ?2
                 WHERE name = ?3 AND head = ?4",
                params![update.new_head, event_id, update.ref_name, expected_head],
            )?
        } else {
            transaction.execute(
                "INSERT OR IGNORE INTO refs(name, head, updated_event_id) VALUES (?1, ?2, ?3)",
                params![update.ref_name, update.new_head, event_id],
            )?
        };

        // This is defensive in addition to BEGIN IMMEDIATE. If a trigger or a
        // future schema change makes the SQL CAS fail, dropping the transaction
        // also removes the provisional reflog event.
        if changed != 1 {
            let actual_head = query_head(&transaction, update.ref_name)?;
            return Err(RefStoreError::RefConflict {
                ref_name: update.ref_name.to_owned(),
                expected_head: update.expected_head.map(str::to_owned),
                actual_head,
            });
        }

        let entry = ReflogEntry {
            id: event_id,
            ref_name: update.ref_name.to_owned(),
            old_head: current_head,
            new_head: update.new_head.to_owned(),
            occurred_at_unix_nanos: update.metadata.occurred_at_unix_nanos,
            actor: update.metadata.actor.map(str::to_owned),
            message: update.metadata.message.map(str::to_owned),
        };
        transaction.commit()?;
        Ok(entry)
    }

    /// Restore a complete Ref archive into an empty RefStore.
    ///
    /// The helper rejects duplicate or broken event chains, verifies that each
    /// chain ends at the declared snapshot, preserves event IDs, and imports
    /// everything in one transaction. Every distinct archived head is passed
    /// to `validator` before that transaction begins.
    pub fn restore_archive<V>(&mut self, archive: &RefArchive, validator: &V) -> Result<()>
    where
        V: RefTargetValidator + ?Sized,
    {
        let prepared = prepare_archive(archive)?;
        for head in &prepared.heads {
            validator
                .validate_new_head(head)
                .map_err(RefStoreError::Validation)?;
        }

        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let ref_count =
            transaction.query_row("SELECT count(*) FROM refs", [], |row| row.get::<_, i64>(0))?;
        let event_count = transaction.query_row("SELECT count(*) FROM ref_events", [], |row| {
            row.get::<_, i64>(0)
        })?;
        if ref_count != 0 || event_count != 0 {
            return Err(RefStoreError::ArchiveNotEmpty);
        }

        for event in &prepared.reflog {
            transaction.execute(
                "INSERT INTO ref_events(
                    id, ref_name, old_head, new_head, occurred_at_unix_nanos, actor, message
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    event.id,
                    event.ref_name,
                    event.old_head,
                    event.new_head,
                    event.occurred_at_unix_nanos,
                    event.actor,
                    event.message,
                ],
            )?;
        }
        for record in &prepared.refs {
            transaction.execute(
                "INSERT INTO refs(name, head, updated_event_id) VALUES (?1, ?2, ?3)",
                params![record.name, record.head, record.updated_event_id],
            )?;
        }
        transaction.commit()?;
        Ok(())
    }
}

fn ref_record_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<RefRecord> {
    Ok(RefRecord {
        name: row.get(0)?,
        head: row.get(1)?,
        updated_event_id: row.get(2)?,
    })
}

fn reflog_entry_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ReflogEntry> {
    Ok(ReflogEntry {
        id: row.get(0)?,
        ref_name: row.get(1)?,
        old_head: row.get(2)?,
        new_head: row.get(3)?,
        occurred_at_unix_nanos: row.get(4)?,
        actor: row.get(5)?,
        message: row.get(6)?,
    })
}

fn load_snapshot(connection: &Connection) -> Result<RefSnapshot> {
    let mut statement = connection.prepare(
        "SELECT name, head, updated_event_id
         FROM refs
         ORDER BY name ASC",
    )?;
    let rows = statement.query_map([], ref_record_from_row)?;
    let mut refs = Vec::new();
    for row in rows {
        refs.push(row?);
    }
    Ok(RefSnapshot { refs })
}

fn load_reflog(connection: &Connection, ref_name: Option<&str>) -> Result<Vec<ReflogEntry>> {
    let mut entries = Vec::new();
    if let Some(ref_name) = ref_name {
        let mut statement = connection.prepare(
            "SELECT id, ref_name, old_head, new_head,
                    occurred_at_unix_nanos, actor, message
             FROM ref_events
             WHERE ref_name = ?1
             ORDER BY id ASC",
        )?;
        let rows = statement.query_map([ref_name], reflog_entry_from_row)?;
        for row in rows {
            entries.push(row?);
        }
    } else {
        let mut statement = connection.prepare(
            "SELECT id, ref_name, old_head, new_head,
                    occurred_at_unix_nanos, actor, message
             FROM ref_events
             ORDER BY id ASC",
        )?;
        let rows = statement.query_map([], reflog_entry_from_row)?;
        for row in rows {
            entries.push(row?);
        }
    }
    Ok(entries)
}

fn query_head(connection: &Connection, ref_name: &str) -> Result<Option<String>> {
    connection
        .query_row("SELECT head FROM refs WHERE name = ?1", [ref_name], |row| {
            row.get(0)
        })
        .optional()
        .map_err(Into::into)
}

fn validate_metadata(actor: Option<&str>, message: Option<&str>) -> Result<()> {
    if actor.is_some_and(|actor| actor.len() > MAX_ACTOR_BYTES) {
        return Err(RefStoreError::InvalidMetadata {
            message: format!("reflog actor exceeds {MAX_ACTOR_BYTES} UTF-8 bytes"),
        });
    }
    if message.is_some_and(|message| message.len() > MAX_MESSAGE_BYTES) {
        return Err(RefStoreError::InvalidMetadata {
            message: format!("reflog message exceeds {MAX_MESSAGE_BYTES} UTF-8 bytes"),
        });
    }
    Ok(())
}

struct PreparedArchive {
    refs: Vec<RefRecord>,
    reflog: Vec<ReflogEntry>,
    heads: BTreeSet<String>,
}

fn prepare_archive(archive: &RefArchive) -> Result<PreparedArchive> {
    let mut refs = archive.snapshot.refs.clone();
    refs.sort_by(|left, right| left.name.cmp(&right.name));
    if let Some(duplicate) = refs.windows(2).find(|pair| pair[0].name == pair[1].name) {
        return Err(archive_invalid(format!(
            "duplicate snapshot Ref {:?}",
            duplicate[0].name
        )));
    }
    for record in &refs {
        validate_ref_name(&record.name)
            .map_err(|error| archive_invalid(format!("snapshot contains {}", error)))?;
        validate_commit_oid(&record.head)
            .map_err(|error| archive_invalid(format!("snapshot contains {}", error)))?;
        if record.updated_event_id <= 0 {
            return Err(archive_invalid(format!(
                "Ref {:?} has non-positive updated_event_id {}",
                record.name, record.updated_event_id
            )));
        }
    }

    let mut reflog = archive.reflog.clone();
    reflog.sort_by_key(|event| event.id);
    if let Some(duplicate) = reflog.windows(2).find(|pair| pair[0].id == pair[1].id) {
        return Err(archive_invalid(format!(
            "duplicate reflog event ID {}",
            duplicate[0].id
        )));
    }

    let mut heads = BTreeSet::new();
    let mut final_state: BTreeMap<String, (String, i64)> = BTreeMap::new();
    for event in &reflog {
        if event.id <= 0 {
            return Err(archive_invalid(format!(
                "reflog event has non-positive ID {}",
                event.id
            )));
        }
        validate_ref_name(&event.ref_name).map_err(|error| {
            archive_invalid(format!("reflog event {} contains {}", event.id, error))
        })?;
        if let Some(old_head) = &event.old_head {
            validate_commit_oid(old_head).map_err(|error| {
                archive_invalid(format!("reflog event {} contains {}", event.id, error))
            })?;
        }
        validate_commit_oid(&event.new_head).map_err(|error| {
            archive_invalid(format!("reflog event {} contains {}", event.id, error))
        })?;
        validate_metadata(event.actor.as_deref(), event.message.as_deref()).map_err(|error| {
            archive_invalid(format!("reflog event {} contains {}", event.id, error))
        })?;

        let expected_old = final_state
            .get(&event.ref_name)
            .map(|(head, _)| head.as_str());
        if event.old_head.as_deref() != expected_old {
            return Err(archive_invalid(format!(
                "reflog event {} for {:?} expects old head {:?}, chain has {:?}",
                event.id, event.ref_name, event.old_head, expected_old
            )));
        }
        heads.insert(event.new_head.clone());
        final_state.insert(event.ref_name.clone(), (event.new_head.clone(), event.id));
    }

    if final_state.len() != refs.len() {
        return Err(archive_invalid(
            "snapshot Ref set does not match the complete reflog Ref set",
        ));
    }
    for record in &refs {
        let Some((final_head, final_event_id)) = final_state.get(&record.name) else {
            return Err(archive_invalid(format!(
                "snapshot Ref {:?} has no reflog chain",
                record.name
            )));
        };
        if final_head != &record.head || *final_event_id != record.updated_event_id {
            return Err(archive_invalid(format!(
                "snapshot Ref {:?} does not match reflog final head/event",
                record.name
            )));
        }
    }

    Ok(PreparedArchive {
        refs,
        reflog,
        heads,
    })
}

fn archive_invalid(message: impl Into<String>) -> RefStoreError {
    RefStoreError::ArchiveInvalid {
        message: message.into(),
    }
}

/// Validate the exact Core v0.1 `RefName` lexical profile.
pub fn validate_ref_name(name: &str) -> Result<()> {
    let valid = name.len() <= 500 && {
        let mut segments = name.split('/');
        let namespace = segments.next().unwrap_or_default();
        let namespace_valid = matches!(
            namespace,
            "proposal" | "decision" | "release" | "observed" | "material-events"
        );
        let rest: Vec<&str> = segments.collect();
        namespace_valid && !rest.is_empty() && rest.iter().all(|segment| valid_ref_segment(segment))
    };

    if valid {
        Ok(())
    } else {
        Err(RefStoreError::InvalidRefName {
            value: name.to_owned(),
        })
    }
}

fn valid_ref_segment(segment: &str) -> bool {
    let Some(first) = segment.as_bytes().first() else {
        return false;
    };
    (1..=128).contains(&segment.len())
        && (first.is_ascii_lowercase() || first.is_ascii_digit())
        && segment.as_bytes()[1..].iter().all(|byte| {
            byte.is_ascii_lowercase()
                || byte.is_ascii_digit()
                || matches!(byte, b'.' | b'_' | b':' | b'-')
        })
}

/// Validate the exact Core v0.1 Commit OID lexical profile.
pub fn validate_commit_oid(oid: &str) -> Result<()> {
    let digest = oid.strip_prefix(COMMIT_OID_PREFIX);
    if digest.is_some_and(|digest| {
        digest.len() == 64
            && digest
                .as_bytes()
                .iter()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    }) {
        Ok(())
    } else {
        Err(RefStoreError::InvalidCommitOid {
            value: oid.to_owned(),
        })
    }
}
