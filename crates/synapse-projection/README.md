# synapse-projection

`synapse-projection` is a disposable SQLite query index over a verified
SynapseGit `FileObjectStore` and one consistent `RefSnapshot`.

The projection is never an authorization source, object source of truth, Ref
store, archive input, or recovery prerequisite. `rebuild` starts only from the
caller-supplied Ref snapshot, follows current Commit closures in the immutable
CAS, and atomically replaces every derived row in one SQLite transaction.
`RefScope` only filters queries; it is not an ACL. Embedding services must
authorize callers before exposing projection rows or the distinction between
an AnalysisResult that is not indexed and one that is indexed but outside the
selected Ref scope, because that difference can reveal global-index existence.

The Stage 0 baseline indexes:

- current Ref heads, per-Ref reachability, objects, and typed graph edges;
- common Record identity fields and Subject/ObservationSeries links;
- Observation and Activity timeline time sources;
- Observation capture-profile, station, calibration, environment, and media
  dependencies;
- AnalysisResult adapter identity/configuration, ordered inputs, transforms,
  derived Blobs, typed masks, and prerequisite availability; and
- per-Ref closure completeness and missing-object diagnostics.

## Public boundary

- `SqliteProjectionStore::open` / `open_in_memory` open the disposable index.
- `rebuild(FileObjectStore, RefSnapshot, GraphLimits)` uses safe default
  Tombstone-scan limits and atomically replaces all derived state.
- `rebuild_with_limits(..., ProjectionLimits)` additionally lets the caller
  set inclusive Record-count and cumulative canonical-byte bounds.
- `metadata` and `get_object` expose rebuild identity and reachable object
  availability.
- `subject_timeline` queries Observation/Activity time with an explicit
  `RefScope` and optional ObservationSeries filter.
- `observation_dependencies` returns typed capture inputs and media roles.
- `analysis_lineage` returns one AnalysisResult's typed adapter/input/output
  lineage within an explicit `RefScope`, including every target's projected
  kind and availability and the selected Refs that reach the result.
- `closure_summaries` / `closure_issues` expose per-Ref completeness without
  pretending missing payloads are present.

`AnalysisReplayReadiness::Ready` means that ordered inputs, adapter
implementation/configuration objects, and transform Records are currently
present. It is an availability check only: it does not assert byte-identical or
otherwise exact replay, including for an adapter declaring `deterministic`.
Missing and Tombstoned prerequisites are reported separately (or together).
Derived output Blobs and masks describe the stored result and therefore do not
block an attempt to replay it.

Valid CAS objects unrelated to a reachable closure or its Tombstone resolution
are excluded from projection rows and the source fingerprint. Tombstone
availability is store-wide in Core v0.1, so
closure verification must scan Record OID paths for possible Tombstones; an
unreadable or digest-corrupt orphan Record therefore fails rebuild closed even
though it is not indexed. Missing and tombstoned objects remain distinguishable.
Corrupt, schema-invalid, type-invalid, cyclic, or resource-truncated reachable
input also aborts the rebuild and leaves the previous projection queryable.

For each non-empty rebuild, the projection creates one
`PreparedClosureVerifier`. Its `BoundedObjectStore` path enumerates only Record
OIDs, applies `TombstoneScanLimits` before retaining more than the configured
count, bounds cumulative verified Record bytes, and reuses one resolver catalog
across every Ref. Duplicate heads reuse one closure report. Exceeding either
limit returns `resource_limit` before SQLite replacement, so the old projection
remains queryable. Empty Ref snapshots perform no Tombstone scan.

The resolver catalog is per rebuild, not a persistent incremental index. Large
stores therefore still need scan-latency/I/O monitoring and a future cache if
the bounded linear Record scan is too expensive.

The compatibility `rebuild` wrapper defaults to 100,000 Record objects and
1 GiB of cumulative verified Record bytes. Services can lower or raise both
bounds explicitly with `ProjectionLimits`; raising them is an operational
capacity decision, not a change to canonical object identity.

## Consistency and operations

The caller must supply a Ref snapshot obtained at one RefStore consistency
point. Concurrent append-only CAS publication is harmless because unrelated
new objects are orphans relative to that snapshot. Cooperative GC, manual file
replacement, and removal of snapshot-reachable objects must not run during a
rebuild. If a previously verified source object disappears or changes while a
plan is being built, rebuild fails rather than silently downgrading it to
`missing`, and the previous projection remains active.

Operators should monitor rebuild failures and the stored source fingerprint.
The fingerprint covers the sorted Ref snapshot, projected availability states,
tombstone resolution, and graph edges; an unchanged successful source produces
the same fingerprint. It is diagnostic metadata, not an authorization token or
an integrity substitute for re-reading the CAS.

## Verification

```bash
cargo test -p synapse-projection --locked
```

The baseline is covered by unit and integration tests for rebuild atomicity,
resource bounds, scoped queries, missing/Tombstoned availability, and typed
Record projections.
