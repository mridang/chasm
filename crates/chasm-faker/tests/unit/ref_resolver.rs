//! Tests for `ref_resolver.rs`.

use crate::common::{default_opts, seeded_opts};
use chasm_faker::__test_internals::RefResolver;
use chasm_faker::{generate, FakerError};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;

/// A nested `$id` is resolved against its parent's `$id` base URI for absolute lookups.
#[test]
fn test_nested_id_resolves_against_parent_base_uri() {
    let schema = json!({
        "$id": "https://example.com/schema",
        "$defs": {
            "Foo": {
                "$id": "foo-sub",
                "type": "object",
                "properties": {"x": {"type": "integer"}}
            }
        }
    });
    let resolver = RefResolver::new(&schema);
    let mut visited = Vec::new();

    let resolved = resolver
        .resolve("https://example.com/foo-sub", &mut visited)
        .expect("expected nested $id to resolve against parent base URI");

    assert_eq!(resolved.get("type"), Some(&json!("object")));
}

/// The literal nested `$id` value remains resolvable alongside the composed absolute form.
#[test]
fn test_nested_id_literal_still_resolves() {
    let schema = json!({
        "$id": "https://example.com/schema",
        "$defs": {
            "Foo": {"$id": "foo-sub", "type": "string"}
        }
    });
    let resolver = RefResolver::new(&schema);
    let mut visited = Vec::new();

    let resolved = resolver
        .resolve("foo-sub", &mut visited)
        .expect("expected literal $id to remain resolvable");

    assert_eq!(resolved.get("type"), Some(&json!("string")));
}

/// A schema referencing `#/$defs/address` resolves and emits the `street` required property.
#[test]
fn test_ref_to_defs_populates_street() {
    let schema = ref_to_defs_address_schema();

    let value = generate(&schema, &default_opts()).unwrap();
    let billing = value.get("billing_address").unwrap().as_object().unwrap();

    assert!(billing.contains_key("street"));
}

/// A schema referencing `#/$defs/address` resolves and emits the `city` required property.
#[test]
fn test_ref_to_defs_populates_city() {
    let schema = ref_to_defs_address_schema();

    let value = generate(&schema, &default_opts()).unwrap();
    let billing = value.get("billing_address").unwrap().as_object().unwrap();

    assert!(billing.contains_key("city"));
}

/// A schema referencing `#/$defs/address` resolves and emits the `state` required property.
#[test]
fn test_ref_to_defs_populates_state() {
    let schema = ref_to_defs_address_schema();

    let value = generate(&schema, &default_opts()).unwrap();
    let billing = value.get("billing_address").unwrap().as_object().unwrap();

    assert!(billing.contains_key("state"));
}

/// Fixture: schema whose `billing_address` resolves to a three-required-key address object.
fn ref_to_defs_address_schema() -> serde_json::Value {
    json!({
        "$defs": {
            "address": {
                "type": "object",
                "properties": {
                    "street": {"type": "string"},
                    "city": {"type": "string"},
                    "state": {"type": "string"}
                },
                "required": ["street", "city", "state"]
            }
        },
        "type": "object",
        "properties": {"billing_address": {"$ref": "#/$defs/address"}},
        "required": ["billing_address"]
    })
}

/// `$id`-based references resolve correctly via inner-reference lookup.
#[test]
fn test_id_based_ref_resolves() {
    let schema = json!({
        "type": "object",
        "properties": {
            "test": {
                "$id": "foo:bar",
                "type": "object",
                "properties": {"foo": {"type": "string", "enum": ["x"]}},
                "required": ["foo"]
            },
            "other": {"$ref": "foo:bar"}
        },
        "required": ["test", "other"]
    });

    let value = generate(&schema, &default_opts()).unwrap();
    let test_val = value.get("test").unwrap().as_object().unwrap();

    assert_eq!(test_val.get("foo"), Some(&json!("x")));
}

/// `$anchor` references resolve to the anchored sub-schema instead of the root.
#[test]
fn test_anchor_reference_resolves_to_sub_schema() {
    let schema = json!({
        "type": "object",
        "properties": {"x": {"$ref": "#named"}},
        "required": ["x"],
        "$defs": {
            "Thing": {"$anchor": "named", "type": "string", "const": "anchored"}
        }
    });

    let value = generate(&schema, &default_opts()).unwrap();

    assert_eq!(value["x"], "anchored");
}

/// Sibling keywords on a `$ref` are merged with the resolved target.
#[test]
fn test_ref_sibling_keywords_merged() {
    let schema = json!({
        "type": "object",
        "properties": {
            "name": {"$ref": "#/$defs/Name", "minLength": 5}
        },
        "required": ["name"],
        "$defs": {"Name": {"type": "string"}}
    });

    let value = generate(&schema, &seeded_opts(7)).unwrap();
    let s = value["name"].as_str().expect("expected string");

    assert!(s.chars().count() >= 5);
}

/// An unresolved internal `$ref` surfaces a `FakerError::UnresolvedRef`.
#[test]
fn test_internal_unresolved_ref_surfaces_error() {
    let schema = json!({
        "type": "object",
        "properties": {"x": {"$ref": "#/$defs/missing"}},
        "required": ["x"]
    });

    let err = generate(&schema, &default_opts())
        .expect_err("expected UnresolvedRef for missing internal ref");

    match err {
        FakerError::UnresolvedRef { path } => assert!(path.contains("missing")),
        other => panic!("expected UnresolvedRef, got {:?}", other),
    }
}

/// An inline schema whose `$ref` points inside `components.schemas` resolves and contains `id`.
#[test]
fn test_components_schema_ref_contains_id() {
    let root = components_pet_root_schema();
    let mut opts = seeded_opts(7);
    opts.always_fake_optionals = true;

    let value = generate(&root, &opts).unwrap();
    let obj = value.as_object().expect("expected a generated object");

    assert!(obj.contains_key("id"));
}

/// An inline schema whose `$ref` points inside `components.schemas` resolves and contains `name`.
#[test]
fn test_components_schema_ref_contains_name() {
    let root = components_pet_root_schema();
    let mut opts = seeded_opts(7);
    opts.always_fake_optionals = true;

    let value = generate(&root, &opts).unwrap();
    let obj = value.as_object().expect("expected a generated object");

    assert!(obj.contains_key("name"));
}

/// Fixture: root schema whose `$ref` resolves to `components.schemas.Pet`.
fn components_pet_root_schema() -> serde_json::Value {
    json!({
        "$ref": "#/components/schemas/Pet",
        "components": {
            "schemas": {
                "Pet": {
                    "type": "object",
                    "required": ["id", "name"],
                    "properties": {
                        "id": {"type": "integer"},
                        "name": {"type": "string", "minLength": 1, "maxLength": 8},
                        "tag": {"type": "string"}
                    }
                }
            }
        }
    })
}

/// An array schema whose `items` is a `$ref` produces an array of real objects.
#[test]
fn test_array_of_ref_produces_non_null_objects() {
    let root = json!({
        "type": "array",
        "minItems": 2,
        "maxItems": 2,
        "items": {"$ref": "#/components/schemas/Pet"},
        "components": {
            "schemas": {
                "Pet": {
                    "type": "object",
                    "required": ["id", "name"],
                    "properties": {
                        "id": {"type": "integer"},
                        "name": {"type": "string", "minLength": 1, "maxLength": 8}
                    }
                }
            }
        }
    });
    let mut opts = seeded_opts(11);
    opts.always_fake_optionals = true;

    let value = generate(&root, &opts).unwrap();
    let arr = value.as_array().expect("expected an array value");

    for item in arr.iter() {
        assert!(!item.is_null());
    }
}

/// A self-referential `$ref` at the depth cutoff does not stack-overflow.
#[test]
fn test_self_ref_at_depth_cutoff_is_safe() {
    let schema = json!({
        "type": "object",
        "properties": {"child": {"$ref": "#"}},
        "required": ["child"]
    });
    let mut opts = default_opts();
    opts.max_depth = 3;

    let value = generate(&schema, &opts).unwrap();

    assert!(value.is_object());
}

/// `ref_depth_max` bounds recursion in a recursive self-referential schema.
#[test]
fn test_ref_depth_window_bounds_recursion() {
    let schema = json!({
        "$ref": "#/$defs/Node",
        "$defs": {
            "Node": {
                "type": "object",
                "properties": {"child": {"$ref": "#/$defs/Node"}},
                "required": ["child"]
            }
        }
    });
    let mut opts = seeded_opts(17);
    opts.ref_depth_max = 2;

    let value = generate(&schema, &opts).unwrap();

    let mut current = &value;
    let mut depth = 0usize;
    while let Some(child) = current.get("child") {
        if child.is_null() || !child.is_object() {
            break;
        }
        depth += 1;
        if depth > 50 {
            panic!("recursion did not terminate within depth budget");
        }
        current = child;
    }
    assert!(
        depth <= 3,
        "ref_depth_max=2 should bound traversal at depth<=3, got depth={depth}"
    );
}

/// `RefResolver::new_with_id_map` honours a pre-built `$id` index, so an externally
/// constructed map drives lookups even when the root has no `$id` declarations of its own.
#[test]
fn test_new_with_id_map_uses_external_index() {
    let root = json!({
        "$defs": {
            "Thing": {"type": "string", "const": "hello"}
        }
    });
    let mut map: HashMap<String, Vec<String>> = HashMap::new();
    map.insert(
        "urn:example:thing".to_string(),
        vec!["$defs".to_string(), "Thing".to_string()],
    );
    let resolver = RefResolver::new_with_id_map(&root, Arc::new(map));
    let mut visited = Vec::new();

    let resolved = resolver
        .resolve("urn:example:thing", &mut visited)
        .expect("expected pre-built id_map to drive resolution");

    assert_eq!(resolved.get("const"), Some(&json!("hello")));
}

/// `with_depth_window` records the configured `min` floor, surfaced by `min_depth()`.
#[test]
fn test_with_depth_window_records_min_depth() {
    let root = json!({});

    let resolver = RefResolver::new(&root).with_depth_window(3, 8);

    assert_eq!(resolver.min_depth(), 3);
}

/// A JSON-pointer `$ref` containing the `~1` escape decodes the segment to `/` for the lookup.
#[test]
fn test_resolve_decodes_tilde_one_escape() {
    let root = json!({
        "paths": {
            "/things": {"type": "object", "marker": "slash-key"}
        }
    });
    let resolver = RefResolver::new(&root);
    let mut visited = Vec::new();

    let resolved = resolver
        .resolve("#/paths/~1things", &mut visited)
        .expect("expected ~1 to decode into a `/` segment");

    assert_eq!(resolved.get("marker"), Some(&json!("slash-key")));
}

/// A JSON-pointer `$ref` containing the `~0` escape decodes the segment to `~` for the lookup.
#[test]
fn test_resolve_decodes_tilde_zero_escape() {
    let root = json!({
        "weird~key": {"marker": "tilde-key"}
    });
    let resolver = RefResolver::new(&root);
    let mut visited = Vec::new();

    let resolved = resolver
        .resolve("#/weird~0key", &mut visited)
        .expect("expected ~0 to decode into a `~` segment");

    assert_eq!(resolved.get("marker"), Some(&json!("tilde-key")));
}

/// `with_external` consults a pre-loaded substitution map before falling back to the in-document `$id` index.
#[test]
fn test_with_external_substitutes_pre_loaded_target() {
    let root = json!({});
    let target = json!({"type": "string", "marker": "from-external"});
    let mut external: HashMap<String, Value> = HashMap::new();
    external.insert("https://example.com/x.yaml".to_string(), target);
    let resolver = RefResolver::new(&root).with_external(&external);
    let mut visited = Vec::new();

    let resolved = resolver
        .resolve("https://example.com/x.yaml", &mut visited)
        .expect("expected external substitution to resolve");

    assert_eq!(resolved.get("marker"), Some(&json!("from-external")));
}

/// `collect_ids` also indexes the older unprefixed `id` keyword used by JSON Schema draft-04,
/// so a `$ref` to that bare id still resolves to the declaring subschema.
#[test]
fn test_collect_ids_indexes_legacy_unprefixed_id() {
    let root = json!({
        "$defs": {
            "Thing": {"id": "legacy-thing", "type": "string", "marker": "legacy"}
        }
    });
    let resolver = RefResolver::new(&root);
    let mut visited = Vec::new();

    let resolved = resolver
        .resolve("legacy-thing", &mut visited)
        .expect("expected legacy `id` keyword to be indexed");

    assert_eq!(resolved.get("marker"), Some(&json!("legacy")));
}

/// In a two-step `$ref` cycle (A → B → A), the first inner hop from B still
/// resolves to the target node before the cycle is detected.
#[test]
fn test_cycle_detection_first_inner_hop_resolves() {
    let root = json!({
        "$defs": {
            "A": {"$ref": "#/$defs/B"},
            "B": {"$ref": "#/$defs/A"}
        }
    });
    let resolver = RefResolver::new(&root);
    let mut visited = Vec::new();
    let first = resolver
        .resolve("#/$defs/A", &mut visited)
        .expect("first hop must resolve to the B node");

    let inner_ref = first.get("$ref").and_then(|v| v.as_str()).unwrap_or("");
    let cycled = resolver.resolve(inner_ref, &mut visited);

    assert!(cycled.is_some(), "first inner hop must resolve");
}

/// In a two-step `$ref` cycle (A → B → A), re-resolving the original target
/// node returns `None` because the `visited` set short-circuits the cycle.
#[test]
fn test_cycle_detection_revisit_returns_none() {
    let root = json!({
        "$defs": {
            "A": {"$ref": "#/$defs/B"},
            "B": {"$ref": "#/$defs/A"}
        }
    });
    let resolver = RefResolver::new(&root);
    let mut visited = Vec::new();
    let first = resolver
        .resolve("#/$defs/A", &mut visited)
        .expect("first hop must resolve to the B node");

    let inner_ref = first.get("$ref").and_then(|v| v.as_str()).unwrap_or("");
    let _ = resolver.resolve(inner_ref, &mut visited);
    let again = resolver.resolve("#/$defs/A", &mut visited);

    assert!(again.is_none(), "second hop must be cycle-suppressed");
}
