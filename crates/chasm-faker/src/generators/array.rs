use crate::options::GenerateOptions;
use crate::random::Random;
use crate::schema_walker;
use serde_json::Value;

/// Max attempts to generate a unique array element when `uniqueItems` is set,
/// before giving up and accepting whatever length has been reached.
const UNIQUE_ITEM_RETRY_BUDGET: usize = 100;

/// Dispatches an item schema, intercepting the `x-faker` object form
/// `{ "helpers.arrayElement": [ ... ] }` so that an array-element helper
/// fires correctly when present on an item schema.
///
/// The naive interpretation of this shape would iterate the characters of
/// the first option (returning e.g. a random letter of "yard"). This
/// helper avoids that by recognising the `helpers.arrayElement` (and the
/// alias `random.arrayElement`) key on the schema's `x-faker` value and
/// returning one of the listed entries verbatim. Any other shape falls
/// through to the standard walker so existing top-level string-form
/// `x-faker` handling stays intact.
fn walk_item(
    item_schema: &Value,
    root: &Value,
    opts: &GenerateOptions,
    rng: &mut Random,
    depth: usize,
) -> Value {
    if let Some(picked) = pick_array_element_from_x_faker(item_schema, rng) {
        return picked;
    }
    schema_walker::walk(item_schema, root, opts, rng, depth)
}

/// Returns one randomly-chosen entry from an `x-faker` object of the form
/// `{ "helpers.arrayElement": [ ... ] }` (or the `random.arrayElement`
/// alias), or `None` when the schema does not carry that hint.
fn pick_array_element_from_x_faker(schema: &Value, rng: &mut Random) -> Option<Value> {
    let x_faker = schema.get("x-faker")?.as_object()?;
    for key in ["helpers.arrayElement", "random.arrayElement"] {
        if let Some(entries) = x_faker.get(key).and_then(|v| v.as_array()) {
            if entries.is_empty() {
                return None;
            }
            let idx = rng.int(0, (entries.len() as i64) - 1) as usize;
            return Some(entries[idx].clone());
        }
    }
    None
}

/// Generates a JSON array value respecting all array-related schema constraints.
///
/// Handles `items`, `prefixItems`, `additionalItems`, `minItems`, `maxItems`,
/// `uniqueItems`, and `contains`. Generates between `minItems` and `maxItems` elements.
/// Schema-level `maxItems` is always treated as a hard upper cap that cannot be exceeded
/// even when global options request more items.
pub fn generate(
    schema: &Value,
    root: &Value,
    opts: &GenerateOptions,
    rng: &mut Random,
    depth: usize,
) -> Value {
    let raw_schema_min = schema
        .get("minItems")
        .and_then(|v| v.as_u64())
        .map(|v| v as usize);
    let raw_schema_max = schema
        .get("maxItems")
        .and_then(|v| v.as_u64())
        .map(|v| v as usize);
    if let Some(rmin) = raw_schema_min {
        if rmin > crate::generators::string::MAX_GENERATED_ITEMS {
            let path = rng.current_path();
            rng.set_error(crate::FakerError::SchemaError {
                path,
                message: format!(
                    "array keyword minItems={} capped to {} for safety",
                    rmin,
                    crate::generators::string::MAX_GENERATED_ITEMS
                ),
            });
        }
    }
    if let Some(rmax) = raw_schema_max {
        if rmax > crate::generators::string::MAX_GENERATED_ITEMS {
            let path = rng.current_path();
            rng.set_error(crate::FakerError::SchemaError {
                path,
                message: format!(
                    "array keyword maxItems={} capped to {} for safety",
                    rmax,
                    crate::generators::string::MAX_GENERATED_ITEMS
                ),
            });
        }
    }
    let schema_min = raw_schema_min.map(|v| v.min(crate::generators::string::MAX_GENERATED_ITEMS));
    let schema_max = raw_schema_max.map(|v| v.min(crate::generators::string::MAX_GENERATED_ITEMS));
    let global_min = opts.min_items;
    let global_max = opts.max_items;

    let prefix_len = schema
        .get("prefixItems")
        .and_then(|v| v.as_array())
        .map(|a| a.len())
        .or_else(|| {
            schema
                .get("items")
                .and_then(|v| v.as_array())
                .map(|a| a.len())
        })
        .unwrap_or(0);

    let items_val = schema.get("items");

    let additional_items_prohibited = items_val
        .and_then(|v| v.as_bool())
        .map(|b| !b)
        .unwrap_or(false)
        || schema
            .get("additionalItems")
            .and_then(|v| v.as_bool())
            .map(|b| !b)
            .unwrap_or(false);

    let items_is_schema = items_val
        .map(|v| v.is_object() || v.as_bool().unwrap_or(false))
        .unwrap_or(false);

    let has_additional_schema = !additional_items_prohibited
        && (items_is_schema
            || schema
                .get("additionalItems")
                .map(|v| v.is_object() || v.as_bool().unwrap_or(false))
                .unwrap_or(false));

    let min_items = match (global_min, schema_min) {
        (Some(g), Some(s)) => g.max(s),
        (Some(g), None) => g,
        (None, Some(s)) => s,
        (None, None) => 0,
    };

    let max_items = match (global_max, schema_max) {
        (Some(g), Some(s)) => {
            if s > 0 {
                g.min(s)
            } else {
                s
            }
        }
        (Some(g), None) => g,
        (None, Some(s)) => s,
        (None, None) => {
            if prefix_len > 0 && !has_additional_schema {
                prefix_len
            } else if opts.always_fake_optionals {
                min_items.max(3)
            } else {
                3
            }
        }
    };

    let effective_min = min_items.max(prefix_len);
    let raw_max = max_items.max(effective_min);
    let safe_max = if let Some(smax) = schema_max {
        raw_max.min(smax)
    } else {
        raw_max
    };
    let effective_min_clamped = effective_min.min(safe_max);

    let target_count = if opts.always_fake_optionals {
        safe_max
    } else if let Some(p) = opts.optionals_probability {
        if (p - 1.0).abs() < f64::EPSILON {
            safe_max
        } else if p <= 0.0 {
            effective_min_clamped
        } else {
            let span = safe_max.saturating_sub(effective_min_clamped) as f64;
            effective_min_clamped + (span * p).round() as usize
        }
    } else {
        rng.int(effective_min_clamped as i64, safe_max as i64) as usize
    };

    let target_count = if opts.use_default_value {
        if let Some(cap) = opts.max_default_items {
            if items_val
                .and_then(|v| v.as_object())
                .map(|m| m.contains_key("default"))
                .unwrap_or(false)
            {
                target_count.min(cap).max(effective_min_clamped)
            } else {
                target_count
            }
        } else {
            target_count
        }
    } else {
        target_count
    };

    let unique_items = schema
        .get("uniqueItems")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let prefix_items: Option<&Vec<Value>> = schema
        .get("prefixItems")
        .and_then(|v| v.as_array())
        .or_else(|| {
            schema
                .get("items")
                .and_then(|v| if v.is_array() { v.as_array() } else { None })
        });

    let items_schema: Option<&Value> = schema.get("items").and_then(|v| {
        if v.is_object() || v.as_bool().is_some() {
            Some(v)
        } else {
            None
        }
    });

    let additional_items: Option<&Value> = schema.get("additionalItems");

    let mut result: Vec<Value> = Vec::new();

    let empty_obj = Value::Object(serde_json::Map::new());
    if let Some(prefix) = prefix_items {
        for (idx, item_schema) in prefix.iter().enumerate() {
            if idx >= target_count {
                break;
            }
            let val = walk_item(item_schema, root, opts, rng, depth + 1);
            result.push(val);
        }
        if !additional_items_prohibited {
            let additional_schema = additional_items
                .filter(|v| !v.as_bool().map(|b| !b).unwrap_or(false))
                .or(items_schema.filter(|v| !v.as_bool().map(|b| !b).unwrap_or(false)))
                .unwrap_or(&empty_obj);
            while result.len() < target_count {
                let val = walk_item(additional_schema, root, opts, rng, depth + 1);
                result.push(val);
            }
        }
    } else {
        let item_schema = items_schema.unwrap_or(&empty_obj);
        if unique_items {
            let mut attempts = 0;
            while result.len() < target_count && attempts < UNIQUE_ITEM_RETRY_BUDGET {
                let val = walk_item(item_schema, root, opts, rng, depth + 1);
                if !result.contains(&val) {
                    result.push(val);
                }
                attempts += 1;
            }
            if result.len() < effective_min_clamped {
                rng.set_error(crate::FakerError::MissingItems {
                    target: format!(
                        "'{}' (uniqueItems exhausted after retry budget; produced {} unique)",
                        effective_min_clamped,
                        result.len()
                    ),
                    path: rng.current_path(),
                });
            }
        } else {
            for _ in 0..target_count {
                let val = walk_item(item_schema, root, opts, rng, depth + 1);
                result.push(val);
            }
        }
    }

    if let Some(contains_schema) = schema.get("contains") {
        let ctx = ContainsContext {
            additional_items_prohibited,
            prefix_len,
            min_contains: schema
                .get("minContains")
                .and_then(|v| v.as_u64())
                .map(|v| v as usize)
                .unwrap_or(1),
            max_contains: schema
                .get("maxContains")
                .and_then(|v| v.as_u64())
                .map(|v| v as usize),
        };
        apply_contains_constraint(contains_schema, root, opts, rng, depth, &mut result, ctx);
    }

    if result.len() < target_count
        && items_val.and_then(|v| v.as_bool()) == Some(false)
        && prefix_len == 0
    {
        rng.set_error(crate::FakerError::MissingItems {
            target: format!("'{}'", target_count),
            path: rng.current_path(),
        });
    }

    if prefix_len > 0 && additional_items_prohibited && effective_min > prefix_len {
        rng.set_error(crate::FakerError::MissingItems {
            target: format!("'{}'", effective_min),
            path: rng.current_path(),
        });
    }

    Value::Array(result)
}

/// Enforces `contains`, `minContains`, and `maxContains` on a freshly generated array.
///
/// `minContains` defaults to 1 (drawing from JSON Schema 2019-09 semantics). The function
/// counts elements already satisfying `contains_schema`, then mutates existing slots or
/// appends new ones so the final count of satisfying elements lies in
/// `[min_contains, max_contains]`. When the array is empty and no slot exists, a single
/// satisfying element is appended.
///
/// When `additional_items_prohibited` is true (for example `items: false` or
/// `additionalItems: false`) AND the prefix length is less than `minContains`, the
/// `contains` requirement is contradictory with the items prohibition: no slot exists
/// for a contains-satisfying element. In that case, an error is recorded via
/// `rng.set_error` and no extra items are appended beyond what the prefix supports.
/// Surrounding-array context passed to `apply_contains_constraint` so the function
/// can short-circuit when the schema is contradictory and so the argument list stays
/// within clippy's default limit.
struct ContainsContext {
    additional_items_prohibited: bool,
    prefix_len: usize,
    min_contains: usize,
    max_contains: Option<usize>,
}

/// Adjusts a freshly generated array so that the count of elements satisfying
/// `contains_schema` lies in `[min_contains, max_contains]`, recording an error via
/// `rng.set_error` when the surrounding schema makes the constraint unsatisfiable.
fn apply_contains_constraint(
    contains_schema: &Value,
    root: &Value,
    opts: &GenerateOptions,
    rng: &mut Random,
    depth: usize,
    result: &mut Vec<Value>,
    ctx: ContainsContext,
) {
    let min_contains = ctx.min_contains;
    let max_contains = ctx.max_contains;

    // `maxContains: 0` forbids any matching element, but `minContains` defaults to 1
    // (and may be set explicitly higher). These cannot coexist; record an error and
    // skip the insertion logic so we do not append items the schema then disallows.
    if max_contains == Some(0) && min_contains > 0 {
        let path = rng.current_path();
        rng.set_error(crate::FakerError::SchemaError {
            path,
            message: format!(
                "contains constraint contradictory: maxContains: 0 cannot coexist with minContains: {}",
                min_contains
            ),
        });
        return;
    }

    if ctx.additional_items_prohibited && ctx.prefix_len < min_contains {
        let path = rng.current_path();
        rng.set_error(crate::FakerError::SchemaError {
            path,
            message: format!(
                "contradictory schema: minContains={} but items/additionalItems is false with prefix length {}",
                min_contains, ctx.prefix_len
            ),
        });
        return;
    }

    if result.is_empty() && min_contains > 0 {
        for _ in 0..min_contains {
            let val = schema_walker::walk(contains_schema, root, opts, rng, depth + 1);
            result.push(val);
        }
        return;
    }

    let matching: Vec<usize> = result
        .iter()
        .enumerate()
        .filter(|(_, v)| schema_walker::value_satisfies_simple_constraints(v, contains_schema))
        .map(|(i, _)| i)
        .collect();
    let current_count = matching.len();

    if current_count < min_contains {
        let needed = min_contains - current_count;
        let non_matching: Vec<usize> = (0..result.len())
            .filter(|i| !matching.contains(i))
            .collect();
        let mut produced = 0usize;
        for idx in non_matching.iter().take(needed) {
            let val = schema_walker::walk(contains_schema, root, opts, rng, depth + 1);
            result[*idx] = val;
            produced += 1;
        }
        while produced < needed {
            let val = schema_walker::walk(contains_schema, root, opts, rng, depth + 1);
            result.push(val);
            produced += 1;
        }
    } else if let Some(max) = max_contains {
        if current_count > max {
            let excess = current_count - max;
            let mut to_remove: Vec<usize> = matching.iter().take(excess).copied().collect();
            to_remove.sort_by(|a, b| b.cmp(a));
            for idx in to_remove {
                result.remove(idx);
            }
        }
    }
}
