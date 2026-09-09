use crate::{CreatorError, CreatorReport, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use synapse_canonical::{ObjectKind, parse_oid};

pub const CREATOR_SOURCE_KEY: &str = "org.synapsegit.creator-source";
pub const CREATOR_SOURCE_FORMAT: &str = "synapsegit-creator-source-v1";
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
