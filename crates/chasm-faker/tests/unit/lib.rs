//! Tests for `lib.rs`.

use crate::common::{default_opts, seeded_opts};
use chasm_faker::{generate, generate_static, FakerError};
use serde_json::{json, Value};
use serial_test::serial;
use std::sync::{Arc, Mutex};

/// `generate_static` produces byte-identical output across runs.
#[test]
fn test_generate_static_is_deterministic() {
    let schema = json!({
        "type": "object",
        "properties": {
            "name": {"type": "string"},
            "age": {"type": "integer"},
            "active": {"type": "boolean"},
            "tags": {"type": "array", "items": {"type": "string"}, "minItems": 2}
        },
        "required": ["name", "age", "active", "tags"]
    });

    let a = generate_static(&schema, &default_opts()).unwrap();
    let b = generate_static(&schema, &default_opts()).unwrap();

    assert_eq!(a, b);
}

/// `generate_static` emits type-default scalars for primitive types.
#[test]
fn test_generate_static_emits_type_defaults() {
    let schema = json!({
        "type": "object",
        "properties": {
            "name": {"type": "string"},
            "age": {"type": "integer"},
            "active": {"type": "boolean"}
        },
        "required": ["name", "age", "active"]
    });

    let value = generate_static(&schema, &default_opts()).unwrap();
    let obj = value.as_object().unwrap();

    assert_eq!(obj.get("name"), Some(&json!("")));
}

/// `generate_static` honours `const` selection.
#[test]
fn test_generate_static_honours_const() {
    let schema = json!({
        "type": "object",
        "properties": {"k": {"const": 42}},
        "required": ["k"]
    });

    let value = generate_static(&schema, &default_opts()).unwrap();
    let obj = value.as_object().unwrap();

    assert_eq!(obj.get("k"), Some(&json!(42)));
}

/// `generate_static` honours `enum` selection by picking the first entry.
#[test]
fn test_generate_static_honours_enum_first_entry() {
    let schema = json!({
        "type": "object",
        "properties": {"e": {"enum": ["a", "b"]}},
        "required": ["e"]
    });

    let value = generate_static(&schema, &default_opts()).unwrap();
    let obj = value.as_object().unwrap();

    assert_eq!(obj.get("e"), Some(&json!("a")));
}

/// `generate_static` recurses through `$ref` to populate the referenced sub-schema.
#[test]
fn test_generate_static_recurses_through_ref() {
    let schema = json!({
        "$defs": {
            "Inner": {
                "type": "object",
                "properties": {"x": {"type": "integer"}},
                "required": ["x"]
            }
        },
        "type": "object",
        "properties": {"i": {"$ref": "#/$defs/Inner"}},
        "required": ["i"]
    });

    let value = generate_static(&schema, &default_opts()).unwrap();
    let inner = value.get("i").unwrap().as_object().unwrap();

    assert_eq!(inner.get("x"), Some(&json!(0)));
}

/// `generate_static` for an array with `minItems` produces an array of length `minItems`.
#[test]
fn test_generate_static_array_min_items_length() {
    let schema = json!({
        "type": "array",
        "items": {"type": "integer"},
        "minItems": 3
    });

    let value = generate_static(&schema, &default_opts()).unwrap();
    let arr = value.as_array().unwrap();

    assert_eq!(arr.len(), 3);
}

/// `generate_static` for an integer array fills every slot with the integer type default `0`.
#[test]
fn test_generate_static_array_min_items_fills_with_zero() {
    let schema = json!({
        "type": "array",
        "items": {"type": "integer"},
        "minItems": 3
    });

    let value = generate_static(&schema, &default_opts()).unwrap();

    assert_eq!(value, json!([0, 0, 0]));
}

/// `generate_static` merges `allOf` sub-schemas into a single object with both sub-schema defaults.
#[test]
fn test_generate_static_merges_all_of() {
    let schema = json!({
        "allOf": [
            {"type": "object", "properties": {"a": {"type": "integer"}}, "required": ["a"]},
            {"type": "object", "properties": {"b": {"type": "string"}}, "required": ["b"]}
        ]
    });

    let value = generate_static(&schema, &default_opts()).unwrap();

    assert_eq!(value, json!({"a": 0, "b": ""}));
}

/// `generate_static` returns the schema's `default` value when `use_default_value` is on.
#[test]
fn test_generate_static_uses_default_value() {
    let schema = json!({"type": "string", "default": "hello"});
    let mut opts = default_opts();
    opts.use_default_value = true;

    let value = generate_static(&schema, &opts).unwrap();

    assert_eq!(value, json!("hello"));
}

/// `generate_static` returns the first `examples` entry when `use_examples_value` is on.
#[test]
fn test_generate_static_uses_examples_first() {
    let schema = json!({"type": "string", "examples": ["foo", "bar"]});
    let mut opts = default_opts();
    opts.use_examples_value = true;

    let value = generate_static(&schema, &opts).unwrap();

    assert_eq!(value, json!("foo"));
}

/// `generate_static` picks the first branch of an `anyOf` schema.
#[test]
fn test_generate_static_picks_any_of_first_branch() {
    let schema = json!({
        "anyOf": [{"type": "integer"}, {"type": "string"}]
    });

    let value = generate_static(&schema, &default_opts()).unwrap();

    assert_eq!(value, json!(0));
}

/// `output_transform` is also applied to the value produced by `generate_static`.
#[test]
fn test_output_transform_applied_for_generate_static() {
    let schema = json!({"type": "string"});
    let mut opts = default_opts();
    opts.output_transform = Some(Arc::new(|_v: &Value, _root: &Value| {
        Value::String("static-transformed".to_string())
    }));

    let value = generate_static(&schema, &opts).unwrap();

    assert_eq!(value, Value::String("static-transformed".to_string()));
}

/// `FakerError::UnresolvedRef` renders a Rust-idiomatic Display string.
#[test]
fn test_unresolved_ref_error_display() {
    let err = FakerError::UnresolvedRef {
        path: "#/components/schemas/Missing".to_string(),
    };

    let rendered = err.to_string();

    assert_eq!(rendered, "unresolved $ref: #/components/schemas/Missing");
}

/// `FakerError::UnresolvedRef` Display does not leak the Node-style ENOENT envelope.
#[test]
fn test_unresolved_ref_error_display_no_enoent() {
    let err = FakerError::UnresolvedRef {
        path: "#/components/schemas/Missing".to_string(),
    };

    let rendered = err.to_string();

    assert!(!rendered.contains("ENOENT"));
}

/// `define`/`invoke_extension` round-trip retrieves a globally-registered value.
#[test]
#[serial(registry)]
fn test_define_and_invoke_extension_round_trip() {
    chasm_faker::reset_extensions();
    chasm_faker::define(
        "test_lib_round_trip",
        Box::new(|_schema, _root, _prop, _path| json!("from-extension")),
    );

    let v = chasm_faker::invoke_extension("test_lib_round_trip", &json!({}), &json!({}), None, "");

    chasm_faker::reset_extensions();
    assert_eq!(v, Some(json!("from-extension")));
}

/// `reset_extensions` clears the globally-registered extension.
#[test]
#[serial(registry)]
fn test_reset_extensions_clears_registration() {
    chasm_faker::reset_extensions();
    chasm_faker::define(
        "to_reset",
        Box::new(|_schema, _root, _prop, _path| json!("present")),
    );
    chasm_faker::reset_extensions();

    let v = chasm_faker::invoke_extension("to_reset", &json!({}), &json!({}), None, "");

    assert!(v.is_none());
}

/// `invoke_extension` returns `None` for keywords that were never registered.
#[test]
#[serial(registry)]
fn test_invoke_extension_returns_none_for_unknown() {
    chasm_faker::reset_extensions();

    let v = chasm_faker::invoke_extension(
        "definitely_not_registered_xyz",
        &json!({}),
        &json!({}),
        None,
        "",
    );

    assert!(v.is_none());
}

/// A globally-defined `faker` extension is invoked from the schema walker.
#[test]
#[serial(registry)]
fn test_faker_keyword_invokes_global_extension() {
    chasm_faker::reset_extensions();
    chasm_faker::define(
        "faker",
        Box::new(
            |_schema: &Value, _root: &Value, _prop: Option<&str>, _path: &str| {
                Value::String("custom-faker-output".into())
            },
        ),
    );

    let schema = json!({"type": "string", "faker": "any.token"});
    let mut opts = default_opts();
    opts.seed = Some(1);
    let value = chasm_faker::generate(&schema, &opts).unwrap();

    chasm_faker::reset_extensions();
    assert_eq!(value, Value::String("custom-faker-output".into()));
}

/// A `faker` keyword with no registered extension falls through to type-based generation.
#[test]
#[serial(registry)]
fn test_faker_keyword_without_extension_falls_through_to_type() {
    chasm_faker::reset_extensions();
    let schema = json!({"type": "string", "faker": "person.fullName"});
    let mut opts = default_opts();
    opts.seed = Some(3);

    let value = chasm_faker::generate(&schema, &opts).unwrap();

    assert!(value.is_string());
}

/// Captured callback arguments for the four-argument extension probe.
type CapturedArgs = (Value, Value, Option<String>, String);

/// An extension callback receives all four arguments verbatim from `invoke_extension`.
#[test]
#[serial(registry)]
fn test_extension_callback_receives_four_args() {
    chasm_faker::reset_extensions();
    let captured: Arc<Mutex<Option<CapturedArgs>>> = Arc::new(Mutex::new(None));
    let captured_clone = Arc::clone(&captured);
    chasm_faker::define(
        "four_args_probe",
        Box::new(
            move |schema: &Value, root: &Value, prop: Option<&str>, path: &str| {
                *captured_clone.lock().unwrap() = Some((
                    schema.clone(),
                    root.clone(),
                    prop.map(|s| s.to_string()),
                    path.to_string(),
                ));
                Value::String("ok".into())
            },
        ),
    );

    let schema = json!({"type": "string", "faker": "x"});
    let root = json!({"$root": true});
    let _ = chasm_faker::invoke_extension(
        "four_args_probe",
        &schema,
        &root,
        Some("the_prop"),
        "#/properties/the_prop",
    );

    let recorded = captured.lock().unwrap().clone().expect("callback ran");
    chasm_faker::reset_extensions();
    assert_eq!(
        recorded,
        (
            schema,
            root,
            Some("the_prop".to_string()),
            "#/properties/the_prop".to_string()
        )
    );
}

/// A globally-registered format can be retrieved and invoked via `register_format`/`invoke_format`.
#[test]
#[serial(registry)]
fn test_register_format_round_trip() {
    chasm_faker::reset_formats();
    chasm_faker::register_format(
        "my-custom-format",
        Box::new(|_rng| Value::String("formatted-value".into())),
    );
    let mut rng = chasm_faker::__test_internals::Random::new(Some(7));

    let v = chasm_faker::invoke_format("my-custom-format", &mut rng);

    chasm_faker::reset_formats();
    assert_eq!(v, Some(Value::String("formatted-value".into())));
}

/// `invoke_format` returns `None` for names that have not been registered.
#[test]
#[serial(registry)]
fn test_invoke_format_returns_none_for_unregistered() {
    chasm_faker::reset_formats();
    let mut rng = chasm_faker::__test_internals::Random::new(Some(11));

    let v = chasm_faker::invoke_format("never-registered-format-xyz", &mut rng);

    assert!(v.is_none());
}

/// A panic inside a user-supplied extension callback propagates out of `invoke_extension`.
#[test]
#[serial(registry)]
fn test_extension_callback_panic_propagates() {
    chasm_faker::reset_extensions();
    chasm_faker::define(
        "poison-probe",
        Box::new(|_s: &Value, _r: &Value, _p: Option<&str>, _path: &str| {
            panic!("intentional panic to poison the global registry mutex");
        }),
    );

    let panic_result = std::thread::spawn(|| {
        chasm_faker::invoke_extension("poison-probe", &json!({}), &json!({}), None, "");
    })
    .join();

    chasm_faker::reset_extensions();
    assert!(panic_result.is_err());
}

/// After a callback panic poisons the registry mutex, subsequent `define` / `invoke_extension`
/// calls still succeed (the lock recovery path keeps the global registry usable).
#[test]
#[serial(registry)]
fn test_define_after_poisoned_mutex_still_works() {
    chasm_faker::reset_extensions();
    chasm_faker::define(
        "poison-probe",
        Box::new(|_s: &Value, _r: &Value, _p: Option<&str>, _path: &str| {
            panic!("intentional panic to poison the global registry mutex");
        }),
    );
    let _ = std::thread::spawn(|| {
        chasm_faker::invoke_extension("poison-probe", &json!({}), &json!({}), None, "");
    })
    .join();

    chasm_faker::define(
        "post-poison",
        Box::new(|_s: &Value, _r: &Value, _p: Option<&str>, _path: &str| {
            Value::String("survived".into())
        }),
    );
    let v = chasm_faker::invoke_extension("post-poison", &json!({}), &json!({}), None, "");

    chasm_faker::reset_extensions();
    assert_eq!(v, Some(Value::String("survived".into())));
}

/// The registry snapshot taken at `generate` entry isolates subsequent re-definitions.
#[test]
#[serial(registry)]
fn test_registry_snapshot_per_generate_call() {
    chasm_faker::reset_extensions();
    chasm_faker::define(
        "faker",
        Box::new(|_s: &Value, _r: &Value, _p: Option<&str>, _path: &str| Value::String("A".into())),
    );
    let schema = json!({"type": "string", "faker": "x.y"});
    let mut opts = default_opts();
    opts.seed = Some(7);
    let v1 = chasm_faker::generate(&schema, &opts).unwrap();
    assert_eq!(v1, Value::String("A".into()));

    chasm_faker::define(
        "faker",
        Box::new(|_s: &Value, _r: &Value, _p: Option<&str>, _path: &str| Value::String("B".into())),
    );
    let v2 = chasm_faker::generate(&schema, &opts).unwrap();

    chasm_faker::reset_extensions();
    assert_eq!(v2, Value::String("B".into()));
}

/// A schema declaring an unrecognised primitive `type` surfaces a `FakerError::InvalidType` variant
/// when `fail_on_invalid_type` is true.
#[test]
fn test_invalid_type_variant_matches() {
    let schema = json!({"type": "not-a-real-type"});
    let mut opts = default_opts();
    opts.fail_on_invalid_type = true;

    let err = generate(&schema, &opts).expect_err("expected InvalidType");

    assert!(matches!(err, FakerError::InvalidType { .. }));
}

/// `FakerError::InvalidType` Display includes the unknown type substring.
#[test]
fn test_invalid_type_display_contains_type() {
    let err = FakerError::InvalidType {
        value: "bogus".to_string(),
        path: "#/type".to_string(),
    };

    let rendered = err.to_string();

    assert!(rendered.contains("bogus"));
}

/// A schema where `additionalProperties: false` blocks required keys not declared in `properties`
/// surfaces a `FakerError::MissingProperties` variant.
#[test]
fn test_missing_properties_variant_matches() {
    let schema = json!({
        "type": "object",
        "properties": {"a": {"type": "string"}},
        "required": ["a", "b"],
        "additionalProperties": false
    });

    let err = generate(&schema, &default_opts()).expect_err("expected MissingProperties");

    assert!(matches!(err, FakerError::MissingProperties { .. }));
}

/// `FakerError::MissingProperties` Display includes the `missing properties` substring.
#[test]
fn test_missing_properties_display_contains_substring() {
    let err = FakerError::MissingProperties {
        target: "'x'".to_string(),
        path: "#".to_string(),
    };

    let rendered = err.to_string();

    assert!(rendered.contains("missing properties"));
}

/// An array schema with `uniqueItems`, a single-value enum, and `minItems` greater than 1 exhausts
/// the unique-item retry budget and surfaces a `FakerError::MissingItems` variant.
#[test]
fn test_missing_items_variant_matches() {
    let schema = json!({
        "type": "array",
        "items": {"enum": [1]},
        "uniqueItems": true,
        "minItems": 5
    });

    let err = generate(&schema, &default_opts()).expect_err("expected MissingItems");

    assert!(matches!(err, FakerError::MissingItems { .. }));
}

/// `FakerError::MissingItems` Display includes the `missing items` substring.
#[test]
fn test_missing_items_display_contains_substring() {
    let err = FakerError::MissingItems {
        target: "'3'".to_string(),
        path: "#".to_string(),
    };

    let rendered = err.to_string();

    assert!(rendered.contains("missing items"));
}

/// `FakerError::AdditionalPropertiesBlocked` Display includes the additionalProperties substring.
#[test]
fn test_additional_properties_blocked_display_contains_substring() {
    let err = FakerError::AdditionalPropertiesBlocked {
        name: "name".to_string(),
    };

    let rendered = err.to_string();

    assert!(rendered.contains("additionalProperties"));
}

/// A schema with `minProperties` exceeding the declared property count under
/// `additionalProperties: false` surfaces a `FakerError::AdditionalPropertiesBlocked` variant.
#[test]
fn test_additional_properties_blocked_variant_matches() {
    let schema = json!({
        "type": "object",
        "properties": {"only": {"type": "string"}},
        "required": ["only"],
        "additionalProperties": false,
        "minProperties": 3
    });

    let err = generate(&schema, &default_opts()).expect_err("expected AdditionalPropertiesBlocked");

    assert!(matches!(
        err,
        FakerError::AdditionalPropertiesBlocked { .. }
    ));
}

/// A schema with both `chance` and `faker` keywords surfaces a `FakerError::AmbiguousGenerator`.
#[test]
fn test_ambiguous_generator_variant_matches() {
    let schema = json!({"type": "string", "chance": {"word": []}, "faker": "lorem.word"});

    let err = generate(&schema, &seeded_opts(1)).expect_err("expected AmbiguousGenerator");

    assert!(matches!(err, FakerError::AmbiguousGenerator));
}

/// `FakerError::AmbiguousGenerator` Display includes the `ambiguous` substring.
#[test]
fn test_ambiguous_generator_display_contains_substring() {
    let err = FakerError::AmbiguousGenerator;

    let rendered = err.to_string();

    assert!(rendered.contains("ambiguous"));
}

/// A string schema with an unknown `format` plus `fail_on_invalid_format: true` surfaces a
/// `FakerError::UnknownRegistryKey` variant.
#[test]
fn test_unknown_registry_key_variant_matches() {
    let schema = json!({"type": "string", "format": "no-such-format-xyz"});
    let mut opts = seeded_opts(1);
    opts.fail_on_invalid_format = true;

    let err = generate(&schema, &opts).expect_err("expected UnknownRegistryKey");

    assert!(matches!(err, FakerError::UnknownRegistryKey { .. }));
}

/// `FakerError::UnknownRegistryKey` Display includes the unknown name substring.
#[test]
fn test_unknown_registry_key_display_contains_name() {
    let err = FakerError::UnknownRegistryKey {
        name: "no-such-format-xyz".to_string(),
        path: "#/format".to_string(),
    };

    let rendered = err.to_string();

    assert!(rendered.contains("no-such-format-xyz"));
}

/// A `faker:` key not in the built-in allow-list surfaces `FakerError::UnknownFakerGenerator`
/// under `fail_on_invalid_format: true`.
#[test]
fn test_unknown_faker_generator_variant_matches() {
    let schema = json!({"type": "string", "faker": "no.such.faker.namespace"});
    let mut opts = seeded_opts(1);
    opts.fail_on_invalid_format = true;

    let err = generate(&schema, &opts).expect_err("expected UnknownFakerGenerator");

    assert!(matches!(err, FakerError::UnknownFakerGenerator { .. }));
}

/// `FakerError::UnknownFakerGenerator` Display includes the unresolved name.
#[test]
fn test_unknown_faker_generator_display_contains_name() {
    let err = FakerError::UnknownFakerGenerator {
        name: "no.such.faker.namespace".to_string(),
        path: "#".to_string(),
    };

    let rendered = err.to_string();

    assert!(rendered.contains("no.such.faker.namespace"));
}

/// An `anyOf` whose every branch fails surfaces `FakerError::AllBranchesFailed`.
#[test]
fn test_all_branches_failed_variant_matches() {
    let schema = json!({
        "anyOf": [
            {"type": "bogus-a"},
            {"type": "bogus-b"}
        ]
    });
    let mut opts = seeded_opts(1);
    opts.fail_on_invalid_type = true;

    let err = generate(&schema, &opts).expect_err("expected AllBranchesFailed");

    assert!(matches!(err, FakerError::AllBranchesFailed { .. }));
}

/// `FakerError::AllBranchesFailed` Display includes the keyword (`anyOf` or `oneOf`).
#[test]
fn test_all_branches_failed_display_contains_keyword() {
    let err = FakerError::AllBranchesFailed {
        keyword: "anyOf",
        branch_count: 2,
        last_error: None,
    };

    let rendered = err.to_string();

    assert!(rendered.contains("anyOf"));
}

/// A literal `false` schema surfaces `FakerError::CannotGenerateForFalseSchema`.
#[test]
fn test_cannot_generate_for_false_schema_variant_matches() {
    let schema = json!(false);

    let err =
        generate(&schema, &default_opts()).expect_err("expected CannotGenerateForFalseSchema");

    assert!(matches!(err, FakerError::CannotGenerateForFalseSchema));
}

/// `FakerError::CannotGenerateForFalseSchema` Display includes the `false` substring.
#[test]
fn test_cannot_generate_for_false_schema_display_contains_substring() {
    let err = FakerError::CannotGenerateForFalseSchema;

    let rendered = err.to_string();

    assert!(rendered.contains("false"));
}

/// An unsatisfiable `not` constraint surfaces a `FakerError::SchemaError` variant.
#[test]
fn test_schema_error_variant_matches() {
    let schema = json!({
        "type": "string",
        "minLength": 1,
        "maxLength": 1,
        "pattern": "^a$",
        "not": {"type": "string"}
    });

    let err = generate(&schema, &seeded_opts(1)).expect_err("expected SchemaError");

    assert!(matches!(err, FakerError::SchemaError { .. }));
}

/// `FakerError::SchemaError` Display includes the inner message substring.
#[test]
fn test_schema_error_display_includes_message() {
    let err = FakerError::SchemaError {
        path: "/".to_string(),
        message: "custom message body".to_string(),
    };

    let rendered = err.to_string();

    assert!(rendered.contains("custom message body"));
}

/// A callback registered via `define` may itself call `define` without
/// deadlocking, because the registry mutex is released before the
/// callback runs.
#[test]
#[serial(registry)]
fn test_define_callback_can_reenter_define_without_deadlock() {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;
    let signal = Arc::new(Mutex::new(false));
    let signal_clone = signal.clone();
    chasm_faker::reset_extensions();
    chasm_faker::define(
        "outer",
        Box::new(move |_, _, _, _| {
            chasm_faker::define("inner", Box::new(|_, _, _, _| json!(0)));
            *signal_clone.lock().unwrap() = true;
            serde_json::json!("ok")
        }),
    );
    let done = Arc::new(AtomicBool::new(false));
    let done_clone = done.clone();
    let handle = std::thread::spawn(move || {
        let _ = chasm_faker::invoke_extension(
            "outer",
            &serde_json::Value::Null,
            &serde_json::Value::Null,
            None,
            "",
        );
        done_clone.store(true, Ordering::SeqCst);
    });
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    while !done.load(Ordering::SeqCst) && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(done.load(Ordering::SeqCst), "callback deadlocked");
    handle.join().unwrap();
    assert!(*signal.lock().unwrap(), "inner define did not run");
    chasm_faker::reset_extensions();
}

/// A format generator registered via `register_format` may itself call
/// `register_format` without deadlocking, because the format-registry mutex
/// is released before the user's generator runs.
#[test]
#[serial(registry)]
fn test_register_format_callback_can_reenter_without_deadlock() {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;
    let signal = Arc::new(Mutex::new(false));
    let signal_clone = signal.clone();
    chasm_faker::reset_formats();
    chasm_faker::register_format(
        "outer-format",
        Box::new(move |_rng| {
            chasm_faker::register_format(
                "inner-format",
                Box::new(|_rng| Value::String("inner".into())),
            );
            *signal_clone.lock().unwrap() = true;
            Value::String("outer".into())
        }),
    );
    let done = Arc::new(AtomicBool::new(false));
    let done_clone = done.clone();
    let handle = std::thread::spawn(move || {
        let mut rng = chasm_faker::__test_internals::Random::new(Some(1));
        let _ = chasm_faker::invoke_format("outer-format", &mut rng);
        done_clone.store(true, Ordering::SeqCst);
    });
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    while !done.load(Ordering::SeqCst) && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(done.load(Ordering::SeqCst), "format callback deadlocked");
    handle.join().unwrap();
    assert!(*signal.lock().unwrap(), "inner register_format did not run");
    chasm_faker::reset_formats();
}

/// `FakerError::AllBranchesFailed` exposes the captured last branch error via
/// the `std::error::Error::source()` chain so downstream consumers can inspect
/// the underlying cause.
#[test]
fn test_all_branches_failed_exposes_source() {
    use std::error::Error;
    let inner = FakerError::CannotGenerateForFalseSchema;
    let outer = FakerError::AllBranchesFailed {
        keyword: "oneOf",
        branch_count: 3,
        last_error: Some(Box::new(inner)),
    };

    let source = outer.source();

    assert!(
        source.is_some(),
        "expected AllBranchesFailed.source() to return the underlying error, got None"
    );
    let source_str = source.unwrap().to_string();
    assert!(
        source_str.contains("false"),
        "expected source error message to mention the false schema, got {:?}",
        source_str
    );
}
