//! Openapi contract DTO-shape parity checks.
//!
//! `api/local/v1/openapi.json` is the hand-maintained wire contract for the
//! localhost HTTP API. This file generalizes the single-DTO comparison in
//! `tests/maintenance.rs` (`maintenance_operation_dtos_serialize_to_the_openapi_shape`,
//! which hand-writes an expected `json!` literal) into a table-driven check
//! that instead loads the real `components.schemas.<Name>` entries from the
//! openapi document and compares them against `serde_json::to_value` of a
//! constructed DTO instance.
//!
//! For each covered DTO this asserts:
//!   (a) every key produced by serialization appears in the schema's
//!       `properties`, and
//!   (b) every property the schema's `required` array names appears in the
//!       serialized keys.
//!
//! This is a shape check, not a full JSON Schema validator: it does not
//! check types, string patterns, enum membership, or array/length bounds.
//! It exists to catch the two most common and most silent drift failures —
//! a Rust field renamed/added/removed without updating the schema, or a
//! schema `required` list that no longer matches what the DTO actually
//! serializes — both of which are otherwise undetectable without reading
//! the Rust source and the JSON side by side.
//!
//! `#[serde(untagged)]` enums (`CreatorSessionDetail`, `CreatorDecisionResponse`,
//! `OperationResult`) and their openapi `oneOf` counterparts have no schema of
//! their own to compare against (a `oneOf` schema carries no `properties` or
//! `required`); each variant is instead constructed and checked against its
//! own concrete named schema.

use serde_json::Value;
use std::fs;
use synapse_local_service::{
    ArchiveExportRequest, ArchiveList, ArchiveResult, ArchiveResultKind, ArchiveState,
    ArchiveSummary, CommittedCreatorSession, CommittedState, ComparisonEvidence,
    CompleteCreatorSession, CompleteState, CreatorDecision, CreatorDecisionReceipt,
    CreatorDecisionRequest, CreatorReport, CreatorSessionCounts, CreatorSessionDiagnostic,
    CreatorSessionList, CreatorSessionState, CreatorSessionSummary, FsckResult, HealthResponse,
    IncompleteCreatorSession, IncompleteState, OperationAccepted, OperationKind, OperationResult,
    OperationState, OperationStatus, PendingCreatorSession, PendingReviewState, Problem,
    ProjectCapabilities, ProjectConfirmation, ProjectList, ProjectState, ProjectStatus,
    ProjectSummary, ProjectionState, RefList, RefRecord, ReflogEntry, ReflogPage, SnapshotContext,
    TimelineEntry,
};

/// Loads `api/local/v1/openapi.json` once per test process invocation (each
/// `#[test]` runs in its own process under the default test harness, so this
/// is a plain synchronous read rather than a shared-once-cell).
fn openapi_document() -> Value {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../api/local/v1/openapi.json"
    );
    let raw =
        fs::read_to_string(path).unwrap_or_else(|error| panic!("failed to read {path}: {error}"));
    serde_json::from_str(&raw).unwrap_or_else(|error| panic!("{path} is not valid JSON: {error}"))
}

/// Compares one serialized DTO instance against `components.schemas.<schema_name>`
/// from the openapi document.
///
/// Asserts:
///   (a) every serialized object key exists in the schema's `properties`;
///   (b) every schema `required` entry exists among the serialized keys.
///
/// `document` is threaded through explicitly (rather than reloaded per call)
/// so a table-driven test that checks many schemas in one `#[test]` pays the
/// file-read and JSON-parse cost once.
fn assert_matches_openapi_schema(document: &Value, schema_name: &str, value: &Value) {
    let schema = document
        .pointer(&format!("/components/schemas/{schema_name}"))
        .unwrap_or_else(|| panic!("openapi.json has no components.schemas.{schema_name} entry"));
    let properties = schema
        .get("properties")
        .and_then(Value::as_object)
        .unwrap_or_else(|| {
            panic!(
                "components.schemas.{schema_name} has no properties object \
                 (is this actually a oneOf/allOf wrapper? compare against a \
                 concrete variant schema name instead)"
            )
        });
    let required: Vec<&str> = schema
        .get("required")
        .and_then(Value::as_array)
        .map(|entries| entries.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();

    let serialized = value
        .as_object()
        .unwrap_or_else(|| panic!("{schema_name} instance did not serialize to a JSON object"));

    for key in serialized.keys() {
        assert!(
            properties.contains_key(key),
            "{schema_name}: serialized key {key:?} is not declared in \
             components.schemas.{schema_name}.properties ({:?}); \
             either the Rust field is undocumented or the schema is stale",
            properties.keys().collect::<Vec<_>>()
        );
    }

    for key in required {
        assert!(
            serialized.contains_key(key),
            "{schema_name}: components.schemas.{schema_name}.required lists \
             {key:?}, but the serialized DTO does not produce that key \
             ({:?}); either the schema over-requires or the Rust struct \
             dropped a field",
            serialized.keys().collect::<Vec<_>>()
        );
    }
}

fn sample_snapshot() -> SnapshotContext {
    SnapshotContext {
        watermark: format!("sha256:{}", "a".repeat(64)),
        ref_count: 3,
        projection_source_fingerprint: Some(format!(
            "projection-source-v1:sha256:{}",
            "b".repeat(64)
        )),
    }
}

fn sample_capabilities() -> ProjectCapabilities {
    ProjectCapabilities {
        read: true,
        creator_import: true,
        human_decision: true,
        fsck: true,
        archive_export: false,
        archive_restore: false,
    }
}

fn sample_project_summary() -> ProjectSummary {
    ProjectSummary {
        project_key: "demo".into(),
        display_label: "Demo project".into(),
        state: ProjectState::Ready,
        capabilities: sample_capabilities(),
    }
}

fn sample_fsck_result() -> FsckResult {
    FsckResult {
        clean: true,
        objects_seen: 12,
        objects_verified: 12,
        closure_count: 4,
        issue_count: 0,
    }
}

#[test]
fn archive_export_request_matches_the_openapi_schema() {
    let document = openapi_document();
    assert_matches_openapi_schema(
        &document,
        "ArchiveExportRequest",
        &serde_json::to_value(ArchiveExportRequest {
            archive_name: "nightly".into(),
            confirm_project_key: "demo".into(),
        })
        .unwrap(),
    );
}

fn sample_comparison_evidence() -> ComparisonEvidence {
    ComparisonEvidence {
        analysis_oid: "analysis:sg-oid-v1:opaque-1".into(),
        tool_id: "byte-identity-v1".into(),
        tool_actor_oid: "actor:sg-oid-v1:opaque-1".into(),
        adapter_id: "local-adapter".into(),
        adapter_version: "1.0.0".into(),
        implementation_oid: format!("blob:sg-oid-v1:sha256:{}", "c".repeat(64)),
        configuration_oid: format!("blob:sg-oid-v1:sha256:{}", "d".repeat(64)),
        status: "succeeded".into(),
        comparability: "partial".into(),
        outcome: "different".into(),
        reason_codes: vec!["dimensions_differ".into()],
        warnings: vec!["decoded raster metadata was stripped".into()],
        base_observation_oid: "observation:sg-oid-v1:opaque-1".into(),
        target_observation_oid: "observation:sg-oid-v1:opaque-2".into(),
        base_media_oid: format!("blob:sg-oid-v1:sha256:{}", "e".repeat(64)),
        target_media_oid: format!("blob:sg-oid-v1:sha256:{}", "f".repeat(64)),
        replay_ready: true,
        reachable_from: vec!["refs/creator/decision/render-session".into()],
    }
}

fn sample_timeline_entry() -> TimelineEntry {
    TimelineEntry {
        oid: "activity:sg-oid-v1:opaque-1".into(),
        stage: "decision".into(),
        kind: "activity".into(),
        entity_id: "activity-1".into(),
        ordering_time: "2026-07-16T00:00:00Z".into(),
        time_basis: "commit".into(),
        reachable_from: vec!["refs/creator/decision/render-session".into()],
    }
}

fn sample_creator_report() -> CreatorReport {
    let commit_oid = |seed: &str| format!("commit:sg-oid-v1:sha256:{}", seed.repeat(64));
    let blob_oid = |seed: &str| format!("blob:sg-oid-v1:sha256:{}", seed.repeat(64));
    CreatorReport {
        snapshot: sample_snapshot(),
        session: "render-session".into(),
        project_id: "project:sg-oid-v1:opaque-1".into(),
        subject_id: "subject:sg-oid-v1:opaque-1".into(),
        creator_id: "creator:sg-oid-v1:opaque-1".into(),
        agent_id: "agent:sg-oid-v1:opaque-1".into(),
        decision_ref: "refs/creator/decision/render-session".into(),
        proposal_ref: "refs/creator/proposal/render-session".into(),
        decision_head: commit_oid("1"),
        proposal_head: commit_oid("2"),
        base_head: commit_oid("3"),
        base_snapshot: "tree:sg-oid-v1:opaque-1".into(),
        proposal_snapshot: "tree:sg-oid-v1:opaque-2".into(),
        decision_snapshot: "tree:sg-oid-v1:opaque-3".into(),
        disposition: "adopt".into(),
        selected_ai_output: true,
        proposal_attributed_to_agent: "agent:sg-oid-v1:opaque-1".into(),
        ai_output_source: "caller_supplied".into(),
        reviewed_by_human: "creator:sg-oid-v1:opaque-1".into(),
        rationale: Some("Reviewed through the openapi shape fixture.".into()),
        original_blob_oid: blob_oid("4"),
        current_blob_oid: blob_oid("5"),
        ai_output_blob_oid: blob_oid("6"),
        comparison: Some(sample_comparison_evidence()),
        timeline: vec![sample_timeline_entry()],
        fsck_objects: 12,
    }
}

fn sample_creator_decision_receipt() -> CreatorDecisionReceipt {
    let commit_oid = |seed: &str| format!("commit:sg-oid-v1:sha256:{}", seed.repeat(64));
    let blob_oid = |seed: &str| format!("blob:sg-oid-v1:sha256:{}", seed.repeat(64));
    CreatorDecisionReceipt {
        session: "render-session".into(),
        project_id: "project:sg-oid-v1:opaque-1".into(),
        subject_id: "subject:sg-oid-v1:opaque-1".into(),
        creator_id: "creator:sg-oid-v1:opaque-1".into(),
        agent_id: "agent:sg-oid-v1:opaque-1".into(),
        decision_ref: "refs/creator/decision/render-session".into(),
        proposal_ref: "refs/creator/proposal/render-session".into(),
        base_head: commit_oid("1"),
        proposal_head: commit_oid("2"),
        decision_head: commit_oid("3"),
        original_blob_oid: blob_oid("4"),
        current_blob_oid: blob_oid("5"),
        ai_output_blob_oid: blob_oid("6"),
        capture_profile_oid: "profile:sg-oid-v1:opaque-1".into(),
        original_observation_oid: "observation:sg-oid-v1:opaque-1".into(),
        current_observation_oid: "observation:sg-oid-v1:opaque-2".into(),
        comparison_tool_id: "byte-identity-v1".into(),
        comparison_tool_actor_oid: "actor:sg-oid-v1:opaque-1".into(),
        comparison_analysis_oid: "analysis:sg-oid-v1:opaque-1".into(),
        comparison_implementation_oid: blob_oid("7"),
        comparison_configuration_oid: blob_oid("8"),
        byte_identity_outcome: "different".into(),
        comparison_status: "succeeded".into(),
        comparison_comparability: "partial".into(),
        comparison_reason_codes: vec!["dimensions_differ".into()],
        ai_activity_oid: "activity:sg-oid-v1:opaque-1".into(),
        decision_feedback_oid: "feedback:sg-oid-v1:opaque-1".into(),
        disposition: CreatorDecision::Adopt,
    }
}

fn sample_problem() -> Problem {
    Problem {
        r#type: "urn:synapsegit:error:project_not_found".into(),
        title: "Project not found".into(),
        status: 404,
        code: "project_not_found".into(),
        detail: "The requested project is not registered on this server.".into(),
        request_id: "local-test-instance-1".into(),
        retryable: false,
    }
}

#[test]
fn creator_decision_request_matches_the_openapi_schema() {
    let document = openapi_document();
    let value = CreatorDecisionRequest {
        review_id: "a".repeat(22),
        disposition: CreatorDecision::Adopt,
        rationale: Some("Reviewed through the openapi shape fixture.".into()),
    };
    assert_matches_openapi_schema(
        &document,
        "CreatorDecisionRequest",
        &serde_json::to_value(value).unwrap(),
    );
}

#[test]
fn health_response_matches_the_openapi_schema() {
    let document = openapi_document();
    let value = HealthResponse::new("local-test-instance");
    assert_matches_openapi_schema(
        &document,
        "HealthResponse",
        &serde_json::to_value(value).unwrap(),
    );
}

#[test]
fn problem_matches_the_openapi_schema() {
    let document = openapi_document();
    let value = sample_problem();
    assert_matches_openapi_schema(&document, "Problem", &serde_json::to_value(value).unwrap());
}

#[test]
fn project_summary_and_capabilities_match_the_openapi_schema() {
    let document = openapi_document();
    let summary = sample_project_summary();
    assert_matches_openapi_schema(
        &document,
        "ProjectSummary",
        &serde_json::to_value(summary.clone()).unwrap(),
    );
    // Nested component type of ProjectSummary.
    assert_matches_openapi_schema(
        &document,
        "ProjectCapabilities",
        &serde_json::to_value(summary.capabilities).unwrap(),
    );
}

#[test]
fn project_list_matches_the_openapi_schema() {
    let document = openapi_document();
    let value = ProjectList {
        projects: vec![sample_project_summary()],
    };
    assert_matches_openapi_schema(
        &document,
        "ProjectList",
        &serde_json::to_value(value).unwrap(),
    );
}

#[test]
fn project_status_and_nested_types_match_the_openapi_schema() {
    let document = openapi_document();
    let status = ProjectStatus {
        project: sample_project_summary(),
        snapshot: sample_snapshot(),
        creator_session_counts: CreatorSessionCounts {
            complete: 1,
            pending_review: 1,
            incomplete: 0,
        },
        projection_state: ProjectionState::Current,
        last_fsck: Some(sample_fsck_result()),
    };
    assert_matches_openapi_schema(
        &document,
        "ProjectStatus",
        &serde_json::to_value(status.clone()).unwrap(),
    );
    // Nested component types of ProjectStatus.
    assert_matches_openapi_schema(
        &document,
        "SnapshotContext",
        &serde_json::to_value(status.snapshot).unwrap(),
    );
    assert_matches_openapi_schema(
        &document,
        "FsckResult",
        &serde_json::to_value(status.last_fsck.unwrap()).unwrap(),
    );
    // CreatorSessionCounts is an inline object in the openapi schema
    // (ProjectStatus.properties.creator_session_counts), not a named
    // components.schemas entry, so it has no standalone schema name to
    // compare against; ProjectStatus's own key/required check above already
    // covers the field that carries it.
}

#[test]
fn ref_record_and_ref_list_match_the_openapi_schema() {
    let document = openapi_document();
    let record = RefRecord {
        name: "refs/creator/decision/render-session".into(),
        head: format!("commit:sg-oid-v1:sha256:{}", "1".repeat(64)),
        updated_event_id: "42".into(),
    };
    assert_matches_openapi_schema(
        &document,
        "RefRecord",
        &serde_json::to_value(record.clone()).unwrap(),
    );
    let list = RefList {
        snapshot: sample_snapshot(),
        refs: vec![record],
    };
    assert_matches_openapi_schema(&document, "RefList", &serde_json::to_value(list).unwrap());
}

#[test]
fn reflog_entry_and_reflog_page_match_the_openapi_schema() {
    let document = openapi_document();
    let entry = ReflogEntry {
        event_id: "42".into(),
        ref_name: "refs/creator/decision/render-session".into(),
        old_head: Some(format!("commit:sg-oid-v1:sha256:{}", "1".repeat(64))),
        new_head: format!("commit:sg-oid-v1:sha256:{}", "2".repeat(64)),
        occurred_at_unix_nanos: "1752624000000000000".into(),
        actor: Some("creator:sg-oid-v1:opaque-1".into()),
        message: Some("adopt".into()),
    };
    assert_matches_openapi_schema(
        &document,
        "ReflogEntry",
        &serde_json::to_value(entry.clone()).unwrap(),
    );
    let page = ReflogPage {
        snapshot: sample_snapshot(),
        entries: vec![entry],
        next_after_event_id: Some("43".into()),
    };
    assert_matches_openapi_schema(
        &document,
        "ReflogPage",
        &serde_json::to_value(page).unwrap(),
    );
}

#[test]
fn creator_session_summary_and_list_match_the_openapi_schema() {
    let document = openapi_document();
    let summary = CreatorSessionSummary {
        session: "render-session".into(),
        state: CreatorSessionState::Complete,
        proposal_ref: Some("refs/creator/proposal/render-session".into()),
        proposal_head: Some(format!("commit:sg-oid-v1:sha256:{}", "1".repeat(64))),
        decision_ref: Some("refs/creator/decision/render-session".into()),
        decision_head: Some(format!("commit:sg-oid-v1:sha256:{}", "2".repeat(64))),
    };
    assert_matches_openapi_schema(
        &document,
        "CreatorSessionSummary",
        &serde_json::to_value(summary.clone()).unwrap(),
    );
    let list = CreatorSessionList {
        snapshot: sample_snapshot(),
        sessions: vec![summary],
    };
    assert_matches_openapi_schema(
        &document,
        "CreatorSessionList",
        &serde_json::to_value(list).unwrap(),
    );
}

#[test]
fn creator_session_diagnostic_matches_the_openapi_schema() {
    let document = openapi_document();
    let diagnostic = CreatorSessionDiagnostic {
        snapshot: sample_snapshot(),
        session: "incomplete-session".into(),
        state: CreatorSessionState::Incomplete,
        proposal_ref: Some("refs/creator/proposal/incomplete-session".into()),
        proposal_head: Some(format!("commit:sg-oid-v1:sha256:{}", "1".repeat(64))),
        decision_ref: Some("refs/creator/decision/incomplete-session".into()),
        decision_head: Some(format!("commit:sg-oid-v1:sha256:{}", "2".repeat(64))),
        automatic_resume_supported: false,
        automatic_cleanup_supported: false,
        recommended_action: "Run fsck to determine whether the proposal Blob set is intact.".into(),
    };
    assert_matches_openapi_schema(
        &document,
        "CreatorSessionDiagnostic",
        &serde_json::to_value(diagnostic).unwrap(),
    );
}

#[test]
fn creator_report_and_nested_types_match_the_openapi_schema() {
    let document = openapi_document();
    let report = sample_creator_report();
    assert_matches_openapi_schema(
        &document,
        "CreatorReport",
        &serde_json::to_value(report.clone()).unwrap(),
    );
    // Nested component types of CreatorReport.
    assert_matches_openapi_schema(
        &document,
        "ComparisonEvidence",
        &serde_json::to_value(report.comparison.unwrap()).unwrap(),
    );
    assert_matches_openapi_schema(
        &document,
        "TimelineEntry",
        &serde_json::to_value(report.timeline[0].clone()).unwrap(),
    );
}

#[test]
fn creator_decision_receipt_matches_the_openapi_schema() {
    let document = openapi_document();
    let receipt = sample_creator_decision_receipt();
    assert_matches_openapi_schema(
        &document,
        "CreatorDecisionReceipt",
        &serde_json::to_value(receipt).unwrap(),
    );
}

#[test]
fn fsck_result_matches_the_openapi_schema() {
    let document = openapi_document();
    let result = sample_fsck_result();
    assert_matches_openapi_schema(
        &document,
        "FsckResult",
        &serde_json::to_value(result).unwrap(),
    );
}

#[test]
fn archive_summary_and_archive_list_match_the_openapi_schema() {
    let document = openapi_document();
    let valid = ArchiveSummary {
        archive_name: "nightly-2026-08-17".into(),
        state: ArchiveState::Valid,
        manifest_checksum: Some("a".repeat(64)),
    };
    assert_matches_openapi_schema(
        &document,
        "ArchiveSummary",
        &serde_json::to_value(valid.clone()).unwrap(),
    );
    let staging = ArchiveSummary {
        archive_name: "in-progress".into(),
        state: ArchiveState::StagingOrUnknown,
        manifest_checksum: None,
    };
    assert_matches_openapi_schema(
        &document,
        "ArchiveSummary",
        &serde_json::to_value(staging.clone()).unwrap(),
    );
    let list = ArchiveList {
        archives: vec![staging, valid],
    };
    assert_matches_openapi_schema(
        &document,
        "ArchiveList",
        &serde_json::to_value(list).unwrap(),
    );
}

#[test]
fn project_confirmation_matches_the_openapi_schema() {
    let document = openapi_document();
    let value = ProjectConfirmation {
        confirm_project_key: "demo".into(),
    };
    assert_matches_openapi_schema(
        &document,
        "ProjectConfirmation",
        &serde_json::to_value(value).unwrap(),
    );
}

#[test]
fn operation_accepted_matches_the_openapi_schema() {
    let document = openapi_document();
    let value = OperationAccepted {
        operation_id: "a".repeat(22),
        state: OperationState::Queued,
        poll_path: format!("/api/v1/operations/{}", "a".repeat(22)),
    };
    assert_matches_openapi_schema(
        &document,
        "OperationAccepted",
        &serde_json::to_value(value).unwrap(),
    );
}

/// `OperationStatus.result` is `#[serde(untagged)] OperationResult { Fsck,
/// Archive }`, matching the openapi `oneOf: [FsckResult, ArchiveResult,
/// null]`. Both branches are covered: the fsck-kind status checks
/// `OperationStatus` and nested `FsckResult`, the archive-kind status
/// separately checks the nested `ArchiveResult` variant.
#[test]
fn operation_status_with_fsck_result_matches_the_openapi_schema() {
    let document = openapi_document();
    let status = OperationStatus {
        operation_id: "a".repeat(22),
        kind: OperationKind::Fsck,
        project_key: "demo".into(),
        state: OperationState::Succeeded,
        submitted_at: "2026-07-16T00:00:00Z".into(),
        completed_at: Some("2026-07-16T00:00:01Z".into()),
        result: Some(OperationResult::Fsck(sample_fsck_result())),
        error: None,
    };
    assert_matches_openapi_schema(
        &document,
        "OperationStatus",
        &serde_json::to_value(status).unwrap(),
    );
}

#[test]
fn operation_status_with_archive_result_matches_the_openapi_schema() {
    let document = openapi_document();
    let archive_result = ArchiveResult {
        archive_name: "nightly".into(),
        result_kind: ArchiveResultKind::Restored,
        report_equivalence_required: true,
    };
    let status = OperationStatus {
        operation_id: "b".repeat(22),
        kind: OperationKind::ArchiveRestore,
        project_key: "demo".into(),
        state: OperationState::Succeeded,
        submitted_at: "2026-07-16T00:00:00Z".into(),
        completed_at: Some("2026-07-16T00:00:01Z".into()),
        result: Some(OperationResult::Archive(archive_result.clone())),
        error: None,
    };
    assert_matches_openapi_schema(
        &document,
        "OperationStatus",
        &serde_json::to_value(status).unwrap(),
    );
    // Nested component type carried by the untagged OperationResult::Archive
    // variant; checked against its own concrete schema (see module doc).
    assert_matches_openapi_schema(
        &document,
        "ArchiveResult",
        &serde_json::to_value(archive_result).unwrap(),
    );
}

/// `OperationStatus.error` carries `Problem` when the job failed; checked
/// separately from the two happy-path variants above.
#[test]
fn operation_status_with_error_matches_the_openapi_schema() {
    let document = openapi_document();
    let status = OperationStatus {
        operation_id: "c".repeat(22),
        kind: OperationKind::Fsck,
        project_key: "demo".into(),
        state: OperationState::Failed,
        submitted_at: "2026-07-16T00:00:00Z".into(),
        completed_at: Some("2026-07-16T00:00:01Z".into()),
        result: None,
        error: Some(sample_problem()),
    };
    assert_matches_openapi_schema(
        &document,
        "OperationStatus",
        &serde_json::to_value(status).unwrap(),
    );
}

/// `CreatorSessionDetail` is `#[serde(untagged)]` over three variants,
/// matching the openapi `oneOf: [CompleteCreatorSession,
/// PendingCreatorSession, IncompleteCreatorSession]`. Each variant is
/// constructed and checked against its own concrete schema name; see the
/// module doc comment for why `CreatorSessionDetail` itself has no schema to
/// compare against directly.
#[test]
fn creator_session_detail_complete_variant_matches_the_openapi_schema() {
    let document = openapi_document();
    let complete = CompleteCreatorSession {
        state: CompleteState::Complete,
        report: sample_creator_report(),
    };
    assert_matches_openapi_schema(
        &document,
        "CompleteCreatorSession",
        &serde_json::to_value(complete).unwrap(),
    );
}

#[test]
fn creator_session_detail_pending_variant_matches_the_openapi_schema() {
    let document = openapi_document();
    let pending = PendingCreatorSession {
        state: PendingReviewState::PendingReview,
        snapshot: sample_snapshot(),
        server_instance: "local-test-instance".into(),
        review_id: "a".repeat(22),
        session: "web-review".into(),
        project_id: "project:sg-oid-v1:opaque-1".into(),
        subject_id: "subject:sg-oid-v1:opaque-1".into(),
        proposal_ref: "refs/creator/proposal/web-review".into(),
        proposal_head: format!("commit:sg-oid-v1:sha256:{}", "1".repeat(64)),
        original_blob_oid: format!("blob:sg-oid-v1:sha256:{}", "2".repeat(64)),
        current_blob_oid: format!("blob:sg-oid-v1:sha256:{}", "3".repeat(64)),
        ai_output_blob_oid: format!("blob:sg-oid-v1:sha256:{}", "4".repeat(64)),
        ai_output_source: "caller_supplied".into(),
        comparison: sample_comparison_evidence(),
    };
    assert_matches_openapi_schema(
        &document,
        "PendingCreatorSession",
        &serde_json::to_value(pending).unwrap(),
    );
}

#[test]
fn creator_session_detail_incomplete_variant_matches_the_openapi_schema() {
    let document = openapi_document();
    let incomplete = IncompleteCreatorSession {
        state: IncompleteState::Incomplete,
        snapshot: sample_snapshot(),
        session: "incomplete-session".into(),
        recovery_supported: false,
        diagnostic: "The proposal Ref is unreachable from the current snapshot.".into(),
    };
    assert_matches_openapi_schema(
        &document,
        "IncompleteCreatorSession",
        &serde_json::to_value(incomplete).unwrap(),
    );
}

/// `CreatorDecisionResponse` is `#[serde(untagged)]` over two variants,
/// matching the openapi `oneOf: [CompleteCreatorSession,
/// CommittedCreatorSession]`. The complete variant reuses
/// `CompleteCreatorSession` above; this covers the committed variant, whose
/// `receipt` field nests `CreatorDecisionReceipt` (already covered above by
/// `creator_decision_receipt_matches_the_openapi_schema`, exercised
/// standalone here too since it is embedded rather than top-level in this
/// shape).
#[test]
fn creator_decision_response_committed_variant_matches_the_openapi_schema() {
    let document = openapi_document();
    let committed = CommittedCreatorSession {
        state: CommittedState::Committed,
        receipt: sample_creator_decision_receipt(),
        report_available: false,
        inspection_required: true,
    };
    assert_matches_openapi_schema(
        &document,
        "CommittedCreatorSession",
        &serde_json::to_value(committed).unwrap(),
    );
}
