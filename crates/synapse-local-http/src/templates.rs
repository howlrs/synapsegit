use askama::Template;
use axum::http::header::CONTENT_TYPE;
use axum::response::{IntoResponse, Response};

use crate::i18n::Messages;
use crate::views::{
    ArchiveView, ImageView, ProjectCardView, RefView, ReflogView, SessionSummaryView, TimelineView,
};

pub(crate) const APP_CSS: &str = include_str!("../assets/app.css");
pub(crate) const APP_JS: &str = include_str!("../assets/app.js");

pub(crate) async fn css_asset() -> Response {
    ([(CONTENT_TYPE, "text/css; charset=utf-8")], APP_CSS).into_response()
}

pub(crate) async fn js_asset() -> Response {
    ([(CONTENT_TYPE, "text/javascript; charset=utf-8")], APP_JS).into_response()
}

#[derive(Template)]
#[template(path = "index.html")]
pub(crate) struct IndexTemplate<'a> {
    pub(crate) m: &'static Messages,
    pub(crate) page_title: &'a str,
    pub(crate) token: &'a str,
    pub(crate) projects: &'a [ProjectCardView],
    pub(crate) archives: &'a [ArchiveView],
    pub(crate) archives_error: Option<&'a str>,
}

#[derive(Template)]
#[template(path = "project.html")]
pub(crate) struct ProjectTemplate<'a> {
    pub(crate) m: &'static Messages,
    pub(crate) page_title: &'a str,
    pub(crate) token: &'a str,
    pub(crate) project_key: &'a str,
    pub(crate) project_label: &'a str,
    /// One of `sessions`, `import`, `maintenance`, or `history`.
    pub(crate) tab: &'static str,
    /// Ready import-inbox candidates, shown on the Import link.
    pub(crate) inbox_ready: usize,
    pub(crate) watermark: &'a str,
    pub(crate) complete_sessions: usize,
    pub(crate) pending_sessions: usize,
    pub(crate) incomplete_sessions: usize,
    pub(crate) fsck_supported: bool,
    pub(crate) archive_export_supported: bool,
    pub(crate) archive_restore_supported: bool,
    pub(crate) import_inbox_supported: bool,
    pub(crate) archive_restore_target_ready: bool,
    pub(crate) has_last_fsck: bool,
    pub(crate) last_fsck_clean: bool,
    pub(crate) last_fsck_objects: usize,
    pub(crate) last_fsck_issues: usize,
    pub(crate) refs: &'a [RefView],
    pub(crate) reflog: &'a [ReflogView],
    pub(crate) sessions: &'a [SessionSummaryView],
}

#[derive(Template)]
#[template(path = "session.html")]
pub(crate) struct SessionTemplate<'a> {
    pub(crate) m: &'static Messages,
    pub(crate) page_title: &'a str,
    pub(crate) token: &'a str,
    pub(crate) project_key: &'a str,
    pub(crate) project_label: &'a str,
    pub(crate) session: &'a str,
    pub(crate) complete: bool,
    pub(crate) pending: bool,
    pub(crate) show_evidence: bool,
    pub(crate) state_label: &'a str,
    pub(crate) state_tone: &'a str,
    pub(crate) state_description: &'a str,
    pub(crate) ai_output_source: &'a str,
    pub(crate) review_id: &'a str,
    pub(crate) decision_url: &'a str,
    pub(crate) disposition: &'a str,
    pub(crate) decision_outcome: &'a str,
    pub(crate) rationale: &'a str,
    pub(crate) subject_label: &'a str,
    pub(crate) creator_name: &'a str,
    pub(crate) decision_recorded_at: &'a crate::views::TimeView,
    pub(crate) generation_note: &'a str,
    pub(crate) source: Option<&'a synapse_local_service::CreatorSourceBinding>,
    pub(crate) reuse_source: Option<&'a synapse_local_service::CreatorReuseSourceBinding>,
    pub(crate) reuse_available: bool,
    pub(crate) reuse_reference: Option<&'a synapse_local_service::CreatorReuseReferenceContext>,
    pub(crate) annotations: &'a [synapse_local_service::CreatorPin],
    pub(crate) annotations_json: &'a str,
    pub(crate) annotations_unavailable: bool,
    pub(crate) selected: &'a str,
    pub(crate) fsck_objects: usize,
    pub(crate) images: &'a [ImageView],
    pub(crate) has_comparison: bool,
    pub(crate) comparison_outcome: &'a str,
    pub(crate) comparison_warning: &'a str,
    pub(crate) comparison_status: &'a str,
    pub(crate) comparison_comparability: &'a str,
    pub(crate) comparison_adapter: &'a str,
    pub(crate) comparison_replay: &'a str,
    pub(crate) timeline: &'a [TimelineView],
    pub(crate) diagnostic: &'a str,
    pub(crate) diagnostic_proposal_ref: &'a str,
    pub(crate) diagnostic_proposal_head: &'a str,
    pub(crate) diagnostic_decision_ref: &'a str,
    pub(crate) diagnostic_decision_head: &'a str,
    pub(crate) report_proposal_head: &'a str,
    pub(crate) report_decision_head: &'a str,
    pub(crate) derived_sessions: &'a [String],
}

#[derive(Template)]
#[template(path = "error.html")]
pub(crate) struct ErrorTemplate<'a> {
    pub(crate) m: &'static Messages,
    pub(crate) page_title: &'a str,
    pub(crate) token: &'a str,
    pub(crate) status: &'a str,
    pub(crate) heading: &'a str,
    pub(crate) summary: &'a str,
    pub(crate) detail: &'a str,
    pub(crate) request_id: &'a str,
}

#[derive(Template)]
#[template(path = "derive.html")]
pub(crate) struct DeriveTemplate<'a> {
    pub(crate) m: &'static Messages,
    pub(crate) page_title: &'a str,
    pub(crate) token: &'a str,
    pub(crate) project_key: &'a str,
    pub(crate) project_label: &'a str,
    pub(crate) preview: &'a synapse_local_service::CreatorSourcePreview,
}

#[derive(Template)]
#[template(path = "reuse.html")]
pub(crate) struct ReuseTemplate<'a> {
    pub(crate) m: &'static Messages,
    pub(crate) page_title: &'a str,
    pub(crate) token: &'a str,
    pub(crate) project_key: &'a str,
    pub(crate) project_label: &'a str,
    pub(crate) preview: &'a synapse_local_service::CreatorReuseSourcePreview,
}

#[derive(Template)]
#[template(path = "presentation.html")]
pub(crate) struct PresentationTemplate<'a> {
    pub(crate) m: &'static Messages,
    pub(crate) page_title: &'a str,
    pub(crate) token: &'a str,
    pub(crate) project_key: &'a str,
    pub(crate) project_label: &'a str,
    pub(crate) sessions: &'a [String],
}
