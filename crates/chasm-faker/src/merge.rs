//! Deep merge helpers for JSON-Schema-shaped values.
//!
//! Powers the `allOf` collapse the static-mode walker performs before
//! dispatch: subschemas are merged into one object, with composition
//! keywords (`required`, `allOf`, `anyOf`, `oneOf`) concatenated rather
//! than last-write-wins-overwritten.

use serde_json::Value;

/// Keys whose array values should be concatenated rather than replaced when merging schemas.
const ARRAY_CONCAT_KEYS: &[&str] = &["required", "allOf", "anyOf", "oneOf"];

/// Recursively merges two JSON values, with `overlay` taking precedence over `base`.
///
/// For objects, keys from `overlay` overwrite matching keys in `base` and new keys are added.
/// For schema-specific array keywords like `required`, arrays are concatenated (deduplicated).
/// For all other arrays and scalar types, `overlay` completely replaces `base`.
pub fn deep_merge(base: Value, overlay: Value) -> Value {
    match (base, overlay) {
        (Value::Object(mut base_map), Value::Object(overlay_map)) => {
            for (key, overlay_val) in overlay_map {
                let merged = match base_map.remove(&key) {
                    Some(base_val) => {
                        if ARRAY_CONCAT_KEYS.contains(&key.as_str()) {
                            merge_arrays_dedup(base_val, overlay_val)
                        } else {
                            deep_merge(base_val, overlay_val)
                        }
                    }
                    None => overlay_val,
                };
                base_map.insert(key, merged);
            }
            Value::Object(base_map)
        }
        (_, overlay) => overlay,
    }
}

/// Merges two JSON arrays by concatenating and deduplicating their elements.
fn merge_arrays_dedup(base: Value, overlay: Value) -> Value {
    let mut combined = Vec::new();
    if let Value::Array(arr) = base {
        combined.extend(arr);
    }
    if let Value::Array(arr) = overlay {
        for item in arr {
            if !combined.contains(&item) {
                combined.push(item);
            }
        }
    }
    Value::Array(combined)
}
