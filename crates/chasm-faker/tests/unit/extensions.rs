//! Tests for `extensions.rs`.

use chasm_faker::__test_internals::Random;
use chasm_faker::extensions::{ExtensionRegistry, FormatRegistry};
use serde_json::{json, Value};

/// `define` followed by `get` returns a handler that can be invoked.
#[test]
fn test_registry_define_and_get_invokes_callback() {
    let mut reg = ExtensionRegistry::new();
    reg.define(
        "my_gen",
        Box::new(|_s: &Value, _r: &Value, _p: Option<&str>, _path: &str| Value::String("x".into())),
    );

    let handler = reg.get("my_gen").expect("expected registered handler");
    let value = handler(&json!({}), &json!({}), None, "");

    assert_eq!(value, Value::String("x".into()));
}

/// `get` returns `None` for keywords that were never registered.
#[test]
fn test_registry_get_returns_none_for_unknown_keyword() {
    let reg = ExtensionRegistry::new();

    assert!(reg.get("never_registered").is_none());
}

/// Calling `define` twice with the same keyword replaces the earlier handler.
#[test]
fn test_registry_define_overrides_previous_registration() {
    let mut reg = ExtensionRegistry::new();
    reg.define(
        "k",
        Box::new(|_s: &Value, _r: &Value, _p: Option<&str>, _path: &str| {
            Value::String("first".into())
        }),
    );
    reg.define(
        "k",
        Box::new(|_s: &Value, _r: &Value, _p: Option<&str>, _path: &str| {
            Value::String("second".into())
        }),
    );

    let handler = reg.get("k").unwrap();
    let v = handler(&json!({}), &json!({}), None, "");

    assert_eq!(v, Value::String("second".into()));
}

/// `reset` clears the first registered handler.
#[test]
fn test_registry_reset_clears_first_handler() {
    let mut reg = ExtensionRegistry::new();
    reg.define(
        "a",
        Box::new(|_s: &Value, _r: &Value, _p: Option<&str>, _path: &str| Value::Bool(true)),
    );
    reg.define(
        "b",
        Box::new(|_s: &Value, _r: &Value, _p: Option<&str>, _path: &str| Value::Bool(false)),
    );

    reg.reset();

    assert!(reg.get("a").is_none());
}

/// `reset` clears the second registered handler.
#[test]
fn test_registry_reset_clears_second_handler() {
    let mut reg = ExtensionRegistry::new();
    reg.define(
        "a",
        Box::new(|_s: &Value, _r: &Value, _p: Option<&str>, _path: &str| Value::Bool(true)),
    );
    reg.define(
        "b",
        Box::new(|_s: &Value, _r: &Value, _p: Option<&str>, _path: &str| Value::Bool(false)),
    );

    reg.reset();

    assert!(reg.get("b").is_none());
}

/// `ExtensionRegistry::default()` is equivalent to `new()`.
#[test]
fn test_registry_default_is_empty() {
    let reg = ExtensionRegistry::default();

    assert!(reg.get("anything").is_none());
}

/// The handler callback receives the schema, root, property name, and path arguments verbatim.
#[test]
fn test_registry_handler_receives_schema_root_prop_path() {
    let mut reg = ExtensionRegistry::new();
    reg.define(
        "echo",
        Box::new(
            |schema: &Value, root: &Value, prop: Option<&str>, path: &str| {
                json!({
                    "schema": schema.clone(),
                    "root": root.clone(),
                    "prop": prop.map(|s| s.to_string()),
                    "path": path.to_string(),
                })
            },
        ),
    );

    let schema = json!({"type": "integer"});
    let root = json!({"top": "level"});
    let handler = reg.get("echo").unwrap();
    let v = handler(&schema, &root, Some("field"), "#/properties/field");

    assert_eq!(v.get("schema").unwrap(), &schema);
}

/// The handler callback's `root` argument matches what was passed in.
#[test]
fn test_registry_handler_receives_root_verbatim() {
    let mut reg = ExtensionRegistry::new();
    reg.define(
        "echo",
        Box::new(
            |_schema: &Value, root: &Value, _prop: Option<&str>, _path: &str| {
                json!({"root": root.clone()})
            },
        ),
    );

    let root = json!({"top": "level"});
    let handler = reg.get("echo").unwrap();
    let v = handler(&json!({}), &root, None, "");

    assert_eq!(v.get("root").unwrap(), &root);
}

/// The handler callback's `prop` argument is passed through.
#[test]
fn test_registry_handler_receives_prop_verbatim() {
    let mut reg = ExtensionRegistry::new();
    reg.define(
        "echo",
        Box::new(
            |_schema: &Value, _root: &Value, prop: Option<&str>, _path: &str| {
                json!({"prop": prop.map(|s| s.to_string())})
            },
        ),
    );

    let handler = reg.get("echo").unwrap();
    let v = handler(&json!({}), &json!({}), Some("field"), "");

    assert_eq!(v.get("prop").unwrap(), &json!("field"));
}

/// The handler callback's `path` argument is passed through.
#[test]
fn test_registry_handler_receives_path_verbatim() {
    let mut reg = ExtensionRegistry::new();
    reg.define(
        "echo",
        Box::new(
            |_schema: &Value, _root: &Value, _prop: Option<&str>, path: &str| {
                json!({"path": path.to_string()})
            },
        ),
    );

    let handler = reg.get("echo").unwrap();
    let v = handler(&json!({}), &json!({}), None, "#/properties/field");

    assert_eq!(v.get("path").unwrap(), &json!("#/properties/field"));
}

/// Cloning `ExtensionRegistry` snapshots its handler set via `Arc::clone` so a later `reset`
/// on the original does not erase handlers held by the clone.
#[test]
fn test_extension_registry_clone_uses_arc_bump() {
    let mut original = ExtensionRegistry::new();
    original.define(
        "k",
        Box::new(|_s: &Value, _r: &Value, _p: Option<&str>, _path: &str| Value::Bool(true)),
    );
    let snapshot = original.clone();

    original.reset();

    assert!(snapshot.get("k").is_some());
}

/// `FormatRegistry::define` followed by `get` returns a handler that can be invoked.
#[test]
fn test_format_registry_define_and_get_invokes_callback() {
    let mut reg = FormatRegistry::new();
    reg.define(
        "my_fmt",
        Box::new(|_r: &mut Random| Value::String("ok".into())),
    );

    let handler = reg.get("my_fmt").expect("expected registered format");
    let mut rng = Random::new(Some(0));
    let value = handler(&mut rng);

    assert_eq!(value, Value::String("ok".into()));
}

/// `FormatRegistry::get` returns `None` for format names that were never registered.
#[test]
fn test_format_registry_get_returns_none_for_unknown() {
    let reg = FormatRegistry::new();

    assert!(reg.get("not-registered").is_none());
}

/// `FormatRegistry::reset` clears the first registered format generator.
#[test]
fn test_format_registry_reset_clears_first_handler() {
    let mut reg = FormatRegistry::new();
    reg.define("a", Box::new(|_r: &mut Random| Value::Null));
    reg.define("b", Box::new(|_r: &mut Random| Value::Null));

    reg.reset();

    assert!(reg.get("a").is_none());
}

/// `FormatRegistry::reset` clears the second registered format generator.
#[test]
fn test_format_registry_reset_clears_second_handler() {
    let mut reg = FormatRegistry::new();
    reg.define("a", Box::new(|_r: &mut Random| Value::Null));
    reg.define("b", Box::new(|_r: &mut Random| Value::Null));

    reg.reset();

    assert!(reg.get("b").is_none());
}

/// `FormatRegistry::default()` is equivalent to `new()` — an empty registry.
#[test]
fn test_format_registry_default_is_empty() {
    let reg = FormatRegistry::default();

    assert!(reg.get("anything").is_none());
}
