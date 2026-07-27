use crate::CreatorDisposition;
use crate::session::{PILOT_MAX_OUTPUT_BYTES, SessionIds};
use serde_json::{Map as JsonMap, Value as JsonValue, json};
use synapse_canonical::{canonical_bytes, parse_strict};

pub(crate) const SCHEMA_VERSION: &str = "0.1.0";

fn canonical_set(mut values: Vec<JsonValue>) -> Vec<JsonValue> {
    values.sort_by_cached_key(|value| {
        let json = serde_json::to_vec(value).expect("JsonValue serialization cannot fail");
        let parsed = parse_strict(&json).expect("internal set member is strict JSON");
        canonical_bytes(&parsed).expect("internal set member fits canonical limits")
    });
    values
}

fn envelope(
    record_type: &str,
    entity_id: &str,
    recorded_at: &str,
    asserted_by: &str,
    origin: &str,
    payload: JsonValue,
) -> JsonValue {
    json!({
        "object_type": "record",
        "schema_version": SCHEMA_VERSION,
        "record_type": record_type,
        "entity_id": entity_id,
        "recorded_at": recorded_at,
        "asserted_by": asserted_by,
        "origin": origin,
        "source_refs": [],
        "payload": payload,
        "extensions": {}
    })
}

pub(crate) fn actor_record(
    entity_id: &str,
    asserted_by: &str,
    recorded_at: &str,
    actor_kind: &str,
    display_name: &str,
) -> JsonValue {
    envelope(
        "actor",
        entity_id,
        recorded_at,
        asserted_by,
        "self_declared",
        json!({
            "actor_kind": actor_kind,
            "display_name": display_name
        }),
    )
}

pub(crate) fn ai_actor_record(entity_id: &str, asserted_by: &str, recorded_at: &str) -> JsonValue {
    envelope(
        "actor",
        entity_id,
        recorded_at,
        asserted_by,
        "tool_recorded",
        json!({
            "actor_kind": "ai_agent",
            "display_name": "Local creator integration",
            "ai_profile": {
                "provider": "local",
                "model_id": "creator-supplied-output",
                "model_version": "stage0-pilot-v1",
                "capabilities": canonical_set(vec![json!("propose_branch"), json!("read_context")])
            },
            "description": "Records a caller-supplied AI output through the authenticated Pilot boundary."
        }),
    )
}

pub(crate) fn observation_tool_actor_record(
    entity_id: &str,
    asserted_by: &str,
    recorded_at: &str,
) -> JsonValue {
    envelope(
        "actor",
        entity_id,
        recorded_at,
        asserted_by,
        "tool_recorded",
        json!({
            "actor_kind": "software_tool",
            "display_name": "SynapseGit byte-identity adapter",
            "description": "Compares verified primary-media Blob OIDs without decoding media or inferring visual or physical change."
        }),
    )
}

pub(crate) fn policy_record(
    entity_id: &str,
    creator_id: &str,
    project_id: &str,
    decision_ref: &str,
    proposal_ref: &str,
    recorded_at: &str,
) -> JsonValue {
    envelope(
        "policy",
        entity_id,
        recorded_at,
        creator_id,
        "self_declared",
        json!({
            "scope_refs": canonical_set(vec![json!(project_id)]),
            "rules": [
                {
                    "rule_id": "allow-context-read",
                    "effect": "allow",
                    "action": "read",
                    "resource_selector": "project/**"
                },
                {
                    "rule_id": "allow-session-proposal",
                    "effect": "allow",
                    "action": "propose",
                    "resource_selector": proposal_ref
                },
                {
                    "rule_id": "gate-session-decision",
                    "effect": "require_human_gate",
                    "action": "publish",
                    "resource_selector": decision_ref,
                    "human_gate": "before_decision_ref"
                }
            ],
            "default_effect": "deny",
            "notes": "Local single-creator Stage 0 Pilot policy."
        }),
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn grant_record(
    entity_id: &str,
    creator_id: &str,
    agent_id: &str,
    project_id: &str,
    proposal_ref: &str,
    recorded_at: &str,
    active_at: &str,
    expires_at: &str,
) -> JsonValue {
    let mut record = envelope(
        "delegation_grant",
        entity_id,
        recorded_at,
        creator_id,
        "self_declared",
        json!({
            "principal_ref": creator_id,
            "delegate_ref": agent_id,
            "project_ref": project_id,
            "purpose": "Record one bounded creator-facing AI proposal.",
            "capabilities": canonical_set(vec![json!("propose_branch"), json!("read_context")]),
            "resource_selectors": canonical_set(vec![json!("project/**")]),
            "writable_ref_prefixes": canonical_set(vec![json!(proposal_ref)]),
            "data_classes": canonical_set(vec![json!("internal")]),
            "allowed_egress": [],
            "may_delegate": false,
            "max_child_depth": 0,
            "max_output_bytes": PILOT_MAX_OUTPUT_BYTES,
            "required_human_gates": canonical_set(vec![json!("before_decision_ref"), json!("before_release_ref")]),
            "expires_at": expires_at
        }),
    );
    record
        .as_object_mut()
        .expect("record envelope is an object")
        .insert(
            "valid_time".into(),
            json!({ "kind": "instant", "at": active_at }),
        );
    record
}

pub(crate) fn subject_record(
    session: &str,
    ids: &SessionIds,
    capture_profile_id: &str,
    recorded_at: &str,
    label: &str,
) -> JsonValue {
    let mut record = envelope(
        "subject",
        &ids.subject,
        recorded_at,
        &ids.creator,
        "self_declared",
        json!({
            "subject_kind": "hybrid",
            "label": label,
            "description": "Creator subject tracked by the local Stage 0 Pilot.",
            "relation_refs": [],
            "spatial_frame_refs": []
        }),
    );
    record
        .get_mut("extensions")
        .and_then(JsonValue::as_object_mut)
        .expect("record extensions is an object")
        .insert(
            "org.synapsegit.creator-session".into(),
            json!({
                "format": "synapsegit-creator-session-v1",
                "session": session,
                "project_id": ids.project,
                "creator_id": ids.creator,
                "agent_id": ids.agent,
                "subject_id": ids.subject,
                "series_id": ids.series,
                "original_observation_id": ids.original_observation,
                "current_observation_id": ids.current_observation,
                "import_activity_id": ids.import_activity,
                "capture_profile_id": capture_profile_id,
                "policy_id": ids.policy,
                "grant_id": ids.grant,
                "context_id": ids.context,
                "ai_activity_id": ids.ai_activity,
                "feedback_id": ids.feedback
            }),
        );
    record
}

pub(crate) fn imported_capture_profile_record(
    entity_id: &str,
    creator_id: &str,
    recorded_at: &str,
) -> JsonValue {
    envelope(
        "capture_profile",
        entity_id,
        recorded_at,
        creator_id,
        "tool_recorded",
        json!({
            "profile_level": "imported",
            "required_conditions": [],
            "allowed_claims": ["reference_only"],
            "description": "Imported files with no verified station, calibration, viewpoint, lighting, or capture-time assurances."
        }),
    )
}

pub(crate) fn observation_record(
    entity_id: &str,
    creator_id: &str,
    subject_id: &str,
    series_id: &str,
    timestamp: &str,
    image_oid: &str,
    capture_profile_oid: &str,
) -> JsonValue {
    envelope(
        "observation",
        entity_id,
        timestamp,
        creator_id,
        "imported",
        json!({
            "subject_ref": subject_id,
            "series_ref": series_id,
            "capture_time": {
                "kind": "unknown",
                "reason": "Imported file; capture time was not supplied or independently verified."
            },
            "capture_profile_ref": capture_profile_oid,
            "media_refs": canonical_set(vec![json!({ "role": "primary", "oid": image_oid })]),
            "calibration_refs": [],
            "protocol_deviations": ["Capture time and capture metadata were not supplied or independently verified."],
            "environment_refs": [],
            "missing_regions": []
        }),
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn import_activity_record(
    entity_id: &str,
    creator_id: &str,
    subject_id: &str,
    timestamp: &str,
    original_blob: &str,
    current_blob: &str,
) -> JsonValue {
    let mut record = envelope(
        "activity",
        entity_id,
        timestamp,
        creator_id,
        "tool_recorded",
        json!({
            "activity_kind": "import",
            "actor_refs": canonical_set(vec![json!({ "role": "creator", "actor_ref": creator_id })]),
            "subject_refs": canonical_set(vec![json!(subject_id)]),
            "input_refs": [],
            "output_refs": canonical_set(vec![
                json!({ "role": "current", "oid": current_blob }),
                json!({ "role": "original", "oid": original_blob })
            ]),
            "before_observation_refs": [],
            "after_observation_refs": [],
            "reversibility": "reversible",
            "summary": "Imported original and current images without interpreting their pixels.",
            "side_effect_class": "project_internal"
        }),
    );
    record
        .as_object_mut()
        .expect("record envelope is an object")
        .insert(
            "valid_time".into(),
            json!({
                "kind": "unknown",
                "reason": "The local Pilot did not receive an external import-event timestamp."
            }),
        );
    record
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn context_record(
    entity_id: &str,
    creator_id: &str,
    subject_id: &str,
    base_head: &str,
    decision_ref: &str,
    policy_oid: &str,
    grant_oid: &str,
    recorded_at: &str,
) -> JsonValue {
    envelope(
        "context_pack",
        entity_id,
        recorded_at,
        creator_id,
        "tool_recorded",
        json!({
            "base_commit": base_head,
            "base_ref_name": decision_ref,
            "expected_ref_head": base_head,
            "subject_refs": canonical_set(vec![json!(subject_id)]),
            "selected_context_refs": canonical_set(vec![json!(base_head)]),
            "must_preserve_constraints": ["Preserve creator ownership of the canonical decision Ref."],
            "allowed_transformations": canonical_set(vec![json!("image_proposal")]),
            "unresolved_questions": [],
            "policy_snapshot_ref": policy_oid,
            "delegation_grant_ref": grant_oid,
            "data_classification": "internal",
            "retrieval_method": "creator session base Commit"
        }),
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn ai_activity_record(
    entity_id: &str,
    agent_id: &str,
    creator_id: &str,
    subject_id: &str,
    timestamp: &str,
    context_oid: &str,
    grant_oid: &str,
    current_blob: &str,
    output_blob: &str,
) -> JsonValue {
    let mut record = envelope(
        "activity",
        entity_id,
        timestamp,
        agent_id,
        "tool_recorded",
        json!({
            "activity_kind": "ai_run",
            "actor_refs": canonical_set(vec![
                json!({ "role": "agent", "actor_ref": agent_id }),
                json!({ "role": "responsible_principal", "actor_ref": creator_id })
            ]),
            "subject_refs": canonical_set(vec![json!(subject_id)]),
            "input_refs": canonical_set(vec![
                json!({ "role": "context", "oid": context_oid }),
                json!({ "role": "source_image", "oid": current_blob })
            ]),
            "output_refs": canonical_set(vec![json!({ "role": "proposal", "oid": output_blob })]),
            "before_observation_refs": [],
            "after_observation_refs": [],
            "reversibility": "reversible",
            "summary": "Recorded a creator-supplied AI image proposal.",
            "side_effect_class": "none",
            "ai_run": {
                "agent_ref": agent_id,
                "responsible_principal_ref": creator_id,
                "context_pack_ref": context_oid,
                "delegation_grant_ref": grant_oid,
                "requested_capabilities": canonical_set(vec![json!("propose_branch"), json!("read_context")]),
                "required_human_gates": canonical_set(vec![json!("before_decision_ref"), json!("before_release_ref")]),
                "status": "proposal_ready",
                "reproducibility_class": "not_reproducible"
            }
        }),
    );
    record
        .as_object_mut()
        .expect("record envelope is an object")
        .insert(
            "valid_time".into(),
            json!({
                "kind": "unknown",
                "reason": "The caller-supplied AI output had no independently verified execution timestamp."
            }),
        );
    record
}

pub(crate) fn feedback_record(
    entity_id: &str,
    creator_id: &str,
    subject_id: &str,
    proposal_head: &str,
    disposition: CreatorDisposition,
    rationale: &str,
    recorded_at: &str,
) -> JsonValue {
    envelope(
        "decision_feedback",
        entity_id,
        recorded_at,
        creator_id,
        "self_declared",
        json!({
            "proposal_ref": proposal_head,
            "disposition": disposition.as_protocol_str(),
            "reason_codes": canonical_set(vec![json!(disposition.reason_code())]),
            "human_rationale": rationale,
            "applies_to_subjects": canonical_set(vec![json!(subject_id)]),
            "visibility": "private",
            "training_use_policy": "prohibited"
        }),
    )
}

pub(crate) fn manifest_tree(entries: JsonMap<String, JsonValue>) -> JsonValue {
    json!({
        "object_type": "tree",
        "schema_version": SCHEMA_VERSION,
        "entries": entries,
        "extensions": {}
    })
}

pub(crate) fn commit(
    kind: &str,
    parents: &[String],
    snapshot: &str,
    transitions: &[String],
    author: &str,
    authored_at: &str,
    message: &str,
) -> JsonValue {
    json!({
        "object_type": "commit",
        "schema_version": SCHEMA_VERSION,
        "commit_kind": kind,
        "parents": parents,
        "snapshot": snapshot,
        "transition_refs": canonical_set(transitions.iter().map(|value| json!(value)).collect()),
        "bound_declaration_refs": [],
        "author_ref": author,
        "authored_at": authored_at,
        "message": message,
        "extensions": {}
    })
}
