use crate::io::put_json;
use crate::records::{actor_record, manifest_tree};
use crate::report::{
    creator_report_from_snapshot_with_limits, load_base_snapshot_pointers,
    validate_byte_identity_metric,
};
use crate::session::{
    COMPARISON_ANALYSIS_ENTRY, COMPARISON_CONFIGURATION_ENTRY, COMPARISON_IMPLEMENTATION_ENTRY,
    COMPARISON_TOOL_ENTRY, CREATOR_BEGIN_RESERVE, CREATOR_DECISION_RESERVE, CREATOR_FSCK_LIMITS,
    CREATOR_PENDING_DECISION_POOL_RESERVE, SessionIds, begin_creator_session,
    begin_creator_session_with_limits, decide_creator_session, decide_creator_session_with_limits,
    entity_id, insert_entry,
};
use crate::time::{civil_from_days, format_timestamp};
use crate::{
    CreatorBeginOptions, CreatorDecisionOptions, CreatorDisposition, CreatorPendingDecisionState,
};
use serde_json::{Map as JsonMap, json};
use std::fs;
use std::io::Cursor;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use synapse_core::Repository;

static NEXT_RESOURCE_TEST: AtomicU64 = AtomicU64::new(0);

fn resource_test_path() -> PathBuf {
    let sequence = NEXT_RESOURCE_TEST.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "synapse-creator-resource-test-{}-{sequence}",
        std::process::id()
    ))
}

#[test]
fn timestamp_conversion_matches_epoch_and_leap_day() {
    assert_eq!(civil_from_days(0), (1970, 1, 1));
    assert_eq!(
        format_timestamp(0, 0).unwrap(),
        "1970-01-01T00:00:00.000000000Z"
    );
    assert_eq!(
        format_timestamp(951_782_400, 123).unwrap(),
        "2000-02-29T00:00:00.000000123Z"
    );
}

#[test]
fn entity_ids_are_stable_uuid_v4_values() {
    let seed = [7; 32];
    let first = entity_id(&seed, "subject");
    assert_eq!(first, entity_id(&seed, "subject"));
    assert_ne!(first, entity_id(&seed, "creator"));
    assert!(first.starts_with("urn:uuid:"));
    assert_eq!(first.as_bytes()[23], b'4');
    assert!(matches!(first.as_bytes()[28], b'8' | b'9' | b'a' | b'b'));

    assert_ne!(
        SessionIds::fresh().unwrap().subject,
        SessionIds::fresh().unwrap().subject
    );
}

#[test]
fn comparison_tree_entries_are_optional_only_as_a_complete_legacy_set() {
    let path = std::env::temp_dir().join(format!(
        "synapse-creator-comparison-tree-test-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&path);
    let repository = Repository::open(&path).unwrap();
    let ids = SessionIds::from_seed(&[9; 32]);
    let record_oid = put_json(
        &repository,
        actor_record(
            &ids.creator,
            &ids.creator,
            "2026-07-13T00:00:00.000000000Z",
            "human",
            "Test creator",
        ),
    )
    .unwrap();
    let implementation_oid = repository
        .put_blob(Cursor::new(b"implementation"))
        .unwrap()
        .oid;
    let configuration_oid = repository
        .put_blob(Cursor::new(b"configuration"))
        .unwrap()
        .oid;
    let mut entries = JsonMap::new();
    insert_entry(
        &mut entries,
        "image-import.activity.json",
        "record",
        &record_oid,
    );
    let legacy_tree = put_json(&repository, manifest_tree(entries.clone())).unwrap();
    assert!(
        load_base_snapshot_pointers(&repository, &legacy_tree)
            .unwrap()
            .comparison
            .is_none()
    );

    insert_entry(
        &mut entries,
        COMPARISON_ANALYSIS_ENTRY,
        "record",
        &record_oid,
    );
    let partial_tree = put_json(&repository, manifest_tree(entries.clone())).unwrap();
    assert!(load_base_snapshot_pointers(&repository, &partial_tree).is_err());

    insert_entry(&mut entries, COMPARISON_TOOL_ENTRY, "record", &record_oid);
    insert_entry(
        &mut entries,
        COMPARISON_IMPLEMENTATION_ENTRY,
        "blob",
        &implementation_oid,
    );
    insert_entry(
        &mut entries,
        COMPARISON_CONFIGURATION_ENTRY,
        "blob",
        &configuration_oid,
    );
    let complete_tree = put_json(&repository, manifest_tree(entries)).unwrap();
    let pointers = load_base_snapshot_pointers(&repository, &complete_tree)
        .unwrap()
        .comparison
        .unwrap();
    assert_eq!(pointers.analysis_oid, record_oid);
    assert_eq!(pointers.implementation_oid, implementation_oid);
    assert_eq!(pointers.configuration_oid, configuration_oid);
    drop(repository);
    std::fs::remove_dir_all(path).unwrap();
}

#[test]
fn byte_identity_metric_rejects_extra_semantic_claims() {
    let mut payload = json!({
        "metrics": {
            "byte_identical": {
                "mantissa": "1",
                "scale": 0,
                "unit": "unitless"
            }
        }
    });
    validate_byte_identity_metric(&payload, true).unwrap();
    payload
        .get_mut("metrics")
        .and_then(|metrics| metrics.as_object_mut())
        .unwrap()
        .insert(
            "physical_change".into(),
            json!({ "mantissa": "1", "scale": 0, "unit": "unitless" }),
        );
    assert!(validate_byte_identity_metric(&payload, true).is_err());
}

#[test]
fn creator_begin_decision_and_report_fail_closed_at_integrity_limits() {
    let root = resource_test_path();
    fs::create_dir(&root).unwrap();
    let repository_path = root.join("repo");
    let repository = Repository::open(&repository_path).unwrap();
    repository
        .put_blob(Cursor::new(b"preexisting repository object"))
        .unwrap();
    drop(repository);

    let original = root.join("original.png");
    let current = root.join("current.png");
    let ai_output = root.join("ai-output.png");
    fs::write(&original, b"original image").unwrap();
    fs::write(&current, b"current image").unwrap();
    fs::write(&ai_output, b"AI output image").unwrap();
    let options = CreatorBeginOptions {
        repository: repository_path.clone(),
        session: "bounded-integrity".into(),
        original_image: original,
        current_image: current,
        ai_output,
        subject_label: "Bounded subject".into(),
        creator_name: "Bounded creator".into(),
    };

    let mut begin_limits = CREATOR_FSCK_LIMITS;
    begin_limits.max_object_bytes = 1;
    let error = begin_creator_session_with_limits(&options, begin_limits).unwrap_err();
    assert_eq!(error.code(), "resource_limit");
    assert!(
        Repository::open(&repository_path)
            .unwrap()
            .refs()
            .snapshot()
            .unwrap()
            .is_empty(),
        "begin limit failure must precede publication"
    );

    let mut pending = begin_creator_session(&options).unwrap();
    let repository = Repository::open(&repository_path).unwrap();
    let before_second_begin_refs = repository.refs().snapshot().unwrap();
    let before_second_begin_objects = repository.objects().list_oids().unwrap();
    let ready_record_oids = before_second_begin_objects
        .iter()
        .filter(|oid| oid.starts_with("record:"))
        .collect::<Vec<_>>();
    let ready_record_bytes = ready_record_oids
        .iter()
        .map(|oid| {
            repository
                .objects()
                .stored_object_byte_len(oid)
                .unwrap()
                .unwrap()
        })
        .sum::<u64>();
    let ready_fsck = repository.fsck_with_limits(CREATOR_FSCK_LIMITS).unwrap();
    let ready_nodes = ready_fsck
        .closures
        .iter()
        .map(|closure| closure.nodes.len())
        .sum::<usize>();
    let ready_edges = ready_fsck
        .closures
        .iter()
        .map(|closure| closure.edges.len())
        .sum::<usize>();
    assert!(
        before_second_begin_objects.len() - 1 <= CREATOR_BEGIN_RESERVE.objects,
        "creator begin object growth exceeded its fixed reservation"
    );
    assert!(ready_nodes <= CREATOR_BEGIN_RESERVE.closure_nodes);
    assert!(ready_edges <= CREATOR_BEGIN_RESERVE.closure_edges);
    assert!(ready_record_oids.len() <= CREATOR_BEGIN_RESERVE.tombstone_records);
    assert!(ready_record_bytes <= CREATOR_BEGIN_RESERVE.tombstone_bytes);
    drop(repository);
    let mut second_options = options.clone();
    second_options.session = "bounded-headroom".into();
    let mut ref_limits = CREATOR_FSCK_LIMITS;
    ref_limits.max_ref_roots = before_second_begin_refs.len() + 1;
    let error = begin_creator_session_with_limits(&second_options, ref_limits).unwrap_err();
    assert_eq!(error.code(), "resource_limit");
    let repository = Repository::open(&repository_path).unwrap();
    assert_eq!(
        repository.refs().snapshot().unwrap(),
        before_second_begin_refs
    );
    assert_eq!(
        repository.objects().list_oids().unwrap(),
        before_second_begin_objects,
        "begin Ref headroom failure must precede CAS writes"
    );
    drop(repository);

    let mut record_options = options.clone();
    record_options.session = "bounded-record-headroom".into();
    let mut record_limits = CREATOR_FSCK_LIMITS;
    record_limits.tombstone_scan.max_record_bytes = ready_record_bytes
        + CREATOR_BEGIN_RESERVE.tombstone_bytes
        + CREATOR_PENDING_DECISION_POOL_RESERVE.tombstone_bytes
        - 1;
    let error = begin_creator_session_with_limits(&record_options, record_limits).unwrap_err();
    assert_eq!(error.code(), "resource_limit");
    assert_eq!(
        Repository::open(&repository_path)
            .unwrap()
            .objects()
            .list_oids()
            .unwrap(),
        before_second_begin_objects,
        "begin Record-byte headroom failure must precede CAS writes"
    );

    let mut decision_limits = CREATOR_FSCK_LIMITS;
    decision_limits.max_objects = 1;
    let error = decide_creator_session_with_limits(
        &mut pending,
        &CreatorDecisionOptions {
            disposition: CreatorDisposition::Adopt,
            rationale: Some("Bounded review".into()),
        },
        decision_limits,
    )
    .unwrap_err();
    assert_eq!(error.code(), "resource_limit");
    assert_eq!(
        pending.decision_state(),
        CreatorPendingDecisionState::Ready,
        "decision capacity failure must precede publication"
    );
    assert!(pending.completed_receipt().is_none());

    let repository = Repository::open(&repository_path).unwrap();
    let snapshot = repository.refs().snapshot().unwrap();
    let error = creator_report_from_snapshot_with_limits(
        &repository,
        &snapshot,
        "bounded-integrity",
        decision_limits,
    )
    .unwrap_err();
    assert_eq!(error.code(), "resource_limit");
    drop(repository);

    decide_creator_session(
        &mut pending,
        &CreatorDecisionOptions {
            disposition: CreatorDisposition::Adopt,
            rationale: Some("Bounded review retry".into()),
        },
    )
    .unwrap();
    let repository = Repository::open(&repository_path).unwrap();
    let completed_objects = repository.objects().list_oids().unwrap();
    let completed_record_oids = completed_objects
        .iter()
        .filter(|oid| oid.starts_with("record:"))
        .collect::<Vec<_>>();
    let completed_record_bytes = completed_record_oids
        .iter()
        .map(|oid| {
            repository
                .objects()
                .stored_object_byte_len(oid)
                .unwrap()
                .unwrap()
        })
        .sum::<u64>();
    let completed_fsck = repository.fsck_with_limits(CREATOR_FSCK_LIMITS).unwrap();
    let completed_nodes = completed_fsck
        .closures
        .iter()
        .map(|closure| closure.nodes.len())
        .sum::<usize>();
    let completed_edges = completed_fsck
        .closures
        .iter()
        .map(|closure| closure.edges.len())
        .sum::<usize>();
    assert!(
        completed_objects.len() - before_second_begin_objects.len()
            <= CREATOR_DECISION_RESERVE.objects
    );
    assert!(completed_nodes - ready_nodes <= CREATOR_DECISION_RESERVE.closure_nodes);
    assert!(completed_edges - ready_edges <= CREATOR_DECISION_RESERVE.closure_edges);
    assert!(
        completed_record_oids.len() - ready_record_oids.len()
            <= CREATOR_DECISION_RESERVE.tombstone_records
    );
    assert!(
        completed_record_bytes - ready_record_bytes <= CREATOR_DECISION_RESERVE.tombstone_bytes
    );
    drop(repository);
    drop(pending);
    fs::remove_dir_all(root).unwrap();
}
