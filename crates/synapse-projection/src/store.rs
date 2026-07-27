use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::time::Duration;
use synapse_canonical::ObjectKind;
use synapse_sqlite::validate_ref_name;

use crate::error::{ProjectionError, Result};
use crate::mapping::{AnalysisLinkCategory, AnalysisQueryRow};
use crate::schema::create_schema;

pub const PROJECTION_SCHEMA_VERSION: i64 = 2;

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
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Present => "present",
            Self::Tombstoned => "tombstoned",
            Self::Missing => "missing",
        }
    }

    pub(crate) fn parse(value: &str) -> Result<Self> {
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
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Observation => "observation",
            Self::Activity => "activity",
        }
    }

    pub(crate) fn parse(value: &str) -> Result<Self> {
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
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::ObservationCaptureInstant => "observation_capture_instant",
            Self::ObservationCaptureInterval => "observation_capture_interval",
            Self::ObservationRecordedAtFallback => "observation_recorded_at_fallback",
            Self::ActivityValidInstant => "activity_valid_instant",
            Self::ActivityValidInterval => "activity_valid_interval",
            Self::ActivityRecordedAtFallback => "activity_recorded_at_fallback",
        }
    }

    pub(crate) fn parse(value: &str) -> Result<Self> {
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
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::CaptureProfile => "capture_profile",
            Self::Station => "station",
            Self::StationDeployment => "station_deployment",
            Self::Calibration => "calibration",
            Self::Environment => "environment",
            Self::Media => "media",
        }
    }

    pub(crate) fn parse(value: &str) -> Result<Self> {
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
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Deterministic => "deterministic",
            Self::Seeded => "seeded",
            Self::Probabilistic => "probabilistic",
        }
    }

    pub(crate) fn parse(value: &str) -> Result<Self> {
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
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Changed => "changed",
            Self::Unchanged => "unchanged",
            Self::Ambiguous => "ambiguous",
            Self::Unobservable => "unobservable",
            Self::Validity => "validity",
        }
    }

    pub(crate) fn parse(value: &str) -> Result<Self> {
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

pub struct SqliteProjectionStore {
    pub(crate) connection: Connection,
}

impl SqliteProjectionStore {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        Self::initialize(Connection::open(path)?)
    }

    pub fn open_in_memory() -> Result<Self> {
        Self::initialize(Connection::open_in_memory()?)
    }

    pub(crate) fn initialize(mut connection: Connection) -> Result<Self> {
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
                        issue_count: nonnegative_u64_from_row(row, 4)?,
                        present_count: nonnegative_u64_from_row(row, 5)?,
                        tombstoned_count: nonnegative_u64_from_row(row, 6)?,
                        missing_count: nonnegative_u64_from_row(row, 7)?,
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

    pub(crate) fn resolve_scope(&self, scope: &RefScope) -> Result<Vec<String>> {
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
}

pub(crate) fn meta_value(connection: &Connection, key: &str) -> Result<Option<String>> {
    connection
        .query_row(
            "SELECT value FROM projection_meta WHERE key = ?1",
            [key],
            |row| row.get(0),
        )
        .optional()
        .map_err(Into::into)
}

pub(crate) fn parse_meta_u64(connection: &Connection, key: &str) -> Result<u64> {
    let value = meta_value(connection, key)?.ok_or_else(|| {
        ProjectionError::CorruptProjection(format!("projection metadata lacks {key:?}"))
    })?;
    value.parse().map_err(|_| {
        ProjectionError::CorruptProjection(format!(
            "projection metadata {key:?} is not a u64: {value:?}"
        ))
    })
}

pub(crate) fn projected_object_from_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<ProjectedObject> {
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

pub(crate) fn nonnegative_u64_from_row(
    row: &rusqlite::Row<'_>,
    index: usize,
) -> rusqlite::Result<u64> {
    let value = row.get::<_, i64>(index)?;
    u64::try_from(value).map_err(|_| rusqlite::Error::IntegralValueOutOfRange(index, value))
}

pub(crate) fn parse_kind(value: &str) -> Result<ObjectKind> {
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

pub(crate) fn analysis_object_ref_from_parts(
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

pub(crate) fn analysis_replay_readiness(
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
