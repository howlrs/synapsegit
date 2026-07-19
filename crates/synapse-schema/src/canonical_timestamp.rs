use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::error::Error;
use std::fmt;
use std::str::FromStr;

const NANOS_PER_SECOND: i128 = 1_000_000_000;
const SECONDS_PER_DAY: i128 = 86_400;

/// A canonical UTC timestamp in the exact Core v0.1 wire format.
///
/// Values always use `YYYY-MM-DDTHH:mm:ss.nnnnnnnnnZ`, including exactly nine
/// fractional digits. Parsing validates the proleptic Gregorian calendar.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct CanonicalTimestamp {
    encoded: String,
    unix_nanos: i128,
}

impl CanonicalTimestamp {
    /// Parses and validates an exact canonical timestamp.
    pub fn parse(value: &str) -> Result<Self, CanonicalTimestampError> {
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
            return Err(CanonicalTimestampError::new(
                CanonicalTimestampErrorKind::InvalidLexicalForm,
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
            || day > days_in_month(year, month)
            || hour > 23
            || minute > 59
            || second > 59
        {
            return Err(CanonicalTimestampError::new(
                CanonicalTimestampErrorKind::InvalidCalendarDate,
            ));
        }

        let day_of_year = (1..month)
            .map(|candidate| i128::from(days_in_month(year, candidate)))
            .sum::<i128>()
            + i128::from(day - 1);
        let absolute_days = days_before_year(year) + day_of_year;
        let unix_days = absolute_days - days_before_year(1970);
        let unix_seconds = unix_days * SECONDS_PER_DAY
            + i128::from(hour) * 3_600
            + i128::from(minute) * 60
            + i128::from(second);

        Ok(Self {
            encoded: value.to_owned(),
            unix_nanos: unix_seconds * NANOS_PER_SECOND + i128::from(nanos),
        })
    }

    /// Formats a Unix timestamp expressed as nanoseconds from 1970-01-01 UTC.
    ///
    /// Values outside the representable four-digit year range are rejected.
    pub fn from_unix_nanos(unix_nanos: i128) -> Result<Self, CanonicalTimestampError> {
        let unix_seconds = unix_nanos.div_euclid(NANOS_PER_SECOND);
        let nanos = unix_nanos.rem_euclid(NANOS_PER_SECOND) as u32;
        let unix_days = unix_seconds.div_euclid(SECONDS_PER_DAY);
        let second_of_day = unix_seconds.rem_euclid(SECONDS_PER_DAY);
        let absolute_days = unix_days + days_before_year(1970);

        if absolute_days < 0 || absolute_days >= days_before_year(10_000) {
            return Err(CanonicalTimestampError::new(
                CanonicalTimestampErrorKind::OutOfRange,
            ));
        }

        let mut lower = 0_u32;
        let mut upper = 10_000_u32;
        while lower + 1 < upper {
            let middle = lower + (upper - lower) / 2;
            if days_before_year(middle) <= absolute_days {
                lower = middle;
            } else {
                upper = middle;
            }
        }
        let year = lower;
        let mut day_of_year = absolute_days - days_before_year(year);
        let mut month = 1_u32;
        while day_of_year >= i128::from(days_in_month(year, month)) {
            day_of_year -= i128::from(days_in_month(year, month));
            month += 1;
        }
        let day = day_of_year + 1;
        let hour = second_of_day / 3_600;
        let minute = second_of_day % 3_600 / 60;
        let second = second_of_day % 60;
        let encoded =
            format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{nanos:09}Z");

        Ok(Self {
            encoded,
            unix_nanos,
        })
    }

    /// Returns the canonical wire representation.
    pub fn as_str(&self) -> &str {
        &self.encoded
    }

    /// Returns nanoseconds from 1970-01-01 00:00:00 UTC.
    pub const fn unix_nanos(&self) -> i128 {
        self.unix_nanos
    }
}

impl fmt::Display for CanonicalTimestamp {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for CanonicalTimestamp {
    type Err = CanonicalTimestampError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value)
    }
}

impl TryFrom<&str> for CanonicalTimestamp {
    type Error = CanonicalTimestampError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::parse(value)
    }
}

impl TryFrom<String> for CanonicalTimestamp {
    type Error = CanonicalTimestampError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(&value)
    }
}

impl Serialize for CanonicalTimestamp {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for CanonicalTimestamp {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(&value).map_err(serde::de::Error::custom)
    }
}

/// Stable category for a canonical timestamp validation failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CanonicalTimestampErrorKind {
    InvalidLexicalForm,
    InvalidCalendarDate,
    OutOfRange,
}

/// Error returned when a timestamp is not exactly representable by the Core
/// v0.1 canonical timestamp format.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CanonicalTimestampError {
    kind: CanonicalTimestampErrorKind,
}

impl CanonicalTimestampError {
    const fn new(kind: CanonicalTimestampErrorKind) -> Self {
        Self { kind }
    }

    pub const fn kind(self) -> CanonicalTimestampErrorKind {
        self.kind
    }
}

impl fmt::Display for CanonicalTimestampError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self.kind {
            CanonicalTimestampErrorKind::InvalidLexicalForm => {
                "timestamp must use YYYY-MM-DDTHH:mm:ss.nnnnnnnnnZ"
            }
            CanonicalTimestampErrorKind::InvalidCalendarDate => {
                "timestamp is not a valid Gregorian date and time"
            }
            CanonicalTimestampErrorKind::OutOfRange => {
                "timestamp is outside the four-digit year range"
            }
        };
        formatter.write_str(message)
    }
}

impl Error for CanonicalTimestampError {}

const fn is_leap_year(year: u32) -> bool {
    year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400))
}

const fn days_in_month(year: u32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap_year(year) => 29,
        2 => 28,
        _ => 0,
    }
}

const fn days_before_year(year: u32) -> i128 {
    if year == 0 {
        return 0;
    }
    let completed = (year - 1) as i128;
    year as i128 * 365 + completed / 4 - completed / 100 + completed / 400 + 1
}
