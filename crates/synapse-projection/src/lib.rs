//! Rebuildable SQLite query projection over SynapseGit's immutable CAS.
//!
//! This crate owns no authoritative state. Callers explicitly provide a
//! verified [`FileObjectStore`] and one consistent [`RefSnapshot`] to
//! [`SqliteProjectionStore::rebuild`]. Authorization, Ref updates, archives,
//! and object identity must never depend on this disposable index.
//! [`RefScope`] is a query filter, not an authorization boundary. An embedding
//! service must authorize the caller before exposing projection data or error
//! distinctions such as indexed versus indexed-but-not-reachable.
//!
//! Rebuild assumes snapshot-reachable CAS objects are append-only for its
//! duration; cooperative GC/removal must be paused. Concurrent unrelated object
//! publication is safe. A source that disappears or changes during planning
//! fails the rebuild and leaves the prior projection active. Operators should
//! monitor rebuild failures and source-fingerprint changes.
//!
//! Valid CAS orphans are not indexed. Core v0.1 Tombstones are store-wide,
//! however, so each non-empty rebuild performs one bounded Record-family scan
//! and reuses its resolver catalog for every Ref. A corrupt orphan Record fails
//! closed even though it would not become a row. [`ProjectionLimits`] exposes
//! independent Record-count and cumulative canonical-byte bounds; the legacy
//! [`SqliteProjectionStore::rebuild`] entry point uses their documented
//! defaults.

#![forbid(unsafe_code)]

use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt::{self, Write as _};
use std::path::Path;
use std::time::Duration;
use synapse_canonical::{ErrorCode, ObjectKind, Value, parse_oid};
use synapse_cas::{
    ClosureIssueKind, ClosureNodeState, FileObjectStore, GraphLimits, PreparedClosureVerifier,
    StoreError, TombstoneScanLimits,
};
use synapse_schema::validate;
use synapse_sqlite::{RefRecord, RefSnapshot, validate_ref_name};

pub const PROJECTION_SCHEMA_VERSION: i64 = 2;

#[derive(Debug)]
pub enum ProjectionError {
    Storage(rusqlite::Error),
    ObjectStore(StoreError),
    InvalidSnapshot(String),
    InvalidSource(String),
    ResourceLimit(String),
    UnsupportedSchemaVersion { found: String },
    CorruptProjection(String),
    UnknownRef(String),
    ObservationNotIndexed(String),
    AnalysisNotIndexed(String),
    AnalysisNotReachable(String),
}

impl ProjectionError {
    pub const fn code(&self) -> &'static str {
        match self {
            Self::Storage(_) | Self::ObjectStore(_) | Self::CorruptProjection(_) => "storage_error",
            Self::InvalidSnapshot(_) | Self::InvalidSource(_) => "projection_source_invalid",
            Self::ResourceLimit(_) => "resource_limit",
            Self::UnsupportedSchemaVersion { .. } => "projection_schema_unsupported",
            Self::UnknownRef(_) => "projection_ref_unknown",
            Self::ObservationNotIndexed(_) => "projection_observation_unknown",
            Self::AnalysisNotIndexed(_) => "projection_analysis_unknown",
            Self::AnalysisNotReachable(_) => "projection_analysis_not_reachable",
        }
    }
}

impl fmt::Display for ProjectionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Storage(error) => write!(formatter, "projection SQLite error: {error}"),
            Self::ObjectStore(error) => write!(formatter, "projection object-store error: {error}"),
            Self::InvalidSnapshot(message) => write!(formatter, "invalid Ref snapshot: {message}"),
            Self::InvalidSource(message) => {
                write!(formatter, "invalid projection source: {message}")
            }
            Self::ResourceLimit(message) => {
                write!(formatter, "projection resource limit: {message}")
            }
            Self::UnsupportedSchemaVersion { found } => {
                write!(formatter, "unsupported projection schema version {found:?}")
            }
            Self::CorruptProjection(message) => write!(formatter, "corrupt projection: {message}"),
            Self::UnknownRef(name) => write!(formatter, "Ref {name:?} is not in the projection"),
            Self::ObservationNotIndexed(oid) => {
                write!(formatter, "Observation {oid} is not indexed")
            }
            Self::AnalysisNotIndexed(oid) => {
                write!(formatter, "AnalysisResult {oid} is not indexed")
            }
            Self::AnalysisNotReachable(oid) => write!(
                formatter,
                "AnalysisResult {oid} is not reachable from the selected Refs"
            ),
        }
    }
}

impl Error for ProjectionError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Storage(error) => Some(error),
            Self::ObjectStore(error) => Some(error),
            _ => None,
        }
    }
}

impl From<rusqlite::Error> for ProjectionError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Storage(error)
    }
}

impl From<StoreError> for ProjectionError {
    fn from(error: StoreError) -> Self {
        Self::ObjectStore(error)
    }
}

pub type Result<T> = std::result::Result<T, ProjectionError>;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RefScope {
    /// Every Ref in the snapshot used for the most recent rebuild.
    All,
    /// An exact set of current Ref names. Duplicates are removed; unknown Refs
    /// are rejected instead of silently broadening the query.
    Names(Vec<String>),
}

impl RefScope {
    pub fn one(name: impl Into<String>) -> Self {
        Self::Names(vec![name.into()])
    }

    pub fn names(names: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self::Names(names.into_iter().map(Into::into).collect())
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum ObjectAvailability {
    Present,
    Tombstoned,
    Missing,
}

impl ObjectAvailability {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Present => "present",
            Self::Tombstoned => "tombstoned",
            Self::Missing => "missing",
        }
    }

    fn parse(value: &str) -> Result<Self> {
        match value {
            "present" => Ok(Self::Present),
            "tombstoned" => Ok(Self::Tombstoned),
            "missing" => Ok(Self::Missing),
            _ => Err(ProjectionError::CorruptProjection(format!(
                "unknown availability {value:?}"
            ))),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectedObject {
    pub oid: String,
    pub kind: ObjectKind,
    pub availability: ObjectAvailability,
    pub byte_len: Option<u64>,
    pub tombstone_oid: Option<String>,
    pub record_type: Option<String>,
    pub entity_id: Option<String>,
    pub recorded_at: Option<String>,
    pub asserted_by: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum TimelineRecordKind {
    Observation,
    Activity,
}

impl TimelineRecordKind {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Observation => "observation",
            Self::Activity => "activity",
        }
    }

    fn parse(value: &str) -> Result<Self> {
        match value {
            "observation" => Ok(Self::Observation),
            "activity" => Ok(Self::Activity),
            _ => Err(ProjectionError::CorruptProjection(format!(
                "unknown timeline record kind {value:?}"
            ))),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum TimelineTimeBasis {
    ObservationCaptureInstant,
    ObservationCaptureInterval,
    ObservationRecordedAtFallback,
    ActivityValidInstant,
    ActivityValidInterval,
    ActivityRecordedAtFallback,
}

impl TimelineTimeBasis {
    const fn as_str(self) -> &'static str {
        match self {
            Self::ObservationCaptureInstant => "observation_capture_instant",
            Self::ObservationCaptureInterval => "observation_capture_interval",
            Self::ObservationRecordedAtFallback => "observation_recorded_at_fallback",
            Self::ActivityValidInstant => "activity_valid_instant",
            Self::ActivityValidInterval => "activity_valid_interval",
            Self::ActivityRecordedAtFallback => "activity_recorded_at_fallback",
        }
    }

    fn parse(value: &str) -> Result<Self> {
        match value {
            "observation_capture_instant" => Ok(Self::ObservationCaptureInstant),
            "observation_capture_interval" => Ok(Self::ObservationCaptureInterval),
            "observation_recorded_at_fallback" => Ok(Self::ObservationRecordedAtFallback),
            "activity_valid_instant" => Ok(Self::ActivityValidInstant),
            "activity_valid_interval" => Ok(Self::ActivityValidInterval),
            "activity_recorded_at_fallback" => Ok(Self::ActivityRecordedAtFallback),
            _ => Err(ProjectionError::CorruptProjection(format!(
                "unknown timeline time basis {value:?}"
            ))),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TimelineEntry {
    pub oid: String,
    pub kind: TimelineRecordKind,
    pub entity_id: String,
    pub subject_id: String,
    pub series_id: Option<String>,
    pub ordering_time: String,
    pub time_basis: TimelineTimeBasis,
    pub event_time_start: Option<String>,
    pub event_time_end: Option<String>,
    pub recorded_at: String,
    pub asserted_by: String,
    pub reachable_from: Vec<String>,
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum ObservationDependencyKind {
    CaptureProfile,
    Station,
    StationDeployment,
    Calibration,
    Environment,
    Media,
}

impl ObservationDependencyKind {
    const fn as_str(self) -> &'static str {
        match self {
            Self::CaptureProfile => "capture_profile",
            Self::Station => "station",
            Self::StationDeployment => "station_deployment",
            Self::Calibration => "calibration",
            Self::Environment => "environment",
            Self::Media => "media",
        }
    }

    fn parse(value: &str) -> Result<Self> {
        match value {
            "capture_profile" => Ok(Self::CaptureProfile),
            "station" => Ok(Self::Station),
            "station_deployment" => Ok(Self::StationDeployment),
            "calibration" => Ok(Self::Calibration),
            "environment" => Ok(Self::Environment),
            "media" => Ok(Self::Media),
            _ => Err(ProjectionError::CorruptProjection(format!(
                "unknown Observation dependency kind {value:?}"
            ))),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DependencyTargetKind {
    Entity,
    Object(ObjectKind),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ObservationDependency {
    pub observation_oid: String,
    pub kind: ObservationDependencyKind,
    pub target_ref: String,
    pub target_kind: DependencyTargetKind,
    pub role: Option<String>,
    pub ordinal: u32,
    pub availability: Option<ObjectAvailability>,
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum AdapterDeterminism {
    Deterministic,
    Seeded,
    Probabilistic,
}

impl AdapterDeterminism {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Deterministic => "deterministic",
            Self::Seeded => "seeded",
            Self::Probabilistic => "probabilistic",
        }
    }

    fn parse(value: &str) -> Result<Self> {
        match value {
            "deterministic" => Ok(Self::Deterministic),
            "seeded" => Ok(Self::Seeded),
            "probabilistic" => Ok(Self::Probabilistic),
            _ => Err(ProjectionError::CorruptProjection(format!(
                "unknown adapter determinism {value:?}"
            ))),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum AnalysisMaskRole {
    Changed,
    Unchanged,
    Ambiguous,
    Unobservable,
    Validity,
}

impl AnalysisMaskRole {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Changed => "changed",
            Self::Unchanged => "unchanged",
            Self::Ambiguous => "ambiguous",
            Self::Unobservable => "unobservable",
            Self::Validity => "validity",
        }
    }

    fn parse(value: &str) -> Result<Self> {
        match value {
            "changed" => Ok(Self::Changed),
            "unchanged" => Ok(Self::Unchanged),
            "ambiguous" => Ok(Self::Ambiguous),
            "unobservable" => Ok(Self::Unobservable),
            "validity" => Ok(Self::Validity),
            _ => Err(ProjectionError::CorruptProjection(format!(
                "unknown Analysis mask role {value:?}"
            ))),
        }
    }
}

/// Availability of every prerequisite needed to attempt replay.
///
/// `Ready` means inputs, adapter/configuration digests, and transforms are
/// present. It does not promise byte-identical replay, even when the adapter's
/// declared determinism is `deterministic`.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum AnalysisReplayReadiness {
    Ready,
    BlockedMissing,
    BlockedTombstoned,
    BlockedMissingAndTombstoned,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AnalysisObjectRef {
    pub oid: String,
    pub kind: ObjectKind,
    pub availability: ObjectAvailability,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AnalysisInput {
    pub ordinal: u32,
    pub role: String,
    pub object: AnalysisObjectRef,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AnalysisMask {
    pub role: AnalysisMaskRole,
    pub object: AnalysisObjectRef,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AnalysisAdapter {
    pub id: String,
    pub version: String,
    pub implementation: AnalysisObjectRef,
    pub configuration: AnalysisObjectRef,
    pub determinism: AdapterDeterminism,
    pub seed: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AnalysisLineage {
    pub analysis_oid: String,
    pub entity_id: String,
    pub recorded_at: String,
    pub asserted_by: String,
    pub analysis_kind: String,
    pub comparison_kind: String,
    pub status: String,
    pub comparability: String,
    pub adapter: AnalysisAdapter,
    pub inputs: Vec<AnalysisInput>,
    pub transforms: Vec<AnalysisObjectRef>,
    pub derived_blobs: Vec<AnalysisObjectRef>,
    pub masks: Vec<AnalysisMask>,
    pub replay_readiness: AnalysisReplayReadiness,
    pub reachable_from: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClosureSummary {
    pub ref_name: String,
    pub head_oid: String,
    pub complete: bool,
    pub truncated: bool,
    pub issue_count: u64,
    pub present_count: u64,
    pub tombstoned_count: u64,
    pub missing_count: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectedClosureIssue {
    pub ref_name: String,
    pub ordinal: u32,
    pub oid: String,
    pub referenced_by: Option<String>,
    pub role: Option<String>,
    pub issue_kind: String,
    pub detail: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectionMetadata {
    pub schema_version: i64,
    pub source_fingerprint: String,
    pub ref_count: u64,
    pub object_count: u64,
    pub edge_count: u64,
    pub incomplete_ref_count: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RebuildReport {
    pub metadata: ProjectionMetadata,
}

/// Limits for one projection rebuild. Graph limits bound reachable and
/// derived state; Tombstone limits bound the one store-wide Record scan shared
/// by every Ref closure in the rebuild.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ProjectionLimits {
    pub graph: GraphLimits,
    pub tombstone_scan: TombstoneScanLimits,
}

impl From<GraphLimits> for ProjectionLimits {
    fn from(graph: GraphLimits) -> Self {
        Self {
            graph,
            tombstone_scan: TombstoneScanLimits::default(),
        }
    }
}

pub struct SqliteProjectionStore {
    connection: Connection,
}

impl SqliteProjectionStore {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        Self::initialize(Connection::open(path)?)
    }

    pub fn open_in_memory() -> Result<Self> {
        Self::initialize(Connection::open_in_memory()?)
    }

    fn initialize(mut connection: Connection) -> Result<Self> {
        connection.busy_timeout(Duration::from_secs(10))?;
        connection.pragma_update(None, "foreign_keys", "ON")?;
        connection.pragma_update(None, "journal_mode", "WAL")?;
        connection.pragma_update(None, "synchronous", "FULL")?;
        connection.execute_batch(
            "CREATE TABLE IF NOT EXISTS projection_meta (
                key TEXT PRIMARY KEY NOT NULL,
                value TEXT NOT NULL
            ) STRICT;",
        )?;
        let existing = connection
            .query_row(
                "SELECT value FROM projection_meta WHERE key = 'schema_version'",
                [],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        if let Some(found) = existing.as_deref()
            && found != PROJECTION_SCHEMA_VERSION.to_string()
        {
            return Err(ProjectionError::UnsupportedSchemaVersion {
                found: found.to_owned(),
            });
        }

        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        transaction.execute(
            "INSERT OR IGNORE INTO projection_meta(key, value) VALUES ('schema_version', ?1)",
            [PROJECTION_SCHEMA_VERSION.to_string()],
        )?;
        create_schema(&transaction)?;
        transaction.commit()?;
        Ok(Self { connection })
    }

    /// Verify current Ref closures, build a deterministic projection plan, and
    /// replace every derived row in one immediate SQLite transaction.
    ///
    /// Source validation and resource-limit failures happen before replacement;
    /// SQLite insertion failures roll back the transaction. In both cases the
    /// previous projection remains queryable. Tombstone discovery uses
    /// [`TombstoneScanLimits::default`]; use [`Self::rebuild_with_limits`] to
    /// configure it explicitly.
    pub fn rebuild(
        &mut self,
        object_store: &FileObjectStore,
        refs: &RefSnapshot,
        limits: GraphLimits,
    ) -> Result<RebuildReport> {
        self.rebuild_with_limits(object_store, refs, limits.into())
    }

    /// Rebuild with an explicit hard bound for the one Record inventory scan
    /// used by all Ref closures. Empty snapshots do not perform that scan. The
    /// prepared catalog is valid only for this cooperative no-GC/no-removal
    /// operation; Tombstones published later appear on the next rebuild.
    pub fn rebuild_with_limits(
        &mut self,
        object_store: &FileObjectStore,
        refs: &RefSnapshot,
        limits: ProjectionLimits,
    ) -> Result<RebuildReport> {
        let plan = BuildPlan::from_sources(object_store, refs, limits)?;
        let metadata = plan.metadata();
        self.replace(&plan, &metadata)?;
        Ok(RebuildReport { metadata })
    }

    /// Return metadata for the last successful rebuild, or `None` before the
    /// first rebuild. Failed rebuilds leave this value unchanged.
    pub fn metadata(&self) -> Result<Option<ProjectionMetadata>> {
        let Some(source_fingerprint) = meta_value(&self.connection, "source_fingerprint")? else {
            return Ok(None);
        };
        Ok(Some(ProjectionMetadata {
            schema_version: PROJECTION_SCHEMA_VERSION,
            source_fingerprint,
            ref_count: parse_meta_u64(&self.connection, "ref_count")?,
            object_count: parse_meta_u64(&self.connection, "object_count")?,
            edge_count: parse_meta_u64(&self.connection, "edge_count")?,
            incomplete_ref_count: parse_meta_u64(&self.connection, "incomplete_ref_count")?,
        }))
    }

    /// Look up one reachable projected object. CAS orphans return `None`.
    pub fn get_object(&self, oid: &str) -> Result<Option<ProjectedObject>> {
        self.connection
            .query_row(
                "SELECT oid, kind, availability, byte_len, tombstone_oid,
                        record_type, entity_id, recorded_at, asserted_by
                 FROM objects WHERE oid = ?1",
                [oid],
                projected_object_from_row,
            )
            .optional()
            .map_err(Into::into)
    }

    /// Query current reachable Observation and Activity Records for a Subject.
    ///
    /// Supplying `series_id` restricts results to Observations in that series;
    /// Activities have no v0.1 series field and are therefore omitted. Without
    /// a series filter both kinds are returned. Ordering uses the authoritative
    /// Observation capture time or Activity valid time, falling back explicitly
    /// to `recorded_at` only for an unknown ValidTime, then breaks ties by OID.
    pub fn subject_timeline(
        &self,
        subject_id: &str,
        series_id: Option<&str>,
        scope: &RefScope,
    ) -> Result<Vec<TimelineEntry>> {
        let refs = self.resolve_scope(scope)?;
        let mut entries = BTreeMap::<String, (TimelineEntry, BTreeSet<String>)>::new();
        for ref_name in refs {
            let mut statement = self.connection.prepare(
                "SELECT t.record_oid, t.record_kind, t.entity_id,
                        sl.series_id, t.ordering_time, t.time_basis,
                        t.event_time_start, t.event_time_end,
                        t.recorded_at, t.asserted_by
                 FROM timeline_records t
                 JOIN subject_links subject ON subject.record_oid = t.record_oid
                 JOIN ref_reachability reachable ON reachable.oid = t.record_oid
                 LEFT JOIN series_links sl ON sl.record_oid = t.record_oid
                 WHERE subject.subject_id = ?1
                   AND reachable.ref_name = ?2
                   AND (?3 IS NULL OR sl.series_id = ?3)
                 ORDER BY t.ordering_time, t.record_oid",
            )?;
            let rows = statement.query_map(params![subject_id, ref_name, series_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, Option<String>>(6)?,
                    row.get::<_, Option<String>>(7)?,
                    row.get::<_, String>(8)?,
                    row.get::<_, String>(9)?,
                ))
            })?;
            for row in rows {
                let (
                    oid,
                    kind,
                    entity_id,
                    series_id,
                    ordering_time,
                    time_basis,
                    event_time_start,
                    event_time_end,
                    recorded_at,
                    asserted_by,
                ) = row?;
                let entry = TimelineEntry {
                    oid: oid.clone(),
                    kind: TimelineRecordKind::parse(&kind)?,
                    entity_id,
                    subject_id: subject_id.to_owned(),
                    series_id,
                    ordering_time,
                    time_basis: TimelineTimeBasis::parse(&time_basis)?,
                    event_time_start,
                    event_time_end,
                    recorded_at,
                    asserted_by,
                    reachable_from: Vec::new(),
                };
                let stored = entries
                    .entry(oid)
                    .or_insert_with(|| (entry, BTreeSet::new()));
                stored.1.insert(ref_name.clone());
            }
        }
        let mut result = entries
            .into_values()
            .map(|(mut entry, refs)| {
                entry.reachable_from = refs.into_iter().collect();
                entry
            })
            .collect::<Vec<_>>();
        result.sort_by(|left, right| {
            left.ordering_time
                .cmp(&right.ordering_time)
                .then_with(|| left.oid.cmp(&right.oid))
        });
        Ok(result)
    }

    /// Return the typed capture dependencies of one reachable Observation.
    /// Entity targets have no object availability; OID targets report their
    /// projected `present`, `tombstoned`, or `missing` state.
    pub fn observation_dependencies(
        &self,
        observation_oid: &str,
    ) -> Result<Vec<ObservationDependency>> {
        let indexed = self.connection.query_row(
            "SELECT EXISTS(
                SELECT 1 FROM objects
                WHERE oid = ?1 AND availability = 'present' AND record_type = 'observation'
             )",
            [observation_oid],
            |row| row.get::<_, bool>(0),
        )?;
        if !indexed {
            return Err(ProjectionError::ObservationNotIndexed(
                observation_oid.to_owned(),
            ));
        }
        let mut statement = self.connection.prepare(
            "SELECT dependency_kind, target_ref, target_kind, role, ordinal,
                    objects.availability
             FROM observation_dependencies dependencies
             LEFT JOIN objects ON objects.oid = dependencies.target_ref
             WHERE observation_oid = ?1
             ORDER BY CASE dependency_kind
                        WHEN 'capture_profile' THEN 0
                        WHEN 'station' THEN 1
                        WHEN 'station_deployment' THEN 2
                        WHEN 'calibration' THEN 3
                        WHEN 'environment' THEN 4
                        WHEN 'media' THEN 5
                      END,
                      ordinal, target_ref",
        )?;
        let rows = statement.query_map([observation_oid], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, i64>(4)?,
                row.get::<_, Option<String>>(5)?,
            ))
        })?;
        let mut result = Vec::new();
        for row in rows {
            let (kind, target_ref, target_kind, role, ordinal, availability) = row?;
            let target_kind = if target_kind == "entity" {
                DependencyTargetKind::Entity
            } else {
                DependencyTargetKind::Object(parse_kind(&target_kind)?)
            };
            result.push(ObservationDependency {
                observation_oid: observation_oid.to_owned(),
                kind: ObservationDependencyKind::parse(&kind)?,
                target_ref,
                target_kind,
                role,
                ordinal: u32::try_from(ordinal).map_err(|_| {
                    ProjectionError::CorruptProjection("negative dependency ordinal".into())
                })?,
                availability: availability
                    .as_deref()
                    .map(ObjectAvailability::parse)
                    .transpose()?,
            });
        }
        Ok(result)
    }

    /// Return typed provenance for one indexed AnalysisResult within an
    /// explicit current-Ref scope.
    ///
    /// Scope validation happens before object lookup. An AnalysisResult that
    /// is absent from the rebuilt index is distinct from one that is indexed
    /// globally but not reachable from any selected Ref. Replay readiness only
    /// summarizes prerequisite availability; it never promises exact replay.
    /// Callers must authorize access before exposing this existence distinction;
    /// `RefScope` is not an ACL.
    pub fn analysis_lineage(
        &self,
        analysis_oid: &str,
        scope: &RefScope,
    ) -> Result<AnalysisLineage> {
        let refs = self.resolve_scope(scope)?;
        let row = self
            .connection
            .query_row(
                "SELECT records.entity_id, records.recorded_at, records.asserted_by,
                        analyses.analysis_kind, analyses.comparison_kind,
                        analyses.status, analyses.comparability,
                        analyses.adapter_id, analyses.adapter_version,
                        analyses.implementation_oid, analyses.configuration_oid,
                        analyses.determinism, analyses.seed
                 FROM analysis_results analyses
                 JOIN records ON records.oid = analyses.analysis_oid
                 WHERE analyses.analysis_oid = ?1",
                [analysis_oid],
                |row| {
                    Ok(AnalysisQueryRow {
                        entity_id: row.get(0)?,
                        recorded_at: row.get(1)?,
                        asserted_by: row.get(2)?,
                        analysis_kind: row.get(3)?,
                        comparison_kind: row.get(4)?,
                        status: row.get(5)?,
                        comparability: row.get(6)?,
                        adapter_id: row.get(7)?,
                        adapter_version: row.get(8)?,
                        implementation_oid: row.get(9)?,
                        configuration_oid: row.get(10)?,
                        determinism: row.get(11)?,
                        seed: row.get(12)?,
                    })
                },
            )
            .optional()?;
        let Some(row) = row else {
            return Err(ProjectionError::AnalysisNotIndexed(analysis_oid.to_owned()));
        };

        let mut reachable_from = Vec::new();
        for ref_name in refs {
            let reachable = self.connection.query_row(
                "SELECT EXISTS(
                    SELECT 1 FROM ref_reachability
                    WHERE ref_name = ?1 AND oid = ?2
                 )",
                params![ref_name, analysis_oid],
                |query_row| query_row.get::<_, bool>(0),
            )?;
            if reachable {
                reachable_from.push(ref_name);
            }
        }
        if reachable_from.is_empty() {
            return Err(ProjectionError::AnalysisNotReachable(
                analysis_oid.to_owned(),
            ));
        }

        let implementation = self.analysis_object_ref(&row.implementation_oid)?;
        let configuration = self.analysis_object_ref(&row.configuration_oid)?;
        let adapter = AnalysisAdapter {
            id: row.adapter_id,
            version: row.adapter_version,
            implementation,
            configuration,
            determinism: AdapterDeterminism::parse(&row.determinism)?,
            seed: row.seed,
        };

        let mut inputs = Vec::new();
        let mut transforms = Vec::new();
        let mut derived_blobs = Vec::new();
        let mut masks = Vec::new();
        let mut statement = self.connection.prepare(
            "SELECT links.category, links.ordinal, links.role,
                    objects.oid, objects.kind, objects.availability
             FROM analysis_links links
             JOIN objects ON objects.oid = links.target_oid
             WHERE links.analysis_oid = ?1
             ORDER BY CASE links.category
                        WHEN 'input' THEN 0
                        WHEN 'transform' THEN 1
                        WHEN 'derived_blob' THEN 2
                        WHEN 'mask' THEN 3
                      END,
                      links.ordinal, links.target_oid",
        )?;
        let link_rows = statement.query_map([analysis_oid], |query_row| {
            Ok((
                query_row.get::<_, String>(0)?,
                query_row.get::<_, i64>(1)?,
                query_row.get::<_, Option<String>>(2)?,
                query_row.get::<_, String>(3)?,
                query_row.get::<_, String>(4)?,
                query_row.get::<_, String>(5)?,
            ))
        })?;
        for link_row in link_rows {
            let (category, ordinal, role, target_oid, target_kind, availability) = link_row?;
            let ordinal = u32::try_from(ordinal).map_err(|_| {
                ProjectionError::CorruptProjection(
                    "Analysis link ordinal is outside the u32 range".into(),
                )
            })?;
            let object = analysis_object_ref_from_parts(target_oid, target_kind, availability)?;
            match AnalysisLinkCategory::parse(&category)? {
                AnalysisLinkCategory::Input => inputs.push(AnalysisInput {
                    ordinal,
                    role: role.ok_or_else(|| {
                        ProjectionError::CorruptProjection("Analysis input link has no role".into())
                    })?,
                    object,
                }),
                AnalysisLinkCategory::Transform => transforms.push(object),
                AnalysisLinkCategory::DerivedBlob => derived_blobs.push(object),
                AnalysisLinkCategory::Mask => masks.push(AnalysisMask {
                    role: AnalysisMaskRole::parse(role.as_deref().ok_or_else(|| {
                        ProjectionError::CorruptProjection("Analysis mask link has no role".into())
                    })?)?,
                    object,
                }),
            }
        }

        let replay_readiness = analysis_replay_readiness(&adapter, &inputs, &transforms);
        Ok(AnalysisLineage {
            analysis_oid: analysis_oid.to_owned(),
            entity_id: row.entity_id,
            recorded_at: row.recorded_at,
            asserted_by: row.asserted_by,
            analysis_kind: row.analysis_kind,
            comparison_kind: row.comparison_kind,
            status: row.status,
            comparability: row.comparability,
            adapter,
            inputs,
            transforms,
            derived_blobs,
            masks,
            replay_readiness,
            reachable_from,
        })
    }

    /// Return per-Ref closure completeness and availability counts.
    /// `complete` means there are no traversal issues; tombstoned payloads are
    /// counted separately and do not make the historical graph untraversable.
    pub fn closure_summaries(&self, scope: &RefScope) -> Result<Vec<ClosureSummary>> {
        let refs = self.resolve_scope(scope)?;
        let mut result = Vec::with_capacity(refs.len());
        for ref_name in refs {
            result.push(self.connection.query_row(
                "SELECT ref_name, head_oid, complete, truncated, issue_count,
                        present_count, tombstoned_count, missing_count
                 FROM closure_summaries WHERE ref_name = ?1",
                [&ref_name],
                |row| {
                    Ok(ClosureSummary {
                        ref_name: row.get(0)?,
                        head_oid: row.get(1)?,
                        complete: row.get(2)?,
                        truncated: row.get(3)?,
                        issue_count: row.get(4)?,
                        present_count: row.get(5)?,
                        tombstoned_count: row.get(6)?,
                        missing_count: row.get(7)?,
                    })
                },
            )?);
        }
        Ok(result)
    }

    /// Return deterministic missing-object diagnostics for one projected Ref.
    pub fn closure_issues(&self, ref_name: &str) -> Result<Vec<ProjectedClosureIssue>> {
        let scope = self.resolve_scope(&RefScope::one(ref_name))?;
        let ref_name = &scope[0];
        let mut statement = self.connection.prepare(
            "SELECT ref_name, ordinal, oid, referenced_by, role, issue_kind, detail
             FROM closure_issues WHERE ref_name = ?1 ORDER BY ordinal",
        )?;
        let rows = statement.query_map([ref_name], |row| {
            Ok(ProjectedClosureIssue {
                ref_name: row.get(0)?,
                ordinal: row.get(1)?,
                oid: row.get(2)?,
                referenced_by: row.get(3)?,
                role: row.get(4)?,
                issue_kind: row.get(5)?,
                detail: row.get(6)?,
            })
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    fn resolve_scope(&self, scope: &RefScope) -> Result<Vec<String>> {
        match scope {
            RefScope::All => {
                let mut statement = self
                    .connection
                    .prepare("SELECT ref_name FROM ref_heads ORDER BY ref_name")?;
                let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
                rows.collect::<std::result::Result<Vec<_>, _>>()
                    .map_err(Into::into)
            }
            RefScope::Names(requested) => {
                let mut names = requested.iter().cloned().collect::<BTreeSet<_>>();
                let mut result = Vec::with_capacity(names.len());
                for name in std::mem::take(&mut names) {
                    validate_ref_name(&name).map_err(|error| {
                        ProjectionError::InvalidSnapshot(format!(
                            "invalid query Ref name {name:?}: {error}"
                        ))
                    })?;
                    let exists = self.connection.query_row(
                        "SELECT EXISTS(SELECT 1 FROM ref_heads WHERE ref_name = ?1)",
                        [&name],
                        |row| row.get::<_, bool>(0),
                    )?;
                    if !exists {
                        return Err(ProjectionError::UnknownRef(name));
                    }
                    result.push(name);
                }
                Ok(result)
            }
        }
    }

    fn analysis_object_ref(&self, oid: &str) -> Result<AnalysisObjectRef> {
        let row = self
            .connection
            .query_row(
                "SELECT oid, kind, availability FROM objects WHERE oid = ?1",
                [oid],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                },
            )
            .optional()?;
        let Some((oid, kind, availability)) = row else {
            return Err(ProjectionError::CorruptProjection(format!(
                "Analysis target {oid} has no object row"
            )));
        };
        analysis_object_ref_from_parts(oid, kind, availability)
    }

    fn replace(&mut self, plan: &BuildPlan, metadata: &ProjectionMetadata) -> Result<()> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        replace_rows(&transaction, plan, metadata)?;
        transaction.commit()?;
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ObjectRow {
    oid: String,
    kind: ObjectKind,
    availability: ObjectAvailability,
    byte_len: Option<u64>,
    tombstone_oid: Option<String>,
    record_type: Option<String>,
    entity_id: Option<String>,
    recorded_at: Option<String>,
    asserted_by: Option<String>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct ReachabilityRow {
    ref_name: String,
    oid: String,
    depth: usize,
    availability: ObjectAvailability,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct EdgeRow {
    source_oid: String,
    target_oid: String,
    role: String,
    expected_kind: String,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct RecordRow {
    oid: String,
    record_type: String,
    entity_id: String,
    recorded_at: String,
    asserted_by: String,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct SubjectLinkRow {
    record_oid: String,
    subject_id: String,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct SeriesLinkRow {
    record_oid: String,
    series_id: String,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct TimelineRow {
    record_oid: String,
    record_kind: String,
    entity_id: String,
    ordering_time: String,
    time_basis: String,
    event_time_start: Option<String>,
    event_time_end: Option<String>,
    recorded_at: String,
    asserted_by: String,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct DependencyRow {
    observation_oid: String,
    dependency_kind: String,
    target_ref: String,
    target_kind: String,
    role: Option<String>,
    ordinal: usize,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct AnalysisRow {
    analysis_oid: String,
    analysis_kind: String,
    comparison_kind: String,
    status: String,
    comparability: String,
    adapter_id: String,
    adapter_version: String,
    implementation_oid: String,
    configuration_oid: String,
    determinism: String,
    seed: Option<String>,
}

struct AnalysisQueryRow {
    entity_id: String,
    recorded_at: String,
    asserted_by: String,
    analysis_kind: String,
    comparison_kind: String,
    status: String,
    comparability: String,
    adapter_id: String,
    adapter_version: String,
    implementation_oid: String,
    configuration_oid: String,
    determinism: String,
    seed: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum AnalysisLinkCategory {
    Input,
    Transform,
    DerivedBlob,
    Mask,
}

impl AnalysisLinkCategory {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Input => "input",
            Self::Transform => "transform",
            Self::DerivedBlob => "derived_blob",
            Self::Mask => "mask",
        }
    }

    fn parse(value: &str) -> Result<Self> {
        match value {
            "input" => Ok(Self::Input),
            "transform" => Ok(Self::Transform),
            "derived_blob" => Ok(Self::DerivedBlob),
            "mask" => Ok(Self::Mask),
            _ => Err(ProjectionError::CorruptProjection(format!(
                "unknown Analysis link category {value:?}"
            ))),
        }
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct AnalysisLinkRow {
    analysis_oid: String,
    category: AnalysisLinkCategory,
    ordinal: usize,
    role: Option<String>,
    target_oid: String,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct SummaryRow {
    ref_name: String,
    head_oid: String,
    complete: bool,
    truncated: bool,
    issue_count: usize,
    present_count: usize,
    tombstoned_count: usize,
    missing_count: usize,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct IssueRow {
    ref_name: String,
    ordinal: usize,
    oid: String,
    referenced_by: Option<String>,
    role: Option<String>,
    issue_kind: String,
    detail: Option<String>,
}

#[derive(Default)]
struct BuildPlan {
    refs: Vec<RefRecord>,
    objects: BTreeMap<String, ObjectRow>,
    reachability: BTreeSet<ReachabilityRow>,
    edges: BTreeSet<EdgeRow>,
    records: BTreeSet<RecordRow>,
    subject_links: BTreeSet<SubjectLinkRow>,
    series_links: BTreeSet<SeriesLinkRow>,
    timelines: BTreeSet<TimelineRow>,
    dependencies: BTreeSet<DependencyRow>,
    analyses: BTreeSet<AnalysisRow>,
    analysis_links: BTreeSet<AnalysisLinkRow>,
    summaries: BTreeSet<SummaryRow>,
    issues: BTreeSet<IssueRow>,
    source_fingerprint: String,
}

impl BuildPlan {
    fn from_sources(
        store: &FileObjectStore,
        snapshot: &RefSnapshot,
        limits: ProjectionLimits,
    ) -> Result<Self> {
        let graph_limits = limits.graph;
        if graph_limits.max_objects == 0 || graph_limits.max_edges == 0 {
            return Err(ProjectionError::ResourceLimit(
                "GraphLimits max_objects and max_edges must be positive".into(),
            ));
        }
        if snapshot.refs.len() > graph_limits.max_objects {
            return Err(ProjectionError::ResourceLimit(format!(
                "Ref snapshot contains {} Refs, exceeding object limit {}",
                snapshot.refs.len(),
                graph_limits.max_objects
            )));
        }
        let refs = normalize_snapshot(snapshot)?;
        let mut plan = Self {
            refs,
            ..Self::default()
        };

        let mut verifier = if plan.refs.is_empty() {
            None
        } else {
            Some(
                PreparedClosureVerifier::new(store, graph_limits, limits.tombstone_scan)
                    .map_err(map_source_store_error)?,
            )
        };

        for reference in &plan.refs {
            let Some(verifier) = verifier.as_mut() else {
                return Err(ProjectionError::InvalidSource(
                    "non-empty Ref plan has no prepared closure verifier".into(),
                ));
            };
            let report = verifier
                .verify(&reference.head)
                .map_err(map_source_store_error)?;
            if report.truncated {
                return Err(ProjectionError::ResourceLimit(format!(
                    "closure for Ref {:?} was truncated",
                    reference.name
                )));
            }
            for issue in &report.issues {
                match &issue.kind {
                    ClosureIssueKind::Missing => {}
                    ClosureIssueKind::ResourceLimit { resource, limit } => {
                        return Err(ProjectionError::ResourceLimit(format!(
                            "Ref {:?} closure exceeded {resource} limit {limit}",
                            reference.name
                        )));
                    }
                    kind => {
                        return Err(ProjectionError::InvalidSource(format!(
                            "Ref {:?} closure issue at {}: {}",
                            reference.name,
                            issue.oid,
                            closure_issue_description(kind)
                        )));
                    }
                }
            }

            let mut present_count = 0_usize;
            let mut tombstoned_count = 0_usize;
            let mut missing_count = 0_usize;
            for node in report.nodes.values() {
                let row = match &node.state {
                    ClosureNodeState::Present { kind, byte_len } => {
                        present_count += 1;
                        ObjectRow {
                            oid: node.oid.clone(),
                            kind: *kind,
                            availability: ObjectAvailability::Present,
                            byte_len: Some(*byte_len),
                            tombstone_oid: None,
                            record_type: None,
                            entity_id: None,
                            recorded_at: None,
                            asserted_by: None,
                        }
                    }
                    ClosureNodeState::Tombstoned {
                        kind,
                        tombstone_oid,
                    } => {
                        validate_resolving_tombstone(store, &node.oid, tombstone_oid)?;
                        tombstoned_count += 1;
                        ObjectRow {
                            oid: node.oid.clone(),
                            kind: *kind,
                            availability: ObjectAvailability::Tombstoned,
                            byte_len: None,
                            tombstone_oid: Some(tombstone_oid.clone()),
                            record_type: None,
                            entity_id: None,
                            recorded_at: None,
                            asserted_by: None,
                        }
                    }
                    ClosureNodeState::Missing { kind } => {
                        missing_count += 1;
                        ObjectRow {
                            oid: node.oid.clone(),
                            kind: *kind,
                            availability: ObjectAvailability::Missing,
                            byte_len: None,
                            tombstone_oid: None,
                            record_type: None,
                            entity_id: None,
                            recorded_at: None,
                            asserted_by: None,
                        }
                    }
                    ClosureNodeState::Corrupt { detail, .. }
                    | ClosureNodeState::ReadFailure { detail, .. } => {
                        return Err(ProjectionError::InvalidSource(format!(
                            "Ref {:?} contains unreadable object {}: {detail}",
                            reference.name, node.oid
                        )));
                    }
                };
                merge_object(&mut plan.objects, row)?;
                if plan.objects.len() > graph_limits.max_objects {
                    return Err(ProjectionError::ResourceLimit(format!(
                        "projection reaches more than {} unique objects",
                        graph_limits.max_objects
                    )));
                }
                plan.reachability.insert(ReachabilityRow {
                    ref_name: reference.name.clone(),
                    oid: node.oid.clone(),
                    depth: node.depth,
                    availability: availability_for_state(&node.state),
                });
                if plan.reachability.len() > graph_limits.max_edges {
                    return Err(ProjectionError::ResourceLimit(format!(
                        "projection contains more than {} per-Ref reachability rows",
                        graph_limits.max_edges
                    )));
                }
            }
            for edge in report.edges {
                plan.edges.insert(EdgeRow {
                    source_oid: edge.source,
                    target_oid: edge.target,
                    role: edge.role.to_string(),
                    expected_kind: kind_name(edge.expected_kind).to_owned(),
                });
                if plan.edges.len() > graph_limits.max_edges {
                    return Err(ProjectionError::ResourceLimit(format!(
                        "projection reaches more than {} unique edges",
                        graph_limits.max_edges
                    )));
                }
            }
            let issue_count = report.issues.len();
            for (ordinal, issue) in report.issues.into_iter().enumerate() {
                plan.issues.insert(IssueRow {
                    ref_name: reference.name.clone(),
                    ordinal,
                    oid: issue.oid,
                    referenced_by: issue.referenced_by,
                    role: issue.role.map(|role| role.to_string()),
                    issue_kind: "missing".to_owned(),
                    detail: None,
                });
                if plan.issues.len() > graph_limits.max_edges {
                    return Err(ProjectionError::ResourceLimit(format!(
                        "projection contains more than {} closure issues",
                        graph_limits.max_edges
                    )));
                }
            }
            plan.summaries.insert(SummaryRow {
                ref_name: reference.name.clone(),
                head_oid: reference.head.clone(),
                complete: issue_count == 0,
                truncated: false,
                issue_count,
                present_count,
                tombstoned_count,
                missing_count,
            });
        }

        if plan.objects.len() > graph_limits.max_objects {
            return Err(ProjectionError::ResourceLimit(format!(
                "projection reaches {} unique objects, exceeding limit {}",
                plan.objects.len(),
                graph_limits.max_objects
            )));
        }
        if plan.edges.len() > graph_limits.max_edges {
            return Err(ProjectionError::ResourceLimit(format!(
                "projection reaches {} unique edges, exceeding limit {}",
                plan.edges.len(),
                graph_limits.max_edges
            )));
        }
        if plan.reachability.len() > graph_limits.max_edges {
            return Err(ProjectionError::ResourceLimit(format!(
                "projection contains {} per-Ref reachability rows, exceeding edge limit {}",
                plan.reachability.len(),
                graph_limits.max_edges
            )));
        }
        plan.map_present_objects(store, graph_limits.max_edges)?;
        let derived_rows = plan.derived_row_count()?;
        if derived_rows > graph_limits.max_edges {
            return Err(ProjectionError::ResourceLimit(format!(
                "projection contains {derived_rows} derived rows, exceeding edge limit {}",
                graph_limits.max_edges
            )));
        }
        plan.source_fingerprint = fingerprint(&plan);
        Ok(plan)
    }

    fn map_present_objects(
        &mut self,
        store: &FileObjectStore,
        max_derived_rows: usize,
    ) -> Result<()> {
        let present_oids = self
            .objects
            .values()
            .filter(|row| row.availability == ObjectAvailability::Present)
            .map(|row| row.oid.clone())
            .collect::<Vec<_>>();
        for oid in present_oids {
            let object = store
                .get_verified(&oid)
                .map_err(map_source_store_error)?
                .ok_or_else(|| {
                    ProjectionError::InvalidSource(format!(
                        "reachable object disappeared during rebuild: {oid}"
                    ))
                })?;
            if object.kind().is_structured() {
                let value = object.structured().ok_or_else(|| {
                    ProjectionError::InvalidSource(format!(
                        "reachable structured object has no parsed body: {oid}"
                    ))
                })?;
                validate(value).map_err(|error| {
                    ProjectionError::InvalidSource(format!(
                        "reachable object {oid} fails schema/semantic validation: {error}"
                    ))
                })?;
                if object.kind() == ObjectKind::Record {
                    self.map_record(&oid, value)?;
                    if self.derived_row_count()? > max_derived_rows {
                        return Err(ProjectionError::ResourceLimit(format!(
                            "projection contains more than {max_derived_rows} derived rows"
                        )));
                    }
                }
            }
        }
        Ok(())
    }

    fn derived_row_count(&self) -> Result<usize> {
        self.records
            .len()
            .checked_add(self.subject_links.len())
            .and_then(|count| count.checked_add(self.series_links.len()))
            .and_then(|count| count.checked_add(self.timelines.len()))
            .and_then(|count| count.checked_add(self.dependencies.len()))
            .and_then(|count| count.checked_add(self.analyses.len()))
            .and_then(|count| count.checked_add(self.analysis_links.len()))
            .ok_or_else(|| {
                ProjectionError::ResourceLimit("derived projection row count overflow".into())
            })
    }

    fn map_record(&mut self, oid: &str, value: &Value) -> Result<()> {
        let record_type = required_string(value, "record_type", oid)?;
        let entity_id = required_string(value, "entity_id", oid)?;
        let recorded_at = required_string(value, "recorded_at", oid)?;
        let asserted_by = required_string(value, "asserted_by", oid)?;
        let row = self.objects.get_mut(oid).ok_or_else(|| {
            ProjectionError::InvalidSource(format!("Record {oid} is absent from object plan"))
        })?;
        row.record_type = Some(record_type.to_owned());
        row.entity_id = Some(entity_id.to_owned());
        row.recorded_at = Some(recorded_at.to_owned());
        row.asserted_by = Some(asserted_by.to_owned());
        self.records.insert(RecordRow {
            oid: oid.to_owned(),
            record_type: record_type.to_owned(),
            entity_id: entity_id.to_owned(),
            recorded_at: recorded_at.to_owned(),
            asserted_by: asserted_by.to_owned(),
        });

        match record_type {
            "observation" => self.map_observation(oid, value),
            "activity" => self.map_activity(oid, value),
            "analysis_result" => self.map_analysis(oid, value),
            _ => Ok(()),
        }
    }

    fn map_analysis(&mut self, oid: &str, value: &Value) -> Result<()> {
        let payload = required_object(value, "payload", oid)?;
        let adapter = required_object(payload, "adapter", oid)?;
        let implementation_oid = required_string(adapter, "implementation_digest", oid)?;
        let configuration_oid = required_string(adapter, "configuration_digest", oid)?;
        self.validate_analysis_target(
            oid,
            "adapter.implementation_digest",
            implementation_oid,
            None,
        )?;
        self.validate_analysis_target(
            oid,
            "adapter.configuration_digest",
            configuration_oid,
            None,
        )?;
        let determinism = required_string(adapter, "determinism", oid)?;
        let determinism = match determinism {
            "deterministic" => AdapterDeterminism::Deterministic,
            "seeded" => AdapterDeterminism::Seeded,
            "probabilistic" => AdapterDeterminism::Probabilistic,
            value => {
                return Err(ProjectionError::InvalidSource(format!(
                    "AnalysisResult {oid} has unsupported adapter determinism {value:?}"
                )));
            }
        };
        self.analyses.insert(AnalysisRow {
            analysis_oid: oid.to_owned(),
            analysis_kind: required_string(payload, "analysis_kind", oid)?.to_owned(),
            comparison_kind: required_string(payload, "comparison_kind", oid)?.to_owned(),
            status: required_string(payload, "status", oid)?.to_owned(),
            comparability: required_string(payload, "comparability", oid)?.to_owned(),
            adapter_id: required_string(adapter, "id", oid)?.to_owned(),
            adapter_version: required_string(adapter, "version", oid)?.to_owned(),
            implementation_oid: implementation_oid.to_owned(),
            configuration_oid: configuration_oid.to_owned(),
            determinism: determinism.as_str().to_owned(),
            seed: adapter
                .get("seed")
                .and_then(Value::as_str)
                .map(str::to_owned),
        });

        for (ordinal, input) in required_array(payload, "inputs", oid)?.iter().enumerate() {
            let target_oid = required_string(input, "ref", oid)?;
            self.validate_analysis_target(oid, "inputs[].ref", target_oid, None)?;
            self.analysis_links.insert(AnalysisLinkRow {
                analysis_oid: oid.to_owned(),
                category: AnalysisLinkCategory::Input,
                ordinal,
                role: Some(required_string(input, "role", oid)?.to_owned()),
                target_oid: target_oid.to_owned(),
            });
        }

        if let Some(transforms) = payload.get("transform_refs").and_then(Value::as_array) {
            for (ordinal, target) in transforms.iter().enumerate() {
                let target_oid = target.as_str().ok_or_else(|| {
                    ProjectionError::InvalidSource(format!(
                        "AnalysisResult {oid} transform_refs contains a non-string"
                    ))
                })?;
                self.validate_analysis_target(
                    oid,
                    "transform_refs[]",
                    target_oid,
                    Some(ObjectKind::Record),
                )?;
                self.analysis_links.insert(AnalysisLinkRow {
                    analysis_oid: oid.to_owned(),
                    category: AnalysisLinkCategory::Transform,
                    ordinal,
                    role: None,
                    target_oid: target_oid.to_owned(),
                });
            }
        }

        for (ordinal, target) in required_array(payload, "derived_blob_refs", oid)?
            .iter()
            .enumerate()
        {
            let target_oid = target.as_str().ok_or_else(|| {
                ProjectionError::InvalidSource(format!(
                    "AnalysisResult {oid} derived_blob_refs contains a non-string"
                ))
            })?;
            self.validate_analysis_target(
                oid,
                "derived_blob_refs[]",
                target_oid,
                Some(ObjectKind::Blob),
            )?;
            self.analysis_links.insert(AnalysisLinkRow {
                analysis_oid: oid.to_owned(),
                category: AnalysisLinkCategory::DerivedBlob,
                ordinal,
                role: None,
                target_oid: target_oid.to_owned(),
            });
        }

        if let Some(mask_refs) = payload.get("mask_refs") {
            mask_refs.as_object().ok_or_else(|| {
                ProjectionError::InvalidSource(format!(
                    "AnalysisResult {oid} field mask_refs is not an object"
                ))
            })?;
            for (ordinal, role) in [
                AnalysisMaskRole::Changed,
                AnalysisMaskRole::Unchanged,
                AnalysisMaskRole::Ambiguous,
                AnalysisMaskRole::Unobservable,
                AnalysisMaskRole::Validity,
            ]
            .into_iter()
            .enumerate()
            {
                let Some(target_oid) = mask_refs.get(role.as_str()).and_then(Value::as_str) else {
                    continue;
                };
                self.validate_analysis_target(
                    oid,
                    "mask_refs",
                    target_oid,
                    Some(ObjectKind::Blob),
                )?;
                self.analysis_links.insert(AnalysisLinkRow {
                    analysis_oid: oid.to_owned(),
                    category: AnalysisLinkCategory::Mask,
                    ordinal,
                    role: Some(role.as_str().to_owned()),
                    target_oid: target_oid.to_owned(),
                });
            }
        }
        Ok(())
    }

    fn validate_analysis_target(
        &self,
        analysis_oid: &str,
        field: &str,
        target_oid: &str,
        expected_kind: Option<ObjectKind>,
    ) -> Result<()> {
        let actual_kind = parse_oid(target_oid).map_err(|error| {
            ProjectionError::InvalidSource(format!(
                "AnalysisResult {analysis_oid} {field} has invalid OID: {error}"
            ))
        })?;
        if let Some(expected_kind) = expected_kind
            && actual_kind != expected_kind
        {
            return Err(ProjectionError::InvalidSource(format!(
                "AnalysisResult {analysis_oid} {field} requires {} OID, found {}",
                expected_kind.prefix(),
                actual_kind.prefix()
            )));
        }
        if !self.objects.contains_key(target_oid) {
            return Err(ProjectionError::InvalidSource(format!(
                "AnalysisResult {analysis_oid} {field} target {target_oid} is absent from its verified closure"
            )));
        }
        let edge_start = EdgeRow {
            source_oid: analysis_oid.to_owned(),
            target_oid: target_oid.to_owned(),
            role: String::new(),
            expected_kind: String::new(),
        };
        let directly_linked = self.edges.range(edge_start..).next().is_some_and(|edge| {
            edge.source_oid == analysis_oid
                && edge.target_oid == target_oid
                && edge.expected_kind == actual_kind.prefix()
        });
        if !directly_linked {
            return Err(ProjectionError::InvalidSource(format!(
                "AnalysisResult {analysis_oid} {field} target {target_oid} has no matching verified graph edge"
            )));
        }
        Ok(())
    }

    fn map_observation(&mut self, oid: &str, value: &Value) -> Result<()> {
        let payload = required_object(value, "payload", oid)?;
        let subject_id = required_string(payload, "subject_ref", oid)?;
        let series_id = required_string(payload, "series_ref", oid)?;
        self.subject_links.insert(SubjectLinkRow {
            record_oid: oid.to_owned(),
            subject_id: subject_id.to_owned(),
        });
        self.series_links.insert(SeriesLinkRow {
            record_oid: oid.to_owned(),
            series_id: series_id.to_owned(),
        });
        let record = self
            .records
            .iter()
            .find(|row| row.oid == oid)
            .ok_or_else(|| {
                ProjectionError::InvalidSource(format!("Observation {oid} lacks common Record row"))
            })?;
        let time = project_valid_time(
            required_object(payload, "capture_time", oid)?,
            &record.recorded_at,
            true,
            oid,
        )?;
        self.timelines.insert(TimelineRow {
            record_oid: oid.to_owned(),
            record_kind: TimelineRecordKind::Observation.as_str().to_owned(),
            entity_id: record.entity_id.clone(),
            ordering_time: time.ordering_time,
            time_basis: time.basis.as_str().to_owned(),
            event_time_start: time.start,
            event_time_end: time.end,
            recorded_at: record.recorded_at.clone(),
            asserted_by: record.asserted_by.clone(),
        });

        self.map_optional_dependency(
            oid,
            payload,
            "capture_profile_ref",
            ObservationDependencyKind::CaptureProfile,
            None,
        )?;
        self.map_optional_dependency(
            oid,
            payload,
            "station_ref",
            ObservationDependencyKind::Station,
            Some("entity"),
        )?;
        self.map_optional_dependency(
            oid,
            payload,
            "station_deployment_ref",
            ObservationDependencyKind::StationDeployment,
            None,
        )?;
        self.map_oid_array(
            oid,
            payload,
            "calibration_refs",
            ObservationDependencyKind::Calibration,
        )?;
        self.map_oid_array(
            oid,
            payload,
            "environment_refs",
            ObservationDependencyKind::Environment,
        )?;
        for (ordinal, media) in required_array(payload, "media_refs", oid)?
            .iter()
            .enumerate()
        {
            let target = required_string(media, "oid", oid)?;
            let role = required_string(media, "role", oid)?;
            self.dependencies.insert(DependencyRow {
                observation_oid: oid.to_owned(),
                dependency_kind: ObservationDependencyKind::Media.as_str().to_owned(),
                target_ref: target.to_owned(),
                target_kind: kind_name(parse_oid(target).map_err(|error| {
                    ProjectionError::InvalidSource(format!(
                        "Observation {oid} media OID is invalid: {error}"
                    ))
                })?)
                .to_owned(),
                role: Some(role.to_owned()),
                ordinal,
            });
        }
        Ok(())
    }

    fn map_activity(&mut self, oid: &str, value: &Value) -> Result<()> {
        let payload = required_object(value, "payload", oid)?;
        for subject in required_array(payload, "subject_refs", oid)? {
            let subject = subject.as_str().ok_or_else(|| {
                ProjectionError::InvalidSource(format!(
                    "Activity {oid} contains non-string subject_ref"
                ))
            })?;
            self.subject_links.insert(SubjectLinkRow {
                record_oid: oid.to_owned(),
                subject_id: subject.to_owned(),
            });
        }
        let record = self
            .records
            .iter()
            .find(|row| row.oid == oid)
            .ok_or_else(|| {
                ProjectionError::InvalidSource(format!("Activity {oid} lacks common Record row"))
            })?;
        let time = project_valid_time(
            required_object(value, "valid_time", oid)?,
            &record.recorded_at,
            false,
            oid,
        )?;
        self.timelines.insert(TimelineRow {
            record_oid: oid.to_owned(),
            record_kind: TimelineRecordKind::Activity.as_str().to_owned(),
            entity_id: record.entity_id.clone(),
            ordering_time: time.ordering_time,
            time_basis: time.basis.as_str().to_owned(),
            event_time_start: time.start,
            event_time_end: time.end,
            recorded_at: record.recorded_at.clone(),
            asserted_by: record.asserted_by.clone(),
        });
        Ok(())
    }

    fn map_optional_dependency(
        &mut self,
        observation_oid: &str,
        payload: &Value,
        field: &str,
        kind: ObservationDependencyKind,
        forced_target_kind: Option<&str>,
    ) -> Result<()> {
        let Some(target) = payload.get(field).and_then(Value::as_str) else {
            return Ok(());
        };
        let target_kind = match forced_target_kind {
            Some(kind) => kind.to_owned(),
            None => kind_name(parse_oid(target).map_err(|error| {
                ProjectionError::InvalidSource(format!(
                    "Observation {observation_oid} {field} is invalid: {error}"
                ))
            })?)
            .to_owned(),
        };
        self.dependencies.insert(DependencyRow {
            observation_oid: observation_oid.to_owned(),
            dependency_kind: kind.as_str().to_owned(),
            target_ref: target.to_owned(),
            target_kind,
            role: None,
            ordinal: 0,
        });
        Ok(())
    }

    fn map_oid_array(
        &mut self,
        observation_oid: &str,
        payload: &Value,
        field: &str,
        kind: ObservationDependencyKind,
    ) -> Result<()> {
        let Some(values) = payload.get(field).and_then(Value::as_array) else {
            return Ok(());
        };
        for (ordinal, value) in values.iter().enumerate() {
            let target = value.as_str().ok_or_else(|| {
                ProjectionError::InvalidSource(format!(
                    "Observation {observation_oid} {field} contains a non-string"
                ))
            })?;
            self.dependencies.insert(DependencyRow {
                observation_oid: observation_oid.to_owned(),
                dependency_kind: kind.as_str().to_owned(),
                target_ref: target.to_owned(),
                target_kind: kind_name(parse_oid(target).map_err(|error| {
                    ProjectionError::InvalidSource(format!(
                        "Observation {observation_oid} {field} OID is invalid: {error}"
                    ))
                })?)
                .to_owned(),
                role: None,
                ordinal,
            });
        }
        Ok(())
    }

    fn metadata(&self) -> ProjectionMetadata {
        ProjectionMetadata {
            schema_version: PROJECTION_SCHEMA_VERSION,
            source_fingerprint: self.source_fingerprint.clone(),
            ref_count: self.refs.len() as u64,
            object_count: self.objects.len() as u64,
            edge_count: self.edges.len() as u64,
            incomplete_ref_count: self.summaries.iter().filter(|row| !row.complete).count() as u64,
        }
    }
}

struct ProjectedTime {
    ordering_time: String,
    basis: TimelineTimeBasis,
    start: Option<String>,
    end: Option<String>,
}

fn project_valid_time(
    valid_time: &Value,
    recorded_at: &str,
    observation: bool,
    oid: &str,
) -> Result<ProjectedTime> {
    match valid_time.get("kind").and_then(Value::as_str) {
        Some("instant") => {
            let at = required_string(valid_time, "at", oid)?.to_owned();
            Ok(ProjectedTime {
                ordering_time: at.clone(),
                basis: if observation {
                    TimelineTimeBasis::ObservationCaptureInstant
                } else {
                    TimelineTimeBasis::ActivityValidInstant
                },
                start: Some(at),
                end: None,
            })
        }
        Some("interval") => {
            let start = valid_time
                .get("from")
                .and_then(Value::as_str)
                .map(str::to_owned);
            let end = valid_time
                .get("to")
                .and_then(Value::as_str)
                .map(str::to_owned);
            let ordering_time = start
                .as_ref()
                .or(end.as_ref())
                .ok_or_else(|| {
                    ProjectionError::InvalidSource(format!(
                        "Record {oid} has an interval without from or to"
                    ))
                })?
                .clone();
            Ok(ProjectedTime {
                ordering_time,
                basis: if observation {
                    TimelineTimeBasis::ObservationCaptureInterval
                } else {
                    TimelineTimeBasis::ActivityValidInterval
                },
                start,
                end,
            })
        }
        Some("unknown") => Ok(ProjectedTime {
            ordering_time: recorded_at.to_owned(),
            basis: if observation {
                TimelineTimeBasis::ObservationRecordedAtFallback
            } else {
                TimelineTimeBasis::ActivityRecordedAtFallback
            },
            start: None,
            end: None,
        }),
        kind => Err(ProjectionError::InvalidSource(format!(
            "Record {oid} has unsupported ValidTime kind {kind:?}"
        ))),
    }
}

fn normalize_snapshot(snapshot: &RefSnapshot) -> Result<Vec<RefRecord>> {
    let mut by_name = BTreeMap::<String, RefRecord>::new();
    let mut event_ids = BTreeSet::new();
    for reference in &snapshot.refs {
        validate_ref_name(&reference.name).map_err(|error| {
            ProjectionError::InvalidSnapshot(format!(
                "invalid Ref name {:?}: {error}",
                reference.name
            ))
        })?;
        if parse_oid(&reference.head).map_err(|error| {
            ProjectionError::InvalidSnapshot(format!(
                "Ref {:?} has invalid head: {error}",
                reference.name
            ))
        })? != ObjectKind::Commit
        {
            return Err(ProjectionError::InvalidSnapshot(format!(
                "Ref {:?} head is not a Commit OID",
                reference.name
            )));
        }
        if reference.updated_event_id <= 0 {
            return Err(ProjectionError::InvalidSnapshot(format!(
                "Ref {:?} has non-positive updated_event_id {}",
                reference.name, reference.updated_event_id
            )));
        }
        if !event_ids.insert(reference.updated_event_id) {
            return Err(ProjectionError::InvalidSnapshot(format!(
                "updated_event_id {} is shared by multiple Refs",
                reference.updated_event_id
            )));
        }
        if by_name
            .insert(reference.name.clone(), reference.clone())
            .is_some()
        {
            return Err(ProjectionError::InvalidSnapshot(format!(
                "Ref {:?} appears more than once",
                reference.name
            )));
        }
    }
    Ok(by_name.into_values().collect())
}

fn merge_object(objects: &mut BTreeMap<String, ObjectRow>, candidate: ObjectRow) -> Result<()> {
    match objects.get(&candidate.oid) {
        Some(existing) if existing != &candidate => Err(ProjectionError::InvalidSource(format!(
            "object {} has inconsistent closure states",
            candidate.oid
        ))),
        Some(_) => Ok(()),
        None => {
            objects.insert(candidate.oid.clone(), candidate);
            Ok(())
        }
    }
}

fn map_source_store_error(error: StoreError) -> ProjectionError {
    if error.code() == Some(ErrorCode::ResourceLimit) {
        return ProjectionError::ResourceLimit(error.to_string());
    }
    if matches!(
        &error,
        StoreError::Core(_)
            | StoreError::CorruptObject { .. }
            | StoreError::InvalidStoreLayout { .. }
    ) {
        ProjectionError::InvalidSource(error.to_string())
    } else {
        ProjectionError::ObjectStore(error)
    }
}

fn validate_resolving_tombstone(
    store: &FileObjectStore,
    target_oid: &str,
    tombstone_oid: &str,
) -> Result<()> {
    if tombstone_oid == target_oid {
        return Err(ProjectionError::InvalidSource(format!(
            "Tombstone {tombstone_oid} targets itself"
        )));
    }
    let object = store
        .get_verified(tombstone_oid)
        .map_err(map_source_store_error)?
        .ok_or_else(|| {
            ProjectionError::InvalidSource(format!(
                "resolving Tombstone is missing: {tombstone_oid}"
            ))
        })?;
    if object.kind() != ObjectKind::Record {
        return Err(ProjectionError::InvalidSource(format!(
            "Tombstone resolver is not a Record: {tombstone_oid}"
        )));
    }
    let value = object.structured().ok_or_else(|| {
        ProjectionError::InvalidSource(format!(
            "Tombstone resolver has no structured body: {tombstone_oid}"
        ))
    })?;
    validate(value).map_err(|error| {
        ProjectionError::InvalidSource(format!(
            "Tombstone resolver {tombstone_oid} is invalid: {error}"
        ))
    })?;
    if value.get("record_type").and_then(Value::as_str) != Some("tombstone")
        || value
            .get("payload")
            .and_then(|payload| payload.get("target_ref"))
            .and_then(Value::as_str)
            != Some(target_oid)
    {
        return Err(ProjectionError::InvalidSource(format!(
            "Tombstone {tombstone_oid} does not resolve target {target_oid}"
        )));
    }
    Ok(())
}

fn availability_for_state(state: &ClosureNodeState) -> ObjectAvailability {
    match state {
        ClosureNodeState::Present { .. } => ObjectAvailability::Present,
        ClosureNodeState::Tombstoned { .. } => ObjectAvailability::Tombstoned,
        ClosureNodeState::Missing { .. } => ObjectAvailability::Missing,
        ClosureNodeState::Corrupt { .. } | ClosureNodeState::ReadFailure { .. } => {
            unreachable!("unreadable closure state is rejected before reachability insertion")
        }
    }
}

fn closure_issue_description(kind: &ClosureIssueKind) -> String {
    match kind {
        ClosureIssueKind::Missing => "missing object".to_owned(),
        ClosureIssueKind::Corrupt { detail } => format!("corrupt object: {detail}"),
        ClosureIssueKind::ReadFailure { detail } => format!("read failure: {detail}"),
        ClosureIssueKind::ReferenceTypeMismatch { expected, actual } => format!(
            "reference kind mismatch: expected {}, actual {}",
            expected.prefix(),
            actual.prefix()
        ),
        ClosureIssueKind::ReferenceSemanticMismatch { expected, actual } => {
            format!("reference semantic mismatch: expected {expected}, actual {actual}")
        }
        ClosureIssueKind::InvalidObject { detail } => format!("invalid object: {detail}"),
        ClosureIssueKind::InvalidReference { value, detail } => {
            format!("invalid reference {value:?}: {detail}")
        }
        ClosureIssueKind::Cycle { path } => format!("cycle: {}", path.join(" -> ")),
        ClosureIssueKind::ResourceLimit { resource, limit } => {
            format!("{resource} resource limit {limit}")
        }
    }
}

fn required_string<'value>(value: &'value Value, key: &str, oid: &str) -> Result<&'value str> {
    value.get(key).and_then(Value::as_str).ok_or_else(|| {
        ProjectionError::InvalidSource(format!("object {oid} requires string field {key}"))
    })
}

fn required_object<'value>(value: &'value Value, key: &str, oid: &str) -> Result<&'value Value> {
    let child = value.get(key).ok_or_else(|| {
        ProjectionError::InvalidSource(format!("object {oid} requires object field {key}"))
    })?;
    child.as_object().ok_or_else(|| {
        ProjectionError::InvalidSource(format!("object {oid} field {key} is not an object"))
    })?;
    Ok(child)
}

fn required_array<'value>(value: &'value Value, key: &str, oid: &str) -> Result<&'value [Value]> {
    value.get(key).and_then(Value::as_array).ok_or_else(|| {
        ProjectionError::InvalidSource(format!("object {oid} requires array field {key}"))
    })
}

const fn kind_name(kind: ObjectKind) -> &'static str {
    kind.prefix()
}

fn parse_kind(value: &str) -> Result<ObjectKind> {
    match value {
        "blob" => Ok(ObjectKind::Blob),
        "record" => Ok(ObjectKind::Record),
        "tree" => Ok(ObjectKind::Tree),
        "commit" => Ok(ObjectKind::Commit),
        _ => Err(ProjectionError::CorruptProjection(format!(
            "unknown object kind {value:?}"
        ))),
    }
}

fn analysis_object_ref_from_parts(
    oid: String,
    kind: String,
    availability: String,
) -> Result<AnalysisObjectRef> {
    Ok(AnalysisObjectRef {
        oid,
        kind: parse_kind(&kind)?,
        availability: ObjectAvailability::parse(&availability)?,
    })
}

fn analysis_replay_readiness(
    adapter: &AnalysisAdapter,
    inputs: &[AnalysisInput],
    transforms: &[AnalysisObjectRef],
) -> AnalysisReplayReadiness {
    let prerequisite_availability = [
        adapter.implementation.availability,
        adapter.configuration.availability,
    ]
    .into_iter()
    .chain(inputs.iter().map(|input| input.object.availability))
    .chain(transforms.iter().map(|transform| transform.availability));
    let mut missing = false;
    let mut tombstoned = false;
    for availability in prerequisite_availability {
        missing |= availability == ObjectAvailability::Missing;
        tombstoned |= availability == ObjectAvailability::Tombstoned;
    }
    match (missing, tombstoned) {
        (false, false) => AnalysisReplayReadiness::Ready,
        (true, false) => AnalysisReplayReadiness::BlockedMissing,
        (false, true) => AnalysisReplayReadiness::BlockedTombstoned,
        (true, true) => AnalysisReplayReadiness::BlockedMissingAndTombstoned,
    }
}

fn fingerprint(plan: &BuildPlan) -> String {
    let mut hasher = Sha256::new();
    hash_field(&mut hasher, b"synapse-projection-source-v1");
    for reference in &plan.refs {
        hash_field(&mut hasher, b"ref");
        hash_field(&mut hasher, reference.name.as_bytes());
        hash_field(&mut hasher, reference.head.as_bytes());
        hash_field(&mut hasher, &reference.updated_event_id.to_be_bytes());
    }
    for object in plan.objects.values() {
        hash_field(&mut hasher, b"object");
        hash_field(&mut hasher, object.oid.as_bytes());
        hash_field(&mut hasher, kind_name(object.kind).as_bytes());
        hash_field(&mut hasher, object.availability.as_str().as_bytes());
        hash_optional_u64(&mut hasher, object.byte_len);
        hash_optional_string(&mut hasher, object.tombstone_oid.as_deref());
    }
    for edge in &plan.edges {
        hash_field(&mut hasher, b"edge");
        hash_field(&mut hasher, edge.source_oid.as_bytes());
        hash_field(&mut hasher, edge.target_oid.as_bytes());
        hash_field(&mut hasher, edge.role.as_bytes());
        hash_field(&mut hasher, edge.expected_kind.as_bytes());
    }
    let digest = hasher.finalize();
    let mut hex = String::with_capacity(digest.len() * 2);
    for byte in digest {
        write!(&mut hex, "{byte:02x}").expect("writing to String cannot fail");
    }
    format!("projection-source-v1:sha256:{hex}")
}

fn hash_field(hasher: &mut Sha256, bytes: &[u8]) {
    hasher.update((bytes.len() as u64).to_be_bytes());
    hasher.update(bytes);
}

fn hash_optional_u64(hasher: &mut Sha256, value: Option<u64>) {
    match value {
        Some(value) => {
            hash_field(hasher, b"some");
            hash_field(hasher, &value.to_be_bytes());
        }
        None => hash_field(hasher, b"none"),
    }
}

fn hash_optional_string(hasher: &mut Sha256, value: Option<&str>) {
    match value {
        Some(value) => {
            hash_field(hasher, b"some");
            hash_field(hasher, value.as_bytes());
        }
        None => hash_field(hasher, b"none"),
    }
}

fn meta_value(connection: &Connection, key: &str) -> Result<Option<String>> {
    connection
        .query_row(
            "SELECT value FROM projection_meta WHERE key = ?1",
            [key],
            |row| row.get(0),
        )
        .optional()
        .map_err(Into::into)
}

fn parse_meta_u64(connection: &Connection, key: &str) -> Result<u64> {
    let value = meta_value(connection, key)?.ok_or_else(|| {
        ProjectionError::CorruptProjection(format!("projection metadata lacks {key:?}"))
    })?;
    value.parse().map_err(|_| {
        ProjectionError::CorruptProjection(format!(
            "projection metadata {key:?} is not a u64: {value:?}"
        ))
    })
}

fn projected_object_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ProjectedObject> {
    let kind = row.get::<_, String>(1)?;
    let availability = row.get::<_, String>(2)?;
    let byte_len = row.get::<_, Option<i64>>(3)?;
    let byte_len = byte_len
        .map(|value| {
            u64::try_from(value).map_err(|_| rusqlite::Error::IntegralValueOutOfRange(3, value))
        })
        .transpose()?;
    let kind = parse_kind(&kind).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(1, rusqlite::types::Type::Text, Box::new(error))
    })?;
    let availability = ObjectAvailability::parse(&availability).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(2, rusqlite::types::Type::Text, Box::new(error))
    })?;
    Ok(ProjectedObject {
        oid: row.get(0)?,
        kind,
        availability,
        byte_len,
        tombstone_oid: row.get(4)?,
        record_type: row.get(5)?,
        entity_id: row.get(6)?,
        recorded_at: row.get(7)?,
        asserted_by: row.get(8)?,
    })
}

fn create_schema(transaction: &Transaction<'_>) -> Result<()> {
    transaction.execute_batch(
        "CREATE TABLE IF NOT EXISTS ref_heads (
            ref_name TEXT PRIMARY KEY NOT NULL,
            head_oid TEXT NOT NULL,
            updated_event_id INTEGER NOT NULL UNIQUE CHECK(updated_event_id > 0)
        ) STRICT;

        CREATE TABLE IF NOT EXISTS objects (
            oid TEXT PRIMARY KEY NOT NULL,
            kind TEXT NOT NULL CHECK(kind IN ('blob', 'record', 'tree', 'commit')),
            availability TEXT NOT NULL CHECK(availability IN ('present', 'tombstoned', 'missing')),
            byte_len INTEGER CHECK(byte_len IS NULL OR byte_len >= 0),
            tombstone_oid TEXT,
            record_type TEXT,
            entity_id TEXT,
            recorded_at TEXT,
            asserted_by TEXT,
            CHECK((availability = 'present' AND byte_len IS NOT NULL AND tombstone_oid IS NULL)
               OR (availability = 'tombstoned' AND byte_len IS NULL AND tombstone_oid IS NOT NULL)
               OR (availability = 'missing' AND byte_len IS NULL AND tombstone_oid IS NULL))
        ) STRICT;

        CREATE TABLE IF NOT EXISTS ref_reachability (
            ref_name TEXT NOT NULL REFERENCES ref_heads(ref_name) ON DELETE CASCADE,
            oid TEXT NOT NULL REFERENCES objects(oid) ON DELETE CASCADE,
            depth INTEGER NOT NULL CHECK(depth >= 0),
            availability TEXT NOT NULL CHECK(availability IN ('present', 'tombstoned', 'missing')),
            PRIMARY KEY(ref_name, oid)
        ) STRICT;
        CREATE INDEX IF NOT EXISTS ref_reachability_oid_ref
            ON ref_reachability(oid, ref_name);

        CREATE TABLE IF NOT EXISTS object_edges (
            source_oid TEXT NOT NULL REFERENCES objects(oid) ON DELETE CASCADE,
            target_oid TEXT NOT NULL REFERENCES objects(oid) ON DELETE CASCADE,
            role TEXT NOT NULL,
            expected_kind TEXT NOT NULL CHECK(expected_kind IN ('blob', 'record', 'tree', 'commit')),
            PRIMARY KEY(source_oid, target_oid, role)
        ) STRICT;
        CREATE INDEX IF NOT EXISTS object_edges_target
            ON object_edges(target_oid, source_oid);

        CREATE TABLE IF NOT EXISTS records (
            oid TEXT PRIMARY KEY NOT NULL REFERENCES objects(oid) ON DELETE CASCADE,
            record_type TEXT NOT NULL,
            entity_id TEXT NOT NULL,
            recorded_at TEXT NOT NULL,
            asserted_by TEXT NOT NULL
        ) STRICT;
        CREATE INDEX IF NOT EXISTS records_entity_time
            ON records(entity_id, recorded_at, oid);

        CREATE TABLE IF NOT EXISTS subject_links (
            record_oid TEXT NOT NULL REFERENCES records(oid) ON DELETE CASCADE,
            subject_id TEXT NOT NULL,
            PRIMARY KEY(record_oid, subject_id)
        ) STRICT;
        CREATE INDEX IF NOT EXISTS subject_links_subject
            ON subject_links(subject_id, record_oid);

        CREATE TABLE IF NOT EXISTS series_links (
            record_oid TEXT PRIMARY KEY NOT NULL REFERENCES records(oid) ON DELETE CASCADE,
            series_id TEXT NOT NULL
        ) STRICT;
        CREATE INDEX IF NOT EXISTS series_links_series
            ON series_links(series_id, record_oid);

        CREATE TABLE IF NOT EXISTS timeline_records (
            record_oid TEXT PRIMARY KEY NOT NULL REFERENCES records(oid) ON DELETE CASCADE,
            record_kind TEXT NOT NULL CHECK(record_kind IN ('observation', 'activity')),
            entity_id TEXT NOT NULL,
            ordering_time TEXT NOT NULL,
            time_basis TEXT NOT NULL,
            event_time_start TEXT,
            event_time_end TEXT,
            recorded_at TEXT NOT NULL,
            asserted_by TEXT NOT NULL
        ) STRICT;
        CREATE INDEX IF NOT EXISTS timeline_order
            ON timeline_records(ordering_time, record_oid);

        CREATE TABLE IF NOT EXISTS observation_dependencies (
            observation_oid TEXT NOT NULL REFERENCES records(oid) ON DELETE CASCADE,
            dependency_kind TEXT NOT NULL CHECK(dependency_kind IN (
                'capture_profile', 'station', 'station_deployment',
                'calibration', 'environment', 'media'
            )),
            target_ref TEXT NOT NULL,
            target_kind TEXT NOT NULL CHECK(target_kind IN ('entity', 'blob', 'record', 'tree', 'commit')),
            role TEXT,
            ordinal INTEGER NOT NULL CHECK(ordinal >= 0),
            PRIMARY KEY(observation_oid, dependency_kind, ordinal, target_ref)
        ) STRICT;

        CREATE TABLE IF NOT EXISTS analysis_results (
            analysis_oid TEXT PRIMARY KEY NOT NULL REFERENCES records(oid) ON DELETE CASCADE,
            analysis_kind TEXT NOT NULL,
            comparison_kind TEXT NOT NULL CHECK(comparison_kind IN (
                'revision', 'temporal_observation', 'plan_observation',
                'before_after_activity', 'cross_modal', 'intent'
            )),
            status TEXT NOT NULL CHECK(status IN ('succeeded', 'failed', 'not_run')),
            comparability TEXT NOT NULL CHECK(comparability IN (
                'comparable', 'partial', 'incomparable'
            )),
            adapter_id TEXT NOT NULL,
            adapter_version TEXT NOT NULL,
            implementation_oid TEXT NOT NULL REFERENCES objects(oid) ON DELETE CASCADE,
            configuration_oid TEXT NOT NULL REFERENCES objects(oid) ON DELETE CASCADE,
            determinism TEXT NOT NULL CHECK(determinism IN (
                'deterministic', 'seeded', 'probabilistic'
            )),
            seed TEXT,
            CHECK(determinism <> 'seeded' OR seed IS NOT NULL)
        ) STRICT;
        CREATE INDEX IF NOT EXISTS analysis_results_implementation
            ON analysis_results(implementation_oid, analysis_oid);
        CREATE INDEX IF NOT EXISTS analysis_results_configuration
            ON analysis_results(configuration_oid, analysis_oid);

        CREATE TABLE IF NOT EXISTS analysis_links (
            analysis_oid TEXT NOT NULL REFERENCES analysis_results(analysis_oid) ON DELETE CASCADE,
            category TEXT NOT NULL CHECK(category IN (
                'input', 'transform', 'derived_blob', 'mask'
            )),
            ordinal INTEGER NOT NULL CHECK(ordinal >= 0),
            role TEXT,
            target_oid TEXT NOT NULL REFERENCES objects(oid) ON DELETE CASCADE,
            PRIMARY KEY(analysis_oid, category, ordinal),
            CHECK((category IN ('input', 'mask') AND role IS NOT NULL)
               OR (category IN ('transform', 'derived_blob') AND role IS NULL)),
            CHECK(category <> 'mask' OR role IN (
                'changed', 'unchanged', 'ambiguous', 'unobservable', 'validity'
            ))
        ) STRICT;
        CREATE INDEX IF NOT EXISTS analysis_links_target
            ON analysis_links(target_oid, analysis_oid, category);

        CREATE TABLE IF NOT EXISTS closure_summaries (
            ref_name TEXT PRIMARY KEY NOT NULL REFERENCES ref_heads(ref_name) ON DELETE CASCADE,
            head_oid TEXT NOT NULL,
            complete INTEGER NOT NULL CHECK(complete IN (0, 1)),
            truncated INTEGER NOT NULL CHECK(truncated IN (0, 1)),
            issue_count INTEGER NOT NULL CHECK(issue_count >= 0),
            present_count INTEGER NOT NULL CHECK(present_count >= 0),
            tombstoned_count INTEGER NOT NULL CHECK(tombstoned_count >= 0),
            missing_count INTEGER NOT NULL CHECK(missing_count >= 0)
        ) STRICT;

        CREATE TABLE IF NOT EXISTS closure_issues (
            ref_name TEXT NOT NULL REFERENCES ref_heads(ref_name) ON DELETE CASCADE,
            ordinal INTEGER NOT NULL CHECK(ordinal >= 0),
            oid TEXT NOT NULL,
            referenced_by TEXT,
            role TEXT,
            issue_kind TEXT NOT NULL,
            detail TEXT,
            PRIMARY KEY(ref_name, ordinal)
        ) STRICT;",
    )?;
    Ok(())
}

fn replace_rows(
    transaction: &Transaction<'_>,
    plan: &BuildPlan,
    metadata: &ProjectionMetadata,
) -> Result<()> {
    transaction.execute("DELETE FROM ref_heads", [])?;
    transaction.execute("DELETE FROM objects", [])?;
    transaction.execute(
        "DELETE FROM projection_meta WHERE key <> 'schema_version'",
        [],
    )?;

    {
        let mut statement = transaction.prepare(
            "INSERT INTO objects(
                oid, kind, availability, byte_len, tombstone_oid,
                record_type, entity_id, recorded_at, asserted_by
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        )?;
        for row in plan.objects.values() {
            statement.execute(params![
                row.oid,
                kind_name(row.kind),
                row.availability.as_str(),
                checked_i64(row.byte_len, "object byte length")?,
                row.tombstone_oid,
                row.record_type,
                row.entity_id,
                row.recorded_at,
                row.asserted_by,
            ])?;
        }
    }
    {
        let mut statement = transaction.prepare(
            "INSERT INTO ref_heads(ref_name, head_oid, updated_event_id) VALUES (?1, ?2, ?3)",
        )?;
        for row in &plan.refs {
            statement.execute(params![row.name, row.head, row.updated_event_id])?;
        }
    }
    {
        let mut statement = transaction.prepare(
            "INSERT INTO ref_reachability(ref_name, oid, depth, availability)
             VALUES (?1, ?2, ?3, ?4)",
        )?;
        for row in &plan.reachability {
            statement.execute(params![
                row.ref_name,
                row.oid,
                checked_usize_i64(row.depth, "closure depth")?,
                row.availability.as_str(),
            ])?;
        }
    }
    {
        let mut statement = transaction.prepare(
            "INSERT INTO object_edges(source_oid, target_oid, role, expected_kind)
             VALUES (?1, ?2, ?3, ?4)",
        )?;
        for row in &plan.edges {
            statement.execute(params![
                row.source_oid,
                row.target_oid,
                row.role,
                row.expected_kind,
            ])?;
        }
    }
    {
        let mut statement = transaction.prepare(
            "INSERT INTO records(oid, record_type, entity_id, recorded_at, asserted_by)
             VALUES (?1, ?2, ?3, ?4, ?5)",
        )?;
        for row in &plan.records {
            statement.execute(params![
                row.oid,
                row.record_type,
                row.entity_id,
                row.recorded_at,
                row.asserted_by,
            ])?;
        }
    }
    {
        let mut statement = transaction
            .prepare("INSERT INTO subject_links(record_oid, subject_id) VALUES (?1, ?2)")?;
        for row in &plan.subject_links {
            statement.execute(params![row.record_oid, row.subject_id])?;
        }
    }
    {
        let mut statement = transaction
            .prepare("INSERT INTO series_links(record_oid, series_id) VALUES (?1, ?2)")?;
        for row in &plan.series_links {
            statement.execute(params![row.record_oid, row.series_id])?;
        }
    }
    {
        let mut statement = transaction.prepare(
            "INSERT INTO timeline_records(
                record_oid, record_kind, entity_id, ordering_time, time_basis,
                event_time_start, event_time_end, recorded_at, asserted_by
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        )?;
        for row in &plan.timelines {
            statement.execute(params![
                row.record_oid,
                row.record_kind,
                row.entity_id,
                row.ordering_time,
                row.time_basis,
                row.event_time_start,
                row.event_time_end,
                row.recorded_at,
                row.asserted_by,
            ])?;
        }
    }
    {
        let mut statement = transaction.prepare(
            "INSERT INTO observation_dependencies(
                observation_oid, dependency_kind, target_ref, target_kind, role, ordinal
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        )?;
        for row in &plan.dependencies {
            statement.execute(params![
                row.observation_oid,
                row.dependency_kind,
                row.target_ref,
                row.target_kind,
                row.role,
                checked_usize_i64(row.ordinal, "dependency ordinal")?,
            ])?;
        }
    }
    {
        let mut statement = transaction.prepare(
            "INSERT INTO analysis_results(
                analysis_oid, analysis_kind, comparison_kind, status, comparability,
                adapter_id, adapter_version, implementation_oid, configuration_oid,
                determinism, seed
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        )?;
        for row in &plan.analyses {
            statement.execute(params![
                row.analysis_oid,
                row.analysis_kind,
                row.comparison_kind,
                row.status,
                row.comparability,
                row.adapter_id,
                row.adapter_version,
                row.implementation_oid,
                row.configuration_oid,
                row.determinism,
                row.seed,
            ])?;
        }
    }
    {
        let mut statement = transaction.prepare(
            "INSERT INTO analysis_links(
                analysis_oid, category, ordinal, role, target_oid
             ) VALUES (?1, ?2, ?3, ?4, ?5)",
        )?;
        for row in &plan.analysis_links {
            statement.execute(params![
                row.analysis_oid,
                row.category.as_str(),
                checked_usize_i64(row.ordinal, "Analysis link ordinal")?,
                row.role,
                row.target_oid,
            ])?;
        }
    }
    {
        let mut statement = transaction.prepare(
            "INSERT INTO closure_summaries(
                ref_name, head_oid, complete, truncated, issue_count,
                present_count, tombstoned_count, missing_count
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        )?;
        for row in &plan.summaries {
            statement.execute(params![
                row.ref_name,
                row.head_oid,
                row.complete,
                row.truncated,
                checked_usize_i64(row.issue_count, "closure issue count")?,
                checked_usize_i64(row.present_count, "closure present count")?,
                checked_usize_i64(row.tombstoned_count, "closure tombstoned count")?,
                checked_usize_i64(row.missing_count, "closure missing count")?,
            ])?;
        }
    }
    {
        let mut statement = transaction.prepare(
            "INSERT INTO closure_issues(
                ref_name, ordinal, oid, referenced_by, role, issue_kind, detail
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        )?;
        for row in &plan.issues {
            statement.execute(params![
                row.ref_name,
                checked_usize_i64(row.ordinal, "closure issue ordinal")?,
                row.oid,
                row.referenced_by,
                row.role,
                row.issue_kind,
                row.detail,
            ])?;
        }
    }

    for (key, value) in [
        ("source_fingerprint", metadata.source_fingerprint.clone()),
        ("ref_count", metadata.ref_count.to_string()),
        ("object_count", metadata.object_count.to_string()),
        ("edge_count", metadata.edge_count.to_string()),
        (
            "incomplete_ref_count",
            metadata.incomplete_ref_count.to_string(),
        ),
    ] {
        transaction.execute(
            "INSERT INTO projection_meta(key, value) VALUES (?1, ?2)",
            params![key, value],
        )?;
    }
    Ok(())
}

fn checked_i64(value: Option<u64>, label: &str) -> Result<Option<i64>> {
    value
        .map(|value| {
            i64::try_from(value).map_err(|_| {
                ProjectionError::ResourceLimit(format!("{label} exceeds SQLite i64 range"))
            })
        })
        .transpose()
}

fn checked_usize_i64(value: usize, label: &str) -> Result<i64> {
    i64::try_from(value)
        .map_err(|_| ProjectionError::ResourceLimit(format!("{label} exceeds SQLite i64 range")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transaction_failure_after_clear_preserves_previous_projection() {
        let mut store = SqliteProjectionStore::open_in_memory().unwrap();
        let first = BuildPlan {
            source_fingerprint: "projection-source-v1:sha256:first".into(),
            ..BuildPlan::default()
        };
        let first_metadata = first.metadata();
        store.replace(&first, &first_metadata).unwrap();

        store
            .connection
            .execute_batch(
                "CREATE TRIGGER inject_projection_failure
                 BEFORE INSERT ON ref_heads
                 BEGIN
                    SELECT RAISE(ABORT, 'injected projection replacement failure');
                 END;",
            )
            .unwrap();
        let oid = "commit:sg-oid-v1:sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let mut second = BuildPlan {
            refs: vec![RefRecord {
                name: "decision/main".into(),
                head: oid.into(),
                updated_event_id: 1,
            }],
            source_fingerprint: "projection-source-v1:sha256:second".into(),
            ..BuildPlan::default()
        };
        second.objects.insert(
            oid.into(),
            ObjectRow {
                oid: oid.into(),
                kind: ObjectKind::Commit,
                availability: ObjectAvailability::Present,
                byte_len: Some(1),
                tombstone_oid: None,
                record_type: None,
                entity_id: None,
                recorded_at: None,
                asserted_by: None,
            },
        );
        second.reachability.insert(ReachabilityRow {
            ref_name: "decision/main".into(),
            oid: oid.into(),
            depth: 0,
            availability: ObjectAvailability::Present,
        });
        second.summaries.insert(SummaryRow {
            ref_name: "decision/main".into(),
            head_oid: oid.into(),
            complete: true,
            truncated: false,
            issue_count: 0,
            present_count: 1,
            tombstoned_count: 0,
            missing_count: 0,
        });

        let error = store.replace(&second, &second.metadata()).unwrap_err();
        assert_eq!(error.code(), "storage_error");
        assert_eq!(store.metadata().unwrap(), Some(first_metadata));
        assert!(store.get_object(oid).unwrap().is_none());
    }

    #[test]
    fn unsupported_existing_schema_is_rejected() {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(
                "CREATE TABLE projection_meta (
                    key TEXT PRIMARY KEY NOT NULL,
                    value TEXT NOT NULL
                 ) STRICT;
                 INSERT INTO projection_meta(key, value)
                 VALUES ('schema_version', '999');",
            )
            .unwrap();
        let error = SqliteProjectionStore::initialize(connection)
            .err()
            .expect("schema mismatch must fail");
        assert!(matches!(
            error,
            ProjectionError::UnsupportedSchemaVersion { ref found } if found == "999"
        ));
    }

    #[test]
    fn analysis_target_requires_a_direct_typed_graph_edge() {
        let analysis_oid = "record:sg-oid-v1:sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let unrelated_oid = "record:sg-oid-v1:sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
        let target_oid = "blob:sg-oid-v1:sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";
        let mut plan = BuildPlan::default();
        plan.objects.insert(
            target_oid.into(),
            ObjectRow {
                oid: target_oid.into(),
                kind: ObjectKind::Blob,
                availability: ObjectAvailability::Present,
                byte_len: Some(1),
                tombstone_oid: None,
                record_type: None,
                entity_id: None,
                recorded_at: None,
                asserted_by: None,
            },
        );
        plan.edges.insert(EdgeRow {
            source_oid: unrelated_oid.into(),
            target_oid: target_oid.into(),
            role: "unrelated".into(),
            expected_kind: "blob".into(),
        });

        let error = plan
            .validate_analysis_target(analysis_oid, "inputs[].ref", target_oid, None)
            .unwrap_err();
        assert!(matches!(error, ProjectionError::InvalidSource(_)));
        assert!(
            error
                .to_string()
                .contains("no matching verified graph edge")
        );

        plan.edges.insert(EdgeRow {
            source_oid: analysis_oid.into(),
            target_oid: target_oid.into(),
            role: "/payload/inputs/0/ref".into(),
            expected_kind: "blob".into(),
        });
        plan.validate_analysis_target(analysis_oid, "inputs[].ref", target_oid, None)
            .unwrap();
    }
}
