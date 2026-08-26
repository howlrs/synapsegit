use axum::http::StatusCode;
use synapse_local_service::{
    ArchiveState, CreatorReport, CreatorSessionDetail, CreatorSessionDiagnostic,
    CreatorSessionState, ProjectState, ServiceError,
};

use crate::state::AppState;

#[derive(Clone)]
pub(crate) struct HttpFailure {
    pub(crate) status: StatusCode,
    pub(crate) code: String,
    pub(crate) title: String,
    pub(crate) detail: String,
    pub(crate) request_id: String,
    pub(crate) retryable: bool,
}

impl HttpFailure {
    pub(crate) fn service(state: &AppState, error: ServiceError) -> Self {
        let status = match error.code() {
            "project_not_found" | "creator_session_not_found" | "creator_review_state_lost" => {
                StatusCode::NOT_FOUND
            }
            "local_request_denied" | "usage_error" | "path_segment_invalid" => {
                StatusCode::BAD_REQUEST
            }
            "resource_limit" => StatusCode::PAYLOAD_TOO_LARGE,
            "creator_session_exists"
            | "creator_session_incomplete"
            | "creator_review_busy"
            | "creator_outcome_unknown"
            | "ref_conflict"
            | "stale_base"
            | "archive_not_empty" => StatusCode::CONFLICT,
            "creator_report_invalid"
            | "fsck_failed"
            | "oid_mismatch"
            | "closure_missing"
            | "reference_type_mismatch"
            | "schema_invalid" => StatusCode::UNPROCESSABLE_ENTITY,
            "archive_invalid" => StatusCode::CONFLICT,
            "service_unavailable" | "storage_error" => StatusCode::SERVICE_UNAVAILABLE,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        };
        Self {
            status,
            code: error.code().to_owned(),
            title: status
                .canonical_reason()
                .unwrap_or("Local application error")
                .to_owned(),
            detail: error.detail().to_owned(),
            request_id: state.security.request_id(),
            retryable: error.retryable(),
        }
    }

    pub(crate) fn request(state: &AppState, code: &str, detail: &str) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            code: code.into(),
            title: "Invalid local request".into(),
            detail: detail.into(),
            request_id: state.security.request_id(),
            retryable: false,
        }
    }

    pub(crate) fn not_found(state: &AppState, detail: &str) -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            code: "local_request_denied".into(),
            title: "Not found".into(),
            detail: detail.into(),
            request_id: state.security.request_id(),
            retryable: false,
        }
    }

    pub(crate) fn limit(state: &AppState, detail: &str) -> Self {
        Self {
            status: StatusCode::PAYLOAD_TOO_LARGE,
            code: "resource_limit".into(),
            title: "Payload too large".into(),
            detail: detail.into(),
            request_id: state.security.request_id(),
            retryable: false,
        }
    }

    pub(crate) fn storage(state: &AppState, detail: &str) -> Self {
        Self {
            status: StatusCode::SERVICE_UNAVAILABLE,
            code: "storage_error".into(),
            title: "Storage unavailable".into(),
            detail: detail.into(),
            request_id: state.security.request_id(),
            retryable: true,
        }
    }

    pub(crate) fn internal(state: &AppState, detail: &str) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            code: "storage_error".into(),
            title: "Local application error".into(),
            detail: detail.into(),
            request_id: state.security.request_id(),
            retryable: true,
        }
    }
}

pub(crate) fn project_state_label(state: ProjectState) -> &'static str {
    match state {
        ProjectState::Ready => "利用可能",
        ProjectState::EmptyRestoreTarget => "空の復元先",
        ProjectState::Unavailable => "利用不可",
    }
}

pub(crate) fn project_state_tone(state: ProjectState) -> &'static str {
    match state {
        ProjectState::Ready => "success",
        ProjectState::EmptyRestoreTarget => "info",
        ProjectState::Unavailable => "danger",
    }
}

pub(crate) fn session_state_label(state: CreatorSessionState) -> &'static str {
    match state {
        CreatorSessionState::Complete => "完了",
        CreatorSessionState::PendingReview => "レビュー待ち",
        CreatorSessionState::Incomplete => "未完了",
    }
}

pub(crate) fn session_state_tone(state: CreatorSessionState) -> &'static str {
    match state {
        CreatorSessionState::Complete => "success",
        CreatorSessionState::PendingReview => "info",
        CreatorSessionState::Incomplete => "warning",
    }
}

pub(crate) fn archive_state_label(state: ArchiveState) -> &'static str {
    match state {
        ArchiveState::Valid => "valid",
        ArchiveState::Invalid => "invalid",
        ArchiveState::StagingOrUnknown => "staging または unknown",
    }
}

pub(crate) fn archive_state_tone(state: ArchiveState) -> &'static str {
    match state {
        ArchiveState::Valid => "success",
        ArchiveState::Invalid => "danger",
        ArchiveState::StagingOrUnknown => "warning",
    }
}

pub(crate) struct SessionPageView {
    pub(crate) complete: bool,
    pub(crate) pending: bool,
    pub(crate) show_evidence: bool,
    pub(crate) state_label: String,
    pub(crate) state_tone: String,
    pub(crate) state_description: String,
    pub(crate) ai_output_source: String,
    pub(crate) review_id: String,
    pub(crate) decision_url: String,
    pub(crate) disposition: String,
    pub(crate) selected: String,
    pub(crate) fsck_objects: usize,
    pub(crate) images: Vec<ImageView>,
    pub(crate) has_comparison: bool,
    pub(crate) comparison_outcome: String,
    pub(crate) comparison_warning: String,
    pub(crate) comparison_status: String,
    pub(crate) comparison_comparability: String,
    pub(crate) comparison_adapter: String,
    pub(crate) comparison_replay: String,
    pub(crate) timeline: Vec<TimelineView>,
    pub(crate) diagnostic: String,
    pub(crate) diagnostic_proposal_ref: String,
    pub(crate) diagnostic_proposal_head: String,
    pub(crate) diagnostic_decision_ref: String,
    pub(crate) diagnostic_decision_head: String,
}

impl SessionPageView {
    pub(crate) fn new(
        project_key: &str,
        _project_label: &str,
        session: &str,
        detail: CreatorSessionDetail,
        diagnostic: Option<CreatorSessionDiagnostic>,
    ) -> Self {
        match detail {
            CreatorSessionDetail::Complete(detail) => {
                Self::complete(project_key, session, detail.report)
            }
            CreatorSessionDetail::PendingReview(detail) => {
                let detail = *detail;
                let images = Self::images(
                    project_key,
                    session,
                    &detail.original_blob_oid,
                    &detail.current_blob_oid,
                    &detail.ai_output_blob_oid,
                );
                let comparison = detail.comparison;
                Self {
                    complete: false,
                    pending: true,
                    show_evidence: true,
                    state_label: "レビュー待ち".into(),
                    state_tone: "info".into(),
                    state_description: "このprocess内でHuman reviewを待っています。".into(),
                    ai_output_source: detail.ai_output_source,
                    review_id: detail.review_id,
                    decision_url: format!(
                        "/api/v1/projects/{project_key}/creator-sessions/{session}/decisions"
                    ),
                    disposition: "—".into(),
                    selected: "—".into(),
                    fsck_objects: 0,
                    images,
                    has_comparison: true,
                    comparison_outcome: comparison.outcome,
                    comparison_warning: comparison.warnings.join(" "),
                    comparison_status: comparison.status,
                    comparison_comparability: comparison.comparability,
                    comparison_adapter: format!(
                        "{} {}",
                        comparison.adapter_id, comparison.adapter_version
                    ),
                    comparison_replay: if comparison.replay_ready {
                        "はい"
                    } else {
                        "いいえ"
                    }
                    .into(),
                    timeline: Vec::new(),
                    diagnostic: String::new(),
                    diagnostic_proposal_ref: String::new(),
                    diagnostic_proposal_head: String::new(),
                    diagnostic_decision_ref: String::new(),
                    diagnostic_decision_head: String::new(),
                }
            }
            CreatorSessionDetail::Incomplete(incomplete) => {
                let (
                    diagnostic,
                    diagnostic_proposal_ref,
                    diagnostic_proposal_head,
                    diagnostic_decision_ref,
                    diagnostic_decision_head,
                ) = diagnostic.map_or_else(
                    || {
                        (
                            incomplete.diagnostic,
                            "—".into(),
                            "—".into(),
                            "—".into(),
                            "—".into(),
                        )
                    },
                    |diagnostic| {
                        (
                            diagnostic.recommended_action,
                            diagnostic.proposal_ref.unwrap_or_else(|| "—".into()),
                            diagnostic.proposal_head.unwrap_or_else(|| "—".into()),
                            diagnostic.decision_ref.unwrap_or_else(|| "—".into()),
                            diagnostic.decision_head.unwrap_or_else(|| "—".into()),
                        )
                    },
                );
                Self {
                    complete: false,
                    pending: false,
                    show_evidence: false,
                    state_label: "未完了".into(),
                    state_tone: "warning".into(),
                    state_description: "現在のRefsは完了したCreator sessionを構成していません。"
                        .into(),
                    ai_output_source: String::new(),
                    review_id: String::new(),
                    decision_url: String::new(),
                    disposition: "—".into(),
                    selected: "—".into(),
                    fsck_objects: 0,
                    images: Vec::new(),
                    has_comparison: false,
                    comparison_outcome: String::new(),
                    comparison_warning: String::new(),
                    comparison_status: String::new(),
                    comparison_comparability: String::new(),
                    comparison_adapter: String::new(),
                    comparison_replay: String::new(),
                    timeline: Vec::new(),
                    diagnostic,
                    diagnostic_proposal_ref,
                    diagnostic_proposal_head,
                    diagnostic_decision_ref,
                    diagnostic_decision_head,
                }
            }
        }
    }

    fn complete(project_key: &str, session: &str, report: CreatorReport) -> Self {
        let images = Self::images(
            project_key,
            session,
            &report.original_blob_oid,
            &report.current_blob_oid,
            &report.ai_output_blob_oid,
        );
        let timeline = report
            .timeline
            .into_iter()
            .map(|entry| TimelineView {
                oid: entry.oid,
                stage: entry.stage,
                kind: entry.kind,
                ordering_time: entry.ordering_time,
                time_basis: entry.time_basis,
            })
            .collect();
        let (
            has_comparison,
            comparison_outcome,
            comparison_warning,
            comparison_status,
            comparison_comparability,
            comparison_adapter,
            comparison_replay,
        ) = if let Some(comparison) = report.comparison {
            (
                true,
                comparison.outcome,
                comparison.warnings.join(" "),
                comparison.status,
                comparison.comparability,
                format!("{} {}", comparison.adapter_id, comparison.adapter_version),
                if comparison.replay_ready {
                    "はい"
                } else {
                    "いいえ"
                }
                .into(),
            )
        } else {
            (
                false,
                String::new(),
                String::new(),
                String::new(),
                String::new(),
                String::new(),
                String::new(),
            )
        };
        Self {
            complete: true,
            pending: false,
            show_evidence: true,
            state_label: "完了".into(),
            state_tone: "success".into(),
            state_description: "現在のRefsとCASから検証済みレポートを再構築しました。".into(),
            ai_output_source: report.ai_output_source,
            review_id: String::new(),
            decision_url: String::new(),
            disposition: report.disposition,
            selected: if report.selected_ai_output {
                "はい".into()
            } else {
                "いいえ".into()
            },
            fsck_objects: report.fsck_objects,
            images,
            has_comparison,
            comparison_outcome,
            comparison_warning,
            comparison_status,
            comparison_comparability,
            comparison_adapter,
            comparison_replay,
            timeline,
            diagnostic: String::new(),
            diagnostic_proposal_ref: String::new(),
            diagnostic_proposal_head: String::new(),
            diagnostic_decision_ref: String::new(),
            diagnostic_decision_head: String::new(),
        }
    }

    fn images(
        project_key: &str,
        session: &str,
        original_oid: &str,
        current_oid: &str,
        ai_output_oid: &str,
    ) -> Vec<ImageView> {
        let image_base =
            format!("/api/v1/projects/{project_key}/creator-sessions/{session}/images");
        vec![
            ImageView {
                label: "Original".into(),
                alt: "取り込まれたoriginal画像".into(),
                url: format!("{image_base}/original"),
                oid: original_oid.into(),
                download_name: format!("{session}-original.bin"),
            },
            ImageView {
                label: "Current".into(),
                alt: "取り込まれたcurrent画像".into(),
                url: format!("{image_base}/current"),
                oid: current_oid.into(),
                download_name: format!("{session}-current.bin"),
            },
            ImageView {
                label: "AI output".into(),
                alt: "caller supplied AI output".into(),
                url: format!("{image_base}/ai-output"),
                oid: ai_output_oid.into(),
                download_name: format!("{session}-ai-output.bin"),
            },
        ]
    }
}

pub(crate) struct ProjectCardView {
    pub(crate) key: String,
    pub(crate) label: String,
    pub(crate) state_label: &'static str,
    pub(crate) tone: &'static str,
    pub(crate) ref_count: usize,
    pub(crate) complete_sessions: usize,
    pub(crate) pending_sessions: usize,
    pub(crate) incomplete_sessions: usize,
}

pub(crate) struct ArchiveView {
    pub(crate) archive_name: String,
    pub(crate) state_label: &'static str,
    pub(crate) tone: &'static str,
    pub(crate) checksum_preview: String,
}

const ARCHIVE_CHECKSUM_PREVIEW_LEN: usize = 12;

pub(crate) fn archive_checksum_preview(manifest_checksum: Option<&str>) -> String {
    match manifest_checksum {
        Some(checksum) if checksum.len() > ARCHIVE_CHECKSUM_PREVIEW_LEN => {
            format!("{}…", &checksum[..ARCHIVE_CHECKSUM_PREVIEW_LEN])
        }
        Some(checksum) => checksum.to_owned(),
        None => "—".into(),
    }
}

pub(crate) struct RefView {
    pub(crate) name: String,
    pub(crate) head: String,
    pub(crate) event_id: String,
}

pub(crate) struct ReflogView {
    pub(crate) event_id: String,
    pub(crate) ref_name: String,
    pub(crate) new_head: String,
    pub(crate) message: String,
}

pub(crate) struct SessionSummaryView {
    pub(crate) session: String,
    pub(crate) state_label: &'static str,
    pub(crate) tone: &'static str,
    pub(crate) proposal_head: String,
    pub(crate) decision_head: String,
}

pub(crate) struct ImageView {
    pub(crate) label: String,
    pub(crate) alt: String,
    pub(crate) url: String,
    pub(crate) oid: String,
    pub(crate) download_name: String,
}

pub(crate) struct TimelineView {
    pub(crate) oid: String,
    pub(crate) stage: String,
    pub(crate) kind: String,
    pub(crate) ordering_time: String,
    pub(crate) time_basis: String,
}
