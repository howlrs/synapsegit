//! Local creator-facing orchestration for one SynapseGit Stage 0 workflow.
//!
//! This crate turns image files and a human disposition into Core objects
//! without caller-authored JSON. It is intentionally a synchronous,
//! single-process Pilot boundary: images remain opaque Blobs, the AI output is
//! supplied by a trusted local integration, and publication still passes
//! through [`synapse_application`] AI and Human admission routes.

#![forbid(unsafe_code)]

#[macro_use]
mod json_object;

mod annotations;
pub use annotations::{
    ANNOTATIONS_FORMAT, ANNOTATIONS_KEY, CreatorAnnotations, CreatorImageRole, CreatorPin,
    PIN_COORDINATE_MAX,
};
mod error;
mod fsck;
mod inbox;
pub use inbox::{
    IMPORT_INBOX_AI_OUTPUT_NAME, IMPORT_INBOX_CREATOR_MAX_BYTES, IMPORT_INBOX_CURRENT_NAME,
    IMPORT_INBOX_FILE_MAX_BYTES, IMPORT_INBOX_MANIFEST_MAX_BYTES, IMPORT_INBOX_MANIFEST_NAME,
    IMPORT_INBOX_MANIFEST_VERSION, IMPORT_INBOX_ORIGINAL_NAME, IMPORT_INBOX_SUBJECT_MAX_BYTES,
    ImportInboxCandidate, ImportInboxFile, ImportInboxManifest, ImportInboxManifestError,
    ImportInboxMetadata, ImportInboxReceipt, RetainedInboxCandidate, is_import_inbox_slug,
    put_import_inbox_candidate, retain_import_inbox_candidate,
};
mod io;
mod notes;
mod metadata;
pub use metadata::{image_metadata_warning, ImageMetadataCheck, ImageMetadataWarning};
mod overview;
pub use notes::{CreatorGenerationNote, GENERATION_NOTE_KEY};
pub use overview::{
    CreatorOverviewTimeBasis, CreatorSessionOverview, read_creator_session_overview,
};
mod records;
mod report;
mod session;
mod source;
mod time;
pub use source::{
    CREATOR_MAX_SOURCE_DEPTH, CREATOR_REUSE_SOURCE_FORMAT, CREATOR_REUSE_SOURCE_KEY,
    CREATOR_SOURCE_FORMAT, CREATOR_SOURCE_KEY, CreatorReuseSourceBinding, CreatorSourceBinding,
};
mod types;

#[cfg(test)]
mod tests;

pub use error::{CreatorError, Result};
pub use report::{
    CreatorReuseSourceDisplay, PreparedCreatorReportReader, creator_report,
    creator_report_from_snapshot, creator_reuse_source_context_from_binding,
    creator_reuse_source_display_from_snapshot, creator_reuse_source_from_snapshot,
    discover_creator_sessions,
};
pub use session::{
    CREATOR_FSCK_MAX_CLOSURE_EDGES, CREATOR_FSCK_MAX_CLOSURE_NODES, CREATOR_FSCK_MAX_OBJECT_BYTES,
    CREATOR_FSCK_MAX_OBJECTS, CREATOR_FSCK_MAX_REF_ROOTS, CREATOR_RESERVED_PENDING_DECISIONS,
    PendingCreatorSession, begin_creator_session, begin_creator_session_existing,
    begin_creator_session_with_note, begin_creator_session_with_note_existing,
    begin_creator_session_with_reuse_source, begin_creator_session_with_reuse_source_existing,
    begin_creator_session_with_source, begin_creator_session_with_source_existing,
    decide_creator_session, decide_creator_session_with_annotations, run_creator_session,
    run_creator_session_with_note,
};
pub use synapse_observation::{AnalysisComparability, AnalysisStatus, ByteIdentityOutcome};
pub use types::{
    CreatorBeginOptions, CreatorComparisonReport, CreatorDecisionOptions, CreatorDisposition,
    CreatorPendingDecisionState, CreatorPendingReceipt, CreatorRationaleSource, CreatorReport,
    CreatorRunOptions, CreatorRunReceipt, CreatorSessionState, CreatorSessionSummary,
    CreatorSnapshotReport, CreatorTimelineEntry,
};
