//! chasm-faker: JSON Schema-driven fake-value generator.
//!
//! Give it a JSON Schema, get back a `serde_json::Value` that satisfies it.
//! The crate is standalone — no OpenAPI, no HTTP. It's the random-data engine
//! that `chasm-engine` calls when no `example:` is declared in a spec.
//!
//! # Quick start
//!
//! ```
//! use chasm_faker::{generate, GenerateOptions};
//! use serde_json::json;
//!
//! let schema = json!({
//!     "type": "object",
//!     "properties": {
//!         "name": { "type": "string" },
//!         "age":  { "type": "integer", "minimum": 18, "maximum": 99 }
//!     },
//!     "required": ["name", "age"]
//! });
//!
//! let mut opts = GenerateOptions::default();
//! opts.seed = Some(42);
//!
//! let value = generate(&schema, &opts).unwrap();
//! assert!(value.is_object());
//! ```
//!
//! # Where to look next
//!
//! - [`generate`] — the main entry point. Takes a schema + options, returns a value.
//! - [`GenerateOptions`] — knobs: seed, depth caps, optional-property probability,
//!   example/default handling, output transforms.
//! - [`extensions`] — register custom `x-` keyword handlers or custom `format:`
//!   generators. Process-global; thread-safe.
//! - [`FakerError`] — what goes wrong: schema errors, depth blow-ups, all-branches-failed.

pub mod extensions;
pub(crate) mod formats;
pub(crate) mod generators;
pub(crate) mod merge;
pub mod options;
pub(crate) mod random;
pub(crate) mod ref_resolver;
pub(crate) mod schema_walker;

pub use extensions::{ExtensionRegistry, FormatRegistry};
pub use options::GenerateOptions;
use random::Random;

/// Re-exports of internal items that integration tests in this crate's
/// `tests/` directory need to construct directly.
///
/// This module is hidden from rustdoc and is **not** part of the public API:
/// downstream users must not depend on its contents. It exists solely so the
/// crate's own integration tests can exercise the internal `Random` and
/// `RefResolver` types without forcing every test to be rewritten as a
/// `#[cfg(test)]` unit test inside `src/`.
#[doc(hidden)]
pub mod __test_internals {
    pub use crate::random::{random_string_seed, Random};
    pub use crate::ref_resolver::RefResolver;
    pub use crate::schema_walker::schemas_contradict;
}
use serde_json::Value;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use thiserror::Error;

/// Errors that can occur during fake data generation.
#[derive(Error, Debug)]
#[non_exhaustive]
pub enum FakerError {
    /// The schema specifies an unrecognised or invalid `type` value.
    #[error("unknown primitive {value} in {path}")]
    InvalidType {
        /// The invalid type string encountered.
        value: String,
        /// The JSON pointer path where the bad type was found.
        path: String,
    },
    /// A general schema processing error occurred.
    #[error("schema error at {path}: {message}")]
    SchemaError {
        /// The JSON pointer path where the error occurred.
        path: String,
        /// Human-readable description of the schema error.
        message: String,
    },
    /// The schema requires more properties than can be satisfied.
    #[error("missing properties for {target} in {path}")]
    MissingProperties {
        /// Human-readable description of what was missing.
        target: String,
        /// The JSON pointer path where the error occurred.
        path: String,
    },
    /// A required `items` schema could not produce any value.
    #[error("missing items for {target} in {path}")]
    MissingItems {
        /// Human-readable description of what was missing.
        target: String,
        /// The JSON pointer path where the error occurred.
        path: String,
    },
    /// A `properties` set was constrained to be present but `additionalProperties: false`
    /// would have to permit them.
    #[error("properties '{name}' were not found while additionalProperties is false")]
    AdditionalPropertiesBlocked {
        /// The property name that could not be created.
        name: String,
    },
    /// Both `chance` and `faker` keywords were used on the same schema node.
    #[error("ambiguous generator")]
    AmbiguousGenerator,
    /// A faker/chance/registry key was referenced that no extension implements.
    #[error("Error: unknown registry key {name} in {path}")]
    UnknownRegistryKey {
        /// The keyword that failed to resolve.
        name: String,
        /// The JSON pointer path where the keyword was found.
        path: String,
    },
    /// A `faker` or `chance` reference could not be resolved.
    #[error("cannot resolve faker-generator for {name} in {path}")]
    UnknownFakerGenerator {
        /// The faker/chance keyword that failed to resolve.
        name: String,
        /// The JSON pointer path where the keyword was found.
        path: String,
    },
    /// A `$ref` could not be resolved.
    #[error("unresolved $ref: {path}")]
    UnresolvedRef {
        /// The unresolved reference string.
        path: String,
    },
    /// A schema literal of `false` rejects every value, so no value can be generated.
    #[error("cannot generate a value for a `false` schema")]
    CannotGenerateForFalseSchema,
    /// Every branch of an `anyOf` or `oneOf` composition failed to produce a
    /// value, so the walker could not satisfy the keyword.
    ///
    /// Carries the keyword (`"anyOf"` or `"oneOf"`), the number of branches the
    /// walker attempted before giving up, and the most recently observed
    /// per-branch error when available.
    #[error("all {branch_count} branches of {keyword} failed to generate")]
    AllBranchesFailed {
        /// The composition keyword whose branches all failed (`"anyOf"` or `"oneOf"`).
        keyword: &'static str,
        /// Number of sub-schemas the walker attempted.
        branch_count: usize,
        /// Most recent per-branch error captured during the attempt loop, if any.
        #[source]
        last_error: Option<Box<FakerError>>,
    },
}

/// Generates a fake JSON value conforming to the given JSON Schema.
///
/// Creates an internal `Random` instance seeded from `opts.seed`, then walks the
/// schema to produce a value that satisfies all applicable schema constraints.
/// Returns `Err` when the walk records a generator error (for example an unknown
/// `type` with `failOnInvalidTypes` enabled).
pub fn generate(schema: &Value, opts: &GenerateOptions) -> Result<Value, FakerError> {
    if schema == &Value::Bool(false) {
        return Err(FakerError::CannotGenerateForFalseSchema);
    }
    if opts.validate_schema_version {
        if let Some(dialect) = schema.get("$schema").and_then(|v| v.as_str()) {
            if !is_supported_schema_dialect(dialect) {
                return Err(FakerError::SchemaError {
                    path: "/".to_string(),
                    message: format!("unsupported $schema dialect: {}", dialect),
                });
            }
        }
    }
    if let Some(unresolved) = first_unresolvable_ref(schema, schema, opts) {
        return Err(FakerError::UnresolvedRef { path: unresolved });
    }
    let mut rng = Random::new(opts.seed);
    rng.set_fixed_probabilities(opts.fixed_probabilities);
    // Snapshot the process-global extension and format registries at entry. The
    // walker reads from these snapshots instead of locking the global on every
    // lookup; this means concurrent `define()` / `register_format()` calls from
    // another thread cannot mutate the set of registered closures mid-generation,
    // which would otherwise break same-seed determinism. We clone-into-Arc so the
    // snapshot can be cheaply propagated through the walker via `&GenerateOptions`.
    let extension_snapshot = snapshot_extension_registry();
    let format_snapshot = snapshot_format_registry();
    let opts_with_snapshots = inject_snapshots(opts, extension_snapshot, format_snapshot);
    let result = schema_walker::walk_schema(schema, schema, &opts_with_snapshots, &mut rng, 0);
    if let Some(err) = rng.take_error() {
        return Err(err);
    }
    let result = strip_skip_sentinels(result);
    let result = apply_output_transform(result, schema, opts);
    Ok(result)
}

/// Applies the caller-supplied `output_transform` closure, if any, returning the
/// transformed value or the original when no transform is configured.
///
/// The closure receives `(&generated_value, &root_schema)` per the
/// [`crate::options::OutputTransform`] signature; this is the single place that
/// invokes it so both [`generate`] and [`generate_static`] honour the field.
fn apply_output_transform(result: Value, schema: &Value, opts: &GenerateOptions) -> Value {
    if let Some(transform) = opts.output_transform.as_ref() {
        return transform(&result, schema);
    }
    result
}

/// Clones `opts` and attaches the given per-call registry snapshots, returning a new
/// `GenerateOptions` suitable for passing to the schema walker.
///
/// Snapshots are only injected when the caller has not pre-populated them. This lets
/// downstream callers (for example, recursive `generate()` invocations from inside an
/// extension callback) re-use a parent snapshot if they choose to.
fn inject_snapshots(
    opts: &GenerateOptions,
    extension_snapshot: Option<Arc<extensions::ExtensionRegistry>>,
    format_snapshot: Option<Arc<extensions::FormatRegistry>>,
) -> GenerateOptions {
    let mut next = opts.clone();
    if next.extension_snapshot.is_none() {
        next.extension_snapshot = extension_snapshot;
    }
    if next.format_snapshot.is_none() {
        next.format_snapshot = format_snapshot;
    }
    next
}

/// Returns an `Arc` snapshot of the current global extension registry, or `None` if
/// no extensions have been registered.
///
/// The walker consults this snapshot instead of the global so a concurrent
/// `define()` from another thread cannot change which closure runs partway through
/// a `generate()` call.
fn snapshot_extension_registry() -> Option<Arc<extensions::ExtensionRegistry>> {
    let guard = recover_lock(GLOBAL_REGISTRY.lock());
    guard.as_ref().map(|r| Arc::new(r.clone()))
}

/// Returns an `Arc` snapshot of the current global format registry, or `None` if
/// no formats have been registered. See [`snapshot_extension_registry`] for rationale.
fn snapshot_format_registry() -> Option<Arc<extensions::FormatRegistry>> {
    let guard = recover_lock(GLOBAL_FORMAT_REGISTRY.lock());
    guard.as_ref().map(|r| Arc::new(r.clone()))
}

/// Recovers the inner value of a [`Mutex`] lock result even when the mutex was
/// poisoned by a panic in another thread.
///
/// User-supplied extension callbacks run inside [`invoke_extension`] while the
/// global registry lock is held (transitively, via the snapshot path). A panic in
/// such a callback poisons the mutex; without this helper the next call to
/// `define`/`register_format`/`reset_*` would itself panic with "global registry
/// poisoned". Recovering the inner value lets subsequent calls continue to work,
/// which matches user expectations: a panicking custom extension should not
/// permanently brick further configuration of the faker.
///
/// The recovered value is the registry as it was at the moment of the panic. That
/// is the same state every subsequent caller would observe, so no extra
/// reconciliation is required.
fn recover_lock<'a, T>(
    result: Result<MutexGuard<'a, T>, PoisonError<MutexGuard<'a, T>>>,
) -> MutexGuard<'a, T> {
    result.unwrap_or_else(|e| e.into_inner())
}

/// Generates a deterministic, faker-free value from `schema`.
///
/// Produces type-default scalars for primitive types and empty composites,
/// recursing only enough to satisfy `required` properties. Used by chasm-engine
/// for static mode response generation. Output is byte-identical across runs
/// for the same input.
pub fn generate_static(schema: &Value, opts: &GenerateOptions) -> Result<Value, FakerError> {
    let mut visited: Vec<String> = Vec::new();
    let result = walk_static(schema, schema, opts, &mut visited, 0);
    Ok(apply_output_transform(result, schema, opts))
}

/// Internal static walker that produces deterministic minimal values.
///
/// Resolves `$ref` against `root`, honours `const`/`enum`/`examples`/`default` selection,
/// merges `allOf` sub-schemas, picks the first branch of `anyOf`/`oneOf`, and emits
/// type-default scalars otherwise. Recurses only enough to satisfy `required` keys
/// of objects and `minItems` of arrays.
fn walk_static(
    schema: &Value,
    root: &Value,
    opts: &GenerateOptions,
    visited: &mut Vec<String>,
    depth: usize,
) -> Value {
    if depth >= opts.max_depth {
        return static_terminal(schema);
    }

    let map = match schema.as_object() {
        Some(m) => m,
        None => return Value::Null,
    };

    if let Some(Value::String(ref_str)) = map.get("$ref") {
        if visited.iter().any(|r| r == ref_str) {
            return static_terminal(schema);
        }
        let resolver = ref_resolver::RefResolver::new(root).with_external(&opts.external_refs);
        let mut local_visited = Vec::new();
        if let Some(resolved) = resolver.resolve(ref_str.as_str(), &mut local_visited) {
            let resolved_owned = resolved.clone();
            visited.push(ref_str.clone());
            let result = walk_static(&resolved_owned, root, opts, visited, depth + 1);
            visited.pop();
            return result;
        }
        return Value::Null;
    }

    if let Some(c) = map.get("const") {
        return c.clone();
    }

    if let Some(Value::Array(arr)) = map.get("enum") {
        if let Some(first) = arr.first() {
            return first.clone();
        }
    }

    if opts.use_examples_value {
        if let Some(Value::Array(arr)) = map.get("examples") {
            if let Some(first) = arr.first() {
                return first.clone();
            }
        }
        if let Some(ex) = map.get("example") {
            return ex.clone();
        }
    }

    if let Some(Value::Array(arr)) = map.get("allOf") {
        let mut merged: Value = Value::Object(serde_json::Map::new());
        for sub in arr {
            let resolved = if let Some(Value::String(ref_str)) = sub.get("$ref") {
                let resolver =
                    ref_resolver::RefResolver::new(root).with_external(&opts.external_refs);
                let mut local_visited = Vec::new();
                resolver
                    .resolve(ref_str.as_str(), &mut local_visited)
                    .cloned()
                    .unwrap_or_else(|| sub.clone())
            } else {
                sub.clone()
            };
            merged = merge::deep_merge(merged, resolved);
        }
        if let Value::Object(ref mut m) = merged {
            for (k, v) in map {
                if k != "allOf" && !m.contains_key(k) {
                    m.insert(k.clone(), v.clone());
                }
            }
            m.remove("allOf");
        }
        return walk_static(&merged, root, opts, visited, depth + 1);
    }

    for kw in &["anyOf", "oneOf"] {
        if let Some(Value::Array(arr)) = map.get(*kw) {
            if let Some(first) = arr.first() {
                return walk_static(first, root, opts, visited, depth + 1);
            }
        }
    }

    if opts.use_default_value {
        if let Some(default_val) = map.get("default") {
            return default_val.clone();
        }
    }

    let type_str = match map.get("type") {
        Some(Value::String(t)) => t.as_str(),
        Some(Value::Array(arr)) => arr.iter().find_map(|v| v.as_str()).unwrap_or(""),
        _ => infer_static_type(map),
    };

    match type_str {
        "string" => Value::String(String::new()),
        "number" => Value::Number(serde_json::Number::from(0)),
        "integer" => Value::Number(serde_json::Number::from(0)),
        "boolean" => Value::Bool(false),
        "null" => Value::Null,
        "array" => static_array(schema, root, opts, visited, depth),
        "object" => static_object(schema, root, opts, visited, depth),
        _ => Value::Null,
    }
}

/// Infers a JSON Schema type from object/array/string-shaped keywords for static mode.
fn infer_static_type(map: &serde_json::Map<String, Value>) -> &'static str {
    if map.contains_key("properties") || map.contains_key("required") {
        return "object";
    }
    if map.contains_key("items") || map.contains_key("prefixItems") || map.contains_key("minItems")
    {
        return "array";
    }
    if map.contains_key("minLength") || map.contains_key("maxLength") || map.contains_key("pattern")
    {
        return "string";
    }
    if map.contains_key("minimum") || map.contains_key("maximum") {
        return "number";
    }
    ""
}

/// Builds a static array satisfying the schema's `minItems` constraint.
///
/// Walks each prefix item once and pads up to `minItems` using the `items` schema.
fn static_array(
    schema: &Value,
    root: &Value,
    opts: &GenerateOptions,
    visited: &mut Vec<String>,
    depth: usize,
) -> Value {
    let map = match schema.as_object() {
        Some(m) => m,
        None => return Value::Array(Vec::new()),
    };
    let min_items = map.get("minItems").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
    let mut out: Vec<Value> = Vec::new();
    if let Some(Value::Array(prefix)) = map.get("prefixItems") {
        for item_schema in prefix {
            out.push(walk_static(item_schema, root, opts, visited, depth + 1));
        }
    } else if let Some(Value::Array(prefix)) = map.get("items") {
        for item_schema in prefix {
            out.push(walk_static(item_schema, root, opts, visited, depth + 1));
        }
    }
    let item_schema_opt = map.get("items").filter(|v| v.is_object());
    let empty = Value::Object(serde_json::Map::new());
    let fill_schema = item_schema_opt.unwrap_or(&empty);
    while out.len() < min_items {
        out.push(walk_static(fill_schema, root, opts, visited, depth + 1));
    }
    Value::Array(out)
}

/// Builds a static object with all `required` properties populated.
fn static_object(
    schema: &Value,
    root: &Value,
    opts: &GenerateOptions,
    visited: &mut Vec<String>,
    depth: usize,
) -> Value {
    let map = match schema.as_object() {
        Some(m) => m,
        None => return Value::Object(serde_json::Map::new()),
    };
    let required: Vec<String> = map
        .get("required")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();
    let empty_props = serde_json::Map::new();
    let properties = map
        .get("properties")
        .and_then(|v| v.as_object())
        .unwrap_or(&empty_props);
    let mut out = serde_json::Map::new();
    for key in &required {
        let prop_schema = properties
            .get(key)
            .cloned()
            .unwrap_or(Value::Object(serde_json::Map::new()));
        out.insert(
            key.clone(),
            walk_static(&prop_schema, root, opts, visited, depth + 1),
        );
    }
    Value::Object(out)
}

/// Returns the depth-cutoff terminal value for a schema in static mode.
fn static_terminal(schema: &Value) -> Value {
    if let Some(Value::String(t)) = schema.get("type") {
        match t.as_str() {
            "string" => return Value::String(String::new()),
            "number" | "integer" => return Value::Number(serde_json::Number::from(0)),
            "boolean" => return Value::Bool(false),
            "array" => return Value::Array(Vec::new()),
            "object" => return Value::Object(serde_json::Map::new()),
            "null" => return Value::Null,
            _ => {}
        }
    }
    Value::Null
}

/// Recursively removes the depth-cutoff omit sentinel produced by the walker.
///
/// When the walker hits the depth limit on a recursive composition for an optional
/// property, the object generator already drops the property. This pass guards
/// against sentinels surfacing at the top level (when generation is invoked directly
/// against a recursive root schema) by replacing them with an empty object.
fn strip_skip_sentinels(value: Value) -> Value {
    if schema_walker::is_skip_sentinel(&value) {
        return Value::Object(serde_json::Map::new());
    }
    match value {
        Value::Object(map) => Value::Object(
            map.into_iter()
                .map(|(k, v)| (k, strip_skip_sentinels(v)))
                .collect(),
        ),
        Value::Array(arr) => Value::Array(arr.into_iter().map(strip_skip_sentinels).collect()),
        other => other,
    }
}

/// Walks a schema and returns the first `$ref` string that the resolver cannot resolve.
///
/// Only references that are not local (`#...`) are checked, so the upstream behaviour of
/// erroring eagerly on an unresolvable remote/file reference is preserved even when the
/// reference sits on an optional property that generation might otherwise skip.
fn first_unresolvable_ref(schema: &Value, root: &Value, opts: &GenerateOptions) -> Option<String> {
    let resolver = ref_resolver::RefResolver::new(root).with_external(&opts.external_refs);
    let mut found: Option<String> = None;
    walk_refs(schema, &mut |r| {
        if found.is_some() {
            return;
        }
        if r.starts_with('#') {
            return;
        }
        let mut visited = Vec::new();
        if resolver.resolve(r, &mut visited).is_none() {
            found = Some(r.to_string());
        }
    });
    found
}

/// Returns true when `dialect` matches a JSON Schema draft this crate understands.
///
/// The faker library targets the modern JSON Schema drafts: draft-07, 2019-09, and
/// 2020-12, plus the older draft-04/draft-06 metaschemas which share the same core
/// keyword set. Unknown drafts (for example a `draft-99/schema`) are rejected when
/// the caller opts into [`GenerateOptions::validate_schema_version`].
fn is_supported_schema_dialect(dialect: &str) -> bool {
    const SUPPORTED: &[&str] = &[
        "http://json-schema.org/draft-04/schema#",
        "https://json-schema.org/draft-04/schema#",
        "http://json-schema.org/draft-06/schema#",
        "https://json-schema.org/draft-06/schema#",
        "http://json-schema.org/draft-07/schema#",
        "https://json-schema.org/draft-07/schema#",
        "https://json-schema.org/draft/2019-09/schema",
        "http://json-schema.org/draft/2019-09/schema",
        "https://json-schema.org/draft/2020-12/schema",
        "http://json-schema.org/draft/2020-12/schema",
    ];
    let trimmed = dialect.trim_end_matches('#');
    SUPPORTED
        .iter()
        .any(|d| *d == dialect || d.trim_end_matches('#') == trimmed)
}

/// Recursively invokes `cb` for every `$ref` string found anywhere within `schema`.
fn walk_refs(schema: &Value, cb: &mut dyn FnMut(&str)) {
    match schema {
        Value::Object(map) => {
            if let Some(Value::String(r)) = map.get("$ref") {
                cb(r.as_str());
            }
            for (_, v) in map {
                walk_refs(v, cb);
            }
        }
        Value::Array(arr) => {
            for v in arr {
                walk_refs(v, cb);
            }
        }
        _ => {}
    }
}

/// Global registry used by the `define()` convenience function.
static GLOBAL_REGISTRY: Mutex<Option<extensions::ExtensionRegistry>> = Mutex::new(None);

/// Global registry used by [`register_format`] / [`invoke_format`].
static GLOBAL_FORMAT_REGISTRY: Mutex<Option<extensions::FormatRegistry>> = Mutex::new(None);

/// Registers a custom keyword generator that the walker invokes when a schema contains
/// the matching keyword.
///
/// This mirrors the upstream `extend()` / `define()` API in json-schema-faker. The
/// callback receives the local schema node, the root schema, the optional property name
/// (when invoked from inside an object generator; `None` at the schema root), and the
/// JSON Pointer schema path. It returns the value substituted in place of the schema.
///
/// # Thread-safety
///
/// The underlying registry is **process-global**. Access is serialised behind a
/// [`std::sync::Mutex`], so concurrent calls do not corrupt state. To preserve
/// same-seed determinism, [`generate`] takes an `Arc` snapshot of the registry at
/// entry and reads from that snapshot throughout the walk: a concurrent
/// `define()` from another thread therefore cannot swap out an extension partway
/// through a single generation, but it *will* affect the next [`generate`] call.
///
/// Tests that mutate the registry can still observe one another's registrations
/// when run in parallel because the global itself is shared across [`generate`]
/// calls. When writing tests that register or assert on extensions, prefer serial
/// execution (`cargo test -- --test-threads=1`) to avoid cross-test interference.
///
/// If an extension callback panics, the mutex is poisoned. Subsequent calls
/// recover the inner registry via [`std::sync::PoisonError::into_inner`] rather
/// than panicking, so a misbehaving extension does not permanently brick the
/// global registry.
pub fn define(name: &str, f: extensions::ExtensionFn) {
    let mut guard = recover_lock(GLOBAL_REGISTRY.lock());
    if guard.is_none() {
        *guard = Some(extensions::ExtensionRegistry::new());
    }
    if let Some(reg) = guard.as_mut() {
        reg.define(name, f);
    }
}

/// Clears all globally-registered keyword generators.
pub fn reset_extensions() {
    let mut guard = recover_lock(GLOBAL_REGISTRY.lock());
    if let Some(reg) = guard.as_mut() {
        reg.reset();
    }
}

/// Returns the value produced by a globally-registered extension for the given keyword.
///
/// `prop` is the property name when the extension is invoked from inside an object
/// generator (`None` at the schema root). `path` is the JSON Pointer schema path of the
/// current node; callers that do not yet thread a path may pass `""`.
///
/// Returns `None` when no extension is registered or when the schema does not contain
/// the keyword.
pub fn invoke_extension(
    keyword: &str,
    schema: &Value,
    root: &Value,
    prop: Option<&str>,
    path: &str,
) -> Option<Value> {
    let handler = {
        let guard = recover_lock(GLOBAL_REGISTRY.lock());
        guard.as_ref().and_then(|reg| reg.get(keyword).cloned())
    };
    handler.map(|f| f(schema, root, prop, path))
}

/// Looks up `keyword` in the per-call extension snapshot attached to `opts` and
/// invokes its handler if registered.
///
/// This is the walker's primary entry point for extension dispatch: it consults the
/// snapshot taken at [`generate`] entry rather than re-locking the global
/// registry, so a concurrent [`define`] from another thread cannot change which
/// closure runs partway through generation. When `opts` carries no snapshot — for
/// example because the caller bypassed [`generate`] and walked the schema
/// directly — this falls back to [`invoke_extension`] on the global.
pub fn invoke_extension_from_opts(
    opts: &GenerateOptions,
    keyword: &str,
    schema: &Value,
    root: &Value,
    prop: Option<&str>,
    path: &str,
) -> Option<Value> {
    if let Some(snapshot) = opts.extension_snapshot.as_ref() {
        let f = snapshot.get(keyword)?;
        return Some(f(schema, root, prop, path));
    }
    invoke_extension(keyword, schema, root, prop, path)
}

/// Registers a custom string format generator.
///
/// Once registered, callers can resolve the format via [`invoke_format`]. The format
/// dispatcher in `formats/mod.rs` is expected to consult this registry **before**
/// falling through to its built-in format generators, allowing users to override or
/// extend the built-in set.
///
/// # Thread-safety
///
/// The underlying registry is **process-global**. Access is serialised behind a
/// [`std::sync::Mutex`], so concurrent calls do not corrupt state. [`generate`]
/// takes an `Arc` snapshot of the registry at entry and exposes it on
/// [`GenerateOptions::format_snapshot`] for downstream consumers; the snapshot
/// path is in place even though the built-in format dispatcher in `formats/mod.rs`
/// still consults the global directly. Tests that mutate the registry can
/// observe one another's registrations when run in parallel: prefer serial
/// execution (`cargo test -- --test-threads=1`) to avoid cross-test interference.
///
/// If a format generator panics, the mutex is poisoned. Subsequent calls recover
/// the inner registry via [`std::sync::PoisonError::into_inner`] rather than
/// panicking, so a misbehaving generator does not permanently brick the global
/// registry.
pub fn register_format(name: &str, generator: extensions::FormatFn) {
    let mut guard = recover_lock(GLOBAL_FORMAT_REGISTRY.lock());
    if guard.is_none() {
        *guard = Some(extensions::FormatRegistry::new());
    }
    if let Some(reg) = guard.as_mut() {
        reg.define(name, generator);
    }
}

/// Clears all globally-registered format generators.
pub fn reset_formats() {
    let mut guard = recover_lock(GLOBAL_FORMAT_REGISTRY.lock());
    if let Some(reg) = guard.as_mut() {
        reg.reset();
    }
}

/// Returns the value produced by a globally-registered format generator.
///
/// The caller — currently only built-in format dispatch in `formats/mod.rs`, but
/// eventually any custom format consumer — **should consult this registry before
/// falling through to built-in format generators**. This ordering lets user-registered
/// formats override or extend the built-ins.
///
/// Returns `None` when no generator is registered for `name`.
pub fn invoke_format(name: &str, rng: &mut Random) -> Option<Value> {
    let handler = {
        let guard = recover_lock(GLOBAL_FORMAT_REGISTRY.lock());
        guard.as_ref().and_then(|reg| reg.get(name).cloned())
    };
    handler.map(|f| f(rng))
}
