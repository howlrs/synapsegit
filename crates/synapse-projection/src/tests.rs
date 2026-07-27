use rusqlite::Connection;
use synapse_canonical::ObjectKind;
use synapse_sqlite::RefRecord;

use crate::error::ProjectionError;
use crate::rebuild::{BuildPlan, EdgeRow, ObjectRow, ReachabilityRow, SummaryRow};
use crate::store::{ObjectAvailability, SqliteProjectionStore};

#[test]
fn transaction_failure_after_clear_preserves_previous_projection() {
    let mut store = SqliteProjectionStore::open_in_memory().unwrap();
    let first = BuildPlan {
        source_fingerprint: "projection-source-v1:sha256:first".into(),
        ..BuildPlan::default()
    };
    let first_metadata = first.metadata();
    store.replace(&first, &first_metadata).unwrap();

    store
        .connection
        .execute_batch(
            "CREATE TRIGGER inject_projection_failure
             BEFORE INSERT ON ref_heads
             BEGIN
                SELECT RAISE(ABORT, 'injected projection replacement failure');
             END;",
        )
        .unwrap();
    let oid =
        "commit:sg-oid-v1:sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    let mut second = BuildPlan {
        refs: vec![RefRecord {
            name: "decision/main".into(),
            head: oid.into(),
            updated_event_id: 1,
        }],
        source_fingerprint: "projection-source-v1:sha256:second".into(),
        ..BuildPlan::default()
    };
    second.objects.insert(
        oid.into(),
        ObjectRow {
            oid: oid.into(),
            kind: ObjectKind::Commit,
            availability: ObjectAvailability::Present,
            byte_len: Some(1),
            tombstone_oid: None,
            record_type: None,
            entity_id: None,
            recorded_at: None,
            asserted_by: None,
        },
    );
    second.reachability.insert(ReachabilityRow {
        ref_name: "decision/main".into(),
        oid: oid.into(),
        depth: 0,
        availability: ObjectAvailability::Present,
    });
    second.summaries.insert(SummaryRow {
        ref_name: "decision/main".into(),
        head_oid: oid.into(),
        complete: true,
        truncated: false,
        issue_count: 0,
        present_count: 1,
        tombstoned_count: 0,
        missing_count: 0,
    });

    let error = store.replace(&second, &second.metadata()).unwrap_err();
    assert_eq!(error.code(), "storage_error");
    assert_eq!(store.metadata().unwrap(), Some(first_metadata));
    assert!(store.get_object(oid).unwrap().is_none());
}

#[test]
fn unsupported_existing_schema_is_rejected() {
    let connection = Connection::open_in_memory().unwrap();
    connection
        .execute_batch(
            "CREATE TABLE projection_meta (
                key TEXT PRIMARY KEY NOT NULL,
                value TEXT NOT NULL
             ) STRICT;
             INSERT INTO projection_meta(key, value)
             VALUES ('schema_version', '999');",
        )
        .unwrap();
    let error = SqliteProjectionStore::initialize(connection)
        .err()
        .expect("schema mismatch must fail");
    assert!(matches!(
        error,
        ProjectionError::UnsupportedSchemaVersion { ref found } if found == "999"
    ));
}

#[test]
fn analysis_target_requires_a_direct_typed_graph_edge() {
    let analysis_oid =
        "record:sg-oid-v1:sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    let unrelated_oid =
        "record:sg-oid-v1:sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    let target_oid =
        "blob:sg-oid-v1:sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";
    let mut plan = BuildPlan::default();
    plan.objects.insert(
        target_oid.into(),
        ObjectRow {
            oid: target_oid.into(),
            kind: ObjectKind::Blob,
            availability: ObjectAvailability::Present,
            byte_len: Some(1),
            tombstone_oid: None,
            record_type: None,
            entity_id: None,
            recorded_at: None,
            asserted_by: None,
        },
    );
    plan.edges.insert(EdgeRow {
        source_oid: unrelated_oid.into(),
        target_oid: target_oid.into(),
        role: "unrelated".into(),
        expected_kind: "blob".into(),
    });

    let error = plan
        .validate_analysis_target(analysis_oid, "inputs[].ref", target_oid, None)
        .unwrap_err();
    assert!(matches!(error, ProjectionError::InvalidSource(_)));
    assert!(
        error
            .to_string()
            .contains("no matching verified graph edge")
    );

    plan.edges.insert(EdgeRow {
        source_oid: analysis_oid.into(),
        target_oid: target_oid.into(),
        role: "/payload/inputs/0/ref".into(),
        expected_kind: "blob".into(),
    });
    plan.validate_analysis_target(analysis_oid, "inputs[].ref", target_oid, None)
        .unwrap();
}
