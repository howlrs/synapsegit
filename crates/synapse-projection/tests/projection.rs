use serde_json::{Map as JsonMap, Value as JsonValue, json};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use synapse_canonical::{ObjectKind, parse_strict};
use synapse_cas::{FileObjectStore, GraphLimits, TombstoneScanLimits};
use synapse_projection::{
    AdapterDeterminism, AnalysisMaskRole, AnalysisReplayReadiness, DependencyTargetKind,
    ObjectAvailability, ObservationDependencyKind, ProjectionError, ProjectionLimits, RefScope,
    SqliteProjectionStore, TimelineRecordKind, TimelineTimeBasis,
};
use synapse_schema::validate;
use synapse_sqlite::{RefRecord, RefSnapshot};

const HUMAN_ID: &str = "urn:uuid:20000000-0000-4000-8000-000000000001";
const SUBJECT_ID: &str = "urn:uuid:20000000-0000-4000-8000-000000000010";
const SERIES_A: &str = "urn:uuid:20000000-0000-4000-8000-000000000020";
const SERIES_B: &str = "urn:uuid:20000000-0000-4000-8000-000000000021";
const STATION_ID: &str = "urn:uuid:20000000-0000-4000-8000-000000000030";
const RECORDED_AT: &str = "2026-07-12T00:00:00.000000000Z";

static NEXT_DIRECTORY_ID: AtomicU64 = AtomicU64::new(1);

struct TempDirectory(PathBuf);

impl TempDirectory {
    fn new(label: &str) -> Self {
        loop {
            let id = NEXT_DIRECTORY_ID.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "synapse-projection-{label}-{}-{id}",
                std::process::id()
            ));
            match fs::create_dir(&path) {
                Ok(()) => return Self(path),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => panic!("create {}: {error}", path.display()),
            }
        }
    }

    fn join(&self, path: impl AsRef<Path>) -> PathBuf {
        self.0.join(path)
    }
}

impl Drop for TempDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct TestRepository {
    temporary: TempDirectory,
    objects: FileObjectStore,
}

impl TestRepository {
    fn new(label: &str) -> Self {
        let temporary = TempDirectory::new(label);
        let objects = FileObjectStore::open(temporary.join("objects")).unwrap();
        Self { temporary, objects }
    }

    fn projection_path(&self) -> PathBuf {
        self.temporary.join("projection.sqlite3")
    }
}

fn put_json(store: &FileObjectStore, value: JsonValue) -> String {
    let bytes = serde_json::to_vec(&value).unwrap();
    let parsed = parse_strict(&bytes).unwrap();
    validate(&parsed).unwrap_or_else(|error| panic!("invalid fixture: {error}\n{value:#}"));
    store.put_structured_unchecked(&bytes).unwrap().oid
}

fn put_unchecked(store: &FileObjectStore, value: JsonValue) -> String {
    store
        .put_structured_unchecked(&serde_json::to_vec(&value).unwrap())
        .unwrap()
        .oid
}

fn put_blob(store: &FileObjectStore, bytes: &[u8]) -> String {
    store.put_blob(bytes).unwrap().oid
}

fn fake_oid(kind: &str, digit: char) -> String {
    format!("{kind}:sg-oid-v1:sha256:{}", digit.to_string().repeat(64))
}

fn indexed_fake_oid(kind: &str, index: u64) -> String {
    format!("{kind}:sg-oid-v1:sha256:{index:064x}")
}

fn indexed_entity_id(index: u64) -> String {
    format!("urn:uuid:30000000-0000-4000-8000-{index:012x}")
}

fn object_path(root: &Path, oid: &str) -> PathBuf {
    let family = oid.split(':').next().unwrap();
    let digest = oid.rsplit(':').next().unwrap();
    root.join("objects")
        .join(family)
        .join(&digest[..2])
        .join(&digest[2..])
}

fn entry_kind(oid: &str) -> &str {
    oid.split(':').next().unwrap()
}

fn put_tree(store: &FileObjectStore, entries: &[(&str, &str)]) -> String {
    let mut tree_entries = JsonMap::new();
    for (name, oid) in entries {
        tree_entries.insert(
            (*name).to_owned(),
            json!({ "entry_kind": entry_kind(oid), "oid": oid }),
        );
    }
    put_json(
        store,
        json!({
            "object_type": "tree",
            "schema_version": "0.1.0",
            "entries": tree_entries,
            "extensions": {}
        }),
    )
}

fn put_tree_with_declared_kind(
    store: &FileObjectStore,
    name: &str,
    declared_kind: &str,
    oid: &str,
) -> String {
    let mut entries = JsonMap::new();
    entries.insert(
        name.to_owned(),
        json!({ "entry_kind": declared_kind, "oid": oid }),
    );
    put_unchecked(
        store,
        json!({
            "object_type": "tree",
            "schema_version": "0.1.0",
            "entries": entries,
            "extensions": {}
        }),
    )
}

fn put_commit(store: &FileObjectStore, tree_oid: &str, transitions: &[String]) -> String {
    let mut transitions = transitions.to_vec();
    transitions.sort();
    put_json(
        store,
        json!({
            "object_type": "commit",
            "schema_version": "0.1.0",
            "commit_kind": "checkpoint",
            "parents": [],
            "snapshot": tree_oid,
            "transition_refs": transitions,
            "bound_declaration_refs": [],
            "author_ref": HUMAN_ID,
            "authored_at": RECORDED_AT,
            "message": "projection fixture",
            "extensions": {}
        }),
    )
}

fn put_head(
    store: &FileObjectStore,
    entries: &[(&str, &str)],
    transitions: &[String],
) -> (String, String) {
    let tree = put_tree(store, entries);
    let commit = put_commit(store, &tree, transitions);
    (tree, commit)
}

fn snapshot(refs: &[(&str, &str, i64)]) -> RefSnapshot {
    RefSnapshot {
        refs: refs
            .iter()
            .map(|(name, head, updated_event_id)| RefRecord {
                name: (*name).to_owned(),
                head: (*head).to_owned(),
                updated_event_id: *updated_event_id,
            })
            .collect(),
    }
}

fn subject_record(entity_id: &str, label: &str) -> JsonValue {
    json!({
        "object_type": "record",
        "schema_version": "0.1.0",
        "record_type": "subject",
        "entity_id": entity_id,
        "recorded_at": RECORDED_AT,
        "asserted_by": HUMAN_ID,
        "origin": "self_declared",
        "source_refs": [],
        "payload": {
            "subject_kind": "hybrid",
            "label": label,
            "relation_refs": [],
            "spatial_frame_refs": []
        },
        "extensions": {}
    })
}

fn capture_profile_record(entity_id: &str) -> JsonValue {
    json!({
        "object_type": "record",
        "schema_version": "0.1.0",
        "record_type": "capture_profile",
        "entity_id": entity_id,
        "recorded_at": RECORDED_AT,
        "asserted_by": HUMAN_ID,
        "origin": "self_declared",
        "source_refs": [],
        "payload": {
            "profile_level": "repeatable",
            "required_conditions": ["station"],
            "allowed_claims": ["reference_only"],
            "description": "Projection dependency fixture"
        },
        "extensions": {}
    })
}

fn claim_record(entity_id: &str, subject_id: &str) -> JsonValue {
    json!({
        "object_type": "record",
        "schema_version": "0.1.0",
        "record_type": "claim",
        "entity_id": entity_id,
        "recorded_at": RECORDED_AT,
        "asserted_by": HUMAN_ID,
        "origin": "self_declared",
        "source_refs": [],
        "payload": {
            "claim_kind": "interpretation",
            "epistemic_class": "declared",
            "subject_refs": [subject_id],
            "predicate": "projection_fixture",
            "value_text": "Fixture dependency record.",
            "evidence_refs": []
        },
        "extensions": {}
    })
}

fn analysis_result_record(
    entity_id: &str,
    inputs: &[(&str, &str)],
    implementation_oid: &str,
    configuration_oid: &str,
    transform_oids: &[String],
    derived_blob_oids: &[String],
    masks: &[(&str, &str)],
) -> JsonValue {
    let inputs = inputs
        .iter()
        .map(|(role, oid)| json!({ "role": role, "ref": oid }))
        .collect::<Vec<_>>();
    let mut transform_oids = transform_oids.to_vec();
    transform_oids.sort();
    let mut derived_blob_oids = derived_blob_oids.to_vec();
    derived_blob_oids.sort();
    let mut mask_refs = JsonMap::new();
    for (role, oid) in masks {
        mask_refs.insert((*role).to_owned(), json!(oid));
    }
    json!({
        "object_type": "record",
        "schema_version": "0.1.0",
        "record_type": "analysis_result",
        "entity_id": entity_id,
        "recorded_at": "2026-07-12T05:00:00.000000000Z",
        "asserted_by": HUMAN_ID,
        "origin": "inferred",
        "source_refs": [],
        "payload": {
            "analysis_kind": "projection_lineage_fixture",
            "comparison_kind": "temporal_observation",
            "inputs": inputs,
            "adapter": {
                "id": "fixture.analysis-adapter",
                "version": "2.1",
                "implementation_digest": implementation_oid,
                "configuration_digest": configuration_oid,
                "determinism": "seeded",
                "seed": "fixture-seed-42"
            },
            "status": "succeeded",
            "comparability": "comparable",
            "reason_codes": [],
            "transform_refs": transform_oids,
            "derived_blob_refs": derived_blob_oids,
            "mask_refs": mask_refs,
            "warnings": [],
            "limitations": []
        },
        "extensions": {}
    })
}

fn observation_record(
    entity_id: &str,
    subject_id: &str,
    series_id: &str,
    capture_time: JsonValue,
    recorded_at: &str,
    media_oid: &str,
) -> JsonValue {
    json!({
        "object_type": "record",
        "schema_version": "0.1.0",
        "record_type": "observation",
        "entity_id": entity_id,
        "recorded_at": recorded_at,
        "asserted_by": HUMAN_ID,
        "origin": "device_recorded",
        "source_refs": [],
        "payload": {
            "subject_ref": subject_id,
            "series_ref": series_id,
            "capture_time": capture_time,
            "media_refs": [{ "role": "primary", "oid": media_oid }],
            "calibration_refs": [],
            "protocol_deviations": [],
            "environment_refs": [],
            "missing_regions": []
        },
        "extensions": {}
    })
}

fn activity_record(
    entity_id: &str,
    subject_ids: &[&str],
    valid_time: JsonValue,
    recorded_at: &str,
) -> JsonValue {
    json!({
        "object_type": "record",
        "schema_version": "0.1.0",
        "record_type": "activity",
        "entity_id": entity_id,
        "valid_time": valid_time,
        "recorded_at": recorded_at,
        "asserted_by": HUMAN_ID,
        "origin": "tool_recorded",
        "source_refs": [],
        "payload": {
            "activity_kind": "review",
            "actor_refs": [{ "role": "reviewer", "actor_ref": HUMAN_ID }],
            "subject_refs": subject_ids,
            "input_refs": [],
            "output_refs": [],
            "reversibility": "reversible",
            "side_effect_class": "none"
        },
        "extensions": {}
    })
}

fn tombstone_record(entity_id: &str, target_oid: &str) -> JsonValue {
    json!({
        "object_type": "record",
        "schema_version": "0.1.0",
        "record_type": "tombstone",
        "entity_id": entity_id,
        "recorded_at": RECORDED_AT,
        "asserted_by": HUMAN_ID,
        "origin": "self_declared",
        "source_refs": [],
        "payload": {
            "target_ref": target_oid,
            "erasure_kind": "withheld",
            "reason_code": "project_policy",
            "acted_at": RECORDED_AT,
            "affected_derivative_refs": []
        },
        "extensions": {}
    })
}

fn assert_error(error: ProjectionError, expected_code: &str, expected_text: &str) {
    assert_eq!(error.code(), expected_code, "unexpected error: {error}");
    assert!(
        error.to_string().contains(expected_text),
        "unexpected error: {error}"
    );
}

#[test]
fn multi_ref_scope_excludes_orphans_and_rebuild_removes_stale_rows() {
    let repository = TestRepository::new("multi-ref");
    let media = put_blob(&repository.objects, b"shared observation media");
    let shared = put_json(
        &repository.objects,
        observation_record(
            "urn:uuid:20000000-0000-4000-8000-000000000101",
            SUBJECT_ID,
            SERIES_A,
            json!({ "kind": "instant", "at": "2026-07-12T00:01:00.000000000Z" }),
            "2026-07-12T00:01:01.000000000Z",
            &media,
        ),
    );
    let only_a = put_json(
        &repository.objects,
        observation_record(
            "urn:uuid:20000000-0000-4000-8000-000000000102",
            SUBJECT_ID,
            SERIES_A,
            json!({ "kind": "instant", "at": "2026-07-12T00:02:00.000000000Z" }),
            "2026-07-12T00:02:01.000000000Z",
            &media,
        ),
    );
    let only_b = put_json(
        &repository.objects,
        observation_record(
            "urn:uuid:20000000-0000-4000-8000-000000000103",
            SUBJECT_ID,
            SERIES_B,
            json!({ "kind": "instant", "at": "2026-07-12T00:03:00.000000000Z" }),
            "2026-07-12T00:03:01.000000000Z",
            &media,
        ),
    );
    let orphan = put_json(
        &repository.objects,
        subject_record(
            "urn:uuid:20000000-0000-4000-8000-000000000199",
            "Unreachable orphan",
        ),
    );
    let (_, head_a) = put_head(
        &repository.objects,
        &[
            ("media.bin", &media),
            ("only-a.json", &only_a),
            ("shared.json", &shared),
        ],
        &[],
    );
    let (_, head_b) = put_head(
        &repository.objects,
        &[
            ("media.bin", &media),
            ("only-b.json", &only_b),
            ("shared.json", &shared),
        ],
        &[],
    );
    let all_refs = snapshot(&[("observed/b", &head_b, 2), ("observed/a", &head_a, 1)]);
    let mut projection = SqliteProjectionStore::open(repository.projection_path()).unwrap();

    let first = projection
        .rebuild(&repository.objects, &all_refs, GraphLimits::default())
        .unwrap()
        .metadata;

    assert_eq!(first.ref_count, 2);
    assert!(projection.get_object(&orphan).unwrap().is_none());
    let all = projection
        .subject_timeline(SUBJECT_ID, None, &RefScope::All)
        .unwrap();
    assert_eq!(
        all.iter()
            .map(|entry| entry.oid.as_str())
            .collect::<Vec<_>>(),
        vec![shared.as_str(), only_a.as_str(), only_b.as_str()]
    );
    assert_eq!(
        all[0].reachable_from,
        vec!["observed/a".to_owned(), "observed/b".to_owned()]
    );

    let only_b_scope = projection
        .subject_timeline(
            SUBJECT_ID,
            None,
            &RefScope::names(["observed/b", "observed/b"]),
        )
        .unwrap();
    assert_eq!(
        only_b_scope
            .iter()
            .map(|entry| entry.oid.as_str())
            .collect::<Vec<_>>(),
        vec![shared.as_str(), only_b.as_str()]
    );
    assert!(
        only_b_scope
            .iter()
            .all(|entry| entry.reachable_from == ["observed/b".to_owned()])
    );
    assert_error(
        projection
            .closure_summaries(&RefScope::one("observed/not-there"))
            .unwrap_err(),
        "projection_ref_unknown",
        "not in the projection",
    );
    assert_error(
        projection
            .subject_timeline(SUBJECT_ID, None, &RefScope::one("observed/x' OR 1=1 --"))
            .unwrap_err(),
        "projection_source_invalid",
        "invalid query Ref name",
    );
    assert!(
        projection
            .subject_timeline("' OR 1=1 --", None, &RefScope::All)
            .unwrap()
            .is_empty()
    );

    let after_b_only = projection
        .rebuild(
            &repository.objects,
            &snapshot(&[("observed/b", &head_b, 2)]),
            GraphLimits::default(),
        )
        .unwrap()
        .metadata;
    assert_ne!(after_b_only.source_fingerprint, first.source_fingerprint);
    assert_eq!(after_b_only.ref_count, 1);
    assert!(projection.get_object(&only_a).unwrap().is_none());
    assert!(projection.get_object(&head_a).unwrap().is_none());
    assert!(projection.get_object(&shared).unwrap().is_some());
    assert_eq!(
        projection
            .closure_summaries(&RefScope::All)
            .unwrap()
            .into_iter()
            .map(|summary| summary.ref_name)
            .collect::<Vec<_>>(),
        vec!["observed/b"]
    );

    drop(projection);
    let reopened = SqliteProjectionStore::open(repository.projection_path()).unwrap();
    assert_eq!(reopened.metadata().unwrap(), Some(after_b_only));
    assert!(reopened.get_object(&only_b).unwrap().is_some());
}

#[test]
fn idempotent_rebuild_has_stable_fingerprint_and_deduplicated_objects_and_edges() {
    let repository = TestRepository::new("idempotence");
    let media = put_blob(&repository.objects, b"deduplicated bytes");
    let observation = put_json(
        &repository.objects,
        observation_record(
            "urn:uuid:20000000-0000-4000-8000-000000000201",
            SUBJECT_ID,
            SERIES_A,
            json!({ "kind": "instant", "at": "2026-07-12T01:00:00.000000000Z" }),
            "2026-07-12T01:00:01.000000000Z",
            &media,
        ),
    );
    let (tree, head) = put_head(
        &repository.objects,
        &[
            ("media-a.bin", &media),
            ("media-b.bin", &media),
            ("observation.json", &observation),
        ],
        std::slice::from_ref(&observation),
    );
    let refs = snapshot(&[
        ("observed/z-alias", &head, 8),
        ("observed/a-main", &head, 7),
    ]);
    let reordered_refs = snapshot(&[
        ("observed/a-main", &head, 7),
        ("observed/z-alias", &head, 8),
    ]);
    let mut projection = SqliteProjectionStore::open_in_memory().unwrap();

    let first = projection
        .rebuild(&repository.objects, &refs, GraphLimits::default())
        .unwrap()
        .metadata;
    let second = projection
        .rebuild(&repository.objects, &reordered_refs, GraphLimits::default())
        .unwrap()
        .metadata;
    let orphan = put_json(
        &repository.objects,
        subject_record(
            "urn:uuid:20000000-0000-4000-8000-000000000299",
            "Added after projection",
        ),
    );
    let after_orphan = projection
        .rebuild(&repository.objects, &refs, GraphLimits::default())
        .unwrap()
        .metadata;

    assert_eq!(first, second);
    assert_eq!(second, after_orphan);
    assert_eq!(first.object_count, 4);
    assert_eq!(first.edge_count, 6);
    assert!(projection.get_object(&tree).unwrap().is_some());
    assert!(projection.get_object(&media).unwrap().is_some());
    assert!(projection.get_object(&orphan).unwrap().is_none());
}

#[test]
fn tombstone_scan_exact_limits_exclude_orphans_and_failures_preserve_projection() {
    let repository = TestRepository::new("tombstone-scan-limits");
    let payload = put_blob(&repository.objects, b"stable bounded inventory projection");
    let (_, head) = put_head(&repository.objects, &[("payload.bin", &payload)], &[]);
    let refs = snapshot(&[("observed/tombstone-scan-limits", &head, 1)]);
    let projection_path = repository.projection_path();
    let mut projection = SqliteProjectionStore::open(&projection_path).unwrap();
    let baseline = projection
        .rebuild(&repository.objects, &refs, GraphLimits::default())
        .unwrap()
        .metadata;
    let baseline_summaries = projection.closure_summaries(&RefScope::All).unwrap();

    let mut orphan_records = Vec::new();
    for index in 1..=8 {
        orphan_records.push(put_json(
            &repository.objects,
            subject_record(
                &indexed_entity_id(0x300 + index),
                &format!("Unreachable subject {index}"),
            ),
        ));
    }
    for index in 1..=8 {
        orphan_records.push(put_json(
            &repository.objects,
            tombstone_record(
                &indexed_entity_id(0x400 + index),
                &indexed_fake_oid("blob", 0x1000 + index),
            ),
        ));
    }
    let total_record_bytes = orphan_records
        .iter()
        .map(|oid| {
            repository
                .objects
                .get_verified(oid)
                .unwrap()
                .unwrap()
                .byte_len()
        })
        .sum::<u64>();
    let exact_limits = ProjectionLimits {
        graph: GraphLimits::default(),
        tombstone_scan: TombstoneScanLimits {
            max_record_objects: orphan_records.len(),
            max_record_bytes: total_record_bytes,
        },
    };

    let exact = projection
        .rebuild_with_limits(&repository.objects, &refs, exact_limits)
        .unwrap()
        .metadata;
    assert_eq!(exact, baseline);
    assert_eq!(
        projection.closure_summaries(&RefScope::All).unwrap(),
        baseline_summaries
    );
    for oid in &orphan_records {
        assert!(projection.get_object(oid).unwrap().is_none());
    }

    let object_limit = orphan_records.len() - 1;
    let object_error = projection
        .rebuild_with_limits(
            &repository.objects,
            &refs,
            ProjectionLimits {
                graph: GraphLimits::default(),
                tombstone_scan: TombstoneScanLimits {
                    max_record_objects: object_limit,
                    max_record_bytes: total_record_bytes,
                },
            },
        )
        .unwrap_err();
    assert_error(
        object_error,
        "resource_limit",
        &format!("record inventory exceeds max_objects {object_limit}"),
    );

    let byte_limit = total_record_bytes - 1;
    let byte_error = projection
        .rebuild_with_limits(
            &repository.objects,
            &refs,
            ProjectionLimits {
                graph: GraphLimits::default(),
                tombstone_scan: TombstoneScanLimits {
                    max_record_objects: orphan_records.len(),
                    max_record_bytes: byte_limit,
                },
            },
        )
        .unwrap_err();
    assert_error(
        byte_error,
        "resource_limit",
        &format!("Tombstone scan exceeds max_record_bytes {byte_limit}"),
    );

    assert_eq!(projection.metadata().unwrap(), Some(baseline.clone()));
    assert_eq!(
        projection.closure_summaries(&RefScope::All).unwrap(),
        baseline_summaries
    );
    assert!(projection.get_object(&payload).unwrap().is_some());
    for oid in &orphan_records {
        assert!(projection.get_object(oid).unwrap().is_none());
    }

    drop(projection);
    let reopened = SqliteProjectionStore::open(&projection_path).unwrap();
    assert_eq!(reopened.metadata().unwrap(), Some(baseline));
    assert_eq!(
        reopened.closure_summaries(&RefScope::All).unwrap(),
        baseline_summaries
    );
    assert!(reopened.get_object(&payload).unwrap().is_some());
}

#[test]
fn store_wide_orphan_tombstone_changes_missing_to_tombstoned() {
    let repository = TestRepository::new("store-wide-tombstone");
    let missing_blob = fake_oid("blob", 'c');
    let (_, head) = put_head(&repository.objects, &[("missing.bin", &missing_blob)], &[]);
    let refs = snapshot(&[("observed/store-wide-tombstone", &head, 1)]);
    let mut projection = SqliteProjectionStore::open_in_memory().unwrap();

    let before = projection
        .rebuild(&repository.objects, &refs, GraphLimits::default())
        .unwrap()
        .metadata;
    assert_eq!(before.incomplete_ref_count, 1);
    assert_eq!(
        projection
            .get_object(&missing_blob)
            .unwrap()
            .unwrap()
            .availability,
        ObjectAvailability::Missing
    );
    assert_eq!(
        projection
            .closure_issues("observed/store-wide-tombstone")
            .unwrap()
            .len(),
        1
    );

    let tombstone = put_json(
        &repository.objects,
        tombstone_record(
            "urn:uuid:20000000-0000-4000-8000-0000000002a0",
            &missing_blob,
        ),
    );
    let after = projection
        .rebuild(&repository.objects, &refs, GraphLimits::default())
        .unwrap()
        .metadata;

    assert_ne!(after.source_fingerprint, before.source_fingerprint);
    assert_eq!(after.object_count, before.object_count);
    assert_eq!(after.incomplete_ref_count, 0);
    let resolved = projection.get_object(&missing_blob).unwrap().unwrap();
    assert_eq!(resolved.availability, ObjectAvailability::Tombstoned);
    assert_eq!(resolved.tombstone_oid.as_deref(), Some(tombstone.as_str()));
    assert!(projection.get_object(&tombstone).unwrap().is_none());
    assert!(
        projection
            .closure_issues("observed/store-wide-tombstone")
            .unwrap()
            .is_empty()
    );
    let summary = projection
        .closure_summaries(&RefScope::All)
        .unwrap()
        .pop()
        .unwrap();
    assert!(summary.complete);
    assert_eq!(summary.missing_count, 0);
    assert_eq!(summary.tombstoned_count, 1);
}

#[test]
fn timeline_uses_observation_capture_and_activity_valid_time_with_fallback_ordering() {
    let repository = TestRepository::new("timeline");
    let media = put_blob(&repository.objects, b"timeline media");
    let records = [
        put_json(
            &repository.objects,
            observation_record(
                "urn:uuid:20000000-0000-4000-8000-000000000301",
                SUBJECT_ID,
                SERIES_A,
                json!({ "kind": "instant", "at": "2026-07-12T02:01:00.000000000Z" }),
                "2026-07-12T02:50:00.000000000Z",
                &media,
            ),
        ),
        put_json(
            &repository.objects,
            activity_record(
                "urn:uuid:20000000-0000-4000-8000-000000000302",
                &[SUBJECT_ID],
                json!({ "kind": "instant", "at": "2026-07-12T02:02:00.000000000Z" }),
                "2026-07-12T02:51:00.000000000Z",
            ),
        ),
        put_json(
            &repository.objects,
            observation_record(
                "urn:uuid:20000000-0000-4000-8000-000000000303",
                SUBJECT_ID,
                SERIES_B,
                json!({
                    "kind": "interval",
                    "from": "2026-07-12T02:03:00.000000000Z",
                    "to": "2026-07-12T02:03:30.000000000Z"
                }),
                "2026-07-12T02:52:00.000000000Z",
                &media,
            ),
        ),
        put_json(
            &repository.objects,
            activity_record(
                "urn:uuid:20000000-0000-4000-8000-000000000304",
                &[SUBJECT_ID],
                json!({
                    "kind": "interval",
                    "to": "2026-07-12T02:04:00.000000000Z"
                }),
                "2026-07-12T02:53:00.000000000Z",
            ),
        ),
        put_json(
            &repository.objects,
            observation_record(
                "urn:uuid:20000000-0000-4000-8000-000000000305",
                SUBJECT_ID,
                SERIES_A,
                json!({ "kind": "unknown", "reason": "legacy clock unavailable" }),
                "2026-07-12T02:05:00.000000000Z",
                &media,
            ),
        ),
        put_json(
            &repository.objects,
            activity_record(
                "urn:uuid:20000000-0000-4000-8000-000000000306",
                &[SUBJECT_ID],
                json!({ "kind": "unknown", "reason": "legacy event time unavailable" }),
                "2026-07-12T02:06:00.000000000Z",
            ),
        ),
    ];
    let mut entries = vec![("media.bin", media.as_str())];
    let names = [
        "01.json", "02.json", "03.json", "04.json", "05.json", "06.json",
    ];
    entries.extend(
        names
            .iter()
            .zip(records.iter())
            .map(|(name, oid)| (*name, oid.as_str())),
    );
    let (_, head) = put_head(&repository.objects, &entries, &[]);
    let mut projection = SqliteProjectionStore::open_in_memory().unwrap();
    projection
        .rebuild(
            &repository.objects,
            &snapshot(&[("observed/timeline", &head, 1)]),
            GraphLimits::default(),
        )
        .unwrap();

    let timeline = projection
        .subject_timeline(SUBJECT_ID, None, &RefScope::All)
        .unwrap();
    assert_eq!(
        timeline
            .iter()
            .map(|entry| entry.oid.as_str())
            .collect::<Vec<_>>(),
        records.iter().map(String::as_str).collect::<Vec<_>>()
    );
    assert_eq!(
        timeline
            .iter()
            .map(|entry| (entry.kind, entry.time_basis))
            .collect::<Vec<_>>(),
        vec![
            (
                TimelineRecordKind::Observation,
                TimelineTimeBasis::ObservationCaptureInstant,
            ),
            (
                TimelineRecordKind::Activity,
                TimelineTimeBasis::ActivityValidInstant,
            ),
            (
                TimelineRecordKind::Observation,
                TimelineTimeBasis::ObservationCaptureInterval,
            ),
            (
                TimelineRecordKind::Activity,
                TimelineTimeBasis::ActivityValidInterval,
            ),
            (
                TimelineRecordKind::Observation,
                TimelineTimeBasis::ObservationRecordedAtFallback,
            ),
            (
                TimelineRecordKind::Activity,
                TimelineTimeBasis::ActivityRecordedAtFallback,
            ),
        ]
    );
    assert_eq!(
        timeline[2].event_time_start.as_deref(),
        Some("2026-07-12T02:03:00.000000000Z")
    );
    assert_eq!(
        timeline[2].event_time_end.as_deref(),
        Some("2026-07-12T02:03:30.000000000Z")
    );
    assert_eq!(timeline[3].event_time_start, None);
    assert_eq!(
        timeline[3].event_time_end.as_deref(),
        Some("2026-07-12T02:04:00.000000000Z")
    );
    let series_a = projection
        .subject_timeline(SUBJECT_ID, Some(SERIES_A), &RefScope::All)
        .unwrap();
    assert_eq!(
        series_a
            .iter()
            .map(|entry| entry.oid.as_str())
            .collect::<Vec<_>>(),
        vec![records[0].as_str(), records[4].as_str()]
    );
    assert!(
        projection
            .subject_timeline(SUBJECT_ID, Some("' OR 1=1 --"), &RefScope::All)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn equal_timeline_times_use_oid_as_a_stable_cross_kind_tiebreaker() {
    let repository = TestRepository::new("timeline-tie");
    let media = put_blob(&repository.objects, b"equal-time media");
    let observation = put_json(
        &repository.objects,
        observation_record(
            "urn:uuid:20000000-0000-4000-8000-000000000311",
            SUBJECT_ID,
            SERIES_A,
            json!({ "kind": "instant", "at": "2026-07-12T02:30:00.000000000Z" }),
            "2026-07-12T02:31:00.000000000Z",
            &media,
        ),
    );
    let activity = put_json(
        &repository.objects,
        activity_record(
            "urn:uuid:20000000-0000-4000-8000-000000000312",
            &[SUBJECT_ID],
            json!({ "kind": "instant", "at": "2026-07-12T02:30:00.000000000Z" }),
            "2026-07-12T02:32:00.000000000Z",
        ),
    );
    let (_, head) = put_head(
        &repository.objects,
        &[
            ("activity.json", &activity),
            ("media.bin", &media),
            ("observation.json", &observation),
        ],
        &[],
    );
    let refs = snapshot(&[("observed/tie-b", &head, 2), ("observed/tie-a", &head, 1)]);
    let mut projection = SqliteProjectionStore::open_in_memory().unwrap();
    projection
        .rebuild(&repository.objects, &refs, GraphLimits::default())
        .unwrap();
    let mut expected = vec![observation.as_str(), activity.as_str()];
    expected.sort();

    let first = projection
        .subject_timeline(SUBJECT_ID, None, &RefScope::All)
        .unwrap();
    assert_eq!(
        first
            .iter()
            .map(|entry| entry.oid.as_str())
            .collect::<Vec<_>>(),
        expected
    );
    assert!(
        first
            .iter()
            .all(|entry| entry.ordering_time == "2026-07-12T02:30:00.000000000Z")
    );
    projection
        .rebuild(
            &repository.objects,
            &snapshot(&[("observed/tie-a", &head, 1), ("observed/tie-b", &head, 2)]),
            GraphLimits::default(),
        )
        .unwrap();
    let scoped = projection
        .subject_timeline(SUBJECT_ID, None, &RefScope::one("observed/tie-b"))
        .unwrap();
    assert_eq!(
        scoped
            .iter()
            .map(|entry| entry.oid.as_str())
            .collect::<Vec<_>>(),
        expected
    );
}

#[test]
fn observation_dependencies_report_roles_target_kinds_and_availability() {
    let repository = TestRepository::new("dependencies");
    let media = put_blob(&repository.objects, b"present primary media");
    let capture_profile = put_json(
        &repository.objects,
        capture_profile_record("urn:uuid:20000000-0000-4000-8000-000000000401"),
    );
    let environment = put_json(
        &repository.objects,
        claim_record("urn:uuid:20000000-0000-4000-8000-000000000402", SUBJECT_ID),
    );
    let missing_deployment = fake_oid("record", 'a');
    let tombstoned_calibration = fake_oid("record", 'b');
    let tombstone = put_json(
        &repository.objects,
        tombstone_record(
            "urn:uuid:20000000-0000-4000-8000-000000000403",
            &tombstoned_calibration,
        ),
    );
    let mut observation_body = observation_record(
        "urn:uuid:20000000-0000-4000-8000-000000000404",
        SUBJECT_ID,
        SERIES_A,
        json!({ "kind": "instant", "at": "2026-07-12T03:00:00.000000000Z" }),
        "2026-07-12T03:00:01.000000000Z",
        &media,
    );
    observation_body["payload"]["capture_profile_ref"] = json!(capture_profile);
    observation_body["payload"]["station_ref"] = json!(STATION_ID);
    observation_body["payload"]["station_deployment_ref"] = json!(missing_deployment);
    observation_body["payload"]["calibration_refs"] = json!([tombstoned_calibration]);
    observation_body["payload"]["environment_refs"] = json!([environment]);
    let observation = put_json(&repository.objects, observation_body);
    let (_, head) = put_head(
        &repository.objects,
        &[
            ("capture-profile.json", &capture_profile),
            ("environment.json", &environment),
            ("media.bin", &media),
            ("observation.json", &observation),
            ("tombstone.json", &tombstone),
        ],
        &[],
    );
    let mut projection = SqliteProjectionStore::open_in_memory().unwrap();
    let report = projection
        .rebuild(
            &repository.objects,
            &snapshot(&[("observed/dependencies", &head, 1)]),
            GraphLimits::default(),
        )
        .unwrap();

    assert_eq!(report.metadata.incomplete_ref_count, 1);
    let dependencies = projection.observation_dependencies(&observation).unwrap();
    assert_eq!(dependencies.len(), 6);
    let find = |kind| {
        dependencies
            .iter()
            .find(|dependency| dependency.kind == kind)
            .unwrap()
    };
    assert_eq!(
        find(ObservationDependencyKind::CaptureProfile).availability,
        Some(ObjectAvailability::Present)
    );
    assert_eq!(
        find(ObservationDependencyKind::CaptureProfile).target_kind,
        DependencyTargetKind::Object(ObjectKind::Record)
    );
    assert_eq!(
        find(ObservationDependencyKind::Station).target_kind,
        DependencyTargetKind::Entity
    );
    assert_eq!(find(ObservationDependencyKind::Station).availability, None);
    assert_eq!(
        find(ObservationDependencyKind::StationDeployment).availability,
        Some(ObjectAvailability::Missing)
    );
    assert_eq!(
        find(ObservationDependencyKind::Calibration).availability,
        Some(ObjectAvailability::Tombstoned)
    );
    assert_eq!(
        find(ObservationDependencyKind::Environment).availability,
        Some(ObjectAvailability::Present)
    );
    let media_dependency = find(ObservationDependencyKind::Media);
    assert_eq!(media_dependency.role.as_deref(), Some("primary"));
    assert_eq!(
        media_dependency.target_kind,
        DependencyTargetKind::Object(ObjectKind::Blob)
    );
    assert_eq!(
        media_dependency.availability,
        Some(ObjectAvailability::Present)
    );

    let missing = projection.get_object(&missing_deployment).unwrap().unwrap();
    assert_eq!(missing.availability, ObjectAvailability::Missing);
    let tombstoned = projection
        .get_object(&tombstoned_calibration)
        .unwrap()
        .unwrap();
    assert_eq!(tombstoned.availability, ObjectAvailability::Tombstoned);
    assert_eq!(
        tombstoned.tombstone_oid.as_deref(),
        Some(tombstone.as_str())
    );
    let summary = projection
        .closure_summaries(&RefScope::All)
        .unwrap()
        .pop()
        .unwrap();
    assert!(!summary.complete);
    assert!(!summary.truncated);
    assert_eq!(summary.missing_count, 1);
    assert_eq!(summary.tombstoned_count, 1);
    assert_eq!(summary.issue_count, 1);
    let issues = projection.closure_issues("observed/dependencies").unwrap();
    assert_eq!(issues.len(), 1);
    assert_eq!(issues[0].issue_kind, "missing");
    assert_eq!(issues[0].oid, missing_deployment);
    assert!(
        issues[0]
            .role
            .as_deref()
            .unwrap()
            .contains("station_deployment_ref")
    );
}

#[test]
fn rebuilding_same_ref_after_missing_object_arrives_removes_stale_closure_issues() {
    let repository = TestRepository::new("missing-resolution");
    let donor = TestRepository::new("missing-resolution-donor");
    let payload = b"payload delivered after the first projection";
    let missing_oid = put_blob(&donor.objects, payload);
    let (_, head) = put_head(
        &repository.objects,
        &[("late-arrival.bin", &missing_oid)],
        &[],
    );
    let refs = snapshot(&[("observed/late-arrival", &head, 1)]);
    let mut projection = SqliteProjectionStore::open_in_memory().unwrap();

    let incomplete = projection
        .rebuild(&repository.objects, &refs, GraphLimits::default())
        .unwrap()
        .metadata;
    assert_eq!(incomplete.incomplete_ref_count, 1);
    assert_eq!(
        projection
            .get_object(&missing_oid)
            .unwrap()
            .unwrap()
            .availability,
        ObjectAvailability::Missing
    );
    assert_eq!(
        projection
            .closure_issues("observed/late-arrival")
            .unwrap()
            .len(),
        1
    );

    repository
        .objects
        .put_verified_raw(&missing_oid, payload)
        .unwrap();
    let complete = projection
        .rebuild(&repository.objects, &refs, GraphLimits::default())
        .unwrap()
        .metadata;

    assert_ne!(complete.source_fingerprint, incomplete.source_fingerprint);
    assert_eq!(complete.incomplete_ref_count, 0);
    assert_eq!(
        projection
            .get_object(&missing_oid)
            .unwrap()
            .unwrap()
            .availability,
        ObjectAvailability::Present
    );
    assert!(
        projection
            .closure_issues("observed/late-arrival")
            .unwrap()
            .is_empty()
    );
    assert!(projection.closure_summaries(&RefScope::All).unwrap()[0].complete);
}

#[test]
fn corrupt_orphan_record_fails_closed_and_preserves_reopenable_projection() {
    let repository = TestRepository::new("corrupt-orphan-record");
    let media = put_blob(&repository.objects, b"stable orphan-corruption baseline");
    let observation = put_json(
        &repository.objects,
        observation_record(
            "urn:uuid:20000000-0000-4000-8000-0000000002a1",
            SUBJECT_ID,
            SERIES_A,
            json!({ "kind": "instant", "at": "2026-07-12T03:30:00.000000000Z" }),
            "2026-07-12T03:30:01.000000000Z",
            &media,
        ),
    );
    let (_, head) = put_head(
        &repository.objects,
        &[("media.bin", &media), ("observation.json", &observation)],
        &[],
    );
    let refs = snapshot(&[("observed/corrupt-orphan", &head, 1)]);
    let projection_path = repository.projection_path();
    let mut projection = SqliteProjectionStore::open(&projection_path).unwrap();
    let baseline_metadata = projection
        .rebuild(&repository.objects, &refs, GraphLimits::default())
        .unwrap()
        .metadata;
    let baseline_summaries = projection.closure_summaries(&RefScope::All).unwrap();
    let baseline_timeline = projection
        .subject_timeline(SUBJECT_ID, None, &RefScope::All)
        .unwrap();

    let corrupt_orphan = put_json(
        &repository.objects,
        subject_record(
            "urn:uuid:20000000-0000-4000-8000-0000000002a2",
            "Corrupt CAS orphan",
        ),
    );
    fs::write(
        object_path(&repository.temporary.join("objects"), &corrupt_orphan),
        b"different bytes",
    )
    .unwrap();

    let error = projection
        .rebuild(&repository.objects, &refs, GraphLimits::default())
        .unwrap_err();
    assert_error(error, "projection_source_invalid", "corrupt object");
    assert_eq!(
        projection.metadata().unwrap(),
        Some(baseline_metadata.clone())
    );
    assert_eq!(
        projection.closure_summaries(&RefScope::All).unwrap(),
        baseline_summaries
    );
    assert_eq!(
        projection
            .subject_timeline(SUBJECT_ID, None, &RefScope::All)
            .unwrap(),
        baseline_timeline
    );
    assert!(projection.get_object(&observation).unwrap().is_some());
    assert!(projection.get_object(&corrupt_orphan).unwrap().is_none());

    drop(projection);
    let reopened = SqliteProjectionStore::open(&projection_path).unwrap();
    assert_eq!(reopened.metadata().unwrap(), Some(baseline_metadata));
    assert_eq!(
        reopened.closure_summaries(&RefScope::All).unwrap(),
        baseline_summaries
    );
    assert_eq!(
        reopened
            .subject_timeline(SUBJECT_ID, None, &RefScope::All)
            .unwrap(),
        baseline_timeline
    );
    assert!(reopened.get_object(&corrupt_orphan).unwrap().is_none());
}

#[test]
fn corrupt_schema_invalid_and_type_invalid_rebuilds_preserve_the_old_projection() {
    let repository = TestRepository::new("rollback");
    let media = put_blob(&repository.objects, b"stable baseline media");
    let observation = put_json(
        &repository.objects,
        observation_record(
            "urn:uuid:20000000-0000-4000-8000-000000000501",
            SUBJECT_ID,
            SERIES_A,
            json!({ "kind": "instant", "at": "2026-07-12T04:00:00.000000000Z" }),
            "2026-07-12T04:00:01.000000000Z",
            &media,
        ),
    );
    let (_, baseline_head) = put_head(
        &repository.objects,
        &[("media.bin", &media), ("observation.json", &observation)],
        &[],
    );
    let baseline_refs = snapshot(&[("observed/stable", &baseline_head, 1)]);
    let mut projection = SqliteProjectionStore::open_in_memory().unwrap();
    let baseline_metadata = projection
        .rebuild(&repository.objects, &baseline_refs, GraphLimits::default())
        .unwrap()
        .metadata;
    let baseline_timeline = projection
        .subject_timeline(SUBJECT_ID, None, &RefScope::All)
        .unwrap();

    let invalid_record = put_unchecked(
        &repository.objects,
        json!({
            "object_type": "record",
            "schema_version": "0.1.0",
            "record_type": "subject"
        }),
    );
    let (_, schema_invalid_head) = put_head(
        &repository.objects,
        &[("invalid.json", &invalid_record)],
        &[],
    );
    let error = projection
        .rebuild(
            &repository.objects,
            &snapshot(&[("observed/invalid-schema", &schema_invalid_head, 2)]),
            GraphLimits::default(),
        )
        .unwrap_err();
    assert_error(
        error,
        "projection_source_invalid",
        "schema/semantic validation",
    );
    assert_eq!(
        projection.metadata().unwrap(),
        Some(baseline_metadata.clone())
    );
    assert_eq!(
        projection
            .subject_timeline(SUBJECT_ID, None, &RefScope::All)
            .unwrap(),
        baseline_timeline
    );

    let valid_record = put_json(
        &repository.objects,
        subject_record(
            "urn:uuid:20000000-0000-4000-8000-000000000502",
            "Wrongly declared as a Blob",
        ),
    );
    let invalid_tree =
        put_tree_with_declared_kind(&repository.objects, "wrong.bin", "blob", &valid_record);
    let type_invalid_head = put_commit(&repository.objects, &invalid_tree, &[]);
    let error = projection
        .rebuild(
            &repository.objects,
            &snapshot(&[("observed/invalid-type", &type_invalid_head, 3)]),
            GraphLimits::default(),
        )
        .unwrap_err();
    assert_error(
        error,
        "projection_source_invalid",
        "reference kind mismatch",
    );
    assert_eq!(
        projection.metadata().unwrap(),
        Some(baseline_metadata.clone())
    );

    let corrupt_blob = put_blob(&repository.objects, b"bytes that will be corrupted");
    let (_, corrupt_head) = put_head(&repository.objects, &[("corrupt.bin", &corrupt_blob)], &[]);
    fs::write(
        object_path(&repository.temporary.join("objects"), &corrupt_blob),
        b"different bytes",
    )
    .unwrap();
    let error = projection
        .rebuild(
            &repository.objects,
            &snapshot(&[("observed/corrupt", &corrupt_head, 4)]),
            GraphLimits::default(),
        )
        .unwrap_err();
    assert_error(error, "projection_source_invalid", "corrupt object");
    assert_eq!(
        projection.metadata().unwrap(),
        Some(baseline_metadata.clone())
    );

    let error = projection
        .rebuild(
            &repository.objects,
            &baseline_refs,
            GraphLimits {
                max_objects: 2,
                max_edges: 100,
                max_depth: 10,
            },
        )
        .unwrap_err();
    assert_error(error, "resource_limit", "truncated");
    assert_eq!(projection.metadata().unwrap(), Some(baseline_metadata));
    assert!(projection.get_object(&observation).unwrap().is_some());
}

#[test]
fn sqlite_replace_failure_rolls_back_deleted_rows_and_metadata() {
    let repository = TestRepository::new("sqlite-rollback");
    let old_blob = put_blob(&repository.objects, b"old projection row");
    let new_blob = put_blob(&repository.objects, b"new projection row");
    let (_, old_head) = put_head(&repository.objects, &[("old.bin", &old_blob)], &[]);
    let (_, new_head) = put_head(&repository.objects, &[("new.bin", &new_blob)], &[]);
    let projection_path = repository.projection_path();
    let mut projection = SqliteProjectionStore::open(&projection_path).unwrap();
    let baseline = projection
        .rebuild(
            &repository.objects,
            &snapshot(&[("observed/old", &old_head, 1)]),
            GraphLimits::default(),
        )
        .unwrap()
        .metadata;
    let blocker = rusqlite::Connection::open(&projection_path).unwrap();
    blocker
        .execute_batch(
            "CREATE TRIGGER force_projection_insert_failure
             BEFORE INSERT ON objects
             BEGIN
                 SELECT RAISE(ABORT, 'forced projection insert failure');
             END;",
        )
        .unwrap();
    drop(blocker);

    let error = projection
        .rebuild(
            &repository.objects,
            &snapshot(&[("observed/new", &new_head, 2)]),
            GraphLimits::default(),
        )
        .unwrap_err();

    assert_error(error, "storage_error", "forced projection insert failure");
    assert_eq!(projection.metadata().unwrap(), Some(baseline));
    assert!(projection.get_object(&old_blob).unwrap().is_some());
    assert!(projection.get_object(&old_head).unwrap().is_some());
    assert!(projection.get_object(&new_blob).unwrap().is_none());
    assert_eq!(
        projection
            .closure_summaries(&RefScope::All)
            .unwrap()
            .into_iter()
            .map(|summary| summary.ref_name)
            .collect::<Vec<_>>(),
        vec!["observed/old"]
    );
}

#[test]
fn global_union_and_reachability_limits_fail_before_replacing_old_rows() {
    let repository = TestRepository::new("global-limits");
    let first_blob = put_blob(&repository.objects, b"first unique ref payload");
    let second_blob = put_blob(&repository.objects, b"second unique ref payload");
    let (_, first_head) = put_head(&repository.objects, &[("first.bin", &first_blob)], &[]);
    let (_, second_head) = put_head(&repository.objects, &[("second.bin", &second_blob)], &[]);
    let first_only = snapshot(&[("observed/first", &first_head, 1)]);
    let both_unique = snapshot(&[
        ("observed/first", &first_head, 1),
        ("observed/second", &second_head, 2),
    ]);
    let mut projection = SqliteProjectionStore::open_in_memory().unwrap();
    let baseline = projection
        .rebuild(&repository.objects, &first_only, GraphLimits::default())
        .unwrap()
        .metadata;

    let union_error = projection
        .rebuild(
            &repository.objects,
            &both_unique,
            GraphLimits {
                max_objects: 3,
                max_edges: 100,
                max_depth: 10,
            },
        )
        .unwrap_err();
    assert_error(union_error, "resource_limit", "unique objects");
    assert_eq!(projection.metadata().unwrap(), Some(baseline.clone()));

    let shared_twice = snapshot(&[
        ("observed/first", &first_head, 1),
        ("observed/alias", &first_head, 2),
    ]);
    let reachability_error = projection
        .rebuild(
            &repository.objects,
            &shared_twice,
            GraphLimits {
                max_objects: 10,
                max_edges: 3,
                max_depth: 10,
            },
        )
        .unwrap_err();
    assert_error(reachability_error, "resource_limit", "reachability rows");
    assert_eq!(projection.metadata().unwrap(), Some(baseline));
    assert!(projection.get_object(&second_blob).unwrap().is_none());
}

#[test]
fn empty_ref_snapshot_is_queryable_and_clears_a_previous_projection() {
    let repository = TestRepository::new("empty");
    let blob = put_blob(&repository.objects, b"row removed by empty rebuild");
    let (_, head) = put_head(&repository.objects, &[("payload.bin", &blob)], &[]);
    let mut projection = SqliteProjectionStore::open(repository.projection_path()).unwrap();
    assert_eq!(projection.metadata().unwrap(), None);
    assert!(
        projection
            .closure_summaries(&RefScope::All)
            .unwrap()
            .is_empty()
    );
    assert!(
        projection
            .subject_timeline(SUBJECT_ID, None, &RefScope::names(Vec::<String>::new()))
            .unwrap()
            .is_empty()
    );
    projection
        .rebuild(
            &repository.objects,
            &snapshot(&[("observed/main", &head, 1)]),
            GraphLimits::default(),
        )
        .unwrap();
    assert!(projection.get_object(&blob).unwrap().is_some());

    let empty_metadata = projection
        .rebuild(
            &repository.objects,
            &RefSnapshot::default(),
            GraphLimits::default(),
        )
        .unwrap()
        .metadata;
    assert_eq!(empty_metadata.ref_count, 0);
    assert_eq!(empty_metadata.object_count, 0);
    assert_eq!(empty_metadata.edge_count, 0);
    assert_eq!(empty_metadata.incomplete_ref_count, 0);
    assert!(projection.get_object(&blob).unwrap().is_none());
    assert!(
        projection
            .closure_summaries(&RefScope::All)
            .unwrap()
            .is_empty()
    );
    assert_error(
        projection
            .observation_dependencies(&fake_oid("record", 'c'))
            .unwrap_err(),
        "projection_observation_unknown",
        "is not indexed",
    );
    let zero_scan_empty = projection
        .rebuild_with_limits(
            &repository.objects,
            &RefSnapshot::default(),
            ProjectionLimits {
                graph: GraphLimits::default(),
                tombstone_scan: TombstoneScanLimits {
                    max_record_objects: 0,
                    max_record_bytes: 0,
                },
            },
        )
        .expect("an empty Ref snapshot must not prepare a Tombstone scan")
        .metadata;
    assert_eq!(zero_scan_empty, empty_metadata);
    drop(projection);
    let reopened = SqliteProjectionStore::open(repository.projection_path()).unwrap();
    assert_eq!(reopened.metadata().unwrap(), Some(zero_scan_empty));
}

#[test]
fn malformed_duplicate_or_nonpositive_ref_snapshots_are_rejected_atomically() {
    let repository = TestRepository::new("invalid-snapshot");
    let blob = put_blob(&repository.objects, b"valid snapshot payload");
    let (_, head) = put_head(&repository.objects, &[("payload.bin", &blob)], &[]);
    let mut projection = SqliteProjectionStore::open_in_memory().unwrap();
    let baseline = projection
        .rebuild(
            &repository.objects,
            &snapshot(&[("observed/main", &head, 1)]),
            GraphLimits::default(),
        )
        .unwrap()
        .metadata;

    for (invalid, expected) in [
        (
            snapshot(&[
                ("observed/duplicate", &head, 2),
                ("observed/duplicate", &head, 3),
            ]),
            "appears more than once",
        ),
        (
            snapshot(&[("observed/x' OR 1=1 --", &head, 2)]),
            "invalid Ref name",
        ),
        (
            snapshot(&[("observed/not-a-commit", &blob, 2)]),
            "head is not a Commit",
        ),
        (
            snapshot(&[("observed/nonpositive", &head, 0)]),
            "non-positive updated_event_id",
        ),
        (
            snapshot(&[
                ("observed/event-a", &head, 9),
                ("observed/event-b", &head, 9),
            ]),
            "shared by multiple Refs",
        ),
    ] {
        let error = projection
            .rebuild(&repository.objects, &invalid, GraphLimits::default())
            .unwrap_err();
        assert_error(error, "projection_source_invalid", expected);
        assert_eq!(projection.metadata().unwrap(), Some(baseline.clone()));
    }
}

#[test]
fn analysis_lineage_preserves_typed_adapter_inputs_outputs_masks_and_ref_scope() {
    let repository = TestRepository::new("analysis-lineage");
    let input_record = put_json(
        &repository.objects,
        claim_record("urn:uuid:20000000-0000-4000-8000-000000000601", SUBJECT_ID),
    );
    let input_blob = put_blob(&repository.objects, b"analysis after input");
    let implementation = put_blob(&repository.objects, b"adapter implementation v2.1");
    let configuration = put_blob(&repository.objects, b"adapter configuration");
    let transform_a = put_json(
        &repository.objects,
        claim_record("urn:uuid:20000000-0000-4000-8000-000000000602", SUBJECT_ID),
    );
    let transform_b = put_json(
        &repository.objects,
        claim_record("urn:uuid:20000000-0000-4000-8000-000000000604", SUBJECT_ID),
    );
    let derived_a = put_blob(&repository.objects, b"derived output a");
    let derived_b = put_blob(&repository.objects, b"derived output b");
    let changed_mask = put_blob(&repository.objects, b"changed mask");
    let unchanged_mask = put_blob(&repository.objects, b"unchanged mask");
    let ambiguous_mask = put_blob(&repository.objects, b"ambiguous mask");
    let unobservable_mask = put_blob(&repository.objects, b"unobservable mask");
    let validity_mask = put_blob(&repository.objects, b"validity mask");
    let analysis = put_json(
        &repository.objects,
        analysis_result_record(
            "urn:uuid:20000000-0000-4000-8000-000000000603",
            &[("before", &input_record), ("after", &input_blob)],
            &implementation,
            &configuration,
            &[transform_b.clone(), transform_a.clone()],
            &[derived_b.clone(), derived_a.clone()],
            &[
                ("validity", &validity_mask),
                ("unobservable", &unobservable_mask),
                ("changed", &changed_mask),
                ("unchanged", &unchanged_mask),
                ("ambiguous", &ambiguous_mask),
            ],
        ),
    );
    let (_, head) = put_head(&repository.objects, &[("analysis.json", &analysis)], &[]);
    let mut projection = SqliteProjectionStore::open_in_memory().unwrap();
    projection
        .rebuild(
            &repository.objects,
            &snapshot(&[
                ("observed/analysis-b", &head, 2),
                ("observed/analysis-a", &head, 1),
            ]),
            GraphLimits::default(),
        )
        .unwrap();

    let lineage = projection
        .analysis_lineage(&analysis, &RefScope::All)
        .unwrap();

    assert_eq!(lineage.analysis_oid, analysis);
    assert_eq!(
        lineage.entity_id,
        "urn:uuid:20000000-0000-4000-8000-000000000603"
    );
    assert_eq!(lineage.recorded_at, "2026-07-12T05:00:00.000000000Z");
    assert_eq!(lineage.asserted_by, HUMAN_ID);
    assert_eq!(lineage.analysis_kind, "projection_lineage_fixture");
    assert_eq!(lineage.comparison_kind, "temporal_observation");
    assert_eq!(lineage.status, "succeeded");
    assert_eq!(lineage.comparability, "comparable");
    assert_eq!(lineage.adapter.id, "fixture.analysis-adapter");
    assert_eq!(lineage.adapter.version, "2.1");
    assert_eq!(lineage.adapter.determinism, AdapterDeterminism::Seeded);
    assert_eq!(lineage.adapter.seed.as_deref(), Some("fixture-seed-42"));
    assert_eq!(lineage.adapter.implementation.oid, implementation);
    assert_eq!(lineage.adapter.implementation.kind, ObjectKind::Blob);
    assert_eq!(
        lineage.adapter.implementation.availability,
        ObjectAvailability::Present
    );
    assert_eq!(lineage.adapter.configuration.oid, configuration);
    assert_eq!(lineage.inputs.len(), 2);
    assert_eq!(
        lineage
            .inputs
            .iter()
            .map(|input| (
                input.ordinal,
                input.role.as_str(),
                input.object.oid.as_str()
            ))
            .collect::<Vec<_>>(),
        vec![
            (0, "before", input_record.as_str()),
            (1, "after", input_blob.as_str()),
        ]
    );
    assert_eq!(lineage.inputs[0].object.kind, ObjectKind::Record);
    assert_eq!(lineage.inputs[1].object.kind, ObjectKind::Blob);
    let mut expected_transforms = vec![transform_a.as_str(), transform_b.as_str()];
    expected_transforms.sort();
    assert_eq!(
        lineage
            .transforms
            .iter()
            .map(|object| object.oid.as_str())
            .collect::<Vec<_>>(),
        expected_transforms
    );
    assert!(
        lineage
            .transforms
            .iter()
            .all(|object| object.kind == ObjectKind::Record)
    );
    let mut expected_derived = vec![derived_a.as_str(), derived_b.as_str()];
    expected_derived.sort();
    assert_eq!(
        lineage
            .derived_blobs
            .iter()
            .map(|object| object.oid.as_str())
            .collect::<Vec<_>>(),
        expected_derived
    );
    assert!(
        lineage
            .derived_blobs
            .iter()
            .all(|object| object.kind == ObjectKind::Blob
                && object.availability == ObjectAvailability::Present)
    );
    assert_eq!(
        lineage
            .masks
            .iter()
            .map(|mask| (mask.role, mask.object.oid.as_str()))
            .collect::<Vec<_>>(),
        vec![
            (AnalysisMaskRole::Changed, changed_mask.as_str()),
            (AnalysisMaskRole::Unchanged, unchanged_mask.as_str()),
            (AnalysisMaskRole::Ambiguous, ambiguous_mask.as_str()),
            (AnalysisMaskRole::Unobservable, unobservable_mask.as_str()),
            (AnalysisMaskRole::Validity, validity_mask.as_str()),
        ]
    );
    assert_eq!(lineage.replay_readiness, AnalysisReplayReadiness::Ready);
    assert_eq!(
        lineage.reachable_from,
        vec![
            "observed/analysis-a".to_owned(),
            "observed/analysis-b".to_owned(),
        ]
    );
    let scoped = projection
        .analysis_lineage(&analysis, &RefScope::one("observed/analysis-b"))
        .unwrap();
    assert_eq!(scoped.reachable_from, vec!["observed/analysis-b"]);
}

#[test]
fn analysis_replay_readiness_tracks_each_prerequisite_but_not_missing_outputs() {
    #[derive(Clone, Copy, Debug)]
    enum Prerequisite {
        Input,
        Implementation,
        Configuration,
        Transform,
    }

    for prerequisite in [
        Prerequisite::Input,
        Prerequisite::Implementation,
        Prerequisite::Configuration,
        Prerequisite::Transform,
    ] {
        for unavailable_state in [ObjectAvailability::Missing, ObjectAvailability::Tombstoned] {
            let repository = TestRepository::new("analysis-readiness");
            let mut input = put_blob(&repository.objects, b"present replay input");
            let mut implementation =
                put_blob(&repository.objects, b"present replay implementation");
            let mut configuration = put_blob(&repository.objects, b"present replay configuration");
            let mut transform = put_json(
                &repository.objects,
                claim_record("urn:uuid:20000000-0000-4000-8000-000000000611", SUBJECT_ID),
            );
            let unavailable_kind = if matches!(prerequisite, Prerequisite::Transform) {
                "record"
            } else {
                "blob"
            };
            let unavailable_oid = fake_oid(
                unavailable_kind,
                match prerequisite {
                    Prerequisite::Input => 'a',
                    Prerequisite::Implementation => 'b',
                    Prerequisite::Configuration => 'c',
                    Prerequisite::Transform => 'd',
                },
            );
            match prerequisite {
                Prerequisite::Input => input.clone_from(&unavailable_oid),
                Prerequisite::Implementation => implementation.clone_from(&unavailable_oid),
                Prerequisite::Configuration => configuration.clone_from(&unavailable_oid),
                Prerequisite::Transform => transform.clone_from(&unavailable_oid),
            }
            let missing_derived = fake_oid("blob", 'e');
            let missing_mask = fake_oid("blob", 'f');
            let analysis = put_json(
                &repository.objects,
                analysis_result_record(
                    "urn:uuid:20000000-0000-4000-8000-000000000612",
                    &[("source", &input)],
                    &implementation,
                    &configuration,
                    std::slice::from_ref(&transform),
                    std::slice::from_ref(&missing_derived),
                    &[("unobservable", &missing_mask)],
                ),
            );
            let tombstone = (unavailable_state == ObjectAvailability::Tombstoned).then(|| {
                put_json(
                    &repository.objects,
                    tombstone_record(
                        "urn:uuid:20000000-0000-4000-8000-000000000613",
                        &unavailable_oid,
                    ),
                )
            });
            let mut entries = vec![("analysis.json", analysis.as_str())];
            if let Some(tombstone) = tombstone.as_deref() {
                entries.push(("tombstone.json", tombstone));
            }
            let (_, head) = put_head(&repository.objects, &entries, &[]);
            let mut projection = SqliteProjectionStore::open_in_memory().unwrap();
            projection
                .rebuild(
                    &repository.objects,
                    &snapshot(&[("observed/replay", &head, 1)]),
                    GraphLimits::default(),
                )
                .unwrap();

            let lineage = projection
                .analysis_lineage(&analysis, &RefScope::All)
                .unwrap();
            let actual_target = match prerequisite {
                Prerequisite::Input => &lineage.inputs[0].object,
                Prerequisite::Implementation => &lineage.adapter.implementation,
                Prerequisite::Configuration => &lineage.adapter.configuration,
                Prerequisite::Transform => &lineage.transforms[0],
            };
            assert_eq!(actual_target.oid, unavailable_oid);
            assert_eq!(actual_target.availability, unavailable_state);
            assert_eq!(
                lineage.replay_readiness,
                match unavailable_state {
                    ObjectAvailability::Missing => AnalysisReplayReadiness::BlockedMissing,
                    ObjectAvailability::Tombstoned => {
                        AnalysisReplayReadiness::BlockedTombstoned
                    }
                    ObjectAvailability::Present => unreachable!(),
                },
                "{prerequisite:?} {unavailable_state:?}"
            );
            assert_eq!(
                lineage.derived_blobs[0].availability,
                ObjectAvailability::Missing
            );
            assert_eq!(
                lineage.masks[0].object.availability,
                ObjectAvailability::Missing
            );
        }
    }
}

#[test]
fn analysis_replay_readiness_distinguishes_combined_missing_and_tombstoned_blockers() {
    let repository = TestRepository::new("analysis-readiness-both");
    let missing_input = fake_oid("blob", 'a');
    let tombstoned_implementation = fake_oid("blob", 'b');
    let configuration = put_blob(&repository.objects, b"present configuration");
    let analysis = put_json(
        &repository.objects,
        analysis_result_record(
            "urn:uuid:20000000-0000-4000-8000-000000000621",
            &[("source", &missing_input)],
            &tombstoned_implementation,
            &configuration,
            &[],
            &[],
            &[],
        ),
    );
    let tombstone = put_json(
        &repository.objects,
        tombstone_record(
            "urn:uuid:20000000-0000-4000-8000-000000000622",
            &tombstoned_implementation,
        ),
    );
    let (_, head) = put_head(
        &repository.objects,
        &[("analysis.json", &analysis), ("tombstone.json", &tombstone)],
        &[],
    );
    let mut projection = SqliteProjectionStore::open_in_memory().unwrap();
    projection
        .rebuild(
            &repository.objects,
            &snapshot(&[("observed/replay-both", &head, 1)]),
            GraphLimits::default(),
        )
        .unwrap();

    let lineage = projection
        .analysis_lineage(&analysis, &RefScope::All)
        .unwrap();
    assert_eq!(
        lineage.replay_readiness,
        AnalysisReplayReadiness::BlockedMissingAndTombstoned
    );
    assert_eq!(
        lineage.inputs[0].object.availability,
        ObjectAvailability::Missing
    );
    assert_eq!(
        lineage.adapter.implementation.availability,
        ObjectAvailability::Tombstoned
    );
}

#[test]
fn analysis_query_errors_scope_precedence_schema_rollback_and_stale_removal_are_distinct() {
    let repository = TestRepository::new("analysis-errors");
    let input = put_blob(&repository.objects, b"analysis query input");
    let implementation = put_blob(&repository.objects, b"analysis query implementation");
    let configuration = put_blob(&repository.objects, b"analysis query configuration");
    let analysis_body = analysis_result_record(
        "urn:uuid:20000000-0000-4000-8000-000000000631",
        &[("source", &input)],
        &implementation,
        &configuration,
        &[],
        &[],
        &[],
    );
    let analysis = put_json(&repository.objects, analysis_body.clone());
    let non_analysis = put_json(
        &repository.objects,
        subject_record(
            "urn:uuid:20000000-0000-4000-8000-000000000632",
            "Reachable non-analysis",
        ),
    );
    let orphan_analysis = put_json(
        &repository.objects,
        analysis_result_record(
            "urn:uuid:20000000-0000-4000-8000-000000000633",
            &[("source", &input)],
            &implementation,
            &configuration,
            &[],
            &[],
            &[],
        ),
    );
    let (_, analysis_head) = put_head(&repository.objects, &[("analysis.json", &analysis)], &[]);
    let (_, unrelated_head) =
        put_head(&repository.objects, &[("subject.json", &non_analysis)], &[]);
    let mut projection = SqliteProjectionStore::open_in_memory().unwrap();
    projection
        .rebuild(
            &repository.objects,
            &snapshot(&[
                ("observed/analysis", &analysis_head, 1),
                ("observed/unrelated", &unrelated_head, 2),
            ]),
            GraphLimits::default(),
        )
        .unwrap();

    for unknown_oid in [
        fake_oid("record", 'a'),
        non_analysis.clone(),
        orphan_analysis,
    ] {
        assert_error(
            projection
                .analysis_lineage(&unknown_oid, &RefScope::All)
                .unwrap_err(),
            "projection_analysis_unknown",
            "is not indexed",
        );
    }
    assert_error(
        projection
            .analysis_lineage(&analysis, &RefScope::one("observed/unrelated"))
            .unwrap_err(),
        "projection_analysis_not_reachable",
        "not reachable from the selected Refs",
    );
    assert_error(
        projection
            .analysis_lineage(
                &fake_oid("record", 'b'),
                &RefScope::one("observed/not-there"),
            )
            .unwrap_err(),
        "projection_ref_unknown",
        "not in the projection",
    );
    assert_error(
        projection
            .analysis_lineage(&analysis, &RefScope::one("observed/x' OR 1=1 --"))
            .unwrap_err(),
        "projection_source_invalid",
        "invalid query Ref name",
    );

    let metadata_before = projection.metadata().unwrap().unwrap();
    let mut invalid_analysis_body = analysis_body;
    invalid_analysis_body["payload"]["adapter"]
        .as_object_mut()
        .unwrap()
        .remove("version");
    let invalid_analysis = put_unchecked(&repository.objects, invalid_analysis_body);
    let (_, invalid_head) = put_head(
        &repository.objects,
        &[("invalid-analysis.json", &invalid_analysis)],
        &[],
    );
    let error = projection
        .rebuild(
            &repository.objects,
            &snapshot(&[("observed/invalid-analysis", &invalid_head, 3)]),
            GraphLimits::default(),
        )
        .unwrap_err();
    assert_error(
        error,
        "projection_source_invalid",
        "schema/semantic validation",
    );
    assert_eq!(projection.metadata().unwrap(), Some(metadata_before));
    assert!(
        projection
            .analysis_lineage(&analysis, &RefScope::one("observed/analysis"))
            .is_ok()
    );

    projection
        .rebuild(
            &repository.objects,
            &snapshot(&[("observed/unrelated", &unrelated_head, 2)]),
            GraphLimits::default(),
        )
        .unwrap();
    assert_error(
        projection
            .analysis_lineage(&analysis, &RefScope::All)
            .unwrap_err(),
        "projection_analysis_unknown",
        "is not indexed",
    );
}

#[test]
fn analysis_rows_participate_in_the_global_derived_row_limit_atomically() {
    let repository = TestRepository::new("analysis-derived-limit");
    let inputs = [
        put_json(
            &repository.objects,
            claim_record("urn:uuid:20000000-0000-4000-8000-000000000641", SUBJECT_ID),
        ),
        put_json(
            &repository.objects,
            claim_record("urn:uuid:20000000-0000-4000-8000-000000000642", SUBJECT_ID),
        ),
        put_json(
            &repository.objects,
            claim_record("urn:uuid:20000000-0000-4000-8000-000000000643", SUBJECT_ID),
        ),
        put_json(
            &repository.objects,
            claim_record("urn:uuid:20000000-0000-4000-8000-000000000644", SUBJECT_ID),
        ),
    ];
    let implementation = put_blob(&repository.objects, b"derived limit implementation");
    let configuration = put_blob(&repository.objects, b"derived limit configuration");
    let analysis = put_json(
        &repository.objects,
        analysis_result_record(
            "urn:uuid:20000000-0000-4000-8000-000000000645",
            &[
                ("input-0", &inputs[0]),
                ("input-1", &inputs[1]),
                ("input-2", &inputs[2]),
                ("input-3", &inputs[3]),
            ],
            &implementation,
            &configuration,
            &[],
            &[],
            &[],
        ),
    );
    let (_, head) = put_head(&repository.objects, &[("analysis.json", &analysis)], &[]);
    let refs = snapshot(&[("observed/analysis-derived-limit", &head, 1)]);
    let mut projection = SqliteProjectionStore::open_in_memory().unwrap();
    let baseline = projection
        .rebuild(&repository.objects, &refs, GraphLimits::default())
        .unwrap()
        .metadata;

    let error = projection
        .rebuild(
            &repository.objects,
            &refs,
            GraphLimits {
                max_objects: 20,
                max_edges: 9,
                max_depth: 20,
            },
        )
        .unwrap_err();

    assert_error(error, "resource_limit", "derived rows");
    assert_eq!(projection.metadata().unwrap(), Some(baseline));
    assert!(
        projection
            .analysis_lineage(&analysis, &RefScope::All)
            .is_ok()
    );
}
