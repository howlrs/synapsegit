//! CLI-owned, versioned JSON contract for `synapse creator-report --format json`.
//!
//! This module intentionally does not `derive(Serialize)` on any
//! `synapse_creator` type. Every field below is mapped explicitly from
//! [`CreatorReport`] so that internal Core/Creator shapes can change without
//! silently changing this wire contract, and so that this contract can add
//! fields without exposing anything the source types do not already carry.
//!
//! This is a **private, local** report. It may contain rationale text,
//! user-declared generation notes, decision pins, and internal identifiers
//! that are not safe to publish. It is a separate contract from the public
//! projection bundle (`synapse-present export ... --public`, `projection.json`);
//! nothing here is a proof of authorship, truth, rights, or physical change.
//!
//! # Versioning
//!
//! `"format": "synapsegit-cli-creator-report-v1"` identifies this contract.
//! While the format string stays at `-v1`, the meaning of every field below
//! is fixed: fields are never repurposed or removed. New fields may be added
//! in an additive, backward-compatible way (readers must ignore unknown
//! fields). Any incompatible change (removing/renaming a field, changing a
//! field's meaning or type) requires a new format identifier (e.g. `-v2`),
//! never a silent change under `-v1`.

use serde::Serialize;
use synapse_creator::{
    CreatorComparisonReport, CreatorDisposition, CreatorGenerationNote, CreatorPin, CreatorReport,
    CreatorReuseSourceBinding, CreatorSourceBinding, CreatorTimelineEntry,
};

/// Format identifier for this CLI-owned JSON contract. See module docs.
pub const CREATOR_REPORT_JSON_FORMAT: &str = "synapsegit-cli-creator-report-v1";

/// Machine-visible marker that this document is a private, local report and
/// not the public projection bundle.
const SCOPE_PRIVATE_LOCAL: &str = "private_local";

/// Distinguishes "never recorded" from "recorded but could not be loaded or
/// is in an unsupported shape" from "recorded and present below".
#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Availability {
    Absent,
    Present,
    Unavailable,
}

#[derive(Serialize)]
pub struct RefPointerJson {
    pub name: String,
    pub head: String,
}

#[derive(Serialize)]
pub struct BlobRefsJson {
    pub original: String,
    pub current: String,
    pub ai_output: String,
}

#[derive(Serialize)]
pub struct SourceBindingJson {
    pub availability: Availability,
    pub format: Option<String>,
    pub session: Option<String>,
    pub proposal_head: Option<String>,
    pub decision_head: Option<String>,
    pub disposition: Option<String>,
    pub original_blob_oid: Option<String>,
    pub current_blob_oid: Option<String>,
}

impl SourceBindingJson {
    fn from_binding(binding: Option<&CreatorSourceBinding>) -> Self {
        match binding {
            None => Self {
                availability: Availability::Absent,
                format: None,
                session: None,
                proposal_head: None,
                decision_head: None,
                disposition: None,
                original_blob_oid: None,
                current_blob_oid: None,
            },
            Some(binding) => Self {
                availability: Availability::Present,
                format: Some(binding.format.clone()),
                session: Some(binding.session.clone()),
                proposal_head: Some(binding.proposal_head.clone()),
                decision_head: Some(binding.decision_head.clone()),
                disposition: Some(binding.disposition.clone()),
                original_blob_oid: Some(binding.original_blob_oid.clone()),
                current_blob_oid: Some(binding.current_blob_oid.clone()),
            },
        }
    }
}

#[derive(Serialize)]
pub struct ReuseSourceBindingJson {
    pub availability: Availability,
    pub format: Option<String>,
    pub kind: Option<String>,
    pub session: Option<String>,
    pub proposal_head: Option<String>,
    pub decision_head: Option<String>,
    pub original_blob_oid: Option<String>,
    pub current_blob_oid: Option<String>,
    pub ai_output_blob_oid: Option<String>,
}

impl ReuseSourceBindingJson {
    fn from_binding(binding: Option<&CreatorReuseSourceBinding>) -> Self {
        match binding {
            None => Self {
                availability: Availability::Absent,
                format: None,
                kind: None,
                session: None,
                proposal_head: None,
                decision_head: None,
                original_blob_oid: None,
                current_blob_oid: None,
                ai_output_blob_oid: None,
            },
            Some(binding) => Self {
                availability: Availability::Present,
                format: Some(binding.format.clone()),
                kind: Some(binding.kind.clone()),
                session: Some(binding.session.clone()),
                proposal_head: Some(binding.proposal_head.clone()),
                decision_head: Some(binding.decision_head.clone()),
                original_blob_oid: Some(binding.original_blob_oid.clone()),
                current_blob_oid: Some(binding.current_blob_oid.clone()),
                ai_output_blob_oid: Some(binding.ai_output_blob_oid.clone()),
            },
        }
    }
}

#[derive(Serialize)]
pub struct GenerationNoteJson {
    pub availability: Availability,
    pub tool: Option<String>,
    pub model: Option<String>,
    pub prompt: Option<String>,
    pub intent: Option<String>,
}

impl GenerationNoteJson {
    fn from_note(note: Option<&CreatorGenerationNote>) -> Self {
        match note {
            None => Self {
                availability: Availability::Absent,
                tool: None,
                model: None,
                prompt: None,
                intent: None,
            },
            Some(note) => Self {
                availability: Availability::Present,
                tool: Some(note.tool.clone()),
                model: Some(note.model.clone()),
                prompt: Some(note.prompt.clone()),
                intent: Some(note.intent.clone()),
            },
        }
    }
}

#[derive(Serialize)]
pub struct DecisionPinJson {
    pub role: String,
    pub blob_oid: String,
    pub x: u32,
    pub y: u32,
    pub note: String,
}

impl From<&CreatorPin> for DecisionPinJson {
    fn from(pin: &CreatorPin) -> Self {
        Self {
            role: pin.role.as_str().to_string(),
            blob_oid: pin.blob_oid.clone(),
            x: pin.x,
            y: pin.y,
            note: pin.note.clone(),
        }
    }
}

#[derive(Serialize)]
pub struct DecisionPinsJson {
    pub availability: Availability,
    pub format: Option<String>,
    pub pins: Vec<DecisionPinJson>,
}

impl DecisionPinsJson {
    fn from_report(report: &CreatorReport) -> Self {
        if report.annotations_unavailable {
            return Self {
                availability: Availability::Unavailable,
                format: None,
                pins: Vec::new(),
            };
        }
        match &report.annotations {
            None => Self {
                availability: Availability::Absent,
                format: None,
                pins: Vec::new(),
            },
            Some(annotations) => Self {
                availability: Availability::Present,
                format: Some(annotations.format.clone()),
                pins: annotations.pins.iter().map(DecisionPinJson::from).collect(),
            },
        }
    }
}

/// `availability` here is whether a byte-identity comparison exists at all
/// for this session (legacy-shaped sessions may have none). It is distinct
/// from `status`, which is the analysis adapter's own outcome status.
#[derive(Serialize)]
pub struct ComparisonJson {
    pub availability: Availability,
    pub analysis_oid: Option<String>,
    pub tool_id: Option<String>,
    pub tool_actor_oid: Option<String>,
    pub adapter_id: Option<String>,
    pub adapter_version: Option<String>,
    pub implementation_oid: Option<String>,
    pub configuration_oid: Option<String>,
    pub status: Option<String>,
    pub comparability: Option<String>,
    pub outcome: Option<String>,
    pub reason_codes: Vec<String>,
    pub warnings: Vec<String>,
    pub base_observation_oid: Option<String>,
    pub target_observation_oid: Option<String>,
    pub base_media_oid: Option<String>,
    pub target_media_oid: Option<String>,
    pub replay_ready: Option<bool>,
    pub reachable_from: Vec<String>,
}

impl ComparisonJson {
    fn from_comparison(comparison: Option<&CreatorComparisonReport>) -> Self {
        match comparison {
            None => Self {
                availability: Availability::Unavailable,
                analysis_oid: None,
                tool_id: None,
                tool_actor_oid: None,
                adapter_id: None,
                adapter_version: None,
                implementation_oid: None,
                configuration_oid: None,
                status: None,
                comparability: None,
                outcome: None,
                reason_codes: Vec::new(),
                warnings: Vec::new(),
                base_observation_oid: None,
                target_observation_oid: None,
                base_media_oid: None,
                target_media_oid: None,
                replay_ready: None,
                reachable_from: Vec::new(),
            },
            Some(comparison) => Self {
                availability: Availability::Present,
                analysis_oid: Some(comparison.analysis_oid.clone()),
                tool_id: Some(comparison.tool_id.clone()),
                tool_actor_oid: Some(comparison.tool_actor_oid.clone()),
                adapter_id: Some(comparison.adapter_id.clone()),
                adapter_version: Some(comparison.adapter_version.clone()),
                implementation_oid: Some(comparison.implementation_oid.clone()),
                configuration_oid: Some(comparison.configuration_oid.clone()),
                status: Some(comparison.status.clone()),
                comparability: Some(comparison.comparability.clone()),
                outcome: Some(comparison.outcome.clone()),
                reason_codes: comparison.reason_codes.clone(),
                warnings: comparison.warnings.clone(),
                base_observation_oid: Some(comparison.base_observation_oid.clone()),
                target_observation_oid: Some(comparison.target_observation_oid.clone()),
                base_media_oid: Some(comparison.base_media_oid.clone()),
                target_media_oid: Some(comparison.target_media_oid.clone()),
                replay_ready: Some(comparison.replay_ready),
                reachable_from: comparison.reachable_from.clone(),
            },
        }
    }
}

#[derive(Serialize)]
pub struct TimelineEntryJson {
    pub oid: String,
    pub stage: String,
    pub kind: String,
    pub entity_id: String,
    pub ordering_time: String,
    pub time_basis: String,
    pub reachable_from: Vec<String>,
}

impl From<&CreatorTimelineEntry> for TimelineEntryJson {
    fn from(entry: &CreatorTimelineEntry) -> Self {
        Self {
            oid: entry.oid.clone(),
            stage: entry.stage.to_string(),
            kind: entry.kind.to_string(),
            entity_id: entry.entity_id.clone(),
            ordering_time: entry.ordering_time.clone(),
            time_basis: entry.time_basis.to_string(),
            reachable_from: entry.reachable_from.clone(),
        }
    }
}

#[derive(Serialize)]
pub struct FsckSummaryJson {
    /// This report is only ever produced after a successful, clean fsck-style
    /// verification of the reachable creator session graph; a verification
    /// failure returns an error before any document is built at all.
    pub clean: bool,
    pub objects: usize,
}

fn disposition_str(disposition: CreatorDisposition) -> String {
    disposition.as_cli_str().to_string()
}

/// The full CLI-owned, versioned JSON contract for one creator report.
///
/// Every field is mapped explicitly from [`CreatorReport`]; see module docs
/// for the versioning rule.
#[derive(Serialize)]
pub struct CreatorReportDocument {
    pub format: String,
    pub scope: String,
    pub ai_output_source: String,
    pub session: String,
    pub project_id: String,
    pub subject_id: String,
    pub agent_id: String,
    pub creator_id: String,
    pub selected_ai_output: bool,
    pub disposition: String,
    pub rationale: Option<String>,
    pub base_head: String,
    pub base_snapshot: String,
    pub proposal_snapshot: String,
    pub decision_snapshot: String,
    pub proposal_ref: RefPointerJson,
    pub decision_ref: RefPointerJson,
    pub blobs: BlobRefsJson,
    pub source: SourceBindingJson,
    pub reuse_source: ReuseSourceBindingJson,
    pub source_depth: usize,
    pub generation_note: GenerationNoteJson,
    pub decision_pins: DecisionPinsJson,
    pub comparison: ComparisonJson,
    pub fsck: FsckSummaryJson,
    pub timeline: Vec<TimelineEntryJson>,
}

impl CreatorReportDocument {
    pub fn from_report(report: &CreatorReport) -> Self {
        Self {
            format: CREATOR_REPORT_JSON_FORMAT.to_string(),
            scope: SCOPE_PRIVATE_LOCAL.to_string(),
            ai_output_source: "caller_supplied".to_string(),
            session: report.session.clone(),
            project_id: report.project_id.clone(),
            subject_id: report.subject_id.clone(),
            agent_id: report.agent_id.clone(),
            creator_id: report.creator_id.clone(),
            selected_ai_output: report.selected_ai_output,
            disposition: disposition_str(report.disposition),
            rationale: report.rationale.clone(),
            base_head: report.base_head.clone(),
            base_snapshot: report.base_snapshot.clone(),
            proposal_snapshot: report.proposal_snapshot.clone(),
            decision_snapshot: report.decision_snapshot.clone(),
            proposal_ref: RefPointerJson {
                name: report.proposal_ref.clone(),
                head: report.proposal_head.clone(),
            },
            decision_ref: RefPointerJson {
                name: report.decision_ref.clone(),
                head: report.decision_head.clone(),
            },
            blobs: BlobRefsJson {
                original: report.original_blob_oid.clone(),
                current: report.current_blob_oid.clone(),
                ai_output: report.ai_output_blob_oid.clone(),
            },
            source: SourceBindingJson::from_binding(report.source.as_ref()),
            reuse_source: ReuseSourceBindingJson::from_binding(report.reuse_source.as_ref()),
            source_depth: report.source_depth,
            generation_note: GenerationNoteJson::from_note(report.generation_note.as_ref()),
            decision_pins: DecisionPinsJson::from_report(report),
            comparison: ComparisonJson::from_comparison(report.comparison.as_ref()),
            fsck: FsckSummaryJson {
                clean: true,
                objects: report.fsck_objects,
            },
            timeline: report
                .timeline
                .iter()
                .map(TimelineEntryJson::from)
                .collect(),
        }
    }

    /// Serialize to a pretty-printed JSON document with a trailing newline.
    /// The whole document is built and serialized to a `String` before any
    /// output is written, so a serialization failure never produces partial
    /// JSON on stdout.
    pub fn to_pretty_string(&self) -> serde_json::Result<String> {
        let mut text = serde_json::to_string_pretty(self)?;
        text.push('\n');
        Ok(text)
    }
}
