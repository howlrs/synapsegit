use rusqlite::Transaction;

use crate::error::Result;

pub(crate) fn create_schema(transaction: &Transaction<'_>) -> Result<()> {
    transaction.execute_batch(
        "CREATE TABLE IF NOT EXISTS ref_heads (
            ref_name TEXT PRIMARY KEY NOT NULL,
            head_oid TEXT NOT NULL,
            updated_event_id INTEGER NOT NULL UNIQUE CHECK(updated_event_id > 0)
        ) STRICT;

        CREATE TABLE IF NOT EXISTS objects (
            oid TEXT PRIMARY KEY NOT NULL,
            kind TEXT NOT NULL CHECK(kind IN ('blob', 'record', 'tree', 'commit')),
            availability TEXT NOT NULL CHECK(availability IN ('present', 'tombstoned', 'missing')),
            byte_len INTEGER CHECK(byte_len IS NULL OR byte_len >= 0),
            tombstone_oid TEXT,
            record_type TEXT,
            entity_id TEXT,
            recorded_at TEXT,
            asserted_by TEXT,
            CHECK((availability = 'present' AND byte_len IS NOT NULL AND tombstone_oid IS NULL)
               OR (availability = 'tombstoned' AND byte_len IS NULL AND tombstone_oid IS NOT NULL)
               OR (availability = 'missing' AND byte_len IS NULL AND tombstone_oid IS NULL))
        ) STRICT;

        CREATE TABLE IF NOT EXISTS ref_reachability (
            ref_name TEXT NOT NULL REFERENCES ref_heads(ref_name) ON DELETE CASCADE,
            oid TEXT NOT NULL REFERENCES objects(oid) ON DELETE CASCADE,
            depth INTEGER NOT NULL CHECK(depth >= 0),
            availability TEXT NOT NULL CHECK(availability IN ('present', 'tombstoned', 'missing')),
            PRIMARY KEY(ref_name, oid)
        ) STRICT;
        CREATE INDEX IF NOT EXISTS ref_reachability_oid_ref
            ON ref_reachability(oid, ref_name);

        CREATE TABLE IF NOT EXISTS object_edges (
            source_oid TEXT NOT NULL REFERENCES objects(oid) ON DELETE CASCADE,
            target_oid TEXT NOT NULL REFERENCES objects(oid) ON DELETE CASCADE,
            role TEXT NOT NULL,
            expected_kind TEXT NOT NULL CHECK(expected_kind IN ('blob', 'record', 'tree', 'commit')),
            PRIMARY KEY(source_oid, target_oid, role)
        ) STRICT;
        CREATE INDEX IF NOT EXISTS object_edges_target
            ON object_edges(target_oid, source_oid);

        CREATE TABLE IF NOT EXISTS records (
            oid TEXT PRIMARY KEY NOT NULL REFERENCES objects(oid) ON DELETE CASCADE,
            record_type TEXT NOT NULL,
            entity_id TEXT NOT NULL,
            recorded_at TEXT NOT NULL,
            asserted_by TEXT NOT NULL
        ) STRICT;
        CREATE INDEX IF NOT EXISTS records_entity_time
            ON records(entity_id, recorded_at, oid);

        CREATE TABLE IF NOT EXISTS subject_links (
            record_oid TEXT NOT NULL REFERENCES records(oid) ON DELETE CASCADE,
            subject_id TEXT NOT NULL,
            PRIMARY KEY(record_oid, subject_id)
        ) STRICT;
        CREATE INDEX IF NOT EXISTS subject_links_subject
            ON subject_links(subject_id, record_oid);

        CREATE TABLE IF NOT EXISTS series_links (
            record_oid TEXT PRIMARY KEY NOT NULL REFERENCES records(oid) ON DELETE CASCADE,
            series_id TEXT NOT NULL
        ) STRICT;
        CREATE INDEX IF NOT EXISTS series_links_series
            ON series_links(series_id, record_oid);

        CREATE TABLE IF NOT EXISTS timeline_records (
            record_oid TEXT PRIMARY KEY NOT NULL REFERENCES records(oid) ON DELETE CASCADE,
            record_kind TEXT NOT NULL CHECK(record_kind IN ('observation', 'activity')),
            entity_id TEXT NOT NULL,
            ordering_time TEXT NOT NULL,
            time_basis TEXT NOT NULL,
            event_time_start TEXT,
            event_time_end TEXT,
            recorded_at TEXT NOT NULL,
            asserted_by TEXT NOT NULL
        ) STRICT;
        CREATE INDEX IF NOT EXISTS timeline_order
            ON timeline_records(ordering_time, record_oid);

        CREATE TABLE IF NOT EXISTS observation_dependencies (
            observation_oid TEXT NOT NULL REFERENCES records(oid) ON DELETE CASCADE,
            dependency_kind TEXT NOT NULL CHECK(dependency_kind IN (
                'capture_profile', 'station', 'station_deployment',
                'calibration', 'environment', 'media'
            )),
            target_ref TEXT NOT NULL,
            target_kind TEXT NOT NULL CHECK(target_kind IN ('entity', 'blob', 'record', 'tree', 'commit')),
            role TEXT,
            ordinal INTEGER NOT NULL CHECK(ordinal >= 0),
            PRIMARY KEY(observation_oid, dependency_kind, ordinal, target_ref)
        ) STRICT;

        CREATE TABLE IF NOT EXISTS analysis_results (
            analysis_oid TEXT PRIMARY KEY NOT NULL REFERENCES records(oid) ON DELETE CASCADE,
            analysis_kind TEXT NOT NULL,
            comparison_kind TEXT NOT NULL CHECK(comparison_kind IN (
                'revision', 'temporal_observation', 'plan_observation',
                'before_after_activity', 'cross_modal', 'intent'
            )),
            status TEXT NOT NULL CHECK(status IN ('succeeded', 'failed', 'not_run')),
            comparability TEXT NOT NULL CHECK(comparability IN (
                'comparable', 'partial', 'incomparable'
            )),
            adapter_id TEXT NOT NULL,
            adapter_version TEXT NOT NULL,
            implementation_oid TEXT NOT NULL REFERENCES objects(oid) ON DELETE CASCADE,
            configuration_oid TEXT NOT NULL REFERENCES objects(oid) ON DELETE CASCADE,
            determinism TEXT NOT NULL CHECK(determinism IN (
                'deterministic', 'seeded', 'probabilistic'
            )),
            seed TEXT,
            CHECK(determinism <> 'seeded' OR seed IS NOT NULL)
        ) STRICT;
        CREATE INDEX IF NOT EXISTS analysis_results_implementation
            ON analysis_results(implementation_oid, analysis_oid);
        CREATE INDEX IF NOT EXISTS analysis_results_configuration
            ON analysis_results(configuration_oid, analysis_oid);

        CREATE TABLE IF NOT EXISTS analysis_links (
            analysis_oid TEXT NOT NULL REFERENCES analysis_results(analysis_oid) ON DELETE CASCADE,
            category TEXT NOT NULL CHECK(category IN (
                'input', 'transform', 'derived_blob', 'mask'
            )),
            ordinal INTEGER NOT NULL CHECK(ordinal >= 0),
            role TEXT,
            target_oid TEXT NOT NULL REFERENCES objects(oid) ON DELETE CASCADE,
            PRIMARY KEY(analysis_oid, category, ordinal),
            CHECK((category IN ('input', 'mask') AND role IS NOT NULL)
               OR (category IN ('transform', 'derived_blob') AND role IS NULL)),
            CHECK(category <> 'mask' OR role IN (
                'changed', 'unchanged', 'ambiguous', 'unobservable', 'validity'
            ))
        ) STRICT;
        CREATE INDEX IF NOT EXISTS analysis_links_target
            ON analysis_links(target_oid, analysis_oid, category);

        CREATE TABLE IF NOT EXISTS closure_summaries (
            ref_name TEXT PRIMARY KEY NOT NULL REFERENCES ref_heads(ref_name) ON DELETE CASCADE,
            head_oid TEXT NOT NULL,
            complete INTEGER NOT NULL CHECK(complete IN (0, 1)),
            truncated INTEGER NOT NULL CHECK(truncated IN (0, 1)),
            issue_count INTEGER NOT NULL CHECK(issue_count >= 0),
            present_count INTEGER NOT NULL CHECK(present_count >= 0),
            tombstoned_count INTEGER NOT NULL CHECK(tombstoned_count >= 0),
            missing_count INTEGER NOT NULL CHECK(missing_count >= 0)
        ) STRICT;

        CREATE TABLE IF NOT EXISTS closure_issues (
            ref_name TEXT NOT NULL REFERENCES ref_heads(ref_name) ON DELETE CASCADE,
            ordinal INTEGER NOT NULL CHECK(ordinal >= 0),
            oid TEXT NOT NULL,
            referenced_by TEXT,
            role TEXT,
            issue_kind TEXT NOT NULL,
            detail TEXT,
            PRIMARY KEY(ref_name, ordinal)
        ) STRICT;",
    )?;
    Ok(())
}
