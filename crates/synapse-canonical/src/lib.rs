//! Synapse Canonical JSON v0.1 and `sg-oid-v1` identifiers.
//!
//! The parser deliberately does not use a generic JSON value parser. SynapseGit
//! must reject duplicate decoded keys and forbidden number spellings before a
//! normal JSON API can discard that information.

#![forbid(unsafe_code)]

use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::error::Error;
use std::fmt::{self, Write};
use unicode_normalization::UnicodeNormalization;

const PROFILE_DOMAIN: &str = "synapsegit/core/v0.1";
const MAX_SAFE_INTEGER: i64 = 9_007_199_254_740_991;

pub const DEFAULT_MAX_STRUCTURED_BYTES: usize = 16 * 1024 * 1024;
pub const DEFAULT_MAX_NESTING_DEPTH: usize = 128;
pub const HARD_MAX_NESTING_DEPTH: usize = 256;
pub const DEFAULT_MAX_NODES: usize = 100_000;
pub const DEFAULT_MAX_CONTAINER_ITEMS: usize = 50_000;

/// Resource limits for untrusted structured objects.
///
/// Blobs are streamed and limited by the storage layer instead. These limits
/// protect only the recursively parsed and serialized structured JSON domain.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ResourceLimits {
    pub max_input_bytes: usize,
    pub max_nesting_depth: usize,
    pub max_nodes: usize,
    pub max_container_items: usize,
    pub max_canonical_bytes: usize,
}

impl Default for ResourceLimits {
    fn default() -> Self {
        Self {
            max_input_bytes: DEFAULT_MAX_STRUCTURED_BYTES,
            max_nesting_depth: DEFAULT_MAX_NESTING_DEPTH,
            max_nodes: DEFAULT_MAX_NODES,
            max_container_items: DEFAULT_MAX_CONTAINER_ITEMS,
            max_canonical_bytes: DEFAULT_MAX_STRUCTURED_BYTES,
        }
    }
}

/// Stable Stage 0 error codes emitted by the canonical boundary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum ErrorCode {
    InvalidUtf8,
    BomForbidden,
    DuplicateKey,
    NumberTokenForbidden,
    UnsafeInteger,
    LoneSurrogate,
    KeyNotNfc,
    IdentifierNotNfc,
    SetNotSorted,
    SetDuplicate,
    TimestampInvalid,
    IntervalInvalid,
    FixedPointNotNormalized,
    PathSegmentInvalid,
    ResourceLimit,
    SchemaInvalid,
    ReferenceTypeMismatch,
    OidMismatch,
    ClosureMissing,
    AuthorizationDenied,
    HumanGateRequired,
    StaleBase,
    RefConflict,
}

impl ErrorCode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InvalidUtf8 => "invalid_utf8",
            Self::BomForbidden => "bom_forbidden",
            Self::DuplicateKey => "duplicate_key",
            Self::NumberTokenForbidden => "number_token_forbidden",
            Self::UnsafeInteger => "unsafe_integer",
            Self::LoneSurrogate => "lone_surrogate",
            Self::KeyNotNfc => "key_not_nfc",
            Self::IdentifierNotNfc => "identifier_not_nfc",
            Self::SetNotSorted => "set_not_sorted",
            Self::SetDuplicate => "set_duplicate",
            Self::TimestampInvalid => "timestamp_invalid",
            Self::IntervalInvalid => "interval_invalid",
            Self::FixedPointNotNormalized => "fixed_point_not_normalized",
            Self::PathSegmentInvalid => "path_segment_invalid",
            Self::ResourceLimit => "resource_limit",
            Self::SchemaInvalid => "schema_invalid",
            Self::ReferenceTypeMismatch => "reference_type_mismatch",
            Self::OidMismatch => "oid_mismatch",
            Self::ClosureMissing => "closure_missing",
            Self::AuthorizationDenied => "authorization_denied",
            Self::HumanGateRequired => "human_gate_required",
            Self::StaleBase => "stale_base",
            Self::RefConflict => "ref_conflict",
        }
    }
}

impl fmt::Display for ErrorCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// An error whose code is suitable for the protocol boundary.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CoreError {
    code: ErrorCode,
    message: String,
}

impl CoreError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    pub const fn code(&self) -> ErrorCode {
        self.code
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

/// The four content-addressed object families in the Core v0.1 OID profile.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ObjectKind {
    Blob,
    Record,
    Tree,
    Commit,
}

impl ObjectKind {
    pub const fn prefix(self) -> &'static str {
        match self {
            Self::Blob => "blob",
            Self::Record => "record",
            Self::Tree => "tree",
            Self::Commit => "commit",
        }
    }

    pub const fn is_structured(self) -> bool {
        !matches!(self, Self::Blob)
    }
}

/// Validate the complete lexical form of an `sg-oid-v1` identifier.
pub fn parse_oid(oid: &str) -> Result<ObjectKind, CoreError> {
    let mut parts = oid.split(':');
    let kind = match parts.next() {
        Some("blob") => ObjectKind::Blob,
        Some("record") => ObjectKind::Record,
        Some("tree") => ObjectKind::Tree,
        Some("commit") => ObjectKind::Commit,
        _ => return Err(invalid_oid(oid)),
    };
    if parts.next() != Some("sg-oid-v1") || parts.next() != Some("sha256") {
        return Err(invalid_oid(oid));
    }
    let digest = parts.next().ok_or_else(|| invalid_oid(oid))?;
    if parts.next().is_some()
        || digest.len() != 64
        || !digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(invalid_oid(oid));
    }
    Ok(kind)
}

fn invalid_oid(oid: &str) -> CoreError {
    CoreError::new(
        ErrorCode::SchemaInvalid,
        format!("invalid Core v0.1 OID {oid:?}"),
    )
}

impl fmt::Display for CoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.code, self.message)
    }
}

impl Error for CoreError {}

/// Stable category for a canonical timestamp parse failure.
///
/// This mirrors the lexical-versus-calendar distinction used throughout the
/// workspace's canonical timestamp validators. It intentionally omits an
/// out-of-range kind: [`parse_canonical_timestamp_unix_nanos`] only parses
/// text into nanoseconds and never rejects a value for being outside a
/// representable range (that concern belongs to callers that convert
/// nanoseconds back into wire text).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CanonicalTimestampParseErrorKind {
    /// The value is not exactly `YYYY-MM-DDTHH:mm:ss.nnnnnnnnnZ`.
    InvalidLexicalForm,
    /// The value has the right shape but is not a valid proleptic Gregorian
    /// calendar date and time (for example month 13 or February 30th).
    InvalidCalendarDate,
}

/// Error returned when text is not a valid Core v0.1 canonical timestamp.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CanonicalTimestampParseError {
    kind: CanonicalTimestampParseErrorKind,
}

impl CanonicalTimestampParseError {
    const fn new(kind: CanonicalTimestampParseErrorKind) -> Self {
        Self { kind }
    }

    /// Returns the stable category of this parse failure.
    pub const fn kind(self) -> CanonicalTimestampParseErrorKind {
        self.kind
    }
}

impl fmt::Display for CanonicalTimestampParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self.kind {
            CanonicalTimestampParseErrorKind::InvalidLexicalForm => {
                "timestamp must use YYYY-MM-DDTHH:mm:ss.nnnnnnnnnZ"
            }
            CanonicalTimestampParseErrorKind::InvalidCalendarDate => {
                "timestamp is not a valid Gregorian date and time"
            }
        };
        formatter.write_str(message)
    }
}

impl Error for CanonicalTimestampParseError {}

/// Parses a Core v0.1 canonical timestamp into Unix nanoseconds.
///
/// The input must be exactly `YYYY-MM-DDTHH:mm:ss.nnnnnnnnnZ`: a fixed-width
/// ASCII-digit layout with literal `-`, `T`, `:`, `.`, and `Z` separators and
/// exactly nine fractional digits. Beyond the lexical shape, the calendar
/// date and time of day must be valid under the proleptic Gregorian
/// calendar (correct days-per-month, including leap years, and hour/minute/
/// second within their ordinary ranges).
///
/// This is the single source of truth for canonical timestamp parsing in the
/// workspace: `synapse-schema`, `synapse-core`, and `synapse-publication` all
/// delegate to this function rather than re-deriving the lexical check or
/// the civil-calendar arithmetic.
///
/// # Errors
///
/// Returns [`CanonicalTimestampParseError`] with
/// [`CanonicalTimestampParseErrorKind::InvalidLexicalForm`] if `value` does
/// not match the fixed-width wire layout, or
/// [`CanonicalTimestampParseErrorKind::InvalidCalendarDate`] if it matches
/// the layout but is not a valid calendar date and time.
pub fn parse_canonical_timestamp_unix_nanos(
    value: &str,
) -> Result<i128, CanonicalTimestampParseError> {
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
        return Err(CanonicalTimestampParseError::new(
            CanonicalTimestampParseErrorKind::InvalidLexicalForm,
        ));
    }

    let number = |start: usize, end: usize| -> u32 {
        bytes[start..end]
            .iter()
            .fold(0, |number, digit| number * 10 + u32::from(digit - b'0'))
    };
    let year = number(0, 4);
    let month = number(5, 7);
    let day = number(8, 10);
    let hour = number(11, 13);
    let minute = number(14, 16);
    let second = number(17, 19);
    let nanos = number(20, 29);

    if !(1..=12).contains(&month)
        || day == 0
        || day > days_in_civil_month(year, month)
        || hour > 23
        || minute > 59
        || second > 59
    {
        return Err(CanonicalTimestampParseError::new(
            CanonicalTimestampParseErrorKind::InvalidCalendarDate,
        ));
    }

    let day_of_year = (1..month)
        .map(|candidate| i128::from(days_in_civil_month(year, candidate)))
        .sum::<i128>()
        + i128::from(day - 1);
    let absolute_days = days_before_civil_year(year) + day_of_year;
    let unix_days = absolute_days - days_before_civil_year(1970);
    let unix_seconds = unix_days * 86_400
        + i128::from(hour) * 3_600
        + i128::from(minute) * 60
        + i128::from(second);

    Ok(unix_seconds * 1_000_000_000 + i128::from(nanos))
}

const fn is_civil_leap_year(year: u32) -> bool {
    year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400))
}

const fn days_in_civil_month(year: u32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_civil_leap_year(year) => 29,
        2 => 28,
        _ => 0,
    }
}

const fn days_before_civil_year(year: u32) -> i128 {
    if year == 0 {
        return 0;
    }
    let completed = (year - 1) as i128;
    year as i128 * 365 + completed / 4 - completed / 100 + completed / 400 + 1
}

/// The restricted JSON domain accepted at the content-addressed boundary.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Integer(i64),
    String(String),
    Array(Vec<Value>),
    Object(Vec<(String, Value)>),
}

impl Value {
    pub fn get(&self, key: &str) -> Option<&Value> {
        match self {
            Self::Object(entries) => entries
                .iter()
                .find_map(|(candidate, value)| (candidate == key).then_some(value)),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::String(value) => Some(value),
            _ => None,
        }
    }

    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Self::Integer(value) => Some(*value),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&[Value]> {
        match self {
            Self::Array(values) => Some(values),
            _ => None,
        }
    }

    pub fn as_object(&self) -> Option<&[(String, Value)]> {
        match self {
            Self::Object(entries) => Some(entries),
            _ => None,
        }
    }
}

/// Parse bytes according to Synapse Canonical JSON v0.1.
pub fn parse_strict(input: &[u8]) -> Result<Value, CoreError> {
    parse_strict_with_limits(input, ResourceLimits::default())
}

/// Parse bytes with explicit deployment resource limits.
pub fn parse_strict_with_limits(input: &[u8], limits: ResourceLimits) -> Result<Value, CoreError> {
    validate_limits(limits)?;
    if input.len() > limits.max_input_bytes {
        return Err(CoreError::new(
            ErrorCode::ResourceLimit,
            format!(
                "structured input is {} bytes; limit is {}",
                input.len(),
                limits.max_input_bytes
            ),
        ));
    }
    if input.starts_with(&[0xef, 0xbb, 0xbf]) {
        return Err(CoreError::new(
            ErrorCode::BomForbidden,
            "input starts with a UTF-8 BOM",
        ));
    }

    let text = std::str::from_utf8(input)
        .map_err(|_| CoreError::new(ErrorCode::InvalidUtf8, "input is not valid UTF-8"))?;
    if text.starts_with('\u{feff}') {
        return Err(CoreError::new(
            ErrorCode::BomForbidden,
            "input starts with a BOM",
        ));
    }

    Parser::new(text, limits).parse()
}

/// Emit the exact canonical byte sequence used by `sg-oid-v1`.
pub fn canonical_bytes(value: &Value) -> Result<Vec<u8>, CoreError> {
    canonical_bytes_with_limits(value, ResourceLimits::default())
}

/// Emit canonical bytes with explicit deployment resource limits.
pub fn canonical_bytes_with_limits(
    value: &Value,
    limits: ResourceLimits,
) -> Result<Vec<u8>, CoreError> {
    canonical_json_with_limits(value, limits).map(String::into_bytes)
}

/// Emit Synapse Canonical JSON v0.1 as a string.
pub fn canonical_json(value: &Value) -> Result<String, CoreError> {
    canonical_json_with_limits(value, ResourceLimits::default())
}

/// Emit canonical JSON with explicit deployment resource limits.
pub fn canonical_json_with_limits(
    value: &Value,
    limits: ResourceLimits,
) -> Result<String, CoreError> {
    validate_limits(limits)?;
    let mut nodes = 0;
    let size = canonical_size(value, 0, limits, &mut nodes)?;
    let mut output = String::with_capacity(size);
    write_canonical(value, &mut output)?;
    debug_assert_eq!(output.len(), size);
    Ok(output)
}

/// Calculate a domain-separated structured OID without schema validation.
///
/// This low-level primitive validates only the canonical JSON domain and the
/// `object_type` family. Callers MUST complete concrete schema, annotation,
/// semantic, and graph validation before treating the returned OID as an
/// accepted Core object. Production ingestion should expose a validated wrapper
/// from `synapse-schema`, not this primitive directly.
pub fn structured_oid_unchecked(value: &Value) -> Result<String, CoreError> {
    structured_oid_unchecked_with_limits(value, ResourceLimits::default())
}

/// Calculate an unchecked structured OID with explicit resource limits.
///
/// The schema-validation precondition of [`structured_oid_unchecked`] also
/// applies to this function.
pub fn structured_oid_unchecked_with_limits(
    value: &Value,
    limits: ResourceLimits,
) -> Result<String, CoreError> {
    let object_type = value
        .get("object_type")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            CoreError::new(
                ErrorCode::SchemaInvalid,
                "structured object requires a string object_type",
            )
        })?;

    let prefix = match object_type {
        "record" => "record",
        "tree" => "tree",
        "commit" => "commit",
        other => {
            return Err(CoreError::new(
                ErrorCode::SchemaInvalid,
                format!("unsupported object_type {other}"),
            ));
        }
    };

    let body = canonical_bytes_with_limits(value, limits)?;
    let domain = format!("{PROFILE_DOMAIN}\0{object_type}\0{}\0", body.len());
    let mut digest = Sha256::new();
    digest.update(domain.as_bytes());
    digest.update(&body);
    Ok(format!(
        "{prefix}:sg-oid-v1:sha256:{}",
        lower_hex(digest.finalize())
    ))
}

/// Calculate the raw byte OID. No decoding or normalization is performed.
pub fn blob_oid(bytes: &[u8]) -> String {
    format!("blob:sg-oid-v1:sha256:{}", sha256_hex(bytes))
}

/// Verify that transport metadata names the exact supplied raw bytes.
pub fn verify_blob_oid(claimed_oid: &str, bytes: &[u8]) -> Result<(), CoreError> {
    if parse_oid(claimed_oid)? != ObjectKind::Blob {
        return Err(CoreError::new(
            ErrorCode::OidMismatch,
            format!("claimed OID {claimed_oid} is not a Blob OID"),
        ));
    }
    let expected = blob_oid(bytes);
    if claimed_oid != expected {
        return Err(CoreError::new(
            ErrorCode::OidMismatch,
            format!("claimed {claimed_oid}, expected {expected}"),
        ));
    }
    Ok(())
}

/// Hash `bytes` with SHA-256 and return the digest as lowercase hex, no prefix.
pub fn sha256_hex(bytes: &[u8]) -> String {
    lower_hex(Sha256::digest(bytes))
}

/// Encode a byte sequence as lowercase hex, no prefix.
pub fn lower_hex(bytes: impl IntoIterator<Item = u8>) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let bytes = bytes.into_iter();
    let mut output = String::with_capacity(bytes.size_hint().0.saturating_mul(2));
    for byte in bytes {
        output.push(DIGITS[usize::from(byte >> 4)] as char);
        output.push(DIGITS[usize::from(byte & 0x0f)] as char);
    }
    output
}

/// Verify only the digest of a transport-supplied OID.
///
/// As with [`structured_oid_unchecked`], success does not mean the body passed
/// schema or semantic validation.
pub fn verify_claimed_oid_unchecked(claimed_oid: &str, value: &Value) -> Result<(), CoreError> {
    verify_claimed_oid_unchecked_with_limits(claimed_oid, value, ResourceLimits::default())
}

/// Verify only the claimed digest with explicit resource limits.
pub fn verify_claimed_oid_unchecked_with_limits(
    claimed_oid: &str,
    value: &Value,
    limits: ResourceLimits,
) -> Result<(), CoreError> {
    let expected = structured_oid_unchecked_with_limits(value, limits)?;
    if claimed_oid != expected {
        return Err(CoreError::new(
            ErrorCode::OidMismatch,
            format!("claimed {claimed_oid}, expected {expected}"),
        ));
    }
    Ok(())
}

fn canonical_size(
    value: &Value,
    depth: usize,
    limits: ResourceLimits,
    nodes: &mut usize,
) -> Result<usize, CoreError> {
    consume_node(nodes, limits.max_nodes)?;
    let size = match value {
        Value::Null => 4,
        Value::Bool(true) => 4,
        Value::Bool(false) => 5,
        Value::Integer(value) => {
            if !(-MAX_SAFE_INTEGER..=MAX_SAFE_INTEGER).contains(value) {
                return Err(CoreError::new(
                    ErrorCode::NumberTokenForbidden,
                    "canonical integer is outside the safe integer range",
                ));
            }
            value.to_string().len()
        }
        Value::String(value) => canonical_string_size(value, limits.max_canonical_bytes)?,
        Value::Array(values) => {
            let child_depth = checked_child_depth(depth, limits.max_nesting_depth)?;
            check_container_items(values.len(), limits.max_container_items)?;
            let mut total = 2;
            for (index, child) in values.iter().enumerate() {
                if index != 0 {
                    total = checked_size_add(total, 1, limits.max_canonical_bytes)?;
                }
                total = checked_size_add(
                    total,
                    canonical_size(child, child_depth, limits, nodes)?,
                    limits.max_canonical_bytes,
                )?;
            }
            total
        }
        Value::Object(entries) => {
            let child_depth = checked_child_depth(depth, limits.max_nesting_depth)?;
            check_container_items(entries.len(), limits.max_container_items)?;
            let mut seen = HashSet::with_capacity(entries.len());
            let mut total = 2;
            for (index, (key, child)) in entries.iter().enumerate() {
                if !seen.insert(key.as_str()) {
                    return Err(CoreError::new(
                        ErrorCode::DuplicateKey,
                        format!("object repeats key {key:?}"),
                    ));
                }
                require_nfc_key(key)?;
                if index != 0 {
                    total = checked_size_add(total, 1, limits.max_canonical_bytes)?;
                }
                total = checked_size_add(
                    total,
                    canonical_string_size(key, limits.max_canonical_bytes)?,
                    limits.max_canonical_bytes,
                )?;
                total = checked_size_add(total, 1, limits.max_canonical_bytes)?;
                total = checked_size_add(
                    total,
                    canonical_size(child, child_depth, limits, nodes)?,
                    limits.max_canonical_bytes,
                )?;
            }
            total
        }
    };
    ensure_size(size, limits.max_canonical_bytes)
}

fn validate_limits(limits: ResourceLimits) -> Result<(), CoreError> {
    if limits.max_nesting_depth > HARD_MAX_NESTING_DEPTH {
        return Err(CoreError::new(
            ErrorCode::ResourceLimit,
            format!(
                "configured nesting depth {} exceeds hard ceiling {}",
                limits.max_nesting_depth, HARD_MAX_NESTING_DEPTH
            ),
        ));
    }
    Ok(())
}

fn consume_node(nodes: &mut usize, limit: usize) -> Result<(), CoreError> {
    *nodes = nodes.checked_add(1).ok_or_else(|| {
        CoreError::new(ErrorCode::ResourceLimit, "structured node count overflowed")
    })?;
    if *nodes > limit {
        return Err(CoreError::new(
            ErrorCode::ResourceLimit,
            format!("structured node count exceeds limit {limit}"),
        ));
    }
    Ok(())
}

fn check_container_items(items: usize, limit: usize) -> Result<(), CoreError> {
    if items > limit {
        return Err(CoreError::new(
            ErrorCode::ResourceLimit,
            format!("container item count {items} exceeds limit {limit}"),
        ));
    }
    Ok(())
}

fn canonical_string_size(value: &str, limit: usize) -> Result<usize, CoreError> {
    let mut total = 2;
    for character in value.chars() {
        let encoded = match character {
            '"' | '\\' | '\u{08}' | '\t' | '\n' | '\u{0c}' | '\r' => 2,
            '\u{00}'..='\u{1f}' => 6,
            _ => character.len_utf8(),
        };
        total = checked_size_add(total, encoded, limit)?;
    }
    Ok(total)
}

fn checked_child_depth(depth: usize, limit: usize) -> Result<usize, CoreError> {
    let child_depth = depth.checked_add(1).ok_or_else(|| {
        CoreError::new(
            ErrorCode::ResourceLimit,
            "structured nesting depth overflowed",
        )
    })?;
    if child_depth > limit {
        return Err(CoreError::new(
            ErrorCode::ResourceLimit,
            format!("structured nesting depth exceeds limit {limit}"),
        ));
    }
    Ok(child_depth)
}

fn checked_size_add(total: usize, addition: usize, limit: usize) -> Result<usize, CoreError> {
    let size = total.checked_add(addition).ok_or_else(|| {
        CoreError::new(ErrorCode::ResourceLimit, "canonical byte length overflowed")
    })?;
    ensure_size(size, limit)
}

fn ensure_size(size: usize, limit: usize) -> Result<usize, CoreError> {
    if size > limit {
        return Err(CoreError::new(
            ErrorCode::ResourceLimit,
            format!("canonical output is at least {size} bytes; limit is {limit}"),
        ));
    }
    Ok(size)
}

fn write_canonical(value: &Value, output: &mut String) -> Result<(), CoreError> {
    match value {
        Value::Null => output.push_str("null"),
        Value::Bool(true) => output.push_str("true"),
        Value::Bool(false) => output.push_str("false"),
        Value::Integer(value) => {
            if !(-MAX_SAFE_INTEGER..=MAX_SAFE_INTEGER).contains(value) {
                return Err(CoreError::new(
                    ErrorCode::NumberTokenForbidden,
                    "canonical integer is outside the safe integer range",
                ));
            }
            write!(output, "{value}").expect("writing to a String cannot fail");
        }
        Value::String(value) => write_json_string(value, output),
        Value::Array(values) => {
            output.push('[');
            for (index, child) in values.iter().enumerate() {
                if index != 0 {
                    output.push(',');
                }
                write_canonical(child, output)?;
            }
            output.push(']');
        }
        Value::Object(entries) => {
            let mut seen = HashSet::with_capacity(entries.len());
            for (key, _) in entries {
                if !seen.insert(key.as_str()) {
                    return Err(CoreError::new(
                        ErrorCode::DuplicateKey,
                        format!("object repeats key {key:?}"),
                    ));
                }
                require_nfc_key(key)?;
            }

            let mut ordered: Vec<_> = entries.iter().collect();
            ordered.sort_by(|(left, _), (right, _)| utf16_compare(left, right));

            output.push('{');
            for (index, (key, child)) in ordered.into_iter().enumerate() {
                if index != 0 {
                    output.push(',');
                }
                write_json_string(key, output);
                output.push(':');
                write_canonical(child, output)?;
            }
            output.push('}');
        }
    }
    Ok(())
}

fn write_json_string(value: &str, output: &mut String) {
    output.push('"');
    for character in value.chars() {
        match character {
            '"' => output.push_str("\\\""),
            '\\' => output.push_str("\\\\"),
            '\u{08}' => output.push_str("\\b"),
            '\t' => output.push_str("\\t"),
            '\n' => output.push_str("\\n"),
            '\u{0c}' => output.push_str("\\f"),
            '\r' => output.push_str("\\r"),
            '\u{00}'..='\u{1f}' => {
                write!(output, "\\u{:04x}", character as u32)
                    .expect("writing to a String cannot fail");
            }
            _ => output.push(character),
        }
    }
    output.push('"');
}

fn utf16_compare(left: &str, right: &str) -> std::cmp::Ordering {
    left.encode_utf16().cmp(right.encode_utf16())
}

fn require_nfc_key(key: &str) -> Result<(), CoreError> {
    if !key.nfc().eq(key.chars()) {
        return Err(CoreError::new(
            ErrorCode::KeyNotNfc,
            format!("object key {key:?} is not NFC"),
        ));
    }
    Ok(())
}

struct Parser<'a> {
    text: &'a str,
    bytes: &'a [u8],
    position: usize,
    limits: ResourceLimits,
    nodes: usize,
}

impl<'a> Parser<'a> {
    fn new(text: &'a str, limits: ResourceLimits) -> Self {
        Self {
            text,
            bytes: text.as_bytes(),
            position: 0,
            limits,
            nodes: 0,
        }
    }

    fn parse(mut self) -> Result<Value, CoreError> {
        self.skip_whitespace();
        let value = self.parse_value(0)?;
        self.skip_whitespace();
        if self.position != self.bytes.len() {
            return self.syntax("trailing content");
        }
        Ok(value)
    }

    fn parse_value(&mut self, depth: usize) -> Result<Value, CoreError> {
        self.skip_whitespace();
        self.consume_node()?;
        match self.peek() {
            Some(b'{') => {
                let child_depth = self.child_depth(depth)?;
                self.parse_object(child_depth)
            }
            Some(b'[') => {
                let child_depth = self.child_depth(depth)?;
                self.parse_array(child_depth)
            }
            Some(b'"') => self.parse_string().map(Value::String),
            Some(b't') => self.parse_keyword(b"true", Value::Bool(true)),
            Some(b'f') => self.parse_keyword(b"false", Value::Bool(false)),
            Some(b'n') => self.parse_keyword(b"null", Value::Null),
            Some(b'-' | b'0'..=b'9') => self.parse_number(),
            _ => self.syntax("expected JSON value"),
        }
    }

    fn parse_object(&mut self, depth: usize) -> Result<Value, CoreError> {
        self.position += 1;
        self.skip_whitespace();
        let mut entries = Vec::new();
        let mut keys = HashSet::new();
        if self.peek() == Some(b'}') {
            self.position += 1;
            return Ok(Value::Object(entries));
        }

        loop {
            if entries.len() >= self.limits.max_container_items {
                return Err(CoreError::new(
                    ErrorCode::ResourceLimit,
                    format!(
                        "container item count exceeds limit {}",
                        self.limits.max_container_items
                    ),
                ));
            }
            if self.peek() != Some(b'"') {
                return self.syntax("object key must be a string");
            }
            let key = self.parse_string()?;
            if !keys.insert(key.clone()) {
                return Err(CoreError::new(
                    ErrorCode::DuplicateKey,
                    format!("input repeats key {key:?}"),
                ));
            }
            require_nfc_key(&key)?;

            self.skip_whitespace();
            if self.peek() != Some(b':') {
                return self.syntax("expected ':'");
            }
            self.position += 1;
            self.skip_whitespace();
            entries.push((key, self.parse_value(depth)?));
            self.skip_whitespace();

            match self.peek() {
                Some(b'}') => {
                    self.position += 1;
                    return Ok(Value::Object(entries));
                }
                Some(b',') => {
                    self.position += 1;
                    self.skip_whitespace();
                }
                _ => return self.syntax("expected ',' or '}'"),
            }
        }
    }

    fn parse_array(&mut self, depth: usize) -> Result<Value, CoreError> {
        self.position += 1;
        self.skip_whitespace();
        let mut values = Vec::new();
        if self.peek() == Some(b']') {
            self.position += 1;
            return Ok(Value::Array(values));
        }

        loop {
            if values.len() >= self.limits.max_container_items {
                return Err(CoreError::new(
                    ErrorCode::ResourceLimit,
                    format!(
                        "container item count exceeds limit {}",
                        self.limits.max_container_items
                    ),
                ));
            }
            values.push(self.parse_value(depth)?);
            self.skip_whitespace();
            match self.peek() {
                Some(b']') => {
                    self.position += 1;
                    return Ok(Value::Array(values));
                }
                Some(b',') => {
                    self.position += 1;
                    self.skip_whitespace();
                }
                _ => return self.syntax("expected ',' or ']'"),
            }
        }
    }

    fn parse_string(&mut self) -> Result<String, CoreError> {
        debug_assert_eq!(self.peek(), Some(b'"'));
        self.position += 1;
        let mut value = String::new();

        while let Some(byte) = self.peek() {
            match byte {
                b'"' => {
                    self.position += 1;
                    return Ok(value);
                }
                b'\\' => {
                    self.position += 1;
                    self.parse_escape(&mut value)?;
                }
                0x00..=0x1f => return self.syntax("unescaped control character in string"),
                0x20..=0x7f => {
                    value.push(char::from(byte));
                    self.position += 1;
                }
                _ => {
                    let character = self.text[self.position..]
                        .chars()
                        .next()
                        .expect("validated UTF-8 has a character at this byte offset");
                    value.push(character);
                    self.position += character.len_utf8();
                }
            }
        }

        self.syntax("unterminated string")
    }

    fn parse_escape(&mut self, output: &mut String) -> Result<(), CoreError> {
        let escaped = self
            .peek()
            .ok_or_else(|| CoreError::new(ErrorCode::SchemaInvalid, "unterminated escape"))?;
        self.position += 1;
        match escaped {
            b'"' => output.push('"'),
            b'\\' => output.push('\\'),
            b'/' => output.push('/'),
            b'b' => output.push('\u{08}'),
            b'f' => output.push('\u{0c}'),
            b'n' => output.push('\n'),
            b'r' => output.push('\r'),
            b't' => output.push('\t'),
            b'u' => self.parse_unicode_escape(output)?,
            _ => return self.syntax("invalid JSON escape"),
        }
        Ok(())
    }

    fn parse_unicode_escape(&mut self, output: &mut String) -> Result<(), CoreError> {
        let first = self.parse_hex_u16()?;
        let scalar = if (0xd800..=0xdbff).contains(&first) {
            if self.bytes.get(self.position..self.position + 2) != Some(b"\\u") {
                return Err(CoreError::new(
                    ErrorCode::LoneSurrogate,
                    "string contains an unpaired high surrogate",
                ));
            }
            self.position += 2;
            let second = self.parse_hex_u16()?;
            if !(0xdc00..=0xdfff).contains(&second) {
                return Err(CoreError::new(
                    ErrorCode::LoneSurrogate,
                    "string contains an unpaired high surrogate",
                ));
            }
            0x1_0000 + (((u32::from(first) - 0xd800) << 10) | (u32::from(second) - 0xdc00))
        } else if (0xdc00..=0xdfff).contains(&first) {
            return Err(CoreError::new(
                ErrorCode::LoneSurrogate,
                "string contains an unpaired low surrogate",
            ));
        } else {
            u32::from(first)
        };

        output.push(char::from_u32(scalar).expect("validated surrogate pair forms a scalar"));
        Ok(())
    }

    fn parse_hex_u16(&mut self) -> Result<u16, CoreError> {
        let digits = self
            .bytes
            .get(self.position..self.position + 4)
            .ok_or_else(|| CoreError::new(ErrorCode::SchemaInvalid, "invalid Unicode escape"))?;
        if !digits.iter().all(u8::is_ascii_hexdigit) {
            return self.syntax("invalid Unicode escape");
        }
        self.position += 4;
        let token = std::str::from_utf8(digits).expect("ASCII hex is valid UTF-8");
        Ok(u16::from_str_radix(token, 16).expect("four checked hex digits fit u16"))
    }

    fn parse_number(&mut self) -> Result<Value, CoreError> {
        let start = self.position;
        while let Some(byte) = self.peek() {
            if is_whitespace(byte) || matches!(byte, b',' | b']' | b'}') {
                break;
            }
            self.position += 1;
        }
        let token = &self.text[start..self.position];
        if !valid_integer_token(token) || token == "-0" {
            return Err(CoreError::new(
                ErrorCode::NumberTokenForbidden,
                format!("input contains forbidden number token {token}"),
            ));
        }
        let value = token.parse::<i64>().map_err(|_| {
            CoreError::new(
                ErrorCode::UnsafeInteger,
                format!("input contains unsafe integer {token}"),
            )
        })?;
        if !(-MAX_SAFE_INTEGER..=MAX_SAFE_INTEGER).contains(&value) {
            return Err(CoreError::new(
                ErrorCode::UnsafeInteger,
                format!("input contains unsafe integer {token}"),
            ));
        }
        Ok(Value::Integer(value))
    }

    fn parse_keyword(&mut self, keyword: &[u8], value: Value) -> Result<Value, CoreError> {
        if !self.bytes[self.position..].starts_with(keyword) {
            return self.syntax("invalid JSON keyword");
        }
        self.position += keyword.len();
        Ok(value)
    }

    fn skip_whitespace(&mut self) {
        while self.peek().is_some_and(is_whitespace) {
            self.position += 1;
        }
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.position).copied()
    }

    fn child_depth(&self, depth: usize) -> Result<usize, CoreError> {
        checked_child_depth(depth, self.limits.max_nesting_depth)
    }

    fn consume_node(&mut self) -> Result<(), CoreError> {
        consume_node(&mut self.nodes, self.limits.max_nodes)
    }

    fn syntax<T>(&self, message: &str) -> Result<T, CoreError> {
        Err(CoreError::new(
            ErrorCode::SchemaInvalid,
            format!("input at byte {}: {message}", self.position),
        ))
    }
}

fn is_whitespace(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | b'\r' | b'\n')
}

fn valid_integer_token(token: &str) -> bool {
    let digits = token.strip_prefix('-').unwrap_or(token);
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return false;
    }
    digits == "0" || !digits.starts_with('0')
}

// -- Portable path syntax -------------------------------------------------
//
// Pure string predicates for the workspace's shared "portable path" syntax
// floor: no filesystem I/O, no symlink handling, no OS `Path`/`Component`
// types.
//
// [`is_portable_segment`] is the floor every segment-level site agrees on.
// It has three direct callers: `synapse-cas` and `synapse-schema` call it
// as their whole check; `synapse-artifact::checkout` composes it with its
// own additional local checks. It deliberately does NOT reject a bare `\`
// byte: `synapse-cas` and `synapse-schema` validate one key of a
// content-addressed structured object, not an OS-style file path, and have
// never rejected a literal backslash character in that position — only
// `/`-splitting boundary characters (`/`, NUL, `.`, `..`) are universal
// across every segment-level site. Callers that also need to reject
// backslash (because they are validating something that will eventually
// reach a filesystem, where a literal `\` is a Windows path separator)
// check it explicitly; [`is_portable_relative_path`] does, and additionally
// re-derives (rather than calls) the same empty/`.`/`..` per-segment rules
// inline so it can report each one as its own [`PortablePathErrorKind`]
// variant — [`is_portable_segment`] only returns a `bool`. The two are kept
// in exact agreement by a dedicated cross-check unit test.
//
// `synapse-artifact`, `synapse-publication`, `synapse-cas`, and
// `synapse-schema` all delegate the syntax slice of their path/segment
// validation here rather than re-deriving the dot-segment and NUL-byte
// checks independently. Each caller keeps any additional domain-specific
// checks (resource limits, NFC normalization, Windows reserved device
// names, a fixed ASCII character allowlist, a broader ASCII-control-byte
// sweep, and so on) layered on top, since those are not part of the common
// syntax floor every site agrees on.

/// Stable category for a portable path syntax rejection.
///
/// Every variant here corresponds to exactly one syntax check shared by at
/// least two workspace crates (see the crate-level module comment). This
/// type intentionally does not cover domain-specific checks such as NFC
/// normalization, Windows reserved device names, or a caller's fixed
/// character allowlist — those remain in each consumer.
///
/// Kept exhaustive per the pre-1.0 policy in `CONTRIBUTING.md`: public Rust
/// error enums do not gain a new `#[non_exhaustive]` before 1.0, so that
/// cross-crate match exhaustiveness checking still catches a missed variant
/// at compile time. Only `synapse-canonical::ErrorCode` and
/// `synapse-publication::GenericArtifactPublicationError` are grandfathered
/// wire-extensibility exceptions; this type is not one of them.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PortablePathErrorKind {
    /// The path or segment is empty.
    Empty,
    /// The path has a leading `/` (an absolute POSIX-style path).
    LeadingSlash,
    /// The path contains a `\` byte anywhere.
    Backslash,
    /// The path starts with an ASCII drive-letter prefix (`[A-Za-z]:`, for
    /// example `C:`), a Windows-style absolute path form.
    WindowsDriveLetter,
    /// A `/`-separated segment is exactly `.`.
    CurrentDirSegment,
    /// A `/`-separated segment is exactly `..`.
    ParentDirSegment,
    /// A `/`-separated segment is empty (for example from `//` or a
    /// trailing `/`).
    EmptySegment,
    /// The path contains a NUL (`\0`) byte.
    NulByte,
}

/// Error returned when a path or segment fails the shared portable-path
/// syntax floor.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PortablePathError {
    kind: PortablePathErrorKind,
}

impl PortablePathError {
    const fn new(kind: PortablePathErrorKind) -> Self {
        Self { kind }
    }

    /// Returns the stable category of this rejection.
    pub const fn kind(self) -> PortablePathErrorKind {
        self.kind
    }
}

impl fmt::Display for PortablePathError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self.kind {
            PortablePathErrorKind::Empty => "path is empty",
            PortablePathErrorKind::LeadingSlash => "path starts with '/'",
            PortablePathErrorKind::Backslash => "path contains '\\'",
            PortablePathErrorKind::WindowsDriveLetter => {
                "path starts with a Windows drive-letter prefix"
            }
            PortablePathErrorKind::CurrentDirSegment => "path contains a '.' segment",
            PortablePathErrorKind::ParentDirSegment => "path contains a '..' segment",
            PortablePathErrorKind::EmptySegment => "path contains an empty segment",
            PortablePathErrorKind::NulByte => "path contains a NUL byte",
        };
        formatter.write_str(message)
    }
}

impl Error for PortablePathError {}

/// Returns `true` if `segment` is a syntactically safe single path segment.
///
/// A safe segment is non-empty, is not `.` or `..`, and contains no `/` or
/// NUL byte. This is the per-segment building block for callers that
/// validate one path component at a time (for example a content-addressed
/// Tree entry key) rather than a whole slash-joined path.
/// [`is_portable_relative_path`] enforces the same per-segment rules on
/// each `/`-split piece of a whole path, but re-derives them inline (rather
/// than calling this function) so it can report which specific rule failed
/// as a [`PortablePathErrorKind`] variant; a dedicated unit test keeps the
/// two in exact agreement.
///
/// This function does not reject a bare `\` byte — see the module-level
/// documentation for why that is not part of the universal segment floor —
/// nor does it check NFC normalization, Windows reserved device names,
/// reserved characters, or any caller-specific character allowlist.
/// Callers that need those layer them on top.
pub fn is_portable_segment(segment: &str) -> bool {
    !segment.is_empty() && segment != "." && segment != ".." && !segment.contains(['/', '\0'])
}

/// Returns `true` if `path` starts with an ASCII drive-letter prefix
/// (`[A-Za-z]:`, for example `C:` or `c:`), a Windows-style absolute path
/// form that is unsafe to treat as a portable relative path.
pub fn has_windows_drive_letter_prefix(path: &str) -> bool {
    let bytes = path.as_bytes();
    bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':'
}

/// Validates the shared portable-path syntax floor for a whole `/`-joined
/// relative path.
///
/// Rejects: an empty path, a leading `/`, any `\` byte anywhere in the
/// path, an ASCII drive-letter prefix (see
/// [`has_windows_drive_letter_prefix`]), any NUL byte, and any `/`-split
/// segment that is empty, `.`, or `..` (via [`is_portable_segment`]).
///
/// Unlike [`is_portable_segment`], this whole-path validator does reject a
/// bare `\` byte: every workspace site that validates a whole slash-joined
/// relative path (as opposed to one already-split segment) is guarding
/// something that can reach a filesystem, where `\` is a meaningful
/// Windows path separator.
///
/// This function does not check a trailing `/` as an independent rule — a
/// trailing `/` always produces an empty final segment once split on `/`,
/// which the segment check already rejects — nor NFC normalization,
/// Windows reserved device names, reserved characters beyond `\` and NUL,
/// or any caller-specific character allowlist. Callers that need those (for
/// example `synapse-artifact`'s `relative-nfc-portable-v1` profile) perform
/// them in addition to this call, not instead of it.
///
/// # Errors
///
/// Returns [`PortablePathError`] describing the first syntax rule that
/// `path` violates.
pub fn is_portable_relative_path(path: &str) -> Result<(), PortablePathError> {
    if path.is_empty() {
        return Err(PortablePathError::new(PortablePathErrorKind::Empty));
    }
    if path.starts_with('/') {
        return Err(PortablePathError::new(PortablePathErrorKind::LeadingSlash));
    }
    if path.contains('\\') {
        return Err(PortablePathError::new(PortablePathErrorKind::Backslash));
    }
    if has_windows_drive_letter_prefix(path) {
        return Err(PortablePathError::new(
            PortablePathErrorKind::WindowsDriveLetter,
        ));
    }
    if path.contains('\0') {
        return Err(PortablePathError::new(PortablePathErrorKind::NulByte));
    }
    for segment in path.split('/') {
        if segment.is_empty() {
            return Err(PortablePathError::new(PortablePathErrorKind::EmptySegment));
        }
        if segment == "." {
            return Err(PortablePathError::new(
                PortablePathErrorKind::CurrentDirSegment,
            ));
        }
        if segment == ".." {
            return Err(PortablePathError::new(
                PortablePathErrorKind::ParentDirSegment,
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod unit_tests {
    use super::*;

    fn error_code(input: &[u8]) -> ErrorCode {
        parse_strict(input).expect_err("input should fail").code()
    }

    #[test]
    fn stable_error_codes_match_the_operations_profile() {
        let codes = [
            (ErrorCode::InvalidUtf8, "invalid_utf8"),
            (ErrorCode::BomForbidden, "bom_forbidden"),
            (ErrorCode::DuplicateKey, "duplicate_key"),
            (ErrorCode::NumberTokenForbidden, "number_token_forbidden"),
            (ErrorCode::UnsafeInteger, "unsafe_integer"),
            (ErrorCode::LoneSurrogate, "lone_surrogate"),
            (ErrorCode::KeyNotNfc, "key_not_nfc"),
            (ErrorCode::IdentifierNotNfc, "identifier_not_nfc"),
            (ErrorCode::SetNotSorted, "set_not_sorted"),
            (ErrorCode::SetDuplicate, "set_duplicate"),
            (ErrorCode::TimestampInvalid, "timestamp_invalid"),
            (ErrorCode::IntervalInvalid, "interval_invalid"),
            (
                ErrorCode::FixedPointNotNormalized,
                "fixed_point_not_normalized",
            ),
            (ErrorCode::PathSegmentInvalid, "path_segment_invalid"),
            (ErrorCode::SchemaInvalid, "schema_invalid"),
            (ErrorCode::ReferenceTypeMismatch, "reference_type_mismatch"),
            (ErrorCode::OidMismatch, "oid_mismatch"),
            (ErrorCode::ClosureMissing, "closure_missing"),
            (ErrorCode::AuthorizationDenied, "authorization_denied"),
            (ErrorCode::HumanGateRequired, "human_gate_required"),
            (ErrorCode::StaleBase, "stale_base"),
            (ErrorCode::RefConflict, "ref_conflict"),
            (ErrorCode::ResourceLimit, "resource_limit"),
        ];
        for (code, expected) in codes {
            assert_eq!(code.as_str(), expected);
            assert_eq!(code.to_string(), expected);
        }
    }

    #[test]
    fn strict_parser_preserves_protocol_errors() {
        assert_eq!(
            error_code(&[b'"', 0xc3, 0x28, b'"']),
            ErrorCode::InvalidUtf8
        );
        assert_eq!(error_code(b"\xef\xbb\xbf{}"), ErrorCode::BomForbidden);
        assert_eq!(
            error_code(br#"{"a":1,"\u0061":2}"#),
            ErrorCode::DuplicateKey
        );
        assert_eq!(error_code(br#"{"a":1.0}"#), ErrorCode::NumberTokenForbidden);
        assert_eq!(error_code(br#"{"a":1e0}"#), ErrorCode::NumberTokenForbidden);
        assert_eq!(error_code(br#"{"a":-0}"#), ErrorCode::NumberTokenForbidden);
        assert_eq!(
            error_code(br#"{"a":9007199254740992}"#),
            ErrorCode::UnsafeInteger
        );
        assert_eq!(error_code(br#"{"a":"\ud800"}"#), ErrorCode::LoneSurrogate);
        assert_eq!(error_code(br#"{"a":"\udc00"}"#), ErrorCode::LoneSurrogate);
        assert_eq!(error_code(br#"{"e\u0301":1}"#), ErrorCode::KeyNotNfc);
    }

    #[test]
    fn integer_and_surrogate_boundaries_are_lossless() {
        assert!(parse_strict(b"9007199254740991").is_ok());
        assert!(parse_strict(b"-9007199254740991").is_ok());
        assert_eq!(error_code(b"9007199254740992"), ErrorCode::UnsafeInteger);
        assert_eq!(error_code(b"-9007199254740992"), ErrorCode::UnsafeInteger);
        assert_eq!(error_code(b"01"), ErrorCode::NumberTokenForbidden);

        let escaped = parse_strict(br#""\ud800\udc00""#).unwrap();
        assert_eq!(escaped, Value::String("\u{10000}".to_owned()));
        assert_eq!(canonical_json(&escaped).unwrap(), "\"\u{10000}\"");
    }

    #[test]
    fn canonical_strings_match_json_stringify_rules() {
        let value = Value::String("\0\u{8}\t\n\u{c}\r\"\\/é\u{2028}".to_owned());
        assert_eq!(
            canonical_json(&value).unwrap(),
            "\"\\u0000\\b\\t\\n\\f\\r\\\"\\\\/é\u{2028}\""
        );
    }

    #[test]
    fn object_keys_use_utf16_code_unit_order() {
        let value = Value::Object(vec![
            ("\u{e000}".to_owned(), Value::Null),
            ("\u{10000}".to_owned(), Value::Null),
            ("2".to_owned(), Value::Null),
            ("10".to_owned(), Value::Null),
        ]);
        assert_eq!(
            canonical_json(&value).unwrap(),
            "{\"10\":null,\"2\":null,\"𐀀\":null,\"\":null}"
        );
    }

    #[test]
    fn blob_hashing_preserves_every_input_byte() {
        assert_ne!(blob_oid(b"line\n"), blob_oid(b"line\r\n"));
        assert_ne!(blob_oid(b"data"), blob_oid(b"\xef\xbb\xbfdata"));
        assert_ne!(blob_oid(b"a\0b"), blob_oid(b"ab"));

        let oid = blob_oid(b"data");
        verify_blob_oid(&oid, b"data").unwrap();
        assert_eq!(
            verify_blob_oid(&oid, b"other").unwrap_err().code(),
            ErrorCode::OidMismatch
        );
    }

    #[test]
    fn oid_parser_rejects_noncanonical_or_unsafe_spellings() {
        let digest = "a".repeat(64);
        assert_eq!(
            parse_oid(&format!("commit:sg-oid-v1:sha256:{digest}")).unwrap(),
            ObjectKind::Commit
        );
        for invalid in [
            format!("other:sg-oid-v1:sha256:{digest}"),
            format!("blob:sg-oid-v2:sha256:{digest}"),
            format!("blob:sg-oid-v1:sha1:{digest}"),
            format!("blob:sg-oid-v1:sha256:{}", "A".repeat(64)),
            format!("blob:sg-oid-v1:sha256:{}", "a".repeat(63)),
            format!("blob:sg-oid-v1:sha256:{digest}:extra"),
        ] {
            assert_eq!(
                parse_oid(&invalid).unwrap_err().code(),
                ErrorCode::SchemaInvalid
            );
        }
    }

    #[test]
    fn structured_resource_limits_fail_before_unbounded_recursion_or_output() {
        let one_level = ResourceLimits {
            max_input_bytes: 64,
            max_nesting_depth: 1,
            max_canonical_bytes: 64,
            ..ResourceLimits::default()
        };
        assert!(parse_strict_with_limits(b"[0]", one_level).is_ok());
        assert_eq!(
            parse_strict_with_limits(b"[[0]]", one_level)
                .unwrap_err()
                .code(),
            ErrorCode::ResourceLimit
        );

        let nested = Value::Array(vec![Value::Array(vec![Value::Integer(0)])]);
        assert_eq!(
            canonical_json_with_limits(&nested, one_level)
                .unwrap_err()
                .code(),
            ErrorCode::ResourceLimit
        );

        let exact_input = ResourceLimits {
            max_input_bytes: 4,
            ..ResourceLimits::default()
        };
        assert!(parse_strict_with_limits(b"null", exact_input).is_ok());
        assert_eq!(
            parse_strict_with_limits(b" null", exact_input)
                .unwrap_err()
                .code(),
            ErrorCode::ResourceLimit
        );

        let exact_output = ResourceLimits {
            max_canonical_bytes: 5,
            ..ResourceLimits::default()
        };
        let text = Value::String("abc".to_owned());
        assert!(canonical_json_with_limits(&text, exact_output).is_ok());
        let tiny_output = ResourceLimits {
            max_canonical_bytes: 4,
            ..ResourceLimits::default()
        };
        assert_eq!(
            canonical_json_with_limits(&text, tiny_output)
                .unwrap_err()
                .code(),
            ErrorCode::ResourceLimit
        );

        let two_nodes = ResourceLimits {
            max_nodes: 2,
            ..ResourceLimits::default()
        };
        assert!(parse_strict_with_limits(b"[0]", two_nodes).is_ok());
        assert_eq!(
            parse_strict_with_limits(b"[0,1]", two_nodes)
                .unwrap_err()
                .code(),
            ErrorCode::ResourceLimit
        );
        let two_values = Value::Array(vec![Value::Integer(0), Value::Integer(1)]);
        assert_eq!(
            canonical_json_with_limits(&two_values, two_nodes)
                .unwrap_err()
                .code(),
            ErrorCode::ResourceLimit
        );

        let one_item = ResourceLimits {
            max_container_items: 1,
            ..ResourceLimits::default()
        };
        assert!(parse_strict_with_limits(b"[0]", one_item).is_ok());
        assert_eq!(
            parse_strict_with_limits(b"[0,1]", one_item)
                .unwrap_err()
                .code(),
            ErrorCode::ResourceLimit
        );
        assert_eq!(
            canonical_json_with_limits(&two_values, one_item)
                .unwrap_err()
                .code(),
            ErrorCode::ResourceLimit
        );

        let hard_depth = ResourceLimits {
            max_nesting_depth: HARD_MAX_NESTING_DEPTH,
            ..ResourceLimits::default()
        };
        assert!(parse_strict_with_limits(b"null", hard_depth).is_ok());
        let unsafe_depth = ResourceLimits {
            max_nesting_depth: HARD_MAX_NESTING_DEPTH + 1,
            ..ResourceLimits::default()
        };
        assert_eq!(
            parse_strict_with_limits(b"null", unsafe_depth)
                .unwrap_err()
                .code(),
            ErrorCode::ResourceLimit
        );
    }

    #[test]
    fn custom_limits_flow_through_unchecked_oid_and_digest_verification() {
        let value = Value::Object(vec![(
            "object_type".to_owned(),
            Value::String("record".to_owned()),
        )]);
        let too_small = ResourceLimits {
            max_canonical_bytes: 8,
            ..ResourceLimits::default()
        };
        assert_eq!(
            structured_oid_unchecked_with_limits(&value, too_small)
                .unwrap_err()
                .code(),
            ErrorCode::ResourceLimit
        );

        let accepted = ResourceLimits {
            max_canonical_bytes: 64,
            ..ResourceLimits::default()
        };
        let oid = structured_oid_unchecked_with_limits(&value, accepted).unwrap();
        verify_claimed_oid_unchecked_with_limits(&oid, &value, accepted).unwrap();
    }

    // -- Canonical timestamp parsing --------------------------------------
    //
    // These cover `parse_canonical_timestamp_unix_nanos`, the single source
    // of truth for canonical timestamp parsing shared by `synapse-schema`,
    // `synapse-core`, and `synapse-publication`.

    #[test]
    fn canonical_timestamp_accepts_the_unix_epoch_and_its_neighbors() {
        assert_eq!(
            parse_canonical_timestamp_unix_nanos("1970-01-01T00:00:00.000000000Z").unwrap(),
            0
        );
        assert_eq!(
            parse_canonical_timestamp_unix_nanos("1969-12-31T23:59:59.000000000Z").unwrap(),
            -1_000_000_000
        );
        assert_eq!(
            parse_canonical_timestamp_unix_nanos("1970-01-01T00:00:00.000000001Z").unwrap(),
            1
        );
        assert_eq!(
            parse_canonical_timestamp_unix_nanos("1970-01-01T00:00:01.000000000Z").unwrap(),
            1_000_000_000
        );
    }

    #[test]
    fn canonical_timestamp_preserves_full_nanosecond_precision() {
        assert_eq!(
            parse_canonical_timestamp_unix_nanos("2000-02-29T12:34:56.123456789Z").unwrap(),
            951_827_696_123_456_789
        );
        assert_eq!(
            parse_canonical_timestamp_unix_nanos("1970-01-01T00:00:00.999999999Z").unwrap(),
            999_999_999
        );
    }

    #[test]
    fn canonical_timestamp_validates_leap_years_under_the_gregorian_400_year_rule() {
        // Divisible by 4 and 400: a leap year, so February 29th exists.
        assert!(parse_canonical_timestamp_unix_nanos("2000-02-29T00:00:00.000000000Z").is_ok());
        // Divisible by 4 and 100 but not 400: not a leap year.
        assert_eq!(
            parse_canonical_timestamp_unix_nanos("1900-02-29T00:00:00.000000000Z")
                .unwrap_err()
                .kind(),
            CanonicalTimestampParseErrorKind::InvalidCalendarDate
        );
        // Ordinary leap year (divisible by 4, not by 100).
        assert!(parse_canonical_timestamp_unix_nanos("2004-02-29T00:00:00.000000000Z").is_ok());
        // Ordinary non-leap year.
        assert_eq!(
            parse_canonical_timestamp_unix_nanos("2001-02-29T00:00:00.000000000Z")
                .unwrap_err()
                .kind(),
            CanonicalTimestampParseErrorKind::InvalidCalendarDate
        );
        // The leap day and the following day must be exactly one day apart.
        let leap_day =
            parse_canonical_timestamp_unix_nanos("2000-02-29T00:00:00.000000000Z").unwrap();
        let next_day =
            parse_canonical_timestamp_unix_nanos("2000-03-01T00:00:00.000000000Z").unwrap();
        assert_eq!(next_day - leap_day, 86_400 * 1_000_000_000);
    }

    #[test]
    fn canonical_timestamp_rejects_calendar_invalid_month_day_and_time_fields() {
        for invalid in [
            "2025-00-01T00:00:00.000000000Z",
            "2025-13-01T00:00:00.000000000Z",
            "2025-01-00T00:00:00.000000000Z",
            "2025-01-32T00:00:00.000000000Z",
            "2025-04-31T00:00:00.000000000Z",
            "2025-02-30T00:00:00.000000000Z",
            "2025-01-01T24:00:00.000000000Z",
            "2025-01-01T00:60:00.000000000Z",
            "2025-01-01T00:00:60.000000000Z",
        ] {
            assert_eq!(
                parse_canonical_timestamp_unix_nanos(invalid)
                    .unwrap_err()
                    .kind(),
                CanonicalTimestampParseErrorKind::InvalidCalendarDate,
                "expected calendar rejection for {invalid:?}"
            );
        }
    }

    #[test]
    fn canonical_timestamp_rejects_malformed_lexical_shapes() {
        for invalid in [
            "",
            "1970-01-01T00:00:00.000000000",
            "1970-01-01T00:00:00.00000000Z",
            "1970-01-01T00:00:00.0000000000Z",
            "1970/01-01T00:00:00.000000000Z",
            "1970-01-01 00:00:00.000000000Z",
            "1970-01-01T00-00:00.000000000Z",
            "1970-01-01T00:00-00.000000000Z",
            "1970-01-01T00:00:00,000000000Z",
            "1970-01-01T00:00:00.000000000z",
            "197a-01-01T00:00:00.000000000Z",
            "1970-01-01T00:00:00.00000000aZ",
        ] {
            assert_eq!(
                parse_canonical_timestamp_unix_nanos(invalid)
                    .unwrap_err()
                    .kind(),
                CanonicalTimestampParseErrorKind::InvalidLexicalForm,
                "expected lexical rejection for {invalid:?}"
            );
        }
    }

    #[test]
    fn canonical_timestamp_error_kind_display_text_is_stable() {
        assert_eq!(
            CanonicalTimestampParseError::new(CanonicalTimestampParseErrorKind::InvalidLexicalForm)
                .to_string(),
            "timestamp must use YYYY-MM-DDTHH:mm:ss.nnnnnnnnnZ"
        );
        assert_eq!(
            CanonicalTimestampParseError::new(
                CanonicalTimestampParseErrorKind::InvalidCalendarDate
            )
            .to_string(),
            "timestamp is not a valid Gregorian date and time"
        );
    }

    // -- Portable path syntax ----------------------------------------------
    //
    // These cover `is_portable_segment`, `has_windows_drive_letter_prefix`,
    // and `is_portable_relative_path`, the shared portable-path syntax floor
    // used by `synapse-artifact`, `synapse-publication`, `synapse-cas`, and
    // `synapse-schema`.

    #[test]
    fn portable_segment_accepts_ordinary_single_component_names() {
        for segment in ["a", "file.txt", "assets", "..hidden", "...", "a.b.c", "😀"] {
            assert!(is_portable_segment(segment), "segment={segment:?}");
        }
    }

    #[test]
    fn portable_segment_rejects_empty_dot_dotdot_slash_and_nul() {
        for segment in ["", ".", "..", "a/b", "a\0b", "/", "\0"] {
            assert!(!is_portable_segment(segment), "segment={segment:?}");
        }
    }

    #[test]
    fn portable_segment_deliberately_does_not_reject_a_bare_backslash() {
        // `synapse-cas` and `synapse-schema` validate one key of a
        // content-addressed structured object, not an OS-style file path,
        // and have never rejected a literal '\' character in that
        // position. This is the exact pre-existing behavior for both
        // sites (see `valid_path_segment` in synapse-cas::graph and the
        // inline check in synapse-schema::validate_manifest) — not a gap.
        for segment in ["a\\b", "\\", "assets\\secret"] {
            assert!(is_portable_segment(segment), "segment={segment:?}");
        }
    }

    #[test]
    fn drive_letter_prefix_matches_the_synapse_artifact_precedent() {
        for path in ["C:/secret", "c:/secret", "Z:\\x", "a:", "a:b"] {
            assert!(has_windows_drive_letter_prefix(path), "path={path:?}");
        }
        for path in ["", "a", "/a:b", "1:", ":a", "assets/a:b", "ab:"] {
            assert!(!has_windows_drive_letter_prefix(path), "path={path:?}");
        }
    }

    #[test]
    fn portable_relative_path_accepts_ordinary_nested_paths() {
        for path in ["a", "a/b", "a/b/c", "assets/site.css", "a.b/c..d/e"] {
            assert!(is_portable_relative_path(path).is_ok(), "path={path:?}");
        }
    }

    #[test]
    fn portable_relative_path_reports_the_first_violated_rule() {
        let cases = [
            ("", PortablePathErrorKind::Empty),
            ("/etc/passwd", PortablePathErrorKind::LeadingSlash),
            ("assets\\secret", PortablePathErrorKind::Backslash),
            ("a/b\\c", PortablePathErrorKind::Backslash),
            ("C:/secret", PortablePathErrorKind::WindowsDriveLetter),
            ("c:\\windows", PortablePathErrorKind::Backslash),
            ("a\0b", PortablePathErrorKind::NulByte),
            ("a//b", PortablePathErrorKind::EmptySegment),
            ("a/", PortablePathErrorKind::EmptySegment),
            ("a/./b", PortablePathErrorKind::CurrentDirSegment),
            (".", PortablePathErrorKind::CurrentDirSegment),
            ("a/../b", PortablePathErrorKind::ParentDirSegment),
            ("..", PortablePathErrorKind::ParentDirSegment),
        ];
        for (path, expected) in cases {
            assert_eq!(
                is_portable_relative_path(path).unwrap_err().kind(),
                expected,
                "path={path:?}"
            );
        }
    }

    #[test]
    fn portable_relative_path_and_segment_agree_except_on_drive_letter_and_backslash() {
        // Splitting on '/' and running `is_portable_segment` on each piece
        // must agree with `is_portable_relative_path` for the dot-segment,
        // empty-segment, and NUL-byte rules — the two checks that only
        // apply at the whole-path level (drive-letter prefix, and the
        // deliberately-whole-path-only backslash rejection; see the
        // module-level documentation) are excluded from this comparison.
        let paths = ["a/b/c", "a//b", "a/./b", "a/../b", "a\0b", ""];
        for path in paths {
            let whole = is_portable_relative_path(path);
            let per_segment = !path.is_empty() && path.split('/').all(is_portable_segment);
            assert_eq!(
                whole.is_ok(),
                per_segment,
                "path={path:?} whole={whole:?} per_segment={per_segment}"
            );
        }
    }

    #[test]
    fn portable_path_error_kind_display_text_is_stable() {
        let cases = [
            (PortablePathErrorKind::Empty, "path is empty"),
            (PortablePathErrorKind::LeadingSlash, "path starts with '/'"),
            (PortablePathErrorKind::Backslash, "path contains '\\'"),
            (
                PortablePathErrorKind::WindowsDriveLetter,
                "path starts with a Windows drive-letter prefix",
            ),
            (
                PortablePathErrorKind::CurrentDirSegment,
                "path contains a '.' segment",
            ),
            (
                PortablePathErrorKind::ParentDirSegment,
                "path contains a '..' segment",
            ),
            (
                PortablePathErrorKind::EmptySegment,
                "path contains an empty segment",
            ),
            (PortablePathErrorKind::NulByte, "path contains a NUL byte"),
        ];
        for (kind, expected) in cases {
            assert_eq!(PortablePathError::new(kind).to_string(), expected);
        }
    }
}
