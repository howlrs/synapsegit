use std::str::FromStr;

use serde_json::json;
use synapse_schema::{
    CanonicalTimestamp, CanonicalTimestampErrorKind, ScaledInteger, ScaledIntegerErrorKind, Unit,
};

#[test]
fn canonical_timestamps_parse_format_and_round_trip_unix_nanoseconds() {
    let cases = [
        ("1970-01-01T00:00:00.000000000Z", 0),
        ("1969-12-31T23:59:59.999999999Z", -1),
        ("2000-02-29T23:59:59.120000000Z", 951_868_799_120_000_000),
    ];
    for (encoded, unix_nanos) in cases {
        let timestamp = CanonicalTimestamp::parse(encoded).expect(encoded);
        assert_eq!(timestamp.as_str(), encoded);
        assert_eq!(timestamp.unix_nanos(), unix_nanos);
        assert_eq!(
            CanonicalTimestamp::from_unix_nanos(unix_nanos).unwrap(),
            timestamp
        );
        assert_eq!(serde_json::to_value(&timestamp).unwrap(), json!(encoded));
        assert_eq!(
            serde_json::from_value::<CanonicalTimestamp>(json!(encoded)).unwrap(),
            timestamp
        );
    }

    let minimum = CanonicalTimestamp::parse("0000-01-01T00:00:00.000000000Z").unwrap();
    let maximum = CanonicalTimestamp::parse("9999-12-31T23:59:59.999999999Z").unwrap();
    assert_eq!(
        CanonicalTimestamp::from_unix_nanos(minimum.unix_nanos()).unwrap(),
        minimum
    );
    assert_eq!(
        CanonicalTimestamp::from_unix_nanos(maximum.unix_nanos()).unwrap(),
        maximum
    );
    assert_eq!(
        CanonicalTimestamp::from_unix_nanos(minimum.unix_nanos() - 1)
            .unwrap_err()
            .kind(),
        CanonicalTimestampErrorKind::OutOfRange
    );
    assert_eq!(
        CanonicalTimestamp::from_unix_nanos(maximum.unix_nanos() + 1)
            .unwrap_err()
            .kind(),
        CanonicalTimestampErrorKind::OutOfRange
    );
}

#[test]
fn canonical_timestamps_reject_noncanonical_or_impossible_values() {
    for invalid in [
        "2026-07-20T00:00:00Z",
        "2026-07-20T00:00:00.12000000Z",
        "2026-07-20T00:00:00.000000000+00:00",
    ] {
        assert_eq!(
            CanonicalTimestamp::parse(invalid).unwrap_err().kind(),
            CanonicalTimestampErrorKind::InvalidLexicalForm,
            "{invalid}"
        );
    }
    for invalid in [
        "1900-02-29T00:00:00.000000000Z",
        "2026-02-30T00:00:00.000000000Z",
        "2026-07-20T24:00:00.000000000Z",
        "2026-07-20T23:59:60.000000000Z",
    ] {
        assert_eq!(
            CanonicalTimestamp::parse(invalid).unwrap_err().kind(),
            CanonicalTimestampErrorKind::InvalidCalendarDate,
            "{invalid}"
        );
    }
}

#[test]
fn exact_decimal_strings_normalize_without_floating_point() {
    let cases = [
        ("1440.0", Unit::Px, "144", 1),
        ("0.9200", Unit::Ratio, "92", -2),
        ("0001.2300", Unit::Unitless, "123", -2),
        ("1000", Unit::Count, "1", 3),
        ("-12.3400", Unit::Celsius, "-1234", -2),
        ("0.000", Unit::Unitless, "0", 0),
    ];
    for (decimal, unit, mantissa, scale) in cases {
        let scaled = ScaledInteger::from_decimal_str(decimal, unit).expect(decimal);
        assert_eq!(scaled.mantissa(), mantissa);
        assert_eq!(scaled.scale(), scale);
        assert_eq!(scaled.unit(), unit);
        assert_eq!(
            serde_json::to_value(&scaled).unwrap(),
            json!({"mantissa": mantissa, "scale": scale, "unit": unit.as_str()})
        );
    }
}

#[test]
fn scaled_integer_parts_and_serde_enforce_normalized_wire_form() {
    let value = ScaledInteger::try_from_parts("92", -2, Unit::Ratio).unwrap();
    assert_eq!(
        serde_json::from_value::<ScaledInteger>(json!({
            "mantissa": "92",
            "scale": -2,
            "unit": "ratio"
        }))
        .unwrap(),
        value
    );

    let invalid_parts = [
        ("10", 0, ScaledIntegerErrorKind::NotNormalized),
        ("01", 0, ScaledIntegerErrorKind::NotNormalized),
        ("-0", 0, ScaledIntegerErrorKind::NegativeZero),
        ("0", -1, ScaledIntegerErrorKind::NotNormalized),
        ("1", 25, ScaledIntegerErrorKind::ScaleOutOfRange),
    ];
    for (mantissa, scale, kind) in invalid_parts {
        assert_eq!(
            ScaledInteger::try_from_parts(mantissa, scale, Unit::Unitless)
                .unwrap_err()
                .kind(),
            kind,
            "{mantissa}e{scale}"
        );
    }
    assert_eq!(
        ScaledInteger::try_from_parts("1".repeat(258), 0, Unit::Unitless)
            .unwrap_err()
            .kind(),
        ScaledIntegerErrorKind::MantissaTooLong
    );
    assert!(
        serde_json::from_value::<ScaledInteger>(
            json!({"mantissa": "10", "scale": 0, "unit": "px"})
        )
        .is_err()
    );
    assert!(
        serde_json::from_value::<ScaledInteger>(
            json!({"mantissa": "1", "scale": 0, "unit": "px", "extra": true})
        )
        .is_err()
    );
}

#[test]
fn decimal_conversion_rejects_ambiguous_tokens_negative_zero_and_range_overflow() {
    for invalid in ["", "+1", " 1", "1 ", ".1", "1.", "1e3", "NaN", "inf"] {
        assert_eq!(
            ScaledInteger::from_decimal_str(invalid, Unit::Unitless)
                .unwrap_err()
                .kind(),
            ScaledIntegerErrorKind::InvalidDecimal,
            "{invalid:?}"
        );
    }
    for negative_zero in ["-0", "-0.0", "-000.000"] {
        assert_eq!(
            ScaledInteger::from_decimal_str(negative_zero, Unit::Unitless)
                .unwrap_err()
                .kind(),
            ScaledIntegerErrorKind::NegativeZero,
            "{negative_zero}"
        );
    }
    assert_eq!(
        ScaledInteger::from_decimal_str("0.0000000000000000000000001", Unit::Unitless)
            .unwrap_err()
            .kind(),
        ScaledIntegerErrorKind::ScaleOutOfRange
    );
    assert_eq!(
        ScaledInteger::from_decimal_str("10000000000000000000000000", Unit::Unitless)
            .unwrap_err()
            .kind(),
        ScaledIntegerErrorKind::ScaleOutOfRange
    );
    assert_eq!(
        Unit::from_str("inch").unwrap_err().kind(),
        ScaledIntegerErrorKind::InvalidUnit
    );
}
