//! Filesystem content-addressed storage and graph verification for SynapseGit.
//!
//! The filesystem layout is deliberately private. Callers address objects only
//! by fully validated Core v0.1 OIDs and use [`FileObjectStore::list_oids`] and
//! [`FileObjectStore::read_raw`] for archive/export operations.

#![forbid(unsafe_code)]

mod graph;
mod store;

pub use graph::{
    ClosureIssue, ClosureIssueKind, ClosureNode, ClosureNodeState, ClosureReport, FsckIssue,
    FsckIssueKind, FsckReport, GraphEdge, GraphLimits, ReferenceRole, fsck, fsck_all,
    verify_closure,
};
pub use store::{
    FileObjectStore, ObjectInfo, ObjectState, ObjectStore, PutDisposition, PutResult, StoreError,
    StoreLimits, VerifiedObject,
};
pub use synapse_canonical::ObjectKind;
