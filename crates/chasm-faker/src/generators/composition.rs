use crate::merge::deep_merge;
use crate::options::GenerateOptions;
use crate::random::Random;
use crate::schema_walker;
use crate::FakerError;
use serde_json::Value;

/// Caps how many `anyOf` / `oneOf` sub-schemas chasm will attempt per node,
/// defending against CPU DoS via specs containing thousands of branches.
///
/// The first `MAX_COMPOSITION_BRANCHES` branches are considered; any beyond
/// that are ignored with a `tracing::warn!`. `allOf` is intentionally exempt
/// from this cap: `allOf` semantics require merging every branch, so capping
/// would silently produce values that violate a valid spec.
const MAX_COMPOSITION_BRANCHES: usize = 64;

/// Generates a value satisfying all sub-schemas in an `allOf` keyword.
///
/// Resolves all `$ref` references, collects `contains` constraints from each
/// sub-schema separately, merges the remaining sub-schemas, and then applies
/// each `contains` constraint to a distinct position in the result array.
pub fn generate_all_of(
    schema: &Value,
    root: &Value,
    opts: &GenerateOptions,
    rng: &mut Random,
    depth: usize,
) -> Value {
    let sub_schemas = match schema.get("allOf").and_then(|v| v.as_array()) {
        Some(arr) => arr,
        None => return Value::Null,
    };

    let resolver = crate::ref_resolver::RefResolver::new(root)
        .with_external(&opts.external_refs)
        .with_depth_window(opts.ref_depth_min, opts.ref_depth_max);
    let mut merged_schema = Value::Object(serde_json::Map::new());
    let mut extra_contains: Vec<Value> = Vec::new();

    for sub in sub_schemas {
        let mut resolved = resolve_if_ref(sub, &resolver);
        if let Value::Object(ref mut m) = resolved {
            if let Some(c) = m.remove("contains") {
                extra_contains.push(c);
            }
        }
        merged_schema = deep_merge(merged_schema, resolved);
    }

    let mut result = schema_walker::walk(&merged_schema, root, opts, rng, depth + 1);

    for (i, contains_schema) in extra_contains.iter().enumerate() {
        if let Value::Array(ref mut arr) = result {
            if arr.len() > i {
                arr[i] = schema_walker::walk(contains_schema, root, opts, rng, depth + 1);
            } else {
                let val = schema_walker::walk(contains_schema, root, opts, rng, depth + 1);
                arr.push(val);
            }
        }
    }

    result
}

/// Resolves a `$ref` schema to its target value, or returns a clone of the original.
fn resolve_if_ref(schema: &Value, resolver: &crate::ref_resolver::RefResolver<'_>) -> Value {
    if let Some(Value::String(ref_str)) = schema.get("$ref") {
        let mut visited = Vec::new();
        if let Some(resolved) = resolver.resolve(ref_str.as_str(), &mut visited) {
            return resolved.clone();
        }
    }
    schema.clone()
}

/// Generates a value satisfying one randomly chosen sub-schema in an `anyOf` keyword.
///
/// Mirrors upstream json-schema-faker by shuffling branches and trying each in turn;
/// when a branch records a generator error in `rng`, the error is cleared and the
/// next branch is attempted. Returns `Value::Null` with `AllAnyOfBranchesFailed`
/// recorded when every branch fails. When recursion is near the depth limit, the
/// branch order is biased to start from a terminal-friendly sub-schema so generation
/// can bottom out without recursing further through `$ref`.
pub fn generate_any_of(
    schema: &Value,
    root: &Value,
    opts: &GenerateOptions,
    rng: &mut Random,
    depth: usize,
) -> Value {
    let sub_schemas = match schema.get("anyOf").and_then(|v| v.as_array()) {
        Some(arr) => arr,
        None => return Value::Null,
    };
    if sub_schemas.is_empty() {
        return Value::Null;
    }
    let sub_schemas: &[Value] = if sub_schemas.len() > MAX_COMPOSITION_BRANCHES {
        tracing::warn!(
            branch_count = sub_schemas.len(),
            cap = MAX_COMPOSITION_BRANCHES,
            "anyOf branch count exceeds cap; ignoring excess branches"
        );
        &sub_schemas[..MAX_COMPOSITION_BRANCHES]
    } else {
        sub_schemas.as_slice()
    };

    let order = shuffled_branch_order(sub_schemas, opts, rng, depth);
    let branch_count = sub_schemas.len();
    let mut last_error: Option<FakerError> = None;
    for idx in order {
        let attempt = schema_walker::walk(&sub_schemas[idx], root, opts, rng, depth + 1);
        if rng.has_error() {
            last_error = rng.take_error();
            continue;
        }
        return apply_discriminator(schema, &sub_schemas[idx], attempt);
    }
    rng.set_error(FakerError::AllBranchesFailed {
        keyword: "anyOf",
        branch_count,
        last_error: last_error.map(Box::new),
    });
    Value::Null
}

/// Returns an ordering over `sub_schemas` for try-each iteration.
///
/// The first index respects depth-aware preferences (terminal-friendly branches when
/// recursion is bounded); subsequent indices follow a Fisher-Yates shuffle so that
/// generation falls back through the remaining branches in randomised order.
fn shuffled_branch_order(
    sub_schemas: &[Value],
    opts: &GenerateOptions,
    rng: &mut Random,
    depth: usize,
) -> Vec<usize> {
    let n = sub_schemas.len();
    let mut order: Vec<usize> = (0..n).collect();
    for i in (1..n).rev() {
        let j = rng.pick_index(i + 1);
        order.swap(i, j);
    }
    let first = pick_branch_with_depth(sub_schemas, opts, rng, depth);
    if let Some(pos) = order.iter().position(|&i| i == first) {
        order.swap(0, pos);
    }
    order
}

/// Generates a value satisfying one randomly chosen sub-schema in a `oneOf` keyword.
///
/// Mirrors upstream json-schema-faker by walking the chosen branch under an options
/// override that disables optional-property inclusion (`optionals_probability = 0`
/// and `always_fake_optionals = false`), so the produced value contains only the
/// required keys of the chosen branch and is less likely to accidentally satisfy
/// any of the other alternatives. When recursion is close to the depth limit, a
/// terminal-friendly sub-schema is preferred over recursive `$ref` branches.
pub fn generate_one_of(
    schema: &Value,
    root: &Value,
    opts: &GenerateOptions,
    rng: &mut Random,
    depth: usize,
) -> Value {
    let sub_schemas = match schema.get("oneOf").and_then(|v| v.as_array()) {
        Some(arr) => arr,
        None => return Value::Null,
    };
    if sub_schemas.is_empty() {
        return Value::Null;
    }
    let sub_schemas: &[Value] = if sub_schemas.len() > MAX_COMPOSITION_BRANCHES {
        tracing::warn!(
            branch_count = sub_schemas.len(),
            cap = MAX_COMPOSITION_BRANCHES,
            "oneOf branch count exceeds cap; ignoring excess branches"
        );
        &sub_schemas[..MAX_COMPOSITION_BRANCHES]
    } else {
        sub_schemas.as_slice()
    };
    let idx = pick_branch_with_depth(sub_schemas, opts, rng, depth);
    let exclusive_opts = GenerateOptions {
        optionals_probability: Some(0.0),
        always_fake_optionals: false,
        ..opts.clone()
    };
    let produced = schema_walker::walk(&sub_schemas[idx], root, &exclusive_opts, rng, depth + 1);
    let final_value = apply_discriminator(schema, &sub_schemas[idx], produced);
    warn_oneof_exclusivity_violation(sub_schemas, idx, &final_value);
    final_value
}

/// Emits a `tracing::debug!` event when the value produced by branch `idx` of a
/// `oneOf` also satisfies one or more of its siblings, surfacing the otherwise
/// silent spec violation without trying to retry or re-pick.
///
/// `oneOf` semantics require the value to satisfy exactly one branch; detecting
/// every form of overlap is in NP-hard territory, so this best-effort check
/// uses [`schema_walker::value_satisfies_simple_constraints`] to cover the
/// scalar and enum cases that are cheap to verify. A logged-but-accepted value
/// is still returned to the caller because rejecting it provides no path
/// forward when every branch overlaps.
fn warn_oneof_exclusivity_violation(sub_schemas: &[Value], chosen_idx: usize, value: &Value) {
    let mut overlapping: Vec<usize> = Vec::new();
    for (i, branch) in sub_schemas.iter().enumerate() {
        if i == chosen_idx {
            continue;
        }
        if schema_walker::value_satisfies_simple_constraints(value, branch) {
            overlapping.push(i);
        }
    }
    if !overlapping.is_empty() {
        tracing::debug!(
            target: "chasm_faker::generators::composition",
            chosen_idx,
            overlapping_branches = ?overlapping,
            "oneOf produced value satisfies more than one branch; strict exclusivity violated"
        );
    }
}

/// Injects an OAS 3.0 / 3.1 `discriminator.propertyName` value into an object produced
/// by a `oneOf`/`anyOf` branch.
///
/// When `parent_schema.discriminator` is present and the produced value is an object,
/// the discriminator value is chosen by, in order: (a) the key in `discriminator.mapping`
/// whose value matches the chosen branch's `$ref`; (b) the last path segment of the
/// chosen branch's `$ref` (e.g. `#/components/schemas/Cat` → `Cat`); (c) the chosen
/// branch's literal `properties.<propertyName>.const`. When none apply, the value is
/// returned unchanged.
fn apply_discriminator(parent_schema: &Value, chosen_branch: &Value, value: Value) -> Value {
    let discriminator = match parent_schema.get("discriminator") {
        Some(d) => d,
        None => return value,
    };
    let property_name = match discriminator.get("propertyName").and_then(|v| v.as_str()) {
        Some(name) => name,
        None => return value,
    };
    let mut obj = match value {
        Value::Object(map) => map,
        other => return other,
    };

    let branch_ref = chosen_branch.get("$ref").and_then(|v| v.as_str());
    let mapping = discriminator.get("mapping").and_then(|v| v.as_object());

    let mut discriminator_value: Option<String> = None;
    if let (Some(ref_str), Some(map)) = (branch_ref, mapping) {
        for (key, mapped) in map.iter() {
            if mapped.as_str() == Some(ref_str) {
                discriminator_value = Some(key.clone());
                break;
            }
        }
    }
    if discriminator_value.is_none() {
        if let Some(ref_str) = branch_ref {
            if let Some(last) = ref_str.rsplit('/').next() {
                if !last.is_empty() {
                    discriminator_value = Some(last.to_string());
                }
            }
        }
    }
    if discriminator_value.is_none() {
        if let Some(constant) = chosen_branch
            .get("properties")
            .and_then(|p| p.get(property_name))
            .and_then(|p| p.get("const"))
        {
            if let Some(s) = constant.as_str() {
                discriminator_value = Some(s.to_string());
            }
        }
    }

    if let Some(value_str) = discriminator_value {
        obj.insert(property_name.to_string(), Value::String(value_str));
    }
    Value::Object(obj)
}

/// Picks a branch index, preferring terminal-friendly branches when near the depth limit
/// or when a `$ref` branch would trigger recursive expansion deeper than the budget allows.
///
/// When a self-referential `$ref` alternative exists alongside a non-recursive branch,
/// the non-recursive branch is preferred at any non-zero depth so the produced value
/// still satisfies the composition once recursion bottoms out.
fn pick_branch_with_depth(
    sub_schemas: &[Value],
    opts: &GenerateOptions,
    rng: &mut Random,
    depth: usize,
) -> usize {
    let has_self_ref = sub_schemas.iter().any(|s| {
        s.get("$ref")
            .and_then(|v| v.as_str())
            .map(|r| r == "#" || r.starts_with("#/"))
            .unwrap_or(false)
    });
    let terminal_idx: Option<usize> = sub_schemas.iter().position(is_terminal_friendly);
    if has_self_ref && depth >= 1 {
        if let Some(i) = terminal_idx {
            return i;
        }
    }
    if depth + 1 >= opts.max_depth {
        if let Some(i) = terminal_idx {
            return i;
        }
    }
    rng.pick_index(sub_schemas.len())
}

/// Returns true when a sub-schema can produce a value without further recursion through `$ref`.
fn is_terminal_friendly(schema: &Value) -> bool {
    if schema.get("$ref").is_some() {
        return false;
    }
    if let Some(Value::String(t)) = schema.get("type") {
        if t == "null" || t == "boolean" || t == "string" || t == "number" || t == "integer" {
            return true;
        }
    }
    if schema.get("const").is_some() || schema.get("enum").is_some() {
        return true;
    }
    false
}
