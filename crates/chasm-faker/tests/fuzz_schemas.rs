//! Property-based fuzz tests for chasm-faker.
//!
//! These tests feed randomly-generated JSON Schemas into `generate` and assert
//! that it never panics, never hangs for longer than a wall-clock budget, and
//! always returns a JSON value of an acceptable shape.

use chasm_faker::{generate, GenerateOptions};
use proptest::collection::{hash_map, vec};
use proptest::prelude::*;
use serde_json::{json, Map, Value};
use std::panic::AssertUnwindSafe;
use std::time::{Duration, Instant};

/// Per-schema wall-clock budget. If a single `generate` call exceeds this we
/// treat it as a hang and report it as a failure.
const PER_SCHEMA_BUDGET: Duration = Duration::from_secs(1);

/// Memory pressure ceiling for the serialised generated value.
const MAX_SERIALISED_BYTES: usize = 100 * 1024 * 1024;

/// Builds a `GenerateOptions` with a deterministic seed for reproducibility.
fn opts_with_seed(seed: u64) -> GenerateOptions {
    let mut opts = GenerateOptions::default();
    opts.seed = Some(seed);
    opts.fail_on_invalid_type = false;
    opts.fail_on_invalid_format = false;
    opts
}

/// Returns a strategy for primitive JSON values that may appear inside
/// `enum`/`const`/`examples`/`default` slots.
fn arb_primitive() -> impl Strategy<Value = Value> {
    prop_oneof![
        Just(Value::Null),
        any::<bool>().prop_map(Value::Bool),
        any::<i64>().prop_map(|n| json!(n)),
        any::<f64>()
            .prop_filter("finite", |f| f.is_finite())
            .prop_map(|f| json!(f)),
        "[a-zA-Z0-9_ ]{0,16}".prop_map(Value::String),
    ]
}

/// Returns a strategy for a single JSON Schema `type` keyword value.
fn arb_type_keyword() -> impl Strategy<Value = Value> {
    prop_oneof![
        Just(Value::String("null".to_string())),
        Just(Value::String("boolean".to_string())),
        Just(Value::String("integer".to_string())),
        Just(Value::String("number".to_string())),
        Just(Value::String("string".to_string())),
        Just(Value::String("array".to_string())),
        Just(Value::String("object".to_string())),
    ]
}

/// Returns a strategy for a known or unknown `format` keyword value.
fn arb_format_keyword() -> impl Strategy<Value = Value> {
    let known = prop_oneof![
        Just("date-time"),
        Just("date"),
        Just("time"),
        Just("email"),
        Just("uuid"),
        Just("uri"),
        Just("ipv4"),
        Just("ipv6"),
        Just("hostname"),
        Just("regex"),
    ]
    .prop_map(|s| Value::String(s.to_string()));
    let unknown = "[a-z]{1,12}".prop_map(|s| Value::String(format!("x-{}", s)));
    prop_oneof![known, unknown]
}

/// Returns a strategy for arbitrary `$ref` strings, most of which point to
/// non-existent paths.
fn arb_ref() -> impl Strategy<Value = String> {
    prop_oneof![
        "#/components/schemas/[A-Z][a-z]{1,8}".prop_map(|s| s),
        "#/definitions/[A-Z][a-z]{1,8}".prop_map(|s| s),
        Just("#/does/not/exist".to_string()),
        "#".prop_map(|_| "#".to_string()),
    ]
}

/// Returns a strategy that builds a leaf JSON Schema object.
///
/// Leaf schemas can have a type, optional constraints (sometimes contradictory),
/// optional `enum`/`const`, optional `format`, and optional `$ref`. They never
/// recurse.
fn arb_leaf_schema() -> BoxedStrategy<Value> {
    let with_type = (
        arb_type_keyword(),
        any::<bool>(),
        any::<i32>(),
        any::<i32>(),
        any::<u32>(),
        any::<u32>(),
        proptest::option::of(arb_format_keyword()),
        proptest::option::of("[a-z]{1,8}".prop_map(Value::String)),
    )
        .prop_map(|(ty, swap_bounds, a, b, c, d, fmt, pattern)| {
            let mut m = Map::new();
            m.insert("type".to_string(), ty.clone());
            let type_str = ty.as_str().unwrap_or("");
            let (lo_num, hi_num) = if swap_bounds { (b, a) } else { (a, b) };
            let (lo_len, hi_len) = if swap_bounds {
                (d % 32, c % 32)
            } else {
                (c % 32, d % 32)
            };
            match type_str {
                "integer" | "number" => {
                    m.insert("minimum".to_string(), json!(lo_num));
                    m.insert("maximum".to_string(), json!(hi_num));
                }
                "string" => {
                    m.insert("minLength".to_string(), json!(lo_len));
                    m.insert("maxLength".to_string(), json!(hi_len));
                    if let Some(p) = pattern {
                        m.insert("pattern".to_string(), p);
                    }
                    if let Some(f) = fmt {
                        m.insert("format".to_string(), f);
                    }
                }
                "array" => {
                    m.insert("minItems".to_string(), json!(lo_len));
                    m.insert("maxItems".to_string(), json!(hi_len));
                }
                _ => {}
            }
            Value::Object(m)
        })
        .boxed();

    let with_enum = vec(arb_primitive(), 0..5)
        .prop_map(|vs| {
            let mut m = Map::new();
            m.insert("enum".to_string(), Value::Array(vs));
            Value::Object(m)
        })
        .boxed();

    let with_const = arb_primitive()
        .prop_map(|v| {
            let mut m = Map::new();
            m.insert("const".to_string(), v);
            Value::Object(m)
        })
        .boxed();

    let with_ref = arb_ref()
        .prop_map(|r| {
            let mut m = Map::new();
            m.insert("$ref".to_string(), Value::String(r));
            Value::Object(m)
        })
        .boxed();

    let empty = Just(Value::Object(Map::new())).boxed();

    prop_oneof![with_type, with_enum, with_const, with_ref, empty].boxed()
}

/// Returns a strategy that generates arbitrary JSON Schema objects with a
/// depth cap so generation terminates.
pub fn arb_schema(depth: u32) -> BoxedStrategy<Value> {
    let leaf = arb_leaf_schema();
    leaf.prop_recursive(depth, 64, 4, |inner| {
        prop_oneof![
            hash_map("[a-z]{1,8}".prop_map(|s| s), inner.clone(), 0..4).prop_map(|props| {
                let mut m = Map::new();
                m.insert("type".to_string(), Value::String("object".to_string()));
                let mut p = Map::new();
                for (k, v) in props {
                    p.insert(k, v);
                }
                m.insert("properties".to_string(), Value::Object(p));
                Value::Object(m)
            }),
            inner.clone().prop_map(|items| {
                let mut m = Map::new();
                m.insert("type".to_string(), Value::String("array".to_string()));
                m.insert("items".to_string(), items);
                Value::Object(m)
            }),
            vec(inner.clone(), 0..4).prop_map(|arr| {
                let mut m = Map::new();
                m.insert("allOf".to_string(), Value::Array(arr));
                Value::Object(m)
            }),
            vec(inner.clone(), 0..4).prop_map(|arr| {
                let mut m = Map::new();
                m.insert("anyOf".to_string(), Value::Array(arr));
                Value::Object(m)
            }),
            vec(inner, 0..4).prop_map(|arr| {
                let mut m = Map::new();
                m.insert("oneOf".to_string(), Value::Array(arr));
                Value::Object(m)
            }),
        ]
    })
    .boxed()
}

/// Runs `generate` while capturing panics. Returns the result along with the
/// observed wall-clock duration.
fn run_generate(
    schema: &Value,
    opts: &GenerateOptions,
) -> (
    Result<Result<Value, chasm_faker::FakerError>, String>,
    Duration,
) {
    let start = Instant::now();
    let captured = std::panic::catch_unwind(AssertUnwindSafe(|| generate(schema, opts)));
    let elapsed = start.elapsed();
    let mapped = captured.map_err(|e| {
        if let Some(s) = e.downcast_ref::<&'static str>() {
            (*s).to_string()
        } else if let Some(s) = e.downcast_ref::<String>() {
            s.clone()
        } else {
            "non-string panic payload".to_string()
        }
    });
    (mapped, elapsed)
}

/// Inspects `schema`'s top-level `type` keyword, if any.
fn top_type(schema: &Value) -> Option<&str> {
    schema.get("type").and_then(|v| v.as_str())
}

/// Inspects `schema`'s top-level `enum` array, if any.
fn top_enum(schema: &Value) -> Option<&Vec<Value>> {
    schema.get("enum").and_then(|v| v.as_array())
}

/// Asserts a generated value matches type/enum invariants reported in the report.
fn check_shape(schema: &Value, value: &Value) -> Result<(), String> {
    match top_type(schema) {
        Some("integer") if !(value.is_number() || value.is_null()) => {
            return Err(format!("expected integer/null, got {:?}", value));
        }
        Some("array") if !(value.is_array() || value.is_null()) => {
            return Err(format!("expected array/null, got {:?}", value));
        }
        _ => {}
    }
    if let Some(values) = top_enum(schema) {
        if !values.is_empty() && !value.is_null() && !values.iter().any(|v| v == value) {
            return Err(format!("value {:?} not in enum {:?}", value, values));
        }
    }
    Ok(())
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 1000,
        max_shrink_iters: 64,
        .. ProptestConfig::default()
    })]

    /// Asserts `generate` never panics, never exceeds the wall-clock budget,
    /// returns a JSON document, and respects basic type/enum shape rules.
    #[test]
    fn test_fuzz_no_panic_and_shape(schema in arb_schema(4)) {
        let opts = opts_with_seed(0xC0FFEE);
        let (result, elapsed) = run_generate(&schema, &opts);
        prop_assert!(
            elapsed < PER_SCHEMA_BUDGET,
            "schema generation exceeded budget: {:?} for {}",
            elapsed,
            schema_preview(&schema),
        );
        match result {
            Err(msg) => prop_assert!(
                false,
                "panic from generate({}): {}",
                schema_preview(&schema),
                msg
            ),
            Ok(Err(_)) => {}
            Ok(Ok(value)) => {
                let serialised = serde_json::to_string(&value)
                    .expect("generated value must serialise");
                prop_assert!(
                    serialised.len() < MAX_SERIALISED_BYTES,
                    "generated output blew memory budget ({} bytes) for {}",
                    serialised.len(),
                    schema_preview(&schema),
                );
                if let Err(e) = check_shape(&schema, &value) {
                    prop_assert!(false, "shape invariant violated: {} for {}", e, schema_preview(&schema));
                }
            }
        }
    }

    /// Asserts that repeating `generate` with the same seed yields byte-identical output.
    #[test]
    fn test_fuzz_determinism(schema in arb_schema(3), seed in any::<u64>()) {
        let opts = opts_with_seed(seed);
        let (first, _) = run_generate(&schema, &opts);
        let (second, _) = run_generate(&schema, &opts);
        match (&first, &second) {
            (Ok(Ok(a)), Ok(Ok(b))) => {
                let sa = serde_json::to_string(a).unwrap();
                let sb = serde_json::to_string(b).unwrap();
                prop_assert_eq!(sa, sb, "non-deterministic output for {}", schema_preview(&schema));
            }
            (Ok(Err(_)), Ok(Err(_))) => {}
            (Err(_), Err(_)) => {}
            _ => {
                prop_assert!(
                    false,
                    "non-deterministic outcome for {}: {:?} vs {:?}",
                    schema_preview(&schema),
                    first,
                    second
                );
            }
        }
    }
}

/// Renders a short preview of a schema for use in failure messages.
fn schema_preview(schema: &Value) -> String {
    let s = serde_json::to_string(schema).unwrap_or_default();
    if s.len() > 200 {
        format!("{}...", &s[..200])
    } else {
        s
    }
}

/// Regression: an empty `oneOf` array must not panic; it must produce a generated value.
#[test]
fn test_regression_empty_oneof() {
    let schema = json!({ "oneOf": [] });
    let (result, _) = run_generate(&schema, &opts_with_seed(1));
    assert!(
        matches!(result, Ok(Ok(_))),
        "regression: expected Ok(Ok(_)), got {result:?}"
    );
}

/// Regression: an empty `enum` array must not panic; it must produce a generated value.
#[test]
fn test_regression_empty_enum() {
    let schema = json!({ "enum": [] });
    let (result, _) = run_generate(&schema, &opts_with_seed(1));
    assert!(
        matches!(result, Ok(Ok(_))),
        "regression: expected Ok(Ok(_)), got {result:?}"
    );
}

/// Regression: a contradictory integer range (`minimum > maximum`) must not panic.
#[test]
fn test_regression_contradictory_integer_range() {
    let schema = json!({ "type": "integer", "minimum": 100, "maximum": 10 });
    let (result, _) = run_generate(&schema, &opts_with_seed(1));
    assert!(
        matches!(result, Ok(Ok(_))),
        "regression: expected Ok(Ok(_)), got {result:?}"
    );
}

/// Regression: a contradictory array length range (`minItems > maxItems`) must not panic.
#[test]
fn test_regression_contradictory_array_range() {
    let schema =
        json!({ "type": "array", "minItems": 100, "maxItems": 1, "items": { "type": "integer" } });
    let (result, _) = run_generate(&schema, &opts_with_seed(1));
    assert!(
        matches!(result, Ok(Ok(_))),
        "regression: expected Ok(Ok(_)), got {result:?}"
    );
}

/// Regression: an unresolved local `$ref` must produce a structured error via
/// the inner `Result` rather than panicking.
#[test]
fn test_regression_unresolved_local_ref() {
    let schema = json!({ "$ref": "#/components/schemas/DoesNotExist" });
    let (result, _) = run_generate(&schema, &opts_with_seed(1));
    assert!(
        matches!(result, Ok(Err(_))),
        "regression: expected Ok(Err(_)) for unresolved ref, got {result:?}"
    );
}
