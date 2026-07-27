//! Rebuildable SQLite query projection over SynapseGit's immutable CAS.
//!
//! This crate owns no authoritative state. Callers explicitly provide a
//! verified [`synapse_cas::FileObjectStore`] and one consistent
//! [`synapse_sqlite::RefSnapshot`] to [`SqliteProjectionStore::rebuild`].
//! Authorization, Ref updates, archives, and object identity must never
//! depend on this disposable index. [`RefScope`] is a query filter, not an
//! authorization boundary. An embedding service must authorize the caller
//! before exposing projection data or error distinctions such as indexed
//! versus indexed-but-not-reachable.
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

mod error;
mod mapping;
mod rebuild;
mod schema;
mod store;

#[cfg(test)]
mod tests;

pub use error::{ProjectionError, Result};
pub use rebuild::ProjectionLimits;
pub use store::{
    AdapterDeterminism, AnalysisAdapter, AnalysisInput, AnalysisLineage, AnalysisMask,
    AnalysisMaskRole, AnalysisObjectRef, AnalysisReplayReadiness, ClosureSummary,
    DependencyTargetKind, ObjectAvailability, ObservationDependency, ObservationDependencyKind,
    PROJECTION_SCHEMA_VERSION, ProjectedClosureIssue, ProjectedObject, ProjectionMetadata,
    RebuildReport, RefScope, SqliteProjectionStore, TimelineEntry, TimelineRecordKind,
    TimelineTimeBasis,
};
