//! Tests for `generators/object.rs`.

use crate::common::{default_opts, seeded_opts};
use chasm_faker::{generate, FakerError};
use serde_json::json;

/// `generate()` returns an object when the schema declares `type: "object"`.
#[test]
fn test_returns_object_for_object_type() {
    let schema = json!({"type": "object"});

    let value = generate(&schema, &default_opts()).unwrap();

    assert!(value.is_object());
}

/// The required `name` property is always present in the generated object.
#[test]
fn test_required_property_name_present() {
    let schema = required_name_and_age_schema();

    let value = generate(&schema, &default_opts()).unwrap();
    let obj = value.as_object().unwrap();

    assert!(obj.contains_key("name"));
}

/// The required `age` property is always present in the generated object.
#[test]
fn test_required_property_age_present() {
    let schema = required_name_and_age_schema();

    let value = generate(&schema, &default_opts()).unwrap();
    let obj = value.as_object().unwrap();

    assert!(obj.contains_key("age"));
}

/// Fixture: object schema with two required scalar properties.
fn required_name_and_age_schema() -> serde_json::Value {
    json!({
        "type": "object",
        "properties": {
            "name": {"type": "string"},
            "age": {"type": "integer"}
        },
        "required": ["name", "age"]
    })
}

/// `required_only` keeps the declared required property in the generated object.
#[test]
fn test_required_only_keeps_required_field() {
    let schema = required_with_optional_schema();
    let mut opts = default_opts();
    opts.required_only = true;

    let value = generate(&schema, &opts).unwrap();
    let obj = value.as_object().unwrap();

    assert!(obj.contains_key("required_field"));
}

/// `required_only` skips non-required properties from the generated object.
#[test]
fn test_required_only_skips_optionals() {
    let schema = required_with_optional_schema();
    let mut opts = default_opts();
    opts.required_only = true;

    let value = generate(&schema, &opts).unwrap();
    let obj = value.as_object().unwrap();

    assert!(!obj.contains_key("optional_field"));
}

/// Fixture: object schema with one required and one optional property.
fn required_with_optional_schema() -> serde_json::Value {
    json!({
        "type": "object",
        "properties": {
            "required_field": {"type": "string"},
            "optional_field": {"type": "string"}
        },
        "required": ["required_field"]
    })
}

/// `always_fake_optionals` emits an object whose key set equals the full declared property set.
#[test]
fn test_always_fake_optionals_emits_all_keys() {
    let schema = json!({
        "type": "object",
        "properties": {
            "a": {"type": "string"},
            "b": {"type": "string"},
            "c": {"type": "string"}
        }
    });
    let mut opts = default_opts();
    opts.always_fake_optionals = true;

    let value = generate(&schema, &opts).unwrap();
    let obj = value.as_object().unwrap();
    let mut keys: Vec<&str> = obj.keys().map(|s| s.as_str()).collect();
    keys.sort();

    assert_eq!(keys, vec!["a", "b", "c"]);
}

/// `ignore_properties` removes the listed property keys from the generated object.
#[test]
fn test_ignore_properties_skips_listed_keys() {
    let schema = ignore_properties_schema();
    let mut opts = default_opts();
    opts.ignore_properties = vec!["skip_me".to_string()];

    let value = generate(&schema, &opts).unwrap();
    let obj = value.as_object().unwrap();

    assert!(!obj.contains_key("skip_me"));
}

/// `ignore_properties` leaves un-listed required keys in place.
#[test]
fn test_ignore_properties_keeps_unlisted_required_key() {
    let schema = ignore_properties_schema();
    let mut opts = default_opts();
    opts.ignore_properties = vec!["skip_me".to_string()];

    let value = generate(&schema, &opts).unwrap();
    let obj = value.as_object().unwrap();

    assert!(obj.contains_key("keep"));
}

/// Fixture: object schema with two required properties, one of which is the target of `ignore_properties`.
fn ignore_properties_schema() -> serde_json::Value {
    json!({
        "type": "object",
        "properties": {
            "keep": {"type": "string"},
            "skip_me": {"type": "string"}
        },
        "required": ["keep", "skip_me"]
    })
}

/// `additionalProperties` schema generates extra entries when `minProperties` requires them.
#[test]
fn test_additional_properties_schema_satisfies_min_properties() {
    let schema = json!({
        "type": "object",
        "properties": {"name": {"type": "string"}},
        "required": ["name"],
        "additionalProperties": {"type": "number"},
        "minProperties": 3
    });

    let value = generate(&schema, &default_opts()).unwrap();
    let obj = value.as_object().unwrap();

    assert!(obj.len() >= 3);
}

/// Nested objects are walked so the inner schema's `street` required key is emitted.
#[test]
fn test_nested_objects_emit_inner_street() {
    let schema = nested_address_schema();

    let value = generate(&schema, &default_opts()).unwrap();
    let addr = value.get("address").unwrap().as_object().unwrap();

    assert!(addr.contains_key("street"));
}

/// Nested objects are walked so the inner schema's `city` required key is emitted.
#[test]
fn test_nested_objects_emit_inner_city() {
    let schema = nested_address_schema();

    let value = generate(&schema, &default_opts()).unwrap();
    let addr = value.get("address").unwrap().as_object().unwrap();

    assert!(addr.contains_key("city"));
}

/// Fixture: parent object whose required `address` property is itself a two-required-key object.
fn nested_address_schema() -> serde_json::Value {
    json!({
        "type": "object",
        "properties": {
            "address": {
                "type": "object",
                "properties": {
                    "street": {"type": "string"},
                    "city": {"type": "string"}
                },
                "required": ["street", "city"]
            }
        },
        "required": ["address"]
    })
}

/// `optionals_probability` of 1.0 emits an object whose key set equals the declared properties.
#[test]
fn test_optionals_probability_one_emits_all_optionals() {
    let schema = json!({
        "type": "object",
        "properties": {
            "a": {"type": "string"},
            "b": {"type": "string"},
            "c": {"type": "string"}
        }
    });
    let mut opts = default_opts();
    opts.optionals_probability = Some(1.0);

    let value = generate(&schema, &opts).unwrap();
    let obj = value.as_object().unwrap();
    let mut keys: Vec<&str> = obj.keys().map(|s| s.as_str()).collect();
    keys.sort();

    assert_eq!(keys, vec!["a", "b", "c"]);
}

/// `optionals_probability` of 0.0 keeps required properties in the generated object.
#[test]
fn test_optionals_probability_zero_keeps_required() {
    let schema = required_with_optional_schema();
    let mut opts = default_opts();
    opts.optionals_probability = Some(0.0);

    let value = generate(&schema, &opts).unwrap();
    let obj = value.as_object().unwrap();

    assert!(obj.contains_key("required_field"));
}

/// `optionals_probability` of 0.0 drops optional properties from the generated object.
#[test]
fn test_optionals_probability_zero_skips_optionals() {
    let schema = required_with_optional_schema();
    let mut opts = default_opts();
    opts.optionals_probability = Some(0.0);

    let value = generate(&schema, &opts).unwrap();
    let obj = value.as_object().unwrap();

    assert!(!obj.contains_key("optional_field"));
}

/// `propertyNames.pattern` constrains every generated key to match.
#[test]
fn test_property_names_pattern_constrains_keys() {
    let schema = json!({
        "type": "object",
        "additionalProperties": {"type": "integer"},
        "propertyNames": {"pattern": "^[a-z]{3}$"},
        "minProperties": 2
    });

    let value = generate(&schema, &default_opts()).unwrap();
    let obj = value.as_object().unwrap();

    let re = regex::Regex::new("^[a-z]{3}$").unwrap();
    for k in obj.keys() {
        assert!(re.is_match(k));
    }
}

/// The `additionalProperties` fallback path used to fill up to `minProperties` honours `propertyNames.pattern`.
#[test]
fn test_property_names_pattern_constrains_additional_keys() {
    let schema = json!({
        "type": "object",
        "additionalProperties": {"type": "string"},
        "propertyNames": {"pattern": "^[a-z]{3,8}$"},
        "minProperties": 5
    });

    let value = generate(&schema, &default_opts()).unwrap();
    let obj = value.as_object().unwrap();

    let re = regex::Regex::new("^[a-z]{3,8}$").unwrap();
    for k in obj.keys() {
        assert!(re.is_match(k));
    }
}

/// `propertyNames` with `enum` constrains generated keys to the enum set.
#[test]
fn test_property_names_enum_constrains_keys() {
    let schema = json!({
        "type": "object",
        "minProperties": 2,
        "propertyNames": {"enum": ["red", "green", "blue"]},
        "additionalProperties": {"type": "integer"}
    });
    let opts = seeded_opts(1);

    let value = generate(&schema, &opts).unwrap();
    let obj = value.as_object().expect("object");

    let allowed = ["red", "green", "blue"];
    for key in obj.keys() {
        assert!(allowed.contains(&key.as_str()), "key {:?} not in enum", key);
    }
}

/// `propertyNames` with `const` constrains every generated key to that value.
#[test]
fn test_property_names_const_constrains_keys() {
    let schema = json!({
        "type": "object",
        "minProperties": 1,
        "propertyNames": {"const": "only-key"},
        "additionalProperties": {"type": "integer"}
    });
    let opts = seeded_opts(2);

    let value = generate(&schema, &opts).unwrap();
    let obj = value.as_object().expect("object");

    for key in obj.keys() {
        assert_eq!(key, "only-key", "all keys must equal const value");
    }
}

/// `propertyNames` with `format: uuid` produces UUID-shaped keys.
#[test]
fn test_property_names_format_uuid_constrains_keys() {
    let schema = json!({
        "type": "object",
        "minProperties": 2,
        "propertyNames": {"format": "uuid"},
        "additionalProperties": {"type": "integer"}
    });
    let opts = seeded_opts(3);

    let value = generate(&schema, &opts).unwrap();
    let obj = value.as_object().expect("object");

    let re = regex::Regex::new(
        "^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$",
    )
    .unwrap();
    for key in obj.keys() {
        assert!(re.is_match(key), "key {:?} not UUID-shaped", key);
    }
}

/// `propertyNames.pattern: ^$` only permits the empty-string key.
#[test]
fn test_property_names_empty_pattern_only_empty_keys() {
    let schema = json!({
        "type": "object",
        "additionalProperties": {"type": "integer"},
        "propertyNames": {"pattern": "^$"},
        "minProperties": 1
    });
    let mut opts = default_opts();
    opts.fill_properties = true;

    let value = generate(&schema, &opts).unwrap();
    let obj = value.as_object().unwrap();

    for k in obj.keys() {
        assert_eq!(k, "");
    }
}

/// `patternProperties` resolution is deterministic when a key matches multiple patterns.
#[test]
fn test_pattern_properties_iteration_order_stable() {
    let schema = json!({
        "type": "object",
        "patternProperties": {
            "x": {"type": "string"},
            "y": {"type": "integer"}
        },
        "required": ["xy"]
    });

    let a = generate(&schema, &seeded_opts(42)).unwrap();
    let b = generate(&schema, &seeded_opts(42)).unwrap();

    assert_eq!(a, b);
}

/// `patternProperties` last-wins on alphabetical pattern order for a matching key.
#[test]
fn test_pattern_properties_last_alphabetical_wins() {
    let schema = json!({
        "type": "object",
        "patternProperties": {
            "x": {"type": "string"},
            "y": {"type": "integer"}
        },
        "required": ["xy"]
    });

    let value = generate(&schema, &seeded_opts(42)).unwrap();
    let val = value.get("xy").unwrap();

    assert!(val.is_i64() || val.is_u64());
}

/// A `maxProperties` strictly less than the `required` count surfaces a schema error.
#[test]
fn test_max_properties_less_than_required_errors() {
    let schema = json!({
        "type": "object",
        "properties": {
            "a": {"type": "string"},
            "b": {"type": "string"}
        },
        "required": ["a", "b"],
        "maxProperties": 0
    });

    let result = generate(&schema, &default_opts());

    assert!(
        matches!(result, Err(FakerError::SchemaError { .. })),
        "expected SchemaError, got {result:?}",
    );
}

/// `omit_nulls` removes optional properties that would otherwise resolve to `null`.
/// At least one seed in `0..32` must emit the optional `value` key, and no emitted
/// value is ever `null` — the absence of the key is the legitimate outcome only
/// when the generator chose the `null` branch and `omit_nulls` stripped it.
#[test]
fn test_omit_nulls_removes_null_optional_property() {
    let schema = json!({
        "type": "object",
        "properties": {
            "value": {"type": ["string", "null"]}
        }
    });
    let mut opts = default_opts();
    opts.always_fake_optionals = true;
    opts.omit_nulls = true;

    let mut saw_value = false;
    for seed in 0u64..32 {
        opts.seed = Some(seed);
        let value = generate(&schema, &opts).unwrap();
        let obj = value.as_object().unwrap();
        if let Some(v) = obj.get("value") {
            saw_value = true;
            assert!(
                !v.is_null(),
                "omit_nulls should drop null values, seed={seed}"
            );
        }
    }
    assert!(
        saw_value,
        "no seed in 0..32 produced the optional `value` key"
    );
}

/// `omit_nulls` preserves required properties whose generated value is `null`.
#[test]
fn test_omit_nulls_preserves_required_null_property() {
    let schema = json!({
        "type": "object",
        "properties": {"value": {"type": "null"}},
        "required": ["value"]
    });
    let mut opts = seeded_opts(42);
    opts.omit_nulls = true;

    let value = generate(&schema, &opts).unwrap();
    let obj = value.as_object().unwrap();

    assert!(obj.contains_key("value"));
}

/// Two `autoIncrement` schemas at different positions maintain independent counters.
#[test]
fn test_auto_increment_keys_distinct_across_positions() {
    let schema = json!({
        "type": "object",
        "properties": {
            "a": {"type": "integer", "autoIncrement": true, "initialOffset": 100},
            "b": {"type": "integer", "autoIncrement": true, "initialOffset": 100}
        },
        "required": ["a", "b"]
    });

    let result = generate(&schema, &seeded_opts(13)).unwrap();

    assert_eq!(result["a"].as_i64(), Some(100));
}

/// `autoIncrement` initialises the second declared property from its own offset.
#[test]
fn test_auto_increment_second_key_uses_own_initial() {
    let schema = json!({
        "type": "object",
        "properties": {
            "a": {"type": "integer", "autoIncrement": true, "initialOffset": 100},
            "b": {"type": "integer", "autoIncrement": true, "initialOffset": 100}
        },
        "required": ["a", "b"]
    });

    let result = generate(&schema, &seeded_opts(13)).unwrap();

    assert_eq!(result["b"].as_i64(), Some(100));
}

/// A nested property declared `nullable: true` emits null at least once across seeds.
#[test]
fn test_nullable_property_sometimes_emits_null() {
    let schema = json!({
        "type": "object",
        "properties": {"name": {"type": "string", "nullable": true}},
        "required": ["name"]
    });

    let mut saw_null = false;
    for seed in 0u64..100 {
        let value = generate(&schema, &seeded_opts(seed)).unwrap();
        if value.get("name").unwrap().is_null() {
            saw_null = true;
            break;
        }
    }

    assert!(saw_null);
}

/// A nested property declared `nullable: true` also emits a string at least once across seeds.
#[test]
fn test_nullable_property_sometimes_emits_string() {
    let schema = json!({
        "type": "object",
        "properties": {"name": {"type": "string", "nullable": true}},
        "required": ["name"]
    });

    let mut saw_string = false;
    for seed in 0u64..100 {
        let value = generate(&schema, &seeded_opts(seed)).unwrap();
        if value.get("name").unwrap().is_string() {
            saw_string = true;
            break;
        }
    }

    assert!(saw_string);
}

/// `dependentRequired` adds `billing_address` to the required set whenever `credit_card` is present.
#[test]
fn test_dependent_required_adds_dependency_when_trigger_present() {
    let schema = json!({
        "type": "object",
        "properties": {
            "credit_card": {"type": "string"},
            "billing_address": {"type": "string"}
        },
        "required": ["credit_card"],
        "dependentRequired": {"credit_card": ["billing_address"]}
    });

    let value = generate(&schema, &seeded_opts(7)).unwrap();
    let obj = value.as_object().unwrap();

    assert!(obj.contains_key("billing_address"));
}

/// `dependentSchemas` applies an additional sub-schema when the trigger property is present.
///
/// When `kind` is present, the dependent sub-schema requires `value`; we satisfy the trigger
/// via the outer `required` list so the dependency must fire.
#[test]
fn test_dependent_schemas_applies_when_trigger_present() {
    let schema = json!({
        "type": "object",
        "properties": {
            "kind": {"type": "string"},
            "value": {"type": "string"}
        },
        "required": ["kind"],
        "dependentSchemas": {
            "kind": {"required": ["value"]}
        }
    });

    let value = generate(&schema, &seeded_opts(7)).unwrap();
    let obj = value.as_object().unwrap();

    assert!(obj.contains_key("value"));
}

/// Legacy `dependencies` keyword (array form) is treated like `dependentRequired`.
#[test]
fn test_legacy_dependencies_array_form_adds_required() {
    let schema = json!({
        "type": "object",
        "properties": {
            "credit_card": {"type": "string"},
            "billing_address": {"type": "string"}
        },
        "required": ["credit_card"],
        "dependencies": {"credit_card": ["billing_address"]}
    });

    let value = generate(&schema, &seeded_opts(7)).unwrap();
    let obj = value.as_object().unwrap();

    assert!(obj.contains_key("billing_address"));
}

/// `additionalProperties: false` restricts the emitted object to declared properties only.
#[test]
fn test_additional_properties_false_restricts_keys_to_declared() {
    let schema = json!({
        "type": "object",
        "properties": {"a": {"type": "string"}, "b": {"type": "string"}},
        "required": ["a", "b"],
        "additionalProperties": false
    });

    let value = generate(&schema, &seeded_opts(7)).unwrap();
    let obj = value.as_object().unwrap();

    for key in obj.keys() {
        assert!(key == "a" || key == "b", "unexpected key `{}`", key);
    }
}

/// A `not` constraint inside an object property rejects the disallowed concrete value.
#[test]
fn test_not_inside_object_property_excludes_value() {
    let schema = json!({
        "type": "object",
        "properties": {
            "v": {
                "type": "string",
                "minLength": 1,
                "maxLength": 5,
                "not": {"const": "forbid"}
            }
        },
        "required": ["v"]
    });

    for seed in 0u64..20 {
        let value = generate(&schema, &seeded_opts(seed)).unwrap();
        let v = value.get("v").unwrap().as_str().unwrap();
        assert_ne!(v, "forbid", "seed={seed} produced the forbidden value");
    }
}

/// `apply_prop_aliases` produces deterministic output even when multiple
/// `from` keys map to the same `to` key. Iteration order is established by
/// sorted alias keys.
#[test]
fn test_apply_prop_aliases_deterministic_with_many_to_one_mapping() {
    let schema = json!({
        "type": "object",
        "properties": {
            "foo": {"type": "string", "const": "FOO"},
            "bar": {"type": "string", "const": "BAR"}
        },
        "required": ["foo", "bar"]
    });
    let mut opts = seeded_opts(1);
    let mut aliases = std::collections::HashMap::new();
    aliases.insert("foo".to_string(), "unified".to_string());
    aliases.insert("bar".to_string(), "unified".to_string());
    opts.prop_aliases = aliases;

    let first = generate(&schema, &opts).unwrap();
    for i in 1..10 {
        let next = generate(&schema, &opts).unwrap();
        assert_eq!(
            next, first,
            "iteration {} diverged: {:?} vs {:?}",
            i, next, first
        );
    }
}

/// A `type: ["string", "null"]` property emits a literal JSON null on at least one seed.
#[test]
fn test_object_property_nullable_union_emits_json_null() {
    let schema = json!({
        "type": "object",
        "required": ["price_unit"],
        "properties": {
            "price_unit": {"type": ["string", "null"], "format": "currency"}
        }
    });

    let mut saw_null = false;
    for seed in 0u64..40 {
        let value = generate(&schema, &seeded_opts(seed)).unwrap();
        let v = value.get("price_unit").unwrap();
        assert!(v != &json!("string"));
        if v.is_null() {
            saw_null = true;
        }
    }

    assert!(saw_null);
}
