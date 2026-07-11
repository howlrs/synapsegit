//! Production schema and semantic validation for SynapseGit Core v0.1.
//!
//! This crate is the structured-object ingestion boundary. It combines the
//! strict parser from `synapse-canonical`, concrete Draft 2020-12 schemas,
//! Synapse annotations and local semantic rules before exposing canonical
//! bytes or a structured OID as validated data.

#![forbid(unsafe_code)]

use jsonschema::{Retrieve, Uri, Validator as JsonSchemaValidator};
use serde_json::Value as JsonValue;
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::sync::{Arc, OnceLock};
use synapse_canonical::{
    CoreError, ErrorCode, ResourceLimits, Value, canonical_bytes_with_limits,
    parse_strict_with_limits, structured_oid_unchecked_with_limits,
};
use unicode_normalization::UnicodeNormalization;

const SCHEMA_BASE: &str = "https://schemas.synapsegit.dev/core/v0.1/";

const RECORD_SCHEMAS: &[(&str, &str)] = &[
    ("actor", "actor.schema.json"),
    ("subject", "subject.schema.json"),
    ("activity", "activity.schema.json"),
    ("observation", "observation.schema.json"),
    ("claim", "claim.schema.json"),
    ("claim_reaction", "claim-reaction.schema.json"),
    ("capture_profile", "capture-profile.schema.json"),
    ("analysis_result", "analysis-result.schema.json"),
    ("context_pack", "context-pack.schema.json"),
    ("delegation_grant", "delegation-grant.schema.json"),
    ("decision_feedback", "decision-feedback.schema.json"),
    ("policy", "policy.schema.json"),
    ("assurance", "assurance.schema.json"),
    ("evidence_gap", "evidence-gap.schema.json"),
    ("tombstone", "tombstone.schema.json"),
];

const EMBEDDED_SCHEMAS: &[(&str, &str)] = &[
    (
        "common.schema.json",
        include_str!("../../../spec/core/v0.1/schemas/common.schema.json"),
    ),
    (
        "record-envelope.schema.json",
        include_str!("../../../spec/core/v0.1/schemas/record-envelope.schema.json"),
    ),
    (
        "record.schema.json",
        include_str!("../../../spec/core/v0.1/schemas/record.schema.json"),
    ),
    (
        "actor.schema.json",
        include_str!("../../../spec/core/v0.1/schemas/actor.schema.json"),
    ),
    (
        "subject.schema.json",
        include_str!("../../../spec/core/v0.1/schemas/subject.schema.json"),
    ),
    (
        "activity.schema.json",
        include_str!("../../../spec/core/v0.1/schemas/activity.schema.json"),
    ),
    (
        "observation.schema.json",
        include_str!("../../../spec/core/v0.1/schemas/observation.schema.json"),
    ),
    (
        "claim.schema.json",
        include_str!("../../../spec/core/v0.1/schemas/claim.schema.json"),
    ),
    (
        "claim-reaction.schema.json",
        include_str!("../../../spec/core/v0.1/schemas/claim-reaction.schema.json"),
    ),
    (
        "capture-profile.schema.json",
        include_str!("../../../spec/core/v0.1/schemas/capture-profile.schema.json"),
    ),
    (
        "analysis-result.schema.json",
        include_str!("../../../spec/core/v0.1/schemas/analysis-result.schema.json"),
    ),
    (
        "context-pack.schema.json",
        include_str!("../../../spec/core/v0.1/schemas/context-pack.schema.json"),
    ),
    (
        "delegation-grant.schema.json",
        include_str!("../../../spec/core/v0.1/schemas/delegation-grant.schema.json"),
    ),
    (
        "decision-feedback.schema.json",
        include_str!("../../../spec/core/v0.1/schemas/decision-feedback.schema.json"),
    ),
    (
        "policy.schema.json",
        include_str!("../../../spec/core/v0.1/schemas/policy.schema.json"),
    ),
    (
        "assurance.schema.json",
        include_str!("../../../spec/core/v0.1/schemas/assurance.schema.json"),
    ),
    (
        "evidence-gap.schema.json",
        include_str!("../../../spec/core/v0.1/schemas/evidence-gap.schema.json"),
    ),
    (
        "tombstone.schema.json",
        include_str!("../../../spec/core/v0.1/schemas/tombstone.schema.json"),
    ),
    (
        "manifest-tree.schema.json",
        include_str!("../../../spec/core/v0.1/schemas/manifest-tree.schema.json"),
    ),
    (
        "commit.schema.json",
        include_str!("../../../spec/core/v0.1/schemas/commit.schema.json"),
    ),
];

/// A structured object that passed strict parsing, concrete schema validation,
/// local semantic validation and structured OID calculation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ValidatedObject {
    value: Value,
    canonical_bytes: Vec<u8>,
    oid: String,
    schema_name: &'static str,
}

impl ValidatedObject {
    pub fn value(&self) -> &Value {
        &self.value
    }
    pub fn canonical_bytes(&self) -> &[u8] {
        &self.canonical_bytes
    }
    pub fn oid(&self) -> &str {
        &self.oid
    }
    pub const fn schema_name(&self) -> &'static str {
        self.schema_name
    }

    pub fn into_parts(self) -> (Value, Vec<u8>, String) {
        (self.value, self.canonical_bytes, self.oid)
    }
}

/// Reusable offline registry for the bundled Core v0.1 schemas.
pub struct SchemaRegistry {
    documents: HashMap<&'static str, JsonValue>,
    validators: HashMap<&'static str, JsonSchemaValidator>,
}

static DEFAULT_REGISTRY: OnceLock<Result<SchemaRegistry, CoreError>> = OnceLock::new();

/// Strictly parse and fully validate an unclaimed structured object.
pub fn ingest(input: &[u8]) -> Result<ValidatedObject, CoreError> {
    ingest_with_limits(input, ResourceLimits::default())
}

/// Strictly parse and fully validate an unclaimed object with resource limits.
pub fn ingest_with_limits(
    input: &[u8],
    limits: ResourceLimits,
) -> Result<ValidatedObject, CoreError> {
    default_registry()?.ingest_with_limits(input, limits)
}

/// Strictly parse, fully validate and verify a transport-supplied OID.
pub fn ingest_claimed(claimed_oid: &str, input: &[u8]) -> Result<ValidatedObject, CoreError> {
    ingest_claimed_with_limits(claimed_oid, input, ResourceLimits::default())
}

/// Claimed-OID ingestion with explicit structured resource limits.
pub fn ingest_claimed_with_limits(
    claimed_oid: &str,
    input: &[u8],
    limits: ResourceLimits,
) -> Result<ValidatedObject, CoreError> {
    default_registry()?.ingest_claimed_with_limits(claimed_oid, input, limits)
}

/// Validate an already parsed restricted-domain value.
pub fn validate(value: &Value) -> Result<(), CoreError> {
    validate_with_limits(value, ResourceLimits::default())
}

/// Validate an already parsed value with explicit traversal/output limits.
pub fn validate_with_limits(value: &Value, limits: ResourceLimits) -> Result<(), CoreError> {
    default_registry()?.validate_with_limits(value, limits)
}

fn default_registry() -> Result<&'static SchemaRegistry, CoreError> {
    match DEFAULT_REGISTRY.get_or_init(SchemaRegistry::new) {
        Ok(registry) => Ok(registry),
        Err(error) => Err(error.clone()),
    }
}

#[derive(Clone)]
struct EmbeddedRetriever {
    documents: Arc<HashMap<String, JsonValue>>,
}

impl Retrieve for EmbeddedRetriever {
    fn retrieve(&self, uri: &Uri<String>) -> Result<JsonValue, Box<dyn Error + Send + Sync>> {
        self.documents
            .get(uri.as_str())
            .cloned()
            .ok_or_else(|| format!("unregistered schema URI: {uri}").into())
    }
}

#[derive(Clone, Copy)]
struct SchemaEntry<'a> {
    schema: &'a JsonValue,
    file: &'static str,
}

impl SchemaRegistry {
    /// Parse, audit and compile the bundled schema registry without external
    /// network or file resolution.
    pub fn new() -> Result<Self, CoreError> {
        let mut documents = HashMap::with_capacity(EMBEDDED_SCHEMAS.len());
        for &(name, source) in EMBEDDED_SCHEMAS {
            let mut document: JsonValue = serde_json::from_str(source).map_err(|error| {
                CoreError::new(
                    ErrorCode::SchemaInvalid,
                    format!("bundled schema {name} is not valid JSON: {error}"),
                )
            })?;
            let object = document.as_object_mut().ok_or_else(|| {
                CoreError::new(
                    ErrorCode::SchemaInvalid,
                    format!("bundled schema {name} is not an object"),
                )
            })?;
            object.insert(
                "$id".to_owned(),
                JsonValue::String(format!("{SCHEMA_BASE}{name}")),
            );
            audit_schema_node(name, &document, "")?;
            documents.insert(name, document);
        }

        audit_record_dispatch(&documents)?;

        let retrieval_documents = Arc::new(
            documents
                .iter()
                .map(|(&name, schema)| (format!("{SCHEMA_BASE}{name}"), schema.clone()))
                .collect(),
        );
        let retriever = EmbeddedRetriever {
            documents: retrieval_documents,
        };

        let mut validators = HashMap::with_capacity(RECORD_SCHEMAS.len() + 2);
        for &schema_name in concrete_schema_names() {
            let schema = documents.get(schema_name).ok_or_else(|| {
                CoreError::new(
                    ErrorCode::SchemaInvalid,
                    format!("missing bundled concrete schema {schema_name}"),
                )
            })?;
            let validator = jsonschema::draft202012::options()
                .with_retriever(retriever.clone())
                .should_validate_formats(false)
                .build(schema)
                .map_err(|error| {
                    CoreError::new(
                        ErrorCode::SchemaInvalid,
                        format!("failed to compile {schema_name}: {error}"),
                    )
                })?;
            validators.insert(schema_name, validator);
        }

        Ok(Self {
            documents,
            validators,
        })
    }

    pub fn ingest(&self, input: &[u8]) -> Result<ValidatedObject, CoreError> {
        self.ingest_with_limits(input, ResourceLimits::default())
    }

    pub fn ingest_with_limits(
        &self,
        input: &[u8],
        limits: ResourceLimits,
    ) -> Result<ValidatedObject, CoreError> {
        let value = parse_strict_with_limits(input, limits)?;
        self.finish_ingestion(value, limits, None)
    }

    pub fn ingest_claimed(
        &self,
        claimed_oid: &str,
        input: &[u8],
    ) -> Result<ValidatedObject, CoreError> {
        self.ingest_claimed_with_limits(claimed_oid, input, ResourceLimits::default())
    }

    pub fn ingest_claimed_with_limits(
        &self,
        claimed_oid: &str,
        input: &[u8],
        limits: ResourceLimits,
    ) -> Result<ValidatedObject, CoreError> {
        let value = parse_strict_with_limits(input, limits)?;
        self.finish_ingestion(value, limits, Some(claimed_oid))
    }

    pub fn validate(&self, value: &Value) -> Result<(), CoreError> {
        self.validate_with_limits(value, ResourceLimits::default())
    }

    pub fn validate_with_limits(
        &self,
        value: &Value,
        limits: ResourceLimits,
    ) -> Result<(), CoreError> {
        // Enforce canonical traversal limits before semantic recursion. This is
        // also required for callers constructing `Value` directly.
        canonical_bytes_with_limits(value, limits)?;
        let schema_name = select_schema(value)?;
        self.validate_selected(value, schema_name, limits)
    }

    fn finish_ingestion(
        &self,
        value: Value,
        limits: ResourceLimits,
        claimed_oid: Option<&str>,
    ) -> Result<ValidatedObject, CoreError> {
        let canonical_bytes = canonical_bytes_with_limits(&value, limits)?;
        let schema_name = select_schema(&value)?;
        self.validate_selected(&value, schema_name, limits)?;
        let oid = structured_oid_unchecked_with_limits(&value, limits)?;
        if let Some(claimed_oid) = claimed_oid {
            if claimed_oid != oid {
                return Err(CoreError::new(
                    ErrorCode::OidMismatch,
                    format!("claimed {claimed_oid}, expected {oid}"),
                ));
            }
        }
        Ok(ValidatedObject {
            value,
            canonical_bytes,
            oid,
            schema_name,
        })
    }

    fn validate_selected(
        &self,
        value: &Value,
        schema_name: &'static str,
        limits: ResourceLimits,
    ) -> Result<(), CoreError> {
        let root_schema = self.documents.get(schema_name).ok_or_else(|| {
            CoreError::new(
                ErrorCode::SchemaInvalid,
                format!("missing concrete schema {schema_name}"),
            )
        })?;

        // Stable semantic classifications are checked before the generic
        // schema error. Several of these constraints intentionally overlap a
        // schema keyword (for example set uniqueness and path syntax).
        self.validate_annotated(
            value,
            &[SchemaEntry {
                schema: root_schema,
                file: schema_name,
            }],
            "",
            limits,
        )?;
        self.validate_object_semantics(value, schema_name)?;

        let instance = to_json_value(value);
        let validator = self.validators.get(schema_name).ok_or_else(|| {
            CoreError::new(
                ErrorCode::SchemaInvalid,
                format!("missing compiled concrete schema {schema_name}"),
            )
        })?;
        if let Some(error) = validator.iter_errors(&instance).next() {
            return Err(CoreError::new(
                ErrorCode::SchemaInvalid,
                format!(
                    "{schema_name} rejected instance at {}: {error}",
                    display_pointer(error.instance_path.as_str())
                ),
            ));
        }
        Ok(())
    }
}

fn concrete_schema_names() -> impl Iterator<Item = &'static &'static str> {
    const BASE: &[&str] = &["manifest-tree.schema.json", "commit.schema.json"];
    RECORD_SCHEMAS
        .iter()
        .map(|(_, schema)| schema)
        .chain(BASE.iter())
}

fn select_schema(value: &Value) -> Result<&'static str, CoreError> {
    let object_type = value
        .get("object_type")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            CoreError::new(
                ErrorCode::SchemaInvalid,
                "structured object requires a string object_type",
            )
        })?;
    match object_type {
        "tree" => Ok("manifest-tree.schema.json"),
        "commit" => Ok("commit.schema.json"),
        "record" => {
            let record_type = value
                .get("record_type")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    CoreError::new(
                        ErrorCode::SchemaInvalid,
                        "record object requires a string record_type",
                    )
                })?;
            RECORD_SCHEMAS
                .iter()
                .find_map(|(candidate, schema)| (*candidate == record_type).then_some(*schema))
                .ok_or_else(|| {
                    CoreError::new(
                        ErrorCode::SchemaInvalid,
                        format!("unsupported record_type {record_type}"),
                    )
                })
        }
        other => Err(CoreError::new(
            ErrorCode::SchemaInvalid,
            format!("unsupported object_type {other}"),
        )),
    }
}

fn to_json_value(value: &Value) -> JsonValue {
    match value {
        Value::Null => JsonValue::Null,
        Value::Bool(value) => JsonValue::Bool(*value),
        Value::Integer(value) => JsonValue::Number((*value).into()),
        Value::String(value) => JsonValue::String(value.clone()),
        Value::Array(values) => JsonValue::Array(values.iter().map(to_json_value).collect()),
        Value::Object(entries) => JsonValue::Object(
            entries
                .iter()
                .map(|(key, value)| (key.clone(), to_json_value(value)))
                .collect(),
        ),
    }
}

fn audit_schema_node(file: &str, value: &JsonValue, pointer: &str) -> Result<(), CoreError> {
    match value {
        JsonValue::Array(values) => {
            for (index, child) in values.iter().enumerate() {
                audit_schema_node(file, child, &format!("{pointer}/{index}"))?;
            }
        }
        JsonValue::Object(object) => {
            if object.get("type").and_then(JsonValue::as_str) == Some("array") {
                let order = object.get("x-synapse-order").and_then(JsonValue::as_str);
                if !matches!(order, Some("set" | "sequence")) {
                    return Err(CoreError::new(
                        ErrorCode::SchemaInvalid,
                        format!("{file}{pointer} array lacks a valid x-synapse-order"),
                    ));
                }
                if order == Some("set")
                    && object.get("uniqueItems").and_then(JsonValue::as_bool) != Some(true)
                {
                    return Err(CoreError::new(
                        ErrorCode::SchemaInvalid,
                        format!("{file}{pointer} set lacks uniqueItems=true"),
                    ));
                }
            }
            for (key, child) in object {
                audit_schema_node(file, child, &format!("{pointer}/{}", escape_pointer(key)))?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn audit_record_dispatch(documents: &HashMap<&'static str, JsonValue>) -> Result<(), CoreError> {
    let envelope = documents
        .get("record-envelope.schema.json")
        .and_then(|schema| schema.pointer("/properties/record_type/enum"))
        .and_then(JsonValue::as_array)
        .ok_or_else(|| {
            CoreError::new(
                ErrorCode::SchemaInvalid,
                "record envelope lacks record_type enum",
            )
        })?;
    let declared: HashSet<&str> = envelope.iter().filter_map(JsonValue::as_str).collect();
    let expected: HashSet<&str> = RECORD_SCHEMAS.iter().map(|(kind, _)| *kind).collect();
    if declared != expected {
        return Err(CoreError::new(
            ErrorCode::SchemaInvalid,
            "record envelope and concrete Rust dispatcher disagree",
        ));
    }

    let dispatcher = documents
        .get("record.schema.json")
        .and_then(|schema| schema.get("oneOf"))
        .and_then(JsonValue::as_array)
        .ok_or_else(|| {
            CoreError::new(
                ErrorCode::SchemaInvalid,
                "record.schema.json lacks oneOf dispatcher",
            )
        })?;
    let dispatched: HashSet<&str> = dispatcher
        .iter()
        .filter_map(|entry| entry.get("$ref"))
        .filter_map(JsonValue::as_str)
        .collect();
    let expected_files: HashSet<&str> = RECORD_SCHEMAS.iter().map(|(_, file)| *file).collect();
    if dispatched != expected_files {
        return Err(CoreError::new(
            ErrorCode::SchemaInvalid,
            "record.schema.json and concrete Rust dispatcher disagree",
        ));
    }
    Ok(())
}

impl SchemaRegistry {
    fn expand_entries<'a>(&'a self, entries: &[SchemaEntry<'a>]) -> Vec<SchemaEntry<'a>> {
        fn visit<'a>(
            registry: &'a SchemaRegistry,
            entry: SchemaEntry<'a>,
            seen: &mut HashSet<*const JsonValue>,
            output: &mut Vec<SchemaEntry<'a>>,
        ) {
            if !entry.schema.is_object() || !seen.insert(entry.schema as *const JsonValue) {
                return;
            }
            output.push(entry);
            if let Some(reference) = entry.schema.get("$ref").and_then(JsonValue::as_str) {
                if let Ok(resolved) = registry.resolve_schema_ref(reference, entry.file) {
                    visit(registry, resolved, seen, output);
                }
            }
            for keyword in ["allOf", "oneOf", "anyOf"] {
                if let Some(children) = entry.schema.get(keyword).and_then(JsonValue::as_array) {
                    for child in children {
                        visit(
                            registry,
                            SchemaEntry {
                                schema: child,
                                file: entry.file,
                            },
                            seen,
                            output,
                        );
                    }
                }
            }
        }

        let mut seen = HashSet::new();
        let mut output = Vec::new();
        for &entry in entries {
            visit(self, entry, &mut seen, &mut output);
        }
        output
    }

    fn resolve_schema_ref<'a>(
        &'a self,
        reference: &str,
        base_file: &'static str,
    ) -> Result<SchemaEntry<'a>, CoreError> {
        let (file_part, fragment) = reference.split_once('#').unwrap_or((reference, ""));
        let requested = if file_part.is_empty() {
            base_file
        } else {
            file_part
        };
        let (&file, document) = self.documents.get_key_value(requested).ok_or_else(|| {
            CoreError::new(
                ErrorCode::SchemaInvalid,
                format!("{base_file} references missing schema {requested}"),
            )
        })?;
        let schema = if fragment.is_empty() {
            document
        } else {
            if !fragment.starts_with('/') {
                return Err(CoreError::new(
                    ErrorCode::SchemaInvalid,
                    format!("unsupported schema fragment in {reference}"),
                ));
            }
            document.pointer(fragment).ok_or_else(|| {
                CoreError::new(
                    ErrorCode::SchemaInvalid,
                    format!("schema reference {reference} does not resolve"),
                )
            })?
        };
        Ok(SchemaEntry { schema, file })
    }

    fn property_entries<'a>(
        &'a self,
        entries: &[SchemaEntry<'a>],
        property: &str,
    ) -> Vec<SchemaEntry<'a>> {
        let mut output = Vec::new();
        for entry in self.expand_entries(entries) {
            if let Some(schema) = entry
                .schema
                .get("properties")
                .and_then(|properties| properties.get(property))
            {
                output.push(SchemaEntry {
                    schema,
                    file: entry.file,
                });
            } else if let Some(schema) = entry
                .schema
                .get("additionalProperties")
                .filter(|schema| schema.is_object())
            {
                output.push(SchemaEntry {
                    schema,
                    file: entry.file,
                });
            }
        }
        output
    }

    fn item_entries<'a>(&'a self, entries: &[SchemaEntry<'a>]) -> Vec<SchemaEntry<'a>> {
        self.expand_entries(entries)
            .into_iter()
            .filter_map(|entry| {
                entry
                    .schema
                    .get("items")
                    .filter(|schema| schema.is_object())
                    .map(|schema| SchemaEntry {
                        schema,
                        file: entry.file,
                    })
            })
            .collect()
    }

    fn entries_include_common_def(&self, expanded: &[SchemaEntry<'_>], definition: &str) -> bool {
        let Some(target) = self
            .documents
            .get("common.schema.json")
            .and_then(|schema| schema.get("$defs"))
            .and_then(|definitions| definitions.get(definition))
        else {
            return false;
        };
        expanded
            .iter()
            .any(|entry| std::ptr::eq(entry.schema, target))
    }

    fn validate_annotated(
        &self,
        value: &Value,
        entries: &[SchemaEntry<'_>],
        pointer: &str,
        limits: ResourceLimits,
    ) -> Result<(), CoreError> {
        let expanded = self.expand_entries(entries);
        match value {
            Value::String(value) => {
                let mut identifier_nfc = false;
                let mut canonical_timestamp = false;
                for entry in &expanded {
                    match entry
                        .schema
                        .get("x-synapse-string")
                        .and_then(JsonValue::as_str)
                    {
                        Some("identifier-nfc") => identifier_nfc = true,
                        Some("canonical-timestamp") => canonical_timestamp = true,
                        _ => {}
                    }
                }
                if identifier_nfc && !value.nfc().eq(value.chars()) {
                    return Err(CoreError::new(
                        ErrorCode::IdentifierNotNfc,
                        format!("{} is not NFC", display_pointer(pointer)),
                    ));
                }
                if canonical_timestamp {
                    validate_timestamp(value, pointer)?;
                }
            }
            Value::Array(values) => {
                let mut order = None;
                for entry in &expanded {
                    if let Some(candidate) = entry
                        .schema
                        .get("x-synapse-order")
                        .and_then(JsonValue::as_str)
                    {
                        if order.is_some_and(|current| current != candidate) {
                            return Err(CoreError::new(
                                ErrorCode::SchemaInvalid,
                                format!(
                                    "{} has conflicting array order annotations",
                                    display_pointer(pointer)
                                ),
                            ));
                        }
                        order = Some(candidate);
                    }
                }
                if order == Some("set") {
                    let mut previous: Option<Vec<u8>> = None;
                    for item in values {
                        let encoded = canonical_bytes_with_limits(item, limits)?;
                        if let Some(previous) = previous.as_ref() {
                            match previous.cmp(&encoded) {
                                Ordering::Equal => {
                                    return Err(CoreError::new(
                                        ErrorCode::SetDuplicate,
                                        format!(
                                            "{} has duplicate set items",
                                            display_pointer(pointer)
                                        ),
                                    ));
                                }
                                Ordering::Greater => {
                                    return Err(CoreError::new(
                                        ErrorCode::SetNotSorted,
                                        format!(
                                            "{} is not in canonical set order",
                                            display_pointer(pointer)
                                        ),
                                    ));
                                }
                                Ordering::Less => {}
                            }
                        }
                        previous = Some(encoded);
                    }
                }
                let child_entries = self.item_entries(&expanded);
                for (index, child) in values.iter().enumerate() {
                    self.validate_annotated(
                        child,
                        &child_entries,
                        &format!("{pointer}/{index}"),
                        limits,
                    )?;
                }
            }
            Value::Object(object) => {
                if self.entries_include_common_def(&expanded, "ScaledInteger") {
                    validate_scaled_integer(value, pointer)?;
                }
                if self.entries_include_common_def(&expanded, "ValidTime") {
                    validate_valid_time(value, pointer)?;
                }
                if self.entries_include_common_def(&expanded, "TemporalPrecision") {
                    validate_temporal_precision(value, pointer)?;
                }
                if self.entries_include_common_def(&expanded, "Confidence") {
                    validate_confidence(value, pointer)?;
                }

                for (key, child) in object {
                    let child_entries = self.property_entries(&expanded, key);
                    self.validate_annotated(
                        child,
                        &child_entries,
                        &format!("{pointer}/{}", escape_pointer(key)),
                        limits,
                    )?;
                }
            }
            Value::Null | Value::Bool(_) | Value::Integer(_) => {}
        }
        Ok(())
    }

    fn validate_object_semantics(
        &self,
        value: &Value,
        schema_name: &'static str,
    ) -> Result<(), CoreError> {
        if schema_name == "manifest-tree.schema.json" {
            validate_manifest(value)?;
        }
        match value.get("record_type").and_then(Value::as_str) {
            Some("observation") if value.get("valid_time").is_some() => {
                return Err(CoreError::new(
                    ErrorCode::SchemaInvalid,
                    "Observation v0.1 must use payload.capture_time, not Envelope valid_time",
                ));
            }
            Some("delegation_grant") => validate_delegation_expiration(value)?,
            _ => {}
        }
        Ok(())
    }
}

fn validate_manifest(value: &Value) -> Result<(), CoreError> {
    let Some(entries) = value.get("entries").and_then(Value::as_object) else {
        return Ok(());
    };
    for (segment, entry) in entries {
        if segment.is_empty()
            || segment == "."
            || segment == ".."
            || segment.contains('/')
            || segment.contains('\0')
        {
            return Err(CoreError::new(
                ErrorCode::PathSegmentInvalid,
                format!("invalid Manifest segment {segment:?}"),
            ));
        }
        let Some(entry_kind) = entry.get("entry_kind").and_then(Value::as_str) else {
            continue;
        };
        let Some(oid) = entry.get("oid").and_then(Value::as_str) else {
            continue;
        };
        let prefix = oid.split(':').next().unwrap_or_default();
        if prefix != entry_kind {
            return Err(CoreError::new(
                ErrorCode::ReferenceTypeMismatch,
                format!("{segment:?} says {entry_kind} but uses {oid}"),
            ));
        }
    }
    Ok(())
}

fn validate_delegation_expiration(value: &Value) -> Result<(), CoreError> {
    let Some(recorded_at) = value.get("recorded_at").and_then(Value::as_str) else {
        return Ok(());
    };
    let Some(expires_at) = value
        .get("payload")
        .and_then(|payload| payload.get("expires_at"))
        .and_then(Value::as_str)
    else {
        return Ok(());
    };
    validate_timestamp(recorded_at, "/recorded_at")?;
    validate_timestamp(expires_at, "/payload/expires_at")?;
    if expires_at < recorded_at {
        return Err(CoreError::new(
            ErrorCode::IntervalInvalid,
            "DelegationGrant expires before recorded_at",
        ));
    }
    Ok(())
}

fn validate_timestamp(value: &str, pointer: &str) -> Result<(), CoreError> {
    let bytes = value.as_bytes();
    let lexical = bytes.len() == 30
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes[10] == b'T'
        && bytes[13] == b':'
        && bytes[16] == b':'
        && bytes[19] == b'.'
        && bytes[29] == b'Z'
        && bytes.iter().enumerate().all(|(index, byte)| {
            matches!(index, 4 | 7 | 10 | 13 | 16 | 19 | 29) || byte.is_ascii_digit()
        });
    if !lexical {
        return Err(CoreError::new(
            ErrorCode::TimestampInvalid,
            format!(
                "invalid canonical timestamp at {}: {value}",
                display_pointer(pointer)
            ),
        ));
    }

    let number = |start: usize, end: usize| -> u32 {
        bytes[start..end]
            .iter()
            .fold(0, |value, digit| value * 10 + u32::from(digit - b'0'))
    };
    let year = number(0, 4);
    let month = number(5, 7);
    let day = number(8, 10);
    let hour = number(11, 13);
    let minute = number(14, 16);
    let second = number(17, 19);
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let days = [
        31,
        if leap { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    let calendar_valid = (1..=12).contains(&month)
        && day >= 1
        && day <= days[(month - 1) as usize]
        && hour <= 23
        && minute <= 59
        && second <= 59;
    if !calendar_valid {
        return Err(CoreError::new(
            ErrorCode::TimestampInvalid,
            format!(
                "invalid calendar timestamp at {}: {value}",
                display_pointer(pointer)
            ),
        ));
    }
    Ok(())
}

fn validate_valid_time(value: &Value, pointer: &str) -> Result<(), CoreError> {
    if value.get("kind").and_then(Value::as_str) != Some("interval") {
        return Ok(());
    }
    let from = value.get("from").and_then(Value::as_str);
    let to = value.get("to").and_then(Value::as_str);
    if let (Some(from), Some(to)) = (from, to) {
        validate_timestamp(from, &format!("{pointer}/from"))?;
        validate_timestamp(to, &format!("{pointer}/to"))?;
        if from > to {
            return Err(CoreError::new(
                ErrorCode::IntervalInvalid,
                format!("{} has from later than to", display_pointer(pointer)),
            ));
        }
    }
    Ok(())
}

fn validate_temporal_precision(value: &Value, pointer: &str) -> Result<(), CoreError> {
    if !matches!(
        value.get("kind").and_then(Value::as_str),
        Some("resolution" | "uncertainty")
    ) {
        return Ok(());
    }
    let Some(scaled) = value.get("value") else {
        return Ok(());
    };
    let Some(number) = scaled_integer(scaled) else {
        return Ok(());
    };
    if number.negative || !matches!(number.unit, "ms" | "s") {
        return Err(CoreError::new(
            ErrorCode::FixedPointNotNormalized,
            format!(
                "{} has invalid temporal precision",
                display_pointer(pointer)
            ),
        ));
    }
    Ok(())
}

const UNITS: &[&str] = &[
    "unitless", "ratio", "percent", "count", "byte", "px", "mm", "m", "ms", "s", "deg", "rad",
    "kelvin", "celsius", "delta_e",
];

#[derive(Clone, Copy)]
struct ScaledNumber<'a> {
    negative: bool,
    digits: &'a str,
    scale: i64,
    unit: &'a str,
}

fn validate_scaled_integer(value: &Value, pointer: &str) -> Result<(), CoreError> {
    let Some(object) = value.as_object() else {
        return Ok(());
    };
    let mantissa = object
        .iter()
        .find_map(|(key, value)| (key == "mantissa").then_some(value))
        .and_then(Value::as_str);
    let scale = object
        .iter()
        .find_map(|(key, value)| (key == "scale").then_some(value))
        .and_then(Value::as_i64);
    let unit = object
        .iter()
        .find_map(|(key, value)| (key == "unit").then_some(value))
        .and_then(Value::as_str);
    let (Some(mantissa), Some(scale), Some(unit)) = (mantissa, scale, unit) else {
        // Required fields and their primitive types are schema concerns.
        return Ok(());
    };

    let digits = mantissa.strip_prefix('-').unwrap_or(mantissa);
    let mantissa_valid = mantissa == "0"
        || (!digits.is_empty()
            && !digits.starts_with('0')
            && !digits.ends_with('0')
            && digits.bytes().all(|byte| byte.is_ascii_digit()));
    if !mantissa_valid
        || mantissa.len() > 257
        || !(-24..=24).contains(&scale)
        || !UNITS.contains(&unit)
        || (mantissa == "0" && scale != 0)
    {
        return Err(CoreError::new(
            ErrorCode::FixedPointNotNormalized,
            format!("invalid ScaledInteger at {}", display_pointer(pointer)),
        ));
    }
    Ok(())
}

fn scaled_integer(value: &Value) -> Option<ScaledNumber<'_>> {
    let mantissa = value.get("mantissa")?.as_str()?;
    let scale = value.get("scale")?.as_i64()?;
    let unit = value.get("unit")?.as_str()?;
    Some(ScaledNumber {
        negative: mantissa.starts_with('-'),
        digits: mantissa.strip_prefix('-').unwrap_or(mantissa),
        scale,
        unit,
    })
}

fn validate_confidence(value: &Value, pointer: &str) -> Result<(), CoreError> {
    match value.get("kind").and_then(Value::as_str) {
        Some("probability") => {
            let Some(number) = value.get("value").and_then(scaled_integer) else {
                return Ok(());
            };
            let zero = ScaledNumber {
                negative: false,
                digits: "0",
                scale: 0,
                unit: "ratio",
            };
            let one = ScaledNumber {
                negative: false,
                digits: "1",
                scale: 0,
                unit: "ratio",
            };
            if number.unit != "ratio"
                || compare_scaled(number, zero) == Ordering::Less
                || compare_scaled(number, one) == Ordering::Greater
            {
                return Err(CoreError::new(
                    ErrorCode::FixedPointNotNormalized,
                    format!(
                        "{} probability must be a ratio from 0 through 1",
                        display_pointer(pointer)
                    ),
                ));
            }
        }
        Some("interval") => {
            let Some(lower) = value.get("lower").and_then(scaled_integer) else {
                return Ok(());
            };
            let Some(upper) = value.get("upper").and_then(scaled_integer) else {
                return Ok(());
            };
            if lower.unit != upper.unit {
                return Err(CoreError::new(
                    ErrorCode::FixedPointNotNormalized,
                    format!(
                        "{} confidence interval bounds use different units",
                        display_pointer(pointer)
                    ),
                ));
            }
            if compare_scaled(lower, upper) == Ordering::Greater {
                return Err(CoreError::new(
                    ErrorCode::IntervalInvalid,
                    format!(
                        "{} confidence interval lower exceeds upper",
                        display_pointer(pointer)
                    ),
                ));
            }
        }
        _ => {}
    }
    Ok(())
}

fn compare_scaled(left: ScaledNumber<'_>, right: ScaledNumber<'_>) -> Ordering {
    if left.digits == "0" {
        return if right.digits == "0" {
            Ordering::Equal
        } else if right.negative {
            Ordering::Greater
        } else {
            Ordering::Less
        };
    }
    if right.digits == "0" {
        return if left.negative {
            Ordering::Less
        } else {
            Ordering::Greater
        };
    }
    if left.negative != right.negative {
        return if left.negative {
            Ordering::Less
        } else {
            Ordering::Greater
        };
    }

    let common_scale = left.scale.min(right.scale);
    let left_zeroes = usize::try_from(left.scale - common_scale).unwrap_or(0);
    let right_zeroes = usize::try_from(right.scale - common_scale).unwrap_or(0);
    let mut left_aligned = String::with_capacity(left.digits.len() + left_zeroes);
    left_aligned.push_str(left.digits);
    left_aligned.extend(std::iter::repeat_n('0', left_zeroes));
    let mut right_aligned = String::with_capacity(right.digits.len() + right_zeroes);
    right_aligned.push_str(right.digits);
    right_aligned.extend(std::iter::repeat_n('0', right_zeroes));
    let absolute = left_aligned
        .len()
        .cmp(&right_aligned.len())
        .then_with(|| left_aligned.cmp(&right_aligned));
    if left.negative {
        absolute.reverse()
    } else {
        absolute
    }
}

fn escape_pointer(part: &str) -> String {
    part.replace('~', "~0").replace('/', "~1")
}

fn display_pointer(pointer: &str) -> &str {
    if pointer.is_empty() { "/" } else { pointer }
}
