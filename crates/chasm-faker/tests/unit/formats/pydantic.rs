//! Tests for Pydantic-style formats (condate, aware-datetime, positive/negative/strict ints, etc.).

use crate::common::{default_opts, seeded_opts};
use chasm_faker::generate;
use serde_json::json;

/// `format: condate` produces a value parseable as a `YYYY-MM-DD` chrono `NaiveDate`.
#[test]
fn test_format_condate_iso_date_parses() {
    use chrono::NaiveDate;
    let schema = json!({"type": "string", "format": "condate"});

    let value = generate(&schema, &default_opts()).unwrap();
    let s = value.as_str().unwrap();

    NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap();
}

/// `format: aware-datetime` produces a value parseable by chrono's RFC-3339 parser.
#[test]
fn test_format_aware_datetime_parses_as_rfc_3339() {
    use chrono::DateTime;
    let schema = json!({"type": "string", "format": "aware-datetime"});

    let value = generate(&schema, &default_opts()).unwrap();
    let s = value.as_str().unwrap();

    DateTime::parse_from_rfc3339(s).unwrap();
}

/// `format: aware-datetime` produces a value with an explicit timezone designator (`Z` or `+00:00`).
#[test]
fn test_format_aware_datetime_has_timezone() {
    let schema = json!({"type": "string", "format": "aware-datetime"});

    let value = generate(&schema, &default_opts()).unwrap();
    let s = value.as_str().unwrap();

    assert!(s.ends_with("+00:00") || s.ends_with('Z'));
}

/// `format: name-email` produces an RFC-5322 `Display Name <addr@host>` value matching the canonical regex.
#[test]
fn test_format_name_email_rfc_5322_shape() {
    let schema = json!({"type": "string", "format": "name-email"});
    let re = regex::Regex::new(r"^.+ <[^<>@\s]+@[^<>@\s]+>$").unwrap();

    let value = generate(&schema, &default_opts()).unwrap();
    let s = value.as_str().unwrap();

    assert!(re.is_match(s), "name-email `{}` not RFC-5322 shape", s);
}

/// `format: positive-int` (string variant) parses as a strictly positive integer.
#[test]
fn test_format_positive_int_string_is_positive() {
    let schema = json!({"type": "string", "format": "positive-int"});

    let value = generate(&schema, &seeded_opts(42)).unwrap();
    let s = value.as_str().unwrap();

    let n: i64 = s.parse().unwrap();
    assert!(n > 0);
}

/// `format: negative-int` (string variant) parses as a strictly negative integer.
#[test]
fn test_format_negative_int_string_is_negative() {
    let schema = json!({"type": "string", "format": "negative-int"});

    let value = generate(&schema, &seeded_opts(42)).unwrap();
    let s = value.as_str().unwrap();

    let n: i64 = s.parse().unwrap();
    assert!(n < 0);
}

/// `format: nonnegative-int` (string variant) parses as a non-negative integer.
#[test]
fn test_format_nonnegative_int_string_is_non_negative() {
    let schema = json!({"type": "string", "format": "nonnegative-int"});

    let value = generate(&schema, &seeded_opts(42)).unwrap();
    let s = value.as_str().unwrap();

    let n: i64 = s.parse().unwrap();
    assert!(n >= 0);
}

/// `format: nonpositive-int` (string variant) parses as a non-positive integer.
#[test]
fn test_format_nonpositive_int_string_is_non_positive() {
    let schema = json!({"type": "string", "format": "nonpositive-int"});

    let value = generate(&schema, &seeded_opts(42)).unwrap();
    let s = value.as_str().unwrap();

    let n: i64 = s.parse().unwrap();
    assert!(n <= 0);
}

/// `format: strict-int` (string variant) parses as `i64`.
#[test]
fn test_format_strict_int_string_parses_as_i64() {
    let schema = json!({"type": "string", "format": "strict-int"});

    let value = generate(&schema, &seeded_opts(42)).unwrap();
    let s = value.as_str().unwrap();

    s.parse::<i64>().unwrap();
}

/// `format: strict-float` (string variant) parses as `f64`.
#[test]
fn test_format_strict_float_string_parses_as_f64() {
    let schema = json!({"type": "string", "format": "strict-float"});

    let value = generate(&schema, &seeded_opts(42)).unwrap();
    let s = value.as_str().unwrap();

    s.parse::<f64>().unwrap();
}

/// `format: negative-int` on a string schema emits a value matching the `-[0-9]+` regex.
#[test]
fn test_format_negative_int_string_matches_minus_digits_regex() {
    let schema = json!({"type": "string", "format": "negative-int"});
    let re = regex::Regex::new(r"^-[0-9]+$").unwrap();

    let value = generate(&schema, &default_opts()).unwrap();
    let s = value.as_str().unwrap();

    assert!(re.is_match(s), "negative-int `{}` did not match -[0-9]+", s);
}

/// `{type: integer, format: positive-int}` produces a strictly-positive integer.
#[test]
fn test_format_positive_int_on_integer_type() {
    let schema = json!({"type": "integer", "format": "positive-int"});

    for seed in 0..30_u64 {
        let value = generate(&schema, &seeded_opts(seed)).unwrap();
        let n = value.as_i64().unwrap();
        assert!(n > 0);
    }
}

/// `{type: integer, format: negative-int}` produces a strictly-negative integer.
#[test]
fn test_format_negative_int_on_integer_type() {
    let schema = json!({"type": "integer", "format": "negative-int"});

    for seed in 0..30_u64 {
        let value = generate(&schema, &seeded_opts(seed)).unwrap();
        let n = value.as_i64().unwrap();
        assert!(n < 0);
    }
}

/// `{type: number, format: strict-float}` produces a finite number in the default range.
#[test]
fn test_format_strict_float_on_number_type() {
    let schema = json!({"type": "number", "format": "strict-float"});

    for seed in 0..30_u64 {
        let value = generate(&schema, &seeded_opts(seed)).unwrap();
        let v = value.as_f64().unwrap();
        assert!(v.is_finite() && (-1000.0..=1000.0).contains(&v));
    }
}

/// `format: condate` produces byte-identical output across runs under the
/// same seed — the date anchor is fixed, not wall-clock derived.
#[test]
fn test_format_condate_is_seed_deterministic_across_runs() {
    let schema = json!({"type": "string", "format": "condate"});

    let first = generate(&schema, &seeded_opts(42)).unwrap();
    let second = generate(&schema, &seeded_opts(42)).unwrap();

    assert_eq!(
        first, second,
        "condate output must be byte-identical under fixed seed"
    );
}

/// `format: condate` produces dates within 10 years of the fixed anchor
/// (2024-01-15), not the wall-clock current date.
#[test]
fn test_format_condate_uses_fixed_anchor_not_wall_clock() {
    let schema = json!({"type": "string", "format": "condate"});

    let value = generate(&schema, &seeded_opts(0)).unwrap();
    let s = value.as_str().expect("string");
    let parsed = chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d")
        .unwrap_or_else(|e| panic!("expected ISO date, got {:?}: {}", s, e));

    let anchor = chrono::NaiveDate::from_ymd_opt(2024, 1, 15).unwrap();
    let diff = (parsed - anchor).num_days().abs();
    assert!(
        diff <= 365 * 10,
        "expected within 10 years of 2024-01-15, got {:?} (diff {} days)",
        parsed,
        diff
    );
}
