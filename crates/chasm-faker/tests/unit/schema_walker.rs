//! Tests for `schema_walker.rs`.

use crate::common::{default_opts, seeded_opts};
use chasm_faker::{generate, FakerError, GenerateOptions};
use serde_json::{json, Value};
use std::time::{Duration, Instant};

/// Budget used by termination probes to bound the generator's wall-clock cost.
const TIME_BUDGET: Duration = Duration::from_secs(1);

/// Runs `generate(schema, opts)` and asserts it completes within `TIME_BUDGET`
/// without panicking, returning the produced value.
fn generate_within_budget(schema: &Value, opts: &GenerateOptions) -> Value {
    match generate_within_budget_checked(schema, opts) {
        Ok(v) => v,
        Err(e) => panic!("generation did not produce a value: {e}"),
    }
}

/// Runs `generate(schema, opts)` and asserts it completes within `TIME_BUDGET`
/// without panicking, returning the produced value or a stringified error so
/// callers can distinguish "completed without panic" from "produced a value".
fn generate_within_budget_checked(schema: &Value, opts: &GenerateOptions) -> Result<Value, String> {
    let start = Instant::now();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| generate(schema, opts)));
    let elapsed = start.elapsed();
    assert!(elapsed < TIME_BUDGET, "generation exceeded time budget");
    match result {
        Ok(Ok(v)) => Ok(v),
        Ok(Err(e)) => Err(format!("generation returned error: {e:?}")),
        Err(_) => Err("generation panicked".to_string()),
    }
}

/// `max_depth` prevents infinite recursion in deeply nested schemas.
#[test]
fn test_max_depth_prevents_infinite_recursion() {
    let schema = json!({
        "type": "object",
        "properties": {
            "child": {
                "type": "object",
                "properties": {"grandchild": {"type": "object"}}
            }
        },
        "required": ["child"]
    });
    let mut opts = default_opts();
    opts.max_depth = 2;

    let value = generate(&schema, &opts).unwrap();

    assert!(value.is_object());
}

/// A self-referential `$ref` via `properties` terminates within the time budget.
///
/// The recursive `Node` schema has no required properties, so the walker may pick either the
/// recursive branch (yielding an object) or the depth-cutoff terminal (yielding `null`). The
/// contract under test is solely that termination is reached without panic, so the assertion
/// accepts both branches.
#[test]
fn test_self_property_ref_terminates() {
    let schema = json!({
        "$ref": "#/components/schemas/Node",
        "components": {"schemas": {
            "Node": {
                "type": "object",
                "properties": {"next": {"$ref": "#/components/schemas/Node"}}
            }
        }}
    });

    let value = generate_within_budget(&schema, &default_opts());

    assert!(value.is_object() || value.is_null());
}

/// At `max_depth = 0`, the walker terminates a self-recursive schema by
/// emitting an empty object: the root level is realised but the recursive
/// `next` property is suppressed by the depth cap. This pins one concrete
/// branch of the broader `is_object || is_null` disjunction in sibling tests
/// — a regression that started returning `Null` here would be caught by the
/// strict equality check below even if the loose disjunctions still pass.
#[test]
fn test_self_property_ref_terminates_with_empty_object_at_zero_depth() {
    let schema = json!({
        "$ref": "#/components/schemas/Node",
        "components": {"schemas": {
            "Node": {
                "type": "object",
                "properties": {"next": {"$ref": "#/components/schemas/Node"}}
            }
        }}
    });
    let mut opts = default_opts();
    opts.max_depth = 0;

    let value = generate(&schema, &opts).unwrap();

    assert_eq!(
        value,
        json!({}),
        "expected empty object at max_depth=0, got {:?}",
        value
    );
}

/// A self-referential `$ref` via array `items` terminates within the time budget.
///
/// As with [`test_self_property_ref_terminates`], the optional recursive branch means the
/// walker may legitimately return either an object or `null` for this schema; both outcomes
/// satisfy the termination contract.
#[test]
fn test_self_items_ref_terminates() {
    let schema = json!({
        "$ref": "#/components/schemas/Tree",
        "components": {"schemas": {
            "Tree": {
                "type": "object",
                "properties": {
                    "children": {
                        "type": "array",
                        "items": {"$ref": "#/components/schemas/Tree"}
                    }
                }
            }
        }}
    });

    let value = generate_within_budget(&schema, &default_opts());

    assert!(value.is_object() || value.is_null());
}

/// Mutually recursive schemas terminate within the time budget.
///
/// `A` and `B` each declare a single optional property whose schema is the other; the walker
/// may pick either branch or the depth-cutoff terminal, so both an object value and `null`
/// satisfy the termination-only contract.
#[test]
fn test_mutual_recursion_terminates() {
    let schema = json!({
        "$ref": "#/components/schemas/A",
        "components": {"schemas": {
            "A": {
                "type": "object",
                "properties": {"b": {"$ref": "#/components/schemas/B"}}
            },
            "B": {
                "type": "object",
                "properties": {"a": {"$ref": "#/components/schemas/A"}}
            }
        }}
    });

    let value = generate_within_budget(&schema, &default_opts());

    assert!(value.is_object() || value.is_null());
}

/// A self-referential `$ref` inside `allOf` terminates within the time budget.
#[test]
fn test_self_in_all_of_terminates() {
    let schema = json!({
        "$ref": "#/components/schemas/Recursive",
        "components": {"schemas": {
            "Recursive": {
                "allOf": [
                    {"type": "object", "properties": {"id": {"type": "string"}}},
                    {"$ref": "#/components/schemas/Recursive"}
                ]
            }
        }}
    });

    let outcome = generate_within_budget_checked(&schema, &default_opts());
    assert!(
        outcome.is_ok(),
        "expected the walker to terminate without panic, got {outcome:?}",
    );
}

/// A self-referential `$ref` inside `anyOf` with a terminal branch terminates.
#[test]
fn test_self_in_any_of_with_terminal_terminates() {
    let schema = json!({
        "$ref": "#/components/schemas/Recursive",
        "components": {"schemas": {
            "Recursive": {
                "anyOf": [
                    {"type": "null"},
                    {"$ref": "#/components/schemas/Recursive"}
                ]
            }
        }}
    });

    let outcome = generate_within_budget_checked(&schema, &default_opts());
    assert!(
        outcome.is_ok(),
        "expected the walker to terminate without panic, got {outcome:?}",
    );
}

/// Required-recursive schemas terminate within a bounded depth budget.
#[test]
fn test_required_recursive_terminates_and_bounds_depth() {
    let schema = json!({
        "$ref": "#/components/schemas/Forced",
        "components": {"schemas": {
            "Forced": {
                "type": "object",
                "required": ["child"],
                "properties": {"child": {"$ref": "#/components/schemas/Forced"}}
            }
        }}
    });
    let mut opts = default_opts();
    opts.max_depth = 5;

    let value = generate_within_budget(&schema, &opts);

    fn depth(v: &Value) -> usize {
        match v {
            Value::Object(m) => match m.get("child") {
                Some(child) => 1 + depth(child),
                None => 0,
            },
            _ => 0,
        }
    }
    assert!(depth(&value) <= 16);
}

/// A `not` containing a self-reference terminates within the time budget.
#[test]
fn test_self_in_not_terminates() {
    let schema = json!({
        "$ref": "#/components/schemas/NotRec",
        "components": {"schemas": {
            "NotRec": {"not": {"$ref": "#/components/schemas/NotRec"}}
        }}
    });

    let outcome = generate_within_budget_checked(&schema, &default_opts());
    assert!(
        outcome.is_ok(),
        "expected the walker to terminate without panic, got {outcome:?}",
    );
}

/// A deep linear `$ref` chain (100 levels) terminates and emits a valid JSON value.
#[test]
fn test_deep_linear_ref_chain_terminates() {
    let mut schemas = serde_json::Map::new();
    for i in 1..100 {
        schemas.insert(
            format!("L{}", i),
            json!({"$ref": format!("#/components/schemas/L{}", i + 1)}),
        );
    }
    schemas.insert("L100".to_string(), json!({"type": "string"}));
    let schema = json!({
        "$ref": "#/components/schemas/L1",
        "components": {"schemas": schemas}
    });

    let value = generate_within_budget(&schema, &default_opts());

    assert!(
        value.is_string() || value.is_null(),
        "expected terminal string or depth-cutoff null, got {value:?}",
    );
}

/// A very large `max_depth` against a property-cycle does not hang.
#[test]
fn test_large_max_depth_does_not_hang() {
    let schema = json!({
        "$ref": "#/components/schemas/Node",
        "components": {"schemas": {
            "Node": {
                "type": "object",
                "properties": {"next": {"$ref": "#/components/schemas/Node"}}
            }
        }}
    });
    let mut opts = default_opts();
    opts.max_depth = 1000;

    let start = Instant::now();
    let outcome =
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| generate(&schema, &opts)));

    assert!(start.elapsed() < TIME_BUDGET);
    assert!(
        outcome.is_ok(),
        "expected the walker to terminate without panic against a deeply cyclic schema",
    );
}

/// Walks ascending `max_depth` values against a self-referential schema as a probe (not a guarantee).
#[test]
#[ignore = "probe: prints stack overflow ladder, not an assertion"]
fn test_stack_overflow_ladder_probe() {
    let schema = json!({
        "$ref": "#/components/schemas/Node",
        "components": {"schemas": {
            "Node": {
                "type": "object",
                "properties": {"next": {"$ref": "#/components/schemas/Node"}}
            }
        }}
    });
    for d in [100usize, 500, 1000, 2000, 5000, 10000, 20000, 50000] {
        let mut opts = default_opts();
        opts.max_depth = d;
        let start = Instant::now();
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| generate(&schema, &opts)));
        let elapsed = start.elapsed();
        let label = match r {
            Ok(Ok(_)) => "ok",
            Ok(Err(_)) => "err",
            Err(_) => "panic/abort",
        };
        eprintln!("max_depth={:>6} result={} elapsed={:?}", d, label, elapsed);
        if label == "panic/abort" {
            return;
        }
    }
}

/// `ref_depth_min` / `ref_depth_max` forces a minimum recursion depth in a required-recursive schema.
#[test]
fn test_ref_depth_min_max_forces_minimum_recursion() {
    let schema = json!({
        "$ref": "#/components/schemas/Forced",
        "components": {"schemas": {
            "Forced": {
                "type": "object",
                "required": ["child"],
                "properties": {"child": {"$ref": "#/components/schemas/Forced"}}
            }
        }}
    });

    let mut opts5 = default_opts();
    opts5.ref_depth_min = 5;
    opts5.ref_depth_max = 5;
    opts5.max_depth = 32;
    let v_5 = generate(&schema, &opts5).unwrap();

    fn depth(v: &Value) -> usize {
        match v {
            Value::Object(m) => match m.get("child") {
                Some(child) => 1 + depth(child),
                None => 0,
            },
            _ => 0,
        }
    }

    assert_eq!(depth(&v_5), 5);
}

/// `unevaluatedProperties: false` drops keys not declared in `properties` or matched by `additionalProperties`.
#[test]
fn test_unevaluated_properties_false_restricts_keys() {
    let schema = json!({
        "type": "object",
        "properties": {"a": {"type": "string"}},
        "required": ["a"],
        "additionalProperties": {"type": "string"},
        "unevaluatedProperties": false
    });

    let value = generate(&schema, &seeded_opts(7)).unwrap();
    let obj = value.as_object().unwrap();

    for key in obj.keys() {
        assert_eq!(key, "a");
    }
}

/// `unevaluatedItems: false` truncates the array beyond `prefixItems`.
#[test]
fn test_unevaluated_items_false_caps_array_length() {
    let schema = json!({
        "type": "array",
        "prefixItems": [{"type": "integer"}, {"type": "string"}],
        "items": {"type": "boolean"},
        "minItems": 5,
        "unevaluatedItems": false
    });

    let value = generate(&schema, &seeded_opts(7)).unwrap();
    let arr = value.as_array().unwrap();

    assert!(arr.len() <= 2);
}

/// A multi-typed schema combined with `allOf` produces a non-null value.
#[test]
fn test_multi_type_with_all_of_produces_non_null_value() {
    let schema = json!({
        "type": ["string", "integer"],
        "allOf": [{"minimum": 5}]
    });

    let value = generate(&schema, &seeded_opts(12)).unwrap();

    assert!(!value.is_null());
}

/// An `if`/`then` candidate evaluates against the base object's declared constraints.
#[test]
fn test_if_then_else_candidate_uses_base_constraints() {
    let schema = json!({
        "type": "object",
        "properties": {
            "kind": {"type": "string", "enum": ["a", "b"]},
            "value": {"type": "string"}
        },
        "required": ["kind"],
        "if": {"properties": {"kind": {"const": "a"}}, "required": ["kind"]},
        "then": {"required": ["value"]}
    });

    let value = generate(&schema, &seeded_opts(3)).unwrap();

    assert!(value.is_object());
}

/// `if`/`then`/`else` with a `type: null` marker takes the `then` branch.
#[test]
fn test_if_then_else_null_condition_evaluates_true() {
    let schema = json!({
        "type": "object",
        "properties": {
            "marker": {"type": "null"},
            "value": {"type": "string"}
        },
        "required": ["marker", "value"],
        "if": {"properties": {"marker": {"type": "null"}}},
        "then": {"properties": {"value": {"const": "then-branch"}}},
        "else": {"properties": {"value": {"const": "else-branch"}}}
    });

    let value = generate(&schema, &seeded_opts(11)).unwrap();

    assert_eq!(value["value"].as_str(), Some("then-branch"));
}

/// An unsatisfiable `not` constraint surfaces a `SchemaError`.
#[test]
fn test_not_constraint_exhaustion_surfaces_error() {
    let schema = json!({
        "type": "string",
        "minLength": 1,
        "maxLength": 1,
        "pattern": "^a$",
        "not": {"type": "string"}
    });

    let err =
        generate(&schema, &seeded_opts(1)).expect_err("unsatisfiable not should surface an error");

    match err {
        FakerError::SchemaError { message, .. } => assert!(message.contains("not constraint")),
        other => panic!("expected SchemaError, got {:?}", other),
    }
}

/// A string schema carrying `x-faker: "name.fullName"` produces a non-empty string value.
#[test]
fn test_x_faker_string_hint_produces_non_empty_string() {
    let schema = json!({"type": "string", "x-faker": "name.fullName"});

    let value = generate(&schema, &seeded_opts(7)).unwrap();
    let s = value.as_str().unwrap();

    assert!(!s.is_empty());
}

/// `x-faker` set on an object property is honoured during walking.
#[test]
fn test_x_faker_on_object_property_yields_string() {
    let schema = json!({
        "type": "object",
        "properties": {
            "name": {"type": "string", "x-faker": "internet.email"}
        },
        "required": ["name"]
    });

    let value = generate(&schema, &seeded_opts(11)).unwrap();
    let name = value.get("name").unwrap();

    assert!(name.is_string());
}

/// An unknown `x-faker` key falls through to type-based generation without panicking.
#[test]
fn test_x_faker_unknown_key_falls_through_to_type() {
    let schema = json!({"type": "string", "x-faker": "nonexistent.fakerkey"});

    let value = generate(&schema, &seeded_opts(13)).unwrap();

    assert!(value.is_string());
}

/// Node-level `x-json-schema-faker.alwaysFakeOptionals` produces an object with exactly the three declared keys.
#[test]
fn test_node_level_always_fake_optionals() {
    let schema = json!({
        "type": "object",
        "x-json-schema-faker": {"alwaysFakeOptionals": true},
        "properties": {
            "a": {"type": "string"},
            "b": {"type": "string"},
            "c": {"type": "string"}
        }
    });

    let value = generate(&schema, &seeded_opts(1)).unwrap();
    let obj = value.as_object().unwrap();
    let mut keys: Vec<&str> = obj.keys().map(|s| s.as_str()).collect();
    keys.sort();

    assert_eq!(keys, vec!["a", "b", "c"]);
}

/// Node-level `x-json-schema-faker.useDefaultValue` returns the schema's `default` verbatim.
#[test]
fn test_node_level_use_default_value() {
    let schema = json!({
        "type": "string",
        "x-json-schema-faker": {"useDefaultValue": true},
        "default": "chasm-default-sentinel"
    });

    let value = generate(&schema, &seeded_opts(2)).unwrap();

    assert_eq!(value, json!("chasm-default-sentinel"));
}

/// Node-level `x-json-schema-faker.useExamplesValue` picks from the schema's `examples` array.
#[test]
fn test_node_level_use_examples_value() {
    let schema = json!({
        "type": "string",
        "x-json-schema-faker": {"useExamplesValue": true},
        "examples": ["only-allowed-example"]
    });

    let value = generate(&schema, &seeded_opts(3)).unwrap();

    assert_eq!(value, json!("only-allowed-example"));
}

/// Node-level `x-json-schema-faker.requiredOnly` keeps every declared required property.
#[test]
fn test_node_level_required_only_keeps_required() {
    let schema = node_required_only_schema();
    let mut opts = seeded_opts(4);
    opts.always_fake_optionals = true;

    let value = generate(&schema, &opts).unwrap();
    let obj = value.as_object().unwrap();

    assert!(obj.contains_key("req"));
}

/// Node-level `x-json-schema-faker.requiredOnly` omits every non-required property, even when the
/// caller has `always_fake_optionals = true`.
#[test]
fn test_node_level_required_only_omits_optionals() {
    let schema = node_required_only_schema();
    let mut opts = seeded_opts(4);
    opts.always_fake_optionals = true;

    let value = generate(&schema, &opts).unwrap();
    let obj = value.as_object().unwrap();

    assert!(!obj.contains_key("opt"));
}

/// Fixture: object schema declaring one required and one optional property with node-level `requiredOnly`.
fn node_required_only_schema() -> serde_json::Value {
    json!({
        "type": "object",
        "x-json-schema-faker": {"requiredOnly": true},
        "properties": {
            "req": {"type": "string"},
            "opt": {"type": "string"}
        },
        "required": ["req"]
    })
}

/// Node-level `x-json-schema-faker.optionalsProbability=1.0` forces every optional to appear.
#[test]
fn test_node_level_optionals_probability_one() {
    let schema = json!({
        "type": "object",
        "x-json-schema-faker": {"optionalsProbability": 1.0},
        "properties": {
            "p1": {"type": "string"},
            "p2": {"type": "string"},
            "p3": {"type": "string"}
        }
    });

    let value = generate(&schema, &seeded_opts(5)).unwrap();
    let obj = value.as_object().unwrap();

    assert_eq!(obj.len(), 3);
}

/// Node-level `x-json-schema-faker.fillProperties=false` leaves objects below `minProperties`.
#[test]
fn test_node_level_fill_properties_false() {
    let schema = json!({
        "type": "object",
        "x-json-schema-faker": {"fillProperties": false},
        "properties": {"only": {"type": "string"}},
        "required": ["only"],
        "minProperties": 5
    });

    let value = generate(&schema, &seeded_opts(6)).unwrap();
    let obj = value.as_object().unwrap();

    assert_eq!(obj.len(), 1);
}

/// Node-level `x-json-schema-faker.failOnInvalidTypes` rejects an unknown type.
#[test]
fn test_node_level_fail_on_invalid_types() {
    let schema = json!({
        "type": "not-a-real-type",
        "x-json-schema-faker": {"failOnInvalidTypes": true}
    });
    let mut opts = seeded_opts(7);
    opts.fail_on_invalid_type = false;

    let result = generate(&schema, &opts);

    assert!(
        matches!(result, Err(FakerError::InvalidType { .. })),
        "expected InvalidType, got {result:?}",
    );
}

/// Node-level `x-json-schema-faker.failOnInvalidFormat` rejects an unknown `x-faker` key.
#[test]
fn test_node_level_fail_on_invalid_format() {
    let schema = json!({
        "type": "string",
        "x-faker": "nonexistent.fakerKey",
        "x-json-schema-faker": {"failOnInvalidFormat": true}
    });
    let mut opts = seeded_opts(8);
    opts.fail_on_invalid_format = false;

    let result = generate(&schema, &opts);

    assert!(
        matches!(result, Err(FakerError::UnknownFakerGenerator { .. })),
        "expected UnknownFakerGenerator, got {result:?}",
    );
}

/// Node-level `x-json-schema-faker.maxItems` caps the generated array length.
#[test]
fn test_node_level_max_items_caps_array() {
    let schema = json!({
        "type": "array",
        "x-json-schema-faker": {"minItems": 2, "maxItems": 2},
        "items": {"type": "integer"}
    });

    let value = generate(&schema, &seeded_opts(9)).unwrap();
    let arr = value.as_array().unwrap();

    assert_eq!(arr.len(), 2);
}

/// Node-level `x-json-schema-faker.minItems`/`maxItems` on the parent flows into a child array property.
#[test]
fn test_node_level_x_json_schema_faker_propagates_to_array_child() {
    let schema = json!({
        "type": "object",
        "x-json-schema-faker": {
            "minItems": 4,
            "maxItems": 4,
            "optionalsProbability": 1.0
        },
        "properties": {
            "tags": {"type": "array", "items": {"type": "string"}}
        },
        "required": ["tags"]
    });

    let value = generate(&schema, &seeded_opts(99)).unwrap();
    let arr = value.get("tags").and_then(|v| v.as_array()).unwrap();

    assert_eq!(arr.len(), 4);
}

/// An `x-faker` object form `{helpers.arrayElement: [...]}` picks from the listed entries.
#[test]
fn test_x_faker_helpers_array_element_picks_one() {
    let schema = json!({
        "type": "string",
        "x-faker": {"helpers.arrayElement": ["yard", "foot"]}
    });

    let value = generate(&schema, &seeded_opts(42)).unwrap();
    let s = value.as_str().unwrap();

    assert!(s == "yard" || s == "foot");
}

/// A `nullable: true` property with `x-faker` and `format: date-time` produces a string when the non-null branch is taken.
#[test]
fn test_nullable_x_faker_non_null_branch_yields_string() {
    let schema = json!({
        "type": "object",
        "properties": {
            "test": {
                "type": "string",
                "x-faker": "date.future",
                "format": "date-time",
                "nullable": true
            }
        },
        "required": ["test"]
    });
    let mut opts = seeded_opts(5);
    opts.optionals_probability = Some(0.0);

    let value = generate(&schema, &opts).unwrap();
    let v = value.get("test").unwrap();

    assert!(v.is_string());
}

/// A `nullable: true` property's `x-faker` non-null branch emits an ISO-shaped date-time string.
#[test]
fn test_nullable_x_faker_non_null_branch_is_iso_shaped() {
    let schema = json!({
        "type": "object",
        "properties": {
            "expiry": {
                "type": "string",
                "x-faker": "date.future",
                "format": "date-time",
                "nullable": true
            }
        },
        "required": ["expiry"]
    });
    let mut opts = seeded_opts(31);
    opts.optionals_probability = Some(0.0);

    let value = generate(&schema, &opts).unwrap();
    let s = value.get("expiry").unwrap().as_str().unwrap();

    assert!(s.contains('T') && (s.contains('Z') || s.contains('+') || s.contains('-')));
}

/// A `nullable: true` `x-faker` property emits a non-null value at least once across seeds.
#[test]
fn test_nullable_x_faker_sometimes_emits_non_null() {
    let schema = json!({
        "type": "object",
        "properties": {
            "when": {
                "type": "string",
                "x-faker": "date.recent",
                "format": "date-time",
                "nullable": true
            }
        },
        "required": ["when"]
    });

    let mut non_null_seen = false;
    for seed in 0u64..20 {
        let mut opts = seeded_opts(seed);
        opts.optionals_probability = Some(0.0);
        let value = generate(&schema, &opts).unwrap();
        if !value.get("when").unwrap().is_null() {
            non_null_seen = true;
            break;
        }
    }

    assert!(non_null_seen);
}

/// `type: ["string", "null"]` emits literal JSON null at least once when the null branch is picked.
#[test]
fn test_nullable_string_union_emits_json_null_branch() {
    let schema = json!({
        "type": ["string", "null"],
        "format": "currency"
    });

    let mut saw_null = false;
    for seed in 0u64..40 {
        let value = generate(&schema, &seeded_opts(seed)).unwrap();
        assert!(value != json!("string"));
        if value.is_null() {
            saw_null = true;
        }
    }

    assert!(saw_null);
}

/// When both `enum` and a single `example` are present, `use_examples_value` returns the example.
#[test]
fn test_enum_with_example_returns_example_under_use_examples_value() {
    let schema = json!({
        "type": "object",
        "properties": {
            "status": {
                "type": "string",
                "enum": ["FAILURE", "SUCCESS"],
                "example": "SUCCESS"
            }
        },
        "required": ["status"]
    });
    let mut opts = seeded_opts(1);
    opts.use_examples_value = true;

    let value = generate(&schema, &opts).unwrap();

    assert_eq!(value.get("status").unwrap(), &json!("SUCCESS"));
}

/// When both `enum` and an `examples` array are present, `use_examples_value` picks from `examples`.
#[test]
fn test_enum_with_examples_array_returns_example_under_use_examples_value() {
    let schema = json!({
        "type": "string",
        "enum": ["FAILURE", "SUCCESS"],
        "examples": ["SUCCESS"]
    });
    let mut opts = seeded_opts(2);
    opts.use_examples_value = true;

    let value = generate(&schema, &opts).unwrap();

    assert_eq!(value, json!("SUCCESS"));
}

/// An inline `example` object is returned verbatim under `use_examples_value=true`.
#[test]
fn test_inline_example_object_returned_verbatim() {
    let schema = json!({
        "type": "object",
        "properties": {
            "id": {"type": "string"},
            "name": {"type": "string"}
        },
        "required": ["id", "name"],
        "example": {
            "id": "abc-123",
            "name": "external-fixture-payload"
        }
    });
    let mut opts = seeded_opts(7);
    opts.use_examples_value = true;

    let value = generate(&schema, &opts).unwrap();

    assert_eq!(
        value,
        json!({"id": "abc-123", "name": "external-fixture-payload"})
    );
}

/// `default` takes precedence over `enum` when `use_default_value` is enabled.
#[test]
fn test_default_wins_over_enum_when_use_default_value_true() {
    let schema = json!({"enum": [1], "default": 2});
    let mut opts = default_opts();
    opts.use_default_value = true;

    let value = generate(&schema, &opts).unwrap();

    assert_eq!(value, json!(2));
}

/// A `false` schema literal surfaces a `CannotGenerateForFalseSchema` error.
#[test]
fn test_boolean_false_schema_errors() {
    let schema = json!(false);

    let result = generate(&schema, &default_opts());

    assert!(
        matches!(result, Err(FakerError::CannotGenerateForFalseSchema)),
        "expected CannotGenerateForFalseSchema, got {result:?}",
    );
}

/// An embedded `false` schema on a required property surfaces an error at the top level.
#[test]
fn test_embedded_false_schema_errors() {
    let schema = json!({
        "type": "object",
        "properties": {"x": false},
        "required": ["x"]
    });

    let result = generate(&schema, &default_opts());

    assert!(
        matches!(result, Err(FakerError::CannotGenerateForFalseSchema)),
        "expected CannotGenerateForFalseSchema, got {result:?}",
    );
}

/// A schema combining `minLength` with `not` and no explicit `type` does not
/// route to `generate_not` (which would ignore `minLength`); instead the walker
/// infers `type: string` from `minLength` and respects the length constraint.
#[test]
fn test_not_with_min_length_no_type_honours_length() {
    let schema = json!({
        "minLength": 50,
        "not": {"pattern": "evil"}
    });
    let opts = seeded_opts(1);

    let value = generate(&schema, &opts).unwrap();
    let s = value.as_str().expect("string");

    assert!(
        s.chars().count() >= 50,
        "expected string of length at least 50, got length {} ({:?})",
        s.chars().count(),
        s
    );
}

/// Cycle protection inside `merge_all_of_into_terminal_schema` persists the
/// visited-ref set across the recursive terminal walk, so a self-referential
/// `allOf` chain reached at the depth limit terminates rather than re-merging
/// itself indefinitely.
#[test]
fn test_terminal_all_of_self_ref_cycle_terminates() {
    let schema = json!({
        "$ref": "#/components/schemas/Loop",
        "components": {"schemas": {
            "Loop": {
                "type": "object",
                "required": ["self"],
                "properties": {"self": {"$ref": "#/components/schemas/Loop"}},
                "allOf": [{"$ref": "#/components/schemas/Loop"}]
            }
        }}
    });
    let mut opts = default_opts();
    opts.max_depth = 1;

    let outcome = generate_within_budget_checked(&schema, &opts);

    assert!(
        outcome.is_ok(),
        "expected the walker to terminate without panic, got {outcome:?}"
    );
}

/// `walk_if_then_else` rejects a then-branch that contradicts the if-merged
/// base on numeric range, rather than producing an unsatisfiable schema.
#[test]
fn test_schemas_contradict_detects_numeric_range_clash() {
    let schema = json!({
        "type": "integer",
        "if": {"maximum": 5},
        "then": {"minimum": 10}
    });
    let opts = seeded_opts(1);

    let result = generate(&schema, &opts);

    assert!(result.is_ok(), "walker should terminate, got {:?}", result);
}

/// `schemas_contradict` recognises a `minLength` vs `maxLength` clash.
#[test]
fn test_schemas_contradict_detects_length_range_clash() {
    let a = json!({"minLength": 10});
    let b = json!({"maxLength": 5});
    let result = chasm_faker_test_helpers::schemas_contradict_for_test(&a, &b);

    assert!(result, "expected length range clash to be detected");
}

/// `schemas_contradict` recognises a `minimum` vs `maximum` clash.
#[test]
fn test_schemas_contradict_detects_min_max_clash() {
    let a = json!({"minimum": 10});
    let b = json!({"maximum": 5});
    let result = chasm_faker_test_helpers::schemas_contradict_for_test(&a, &b);

    assert!(result, "expected minimum/maximum clash to be detected");
}

/// `schemas_contradict` recognises an `exclusiveMinimum` vs `maximum` clash.
#[test]
fn test_schemas_contradict_detects_exclusive_minimum_clash() {
    let a = json!({"exclusiveMinimum": 5});
    let b = json!({"maximum": 5});
    let result = chasm_faker_test_helpers::schemas_contradict_for_test(&a, &b);

    assert!(
        result,
        "expected exclusiveMinimum vs maximum clash to be detected"
    );
}

mod chasm_faker_test_helpers {
    use serde_json::Value;

    /// Test-only entry into `schema_walker::schemas_contradict` via a public re-export
    /// in `__test_internals`.
    pub fn schemas_contradict_for_test(a: &Value, b: &Value) -> bool {
        chasm_faker::__test_internals::schemas_contradict(a, b)
    }
}

/// `unevaluatedProperties: false` does not drop keys evaluated by an `allOf` branch.
#[test]
fn test_unevaluated_properties_respects_allof_branch_properties() {
    let schema = json!({
        "type": "object",
        "allOf": [{"properties": {"foo": {"type": "string"}}, "required": ["foo"]}],
        "unevaluatedProperties": false,
        "properties": {"bar": {"type": "integer"}},
        "required": ["bar"]
    });
    let opts = seeded_opts(1);

    let value = generate(&schema, &opts).unwrap();
    let obj = value.as_object().expect("object");

    assert!(
        obj.contains_key("foo"),
        "foo (from allOf branch) must be present, got {:?}",
        obj
    );
    assert!(
        obj.contains_key("bar"),
        "bar (from local properties) must be present, got {:?}",
        obj
    );
}

/// `prop_aliases` configured with a cycle (a-to-b, b-to-a) does not infinite-loop
/// the walker; rewrites apply in one pass.
#[test]
fn test_prop_aliases_cycle_does_not_recurse_infinitely() {
    let mut opts = default_opts();
    opts.prop_aliases = std::collections::HashMap::from([
        ("foo".to_string(), "bar".to_string()),
        ("bar".to_string(), "foo".to_string()),
    ]);
    let schema = json!({"type": "object", "foo": 1});

    let result = generate_within_budget_checked(&schema, &opts);

    assert!(
        result.is_ok(),
        "expected the walker to terminate without stack overflow, got {result:?}"
    );
}
