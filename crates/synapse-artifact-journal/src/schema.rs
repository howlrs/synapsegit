use rusqlite::Connection;

use crate::error::Result;

pub(crate) const SCHEMA_VERSION: i64 = 2;

pub(crate) fn create_schema_v2(connection: &Connection) -> Result<()> {
    connection.execute_batch(
        "CREATE TABLE reviews (
            review_id TEXT PRIMARY KEY NOT NULL,
            project_scope TEXT NOT NULL,
            proposal_ref_name TEXT NOT NULL,
            proposal_head TEXT NOT NULL,
            decision_ref_name TEXT NOT NULL,
            expected_decision_head TEXT NOT NULL,
            state TEXT NOT NULL CHECK (state IN (
                'pending_review',
                'decision_committed',
                'terminal_denial',
                'retryable_failure',
                'outcome_unknown'
            )),
            UNIQUE(project_scope, proposal_ref_name, proposal_head)
        );
        CREATE TABLE decision_intents (
            review_id TEXT PRIMARY KEY NOT NULL
                REFERENCES reviews(review_id) ON DELETE RESTRICT,
            idempotency_digest TEXT NOT NULL,
            request_fingerprint TEXT NOT NULL,
            candidate_head TEXT NOT NULL,
            feedback_oid TEXT NOT NULL,
            expected_decision_head TEXT NOT NULL
        );
        CREATE UNIQUE INDEX decision_intents_idempotency
            ON decision_intents(review_id, idempotency_digest);",
    )?;
    create_v2_extension_tables(connection)?;
    connection.execute_batch("PRAGMA user_version = 2;")?;
    Ok(())
}

pub(crate) fn migrate_schema_v1_to_v2(connection: &Connection) -> Result<()> {
    create_v2_extension_tables(connection)?;
    connection.execute_batch("PRAGMA user_version = 2;")?;
    Ok(())
}

fn create_v2_extension_tables(connection: &Connection) -> Result<()> {
    connection.execute_batch(
        "CREATE TABLE proposal_intents (
            proposal_intent_id TEXT PRIMARY KEY NOT NULL
                CHECK (
                    length(proposal_intent_id) = 64
                    AND proposal_intent_id NOT GLOB '*[^0-9a-f]*'
                ),
            idempotency_digest TEXT NOT NULL,
            request_fingerprint TEXT NOT NULL,
            artifact_manifest_sha256 TEXT NOT NULL CHECK (
                length(artifact_manifest_sha256) = 64
                AND artifact_manifest_sha256 NOT GLOB '*[^0-9a-f]*'
            ),
            project_scope TEXT NOT NULL,
            proposal_ref_name TEXT NOT NULL,
            proposal_head TEXT NOT NULL,
            decision_ref_name TEXT NOT NULL,
            expected_decision_head TEXT NOT NULL,
            review_id TEXT UNIQUE
                REFERENCES reviews(review_id) ON DELETE RESTRICT,
            UNIQUE(project_scope, idempotency_digest),
            UNIQUE(project_scope, proposal_ref_name, proposal_head)
        );
        CREATE INDEX proposal_intents_unfinalized
            ON proposal_intents(project_scope, proposal_intent_id)
            WHERE review_id IS NULL;
        CREATE TABLE decision_commit_intents (
            review_id TEXT PRIMARY KEY NOT NULL
                REFERENCES decision_intents(review_id) ON DELETE RESTRICT,
            project_scope TEXT NOT NULL,
            proposal_ref_name TEXT NOT NULL,
            proposal_head TEXT NOT NULL,
            decision_ref_name TEXT NOT NULL,
            disposition TEXT NOT NULL CHECK (disposition IN (
                'adopted_unchanged', 'rejected', 'deferred'
            )),
            selected_snapshot TEXT NOT NULL CHECK (selected_snapshot IN (
                'base', 'proposal'
            )),
            reviewed_artifact_manifest_sha256 TEXT NOT NULL CHECK (
                length(reviewed_artifact_manifest_sha256) = 64
                AND reviewed_artifact_manifest_sha256 NOT GLOB '*[^0-9a-f]*'
            ),
            CHECK (
                (disposition = 'adopted_unchanged' AND selected_snapshot = 'proposal')
                OR (disposition IN ('rejected', 'deferred') AND selected_snapshot = 'base')
            )
        );
        CREATE TABLE decision_outcomes (
            review_id TEXT PRIMARY KEY NOT NULL
                REFERENCES decision_commit_intents(review_id) ON DELETE RESTRICT,
            disposition TEXT NOT NULL CHECK (disposition IN (
                'adopted_unchanged', 'rejected', 'deferred'
            )),
            selected_snapshot TEXT NOT NULL CHECK (selected_snapshot IN (
                'base', 'proposal'
            )),
            reviewed_artifact_manifest_sha256 TEXT NOT NULL CHECK (
                length(reviewed_artifact_manifest_sha256) = 64
                AND reviewed_artifact_manifest_sha256 NOT GLOB '*[^0-9a-f]*'
            ),
            proposal_head TEXT NOT NULL,
            expected_decision_head TEXT NOT NULL,
            new_decision_head TEXT NOT NULL,
            feedback_oid TEXT NOT NULL,
            CHECK (expected_decision_head <> new_decision_head),
            CHECK (
                (disposition = 'adopted_unchanged' AND selected_snapshot = 'proposal')
                OR (disposition IN ('rejected', 'deferred') AND selected_snapshot = 'base')
            )
        );",
    )?;
    Ok(())
}
