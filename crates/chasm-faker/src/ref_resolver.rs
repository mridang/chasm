//! `$ref` resolver used by the schema walker.
//!
//! Owns the cycle-aware [`RefResolver`] type that walks JSON-pointer
//! references (`#/components/schemas/Foo`) inside the root document and
//! optionally falls back to a caller-supplied external-refs map for
//! resolved-remote lookups. Cycles short-circuit to `None` to keep
//! generation finite.

use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;

/// Resolves `$ref` strings within a JSON Schema document.
///
/// Handles local JSON pointer references (`#/...`), `$defs`, `definitions`,
/// and nested `$id`-based references. Tracks visited pointers to detect cycles.
///
/// Future work: caching the `$id` index across `RefResolver::new` calls is
/// tempting because `collect_ids` is a full recursive walk that is repeated
/// 5-6 times per `generate()` invocation against the same root JSON tree.
/// A `thread_local!` single-slot cache keyed on `root as *const Value` was
/// tried and broke fixtures: once the prior root `Value` is dropped, the
/// allocator can hand the same address back to an unrelated subsequent
/// root, and the stale `id_map` would resolve `$ref`s to the wrong target
/// (observed against `urn:`-style absolute refs in
/// `tests/fixtures/schema-faker-tests/core/issues/issue-610.json`). Safe
/// routes to recover this win without correctness risk are to have the
/// top-level entry point build the map once and thread it down via
/// `new_with_id_map`, or to key the cache on `(pointer, content
/// fingerprint)` where the fingerprint is cheap enough to be worth it.
pub struct RefResolver<'a> {
    root: &'a Value,
    id_map: Arc<HashMap<String, Vec<String>>>,
    external: Option<&'a HashMap<String, Value>>,
    min_depth: usize,
    max_depth: usize,
}

impl<'a> RefResolver<'a> {
    /// Builds a new resolver from the root schema, pre-computing the `$id` index.
    pub fn new(root: &'a Value) -> Self {
        let mut id_map: HashMap<String, Vec<String>> = HashMap::new();
        collect_ids(root, &[], &mut id_map, None);
        Self {
            root,
            id_map: Arc::new(id_map),
            external: None,
            min_depth: 0,
            max_depth: usize::MAX,
        }
    }

    /// Builds a resolver using a pre-built `$id` index shared via `Arc`.
    ///
    /// Useful when the caller has already computed the index for `root` (or
    /// constructed an empty one for trivially flat schemas) and wants to
    /// skip the recursive `collect_ids` walk. This is the recommended way to
    /// avoid repeating the walk across multiple `RefResolver` constructions
    /// against the same root during one generation pass.
    pub fn new_with_id_map(root: &'a Value, id_map: Arc<HashMap<String, Vec<String>>>) -> Self {
        Self {
            root,
            id_map,
            external: None,
            min_depth: 0,
            max_depth: usize::MAX,
        }
    }

    /// Returns a builder-style resolver that consults an external `$ref` substitution map.
    ///
    /// When `resolve()` encounters a non-`#` reference, it first checks `external` before
    /// falling back to the in-document `$id` index.
    pub fn with_external(mut self, external: &'a HashMap<String, Value>) -> Self {
        if !external.is_empty() {
            self.external = Some(external);
        }
        self
    }

    /// Sets the inclusive recursion-depth window for `$ref` chains.
    ///
    /// Visited paths shorter than `min` are still followed; paths longer than `max`
    /// abort with `None`, which the caller turns into a terminal value.
    pub fn with_depth_window(mut self, min: usize, max: usize) -> Self {
        self.min_depth = min;
        self.max_depth = max.max(min);
        self
    }

    /// Resolves a `$ref` string to its target schema node, or `None` if unresolvable.
    ///
    /// Supports `#/path/to/node` JSON pointer format, bare `#` (root reference),
    /// and `#<name>` anchor references (resolved via the `$anchor` index).
    /// Returns `None` on cycles or missing paths.
    pub fn resolve(&self, ref_str: &str, visited: &mut Vec<String>) -> Option<&'a Value> {
        if visited.len() >= self.max_depth {
            return None;
        }
        if visited.contains(&ref_str.to_string()) {
            return None;
        }
        visited.push(ref_str.to_string());

        if ref_str == "#" {
            return Some(self.root);
        }

        if let Some(rest) = ref_str.strip_prefix("#/") {
            return self.resolve_pointer(rest);
        }

        if let Some(anchor) = ref_str.strip_prefix('#') {
            if !anchor.is_empty() {
                if let Some(path) = self.id_map.get(anchor) {
                    return self.resolve_path(path);
                }
                if let Some(path) = self.id_map.get(ref_str) {
                    return self.resolve_path(path);
                }
            }
            return Some(self.root);
        }

        if let Some(map) = self.external {
            if let Some(v) = map.get(ref_str) {
                return Some(v);
            }
        }

        if let Some(path) = self.id_map.get(ref_str) {
            return self.resolve_path(path);
        }

        None
    }

    /// Returns the configured minimum recursion depth (informational).
    pub fn min_depth(&self) -> usize {
        self.min_depth
    }

    /// Navigates the root schema along a `/`-delimited pointer string.
    fn resolve_pointer(&self, pointer: &str) -> Option<&'a Value> {
        let parts: Vec<&str> = pointer.split('/').collect();
        self.resolve_parts(&parts, self.root)
    }

    /// Navigates the root schema along a pre-split sequence of path segments.
    fn resolve_parts(&self, parts: &[&str], current: &'a Value) -> Option<&'a Value> {
        if parts.is_empty() {
            return Some(current);
        }
        let key = parts[0].replace("~1", "/").replace("~0", "~");
        match current {
            Value::Object(map) => map
                .get(&key)
                .and_then(|v| self.resolve_parts(&parts[1..], v)),
            Value::Array(arr) => key
                .parse::<usize>()
                .ok()
                .and_then(|i| arr.get(i))
                .and_then(|v| self.resolve_parts(&parts[1..], v)),
            _ => None,
        }
    }

    /// Resolves a path stored in the id_map (a sequence of owned string segments).
    fn resolve_path(&self, path: &[String]) -> Option<&'a Value> {
        let str_parts: Vec<&str> = path.iter().map(|s| s.as_str()).collect();
        self.resolve_parts(&str_parts, self.root)
    }
}

/// Recursively walks the schema tree collecting `$id`, `id`, and `$anchor` values
/// and their paths.
///
/// Older drafts used the unprefixed `id` keyword; both are indexed so a `$ref` can target
/// either spelling. `$anchor` values are stored under their bare name (without the
/// `#` prefix) so a `#name` reference can resolve to the anchored subschema.
///
/// Honours JSON Schema `$id` re-rooting: when a nested schema declares a relative
/// `$id`, it is resolved against the nearest enclosing `$id` and indexed under
/// the resolved absolute form (in addition to its literal form).
fn collect_ids(
    value: &Value,
    path: &[&str],
    id_map: &mut HashMap<String, Vec<String>>,
    base: Option<&str>,
) {
    if let Value::Object(map) = value {
        let mut new_base: Option<String> = base.map(|s| s.to_string());
        if let Some(Value::String(id)) = map.get("$id") {
            let resolved = resolve_against_base(base, id);
            id_map.insert(id.clone(), path.iter().map(|s| s.to_string()).collect());
            if resolved != *id {
                id_map.insert(
                    resolved.clone(),
                    path.iter().map(|s| s.to_string()).collect(),
                );
            }
            new_base = Some(resolved);
        }
        if let Some(Value::String(id)) = map.get("id") {
            let resolved = resolve_against_base(base, id);
            id_map.insert(id.clone(), path.iter().map(|s| s.to_string()).collect());
            if resolved != *id {
                id_map.insert(
                    resolved.clone(),
                    path.iter().map(|s| s.to_string()).collect(),
                );
            }
            if new_base.is_none() {
                new_base = Some(resolved);
            }
        }
        if let Some(Value::String(anchor)) = map.get("$anchor") {
            id_map.insert(anchor.clone(), path.iter().map(|s| s.to_string()).collect());
        }
        for (key, child) in map {
            let mut new_path = path.to_vec();
            new_path.push(key.as_str());
            let owned_path_strs: Vec<String> = new_path.iter().map(|s| s.to_string()).collect();
            let strs_ref: Vec<&str> = owned_path_strs.iter().map(|s| s.as_str()).collect();
            collect_ids(child, &strs_ref, id_map, new_base.as_deref());
        }
    }
}

/// Resolves a (possibly relative) `$id` against an enclosing base URI.
///
/// Handles the common chasm cases without pulling in a full RFC 3986 implementation:
///   - absolute URLs (`scheme://...`) replace the base entirely
///   - fragment-only IDs (`#anchor`) are appended to the base
///   - bare relative paths (`foo-sub`, `./foo`) are joined onto the base directory
///   - when no base is set, returns the input unchanged
fn resolve_against_base(base: Option<&str>, id: &str) -> String {
    let base = match base {
        Some(b) => b,
        None => return id.to_string(),
    };
    if id.contains("://") {
        return id.to_string();
    }
    if let Some(stripped) = id.strip_prefix('#') {
        if let Some(hash_pos) = base.find('#') {
            let mut out = String::from(&base[..hash_pos]);
            out.push('#');
            out.push_str(stripped);
            return out;
        }
        let mut out = String::from(base);
        out.push('#');
        out.push_str(stripped);
        return out;
    }
    let base_no_frag = match base.find('#') {
        Some(p) => &base[..p],
        None => base,
    };
    let dir = match base_no_frag.rfind('/') {
        Some(p) => &base_no_frag[..=p],
        None => "",
    };
    let trimmed = id.strip_prefix("./").unwrap_or(id);
    format!("{}{}", dir, trimmed)
}
