use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Value as JsonValue, json};
use synapse_canonical::{CoreError, ErrorCode, HARD_MAX_NESTING_DEPTH, ResourceLimits, sha256_hex};
use synapse_schema::{
    SchemaRegistry, ingest, ingest_claimed, ingest_with_limits, validate, validate_with_limits,
};

const UNCOVERED_RECORD_FIXTURES: &[(&str, &str, &str)] = &[
    ("subject.json", "subject", "subject.schema.json"),
    ("observation.json", "observation", "observation.schema.json"),
    ("claim.json", "claim", "claim.schema.json"),
    (
        "claim-reaction.json",
        "claim_reaction",
        "claim-reaction.schema.json",
    ),
    (
        "capture-profile.json",
        "capture_profile",
        "capture-profile.schema.json",
    ),
    (
        "analysis-result.json",
        "analysis_result",
        "analysis-result.schema.json",
    ),
    ("assurance.json", "assurance", "assurance.schema.json"),
    (
        "evidence-gap.json",
        "evidence_gap",
        "evidence-gap.schema.json",
    ),
];

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn spec_root() -> PathBuf {
    repository_root().join("spec/core/v0.1")
}

fn schema_fixture_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

fn read(path: &Path) -> Vec<u8> {
    fs::read(path).unwrap_or_else(|error| panic!("read {}: {error}", path.display()))
}

fn read_json(path: &Path) -> JsonValue {
    serde_json::from_slice(&read(path))
        .unwrap_or_else(|error| panic!("parse {} as test JSON: {error}", path.display()))
}

fn schema_fixture(name: &str) -> JsonValue {
    read_json(&schema_fixture_path(name))
}

fn spec_fixture(name: &str) -> JsonValue {
    read_json(&spec_root().join("fixtures").join(name))
}

fn encode(value: &JsonValue) -> Vec<u8> {
    serde_json::to_vec(value).expect("serialize mutated fixture")
}

fn replace(value: &mut JsonValue, pointer: &str, replacement: JsonValue) {
    *value
        .pointer_mut(pointer)
        .unwrap_or_else(|| panic!("fixture must contain {pointer}")) = replacement;
}

fn expect_error<T>(result: Result<T, CoreError>, expected: ErrorCode, case: &str) {
    match result {
        Ok(_) => panic!("{case}: expected {expected}, but validation succeeded"),
        Err(error) => assert_eq!(
            error.code(),
            expected,
            "{case}: unexpected error classification: {error}"
        ),
    }
}

fn expect_json_error(value: &JsonValue, expected: ErrorCode, case: &str) {
    expect_error(ingest(&encode(value)), expected, case);
}

fn expected_schema_name(value: &synapse_canonical::Value) -> &'static str {
    match value
        .get("object_type")
        .and_then(synapse_canonical::Value::as_str)
        .expect("validated object_type")
    {
        "tree" => "manifest-tree.schema.json",
        "commit" => "commit.schema.json",
        "record" => match value
            .get("record_type")
            .and_then(synapse_canonical::Value::as_str)
            .expect("validated record_type")
        {
            "actor" => "actor.schema.json",
            "subject" => "subject.schema.json",
            "activity" => "activity.schema.json",
            "observation" => "observation.schema.json",
            "claim" => "claim.schema.json",
            "claim_reaction" => "claim-reaction.schema.json",
            "capture_profile" => "capture-profile.schema.json",
            "analysis_result" => "analysis-result.schema.json",
            "context_pack" => "context-pack.schema.json",
            "delegation_grant" => "delegation-grant.schema.json",
            "decision_feedback" => "decision-feedback.schema.json",
            "policy" => "policy.schema.json",
            "assurance" => "assurance.schema.json",
            "evidence_gap" => "evidence-gap.schema.json",
            "tombstone" => "tombstone.schema.json",
            other => panic!("unexpected validated record type {other}"),
        },
        other => panic!("unexpected validated object type {other}"),
    }
}

#[test]
fn all_seventeen_structured_golden_fixtures_validate_and_match_their_oids() {
    let golden = read_json(&spec_root().join("fixtures/golden.json"));
    assert_eq!(golden["profile"], "synapsegit/core/v0.1");
    let rows = golden["objects"].as_array().expect("golden objects array");
    assert_eq!(rows.len(), 17, "the committed golden set changed");

    for row in rows {
        let relative_path = row["path"].as_str().expect("golden fixture path");
        let input = read(&spec_root().join(relative_path));
        let validated =
            ingest(&input).unwrap_or_else(|error| panic!("{relative_path} must validate: {error}"));
        let expected_oid = row["oid"].as_str().expect("golden OID");

        assert_eq!(validated.oid(), expected_oid, "OID for {relative_path}");
        assert_eq!(
            validated.canonical_bytes().len() as u64,
            row["canonical_length"]
                .as_u64()
                .expect("golden canonical length"),
            "canonical length for {relative_path}"
        );
        assert_eq!(
            sha256_hex(validated.canonical_bytes()),
            row["canonical_sha256"]
                .as_str()
                .expect("golden canonical SHA-256"),
            "canonical SHA-256 for {relative_path}"
        );
        assert_eq!(
            validated.schema_name(),
            expected_schema_name(validated.value()),
            "schema dispatch for {relative_path}"
        );

        validate(validated.value())
            .unwrap_or_else(|error| panic!("direct validation of {relative_path}: {error}"));
        let claimed = ingest_claimed(expected_oid, &input)
            .unwrap_or_else(|error| panic!("claimed ingestion of {relative_path}: {error}"));
        assert_eq!(claimed, validated, "claimed ingestion for {relative_path}");
    }
}

#[test]
fn all_eight_previously_uncovered_record_fixtures_validate() {
    let registry = SchemaRegistry::new().expect("compile the bundled offline registry");
    let mut failures = Vec::new();

    for &(name, record_type, schema_name) in UNCOVERED_RECORD_FIXTURES {
        let input = read(&schema_fixture_path(name));
        let validated = match registry.ingest(&input) {
            Ok(validated) => validated,
            Err(error) => {
                failures.push(format!("{name}: {error}"));
                continue;
            }
        };

        assert_eq!(
            validated
                .value()
                .get("record_type")
                .and_then(synapse_canonical::Value::as_str),
            Some(record_type),
            "record type for {name}"
        );
        assert_eq!(validated.schema_name(), schema_name, "schema for {name}");
        assert!(
            validated.oid().starts_with("record:sg-oid-v1:sha256:"),
            "record OID family for {name}"
        );
        assert_eq!(validated.oid().len(), 88, "record OID length for {name}");

        registry
            .validate(validated.value())
            .unwrap_or_else(|error| panic!("direct registry validation of {name}: {error}"));
        let canonical_reingested = registry
            .ingest_claimed(validated.oid(), validated.canonical_bytes())
            .unwrap_or_else(|error| panic!("canonical re-ingestion of {name}: {error}"));
        assert_eq!(
            canonical_reingested.canonical_bytes(),
            validated.canonical_bytes(),
            "canonical bytes after round trip for {name}"
        );
        assert_eq!(
            canonical_reingested.oid(),
            validated.oid(),
            "OID after round trip for {name}"
        );
        assert_eq!(
            canonical_reingested.schema_name(),
            validated.schema_name(),
            "schema after round trip for {name}"
        );
    }

    assert!(
        failures.is_empty(),
        "uncovered fixtures failed validation:\n{}",
        failures.join("\n")
    );
}

#[test]
fn schema_dispatch_rejects_missing_wrong_and_unsupported_discriminators() {
    let base = schema_fixture("subject.json");

    let mut missing_object_type = base.clone();
    missing_object_type
        .as_object_mut()
        .expect("fixture object")
        .remove("object_type");
    expect_json_error(
        &missing_object_type,
        ErrorCode::SchemaInvalid,
        "missing object_type",
    );

    let mut non_string_object_type = base.clone();
    replace(&mut non_string_object_type, "/object_type", json!(7));
    expect_json_error(
        &non_string_object_type,
        ErrorCode::SchemaInvalid,
        "non-string object_type",
    );

    let mut unsupported_object_type = base.clone();
    replace(&mut unsupported_object_type, "/object_type", json!("blob"));
    expect_json_error(
        &unsupported_object_type,
        ErrorCode::SchemaInvalid,
        "unsupported object_type",
    );

    let mut missing_record_type = base.clone();
    missing_record_type
        .as_object_mut()
        .expect("fixture object")
        .remove("record_type");
    expect_json_error(
        &missing_record_type,
        ErrorCode::SchemaInvalid,
        "missing record_type",
    );

    let mut non_string_record_type = base.clone();
    replace(&mut non_string_record_type, "/record_type", json!(false));
    expect_json_error(
        &non_string_record_type,
        ErrorCode::SchemaInvalid,
        "non-string record_type",
    );

    let mut unsupported_record_type = base.clone();
    replace(
        &mut unsupported_record_type,
        "/record_type",
        json!("unknown_record"),
    );
    expect_json_error(
        &unsupported_record_type,
        ErrorCode::SchemaInvalid,
        "unsupported record_type",
    );

    let mut concrete_schema_mismatch = base;
    replace(
        &mut concrete_schema_mismatch,
        "/record_type",
        json!("actor"),
    );
    expect_json_error(
        &concrete_schema_mismatch,
        ErrorCode::SchemaInvalid,
        "payload rejected by selected concrete schema",
    );
}

#[test]
fn concrete_schemas_reject_missing_and_extra_fields() {
    let base = schema_fixture("subject.json");

    let mut missing_envelope_field = base.clone();
    missing_envelope_field
        .as_object_mut()
        .expect("fixture object")
        .remove("payload");
    expect_json_error(
        &missing_envelope_field,
        ErrorCode::SchemaInvalid,
        "missing required envelope field",
    );

    let mut extra_envelope_field = base.clone();
    extra_envelope_field
        .as_object_mut()
        .expect("fixture object")
        .insert("oid".to_owned(), json!("must-not-be-embedded"));
    expect_json_error(
        &extra_envelope_field,
        ErrorCode::SchemaInvalid,
        "extra envelope field",
    );

    let mut missing_payload_field = base.clone();
    missing_payload_field["payload"]
        .as_object_mut()
        .expect("payload object")
        .remove("subject_kind");
    expect_json_error(
        &missing_payload_field,
        ErrorCode::SchemaInvalid,
        "missing required payload field",
    );

    let mut extra_payload_field = base;
    extra_payload_field["payload"]
        .as_object_mut()
        .expect("payload object")
        .insert("unexpected".to_owned(), JsonValue::Null);
    expect_json_error(
        &extra_payload_field,
        ErrorCode::SchemaInvalid,
        "extra payload field",
    );
}

#[test]
fn claimed_oid_mismatch_is_rejected_after_successful_validation() {
    let input = read(&schema_fixture_path("subject.json"));
    let validated = ingest(&input).expect("valid subject fixture");
    let mut wrong_oid = validated.oid().to_owned();
    let final_byte = wrong_oid.pop().expect("nonempty OID");
    wrong_oid.push(if final_byte == '0' { '1' } else { '0' });

    expect_error(
        ingest_claimed(&wrong_oid, &input),
        ErrorCode::OidMismatch,
        "wrong claimed digest",
    );
    expect_error(
        ingest_claimed(&validated.oid().replacen("record:", "tree:", 1), &input),
        ErrorCode::OidMismatch,
        "claimed OID family conflicts with body",
    );
}

#[test]
fn nfc_is_required_for_identifiers_and_object_keys_but_not_free_text() {
    let mut policy = spec_fixture("policy.json");
    replace(
        &mut policy,
        "/payload/rules/0/rule_id",
        json!("cafe\u{301}"),
    );
    expect_json_error(
        &policy,
        ErrorCode::IdentifierNotNfc,
        "NFD schema identifier",
    );

    let mut tree = spec_fixture("base-tree-a.json");
    let entry = tree["entries"]["2"].clone();
    tree["entries"]
        .as_object_mut()
        .expect("tree entries")
        .insert("cafe\u{301}".to_owned(), entry);
    expect_json_error(&tree, ErrorCode::KeyNotNfc, "NFD manifest key");

    let nfc = ingest(&read(&spec_root().join("fixtures/actor-creator-a.json")))
        .expect("NFC actor free text");
    let nfd = ingest(&read(&spec_root().join("fixtures/actor-creator-nfd.json")))
        .expect("NFD actor free text remains valid");
    assert_ne!(nfc.oid(), nfd.oid(), "free text must not be normalized");
}

#[test]
fn set_arrays_require_canonical_order_and_uniqueness_while_sequences_do_not() {
    let mut unsorted = schema_fixture("capture-profile.json");
    replace(
        &mut unsorted,
        "/payload/required_conditions",
        json!(["station", "color_reference"]),
    );
    expect_json_error(&unsorted, ErrorCode::SetNotSorted, "unsorted set array");

    let mut duplicate = schema_fixture("capture-profile.json");
    replace(
        &mut duplicate,
        "/payload/required_conditions",
        json!(["color_reference", "color_reference"]),
    );
    expect_json_error(
        &duplicate,
        ErrorCode::SetDuplicate,
        "duplicate set array item",
    );

    let mut sequence = schema_fixture("claim.json");
    sequence["payload"]
        .as_object_mut()
        .expect("claim payload")
        .remove("confidence");
    replace(
        &mut sequence,
        "/payload/assumptions",
        json!(["z", "a", "a"]),
    );
    ingest(&encode(&sequence)).expect("sequence arrays preserve order and may repeat values");
}

#[test]
fn timestamps_require_exact_lexical_form_and_real_gregorian_dates() {
    let invalid = [
        ("2026-07-11T01:00:00Z", "missing nanoseconds"),
        ("2026-07-11T01:00:00.000000000+00:00", "UTC offset"),
        ("2026-02-30T01:00:00.000000000Z", "day outside month"),
        ("1900-02-29T01:00:00.000000000Z", "non-leap century"),
        ("2026-07-11T24:00:00.000000000Z", "hour 24"),
    ];
    for (timestamp, case) in invalid {
        let mut subject = schema_fixture("subject.json");
        replace(&mut subject, "/recorded_at", json!(timestamp));
        expect_json_error(&subject, ErrorCode::TimestampInvalid, case);
    }

    let mut leap_day = schema_fixture("subject.json");
    replace(
        &mut leap_day,
        "/recorded_at",
        json!("2000-02-29T23:59:59.999999999Z"),
    );
    ingest(&encode(&leap_day)).expect("year 2000 is a Gregorian leap year");
}

#[test]
fn fixed_point_values_must_be_normalized_and_within_the_vocabulary() {
    let cases = [
        (
            "/payload/tolerances/viewpoint/mantissa",
            json!("20"),
            "trailing zero",
        ),
        (
            "/payload/tolerances/viewpoint/mantissa",
            json!("00"),
            "leading zero",
        ),
        (
            "/payload/tolerances/viewpoint/mantissa",
            json!("-0"),
            "negative zero",
        ),
        (
            "/payload/tolerances/viewpoint/scale",
            json!(25),
            "scale above range",
        ),
        (
            "/payload/tolerances/viewpoint/unit",
            json!("inch"),
            "unknown unit",
        ),
    ];
    for (pointer, replacement, case) in cases {
        let mut profile = schema_fixture("capture-profile.json");
        replace(&mut profile, pointer, replacement);
        expect_json_error(&profile, ErrorCode::FixedPointNotNormalized, case);
    }

    let mut noncanonical_zero = schema_fixture("capture-profile.json");
    replace(
        &mut noncanonical_zero,
        "/payload/tolerances/viewpoint/mantissa",
        json!("0"),
    );
    replace(
        &mut noncanonical_zero,
        "/payload/tolerances/viewpoint/scale",
        json!(-1),
    );
    expect_json_error(
        &noncanonical_zero,
        ErrorCode::FixedPointNotNormalized,
        "zero must have scale zero",
    );

    for (mantissa, unit, case) in [
        ("-1", "s", "negative temporal precision"),
        ("1", "mm", "non-temporal precision unit"),
    ] {
        let mut observation = schema_fixture("observation.json");
        replace(
            &mut observation,
            "/payload/capture_time/precision/value/mantissa",
            json!(mantissa),
        );
        replace(
            &mut observation,
            "/payload/capture_time/precision/value/unit",
            json!(unit),
        );
        expect_json_error(&observation, ErrorCode::FixedPointNotNormalized, case);
    }
}

#[test]
fn temporal_and_confidence_intervals_and_probabilities_are_semantically_bounded() {
    let mut reversed_time = schema_fixture("evidence-gap.json");
    replace(
        &mut reversed_time,
        "/payload/affected_time/from",
        json!("2026-07-12T00:00:00.000000000Z"),
    );
    expect_json_error(
        &reversed_time,
        ErrorCode::IntervalInvalid,
        "valid-time interval from later than to",
    );

    for (mantissa, scale, unit, case) in [
        ("2", 0, "ratio", "probability above one"),
        ("-1", -1, "ratio", "negative probability"),
        ("5", -1, "percent", "probability with non-ratio unit"),
    ] {
        let mut claim = schema_fixture("claim.json");
        replace(
            &mut claim,
            "/payload/confidence/value/mantissa",
            json!(mantissa),
        );
        replace(&mut claim, "/payload/confidence/value/scale", json!(scale));
        replace(&mut claim, "/payload/confidence/value/unit", json!(unit));
        expect_json_error(&claim, ErrorCode::FixedPointNotNormalized, case);
    }

    let mut reversed_confidence = schema_fixture("analysis-result.json");
    replace(
        &mut reversed_confidence,
        "/payload/confidence/lower/mantissa",
        json!("2"),
    );
    replace(
        &mut reversed_confidence,
        "/payload/confidence/upper/mantissa",
        json!("1"),
    );
    replace(
        &mut reversed_confidence,
        "/payload/confidence/upper/scale",
        json!(0),
    );
    expect_json_error(
        &reversed_confidence,
        ErrorCode::IntervalInvalid,
        "confidence lower exceeds upper",
    );

    let mut mismatched_units = schema_fixture("analysis-result.json");
    replace(
        &mut mismatched_units,
        "/payload/confidence/lower/unit",
        json!("mm"),
    );
    expect_json_error(
        &mismatched_units,
        ErrorCode::FixedPointNotNormalized,
        "confidence interval unit mismatch",
    );
}

#[test]
fn manifest_segments_and_entry_oid_families_are_checked_semantically() {
    for segment in ["", ".", "..", "dir/file", "nul\0segment"] {
        let mut tree = spec_fixture("base-tree-a.json");
        let entry = tree["entries"]["2"].clone();
        tree["entries"]
            .as_object_mut()
            .expect("tree entries")
            .insert(segment.to_owned(), entry);
        expect_json_error(
            &tree,
            ErrorCode::PathSegmentInvalid,
            &format!("invalid manifest segment {segment:?}"),
        );
    }

    let mut mismatch = spec_fixture("base-tree-a.json");
    replace(&mut mismatch, "/entries/2/entry_kind", json!("blob"));
    expect_json_error(
        &mismatch,
        ErrorCode::ReferenceTypeMismatch,
        "manifest entry kind and OID family mismatch",
    );
}

#[test]
fn observation_rejects_envelope_valid_time_in_favor_of_payload_capture_time() {
    let mut observation = schema_fixture("observation.json");
    observation
        .as_object_mut()
        .expect("observation object")
        .insert(
            "valid_time".to_owned(),
            json!({
                "kind": "instant",
                "at": "2026-07-11T01:00:30.000000000Z"
            }),
        );
    expect_json_error(
        &observation,
        ErrorCode::SchemaInvalid,
        "Observation envelope valid_time",
    );
}

#[test]
fn delegation_grant_cannot_expire_before_it_is_recorded() {
    let mut expired = spec_fixture("delegation-grant.json");
    replace(
        &mut expired,
        "/payload/expires_at",
        json!("2026-07-10T23:59:59.999999999Z"),
    );
    expect_json_error(
        &expired,
        ErrorCode::IntervalInvalid,
        "delegation expiration before recorded_at",
    );

    let mut expires_immediately = spec_fixture("delegation-grant.json");
    replace(
        &mut expires_immediately,
        "/payload/expires_at",
        json!("2026-07-11T00:03:00.000000000Z"),
    );
    ingest(&encode(&expires_immediately)).expect("expiration equal to recorded_at is allowed");
}

#[test]
fn all_structured_resource_limits_are_enforced_at_ingestion() {
    let input = read(&schema_fixture_path("subject.json"));
    let baseline = ingest(&input).expect("baseline fixture");
    let exact = ResourceLimits {
        max_input_bytes: input.len(),
        max_canonical_bytes: baseline.canonical_bytes().len(),
        ..ResourceLimits::default()
    };
    ingest_with_limits(&input, exact).expect("resource limits are inclusive");

    let cases = [
        (
            "input bytes",
            ResourceLimits {
                max_input_bytes: input.len() - 1,
                ..ResourceLimits::default()
            },
        ),
        (
            "nesting depth",
            ResourceLimits {
                max_nesting_depth: 1,
                ..ResourceLimits::default()
            },
        ),
        (
            "node count",
            ResourceLimits {
                max_nodes: 1,
                ..ResourceLimits::default()
            },
        ),
        (
            "container items",
            ResourceLimits {
                max_container_items: 1,
                ..ResourceLimits::default()
            },
        ),
        (
            "canonical bytes",
            ResourceLimits {
                max_canonical_bytes: baseline.canonical_bytes().len() - 1,
                ..ResourceLimits::default()
            },
        ),
        (
            "configured depth above the hard ceiling",
            ResourceLimits {
                max_nesting_depth: HARD_MAX_NESTING_DEPTH + 1,
                ..ResourceLimits::default()
            },
        ),
    ];
    for (case, limits) in cases {
        expect_error(
            ingest_with_limits(&input, limits),
            ErrorCode::ResourceLimit,
            case,
        );
    }
}

#[test]
fn direct_value_validation_enforces_traversal_resource_limits() {
    let input = read(&schema_fixture_path("subject.json"));
    let baseline = ingest(&input).expect("baseline fixture");

    for (case, limits) in [
        (
            "direct node count",
            ResourceLimits {
                max_nodes: 1,
                ..ResourceLimits::default()
            },
        ),
        (
            "direct canonical bytes",
            ResourceLimits {
                max_canonical_bytes: baseline.canonical_bytes().len() - 1,
                ..ResourceLimits::default()
            },
        ),
    ] {
        expect_error(
            validate_with_limits(baseline.value(), limits),
            ErrorCode::ResourceLimit,
            case,
        );
    }
}
