// Each integration-test binary compiles this module independently, so a
// helper unused by one binary's tests is still live code to the compiler in
// another; silence the resulting dead_code lint rather than importing only
// a subset per file.
#![allow(dead_code)]

use serde_json::Value as JsonValue;
use synapse_artifact::{ArtifactLimits, ArtifactManifestEntry, RegularFileManifest};
use synapse_core::Repository;

/// Two-entry manifest (`index.html` + `assets/site.css`) whose content is
/// derived from `label`. Shared verbatim by `acceptance_contract.rs` and
/// `workflow.rs`; other files build differently-shaped manifests (different
/// entry counts, fixed bytes, or raw-byte parameters) and keep their own
/// `manifest()` for that reason.
pub fn manifest(label: &str) -> RegularFileManifest {
    RegularFileManifest::from_entries(
        [
            ArtifactManifestEntry::regular_file(
                "index.html",
                format!("<!doctype html><title>{label}</title>").into_bytes(),
            ),
            ArtifactManifestEntry::regular_file(
                "assets/site.css",
                format!("/* {label} */ body {{ color: #123456; }}").into_bytes(),
            ),
        ],
        ArtifactLimits::default(),
    )
    .unwrap()
}

/// Reads a stored object and parses it as JSON, panicking with the raw
/// storage/parse error on failure. Shared verbatim by
/// `acceptance_contract.rs` and `durable_workflow.rs`; `approval.rs` and
/// `checkout.rs` keep their own copy (differs only in panic-message text,
/// not behavior, so consolidating would change diagnostic wording without
/// being a true no-op).
pub fn object_json(repository: &Repository, oid: &str) -> JsonValue {
    let bytes = repository
        .objects()
        .read_raw(oid)
        .unwrap_or_else(|error| panic!("read {oid}: {error}"))
        .unwrap_or_else(|| panic!("missing object {oid}"));
    serde_json::from_slice(&bytes).unwrap_or_else(|error| panic!("parse {oid}: {error}"))
}
