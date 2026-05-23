//! Tests for `generators/number.rs`.

use crate::common::{default_opts, seeded_opts};
use chasm_faker::generate;
use serde_json::json;

/// `generate()` returns a number when the schema declares `type: "number"`.
#[test]
fn test_returns_number_for_number_type() {
    let schema = json!({"type": "number"});

    let value = generate(&schema, &default_opts()).unwrap();

    assert!(value.is_number());
}

/// `generate()` returns an integer-shaped JSON number when the schema declares `type: "integer"`.
#[test]
fn test_returns_integer_for_integer_type() {
    let schema = json!({"type": "integer"});

    let value = generate(&schema, &default_opts()).unwrap();

    assert!(value.is_i64() || value.is_u64());
}

/// Integer `minimum` and `maximum` bounds are respected across many trials.
#[test]
fn test_integer_min_max_bounds_respected() {
    let schema = json!({"type": "integer", "minimum": 5, "maximum": 10});

    for _ in 0..20 {
        let value = generate(&schema, &default_opts()).unwrap();
        let n = value.as_i64().unwrap();

        assert!((5..=10).contains(&n));
    }
}

/// Integer `multipleOf` constraint yields only multiples of the divisor.
#[test]
fn test_integer_multiple_of_constraint_respected() {
    let schema = json!({"type": "integer", "minimum": 0, "maximum": 100, "multipleOf": 5});

    let value = generate(&schema, &default_opts()).unwrap();
    let n = value.as_i64().unwrap();

    assert_eq!(n % 5, 0);
}

/// Integer `multipleOf` with an even divisor and a bounded range yields valid even values.
#[test]
fn test_integer_multiple_of_even_in_range() {
    let schema = json!({"type": "integer", "multipleOf": 2, "minimum": 0, "maximum": 10});

    for seed in 0..50_u64 {
        let value = generate(&schema, &seeded_opts(seed)).unwrap();
        let n = value.as_i64().expect("expected i64");

        assert!((0..=10).contains(&n) && n % 2 == 0);
    }
}

/// Fractional `multipleOf` on an integer schema does not silently disable the constraint.
#[test]
fn test_integer_multiple_of_fractional_safe() {
    let schema = json!({"type": "integer", "multipleOf": 0.5});

    for seed in 0..20_u64 {
        let value = generate(&schema, &seeded_opts(seed)).unwrap();
        let n = value.as_i64().expect("expected i64 integer");
        let r = (n as f64 / 0.5) % 1.0;

        assert!(r.abs() < 1e-9 || (1.0 - r.abs()).abs() < 1e-9);
    }
}

/// A very small `multipleOf` (e.g. `1e-7`) still produces values satisfying the strict check.
#[test]
fn test_number_multiple_of_tiny_value_respected() {
    let schema = json!({
        "type": "number",
        "multipleOf": 0.0000001,
        "minimum": 0,
        "maximum": 0.0001
    });

    for seed in 0..20_u64 {
        let value = generate(&schema, &seeded_opts(seed)).unwrap();
        let v = value.as_f64().expect("expected f64");
        let r = (v / 0.0000001) % 1.0;

        assert!(r.abs() < 1e-6 || (1.0 - r.abs()).abs() < 1e-6);
    }
}

/// Tight exclusive bounds smaller than the legacy epsilon still produce values strictly between.
#[test]
fn test_number_exclusive_tight_bounds_strictly_between() {
    let schema = json!({
        "type": "number",
        "minimum": 0.0000001,
        "maximum": 0.0000002,
        "exclusiveMinimum": true,
        "exclusiveMaximum": true
    });

    for seed in 0..20_u64 {
        let value = generate(&schema, &seeded_opts(seed)).unwrap();
        let v = value.as_f64().expect("expected f64");

        assert!(v > 0.0000001 && v < 0.0000002);
    }
}

/// Ordinary exclusive bounds produce values strictly between them.
#[test]
fn test_number_exclusive_normal_bounds_strictly_between() {
    let schema = json!({
        "type": "number",
        "minimum": 5,
        "maximum": 6,
        "exclusiveMinimum": true,
        "exclusiveMaximum": true
    });

    for seed in 0..20_u64 {
        let value = generate(&schema, &seeded_opts(seed)).unwrap();
        let v = value.as_f64().expect("expected f64");

        assert!(v > 5.0 && v < 6.0);
    }
}

/// Full i32 range generates a value without panicking.
#[test]
fn test_integer_full_i32_range_returns_value() {
    let schema = json!({"type": "integer", "minimum": -2147483648_i64, "maximum": 2147483647_i64});

    let value = generate(&schema, &default_opts()).unwrap();
    let n = value.as_i64().expect("expected i64");

    assert!((-2147483648..=2147483647).contains(&n));
}

/// `multipleOf` with a value too large for the scaled-i64 path stays finite and in range.
#[test]
fn test_multiple_of_overflow_guarded_returns_finite_in_range() {
    let schema = json!({
        "type": "number",
        "minimum": -1e21,
        "maximum": 1e21,
        "multipleOf": 1e20
    });

    let value = generate(&schema, &default_opts()).unwrap();
    let n = value.as_f64().expect("expected number");

    assert!(n.is_finite() && (-1e21..=1e21).contains(&n));
}

/// `multiple types in a type array` produces a value of one of the listed types.
#[test]
fn test_multi_type_string_or_integer() {
    let schema = json!({"type": ["string", "integer"]});

    let value = generate(&schema, &default_opts()).unwrap();

    assert!(value.is_string() || value.is_number());
}

/// `type: number` with `format: double` yields fractional values most of the time.
#[test]
fn test_number_format_double_yields_decimals_majority() {
    let schema = json!({"type": "number", "format": "double"});
    let total = 30;
    let mut fractional = 0;

    for seed in 0..total {
        let value = generate(&schema, &seeded_opts(seed as u64)).unwrap();
        let n = value.as_f64().unwrap();
        if n.fract() != 0.0 {
            fractional += 1;
        }
    }

    assert!(fractional >= 20);
}

/// Bare `type: number` yields fractional values most of the time.
#[test]
fn test_number_bare_yields_decimals_majority() {
    let schema = json!({"type": "number"});
    let total = 30;
    let mut fractional = 0;

    for seed in 0..total {
        let value = generate(&schema, &seeded_opts(seed as u64)).unwrap();
        let n = value.as_f64().unwrap();
        if n.fract() != 0.0 {
            fractional += 1;
        }
    }

    assert!(fractional >= 20);
}

/// `type: number` with `format: float` yields fractional values most of the time.
#[test]
fn test_number_format_float_yields_decimals_majority() {
    let schema = json!({"type": "number", "format": "float"});
    let total = 30;
    let mut fractional = 0;

    for seed in 0..total {
        let value = generate(&schema, &seeded_opts(seed as u64)).unwrap();
        let n = value.as_f64().unwrap();
        if n.fract() != 0.0 {
            fractional += 1;
        }
    }

    assert!(fractional >= 20);
}

/// `multipleOf: 0.01` produces values that round-trip exactly to two decimal places.
#[test]
fn test_number_multiple_of_two_decimals_round_trips_multiple_of_precision_holds() {
    let schema = json!({
        "type": "number",
        "multipleOf": 0.01,
        "minimum": 0.0,
        "maximum": 100.0
    });

    for seed in 0..30 {
        let value = generate(&schema, &seeded_opts(seed as u64)).unwrap();
        let n = value.as_f64().unwrap();
        let scaled = (n * 100.0).round();

        assert!(
            (scaled - n * 100.0).abs() < 1e-6,
            "multipleOf precision violated for n={n}",
        );
    }
}

/// `multipleOf: 0.01` with `minimum: 0.0` and `maximum: 100.0` yields values
/// inside the declared inclusive range.
#[test]
fn test_number_multiple_of_two_decimals_round_trips_value_within_declared_range() {
    let schema = json!({
        "type": "number",
        "multipleOf": 0.01,
        "minimum": 0.0,
        "maximum": 100.0
    });

    for seed in 0..30 {
        let value = generate(&schema, &seeded_opts(seed as u64)).unwrap();
        let n = value.as_f64().unwrap();

        assert!(
            (0.0..=100.0).contains(&n),
            "value {n} outside declared range",
        );
    }
}

/// `type: integer` continues to return integer-shaped JSON numbers (no fractional component).
#[test]
fn test_integer_returns_integer_shaped_value() {
    let schema = json!({"type": "integer", "minimum": 0, "maximum": 1000});

    for seed in 0..30 {
        let value = generate(&schema, &seeded_opts(seed as u64)).unwrap();

        assert!(value.is_i64() || value.is_u64());
    }
}
