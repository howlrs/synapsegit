// Each integration-test binary compiles this module independently, so a
// helper unused by one binary's tests is still live code to the compiler in
// another; silence the resulting dead_code lint rather than importing only
// a subset per file.
#![allow(dead_code)]

use serde_json::Value as JsonValue;
use synapse_core::Repository;

/// Serializes `value` and stores it as a raw object, returning its OID.
/// Shared verbatim by `authorization.rs` and `human_decision.rs`.
pub fn put_json(repository: &Repository, value: JsonValue) -> String {
    repository
        .put_object(&serde_json::to_vec(&value).unwrap())
        .unwrap()
        .oid
}
