use serde_json::{Map as JsonMap, Value as JsonValue, json};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use synapse_canonical::{canonical_bytes, parse_strict};
use synapse_core::Repository;
use synapse_observation::{
    AnalysisComparability, AnalysisStatus, ByteIdentityComparisonRequest, ByteIdentityOutcome,
    ObservationAnalysisError, byte_identity_configuration_oid, byte_identity_implementation_oid,
    record_byte_identity_comparison,
};
use synapse_sqlite::{RefUpdate, ReflogMetadata};

const CREATOR_ID: &str = "urn:uuid:00000000-0000-4000-8000-000000000001";
const TOOL_ID: &str = "urn:uuid:00000000-0000-4000-8000-000000000002";
const SUBJECT_ID: &str = "urn:uuid:00000000-0000-4000-8000-000000000003";
const OTHER_SUBJECT_ID: &str = "urn:uuid:00000000-0000-4000-8000-000000000004";
const SERIES_ID: &str = "urn:uuid:00000000-0000-4000-8000-000000000005";
const OTHER_SERIES_ID: &str = "urn:uuid:00000000-0000-4000-8000-000000000006";
const PROFILE_ID: &str = "urn:uuid:00000000-0000-4000-8000-000000000007";
const RECORDED_AT: &str = "2026-07-13T13:30:00.000000000Z";

static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

struct TempDirectory(PathBuf);

impl TempDirectory {
    fn new() -> Self {
        let sequence = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "synapse-observation-test-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
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

#[test]
fn identical_different_and_swapped_inputs_are_distinct_and_restorable() {
    let temporary = TempDirectory::new();
    let repository_path = temporary.join("repo");
    let mut repository = Repository::open(&repository_path).unwrap();
    let profile_oid = put_profile(&repository);
    let first_blob = repository.put_blob(&b"first image"[..]).unwrap().oid;
    let second_blob = repository.put_blob(&b"second image"[..]).unwrap().oid;
    let base_observation = put_observation(
        &repository,
        "urn:uuid:10000000-0000-4000-8000-000000000001",
        SUBJECT_ID,
        SERIES_ID,
        &[(&first_blob, "primary")],
        Some(&profile_oid),
    );
    let same_observation = put_observation(
        &repository,
        "urn:uuid:10000000-0000-4000-8000-000000000002",
        SUBJECT_ID,
        SERIES_ID,
        &[(&first_blob, "primary")],
        Some(&profile_oid),
    );
    let different_observation = put_observation(
        &repository,
        "urn:uuid:10000000-0000-4000-8000-000000000003",
        SUBJECT_ID,
        SERIES_ID,
        &[(&second_blob, "primary")],
        Some(&profile_oid),
    );

    let identical = record_byte_identity_comparison(
        &repository,
        &request(
            &base_observation,
            &same_observation,
            "urn:uuid:20000000-0000-4000-8000-000000000001",
        ),
    )
    .unwrap();
    assert_eq!(identical.outcome, ByteIdentityOutcome::Identical);
    assert_eq!(identical.status, AnalysisStatus::Succeeded);
    assert_eq!(identical.comparability, AnalysisComparability::Partial);
    assert_eq!(
        identical.reason_codes,
        [
            "byte_identity_only",
            "capture_profile_imported",
            "capture_time_unknown"
        ]
    );

    let different = record_byte_identity_comparison(
        &repository,
        &request(
            &base_observation,
            &different_observation,
            "urn:uuid:20000000-0000-4000-8000-000000000002",
        ),
    )
    .unwrap();
    assert_eq!(different.outcome, ByteIdentityOutcome::Different);
    assert_eq!(
        different.base_media_oid.as_deref(),
        Some(first_blob.as_str())
    );
    assert_eq!(
        different.target_media_oid.as_deref(),
        Some(second_blob.as_str())
    );
    assert_eq!(
        different.implementation_oid,
        byte_identity_implementation_oid()
    );
    assert_eq!(
        different.configuration_oid,
        byte_identity_configuration_oid()
    );

    let swapped = record_byte_identity_comparison(
        &repository,
        &request(
            &different_observation,
            &base_observation,
            "urn:uuid:20000000-0000-4000-8000-000000000002",
        ),
    )
    .unwrap();
    assert_ne!(swapped.analysis_oid, different.analysis_oid);
    let swapped_json = read_json(&repository, &swapped.analysis_oid);
    assert_eq!(
        swapped_json["payload"]["inputs"][0]["ref"],
        different_observation
    );
    assert_eq!(
        swapped_json["payload"]["inputs"][1]["ref"],
        base_observation
    );

    let different_json = read_json(&repository, &different.analysis_oid);
    assert_eq!(different_json["payload"]["analysis_kind"], "byte_identity");
    assert_eq!(different_json["payload"]["status"], "succeeded");
    assert_eq!(different_json["payload"]["comparability"], "partial");
    assert_eq!(
        different_json["payload"]["metrics"]["byte_identical"],
        json!({ "mantissa": "0", "scale": 0, "unit": "unitless" })
    );
    assert!(
        different_json["payload"]["warnings"][0]
            .as_str()
            .unwrap()
            .contains("do not establish visual or physical change")
    );
    assert!(
        repository.refs().list().unwrap().is_empty(),
        "adapter recording must not publish a Ref"
    );

    let head = publish_analysis(
        &mut repository,
        &different.analysis_oid,
        &[
            (&profile_oid, "record", "capture-profile.json"),
            (&base_observation, "record", "base.observation.json"),
            (&different_observation, "record", "target.observation.json"),
            (&different.analysis_oid, "record", "analysis.json"),
            (&first_blob, "blob", "base.image"),
            (&second_blob, "blob", "target.image"),
            (&different.implementation_oid, "blob", "adapter.source"),
            (&different.configuration_oid, "blob", "adapter.config"),
        ],
    );
    assert!(repository.fsck().unwrap().is_clean());

    let archive = temporary.join("archive");
    let restored = temporary.join("restored");
    repository.export_archive(&archive).unwrap();
    Repository::restore_archive(&archive, &restored).unwrap();
    let restored_repository = Repository::open(&restored).unwrap();
    assert_eq!(
        restored_repository
            .refs()
            .get("proposal/observation-analysis/test")
            .unwrap()
            .unwrap()
            .head,
        head
    );
    assert_eq!(
        read_json(&restored_repository, &different.analysis_oid),
        different_json
    );
}

#[test]
fn subject_series_and_primary_role_problems_are_incomparable_results() {
    let temporary = TempDirectory::new();
    let repository = Repository::open(temporary.join("repo")).unwrap();
    let blob_a = repository.put_blob(&b"a"[..]).unwrap().oid;
    let blob_b = repository.put_blob(&b"b"[..]).unwrap().oid;
    let base = put_observation(
        &repository,
        "urn:uuid:30000000-0000-4000-8000-000000000001",
        SUBJECT_ID,
        SERIES_ID,
        &[(&blob_a, "primary")],
        None,
    );
    let cases = [
        (
            put_observation(
                &repository,
                "urn:uuid:30000000-0000-4000-8000-000000000002",
                OTHER_SUBJECT_ID,
                SERIES_ID,
                &[(&blob_b, "primary")],
                None,
            ),
            "subject_mismatch",
        ),
        (
            put_observation(
                &repository,
                "urn:uuid:30000000-0000-4000-8000-000000000003",
                SUBJECT_ID,
                OTHER_SERIES_ID,
                &[(&blob_b, "primary")],
                None,
            ),
            "series_mismatch",
        ),
        (
            put_observation(
                &repository,
                "urn:uuid:30000000-0000-4000-8000-000000000004",
                SUBJECT_ID,
                SERIES_ID,
                &[(&blob_b, "preview")],
                None,
            ),
            "primary_media_missing",
        ),
        (
            put_observation(
                &repository,
                "urn:uuid:30000000-0000-4000-8000-000000000005",
                SUBJECT_ID,
                SERIES_ID,
                &[(&blob_a, "primary"), (&blob_b, "primary")],
                None,
            ),
            "primary_media_ambiguous",
        ),
    ];
    for (index, (target, reason)) in cases.into_iter().enumerate() {
        let result = record_byte_identity_comparison(
            &repository,
            &request(
                &base,
                &target,
                &format!("urn:uuid:40000000-0000-4000-8000-{index:012}"),
            ),
        )
        .unwrap();
        assert_eq!(result.outcome, ByteIdentityOutcome::NotCompared);
        assert_eq!(result.status, AnalysisStatus::NotRun);
        assert_eq!(result.comparability, AnalysisComparability::Incomparable);
        assert!(result.reason_codes.iter().any(|value| value == reason));
        let analysis = read_json(&repository, &result.analysis_oid);
        assert!(
            analysis["payload"]["metrics"]
                .as_object()
                .unwrap()
                .is_empty()
        );
    }
}

#[test]
fn missing_blob_and_invalid_metadata_fail_before_adapter_publication() {
    let temporary = TempDirectory::new();
    let repository = Repository::open(temporary.join("repo")).unwrap();
    let present_blob = repository.put_blob(&b"present"[..]).unwrap().oid;
    let missing_blob =
        "blob:sg-oid-v1:sha256:ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff";
    let base = put_observation(
        &repository,
        "urn:uuid:50000000-0000-4000-8000-000000000001",
        SUBJECT_ID,
        SERIES_ID,
        &[(&present_blob, "primary")],
        None,
    );
    let target = put_observation(
        &repository,
        "urn:uuid:50000000-0000-4000-8000-000000000002",
        SUBJECT_ID,
        SERIES_ID,
        &[(missing_blob, "primary")],
        None,
    );
    let before = repository.objects().list_oids().unwrap();
    let error = record_byte_identity_comparison(
        &repository,
        &request(
            &base,
            &target,
            "urn:uuid:50000000-0000-4000-8000-000000000003",
        ),
    )
    .unwrap_err();
    assert!(matches!(error, ObservationAnalysisError::MissingObject(_)));
    assert_eq!(repository.objects().list_oids().unwrap(), before);

    let missing_preview_target = put_observation(
        &repository,
        "urn:uuid:50000000-0000-4000-8000-000000000006",
        SUBJECT_ID,
        SERIES_ID,
        &[(&present_blob, "primary"), (missing_blob, "preview")],
        None,
    );
    let before = repository.objects().list_oids().unwrap();
    let error = record_byte_identity_comparison(
        &repository,
        &request(
            &base,
            &missing_preview_target,
            "urn:uuid:50000000-0000-4000-8000-000000000007",
        ),
    )
    .unwrap_err();
    assert!(matches!(error, ObservationAnalysisError::MissingObject(_)));
    assert_eq!(repository.objects().list_oids().unwrap(), before);

    let valid_target = put_observation(
        &repository,
        "urn:uuid:50000000-0000-4000-8000-000000000004",
        SUBJECT_ID,
        SERIES_ID,
        &[(&present_blob, "primary")],
        None,
    );
    let before = repository.objects().list_oids().unwrap();
    let mut invalid_request = request(&base, &valid_target, "not-an-entity-id");
    invalid_request.recorded_at = "not-a-timestamp".into();
    assert!(record_byte_identity_comparison(&repository, &invalid_request).is_err());
    assert_eq!(repository.objects().list_oids().unwrap(), before);

    let blob_path = stored_object_path(&repository, &present_blob);
    fs::write(blob_path, b"corrupt bytes").unwrap();
    let before = repository.objects().list_oids().unwrap();
    let error = record_byte_identity_comparison(
        &repository,
        &request(
            &base,
            &valid_target,
            "urn:uuid:50000000-0000-4000-8000-000000000005",
        ),
    )
    .unwrap_err();
    assert_eq!(error.code(), "oid_mismatch");
    assert_eq!(repository.objects().list_oids().unwrap(), before);
}

#[test]
fn implementation_configuration_and_result_oids_are_stable_across_repositories() {
    let temporary = TempDirectory::new();
    let mut receipts = Vec::new();
    for name in ["first", "second"] {
        let repository = Repository::open(temporary.join(name)).unwrap();
        let blob = repository.put_blob(&b"same bytes"[..]).unwrap().oid;
        let base = put_observation(
            &repository,
            "urn:uuid:60000000-0000-4000-8000-000000000001",
            SUBJECT_ID,
            SERIES_ID,
            &[(&blob, "primary")],
            None,
        );
        let target = put_observation(
            &repository,
            "urn:uuid:60000000-0000-4000-8000-000000000002",
            SUBJECT_ID,
            SERIES_ID,
            &[(&blob, "primary")],
            None,
        );
        receipts.push(
            record_byte_identity_comparison(
                &repository,
                &request(
                    &base,
                    &target,
                    "urn:uuid:60000000-0000-4000-8000-000000000003",
                ),
            )
            .unwrap(),
        );
    }
    assert_eq!(receipts[0], receipts[1]);
}

fn request(base: &str, target: &str, entity_id: &str) -> ByteIdentityComparisonRequest {
    ByteIdentityComparisonRequest {
        base_observation_oid: base.to_owned(),
        target_observation_oid: target.to_owned(),
        analysis_entity_id: entity_id.to_owned(),
        asserted_by: TOOL_ID.into(),
        recorded_at: RECORDED_AT.into(),
    }
}

fn put_profile(repository: &Repository) -> String {
    put_json(
        repository,
        json!({
            "object_type": "record",
            "schema_version": "0.1.0",
            "record_type": "capture_profile",
            "entity_id": PROFILE_ID,
            "recorded_at": RECORDED_AT,
            "asserted_by": CREATOR_ID,
            "origin": "tool_recorded",
            "source_refs": [],
            "payload": {
                "profile_level": "imported",
                "required_conditions": [],
                "allowed_claims": ["reference_only"],
                "description": "test imported profile"
            },
            "extensions": {}
        }),
    )
}

fn put_observation(
    repository: &Repository,
    entity_id: &str,
    subject_id: &str,
    series_id: &str,
    media: &[(&str, &str)],
    capture_profile_oid: Option<&str>,
) -> String {
    let mut payload = json!({
        "subject_ref": subject_id,
        "series_ref": series_id,
        "capture_time": { "kind": "unknown", "reason": "test import" },
        "media_refs": canonical_set(
            media
                .iter()
                .map(|(oid, role)| json!({ "role": role, "oid": oid }))
                .collect()
        ),
        "calibration_refs": [],
        "protocol_deviations": [],
        "environment_refs": [],
        "missing_regions": []
    });
    if let Some(profile_oid) = capture_profile_oid {
        payload
            .as_object_mut()
            .unwrap()
            .insert("capture_profile_ref".into(), json!(profile_oid));
    }
    put_json(
        repository,
        json!({
            "object_type": "record",
            "schema_version": "0.1.0",
            "record_type": "observation",
            "entity_id": entity_id,
            "recorded_at": RECORDED_AT,
            "asserted_by": CREATOR_ID,
            "origin": "imported",
            "source_refs": [],
            "payload": payload,
            "extensions": {}
        }),
    )
}

fn publish_analysis(
    repository: &mut Repository,
    analysis_oid: &str,
    entries: &[(&str, &str, &str)],
) -> String {
    let mut manifest_entries = JsonMap::new();
    for (oid, kind, name) in entries {
        manifest_entries.insert((*name).into(), json!({ "entry_kind": kind, "oid": oid }));
    }
    let tree = put_json(
        repository,
        json!({
            "object_type": "tree",
            "schema_version": "0.1.0",
            "entries": manifest_entries,
            "extensions": {}
        }),
    );
    let commit = put_json(
        repository,
        json!({
            "object_type": "commit",
            "schema_version": "0.1.0",
            "commit_kind": "checkpoint",
            "parents": [],
            "snapshot": tree,
            "transition_refs": [analysis_oid],
            "bound_declaration_refs": [],
            "author_ref": TOOL_ID,
            "authored_at": RECORDED_AT,
            "message": "Record byte identity Observation analysis",
            "extensions": {}
        }),
    );
    repository
        .update_ref(RefUpdate {
            ref_name: "proposal/observation-analysis/test",
            expected_head: None,
            new_head: &commit,
            metadata: ReflogMetadata::at(1),
        })
        .unwrap();
    commit
}

fn put_json(repository: &Repository, value: JsonValue) -> String {
    repository
        .put_object(&serde_json::to_vec(&value).unwrap())
        .unwrap()
        .oid
}

fn read_json(repository: &Repository, oid: &str) -> JsonValue {
    serde_json::from_slice(&repository.objects().read_raw(oid).unwrap().unwrap()).unwrap()
}

fn canonical_set(values: Vec<JsonValue>) -> JsonValue {
    let mut keyed = values
        .into_iter()
        .map(|value| {
            let parsed = parse_strict(&serde_json::to_vec(&value).unwrap()).unwrap();
            (canonical_bytes(&parsed).unwrap(), value)
        })
        .collect::<Vec<_>>();
    keyed.sort_by(|left, right| left.0.cmp(&right.0));
    JsonValue::Array(keyed.into_iter().map(|(_, value)| value).collect())
}

fn stored_object_path(repository: &Repository, oid: &str) -> PathBuf {
    let mut parts = oid.rsplit(':');
    let digest = parts.next().unwrap();
    let kind = oid.split(':').next().unwrap();
    repository
        .root()
        .join("cas")
        .join("objects")
        .join(kind)
        .join(&digest[..2])
        .join(&digest[2..])
}
