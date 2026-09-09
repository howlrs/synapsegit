use crate::{CreatorError, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const ANNOTATIONS_KEY: &str = "org.synapsegit.creator-decision-pins";
pub const ANNOTATIONS_FORMAT: &str = "synapsegit-creator-decision-pins-v1";
pub const PIN_COORDINATE_MAX: u32 = 1_000_000;

/// A role in one verified Creator proposal, never a caller-selected storage path.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CreatorImageRole {
    Original,
    Current,
    AiOutput,
}

impl CreatorImageRole {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Original => "original",
            Self::Current => "current",
            Self::AiOutput => "ai_output",
        }
    }
}

/// Private Human note positioned in the oriented decoded image's unit square.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreatorPin {
    pub role: CreatorImageRole,
    pub blob_oid: String,
    pub x: u32,
    pub y: u32,
    pub note: String,
}

/// Sequence order is the displayed pin number; integer positions are authoritative.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreatorAnnotations {
    pub format: String,
    pub pins: Vec<CreatorPin>,
}

impl CreatorAnnotations {
    pub fn validate(&self, original: &str, current: &str, ai_output: &str) -> Result<()> {
        if self.format != ANNOTATIONS_FORMAT || self.pins.len() > 10 {
            return Err(CreatorError::InvalidArgument(
                "invalid decision pin version or count".into(),
            ));
        }
        for pin in &self.pins {
            let expected = match pin.role {
                CreatorImageRole::Original => original,
                CreatorImageRole::Current => current,
                CreatorImageRole::AiOutput => ai_output,
            };
            if pin.blob_oid != expected
                || pin.x > PIN_COORDINATE_MAX
                || pin.y > PIN_COORDINATE_MAX
                || pin.note.is_empty()
                || pin.note.len() > 200
            {
                return Err(CreatorError::InvalidArgument(
                    "decision pin has invalid image binding, coordinates, or note byte length"
                        .into(),
                ));
            }
        }
        Ok(())
    }
}

pub(crate) fn attach_annotations(
    record: &mut Value,
    annotations: &CreatorAnnotations,
) -> Result<()> {
    let value = serde_json::to_value(annotations)
        .map_err(|_| CreatorError::InvalidArgument("invalid decision pins".into()))?;
    record
        .get_mut("extensions")
        .and_then(Value::as_object_mut)
        .ok_or_else(|| CreatorError::InvalidArgument("invalid feedback extensions".into()))?
        .insert(ANNOTATIONS_KEY.into(), value);
    Ok(())
}

/// Invalid extension text never prevents validating the enclosing Human decision.
/// The separate unavailable flag prevents treating malformed data as valid pins.
pub(crate) fn read_annotations(
    record: &Value,
    original: &str,
    current: &str,
    ai_output: &str,
) -> (Option<CreatorAnnotations>, bool) {
    let Some(value) = record
        .get("extensions")
        .and_then(|e| e.get(ANNOTATIONS_KEY))
    else {
        return (None, false);
    };
    match serde_json::from_value::<CreatorAnnotations>(value.clone()) {
        Ok(annotations) if annotations.validate(original, current, ai_output).is_ok() => {
            (Some(annotations), false)
        }
        _ => (None, true),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fixture_and_invalid_storage_are_decoded_separately_from_core() {
        let fixture: Value = serde_json::from_str(include_str!(
            "../../../spec/application/creator-decision-pins/v1/valid.json"
        ))
        .unwrap();
        let oid = fixture["pins"][0]["blob_oid"].as_str().unwrap();
        let record = serde_json::json!({"extensions":{ ANNOTATIONS_KEY:fixture.clone() }});
        let (pins, unavailable) = read_annotations(&record, "other", "other", oid);
        assert!(!unavailable);
        assert_eq!(pins.unwrap().pins[0].x, 123456);
        for (field, value) in [
            ("x", serde_json::json!(-1)),
            ("y", serde_json::json!(0.5)),
            ("role", serde_json::json!("window")),
            ("unknown", serde_json::json!(true)),
            ("note", serde_json::json!("あ".repeat(67))),
        ] {
            let mut invalid = record.clone();
            invalid["extensions"][ANNOTATIONS_KEY]["pins"][0][field] = value;
            assert_eq!(
                read_annotations(&invalid, "other", "other", oid),
                (None, true)
            );
        }
        assert_eq!(
            read_annotations(&record, "other", "other", "mismatched"),
            (None, true)
        );
        assert_eq!(
            read_annotations(&serde_json::json!({"extensions":{}}), "a", "b", "c"),
            (None, false)
        );
    }
}
