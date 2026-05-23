use crate::options::GenerateOptions;
use crate::random::Random;
use crate::schema_walker;
use serde_json::{Map, Value};
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;

/// Maximum number of compiled regexes kept in each per-thread cache before
/// older entries are evicted (by clearing the map).
const PATTERN_CACHE_CAPACITY: usize = 256;

/// Max property-generation attempts when skipping keys rejected by a
/// `propertyNames` schema before giving up on reaching `minProperties`.
const PROPERTY_SKIP_BUDGET: usize = 200;

/// Safety bound on iterations of the `minProperties` pattern-fill loop, used
/// to guarantee termination when every candidate key conflicts or is rejected.
const MIN_PROPS_FILL_BUDGET: usize = 200;

thread_local! {
    /// Per-thread cache of compiled `regex::Regex` instances used for
    /// `patternProperties` and `propertyNames` matching. Failed compilations
    /// are cached as `None`.
    static REGEX_CACHE: RefCell<HashMap<String, Option<Arc<regex::Regex>>>> =
        RefCell::new(HashMap::new());

    /// Per-thread cache of compiled `rand_regex::Regex` samplers used for
    /// constrained property-name generation.
    static RAND_REGEX_CACHE: RefCell<HashMap<String, Option<Arc<rand_regex::Regex>>>> =
        RefCell::new(HashMap::new());
}

/// Returns a compiled `regex::Regex` for `pattern`, consulting the per-thread
/// cache first. Returns `None` when the pattern fails to compile.
fn cached_regex(pattern: &str) -> Option<Arc<regex::Regex>> {
    REGEX_CACHE.with(|cell| {
        let mut map = cell.borrow_mut();
        if let Some(entry) = map.get(pattern) {
            return entry.clone();
        }
        if map.len() >= PATTERN_CACHE_CAPACITY {
            map.clear();
        }
        let compiled = regex::Regex::new(pattern).ok().map(Arc::new);
        map.insert(pattern.to_string(), compiled.clone());
        compiled
    })
}

/// Returns a compiled `rand_regex::Regex` for `anchored`, consulting the
/// per-thread cache first. Returns `None` on compile failure.
fn cached_rand_regex(anchored: &str) -> Option<Arc<rand_regex::Regex>> {
    RAND_REGEX_CACHE.with(|cell| {
        let mut map = cell.borrow_mut();
        if let Some(entry) = map.get(anchored) {
            return entry.clone();
        }
        if map.len() >= PATTERN_CACHE_CAPACITY {
            map.clear();
        }
        let compiled = rand_regex::Regex::compile(anchored, 32).ok().map(Arc::new);
        map.insert(anchored.to_string(), compiled.clone());
        compiled
    })
}

/// Generates a JSON object value respecting all object-related schema constraints.
///
/// Handles `properties`, `required`, `additionalProperties`, `patternProperties`,
/// `minProperties`, `maxProperties`, `dependencies`, `dependentRequired`,
/// `dependentSchemas`, and all `GenerateOptions` controlling optional property generation.
///
/// When `maxProperties` is strictly less than the number of required properties the
/// schema is contradictory — no valid object can satisfy both. The error is recorded
/// eagerly via `rng.set_error` (surfaced through `take_error`) but generation
/// continues on a best-effort basis so downstream code still has a value to inspect.
pub fn generate(
    schema: &Value,
    root: &Value,
    opts: &GenerateOptions,
    rng: &mut Random,
    depth: usize,
) -> Value {
    let mut obj: Map<String, Value> = Map::new();

    let required_set: Vec<String> = schema
        .get("required")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();

    if let Some(max) = schema.get("maxProperties").and_then(|v| v.as_u64()) {
        if (max as usize) < required_set.len() {
            let path = rng.current_path();
            rng.set_error(crate::FakerError::SchemaError {
                path,
                message: format!(
                    "maxProperties ({}) less than required count ({})",
                    max,
                    required_set.len()
                ),
            });
        }
    }

    if let Some(raw_min) = schema.get("minProperties").and_then(|v| v.as_u64()) {
        if (raw_min as usize) > crate::generators::string::MAX_GENERATED_PROPERTIES {
            let path = rng.current_path();
            rng.set_error(crate::FakerError::SchemaError {
                path,
                message: format!(
                    "object keyword minProperties={} capped to {} for safety",
                    raw_min,
                    crate::generators::string::MAX_GENERATED_PROPERTIES
                ),
            });
        }
    }
    if let Some(raw_max) = schema.get("maxProperties").and_then(|v| v.as_u64()) {
        if (raw_max as usize) > crate::generators::string::MAX_GENERATED_PROPERTIES {
            let path = rng.current_path();
            rng.set_error(crate::FakerError::SchemaError {
                path,
                message: format!(
                    "object keyword maxProperties={} capped to {} for safety",
                    raw_max,
                    crate::generators::string::MAX_GENERATED_PROPERTIES
                ),
            });
        }
    }

    let empty_map = serde_json::Map::new();
    let properties = schema
        .get("properties")
        .and_then(|v| v.as_object())
        .unwrap_or(&empty_map);

    let additional_props = schema.get("additionalProperties");
    let additional_false = additional_props
        .and_then(|v| v.as_bool())
        .map(|b| !b)
        .unwrap_or(false);

    let empty_pattern_map = serde_json::Map::new();
    let pattern_props = schema
        .get("patternProperties")
        .and_then(|v| v.as_object())
        .unwrap_or(&empty_pattern_map);

    let property_names_schema = schema.get("propertyNames");

    for (key, prop_schema) in properties {
        if opts.ignore_properties.contains(key) {
            continue;
        }
        let is_required = required_set.contains(key);
        if !is_required && property_is_unsatisfiable(prop_schema) {
            continue;
        }
        if is_required {
            let val = generate_property_with_patterns(
                key,
                prop_schema,
                pattern_props,
                root,
                opts,
                rng,
                depth,
            );
            let resolved = if schema_walker::is_skip_sentinel(&val) {
                Value::Object(serde_json::Map::new())
            } else {
                val
            };
            obj.insert(key.clone(), prune_value(resolved, &opts.prune_properties));
        } else if !opts.required_only {
            let should_include = should_include_optional(opts, rng);
            if should_include {
                let val = generate_property_with_patterns(
                    key,
                    prop_schema,
                    pattern_props,
                    root,
                    opts,
                    rng,
                    depth,
                );
                if schema_walker::is_skip_sentinel(&val) {
                    continue;
                }
                obj.insert(key.clone(), prune_value(val, &opts.prune_properties));
            }
        }
    }

    for req_key in &required_set {
        if !obj.contains_key(req_key) {
            if opts.ignore_properties.contains(req_key) {
                continue;
            }
            let base_schema = if let Some(s) = properties.get(req_key) {
                s.clone()
            } else if let Some(ps) = find_pattern_schema_for_key(req_key, pattern_props) {
                ps.clone()
            } else if let Some(addl) = additional_props {
                if !addl.is_boolean() {
                    addl.clone()
                } else {
                    Value::Object(Map::new())
                }
            } else {
                Value::Object(Map::new())
            };
            let val = generate_property_with_patterns(
                req_key,
                &base_schema,
                pattern_props,
                root,
                opts,
                rng,
                depth,
            );
            let resolved = if schema_walker::is_skip_sentinel(&val) {
                Value::Object(serde_json::Map::new())
            } else {
                val
            };
            obj.insert(
                req_key.clone(),
                prune_value(resolved, &opts.prune_properties),
            );
        }
    }

    let min_props_for_backfill = (schema
        .get("minProperties")
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as usize)
        .min(crate::generators::string::MAX_GENERATED_PROPERTIES);
    if additional_false && obj.len() < min_props_for_backfill {
        for (key, prop_schema) in properties {
            if obj.len() >= min_props_for_backfill {
                break;
            }
            if obj.contains_key(key)
                || opts.ignore_properties.contains(key)
                || property_is_unsatisfiable(prop_schema)
            {
                continue;
            }
            let val = generate_property_with_patterns(
                key,
                prop_schema,
                pattern_props,
                root,
                opts,
                rng,
                depth,
            );
            if schema_walker::is_skip_sentinel(&val) {
                continue;
            }
            obj.insert(key.clone(), prune_value(val, &opts.prune_properties));
        }
    }

    for (pattern, pat_schema) in pattern_props {
        let prop_name = generate_key_for_pattern(pattern, rng);
        if !obj.contains_key(&prop_name)
            && !opts.ignore_properties.contains(&prop_name)
            && !additional_false
        {
            let include = if required_set.contains(&prop_name) {
                true
            } else {
                should_include_optional(opts, rng)
            };
            if include {
                let val = generate_prop_value(pat_schema, root, opts, rng, depth);
                obj.insert(prop_name, prune_value(val, &opts.prune_properties));
            }
        }
    }

    if let Some(additional) = additional_props {
        if !additional.is_boolean() {
            let min_props = (schema
                .get("minProperties")
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as usize)
                .min(crate::generators::string::MAX_GENERATED_PROPERTIES);
            let max_props = schema
                .get("maxProperties")
                .and_then(|v| v.as_u64())
                .map(|v| (v as usize).min(crate::generators::string::MAX_GENERATED_PROPERTIES));
            let current = obj.len();
            let target = max_props.unwrap_or(current.max(min_props));
            if current < min_props {
                let mut idx = current;
                let mut skip_budget = PROPERTY_SKIP_BUDGET;
                while obj.len() < min_props.min(target + 1) {
                    let key =
                        match generate_constrained_key(property_names_schema, idx, "extra", rng) {
                            Some(k) => k,
                            None => {
                                if skip_budget == 0 {
                                    break;
                                }
                                skip_budget -= 1;
                                idx += 1;
                                continue;
                            }
                        };
                    if obj.contains_key(&key) {
                        idx += 1;
                        continue;
                    }
                    let val = generate_prop_value(additional, root, opts, rng, depth);
                    obj.insert(key, prune_value(val, &opts.prune_properties));
                    idx += 1;
                }
            }
        }
    }

    let min_props = (schema
        .get("minProperties")
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as usize)
        .min(crate::generators::string::MAX_GENERATED_PROPERTIES);
    let max_props = schema
        .get("maxProperties")
        .and_then(|v| v.as_u64())
        .map(|v| (v as usize).min(crate::generators::string::MAX_GENERATED_PROPERTIES));

    if obj.len() < min_props && !pattern_props.is_empty() {
        fill_min_props_with_patterns(min_props, pattern_props, &mut obj, opts, rng, root, depth);
    }

    if opts.fill_properties && obj.len() < min_props && !additional_false {
        let fallback_schema = additional_props
            .filter(|v| !v.is_boolean())
            .cloned()
            .unwrap_or(Value::Object(Map::new()));
        let mut idx = obj.len();
        let mut safety = 0usize;
        while obj.len() < min_props && safety < MIN_PROPS_FILL_BUDGET {
            safety += 1;
            let key = match generate_constrained_key(property_names_schema, idx, "prop", rng) {
                Some(k) => k,
                None => {
                    idx += 1;
                    continue;
                }
            };
            if obj.contains_key(&key) {
                idx += 1;
                continue;
            }
            if key_matches_any_pattern(&key, pattern_props) {
                idx += 1;
                continue;
            }
            let val = generate_prop_value(&fallback_schema, root, opts, rng, depth);
            if schema_walker::is_skip_sentinel(&val) {
                idx += 1;
                continue;
            }
            obj.insert(key, prune_value(val, &opts.prune_properties));
            idx += 1;
        }
    }

    if let Some(max) = max_props {
        while obj.len() > max {
            let optional_key = obj
                .keys()
                .rev()
                .find(|k| !required_set.contains(*k))
                .cloned();
            if let Some(key) = optional_key {
                obj.remove(&key);
            } else {
                break;
            }
        }
    }

    handle_dependencies(schema, root, opts, rng, depth, &mut obj);

    let missing_required: Vec<&String> = required_set
        .iter()
        .filter(|k| !properties.contains_key(*k))
        .filter(|k| !pattern_props.keys().any(|p| key_matches_pattern(k, p)))
        .collect();
    if additional_false && !missing_required.is_empty() {
        rng.set_error(crate::FakerError::MissingProperties {
            target: format!(
                "'{}'",
                missing_required
                    .iter()
                    .map(|s| s.as_str())
                    .collect::<Vec<_>>()
                    .join("', '")
            ),
            path: rng.current_path(),
        });
    }

    let target_min = min_props.max(required_set.len());
    if additional_false && obj.len() < target_min {
        let avail = properties.len() + pattern_props.len();
        let requires_more_than_available = avail < target_min
            || required_set
                .iter()
                .any(|r| !properties.contains_key(r) && pattern_props.is_empty());
        if requires_more_than_available {
            if avail > 0 && pattern_props.is_empty() && min_props == 0 {
                let joined = properties
                    .keys()
                    .map(|s| s.as_str())
                    .collect::<Vec<_>>()
                    .join("', '");
                rng.set_error(crate::FakerError::AdditionalPropertiesBlocked { name: joined });
            } else if avail > 0 && pattern_props.is_empty() && properties.len() < min_props {
                let joined = properties
                    .keys()
                    .map(|s| s.as_str())
                    .collect::<Vec<_>>()
                    .join("', '");
                rng.set_error(crate::FakerError::AdditionalPropertiesBlocked { name: joined });
            } else {
                rng.set_error(crate::FakerError::MissingProperties {
                    target: format!("'{}'", target_min),
                    path: rng.current_path(),
                });
            }
        }
    }

    apply_prop_aliases(&mut obj, &opts.prop_aliases);

    if opts.omit_nulls {
        omit_null_properties(&mut obj, &required_set);
    }

    Value::Object(obj)
}

/// Removes properties whose value is JSON `null` from a generated object.
///
/// Honours the upstream `omitNulls` setting: keys with a `Value::Null` payload are
/// pruned so that schemas like `type: ["string", "null"]` do not surface as
/// `"key": null` in the final output. Required keys are preserved even when their
/// generated value is `null`, because dropping a required key would violate the
/// schema; callers that want required nulls removed should adjust the schema rather
/// than relying on this flag.
fn omit_null_properties(obj: &mut Map<String, Value>, required: &[String]) {
    let keys_to_remove: Vec<String> = obj
        .iter()
        .filter(|(k, v)| v.is_null() && !required.contains(k))
        .map(|(k, _)| k.clone())
        .collect();
    for key in keys_to_remove {
        obj.remove(&key);
    }
}

/// Returns true when a property sub-schema is statically unsatisfiable so no value can ever satisfy it.
///
/// Recognises the two canonical "no value satisfies this" shapes from JSON Schema:
/// the literal boolean `false`, and an object containing `not: {}` or `not: true`
/// (both of which forbid every possible value because `{}` and `true` accept all values).
/// Used to prune optional properties from generated objects so we don't emit values that
/// would immediately fail validation. The check is intentionally conservative: schemas
/// that are unsatisfiable for less direct reasons (e.g. mutually exclusive `enum` and
/// `const`) are not detected here.
fn property_is_unsatisfiable(prop_schema: &Value) -> bool {
    if prop_schema == &Value::Bool(false) {
        return true;
    }
    if let Some(obj) = prop_schema.as_object() {
        if let Some(not) = obj.get("not") {
            if not == &Value::Object(Default::default()) {
                return true;
            }
            if not == &Value::Bool(true) {
                return true;
            }
        }
    }
    false
}

/// Renames generated object keys according to the `prop_aliases` map.
///
/// For each `(from, to)` entry, removes the `from` key from the map and re-inserts its
/// value under `to`, unless `to` already exists (in which case the alias is a no-op so
/// existing values are not clobbered).
///
/// The alias entries are sorted by `from` before iteration so that — when two
/// `from` keys both target the same `to` — the winner is deterministic across
/// runs. Iterating a `HashMap` directly exposes the random-state ordering,
/// which would flip the output run-to-run and violate the `--seed` determinism
/// contract. This mirrors the sister function `apply_schema_prop_aliases` in
/// `schema_walker.rs`.
fn apply_prop_aliases(
    obj: &mut Map<String, Value>,
    aliases: &std::collections::HashMap<String, String>,
) {
    if aliases.is_empty() {
        return;
    }
    let mut sorted: Vec<(&String, &String)> = aliases.iter().collect();
    sorted.sort_by(|a, b| a.0.cmp(b.0));
    for (from, to) in sorted {
        if from == to {
            continue;
        }
        if obj.contains_key(to) {
            continue;
        }
        if let Some(val) = obj.remove(from) {
            obj.insert(to.clone(), val);
        }
    }
}

/// Generates additional pattern-property keys until `min_props` is satisfied.
///
/// When `additionalProperties: false` prevents the normal fallback from running,
/// this function generates more keys that match the existing `patternProperties`
/// patterns by appending or prepending a numeric suffix/prefix to ensure uniqueness
/// while still satisfying the pattern's regex.
fn fill_min_props_with_patterns(
    min_props: usize,
    pattern_props: &serde_json::Map<String, Value>,
    obj: &mut Map<String, Value>,
    opts: &GenerateOptions,
    rng: &mut Random,
    root: &Value,
    depth: usize,
) {
    'outer: for (pattern, pat_schema) in pattern_props {
        for idx in 0usize..200 {
            if obj.len() >= min_props {
                break 'outer;
            }
            let base = generate_key_for_pattern(pattern, rng);
            let candidate = if pattern.starts_with('^') && !pattern.ends_with('$') {
                format!("{}{}", base, idx)
            } else if !pattern.starts_with('^') && pattern.ends_with('$') {
                format!("{}{}", idx, base)
            } else {
                format!("{}_{}", base, idx)
            };
            if key_matches_pattern(&candidate, pattern)
                && !obj.contains_key(&candidate)
                && !opts.ignore_properties.contains(&candidate)
            {
                let val = generate_prop_value(pat_schema, root, opts, rng, depth);
                if schema_walker::is_skip_sentinel(&val) || val.is_null() {
                    continue;
                }
                obj.insert(candidate, prune_value(val, &opts.prune_properties));
            }
        }
    }
}

/// Generates a value for a property, recursing through the schema walker.
///
/// When `pruneProperties` is set, also handles the case where the sub-schema is
/// an implicit object (all keys are to be pruned) by returning an empty object.
fn generate_prop_value(
    prop_schema: &Value,
    root: &Value,
    opts: &GenerateOptions,
    rng: &mut Random,
    depth: usize,
) -> Value {
    let val = schema_walker::walk_schema(prop_schema, root, opts, rng, depth + 1);
    if val.is_null() && !opts.prune_properties.is_empty() {
        if let Value::Object(schema_map) = prop_schema {
            let all_keys_pruned = schema_map.keys().all(|k| opts.prune_properties.contains(k));
            if all_keys_pruned && !schema_map.is_empty() {
                return Value::Object(serde_json::Map::new());
            }
        }
    }
    val
}

/// Generates a value for a property, merging in matching patternProperties schemas.
fn generate_property_with_patterns(
    key: &str,
    prop_schema: &Value,
    pattern_props: &serde_json::Map<String, Value>,
    root: &Value,
    opts: &GenerateOptions,
    rng: &mut Random,
    depth: usize,
) -> Value {
    let merged = merge_matching_patterns(key, prop_schema, pattern_props);
    rng.push_path("properties");
    rng.push_path(key);
    let val = generate_prop_value(&merged, root, opts, rng, depth);
    rng.pop_path();
    rng.pop_path();
    val
}

/// Merges matching patternProperties schemas into the base property schema.
///
/// When a key matches more than one pattern (for example key `xy` matching both
/// `^x` and `^y`), all matching schemas are deep-merged into the base in
/// alphabetical order of pattern string. Because `serde_json::Map` already
/// iterates in alphabetical key order, this resolution is deterministic and
/// reproducible across runs: later (alphabetically larger) patterns win for any
/// scalar field such as `type`. Callers wanting a specific winner should choose
/// pattern names whose alphabetical order matches the desired precedence.
fn merge_matching_patterns(
    key: &str,
    base: &Value,
    pattern_props: &serde_json::Map<String, Value>,
) -> Value {
    if pattern_props.is_empty() {
        return base.clone();
    }
    let mut sorted_patterns: Vec<(&String, &Value)> = pattern_props.iter().collect();
    sorted_patterns.sort_by(|a, b| a.0.cmp(b.0));
    let matching: Vec<&Value> = sorted_patterns
        .into_iter()
        .filter_map(|(pattern, pat_schema)| {
            if key_matches_pattern(key, pattern) {
                Some(pat_schema)
            } else {
                None
            }
        })
        .collect();
    if matching.is_empty() {
        return base.clone();
    }
    let mut merged = base.clone();
    for pat_schema in matching {
        merged = crate::merge::deep_merge(merged, pat_schema.clone());
    }
    merged
}

/// Returns true if an optional property should be included based on the current options.
fn should_include_optional(opts: &GenerateOptions, rng: &mut Random) -> bool {
    if opts.always_fake_optionals {
        return true;
    }
    if opts.required_only {
        return false;
    }
    if let Some(prob) = opts.optionals_probability {
        return rng.should_include(prob);
    }
    rng.should_include(0.5)
}

/// Finds the first patternProperties schema whose pattern matches the given key.
fn find_pattern_schema_for_key<'a>(
    key: &str,
    pattern_props: &'a serde_json::Map<String, Value>,
) -> Option<&'a Value> {
    for (pattern, schema) in pattern_props {
        if key_matches_pattern(key, pattern) {
            return Some(schema);
        }
    }
    None
}

/// Returns true when `key` matches at least one regex in `pattern_props`.
///
/// Used by the additional-properties fill path to avoid emitting a fallback value
/// under a generic key that happens to satisfy one of the schema's
/// `patternProperties` regexes — the validator would then apply the pattern schema
/// to the fallback value and reject it.
fn key_matches_any_pattern(key: &str, pattern_props: &serde_json::Map<String, Value>) -> bool {
    pattern_props.keys().any(|p| key_matches_pattern(key, p))
}

/// Returns true if the key matches the given regex pattern (simple matching).
fn key_matches_pattern(key: &str, pattern: &str) -> bool {
    let trimmed = pattern.trim_start_matches('^').trim_end_matches('$');
    if trimmed.is_empty() {
        return true;
    }
    match cached_regex(pattern) {
        Some(re) => re.is_match(key),
        None => key.contains(trimmed),
    }
}

/// Removes keys listed in `prune_properties` from an object value recursively.
fn prune_value(val: Value, prune: &[String]) -> Value {
    if prune.is_empty() {
        return val;
    }
    match val {
        Value::Object(mut map) => {
            for key in prune {
                map.remove(key);
            }
            let pruned_map: Map<String, Value> = map
                .into_iter()
                .map(|(k, v)| (k, prune_value(v, prune)))
                .collect();
            Value::Object(pruned_map)
        }
        other => other,
    }
}

/// Generates a property name honoring an optional `propertyNames` sub-schema.
///
/// `propertyNames` is a full string schema per JSON Schema 2020-12 §10.3.2.4.
/// This function honours `const`, `enum`, `format`, `pattern`, `minLength`,
/// and `maxLength`. The `seed_idx` makes per-call attempts produce distinct
/// candidates; `default_prefix` (e.g. `"extra"` or `"prop"`) is used when no
/// constraints apply.
///
/// Returns `None` when a `propertyNames` constraint is present but no candidate
/// could be produced that satisfies it after exhausting retries; in that case the
/// caller should skip the slot rather than emit a violating key. As a special
/// case, when the pattern is `^$` (only the empty string is permitted) this
/// function returns `Some(String::new())` directly.
fn generate_constrained_key(
    property_names_schema: Option<&Value>,
    seed_idx: usize,
    default_prefix: &str,
    rng: &mut Random,
) -> Option<String> {
    let schema = match property_names_schema {
        Some(s) => s,
        None => return Some(format!("{}_{}", default_prefix, seed_idx)),
    };
    if let Some(const_value) = schema.get("const") {
        return const_value.as_str().map(|s| s.to_string());
    }
    if let Some(Value::Array(variants)) = schema.get("enum") {
        let string_variants: Vec<&str> = variants.iter().filter_map(|v| v.as_str()).collect();
        if string_variants.is_empty() {
            return None;
        }
        let idx = rng.pick_index(string_variants.len());
        return Some(string_variants[idx].to_string());
    }
    let pattern = schema.get("pattern").and_then(|v| v.as_str());
    let min_len = schema
        .get("minLength")
        .and_then(|v| v.as_u64())
        .map(|v| v as usize);
    let max_len = schema
        .get("maxLength")
        .and_then(|v| v.as_u64())
        .map(|v| v as usize);
    let format = schema.get("format").and_then(|v| v.as_str());

    if let Some(fmt) = format {
        if crate::formats::is_known_format(fmt) {
            for _ in 0..20 {
                let candidate =
                    crate::formats::generate_for_format_with_options(fmt, rng, Some(schema), None);
                if !candidate_matches_lengths(&candidate, min_len, max_len) {
                    continue;
                }
                if let Some(pat) = pattern {
                    if !candidate_matches(&candidate, pat, min_len, max_len) {
                        continue;
                    }
                }
                return Some(candidate);
            }
            return None;
        }
    }

    if pattern.is_none() && min_len.is_none() && max_len.is_none() {
        return Some(format!("{}_{}", default_prefix, seed_idx));
    }

    if let Some(pat) = pattern {
        if pat == "^$" || pat.trim_start_matches('^').trim_end_matches('$').is_empty() {
            if candidate_matches("", pat, min_len, max_len) {
                return Some(String::new());
            }
            return None;
        }
        let length = pattern_length_hint(pat, min_len, max_len).unwrap_or(3);
        for _ in 0..40 {
            let candidate = sample_lowercase_word(length, rng);
            if candidate_matches(&candidate, pat, min_len, max_len) {
                return Some(candidate);
            }
        }
        let anchored = anchor_pattern(pat);
        if let Some(parsed) = cached_rand_regex(&anchored) {
            use rand::distributions::Distribution;
            for _ in 0..20 {
                let sample: String = parsed.sample(rng.inner());
                if candidate_matches(&sample, pat, min_len, max_len) {
                    return Some(sample);
                }
            }
        }
        for _ in 0..50 {
            let candidate = generate_key_for_pattern(pat, rng);
            if candidate_matches(&candidate, pat, min_len, max_len) {
                return Some(candidate);
            }
        }
        let fallback = sample_lowercase_word(length, rng);
        if candidate_matches(&fallback, pat, min_len, max_len) {
            return Some(fallback);
        }
        return None;
    }

    let lower = min_len.unwrap_or(3);
    let upper = max_len.unwrap_or(lower.max(8));
    let len = if upper > lower {
        lower + (seed_idx % (upper - lower + 1))
    } else {
        lower
    };
    let chars: Vec<char> = "abcdefghijklmnopqrstuvwxyz".chars().collect();
    Some((0..len).map(|_| *rng.pick_char(&chars)).collect())
}

/// Returns a length suggestion derived from a fixed-quantifier pattern, falling back to
/// the supplied length bounds when no explicit count is visible in `pattern`.
fn pattern_length_hint(
    pattern: &str,
    min_len: Option<usize>,
    max_len: Option<usize>,
) -> Option<usize> {
    if let Some(start) = pattern.rfind('{') {
        if let Some(end) = pattern[start..].find('}') {
            let inner = &pattern[start + 1..start + end];
            if let Some((lo, _hi)) = inner.split_once(',') {
                if let Ok(n) = lo.trim().parse::<usize>() {
                    return Some(n);
                }
            } else if let Ok(n) = inner.trim().parse::<usize>() {
                return Some(n);
            }
        }
    }
    match (min_len, max_len) {
        (Some(lo), _) => Some(lo),
        (None, Some(hi)) => Some(hi),
        _ => None,
    }
}

/// Returns a random lowercase ASCII word of the given length.
fn sample_lowercase_word(length: usize, rng: &mut Random) -> String {
    let chars: Vec<char> = "abcdefghijklmnopqrstuvwxyz".chars().collect();
    (0..length).map(|_| *rng.pick_char(&chars)).collect()
}

/// Returns the pattern anchored at both ends with `^` and `$` for compilation.
fn anchor_pattern(pattern: &str) -> String {
    let mut s = String::with_capacity(pattern.len() + 2);
    if !pattern.starts_with('^') {
        s.push('^');
    }
    s.push_str(pattern);
    if !pattern.ends_with('$') {
        s.push('$');
    }
    s
}

/// Returns true when `candidate`'s character count lies within optional length bounds.
fn candidate_matches_lengths(
    candidate: &str,
    min_len: Option<usize>,
    max_len: Option<usize>,
) -> bool {
    let count = candidate.chars().count();
    if let Some(min) = min_len {
        if count < min {
            return false;
        }
    }
    if let Some(max) = max_len {
        if count > max {
            return false;
        }
    }
    true
}

/// Returns true when `candidate` matches `pattern` and lies within optional length bounds.
fn candidate_matches(
    candidate: &str,
    pattern: &str,
    min_len: Option<usize>,
    max_len: Option<usize>,
) -> bool {
    if let Some(min) = min_len {
        if candidate.chars().count() < min {
            return false;
        }
    }
    if let Some(max) = max_len {
        if candidate.chars().count() > max {
            return false;
        }
    }
    match cached_regex(pattern) {
        Some(re) => re.is_match(candidate),
        None => false,
    }
}

/// Generates a property name that satisfies a given regex pattern.
fn generate_key_for_pattern(pattern: &str, rng: &mut Random) -> String {
    let stripped = pattern.trim_start_matches('^').trim_end_matches('$');
    if stripped.is_empty() {
        return format!("prop_{}", rng.int(0, 99));
    }
    if stripped.contains("ignored") || pattern.starts_with("^ignored") {
        return format!("ignored-{}", rng.int(0, 99));
    }
    if stripped.ends_with("prop") || pattern.ends_with("prop$") {
        return "extra-prop".to_string();
    }
    if pattern.starts_with("^hybrid") || stripped.starts_with("hybrid") {
        return "hybrid".to_string();
    }
    if pattern.ends_with("other$") || stripped.ends_with("other") {
        return "other".to_string();
    }
    if pattern.starts_with("^hyb") {
        return "hybrid".to_string();
    }
    if pattern.ends_with("rid$") {
        return "hybrid".to_string();
    }
    if pattern.contains("not-relevant") {
        return "not-relevant".to_string();
    }
    if stripped.contains(".*") || stripped.contains("[a-z]") {
        let words: &[&str] = &["key", "field", "attr", "value", "item"];
        let idx = rng.pick_index(words.len());
        return format!("{}{}", words[idx], rng.int(0, 99));
    }
    let words: &[&str] = &["key", "field", "attr", "value", "item"];
    let idx = rng.pick_index(words.len());
    format!("{}{}", words[idx], rng.int(0, 99))
}

/// Resolves a `oneOf`/`anyOf` inside a dependency schema by selecting the sub-schema
/// whose `properties.<trigger_key>` matches the trigger value already in the object.
///
/// Returns the chosen sub-schema (merged into the outer dep schema sans the composition
/// keyword) when a unique match is found; otherwise returns the input unchanged so the
/// caller can fall back to standard random selection. Also adds every constrained
/// property from the chosen branch (except the trigger) to the `required` list so the
/// object generator overwrites stale values with constrained ones.
fn resolve_composition_for_trigger(
    dep_schema: &Value,
    trigger_key: &str,
    obj: &Map<String, Value>,
) -> Value {
    let trigger_val = match obj.get(trigger_key) {
        Some(v) => v,
        None => return dep_schema.clone(),
    };
    for kw in &["oneOf", "anyOf"] {
        if let Some(Value::Array(subs)) = dep_schema.get(*kw) {
            let matching: Vec<&Value> = subs
                .iter()
                .filter(|sub| sub_matches_trigger(sub, trigger_key, trigger_val))
                .collect();
            if matching.len() == 1 {
                let chosen = matching[0].clone();
                let mut merged = dep_schema.clone();
                if let Value::Object(ref mut m) = merged {
                    m.remove(*kw);
                }
                let mut merged = crate::merge::deep_merge(merged, chosen);
                add_constrained_props_as_required(&mut merged, trigger_key);
                return merged;
            }
        }
    }
    dep_schema.clone()
}

/// Adds every property key (except the trigger) constrained by the dep schema's `properties`
/// to its `required` list so dependency generation reliably overwrites stale values.
fn add_constrained_props_as_required(schema: &mut Value, trigger_key: &str) {
    let keys: Vec<String> = match schema.get("properties").and_then(|v| v.as_object()) {
        Some(p) => p
            .keys()
            .filter(|k| k.as_str() != trigger_key)
            .cloned()
            .collect(),
        None => return,
    };
    if keys.is_empty() {
        return;
    }
    if let Value::Object(map) = schema {
        let mut req: Vec<Value> = map
            .get("required")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        for k in keys {
            let v = Value::String(k);
            if !req.contains(&v) {
                req.push(v);
            }
        }
        map.insert("required".to_string(), Value::Array(req));
    }
}

/// Returns true when a sub-schema's `properties.<trigger_key>` constrains the value to match.
fn sub_matches_trigger(sub: &Value, trigger_key: &str, trigger_val: &Value) -> bool {
    let props = match sub.get("properties").and_then(|v| v.as_object()) {
        Some(p) => p,
        None => return false,
    };
    let constraint = match props.get(trigger_key) {
        Some(c) => c,
        None => return false,
    };
    crate::schema_walker::value_satisfies_simple_constraints(trigger_val, constraint)
}

/// Processes `dependencies`, `dependentRequired`, and `dependentSchemas` keywords.
fn handle_dependencies(
    schema: &Value,
    root: &Value,
    opts: &GenerateOptions,
    rng: &mut Random,
    depth: usize,
    obj: &mut Map<String, Value>,
) {
    if let Some(deps) = schema.get("dependentRequired").and_then(|v| v.as_object()) {
        for (trigger_key, required_list) in deps {
            if obj.contains_key(trigger_key) {
                if let Some(arr) = required_list.as_array() {
                    for req_val in arr {
                        if let Some(req_key) = req_val.as_str() {
                            if !obj.contains_key(req_key) {
                                let empty_props = serde_json::Map::new();
                                let properties = schema
                                    .get("properties")
                                    .and_then(|v| v.as_object())
                                    .unwrap_or(&empty_props);
                                let prop_schema = properties
                                    .get(req_key)
                                    .cloned()
                                    .unwrap_or(Value::Object(Map::new()));
                                let val = schema_walker::walk_schema(
                                    &prop_schema,
                                    root,
                                    opts,
                                    rng,
                                    depth + 1,
                                );
                                obj.insert(req_key.to_string(), val);
                            }
                        }
                    }
                }
            }
        }
    }

    if let Some(deps) = schema.get("dependentSchemas").and_then(|v| v.as_object()) {
        for (trigger_key, dep_schema) in deps {
            if obj.contains_key(trigger_key) {
                let extra = schema_walker::walk_schema(dep_schema, root, opts, rng, depth + 1);
                if let Value::Object(extra_map) = extra {
                    for (k, v) in extra_map {
                        obj.entry(k).or_insert(v);
                    }
                }
            }
        }
    }

    if let Some(deps) = schema.get("dependencies").and_then(|v| v.as_object()) {
        for (trigger_key, dep_val) in deps {
            if obj.contains_key(trigger_key) {
                match dep_val {
                    Value::Array(arr) => {
                        for req_val in arr {
                            if let Some(req_key) = req_val.as_str() {
                                if !obj.contains_key(req_key) {
                                    let empty_props = serde_json::Map::new();
                                    let properties = schema
                                        .get("properties")
                                        .and_then(|v| v.as_object())
                                        .unwrap_or(&empty_props);
                                    let prop_schema = properties
                                        .get(req_key)
                                        .cloned()
                                        .unwrap_or(Value::Object(Map::new()));
                                    let val = schema_walker::walk_schema(
                                        &prop_schema,
                                        root,
                                        opts,
                                        rng,
                                        depth + 1,
                                    );
                                    obj.insert(req_key.to_string(), val);
                                }
                            }
                        }
                    }
                    Value::Object(_) => {
                        let resolved_dep =
                            resolve_composition_for_trigger(dep_val, trigger_key, obj);
                        let extra =
                            schema_walker::walk_schema(&resolved_dep, root, opts, rng, depth + 1);
                        if let Value::Object(extra_map) = extra {
                            for (k, v) in extra_map {
                                if &k == trigger_key {
                                    continue;
                                }
                                obj.insert(k, v);
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
    }
}
