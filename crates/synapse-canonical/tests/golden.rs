use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use synapse_canonical::{
    ErrorCode, Value, blob_oid, canonical_bytes, parse_strict, sha256_hex,
    structured_oid_unchecked, verify_claimed_oid_unchecked,
};

#[derive(Debug)]
struct Calculated {
    value: Value,
    canonical: Vec<u8>,
    oid: String,
}

fn fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../spec/core/v0.1/fixtures")
}

fn field<'a>(value: &'a Value, name: &str) -> &'a Value {
    value
        .get(name)
        .unwrap_or_else(|| panic!("fixture field {name} is required"))
}

fn string_field<'a>(value: &'a Value, name: &str) -> &'a str {
    field(value, name)
        .as_str()
        .unwrap_or_else(|| panic!("fixture field {name} must be a string"))
}

fn calculate(path: &Path) -> Calculated {
    let bytes = fs::read(path).unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
    let value =
        parse_strict(&bytes).unwrap_or_else(|error| panic!("parse {}: {error}", path.display()));
    let canonical = canonical_bytes(&value).unwrap();
    // Golden fixtures were validated independently by the schema verifier.
    let oid = structured_oid_unchecked(&value).unwrap();
    Calculated {
        value,
        canonical,
        oid,
    }
}

fn expected_paths(group: &Value) -> impl Iterator<Item = &str> {
    group
        .as_array()
        .expect("group must be an array")
        .iter()
        .map(|value| value.as_str().expect("path must be a string"))
}

#[test]
fn all_structured_and_blob_fixtures_match_the_js_golden_file() {
    let directory = fixture_dir();
    let golden_bytes = fs::read(directory.join("golden.json")).unwrap();
    let golden = parse_strict(&golden_bytes).unwrap();
    assert_eq!(string_field(&golden, "profile"), "synapsegit/core/v0.1");

    let mut calculated = HashMap::new();
    for row in field(&golden, "objects").as_array().unwrap() {
        let relative_path = string_field(row, "path");
        let fixture_name = relative_path.strip_prefix("fixtures/").unwrap();
        let result = calculate(&directory.join(fixture_name));

        assert_eq!(
            result.canonical.len() as i64,
            field(row, "canonical_length").as_i64().unwrap(),
            "canonical length for {relative_path}"
        );
        assert_eq!(
            sha256_hex(&result.canonical),
            string_field(row, "canonical_sha256"),
            "canonical SHA-256 for {relative_path}"
        );
        assert_eq!(
            result.oid,
            string_field(row, "oid"),
            "structured OID for {relative_path}"
        );
        calculated.insert(relative_path.to_owned(), result);
    }
    assert_eq!(calculated.len(), 17);
    let fixture_json_count = fs::read_dir(&directory)
        .unwrap()
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .path()
                .extension()
                .is_some_and(|value| value == "json")
        })
        .count();
    assert_eq!(
        fixture_json_count,
        calculated.len() + 1,
        "every structured fixture must have a golden row (plus golden.json itself)"
    );

    let blob_row = &field(&golden, "blobs").as_array().unwrap()[0];
    let blob_path = string_field(blob_row, "path");
    let blob = fs::read(directory.join(blob_path.strip_prefix("fixtures/").unwrap())).unwrap();
    assert_eq!(
        blob.len() as i64,
        field(blob_row, "byte_length").as_i64().unwrap()
    );
    assert_eq!(sha256_hex(&blob), string_field(blob_row, "sha256"));
    assert_eq!(blob_oid(&blob), string_field(blob_row, "oid"));

    for group in field(&golden, "equivalent_groups").as_array().unwrap() {
        let mut paths = expected_paths(group);
        let first = calculated.get(paths.next().unwrap()).unwrap();
        for path in paths {
            let other = calculated.get(path).unwrap();
            assert_eq!(
                first.canonical, other.canonical,
                "canonical bytes for {path}"
            );
            assert_eq!(first.oid, other.oid, "OID for {path}");
        }
    }

    for pair in field(&golden, "distinct_pairs").as_array().unwrap() {
        let paths: Vec<_> = expected_paths(pair).collect();
        assert_ne!(
            calculated.get(paths[0]).unwrap().oid,
            calculated.get(paths[1]).unwrap().oid,
            "{} and {} must remain distinct",
            paths[0],
            paths[1]
        );
    }

    let actor = calculated.get("fixtures/actor-creator-a.json").unwrap();
    let tree = calculated.get("fixtures/base-tree-a.json").unwrap();
    assert_eq!(
        verify_claimed_oid_unchecked(&actor.oid, &tree.value)
            .expect_err("a claimed OID from another body must fail")
            .code(),
        ErrorCode::OidMismatch
    );
    assert_ne!(blob_oid(&actor.canonical), actor.oid);
}
