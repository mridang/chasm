//! Tests for `generators/composition.rs`.

use crate::common::{default_opts, seeded_opts};
use chasm_faker::{generate, FakerError};
use serde_json::{json, Value};

/// `allOf` merges the first sub-schema's `a` property into the generated object.
#[test]
fn test_all_of_merges_first_property() {
    let schema = all_of_two_required_schema();

    let value = generate(&schema, &default_opts()).unwrap();
    let obj = value.as_object().unwrap();

    assert!(obj.contains_key("a"));
}

/// `allOf` merges the second sub-schema's `b` property into the generated object.
#[test]
fn test_all_of_merges_second_property() {
    let schema = all_of_two_required_schema();

    let value = generate(&schema, &default_opts()).unwrap();
    let obj = value.as_object().unwrap();

    assert!(obj.contains_key("b"));
}

/// Fixture: `allOf` schema where each sub-schema requires a distinct property.
fn all_of_two_required_schema() -> serde_json::Value {
    json!({
        "allOf": [
            {"type": "object", "properties": {"a": {"type": "string"}}, "required": ["a"]},
            {"type": "object", "properties": {"b": {"type": "integer"}}, "required": ["b"]}
        ]
    })
}

/// `anyOf` produces a value matching at least one of the listed sub-schemas.
#[test]
fn test_any_of_picks_one() {
    let schema = json!({
        "anyOf": [
            {"type": "integer"},
            {"type": "array"}
        ]
    });

    let value = generate(&schema, &default_opts()).unwrap();

    assert!(value.is_number() || value.is_array());
}

/// `oneOf` picks exactly one sub-schema and emits a value matching that branch.
#[test]
fn test_one_of_picks_one() {
    let schema = json!({
        "oneOf": [{"type": "boolean"}, {"type": "null"}]
    });

    let value = generate(&schema, &default_opts()).unwrap();

    assert!(value.is_boolean() || value.is_null());
}

/// `allOf` combining `type: string` with an `enum` selects from the enum.
#[test]
fn test_all_of_type_and_enum_picks_from_enum() {
    let schema = json!({
        "allOf": [
            {"type": "string"},
            {"enum": ["a", "value"]}
        ]
    });

    let value = generate(&schema, &default_opts()).unwrap();

    assert!(value == json!("a") || value == json!("value"));
}

/// `if`/`then`/`else` produces a value from one of the two branches.
#[test]
fn test_if_then_else_produces_branch_value() {
    let schema = json!({
        "if": {"type": "string"},
        "then": {"type": "integer"},
        "else": {"type": "boolean"}
    });

    let value = generate(&schema, &default_opts()).unwrap();

    assert!(value.is_number() || value.is_boolean());
}

/// `examples` takes precedence over `anyOf` when `use_examples_value` is enabled.
#[test]
fn test_examples_override_any_of() {
    let schema = json!({
        "anyOf": [{"type": "string"}],
        "examples": ["abc"]
    });
    let mut opts = default_opts();
    opts.use_examples_value = true;

    let value = generate(&schema, &opts).unwrap();

    assert_eq!(value, json!("abc"));
}

/// `examples` overrides a nullable union plus `default: null` when both flags are on.
#[test]
fn test_examples_override_default_null_in_nullable_union() {
    let schema = json!({
        "anyOf": [{"type": "string"}, {"type": "null"}],
        "default": null,
        "examples": ["xyz"]
    });
    let mut opts = default_opts();
    opts.use_examples_value = true;
    opts.use_default_value = true;

    let value = generate(&schema, &opts).unwrap();

    assert_eq!(value, json!("xyz"));
}

/// Recursive `allOf` with `$ref` emits a `parent` and that `parent` is never an
/// empty object violating `required` at the depth cutoff.
#[test]
fn test_recursive_all_of_omits_optional_at_depth_cutoff() {
    let schema = json!({
        "type": "object",
        "properties": {
            "name": {"type": "string"},
            "parent": {"allOf": [{"$ref": "#"}]}
        },
        "required": ["name"]
    });
    let mut opts = default_opts();
    opts.max_depth = 3;
    opts.always_fake_optionals = true;

    let value = generate(&schema, &opts).unwrap();
    let obj = value.as_object().unwrap();

    let parent = obj.get("parent").expect("parent must be present");
    assert!(!parent.as_object().map(|m| m.is_empty()).unwrap_or(false));
}

/// A recursive `oneOf` branch including `null` always emits a `parent` and that
/// `parent` is never an empty object at the depth cutoff.
#[test]
fn test_recursive_one_of_with_null_terminal() {
    let schema = json!({
        "type": "object",
        "properties": {
            "name": {"type": "string"},
            "parent": {"oneOf": [{"type": "null"}, {"$ref": "#"}]}
        },
        "required": ["name"]
    });
    let mut opts = default_opts();
    opts.max_depth = 3;
    opts.always_fake_optionals = true;

    let value = generate(&schema, &opts).unwrap();
    let obj = value.as_object().unwrap();

    let parent = obj.get("parent").expect("parent must be present");
    let bad = parent.as_object().map(|m| m.is_empty()).unwrap_or(false);
    assert!(!bad);
}

/// Nested `$ref` inside `allOf` resolves so the referenced schema's `street` appears.
#[test]
fn test_all_of_with_ref_emits_street() {
    let schema = all_of_with_ref_address_schema();

    let value = generate(&schema, &default_opts()).unwrap();
    let shipping = value.get("shipping").unwrap().as_object().unwrap();

    assert!(shipping.contains_key("street"));
}

/// Nested `$ref` inside `allOf` resolves so the referenced schema's `city` appears.
#[test]
fn test_all_of_with_ref_emits_city() {
    let schema = all_of_with_ref_address_schema();

    let value = generate(&schema, &default_opts()).unwrap();
    let shipping = value.get("shipping").unwrap().as_object().unwrap();

    assert!(shipping.contains_key("city"));
}

/// The inline sibling sub-schema's `type` property merges alongside the referenced address fields.
#[test]
fn test_all_of_with_ref_emits_inline_sibling_type() {
    let schema = all_of_with_ref_address_schema();

    let value = generate(&schema, &default_opts()).unwrap();
    let shipping = value.get("shipping").unwrap().as_object().unwrap();

    assert!(shipping.contains_key("type"));
}

/// Fixture: `allOf` mixing a `$ref` to an address `$def` with an inline sibling sub-schema.
fn all_of_with_ref_address_schema() -> serde_json::Value {
    json!({
        "$defs": {
            "address": {
                "type": "object",
                "properties": {
                    "street": {"type": "string"},
                    "city": {"type": "string"}
                },
                "required": ["street", "city"]
            }
        },
        "type": "object",
        "properties": {
            "shipping": {
                "allOf": [
                    {"$ref": "#/$defs/address"},
                    {"properties": {"type": {"enum": ["residential", "business"]}}, "required": ["type"]}
                ]
            }
        },
        "required": ["shipping"]
    })
}

/// `anyOf` falls through to the next branch when the first errors out.
#[test]
fn test_any_of_retries_on_failed_branch() {
    let schema = json!({
        "anyOf": [
            {"type": "bogus-type"},
            {"type": "string", "const": "fallback"}
        ]
    });
    let mut opts = seeded_opts(1);
    opts.fail_on_invalid_type = true;

    let value = generate(&schema, &opts).unwrap();

    assert_eq!(value, json!("fallback"));
}

/// `oneOf` walks the chosen branch with optional inclusion disabled so only required keys appear.
fn one_of_exclusivity_schema() -> serde_json::Value {
    json!({
        "oneOf": [
            {
                "type": "object",
                "properties": {
                    "kind": {"type": "string", "const": "a"},
                    "shared": {"type": "string"}
                },
                "required": ["kind"]
            },
            {
                "type": "object",
                "properties": {
                    "kind": {"type": "string", "const": "b"},
                    "shared": {"type": "string"}
                },
                "required": ["kind"]
            }
        ]
    })
}

/// `oneOf` exclusivity: the chosen branch's required `kind` property is present in the result.
#[test]
fn test_one_of_exclusivity_preserves_required_kind() {
    let schema = one_of_exclusivity_schema();
    let mut opts = seeded_opts(7);
    opts.always_fake_optionals = true;

    let value = generate(&schema, &opts).unwrap();
    let obj = value.as_object().unwrap();

    assert!(obj.contains_key("kind"));
}

/// `oneOf` exclusivity: the optional `shared` property is omitted because branch-level
/// `requiredOnly`-style exclusivity overrides the caller's `always_fake_optionals` setting
/// for the chosen branch.
#[test]
fn test_one_of_exclusivity_drops_optional_shared() {
    let schema = one_of_exclusivity_schema();
    let mut opts = seeded_opts(7);
    opts.always_fake_optionals = true;

    let value = generate(&schema, &opts).unwrap();
    let obj = value.as_object().unwrap();

    assert!(!obj.contains_key("shared"));
}

/// Nested `allOf` arrays concatenate so the first inner sub-schema's `a` key surfaces.
#[test]
fn test_nested_all_of_concatenates_a() {
    let schema = nested_all_of_schema();

    let value = generate(&schema, &default_opts()).unwrap();
    let obj = value.as_object().unwrap();

    assert!(obj.contains_key("a"));
}

/// Nested `allOf` arrays concatenate so the second inner sub-schema's `b` key surfaces.
#[test]
fn test_nested_all_of_concatenates_b() {
    let schema = nested_all_of_schema();

    let value = generate(&schema, &default_opts()).unwrap();
    let obj = value.as_object().unwrap();

    assert!(obj.contains_key("b"));
}

/// Fixture: two-level nested `allOf` where each inner branch requires a distinct key.
fn nested_all_of_schema() -> serde_json::Value {
    json!({
        "allOf": [
            {"allOf": [{"type": "object", "properties": {"a": {"type": "string"}}, "required": ["a"]}]},
            {"allOf": [{"type": "object", "properties": {"b": {"type": "string"}}, "required": ["b"]}]}
        ]
    })
}

/// `anyOf` surfaces `AllBranchesFailed` when every branch fails to generate.
#[test]
fn test_any_of_all_branches_fail_returns_error() {
    let schema = json!({
        "anyOf": [
            {"type": "bogus-one"},
            {"type": "bogus-two"}
        ]
    });
    let mut opts = seeded_opts(3);
    opts.fail_on_invalid_type = true;

    let err = generate(&schema, &opts).unwrap_err();

    assert!(matches!(
        err,
        FakerError::AllBranchesFailed {
            keyword: "anyOf",
            ..
        }
    ));
}

/// `oneOf` constrains an outer `enum` so only matching values are emitted.
#[test]
fn test_composition_runs_before_enum_filtering() {
    let schema = json!({
        "enum": [1, 2, 3],
        "oneOf": [{"const": 1}, {"const": 3}]
    });

    for seed in 0..16u64 {
        let value = generate(&schema, &seeded_opts(seed)).unwrap();
        let n = value.as_i64().unwrap();
        assert!(n == 1 || n == 3);
    }
}

/// An `anyOf` with more than the branch cap still generates a string (every branch is `type: string`).
#[test]
fn test_any_of_above_branch_cap_generates_string() {
    let mut branches = Vec::with_capacity(100);
    for _ in 0..100 {
        branches.push(json!({"type": "string"}));
    }
    let schema = json!({"anyOf": branches});

    let value = generate(&schema, &seeded_opts(0)).unwrap();

    assert!(value.is_string());
}

/// A `oneOf` with more than the branch cap still generates a string (every branch is `type: string`).
#[test]
fn test_one_of_above_branch_cap_generates_string() {
    let mut branches = Vec::with_capacity(100);
    for _ in 0..100 {
        branches.push(json!({"type": "string"}));
    }
    let schema = json!({"oneOf": branches});

    let value = generate(&schema, &seeded_opts(0)).unwrap();

    assert!(value.is_string());
}

/// `anyOf` is deterministic: two calls with the same seed produce identical values.
#[test]
fn test_any_of_deterministic_under_same_seed() {
    let schema = json!({
        "anyOf": [
            {"type": "string"},
            {"type": "integer"},
            {"type": "boolean"}
        ]
    });

    let a = generate(&schema, &seeded_opts(42)).unwrap();
    let b = generate(&schema, &seeded_opts(42)).unwrap();

    assert_eq!(a, b);
}

/// Optional recursive `allOf` always emits a terminal value that satisfies `required`.
#[test]
fn test_optional_recursive_all_of_terminal_has_required() {
    let schema = json!({
        "components": {
            "schemas": {
                "Person": {
                    "type": "object",
                    "properties": {
                        "name": {"type": "string"},
                        "parent": {
                            "allOf": [{"$ref": "#/components/schemas/Person"}]
                        }
                    },
                    "required": ["name"]
                }
            }
        },
        "$ref": "#/components/schemas/Person"
    });
    let mut opts = seeded_opts(1);
    opts.always_fake_optionals = true;

    let value = generate(&schema, &opts).unwrap();

    fn find_invalid_parent(value: &Value) -> Option<Value> {
        let obj = value.as_object()?;
        if let Some(parent) = obj.get("parent") {
            if parent.is_null() {
                return None;
            }
            if let Some(p_obj) = parent.as_object() {
                if !p_obj.contains_key("name") {
                    return Some(parent.clone());
                }
                if let Some(bad) = find_invalid_parent(parent) {
                    return Some(bad);
                }
            }
        }
        None
    }

    assert!(find_invalid_parent(&value).is_none());
}

/// Deeply recursive `allOf` under a tight `max_depth` still emits nodes satisfying `required`.
#[test]
fn test_recursive_all_of_under_tight_depth_satisfies_required() {
    let schema = json!({
        "$defs": {
            "Node": {
                "type": "object",
                "properties": {
                    "value": {"type": "string"},
                    "child": {"allOf": [{"$ref": "#/$defs/Node"}]}
                },
                "required": ["value"]
            }
        },
        "$ref": "#/$defs/Node"
    });
    let mut opts = seeded_opts(7);
    opts.always_fake_optionals = true;
    opts.max_depth = 3;

    let value = generate(&schema, &opts).unwrap();

    fn every_node_has_value(value: &Value) -> bool {
        let obj = match value.as_object() {
            Some(o) => o,
            None => return value.is_null(),
        };
        if !obj.contains_key("value") {
            return false;
        }
        if let Some(child) = obj.get("child") {
            return every_node_has_value(child);
        }
        true
    }

    assert!(every_node_has_value(&value));
}

/// A `oneOf` whose branches mix a `readOnly`-bearing schema with a non-readOnly schema does not panic.
#[test]
fn test_one_of_with_read_only_branch_generates_without_panic() {
    let schema = json!({
        "components": {
            "schemas": {
                "ResourceRef": {
                    "type": "object",
                    "properties": {"resourceId": {"type": "integer", "minimum": 1}},
                    "additionalProperties": false,
                    "required": ["resourceId"]
                },
                "Resource": {
                    "type": "object",
                    "properties": {
                        "resourceId": {"type": "integer", "readOnly": true, "minimum": 1},
                        "name": {"type": "string"}
                    }
                }
            }
        },
        "oneOf": [
            {"$ref": "#/components/schemas/ResourceRef"},
            {"$ref": "#/components/schemas/Resource"}
        ]
    });

    for seed in 0u64..10 {
        let value = generate(&schema, &seeded_opts(seed)).unwrap();
        let obj = value.as_object().unwrap();
        for key in obj.keys() {
            assert!(key == "resourceId" || key == "name");
        }
    }
}

/// `allOf` deep-merges the first sub-schema's `a` key into the result object.
#[test]
fn test_all_of_deep_merges_emits_a() {
    let schema = all_of_two_required_schema();

    let value = generate(&schema, &default_opts()).unwrap();
    let obj = value.as_object().unwrap();

    assert!(obj.contains_key("a"));
}

/// `allOf` deep-merges the second sub-schema's `b` key into the result object.
#[test]
fn test_all_of_deep_merges_emits_b() {
    let schema = all_of_two_required_schema();

    let value = generate(&schema, &default_opts()).unwrap();
    let obj = value.as_object().unwrap();

    assert!(obj.contains_key("b"));
}

/// Nested `allOf` branches merge so the first inner sub-schema's `a` key surfaces in the result.
#[test]
fn test_nested_all_of_branches_emit_a() {
    let schema = nested_all_of_schema();

    let value = generate(&schema, &default_opts()).unwrap();
    let obj = value.as_object().unwrap();

    assert!(obj.contains_key("a"));
}

/// Nested `allOf` branches merge so the second inner sub-schema's `b` key surfaces in the result.
#[test]
fn test_nested_all_of_branches_emit_b() {
    let schema = nested_all_of_schema();

    let value = generate(&schema, &default_opts()).unwrap();
    let obj = value.as_object().unwrap();

    assert!(obj.contains_key("b"));
}

/// `allOf` concatenates `required` arrays so the referenced schema's `street` key is emitted.
#[test]
fn test_all_of_concatenates_required_street() {
    let schema = all_of_with_ref_address_schema();

    let value = generate(&schema, &default_opts()).unwrap();
    let shipping = value.get("shipping").unwrap().as_object().unwrap();

    assert!(shipping.contains_key("street"));
}

/// `allOf` concatenates `required` arrays so the referenced schema's `city` key is emitted.
#[test]
fn test_all_of_concatenates_required_city() {
    let schema = all_of_with_ref_address_schema();

    let value = generate(&schema, &default_opts()).unwrap();
    let shipping = value.get("shipping").unwrap().as_object().unwrap();

    assert!(shipping.contains_key("city"));
}

/// `allOf` concatenates `required` arrays so the inline sibling sub-schema's `type` key is emitted.
#[test]
fn test_all_of_concatenates_required_type() {
    let schema = all_of_with_ref_address_schema();

    let value = generate(&schema, &default_opts()).unwrap();
    let shipping = value.get("shipping").unwrap().as_object().unwrap();

    assert!(shipping.contains_key("type"));
}

/// `discriminator.propertyName` injects the discriminator key into the chosen branch's value.
#[test]
fn test_discriminator_property_name_is_set_on_output() {
    let schema = json!({
        "discriminator": {"propertyName": "kind"},
        "oneOf": [
            {"$ref": "#/components/schemas/Cat"},
            {"$ref": "#/components/schemas/Dog"}
        ],
        "components": {
            "schemas": {
                "Cat": {
                    "type": "object",
                    "properties": {"kind": {"type": "string"}, "claws": {"type": "boolean"}},
                    "required": ["kind"]
                },
                "Dog": {
                    "type": "object",
                    "properties": {"kind": {"type": "string"}, "bark": {"type": "boolean"}},
                    "required": ["kind"]
                }
            }
        }
    });

    let value = generate(&schema, &seeded_opts(7)).unwrap();
    let kind = value.get("kind").and_then(|v| v.as_str()).unwrap();

    assert!(
        kind == "Cat" || kind == "Dog",
        "unexpected discriminator value `{}`",
        kind
    );
}

/// `discriminator.mapping` resolves the discriminator value to the mapping key whose value matches the `$ref`.
#[test]
fn test_discriminator_mapping_resolves_to_alias() {
    let schema = json!({
        "discriminator": {
            "propertyName": "kind",
            "mapping": {
                "cat-alias": "#/components/schemas/Cat",
                "dog-alias": "#/components/schemas/Dog"
            }
        },
        "oneOf": [
            {"$ref": "#/components/schemas/Cat"},
            {"$ref": "#/components/schemas/Dog"}
        ],
        "components": {
            "schemas": {
                "Cat": {"type": "object", "properties": {"kind": {"type": "string"}}, "required": ["kind"]},
                "Dog": {"type": "object", "properties": {"kind": {"type": "string"}}, "required": ["kind"]}
            }
        }
    });

    let value = generate(&schema, &seeded_opts(7)).unwrap();
    let kind = value.get("kind").and_then(|v| v.as_str()).unwrap();

    assert!(kind == "cat-alias" || kind == "dog-alias", "got `{}`", kind);
}

/// An `if`/`then`/`else` schema pins the `if` condition to TRUE so the `then` branch must be applied.
#[test]
fn test_if_then_else_then_branch_applies_when_condition_true() {
    let schema = json!({
        "type": "object",
        "properties": {
            "marker": {"type": "string", "const": "yes"},
            "result": {"type": "string"}
        },
        "required": ["marker"],
        "if": {"properties": {"marker": {"const": "yes"}}, "required": ["marker"]},
        "then": {"properties": {"result": {"const": "then-applied"}}, "required": ["result"]},
        "else": {"properties": {"result": {"const": "else-applied"}}, "required": ["result"]}
    });

    let value = generate(&schema, &seeded_opts(7)).unwrap();

    assert_eq!(value.get("result"), Some(&json!("then-applied")));
}

/// An `if`/`then`/`else` schema whose `if.properties` discriminator matches takes the `then`
/// branch, forcing the `value` property to be present.
#[test]
fn test_if_then_else_with_properties_routes_to_then_branch() {
    let schema = json!({
        "type": "object",
        "properties": {
            "kind": { "const": "a" },
            "value": { "type": "string" }
        },
        "if": { "properties": { "kind": { "const": "a" } } },
        "then": { "required": ["value"] }
    });
    let opts = seeded_opts(1);

    let result = generate(&schema, &opts).unwrap();
    let obj = result.as_object().expect("expected object");

    assert!(
        obj.contains_key("value"),
        "then branch should force value to be required"
    );
}

/// An `if`/`then`/`else` schema whose `if.properties` discriminator does NOT match takes the
/// `else` branch.
#[test]
fn test_if_then_else_else_branch_applies_when_condition_false() {
    let schema = json!({
        "type": "object",
        "properties": {
            "kind": {"type": "string", "const": "b"},
            "result": {"type": "string"}
        },
        "required": ["kind"],
        "if": {"properties": {"kind": {"const": "a"}}},
        "then": {"properties": {"result": {"const": "then-applied"}}, "required": ["result"]},
        "else": {"properties": {"result": {"const": "else-applied"}}, "required": ["result"]}
    });

    let value = generate(&schema, &seeded_opts(7)).unwrap();

    assert_eq!(value.get("result"), Some(&json!("else-applied")));
}

/// `allOf` with a nested `oneOf` selects one of the inner branches and merges its keys.
#[test]
fn test_nested_one_of_inside_all_of_picks_branch() {
    let schema = json!({
        "allOf": [
            {"type": "object", "properties": {"shared": {"type": "string", "const": "yes"}}, "required": ["shared"]},
            {"oneOf": [
                {"type": "object", "properties": {"branch": {"type": "string", "const": "left"}}, "required": ["branch"]},
                {"type": "object", "properties": {"branch": {"type": "string", "const": "right"}}, "required": ["branch"]}
            ]}
        ]
    });

    let value = generate(&schema, &seeded_opts(7)).unwrap();
    let branch = value.get("branch").and_then(|v| v.as_str()).unwrap();

    assert!(branch == "left" || branch == "right", "got `{}`", branch);
}

/// `generate_one_of` produces a value satisfying the chosen branch even when
/// the alternatives overlap; the walker tolerates the inevitable overlap
/// instead of failing.
#[test]
fn test_one_of_overlapping_branches_produces_satisfying_value() {
    let schema = json!({
        "oneOf": [
            {"type": "integer", "minimum": 0},
            {"type": "integer", "maximum": 100}
        ]
    });
    let opts = seeded_opts(1);

    let value = generate(&schema, &opts).unwrap();

    assert!(value.is_number(), "expected an integer, got {:?}", value);
}

/// `if`/`then`/`else` with a `required`-keyword `if` schema correctly routes
/// to `else` when the candidate is a non-object that cannot satisfy `required`.
#[test]
fn test_if_then_else_required_keyword_routes_to_else_for_non_object() {
    let schema = json!({
        "type": "string",
        "if": {"required": ["kind"]},
        "then": {"minLength": 100},
        "else": {"maxLength": 1}
    });
    let opts = seeded_opts(1);

    let value = generate(&schema, &opts).unwrap();
    let s = value.as_str().expect("string");

    assert!(
        s.chars().count() <= 1,
        "else branch should apply (maxLength: 1), got {:?}",
        s
    );
}
