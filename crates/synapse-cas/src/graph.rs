use crate::store::{BoundedObjectStore, FileObjectStore, ObjectStore, StoreError, VerifiedObject};
use std::collections::{BTreeMap, HashSet};
use std::fmt;
use synapse_canonical::{CoreError, ErrorCode, ObjectKind, Value, parse_oid};

pub const DEFAULT_MAX_TOMBSTONE_RECORD_OBJECTS: usize = 100_000;
pub const DEFAULT_MAX_TOMBSTONE_RECORD_BYTES: u64 = 1024 * 1024 * 1024;
/// Hard ceiling for cumulative dynamically sized reference-role metadata
/// retained by one closure report. The extractor charges for every copy that
/// may coexist in its pending visit, edge, and issue representations.
pub const MAX_GRAPH_REFERENCE_BYTES: usize = 64 * 1024 * 1024;

/// Resource limits for the store-wide Record scan used to resolve Tombstones.
/// Both limits are inclusive. Every digest-verified Record contributes to the
/// byte limit, whether or not it is a Tombstone.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TombstoneScanLimits {
    /// Maximum Record OIDs returned by the bounded inventory provider.
    pub max_record_objects: usize,
    /// Maximum cumulative canonical stored byte length of verified Records.
    /// Detection may read one additional Record, itself bounded by
    /// [`crate::StoreLimits`], before rejecting the cumulative overflow.
    pub max_record_bytes: u64,
}

impl Default for TombstoneScanLimits {
    fn default() -> Self {
        Self {
            max_record_objects: DEFAULT_MAX_TOMBSTONE_RECORD_OBJECTS,
            max_record_bytes: DEFAULT_MAX_TOMBSTONE_RECORD_BYTES,
        }
    }
}

/// Work limits for untrusted Commit/Tree graph traversal.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GraphLimits {
    pub max_objects: usize,
    pub max_edges: usize,
    /// Root depth is zero; a direct reference has depth one.
    pub max_depth: usize,
}

impl Default for GraphLimits {
    fn default() -> Self {
        Self {
            max_objects: 100_000,
            max_edges: 1_000_000,
            max_depth: 512,
        }
    }
}

/// The schema-level reason for a Commit or Tree reference.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum ReferenceRole {
    CommitParent { index: usize },
    CommitSnapshot,
    CommitTransition { index: usize },
    CommitBoundDeclaration { index: usize },
    TreeEntry { segment: String },
    RecordReference { pointer: String },
    RecordSupersedes,
}

impl fmt::Display for ReferenceRole {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CommitParent { index } => write!(formatter, "parents[{index}]"),
            Self::CommitSnapshot => formatter.write_str("snapshot"),
            Self::CommitTransition { index } => write!(formatter, "transition_refs[{index}]"),
            Self::CommitBoundDeclaration { index } => {
                write!(formatter, "bound_declaration_refs[{index}]")
            }
            Self::TreeEntry { segment } => write!(formatter, "entries[{segment:?}]"),
            Self::RecordReference { pointer } => write!(formatter, "record{pointer}"),
            Self::RecordSupersedes => formatter.write_str("supersedes"),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GraphEdge {
    pub source: String,
    pub target: String,
    pub expected_kind: ObjectKind,
    pub role: ReferenceRole,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ClosureNodeState {
    Present {
        kind: ObjectKind,
        byte_len: u64,
    },
    Tombstoned {
        kind: ObjectKind,
        tombstone_oid: String,
    },
    Missing {
        kind: ObjectKind,
    },
    Corrupt {
        kind: ObjectKind,
        detail: String,
    },
    ReadFailure {
        kind: ObjectKind,
        detail: String,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClosureNode {
    pub oid: String,
    pub depth: usize,
    pub state: ClosureNodeState,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ClosureIssueKind {
    Missing,
    Corrupt {
        detail: String,
    },
    ReadFailure {
        detail: String,
    },
    ReferenceTypeMismatch {
        expected: ObjectKind,
        actual: ObjectKind,
    },
    ReferenceSemanticMismatch {
        expected: String,
        actual: String,
    },
    InvalidObject {
        detail: String,
    },
    InvalidReference {
        value: String,
        detail: String,
    },
    Cycle {
        path: Vec<String>,
    },
    ResourceLimit {
        resource: &'static str,
        limit: usize,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClosureIssue {
    /// OID being diagnosed. For malformed reference text this is the source OID.
    pub oid: String,
    pub referenced_by: Option<String>,
    pub role: Option<ReferenceRole>,
    pub kind: ClosureIssueKind,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClosureReport {
    pub root: String,
    pub nodes: BTreeMap<String, ClosureNode>,
    pub edges: Vec<GraphEdge>,
    pub issues: Vec<ClosureIssue>,
    pub truncated: bool,
}

impl ClosureReport {
    pub fn is_complete(&self) -> bool {
        !self.truncated && self.issues.is_empty()
    }
}

/// A point-in-operation Tombstone catalog plus graph limits that can verify
/// several roots without repeating the store-wide Record scan.
///
/// Callers should prepare one verifier per consistency operation while the
/// cooperative no-GC/no-removal boundary is held. The catalog intentionally
/// does not observe Tombstones published after preparation; those become
/// visible to the next prepared operation.
pub struct PreparedClosureVerifier<'store, S: BoundedObjectStore + ?Sized> {
    store: &'store S,
    graph_limits: GraphLimits,
    tombstones: BTreeMap<String, String>,
    report_cache: BTreeMap<String, ClosureReport>,
}

impl<'store, S: BoundedObjectStore + ?Sized> PreparedClosureVerifier<'store, S> {
    pub fn new(
        store: &'store S,
        graph_limits: GraphLimits,
        tombstone_limits: TombstoneScanLimits,
    ) -> Result<Self, StoreError> {
        validate_tombstone_scan_limits(tombstone_limits)?;
        let tombstones = collect_tombstones_bounded(store, tombstone_limits)?;
        Ok(Self {
            store,
            graph_limits,
            tombstones,
            report_cache: BTreeMap::new(),
        })
    }

    pub fn verify(&mut self, root: &str) -> Result<ClosureReport, StoreError> {
        if let Some(report) = self.report_cache.get(root) {
            return Ok(report.clone());
        }
        let report = self.verify_uncached(root)?;
        self.report_cache.insert(root.to_owned(), report.clone());
        Ok(report)
    }

    /// Verify one root without retaining its potentially large report.
    ///
    /// This is intended for callers that already deduplicate roots and need to
    /// keep one shared Tombstone catalog without accumulating every report.
    pub fn verify_uncached(&self, root: &str) -> Result<ClosureReport, StoreError> {
        verify_closure_with_tombstones(self.store, root, self.graph_limits, &self.tombstones)
    }

    /// Verify one root with stricter per-call object and edge budgets while
    /// retaining this verifier's configured depth limit and shared Tombstone
    /// catalog. The supplied budgets can narrow, but never raise, the limits
    /// established when the verifier was created.
    pub fn verify_uncached_with_work_limits(
        &self,
        root: &str,
        max_objects: usize,
        max_edges: usize,
    ) -> Result<ClosureReport, StoreError> {
        let limits = GraphLimits {
            max_objects: self.graph_limits.max_objects.min(max_objects),
            max_edges: self.graph_limits.max_edges.min(max_edges),
            max_depth: self.graph_limits.max_depth,
        };
        verify_closure_with_tombstones(self.store, root, limits, &self.tombstones)
    }
}

/// Traverse the required object closure rooted at a Commit.
///
/// Only normative Stage 0 structural edges are followed: Commit parents,
/// snapshot, transition refs and bound declaration refs, plus every Tree entry
/// and the typed Record references defined in Operations section 4. Other
/// Record-internal semantic validation remains the schema/semantic validator's
/// responsibility. Traversal is iterative, so the configured depth bound is
/// enforced without relying on the process call stack. Missing and corrupt
/// objects are reported as closure issues; operational store and configured
/// read-limit failures abort traversal as [`StoreError`].
///
/// This compatibility entry point uses the complete `ObjectStore::list_oids`
/// inventory to discover Tombstones. Service paths that need a hard inventory
/// bound or verify several roots should use [`PreparedClosureVerifier`].
pub fn verify_closure<S: ObjectStore + ?Sized>(
    store: &S,
    root: &str,
    limits: GraphLimits,
) -> Result<ClosureReport, StoreError> {
    // Retain the original generic ObjectStore API and behavior. Callers that
    // require a hard inventory bound use PreparedClosureVerifier, whose store
    // must implement BoundedObjectStore.
    if parse_oid(root)? != ObjectKind::Commit {
        return verify_closure_with_tombstones(store, root, limits, &BTreeMap::new());
    }
    let tombstones = collect_tombstones(store)?;
    verify_closure_with_tombstones(store, root, limits, &tombstones)
}

fn verify_closure_with_tombstones<S: ObjectStore + ?Sized>(
    store: &S,
    root: &str,
    limits: GraphLimits,
    tombstones: &BTreeMap<String, String>,
) -> Result<ClosureReport, StoreError> {
    let root_kind = parse_oid(root)?;
    let mut report = ClosureReport {
        root: root.to_owned(),
        nodes: BTreeMap::new(),
        edges: Vec::new(),
        issues: Vec::new(),
        truncated: false,
    };
    if root_kind != ObjectKind::Commit {
        report.issues.push(ClosureIssue {
            oid: root.to_owned(),
            referenced_by: None,
            role: None,
            kind: ClosureIssueKind::ReferenceTypeMismatch {
                expected: ObjectKind::Commit,
                actual: root_kind,
            },
        });
        return Ok(report);
    }

    let mut stack = vec![WalkItem::Enter(PendingVisit {
        oid: root.to_owned(),
        kind: ObjectKind::Commit,
        depth: 0,
        referenced_by: None,
        role: None,
        record_constraint: None,
    })];
    let mut finished = HashSet::new();
    let mut active = HashSet::new();
    let mut active_path = Vec::new();
    let mut record_semantics = BTreeMap::new();
    let mut reference_bytes = 0_usize;

    'walk: while let Some(item) = stack.pop() {
        match item {
            WalkItem::Exit(oid) => {
                active.remove(&oid);
                if active_path.last() == Some(&oid) {
                    active_path.pop();
                }
                finished.insert(oid);
            }
            WalkItem::Enter(visit) => {
                if visit.depth > limits.max_depth {
                    report.truncated = true;
                    report.issues.push(issue_for_visit(
                        &visit,
                        ClosureIssueKind::ResourceLimit {
                            resource: "depth",
                            limit: limits.max_depth,
                        },
                    ));
                    continue;
                }
                if active.contains(&visit.oid) {
                    if matches!(
                        visit.role.as_ref(),
                        Some(ReferenceRole::RecordReference { .. })
                    ) {
                        // General Record evidence graphs may be cyclic. Only
                        // supersedes, Commit-parent and Tree cycles are errors.
                        record_constraint_issue(&visit, &record_semantics, &mut report.issues);
                        continue;
                    }
                    let start = active_path
                        .iter()
                        .position(|candidate| candidate == &visit.oid)
                        .unwrap_or(0);
                    let mut path = active_path[start..].to_vec();
                    path.push(visit.oid.clone());
                    report
                        .issues
                        .push(issue_for_visit(&visit, ClosureIssueKind::Cycle { path }));
                    continue;
                }
                if finished.contains(&visit.oid) {
                    record_constraint_issue(&visit, &record_semantics, &mut report.issues);
                    continue;
                }
                if report.nodes.len() >= limits.max_objects {
                    report.truncated = true;
                    report.issues.push(issue_for_visit(
                        &visit,
                        ClosureIssueKind::ResourceLimit {
                            resource: "objects",
                            limit: limits.max_objects,
                        },
                    ));
                    break 'walk;
                }

                active.insert(visit.oid.clone());
                active_path.push(visit.oid.clone());
                stack.push(WalkItem::Exit(visit.oid.clone()));

                let object = match store.get_verified(&visit.oid) {
                    Ok(Some(object)) => object,
                    Ok(None) => {
                        if let Some(tombstone_oid) = tombstones.get(&visit.oid) {
                            report.nodes.insert(
                                visit.oid.clone(),
                                ClosureNode {
                                    oid: visit.oid.clone(),
                                    depth: visit.depth,
                                    state: ClosureNodeState::Tombstoned {
                                        kind: visit.kind,
                                        tombstone_oid: tombstone_oid.clone(),
                                    },
                                },
                            );
                            // A Tombstone preserves the availability history
                            // of an object inside a readable Commit closure,
                            // but it cannot stand in for the root Commit
                            // bytes required to resolve a Ref head.
                            if visit.depth == 0 && visit.kind == ObjectKind::Commit {
                                report
                                    .issues
                                    .push(issue_for_visit(&visit, ClosureIssueKind::Missing));
                            }
                            continue;
                        }
                        report.nodes.insert(
                            visit.oid.clone(),
                            ClosureNode {
                                oid: visit.oid.clone(),
                                depth: visit.depth,
                                state: ClosureNodeState::Missing { kind: visit.kind },
                            },
                        );
                        report
                            .issues
                            .push(issue_for_visit(&visit, ClosureIssueKind::Missing));
                        continue;
                    }
                    Err(StoreError::CorruptObject { detail, .. }) => {
                        report.nodes.insert(
                            visit.oid.clone(),
                            ClosureNode {
                                oid: visit.oid.clone(),
                                depth: visit.depth,
                                state: ClosureNodeState::Corrupt {
                                    kind: visit.kind,
                                    detail: detail.clone(),
                                },
                            },
                        );
                        report.issues.push(issue_for_visit(
                            &visit,
                            ClosureIssueKind::Corrupt { detail },
                        ));
                        continue;
                    }
                    // Operational failures are not evidence that the object
                    // graph is invalid. Preserve their StoreError so callers
                    // can distinguish configured resource limits and storage
                    // failures from corrupt or missing repository data.
                    Err(error) => return Err(error),
                };

                if object.kind() != visit.kind {
                    report.nodes.insert(
                        visit.oid.clone(),
                        ClosureNode {
                            oid: visit.oid.clone(),
                            depth: visit.depth,
                            state: ClosureNodeState::Present {
                                kind: object.kind(),
                                byte_len: object.byte_len(),
                            },
                        },
                    );
                    report.issues.push(issue_for_visit(
                        &visit,
                        ClosureIssueKind::ReferenceTypeMismatch {
                            expected: visit.kind,
                            actual: object.kind(),
                        },
                    ));
                    continue;
                }

                if object.kind() == ObjectKind::Record
                    && let Some(semantics) = record_semantics_for(&object)
                {
                    record_semantics.insert(visit.oid.clone(), semantics);
                    record_constraint_issue(&visit, &record_semantics, &mut report.issues);
                }

                report.nodes.insert(
                    visit.oid.clone(),
                    ClosureNode {
                        oid: visit.oid.clone(),
                        depth: visit.depth,
                        state: ClosureNodeState::Present {
                            kind: object.kind(),
                            byte_len: object.byte_len(),
                        },
                    },
                );

                let remaining_edges = limits
                    .max_edges
                    .checked_sub(report.edges.len())
                    .expect("closure edge count never exceeds its configured limit");
                let remaining_reference_bytes = MAX_GRAPH_REFERENCE_BYTES
                    .checked_sub(reference_bytes)
                    .expect("reference metadata stays within its hard ceiling");
                let extraction = extract_references(
                    &object,
                    &mut report.issues,
                    remaining_edges,
                    remaining_reference_bytes,
                    MAX_GRAPH_REFERENCE_BYTES,
                );
                reference_bytes = reference_bytes
                    .checked_add(extraction.charged_bytes)
                    .expect("bounded reference metadata total cannot overflow");
                let mut child_visits = Vec::with_capacity(extraction.references.len());
                for reference in extraction.references {
                    // Charge every extracted reference to the edge budget,
                    // including malformed OID text. Otherwise invalid
                    // references could consume parsing work without advancing
                    // the operation-visible edge count used by multi-root
                    // callers.
                    report.edges.push(GraphEdge {
                        source: visit.oid.clone(),
                        target: reference.target.clone(),
                        expected_kind: reference.expected_kind,
                        role: reference.role.clone(),
                    });
                    let actual_kind = match parse_oid(&reference.target) {
                        Ok(kind) => kind,
                        Err(error) => {
                            report.issues.push(ClosureIssue {
                                oid: visit.oid.clone(),
                                referenced_by: None,
                                role: Some(reference.role),
                                kind: ClosureIssueKind::InvalidReference {
                                    value: reference.target,
                                    detail: error.to_string(),
                                },
                            });
                            continue;
                        }
                    };
                    if actual_kind != reference.expected_kind {
                        report.issues.push(ClosureIssue {
                            oid: reference.target,
                            referenced_by: Some(visit.oid.clone()),
                            role: Some(reference.role),
                            kind: ClosureIssueKind::ReferenceTypeMismatch {
                                expected: reference.expected_kind,
                                actual: actual_kind,
                            },
                        });
                        continue;
                    }
                    child_visits.push(PendingVisit {
                        oid: reference.target,
                        kind: actual_kind,
                        depth: visit.depth + 1,
                        referenced_by: Some(visit.oid.clone()),
                        role: Some(reference.role),
                        record_constraint: reference.record_constraint,
                    });
                }
                if let Some(exceeded) = extraction.exceeded {
                    let (resource, limit) = match exceeded {
                        ReferenceExtractionLimit::Edges => ("edges", limits.max_edges),
                        ReferenceExtractionLimit::ReferenceBytes => {
                            ("reference_bytes", MAX_GRAPH_REFERENCE_BYTES)
                        }
                    };
                    report.truncated = true;
                    report.issues.push(ClosureIssue {
                        oid: visit.oid.clone(),
                        referenced_by: None,
                        role: None,
                        kind: ClosureIssueKind::ResourceLimit { resource, limit },
                    });
                    break 'walk;
                }
                for child in child_visits.into_iter().rev() {
                    stack.push(WalkItem::Enter(child));
                }
            }
        }
    }
    Ok(report)
}

fn collect_tombstones<S: ObjectStore + ?Sized>(
    store: &S,
) -> Result<BTreeMap<String, String>, StoreError> {
    collect_tombstones_from_oids(store, store.list_oids()?, None)
}

fn collect_tombstones_bounded<S: BoundedObjectStore + ?Sized>(
    store: &S,
    limits: TombstoneScanLimits,
) -> Result<BTreeMap<String, String>, StoreError> {
    let oids = store.list_oids_by_kind_limited(ObjectKind::Record, limits.max_record_objects)?;
    if oids.len() > limits.max_record_objects {
        return Err(tombstone_scan_resource_limit(format!(
            "exceeds max_record_objects {}",
            limits.max_record_objects
        )));
    }
    collect_tombstones_from_oids(store, oids, Some(limits.max_record_bytes))
}

fn collect_tombstones_from_oids<S: ObjectStore + ?Sized>(
    store: &S,
    mut oids: Vec<String>,
    max_record_bytes: Option<u64>,
) -> Result<BTreeMap<String, String>, StoreError> {
    oids.sort_unstable();
    oids.dedup();
    let mut result = BTreeMap::new();
    let mut verified_record_bytes = 0_u64;
    for oid in oids {
        if parse_oid(&oid).is_ok_and(|kind| kind == ObjectKind::Record) {
            let Some(object) = store.get_verified(&oid)? else {
                continue;
            };
            if let Some(max_record_bytes) = max_record_bytes {
                verified_record_bytes = verified_record_bytes
                    .checked_add(object.byte_len())
                    .ok_or_else(|| {
                        tombstone_scan_resource_limit("verified Record bytes overflowed u64")
                    })?;
                if verified_record_bytes > max_record_bytes {
                    return Err(tombstone_scan_resource_limit(format!(
                        "exceeds max_record_bytes {max_record_bytes}"
                    )));
                }
            }
            let Some(value) = object.structured() else {
                continue;
            };
            if value.get("record_type").and_then(Value::as_str) != Some("tombstone") {
                continue;
            }
            let Some(target) = value
                .get("payload")
                .and_then(|payload| payload.get("target_ref"))
                .and_then(Value::as_str)
            else {
                continue;
            };
            if target == oid || parse_oid(target).is_err() {
                continue;
            }
            result
                .entry(target.to_owned())
                .and_modify(|existing: &mut String| {
                    if oid < *existing {
                        existing.clone_from(&oid);
                    }
                })
                .or_insert(oid);
        }
    }
    Ok(result)
}

fn validate_tombstone_scan_limits(limits: TombstoneScanLimits) -> Result<(), StoreError> {
    if limits.max_record_objects == 0 {
        return Err(tombstone_scan_resource_limit(
            "max_record_objects must be greater than zero",
        ));
    }
    if limits.max_record_bytes == 0 {
        return Err(tombstone_scan_resource_limit(
            "max_record_bytes must be greater than zero",
        ));
    }
    Ok(())
}

fn tombstone_scan_resource_limit(detail: impl Into<String>) -> StoreError {
    CoreError::new(
        ErrorCode::ResourceLimit,
        format!("Tombstone scan {}", detail.into()),
    )
    .into()
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FsckIssueKind {
    InvalidStorePath { path: String, detail: String },
    MissingScannedObject { oid: String },
    CorruptObject { oid: String, detail: String },
    ReadFailure { oid: String, detail: String },
    Closure(ClosureIssue),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FsckIssue {
    pub kind: FsckIssueKind,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FsckReport {
    pub objects_seen: usize,
    pub objects_verified: usize,
    pub closures: Vec<ClosureReport>,
    pub issues: Vec<FsckIssue>,
}

impl FsckReport {
    pub fn is_clean(&self) -> bool {
        self.issues.is_empty() && self.closures.iter().all(ClosureReport::is_complete)
    }
}

/// Verify every stored object and the required closure of the supplied Commit
/// roots. With an empty root list, every stored Commit is used as a graph root.
pub fn fsck(
    store: &FileObjectStore,
    roots: &[String],
    limits: GraphLimits,
) -> Result<FsckReport, StoreError> {
    let inventory = store.inventory()?;
    let mut issues = inventory
        .invalid_paths
        .into_iter()
        .map(|invalid| FsckIssue {
            kind: FsckIssueKind::InvalidStorePath {
                path: invalid.path.display().to_string(),
                detail: invalid.detail,
            },
        })
        .collect::<Vec<_>>();
    let objects_seen = inventory.oids.len();
    let mut objects_verified = 0;
    let mut stored_commits = Vec::new();

    for oid in &inventory.oids {
        if parse_oid(oid).is_ok_and(|kind| kind == ObjectKind::Commit) {
            stored_commits.push(oid.clone());
        }
        match store.get_verified(oid) {
            Ok(Some(_)) => objects_verified += 1,
            Ok(None) => issues.push(FsckIssue {
                kind: FsckIssueKind::MissingScannedObject { oid: oid.clone() },
            }),
            Err(StoreError::CorruptObject { detail, .. }) => issues.push(FsckIssue {
                kind: FsckIssueKind::CorruptObject {
                    oid: oid.clone(),
                    detail,
                },
            }),
            Err(error) => issues.push(FsckIssue {
                kind: FsckIssueKind::ReadFailure {
                    oid: oid.clone(),
                    detail: error.to_string(),
                },
            }),
        }
    }

    let graph_roots = if roots.is_empty() {
        stored_commits
    } else {
        roots.to_vec()
    };
    let mut closures = Vec::with_capacity(graph_roots.len());
    for root in graph_roots {
        let closure = verify_closure(store, &root, limits)?;
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

pub fn fsck_all(store: &FileObjectStore, limits: GraphLimits) -> Result<FsckReport, StoreError> {
    fsck(store, &[], limits)
}

#[derive(Clone, Debug)]
struct PendingReference {
    target: String,
    expected_kind: ObjectKind,
    role: ReferenceRole,
    record_constraint: Option<RecordConstraint>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ReferenceExtractionLimit {
    Edges,
    ReferenceBytes,
}

struct ReferenceExtraction {
    references: Vec<PendingReference>,
    charged_bytes: usize,
    exceeded: Option<ReferenceExtractionLimit>,
}

struct ReferenceCollector {
    references: Vec<PendingReference>,
    max_edges: usize,
    max_reference_bytes: usize,
    max_pointer_bytes: usize,
    charged_bytes: usize,
    exceeded: Option<ReferenceExtractionLimit>,
}

impl ReferenceCollector {
    fn new(max_edges: usize, max_reference_bytes: usize, max_pointer_bytes: usize) -> Self {
        Self {
            references: Vec::new(),
            max_edges,
            max_reference_bytes,
            max_pointer_bytes,
            charged_bytes: 0,
            exceeded: None,
        }
    }

    fn is_exceeded(&self) -> bool {
        self.exceeded.is_some()
    }

    fn exceed_reference_bytes(&mut self) {
        self.exceeded = Some(ReferenceExtractionLimit::ReferenceBytes);
    }

    fn try_push(
        &mut self,
        dynamic_target_bytes: usize,
        dynamic_role_bytes: usize,
        constraint_bytes: usize,
        build: impl FnOnce() -> PendingReference,
    ) -> bool {
        if self.is_exceeded() {
            return false;
        }
        if self.references.len() >= self.max_edges {
            self.exceeded = Some(ReferenceExtractionLimit::Edges);
            return false;
        }

        // An oversized malformed target may coexist in the parsed object,
        // PendingReference, retained GraphEdge, InvalidReference value, and
        // its escaped diagnostic. Valid/fixed-size OID text is bounded by
        // max_edges; attacker-sized text is conservatively charged eightfold.
        // A dynamic role may coexist in the PendingReference, GraphEdge, and a
        // ClosureIssue. Charge these retained copies before allocating the
        // first one.
        let Some(reference_bytes) = dynamic_target_bytes
            .checked_mul(8)
            .and_then(|bytes| {
                dynamic_role_bytes
                    .checked_mul(3)
                    .and_then(|role_bytes| bytes.checked_add(role_bytes))
            })
            .and_then(|bytes| bytes.checked_add(constraint_bytes))
        else {
            self.exceed_reference_bytes();
            return false;
        };
        let Some(next) = self.charged_bytes.checked_add(reference_bytes) else {
            self.exceed_reference_bytes();
            return false;
        };
        if next > self.max_reference_bytes {
            self.exceed_reference_bytes();
            return false;
        }

        self.references.push(build());
        self.charged_bytes = next;
        true
    }

    fn finish(self) -> ReferenceExtraction {
        ReferenceExtraction {
            references: self.references,
            charged_bytes: self.charged_bytes,
            exceeded: self.exceeded,
        }
    }
}

#[derive(Clone, Debug)]
struct PendingVisit {
    oid: String,
    kind: ObjectKind,
    depth: usize,
    referenced_by: Option<String>,
    role: Option<ReferenceRole>,
    record_constraint: Option<RecordConstraint>,
}

#[derive(Clone, Debug)]
enum RecordConstraint {
    RecordType(&'static str),
    AiActivity,
    Supersedes {
        record_type: String,
        entity_id: String,
    },
}

#[derive(Clone, Debug)]
struct RecordSemantics {
    record_type: Option<String>,
    entity_id: Option<String>,
    activity_kind: Option<String>,
}

#[derive(Clone, Debug)]
enum WalkItem {
    Enter(PendingVisit),
    Exit(String),
}

fn issue_for_visit(visit: &PendingVisit, kind: ClosureIssueKind) -> ClosureIssue {
    ClosureIssue {
        oid: visit.oid.clone(),
        referenced_by: visit.referenced_by.clone(),
        role: visit.role.clone(),
        kind,
    }
}

fn extract_references(
    object: &VerifiedObject,
    issues: &mut Vec<ClosureIssue>,
    max_edges: usize,
    max_reference_bytes: usize,
    max_pointer_bytes: usize,
) -> ReferenceExtraction {
    let mut collector = ReferenceCollector::new(max_edges, max_reference_bytes, max_pointer_bytes);
    match object.kind() {
        ObjectKind::Blob => {}
        ObjectKind::Record => extract_record_references(object, &mut collector, issues),
        ObjectKind::Commit => extract_commit_references(object, &mut collector, issues),
        ObjectKind::Tree => extract_tree_references(object, &mut collector, issues),
    }
    collector.finish()
}

fn extract_commit_references(
    object: &VerifiedObject,
    output: &mut ReferenceCollector,
    issues: &mut Vec<ClosureIssue>,
) {
    let Some(value) = object.structured() else {
        invalid_object(
            object.oid(),
            "verified Commit has no structured body",
            issues,
        );
        return;
    };
    let Some(fields) = value.as_object() else {
        invalid_object(object.oid(), "Commit body is not an object", issues);
        return;
    };
    append_array_references(
        object.oid(),
        fields,
        "parents",
        ObjectKind::Commit,
        |index| ReferenceRole::CommitParent { index },
        output,
        issues,
    );

    match object_field(fields, "snapshot").and_then(Value::as_str) {
        Some(snapshot) => {
            output.try_push(dynamic_target_bytes(snapshot), 0, 0, || PendingReference {
                target: snapshot.to_owned(),
                expected_kind: ObjectKind::Tree,
                role: ReferenceRole::CommitSnapshot,
                record_constraint: None,
            });
        }
        None => invalid_object(
            object.oid(),
            "Commit requires string field snapshot",
            issues,
        ),
    }
    append_array_references(
        object.oid(),
        fields,
        "transition_refs",
        ObjectKind::Record,
        |index| ReferenceRole::CommitTransition { index },
        output,
        issues,
    );
    append_array_references(
        object.oid(),
        fields,
        "bound_declaration_refs",
        ObjectKind::Record,
        |index| ReferenceRole::CommitBoundDeclaration { index },
        output,
        issues,
    );
}

fn extract_record_references(
    object: &VerifiedObject,
    output: &mut ReferenceCollector,
    issues: &mut Vec<ClosureIssue>,
) {
    let Some(value) = object.structured() else {
        invalid_object(
            object.oid(),
            "verified Record has no structured body",
            issues,
        );
        return;
    };
    let record_type = value
        .get("record_type")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let entity_id = value
        .get("entity_id")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let mut pointer = String::new();
    collect_record_oid_strings(
        value,
        &mut pointer,
        object.oid(),
        record_type,
        entity_id,
        output,
        issues,
    );
}

fn collect_record_oid_strings(
    value: &Value,
    pointer: &mut String,
    source_oid: &str,
    source_record_type: &str,
    source_entity_id: &str,
    output: &mut ReferenceCollector,
    issues: &mut Vec<ClosureIssue>,
) {
    if output.is_exceeded() {
        return;
    }
    match value {
        Value::String(target) => {
            let Ok(actual_kind) = parse_oid(target) else {
                return;
            };
            if source_record_type == "tombstone"
                && pointer == "/payload/target_ref"
                && target == source_oid
            {
                invalid_object(source_oid, "Tombstone may not target itself", issues);
            }
            let supersedes = pointer == "/supersedes";
            let record_constraint = if supersedes {
                Some(RecordConstraint::Supersedes {
                    record_type: source_record_type.to_owned(),
                    entity_id: source_entity_id.to_owned(),
                })
            } else {
                record_constraint_for(source_record_type, pointer)
            };
            let expected_kind = if record_constraint.is_some() {
                ObjectKind::Record
            } else {
                actual_kind
            };
            let dynamic_role_bytes = if supersedes { 0 } else { pointer.len() };
            let constraint_bytes = match &record_constraint {
                Some(RecordConstraint::Supersedes {
                    record_type,
                    entity_id,
                }) => record_type.len().saturating_add(entity_id.len()),
                Some(RecordConstraint::RecordType(_) | RecordConstraint::AiActivity) | None => 0,
            };
            output.try_push(0, dynamic_role_bytes, constraint_bytes, || {
                PendingReference {
                    target: target.to_owned(),
                    expected_kind,
                    role: if supersedes {
                        ReferenceRole::RecordSupersedes
                    } else {
                        ReferenceRole::RecordReference {
                            pointer: pointer.clone(),
                        }
                    },
                    record_constraint,
                }
            });
        }
        Value::Array(values) => {
            for (index, child) in values.iter().enumerate() {
                let previous_length = pointer.len();
                if !append_pointer_index(pointer, index, output.max_pointer_bytes) {
                    output.exceed_reference_bytes();
                    return;
                }
                collect_record_oid_strings(
                    child,
                    pointer,
                    source_oid,
                    source_record_type,
                    source_entity_id,
                    output,
                    issues,
                );
                pointer.truncate(previous_length);
                if output.is_exceeded() {
                    return;
                }
            }
        }
        Value::Object(entries) => {
            for (key, child) in entries {
                let previous_length = pointer.len();
                if !append_pointer_key(pointer, key, output.max_pointer_bytes) {
                    output.exceed_reference_bytes();
                    return;
                }
                collect_record_oid_strings(
                    child,
                    pointer,
                    source_oid,
                    source_record_type,
                    source_entity_id,
                    output,
                    issues,
                );
                pointer.truncate(previous_length);
                if output.is_exceeded() {
                    return;
                }
            }
        }
        Value::Null | Value::Bool(_) | Value::Integer(_) => {}
    }
}

fn record_constraint_for(record_type: &str, pointer: &str) -> Option<RecordConstraint> {
    let expected = match (record_type, pointer) {
        ("observation", "/payload/capture_profile_ref") => "capture_profile",
        ("activity", "/payload/ai_run/context_pack_ref") => "context_pack",
        ("activity", "/payload/ai_run/delegation_grant_ref") => "delegation_grant",
        ("context_pack", "/payload/policy_snapshot_ref") => "policy",
        ("context_pack", "/payload/delegation_grant_ref") => "delegation_grant",
        ("claim_reaction", "/payload/claim_ref") => "claim",
        _ => {
            return (record_type == "claim" && pointer == "/payload/ai_run_ref")
                .then_some(RecordConstraint::AiActivity);
        }
    };
    Some(RecordConstraint::RecordType(expected))
}

fn record_semantics_for(object: &VerifiedObject) -> Option<RecordSemantics> {
    let value = object.structured()?;
    Some(RecordSemantics {
        record_type: value
            .get("record_type")
            .and_then(Value::as_str)
            .map(str::to_owned),
        entity_id: value
            .get("entity_id")
            .and_then(Value::as_str)
            .map(str::to_owned),
        activity_kind: value
            .get("payload")
            .and_then(|payload| payload.get("activity_kind"))
            .and_then(Value::as_str)
            .map(str::to_owned),
    })
}

fn record_constraint_mismatch(
    semantics: &RecordSemantics,
    constraint: &RecordConstraint,
) -> Option<(String, String)> {
    let actual_record_type = semantics.record_type.as_deref().unwrap_or("<missing>");
    let actual_entity_id = semantics.entity_id.as_deref().unwrap_or("<missing>");
    match constraint {
        RecordConstraint::RecordType(expected) if actual_record_type != *expected => Some((
            format!("record_type={expected}"),
            format!("record_type={actual_record_type}"),
        )),
        RecordConstraint::AiActivity => {
            let activity_kind = semantics.activity_kind.as_deref().unwrap_or("<missing>");
            (actual_record_type != "activity" || activity_kind != "ai_run").then(|| {
                (
                    "record_type=activity, activity_kind=ai_run".to_owned(),
                    format!("record_type={actual_record_type}, activity_kind={activity_kind}"),
                )
            })
        }
        RecordConstraint::Supersedes {
            record_type,
            entity_id,
        } => (actual_record_type != record_type || actual_entity_id != entity_id).then(|| {
            (
                format!("record_type={record_type}, entity_id={entity_id}"),
                format!("record_type={actual_record_type}, entity_id={actual_entity_id}"),
            )
        }),
        _ => None,
    }
}

fn record_constraint_issue(
    visit: &PendingVisit,
    record_semantics: &BTreeMap<String, RecordSemantics>,
    issues: &mut Vec<ClosureIssue>,
) {
    let Some(constraint) = visit.record_constraint.as_ref() else {
        return;
    };
    let Some(semantics) = record_semantics.get(&visit.oid) else {
        return;
    };
    let Some((expected, actual)) = record_constraint_mismatch(semantics, constraint) else {
        return;
    };
    issues.push(issue_for_visit(
        visit,
        ClosureIssueKind::ReferenceSemanticMismatch { expected, actual },
    ));
}

fn append_array_references(
    source_oid: &str,
    fields: &[(String, Value)],
    field_name: &str,
    expected_kind: ObjectKind,
    role: impl Fn(usize) -> ReferenceRole,
    output: &mut ReferenceCollector,
    issues: &mut Vec<ClosureIssue>,
) {
    let Some(values) = object_field(fields, field_name).and_then(Value::as_array) else {
        invalid_object(
            source_oid,
            format!("Commit requires array field {field_name}"),
            issues,
        );
        return;
    };
    for (index, value) in values.iter().enumerate() {
        if output.is_exceeded() {
            return;
        }
        let Some(target) = value.as_str() else {
            invalid_object(
                source_oid,
                format!("Commit {field_name}[{index}] must be a string OID"),
                issues,
            );
            continue;
        };
        output.try_push(dynamic_target_bytes(target), 0, 0, || PendingReference {
            target: target.to_owned(),
            expected_kind,
            role: role(index),
            record_constraint: None,
        });
    }
}

fn extract_tree_references(
    object: &VerifiedObject,
    output: &mut ReferenceCollector,
    issues: &mut Vec<ClosureIssue>,
) {
    let Some(value) = object.structured() else {
        invalid_object(object.oid(), "verified Tree has no structured body", issues);
        return;
    };
    let Some(entries) = value.get("entries").and_then(Value::as_object) else {
        invalid_object(object.oid(), "Tree requires object field entries", issues);
        return;
    };
    for (segment, entry) in entries {
        if output.is_exceeded() {
            return;
        }
        if !valid_path_segment(segment) {
            invalid_object(
                object.oid(),
                format!("Tree entry segment {segment:?} is not a safe single path segment"),
                issues,
            );
            continue;
        }
        let Some(entry_fields) = entry.as_object() else {
            invalid_object(
                object.oid(),
                format!("Tree entry {segment:?} is not an object"),
                issues,
            );
            continue;
        };
        let expected_kind = match object_field(entry_fields, "entry_kind").and_then(Value::as_str) {
            Some("blob") => ObjectKind::Blob,
            Some("record") => ObjectKind::Record,
            Some("tree") => ObjectKind::Tree,
            Some("commit") => ObjectKind::Commit,
            Some(other) => {
                invalid_object(
                    object.oid(),
                    format!("Tree entry {segment:?} has unsupported entry_kind {other:?}"),
                    issues,
                );
                continue;
            }
            None => {
                invalid_object(
                    object.oid(),
                    format!("Tree entry {segment:?} requires string entry_kind"),
                    issues,
                );
                continue;
            }
        };
        let Some(target) = object_field(entry_fields, "oid").and_then(Value::as_str) else {
            invalid_object(
                object.oid(),
                format!("Tree entry {segment:?} requires string oid"),
                issues,
            );
            continue;
        };
        output.try_push(dynamic_target_bytes(target), segment.len(), 0, || {
            PendingReference {
                target: target.to_owned(),
                expected_kind,
                role: ReferenceRole::TreeEntry {
                    segment: segment.to_owned(),
                },
                record_constraint: None,
            }
        });
    }
}

fn dynamic_target_bytes(target: &str) -> usize {
    // Avoid parse_oid here because its intentionally detailed diagnostic
    // includes the whole malformed input and would allocate before this
    // resource check. This is the same fixed lexical profile without error
    // construction.
    let Some((family, digest)) = target.split_once(":sg-oid-v1:sha256:") else {
        return target.len();
    };
    let valid = matches!(family, "blob" | "record" | "tree" | "commit")
        && digest.len() == 64
        && digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'));
    if valid { 0 } else { target.len() }
}

fn object_field<'a>(fields: &'a [(String, Value)], name: &str) -> Option<&'a Value> {
    fields
        .iter()
        .find_map(|(candidate, value)| (candidate == name).then_some(value))
}

fn append_pointer_index(pointer: &mut String, index: usize, limit: usize) -> bool {
    let index = index.to_string();
    let Some(next_length) = pointer
        .len()
        .checked_add(1)
        .and_then(|length| length.checked_add(index.len()))
    else {
        return false;
    };
    if next_length > limit {
        return false;
    }
    pointer.push('/');
    pointer.push_str(&index);
    true
}

fn append_pointer_key(pointer: &mut String, key: &str, limit: usize) -> bool {
    if pointer
        .len()
        .checked_add(1)
        .is_none_or(|length| length > limit)
    {
        return false;
    }
    pointer.push('/');
    for character in key.chars() {
        let encoded = match character {
            '~' => Some("~0"),
            '/' => Some("~1"),
            _ => None,
        };
        let encoded_length = encoded.map_or_else(|| character.len_utf8(), str::len);
        if pointer
            .len()
            .checked_add(encoded_length)
            .is_none_or(|length| length > limit)
        {
            return false;
        }
        if let Some(encoded) = encoded {
            pointer.push_str(encoded);
        } else {
            pointer.push(character);
        }
    }
    true
}

fn invalid_object(oid: &str, detail: impl Into<String>, issues: &mut Vec<ClosureIssue>) {
    issues.push(ClosureIssue {
        oid: oid.to_owned(),
        referenced_by: None,
        role: None,
        kind: ClosureIssueKind::InvalidObject {
            detail: detail.into(),
        },
    });
}

fn valid_path_segment(segment: &str) -> bool {
    !segment.is_empty()
        && segment != "."
        && segment != ".."
        && !segment.contains('/')
        && !segment.contains('\0')
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::VerifiedObject;
    use std::cell::Cell;
    use std::collections::HashMap;

    struct MockStore {
        objects: HashMap<String, VerifiedObject>,
    }

    impl ObjectStore for MockStore {
        fn get_verified(&self, oid: &str) -> Result<Option<VerifiedObject>, StoreError> {
            Ok(self.objects.get(oid).cloned())
        }

        fn list_oids(&self) -> Result<Vec<String>, StoreError> {
            Ok(self.objects.keys().cloned().collect())
        }
    }

    impl BoundedObjectStore for MockStore {
        fn list_oids_by_kind_limited(
            &self,
            kind: ObjectKind,
            max_objects: usize,
        ) -> Result<Vec<String>, StoreError> {
            let mut oids = self
                .objects
                .keys()
                .filter(|oid| parse_oid(oid).is_ok_and(|actual| actual == kind))
                .cloned()
                .collect::<Vec<_>>();
            oids.sort_unstable();
            if oids.len() > max_objects {
                return Err(tombstone_scan_resource_limit(format!(
                    "exceeds max_record_objects {max_objects}"
                )));
            }
            Ok(oids)
        }
    }

    struct CountingBoundedStore {
        inner: MockStore,
        inventory_calls: Cell<usize>,
        get_calls: Cell<usize>,
    }

    impl ObjectStore for CountingBoundedStore {
        fn get_verified(&self, oid: &str) -> Result<Option<VerifiedObject>, StoreError> {
            self.get_calls.set(self.get_calls.get().saturating_add(1));
            self.inner.get_verified(oid)
        }

        fn list_oids(&self) -> Result<Vec<String>, StoreError> {
            self.inner.list_oids()
        }
    }

    impl BoundedObjectStore for CountingBoundedStore {
        fn list_oids_by_kind_limited(
            &self,
            kind: ObjectKind,
            max_objects: usize,
        ) -> Result<Vec<String>, StoreError> {
            self.inventory_calls
                .set(self.inventory_calls.get().saturating_add(1));
            self.inner.list_oids_by_kind_limited(kind, max_objects)
        }
    }

    fn commit_value(parent: &str, snapshot: &str) -> Value {
        Value::Object(vec![
            ("object_type".to_owned(), Value::String("commit".to_owned())),
            (
                "parents".to_owned(),
                Value::Array(vec![Value::String(parent.to_owned())]),
            ),
            ("snapshot".to_owned(), Value::String(snapshot.to_owned())),
            ("transition_refs".to_owned(), Value::Array(Vec::new())),
            (
                "bound_declaration_refs".to_owned(),
                Value::Array(Vec::new()),
            ),
        ])
    }

    fn root_commit_value(snapshot: &str) -> Value {
        Value::Object(vec![
            ("object_type".to_owned(), Value::String("commit".to_owned())),
            ("parents".to_owned(), Value::Array(Vec::new())),
            ("snapshot".to_owned(), Value::String(snapshot.to_owned())),
            ("transition_refs".to_owned(), Value::Array(Vec::new())),
            (
                "bound_declaration_refs".to_owned(),
                Value::Array(Vec::new()),
            ),
        ])
    }

    fn one_record_tree(record: &str) -> Value {
        Value::Object(vec![
            ("object_type".to_owned(), Value::String("tree".to_owned())),
            (
                "entries".to_owned(),
                Value::Object(vec![(
                    "record.json".to_owned(),
                    Value::Object(vec![
                        ("entry_kind".to_owned(), Value::String("record".to_owned())),
                        ("oid".to_owned(), Value::String(record.to_owned())),
                    ]),
                )]),
            ),
        ])
    }

    fn record_value(
        record_type: &str,
        entity_id: &str,
        supersedes: Option<&str>,
        payload: Value,
    ) -> Value {
        let mut fields = vec![
            ("object_type".to_owned(), Value::String("record".to_owned())),
            (
                "record_type".to_owned(),
                Value::String(record_type.to_owned()),
            ),
            ("entity_id".to_owned(), Value::String(entity_id.to_owned())),
            ("payload".to_owned(), payload),
        ];
        if let Some(target) = supersedes {
            fields.push(("supersedes".to_owned(), Value::String(target.to_owned())));
        }
        Value::Object(fields)
    }

    #[test]
    fn iterative_walk_reports_a_cycle_from_an_adversarial_store() {
        let a = format!("commit:sg-oid-v1:sha256:{}", "a".repeat(64));
        let b = format!("commit:sg-oid-v1:sha256:{}", "b".repeat(64));
        let missing_tree = format!("tree:sg-oid-v1:sha256:{}", "c".repeat(64));
        let mut objects = HashMap::new();
        objects.insert(
            a.clone(),
            VerifiedObject::test_structured(
                &a,
                ObjectKind::Commit,
                commit_value(&b, &missing_tree),
            ),
        );
        objects.insert(
            b.clone(),
            VerifiedObject::test_structured(
                &b,
                ObjectKind::Commit,
                commit_value(&a, &missing_tree),
            ),
        );
        let report = verify_closure(
            &MockStore { objects },
            &a,
            GraphLimits {
                max_objects: 10,
                max_edges: 20,
                max_depth: 10,
            },
        )
        .unwrap();
        assert!(report.issues.iter().any(|issue| {
            matches!(
                &issue.kind,
                ClosureIssueKind::Cycle { path } if path == &vec![a.clone(), b.clone(), a.clone()]
            )
        }));
    }

    #[test]
    fn malformed_oid_references_are_charged_to_the_edge_budget() {
        let commit = format!("commit:sg-oid-v1:sha256:{}", "d".repeat(64));
        let malformed_snapshot = "not-an-oid";
        let mut objects = HashMap::new();
        objects.insert(
            commit.clone(),
            VerifiedObject::test_structured(
                &commit,
                ObjectKind::Commit,
                root_commit_value(malformed_snapshot),
            ),
        );
        let store = MockStore { objects };

        let report = verify_closure(
            &store,
            &commit,
            GraphLimits {
                max_objects: 2,
                max_edges: 1,
                max_depth: 1,
            },
        )
        .unwrap();
        assert_eq!(report.edges.len(), 1);
        assert_eq!(report.edges[0].target, malformed_snapshot);
        assert!(
            report
                .issues
                .iter()
                .any(|issue| { matches!(issue.kind, ClosureIssueKind::InvalidReference { .. }) })
        );

        let exhausted = verify_closure(
            &store,
            &commit,
            GraphLimits {
                max_objects: 2,
                max_edges: 0,
                max_depth: 1,
            },
        )
        .unwrap();
        assert!(exhausted.truncated);
        assert!(exhausted.issues.iter().any(|issue| {
            matches!(
                issue.kind,
                ClosureIssueKind::ResourceLimit {
                    resource: "edges",
                    limit: 0
                }
            )
        }));

        let oversized_target = "x".repeat(129);
        let oversized = VerifiedObject::test_structured(
            &commit,
            ObjectKind::Commit,
            root_commit_value(&oversized_target),
        );
        let mut issues = Vec::new();
        let extraction = extract_references(
            &oversized,
            &mut issues,
            1,
            oversized_target.len() * 8 - 1,
            MAX_GRAPH_REFERENCE_BYTES,
        );
        assert!(extraction.references.is_empty());
        assert_eq!(
            extraction.exceeded,
            Some(ReferenceExtractionLimit::ReferenceBytes)
        );
    }

    #[test]
    fn record_internal_references_are_traversed_and_semantically_typed() {
        let commit = format!("commit:sg-oid-v1:sha256:{}", "1".repeat(64));
        let tree = format!("tree:sg-oid-v1:sha256:{}", "2".repeat(64));
        let context = format!("record:sg-oid-v1:sha256:{}", "3".repeat(64));
        let actor = format!("record:sg-oid-v1:sha256:{}", "4".repeat(64));
        let mut objects = HashMap::new();
        objects.insert(
            commit.clone(),
            VerifiedObject::test_structured(&commit, ObjectKind::Commit, root_commit_value(&tree)),
        );
        objects.insert(
            tree.clone(),
            VerifiedObject::test_structured(&tree, ObjectKind::Tree, one_record_tree(&context)),
        );
        objects.insert(
            context.clone(),
            VerifiedObject::test_structured(
                &context,
                ObjectKind::Record,
                record_value(
                    "context_pack",
                    "urn:uuid:00000000-0000-4000-8000-000000000001",
                    None,
                    Value::Object(vec![(
                        "policy_snapshot_ref".to_owned(),
                        Value::String(actor.clone()),
                    )]),
                ),
            ),
        );
        objects.insert(
            actor.clone(),
            VerifiedObject::test_structured(
                &actor,
                ObjectKind::Record,
                record_value(
                    "actor",
                    "urn:uuid:00000000-0000-4000-8000-000000000002",
                    None,
                    Value::Object(Vec::new()),
                ),
            ),
        );

        let report =
            verify_closure(&MockStore { objects }, &commit, GraphLimits::default()).unwrap();
        assert!(report.nodes.contains_key(&actor));
        assert!(report.issues.iter().any(|issue| {
            matches!(
                &issue.kind,
                ClosureIssueKind::ReferenceSemanticMismatch { expected, actual }
                    if expected == "record_type=policy" && actual == "record_type=actor"
            )
        }));
    }

    #[test]
    fn shared_record_target_is_checked_against_every_incoming_constraint() {
        let commit = format!("commit:sg-oid-v1:sha256:{}", "9".repeat(64));
        let tree = format!("tree:sg-oid-v1:sha256:{}", "a".repeat(64));
        let context = format!("record:sg-oid-v1:sha256:{}", "b".repeat(64));
        let delegation_grant = format!("record:sg-oid-v1:sha256:{}", "c".repeat(64));
        let mut objects = HashMap::new();
        objects.insert(
            commit.clone(),
            VerifiedObject::test_structured(&commit, ObjectKind::Commit, root_commit_value(&tree)),
        );
        objects.insert(
            tree.clone(),
            VerifiedObject::test_structured(&tree, ObjectKind::Tree, one_record_tree(&context)),
        );
        objects.insert(
            context.clone(),
            VerifiedObject::test_structured(
                &context,
                ObjectKind::Record,
                record_value(
                    "context_pack",
                    "urn:uuid:00000000-0000-4000-8000-000000000020",
                    None,
                    Value::Object(vec![
                        (
                            "delegation_grant_ref".to_owned(),
                            Value::String(delegation_grant.clone()),
                        ),
                        (
                            "policy_snapshot_ref".to_owned(),
                            Value::String(delegation_grant.clone()),
                        ),
                    ]),
                ),
            ),
        );
        objects.insert(
            delegation_grant.clone(),
            VerifiedObject::test_structured(
                &delegation_grant,
                ObjectKind::Record,
                record_value(
                    "delegation_grant",
                    "urn:uuid:00000000-0000-4000-8000-000000000021",
                    None,
                    Value::Object(Vec::new()),
                ),
            ),
        );

        let report =
            verify_closure(&MockStore { objects }, &commit, GraphLimits::default()).unwrap();
        let mismatches = report
            .issues
            .iter()
            .filter(|issue| {
                issue.oid == delegation_grant
                    && issue.referenced_by.as_deref() == Some(context.as_str())
                    && matches!(
                        issue.role.as_ref(),
                        Some(ReferenceRole::RecordReference { pointer })
                            if pointer == "/payload/policy_snapshot_ref"
                    )
                    && matches!(
                        &issue.kind,
                        ClosureIssueKind::ReferenceSemanticMismatch { expected, actual }
                            if expected == "record_type=policy"
                                && actual == "record_type=delegation_grant"
                    )
            })
            .count();
        assert_eq!(mismatches, 1);
    }

    #[test]
    fn allowed_record_cycle_still_checks_constraint_on_the_active_target() {
        let commit = format!("commit:sg-oid-v1:sha256:{}", "d".repeat(64));
        let tree = format!("tree:sg-oid-v1:sha256:{}", "e".repeat(64));
        let delegation_grant = format!("record:sg-oid-v1:sha256:{}", "f".repeat(64));
        let context = format!("record:sg-oid-v1:sha256:{}", "0".repeat(64));
        let mut objects = HashMap::new();
        objects.insert(
            commit.clone(),
            VerifiedObject::test_structured(&commit, ObjectKind::Commit, root_commit_value(&tree)),
        );
        objects.insert(
            tree.clone(),
            VerifiedObject::test_structured(
                &tree,
                ObjectKind::Tree,
                one_record_tree(&delegation_grant),
            ),
        );
        objects.insert(
            delegation_grant.clone(),
            VerifiedObject::test_structured(
                &delegation_grant,
                ObjectKind::Record,
                record_value(
                    "delegation_grant",
                    "urn:uuid:00000000-0000-4000-8000-000000000022",
                    None,
                    Value::Object(vec![(
                        "next_ref".to_owned(),
                        Value::String(context.clone()),
                    )]),
                ),
            ),
        );
        objects.insert(
            context.clone(),
            VerifiedObject::test_structured(
                &context,
                ObjectKind::Record,
                record_value(
                    "context_pack",
                    "urn:uuid:00000000-0000-4000-8000-000000000023",
                    None,
                    Value::Object(vec![(
                        "policy_snapshot_ref".to_owned(),
                        Value::String(delegation_grant.clone()),
                    )]),
                ),
            ),
        );

        let report =
            verify_closure(&MockStore { objects }, &commit, GraphLimits::default()).unwrap();
        assert!(report.issues.iter().any(|issue| {
            issue.oid == delegation_grant
                && issue.referenced_by.as_deref() == Some(context.as_str())
                && matches!(
                    issue.role.as_ref(),
                    Some(ReferenceRole::RecordReference { pointer })
                        if pointer == "/payload/policy_snapshot_ref"
                )
                && matches!(
                    &issue.kind,
                    ClosureIssueKind::ReferenceSemanticMismatch { expected, actual }
                        if expected == "record_type=policy"
                            && actual == "record_type=delegation_grant"
                )
        }));
        assert!(
            !report
                .issues
                .iter()
                .any(|issue| matches!(issue.kind, ClosureIssueKind::Cycle { .. }))
        );
    }

    #[test]
    fn supersedes_cycles_are_rejected_even_though_general_record_cycles_are_allowed() {
        let commit = format!("commit:sg-oid-v1:sha256:{}", "5".repeat(64));
        let tree = format!("tree:sg-oid-v1:sha256:{}", "6".repeat(64));
        let first = format!("record:sg-oid-v1:sha256:{}", "7".repeat(64));
        let second = format!("record:sg-oid-v1:sha256:{}", "8".repeat(64));
        let entity = "urn:uuid:00000000-0000-4000-8000-000000000003";
        let mut objects = HashMap::new();
        objects.insert(
            commit.clone(),
            VerifiedObject::test_structured(&commit, ObjectKind::Commit, root_commit_value(&tree)),
        );
        objects.insert(
            tree.clone(),
            VerifiedObject::test_structured(&tree, ObjectKind::Tree, one_record_tree(&first)),
        );
        objects.insert(
            first.clone(),
            VerifiedObject::test_structured(
                &first,
                ObjectKind::Record,
                record_value("claim", entity, Some(&second), Value::Object(Vec::new())),
            ),
        );
        objects.insert(
            second.clone(),
            VerifiedObject::test_structured(
                &second,
                ObjectKind::Record,
                record_value("claim", entity, Some(&first), Value::Object(Vec::new())),
            ),
        );

        let report =
            verify_closure(&MockStore { objects }, &commit, GraphLimits::default()).unwrap();
        assert!(
            report
                .issues
                .iter()
                .any(|issue| matches!(issue.kind, ClosureIssueKind::Cycle { .. }))
        );
    }

    #[test]
    fn prepared_verifier_bounds_record_count_and_canonical_bytes_inclusively() {
        let first = format!("record:sg-oid-v1:sha256:{}", "1".repeat(64));
        let second = format!("record:sg-oid-v1:sha256:{}", "2".repeat(64));
        let mut objects = HashMap::new();
        objects.insert(
            first.clone(),
            VerifiedObject::test_structured_with_len(
                &first,
                ObjectKind::Record,
                record_value(
                    "claim",
                    "urn:uuid:00000000-0000-4000-8000-000000000010",
                    None,
                    Value::Object(Vec::new()),
                ),
                4,
            ),
        );
        objects.insert(
            second.clone(),
            VerifiedObject::test_structured_with_len(
                &second,
                ObjectKind::Record,
                record_value(
                    "claim",
                    "urn:uuid:00000000-0000-4000-8000-000000000011",
                    None,
                    Value::Object(Vec::new()),
                ),
                6,
            ),
        );
        let store = MockStore { objects };

        PreparedClosureVerifier::new(
            &store,
            GraphLimits::default(),
            TombstoneScanLimits {
                max_record_objects: 2,
                max_record_bytes: 10,
            },
        )
        .expect("inclusive object and byte limits accept the exact boundary");

        let object_error = PreparedClosureVerifier::new(
            &store,
            GraphLimits::default(),
            TombstoneScanLimits {
                max_record_objects: 1,
                max_record_bytes: 10,
            },
        )
        .err()
        .expect("one extra Record must reject the complete catalog");
        assert_eq!(object_error.code(), Some(ErrorCode::ResourceLimit));
        assert!(object_error.to_string().contains("max_record_objects 1"));

        let byte_error = PreparedClosureVerifier::new(
            &store,
            GraphLimits::default(),
            TombstoneScanLimits {
                max_record_objects: 2,
                max_record_bytes: 9,
            },
        )
        .err()
        .expect("one byte over the cumulative limit must fail closed");
        assert_eq!(byte_error.code(), Some(ErrorCode::ResourceLimit));
        assert!(byte_error.to_string().contains("max_record_bytes 9"));
    }

    #[test]
    fn prepared_verifier_scans_once_and_selects_the_lowest_resolver_oid() {
        let target = format!("blob:sg-oid-v1:sha256:{}", "a".repeat(64));
        let lower = format!("record:sg-oid-v1:sha256:{}", "3".repeat(64));
        let higher = format!("record:sg-oid-v1:sha256:{}", "4".repeat(64));
        let tombstone = |entity: &str| {
            record_value(
                "tombstone",
                entity,
                None,
                Value::Object(vec![(
                    "target_ref".to_owned(),
                    Value::String(target.clone()),
                )]),
            )
        };
        let mut objects = HashMap::new();
        objects.insert(
            higher.clone(),
            VerifiedObject::test_structured_with_len(
                &higher,
                ObjectKind::Record,
                tombstone("urn:uuid:00000000-0000-4000-8000-000000000012"),
                7,
            ),
        );
        objects.insert(
            lower.clone(),
            VerifiedObject::test_structured_with_len(
                &lower,
                ObjectKind::Record,
                tombstone("urn:uuid:00000000-0000-4000-8000-000000000013"),
                8,
            ),
        );
        let store = CountingBoundedStore {
            inner: MockStore { objects },
            inventory_calls: Cell::new(0),
            get_calls: Cell::new(0),
        };
        let mut verifier = PreparedClosureVerifier::new(
            &store,
            GraphLimits::default(),
            TombstoneScanLimits {
                max_record_objects: 2,
                max_record_bytes: 15,
            },
        )
        .unwrap();
        assert_eq!(verifier.tombstones.get(&target), Some(&lower));

        let first_root = format!("commit:sg-oid-v1:sha256:{}", "5".repeat(64));
        let second_root = format!("commit:sg-oid-v1:sha256:{}", "6".repeat(64));
        assert!(!verifier.verify(&first_root).unwrap().is_complete());
        assert!(!verifier.verify(&first_root).unwrap().is_complete());
        assert!(!verifier.verify(&second_root).unwrap().is_complete());
        assert_eq!(store.inventory_calls.get(), 1);
        assert_eq!(store.get_calls.get(), 4, "two catalog reads plus two roots");

        let uncached_root = format!("commit:sg-oid-v1:sha256:{}", "7".repeat(64));
        assert!(
            !verifier
                .verify_uncached(&uncached_root)
                .unwrap()
                .is_complete()
        );
        assert!(!verifier.report_cache.contains_key(&uncached_root));
        assert_eq!(store.inventory_calls.get(), 1);
        assert_eq!(
            store.get_calls.get(),
            5,
            "uncached verification reuses the catalog but retains no report"
        );
    }

    #[test]
    fn closure_traversal_preserves_operational_store_errors() {
        struct ResourceLimitedStore;

        impl ObjectStore for ResourceLimitedStore {
            fn get_verified(&self, _oid: &str) -> Result<Option<VerifiedObject>, StoreError> {
                Err(CoreError::new(ErrorCode::ResourceLimit, "configured read limit").into())
            }

            fn list_oids(&self) -> Result<Vec<String>, StoreError> {
                Ok(Vec::new())
            }
        }

        let root = format!("commit:sg-oid-v1:sha256:{}", "8".repeat(64));
        let error = verify_closure(&ResourceLimitedStore, &root, GraphLimits::default())
            .expect_err("resource failure must abort traversal");
        assert_eq!(error.code(), Some(ErrorCode::ResourceLimit));
    }

    #[test]
    fn record_reference_extraction_stops_before_repeating_long_pointers() {
        let source = format!("record:sg-oid-v1:sha256:{}", "8".repeat(64));
        let target = format!("record:sg-oid-v1:sha256:{}", "9".repeat(64));
        let long_key = format!("example.{}", "a".repeat(4_096));
        let record = VerifiedObject::test_structured(
            &source,
            ObjectKind::Record,
            Value::Object(vec![
                ("record_type".to_owned(), Value::String("claim".to_owned())),
                (
                    "entity_id".to_owned(),
                    Value::String("urn:uuid:00000000-0000-4000-8000-000000000001".to_owned()),
                ),
                (
                    "extensions".to_owned(),
                    Value::Object(vec![(
                        long_key.clone(),
                        Value::Array(vec![
                            Value::String(target.clone()),
                            Value::String(target.clone()),
                            Value::String(target),
                        ]),
                    )]),
                ),
            ]),
        );
        let first_pointer_bytes = "/extensions/".len() + long_key.len() + "/0".len();
        let one_reference_charge = first_pointer_bytes * 3;

        let mut issues = Vec::new();
        let byte_limited = extract_references(
            &record,
            &mut issues,
            10,
            one_reference_charge,
            MAX_GRAPH_REFERENCE_BYTES,
        );
        assert_eq!(byte_limited.references.len(), 1);
        assert_eq!(byte_limited.charged_bytes, one_reference_charge);
        assert_eq!(
            byte_limited.exceeded,
            Some(ReferenceExtractionLimit::ReferenceBytes)
        );
        assert!(issues.is_empty());

        let edge_limited = extract_references(
            &record,
            &mut Vec::new(),
            1,
            MAX_GRAPH_REFERENCE_BYTES,
            MAX_GRAPH_REFERENCE_BYTES,
        );
        assert_eq!(edge_limited.references.len(), 1);
        assert_eq!(edge_limited.exceeded, Some(ReferenceExtractionLimit::Edges));

        let pointer_limited = extract_references(&record, &mut Vec::new(), 10, usize::MAX, 128);
        assert!(pointer_limited.references.is_empty());
        assert_eq!(
            pointer_limited.exceeded,
            Some(ReferenceExtractionLimit::ReferenceBytes)
        );
    }
}
