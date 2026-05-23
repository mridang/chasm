//! Tests for `generators/enum_const.rs`.

use crate::common::default_opts;
use chasm_faker::generate;
use serde_json::json;

/// `const` returns the exact specified numeric value.
#[test]
fn test_const_returns_exact_numeric_value() {
    let schema = json!({"const": 42});

    let value = generate(&schema, &default_opts()).unwrap();

    assert_eq!(value, json!(42));
}

/// A string `const` value is returned verbatim with no randomisation.
#[test]
fn test_const_string_returned_verbatim() {
    let schema = json!({"const": "X"});

    let value = generate(&schema, &default_opts()).unwrap();

    assert_eq!(value, json!("X"));
}

/// `const: null` produces a literal JSON null rather than being treated as absent.
#[test]
fn test_const_null_returned_verbatim() {
    let schema = json!({"const": null});

    let value = generate(&schema, &default_opts()).unwrap();

    assert_eq!(value, json!(null));
}

/// A nested `const` inside an object property returns the constant value verbatim.
#[test]
fn test_const_nested_in_object_property() {
    let schema = json!({
        "type": "object",
        "properties": {
            "kind": {"const": "a"},
            "name": {"type": "string"}
        },
        "required": ["kind", "name"]
    });

    let value = generate(&schema, &default_opts()).unwrap();

    assert_eq!(value.get("kind"), Some(&json!("a")));
}

/// When both `type` and `const` are present, `const` wins on every invocation.
#[test]
fn test_const_takes_priority_over_type() {
    let schema = json!({"type": "string", "const": "fixed"});

    for _ in 0..10 {
        let value = generate(&schema, &default_opts()).unwrap();

        assert_eq!(value, json!("fixed"));
    }
}

/// `enum` selects one of the declared values.
#[test]
fn test_enum_picks_valid_value() {
    let schema = json!({"enum": ["a", "b", "c"]});

    let value = generate(&schema, &default_opts()).unwrap();

    assert!(value == json!("a") || value == json!("b") || value == json!("c"));
}
