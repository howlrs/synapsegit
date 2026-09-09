use crate::{CreatorError, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const GENERATION_NOTE_KEY: &str = "org.synapsegit.creator-generation-note";

/// Optional private, user-declared text. This is not execution evidence.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreatorGenerationNote {
    #[serde(default)]
    pub tool: String,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub prompt: String,
    #[serde(default)]
    pub intent: String,
}

impl CreatorGenerationNote {
    pub fn validate(&self) -> Result<()> {
        for (value, limit) in [
            (&self.tool, 300),
            (&self.model, 300),
            (&self.prompt, 8192),
            (&self.intent, 2048),
        ] {
            if value.len() > limit {
                return Err(CreatorError::InvalidArgument(
                    "generation note exceeds a UTF-8 byte limit".into(),
                ));
            }
        }
        if serde_json::to_vec(self)
            .map_err(|_| CreatorError::InvalidArgument("invalid generation note".into()))?
            .len()
            > 16384
        {
            return Err(CreatorError::InvalidArgument(
                "generation note exceeds 16 KiB".into(),
            ));
        }
        Ok(())
    }
    pub fn is_empty(&self) -> bool {
        [&self.tool, &self.model, &self.prompt, &self.intent]
            .iter()
            .all(|s| s.is_empty())
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredGenerationNote {
    format: String,
    attribution: String,
    activity_id: String,
    ai_output_blob_oid: String,
    note: CreatorGenerationNote,
}

pub(crate) fn attach_generation_note(
    record: &mut Value,
    oid: &str,
    note: &CreatorGenerationNote,
) -> Result<()> {
    note.validate()?;
    if !note.is_empty() {
        let value = serde_json::to_value(StoredGenerationNote {
            format: "synapsegit-creator-generation-note-v1".into(),
            attribution: "user_declared".into(),
            activity_id: record["entity_id"].as_str().unwrap_or_default().into(),
            ai_output_blob_oid: oid.into(),
            note: note.clone(),
        })
        .map_err(|_| CreatorError::InvalidArgument("invalid generation note".into()))?;
        record
            .get_mut("extensions")
            .and_then(Value::as_object_mut)
            .ok_or_else(|| CreatorError::InvalidArgument("invalid Activity extensions".into()))?
            .insert(GENERATION_NOTE_KEY.into(), value);
    }
    Ok(())
}

pub(crate) fn read_generation_note(
    record: &Value,
    oid: &str,
) -> Result<Option<CreatorGenerationNote>> {
    let Some(value) = record
        .get("extensions")
        .and_then(|e| e.get(GENERATION_NOTE_KEY))
    else {
        return Ok(None);
    };
    let invalid = || {
        CreatorError::ReportInvalid(
            "generation note contract or proposal binding is invalid".into(),
        )
    };
    let stored: StoredGenerationNote =
        serde_json::from_value(value.clone()).map_err(|_| invalid())?;
    if stored.format != "synapsegit-creator-generation-note-v1"
        || stored.attribution != "user_declared"
        || Some(stored.activity_id.as_str()) != record["entity_id"].as_str()
        || stored.ai_output_blob_oid != oid
    {
        return Err(invalid());
    }
    stored.note.validate().map_err(|_| invalid())?;
    Ok(Some(stored.note))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_unknown_format_fields_and_cross_proposal_binding() {
        let mut record = serde_json::json!({"entity_id":"activity-a", "extensions":{}});
        assert_eq!(read_generation_note(&record, "blob-a").unwrap(), None);
        let note = CreatorGenerationNote {
            prompt: "日本語\nprivate".into(),
            ..Default::default()
        };
        attach_generation_note(&mut record, "blob-a", &note).unwrap();
        assert_eq!(read_generation_note(&record, "blob-a").unwrap(), Some(note));
        assert!(read_generation_note(&record, "blob-b").is_err());
        for (key, value) in [
            ("format", serde_json::json!("unknown-v2")),
            ("activity_id", serde_json::json!("activity-b")),
            ("extra", serde_json::json!(true)),
        ] {
            let mut malformed = record.clone();
            malformed["extensions"][GENERATION_NOTE_KEY][key] = value;
            assert!(read_generation_note(&malformed, "blob-a").is_err());
        }
    }
}

#[cfg(test)]
#[test]
fn generation_note_fixture_matches_the_contract() {
    let value: Value = serde_json::from_str(include_str!(
        "../../../spec/application/creator-generation-note/v1/valid.json"
    ))
    .unwrap();
    let record = serde_json::json!({"entity_id":value["activity_id"], "extensions": { GENERATION_NOTE_KEY: value.clone() }});
    let note = read_generation_note(&record, value["ai_output_blob_oid"].as_str().unwrap())
        .unwrap()
        .unwrap();
    assert_eq!(note.prompt, "青い空\n構図の候補");
}
