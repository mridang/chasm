//! Tests for numeric formats (int32, int64, float, double, extreme bounds).

use crate::common::{default_opts, seeded_opts};
use chasm_faker::generate;
use serde_json::json;

/// `format: int32` (string variant) parses as `i32`.
#[test]
fn test_format_int32_string_parses_as_i32() {
    let schema = json!({"type": "string", "format": "int32"});

    let value = generate(&schema, &default_opts()).unwrap();
    let s = value.as_str().unwrap();

    s.parse::<i32>().unwrap();
}

/// `format: int64` (string variant) parses as `i64`.
#[test]
fn test_format_int64_string_parses_as_i64() {
    let schema = json!({"type": "string", "format": "int64"});

    let value = generate(&schema, &default_opts()).unwrap();
    let s = value.as_str().unwrap();

    s.parse::<i64>().unwrap();
}

/// `format: float` (string variant) parses as `f64`.
#[test]
fn test_format_float_string_parses_as_f64() {
    let schema = json!({"type": "string", "format": "float"});

    let value = generate(&schema, &default_opts()).unwrap();
    let s = value.as_str().unwrap();

    s.parse::<f64>().unwrap();
}

/// `{type: integer, format: int32}` is recognised and produces an integer in the i32 range.
#[test]
fn test_format_int32_on_integer_type_in_range() {
    let schema = json!({"type": "integer", "format": "int32"});
    let mut opts = seeded_opts(17);
    opts.fail_on_invalid_format = true;

    let value = generate(&schema, &opts).unwrap();
    let n = value.as_i64().unwrap();

    assert!(n >= i32::MIN as i64 && n <= i32::MAX as i64);
}

/// `{type: integer, format: int64}` is recognised and produces an integer value.
#[test]
fn test_format_int64_on_integer_type() {
    let schema = json!({"type": "integer", "format": "int64"});
    let mut opts = seeded_opts(23);
    opts.fail_on_invalid_format = true;

    let value = generate(&schema, &opts).unwrap();

    assert!(value.is_i64() || value.is_u64());
}

/// Extreme bounds on a numeric format clamp to the declared range without panicking.
#[test]
fn test_numeric_format_extreme_bounds_clamp() {
    let schema = json!({
        "type": "integer",
        "format": "int32",
        "minimum": 1e308_f64
    });

    let value = generate(&schema, &default_opts()).unwrap();
    let n = value.as_i64().unwrap();

    assert!((i32::MIN as i64..=i32::MAX as i64).contains(&n));
}
