use serde::{Deserialize, Deserializer, Serialize};
use std::error::Error;
use std::fmt;
use std::str::FromStr;

const MAX_MANTISSA_BYTES: usize = 257;
const MAX_DECIMAL_INPUT_BYTES: usize = 1_024;
const MIN_SCALE: i64 = -24;
const MAX_SCALE: i64 = 24;

/// Unit vocabulary accepted by Core v0.1 `ScaledInteger` values.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Unit {
    Unitless,
    Ratio,
    Percent,
    Count,
    Byte,
    Px,
    Mm,
    M,
    Ms,
    S,
    Deg,
    Rad,
    Kelvin,
    Celsius,
    DeltaE,
}

impl Unit {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Unitless => "unitless",
            Self::Ratio => "ratio",
            Self::Percent => "percent",
            Self::Count => "count",
            Self::Byte => "byte",
            Self::Px => "px",
            Self::Mm => "mm",
            Self::M => "m",
            Self::Ms => "ms",
            Self::S => "s",
            Self::Deg => "deg",
            Self::Rad => "rad",
            Self::Kelvin => "kelvin",
            Self::Celsius => "celsius",
            Self::DeltaE => "delta_e",
        }
    }
}

impl fmt::Display for Unit {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for Unit {
    type Err = ScaledIntegerError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "unitless" => Ok(Self::Unitless),
            "ratio" => Ok(Self::Ratio),
            "percent" => Ok(Self::Percent),
            "count" => Ok(Self::Count),
            "byte" => Ok(Self::Byte),
            "px" => Ok(Self::Px),
            "mm" => Ok(Self::Mm),
            "m" => Ok(Self::M),
            "ms" => Ok(Self::Ms),
            "s" => Ok(Self::S),
            "deg" => Ok(Self::Deg),
            "rad" => Ok(Self::Rad),
            "kelvin" => Ok(Self::Kelvin),
            "celsius" => Ok(Self::Celsius),
            "delta_e" => Ok(Self::DeltaE),
            _ => Err(ScaledIntegerError::new(ScaledIntegerErrorKind::InvalidUnit)),
        }
    }
}

/// Exact fixed-point representation used by the Core v0.1 schemas.
///
/// The represented value is `mantissa * 10^scale`. Construction always
/// enforces the schema's normalized form and never passes through `f64`.
#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize)]
pub struct ScaledInteger {
    mantissa: String,
    scale: i64,
    unit: Unit,
}

impl ScaledInteger {
    /// Constructs a value that is already in normalized wire form.
    pub fn try_from_parts(
        mantissa: impl Into<String>,
        scale: i64,
        unit: Unit,
    ) -> Result<Self, ScaledIntegerError> {
        let mantissa = mantissa.into();
        validate_parts(&mantissa, scale)?;
        Ok(Self {
            mantissa,
            scale,
            unit,
        })
    }

    /// Converts a plain decimal string into normalized fixed-point form.
    ///
    /// Accepted input consists only of an optional `-`, one or more integer
    /// digits, and an optional fractional part containing one or more digits.
    /// Exponents, a leading `+`, whitespace, `NaN`, and infinities are rejected.
    pub fn from_decimal_str(value: &str, unit: Unit) -> Result<Self, ScaledIntegerError> {
        if value.is_empty() || value.len() > MAX_DECIMAL_INPUT_BYTES || !value.is_ascii() {
            return Err(ScaledIntegerError::new(
                ScaledIntegerErrorKind::InvalidDecimal,
            ));
        }

        let (negative, unsigned) = match value.strip_prefix('-') {
            Some(unsigned) => (true, unsigned),
            None => (false, value),
        };
        let (integer, fraction) = match unsigned.split_once('.') {
            Some((integer, fraction)) => (integer, Some(fraction)),
            None => (unsigned, None),
        };
        if integer.is_empty()
            || !integer.bytes().all(|byte| byte.is_ascii_digit())
            || fraction.is_some_and(|fraction| {
                fraction.is_empty() || !fraction.bytes().all(|byte| byte.is_ascii_digit())
            })
        {
            return Err(ScaledIntegerError::new(
                ScaledIntegerErrorKind::InvalidDecimal,
            ));
        }

        let fraction_len = fraction.map_or(0, str::len);
        let mut digits = String::with_capacity(integer.len() + fraction_len);
        digits.push_str(integer);
        if let Some(fraction) = fraction {
            digits.push_str(fraction);
        }

        let first_nonzero = digits.bytes().position(|byte| byte != b'0');
        let Some(first_nonzero) = first_nonzero else {
            if negative {
                return Err(ScaledIntegerError::new(
                    ScaledIntegerErrorKind::NegativeZero,
                ));
            }
            return Self::try_from_parts("0", 0, unit);
        };
        digits.drain(..first_nonzero);

        let mut scale = -(i64::try_from(fraction_len)
            .map_err(|_| ScaledIntegerError::new(ScaledIntegerErrorKind::ScaleOutOfRange))?);
        while digits.ends_with('0') {
            digits.pop();
            scale = scale
                .checked_add(1)
                .ok_or_else(|| ScaledIntegerError::new(ScaledIntegerErrorKind::ScaleOutOfRange))?;
        }
        if negative {
            digits.insert(0, '-');
        }

        Self::try_from_parts(digits, scale, unit)
    }

    pub fn mantissa(&self) -> &str {
        &self.mantissa
    }

    pub const fn scale(&self) -> i64 {
        self.scale
    }

    pub const fn unit(&self) -> Unit {
        self.unit
    }
}

impl<'de> Deserialize<'de> for ScaledInteger {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct WireValue {
            mantissa: String,
            scale: i64,
            unit: Unit,
        }

        let wire = WireValue::deserialize(deserializer)?;
        Self::try_from_parts(wire.mantissa, wire.scale, wire.unit).map_err(serde::de::Error::custom)
    }
}

/// Stable category for a `ScaledInteger` construction failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScaledIntegerErrorKind {
    InvalidDecimal,
    NegativeZero,
    NotNormalized,
    MantissaTooLong,
    ScaleOutOfRange,
    InvalidUnit,
}

impl ScaledIntegerErrorKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InvalidDecimal => "invalid_decimal",
            Self::NegativeZero => "negative_zero",
            Self::NotNormalized => "not_normalized",
            Self::MantissaTooLong => "mantissa_too_long",
            Self::ScaleOutOfRange => "scale_out_of_range",
            Self::InvalidUnit => "invalid_unit",
        }
    }
}

/// Error returned when an exact decimal or wire representation cannot be
/// represented as a normalized Core v0.1 `ScaledInteger`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ScaledIntegerError {
    kind: ScaledIntegerErrorKind,
}

impl ScaledIntegerError {
    const fn new(kind: ScaledIntegerErrorKind) -> Self {
        Self { kind }
    }

    pub const fn kind(self) -> ScaledIntegerErrorKind {
        self.kind
    }
}

impl fmt::Display for ScaledIntegerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.kind.as_str())
    }
}

impl Error for ScaledIntegerError {}

fn validate_parts(mantissa: &str, scale: i64) -> Result<(), ScaledIntegerError> {
    if mantissa.len() > MAX_MANTISSA_BYTES {
        return Err(ScaledIntegerError::new(
            ScaledIntegerErrorKind::MantissaTooLong,
        ));
    }
    if !(MIN_SCALE..=MAX_SCALE).contains(&scale) {
        return Err(ScaledIntegerError::new(
            ScaledIntegerErrorKind::ScaleOutOfRange,
        ));
    }
    if mantissa == "-0" {
        return Err(ScaledIntegerError::new(
            ScaledIntegerErrorKind::NegativeZero,
        ));
    }
    if mantissa == "0" {
        return if scale == 0 {
            Ok(())
        } else {
            Err(ScaledIntegerError::new(
                ScaledIntegerErrorKind::NotNormalized,
            ))
        };
    }

    let digits = mantissa.strip_prefix('-').unwrap_or(mantissa);
    if digits.is_empty()
        || digits.starts_with('0')
        || digits.ends_with('0')
        || !digits.bytes().all(|byte| byte.is_ascii_digit())
    {
        return Err(ScaledIntegerError::new(
            ScaledIntegerErrorKind::NotNormalized,
        ));
    }
    Ok(())
}
