//! Spec loading + OAS 3.1 normalisation.
//!
//! Reads JSON/YAML from a path or stdin, runs normalisation passes
//! (`nullable`, `exclusiveMinimum/Maximum`, `items: false` after
//! `prefixItems`, path-item/callback `$ref` inlining, Swagger 2.0
//! rejection), then runs post-parse semantic validation before handing
//! a parsed `OpenAPI` to the rest of the engine.

use crate::SpecError;
use openapiv3::OpenAPI;
#[cfg(not(target_arch = "wasm32"))]
use serde_json::Value;
#[cfg(not(target_arch = "wasm32"))]
use std::collections::HashMap;
use std::fs;
#[cfg(not(target_arch = "wasm32"))]
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, ToSocketAddrs};
#[cfg(not(target_arch = "wasm32"))]
use std::time::Duration;
#[cfg(not(target_arch = "wasm32"))]
use std::time::Instant;

/// Per-fetch network timeout for remote `$ref` resolution.
#[cfg(not(target_arch = "wasm32"))]
const REMOTE_REF_FETCH_TIMEOUT: Duration = Duration::from_secs(5);

/// Total wall-clock budget for all remote `$ref` fetches per `load_spec_with_remote_refs` call.
#[cfg(not(target_arch = "wasm32"))]
const REMOTE_REF_WALL_TIMEOUT: Duration = Duration::from_secs(30);

/// Maximum response body size accepted from any single remote `$ref` fetch.
#[cfg(not(target_arch = "wasm32"))]
const REMOTE_REF_MAX_BODY_BYTES: usize = 1024 * 1024;

/// Maximum number of distinct remote URLs fetched per `load_spec_with_remote_refs` call.
#[cfg(not(target_arch = "wasm32"))]
const REMOTE_REF_MAX_FETCHES: usize = 64;

/// Maximum raw byte length of an OpenAPI spec accepted by the loader, whether
/// read from disk or supplied inline. Specs larger than this are rejected
/// with a `SpecError::Parse` before any parser is invoked, so a 1 GiB malformed
/// file cannot exhaust process memory before the YAML/JSON layer has a chance
/// to bail. 64 MiB comfortably covers real-world specs (the largest published
/// AWS service specs sit in the low single-digit MiB range).
const MAX_SPEC_BYTES: usize = 64 * 1024 * 1024;

/// Loads and parses an OAS3 specification from a file path or a raw JSON/YAML string.
///
/// When `src` begins with `{` it is treated as a raw JSON string; otherwise it is
/// read from the filesystem. The format (JSON vs YAML) is detected from the
/// leading character of the content after loading.
///
/// Before deserialising into `openapiv3::OpenAPI`, the raw spec is passed through
/// [`normalize_oas31_nullable`] which rewrites every OAS 3.1 `type: ["X", "null"]`
/// type-array idiom into the OAS 3.0 `type: "X", nullable: true` form that
/// `openapiv3` v2 and `chasm-faker` understand natively. Arbitrary multi-type
/// arrays without `"null"` are left intact so downstream multi-type dispatch keeps
/// working.
pub fn load_spec(src: &str) -> Result<OpenAPI, SpecError> {
    let content = if src.trim_start().starts_with('{') || src.trim_start().starts_with("openapi") {
        if src.len() > MAX_SPEC_BYTES {
            return Err(SpecError::Parse(format!(
                "spec size {} bytes exceeds MAX_SPEC_BYTES={}",
                src.len(),
                MAX_SPEC_BYTES
            )));
        }
        src.to_string()
    } else {
        if let Ok(meta) = fs::metadata(src) {
            if meta.len() as usize > MAX_SPEC_BYTES {
                return Err(SpecError::Parse(format!(
                    "spec file size {} bytes exceeds MAX_SPEC_BYTES={}",
                    meta.len(),
                    MAX_SPEC_BYTES
                )));
            }
        }
        let raw = fs::read_to_string(src).map_err(|source| SpecError::Io {
            path: src.to_string(),
            source,
        })?;
        if raw.len() > MAX_SPEC_BYTES {
            return Err(SpecError::Parse(format!(
                "spec size {} bytes exceeds MAX_SPEC_BYTES={}",
                raw.len(),
                MAX_SPEC_BYTES
            )));
        }
        raw
    };

    reject_unsupported_openapi_version(&content)?;
    reject_swagger_two(&content)?;
    let normalised = normalize_oas31_nullable(&content)?;
    let spec: OpenAPI =
        serde_json::from_str(&normalised).map_err(|e| SpecError::Parse(e.to_string()))?;
    let normalised_value: serde_yaml::Value =
        serde_yaml::from_str(&normalised).map_err(|e| SpecError::Parse(e.to_string()))?;
    validate_spec_semantics(&spec, &normalised_value)?;
    Ok(spec)
}

/// Rejects specs whose top-level `openapi` field is not in the supported
/// `3.0.x` or `3.1.x` range.
///
/// Parses the input as YAML (a superset of JSON) and inspects the top-level
/// `openapi` value. Versions outside `3.0.x` and `3.1.x` are rejected with a
/// `SpecError::Parse` carrying the offending version string so callers can
/// surface a clean, actionable diagnostic. Specs without that key, or that
/// fail to parse here, fall through silently so downstream parsing surfaces
/// its own error.
fn reject_unsupported_openapi_version(content: &str) -> Result<(), SpecError> {
    let parsed = match serde_yaml::from_str::<serde_yaml::Value>(content) {
        Ok(v) => v,
        Err(_) => return Ok(()),
    };
    let serde_yaml::Value::Mapping(map) = &parsed else {
        return Ok(());
    };
    let key = serde_yaml::Value::String("openapi".to_string());
    let Some(serde_yaml::Value::String(version)) = map.get(&key) else {
        return Ok(());
    };
    let re = regex::Regex::new(r"^3\.(0|1)\.\d+$").expect("static openapi version regex is valid");
    if re.is_match(version) {
        return Ok(());
    }
    Err(SpecError::Parse(format!(
        "unsupported openapi version {version}: chasm supports 3.0.x and 3.1.x"
    )))
}

/// Runs post-parse semantic validation against a successfully deserialised
/// `OpenAPI` value and its normalised YAML tree.
///
/// Performs three structural checks that the `openapiv3` v2 deserialiser is
/// too permissive to catch on its own: dangling local `$ref` pointers,
/// duplicate `operationId` declarations across all path/method pairs, and
/// invalid `type` keywords on any schema. Each failure surfaces as a
/// `SpecError::Parse` whose message identifies the kind of violation and
/// includes a JSON-pointer-style locator where applicable.
fn validate_spec_semantics(spec: &OpenAPI, raw_yaml: &serde_yaml::Value) -> Result<(), SpecError> {
    check_dangling_refs(raw_yaml)?;
    check_duplicate_operation_ids(spec)?;
    check_invalid_schema_types(raw_yaml)?;
    Ok(())
}

/// Walks the normalised YAML tree and rejects any local `$ref` whose target
/// does not resolve to an existing component bag entry.
///
/// Targets of the form `#/components/<bag>/<name>` are validated against the
/// canonical bag set: `schemas`, `parameters`, `responses`, `requestBodies`,
/// `headers`, `securitySchemes`, `callbacks`, and `pathItems`. References
/// pointing into other parts of the document (e.g. `#/paths/~1foo`) and
/// non-fragment refs (HTTP, HTTPS, file URIs and relative file paths) are
/// left to other passes — the SSRF guard and the remote-ref resolver own that
/// territory. The pointer carried in the error message is the original
/// `$ref` string verbatim.
fn check_dangling_refs(root: &serde_yaml::Value) -> Result<(), SpecError> {
    let bags = [
        "schemas",
        "parameters",
        "responses",
        "requestBodies",
        "headers",
        "securitySchemes",
        "callbacks",
        "pathItems",
    ];
    let mut error: Option<String> = None;
    walk_for_refs(root, &mut |pointer| {
        if !pointer.starts_with("#/components/") {
            return;
        }
        let rest = match pointer.strip_prefix("#/components/") {
            Some(r) => r,
            None => return,
        };
        let mut parts = rest.splitn(2, '/');
        let bag = match parts.next() {
            Some(b) if bags.contains(&b) => b,
            _ => return,
        };
        let name = match parts.next() {
            Some(n) if !n.is_empty() => n,
            _ => return,
        };
        if resolve_component_entry(root, bag, name).is_some() {
            return;
        }
        if error.is_none() {
            error = Some(format!(
                "dangling $ref '{pointer}': target does not exist in spec"
            ));
        }
    });
    if let Some(msg) = error {
        return Err(SpecError::Parse(msg));
    }
    Ok(())
}

/// Resolves a single `components.<bag>.<name>` entry, decoding the JSON
/// Pointer escapes (`~0` and `~1`) embedded in the name segment.
fn resolve_component_entry(
    root: &serde_yaml::Value,
    bag: &str,
    name: &str,
) -> Option<serde_yaml::Value> {
    let decoded = name.replace("~1", "/").replace("~0", "~");
    let serde_yaml::Value::Mapping(root_map) = root else {
        return None;
    };
    let components = root_map.get(serde_yaml::Value::String("components".to_string()))?;
    let serde_yaml::Value::Mapping(comp_map) = components else {
        return None;
    };
    let bag_val = comp_map.get(serde_yaml::Value::String(bag.to_string()))?;
    let serde_yaml::Value::Mapping(bag_map) = bag_val else {
        return None;
    };
    bag_map.get(serde_yaml::Value::String(decoded)).cloned()
}

/// Recursive helper that invokes `visit` for every `$ref` string value found
/// inside `node`.
///
/// Only `$ref` entries whose value is a string and whose containing mapping
/// has exactly one key (`$ref`) are visited; that matches the OpenAPI
/// reference-object shape and avoids walking into schemas that legitimately
/// carry a sibling property whose name happens to be `$ref`.
fn walk_for_refs(node: &serde_yaml::Value, visit: &mut dyn FnMut(&str)) {
    match node {
        serde_yaml::Value::Mapping(map) => {
            let ref_key = serde_yaml::Value::String("$ref".to_string());
            if let Some(serde_yaml::Value::String(target)) = map.get(&ref_key) {
                visit(target.as_str());
            }
            for (_k, v) in map.iter() {
                walk_for_refs(v, visit);
            }
        }
        serde_yaml::Value::Sequence(seq) => {
            for item in seq.iter() {
                walk_for_refs(item, visit);
            }
        }
        _ => {}
    }
}

/// Walks `spec.paths` and rejects any spec that declares the same
/// `operationId` on two different `(method, path)` pairs.
///
/// Operations without an explicit `operationId` are ignored — the keyword
/// is optional in OpenAPI. Order of detection follows path-map iteration
/// order, which is insertion-stable thanks to `IndexMap` underneath
/// `openapiv3::Paths`.
fn check_duplicate_operation_ids(spec: &OpenAPI) -> Result<(), SpecError> {
    use std::collections::HashMap;
    let mut seen: HashMap<String, (String, String)> = HashMap::new();
    for (path, path_item_ref) in &spec.paths.paths {
        let openapiv3::ReferenceOr::Item(path_item) = path_item_ref else {
            continue;
        };
        for (method, op) in operations_in(path_item) {
            let Some(op_id) = op.operation_id.as_deref() else {
                continue;
            };
            if let Some((prev_method, prev_path)) = seen.get(op_id) {
                return Err(SpecError::Parse(format!(
                    "duplicate operationId '{op_id}': defined on {prev_method} {prev_path} and {method} {path}"
                )));
            }
            seen.insert(op_id.to_string(), (method.to_string(), path.to_string()));
        }
    }
    Ok(())
}

/// Yields the `(method, operation)` pairs declared on a single `PathItem`.
///
/// Order matches the canonical OpenAPI method list (get, put, post, delete,
/// options, head, patch, trace). Methods that the path item does not declare
/// are skipped so the caller does not have to filter out `None` values.
fn operations_in(item: &openapiv3::PathItem) -> Vec<(&'static str, &openapiv3::Operation)> {
    let mut out: Vec<(&'static str, &openapiv3::Operation)> = Vec::new();
    if let Some(op) = item.get.as_ref() {
        out.push(("GET", op));
    }
    if let Some(op) = item.put.as_ref() {
        out.push(("PUT", op));
    }
    if let Some(op) = item.post.as_ref() {
        out.push(("POST", op));
    }
    if let Some(op) = item.delete.as_ref() {
        out.push(("DELETE", op));
    }
    if let Some(op) = item.options.as_ref() {
        out.push(("OPTIONS", op));
    }
    if let Some(op) = item.head.as_ref() {
        out.push(("HEAD", op));
    }
    if let Some(op) = item.patch.as_ref() {
        out.push(("PATCH", op));
    }
    if let Some(op) = item.trace.as_ref() {
        out.push(("TRACE", op));
    }
    out
}

/// Walks the normalised YAML tree and rejects any schema whose `type`
/// keyword is a string outside the closed JSON Schema type set.
///
/// The closed set is `null`, `boolean`, `integer`, `number`, `string`,
/// `array`, and `object`. Sequence-valued `type` entries (the OAS 3.1
/// multi-type form) are checked element-by-element. Non-string entries
/// inside such a sequence are tolerated — they are surfaced by the
/// downstream `openapiv3` deserialiser, which has more context than this
/// pass to produce a helpful error.
///
/// Only schema-bearing locations are inspected: `components.schemas.*` and
/// every other site reachable through schema-carrying keywords
/// (`properties`, `items`, `additionalProperties`, `prefixItems`, `allOf`,
/// `anyOf`, `oneOf`, `not`, `schema`). This excludes `components.securitySchemes`
/// where `type: http` / `type: apiKey` / `type: oauth2` / `type: openIdConnect`
/// are valid OAS keywords distinct from the JSON Schema `type` enum.
fn check_invalid_schema_types(root: &serde_yaml::Value) -> Result<(), SpecError> {
    let mut error: Option<String> = None;
    visit_schemas(root, &mut Vec::new(), &mut |pointer, schema| {
        if error.is_some() {
            return;
        }
        if let Some(msg) = validate_schema_type(pointer, schema) {
            error = Some(msg);
        }
    });
    if let Some(msg) = error {
        return Err(SpecError::Parse(msg));
    }
    Ok(())
}

/// Validates the `type` keyword on a single schema node.
///
/// Returns `None` when the node carries no `type` keyword, when the keyword
/// is well-formed, or when the keyword's value is not a kind this pass can
/// classify (those degenerate forms are left to the downstream deserialiser).
/// Returns `Some(message)` when the `type` is a string outside the closed
/// JSON Schema type set, or when a sequence-valued `type` contains such an
/// entry.
fn validate_schema_type(pointer: &str, schema: &serde_yaml::Value) -> Option<String> {
    let allowed: &[&str] = &[
        "null", "boolean", "integer", "number", "string", "array", "object",
    ];
    let serde_yaml::Value::Mapping(map) = schema else {
        return None;
    };
    let type_key = serde_yaml::Value::String("type".to_string());
    let type_val = map.get(&type_key)?;
    match type_val {
        serde_yaml::Value::String(s) => {
            if allowed.contains(&s.as_str()) {
                None
            } else {
                Some(format!(
                    "invalid schema type '{s}' at {pointer}/type: must be one of null, boolean, integer, number, string, array, object"
                ))
            }
        }
        serde_yaml::Value::Sequence(seq) => {
            for entry in seq.iter() {
                if let serde_yaml::Value::String(s) = entry {
                    if !allowed.contains(&s.as_str()) {
                        return Some(format!(
                            "invalid schema type '{s}' at {pointer}/type: must be one of null, boolean, integer, number, string, array, object"
                        ));
                    }
                }
            }
            None
        }
        _ => None,
    }
}

/// Walks the spec tree and invokes `visit` for every node that is a Schema
/// object.
///
/// Schema objects are reached via three entry points:
/// 1. `components.schemas.*`
/// 2. `parameters.<n>.schema` and `headers.<n>.schema` under any
///    operation or component bag, plus `requestBody.content.<media>.schema`,
///    `responses.<code>.content.<media>.schema`, and
///    `responses.<code>.headers.<n>.schema`.
/// 3. Recursively, from inside another Schema, via `properties.*`,
///    `additionalProperties` (when an object), `items`, every element of
///    `prefixItems`, every element of `allOf` / `anyOf` / `oneOf`, and
///    `not`.
fn visit_schemas(
    root: &serde_yaml::Value,
    path: &mut Vec<String>,
    visit: &mut dyn FnMut(&str, &serde_yaml::Value),
) {
    let serde_yaml::Value::Mapping(root_map) = root else {
        return;
    };
    if let Some(serde_yaml::Value::Mapping(components)) =
        root_map.get(serde_yaml::Value::String("components".to_string()))
    {
        if let Some(serde_yaml::Value::Mapping(schemas)) =
            components.get(serde_yaml::Value::String("schemas".to_string()))
        {
            for (name, schema) in schemas.iter() {
                let name_str = key_as_str(name);
                path.push("components".to_string());
                path.push("schemas".to_string());
                path.push(name_str);
                walk_schema(schema, path, visit);
                path.pop();
                path.pop();
                path.pop();
            }
        }
    }
    if let Some(serde_yaml::Value::Mapping(paths)) =
        root_map.get(serde_yaml::Value::String("paths".to_string()))
    {
        for (path_key, path_item) in paths.iter() {
            let path_str = key_as_str(path_key);
            path.push("paths".to_string());
            path.push(path_str);
            visit_path_item(path_item, path, visit);
            path.pop();
            path.pop();
        }
    }
}

/// Walks one path-item, descending into each declared operation to surface
/// the schema-bearing children (`parameters`, `requestBody`, `responses`).
fn visit_path_item(
    item: &serde_yaml::Value,
    path: &mut Vec<String>,
    visit: &mut dyn FnMut(&str, &serde_yaml::Value),
) {
    let serde_yaml::Value::Mapping(map) = item else {
        return;
    };
    let methods = [
        "get", "put", "post", "delete", "options", "head", "patch", "trace",
    ];
    for method in methods.iter() {
        let Some(op) = map.get(serde_yaml::Value::String((*method).to_string())) else {
            continue;
        };
        path.push((*method).to_string());
        visit_operation(op, path, visit);
        path.pop();
    }
}

/// Walks one operation, surfacing schemas reachable via `parameters`,
/// `requestBody.content.<media>.schema`, and `responses.<code>` (whose
/// `content` and `headers` children carry further schemas).
fn visit_operation(
    op: &serde_yaml::Value,
    path: &mut Vec<String>,
    visit: &mut dyn FnMut(&str, &serde_yaml::Value),
) {
    let serde_yaml::Value::Mapping(map) = op else {
        return;
    };
    if let Some(serde_yaml::Value::Sequence(params)) =
        map.get(serde_yaml::Value::String("parameters".to_string()))
    {
        path.push("parameters".to_string());
        for (idx, param) in params.iter().enumerate() {
            path.push(idx.to_string());
            visit_param_or_header_schema(param, path, visit);
            path.pop();
        }
        path.pop();
    }
    if let Some(rb) = map.get(serde_yaml::Value::String("requestBody".to_string())) {
        path.push("requestBody".to_string());
        visit_content_block(rb, path, visit);
        path.pop();
    }
    if let Some(serde_yaml::Value::Mapping(responses)) =
        map.get(serde_yaml::Value::String("responses".to_string()))
    {
        path.push("responses".to_string());
        for (code, resp) in responses.iter() {
            let code_str = key_as_str(code);
            path.push(code_str);
            visit_response(resp, path, visit);
            path.pop();
        }
        path.pop();
    }
}

/// Walks a Parameter or Header object that may carry a `schema` child.
fn visit_param_or_header_schema(
    node: &serde_yaml::Value,
    path: &mut Vec<String>,
    visit: &mut dyn FnMut(&str, &serde_yaml::Value),
) {
    let serde_yaml::Value::Mapping(map) = node else {
        return;
    };
    if let Some(schema) = map.get(serde_yaml::Value::String("schema".to_string())) {
        path.push("schema".to_string());
        walk_schema(schema, path, visit);
        path.pop();
    }
    visit_content_block(node, path, visit);
}

/// Walks a Response object, descending into its `content` and `headers`
/// children.
fn visit_response(
    node: &serde_yaml::Value,
    path: &mut Vec<String>,
    visit: &mut dyn FnMut(&str, &serde_yaml::Value),
) {
    let serde_yaml::Value::Mapping(map) = node else {
        return;
    };
    visit_content_block(node, path, visit);
    if let Some(serde_yaml::Value::Mapping(headers)) =
        map.get(serde_yaml::Value::String("headers".to_string()))
    {
        path.push("headers".to_string());
        for (name, header) in headers.iter() {
            let name_str = key_as_str(name);
            path.push(name_str);
            visit_param_or_header_schema(header, path, visit);
            path.pop();
        }
        path.pop();
    }
}

/// Walks the `content.<media>.schema` child of any node that carries one.
fn visit_content_block(
    node: &serde_yaml::Value,
    path: &mut Vec<String>,
    visit: &mut dyn FnMut(&str, &serde_yaml::Value),
) {
    let serde_yaml::Value::Mapping(map) = node else {
        return;
    };
    let Some(serde_yaml::Value::Mapping(content)) =
        map.get(serde_yaml::Value::String("content".to_string()))
    else {
        return;
    };
    path.push("content".to_string());
    for (media, entry) in content.iter() {
        let media_str = key_as_str(media);
        path.push(media_str);
        if let serde_yaml::Value::Mapping(entry_map) = entry {
            if let Some(schema) = entry_map.get(serde_yaml::Value::String("schema".to_string())) {
                path.push("schema".to_string());
                walk_schema(schema, path, visit);
                path.pop();
            }
        }
        path.pop();
    }
    path.pop();
}

/// Recursively walks a Schema object, surfacing every nested schema reached
/// via the schema-carrying keywords listed on [`visit_schemas`].
fn walk_schema(
    schema: &serde_yaml::Value,
    path: &mut Vec<String>,
    visit: &mut dyn FnMut(&str, &serde_yaml::Value),
) {
    let pointer = build_pointer_from_path(path);
    visit(&pointer, schema);
    let serde_yaml::Value::Mapping(map) = schema else {
        return;
    };
    let ref_key = serde_yaml::Value::String("$ref".to_string());
    if map.contains_key(&ref_key) {
        return;
    }
    if let Some(serde_yaml::Value::Mapping(props)) =
        map.get(serde_yaml::Value::String("properties".to_string()))
    {
        path.push("properties".to_string());
        for (name, sub) in props.iter() {
            let name_str = key_as_str(name);
            path.push(name_str);
            walk_schema(sub, path, visit);
            path.pop();
        }
        path.pop();
    }
    if let Some(addl) = map.get(serde_yaml::Value::String(
        "additionalProperties".to_string(),
    )) {
        if matches!(addl, serde_yaml::Value::Mapping(_)) {
            path.push("additionalProperties".to_string());
            walk_schema(addl, path, visit);
            path.pop();
        }
    }
    if let Some(items) = map.get(serde_yaml::Value::String("items".to_string())) {
        if matches!(items, serde_yaml::Value::Mapping(_)) {
            path.push("items".to_string());
            walk_schema(items, path, visit);
            path.pop();
        }
    }
    for combinator in ["allOf", "anyOf", "oneOf", "prefixItems"].iter() {
        if let Some(serde_yaml::Value::Sequence(seq)) =
            map.get(serde_yaml::Value::String((*combinator).to_string()))
        {
            path.push((*combinator).to_string());
            for (idx, sub) in seq.iter().enumerate() {
                path.push(idx.to_string());
                walk_schema(sub, path, visit);
                path.pop();
            }
            path.pop();
        }
    }
    if let Some(not_schema) = map.get(serde_yaml::Value::String("not".to_string())) {
        path.push("not".to_string());
        walk_schema(not_schema, path, visit);
        path.pop();
    }
}

/// Renders a YAML mapping key as its string representation, falling back to
/// `<non-string>` for keys that are not strings, numbers, or booleans.
fn key_as_str(key: &serde_yaml::Value) -> String {
    match key {
        serde_yaml::Value::String(s) => s.clone(),
        serde_yaml::Value::Number(n) => n.to_string(),
        serde_yaml::Value::Bool(b) => b.to_string(),
        _ => "<non-string>".to_string(),
    }
}

/// Builds an RFC 6901 JSON Pointer from a trail of unescaped segments.
fn build_pointer_from_path(path: &[String]) -> String {
    let mut out = String::from("#");
    for seg in path {
        out.push('/');
        out.push_str(&escape_pointer_segment(seg));
    }
    out
}

/// Escapes a single JSON Pointer reference token per RFC 6901.
fn escape_pointer_segment(seg: &str) -> String {
    seg.replace('~', "~0").replace('/', "~1")
}

/// Detects Swagger 2.0 documents and rejects them with a clear, actionable error.
///
/// Parses the input as YAML (a superset of JSON) and inspects the top-level
/// `swagger` key; if it equals the string `"2.0"` the function returns a
/// `SpecError::Parse` carrying conversion guidance. Specs without that key — or
/// that fail to parse here — fall through silently so the downstream OpenAPI
/// parser surfaces its own diagnostic.
fn reject_swagger_two(content: &str) -> Result<(), SpecError> {
    if let Ok(serde_yaml::Value::Mapping(map)) = serde_yaml::from_str::<serde_yaml::Value>(content)
    {
        let key = serde_yaml::Value::String("swagger".to_string());
        if let Some(serde_yaml::Value::String(version)) = map.get(&key) {
            if version == "2.0" {
                return Err(SpecError::Parse(
                    "Swagger 2.0 specs are not supported \u{2014} chasm requires OpenAPI 3.0 or 3.1. Convert via `swagger2openapi` or `openapi-generator`.".to_string(),
                ));
            }
        }
    }
    Ok(())
}

/// Normalises OAS 3.1 nullable-via-type-array idioms into OAS 3.0 `nullable: true`.
///
/// Parses `spec_str` as YAML (a superset of JSON) into a `serde_yaml::Value` so the
/// underlying `IndexMap`-backed mapping preserves the original key insertion order,
/// walks the resulting tree recursively, and rewrites every node whose `type`
/// keyword is the two-element array `["X", "null"]` or `["null", "X"]` into a node
/// with the scalar `type: "X"` plus `nullable: true`. Arbitrary multi-type arrays
/// that do not include `"null"` are left intact so that downstream tooling (the
/// `openapiv3` extension-bag deserialiser and `chasm-faker`'s multi-type dispatch)
/// can continue to handle them via their own pathways.
///
/// The rewritten value is re-serialised straight to JSON via `serde_json::to_string`
/// — `serde_yaml::Value`'s `Serialize` impl walks the `IndexMap` in insertion order
/// so the resulting JSON string retains the original keying. This matters for
/// `openapiv3` consumers that depend on declaration order (notably `content`
/// negotiation, which prefers the first declared media type when `Accept: */*`).
pub fn normalize_oas31_nullable(spec_str: &str) -> Result<String, SpecError> {
    let clamped = clamp_integer_bounds_textual(spec_str);
    let mut value: serde_yaml::Value =
        serde_yaml::from_str(&clamped).map_err(|e| SpecError::Parse(e.to_string()))?;
    rewrite_nullable_type_arrays_yaml(&mut value);
    inject_header_schema_fallback_yaml(&mut value);
    coerce_float_bounds_yaml(&mut value);
    rewrite_exclusive_bounds_yaml(&mut value);
    drop_invalid_items_yaml(&mut value);
    inline_path_item_refs_yaml(&mut value);
    inline_callback_refs_yaml(&mut value);
    serde_json::to_string(&value).map_err(|e| SpecError::Parse(e.to_string()))
}

/// Rewrites OAS 3.1 numeric `exclusiveMinimum`/`exclusiveMaximum` into the OAS 3.0
/// boolean form that `openapiv3` v2 understands.
///
/// In OAS 3.0, `exclusiveMinimum: true` is a flag that toggles whether the existing
/// `minimum` keyword is exclusive. OAS 3.1 inherited JSON Schema 2020-12, which
/// promoted `exclusiveMinimum`/`exclusiveMaximum` to numeric keywords that stand on
/// their own. This walker translates the newer numeric form back into the 3.0
/// idiom by replacing `exclusiveMinimum: 5` with `minimum: 5, exclusiveMinimum:
/// true` so the downstream deserialiser accepts the schema. Schemas that already
/// use the boolean form, or that already declare both `minimum` and a numeric
/// `exclusiveMinimum` together, are left untouched. The same logic is applied to
/// `exclusiveMaximum`.
fn rewrite_exclusive_bounds_yaml(value: &mut serde_yaml::Value) {
    match value {
        serde_yaml::Value::Mapping(map) => {
            rewrite_exclusive_bound_pair(map, "exclusiveMinimum", "minimum");
            rewrite_exclusive_bound_pair(map, "exclusiveMaximum", "maximum");
            for (_k, v) in map.iter_mut() {
                rewrite_exclusive_bounds_yaml(v);
            }
        }
        serde_yaml::Value::Sequence(seq) => {
            for item in seq.iter_mut() {
                rewrite_exclusive_bounds_yaml(item);
            }
        }
        _ => {}
    }
}

/// Rewrites one numeric `exclusive*` keyword inside a single mapping into the
/// 3.0 boolean form, paired with the matching `minimum`/`maximum` bound.
///
/// Does nothing when the keyword is already a boolean, when the keyword is
/// absent, or when the matching numeric bound (`minimum`/`maximum`) already
/// coexists with a boolean `exclusive*` flag — that combination is already the
/// 3.0 idiom and must be preserved verbatim.
fn rewrite_exclusive_bound_pair(
    map: &mut serde_yaml::Mapping,
    exclusive_key_name: &str,
    bound_key_name: &str,
) {
    let exclusive_key = serde_yaml::Value::String(exclusive_key_name.to_string());
    let bound_key = serde_yaml::Value::String(bound_key_name.to_string());
    let current = match map.get(&exclusive_key) {
        Some(v) => v.clone(),
        None => return,
    };
    if matches!(current, serde_yaml::Value::Bool(_)) {
        return;
    }
    let numeric = match &current {
        serde_yaml::Value::Number(n) => n.clone(),
        _ => return,
    };
    if let Some(existing_bound) = map.get(&bound_key) {
        if matches!(existing_bound, serde_yaml::Value::Number(_)) {
            return;
        }
    }
    map.insert(bound_key.clone(), serde_yaml::Value::Number(numeric));
    map.insert(exclusive_key.clone(), serde_yaml::Value::Bool(true));
    tracing::debug!(
        keyword = exclusive_key_name,
        "rewrote OAS 3.1 numeric exclusive bound to 3.0 boolean form"
    );
}

/// Walks the YAML tree and drops `items: false` / `items: true` siblings of
/// `prefixItems` so the downstream `openapiv3` schema deserialiser, which only
/// understands `items: <schema>`, does not bail.
///
/// In JSON Schema 2020-12, `items: false` after `prefixItems` means "no items
/// beyond the prefix tuple"; `items: true` means "any items beyond the prefix
/// tuple". `openapiv3` v2 cannot model either form. chasm-faker already honours
/// the implicit no-items-beyond-prefix semantics when `items` is absent, so
/// removing the offending node is the safe normalisation. Schemas without
/// `prefixItems`, or where `items` is itself a schema object, are left intact.
fn drop_invalid_items_yaml(value: &mut serde_yaml::Value) {
    match value {
        serde_yaml::Value::Mapping(map) => {
            let prefix_key = serde_yaml::Value::String("prefixItems".to_string());
            let items_key = serde_yaml::Value::String("items".to_string());
            if map.contains_key(&prefix_key) {
                if let Some(items) = map.get(&items_key) {
                    if matches!(items, serde_yaml::Value::Bool(_)) {
                        let dropped = items.clone();
                        map.remove(&items_key);
                        tracing::debug!(
                            value = ?dropped,
                            "dropped boolean items keyword alongside prefixItems"
                        );
                    }
                }
            }
            for (_k, v) in map.iter_mut() {
                drop_invalid_items_yaml(v);
            }
        }
        serde_yaml::Value::Sequence(seq) => {
            for item in seq.iter_mut() {
                drop_invalid_items_yaml(item);
            }
        }
        _ => {}
    }
}

/// Inlines path-item `$ref` nodes under the top-level `paths` mapping.
///
/// `openapiv3` v2 does not surface `components.pathItems`, so a spec that
/// declares `paths./foo: {$ref: '#/components/pathItems/SharedPath'}` parses
/// cleanly but the route is silently dropped. This walker resolves every such
/// reference against the root document and replaces the `$ref` envelope with
/// the target path-item object. Unresolvable refs are left intact so the
/// downstream parser surfaces a clean error.
fn inline_path_item_refs_yaml(value: &mut serde_yaml::Value) {
    let snapshot = value.clone();
    let serde_yaml::Value::Mapping(root) = value else {
        return;
    };
    let paths_key = serde_yaml::Value::String("paths".to_string());
    let Some(paths_val) = root.get_mut(&paths_key) else {
        return;
    };
    let serde_yaml::Value::Mapping(paths_map) = paths_val else {
        return;
    };
    let ref_key = serde_yaml::Value::String("$ref".to_string());
    for (path_key, path_val) in paths_map.iter_mut() {
        let pointer = match path_val {
            serde_yaml::Value::Mapping(inner) => match inner.get(&ref_key) {
                Some(serde_yaml::Value::String(s)) if inner.len() == 1 => s.clone(),
                _ => continue,
            },
            _ => continue,
        };
        if let Some(resolved) = resolve_json_pointer(&snapshot, &pointer) {
            *path_val = resolved;
            let path_label = path_key.as_str().unwrap_or("<non-string>");
            tracing::debug!(
                path = path_label,
                pointer = pointer.as_str(),
                "inlined path-item $ref"
            );
        }
    }
}

/// Inlines callback-entry `$ref` nodes under any operation's `callbacks` map.
///
/// `openapiv3` v2 does not deserialise `{ "$ref": "..." }` in callback position,
/// so a callback expressed via a reusable component reference fails the whole
/// spec parse. This walker visits every `<operation>.callbacks.<name>` entry,
/// and when it finds a single-key `$ref` envelope it replaces the entry with the
/// resolved object. Failures to resolve are tolerated — the original `$ref` is
/// left in place so a later parse surfaces the structural issue without
/// crashing the loader.
fn inline_callback_refs_yaml(value: &mut serde_yaml::Value) {
    let snapshot = value.clone();
    inline_callback_refs_walker(value, &snapshot);
}

/// Recursive helper for `inline_callback_refs_yaml` that descends the tree
/// looking for `callbacks` mappings and rewrites each entry that is a bare
/// `$ref` envelope.
fn inline_callback_refs_walker(value: &mut serde_yaml::Value, snapshot: &serde_yaml::Value) {
    match value {
        serde_yaml::Value::Mapping(map) => {
            let callbacks_key = serde_yaml::Value::String("callbacks".to_string());
            if let Some(serde_yaml::Value::Mapping(cb_map)) = map.get_mut(&callbacks_key) {
                let ref_key = serde_yaml::Value::String("$ref".to_string());
                for (name_key, entry) in cb_map.iter_mut() {
                    let pointer = match entry {
                        serde_yaml::Value::Mapping(inner) => match inner.get(&ref_key) {
                            Some(serde_yaml::Value::String(s)) if inner.len() == 1 => s.clone(),
                            _ => continue,
                        },
                        _ => continue,
                    };
                    if let Some(resolved) = resolve_json_pointer(snapshot, &pointer) {
                        *entry = resolved;
                        let cb_label = name_key.as_str().unwrap_or("<non-string>");
                        tracing::debug!(
                            callback = cb_label,
                            pointer = pointer.as_str(),
                            "inlined callback $ref"
                        );
                    }
                }
            }
            for (_k, v) in map.iter_mut() {
                inline_callback_refs_walker(v, snapshot);
            }
        }
        serde_yaml::Value::Sequence(seq) => {
            for item in seq.iter_mut() {
                inline_callback_refs_walker(item, snapshot);
            }
        }
        _ => {}
    }
}

/// Resolves a JSON Pointer (`#/components/pathItems/X`) against a YAML root.
///
/// Returns `None` for non-local pointers (anything not starting with `#/`),
/// for empty fragments, or when any segment of the pointer fails to traverse
/// the tree. Pointer-token unescaping follows RFC 6901: `~1` decodes to `/`
/// and `~0` decodes to `~`.
fn resolve_json_pointer(root: &serde_yaml::Value, pointer: &str) -> Option<serde_yaml::Value> {
    let fragment = pointer.strip_prefix("#/")?;
    if fragment.is_empty() {
        return Some(root.clone());
    }
    let mut current = root;
    for raw_token in fragment.split('/') {
        let token = raw_token.replace("~1", "/").replace("~0", "~");
        current = match current {
            serde_yaml::Value::Mapping(map) => {
                let key = serde_yaml::Value::String(token.clone());
                map.get(&key)?
            }
            serde_yaml::Value::Sequence(seq) => {
                let idx: usize = token.parse().ok()?;
                seq.get(idx)?
            }
            _ => return None,
        };
    }
    Some(current.clone())
}

/// Rewrites `minimum:` and `maximum:` lines whose numeric literal falls outside
/// the `i64` range so the downstream `serde_yaml` parser does not bail.
///
/// `serde_yaml` 0.9 refuses to materialise any integer that does not fit into
/// `i64` (or `u64`), surfacing `invalid type: integer ... as i128, expected any
/// YAML value` before we ever get a `Value` to walk. OpenAI's spec hits this
/// path with `seed.minimum: -9223372036854776000`. We therefore work on the
/// raw text: a regex matches the YAML key `minimum:`/`maximum:` followed by an
/// optionally-signed integer literal, and any literal outside `[i64::MIN,
/// i64::MAX]` is rewritten to the nearest `i64` boundary. Plain integers — and
/// floats, scientific-notation, and the rest of the YAML number grammar — are
/// left untouched, the latter because they are handled later inside the YAML
/// `Value` walk by [`coerce_float_bounds_yaml`].
fn clamp_integer_bounds_textual(spec: &str) -> String {
    let bound_re =
        regex::Regex::new(r"(?m)^(?P<indent>\s*)(?P<key>minimum|maximum):\s*(?P<num>-?\d+)\s*$")
            .expect("static regex is valid");
    bound_re
        .replace_all(spec, |caps: &regex::Captures<'_>| {
            let indent = &caps["indent"];
            let key = &caps["key"];
            let num = &caps["num"];
            match num.parse::<i128>() {
                Ok(parsed) => {
                    if parsed < i64::MIN as i128 {
                        tracing::warn!(
                            bound = key,
                            value = num,
                            clamped_to = i64::MIN,
                            "integer bound outside i64 range; clamped"
                        );
                        format!("{indent}{key}: {}", i64::MIN)
                    } else if parsed > i64::MAX as i128 {
                        tracing::warn!(
                            bound = key,
                            value = num,
                            clamped_to = i64::MAX,
                            "integer bound outside i64 range; clamped"
                        );
                        format!("{indent}{key}: {}", i64::MAX)
                    } else {
                        format!("{indent}{key}: {num}")
                    }
                }
                Err(_) => {
                    let sign_negative = num.starts_with('-');
                    let clamped = if sign_negative { i64::MIN } else { i64::MAX };
                    tracing::warn!(
                        bound = key,
                        value = num,
                        clamped_to = clamped,
                        "integer bound exceeds i128 range; clamped"
                    );
                    format!("{indent}{key}: {clamped}")
                }
            }
        })
        .into_owned()
}

/// Walks the spec tree and injects `schema: {type: string}` into response header
/// objects that declare neither `schema` nor `content`.
///
/// Many real-world specs write `description: foo` for response headers
/// without specifying a payload shape. `openapiv3`'s `ParameterSchemaOrContent`
/// enum is `#[serde(flatten)]`-ed without a default variant, so a header with
/// only `description` fails to deserialise. This pass restores forward progress
/// by treating any descriptionless-payload header as a free-form string.
///
/// The fix is scoped strictly to nodes reachable via the `responses.<*>.headers.<*>`
/// path so we never accidentally inject schemas elsewhere. A `tracing::warn!` is
/// emitted for every node we touch.
fn inject_header_schema_fallback_yaml(value: &mut serde_yaml::Value) {
    if let serde_yaml::Value::Mapping(root) = value {
        for (_top_key, top_val) in root.iter_mut() {
            walk_for_responses(top_val);
        }
    }
}

/// Recursively descends into mapping nodes looking for `responses` keys, then
/// dispatches to [`fix_responses_node`] on each match.
fn walk_for_responses(value: &mut serde_yaml::Value) {
    match value {
        serde_yaml::Value::Mapping(map) => {
            let responses_key = serde_yaml::Value::String("responses".to_string());
            if let Some(responses) = map.get_mut(&responses_key) {
                fix_responses_node(responses);
            }
            for (_k, v) in map.iter_mut() {
                walk_for_responses(v);
            }
        }
        serde_yaml::Value::Sequence(seq) => {
            for item in seq.iter_mut() {
                walk_for_responses(item);
            }
        }
        _ => {}
    }
}

/// Iterates the entries of a `responses` mapping and applies the header fix to
/// each individual response object's `headers` child.
fn fix_responses_node(node: &mut serde_yaml::Value) {
    if let serde_yaml::Value::Mapping(map) = node {
        let headers_key = serde_yaml::Value::String("headers".to_string());
        for (_code, response) in map.iter_mut() {
            if let serde_yaml::Value::Mapping(resp_map) = response {
                if let Some(headers) = resp_map.get_mut(&headers_key) {
                    fix_headers_node(headers);
                }
            }
        }
    }
}

/// Visits each header inside a `headers` mapping and injects a string-typed
/// schema fallback when neither `schema` nor `content` is present.
fn fix_headers_node(node: &mut serde_yaml::Value) {
    if let serde_yaml::Value::Mapping(map) = node {
        let schema_key = serde_yaml::Value::String("schema".to_string());
        let content_key = serde_yaml::Value::String("content".to_string());
        let ref_key = serde_yaml::Value::String("$ref".to_string());
        let type_key = serde_yaml::Value::String("type".to_string());
        for (name, header) in map.iter_mut() {
            if let serde_yaml::Value::Mapping(header_map) = header {
                let has_schema = header_map.contains_key(&schema_key);
                let has_content = header_map.contains_key(&content_key);
                let has_ref = header_map.contains_key(&ref_key);
                if !has_schema && !has_content && !has_ref {
                    let mut fallback = serde_yaml::Mapping::new();
                    fallback.insert(
                        type_key.clone(),
                        serde_yaml::Value::String("string".to_string()),
                    );
                    header_map.insert(schema_key.clone(), serde_yaml::Value::Mapping(fallback));
                    let header_name = name.as_str().unwrap_or("<non-string>");
                    tracing::warn!(
                        header = header_name,
                        "response header lacks schema/content; injecting type:string fallback"
                    );
                }
            }
        }
    }
}

/// Walks the YAML tree and coerces any `minimum`/`maximum` bound stored as a
/// float (e.g. `1e21`) into the nearest `i64` so the downstream `openapiv3`
/// schema deserialiser, which expects an `i64`, does not reject it.
///
/// Plain integer literals outside `i64` range are already handled before YAML
/// parsing by [`clamp_integer_bounds_textual`]; this pass picks up the residue:
/// scientific-notation floats that `serde_yaml` happily parsed but `openapiv3`
/// will not accept as an integer schema bound. Non-numeric nodes and in-range
/// integers are left untouched.
fn coerce_float_bounds_yaml(value: &mut serde_yaml::Value) {
    match value {
        serde_yaml::Value::Mapping(map) => {
            let minimum_key = serde_yaml::Value::String("minimum".to_string());
            let maximum_key = serde_yaml::Value::String("maximum".to_string());
            for bound_key in [&minimum_key, &maximum_key] {
                if let Some(node) = map.get_mut(bound_key) {
                    maybe_coerce_float_bound(node, bound_key.as_str().unwrap_or("?"));
                }
            }
            for (_k, v) in map.iter_mut() {
                coerce_float_bounds_yaml(v);
            }
        }
        serde_yaml::Value::Sequence(seq) => {
            for item in seq.iter_mut() {
                coerce_float_bounds_yaml(item);
            }
        }
        _ => {}
    }
}

/// Converts a float-typed bound into the nearest `i64`, clamping infinities and
/// out-of-range magnitudes to the `i64` boundary.
fn maybe_coerce_float_bound(node: &mut serde_yaml::Value, bound_name: &str) {
    let number = match node {
        serde_yaml::Value::Number(n) => n.clone(),
        _ => return,
    };
    if number.as_i64().is_some() {
        return;
    }
    let original = number.to_string();
    let Some(f) = number.as_f64() else {
        return;
    };
    if f.is_nan() {
        return;
    }
    let lo = i64::MIN as f64;
    let hi = i64::MAX as f64;
    let clamped = f.clamp(lo, hi) as i64;
    tracing::warn!(
        bound = bound_name,
        value = original,
        clamped_to = clamped,
        "float-typed integer bound coerced to i64"
    );
    *node = serde_yaml::Value::Number(serde_yaml::Number::from(clamped));
}

/// Walks a `serde_yaml::Value` tree in place, rewriting nullable-via-type-array
/// idioms in mapping nodes and recursing into sequences and nested mappings.
fn rewrite_nullable_type_arrays_yaml(value: &mut serde_yaml::Value) {
    match value {
        serde_yaml::Value::Mapping(map) => {
            let type_key = serde_yaml::Value::String("type".to_string());
            let nullable_key = serde_yaml::Value::String("nullable".to_string());
            let rewrite = map
                .get(&type_key)
                .and_then(|v| v.as_sequence())
                .and_then(|seq| extract_nullable_pair_yaml(seq));
            if let Some(non_null) = rewrite {
                map.insert(type_key.clone(), serde_yaml::Value::String(non_null));
                if !map.contains_key(&nullable_key) {
                    map.insert(nullable_key, serde_yaml::Value::Bool(true));
                }
            }
            for (_k, v) in map.iter_mut() {
                rewrite_nullable_type_arrays_yaml(v);
            }
        }
        serde_yaml::Value::Sequence(seq) => {
            for item in seq.iter_mut() {
                rewrite_nullable_type_arrays_yaml(item);
            }
        }
        _ => {}
    }
}

/// Returns the non-null type name from a two-element YAML sequence whose other
/// element is the string `"null"`, or `None` for any other sequence shape.
fn extract_nullable_pair_yaml(seq: &[serde_yaml::Value]) -> Option<String> {
    if seq.len() != 2 {
        return None;
    }
    let a = seq[0].as_str()?;
    let b = seq[1].as_str()?;
    match (a, b) {
        ("null", other) if other != "null" => Some(other.to_string()),
        (other, "null") if other != "null" => Some(other.to_string()),
        _ => None,
    }
}

/// Walks a `serde_json::Value` tree in place, rewriting nullable-via-type-array
/// idioms in object nodes and recursing into arrays and nested objects.
///
/// Used by `load_spec_with_remote_refs`, which already operates on a JSON tree
/// after inlining remote `$ref` targets; sharing a single rewrite pass with the
/// YAML entry point is intentionally avoided so we don't pay an extra clone.
#[cfg(not(target_arch = "wasm32"))]
fn rewrite_nullable_type_arrays(value: &mut Value) {
    match value {
        Value::Object(map) => {
            let rewrite = map
                .get("type")
                .and_then(|v| v.as_array())
                .and_then(|arr| extract_nullable_pair(arr));
            if let Some(non_null) = rewrite {
                map.insert("type".to_string(), Value::String(non_null));
                map.entry("nullable".to_string())
                    .or_insert(Value::Bool(true));
            }
            for v in map.values_mut() {
                rewrite_nullable_type_arrays(v);
            }
        }
        Value::Array(arr) => {
            for item in arr.iter_mut() {
                rewrite_nullable_type_arrays(item);
            }
        }
        _ => {}
    }
}

/// Returns the non-null type name from a two-element JSON array whose other
/// element is the string `"null"`, or `None` for any other array shape.
#[cfg(not(target_arch = "wasm32"))]
fn extract_nullable_pair(arr: &[Value]) -> Option<String> {
    if arr.len() != 2 {
        return None;
    }
    let a = arr[0].as_str()?;
    let b = arr[1].as_str()?;
    match (a, b) {
        ("null", other) if other != "null" => Some(other.to_string()),
        (other, "null") if other != "null" => Some(other.to_string()),
        _ => None,
    }
}

/// Loads an OAS3 specification with basic remote `$ref` inlining.
///
/// Only compiled for non-WASM targets — `reqwest::blocking` depends on
/// `tokio` which cannot run inside `wasm32-unknown-unknown`.
///
/// Behaves identically to [`load_spec`] except that, after parsing the source
/// into a generic JSON tree, every `$ref` whose value is an `http://` or
/// `https://` URL is fetched via a short-timeout blocking HTTP GET and its
/// JSON/YAML body is inlined in place of the reference. The rewritten tree is
/// then deserialised into `OpenAPI`.
///
/// Limitations (intentional — a full json-schema-ref resolver is its own
/// project):
///
/// * Only one level deep — refs that themselves appear inside a fetched body
///   are left as-is.
/// * Each unique URL is fetched at most once per call; results are cached in
///   an in-memory map for the duration of the load.
/// * 5-second per-fetch timeout.
/// * Fetch failures fall through silently and leave the original `$ref` node
///   intact, so the loader degrades gracefully when the network or the remote
///   schema is unavailable.
#[cfg(not(target_arch = "wasm32"))]
pub fn load_spec_with_remote_refs(src: &str) -> Result<OpenAPI, SpecError> {
    let content = if src.trim_start().starts_with('{') || src.trim_start().starts_with("openapi") {
        if src.len() > MAX_SPEC_BYTES {
            return Err(SpecError::Parse(format!(
                "spec size {} bytes exceeds MAX_SPEC_BYTES={}",
                src.len(),
                MAX_SPEC_BYTES
            )));
        }
        src.to_string()
    } else {
        if let Ok(meta) = fs::metadata(src) {
            if meta.len() as usize > MAX_SPEC_BYTES {
                return Err(SpecError::Parse(format!(
                    "spec file size {} bytes exceeds MAX_SPEC_BYTES={}",
                    meta.len(),
                    MAX_SPEC_BYTES
                )));
            }
        }
        let raw = fs::read_to_string(src).map_err(|source| SpecError::Io {
            path: src.to_string(),
            source,
        })?;
        if raw.len() > MAX_SPEC_BYTES {
            return Err(SpecError::Parse(format!(
                "spec size {} bytes exceeds MAX_SPEC_BYTES={}",
                raw.len(),
                MAX_SPEC_BYTES
            )));
        }
        raw
    };

    let trimmed = content.trim_start();
    let mut tree: Value = if trimmed.starts_with('{') {
        serde_json::from_str(&content).map_err(|e| SpecError::Parse(e.to_string()))?
    } else {
        serde_yaml::from_value(
            serde_yaml::from_str(&content).map_err(|e| SpecError::Parse(e.to_string()))?,
        )
        .map_err(|e| SpecError::Parse(e.to_string()))?
    };

    let mut cache: HashMap<String, Value> = HashMap::new();
    let client = reqwest::blocking::Client::builder()
        .timeout(REMOTE_REF_FETCH_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .ok();

    let start = Instant::now();
    inline_remote_refs(&mut tree, &mut cache, client.as_ref(), start);
    rewrite_nullable_type_arrays(&mut tree);

    serde_json::from_value(tree).map_err(|e| SpecError::Parse(e.to_string()))
}

/// Recursively walks `node` and replaces remote `{ "$ref": "https://…" }` objects
/// with the fetched body of the referenced URL.
///
/// Local refs (`#/…`) and non-HTTP refs (`file://`, relative paths) are left
/// untouched so the existing in-memory resolver in `chasm-faker` keeps owning
/// the rest of the ref graph. Each URL is fetched once per call via the supplied
/// `cache`; fetch errors are logged via `tracing::warn!` and the node is left
/// as the original `$ref` for the downstream parser to deal with.
#[cfg(not(target_arch = "wasm32"))]
fn inline_remote_refs(
    node: &mut Value,
    cache: &mut HashMap<String, Value>,
    client: Option<&reqwest::blocking::Client>,
    start: Instant,
) {
    match node {
        Value::Object(map) => {
            if let Some(Value::String(target)) = map.get("$ref") {
                if is_remote_ref(target) {
                    let url = target.clone();
                    if let Some(fetched) = fetch_remote_ref(&url, cache, client, start) {
                        *node = fetched;
                        return;
                    }
                }
            }
            for v in map.values_mut() {
                inline_remote_refs(v, cache, client, start);
            }
        }
        Value::Array(items) => {
            for v in items.iter_mut() {
                inline_remote_refs(v, cache, client, start);
            }
        }
        _ => {}
    }
}

/// Returns true when `target` looks like an absolute HTTP(S) URL we should resolve.
///
/// Exposed so callers integrating their own ref-resolution pipeline can classify
/// `$ref` targets identically to `load_spec_with_remote_refs`.
#[cfg(not(target_arch = "wasm32"))]
pub fn is_remote_ref(target: &str) -> bool {
    target.starts_with("http://") || target.starts_with("https://")
}

/// Fetches `url` (caching on success) and parses the body as JSON, then YAML.
///
/// Returns `None` when no HTTP client is available, when the wall-clock budget
/// is exhausted, when the fetch fanout cap is reached, when SSRF validation
/// rejects the URL, when the request fails, when the status is non-2xx, when
/// the response body exceeds the configured size cap, or when the body parses
/// as neither JSON nor YAML — in every case the caller leaves the original
/// `$ref` untouched and emits a `tracing::warn!` describing the failure mode.
///
/// `redirect::Policy::none()` is set on the client builder upstream so a 3xx
/// response surfaces directly here (`is_success` returns false) rather than
/// auto-following into a different host — this is what stops the
/// `attacker.com → 169.254.169.254` redirect pivot that the R6 audit flagged.
#[cfg(not(target_arch = "wasm32"))]
fn fetch_remote_ref(
    url: &str,
    cache: &mut HashMap<String, Value>,
    client: Option<&reqwest::blocking::Client>,
    start: Instant,
) -> Option<Value> {
    if let Some(hit) = cache.get(url) {
        return Some(hit.clone());
    }
    if start.elapsed() > REMOTE_REF_WALL_TIMEOUT {
        tracing::warn!(
            url = url,
            "remote $ref skipped: wall-clock budget exhausted"
        );
        return None;
    }
    if cache.len() >= REMOTE_REF_MAX_FETCHES {
        tracing::warn!(
            url = url,
            limit = REMOTE_REF_MAX_FETCHES,
            "remote $ref skipped: fetch fanout limit reached"
        );
        return None;
    }
    if let Err(reason) = validate_remote_url(url) {
        tracing::warn!(
            url = url,
            reason = reason,
            "remote $ref rejected by SSRF guard"
        );
        return None;
    }
    let client = match client {
        Some(c) => c,
        None => {
            tracing::warn!(url = url, "remote $ref skipped: HTTP client unavailable");
            return None;
        }
    };
    let resp = match client.get(url).send() {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(url = url, error = %e, "remote $ref fetch failed");
            return None;
        }
    };
    if !resp.status().is_success() {
        tracing::warn!(url = url, status = %resp.status(), "remote $ref returned non-2xx");
        return None;
    }
    if let Some(declared) = resp.content_length() {
        if declared as usize > REMOTE_REF_MAX_BODY_BYTES {
            tracing::warn!(
                url = url,
                content_length = declared,
                limit = REMOTE_REF_MAX_BODY_BYTES,
                "remote $ref rejected: content-length exceeds cap"
            );
            return None;
        }
    }
    let capped = match read_capped(resp, REMOTE_REF_MAX_BODY_BYTES) {
        Ok(b) => b,
        Err(ReadCappedError::ExceedsCap) => {
            tracing::warn!(
                url = url,
                limit = REMOTE_REF_MAX_BODY_BYTES,
                "remote $ref rejected: streamed body exceeds cap"
            );
            return None;
        }
        Err(ReadCappedError::Io(e)) => {
            tracing::warn!(url = url, error = %e, "remote $ref body read failed");
            return None;
        }
    };
    let body = match String::from_utf8(capped) {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(url = url, error = %e, "remote $ref body was not valid utf-8");
            return None;
        }
    };
    let parsed = parse_remote_ref_body(&body)?;
    cache.insert(url.to_string(), parsed.clone());
    Some(parsed)
}

/// Parses a fetched remote `$ref` response body into a `serde_json::Value`.
///
/// Tries strict JSON first via `serde_json::from_str` and, on failure, falls
/// back to YAML via `serde_yaml::from_str`. The fallback is what lets a remote
/// `$ref` resolve when the server returns `application/yaml` or `text/yaml`.
/// Returns `None` when the body parses as neither.
///
/// Exposed at crate level so the loader integration tests can exercise the
/// JSON-then-YAML dispatch without standing up a real HTTP server (which the
/// SSRF guard would reject for loopback addresses anyway).
#[cfg(not(target_arch = "wasm32"))]
pub fn parse_remote_ref_body(body: &str) -> Option<Value> {
    serde_json::from_str::<Value>(body)
        .ok()
        .or_else(|| serde_yaml::from_str::<Value>(body).ok())
}

/// Failure modes for [`read_capped`].
///
/// `ExceedsCap` is the streaming-DoS guard signal: returned the moment the
/// cumulative byte count crosses the supplied cap, before any further
/// allocation. `Io` propagates the underlying read failure verbatim so the
/// caller can include it in a `tracing::warn!`.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug, thiserror::Error)]
pub enum ReadCappedError {
    /// The reader produced more than `cap` bytes; the read was aborted.
    #[error("stream exceeds cap")]
    ExceedsCap,
    /// The reader returned an I/O error before the cap was reached.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

/// Reads at most `cap` bytes from `reader`, aborting as soon as the cumulative
/// count would exceed the cap.
///
/// Unlike a buffer-then-truncate pattern, this never allocates beyond
/// `cap + chunk_size`: chunks are read in fixed-size slices and the running
/// total is checked before each `extend_from_slice`. A malicious server that
/// streams multi-GiB with `Transfer-Encoding: chunked` (and so no advisory
/// `Content-Length`) is rejected within the first 1 MiB rather than after the
/// whole body has been buffered.
///
/// Exposed at crate level so the streaming-cap behaviour can be exercised
/// against an in-memory `Cursor` in unit tests without standing up a real HTTP
/// server (which the SSRF guard would reject for loopback addresses anyway).
#[cfg(not(target_arch = "wasm32"))]
pub fn read_capped<R: std::io::Read>(
    mut reader: R,
    cap: usize,
) -> Result<Vec<u8>, ReadCappedError> {
    let initial = cap.min(16 * 1024);
    let mut buffer: Vec<u8> = Vec::with_capacity(initial);
    let mut chunk = [0u8; 8192];
    loop {
        let n = match reader.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(ReadCappedError::Io(e)),
        };
        if buffer.len() + n > cap {
            return Err(ReadCappedError::ExceedsCap);
        }
        buffer.extend_from_slice(&chunk[..n]);
    }
    Ok(buffer)
}

/// Validates a candidate remote `$ref` URL against SSRF risk classes.
///
/// Parses `raw` via `url::Url`, requires an `http`/`https` scheme, resolves the
/// host through `std::net::ToSocketAddrs` against a dummy port matching the
/// scheme, and rejects the URL if any resolved address falls into a sensitive
/// range: loopback, RFC 1918 private (`10/8`, `172.16/12`, `192.168/16`),
/// link-local IPv4 (`169.254/16` — covers the AWS/GCP/Azure IMDS endpoints),
/// link-local IPv6 (`fe80::/10`), IPv6 unique-local (`fc00::/7`), multicast,
/// unspecified, broadcast, or an IPv4-mapped IPv6 address whose embedded IPv4
/// falls into any of the unsafe IPv4 classes above.
///
/// Returns `Ok(())` on accept and `Err(&'static str)` carrying a short reason
/// suitable for `tracing::warn!` on reject. Callers must invoke this before
/// any network I/O is performed for the URL.
///
/// Exposed so callers integrating their own ref-resolution pipeline can apply
/// the same SSRF guard that `load_spec_with_remote_refs` enforces.
#[cfg(not(target_arch = "wasm32"))]
pub fn validate_remote_url(raw: &str) -> Result<(), &'static str> {
    let parsed = url::Url::parse(raw).map_err(|_| "url parse failed")?;
    let scheme = parsed.scheme();
    if scheme != "http" && scheme != "https" {
        return Err("scheme not http(s)");
    }
    let host = parsed.host_str().ok_or("missing host")?;
    let port = parsed
        .port_or_known_default()
        .unwrap_or(if scheme == "https" { 443 } else { 80 });
    let addrs = (host, port)
        .to_socket_addrs()
        .map_err(|_| "dns resolution failed")?;
    let mut any = false;
    for sa in addrs {
        any = true;
        let ip = sa.ip();
        if is_unsafe_ip(&ip) {
            return Err("resolved to internal/private address");
        }
    }
    if !any {
        return Err("no addresses resolved");
    }
    Ok(())
}

/// Returns true when `ip` falls into any of the IPv4 or IPv6 ranges that must
/// never be reached by remote `$ref` resolution.
///
/// Covers loopback, multicast, unspecified, IPv4 RFC 1918 private space, IPv4
/// link-local (`169.254/16`), IPv4 broadcast, IPv6 link-local (`fe80::/10`),
/// IPv6 unique-local (`fc00::/7`), and IPv4-mapped IPv6 addresses whose
/// embedded IPv4 itself matches any of the above. The ULA check is a manual
/// first-byte comparison so we do not depend on the unstable
/// `Ipv6Addr::is_unique_local` method.
#[cfg(not(target_arch = "wasm32"))]
fn is_unsafe_ip(ip: &IpAddr) -> bool {
    if ip.is_loopback() || ip.is_multicast() || ip.is_unspecified() {
        return true;
    }
    match ip {
        IpAddr::V4(v4) => is_unsafe_ipv4(v4),
        IpAddr::V6(v6) => {
            if let Some(mapped) = v6.to_ipv4_mapped() {
                if is_unsafe_ipv4(&mapped) {
                    return true;
                }
            }
            if is_ipv6_link_local(v6) {
                return true;
            }
            if is_ipv6_unique_local(v6) {
                return true;
            }
            false
        }
    }
}

/// Returns true when `v4` is loopback, private, link-local, broadcast,
/// multicast, unspecified, or in the IPv4 documentation/benchmark blocks that
/// would be unexpected on the public internet.
#[cfg(not(target_arch = "wasm32"))]
fn is_unsafe_ipv4(v4: &Ipv4Addr) -> bool {
    v4.is_loopback()
        || v4.is_private()
        || v4.is_link_local()
        || v4.is_broadcast()
        || v4.is_multicast()
        || v4.is_unspecified()
}

/// Returns true when `v6` is in the IPv6 link-local prefix `fe80::/10`.
///
/// Implemented as a manual prefix check (first 10 bits == `1111_1110_10`) so we
/// do not depend on the unstable `Ipv6Addr::is_unicast_link_local` method.
#[cfg(not(target_arch = "wasm32"))]
fn is_ipv6_link_local(v6: &Ipv6Addr) -> bool {
    let segs = v6.segments();
    (segs[0] & 0xffc0) == 0xfe80
}

/// Returns true when `v6` is in the IPv6 unique-local prefix `fc00::/7`.
///
/// Manual check on the high byte (`0xfc` or `0xfd`) since the stdlib helper is
/// still unstable.
#[cfg(not(target_arch = "wasm32"))]
fn is_ipv6_unique_local(v6: &Ipv6Addr) -> bool {
    let high = (v6.segments()[0] >> 8) as u8;
    high == 0xfc || high == 0xfd
}

/// Extracts a top-level `x-json-schema-faker` extension object from the spec.
///
/// Returns `serde_json::Value::Null` when the extension is absent. Schemas that
/// set this extension override `chasm-faker` `GenerateOptions` for the entire spec.
pub fn read_jsf_config(spec: &OpenAPI) -> serde_json::Value {
    spec.extensions
        .get("x-json-schema-faker")
        .cloned()
        .unwrap_or(serde_json::Value::Null)
}
