use rusqlite::{Transaction, TransactionBehavior, params};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use synapse_canonical::{ErrorCode, ObjectKind, Value, parse_oid};
use synapse_cas::{
    ClosureIssueKind, ClosureNodeState, FileObjectStore, GraphLimits, PreparedClosureVerifier,
    StoreError, TombstoneScanLimits,
};
use synapse_schema::validate;
use synapse_sqlite::{RefRecord, RefSnapshot, validate_ref_name};

use crate::error::{ProjectionError, Result};
use crate::mapping::{AnalysisLinkCategory, kind_name};
use crate::store::{
    ObjectAvailability, PROJECTION_SCHEMA_VERSION, ProjectionMetadata, RebuildReport,
    SqliteProjectionStore,
};

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

impl SqliteProjectionStore {
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

    pub(crate) fn replace(
        &mut self,
        plan: &BuildPlan,
        metadata: &ProjectionMetadata,
    ) -> Result<()> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        replace_rows(&transaction, plan, metadata)?;
        transaction.commit()?;
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ObjectRow {
    pub(crate) oid: String,
    pub(crate) kind: ObjectKind,
    pub(crate) availability: ObjectAvailability,
    pub(crate) byte_len: Option<u64>,
    pub(crate) tombstone_oid: Option<String>,
    pub(crate) record_type: Option<String>,
    pub(crate) entity_id: Option<String>,
    pub(crate) recorded_at: Option<String>,
    pub(crate) asserted_by: Option<String>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct ReachabilityRow {
    pub(crate) ref_name: String,
    pub(crate) oid: String,
    pub(crate) depth: usize,
    pub(crate) availability: ObjectAvailability,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct EdgeRow {
    pub(crate) source_oid: String,
    pub(crate) target_oid: String,
    pub(crate) role: String,
    pub(crate) expected_kind: String,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct RecordRow {
    pub(crate) oid: String,
    pub(crate) record_type: String,
    pub(crate) entity_id: String,
    pub(crate) recorded_at: String,
    pub(crate) asserted_by: String,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct SubjectLinkRow {
    pub(crate) record_oid: String,
    pub(crate) subject_id: String,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct SeriesLinkRow {
    pub(crate) record_oid: String,
    pub(crate) series_id: String,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct TimelineRow {
    pub(crate) record_oid: String,
    pub(crate) record_kind: String,
    pub(crate) entity_id: String,
    pub(crate) ordering_time: String,
    pub(crate) time_basis: String,
    pub(crate) event_time_start: Option<String>,
    pub(crate) event_time_end: Option<String>,
    pub(crate) recorded_at: String,
    pub(crate) asserted_by: String,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct DependencyRow {
    pub(crate) observation_oid: String,
    pub(crate) dependency_kind: String,
    pub(crate) target_ref: String,
    pub(crate) target_kind: String,
    pub(crate) role: Option<String>,
    pub(crate) ordinal: usize,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct AnalysisRow {
    pub(crate) analysis_oid: String,
    pub(crate) analysis_kind: String,
    pub(crate) comparison_kind: String,
    pub(crate) status: String,
    pub(crate) comparability: String,
    pub(crate) adapter_id: String,
    pub(crate) adapter_version: String,
    pub(crate) implementation_oid: String,
    pub(crate) configuration_oid: String,
    pub(crate) determinism: String,
    pub(crate) seed: Option<String>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct AnalysisLinkRow {
    pub(crate) analysis_oid: String,
    pub(crate) category: AnalysisLinkCategory,
    pub(crate) ordinal: usize,
    pub(crate) role: Option<String>,
    pub(crate) target_oid: String,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct SummaryRow {
    pub(crate) ref_name: String,
    pub(crate) head_oid: String,
    pub(crate) complete: bool,
    pub(crate) truncated: bool,
    pub(crate) issue_count: usize,
    pub(crate) present_count: usize,
    pub(crate) tombstoned_count: usize,
    pub(crate) missing_count: usize,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct IssueRow {
    pub(crate) ref_name: String,
    pub(crate) ordinal: usize,
    pub(crate) oid: String,
    pub(crate) referenced_by: Option<String>,
    pub(crate) role: Option<String>,
    pub(crate) issue_kind: String,
    pub(crate) detail: Option<String>,
}

#[derive(Default)]
pub(crate) struct BuildPlan {
    pub(crate) refs: Vec<RefRecord>,
    pub(crate) objects: BTreeMap<String, ObjectRow>,
    pub(crate) reachability: BTreeSet<ReachabilityRow>,
    pub(crate) edges: BTreeSet<EdgeRow>,
    pub(crate) records: BTreeSet<RecordRow>,
    pub(crate) subject_links: BTreeSet<SubjectLinkRow>,
    pub(crate) series_links: BTreeSet<SeriesLinkRow>,
    pub(crate) timelines: BTreeSet<TimelineRow>,
    pub(crate) dependencies: BTreeSet<DependencyRow>,
    pub(crate) analyses: BTreeSet<AnalysisRow>,
    pub(crate) analysis_links: BTreeSet<AnalysisLinkRow>,
    pub(crate) summaries: BTreeSet<SummaryRow>,
    pub(crate) issues: BTreeSet<IssueRow>,
    pub(crate) source_fingerprint: String,
}

impl BuildPlan {
    pub(crate) fn from_sources(
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

    pub(crate) fn metadata(&self) -> ProjectionMetadata {
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
