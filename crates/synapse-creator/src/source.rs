use crate::{CreatorError, CreatorReport, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use synapse_canonical::{ObjectKind, parse_oid};

pub const CREATOR_SOURCE_KEY: &str = "org.synapsegit.creator-source";
pub const CREATOR_REUSE_SOURCE_KEY: &str = "org.synapsegit.creator-reuse-source";
pub const CREATOR_SOURCE_FORMAT: &str = "synapsegit-creator-source-v1";
pub const CREATOR_REUSE_SOURCE_FORMAT: &str = "synapsegit-creator-reuse-source-v1";
pub const CREATOR_MAX_SOURCE_DEPTH: usize = 16;

/// Private immutable binding to the complete source whose reference images were reused.
/// Browser callers receive an opaque confirmation locator, never this write authority.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreatorSourceBinding {
    pub format: String,
    pub session: String,
    pub proposal_head: String,
    pub decision_head: String,
    pub disposition: String,
    pub original_blob_oid: String,
    pub current_blob_oid: String,
}

/// Private immutable binding for a new review which reuses all three recorded
/// blobs.  It is evidence only: it never recreates the prior Human permit.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreatorReuseSourceBinding {
    pub format: String,
    pub kind: String,
    pub session: String,
    pub proposal_head: String,
    pub decision_head: String,
    pub original_blob_oid: String,
    pub current_blob_oid: String,
    pub ai_output_blob_oid: String,
}

impl CreatorReuseSourceBinding {
    pub fn new(report: &CreatorReport, kind: &str) -> Result<Self> {
        let value = Self {
            format: CREATOR_REUSE_SOURCE_FORMAT.into(),
            kind: kind.into(),
            session: report.session.clone(),
            proposal_head: report.proposal_head.clone(),
            decision_head: report.decision_head.clone(),
            original_blob_oid: report.original_blob_oid.clone(),
            current_blob_oid: report.current_blob_oid.clone(),
            ai_output_blob_oid: report.ai_output_blob_oid.clone(),
        };
        value.validate_shape()?;
        Ok(value)
    }

    pub(crate) fn validate_shape(&self) -> Result<()> {
        crate::session::validate_session(&self.session)?;
        if self.format != CREATOR_REUSE_SOURCE_FORMAT
            || !matches!(
                self.kind.as_str(),
                "interrupted_pending" | "deferred_rereview"
            )
        {
            return Err(invalid_source());
        }
        for (oid, kind) in [
            (&self.proposal_head, ObjectKind::Commit),
            (&self.decision_head, ObjectKind::Commit),
            (&self.original_blob_oid, ObjectKind::Blob),
            (&self.current_blob_oid, ObjectKind::Blob),
            (&self.ai_output_blob_oid, ObjectKind::Blob),
        ] {
            if parse_oid(oid).ok() != Some(kind) {
                return Err(invalid_source());
            }
        }
        Ok(())
    }

    pub(crate) fn is_interrupted(&self) -> bool {
        self.kind == "interrupted_pending"
    }
    pub(crate) fn matches_report(&self, report: &CreatorReport) -> bool {
        self.session == report.session
            && self.proposal_head == report.proposal_head
            && self.decision_head == report.decision_head
            && self.original_blob_oid == report.original_blob_oid
            && self.current_blob_oid == report.current_blob_oid
            && self.ai_output_blob_oid == report.ai_output_blob_oid
    }
}

impl CreatorSourceBinding {
    pub fn from_report(report: &CreatorReport) -> Self {
        Self {
            format: CREATOR_SOURCE_FORMAT.into(),
            session: report.session.clone(),
            proposal_head: report.proposal_head.clone(),
            decision_head: report.decision_head.clone(),
            disposition: report.disposition.as_cli_str().into(),
            original_blob_oid: report.original_blob_oid.clone(),
            current_blob_oid: report.current_blob_oid.clone(),
        }
    }

    pub(crate) fn validate_shape(&self) -> Result<()> {
        crate::session::validate_session(&self.session)?;
        if self.format != CREATOR_SOURCE_FORMAT
            || crate::CreatorDisposition::parse(&self.disposition).is_err()
        {
            return Err(invalid_source());
        }
        for (oid, kind) in [
            (&self.proposal_head, ObjectKind::Commit),
            (&self.decision_head, ObjectKind::Commit),
            (&self.original_blob_oid, ObjectKind::Blob),
            (&self.current_blob_oid, ObjectKind::Blob),
        ] {
            if parse_oid(oid).ok() != Some(kind) {
                return Err(invalid_source());
            }
        }
        Ok(())
    }

    pub(crate) fn matches_report(&self, report: &CreatorReport) -> bool {
        self == &Self::from_report(report)
    }
}

pub(crate) fn invalid_source() -> CreatorError {
    CreatorError::ReportInvalid(
        "creator source binding is invalid or no longer matches the verified source".into(),
    )
}

pub(crate) fn attach_source(record: &mut Value, source: &CreatorSourceBinding) -> Result<()> {
    source.validate_shape()?;
    let value = serde_json::to_value(source).map_err(|_| invalid_source())?;
    record
        .get_mut("extensions")
        .and_then(Value::as_object_mut)
        .ok_or_else(invalid_source)?
        .insert(CREATOR_SOURCE_KEY.into(), value);
    // These typed Core input edges retain both exact source heads and their closure.
    // Extension strings alone would not retain archive reachability.
    record["payload"]["input_refs"] = json!(crate::records::canonical_set(vec![
        json!({"role":"source_decision", "oid":source.decision_head}),
        json!({"role":"source_proposal", "oid":source.proposal_head}),
    ]));
    record["payload"]["summary"] = json!(
        "Reused recorded Original and Current image bytes from a completed session as references, without a new capture."
    );
    Ok(())
}

pub(crate) fn attach_reuse_source(
    record: &mut Value,
    source: &CreatorReuseSourceBinding,
) -> Result<()> {
    source.validate_shape()?;
    record
        .get_mut("extensions")
        .and_then(Value::as_object_mut)
        .ok_or_else(invalid_source)?
        .insert(
            CREATOR_REUSE_SOURCE_KEY.into(),
            serde_json::to_value(source).map_err(|_| invalid_source())?,
        );
    record["payload"]["input_refs"] = json!(crate::records::canonical_set(vec![
        json!({"role":"source_decision", "oid":source.decision_head}),
        json!({"role":"source_proposal", "oid":source.proposal_head}),
    ]));
    record["payload"]["summary"] = json!(
        "Reused recorded Original, Current, and AI output bytes in a new Human review without an AI execution."
    );
    Ok(())
}

pub(crate) fn read_source(record: &Value) -> Result<Option<CreatorSourceBinding>> {
    let Some(value) = record
        .get("extensions")
        .and_then(|e| e.get(CREATOR_SOURCE_KEY))
    else {
        return Ok(None);
    };
    let source: CreatorSourceBinding =
        serde_json::from_value(value.clone()).map_err(|_| invalid_source())?;
    source.validate_shape().map_err(|_| invalid_source())?;
    let expected = json!(crate::records::canonical_set(vec![
        json!({"role":"source_decision", "oid":source.decision_head}),
        json!({"role":"source_proposal", "oid":source.proposal_head}),
    ]));
    if record["record_type"] != "activity"
        || record["payload"]["activity_kind"] != "import"
        || record["payload"]["input_refs"] != expected
    {
        return Err(invalid_source());
    }
    Ok(Some(source))
}

pub(crate) fn read_reuse_source(record: &Value) -> Result<Option<CreatorReuseSourceBinding>> {
    let Some(value) = record
        .get("extensions")
        .and_then(|e| e.get(CREATOR_REUSE_SOURCE_KEY))
    else {
        return Ok(None);
    };
    let source: CreatorReuseSourceBinding =
        serde_json::from_value(value.clone()).map_err(|_| invalid_source())?;
    source.validate_shape()?;
    let expected = json!(crate::records::canonical_set(vec![
        json!({"role":"source_decision", "oid":source.decision_head}),
        json!({"role":"source_proposal", "oid":source.proposal_head}),
    ]));
    if record["record_type"] != "activity"
        || record["payload"]["activity_kind"] != "import"
        || record["payload"]["input_refs"] != expected
    {
        return Err(invalid_source());
    }
    Ok(Some(source))
}
