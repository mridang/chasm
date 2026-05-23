//! HTTP method + path-template matching against an OpenAPI spec.
//!
//! Resolves an incoming `(method, path)` to a concrete `Operation`,
//! disambiguates `405 Method Not Allowed` from `404 Not Found`, and
//! supports synthesis of `OPTIONS` / `HEAD` for operations that do not
//! declare them (unless `--strict-method-matching` is set).

use indexmap::IndexMap;
use openapiv3::{OpenAPI, Operation, PathItem, ServerVariable};
use percent_encoding::percent_decode_str;
use std::collections::HashMap;

/// Percent-decodes a single path segment to a UTF-8 string, falling back to the
/// raw segment when the percent-encoded bytes are not valid UTF-8.
///
/// Path segments arrive raw on the wire (`%20`, `%2F`, etc.) and must be
/// decoded before template matching so a template like `/users/{name}` matches
/// `/users/john%20doe` with `name = "john doe"`.
fn decode_segment(segment: &str) -> String {
    match percent_decode_str(segment).decode_utf8() {
        Ok(decoded) => decoded.into_owned(),
        Err(_) => segment.to_string(),
    }
}

/// The result of matching an incoming request path against an OAS3 path template.
#[derive(Debug)]
#[non_exhaustive]
pub struct MatchedOperation<'a> {
    /// The matched OAS3 operation definition.
    pub operation: &'a Operation,
    /// Path parameter values extracted from the URL, keyed by parameter name.
    pub path_params: HashMap<String, String>,
    /// OAS3 path template key (e.g. `"/pets/{petId}"`) under which `operation`
    /// is declared in [`OpenAPI::paths`]. Carried alongside the match so
    /// callers that need the template (request validation lookup of
    /// path-item-level parameters, metric labelling, span fields) avoid a
    /// second walk over `spec.paths`.
    pub path_template: &'a str,
}

/// Outcome of a router lookup, distinguishing "no path" from "path exists but wrong method".
#[derive(Debug)]
#[non_exhaustive]
pub enum RouteMatch<'a> {
    /// A path template matched and the requested method has an operation defined.
    Operation(MatchedOperation<'a>),
    /// A path template matched but no operation exists for the requested HTTP method.
    ///
    /// Carries the comma-separated, sorted-ascending list of methods declared
    /// on the matched path so the caller can emit a conformant `Allow` header
    /// on the 405 response.
    MethodNotAllowed(String),
    /// A path template matched, the request method was OPTIONS, and the path
    /// item did not declare an explicit `options` operation. The router
    /// synthesises a 200 response carrying an `Allow:` header listing every
    /// HTTP method declared on the matched path. The contained string is the
    /// comma-separated, sorted-ascending list of allowed methods, e.g.
    /// `"GET, POST"`.
    SynthesisedOptions(String),
    /// No path template matched the request URL.
    NotFound,
}

/// Routes a request to a [`RouteMatch`], surfacing method-not-allowed as a distinct case.
///
/// Literal path segments beat parameterised ones; when two templates have the same
/// literal count, the first one in document order wins. The first server URL from
/// `spec.servers`, if present, has its path component stripped from `path` on a
/// best-effort basis before matching. A trailing slash is normalised away unless
/// the request targets the root.
///
/// HEAD requests that don't have a dedicated `head` operation fall through to
/// the path item's `get` operation, mirroring RFC 9110 §9.3.2: HEAD is
/// semantically identical to GET, only the body is suppressed. Callers are
/// responsible for stripping the body downstream.
///
/// OPTIONS requests that don't have a dedicated `options` operation produce a
/// [`RouteMatch::SynthesisedOptions`] outcome carrying the comma-separated list
/// of methods defined on the matched path, so the caller can emit a 200
/// response with an `Allow:` header listing every actual method on the path.
pub fn route_request<'a>(spec: &'a OpenAPI, method: &str, path: &str) -> RouteMatch<'a> {
    route_request_with_strict(spec, method, path, false)
}

/// Variant of [`route_request`] that disables the implicit synthesis of `HEAD`
/// (mirror of `GET`) and `OPTIONS` (preflight) responses when `strict` is true.
///
/// In strict mode neither method is auto-handled: paths that do not declare an
/// explicit `head` or `options` operation produce
/// [`RouteMatch::MethodNotAllowed`] instead of falling back to GET or
/// synthesising a CORS-shaped envelope. The permissive default remains the
/// historical chasm behaviour.
pub fn route_request_with_strict<'a>(
    spec: &'a OpenAPI,
    method: &str,
    path: &str,
    strict: bool,
) -> RouteMatch<'a> {
    let result = route_request_inner(spec, method, path, strict);
    let kind = match &result {
        RouteMatch::Operation(_) => "operation",
        RouteMatch::MethodNotAllowed(_) => "method_not_allowed",
        RouteMatch::SynthesisedOptions(_) => "synthesised_options",
        RouteMatch::NotFound => "not_found",
    };
    tracing::debug!(method, path, strict, result = ?kind, "routed");
    result
}

fn route_request_inner<'a>(
    spec: &'a OpenAPI,
    method: &str,
    path: &str,
    strict: bool,
) -> RouteMatch<'a> {
    let upper = method.to_ascii_uppercase();

    let mut best: Option<(usize, MatchedOperation<'a>)> = None;
    let mut best_path_item: Option<&'a PathItem> = None;
    let mut method_not_allowed_seen = false;

    for (template, path_ref) in &spec.paths.paths {
        let path_item = match path_ref.as_item() {
            Some(item) => item,
            None => continue,
        };
        let candidates = candidate_paths_for_operation(spec, path_item, &upper, path);
        for candidate in &candidates {
            let normalised = normalise_trailing_slash(candidate);
            if let Some(params) = match_template(template, &normalised) {
                let literal_count = count_literal_segments(template);
                let operation = match get_operation(path_item, &upper) {
                    Some(op) => op,
                    None if upper == "HEAD" && !strict => match path_item.get.as_ref() {
                        Some(op) => op,
                        None => {
                            method_not_allowed_seen = true;
                            if best_path_item.is_none() {
                                best_path_item = Some(path_item);
                            }
                            continue;
                        }
                    },
                    None => {
                        method_not_allowed_seen = true;
                        if best_path_item.is_none() {
                            best_path_item = Some(path_item);
                        }
                        continue;
                    }
                };
                let is_better = best
                    .as_ref()
                    .map(|(best_count, _)| literal_count > *best_count)
                    .unwrap_or(true);
                if is_better {
                    best = Some((
                        literal_count,
                        MatchedOperation {
                            operation,
                            path_params: params,
                            path_template: template.as_str(),
                        },
                    ));
                }
                break;
            }
        }
    }

    match best {
        Some((_, m)) => RouteMatch::Operation(m),
        None if upper == "OPTIONS" && !strict && best_path_item.is_some() => {
            let allow = allow_header_for_path_item(best_path_item.unwrap());
            RouteMatch::SynthesisedOptions(allow)
        }
        None if method_not_allowed_seen => {
            let allow = best_path_item
                .map(|item| allow_header_for_path_item_with_strict(item, strict))
                .unwrap_or_default();
            RouteMatch::MethodNotAllowed(allow)
        }
        None => RouteMatch::NotFound,
    }
}

/// Returns the comma-separated `Allow:` header value listing every HTTP method
/// declared on the given path item, sorted ascending for stable output.
///
/// Used when synthesising an OPTIONS response for a path that does not declare
/// its own `options` operation; the resulting string is what the caller should
/// emit verbatim as the `Allow` header.
fn allow_header_for_path_item(item: &PathItem) -> String {
    allow_header_for_path_item_with_strict(item, false)
}

/// Variant of [`allow_header_for_path_item`] that suppresses the implicit
/// `HEAD` advertisement when `strict` is true.
///
/// Without strict matching, the engine mirrors `GET` to satisfy `HEAD`
/// requests, so the `Allow` header advertises `HEAD` whenever `GET` is
/// declared. In strict mode the engine refuses to synthesise `HEAD`, so the
/// header must only list methods actually declared on the path item.
fn allow_header_for_path_item_with_strict(item: &PathItem, strict: bool) -> String {
    let mut methods: Vec<&'static str> = Vec::new();
    if item.get.is_some() {
        methods.push("GET");
    }
    if item.put.is_some() {
        methods.push("PUT");
    }
    if item.post.is_some() {
        methods.push("POST");
    }
    if item.delete.is_some() {
        methods.push("DELETE");
    }
    if item.options.is_some() {
        methods.push("OPTIONS");
    }
    if item.head.is_some() || (!strict && item.get.is_some()) {
        methods.push("HEAD");
    }
    if item.patch.is_some() {
        methods.push("PATCH");
    }
    if item.trace.is_some() {
        methods.push("TRACE");
    }
    methods.sort_unstable();
    methods.dedup();
    methods.join(", ")
}

/// Builds the ordered list of candidate paths for a specific operation,
/// honouring the OAS3 `servers[]` override precedence operation > path-item >
/// spec. The operation-level servers override the path-item-level ones, which
/// in turn override the spec-level entries. The raw request path is always
/// retained as a last-resort fallback so requests that bypass any declared
/// base path still match (this is the behaviour chasm shipped with originally).
///
/// Falls back to spec-level `servers[]` when both the operation-level and
/// path-item-level lists are empty.
fn candidate_paths_for_operation(
    spec: &OpenAPI,
    path_item: &PathItem,
    method: &str,
    path: &str,
) -> Vec<String> {
    let operation = get_operation(path_item, method);
    if let Some(op) = operation {
        if !op.servers.is_empty() {
            return candidate_paths_for_servers_list(&op.servers, path);
        }
    }
    if !path_item.servers.is_empty() {
        return candidate_paths_for_servers_list(&path_item.servers, path);
    }
    candidate_paths_for_servers_list(&spec.servers, path)
}

/// Returns the candidate path list derived from an arbitrary `servers[]`
/// slice, applied to the request path. Shared by both the spec-level and
/// operation/path-item-level override resolvers.
fn candidate_paths_for_servers_list(servers: &[openapiv3::Server], path: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut push_unique = |s: String| {
        if !out.iter().any(|existing| existing == &s) {
            out.push(s);
        }
    };

    for server in servers {
        let base = extract_path_from_url(&server.url, &server.variables);
        if base.is_empty() || base == "/" {
            push_unique(path.to_string());
            continue;
        }
        match path.strip_prefix(&base) {
            Some("") => push_unique("/".to_string()),
            Some(rest) if rest.starts_with('/') => push_unique(rest.to_string()),
            Some(rest) => push_unique(format!("/{}", rest)),
            None => push_unique(path.to_string()),
        }
    }

    if out.is_empty() {
        out.push(path.to_string());
    }
    out
}

/// Extracts the path component from a possibly-absolute server URL, expanding
/// declared server-variable defaults across the entire URL.
///
/// Server templating is supported in the scheme/authority portion as well as
/// the path (e.g. `{scheme}://api.{region}.example.com/{stage}`). The full URL
/// is expanded against the server's `variables` map first, and only then is
/// the scheme/authority stripped to isolate the path component. Placeholders
/// referring to variables that are not declared in the map are left untouched
/// and a warning is logged.
fn extract_path_from_url(
    url: &str,
    variables: &Option<IndexMap<String, ServerVariable>>,
) -> String {
    let expanded = expand_server_variables(url, variables);

    if let Some(idx) = expanded.find("://") {
        let after = &expanded[idx + 3..];
        match after.find('/') {
            Some(p) => after[p..].trim_end_matches('/').to_string(),
            None => String::new(),
        }
    } else if expanded.starts_with('/') {
        expanded.trim_end_matches('/').to_string()
    } else {
        String::new()
    }
}

/// Substitutes `{var}` placeholders inside `input` against the supplied
/// server-variable map, using each variable's `default` value as the
/// replacement.
///
/// Variables that are referenced but not declared in the map are left
/// untouched in the output and trigger a `tracing::warn!` line so spec
/// authors notice missing declarations. The replacement is non-recursive:
/// a default value that itself contains `{...}` placeholders is inserted
/// verbatim.
fn expand_server_variables(
    input: &str,
    variables: &Option<IndexMap<String, ServerVariable>>,
) -> String {
    if !input.contains('{') {
        return input.to_string();
    }
    let mut out = String::with_capacity(input.len());
    let bytes = input.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        let c = bytes[i] as char;
        if c == '{' {
            let mut j = i + 1;
            while j < bytes.len() && bytes[j] as char != '}' {
                j += 1;
            }
            if j < bytes.len() {
                let name = &input[i + 1..j];
                match variables.as_ref().and_then(|v| v.get(name)) {
                    Some(var) => out.push_str(&var.default),
                    None => {
                        tracing::warn!(
                            variable = name,
                            "server URL references undeclared variable {{{}}}; leaving literal",
                            name
                        );
                        out.push_str(&input[i..=j]);
                    }
                }
                i = j + 1;
            } else {
                out.push(c);
                i += 1;
            }
        } else {
            out.push(c);
            i += 1;
        }
    }
    out
}

/// Removes a single trailing `/` from `path`, except when `path` is the root `/`.
fn normalise_trailing_slash(path: &str) -> String {
    if path.len() > 1 && path.ends_with('/') {
        path[..path.len() - 1].to_string()
    } else {
        path.to_string()
    }
}

/// Returns path parameter bindings when `actual` matches `template`, or `None`.
///
/// Template segments wrapped in `{…}` bind to any non-empty path segment. Segments
/// containing `{name}` as part of a larger literal (e.g. `users-{id}`) match any
/// suffix or middle text that satisfies the surrounding literal parts. All other
/// segments must match literally (case-sensitive).
///
/// A cheap segment-count check runs first via `bytes().filter(...).count()` so
/// path/template mismatches short-circuit before allocating any per-segment
/// `Vec<&str>` or percent-decoded `String`. The percent-decode is also
/// short-circuited for segments without any `%` byte, eliminating the
/// `Cow::Owned` allocation that otherwise fired for every plain path segment.
fn match_template(template: &str, actual: &str) -> Option<HashMap<String, String>> {
    let t_count = template.bytes().filter(|b| *b == b'/').count();
    let a_count = actual.bytes().filter(|b| *b == b'/').count();
    if t_count != a_count {
        return None;
    }

    let mut params = HashMap::new();
    for (t, a) in template.split('/').zip(actual.split('/')) {
        let decoded = if a.as_bytes().contains(&b'%') {
            decode_segment(a)
        } else {
            a.to_string()
        };
        if !match_segment(t, decoded.as_str(), &mut params) {
            return None;
        }
    }
    Some(params)
}

/// Matches a single path segment template against an actual segment, populating `params`.
///
/// A pure `{name}` segment captures the whole actual segment. A mixed segment such
/// as `users-{id}` is matched by anchoring the literal prefix and suffix and
/// capturing everything in between.
fn match_segment(template: &str, actual: &str, params: &mut HashMap<String, String>) -> bool {
    if !template.contains('{') {
        return template == actual;
    }
    if template.starts_with('{')
        && template.ends_with('}')
        && !template[1..template.len() - 1].contains('{')
    {
        let name = &template[1..template.len() - 1];
        if actual.is_empty() {
            return false;
        }
        params.insert(name.to_string(), actual.to_string());
        return true;
    }
    match_mixed_segment(template, actual, params)
}

/// Matches a segment template containing one or more `{name}` markers mixed with literals.
///
/// Splits the template around brace markers, then walks the actual segment and
/// advances a cursor across each literal and capture in turn. Adjacent captures
/// are not supported and will fail to match.
fn match_mixed_segment(template: &str, actual: &str, params: &mut HashMap<String, String>) -> bool {
    let parts = split_template_parts(template);
    let mut cursor = 0usize;
    let mut idx = 0usize;
    while idx < parts.len() {
        match &parts[idx] {
            TemplatePart::Literal(lit) => {
                if actual[cursor..].starts_with(lit.as_str()) {
                    cursor += lit.len();
                    idx += 1;
                } else {
                    return false;
                }
            }
            TemplatePart::Param(name) => {
                let next_literal = parts.get(idx + 1);
                match next_literal {
                    Some(TemplatePart::Literal(lit)) => {
                        let search_from = cursor;
                        let found = actual[search_from..].find(lit.as_str());
                        match found {
                            Some(off) if off > 0 => {
                                let captured = &actual[cursor..cursor + off];
                                params.insert(name.clone(), captured.to_string());
                                cursor += off;
                                idx += 1;
                            }
                            _ => return false,
                        }
                    }
                    Some(TemplatePart::Param(_)) => return false,
                    None => {
                        let captured = &actual[cursor..];
                        if captured.is_empty() {
                            return false;
                        }
                        params.insert(name.clone(), captured.to_string());
                        cursor = actual.len();
                        idx += 1;
                    }
                }
            }
        }
    }
    cursor == actual.len()
}

/// Internal node type for a parsed segment template.
enum TemplatePart {
    /// Literal text that must match verbatim.
    Literal(String),
    /// Named capture between `{` and `}`.
    Param(String),
}

/// Splits a segment template like `users-{id}-v{version}` into ordered literal and parameter parts.
fn split_template_parts(template: &str) -> Vec<TemplatePart> {
    let mut parts = Vec::new();
    let bytes = template.as_bytes();
    let mut i = 0usize;
    let mut buf = String::new();
    while i < bytes.len() {
        let c = bytes[i] as char;
        if c == '{' {
            if !buf.is_empty() {
                parts.push(TemplatePart::Literal(std::mem::take(&mut buf)));
            }
            let mut j = i + 1;
            while j < bytes.len() && bytes[j] as char != '}' {
                j += 1;
            }
            if j < bytes.len() {
                let name = template[i + 1..j].to_string();
                parts.push(TemplatePart::Param(name));
                i = j + 1;
            } else {
                buf.push(c);
                i += 1;
            }
        } else {
            buf.push(c);
            i += 1;
        }
    }
    if !buf.is_empty() {
        parts.push(TemplatePart::Literal(buf));
    }
    parts
}

/// Counts path segments that are literal (not path parameters).
fn count_literal_segments(template: &str) -> usize {
    template.split('/').filter(|s| !s.starts_with('{')).count()
}

/// Returns the operation on `item` for the given HTTP method name.
pub fn get_operation<'a>(item: &'a PathItem, method: &str) -> Option<&'a Operation> {
    match method.to_ascii_uppercase().as_str() {
        "GET" => item.get.as_ref(),
        "PUT" => item.put.as_ref(),
        "POST" => item.post.as_ref(),
        "DELETE" => item.delete.as_ref(),
        "OPTIONS" => item.options.as_ref(),
        "HEAD" => item.head.as_ref(),
        "PATCH" => item.patch.as_ref(),
        "TRACE" => item.trace.as_ref(),
        _ => None,
    }
}
