//! Tests for `options.rs`.

use crate::common::{default_opts, seeded_opts};
use chasm_faker::{generate, FakerError};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;

/// `use_default_value` returns the schema's `default` value verbatim.
#[test]
fn test_use_default_value_returns_default() {
    let schema = json!({"type": "string", "default": "Hello"});
    let mut opts = default_opts();
    opts.use_default_value = true;

    let value = generate(&schema, &opts).unwrap();

    assert_eq!(value, json!("Hello"));
}

/// `use_default_value` preserves an empty-string default rather than treating it as falsy.
#[test]
fn test_use_default_value_preserves_empty_string() {
    let schema = json!({"type": "string", "default": ""});
    let mut opts = default_opts();
    opts.use_default_value = true;

    let value = generate(&schema, &opts).unwrap();

    assert_eq!(value, json!(""));
}

/// `use_default_value` preserves a numeric zero default.
#[test]
fn test_use_default_value_preserves_zero() {
    let schema = json!({"type": "number", "default": 0});
    let mut opts = default_opts();
    opts.use_default_value = true;

    let value = generate(&schema, &opts).unwrap();

    assert_eq!(value, json!(0));
}

/// `use_default_value` combined with `always_fake_optionals` applies defaults to optionals.
#[test]
fn test_use_default_value_with_always_fake_optionals() {
    let schema = json!({
        "type": "object",
        "properties": {
            "opt1": {"type": "number", "default": 1},
            "opt2": {"type": "number", "default": 1}
        }
    });
    let mut opts = default_opts();
    opts.use_default_value = true;
    opts.always_fake_optionals = true;

    let value = generate(&schema, &opts).unwrap();
    let obj = value.as_object().unwrap();

    assert_eq!(obj.get("opt1"), Some(&json!(1)));
}

/// `use_examples_value` returns a value drawn from the `examples` array.
#[test]
fn test_use_examples_value_returns_examples_entry() {
    let schema = json!({"type": "string", "examples": ["World"]});
    let mut opts = default_opts();
    opts.use_examples_value = true;

    let value = generate(&schema, &opts).unwrap();

    assert_eq!(value, json!("World"));
}

/// `use_examples_value` returns the value of an `example` property.
#[test]
fn test_use_examples_value_returns_example_property() {
    let schema = json!({"type": "string", "example": "World"});
    let mut opts = default_opts();
    opts.use_examples_value = true;

    let value = generate(&schema, &opts).unwrap();

    assert_eq!(value, json!("World"));
}

/// `output_transform` is invoked on the final generated value.
#[test]
fn test_output_transform_is_applied() {
    let schema = json!({"type": "string"});
    let mut opts = seeded_opts(42);
    opts.output_transform = Some(Arc::new(|_v: &Value, _root: &Value| {
        Value::String("transformed".to_string())
    }));

    let value = generate(&schema, &opts).unwrap();

    assert_eq!(value, Value::String("transformed".to_string()));
}

/// `output_transform` receives the root schema as its second argument.
#[test]
fn test_output_transform_receives_root_schema() {
    let schema = json!({"type": "integer", "minimum": 1, "maximum": 1});
    let mut opts = seeded_opts(1);
    opts.output_transform = Some(Arc::new(|_v: &Value, root: &Value| {
        let t = root.get("type").and_then(|x| x.as_str()).unwrap_or("");
        Value::String(format!("root_type={}", t))
    }));

    let value = generate(&schema, &opts).unwrap();

    assert_eq!(value, Value::String("root_type=integer".to_string()));
}

/// `omit_nulls = true` removes optional properties whose value resolves to `null`.
#[test]
fn test_omit_nulls_true_drops_null_optional() {
    let schema = json!({
        "type": "object",
        "properties": {"v": {"type": "null"}}
    });
    let mut opts = seeded_opts(1);
    opts.always_fake_optionals = true;
    opts.omit_nulls = true;

    let value = generate(&schema, &opts).unwrap();
    let obj = value.as_object().unwrap();

    assert!(!obj.contains_key("v"));
}

/// `omit_nulls = false` (the default off-state) keeps optional properties whose value is `null`.
#[test]
fn test_omit_nulls_false_keeps_null_optional() {
    let schema = json!({
        "type": "object",
        "properties": {"v": {"type": "null"}}
    });
    let mut opts = seeded_opts(1);
    opts.always_fake_optionals = true;
    opts.omit_nulls = false;

    let value = generate(&schema, &opts).unwrap();
    let obj = value.as_object().unwrap();

    assert_eq!(obj.get("v"), Some(&Value::Null));
}

/// `prop_aliases` renames a schema-declared alias key to its canonical name before generation.
///
/// The schema uses `properties_alias` as a stand-in for the canonical `properties` key; the
/// option remaps it so the walker treats the inner object as a normal `properties` block.
#[test]
fn test_prop_aliases_renames_schema_key() {
    let schema = json!({
        "type": "object",
        "properties_alias": {"k": {"type": "string", "const": "renamed"}},
        "required": ["k"]
    });
    let mut opts = seeded_opts(1);
    opts.prop_aliases = HashMap::from([("properties_alias".to_string(), "properties".to_string())]);

    let value = generate(&schema, &opts).unwrap();

    assert_eq!(value.get("k"), Some(&json!("renamed")));
}

/// With no `prop_aliases` configured the alias key is ignored: the canonical-side const value is
/// not recognised because the walker never looked under the alias.
#[test]
fn test_prop_aliases_absent_does_not_rename() {
    let schema = json!({
        "type": "object",
        "properties_alias": {"k": {"type": "string", "const": "renamed"}},
        "required": ["k"]
    });

    let value = generate(&schema, &default_opts()).unwrap();

    assert_ne!(value.get("k"), Some(&json!("renamed")));
}

/// `ignore_properties` strips listed keys from the generated object.
#[test]
fn test_ignore_properties_strips_listed_keys() {
    let schema = json!({
        "type": "object",
        "properties": {
            "keep": {"type": "string"},
            "drop": {"type": "string"}
        },
        "required": ["keep", "drop"]
    });
    let mut opts = default_opts();
    opts.ignore_properties = vec!["drop".to_string()];

    let value = generate(&schema, &opts).unwrap();
    let obj = value.as_object().unwrap();

    assert!(!obj.contains_key("drop"));
}

/// `prune_properties` strips matching keys from nested object values after generation.
#[test]
fn test_prune_properties_strips_nested_keys() {
    let schema = json!({
        "type": "object",
        "properties": {
            "nested": {
                "type": "object",
                "properties": {
                    "keep": {"type": "string"},
                    "drop": {"type": "string"}
                },
                "required": ["keep", "drop"]
            }
        },
        "required": ["nested"]
    });
    let mut opts = seeded_opts(1);
    opts.prune_properties = vec!["drop".to_string()];

    let value = generate(&schema, &opts).unwrap();
    let nested = value.get("nested").unwrap().as_object().unwrap();

    assert!(!nested.contains_key("drop"));
}

/// `min_items` provides a global override that forces array length up to the configured floor.
#[test]
fn test_min_items_global_override_forces_floor() {
    let schema = json!({"type": "array", "items": {"type": "integer"}});
    let mut opts = seeded_opts(1);
    opts.min_items = Some(5);
    opts.max_items = Some(5);

    let value = generate(&schema, &opts).unwrap();
    let arr = value.as_array().unwrap();

    assert_eq!(arr.len(), 5);
}

/// `max_items` provides a global override that caps generated array length.
#[test]
fn test_max_items_global_override_caps_length() {
    let schema =
        json!({"type": "array", "items": {"type": "integer"}, "minItems": 0, "maxItems": 100});
    let mut opts = seeded_opts(1);
    opts.max_items = Some(2);

    let value = generate(&schema, &opts).unwrap();
    let arr = value.as_array().unwrap();

    assert!(arr.len() <= 2);
}

/// `external_refs` supplies a replacement schema for a remote `$ref` URI.
#[test]
fn test_external_refs_substitutes_for_remote_ref() {
    let schema = json!({
        "type": "object",
        "properties": {"thing": {"$ref": "https://example.com/external.json"}},
        "required": ["thing"]
    });
    let mut opts = seeded_opts(1);
    opts.external_refs = HashMap::from([(
        "https://example.com/external.json".to_string(),
        json!({"type": "string", "const": "from-external"}),
    )]);

    let value = generate(&schema, &opts).unwrap();

    assert_eq!(value.get("thing"), Some(&json!("from-external")));
}

/// Without an `external_refs` entry an unresolvable remote `$ref` surfaces `UnresolvedRef`.
#[test]
fn test_external_refs_absent_surfaces_unresolved_ref() {
    let schema = json!({
        "type": "object",
        "properties": {"thing": {"$ref": "https://example.com/external.json"}},
        "required": ["thing"]
    });

    let err = generate(&schema, &default_opts()).expect_err("expected UnresolvedRef");

    assert!(matches!(err, FakerError::UnresolvedRef { .. }));
}

/// `default_invalid_type_product = Some(value)` returns the value verbatim when the type is unknown
/// and `fail_on_invalid_type` is disabled.
#[test]
fn test_default_invalid_type_product_returns_value() {
    let schema = json!({"type": "not-a-real-type"});
    let mut opts = default_opts();
    opts.fail_on_invalid_type = false;
    opts.default_invalid_type_product = Some(json!({"fallback": true}));

    let value = generate(&schema, &opts).unwrap();

    assert_eq!(value, json!({"fallback": true}));
}

/// `fixed_probabilities = true` makes optional inclusion deterministic at the 0.5 cutoff.
///
/// Under a fixed cutoff the walker either always includes or always omits a given optional
/// across seeds, so we expect the SAME inclusion decision under two different seeds.
#[test]
fn test_fixed_probabilities_is_deterministic_across_seeds() {
    let schema = json!({
        "type": "object",
        "properties": {"opt": {"type": "string"}}
    });
    let mut opts_a = seeded_opts(1);
    opts_a.fixed_probabilities = true;
    let mut opts_b = seeded_opts(99);
    opts_b.fixed_probabilities = true;

    let a = generate(&schema, &opts_a).unwrap();
    let b = generate(&schema, &opts_b).unwrap();

    assert_eq!(a, b);
}

/// `optionals_probability = Some(1.0)` forces every optional property to appear in the output.
#[test]
fn test_optionals_probability_one_forces_optional_inclusion() {
    let schema = json!({
        "type": "object",
        "properties": {"opt": {"type": "string"}}
    });
    let mut opts = seeded_opts(1);
    opts.optionals_probability = Some(1.0);

    let value = generate(&schema, &opts).unwrap();
    let obj = value.as_object().unwrap();

    assert!(obj.contains_key("opt"));
}

/// `optionals_probability = Some(0.0)` suppresses every optional property in the output.
#[test]
fn test_optionals_probability_zero_drops_optional() {
    let schema = json!({
        "type": "object",
        "properties": {"opt": {"type": "string"}}
    });
    let mut opts = seeded_opts(1);
    opts.optionals_probability = Some(0.0);

    let value = generate(&schema, &opts).unwrap();
    let obj = value.as_object().unwrap();

    assert!(!obj.contains_key("opt"));
}

/// `max_default_items` caps the number of array items generated when items have a `default`,
/// without underflowing the schema's `minItems`.
#[test]
fn test_max_default_items_caps_array_when_items_have_default() {
    let schema = json!({
        "type": "array",
        "minItems": 2,
        "maxItems": 10,
        "items": { "type": "integer", "default": 7 }
    });
    let mut opts = default_opts();
    opts.max_default_items = Some(3);
    opts.use_default_value = true;
    opts.always_fake_optionals = true;

    let result = generate(&schema, &opts).unwrap();
    let arr = result.as_array().expect("expected array");

    assert_eq!(
        arr.len(),
        3,
        "max_default_items should cap default-driven generation when above minItems"
    );
}

/// `validate_schema_version = true` rejects unknown JSON Schema dialects with a `SchemaError`.
#[test]
fn test_validate_schema_version_rejects_unknown_dialect() {
    let schema = json!({
        "$schema": "http://json-schema.org/draft-99/schema#",
        "type": "object"
    });
    let mut opts = default_opts();
    opts.validate_schema_version = true;

    let result = generate(&schema, &opts);

    assert!(matches!(result, Err(FakerError::SchemaError { .. })));
}

/// `validate_schema_version = true` accepts a known JSON Schema dialect.
#[test]
fn test_validate_schema_version_accepts_known_dialect() {
    let schema = json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "type": "object"
    });
    let mut opts = default_opts();
    opts.validate_schema_version = true;

    let result = generate(&schema, &opts);

    assert!(result.is_ok());
}
