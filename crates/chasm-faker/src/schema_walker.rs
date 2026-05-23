//! Recursive schema walker that dispatches to the per-type generators.
//!
//! Owns the depth-aware `walk_schema` entry point the dynamic
//! [`crate::generate`] path uses, the `is_skip_sentinel` cutoff marker that
//! signals "depth limit hit — drop this property", and the composition
//! pre-flight that resolves `$ref` / `allOf` / `oneOf` / `anyOf` before
//! dispatching to a concrete generator module.

use crate::generators::{array, boolean, composition, enum_const, null, number, object, string};
use crate::options::GenerateOptions;
use crate::random::Random;
use crate::ref_resolver::RefResolver;
use serde_json::Value;
use std::cell::RefCell;

thread_local! {
    /// Thread-local flag indicating that the walker encountered an embedded `Value::Bool(false)`
    /// schema during this `generate()` invocation.
    ///
    /// We do not call `rng.set_error` directly at the site, because `Random::set_error` is
    /// first-write-wins: if the walker recorded `CannotGenerateForFalseSchema` first, other
    /// downstream generators (such as `generators::array`'s `MissingItems`) could not
    /// upgrade the error message that callers and fixtures expect for known failure modes.
    /// Instead, we defer surfacing the false-schema error to the top-level finalize hook in
    /// `walk_schema`, which only sets it when no other generator has already supplied a more
    /// specific error.
    static FALSE_SCHEMA_SEEN: RefCell<bool> = const { RefCell::new(false) };
}

/// Marks the thread-local flag noting that an embedded `false` schema was encountered.
fn mark_false_schema_seen() {
    FALSE_SCHEMA_SEEN.with(|cell| *cell.borrow_mut() = true);
}

/// Returns whether the thread-local flag noting an embedded `false` schema is set.
fn false_schema_seen() -> bool {
    FALSE_SCHEMA_SEEN.with(|cell| *cell.borrow())
}

/// Resets the embedded-`false` flag for a new top-level `generate()` invocation.
fn reset_false_schema_seen() {
    FALSE_SCHEMA_SEEN.with(|cell| *cell.borrow_mut() = false);
}

thread_local! {
    /// Thread-local `$ref` resolution depth that persists across recursive `walk_inner` calls.
    ///
    /// Previously each `walk_inner` call resolved with a fresh `Vec::new()` visited list, so
    /// `ref_depth_max` (the resolver's recursion window) only constrained a single resolve
    /// call. Chained references followed across walker recursion (e.g. `A` -> array of `A`
    /// -> array of `A` ...) were never measured against the window because every recursive
    /// `walk` re-entered the resolver with depth 0.
    ///
    /// We persist the depth across the walk so `ref_depth_max` actually caps the cumulative
    /// `$ref` traversals during one `generate()` call. We track a depth COUNT rather than a
    /// visited-name SET because legitimately recursive schemas (e.g. `{$ref: "#/$defs/A"}`
    /// appearing inside `A`'s own `items`) must be allowed to re-enter the same `$ref`
    /// multiple times within the budget — the resolver itself already detects single-chain
    /// cycles via its per-call visited list.
    ///
    /// A thread-local is used because `walk_inner`'s signature cannot be extended without
    /// breaking call sites in other modules.
    static REF_DEPTH: RefCell<usize> = const { RefCell::new(0) };
}

/// Resets the thread-local `$ref` depth so a new top-level `generate()` call starts
/// from an empty depth window.
///
/// Called from `walk_schema` only when `depth == 0`, which signals the top-level entry
/// from `lib.rs::generate()`. Object/array generators recurse with `depth + 1`, so they
/// will not trigger a mid-generation reset and the accumulated depth will continue to
/// constrain `$ref` chains across the entire walk.
fn reset_ref_visited_if_top_level(depth: usize) {
    if depth == 0 {
        REF_DEPTH.with(|cell| *cell.borrow_mut() = 0);
    }
}

/// Returns the current accumulated `$ref` depth from the thread-local counter.
fn current_ref_depth() -> usize {
    REF_DEPTH.with(|cell| *cell.borrow())
}

/// Increments the accumulated `$ref` depth counter and returns a guard that decrements
/// it on drop, so early returns from `walk_inner` cannot leak the counter.
struct RefDepthGuard;

impl RefDepthGuard {
    /// Pushes one onto the cumulative `$ref` depth counter.
    fn enter() -> Self {
        REF_DEPTH.with(|cell| *cell.borrow_mut() += 1);
        RefDepthGuard
    }
}

impl Drop for RefDepthGuard {
    /// Pops the cumulative `$ref` depth counter so a sibling branch starts from the
    /// pre-`$ref` budget.
    fn drop(&mut self) {
        REF_DEPTH.with(|cell| {
            let mut v = cell.borrow_mut();
            if *v > 0 {
                *v -= 1;
            }
        });
    }
}

/// Walks a JSON Schema node and generates a fake value conforming to it.
///
/// This is the central dispatch function. It applies all applicable keywords
/// in the correct priority order matching the json-schema-faker JavaScript library:
/// depth guard → boolean schemas → $ref → const → enum → examples →
/// if/then/else → allOf → anyOf → oneOf → not → default → type inference → type dispatch.
/// Sentinel key embedded in a one-key object to signal "omit this property if optional".
///
/// Returned by `walk()` when the depth cutoff is reached for a schema that cannot produce
/// a satisfying terminal value (e.g. self-referential `allOf`/`$ref` cycles). The object
/// generator recognises this marker and omits the key entirely when the parent's `required`
/// list does not contain it; otherwise it substitutes an empty object so the value still
/// has a slot.
pub const CHASM_SKIP_KEY: &str = "__chasm_skip";

/// Returns true when `value` is the depth-cutoff omit sentinel produced by `walk()`.
pub fn is_skip_sentinel(value: &Value) -> bool {
    if let Value::Object(map) = value {
        return map.len() == 1 && map.contains_key(CHASM_SKIP_KEY);
    }
    false
}

/// Builds the depth-cutoff omit sentinel.
fn skip_sentinel() -> Value {
    let mut m = serde_json::Map::new();
    m.insert(CHASM_SKIP_KEY.to_string(), Value::Bool(true));
    Value::Object(m)
}

pub fn walk(
    schema: &Value,
    root: &Value,
    opts: &GenerateOptions,
    rng: &mut Random,
    depth: usize,
) -> Value {
    if let Some(local_opts) = node_level_options(schema, opts) {
        return walk_inner(schema, root, &local_opts, rng, depth);
    }
    walk_inner(schema, root, opts, rng, depth)
}

/// Returns a clone of `opts` with node-level `x-json-schema-faker` overrides applied.
///
/// Returns `None` when the schema does not have an `x-json-schema-faker` object, so the
/// caller can skip cloning. Recognised keys mirror the spec-level merger.
fn node_level_options(schema: &Value, opts: &GenerateOptions) -> Option<GenerateOptions> {
    let map = schema.as_object()?;
    let cfg = map.get("x-json-schema-faker")?.as_object()?;
    let mut local = opts.clone();
    if let Some(v) = cfg.get("minItems").and_then(|v| v.as_u64()) {
        local.min_items = Some(v as usize);
    }
    if let Some(v) = cfg.get("maxItems").and_then(|v| v.as_u64()) {
        local.max_items = Some(v as usize);
    }
    if let Some(v) = cfg.get("optionalsProbability").and_then(|v| v.as_f64()) {
        local.optionals_probability = Some(v);
    }
    if let Some(v) = cfg.get("alwaysFakeOptionals").and_then(|v| v.as_bool()) {
        local.always_fake_optionals = v;
    }
    if let Some(v) = cfg.get("useDefaultValue").and_then(|v| v.as_bool()) {
        local.use_default_value = v;
    }
    if let Some(v) = cfg.get("useExamplesValue").and_then(|v| v.as_bool()) {
        local.use_examples_value = v;
    }
    if let Some(v) = cfg.get("requiredOnly").and_then(|v| v.as_bool()) {
        local.required_only = v;
    }
    if let Some(v) = cfg.get("fillProperties").and_then(|v| v.as_bool()) {
        local.fill_properties = v;
    }
    if let Some(v) = cfg.get("failOnInvalidTypes").and_then(|v| v.as_bool()) {
        local.fail_on_invalid_type = v;
    }
    if let Some(v) = cfg.get("failOnInvalidFormat").and_then(|v| v.as_bool()) {
        local.fail_on_invalid_format = v;
    }
    Some(local)
}

/// Internal dispatch body that assumes `opts` has already incorporated any node-level
/// `x-json-schema-faker` overrides.
///
/// `faker` and `chance` extension lookups go through the per-call registry snapshot
/// attached to `opts` (taken at `generate()` entry) rather than the global registry.
/// This stops a concurrent `define()` from another thread from changing which closure
/// runs partway through a walk and breaking same-seed determinism. The snapshot path
/// falls back to the global registry if no snapshot is present, i.e. when a caller
/// bypassed `generate()` and invoked `walk_schema` directly.
fn walk_inner(
    schema: &Value,
    root: &Value,
    opts: &GenerateOptions,
    rng: &mut Random,
    depth: usize,
) -> Value {
    if depth >= opts.max_depth {
        if schema_is_recursive_only(schema) {
            return skip_sentinel();
        }
        let mut visited = Vec::new();
        return terminal_value_for_schema_with_root(schema, root, &mut visited);
    }

    let aliased_owned;
    let schema = if let Some(aliased) = apply_schema_prop_aliases(schema, &opts.prop_aliases) {
        aliased_owned = aliased;
        &aliased_owned
    } else {
        schema
    };

    warn_unevaluated_keywords(schema);

    if let Some(b) = schema.as_bool() {
        if !b {
            mark_false_schema_seen();
        }
        return Value::Null;
    }

    if depth > 0
        && schema
            .get("nullable")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
    {
        let null_prob = opts.optionals_probability.unwrap_or(0.3);
        if rng.should_include(null_prob) {
            return Value::Null;
        }
    }

    if let Some(Value::String(ref_str)) = schema.get("$ref") {
        if current_ref_depth() + 1 >= opts.ref_depth_max {
            let mut visited = Vec::new();
            return terminal_value_for_schema_with_root(schema, root, &mut visited);
        }
        let resolver = RefResolver::new(root)
            .with_external(&opts.external_refs)
            .with_depth_window(opts.ref_depth_min, opts.ref_depth_max);
        let mut visited = Vec::new();
        if let Some(resolved) = resolver.resolve(ref_str.as_str(), &mut visited) {
            let resolved_owned = resolved.clone();
            let _guard = RefDepthGuard::enter();
            if schema_has_sibling_keywords(schema) {
                let siblings = schema_without_ref(schema);
                let merged = crate::merge::deep_merge(resolved_owned, siblings);
                return walk(&merged, root, opts, rng, depth + 1);
            }
            return walk(&resolved_owned, root, opts, rng, depth + 1);
        }
        if let Some(external) = opts.external_refs.get(ref_str.as_str()) {
            let resolved_owned = external.clone();
            let _guard = RefDepthGuard::enter();
            if schema_has_sibling_keywords(schema) {
                let siblings = schema_without_ref(schema);
                let merged = crate::merge::deep_merge(resolved_owned, siblings);
                return walk(&merged, root, opts, rng, depth + 1);
            }
            return walk(&resolved_owned, root, opts, rng, depth + 1);
        }
        rng.set_error(crate::FakerError::UnresolvedRef {
            path: if ref_str.starts_with('#') {
                format!("internal ref '{}' not found", ref_str)
            } else {
                ref_str.to_string()
            },
        });
        return Value::Null;
    }

    if opts.use_examples_value {
        if let Some(examples) = schema.get("examples").and_then(|v| v.as_array()) {
            if !examples.is_empty() {
                let idx = rng.pick_index(examples.len());
                return examples[idx].clone();
            }
        }
        if let Some(example) = schema.get("example") {
            return example.clone();
        }
    }

    if opts.use_default_value {
        if let Some(default_val) = schema.get("default") {
            return default_val.clone();
        }
    }

    if schema.get("if").is_some() {
        return walk_if_then_else(schema, root, opts, rng, depth);
    }

    if schema.get("allOf").is_some() {
        let all_of_arr = schema
            .get("allOf")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        if schema.get("type").is_some() || schema.get("properties").is_some() {
            let type_hint = infer_type(schema);
            if type_hint == "multi" {
                let mut merged = schema.clone();
                if let Value::Object(ref mut m) = merged {
                    m.remove("allOf");
                }
                let resolver = crate::ref_resolver::RefResolver::new(root)
                    .with_external(&opts.external_refs)
                    .with_depth_window(opts.ref_depth_min, opts.ref_depth_max);
                for sub in &all_of_arr {
                    let resolved = resolve_sub_schema(sub, &resolver);
                    merged = crate::merge::deep_merge(merged, resolved);
                }
                return walk_with_multi_type(&merged, root, opts, rng, depth);
            }
            if type_hint != "unknown" {
                let mut merged = schema.clone();
                if let Value::Object(ref mut m) = merged {
                    m.remove("allOf");
                }
                let resolver = crate::ref_resolver::RefResolver::new(root)
                    .with_external(&opts.external_refs)
                    .with_depth_window(opts.ref_depth_min, opts.ref_depth_max);
                let mut extra_contains: Vec<Value> = Vec::new();
                for sub in &all_of_arr {
                    let mut resolved = resolve_sub_schema(sub, &resolver);
                    if let Value::Object(ref mut m) = resolved {
                        if let Some(c) = m.remove("contains") {
                            extra_contains.push(c);
                        }
                    }
                    merged = crate::merge::deep_merge(merged, resolved);
                }
                if merged_has_composition(&merged) {
                    let mut result = walk(&merged, root, opts, rng, depth + 1);
                    for (i, contains_schema) in extra_contains.iter().enumerate() {
                        if let Value::Array(ref mut arr) = result {
                            if arr.len() > i {
                                arr[i] = walk(contains_schema, root, opts, rng, depth + 1);
                            } else {
                                let val = walk(contains_schema, root, opts, rng, depth + 1);
                                arr.push(val);
                            }
                        }
                    }
                    return result;
                }
                let mut result =
                    dispatch_type(&merged, root, opts, rng, depth, type_hint_str(&merged));
                for (i, contains_schema) in extra_contains.iter().enumerate() {
                    if let Value::Array(ref mut arr) = result {
                        if arr.len() > i {
                            arr[i] = walk(contains_schema, root, opts, rng, depth + 1);
                        } else {
                            let val = walk(contains_schema, root, opts, rng, depth + 1);
                            arr.push(val);
                        }
                    }
                }
                return result;
            }
        }
        let result = composition::generate_all_of(schema, root, opts, rng, depth);
        return result;
    }

    if schema.get("anyOf").is_some() {
        if schema.get("enum").is_some() {
            if let Some(filtered) = filter_enum_for_any_of(schema) {
                let mut narrowed = schema.clone();
                if let Value::Object(ref mut m) = narrowed {
                    m.insert("enum".to_string(), Value::Array(filtered));
                    m.remove("anyOf");
                }
                return enum_const::generate_enum(&narrowed, rng);
            }
        }
        if schema.get("type").is_some() || schema.get("properties").is_some() {
            let type_hint = infer_type(schema);
            if type_hint != "unknown" && type_hint != "multi" {
                let (chosen_idx, constraint_schema) =
                    pick_branch_compatible_with_base(schema, "anyOf", rng);
                let mut merged = merge_with_constraint(schema, constraint_schema);
                if let Value::Object(ref mut m) = merged {
                    m.remove("anyOf");
                }
                if type_hint == "object" {
                    apply_exclusivity_pruning(&mut merged, schema, "anyOf", chosen_idx);
                }
                if merged.get("allOf").is_some()
                    || merged_has_composition(&merged)
                    || merged.get("enum").is_some()
                    || merged.get("const").is_some()
                {
                    return walk(&merged, root, opts, rng, depth + 1);
                }
                return dispatch_type(&merged, root, opts, rng, depth, type_hint_str(&merged));
            }
        }
        return composition::generate_any_of(schema, root, opts, rng, depth);
    }

    if schema.get("oneOf").is_some() {
        if schema.get("enum").is_some() {
            if let Some(filtered) = filter_enum_for_one_of(schema) {
                let mut narrowed = schema.clone();
                if let Value::Object(ref mut m) = narrowed {
                    m.insert("enum".to_string(), Value::Array(filtered));
                    m.remove("oneOf");
                }
                return enum_const::generate_enum(&narrowed, rng);
            }
        }
        if schema.get("type").is_some() || schema.get("properties").is_some() {
            let type_hint = infer_type(schema);
            if type_hint != "unknown" && type_hint != "multi" {
                let (chosen_idx, constraint_schema) =
                    pick_branch_compatible_with_base(schema, "oneOf", rng);
                let mut merged = merge_with_constraint(schema, constraint_schema);
                if let Value::Object(ref mut m) = merged {
                    m.remove("oneOf");
                }
                if type_hint == "object" {
                    apply_exclusivity_pruning(&mut merged, schema, "oneOf", chosen_idx);
                }
                if merged.get("allOf").is_some()
                    || merged_has_composition(&merged)
                    || merged.get("enum").is_some()
                    || merged.get("const").is_some()
                {
                    return walk(&merged, root, opts, rng, depth + 1);
                }
                return dispatch_type(&merged, root, opts, rng, depth, type_hint_str(&merged));
            }
        }
        return composition::generate_one_of(schema, root, opts, rng, depth);
    }

    if schema.get("not").is_some() && is_not_only_keyword(schema) {
        return generate_not(schema, root, opts, rng, depth);
    }

    if schema.get("const").is_some() {
        return enum_const::generate_const(schema);
    }

    if schema.get("enum").is_some() {
        if let Some(filtered) = filter_enum_by_outer_constraints(schema) {
            let mut narrowed = schema.clone();
            if let Value::Object(ref mut m) = narrowed {
                m.insert("enum".to_string(), Value::Array(filtered));
            }
            return enum_const::generate_enum(&narrowed, rng);
        }
        return enum_const::generate_enum(schema, rng);
    }

    if schema.get("chance").is_some() && schema.get("faker").is_some() {
        rng.set_error(crate::FakerError::AmbiguousGenerator);
        return Value::Null;
    }

    if let Some(Value::String(name)) = schema.get("faker") {
        if let Some(v) = crate::invoke_extension_from_opts(opts, "faker", schema, root, None, "") {
            return v;
        }
        if opts.fail_on_invalid_format && !is_known_faker_key(name.as_str()) {
            rng.set_error(crate::FakerError::UnknownFakerGenerator {
                name: name.clone(),
                path: rng.current_path(),
            });
        }
        log_faker_extension_missing(name);
    }

    if let Some(Value::String(name)) = schema.get("x-faker") {
        if let Some(v) = crate::invoke_extension_from_opts(opts, "faker", schema, root, None, "") {
            return v;
        }
        if !is_known_faker_key(name.as_str()) {
            tracing::warn!(
                target: "chasm_faker::schema_walker",
                key = %name,
                "x-faker key not recognised, falling through to type-based generation"
            );
            if opts.fail_on_invalid_format {
                rng.set_error(crate::FakerError::UnknownFakerGenerator {
                    name: name.clone(),
                    path: rng.current_path(),
                });
            }
        }
        log_faker_extension_missing(name);
    }

    if let Some(picked) = pick_from_x_faker_object_form(schema, rng) {
        return picked;
    }

    if schema.get("chance").is_some() {
        if let Some(v) = crate::invoke_extension_from_opts(opts, "chance", schema, root, None, "") {
            return v;
        }
    }

    if let Some(Value::Bool(true)) = schema.get("autoIncrement") {
        let key = auto_increment_key(schema, rng);
        let offset = schema
            .get("initialOffset")
            .and_then(|v| v.as_i64())
            .unwrap_or(0);
        let n = rng.next_auto_increment(&key, offset);
        return Value::Number(serde_json::Number::from(n));
    }

    let type_hint = infer_type(schema);
    if type_hint == "unknown" {
        if let Some(Value::String(t)) = schema.get("type") {
            return handle_invalid_type(t.as_str(), opts, rng, root);
        }
        if let Some(implicit) = as_anonymous_object(schema) {
            return dispatch_type(&implicit, root, opts, rng, depth, "object");
        }
        return Value::Null;
    }
    if type_hint == "multi" {
        return walk_with_multi_type(schema, root, opts, rng, depth);
    }

    if let Some(not_schema) = schema.get("not") {
        if not_schema_is_checkable(not_schema) {
            for _ in 0..30 {
                let candidate = dispatch_type(schema, root, opts, rng, depth, type_hint);
                if !value_matches_not_schema(&candidate, not_schema) {
                    return apply_unevaluated_constraints(candidate, schema, root, opts);
                }
            }
            let path = rng.current_path();
            rng.set_error(crate::FakerError::SchemaError {
                path,
                message: "not constraint could not be satisfied after 30 attempts".to_string(),
            });
            return Value::Null;
        }
    }

    let value = dispatch_type(schema, root, opts, rng, depth, type_hint);
    apply_unevaluated_constraints(value, schema, root, opts)
}

/// Restricts an already-generated value to honour `unevaluatedProperties: false` and
/// `unevaluatedItems: false`/`unevaluatedItems: {schema}` as a best-effort post-pass.
///
/// This sits outside the object/array generators so the walker can enforce the
/// minimal contract these keywords imply without the larger refactor needed to track
/// "evaluated" keys through every composition branch. The effects are:
///
/// - `unevaluatedProperties: false` drops object keys that are not listed under the
///   schema's own `properties` and are not matched by its `patternProperties`.
/// - `unevaluatedItems: false` truncates the array to the length of the schema's
///   `prefixItems` (or zero when none).
fn apply_unevaluated_constraints(
    value: Value,
    schema: &Value,
    root: &Value,
    opts: &GenerateOptions,
) -> Value {
    let map = match schema.as_object() {
        Some(m) => m,
        None => return value,
    };
    let mut result = value;
    if let Some(Value::Bool(false)) = map.get("unevaluatedProperties") {
        if let Value::Object(ref mut obj) = result {
            let mut declared: std::collections::HashSet<String> = std::collections::HashSet::new();
            let mut patterns: Vec<regex::Regex> = Vec::new();
            let mut visited: std::collections::HashSet<String> = std::collections::HashSet::new();
            collect_evaluated_properties(
                schema,
                root,
                opts,
                &mut declared,
                &mut patterns,
                &mut visited,
            );
            let keys: Vec<String> = obj.keys().cloned().collect();
            for key in keys {
                let declared_match = declared.contains(&key);
                let pattern_match = patterns.iter().any(|re| re.is_match(&key));
                if !declared_match && !pattern_match {
                    obj.remove(&key);
                }
            }
        }
    }
    if let Some(Value::Bool(false)) = map.get("unevaluatedItems") {
        if let Value::Array(ref mut arr) = result {
            let prefix_len = map
                .get("prefixItems")
                .and_then(|v| v.as_array())
                .map(|a| a.len())
                .unwrap_or(0);
            arr.truncate(prefix_len);
        }
    }
    result
}

/// Walks `schema` (resolving `$ref` against `root` and recursing into every applier)
/// to collect the union of property names declared by `properties` and the regexes
/// declared by `patternProperties`, so [`apply_unevaluated_constraints`] does not
/// strip keys that were legitimately introduced by an applier-resolved sub-schema.
///
/// Per JSON Schema 2020-12, `unevaluatedProperties` is applied AFTER all appliers
/// (`$ref`, `allOf`, `oneOf`, `anyOf`, `if`/`then`/`else`) have evaluated. The
/// previous implementation only consulted the local `properties` and
/// `patternProperties`, so keys evaluated by an applier branch were over-pruned.
///
/// `visited` tracks already-seen `$ref` strings to prevent infinite recursion on
/// self-referential schemas.
fn collect_evaluated_properties(
    schema: &Value,
    root: &Value,
    opts: &GenerateOptions,
    declared: &mut std::collections::HashSet<String>,
    patterns: &mut Vec<regex::Regex>,
    visited: &mut std::collections::HashSet<String>,
) {
    let map = match schema.as_object() {
        Some(m) => m,
        None => return,
    };
    if let Some(Value::Object(props)) = map.get("properties") {
        for key in props.keys() {
            declared.insert(key.clone());
        }
    }
    if let Some(Value::Object(pp)) = map.get("patternProperties") {
        for key in pp.keys() {
            if let Ok(re) = regex::Regex::new(key) {
                patterns.push(re);
            }
        }
    }
    if let Some(Value::String(ref_str)) = map.get("$ref") {
        if !visited.contains(ref_str) {
            visited.insert(ref_str.clone());
            let resolver = RefResolver::new(root).with_external(&opts.external_refs);
            let mut resolver_visited = Vec::new();
            if let Some(resolved) = resolver.resolve(ref_str.as_str(), &mut resolver_visited) {
                let resolved_owned = resolved.clone();
                collect_evaluated_properties(
                    &resolved_owned,
                    root,
                    opts,
                    declared,
                    patterns,
                    visited,
                );
            }
        }
    }
    for keyword in &["allOf", "oneOf", "anyOf"] {
        if let Some(Value::Array(arr)) = map.get(*keyword) {
            for sub in arr {
                collect_evaluated_properties(sub, root, opts, declared, patterns, visited);
            }
        }
    }
    for keyword in &["then", "else"] {
        if let Some(branch) = map.get(*keyword) {
            collect_evaluated_properties(branch, root, opts, declared, patterns, visited);
        }
    }
}

/// Returns true when the `not` sub-schema contains a constraint that
/// [`value_matches_not_schema`] knows how to evaluate.
///
/// Used to gate the retry-and-error loop in `walk_inner`: when the `not` schema only
/// declares constraints outside our cheap checker's vocabulary (for example
/// `required` or nested compositions), every candidate trivially "matches", which
/// would otherwise cause every generation to error. Treating those cases as
/// unchecked preserves the prior best-effort behaviour while still surfacing the
/// new `SchemaError` for genuinely-unsatisfiable scalar constraints.
fn not_schema_is_checkable(not_schema: &Value) -> bool {
    let map = match not_schema.as_object() {
        Some(m) => m,
        None => return false,
    };
    map.contains_key("const") || map.contains_key("enum") || map.contains_key("type")
}

/// Returns true if `value` matches the constraints in `not_schema` (so it would be REJECTED by `not`).
///
/// Covers the cheap cases needed by upstream fixtures: `enum`, `const`, `type`, and combinations
/// thereof. Returns false on anything we can't cheaply evaluate, which conservatively keeps
/// generation from rejecting valid candidates over checks we can't perform.
fn value_matches_not_schema(value: &Value, not_schema: &Value) -> bool {
    if let Some(constant) = not_schema.get("const") {
        if value != constant {
            return false;
        }
    }
    if let Some(enum_arr) = not_schema.get("enum").and_then(|v| v.as_array()) {
        if !enum_arr.iter().any(|e| e == value) {
            return false;
        }
    }
    if let Some(type_val) = not_schema.get("type") {
        if !value_has_type(value, type_val) {
            return false;
        }
    }
    true
}

/// Returns true if the JSON `value` matches the schema `type` keyword.
///
/// Accepts either a single type string or an array of allowed type strings.
fn value_has_type(value: &Value, type_val: &Value) -> bool {
    let actual = match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
        Value::Number(n) if n.is_i64() || n.is_u64() => "integer",
        Value::Number(_) => "number",
    };
    match type_val {
        Value::String(s) => s == actual || (s == "number" && actual == "integer"),
        Value::Array(arr) => arr.iter().any(|t| match t.as_str() {
            Some(s) => s == actual || (s == "number" && actual == "integer"),
            None => false,
        }),
        _ => false,
    }
}

/// Records that a `faker` keyword was encountered but no extension is registered for it,
/// so the walker has fallen through to the standard type generator (typically producing a
/// generic string).
///
/// Emits a structured `tracing::debug!` event so consumers can see at runtime which
/// faker keys would benefit from a registered extension. The event is debug-level so
/// it stays quiet by default and only surfaces when a consumer explicitly enables
/// `chasm_faker=debug`.
fn log_faker_extension_missing(name: &str) {
    tracing::debug!(
        target: "chasm_faker::schema_walker",
        faker = %name,
        "faker keyword has no registered extension; falling through to default type generator"
    );
}

/// Handles the object-form `x-faker` hint when its single key is one of the
/// recognised picker helpers (`helpers.arrayElement` or its `random.arrayElement`
/// alias) and its value is a JSON array. Returns one of the array's entries
/// verbatim, picked via `rng`. Returns `None` for any other shape so the caller
/// falls through to the normal type-based generation path.
///
/// Mirrors json-schema-faker's `x-faker: { "helpers.arrayElement": [...] }`
/// idiom which lets schema authors enumerate fixed choices without an `enum`.
fn pick_from_x_faker_object_form(schema: &Value, rng: &mut Random) -> Option<Value> {
    let obj = schema.get("x-faker")?.as_object()?;
    if obj.len() != 1 {
        return None;
    }
    let (key, value) = obj.iter().next()?;
    if key != "helpers.arrayElement" && key != "random.arrayElement" {
        return None;
    }
    let arr = value.as_array()?;
    if arr.is_empty() {
        return None;
    }
    let idx = rng.int(0, (arr.len() as i64) - 1) as usize;
    Some(arr[idx].clone())
}

/// Returns true when a `faker` keyword names a generator that the upstream `@faker-js/faker`
/// package implements, even when this crate has no bound extension for it.
///
/// We treat well-known faker keys as "would-succeed" so generation does not falsely throw
/// on schemas that exercise the faker namespace without actually validating the produced
/// value. Unknown keys still surface as `UnknownFakerGenerator`.
fn is_known_faker_key(name: &str) -> bool {
    let known_namespaces = [
        "address.",
        "airline.",
        "animal.",
        "book.",
        "color.",
        "commerce.",
        "company.",
        "database.",
        "datatype.",
        "date.",
        "finance.",
        "food.",
        "git.",
        "hacker.",
        "helpers.",
        "image.",
        "internet.",
        "location.",
        "lorem.",
        "music.",
        "name.",
        "person.",
        "phone.",
        "random.",
        "science.",
        "sport.",
        "string.",
        "system.",
        "vehicle.",
        "word.",
    ];
    let known_exact = ["custom.statement"];
    if known_exact.contains(&name) {
        return true;
    }
    known_namespaces.iter().any(|ns| name.starts_with(ns))
}

/// Handles a schema whose `type` keyword is an unrecognised string.
///
/// When `failOnInvalidTypes` is enabled, records an `InvalidType` error and returns
/// null. When `defaultInvalidTypeProduct` is a known JSON-Schema type name, generates
/// a value of that type; when it is a primitive value, returns it directly. Otherwise
/// returns null so generation can continue producing a (possibly-invalid) document.
fn handle_invalid_type(
    type_name: &str,
    opts: &GenerateOptions,
    rng: &mut Random,
    root: &Value,
) -> Value {
    if opts.fail_on_invalid_type {
        let base = rng.current_path();
        let path = if base == "/" {
            "/type".to_string()
        } else {
            format!("{}/type", base)
        };
        rng.set_error(crate::FakerError::InvalidType {
            value: type_name.to_string(),
            path,
        });
        return Value::Null;
    }
    if let Some(default) = &opts.default_invalid_type_product {
        if let Value::String(s) = default {
            let synthetic = serde_json::json!({ "type": s });
            return dispatch_type(
                &synthetic,
                root,
                opts,
                rng,
                0,
                type_str_to_static(s.as_str()),
            );
        }
        return default.clone();
    }
    Value::Null
}

/// Dispatches generation to the appropriate generator based on a type name string.
fn dispatch_type(
    schema: &Value,
    root: &Value,
    opts: &GenerateOptions,
    rng: &mut Random,
    depth: usize,
    type_hint: &str,
) -> Value {
    match type_hint {
        "string" => string::generate(schema, root, opts, rng),
        "number" => number::generate(schema, rng, opts),
        "integer" => number::generate_integer(schema, rng, opts),
        "boolean" => boolean::generate(rng),
        "null" => null::generate(),
        "array" => array::generate(schema, root, opts, rng, depth),
        "object" => object::generate(schema, root, opts, rng, depth),
        _ => Value::Null,
    }
}

/// Infers the JSON Schema type from explicit `type` field or implicit constraint keywords.
///
/// Returns the inferred type string, an empty string for an array of types (handled separately),
/// or `"unknown"` when no type can be determined and no composition keywords are present.
fn infer_type(schema: &Value) -> &'static str {
    if let Some(type_val) = schema.get("type") {
        match type_val {
            Value::String(t) => return type_str_to_static(t.as_str()),
            Value::Array(_types) => {
                return "multi";
            }
            _ => {}
        }
    }

    if schema.get("properties").is_some()
        || schema.get("required").is_some()
        || schema.get("additionalProperties").is_some()
        || schema.get("patternProperties").is_some()
        || schema.get("minProperties").is_some()
        || schema.get("maxProperties").is_some()
    {
        return "object";
    }

    if schema.get("items").is_some()
        || schema.get("prefixItems").is_some()
        || schema.get("minItems").is_some()
        || schema.get("maxItems").is_some()
    {
        return "array";
    }

    if schema.get("minLength").is_some()
        || schema.get("maxLength").is_some()
        || schema.get("pattern").is_some()
        || schema.get("format").is_some()
    {
        return "string";
    }

    if schema.get("minimum").is_some()
        || schema.get("maximum").is_some()
        || schema.get("exclusiveMinimum").is_some()
        || schema.get("exclusiveMaximum").is_some()
        || schema.get("multipleOf").is_some()
    {
        return "number";
    }

    "unknown"
}

/// Maps a JSON Schema type string to a `'static str` suitable for matching.
fn type_str_to_static(t: &str) -> &'static str {
    match t {
        "string" => "string",
        "number" => "number",
        "integer" => "integer",
        "boolean" => "boolean",
        "null" => "null",
        "array" => "array",
        "object" => "object",
        _ => "unknown",
    }
}

/// Returns true when the schema is composed only of recursive composition keywords that
/// have no terminal-friendly branch available without further recursion.
///
/// Detects the pattern where every branch in `allOf`/`anyOf`/`oneOf` either is itself a
/// `$ref` or wraps another composition — implying the depth cutoff has nowhere else to go.
/// A schema with even one non-`$ref` branch is NOT considered recursive-only.
fn schema_is_recursive_only(schema: &Value) -> bool {
    let map = match schema.as_object() {
        Some(m) => m,
        None => return false,
    };
    if map.get("type").is_some() || map.get("const").is_some() || map.get("enum").is_some() {
        return false;
    }
    let mut saw_composition = false;
    for kw in &["allOf", "anyOf", "oneOf"] {
        if let Some(Value::Array(arr)) = map.get(*kw) {
            saw_composition = true;
            let all_recursive = arr.iter().all(|sub| {
                sub.get("$ref").and_then(|v| v.as_str()).is_some() || schema_is_recursive_only(sub)
            });
            if !all_recursive {
                return false;
            }
        }
    }
    saw_composition
}

/// Returns a safe terminal value when the maximum generation depth is reached.
///
/// Uses the schema's `type` hint to produce an appropriate empty/zero value.
/// Schemas that contain only a `$ref` (no explicit `type`) return an empty object
/// because the referenced schema is typically an object type.
/// Returns a terminal value for a schema, optionally resolving local `$ref` to the root.
///
/// Resolving allows the function to find `required` keys on the referenced schema and
/// populate them with terminal values, so deeply nested schemas still satisfy `required`.
/// The `visited` parameter tracks `$ref` strings already entered along the current call
/// path so a self-referential schema like `{"$ref": "#"}` returns `Value::Null` instead
/// of recursing without bound.
fn terminal_value_for_schema_with_root(
    schema: &Value,
    root: &Value,
    visited: &mut Vec<String>,
) -> Value {
    for kw in &["oneOf", "anyOf"] {
        if let Some(Value::Array(arr)) = schema.get(*kw) {
            for sub in arr {
                if let Some(Value::String(t)) = sub.get("type") {
                    if t == "null" {
                        return Value::Null;
                    }
                }
            }
        }
    }
    let saved_len = visited.len();
    if let Some(merged) = merge_all_of_into_terminal_schema(schema, root, visited) {
        let value = terminal_value_for_schema_with_root(&merged, root, visited);
        visited.truncate(saved_len);
        return value;
    }
    visited.truncate(saved_len);
    if let Some(Value::String(ref_str)) = schema.get("$ref") {
        if visited.iter().any(|r| r == ref_str) {
            return Value::Null;
        }
        let resolver = crate::ref_resolver::RefResolver::new(root);
        let mut resolver_visited = Vec::new();
        if let Some(resolved) = resolver.resolve(ref_str.as_str(), &mut resolver_visited) {
            let resolved = resolved.clone();
            visited.push(ref_str.clone());
            let value = terminal_value_for_schema_with_root(&resolved, root, visited);
            visited.pop();
            return value;
        }
        return Value::Object(serde_json::Map::new());
    }
    if let Some(Value::String(t)) = schema.get("type") {
        match t.as_str() {
            "string" => {
                if schema.get("pattern").is_some() || schema.get("format").is_some() {
                    let mut rng = Random::new(Some(0));
                    let opts = crate::options::GenerateOptions::default();
                    return crate::generators::string::generate(schema, root, &opts, &mut rng);
                }
                let raw_min_len = schema
                    .get("minLength")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0) as usize;
                let min_len =
                    raw_min_len.min(crate::generators::string::MAX_GENERATED_STRING_LENGTH);
                if min_len == 0 {
                    return Value::String(String::new());
                }
                return Value::String("a".repeat(min_len));
            }
            "number" | "integer" => {
                let min = schema
                    .get("minimum")
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.0);
                let max = schema
                    .get("maximum")
                    .and_then(|v| v.as_f64())
                    .unwrap_or(f64::MAX);
                let pick = if min > 0.0 {
                    min
                } else if max < 0.0 {
                    max
                } else {
                    0.0
                };
                if t == "integer" {
                    if let Some(n) = serde_json::Number::from_f64(pick.round()) {
                        return Value::Number(n);
                    }
                    return Value::Number(serde_json::Number::from(pick as i64));
                }
                if let Some(n) = serde_json::Number::from_f64(pick) {
                    return Value::Number(n);
                }
                return Value::Number(serde_json::Number::from(0));
            }
            "boolean" => return Value::Bool(false),
            "array" => return Value::Array(Vec::new()),
            "object" => return terminal_object_with_required(schema, root, visited),
            "null" => return Value::Null,
            _ => {}
        }
    }
    Value::Null
}

/// Merges `allOf` branches into the schema at the depth-limit terminal path so a schema
/// like `{type: "object", required: [name], allOf: [{$ref: "#/components/schemas/Self"}]}`
/// surfaces the `name` property declared on the referenced sibling instead of returning
/// an empty object that violates the `required` contract.
///
/// Returns `Some(merged)` when at least one `allOf` branch was successfully resolved and
/// merged; returns `None` when the schema has no `allOf` or none of the branches resolve.
/// `Value::Bool(false)` is treated as opaque and skipped — the terminal path will not
/// recurse into a false schema.
fn merge_all_of_into_terminal_schema(
    schema: &Value,
    root: &Value,
    visited: &mut Vec<String>,
) -> Option<Value> {
    let branches = schema.get("allOf").and_then(|v| v.as_array())?;
    if branches.is_empty() {
        return None;
    }
    let mut base = schema.clone();
    if let Value::Object(ref mut map) = base {
        map.remove("allOf");
    }
    let mut merged_any = false;
    for branch in branches {
        let mut resolved = if let Some(Value::String(ref_str)) = branch.get("$ref") {
            if visited.iter().any(|r| r == ref_str) {
                continue;
            }
            let resolver = crate::ref_resolver::RefResolver::new(root);
            let mut resolver_visited = Vec::new();
            match resolver.resolve(ref_str.as_str(), &mut resolver_visited) {
                Some(r) => {
                    visited.push(ref_str.clone());
                    r.clone()
                }
                None => continue,
            }
        } else if branch == &Value::Bool(false) {
            continue;
        } else {
            branch.clone()
        };
        if let Value::Object(ref mut map) = resolved {
            map.remove("allOf");
        }
        base = crate::merge::deep_merge(base, resolved);
        merged_any = true;
    }
    if let Value::Object(ref mut map) = base {
        map.remove("allOf");
    }
    if merged_any {
        Some(base)
    } else {
        None
    }
}

/// Builds an object value at the depth limit, populating required keys with terminal values
/// from their declared property schemas so deeply nested schemas still satisfy `required`.
fn terminal_object_with_required(schema: &Value, root: &Value, visited: &mut Vec<String>) -> Value {
    let mut map = serde_json::Map::new();
    let required: Vec<String> = schema
        .get("required")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();
    let empty = serde_json::Map::new();
    let props = schema
        .get("properties")
        .and_then(|v| v.as_object())
        .unwrap_or(&empty);
    for key in &required {
        let prop_schema = props
            .get(key)
            .cloned()
            .unwrap_or(Value::Object(serde_json::Map::new()));
        map.insert(
            key.clone(),
            terminal_value_for_schema_with_root(&prop_schema, root, visited),
        );
    }
    Value::Object(map)
}

/// Top-level walk entry point that handles multi-type arrays in the `type` keyword.
///
/// When the schema's `type` is a JSON array, picks one type at random and delegates
/// to the type-specific generator; otherwise falls through to the standard `walk` function.
pub fn walk_with_multi_type(
    schema: &Value,
    root: &Value,
    opts: &GenerateOptions,
    rng: &mut Random,
    depth: usize,
) -> Value {
    if let Some(Value::Array(types)) = schema.get("type") {
        if !types.is_empty() {
            let idx = rng.pick_index(types.len());
            if let Some(Value::String(chosen_type)) = types.get(idx) {
                let mut modified = schema.clone();
                if let Value::Object(ref mut map) = modified {
                    map.insert("type".to_string(), Value::String(chosen_type.clone()));
                }
                return walk(&modified, root, opts, rng, depth);
            }
        }
    }
    walk(schema, root, opts, rng, depth)
}

/// Resolves a sub-schema `$ref` to its target, returning a clone of the resolved value.
fn resolve_sub_schema(schema: &Value, resolver: &crate::ref_resolver::RefResolver<'_>) -> Value {
    if let Some(Value::String(ref_str)) = schema.get("$ref") {
        let mut visited = Vec::new();
        if let Some(resolved) = resolver.resolve(ref_str.as_str(), &mut visited) {
            return resolved.clone();
        }
    }
    schema.clone()
}

/// Picks a composition branch that is most compatible with the base schema's constraints.
///
/// When the base schema's `required` list or `properties` constrain keys that some branches
/// would forbid via `additionalProperties: false`, prefer branches that already include those
/// keys. Falls back to uniform random selection when no branch is preferable.
fn pick_branch_compatible_with_base(
    schema: &Value,
    keyword: &str,
    rng: &mut Random,
) -> (usize, Value) {
    let arr = match schema.get(keyword).and_then(|v| v.as_array()) {
        Some(a) => a,
        None => return (0, Value::Object(serde_json::Map::new())),
    };
    if arr.is_empty() {
        return (0, Value::Object(serde_json::Map::new()));
    }
    let base_required: Vec<String> = schema
        .get("required")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();
    let base_props: Vec<String> = schema
        .get("properties")
        .and_then(|v| v.as_object())
        .map(|m| m.keys().cloned().collect())
        .unwrap_or_default();
    let must_include: std::collections::HashSet<String> = base_required
        .iter()
        .chain(base_props.iter())
        .filter(|k| base_required.contains(k))
        .cloned()
        .collect();
    if must_include.is_empty() {
        let idx = rng.pick_index(arr.len());
        return (idx, arr[idx].clone());
    }
    let mut compatible: Vec<usize> = Vec::new();
    for (i, sub) in arr.iter().enumerate() {
        let blocks_additional = sub
            .get("additionalProperties")
            .and_then(|v| v.as_bool())
            .map(|b| !b)
            .unwrap_or(false);
        if !blocks_additional {
            compatible.push(i);
            continue;
        }
        let sub_props: Vec<String> = sub
            .get("properties")
            .and_then(|v| v.as_object())
            .map(|m| m.keys().cloned().collect())
            .unwrap_or_default();
        let all_covered = must_include.iter().all(|k| sub_props.contains(k));
        if all_covered {
            compatible.push(i);
        }
    }
    if compatible.is_empty() {
        let idx = rng.pick_index(arr.len());
        return (idx, arr[idx].clone());
    }
    let pick = compatible[rng.pick_index(compatible.len())];
    (pick, arr[pick].clone())
}

/// Returns true when a (potentially merged) schema still contains composition keywords
/// that require recursive processing rather than direct type dispatch.
fn merged_has_composition(schema: &Value) -> bool {
    schema.get("oneOf").is_some() || schema.get("anyOf").is_some() || schema.get("if").is_some()
}

/// Removes keys from `properties` that would cause the generator to violate
/// `oneOf`/`anyOf` exclusivity. This covers two cases: (a) other branches' `required`
/// keys when `additionalProperties: false` prevents arbitrary additions, and (b) keys
/// listed under the chosen branch's `not.required` constraint, which must be absent.
fn apply_exclusivity_pruning(
    merged: &mut Value,
    original: &Value,
    keyword: &str,
    chosen_idx: usize,
) {
    let sub_schemas = match original.get(keyword).and_then(|v| v.as_array()) {
        Some(arr) => arr.clone(),
        None => return,
    };
    let chosen_required: Vec<String> = sub_schemas
        .get(chosen_idx)
        .and_then(|s| s.get("required"))
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();
    let base_required: Vec<String> = original
        .get("required")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();
    let mut to_prune: std::collections::HashSet<String> = std::collections::HashSet::new();
    let additional_false = merged
        .get("additionalProperties")
        .and_then(|v| v.as_bool())
        .map(|b| !b)
        .unwrap_or(false);
    if additional_false || keyword == "oneOf" {
        for (i, sub) in sub_schemas.iter().enumerate() {
            if i == chosen_idx {
                continue;
            }
            if let Some(Value::Array(arr)) = sub.get("required") {
                for v in arr {
                    if let Some(s) = v.as_str() {
                        let owned = s.to_string();
                        if !chosen_required.contains(&owned) && !base_required.contains(&owned) {
                            to_prune.insert(owned);
                        }
                    }
                }
            }
        }
    }
    if let Some(chosen) = sub_schemas.get(chosen_idx) {
        if let Some(not_required) = chosen
            .get("not")
            .and_then(|n| n.get("required"))
            .and_then(|v| v.as_array())
        {
            for v in not_required {
                if let Some(s) = v.as_str() {
                    if !base_required.contains(&s.to_string()) {
                        to_prune.insert(s.to_string());
                    }
                }
            }
        }
    }
    if to_prune.is_empty() {
        return;
    }
    if let Value::Object(map) = merged {
        if let Some(Value::Object(props)) = map.get_mut("properties") {
            for key in &to_prune {
                props.remove(key);
            }
        }
        if let Some(Value::Array(req)) = map.get_mut("required") {
            req.retain(|v| v.as_str().map(|s| !to_prune.contains(s)).unwrap_or(true));
        }
    }
}

/// Filters enum values by retaining only those satisfying exactly one `oneOf` sub-schema.
///
/// Returns `None` when filtering would produce zero candidates, indicating the
/// caller should fall back to standard handling.
fn filter_enum_for_one_of(schema: &Value) -> Option<Vec<Value>> {
    let enum_arr = schema.get("enum").and_then(|v| v.as_array())?;
    let one_of = schema.get("oneOf").and_then(|v| v.as_array())?;
    if enum_arr.is_empty() || one_of.is_empty() {
        return None;
    }
    let mut filtered: Vec<Value> = Vec::new();
    for candidate in enum_arr {
        let mut matches = 0usize;
        for sub in one_of {
            if value_satisfies_simple_constraints(candidate, sub) {
                matches += 1;
            }
        }
        if matches == 1 && value_satisfies_simple_constraints(candidate, schema) {
            filtered.push(candidate.clone());
        }
    }
    if filtered.is_empty() {
        return None;
    }
    Some(filtered)
}

/// Filters enum values by retaining only those satisfying at least one `anyOf` sub-schema.
fn filter_enum_for_any_of(schema: &Value) -> Option<Vec<Value>> {
    let enum_arr = schema.get("enum").and_then(|v| v.as_array())?;
    let any_of = schema.get("anyOf").and_then(|v| v.as_array())?;
    if enum_arr.is_empty() || any_of.is_empty() {
        return None;
    }
    let mut filtered: Vec<Value> = Vec::new();
    for candidate in enum_arr {
        let mut ok = false;
        for sub in any_of {
            if value_satisfies_simple_constraints(candidate, sub) {
                ok = true;
                break;
            }
        }
        if ok && value_satisfies_simple_constraints(candidate, schema) {
            filtered.push(candidate.clone());
        }
    }
    if filtered.is_empty() {
        return None;
    }
    Some(filtered)
}

/// Filters enum values by outer schema constraints (minimum, maximum, etc.) without composition.
fn filter_enum_by_outer_constraints(schema: &Value) -> Option<Vec<Value>> {
    let enum_arr = schema.get("enum").and_then(|v| v.as_array())?;
    let has_constraints = schema.get("minimum").is_some()
        || schema.get("maximum").is_some()
        || schema.get("exclusiveMinimum").is_some()
        || schema.get("exclusiveMaximum").is_some()
        || schema.get("multipleOf").is_some()
        || schema.get("minLength").is_some()
        || schema.get("maxLength").is_some()
        || schema.get("pattern").is_some();
    if !has_constraints {
        return None;
    }
    let mut filtered: Vec<Value> = Vec::new();
    for candidate in enum_arr {
        if value_satisfies_simple_constraints(candidate, schema) {
            filtered.push(candidate.clone());
        }
    }
    if filtered.is_empty() {
        return None;
    }
    Some(filtered)
}

/// Returns true if a JSON value satisfies a sub-schema's simple numeric/string/enum constraints.
///
/// Only checks `type`, `const`, `enum`, `minimum`, `maximum`, `exclusiveMinimum`,
/// `exclusiveMaximum`, `multipleOf`, `minLength`, `maxLength`, and `pattern`. Returns
/// `true` for any keyword not in this list, providing a conservative approximation.
pub fn value_satisfies_simple_constraints(value: &Value, schema: &Value) -> bool {
    if let Some(c) = schema.get("const") {
        if c != value {
            return false;
        }
    }
    if let Some(Value::Array(arr)) = schema.get("enum") {
        if !arr.iter().any(|v| v == value) {
            return false;
        }
    }
    if let Some(Value::String(t)) = schema.get("type") {
        let ok = match t.as_str() {
            "string" => value.is_string(),
            "number" => value.is_number(),
            "integer" => {
                value.is_i64()
                    || value.is_u64()
                    || (value.is_f64() && value.as_f64().map(|f| f.fract() == 0.0).unwrap_or(false))
            }
            "boolean" => value.is_boolean(),
            "null" => value.is_null(),
            "array" => value.is_array(),
            "object" => value.is_object(),
            _ => true,
        };
        if !ok {
            return false;
        }
    }
    if let Some(n) = value.as_f64() {
        if let Some(min) = schema.get("minimum").and_then(|v| v.as_f64()) {
            if n < min {
                return false;
            }
        }
        if let Some(max) = schema.get("maximum").and_then(|v| v.as_f64()) {
            if n > max {
                return false;
            }
        }
        if let Some(emin) = schema.get("exclusiveMinimum").and_then(|v| v.as_f64()) {
            if n <= emin {
                return false;
            }
        }
        if let Some(emax) = schema.get("exclusiveMaximum").and_then(|v| v.as_f64()) {
            if n >= emax {
                return false;
            }
        }
        if let Some(m) = schema.get("multipleOf").and_then(|v| v.as_f64()) {
            if m != 0.0 {
                let q = n / m;
                if (q - q.round()).abs() > 1e-9 {
                    return false;
                }
            }
        }
    }
    if let Some(s) = value.as_str() {
        if let Some(min) = schema.get("minLength").and_then(|v| v.as_u64()) {
            if (s.chars().count() as u64) < min {
                return false;
            }
        }
        if let Some(max) = schema.get("maxLength").and_then(|v| v.as_u64()) {
            if (s.chars().count() as u64) > max {
                return false;
            }
        }
        if let Some(Value::String(pat)) = schema.get("pattern") {
            if let Ok(re) = regex::Regex::new(pat) {
                if !re.is_match(s) {
                    return false;
                }
            }
        }
    }
    let schema_has_properties = schema
        .get("properties")
        .and_then(|v| v.as_object())
        .map(|m| !m.is_empty())
        .unwrap_or(false);
    let schema_has_required = schema
        .get("required")
        .and_then(|v| v.as_array())
        .map(|a| !a.is_empty())
        .unwrap_or(false);
    if schema_has_properties || schema_has_required {
        let value_obj = match value.as_object() {
            Some(o) => o,
            None => return false,
        };
        if let Some(Value::Object(props)) = schema.get("properties") {
            for (prop_name, prop_schema) in props {
                if let Some(prop_value) = value_obj.get(prop_name) {
                    if !value_satisfies_simple_constraints(prop_value, prop_schema) {
                        return false;
                    }
                }
            }
        }
        if let Some(Value::Array(req)) = schema.get("required") {
            for r in req {
                if let Some(name) = r.as_str() {
                    if !value_obj.contains_key(name) {
                        return false;
                    }
                }
            }
        }
    }
    true
}

/// Merges a constraint schema into a base schema, preserving the base's required/properties.
fn merge_with_constraint(base: &Value, constraint: Value) -> Value {
    crate::merge::deep_merge(base.clone(), constraint)
}

/// Returns the type hint string for a schema value, for use in dispatch.
fn type_hint_str(schema: &Value) -> &'static str {
    infer_type(schema)
}

/// Returns true when `not` is the only meaningful keyword in the schema (no type, properties, etc.).
fn is_not_only_keyword(schema: &Value) -> bool {
    if let Value::Object(map) = schema {
        let meaningful_keys = [
            "type",
            "properties",
            "required",
            "items",
            "allOf",
            "anyOf",
            "oneOf",
            "enum",
            "const",
            "minLength",
            "maxLength",
            "pattern",
            "format",
            "minimum",
            "maximum",
            "exclusiveMinimum",
            "exclusiveMaximum",
            "multipleOf",
            "minItems",
            "maxItems",
            "uniqueItems",
        ];
        for key in &meaningful_keys {
            if map.contains_key(*key) {
                return false;
            }
        }
        return true;
    }
    false
}

/// Generates a value that does NOT conform to the `not` sub-schema.
///
/// Tries all other JSON types in a shuffled order and returns the first one that
/// does not validate against the excluded schema. Falls back to null if all types match.
fn generate_not(
    schema: &Value,
    root: &Value,
    opts: &GenerateOptions,
    rng: &mut Random,
    depth: usize,
) -> Value {
    let not_schema = match schema.get("not") {
        Some(s) => s,
        None => return Value::Null,
    };
    let all_types = ["string", "number", "boolean", "array", "object", "null"];
    let excluded_type = not_schema
        .get("type")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    for type_name in &all_types {
        if *type_name == excluded_type {
            continue;
        }
        let candidate_schema = serde_json::json!({"type": type_name});
        let val = dispatch_type(&candidate_schema, root, opts, rng, depth, type_name);
        if !val.is_null() || *type_name == "null" {
            return val;
        }
    }
    Value::Null
}

/// Known JSON Schema keywords — used to detect anonymous (non-keyword-keyed) schemas.
const SCHEMA_KEYWORDS: &[&str] = &[
    "$ref",
    "$defs",
    "$id",
    "$schema",
    "$anchor",
    "$vocabulary",
    "definitions",
    "type",
    "properties",
    "required",
    "additionalProperties",
    "patternProperties",
    "minProperties",
    "maxProperties",
    "dependencies",
    "dependentRequired",
    "dependentSchemas",
    "items",
    "prefixItems",
    "additionalItems",
    "unevaluatedItems",
    "minItems",
    "maxItems",
    "uniqueItems",
    "contains",
    "minContains",
    "maxContains",
    "minLength",
    "maxLength",
    "pattern",
    "format",
    "contentEncoding",
    "contentMediaType",
    "minimum",
    "maximum",
    "exclusiveMinimum",
    "exclusiveMaximum",
    "multipleOf",
    "enum",
    "const",
    "examples",
    "example",
    "default",
    "allOf",
    "anyOf",
    "oneOf",
    "not",
    "if",
    "then",
    "else",
    "title",
    "description",
    "readOnly",
    "writeOnly",
    "nullable",
    "deprecated",
    "discriminator",
    "xml",
    "externalDocs",
    "x-faker",
];

/// Returns a schema with `properties` and `required` set if the input has only non-keyword keys.
///
/// Upstream json-schema-faker treats schemas like `{"x": {"enum": ["y"]}}` as
/// `{"properties": {"x": {"enum": ["y"]}}, "required": ["x"]}` when no recognized
/// keywords are present, ensuring all keys are always included in the output.
fn as_anonymous_object(schema: &Value) -> Option<Value> {
    let map = schema.as_object()?;
    if map.is_empty() {
        return None;
    }
    let all_non_keyword = map.keys().all(|k| !SCHEMA_KEYWORDS.contains(&k.as_str()));
    if !all_non_keyword {
        return None;
    }
    let properties = Value::Object(map.clone().into_iter().collect());
    let required: Vec<Value> = map.keys().map(|k| Value::String(k.clone())).collect();
    let mut new_schema = serde_json::Map::new();
    new_schema.insert("type".to_string(), Value::String("object".to_string()));
    new_schema.insert("properties".to_string(), properties);
    new_schema.insert("required".to_string(), Value::Array(required));
    Some(Value::Object(new_schema))
}

/// Public walk function that handles `type` arrays and delegates to the internal walker.
///
/// At the top-level invocation (`depth == 0`) the thread-local `$ref` visited set is
/// cleared so each `generate()` call starts with an empty `ref_depth_window` budget,
/// and the deferred-false-schema flag is finalized after the walk so an unhandled
/// embedded `false` schema becomes a `CannotGenerateForFalseSchema` error iff no other
/// generator already reported a more specific failure.
pub fn walk_schema(
    schema: &Value,
    root: &Value,
    opts: &GenerateOptions,
    rng: &mut Random,
    depth: usize,
) -> Value {
    let is_top_level = depth == 0;
    reset_ref_visited_if_top_level(depth);
    if is_top_level {
        reset_false_schema_seen();
    }
    let result = if let Some(Value::Array(_)) = schema.get("type") {
        walk_with_multi_type(schema, root, opts, rng, depth)
    } else {
        walk(schema, root, opts, rng, depth)
    };
    if is_top_level && false_schema_seen() && !rng.has_error() {
        rng.set_error(crate::FakerError::CannotGenerateForFalseSchema);
    }
    result
}

/// Renames schema-level keys according to the configured `prop_aliases` map.
///
/// Returns `Some(renamed_schema)` only when at least one alias was actually applied,
/// so the caller can avoid a redundant clone when the map is empty or no alias key
/// is present in the current schema. When the canonical key is already present on
/// the schema, the alias is treated as a no-op for that key so existing values are
/// not clobbered.
///
/// This function operates at the SCHEMA stage: it rewrites schema keys (for example
/// renaming a custom `props` keyword to the canonical `properties`) before dispatch.
/// The object generator (`generators::object::apply_prop_aliases`) operates at the
/// OUTPUT stage: it rewrites generated object property keys after the values are
/// produced. The two stages are not redundant — schema-key rewriting governs how
/// the walker interprets a schema, while output-key rewriting governs the shape of
/// the resulting JSON. The implementation here is also idempotent: a second
/// invocation on an already-aliased schema finds no `from` keys to rewrite and
/// returns `None`, so even an accidental double-application is safe.
fn apply_schema_prop_aliases(
    schema: &Value,
    aliases: &std::collections::HashMap<String, String>,
) -> Option<Value> {
    if aliases.is_empty() {
        return None;
    }
    let map = schema.as_object()?;
    let mut sorted_aliases: Vec<(&String, &String)> = aliases.iter().collect();
    sorted_aliases.sort_by(|a, b| a.0.cmp(b.0));
    let mut claimed_targets: std::collections::HashSet<&String> = std::collections::HashSet::new();
    let mut applicable: Vec<(&String, &String)> = Vec::new();
    for (from, to) in sorted_aliases {
        if from == to {
            continue;
        }
        if !map.contains_key(from) || map.contains_key(to) {
            continue;
        }
        if claimed_targets.contains(to) {
            continue;
        }
        claimed_targets.insert(to);
        applicable.push((from, to));
    }
    if applicable.is_empty() {
        return None;
    }
    let mut renamed = map.clone();
    for (from, to) in applicable {
        if let Some(value) = renamed.remove(from) {
            renamed.insert(to.clone(), value);
        }
    }
    Some(Value::Object(renamed))
}

/// Returns true when a schema object containing `$ref` also has any other keyword
/// that should be merged with the resolved target per Draft 2020-12 semantics.
fn schema_has_sibling_keywords(schema: &Value) -> bool {
    match schema.as_object() {
        Some(map) => map.keys().any(|k| k != "$ref"),
        None => false,
    }
}

/// Returns a clone of the schema with the `$ref` key removed, used to build the
/// overlay applied on top of a resolved reference when sibling keywords are present.
fn schema_without_ref(schema: &Value) -> Value {
    match schema.as_object() {
        Some(map) => {
            let mut clone = map.clone();
            clone.remove("$ref");
            Value::Object(clone)
        }
        None => schema.clone(),
    }
}

/// Returns a stable string identifier for a schema, used to key per-schema counters
/// such as the `autoIncrement` keyword.
///
/// Uses canonical JSON serialization so semantically identical schemas produce the
/// same fingerprint regardless of map iteration order at construction time.
fn schema_fingerprint(schema: &Value) -> String {
    serde_json::to_string(schema).unwrap_or_default()
}

/// Returns a per-instance counter key for an `autoIncrement` schema node.
///
/// Combines the walker's current JSON-pointer path with the schema's structural
/// fingerprint so that two structurally identical `{autoIncrement: true}` schemas
/// appearing at different positions in the document maintain independent counters.
/// Without the path component, the previous fingerprint-only key would collide
/// across positions and cause the second occurrence to continue the first's sequence
/// rather than starting from its own `initialOffset`.
fn auto_increment_key(schema: &Value, rng: &Random) -> String {
    format!("{}#{}", rng.current_path(), schema_fingerprint(schema))
}

/// Emits a one-shot `tracing::warn!` when the schema declares `unevaluatedProperties`
/// or `unevaluatedItems` so users see that the walker does not yet honour the keyword
/// rather than experiencing silent miscompilation.
///
/// The 2020-12 `unevaluated*` semantics depend on tracking which properties/items have
/// been "evaluated" by the various sibling keywords (`properties`, `patternProperties`,
/// composition branches). Implementing that bookkeeping faithfully is a substantial
/// addition; until that lands, [`crate::generators::object`] and
/// [`crate::generators::array`] continue to honour the simpler `additionalProperties`
/// / `additionalItems` fallback. This warning makes the gap observable to callers
/// who turn on `chasm_faker::schema_walker` tracing.
fn warn_unevaluated_keywords(schema: &Value) {
    if let Some(map) = schema.as_object() {
        if map.contains_key("unevaluatedProperties") {
            tracing::warn!(
                target: "chasm_faker::schema_walker",
                keyword = "unevaluatedProperties",
                "keyword not yet honoured"
            );
        }
        if map.contains_key("unevaluatedItems") {
            tracing::warn!(
                target: "chasm_faker::schema_walker",
                keyword = "unevaluatedItems",
                "keyword not yet honoured"
            );
        }
    }
}

/// Returns true when two schemas declare incompatible top-level `type` constraints,
/// indicating that merging them would yield a contradiction the walker cannot satisfy.
///
/// Used by the if/then/else walker to decide whether to walk the `if` candidate against
/// the merged base+if schema (preferred, so base constraints flow in) or against just
/// the base (fallback when the merge is inconsistent).
pub fn schemas_contradict(a: &Value, b: &Value) -> bool {
    let type_a = a.get("type").and_then(|v| v.as_str());
    let type_b = b.get("type").and_then(|v| v.as_str());
    let type_clash = match (type_a, type_b) {
        (Some(ta), Some(tb)) => {
            ta != tb && !(ta == "number" && tb == "integer") && !(ta == "integer" && tb == "number")
        }
        _ => false,
    };
    if type_clash {
        return true;
    }
    if numeric_range_contradicts(a, b) || numeric_range_contradicts(b, a) {
        return true;
    }
    if length_range_contradicts(a, b) || length_range_contradicts(b, a) {
        return true;
    }
    if let (Some(Value::Object(pa)), Some(Value::Object(pb))) =
        (a.get("properties"), b.get("properties"))
    {
        for (key, sub_b) in pb {
            if let Some(sub_a) = pa.get(key) {
                if let (Some(ca), Some(cb)) = (sub_a.get("const"), sub_b.get("const")) {
                    if ca != cb {
                        return true;
                    }
                }
                if let (Some(Value::Array(ea)), Some(Value::Array(eb))) =
                    (sub_a.get("enum"), sub_b.get("enum"))
                {
                    if ea.iter().all(|v| !eb.contains(v)) {
                        return true;
                    }
                }
                if let (Some(c), Some(Value::Array(arr))) = (sub_a.get("const"), sub_b.get("enum"))
                {
                    if !arr.contains(c) {
                        return true;
                    }
                }
                if let (Some(Value::Array(arr)), Some(c)) = (sub_a.get("enum"), sub_b.get("const"))
                {
                    if !arr.contains(c) {
                        return true;
                    }
                }
            }
        }
    }
    false
}

/// Returns true when `a`'s lower bound (`minimum` or `exclusiveMinimum`) is incompatible
/// with `b`'s upper bound (`maximum` or `exclusiveMaximum`).
///
/// Specifically:
/// - `{minimum: A}` vs `{maximum: B}` clashes when `A > B`
/// - `{exclusiveMinimum: A}` vs `{maximum: B}` clashes when `A >= B`
/// - `{minimum: A}` vs `{exclusiveMaximum: B}` clashes when `A >= B`
/// - `{exclusiveMinimum: A}` vs `{exclusiveMaximum: B}` clashes when `A >= B`
fn numeric_range_contradicts(a: &Value, b: &Value) -> bool {
    let min = a.get("minimum").and_then(|v| v.as_f64());
    let excl_min = a.get("exclusiveMinimum").and_then(|v| v.as_f64());
    let max = b.get("maximum").and_then(|v| v.as_f64());
    let excl_max = b.get("exclusiveMaximum").and_then(|v| v.as_f64());
    if let (Some(lo), Some(hi)) = (min, max) {
        if lo > hi {
            return true;
        }
    }
    if let (Some(lo), Some(hi)) = (excl_min, max) {
        if lo >= hi {
            return true;
        }
    }
    if let (Some(lo), Some(hi)) = (min, excl_max) {
        if lo >= hi {
            return true;
        }
    }
    if let (Some(lo), Some(hi)) = (excl_min, excl_max) {
        if lo >= hi {
            return true;
        }
    }
    false
}

/// Returns true when `a`'s `minLength` exceeds `b`'s `maxLength`, indicating an
/// unsatisfiable string-length range when both constraints are applied together.
fn length_range_contradicts(a: &Value, b: &Value) -> bool {
    let min = a.get("minLength").and_then(|v| v.as_u64());
    let max = b.get("maxLength").and_then(|v| v.as_u64());
    matches!((min, max), (Some(lo), Some(hi)) if lo > hi)
}

/// Walks an `if`/`then`/`else` conditional schema, deciding the branch via a structural
/// check on the `if` sub-schema rather than treating "non-null walk output" as truth.
///
/// A previous implementation lived in `composition` and evaluated the
/// `if` schema by walking it and treating any non-null result as condition-met. That
/// produced the wrong branch when the `if` schema legitimately required a null value
/// (e.g. `{"type": "null"}`), since the generated value would be `Value::Null` and the
/// caller would always take the `else` branch.
///
/// This local re-implementation generates a candidate value from the merged base+then
/// schema (or base+else if no then) and decides based on whether that candidate
/// satisfies the `if` sub-schema's structural constraints, falling back to walking the
/// `if` schema and using the structural-satisfaction check on its result.
fn walk_if_then_else(
    schema: &Value,
    root: &Value,
    opts: &GenerateOptions,
    rng: &mut Random,
    depth: usize,
) -> Value {
    let if_schema = match schema.get("if") {
        Some(s) => s.clone(),
        None => return Value::Null,
    };

    let mut base = schema.clone();
    if let Value::Object(ref mut map) = base {
        map.remove("if");
        map.remove("then");
        map.remove("else");
    }

    let candidate_schema = crate::merge::deep_merge(base.clone(), if_schema.clone());
    let if_result = if schemas_contradict(&base, &if_schema) {
        walk(&base, root, opts, rng, depth + 1)
    } else {
        walk(&candidate_schema, root, opts, rng, depth + 1)
    };
    let condition_met = if_schema_evaluates_true_for(&if_result, &if_schema);

    let branch_schema = if condition_met {
        schema.get("then").cloned()
    } else {
        schema.get("else").cloned()
    };

    if let Some(branch) = branch_schema {
        if branch != Value::Bool(false) {
            let base_constrained = if condition_met {
                crate::merge::deep_merge(base.clone(), if_schema)
            } else {
                base.clone()
            };
            let merged = crate::merge::deep_merge(base_constrained, branch);
            return walk(&merged, root, opts, rng, depth + 1);
        }
    }

    walk(&base, root, opts, rng, depth + 1)
}

/// Returns true when `value` structurally satisfies the simple constraints expressed by
/// the `if` sub-schema.
///
/// Uses the existing `value_satisfies_simple_constraints` checker which already handles
/// `type`, `const`, `enum`, and the numeric/string constraint keywords. This is the
/// correct condition predicate for `if/then/else`: a legitimate `{type: "null"}` if-schema
/// is satisfied by a null value, whereas the prior "non-null result" heuristic would
/// have always taken the `else` branch in that case.
fn if_schema_evaluates_true_for(value: &Value, if_schema: &Value) -> bool {
    value_satisfies_simple_constraints(value, if_schema)
}
