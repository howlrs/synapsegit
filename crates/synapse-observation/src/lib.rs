//! Conservative first-party Observation analysis adapters.
//!
//! The initial adapter records only content-addressed byte identity. It does
//! not decode media or infer appearance or physical change.

#![forbid(unsafe_code)]

mod byte_identity;

use serde_json::{Map as JsonMap, Value as JsonValue, json};
use std::collections::BTreeSet;
use std::error::Error;
use std::fmt;
use std::io::Cursor;
use synapse_canonical::{
    CoreError, ObjectKind, Value as CanonicalValue, blob_oid, canonical_bytes, parse_strict,
};
use synapse_core::{Repository, RepositoryError};
use synapse_schema::{ingest, validate};

pub const BYTE_IDENTITY_ADAPTER_ID: &str = "synapsegit.observation.byte-identity";
pub const BYTE_IDENTITY_ADAPTER_VERSION: &str = "1";
const SCHEMA_VERSION: &str = "0.1.0";
const LIBRARY_SOURCE: &[u8] = include_bytes!("lib.rs");
const ALGORITHM_SOURCE: &[u8] = include_bytes!("byte_identity.rs");
const PACKAGE_MANIFEST: &[u8] = include_bytes!("../Cargo.toml");
const CONFIGURATION_BYTES: &[u8] = b"{\"algorithm\":\"blob_oid_equality\",\"media_interpretation\":\"none\",\"profile\":\"synapsegit-observation-byte-identity-v1\"}\n";

/// Ordered request for one immutable byte-identity AnalysisResult.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ByteIdentityComparisonRequest {
    pub base_observation_oid: String,
    pub target_observation_oid: String,
    pub analysis_entity_id: String,
    pub asserted_by: String,
    pub recorded_at: String,
}

/// Whether byte identity was evaluated. Neither outcome describes decoded
/// media or the physical subject.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ByteIdentityOutcome {
    Identical,
    Different,
    NotCompared,
}

impl ByteIdentityOutcome {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Identical => "identical",
            Self::Different => "different",
            Self::NotCompared => "not_compared",
        }
    }

    pub const fn byte_identical(self) -> Option<bool> {
        match self {
            Self::Identical => Some(true),
            Self::Different => Some(false),
            Self::NotCompared => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AnalysisStatus {
    Succeeded,
    NotRun,
}

impl AnalysisStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Succeeded => "succeeded",
            Self::NotRun => "not_run",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AnalysisComparability {
    Partial,
    Incomparable,
}

impl AnalysisComparability {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Partial => "partial",
            Self::Incomparable => "incomparable",
        }
    }
}

/// OIDs and conservative interpretation emitted by a successful CAS write.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ByteIdentityComparisonReceipt {
    pub analysis_oid: String,
    pub implementation_oid: String,
    pub configuration_oid: String,
    pub base_observation_oid: String,
    pub target_observation_oid: String,
    pub base_media_oid: Option<String>,
    pub target_media_oid: Option<String>,
    pub outcome: ByteIdentityOutcome,
    pub status: AnalysisStatus,
    pub comparability: AnalysisComparability,
    pub reason_codes: Vec<String>,
}

#[derive(Debug)]
pub enum ObservationAnalysisError {
    InvalidInput(String),
    MissingObject(String),
    Core(CoreError),
    Repository(RepositoryError),
    Json(serde_json::Error),
}

impl ObservationAnalysisError {
    pub fn code(&self) -> &str {
        match self {
            Self::InvalidInput(_) => "observation_input_invalid",
            Self::MissingObject(_) => "closure_missing",
            Self::Core(error) => error.code().as_str(),
            Self::Repository(error) => error.code(),
            Self::Json(_) => "schema_invalid",
        }
    }
}

impl fmt::Display for ObservationAnalysisError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidInput(message) | Self::MissingObject(message) => {
                formatter.write_str(message)
            }
            Self::Core(error) => error.fmt(formatter),
            Self::Repository(error) => error.fmt(formatter),
            Self::Json(error) => write!(formatter, "observation analysis JSON error: {error}"),
        }
    }
}

impl Error for ObservationAnalysisError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Core(error) => Some(error),
            Self::Repository(error) => Some(error),
            Self::Json(error) => Some(error),
            Self::InvalidInput(_) | Self::MissingObject(_) => None,
        }
    }
}

impl From<CoreError> for ObservationAnalysisError {
    fn from(error: CoreError) -> Self {
        Self::Core(error)
    }
}

impl From<RepositoryError> for ObservationAnalysisError {
    fn from(error: RepositoryError) -> Self {
        Self::Repository(error)
    }
}

impl From<serde_json::Error> for ObservationAnalysisError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

pub type Result<T> = std::result::Result<T, ObservationAnalysisError>;

/// OID of a deterministic bundle containing the semantic adapter sources and
/// crate manifest compiled into this crate.
pub fn byte_identity_implementation_oid() -> String {
    blob_oid(&implementation_bundle())
}

/// OID of the fixed Stage 0 byte-identity configuration.
pub fn byte_identity_configuration_oid() -> String {
    blob_oid(CONFIGURATION_BYTES)
}

/// Validate two ordered Observations and record one immutable byte-identity
/// AnalysisResult plus its implementation/configuration Blobs.
///
/// No Ref is updated. Missing or corrupt Observations, CaptureProfiles, or
/// media Blobs fail before the adapter writes any of its own objects. Other
/// optional Observation dependencies remain the Ref-publishing caller's
/// closure-validation responsibility. Subject/series mismatch and absent or
/// ambiguous primary-media roles are normal `not_run`/`incomparable` results.
pub fn record_byte_identity_comparison(
    repository: &Repository,
    request: &ByteIdentityComparisonRequest,
) -> Result<ByteIdentityComparisonReceipt> {
    let base = load_observation(repository, &request.base_observation_oid)?;
    let target = load_observation(repository, &request.target_observation_oid)?;

    for oid in base.media_oids.iter().chain(&target.media_oids) {
        verify_blob(repository, oid)?;
    }

    let mut reasons = BTreeSet::from(["byte_identity_only".to_owned()]);
    if base.capture_time_unknown || target.capture_time_unknown {
        reasons.insert("capture_time_unknown".into());
    }
    if base.capture_profile_level.as_deref() == Some("imported")
        || target.capture_profile_level.as_deref() == Some("imported")
    {
        reasons.insert("capture_profile_imported".into());
    }
    if base.capture_profile_level.is_none() || target.capture_profile_level.is_none() {
        reasons.insert("capture_profile_missing".into());
    }

    let (outcome, status, comparability, base_media_oid, target_media_oid) =
        if base.subject != target.subject {
            reasons.insert("subject_mismatch".into());
            (
                ByteIdentityOutcome::NotCompared,
                AnalysisStatus::NotRun,
                AnalysisComparability::Incomparable,
                base.only_primary(),
                target.only_primary(),
            )
        } else if base.series != target.series {
            reasons.insert("series_mismatch".into());
            (
                ByteIdentityOutcome::NotCompared,
                AnalysisStatus::NotRun,
                AnalysisComparability::Incomparable,
                base.only_primary(),
                target.only_primary(),
            )
        } else if base.primary_media.is_empty() || target.primary_media.is_empty() {
            reasons.insert("primary_media_missing".into());
            (
                ByteIdentityOutcome::NotCompared,
                AnalysisStatus::NotRun,
                AnalysisComparability::Incomparable,
                base.only_primary(),
                target.only_primary(),
            )
        } else if base.primary_media.len() != 1 || target.primary_media.len() != 1 {
            reasons.insert("primary_media_ambiguous".into());
            (
                ByteIdentityOutcome::NotCompared,
                AnalysisStatus::NotRun,
                AnalysisComparability::Incomparable,
                base.only_primary(),
                target.only_primary(),
            )
        } else {
            let base_media = base.primary_media[0].clone();
            let target_media = target.primary_media[0].clone();
            let outcome = if byte_identity::media_oids_are_identical(&base_media, &target_media) {
                ByteIdentityOutcome::Identical
            } else {
                ByteIdentityOutcome::Different
            };
            (
                outcome,
                AnalysisStatus::Succeeded,
                AnalysisComparability::Partial,
                Some(base_media),
                Some(target_media),
            )
        };

    let implementation_bytes = implementation_bundle();
    let implementation_oid = blob_oid(&implementation_bytes);
    let configuration_oid = byte_identity_configuration_oid();
    let decision = ComparisonDecision {
        outcome,
        status,
        comparability,
        reason_codes: reasons.into_iter().collect(),
        base_media_oid,
        target_media_oid,
    };
    let analysis = analysis_record(request, &implementation_oid, &configuration_oid, &decision)?;
    let encoded = serde_json::to_vec(&analysis)?;
    let validated = ingest(&encoded)?;
    let analysis_oid = validated.oid().to_owned();

    repository.put_blob_claimed(&implementation_oid, Cursor::new(&implementation_bytes))?;
    repository.put_blob_claimed(&configuration_oid, Cursor::new(CONFIGURATION_BYTES))?;
    repository.put_object_claimed(&analysis_oid, &encoded)?;

    Ok(ByteIdentityComparisonReceipt {
        analysis_oid,
        implementation_oid,
        configuration_oid,
        base_observation_oid: request.base_observation_oid.clone(),
        target_observation_oid: request.target_observation_oid.clone(),
        base_media_oid: decision.base_media_oid,
        target_media_oid: decision.target_media_oid,
        outcome: decision.outcome,
        status: decision.status,
        comparability: decision.comparability,
        reason_codes: decision.reason_codes,
    })
}

struct ComparisonDecision {
    outcome: ByteIdentityOutcome,
    status: AnalysisStatus,
    comparability: AnalysisComparability,
    reason_codes: Vec<String>,
    base_media_oid: Option<String>,
    target_media_oid: Option<String>,
}

#[derive(Debug)]
struct ObservationFacts {
    subject: String,
    series: String,
    capture_time_unknown: bool,
    capture_profile_level: Option<String>,
    media_oids: Vec<String>,
    primary_media: Vec<String>,
}

impl ObservationFacts {
    fn only_primary(&self) -> Option<String> {
        (self.primary_media.len() == 1).then(|| self.primary_media[0].clone())
    }
}

fn load_observation(repository: &Repository, oid: &str) -> Result<ObservationFacts> {
    let record = load_record(repository, oid, "Observation")?;
    require_string(&record, "record_type", "Observation record_type")
        .and_then(|value| require_equal(value, "observation", "Observation record_type"))?;
    let payload = require_object(&record, "payload", "Observation payload")?;
    let subject = require_string(payload, "subject_ref", "Observation subject_ref")?.to_owned();
    let series = require_string(payload, "series_ref", "Observation series_ref")?.to_owned();
    let capture_time = require_object(payload, "capture_time", "Observation capture_time")?;
    let capture_time_unknown =
        require_string(capture_time, "kind", "Observation capture_time kind")? == "unknown";
    let media_refs = payload
        .get("media_refs")
        .and_then(CanonicalValue::as_array)
        .ok_or_else(|| {
            ObservationAnalysisError::InvalidInput(
                "Observation media_refs is missing or invalid".into(),
            )
        })?;
    let mut primary_media = Vec::new();
    let mut media_oids = Vec::with_capacity(media_refs.len());
    for media_ref in media_refs {
        let role = require_string(media_ref, "role", "Observation media role")?;
        let oid = require_string(media_ref, "oid", "Observation media OID")?.to_owned();
        if role == "primary" {
            primary_media.push(oid.clone());
        }
        media_oids.push(oid);
    }
    let capture_profile_level = payload
        .get("capture_profile_ref")
        .map(|value| {
            value.as_str().ok_or_else(|| {
                ObservationAnalysisError::InvalidInput(
                    "Observation capture_profile_ref is invalid".into(),
                )
            })
        })
        .transpose()?
        .map(|profile_oid| load_capture_profile_level(repository, profile_oid))
        .transpose()?;
    Ok(ObservationFacts {
        subject,
        series,
        capture_time_unknown,
        capture_profile_level,
        media_oids,
        primary_media,
    })
}

fn load_capture_profile_level(repository: &Repository, oid: &str) -> Result<String> {
    let record = load_record(repository, oid, "CaptureProfile")?;
    require_string(&record, "record_type", "CaptureProfile record_type")
        .and_then(|value| require_equal(value, "capture_profile", "CaptureProfile record_type"))?;
    let payload = require_object(&record, "payload", "CaptureProfile payload")?;
    Ok(require_string(payload, "profile_level", "CaptureProfile profile_level")?.to_owned())
}

fn load_record(repository: &Repository, oid: &str, label: &str) -> Result<CanonicalValue> {
    let object = repository
        .objects()
        .get_verified(oid)
        .map_err(|error| ObservationAnalysisError::Repository(error.into()))?
        .ok_or_else(|| {
            ObservationAnalysisError::MissingObject(format!("{label} object is missing: {oid}"))
        })?;
    if object.kind() != ObjectKind::Record {
        return Err(ObservationAnalysisError::InvalidInput(format!(
            "{label} OID is not a Record: {oid}"
        )));
    }
    let value = object.structured().ok_or_else(|| {
        ObservationAnalysisError::InvalidInput(format!("{label} has no structured body: {oid}"))
    })?;
    validate(value)?;
    Ok(value.clone())
}

fn verify_blob(repository: &Repository, oid: &str) -> Result<()> {
    let object = repository
        .objects()
        .get_verified(oid)
        .map_err(|error| ObservationAnalysisError::Repository(error.into()))?
        .ok_or_else(|| {
            ObservationAnalysisError::MissingObject(format!("media Blob is missing: {oid}"))
        })?;
    if object.kind() != ObjectKind::Blob {
        return Err(ObservationAnalysisError::InvalidInput(format!(
            "media OID is not a Blob: {oid}"
        )));
    }
    Ok(())
}

fn analysis_record(
    request: &ByteIdentityComparisonRequest,
    implementation_oid: &str,
    configuration_oid: &str,
    decision: &ComparisonDecision,
) -> Result<JsonValue> {
    let source_refs = canonical_set(vec![
        json!({ "role": "base_observation", "oid": request.base_observation_oid }),
        json!({ "role": "target_observation", "oid": request.target_observation_oid }),
    ])?;
    let reason_codes = canonical_set(
        decision
            .reason_codes
            .iter()
            .map(|value| json!(value))
            .collect(),
    )?;
    let mut metrics = JsonMap::new();
    if let Some(byte_identical) = decision.outcome.byte_identical() {
        metrics.insert(
            "byte_identical".into(),
            json!({
                "mantissa": if byte_identical { "1" } else { "0" },
                "scale": 0,
                "unit": "unitless"
            }),
        );
    }
    let warning = match decision.outcome {
        ByteIdentityOutcome::Identical => {
            "Identical Blob bytes do not establish that the observed physical subject was unchanged."
        }
        ByteIdentityOutcome::Different => {
            "Different Blob bytes do not establish visual or physical change."
        }
        ByteIdentityOutcome::NotCompared => {
            "Byte identity was not compared because the ordered Observation inputs were incompatible."
        }
    };
    let mut evidence = JsonMap::new();
    evidence.insert(
        "format".into(),
        json!("synapsegit-observation-byte-identity-v1"),
    );
    evidence.insert("outcome".into(), json!(decision.outcome.as_str()));
    if let Some(oid) = &decision.base_media_oid {
        evidence.insert("base_media_ref".into(), json!(oid));
    }
    if let Some(oid) = &decision.target_media_oid {
        evidence.insert("target_media_ref".into(), json!(oid));
    }
    Ok(json!({
        "object_type": "record",
        "schema_version": SCHEMA_VERSION,
        "record_type": "analysis_result",
        "entity_id": request.analysis_entity_id,
        "recorded_at": request.recorded_at,
        "asserted_by": request.asserted_by,
        "origin": "tool_recorded",
        "source_refs": source_refs,
        "payload": {
            "analysis_kind": "byte_identity",
            "comparison_kind": "temporal_observation",
            "inputs": [
                { "role": "base_observation", "ref": request.base_observation_oid },
                { "role": "target_observation", "ref": request.target_observation_oid }
            ],
            "adapter": {
                "id": BYTE_IDENTITY_ADAPTER_ID,
                "version": BYTE_IDENTITY_ADAPTER_VERSION,
                "implementation_digest": implementation_oid,
                "configuration_digest": configuration_oid,
                "determinism": "deterministic"
            },
            "status": decision.status.as_str(),
            "comparability": decision.comparability.as_str(),
            "reason_codes": reason_codes,
            "derived_blob_refs": [],
            "metrics": metrics,
            "warnings": [warning],
            "limitations": [
                "This adapter compares verified Blob OIDs only and does not decode media, inspect pixels, register viewpoints, or infer appearance or physical change.",
                "The implementation digest covers the semantic Rust source files and crate manifest, not Cargo.lock, transitive dependency sources, compiler, target, operating system, or full runtime environment."
            ]
        },
        "extensions": {
            "org.synapsegit.observation-byte-identity": evidence
        }
    }))
}

fn implementation_bundle() -> Vec<u8> {
    let members = [
        ("Cargo.toml", PACKAGE_MANIFEST),
        ("src/byte_identity.rs", ALGORITHM_SOURCE),
        ("src/lib.rs", LIBRARY_SOURCE),
    ];
    let mut bundle = b"synapsegit-observation-implementation-bundle-v1\0".to_vec();
    for (name, bytes) in members {
        bundle.extend_from_slice(&(name.len() as u64).to_be_bytes());
        bundle.extend_from_slice(name.as_bytes());
        bundle.extend_from_slice(&(bytes.len() as u64).to_be_bytes());
        bundle.extend_from_slice(bytes);
    }
    bundle
}

fn canonical_set(values: Vec<JsonValue>) -> Result<JsonValue> {
    let mut keyed = Vec::with_capacity(values.len());
    for value in values {
        let encoded = serde_json::to_vec(&value)?;
        let parsed = parse_strict(&encoded)?;
        keyed.push((canonical_bytes(&parsed)?, value));
    }
    keyed.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(JsonValue::Array(
        keyed.into_iter().map(|(_, value)| value).collect(),
    ))
}

fn require_object<'a>(
    value: &'a CanonicalValue,
    key: &str,
    label: &str,
) -> Result<&'a CanonicalValue> {
    value
        .get(key)
        .filter(|value| value.as_object().is_some())
        .ok_or_else(|| {
            ObservationAnalysisError::InvalidInput(format!("{label} is missing or invalid"))
        })
}

fn require_string<'a>(value: &'a CanonicalValue, key: &str, label: &str) -> Result<&'a str> {
    value
        .get(key)
        .and_then(CanonicalValue::as_str)
        .ok_or_else(|| {
            ObservationAnalysisError::InvalidInput(format!("{label} is missing or invalid"))
        })
}

fn require_equal(actual: &str, expected: &str, label: &str) -> Result<()> {
    if actual == expected {
        Ok(())
    } else {
        Err(ObservationAnalysisError::InvalidInput(format!(
            "{label} must be {expected:?}, received {actual:?}"
        )))
    }
}
