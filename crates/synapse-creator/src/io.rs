use crate::session::CREATOR_MAX_INPUT_FILE_BYTES;
use crate::{CreatorError, Result};
use serde_json::Value as JsonValue;
use std::fs::File;
use std::io::{self, Read};
use std::path::Path;
use synapse_core::{Repository, RepositoryError};

pub(crate) fn put_file(repository: &Repository, path: &Path) -> Result<String> {
    let file = File::open(path)
        .map_err(|source| CreatorError::io("open creator input Blob", path, source))?;
    Ok(repository
        .put_blob(CreatorFileReader {
            file,
            remaining: CREATOR_MAX_INPUT_FILE_BYTES,
        })?
        .oid)
}

pub(crate) struct CreatorFileReader {
    file: File,
    remaining: u64,
}

impl Read for CreatorFileReader {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if buffer.is_empty() {
            return Ok(0);
        }
        if self.remaining == 0 {
            let mut overflow = [0_u8; 1];
            return match self.file.read(&mut overflow)? {
                0 => Ok(0),
                _ => Err(io::Error::other(
                    "creator input changed beyond its 64 MiB limit while being read",
                )),
            };
        }
        let allowed = usize::try_from(self.remaining.min(buffer.len() as u64))
            .expect("bounded creator read length fits usize");
        let count = self.file.read(&mut buffer[..allowed])?;
        self.remaining = self
            .remaining
            .checked_sub(count as u64)
            .expect("reader never returns more than the supplied buffer");
        Ok(count)
    }
}

pub(crate) fn put_json(repository: &Repository, value: JsonValue) -> Result<String> {
    Ok(repository.put_object(&serde_json::to_vec(&value)?)?.oid)
}

pub(crate) fn read_json(repository: &Repository, oid: &str) -> Result<JsonValue> {
    let bytes = repository
        .objects()
        .read_raw(oid)
        .map_err(RepositoryError::from)?
        .ok_or_else(|| CreatorError::ReportInvalid(format!("stored object is missing: {oid}")))?;
    Ok(serde_json::from_slice(&bytes)?)
}

pub(crate) fn object_field<'a>(
    value: &'a JsonValue,
    key: &str,
    label: &str,
) -> Result<&'a JsonValue> {
    value
        .get(key)
        .filter(|value| value.is_object())
        .ok_or_else(|| CreatorError::ReportInvalid(format!("{label} is missing or invalid")))
}

pub(crate) fn string_field<'a>(value: &'a JsonValue, key: &str, label: &str) -> Result<&'a str> {
    value
        .get(key)
        .and_then(JsonValue::as_str)
        .ok_or_else(|| CreatorError::ReportInvalid(format!("{label} is missing or invalid")))
}
