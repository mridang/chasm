//! Tests for `generators/boolean.rs`.

use crate::common::default_opts;
use chasm_faker::generate;
use serde_json::json;

/// `generate()` returns a boolean value when the schema declares `type: "boolean"`.
#[test]
fn test_returns_boolean_for_boolean_type() {
    let schema = json!({"type": "boolean"});

    let value = generate(&schema, &default_opts()).unwrap();

    assert!(value.is_boolean());
}

/// `format: boolean` on a string schema yields the literal `"true"` or `"false"`.
#[test]
fn test_format_boolean_string_yields_true_or_false() {
    let schema = json!({"type": "string", "format": "boolean"});

    let value = generate(&schema, &default_opts()).unwrap();

    let s = value.as_str().expect("expected string");
    assert!(s == "true" || s == "false");
}

/// The `bool` alias is recognised as an equivalent for `format: boolean`.
#[test]
fn test_format_bool_alias_yields_true_or_false() {
    let schema = json!({"type": "string", "format": "bool"});

    let value = generate(&schema, &default_opts()).unwrap();

    let s = value.as_str().expect("expected string");
    assert!(s == "true" || s == "false");
}

/// `format: strict-bool` produces only the literal `"true"` or `"false"` string.
#[test]
fn test_format_strict_bool_yields_true_or_false() {
    let schema = json!({"type": "string", "format": "strict-bool"});

    let value = generate(&schema, &default_opts()).unwrap();

    let s = value.as_str().expect("expected string");
    assert!(s == "true" || s == "false");
}
