use serde_json::{Value as JsonValue, json};
use std::fs;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use synapse_artifact::{
    ArtifactLimits, ArtifactManifestEntry, RegularFileManifest, artifact_manifest_sha256,
    review_context_sha256,
};
use synapse_schema::{ScaledInteger, Unit};

fn contract_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../spec/application/generic-artifact/v1")
}

fn read_json(path: &Path) -> JsonValue {
    serde_json::from_slice(
        &fs::read(path).unwrap_or_else(|error| panic!("read {}: {error}", path.display())),
    )
    .unwrap_or_else(|error| panic!("parse {}: {error}", path.display()))
}

fn assert_valid(schema: &JsonValue, instance: &JsonValue, label: &str) {
    let validator = jsonschema::draft202012::new(schema)
        .unwrap_or_else(|error| panic!("compile schema for {label}: {error}"));
    if !validator.is_valid(instance) {
        let errors = validator
            .iter_errors(instance)
            .map(|error| error.to_string())
            .collect::<Vec<_>>()
            .join("; ");
        panic!("{label} did not match its frozen schema: {errors}");
    }
}

fn assert_invalid(schema: &JsonValue, instance: &JsonValue, label: &str) {
    let validator = jsonschema::draft202012::new(schema)
        .unwrap_or_else(|error| panic!("compile schema for {label}: {error}"));
    assert!(
        !validator.is_valid(instance),
        "{label} unexpectedly matched"
    );
}

fn decode_hex(value: &str) -> Vec<u8> {
    assert!(value.len().is_multiple_of(2));
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let text = std::str::from_utf8(pair).unwrap();
            u8::from_str_radix(text, 16).unwrap()
        })
        .collect()
}

#[test]
fn digest_implementation_matches_the_frozen_cross_language_vectors() {
    let vector = read_json(&contract_root().join("digest-vectors.json"));
    let files = vector["manifest"]["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|file| {
            ArtifactManifestEntry::regular_file(
                file["path"].as_str().unwrap(),
                decode_hex(file["content_hex"].as_str().unwrap()),
            )
        })
        .collect::<Vec<_>>();
    let manifest = RegularFileManifest::from_entries(files, ArtifactLimits::default()).unwrap();

    assert_eq!(
        artifact_manifest_sha256(&manifest),
        vector["artifact_manifest_sha256"].as_str().unwrap()
    );
    assert_eq!(
        review_context_sha256(vector["review_context_input"].as_str().unwrap().as_bytes()).unwrap(),
        vector["review_context_sha256"].as_str().unwrap()
    );
}

#[test]
fn exact_decimal_construction_matches_the_additional_scaled_integer_vectors() {
    let vector = read_json(&contract_root().join("scaled-integer-vectors-v1.json"));
    assert_eq!(vector["status"], "frozen");

    for case in vector["equivalent"].as_array().unwrap() {
        let unit = Unit::from_str(case["unit"].as_str().unwrap()).unwrap();
        let mut observed_digest = None;
        for decimal in case["decimals"].as_array().unwrap() {
            let scaled = ScaledInteger::from_decimal_str(decimal.as_str().unwrap(), unit).unwrap();
            assert_eq!(serde_json::to_value(&scaled).unwrap(), case["scaled"]);
            let context = serde_json::to_vec(&json!({"measurement": scaled})).unwrap();
            let review_context_input = case["review_context_input"].as_str().unwrap();
            assert_eq!(context, review_context_input.as_bytes());
            let digest = review_context_sha256(review_context_input.as_bytes()).unwrap();
            assert_eq!(digest, case["review_context_sha256"].as_str().unwrap());
            if let Some(previous) = &observed_digest {
                assert_eq!(previous, &digest);
            }
            observed_digest = Some(digest);
        }
    }

    for case in vector["valid"].as_array().unwrap() {
        let unit = Unit::from_str(case["unit"].as_str().unwrap()).unwrap();
        let scaled =
            ScaledInteger::from_decimal_str(case["decimal"].as_str().unwrap(), unit).unwrap();
        assert_eq!(serde_json::to_value(scaled).unwrap(), case["scaled"]);
    }

    for case in vector["invalid"].as_array().unwrap() {
        let error = match Unit::from_str(case["unit"].as_str().unwrap()) {
            Ok(unit) => ScaledInteger::from_decimal_str(case["decimal"].as_str().unwrap(), unit)
                .unwrap_err(),
            Err(error) => error,
        };
        assert_eq!(error.kind().as_str(), case["reason"].as_str().unwrap());
    }

    let context = &vector["review_context"];
    assert_eq!(
        review_context_sha256(context["input"].as_str().unwrap().as_bytes()).unwrap(),
        context["sha256"].as_str().unwrap()
    );
}

#[test]
fn every_frozen_positive_fixture_matches_its_public_schema() {
    let root = contract_root();
    let capabilities_schema = read_json(&root.join("capabilities.schema.json"));
    let proposal_schema = read_json(&root.join("proposal-receipt.schema.json"));
    let review_schema = read_json(&root.join("review-status.schema.json"));
    let error_schema = read_json(&root.join("public-error.schema.json"));

    assert_valid(
        &capabilities_schema,
        &read_json(&root.join("capabilities.json")),
        "capabilities.json",
    );

    let mut fixture_paths = fs::read_dir(root.join("fixtures"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect::<Vec<_>>();
    fixture_paths.sort();
    assert_eq!(fixture_paths.len(), 7, "update the frozen fixture audit");
    for path in fixture_paths {
        let name = path.file_name().unwrap().to_str().unwrap();
        let schema = if name.starts_with("proposal-receipt-") {
            &proposal_schema
        } else if name.starts_with("review-") {
            &review_schema
        } else if name == "public-error.json" {
            &error_schema
        } else {
            panic!("unrouted contract fixture {name}");
        };
        assert_valid(schema, &read_json(&path), name);
    }
}

#[test]
fn attribution_review_state_and_version_correlations_fail_closed() {
    let root = contract_root();
    let proposal_schema = read_json(&root.join("proposal-receipt.schema.json"));
    let review_schema = read_json(&root.join("review-status.schema.json"));
    let error_schema = read_json(&root.join("public-error.schema.json"));

    let mut caller = read_json(&root.join("fixtures/proposal-receipt-caller-supplied.json"));
    caller["execution_verified"] = json!(true);
    assert_invalid(
        &proposal_schema,
        &caller,
        "caller-supplied execution verification",
    );

    let mut unsupported_attribution =
        read_json(&root.join("fixtures/proposal-receipt-caller-supplied.json"));
    unsupported_attribution["source_attribution"] = json!("trusted_executor");
    unsupported_attribution["execution_verified"] = json!(true);
    assert_invalid(
        &proposal_schema,
        &unsupported_attribution,
        "unnegotiated trusted execution attribution",
    );

    let mut invalid_locator = read_json(&root.join("fixtures/review-pending.json"));
    invalid_locator["review_id"] = json!("ABCDEF");
    assert_invalid(&review_schema, &invalid_locator, "non-v1 review locator");

    let mut rejected = read_json(&root.join("fixtures/review-committed.json"));
    rejected["disposition"] = json!("rejected");
    assert_invalid(
        &review_schema,
        &rejected,
        "rejected Decision selecting proposal",
    );

    let mut retryable = read_json(&root.join("fixtures/review-retryable-failure.json"));
    retryable["failure"]["retryable"] = json!(false);
    assert_invalid(
        &review_schema,
        &retryable,
        "retryable state with terminal retry flag",
    );

    let mut wrong_version = read_json(&root.join("fixtures/public-error.json"));
    wrong_version["contract_version"] = json!(2);
    assert_invalid(&error_schema, &wrong_version, "unknown contract version");

    let mut unknown_property = read_json(&root.join("fixtures/review-pending.json"));
    unknown_property["proposal_head"] = json!("must-not-cross-public-boundary");
    assert_invalid(
        &review_schema,
        &unknown_property,
        "internal authority property",
    );
}
