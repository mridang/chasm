//! Tests for `generators/array.rs`.

use crate::common::{default_opts, seeded_opts};
use chasm_faker::{generate, FakerError};
use serde_json::json;

/// `generate()` returns an array of length within `[minItems, maxItems]` when both bounds are set.
#[test]
fn test_returns_array_for_array_type() {
    let schema =
        json!({"type": "array", "minItems": 1, "maxItems": 4, "items": {"type": "integer"}});

    let value = generate(&schema, &default_opts()).unwrap();
    let arr = value.as_array().expect("expected array value");

    assert!((1..=4).contains(&arr.len()));
}

/// `minItems` is respected when generating arrays.
#[test]
fn test_min_items_respected() {
    let schema = json!({"type": "array", "minItems": 3, "items": {"type": "string"}});

    let value = generate(&schema, &default_opts()).unwrap();
    let arr = value.as_array().unwrap();

    assert!(arr.len() >= 3);
}

/// `maxItems` is respected when generating arrays.
#[test]
fn test_max_items_respected() {
    let schema = json!({"type": "array", "maxItems": 2, "items": {"type": "number"}});

    let value = generate(&schema, &default_opts()).unwrap();
    let arr = value.as_array().unwrap();

    assert!(arr.len() <= 2);
}

/// When `minItems` equals `maxItems`, the array has exactly that length.
#[test]
fn test_exact_items_when_min_equals_max() {
    let schema =
        json!({"type": "array", "minItems": 3, "maxItems": 3, "items": {"type": "integer"}});

    let value = generate(&schema, &default_opts()).unwrap();
    let arr = value.as_array().unwrap();

    assert_eq!(arr.len(), 3);
}

/// The `items` schema controls the type of generated array elements.
#[test]
fn test_items_schema_controls_element_type() {
    let schema =
        json!({"type": "array", "minItems": 1, "maxItems": 3, "items": {"type": "boolean"}});

    let value = generate(&schema, &default_opts()).unwrap();

    for item in value.as_array().unwrap() {
        assert!(item.is_boolean());
    }
}

/// `alwaysFakeOptionals` generates maxItems-sized arrays.
#[test]
fn test_always_fake_optionals_fills_to_max() {
    let schema = json!({
        "type": "array",
        "minItems": 0,
        "maxItems": 5,
        "items": {"type": "string", "enum": ["a"]}
    });
    let mut opts = default_opts();
    opts.always_fake_optionals = true;
    opts.use_default_value = true;

    let value = generate(&schema, &opts).unwrap();
    let arr = value.as_array().unwrap();

    assert_eq!(arr.len(), 5);
}

/// `uniqueItems` prevents duplicate values in the generated array.
#[test]
fn test_unique_items_no_duplicates() {
    let schema = json!({
        "type": "array",
        "minItems": 3,
        "maxItems": 3,
        "uniqueItems": true,
        "items": {"type": "integer", "minimum": 0, "maximum": 100}
    });

    let value = generate(&schema, &default_opts()).unwrap();
    let arr = value.as_array().unwrap();

    let mut seen = std::collections::HashSet::new();
    for item in arr {
        assert!(seen.insert(item.to_string()));
    }
}

/// `prefixItems` with `items: false` produces exactly the prefix-length array.
#[test]
fn test_prefix_items_with_items_false() {
    let schema = json!({
        "type": "array",
        "prefixItems": [{"type": "string"}, {"type": "number"}],
        "items": false
    });

    let value = generate(&schema, &default_opts()).unwrap();
    let arr = value.as_array().unwrap();

    assert_eq!(arr.len(), 2);
}

/// `prefixItems` plus an `items` schema fills additional slots up to `minItems`.
#[test]
fn test_prefix_items_filled_with_items_schema() {
    let schema = json!({
        "type": "array",
        "prefixItems": [{"type": "string"}],
        "items": {"type": "boolean"},
        "minItems": 3
    });

    let value = generate(&schema, &default_opts()).unwrap();
    let arr = value.as_array().unwrap();

    assert_eq!(arr.len(), 3);
}

/// `prefixItems`'s first slot produces a string value matching the declared prefix type.
#[test]
fn test_prefix_items_first_slot_is_string() {
    let schema = json!({
        "type": "array",
        "prefixItems": [{"type": "string"}],
        "items": {"type": "boolean"},
        "minItems": 3
    });

    let value = generate(&schema, &default_opts()).unwrap();
    let arr = value.as_array().unwrap();

    assert!(arr[0].is_string());
}

/// `prefixItems` padding slots (beyond the declared prefix) use the `items` schema's type.
#[test]
fn test_prefix_items_padding_slots_are_boolean() {
    let schema = json!({
        "type": "array",
        "prefixItems": [{"type": "string"}],
        "items": {"type": "boolean"},
        "minItems": 3
    });

    let value = generate(&schema, &default_opts()).unwrap();
    let arr = value.as_array().unwrap();

    for slot in &arr[1..] {
        assert!(slot.is_boolean());
    }
}

/// `prefixItems` plus `additionalItems: false` and a `minItems` larger than the prefix errors out.
#[test]
fn test_prefix_items_contradictory_emits_error() {
    let schema = json!({
        "type": "array",
        "prefixItems": [{"type": "integer"}],
        "additionalItems": false,
        "minItems": 3
    });

    let result = generate(&schema, &default_opts());

    assert!(
        matches!(result, Err(FakerError::MissingItems { .. })),
        "expected MissingItems, got {result:?}",
    );
}

/// Prefix items (array form of `items`) emit an integer in the first slot.
#[test]
fn test_prefix_items_array_form_first_is_integer() {
    let schema = json!({
        "type": "array",
        "items": [{"type": "integer"}, {"type": "string"}],
        "minItems": 2
    });

    let value = generate(&schema, &default_opts()).unwrap();
    let arr = value.as_array().unwrap();

    assert!(arr[0].is_number());
}

/// Prefix items (array form of `items`) emit a string in the second slot.
#[test]
fn test_prefix_items_array_form_second_is_string() {
    let schema = json!({
        "type": "array",
        "items": [{"type": "integer"}, {"type": "string"}],
        "minItems": 2
    });

    let value = generate(&schema, &default_opts()).unwrap();
    let arr = value.as_array().unwrap();

    assert!(arr[1].is_string());
}

/// `minContains` forces at least N elements satisfying the `contains` schema.
#[test]
fn test_min_contains_produces_required_matches() {
    let schema = json!({
        "type": "array",
        "items": {"type": "string"},
        "contains": {"type": "integer"},
        "minContains": 3,
        "minItems": 5
    });

    let value = generate(&schema, &default_opts()).unwrap();
    let arr = value.as_array().unwrap();
    let int_count = arr.iter().filter(|v| v.is_i64() || v.is_u64()).count();

    assert!(int_count >= 3);
}

/// Combining `contains` with `items: false` and a positive `minContains` errors out.
#[test]
fn test_contains_with_items_false_emits_error() {
    let schema = json!({
        "type": "array",
        "items": false,
        "contains": {"type": "integer"},
        "minContains": 2
    });

    let result = generate(&schema, &default_opts());

    assert!(
        matches!(result, Err(FakerError::SchemaError { .. })),
        "expected SchemaError, got {result:?}",
    );
}

/// `uniqueItems: true` with an item enum smaller than `minItems` errors out.
#[test]
fn test_unique_items_exhausted_emits_error() {
    let schema = json!({
        "type": "array",
        "items": {"enum": [1]},
        "uniqueItems": true,
        "minItems": 5
    });

    let result = generate(&schema, &default_opts());

    assert!(
        matches!(result, Err(FakerError::MissingItems { .. })),
        "expected MissingItems, got {result:?}",
    );
}

/// `maxContains: 0` combined with the implicit `minContains: 1` default surfaces a schema error.
#[test]
fn test_contains_max_zero_contradicts_min() {
    let schema = json!({
        "type": "array",
        "items": {"type": "string"},
        "contains": {"type": "integer"},
        "maxContains": 0
    });

    let result = generate(&schema, &default_opts());

    assert!(
        matches!(result, Err(FakerError::SchemaError { .. })),
        "expected SchemaError, got {result:?}",
    );
}

/// An array `items` schema with an `x-faker: helpers.arrayElement` object dispatches per element.
#[test]
fn test_items_x_faker_helpers_array_element() {
    let schema = json!({
        "type": "array",
        "minItems": 5,
        "maxItems": 5,
        "items": {
            "type": "string",
            "x-faker": {"helpers.arrayElement": ["yard", "foot"]}
        }
    });

    let value = generate(&schema, &seeded_opts(42)).unwrap();
    let arr = value.as_array().unwrap();

    for entry in arr.iter() {
        let s = entry.as_str().unwrap();
        assert!(s == "yard" || s == "foot");
    }
}

/// Fixture: per-slot `x-faker: helpers.arrayElement` tuple schema with two slots.
fn prefix_items_per_slot_x_faker_schema() -> serde_json::Value {
    json!({
        "type": "array",
        "prefixItems": [
            {"type": "string", "x-faker": {"helpers.arrayElement": ["alpha", "beta"]}},
            {"type": "string", "x-faker": {"helpers.arrayElement": ["one", "two"]}}
        ],
        "minItems": 2,
        "maxItems": 2,
        "additionalItems": false
    })
}

/// The first prefix slot's `x-faker: helpers.arrayElement` picks from its own allowed values.
#[test]
fn test_prefix_items_first_slot_x_faker_picks_alpha_or_beta() {
    let schema = prefix_items_per_slot_x_faker_schema();

    let value = generate(&schema, &seeded_opts(7)).unwrap();
    let arr = value.as_array().unwrap();
    let a = arr[0].as_str().unwrap();

    assert!(a == "alpha" || a == "beta");
}

/// The second prefix slot's `x-faker: helpers.arrayElement` picks from its own allowed values.
#[test]
fn test_prefix_items_second_slot_x_faker_picks_one_or_two() {
    let schema = prefix_items_per_slot_x_faker_schema();

    let value = generate(&schema, &seeded_opts(7)).unwrap();
    let arr = value.as_array().unwrap();
    let b = arr[1].as_str().unwrap();

    assert!(b == "one" || b == "two");
}

/// A tuple schema with `prefixItems`, fixed min/max and `additionalItems: false` produces exactly the tuple.
#[test]
fn test_tuple_prefix_items_no_additional_dynamic() {
    let schema = json!({
        "type": "array",
        "prefixItems": [{"type": "string"}, {"type": "string"}],
        "minItems": 2,
        "maxItems": 2,
        "additionalItems": false
    });
    let mut opts = seeded_opts(13);
    opts.use_examples_value = false;

    let value = generate(&schema, &opts).unwrap();
    let arr = value.as_array().unwrap();

    assert_eq!(arr.len(), 2);
}

/// `prefixItems` + non-array `items` schema with `minItems` exceeding the prefix pads from `items`.
#[test]
fn test_prefix_items_additional_items_pads_array() {
    let schema = json!({
        "type": "array",
        "prefixItems": [{"type": "string"}, {"type": "integer"}],
        "items": {"type": "boolean"},
        "minItems": 5
    });

    let value = generate(&schema, &seeded_opts(42)).unwrap();
    let arr = value.as_array().unwrap();

    assert_eq!(arr.len(), 5);
}

/// `prefixItems` + non-array `items` schema with `minItems` exceeding the prefix pads with boolean elements.
#[test]
fn test_prefix_items_additional_items_pad_type_is_boolean() {
    let schema = json!({
        "type": "array",
        "prefixItems": [{"type": "string"}, {"type": "integer"}],
        "items": {"type": "boolean"},
        "minItems": 5
    });

    let value = generate(&schema, &seeded_opts(42)).unwrap();
    let arr = value.as_array().unwrap();

    for entry in arr.iter().skip(2) {
        assert!(entry.is_boolean());
    }
}

/// `items` (array form) plus `additionalItems` schema pads from `additionalItems` past the tuple.
#[test]
fn test_items_array_form_with_additional_items_pads_with_additional() {
    let schema = json!({
        "type": "array",
        "items": [{"type": "string"}, {"type": "integer"}],
        "additionalItems": {"type": "boolean"},
        "minItems": 4
    });

    let value = generate(&schema, &seeded_opts(7)).unwrap();
    let arr = value.as_array().unwrap();

    for entry in arr.iter().skip(2) {
        assert!(entry.is_boolean());
    }
}

/// A `minItems` far above the safety cap is clamped or surfaces a schema error.
#[test]
fn test_min_items_capped_at_safety_limit() {
    let schema = json!({
        "type": "array",
        "minItems": 1u64 << 20,
        "items": {"type": "integer"}
    });

    let result = generate(&schema, &default_opts());

    match result {
        Ok(value) => {
            let arr = value.as_array().expect("expected array value");
            assert!(arr.len() <= 1 << 16);
        }
        Err(FakerError::SchemaError { message, .. }) => assert!(message.contains("capped")),
        Err(other) => panic!("unexpected error: {:?}", other),
    }
}

/// When `max_default_items` is smaller than `minItems`, the generator
/// honours `minItems` rather than producing a spec-violating short array.
#[test]
fn test_max_default_items_does_not_underflow_min_items() {
    let schema = json!({
        "type": "array",
        "minItems": 5,
        "items": { "type": "integer", "default": 7 }
    });
    let mut opts = default_opts();
    opts.max_default_items = Some(2);
    opts.use_default_value = true;

    let result = generate(&schema, &opts).unwrap();
    let arr = result.as_array().expect("expected array");

    assert!(
        arr.len() >= 5,
        "max_default_items must not underflow minItems; got len={}",
        arr.len()
    );
}
