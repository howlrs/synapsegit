//! Local creator-facing orchestration for one SynapseGit Stage 0 workflow.
//!
//! This crate turns image files and a human disposition into Core objects
//! without caller-authored JSON. It is intentionally a synchronous,
//! single-process Pilot boundary: images remain opaque Blobs, the AI output is
//! supplied by a trusted local integration, and publication still passes
//! through [`synapse_application`] AI and Human admission routes.

#![forbid(unsafe_code)]

mod error;
mod fsck;
mod io;
mod records;
mod report;
mod session;
mod time;
mod types;

#[cfg(test)]
mod tests;

pub use error::{CreatorError, Result};
pub use report::{
    PreparedCreatorReportReader, creator_report, creator_report_from_snapshot,
    discover_creator_sessions,
};
pub use session::{
    CREATOR_FSCK_MAX_CLOSURE_EDGES, CREATOR_FSCK_MAX_CLOSURE_NODES, CREATOR_FSCK_MAX_OBJECT_BYTES,
    CREATOR_FSCK_MAX_OBJECTS, CREATOR_FSCK_MAX_REF_ROOTS, CREATOR_RESERVED_PENDING_DECISIONS,
    PendingCreatorSession, begin_creator_session, decide_creator_session, run_creator_session,
};
pub use synapse_observation::{AnalysisComparability, AnalysisStatus, ByteIdentityOutcome};
pub use types::{
    CreatorBeginOptions, CreatorComparisonReport, CreatorDecisionOptions, CreatorDisposition,
    CreatorPendingDecisionState, CreatorPendingReceipt, CreatorReport, CreatorRunOptions,
    CreatorRunReceipt, CreatorSessionState, CreatorSessionSummary, CreatorSnapshotReport,
    CreatorTimelineEntry,
};
