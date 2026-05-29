//! Main `mock()` entry point for chasm-engine.
//!
//! Orchestrates the per-request pipeline: routing -> request validation
//! -> security evaluation -> content negotiation -> response status
//! selection -> example/schema body generation. Returns a `MockResponse`
//! or a structured `MockError`.

use crate::prefer::PreferDirectives;
use crate::router::RouteMatch;
use crate::MockError;
use chasm_faker::{generate, generate_static, GenerateOptions};
use openapiv3::{
    Header, OpenAPI, ParameterSchemaOrContent, ReferenceOr, Response, Schema, StatusCode,
};
use serde_json::Value;
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;

/// Cheap structural fingerprint of an `OpenAPI` spec used as a secondary
/// cache-validity check on top of the raw pointer address.
///
/// The pointer alone is insufficient because once an owning `OpenAPI` is
/// dropped, the allocator can return the same address for an unrelated spec,
/// and a pointer-only cache would then hand back stale JSON. This fingerprint
/// captures inexpensive identifying fields (title, version, path count, top
/// path keys) so two specs with non-trivially-different shape always miss the
/// cache, while two requests against the same loaded spec always hit it.
///
/// Stored as a single `u64` hash so the cache-validity check is a single
/// integer compare and no `String::clone` is incurred on the request hot path.
type SpecFingerprint = u64;

/// Cached spec JSON entry: `(spec pointer, fingerprint, root JSON, $defs JSON)`.
type SpecCacheEntry = (usize, SpecFingerprint, Arc<Value>, Arc<Value>);

fn spec_fingerprint(spec: &OpenAPI) -> SpecFingerprint {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    spec.info.title.hash(&mut h);
    spec.info.version.hash(&mut h);
    spec.openapi.hash(&mut h);
    spec.paths.paths.len().hash(&mut h);
    if let Some((k, _)) = spec.paths.paths.iter().next() {
        k.hash(&mut h);
    }
    if let Some((k, _)) = spec.paths.paths.iter().last() {
        k.hash(&mut h);
    }
    h.finish()
}

thread_local! {
    /// Single-slot, per-thread cache of `(spec pointer, fingerprint, root
    /// JSON, $defs JSON)`.
    ///
    /// `serde_json::to_value(spec)` is invoked for every `$ref` resolution on
    /// every request, and the resulting JSON tree is the same for the lifetime
    /// of a given `OpenAPI` instance. A request typically resolves multiple
    /// refs against the same spec, so a one-entry "most recent spec" cache is
    /// enough to eliminate the repeated serialization without growing
    /// unbounded.
    ///
    /// The key combines the raw `*const OpenAPI` address with a structural
    /// fingerprint (see `spec_fingerprint`). The address fast-paths the
    /// extremely common "same loaded spec, many requests" case; the
    /// fingerprint guards against allocator reuse handing back a stale entry
    /// for a different spec at the same address (observed in test suites that
    /// load many small specs sequentially on one thread).
    static SPEC_JSON_CACHE: RefCell<Option<SpecCacheEntry>> =
        const { RefCell::new(None) };
}

/// Returns the cached `(root JSON, $defs JSON)` pair for `spec`, computing it
/// on the first call (or after the cached entry has been evicted) and reusing
/// it across subsequent calls on the same thread.
///
/// Errors from `serde_json::to_value` propagate as `MockError::SpecSerialization`.
fn cached_spec_root_and_defs(spec: &OpenAPI) -> Result<(Arc<Value>, Arc<Value>), MockError> {
    let key = spec as *const OpenAPI as usize;
    let fingerprint = spec_fingerprint(spec);
    SPEC_JSON_CACHE.with(|cell| {
        if let Some((cached_key, cached_fp, root, defs)) = cell.borrow().as_ref() {
            if *cached_key == key && *cached_fp == fingerprint {
                return Ok((root.clone(), defs.clone()));
            }
        }
        let root_value =
            serde_json::to_value(spec).map_err(|e| MockError::SpecSerialization(e.to_string()))?;
        let defs_value = root_value
            .pointer("/components/schemas")
            .cloned()
            .unwrap_or(Value::Object(serde_json::Map::new()));
        let root = Arc::new(root_value);
        let defs = Arc::new(defs_value);
        *cell.borrow_mut() = Some((key, fingerprint, root.clone(), defs.clone()));
        Ok((root, defs))
    })
}

/// Describes an incoming HTTP request for mock resolution.
///
/// `headers` and `query` are flat `HashMap`s, so callers that observe a
/// repeated name on the wire must collapse the duplicate values into a single
/// entry. The chasm-server caller follows RFC 7230 §3.2.2 and joins repeated
/// values with `", "` for header names where comma-joining is safe, and keeps
/// the first occurrence for header names where the value can itself contain
/// commas (`Cookie`, `Set-Cookie`, `Authorization`, `WWW-Authenticate`, and
/// `Proxy-Authenticate`). Query parameters that repeat are likewise joined
/// with `","`. Downstream consumers see only the joined string and cannot
/// recover the original separator, but the join keeps validation correct for
/// the OpenAPI `style=form,explode=true` query convention.
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct MockRequest {
    /// HTTP method in any case (e.g. `"GET"`, `"post"`).
    pub method: String,
    /// URL path (e.g. `"/pets/42"`).
    pub path: String,
    /// Request headers, e.g. `"Prefer"` and `"Accept"`. Duplicate header names
    /// are joined with `", "` (RFC 7230 §3.2.2) by the server adapter except
    /// for names whose values may themselves contain commas (see the
    /// struct-level doc comment).
    ///
    /// Keys are matched case-insensitively by the engine (e.g. `Accept` and
    /// `accept` both resolve to the same header), so the caller MUST
    /// pre-deduplicate keys with the same case-insensitive name. Duplicate
    /// keys with different casing produce non-deterministic value selection
    /// because `HashMap` iteration order is unspecified.
    pub headers: HashMap<String, String>,
    /// Query parameters; the `__code`, `__example`, `__dynamic`, `__seed` keys
    /// are recognised as overrides. Duplicate keys are joined with `","` by
    /// the server adapter to preserve multi-value semantics.
    pub query: HashMap<String, String>,
    /// Parsed JSON body, when the incoming `Content-Type` was
    /// `application/json` and the body parsed successfully. Non-JSON bodies are
    /// left unset and body schema validation is skipped for them.
    pub body: Option<Value>,
}

/// The generated mock HTTP response.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct MockResponse {
    /// HTTP status code to send back.
    pub status: u16,
    /// `Content-Type` value (e.g. `"application/json"`).
    pub content_type: String,
    /// Response body as a JSON value.
    pub body: Value,
    /// Additional response headers derived from the spec, with hop-by-hop and
    /// framing headers (e.g. `Content-Encoding`, `Content-Length`,
    /// `Transfer-Encoding`, `Date`, `Connection`) stripped because chasm does
    /// not actually compute them.
    pub headers: Vec<(String, String)>,
}

impl Default for MockResponse {
    /// Default response carries a `200 OK` status, `application/json` content-type,
    /// empty JSON object body, and no extra headers.
    fn default() -> Self {
        Self {
            status: 200,
            content_type: "application/json".to_string(),
            body: Value::Object(serde_json::Map::new()),
            headers: Vec::new(),
        }
    }
}

/// Controls how mock responses are generated.
///
/// Server-wide defaults live in `MockConfig`; per-request overrides flow in via
/// the `Prefer` header and `__`-prefixed query parameters and are merged into a
/// per-call copy of the default by [`mock`].
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct MockConfig {
    /// When `true` the faker generates values from the schema rather than examples.
    pub dynamic: bool,
    /// When `true` the engine skips the example pipeline entirely and goes straight to generation.
    pub ignore_examples: bool,
    /// Optional seed propagated to the faker for deterministic output.
    pub seed: Option<u64>,
    /// Named example key (matches `examples.<name>` in the spec) to pick when present.
    pub example_key: Option<String>,
    /// Forced HTTP status code; bypasses the lowest-2xx / default fallback chain.
    pub force_code: Option<u16>,
    /// When `true` the engine returns [`MockError::ValidationFailed`] from
    /// `mock()` whenever request validation reports any errors; when `false`
    /// the failures are logged at `warn!` level and the response is still
    /// generated.
    pub errors: bool,
    /// Overrides the json-schema-faker `fillProperties` behaviour when set;
    /// when `None`, the value from the spec's `x-json-schema-faker` block (if
    /// any) is preserved.
    pub fill_properties: Option<bool>,
    /// When `true` the router refuses to synthesise `HEAD` (mirror of `GET`) or
    /// `OPTIONS` (CORS preflight) responses for paths that do not declare those
    /// methods, returning `MethodNotAllowed` instead. Defaults to `false` to
    /// preserve the auto-synthesis behaviour chasm shipped with originally.
    pub strict_method_matching: bool,
    /// When `false` the engine bypasses the security scheme check for this
    /// call, allowing requests through that would otherwise be `401`.
    /// Defaults to `true` (security checks active). Exposed at the request
    /// level via `Prefer: security=false` / `__security=false`.
    pub check_security: bool,
}

impl Default for MockConfig {
    /// Default `MockConfig` matches the safe-by-default posture: examples
    /// pipeline on, request validation off (warn only), security checks on,
    /// HEAD/OPTIONS auto-synthesised, no forced status / example / seed.
    fn default() -> Self {
        Self {
            dynamic: false,
            ignore_examples: false,
            seed: None,
            example_key: None,
            force_code: None,
            errors: false,
            fill_properties: None,
            strict_method_matching: false,
            check_security: true,
        }
    }
}

/// Generates a mock response for `req` against `spec` using `cfg` as the default.
///
/// Per-request overrides are parsed from `req.headers["Prefer"]` and the
/// query parameters (`__code`, `__example`, `__dynamic`, `__seed`) and merged
/// on top of `cfg`. Returns a [`MockError`] when no route matches.
pub fn mock(
    spec: &OpenAPI,
    req: &MockRequest,
    cfg: &MockConfig,
) -> Result<MockResponse, MockError> {
    let directives = PreferDirectives::from_request(&req.headers, &req.query);
    let effective = merge_directives(cfg, &directives);

    let route = crate::router::route_request_with_strict(
        spec,
        &req.method,
        &req.path,
        cfg.strict_method_matching,
    );
    let matched = match route {
        RouteMatch::Operation(m) => m,
        RouteMatch::SynthesisedOptions(allow) => {
            return Ok(MockResponse {
                status: 200,
                content_type: String::new(),
                body: Value::Null,
                headers: vec![("Allow".to_string(), allow)],
            });
        }
        RouteMatch::MethodNotAllowed(allow) => {
            return Err(MockError::MethodNotAllowed {
                method: req.method.clone(),
                path: req.path.clone(),
                allow,
            });
        }
        RouteMatch::NotFound => {
            return Err(MockError::NoRoute {
                method: req.method.clone(),
                path: req.path.clone(),
            });
        }
    };

    tracing::debug!(operation = ?matched.operation.operation_id, "matched operation");

    if cfg.check_security {
        match crate::security::evaluate(spec, matched.operation, req) {
            crate::security::SecurityResult::Authorized => {}
            crate::security::SecurityResult::Unauthorized {
                scheme,
                www_authenticate,
            } => {
                return Err(MockError::Unauthorized {
                    scheme,
                    www_authenticate,
                });
            }
        }
    }

    let path_template = matched.path_template;
    let path_item = spec
        .paths
        .paths
        .get(path_template)
        .and_then(|p| p.as_item());
    let validation_errors = if has_any_validation_target(matched.operation, path_item) {
        crate::validation::validate(
            spec,
            Some(path_template),
            matched.operation,
            &matched.path_params,
            req,
        )
    } else {
        Vec::new()
    };
    if !validation_errors.is_empty() {
        if cfg.errors {
            return Err(MockError::ValidationFailed(validation_errors));
        }
        tracing::debug!(
            error_count = validation_errors.len(),
            first_field = %validation_errors[0].field,
            first_code = %validation_errors[0].code,
            "request validation produced diagnostics (not enforced; --errors disabled)"
        );
    }

    let (status, response) = resolve_status_and_response(spec, matched.operation, &effective)
        .map_err(|e| match e {
            ResolveError::NoResponses => MockError::NoResponseDefined,
            ResolveError::NoResponseForCode(code) => MockError::NoResponseForCode {
                method: req.method.clone(),
                path: req.path.clone(),
                code,
            },
        })?;

    if response.content.is_empty() {
        let headers = collect_response_headers(spec, response, &effective);
        tracing::debug!(status, "response declares no content; emitting empty body");
        return Ok(MockResponse {
            status,
            content_type: String::new(),
            body: Value::Null,
            headers,
        });
    }

    let content_type = match negotiate_content_type(&response.content, &req.headers) {
        ContentNegotiation::Selected(ct) => ct,
        ContentNegotiation::NotAcceptable(acceptable) => {
            return Err(MockError::NotAcceptable { acceptable });
        }
    };
    let media_type = lookup_media_type(&response.content, &content_type);

    let body = build_body(spec, media_type, &content_type, &effective, req)?;
    let headers = collect_response_headers(spec, response, &effective);

    Ok(MockResponse {
        status,
        content_type,
        body,
        headers,
    })
}

/// Returns true when `name` is a hop-by-hop or framing header the server
/// computes itself and therefore must never be emitted from spec headers.
///
/// Comparison is case-insensitive per RFC 7230.
fn is_filtered_header(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "content-encoding" | "content-length" | "transfer-encoding" | "date" | "connection"
    )
}

/// Resolves a `ReferenceOr<Header>` to its underlying `Header`, following
/// `#/components/headers/X` references.
fn resolve_header_ref<'a>(
    spec: &'a OpenAPI,
    header_ref: &'a ReferenceOr<Header>,
) -> Option<&'a Header> {
    match header_ref {
        ReferenceOr::Item(h) => Some(h),
        ReferenceOr::Reference { reference } => {
            let name = reference.strip_prefix("#/components/headers/")?;
            spec.components.as_ref()?.headers.get(name)?.as_item()
        }
    }
}

/// Picks a value for a spec-defined response header using the same priority as
/// the body: inline `example`, then the schema's `example` (when the header
/// uses a schema rather than a content map), then a faker-generated value
/// from the declared schema. The schema-generation fallback fires only when
/// `cfg.ignore_examples` is false or when the header lacks any example —
/// real-world specs (AWS, GitHub) declare response headers as bare
/// `schema: { type: integer }` and expect mocks to still emit a typed value.
/// Returns `None` only when none of the tiers can produce a value (e.g. a
/// `content`-shaped header with no example anywhere).
fn pick_header_value(spec: &OpenAPI, header: &Header, cfg: &MockConfig) -> Option<Value> {
    if let Some(v) = header.example.clone() {
        return Some(v);
    }
    if let ParameterSchemaOrContent::Schema(ReferenceOr::Item(schema)) = &header.format {
        if let Some(v) = schema.schema_data.example.clone() {
            return Some(v);
        }
    }
    if let ParameterSchemaOrContent::Schema(schema_ref) = &header.format {
        return generate_header_value_from_schema(spec, schema_ref, cfg);
    }
    None
}

/// Generates a header value from the declared schema via the faker, honouring
/// `cfg.dynamic` and `cfg.seed`. Returns `None` when the faker fails or the
/// schema cannot be serialised; callers treat that as "skip the header" rather
/// than as a hard error so a single broken schema does not poison the entire
/// response.
fn generate_header_value_from_schema(
    spec: &OpenAPI,
    schema_ref: &ReferenceOr<Schema>,
    cfg: &MockConfig,
) -> Option<Value> {
    let schema_json = schema_ref_to_json(spec, schema_ref).ok()?;
    let jsf_config = crate::loader::read_jsf_config(spec);
    let mut opts = GenerateOptions::default();
    opts.max_depth = 5;
    opts.seed = cfg.seed;
    apply_jsf_config(&mut opts, &jsf_config);
    if let Some(v) = cfg.fill_properties {
        opts.fill_properties = v;
    }
    if cfg.dynamic {
        opts.always_fake_optionals = true;
        return generate(&schema_json, &opts).ok();
    }
    let static_value = generate_static(&schema_json, &opts).ok();
    if header_value_is_meaningful(static_value.as_ref()) {
        return static_value;
    }
    let mut dyn_opts = opts;
    dyn_opts.always_fake_optionals = true;
    generate(&schema_json, &dyn_opts).ok().or(static_value)
}

/// Returns `true` when `value` is non-empty and therefore wire-meaningful as
/// a response header. Static-mode generation returns `Value::String("")` for
/// a bare `type: string` schema, which serialises onto the wire as an empty
/// header that real clients then reject as malformed. Treat that case as
/// "no value" so the caller can fall back to dynamic generation.
fn header_value_is_meaningful(value: Option<&Value>) -> bool {
    match value {
        None => false,
        Some(Value::String(s)) => !s.is_empty(),
        Some(_) => true,
    }
}

/// Renders a header `serde_json::Value` to a wire string, stringifying scalars
/// without surrounding quotes and serialising arrays/objects as JSON.
fn header_value_to_string(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        Value::Null => String::new(),
        _ => value.to_string(),
    }
}

/// Builds the list of response headers to emit, filtering out hop-by-hop and
/// framing headers that chasm does not actually compute. Headers whose example
/// is missing fall through to a faker-driven schema generation so specs that
/// declare typed headers without examples still surface a value.
fn collect_response_headers(
    spec: &OpenAPI,
    response: &Response,
    cfg: &MockConfig,
) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for (name, header_ref) in &response.headers {
        if is_filtered_header(name) {
            continue;
        }
        let header = match resolve_header_ref(spec, header_ref) {
            Some(h) => h,
            None => continue,
        };
        let Some(value) = pick_header_value(spec, header, cfg) else {
            continue;
        };
        out.push((name.clone(), header_value_to_string(&value)));
    }
    out
}

/// Returns a `MockConfig` derived from `cfg` with any [`PreferDirectives`] overrides applied.
fn merge_directives(cfg: &MockConfig, directives: &PreferDirectives) -> MockConfig {
    let mut out = cfg.clone();
    if let Some(d) = directives.dynamic {
        out.dynamic = d;
    }
    if let Some(s) = directives.seed {
        out.seed = Some(s);
    }
    if let Some(c) = directives.code {
        out.force_code = Some(c);
    }
    if let Some(ref e) = directives.example {
        out.example_key = Some(e.clone());
    }
    if let Some(v) = directives.validate {
        out.errors = v;
    }
    if let Some(v) = directives.security {
        out.check_security = v;
    }
    out
}

/// Builds the response body following the example/schema selection priority.
///
/// Priority order:
/// 1. Example pipeline (unless `cfg.ignore_examples` or `cfg.dynamic`):
///    a. `cfg.example_key` against the `examples` map (with `#/components/examples/X` resolution).
///    b. First entry in the `examples` map (with ref resolution).
///    c. Inline `example` field on the media type.
///    d. `schema.example`, if any.
/// 2. Schema-driven generation:
///    a. Faker (`cfg.dynamic == true`).
///    b. `chasm_faker::generate_static` — deterministic type-default values for non-dynamic mode.
/// 3. `Value::Null` when no media type / schema is available.
fn build_body(
    spec: &OpenAPI,
    media_type: Option<&openapiv3::MediaType>,
    content_type: &str,
    cfg: &MockConfig,
    req: &MockRequest,
) -> Result<Value, MockError> {
    if !cfg.ignore_examples && !cfg.dynamic {
        if let Some(name) = cfg.example_key.as_deref() {
            if !example_key_exists(spec, media_type, name) {
                return Err(MockError::ExampleNotFound {
                    content_type: content_type.to_string(),
                    example_key: name.to_string(),
                });
            }
        }
        if let Some(v) = pick_example_from_media_type(spec, media_type, cfg.example_key.as_deref())
        {
            let source = if cfg.example_key.is_some() {
                "example_key"
            } else {
                "first_example|inline|schema_example"
            };
            tracing::debug!(body_source = source, "body picked");
            return Ok(v);
        }
    }

    if let Some(v) = generate_from_media_type(spec, media_type, cfg, req)? {
        let source = if cfg.dynamic { "dynamic" } else { "static" };
        tracing::debug!(body_source = source, "body picked");
        return Ok(v);
    }

    tracing::debug!(body_source = "null", "body picked");
    Ok(Value::Null)
}

/// Returns `true` when the media type defines a resolvable `examples` entry
/// keyed by `name`. Used to surface a `NO_EXAMPLES_DEFINED` error when a
/// requested `Prefer: example=<name>` directive references an example that
/// does not exist in the spec.
fn example_key_exists(
    spec: &OpenAPI,
    media_type: Option<&openapiv3::MediaType>,
    name: &str,
) -> bool {
    let Some(mt) = media_type else { return false };
    mt.examples
        .get(name)
        .and_then(|r| resolve_example_ref(spec, r))
        .is_some()
}

/// Resolves the status code and matching `Response` together.
///
/// Status resolution order:
/// 1. `cfg.force_code` if set.
/// 2. The lowest numeric `2xx` key (numerically sorted, not document-order).
/// 3. The `default` response — returns the numeric code when the matched entry
///    is keyed by a specific code, falling back to `200`.
/// 4. The first response code in the responses map.
/// 5. `200` as a last resort with an empty synthetic response.
///
/// `2XX`/`5XX` range keys are honoured as fallbacks for steps 2 and 4.
fn resolve_status_and_response<'a>(
    spec: &'a OpenAPI,
    operation: &'a openapiv3::Operation,
    cfg: &MockConfig,
) -> Result<(u16, &'a Response), ResolveError> {
    if let Some(code) = cfg.force_code {
        if let Some(resp) = find_response_for_code(spec, operation, code) {
            tracing::trace!(status = code, source = "force_code", "status resolved");
            return Ok((code, resp));
        }
        if let Some(resp) = operation
            .responses
            .default
            .as_ref()
            .and_then(|r| resolve_response_ref(spec, r))
        {
            tracing::trace!(
                status = code,
                source = "force_code+default",
                "status resolved"
            );
            return Ok((code, resp));
        }
        return Err(ResolveError::NoResponseForCode(code));
    }

    if let Some((code, resp)) = lowest_2xx_response(spec, operation) {
        tracing::debug!(status = code, source = "lowest_2xx", "status resolved");
        return Ok((code, resp));
    }

    if let Some(default_ref) = operation.responses.default.as_ref() {
        if let Some(resp) = resolve_response_ref(spec, default_ref) {
            tracing::trace!(status = 200u16, source = "default", "status resolved");
            return Ok((200, resp));
        }
    }

    for (code_key, resp_ref) in &operation.responses.responses {
        let numeric = match code_key {
            StatusCode::Code(n) => *n,
            StatusCode::Range(family) => family * 100,
        };
        if let Some(resp) = resolve_response_ref(spec, resp_ref) {
            tracing::trace!(status = numeric, source = "first", "status resolved");
            return Ok((numeric, resp));
        }
    }

    Err(ResolveError::NoResponses)
}

/// Internal failure mode of [`resolve_status_and_response`].
///
/// Used to distinguish "the route matched but the operation has no usable
/// response definition" from "no route matched at all" so the caller can map
/// each case to the right error envelope.
#[derive(Debug)]
enum ResolveError {
    /// The operation had no responses defined, or none of the defined
    /// responses could be resolved against the spec.
    NoResponses,
    /// The caller forced a specific status code via `Prefer: code=<n>` or
    /// `__code=<n>` but the operation does not declare that code (and no
    /// `default` response is available to fall back to). Surfaced as a
    /// `NO_RESPONSE_DEFINED` 404 envelope; the inner value is the requested
    /// code so the server adapter can include it in the problem detail.
    NoResponseForCode(u16),
}

/// Returns the response for a specific numeric status code, accepting `XXX` range keys as fallbacks.
fn find_response_for_code<'a>(
    spec: &'a OpenAPI,
    operation: &'a openapiv3::Operation,
    code: u16,
) -> Option<&'a Response> {
    let exact_key = StatusCode::Code(code);
    if let Some(r) = operation.responses.responses.get(&exact_key) {
        if let Some(resolved) = resolve_response_ref(spec, r) {
            return Some(resolved);
        }
    }
    let family = code / 100;
    let range_key = StatusCode::Range(family);
    if let Some(r) = operation.responses.responses.get(&range_key) {
        return resolve_response_ref(spec, r);
    }
    None
}

/// Finds the numerically-lowest `2xx` response in the operation, honouring a
/// `2XX` range key as a fallback.
///
/// Tracks the running minimum in a single pass rather than collecting every
/// `2xx` candidate into a `Vec` and sorting; the typical operation declares
/// one or two `2xx` codes so the original sort was pure overhead on the hot
/// path.
fn lowest_2xx_response<'a>(
    spec: &'a OpenAPI,
    operation: &'a openapiv3::Operation,
) -> Option<(u16, &'a Response)> {
    let mut best: Option<(u16, &'a ReferenceOr<Response>)> = None;
    let mut range_2xx: Option<&'a ReferenceOr<Response>> = None;
    for (key, value) in &operation.responses.responses {
        match key {
            StatusCode::Code(n)
                if (200..300).contains(n) && best.map_or(true, |(min, _)| *n < min) =>
            {
                best = Some((*n, value));
            }
            StatusCode::Range(2) => range_2xx = Some(value),
            _ => {}
        }
    }
    if let Some((code, r)) = best {
        if let Some(resp) = resolve_response_ref(spec, r) {
            return Some((code, resp));
        }
    }
    if let Some(r) = range_2xx {
        if let Some(resp) = resolve_response_ref(spec, r) {
            return Some((200, resp));
        }
    }
    None
}

/// Follows a `ReferenceOr<Response>` to the underlying `Response`, resolving `#/components/responses/X` refs.
fn resolve_response_ref<'a>(
    spec: &'a OpenAPI,
    response_ref: &'a ReferenceOr<Response>,
) -> Option<&'a Response> {
    match response_ref {
        ReferenceOr::Item(r) => Some(r),
        ReferenceOr::Reference { reference } => {
            let name = reference.strip_prefix("#/components/responses/")?;
            spec.components.as_ref()?.responses.get(name)?.as_item()
        }
    }
}

/// Returns `true` when `key` has the syntactic shape of a MIME `type/subtype`.
///
/// This is a conservative check: both halves must be non-empty after stripping
/// `;` parameters and trimming. Used to skip malformed content keys such as
/// `"undefined"` rather than crashing.
fn is_plausible_media_key(key: &str) -> bool {
    let bare = strip_media_params(key);
    let mut parts = bare.splitn(2, '/');
    let ty = parts.next().map(str::trim).unwrap_or("");
    let subtype = parts.next().map(str::trim).unwrap_or("");
    !ty.is_empty() && !subtype.is_empty()
}

/// Outcome of media-type negotiation against the response content map.
///
/// `Selected` carries the chosen content key (which the caller looks up in the
/// content map); `NotAcceptable` carries the spec-declared media types in
/// document order so the server adapter can include them in the 406 problem
/// detail. The split lets the engine surface a `NOT_ACCEPTABLE` envelope
/// when the client supplied an explicit `Accept` header that does not
/// intersect any declared range, without conflating that with the fallback
/// path that runs when no `Accept` header was sent at all.
enum ContentNegotiation {
    /// A spec-declared content key was chosen for the response.
    Selected(String),
    /// The client's `Accept` header was set but no declared media type
    /// satisfies any of its ranges. The inner list is the set of declared
    /// media types in spec document order.
    NotAcceptable(Vec<String>),
}

/// Selects a `Content-Type` from the response content map.
///
/// Parses the `Accept` header into media ranges with optional `q=` factors, sorts
/// them by descending quality, and tries each in turn against the spec's content
/// keys. Spec content keys with parameters such as `"application/json; charset=utf-8"`
/// still match a bare `application/json` accept entry. `*/*` and `type/*` wildcards
/// fall back to the first matching key in document order. When no `Accept` header is
/// supplied, the first content key wins (rather than hardcoding `application/json`).
/// Content keys that are not syntactically plausible MIME types
/// (e.g. `"undefined"`) are skipped during iteration. When `Accept` is supplied
/// and none of its ranges intersect any spec-declared media type the result is
/// [`ContentNegotiation::NotAcceptable`] so the caller can emit a 406.
fn negotiate_content_type(
    content: &indexmap::IndexMap<String, openapiv3::MediaType>,
    headers: &HashMap<String, String>,
) -> ContentNegotiation {
    let result = negotiate_content_type_inner(content, headers);
    let accept = headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("accept"))
        .map(|(_, v)| v.as_str());
    match &result {
        ContentNegotiation::Selected(ct) => {
            tracing::debug!(content_type = %ct, accept = ?accept, "negotiated");
        }
        ContentNegotiation::NotAcceptable(acceptable) => {
            tracing::debug!(acceptable = ?acceptable, accept = ?accept, "not acceptable");
        }
    }
    result
}

fn negotiate_content_type_inner(
    content: &indexmap::IndexMap<String, openapiv3::MediaType>,
    headers: &HashMap<String, String>,
) -> ContentNegotiation {
    let valid_keys = || content.keys().filter(|k| is_plausible_media_key(k));

    let accept_raw = headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("accept"))
        .map(|(_, v)| v.as_str());

    let accept = match accept_raw {
        Some(s) => s,
        None => {
            let chosen = valid_keys()
                .next()
                .cloned()
                .unwrap_or_else(|| "application/json".to_string());
            return ContentNegotiation::Selected(chosen);
        }
    };

    let mut ranges: Vec<(String, f64)> = accept
        .split(',')
        .filter_map(parse_accept_entry)
        .filter(|(_, q)| *q > 0.0)
        .collect();
    ranges.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

    for (range, _) in &ranges {
        if range == "*/*" {
            if let Some(ct) = valid_keys().next() {
                return ContentNegotiation::Selected(ct.clone());
            }
            continue;
        }
        if let Some(ct) = valid_keys().find(|k| strip_media_params(k) == range.as_str()) {
            return ContentNegotiation::Selected(ct.clone());
        }
        if let Some(stripped) = range.strip_suffix("/*") {
            let prefix = format!("{}/", stripped);
            if let Some(ct) = valid_keys().find(|k| strip_media_params(k).starts_with(&prefix)) {
                return ContentNegotiation::Selected(ct.clone());
            }
        }
    }

    let declared: Vec<String> = valid_keys().cloned().collect();
    if declared.is_empty() {
        return ContentNegotiation::Selected("application/json".to_string());
    }
    ContentNegotiation::NotAcceptable(declared)
}

/// Looks up a media type entry whose key matches `content_type` ignoring parameters.
///
/// Malformed content keys (those not shaped like `type/subtype`) are skipped so
/// they cannot accidentally satisfy a lookup.
fn lookup_media_type<'a>(
    content: &'a indexmap::IndexMap<String, openapiv3::MediaType>,
    content_type: &str,
) -> Option<&'a openapiv3::MediaType> {
    if is_plausible_media_key(content_type) {
        if let Some(v) = content.get(content_type) {
            return Some(v);
        }
    }
    content
        .iter()
        .filter(|(k, _)| is_plausible_media_key(k))
        .find(|(k, _)| strip_media_params(k) == content_type)
        .map(|(_, v)| v)
}

/// Parses a single `Accept` entry into a `(media_range, q)` pair, defaulting `q` to `1.0`.
fn parse_accept_entry(entry: &str) -> Option<(String, f64)> {
    let entry = entry.trim();
    if entry.is_empty() {
        return None;
    }
    let mut parts = entry.split(';').map(str::trim);
    let media = parts.next()?.to_string();
    let mut q = 1.0_f64;
    for param in parts {
        if let Some(rest) = param.strip_prefix("q=") {
            if let Ok(v) = rest.parse::<f64>() {
                q = v;
            }
        }
    }
    Some((media, q))
}

/// Returns the bare media type with any `;` parameters stripped, trimmed.
fn strip_media_params(media: &str) -> &str {
    media.split(';').next().unwrap_or(media).trim()
}

/// Returns a static example value from the media type object.
///
/// Resolution order:
/// 1. Named example via `example_key` from `examples` (resolves `#/components/examples/X`).
/// 2. First entry in `examples` (resolves refs).
/// 3. Inline `example` field on the media type.
/// 4. `schema.example`, if available on a non-reference schema.
fn pick_example_from_media_type(
    spec: &OpenAPI,
    media_type: Option<&openapiv3::MediaType>,
    example_key: Option<&str>,
) -> Option<Value> {
    let mt = media_type?;

    if !mt.examples.is_empty() {
        if let Some(name) = example_key {
            if let Some(value) = mt
                .examples
                .get(name)
                .and_then(|r| resolve_example_ref(spec, r))
                .and_then(|ex| ex.value.clone())
            {
                return Some(value);
            }
        }
        if let Some(value) = mt
            .examples
            .values()
            .next()
            .and_then(|r| resolve_example_ref(spec, r))
            .and_then(|ex| ex.value.clone())
        {
            return Some(value);
        }
    }

    if let Some(v) = mt.example.clone() {
        return Some(v);
    }

    if let Some(ReferenceOr::Item(schema)) = mt.schema.as_ref() {
        if let Some(v) = schema.schema_data.example.clone() {
            return Some(v);
        }
    }

    None
}

/// Resolves a `ReferenceOr<Example>` to its underlying [`openapiv3::Example`], following `#/components/examples/X` refs.
fn resolve_example_ref<'a>(
    spec: &'a OpenAPI,
    example_ref: &'a ReferenceOr<openapiv3::Example>,
) -> Option<&'a openapiv3::Example> {
    match example_ref {
        ReferenceOr::Item(ex) => Some(ex),
        ReferenceOr::Reference { reference } => {
            let name = reference.strip_prefix("#/components/examples/")?;
            spec.components.as_ref()?.examples.get(name)?.as_item()
        }
    }
}

/// Generates a value from the media type's schema via `chasm_faker`.
///
/// Returns `Ok(None)` when no media type or schema is present, signalling that the
/// caller should fall back to the next priority tier. Honours `cfg.dynamic` and
/// `cfg.seed`. When `cfg.dynamic` is false, delegates to `chasm_faker::generate_static`
/// for deterministic type-default scalars.
fn generate_from_media_type(
    spec: &OpenAPI,
    media_type: Option<&openapiv3::MediaType>,
    cfg: &MockConfig,
    req: &MockRequest,
) -> Result<Option<Value>, MockError> {
    let Some(mt) = media_type else {
        return Ok(None);
    };
    let Some(schema_ref) = mt.schema.as_ref() else {
        return Ok(None);
    };

    let schema_json = schema_ref_to_json(spec, schema_ref)?;
    let jsf_config = crate::loader::read_jsf_config(spec);

    if cfg.dynamic {
        let mut opts = GenerateOptions::default();
        opts.max_depth = 5;
        opts.always_fake_optionals = true;
        opts.seed = cfg.seed;
        apply_jsf_config(&mut opts, &jsf_config);
        if let Some(v) = cfg.fill_properties {
            opts.fill_properties = v;
        }
        let value = generate(&schema_json, &opts).map_err(|source| MockError::Generation {
            method: req.method.clone(),
            path: req.path.clone(),
            source,
        })?;
        return Ok(Some(value));
    }

    let mut opts = GenerateOptions::default();
    opts.max_depth = 5;
    opts.seed = cfg.seed;
    opts.use_examples_value = true;
    opts.use_default_value = true;
    opts.always_fake_optionals = true;
    apply_jsf_config(&mut opts, &jsf_config);
    if let Some(v) = cfg.fill_properties {
        opts.fill_properties = v;
    }
    let value = generate_static(&schema_json, &opts).map_err(|source| MockError::Generation {
        method: req.method.clone(),
        path: req.path.clone(),
        source,
    })?;
    Ok(Some(value))
}

/// Merges a spec-level `x-json-schema-faker` config object into `GenerateOptions`.
///
/// Recognises the keys `minItems`, `maxItems`, `optionalsProbability`, `alwaysFakeOptionals`,
/// `useDefaultValue`, `useExamplesValue`, `requiredOnly`, `fillProperties`,
/// `failOnInvalidTypes`, `failOnInvalidFormat`, and `random` (numeric seed). Unrecognised
/// keys are ignored.
fn apply_jsf_config(opts: &mut GenerateOptions, config: &Value) {
    let map = match config.as_object() {
        Some(m) => m,
        None => return,
    };
    if let Some(v) = map.get("minItems").and_then(|v| v.as_u64()) {
        opts.min_items = Some(v as usize);
    }
    if let Some(v) = map.get("maxItems").and_then(|v| v.as_u64()) {
        opts.max_items = Some(v as usize);
    }
    if let Some(v) = map.get("optionalsProbability").and_then(|v| v.as_f64()) {
        opts.optionals_probability = Some(v);
    }
    if let Some(v) = map.get("alwaysFakeOptionals").and_then(|v| v.as_bool()) {
        opts.always_fake_optionals = v;
    }
    if let Some(v) = map.get("useDefaultValue").and_then(|v| v.as_bool()) {
        opts.use_default_value = v;
    }
    if let Some(v) = map.get("useExamplesValue").and_then(|v| v.as_bool()) {
        opts.use_examples_value = v;
    }
    if let Some(v) = map.get("requiredOnly").and_then(|v| v.as_bool()) {
        opts.required_only = v;
    }
    if let Some(v) = map.get("fillProperties").and_then(|v| v.as_bool()) {
        opts.fill_properties = v;
    }
    if let Some(v) = map.get("failOnInvalidTypes").and_then(|v| v.as_bool()) {
        opts.fail_on_invalid_type = v;
    }
    if let Some(v) = map.get("failOnInvalidFormat").and_then(|v| v.as_bool()) {
        opts.fail_on_invalid_format = v;
    }
    if let Some(v) = map.get("random").and_then(|v| v.as_u64()) {
        opts.seed = Some(v);
    }
}

/// Converts a `ReferenceOr<Schema>` to a plain `serde_json::Value` suitable for the
/// faker, embedding the spec's `components.schemas` map under both `components.schemas`
/// and `$defs` so any `$ref` inside the returned schema (whether a top-level reference
/// or a nested one such as `items: { $ref: ... }`) can be resolved against the value
/// when it is also used as the resolver root.
///
/// The faker resolves `$ref`s against the schema it is given as the root document.
/// Without the embedded components, a nested OpenAPI reference like
/// `#/components/schemas/Pet` inside an inline `type: array` schema would resolve to
/// `None` and the walker would emit `Value::Null`. Embedding both `components.schemas`
/// and `$defs` keeps OpenAPI-style and JSON-Schema-style references resolvable.
///
/// For inline schemas the local schema's serialized form becomes the root; for
/// pure-reference arms a wrapper carrying the reference is returned. The spec's
/// `components.schemas` JSON is fetched from the thread-local `SPEC_JSON_CACHE` so
/// repeated ref resolutions against the same spec share one serialization.
fn schema_ref_to_json(
    spec: &OpenAPI,
    schema_ref: &ReferenceOr<Schema>,
) -> Result<Value, MockError> {
    let (_root, defs) = cached_spec_root_and_defs(spec)?;
    match schema_ref {
        ReferenceOr::Item(schema) => {
            let mut inline = serde_json::to_value(schema)
                .map_err(|e| MockError::SpecSerialization(e.to_string()))?;
            if schema_has_ref(&inline) {
                attach_components(&mut inline, &defs);
            }
            Ok(inline)
        }
        ReferenceOr::Reference { reference } => {
            let mut wrapper = serde_json::json!({ "$ref": reference });
            attach_components(&mut wrapper, &defs);
            Ok(wrapper)
        }
    }
}

/// Reports whether `value` contains any `$ref` key anywhere in its tree.
///
/// Used as a pre-flight before [`attach_components`] so ref-less inline schemas
/// (the common case) skip the deep clone of the components map entirely. A
/// match on the literal key name is sufficient because JSON Schema only treats
/// `$ref` specially at object positions, and any other appearance is benign.
fn schema_has_ref(value: &Value) -> bool {
    match value {
        Value::Object(map) => {
            if map.contains_key("$ref") {
                return true;
            }
            map.values().any(schema_has_ref)
        }
        Value::Array(items) => items.iter().any(schema_has_ref),
        _ => false,
    }
}

/// Inserts the spec's `components.schemas` map under both `components.schemas` and
/// `$defs` on the given JSON value, but only when the value is an object that does
/// not already carry those keys.
///
/// The dual placement keeps OpenAPI references (`#/components/schemas/X`) and
/// JSON-Schema-Draft references (`#/$defs/X`) resolvable against the same root,
/// matching the resolver's pointer navigation. The "don't clobber" guard preserves
/// any caller-supplied components or defs (used by some tests). The shared
/// `defs_copy` local replaces the previous double `defs.clone()` so the
/// (potentially deeply-nested) components map is only cloned once per call in
/// the common no-collision case.
fn attach_components(value: &mut Value, defs: &Value) {
    let map = match value.as_object_mut() {
        Some(m) => m,
        None => return,
    };
    let needs_components = !map.contains_key("components");
    let needs_defs = !map.contains_key("$defs");
    if !needs_components && !needs_defs {
        return;
    }
    let defs_copy = defs.clone();
    if needs_components {
        map.insert(
            "components".to_string(),
            serde_json::json!({ "schemas": defs_copy.clone() }),
        );
    }
    if needs_defs {
        map.insert("$defs".to_string(), defs_copy);
    }
}

/// Cheap pre-flight check that reports whether `op` (combined with its
/// owning `path_item`'s shared parameters) declares anything worth running
/// through [`crate::validation::validate`].
///
/// `validate` clones the operation parameter list, builds a merged-and-deduped
/// view, and dispatches per-location validators even when nothing is declared.
/// Short-circuiting here on the common "operation has no parameters and no
/// request body" shape eliminates that work for the dominant fast-path
/// operation (e.g. simple `GET /pets` style routes) without disturbing the
/// diagnostic stream when there is actually something to validate.
fn has_any_validation_target(
    op: &openapiv3::Operation,
    path_item: Option<&openapiv3::PathItem>,
) -> bool {
    if !op.parameters.is_empty() {
        return true;
    }
    if op.request_body.is_some() {
        return true;
    }
    if let Some(item) = path_item {
        if !item.parameters.is_empty() {
            return true;
        }
    }
    false
}
