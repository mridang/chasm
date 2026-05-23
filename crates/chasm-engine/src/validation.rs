//! Request validation against an Operation's declared schema.
//!
//! Validates path, query, and header parameters as well as JSON request
//! bodies when the server is run with `--errors`. Produces structured
//! diagnostic entries used to build the `422` problem+json envelope.

use crate::engine::MockRequest;
use openapiv3::{
    AdditionalProperties, AnySchema, OpenAPI, Operation, Parameter, ParameterSchemaOrContent,
    PathStyle, QueryStyle, ReferenceOr, RequestBody, Schema, SchemaKind, StringFormat, Type,
    VariantOrUnknownOrEmpty,
};
use serde_json::Value;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::net::{Ipv4Addr, Ipv6Addr};
use std::str::FromStr;
use std::sync::{Arc, Mutex, OnceLock};

/// Maximum number of compiled regexes kept in the per-thread pattern cache
/// before older entries are dropped. Bounded so a pathological spec with
/// thousands of distinct patterns cannot grow this map without limit.
const REGEX_CACHE_CAPACITY: usize = 256;

/// Maximum raw byte length of a `pattern` source we will hand to the regex
/// compiler. Patterns longer than this are treated as unsupported (skipped
/// silently with a `tracing::warn!`) rather than compiled, so a spec
/// declaring a multi-megabyte literal cannot turn the validator into an
/// unbounded allocator on first use.
const MAX_PATTERN_BYTES: usize = 65_536;

thread_local! {
    /// Per-thread compiled-regex cache for `check_pattern`.
    ///
    /// Caching here avoids the per-request cost of `regex::Regex::new` for
    /// patterns that repeat across requests (the common case for spec-defined
    /// patterns on path/query/header/body fields). Compilation failures are
    /// cached as `None` so a broken pattern is not retried on every request.
    static REGEX_CACHE: RefCell<HashMap<String, Option<Arc<regex::Regex>>>> =
        RefCell::new(HashMap::new());
}

/// Returns a compiled `Regex` for `pattern`, consulting the per-thread cache
/// first and inserting the compilation result (success or failure) on miss.
///
/// Returns `None` when the pattern fails to compile; callers treat this as
/// "do not validate" rather than as an error, matching the pre-cache behaviour
/// of `regex::Regex::new(...).ok()`.
fn cached_regex(pattern: &str) -> Option<Arc<regex::Regex>> {
    if pattern.len() > MAX_PATTERN_BYTES {
        tracing::warn!(
            pattern_bytes = pattern.len(),
            limit = MAX_PATTERN_BYTES,
            "regex pattern source exceeds size cap: skipping (treated as unsupported)"
        );
        return None;
    }
    REGEX_CACHE.with(|cell| {
        let mut map = cell.borrow_mut();
        if let Some(entry) = map.get(pattern) {
            return entry.clone();
        }
        if map.len() >= REGEX_CACHE_CAPACITY {
            map.clear();
        }
        let compiled = regex::Regex::new(pattern).ok().map(Arc::new);
        map.insert(pattern.to_string(), compiled.clone());
        compiled
    })
}

/// Indicates which part of the request a [`ValidationError`] applies to.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ValidationLocation {
    /// The error was discovered while validating a path parameter.
    Path,
    /// The error was discovered while validating a query parameter.
    Query,
    /// The error was discovered while validating a request header.
    Header,
    /// The error was discovered while validating the request body.
    Body,
}

impl ValidationLocation {
    /// Returns the lower-case wire string used in the problem-JSON payload.
    pub fn as_str(&self) -> &'static str {
        match self {
            ValidationLocation::Path => "path",
            ValidationLocation::Query => "query",
            ValidationLocation::Header => "header",
            ValidationLocation::Body => "body",
        }
    }
}

/// Severity level for a single [`ValidationError`].
///
/// Carries the severity surfaced on each entry of the `validation` array in
/// the RFC 7807 problem document. Request-level failures are always `Error`
/// because chasm currently only reports blocking failures, but the type is
/// modelled so the engine can surface `Warning` and `Info` levels later
/// without re-shaping the public envelope.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ValidationSeverity {
    /// Hard failure — the request cannot be served as-is.
    Error,
    /// Soft failure — the request is still served but a problem was observed.
    Warning,
    /// Informational — surfaced for diagnostics only.
    Info,
}

impl ValidationSeverity {
    /// Returns the title-cased wire string emitted in problem envelopes
    /// (`"Error"`, `"Warning"`, `"Info"`).
    pub fn as_str(&self) -> &'static str {
        match self {
            ValidationSeverity::Error => "Error",
            ValidationSeverity::Warning => "Warning",
            ValidationSeverity::Info => "Info",
        }
    }
}

/// A single failure discovered while validating an incoming request.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct ValidationError {
    /// Which input area the failure relates to (path, query, header, body).
    pub location: ValidationLocation,
    /// The field name that failed validation, or `"body"` for the request body.
    pub field: String,
    /// Human readable description of the failure.
    pub message: String,
    /// Stable error code derived from the JSON Schema rule that failed
    /// (e.g. `"type"`, `"required"`, `"pattern"`, `"minimum"`).
    pub code: String,
    /// Severity of the failure; defaults to [`ValidationSeverity::Error`].
    pub severity: ValidationSeverity,
}

impl ValidationError {
    /// Constructs an error-severity [`ValidationError`] from the parts a
    /// validator already has in scope, avoiding boilerplate at the call site.
    pub fn new(
        location: ValidationLocation,
        field: impl Into<String>,
        code: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        ValidationError {
            location,
            field: field.into(),
            message: message.into(),
            code: code.into(),
            severity: ValidationSeverity::Error,
        }
    }
}

/// Validates an incoming request against the operation's parameter and body
/// definitions, returning every failure discovered.
///
/// The check is best-effort against the [`Schema`] type from `openapiv3`.
/// Path parameter values supplied in `path_params` are validated through the
/// schema pipeline as strings (coercing to integer/number/boolean as
/// appropriate).
///
/// `path_template` identifies the OAS3 path item that owns `op` (e.g.
/// `"/pets/{petId}"`) and is used to look up path-item-level shared parameters.
/// Callers should pass the exact key the router matched against
/// [`OpenAPI::paths`]. When `path_template` is `None` or does not match any
/// entry in `spec.paths`, path-item-level parameters are not merged. The R10C
/// contract change replaces the older pointer-equality lookup, which silently
/// dropped shared parameters when callers passed a cloned `Operation`.
pub fn validate(
    spec: &OpenAPI,
    path_template: Option<&str>,
    op: &Operation,
    path_params: &HashMap<String, String>,
    req: &MockRequest,
) -> Vec<ValidationError> {
    let mut errors = Vec::new();
    let item_params = path_template.and_then(|t| path_item_parameters_for_template(spec, t));
    let item_len = item_params.map_or(0, |v| v.len());
    if item_len == 0 && op.parameters.is_empty() && op.request_body.is_none() {
        return errors;
    }
    let mut resolved_params: Vec<&Parameter> = Vec::with_capacity(item_len + op.parameters.len());
    if let Some(item_refs) = item_params {
        for p in item_refs {
            if let Some(param) = resolve_parameter(spec, p) {
                resolved_params.push(param);
            }
        }
    }
    for p in &op.parameters {
        if let Some(param) = resolve_parameter(spec, p) {
            resolved_params.push(param);
        }
    }
    let resolved_params = dedupe_parameters(resolved_params);

    for param in resolved_params {
        match param {
            Parameter::Path {
                parameter_data,
                style,
            } => {
                warn_once_unsupported_path_style(op, parameter_data, style);
                validate_path_param(spec, parameter_data, path_params, &mut errors);
            }
            Parameter::Query {
                parameter_data,
                style,
                ..
            } => {
                warn_once_unsupported_query_style(op, parameter_data, style);
                validate_query_param(spec, parameter_data, style, &req.query, &mut errors);
            }
            Parameter::Header { parameter_data, .. } => {
                validate_header_param(spec, parameter_data, &req.headers, &mut errors);
            }
            Parameter::Cookie { .. } => {}
        }
    }

    if let Some(rb_ref) = op.request_body.as_ref() {
        if let Some(rb) = resolve_request_body(spec, rb_ref) {
            validate_body(spec, rb, req, &mut errors);
        }
    }

    errors
}

/// Resolves a `ReferenceOr<Parameter>` to a borrowed [`Parameter`], following
/// `#/components/parameters/X` references.
///
/// Returning a borrow rather than an owned clone matters on the hot path:
/// every request that reaches request validation hits this resolver once per
/// declared parameter, and a clone of [`Parameter`] (which carries a full
/// inline [`Schema`] plus extensions) is far from free. Both arms of the
/// `ReferenceOr` map to data that lives inside `spec`, so the returned slice
/// is bound to its lifetime.
fn resolve_parameter<'a>(
    spec: &'a OpenAPI,
    p: &'a ReferenceOr<Parameter>,
) -> Option<&'a Parameter> {
    match p {
        ReferenceOr::Item(param) => Some(param),
        ReferenceOr::Reference { reference } => {
            let name = reference.strip_prefix("#/components/parameters/")?;
            let components = spec.components.as_ref()?;
            let entry = components.parameters.get(name)?;
            match entry {
                ReferenceOr::Item(p) => Some(p),
                ReferenceOr::Reference { .. } => None,
            }
        }
    }
}

/// Returns the path-item-level `parameters` array for the `PathItem` keyed by
/// `path_template` in `spec.paths`, or `None` when no such path item exists.
///
/// OpenAPI 3 allows parameters to be declared at the path-item level so they
/// apply to every operation under that path (e.g. a `petId` integer path
/// parameter shared across `GET /pets/{petId}`, `DELETE /pets/{petId}`, etc.).
/// Lookup is by exact string key against `spec.paths.paths`, matching how the
/// router selected the path template. This replaces an earlier pointer-equality
/// scan that silently failed for cloned [`Operation`] values.
fn path_item_parameters_for_template<'a>(
    spec: &'a OpenAPI,
    path_template: &str,
) -> Option<&'a Vec<ReferenceOr<Parameter>>> {
    let path_ref = spec.paths.paths.get(path_template)?;
    let item = path_ref.as_item()?;
    Some(&item.parameters)
}

/// Removes duplicate `Parameter` entries that share both their `in` location
/// and their `name`, preferring the operation-level definition over the
/// path-item-level one when both are present.
///
/// OpenAPI 3 §4.7.4 ("Parameter Object") states that an operation-level
/// parameter with the same `name`+`in` as a path-item-level one overrides the
/// shared definition. The merge order in [`validate`] already puts path-item
/// parameters first followed by operation-level; this helper iterates in
/// reverse so the operation-level entry wins ties, then re-reverses to
/// preserve the original surface order for stable error diagnostics.
fn dedupe_parameters<'a>(params: Vec<&'a Parameter>) -> Vec<&'a Parameter> {
    let mut seen: std::collections::HashSet<(&'a str, &'static str)> =
        std::collections::HashSet::new();
    let mut out: Vec<&'a Parameter> = Vec::with_capacity(params.len());
    for param in params.into_iter().rev() {
        let key = parameter_dedupe_key(param);
        if seen.insert(key) {
            out.push(param);
        }
    }
    out.reverse();
    out
}

/// Returns the `(name, location)` key used by [`dedupe_parameters`] so the
/// override rule from OpenAPI 3 §4.7.4 can be applied uniformly across the
/// four parameter locations.
///
/// The name is returned as a borrowed `&str` into the `Parameter` so the
/// dedupe HashSet doesn't have to allocate a fresh `String` per entry on the
/// request hot path.
fn parameter_dedupe_key(param: &Parameter) -> (&str, &'static str) {
    match param {
        Parameter::Path { parameter_data, .. } => (parameter_data.name.as_str(), "path"),
        Parameter::Query { parameter_data, .. } => (parameter_data.name.as_str(), "query"),
        Parameter::Header { parameter_data, .. } => (parameter_data.name.as_str(), "header"),
        Parameter::Cookie { parameter_data, .. } => (parameter_data.name.as_str(), "cookie"),
    }
}

/// Resolves a `ReferenceOr<RequestBody>` to a concrete [`RequestBody`],
/// following `#/components/requestBodies/X` references.
fn resolve_request_body<'a>(
    spec: &'a OpenAPI,
    rb: &'a ReferenceOr<RequestBody>,
) -> Option<&'a RequestBody> {
    match rb {
        ReferenceOr::Item(b) => Some(b),
        ReferenceOr::Reference { reference } => {
            let name = reference.strip_prefix("#/components/requestBodies/")?;
            let components = spec.components.as_ref()?;
            let entry = components.request_bodies.get(name)?;
            match entry {
                ReferenceOr::Item(b) => Some(b),
                ReferenceOr::Reference { .. } => None,
            }
        }
    }
}

/// Process-lifetime set of `(operation, parameter)` pairs for which we have
/// already emitted an "unsupported parameter style" warning. Bounded by the
/// number of parameter declarations in a spec, which is finite per-process,
/// and gated by a `OnceLock` so the first call is the one that initialises
/// it. Used to prevent flooding the log on hot-path requests.
fn unsupported_style_warned() -> &'static Mutex<HashSet<(String, String)>> {
    static CELL: OnceLock<Mutex<HashSet<(String, String)>>> = OnceLock::new();
    CELL.get_or_init(|| Mutex::new(HashSet::new()))
}

/// Inserts `(op_key, param_name)` into the warned set, returning `true` when
/// it was newly inserted (i.e. the caller should emit the warning).
fn record_unsupported_style(op_key: &str, param_name: &str) -> bool {
    let mut guard = match unsupported_style_warned().lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    };
    guard.insert((op_key.to_string(), param_name.to_string()))
}

/// Returns a stable identifier for `op` used as the operation half of the
/// dedupe key in [`record_unsupported_style`]. Prefers `operationId` when
/// declared and falls back to `summary` so two distinct operations without
/// an `operationId` still get separate dedupe entries.
fn operation_dedupe_key(op: &Operation) -> String {
    op.operation_id
        .clone()
        .or_else(|| op.summary.clone())
        .unwrap_or_else(|| "<anonymous>".to_string())
}

/// Emits a one-time `tracing::warn!` when a path parameter declares a style
/// chasm does not implement (`matrix`, `label`). The parameter is still
/// validated as if `style: simple` were declared so existing specs continue
/// to mock; the warning is the new signal that the wire shape is being
/// accepted by relaxation rather than by literal conformance.
fn warn_once_unsupported_path_style(
    op: &Operation,
    parameter_data: &openapiv3::ParameterData,
    style: &PathStyle,
) {
    let label = match style {
        PathStyle::Simple => return,
        PathStyle::Matrix => "matrix",
        PathStyle::Label => "label",
    };
    let op_key = operation_dedupe_key(op);
    if record_unsupported_style(&op_key, &parameter_data.name) {
        tracing::warn!(
            operation = %op_key,
            parameter = %parameter_data.name,
            style = label,
            "unsupported path parameter style; accepting raw segment as if `style: simple` were declared",
        );
    }
}

/// Emits a one-time `tracing::warn!` when a query parameter declares a style
/// chasm does not implement (`pipeDelimited`, `spaceDelimited`). Behaviour
/// mirrors [`warn_once_unsupported_path_style`]; `form` and `deepObject` are
/// natively handled and so are not warned on.
fn warn_once_unsupported_query_style(
    op: &Operation,
    parameter_data: &openapiv3::ParameterData,
    style: &QueryStyle,
) {
    let label = match style {
        QueryStyle::Form | QueryStyle::DeepObject => return,
        QueryStyle::PipeDelimited => "pipeDelimited",
        QueryStyle::SpaceDelimited => "spaceDelimited",
    };
    let op_key = operation_dedupe_key(op);
    if record_unsupported_style(&op_key, &parameter_data.name) {
        tracing::warn!(
            operation = %op_key,
            parameter = %parameter_data.name,
            style = label,
            "unsupported query parameter style; accepting raw value as if `style: form` were declared",
        );
    }
}

/// Validates a path parameter, treating its presence as required because OAS3
/// path parameters are always required.
///
/// Path values arrive as raw strings extracted from the URI; this function
/// hands them to [`validate_string_against_schema`], which coerces to the
/// schema's declared primary type (integer / number / boolean) before running
/// schema validation. When the schema says `type: integer` and the wire value
/// is, e.g., `café` (or any other non-integer string), coercion deliberately
/// falls back to `Value::String`; the downstream type check in
/// [`validate_value`] then produces a `ValidationError` with `code = "type"`
/// against `ValidationLocation::Path`. This is the codepath that prevents
/// `GET /pets/café` from being routed straight through to a 200 when the path
/// parameter `petId` is declared as `integer`.
fn validate_path_param(
    spec: &OpenAPI,
    data: &openapiv3::ParameterData,
    path_params: &HashMap<String, String>,
    errors: &mut Vec<ValidationError>,
) {
    let Some(raw) = path_params.get(&data.name) else {
        errors.push(ValidationError::new(
            ValidationLocation::Path,
            data.name.clone(),
            "required",
            "missing path parameter",
        ));
        return;
    };
    validate_serialised_param_against_schema(
        spec,
        &data.format,
        raw,
        data.explode.unwrap_or(false),
        ValidationLocation::Path,
        &data.name,
        errors,
    );
}

/// Validates a query parameter, applying the `required` flag and then schema
/// constraints when a value was supplied.
///
/// As with [`validate_path_param`], string-shaped query values are routed
/// through [`validate_string_against_schema`] which coerces to the schema's
/// declared primary type before validation. Non-integer values supplied where
/// the schema declares `type: integer` (or non-numeric where the schema is
/// `type: number`, etc.) surface as `ValidationError { code: "type", .. }`
/// against `ValidationLocation::Query`.
///
/// Object-typed parameters declared with `style: deepObject` are decoded from
/// the bracketed wire shape `name[key]=value` into a synthetic JSON object
/// before validation; the simple `name=value` form is still accepted alongside
/// the deep form to match OAS3 §4.7.4 acceptance semantics.
fn validate_query_param(
    spec: &OpenAPI,
    data: &openapiv3::ParameterData,
    style: &QueryStyle,
    query: &HashMap<String, String>,
    errors: &mut Vec<ValidationError>,
) {
    if matches!(style, QueryStyle::DeepObject) && is_object_typed_param(spec, &data.format) {
        validate_deep_object_query_param(spec, data, query, errors);
        return;
    }
    let value = query.get(&data.name);
    match value {
        None => {
            if data.required {
                errors.push(ValidationError::new(
                    ValidationLocation::Query,
                    data.name.clone(),
                    "required",
                    "missing required query parameter",
                ));
            }
        }
        Some(raw) => {
            validate_serialised_param_against_schema(
                spec,
                &data.format,
                raw,
                data.explode.unwrap_or(true),
                ValidationLocation::Query,
                &data.name,
                errors,
            );
        }
    }
}

/// Returns `true` when the parameter's resolved schema is object-typed. Used
/// to gate deepObject decoding to schemas that actually declare an object
/// shape — a `style: deepObject` declaration against a scalar schema is
/// nonsensical and validated like the simple form instead.
fn is_object_typed_param(spec: &OpenAPI, fmt: &ParameterSchemaOrContent) -> bool {
    let schema_ref = match fmt {
        ParameterSchemaOrContent::Schema(s) => s,
        ParameterSchemaOrContent::Content(_) => return false,
    };
    let Some(schema) = resolve_schema(spec, schema_ref) else {
        return false;
    };
    matches!(&schema.schema_kind, SchemaKind::Type(Type::Object(_)))
}

/// Validates a `style: deepObject` query parameter by scanning `query` for
/// keys of the shape `<name>[<key>]` and assembling them into a synthetic JSON
/// object that is then validated against the parameter's schema. Per-property
/// values are coerced through [`coerce_string_to_schema_value`] using each
/// declared property schema so typed fields (`age: integer`) surface
/// `type`-coded errors against the parameter name rather than the inner key.
///
/// When the operator also supplies the simple form (`?filter=foo`), the
/// non-deep value is decoded via the same path the form-shape validator uses.
fn validate_deep_object_query_param(
    spec: &OpenAPI,
    data: &openapiv3::ParameterData,
    query: &HashMap<String, String>,
    errors: &mut Vec<ValidationError>,
) {
    let prefix = format!("{}[", data.name);
    let mut decoded = serde_json::Map::new();
    for (key, value) in query {
        if let Some(rest) = key.strip_prefix(&prefix) {
            if let Some(inner) = rest.strip_suffix(']') {
                if !inner.is_empty() {
                    decoded.insert(inner.to_string(), Value::String(value.clone()));
                }
            }
        }
    }
    let has_deep_entries = !decoded.is_empty();
    let has_simple_entry = query.contains_key(&data.name);

    if !has_deep_entries && !has_simple_entry {
        if data.required {
            errors.push(ValidationError::new(
                ValidationLocation::Query,
                data.name.clone(),
                "required",
                "missing required query parameter",
            ));
        }
        return;
    }

    if !has_deep_entries && has_simple_entry {
        if let Some(raw) = query.get(&data.name) {
            validate_serialised_param_against_schema(
                spec,
                &data.format,
                raw,
                data.explode.unwrap_or(true),
                ValidationLocation::Query,
                &data.name,
                errors,
            );
        }
        return;
    }

    let schema_ref = match &data.format {
        ParameterSchemaOrContent::Schema(s) => s,
        ParameterSchemaOrContent::Content(_) => return,
    };
    let Some(schema) = resolve_schema(spec, schema_ref) else {
        return;
    };
    let coerced = coerce_deep_object_values(spec, schema, decoded);
    validate_value(
        spec,
        schema,
        &coerced,
        ValidationLocation::Query,
        &data.name,
        errors,
    );
}

/// Walks the decoded `<key, string-value>` pairs from a deepObject query
/// parameter and coerces each value against the matching property schema, so
/// typed fields like `age: integer` are validated with the right primary type.
/// Properties not declared on the object schema are left as JSON strings; the
/// downstream validator handles them under the schema's
/// `additionalProperties` rule.
fn coerce_deep_object_values(
    spec: &OpenAPI,
    schema: &Schema,
    decoded: serde_json::Map<String, Value>,
) -> Value {
    let SchemaKind::Type(Type::Object(obj)) = &schema.schema_kind else {
        return Value::Object(decoded);
    };
    let mut out = serde_json::Map::new();
    for (key, raw_value) in decoded {
        let raw_str = match &raw_value {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        };
        let typed = obj
            .properties
            .get(&key)
            .and_then(|prop_ref| resolve_boxed_schema(spec, prop_ref))
            .map(|prop_schema| {
                match coerce_string_to_schema_value(&raw_str, &prop_schema.schema_kind) {
                    Coercion::Value(v) => v,
                    Coercion::IntegerOverflow => Value::String(raw_str.clone()),
                }
            })
            .unwrap_or(raw_value);
        out.insert(key, typed);
    }
    Value::Object(out)
}

/// Returns true when `name` is one of the three reserved HTTP header names
/// (`Accept`, `Content-Type`, `Authorization`) whose declared parameter
/// definitions OpenAPI 3 §4.7.4 explicitly instructs implementations to
/// ignore. Comparison is case-insensitive per RFC 7230 §3.2.
fn is_reserved_header_name(name: &str) -> bool {
    name.eq_ignore_ascii_case("Accept")
        || name.eq_ignore_ascii_case("Content-Type")
        || name.eq_ignore_ascii_case("Authorization")
}

/// Validates a header parameter using a case-insensitive name lookup.
///
/// Header parameters whose name matches one of the three reserved HTTP header
/// names (`Accept`, `Content-Type`, `Authorization`) are ignored, per OpenAPI
/// 3 §4.7.4. For object-typed headers with the default `style: simple` and
/// `explode: false` serialisation, the wire value `a,1,b,2` is decoded into a
/// `{"a":"1","b":"2"}` JSON object before schema validation. For array-typed
/// headers under `style: simple`, the value is split on `,` and each element
/// is coerced against the item schema.
fn validate_header_param(
    spec: &OpenAPI,
    data: &openapiv3::ParameterData,
    headers: &HashMap<String, String>,
    errors: &mut Vec<ValidationError>,
) {
    if is_reserved_header_name(&data.name) {
        return;
    }
    let found = headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(&data.name))
        .map(|(_, v)| v.as_str());
    match found {
        None => {
            if data.required {
                errors.push(ValidationError::new(
                    ValidationLocation::Header,
                    data.name.clone(),
                    "required",
                    "missing required header",
                ));
            }
        }
        Some(raw) => {
            validate_serialised_param_against_schema(
                spec,
                &data.format,
                raw,
                data.explode.unwrap_or(false),
                ValidationLocation::Header,
                &data.name,
                errors,
            );
        }
    }
}

/// Resolves a parameter's schema (skipping `content` parameters) and validates
/// a raw string value against it by coercing to the schema's declared type.
fn validate_string_against_schema(
    spec: &OpenAPI,
    fmt: &ParameterSchemaOrContent,
    raw: &str,
    location: ValidationLocation,
    field: &str,
    errors: &mut Vec<ValidationError>,
) {
    let schema = match fmt {
        ParameterSchemaOrContent::Schema(s) => s,
        ParameterSchemaOrContent::Content(_) => return,
    };
    let Some(schema) = resolve_schema(spec, schema) else {
        return;
    };
    match coerce_string_to_schema_value(raw, &schema.schema_kind) {
        Coercion::Value(v) => validate_value(spec, schema, &v, location, field, errors),
        Coercion::IntegerOverflow => errors.push(ValidationError::new(
            location,
            field,
            "format",
            format!(
                "value '{}' is outside the int64 range [{}, {}]",
                raw,
                i64::MIN,
                i64::MAX,
            ),
        )),
    }
}

/// Resolves a parameter's schema and validates a serialised wire value against
/// it, decoding array and object shapes under the default `style: simple`
/// rules before delegating to [`validate_value`].
///
/// Array schemas: a single comma-separated string is split on `,` and each
/// element is coerced against the item schema. Object schemas under
/// `explode: false` are decoded from `k1,v1,k2,v2,...` into a flat JSON object
/// of string values (an odd token count surfaces as a `type` error). Schemas
/// composed via `anyOf` / `oneOf` are probed branch-by-branch: the first
/// branch whose coercion validates cleanly is accepted, mirroring how OpenAPI
/// expects multi-shape parameters to be resolved. All other shapes fall back
/// to [`validate_string_against_schema`].
fn validate_serialised_param_against_schema(
    spec: &OpenAPI,
    fmt: &ParameterSchemaOrContent,
    raw: &str,
    explode: bool,
    location: ValidationLocation,
    field: &str,
    errors: &mut Vec<ValidationError>,
) {
    let schema_ref = match fmt {
        ParameterSchemaOrContent::Schema(s) => s,
        ParameterSchemaOrContent::Content(_) => return,
    };
    let Some(schema) = resolve_schema(spec, schema_ref) else {
        return;
    };
    match &schema.schema_kind {
        SchemaKind::Type(Type::Array(at)) => {
            let coerced = coerce_array_param(spec, at, raw);
            validate_value(spec, schema, &coerced, location, field, errors);
        }
        SchemaKind::Type(Type::Object(_)) => {
            let coerced = coerce_object_param(raw, explode);
            validate_value(spec, schema, &coerced, location, field, errors);
        }
        SchemaKind::AnyOf { any_of } | SchemaKind::OneOf { one_of: any_of } => {
            if try_validate_composite_param(spec, any_of, raw, explode, location.clone(), field) {
                return;
            }
            validate_string_against_schema(spec, fmt, raw, location, field, errors);
        }
        _ => validate_string_against_schema(spec, fmt, raw, location, field, errors),
    }
}

/// Attempts to coerce `raw` against each branch of an `anyOf` / `oneOf`
/// parameter schema, returning `true` when at least one branch accepts the
/// value cleanly. Used by [`validate_serialised_param_against_schema`] to
/// avoid false rejections when a scalar value is supplied for a schema that
/// also lists an array branch.
fn try_validate_composite_param(
    spec: &OpenAPI,
    branches: &[ReferenceOr<Schema>],
    raw: &str,
    explode: bool,
    location: ValidationLocation,
    field: &str,
) -> bool {
    for branch in branches {
        let Some(resolved) = resolve_schema(spec, branch) else {
            continue;
        };
        let coerced = match &resolved.schema_kind {
            SchemaKind::Type(Type::Array(at)) => coerce_array_param(spec, at, raw),
            SchemaKind::Type(Type::Object(_)) => coerce_object_param(raw, explode),
            _ => match coerce_string_to_schema_value(raw, &resolved.schema_kind) {
                Coercion::Value(v) => v,
                Coercion::IntegerOverflow => continue,
            },
        };
        let mut tmp: Vec<ValidationError> = Vec::new();
        validate_value(spec, resolved, &coerced, location.clone(), field, &mut tmp);
        if tmp.is_empty() {
            return true;
        }
    }
    false
}

/// Coerces a comma-separated path/header value into a JSON array whose
/// elements are typed against the array's `items` schema. Empty input
/// produces an empty array.
fn coerce_array_param(spec: &OpenAPI, at: &openapiv3::ArrayType, raw: &str) -> Value {
    if raw.is_empty() {
        return Value::Array(Vec::new());
    }
    let item_kind = at
        .items
        .as_ref()
        .and_then(|item_ref| resolve_boxed_schema(spec, item_ref))
        .map(|s| s.schema_kind.clone());
    let items: Vec<Value> = raw
        .split(',')
        .map(|piece| match &item_kind {
            Some(kind) => match coerce_string_to_schema_value(piece, kind) {
                Coercion::Value(v) => v,
                Coercion::IntegerOverflow => Value::String(piece.to_string()),
            },
            None => Value::String(piece.to_string()),
        })
        .collect();
    Value::Array(items)
}

/// Coerces a serialised object header/path value into a flat JSON object of
/// string values. With `explode = false`, the wire value is a single
/// comma-separated `k1,v1,k2,v2,...` stream; an odd token count produces an
/// object with an empty value for the trailing key, which downstream schema
/// validation can flag as a missing property. With `explode = true`, the
/// value already arrives as `k1=v1,k2=v2,...` so `=` is used as the key/value
/// separator inside each comma-delimited chunk.
fn coerce_object_param(raw: &str, explode: bool) -> Value {
    let mut map = serde_json::Map::new();
    if raw.is_empty() {
        return Value::Object(map);
    }
    if explode {
        for chunk in raw.split(',') {
            if let Some((k, v)) = chunk.split_once('=') {
                map.insert(k.to_string(), Value::String(v.to_string()));
            }
        }
        return Value::Object(map);
    }
    let parts: Vec<&str> = raw.split(',').collect();
    let mut iter = parts.into_iter();
    while let Some(key) = iter.next() {
        let value = iter.next().unwrap_or("");
        map.insert(key.to_string(), Value::String(value.to_string()));
    }
    Value::Object(map)
}

/// Outcome of converting a raw wire string into a JSON value for schema
/// validation.
///
/// `Value` carries the coerced JSON value to feed to the schema validator,
/// including the fallback case where coercion could not produce a typed value
/// and the original string is passed through (so the validator reports a
/// `type` error). `IntegerOverflow` is a distinct outcome reserved for the
/// case where the raw input is syntactically a valid integer literal but
/// cannot be represented as `i64`; the caller emits a dedicated `format`-coded
/// error rather than misclassifying the failure as a type mismatch.
enum Coercion {
    /// Coercion produced a usable JSON value (typed when possible, otherwise
    /// the original string).
    Value(Value),
    /// The raw input parsed as a base-10 integer that does not fit in `i64`,
    /// meaning the schema's `int64` bound is the real reason for rejection.
    IntegerOverflow,
}

/// Coerces a raw string into a JSON value shaped to match the schema's primary
/// type, falling back to a string value when coercion fails for type-mismatch
/// reasons. When the schema declares `integer` and the input parses as a
/// `i128` outside the `i64` range, the dedicated [`Coercion::IntegerOverflow`]
/// outcome is returned so the caller can emit a `format` error naming the
/// `int64` bound instead of misclassifying the failure as a type error.
fn coerce_string_to_schema_value(raw: &str, kind: &SchemaKind) -> Coercion {
    match primary_type_label(kind) {
        Some("integer") => match raw.parse::<i128>() {
            Ok(n) => {
                if (i64::MIN as i128..=i64::MAX as i128).contains(&n) {
                    Coercion::Value(Value::Number((n as i64).into()))
                } else {
                    Coercion::IntegerOverflow
                }
            }
            Err(_) => Coercion::Value(Value::String(raw.to_string())),
        },
        Some("number") => match raw.parse::<f64>() {
            Ok(n) => Coercion::Value(
                serde_json::Number::from_f64(n)
                    .map(Value::Number)
                    .unwrap_or_else(|| Value::String(raw.to_string())),
            ),
            Err(_) => Coercion::Value(Value::String(raw.to_string())),
        },
        Some("boolean") => match raw {
            "true" => Coercion::Value(Value::Bool(true)),
            "false" => Coercion::Value(Value::Bool(false)),
            _ => Coercion::Value(Value::String(raw.to_string())),
        },
        _ => Coercion::Value(Value::String(raw.to_string())),
    }
}

/// Returns the primary `type` label declared on a schema, considering both the
/// typed and untyped (`AnySchema`) variants.
fn primary_type_label(kind: &SchemaKind) -> Option<&'static str> {
    match kind {
        SchemaKind::Type(Type::String(_)) => Some("string"),
        SchemaKind::Type(Type::Integer(_)) => Some("integer"),
        SchemaKind::Type(Type::Number(_)) => Some("number"),
        SchemaKind::Type(Type::Boolean(_)) => Some("boolean"),
        SchemaKind::Type(Type::Object(_)) => Some("object"),
        SchemaKind::Type(Type::Array(_)) => Some("array"),
        SchemaKind::Any(any) => match any.typ.as_deref() {
            Some("string") => Some("string"),
            Some("integer") => Some("integer"),
            Some("number") => Some("number"),
            Some("boolean") => Some("boolean"),
            Some("object") => Some("object"),
            Some("array") => Some("array"),
            _ => None,
        },
        _ => None,
    }
}

/// Resolves a `ReferenceOr<Schema>` to a concrete schema, following
/// `#/components/schemas/X` references one level deep.
fn resolve_schema<'a>(spec: &'a OpenAPI, s: &'a ReferenceOr<Schema>) -> Option<&'a Schema> {
    match s {
        ReferenceOr::Item(schema) => Some(schema),
        ReferenceOr::Reference { reference } => {
            let name = reference.strip_prefix("#/components/schemas/")?;
            let components = spec.components.as_ref()?;
            let entry = components.schemas.get(name)?;
            match entry {
                ReferenceOr::Item(schema) => Some(schema),
                ReferenceOr::Reference { .. } => None,
            }
        }
    }
}

/// Resolves a `ReferenceOr<Box<Schema>>` to a concrete schema by reusing
/// [`resolve_schema`] on a borrowed projection.
fn resolve_boxed_schema<'a>(
    spec: &'a OpenAPI,
    s: &'a ReferenceOr<Box<Schema>>,
) -> Option<&'a Schema> {
    match s {
        ReferenceOr::Item(schema) => Some(schema.as_ref()),
        ReferenceOr::Reference { reference } => {
            let name = reference.strip_prefix("#/components/schemas/")?;
            let components = spec.components.as_ref()?;
            let entry = components.schemas.get(name)?;
            match entry {
                ReferenceOr::Item(schema) => Some(schema),
                ReferenceOr::Reference { .. } => None,
            }
        }
    }
}

/// Returns the textual `format` keyword from a `StringType`, mapping the typed
/// variants (`date`, `date-time`) back to their wire-form spellings and passing
/// through any `Unknown` value verbatim. Returns `None` when no format was set.
fn string_type_format(format: &VariantOrUnknownOrEmpty<StringFormat>) -> Option<&str> {
    match format {
        VariantOrUnknownOrEmpty::Item(StringFormat::Date) => Some("date"),
        VariantOrUnknownOrEmpty::Item(StringFormat::DateTime) => Some("date-time"),
        VariantOrUnknownOrEmpty::Item(StringFormat::Password) => Some("password"),
        VariantOrUnknownOrEmpty::Item(StringFormat::Byte) => Some("byte"),
        VariantOrUnknownOrEmpty::Item(StringFormat::Binary) => Some("binary"),
        VariantOrUnknownOrEmpty::Unknown(s) => Some(s.as_str()),
        VariantOrUnknownOrEmpty::Empty => None,
    }
}

/// Returns true when `value` matches the JSON Schema / OAS `format` keyword.
///
/// Formats not enumerated here (e.g. `password`) are treated as "not enforced"
/// and accepted unconditionally so that specs which use them purely for
/// documentation do not produce false 422s. `binary` is also treated as a
/// passthrough — it marks a field as binary upload, not a constraint on the
/// string content.
fn matches_format(value: &str, format: &str) -> bool {
    match format {
        "email" => {
            let re = cached_regex(r"^[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}$");
            re.map(|r| r.is_match(value)).unwrap_or(true)
        }
        "uri" | "uri-reference" => {
            let re = cached_regex(r"^[A-Za-z][A-Za-z0-9+.\-]*:");
            re.map(|r| r.is_match(value)).unwrap_or(true)
        }
        "uuid" => {
            let re = cached_regex(
                r"^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$",
            );
            re.map(|r| r.is_match(value)).unwrap_or(true)
        }
        "date-time" => is_valid_rfc3339_date_time(value),
        "date" => is_valid_iso_date(value),
        "time" => is_valid_iso_time(value),
        "ipv4" => Ipv4Addr::from_str(value).is_ok(),
        "ipv6" => Ipv6Addr::from_str(value).is_ok(),
        "hostname" => is_valid_hostname(value),
        "byte" => is_valid_base64(value),
        _ => true,
    }
}

/// Returns true when `s` is a valid RFC 4648 base64 string (canonical alphabet).
///
/// Accepts the standard alphabet (`A-Za-z0-9+/`) with optional `=` padding to
/// the next multiple of 4. Empty strings count as valid (RFC 4648 §10 permits
/// the empty input as the encoding of an empty byte sequence).
fn is_valid_base64(s: &str) -> bool {
    if s.is_empty() {
        return true;
    }
    if s.len() % 4 != 0 {
        return false;
    }
    let bytes = s.as_bytes();
    let mut content_end = bytes.len();
    if bytes[content_end - 1] == b'=' {
        content_end -= 1;
    }
    if content_end > 0 && bytes[content_end - 1] == b'=' {
        content_end -= 1;
    }
    for &b in &bytes[..content_end] {
        let alpha = b.is_ascii_alphanumeric() || b == b'+' || b == b'/';
        if !alpha {
            return false;
        }
    }
    bytes[content_end..].iter().all(|&b| b == b'=')
}

/// Validates a string value against a JSON Schema `format` keyword, pushing a
/// `format`-coded [`ValidationError`] when the value does not satisfy it.
///
/// Acts as a no-op for formats this validator does not enforce, matching the
/// "skip unknown formats" semantics expected by the OAS spec.
fn validate_format(
    value: &str,
    format: &str,
    location: ValidationLocation,
    field: &str,
    errors: &mut Vec<ValidationError>,
) {
    if !matches_format(value, format) {
        errors.push(ValidationError::new(
            location,
            field,
            "format",
            format!("value '{}' is not a valid {}", value, format),
        ));
    }
}

/// Returns true when `s` parses as an RFC 3339 `date-time` literal, mirroring
/// the acceptance criteria of `chrono::DateTime::parse_from_rfc3339`.
///
/// Implemented as a regex check against the canonical RFC 3339 grammar so the
/// validator does not need a date/time crate as a direct dependency.
fn is_valid_rfc3339_date_time(s: &str) -> bool {
    let pattern = r"^\d{4}-\d{2}-\d{2}[Tt]\d{2}:\d{2}:\d{2}(\.\d+)?([Zz]|[+\-]\d{2}:\d{2})$";
    let Some(re) = cached_regex(pattern) else {
        return true;
    };
    if !re.is_match(s) {
        return false;
    }
    let year: u32 = s.get(0..4).and_then(|p| p.parse().ok()).unwrap_or(0);
    let month: u32 = s.get(5..7).and_then(|p| p.parse().ok()).unwrap_or(0);
    let day: u32 = s.get(8..10).and_then(|p| p.parse().ok()).unwrap_or(0);
    let hour: u32 = s.get(11..13).and_then(|p| p.parse().ok()).unwrap_or(0);
    let minute: u32 = s.get(14..16).and_then(|p| p.parse().ok()).unwrap_or(0);
    let second: u32 = s.get(17..19).and_then(|p| p.parse().ok()).unwrap_or(0);
    is_valid_ymd(year, month, day) && hour < 24 && minute < 60 && second <= 60
}

/// Returns true when `s` is a `YYYY-MM-DD` calendar date with valid components.
fn is_valid_iso_date(s: &str) -> bool {
    let re = cached_regex(r"^\d{4}-\d{2}-\d{2}$");
    if !re.map(|r| r.is_match(s)).unwrap_or(false) {
        return false;
    }
    let year: u32 = s.get(0..4).and_then(|p| p.parse().ok()).unwrap_or(0);
    let month: u32 = s.get(5..7).and_then(|p| p.parse().ok()).unwrap_or(0);
    let day: u32 = s.get(8..10).and_then(|p| p.parse().ok()).unwrap_or(0);
    is_valid_ymd(year, month, day)
}

/// Returns true when `s` is an `HH:MM:SS` wall-clock time with valid components.
fn is_valid_iso_time(s: &str) -> bool {
    let re = cached_regex(r"^\d{2}:\d{2}:\d{2}$");
    if !re.map(|r| r.is_match(s)).unwrap_or(false) {
        return false;
    }
    let hour: u32 = s.get(0..2).and_then(|p| p.parse().ok()).unwrap_or(0);
    let minute: u32 = s.get(3..5).and_then(|p| p.parse().ok()).unwrap_or(0);
    let second: u32 = s.get(6..8).and_then(|p| p.parse().ok()).unwrap_or(0);
    hour < 24 && minute < 60 && second <= 60
}

/// Returns true when the (year, month, day) triple is a real Gregorian date.
fn is_valid_ymd(year: u32, month: u32, day: u32) -> bool {
    if !(1..=12).contains(&month) || day == 0 {
        return false;
    }
    let last = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            let leap = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
            if leap {
                29
            } else {
                28
            }
        }
        _ => return false,
    };
    day <= last
}

/// Returns true when `s` matches the RFC 1123 hostname grammar (dot-separated
/// labels of ASCII letters, digits, and hyphens with each label ≤63 characters).
fn is_valid_hostname(s: &str) -> bool {
    let re = cached_regex(r"^[a-zA-Z0-9-]+(\.[a-zA-Z0-9-]+)*$");
    if !re.map(|r| r.is_match(s)).unwrap_or(false) {
        return false;
    }
    s.split('.')
        .all(|label| !label.is_empty() && label.len() <= 63)
}

/// Validates a JSON value against a resolved schema, recording each failure.
///
/// Two OAS 3.1 keyword families are intentionally not enforced here because the
/// underlying `openapiv3` v2 deserialiser drops them during parsing and they
/// never reach this pass:
///
/// - String keywords `contentMediaType` and `contentEncoding`. Equivalent checks
///   would need either a loader-side rewrite or a richer schema model.
/// - Object keywords `dependentRequired` and `dependentSchemas`. Unknown JSON
///   Schema keywords are dropped during parsing, so they cannot be enforced
///   from this validation pass.
fn validate_value(
    spec: &OpenAPI,
    schema: &Schema,
    value: &Value,
    location: ValidationLocation,
    field: &str,
    errors: &mut Vec<ValidationError>,
) {
    if value.is_null() && schema.schema_data.nullable {
        return;
    }
    match &schema.schema_kind {
        SchemaKind::Type(Type::String(st)) => {
            let Some(s) = value.as_str() else {
                errors.push(type_error(location, field, "string"));
                return;
            };
            if !st.enumeration.is_empty() {
                let allowed = st
                    .enumeration
                    .iter()
                    .any(|opt| opt.as_ref().map(|x| x == s).unwrap_or(false));
                if !allowed {
                    errors.push(ValidationError::new(
                        location,
                        field,
                        "enum",
                        format!("value '{}' not in enum", s),
                    ));
                    return;
                }
            }
            if let Some(min) = st.min_length {
                if s.chars().count() < min {
                    errors.push(ValidationError::new(
                        location.clone(),
                        field,
                        "minLength",
                        format!("length below minLength {}", min),
                    ));
                }
            }
            if let Some(max) = st.max_length {
                if s.chars().count() > max {
                    errors.push(ValidationError::new(
                        location.clone(),
                        field,
                        "maxLength",
                        format!("length above maxLength {}", max),
                    ));
                }
            }
            if let Some(pat) = st.pattern.as_deref() {
                check_pattern(s, pat, location.clone(), field, errors);
            }
            if let Some(fmt) = string_type_format(&st.format) {
                validate_format(s, fmt, location, field, errors);
            }
        }
        SchemaKind::Type(Type::Integer(it)) => {
            let Some(n) = value.as_i64() else {
                errors.push(type_error(location, field, "integer"));
                return;
            };
            if matches!(
                &it.format,
                VariantOrUnknownOrEmpty::Item(openapiv3::IntegerFormat::Int32)
            ) && !(i32::MIN as i64..=i32::MAX as i64).contains(&n)
            {
                errors.push(ValidationError::new(
                    location.clone(),
                    field,
                    "format",
                    format!(
                        "value {} is outside the int32 range [{}, {}]",
                        n,
                        i32::MIN,
                        i32::MAX
                    ),
                ));
                return;
            }
            if !it.enumeration.is_empty() {
                let allowed = it
                    .enumeration
                    .iter()
                    .any(|opt| opt.as_ref().map(|x| *x == n).unwrap_or(false));
                if !allowed {
                    errors.push(ValidationError::new(
                        location,
                        field,
                        "enum",
                        format!("value {} not in enum", n),
                    ));
                    return;
                }
            }
            if let Some(min) = it.minimum {
                let ok = if it.exclusive_minimum {
                    n > min
                } else {
                    n >= min
                };
                if !ok {
                    errors.push(ValidationError::new(
                        location.clone(),
                        field,
                        if it.exclusive_minimum {
                            "exclusiveMinimum"
                        } else {
                            "minimum"
                        },
                        format!("value {} below minimum {}", n, min),
                    ));
                }
            }
            if let Some(max) = it.maximum {
                let ok = if it.exclusive_maximum {
                    n < max
                } else {
                    n <= max
                };
                if !ok {
                    errors.push(ValidationError::new(
                        location.clone(),
                        field,
                        if it.exclusive_maximum {
                            "exclusiveMaximum"
                        } else {
                            "maximum"
                        },
                        format!("value {} above maximum {}", n, max),
                    ));
                }
            }
            if let Some(multiple) = it.multiple_of {
                if multiple != 0 && n % multiple != 0 {
                    errors.push(ValidationError::new(
                        location.clone(),
                        field,
                        "multipleOf",
                        format!("value {} is not a multiple of {}", n, multiple),
                    ));
                }
            }
        }
        SchemaKind::Type(Type::Number(nt)) => {
            let Some(n) = value.as_f64() else {
                errors.push(type_error(location, field, "number"));
                return;
            };
            if !nt.enumeration.is_empty() {
                let allowed = nt
                    .enumeration
                    .iter()
                    .any(|opt| opt.as_ref().map(|x| float_eq(*x, n)).unwrap_or(false));
                if !allowed {
                    errors.push(ValidationError::new(
                        location,
                        field,
                        "enum",
                        format!("value {} not in enum", n),
                    ));
                    return;
                }
            }
            if let Some(min) = nt.minimum {
                let ok = if nt.exclusive_minimum {
                    n > min
                } else {
                    n >= min
                };
                if !ok {
                    errors.push(ValidationError::new(
                        location.clone(),
                        field,
                        if nt.exclusive_minimum {
                            "exclusiveMinimum"
                        } else {
                            "minimum"
                        },
                        format!("value {} below minimum {}", n, min),
                    ));
                }
            }
            if let Some(max) = nt.maximum {
                let ok = if nt.exclusive_maximum {
                    n < max
                } else {
                    n <= max
                };
                if !ok {
                    errors.push(ValidationError::new(
                        location.clone(),
                        field,
                        if nt.exclusive_maximum {
                            "exclusiveMaximum"
                        } else {
                            "maximum"
                        },
                        format!("value {} above maximum {}", n, max),
                    ));
                }
            }
            if let Some(multiple) = nt.multiple_of {
                if multiple != 0.0 && !passes_strict_multiple_of_check(n, multiple) {
                    errors.push(ValidationError::new(
                        location.clone(),
                        field,
                        "multipleOf",
                        format!("value {} is not a multiple of {}", n, multiple),
                    ));
                }
            }
        }
        SchemaKind::Type(Type::Boolean(bt)) => {
            let Some(b) = value.as_bool() else {
                errors.push(type_error(location, field, "boolean"));
                return;
            };
            if !bt.enumeration.is_empty() {
                let allowed = bt
                    .enumeration
                    .iter()
                    .any(|opt| opt.as_ref().map(|x| *x == b).unwrap_or(false));
                if !allowed {
                    errors.push(ValidationError::new(
                        location,
                        field,
                        "enum",
                        format!("value {} not in enum", b),
                    ));
                }
            }
        }
        SchemaKind::Type(Type::Object(ot)) => {
            let Some(map) = value.as_object() else {
                errors.push(type_error(location, field, "object"));
                return;
            };
            for required in &ot.required {
                if !map.contains_key(required) {
                    errors.push(ValidationError::new(
                        location.clone(),
                        format!("{}.{}", field, required),
                        "required",
                        "missing required property",
                    ));
                }
            }
            for (key, prop_ref) in &ot.properties {
                if let Some(child) = map.get(key) {
                    if let Some(prop_schema) = resolve_boxed_schema(spec, prop_ref) {
                        validate_value(
                            spec,
                            prop_schema,
                            child,
                            location.clone(),
                            &format!("{}.{}", field, key),
                            errors,
                        );
                    }
                }
            }
            if let Some(AdditionalProperties::Schema(ap)) = ot.additional_properties.as_ref() {
                if let Some(extra_schema) = resolve_schema(spec, ap.as_ref()) {
                    for (key, child) in map {
                        if ot.properties.contains_key(key) {
                            continue;
                        }
                        validate_value(
                            spec,
                            extra_schema,
                            child,
                            location.clone(),
                            &format!("{}.{}", field, key),
                            errors,
                        );
                    }
                }
            }
            if let Some(AdditionalProperties::Any(false)) = ot.additional_properties.as_ref() {
                for key in map.keys() {
                    if !ot.properties.contains_key(key) {
                        errors.push(ValidationError::new(
                            location.clone(),
                            format!("{}.{}", field, key),
                            "additionalProperties",
                            "additional property is not allowed",
                        ));
                    }
                }
            }
        }
        SchemaKind::Type(Type::Array(at)) => {
            let Some(arr) = value.as_array() else {
                errors.push(type_error(location, field, "array"));
                return;
            };
            if let Some(min) = at.min_items {
                if arr.len() < min {
                    errors.push(ValidationError::new(
                        location.clone(),
                        field,
                        "minItems",
                        format!("array length below minItems {}", min),
                    ));
                }
            }
            if let Some(max) = at.max_items {
                if arr.len() > max {
                    errors.push(ValidationError::new(
                        location.clone(),
                        field,
                        "maxItems",
                        format!("array length above maxItems {}", max),
                    ));
                }
            }
            if let Some(item_ref) = at.items.as_ref() {
                if let Some(item_schema) = resolve_boxed_schema(spec, item_ref) {
                    for (idx, child) in arr.iter().enumerate() {
                        validate_value(
                            spec,
                            item_schema,
                            child,
                            location.clone(),
                            &format!("{}[{}]", field, idx),
                            errors,
                        );
                    }
                }
            }
            if at.unique_items && has_duplicate_json_values(arr) {
                errors.push(ValidationError::new(
                    location.clone(),
                    field,
                    "uniqueItems",
                    "array items are not unique",
                ));
            }
        }
        SchemaKind::OneOf { one_of } => {
            if let Some(disc) = schema.schema_data.discriminator.as_ref() {
                if let Some(branch) = resolve_discriminated_branch(spec, one_of, disc, value) {
                    validate_value(spec, branch, value, location, field, errors);
                    return;
                }
            }
            let resolved: Vec<&Schema> = one_of
                .iter()
                .filter_map(|s| resolve_schema(spec, s))
                .collect();
            let total = resolved.len();
            let match_count = resolved
                .iter()
                .filter(|s| validate_value_silently(spec, s, value).is_empty())
                .count();
            if match_count != 1 {
                errors.push(ValidationError::new(
                    location,
                    field,
                    "oneOf",
                    format!(
                        "value matches {} of {} oneOf branches (must match exactly 1)",
                        match_count, total
                    ),
                ));
            }
        }
        SchemaKind::AnyOf { any_of } => {
            let any_match = any_of
                .iter()
                .filter_map(|s| resolve_schema(spec, s))
                .any(|s| validate_value_silently(spec, s, value).is_empty());
            if !any_match {
                errors.push(ValidationError::new(
                    location,
                    field,
                    "anyOf",
                    "value did not match any anyOf branch",
                ));
            }
        }
        SchemaKind::AllOf { all_of } => {
            let union = collect_allof_declared_properties(spec, all_of);
            for inner in all_of {
                if let Some(sub) = resolve_schema(spec, inner) {
                    validate_allof_branch(
                        spec,
                        sub,
                        value,
                        location.clone(),
                        field,
                        &union,
                        errors,
                    );
                }
            }
        }
        SchemaKind::Not { not } => {
            if let Some(sub) = resolve_schema(spec, not.as_ref()) {
                if validate_value_silently(spec, sub, value).is_empty() {
                    errors.push(ValidationError::new(
                        location,
                        field,
                        "not",
                        "value matched a schema it was required not to match",
                    ));
                }
            }
        }
        SchemaKind::Any(any) => {
            validate_any_schema(spec, any, value, location, field, errors);
        }
    }
}

/// Collects the union of property names declared across all branches of an
/// `allOf` composition, following one level of `$ref` resolution. Used to
/// compute the "declared properties" set for the `additionalProperties: false`
/// check so a sibling branch that sets `additionalProperties: false` does not
/// reject properties contributed by an adjacent `$ref` branch.
fn collect_allof_declared_properties(
    spec: &OpenAPI,
    branches: &[ReferenceOr<Schema>],
) -> std::collections::HashSet<String> {
    let mut out = std::collections::HashSet::new();
    for branch in branches {
        if let Some(sub) = resolve_schema(spec, branch) {
            match &sub.schema_kind {
                SchemaKind::Type(Type::Object(ot)) => {
                    for key in ot.properties.keys() {
                        out.insert(key.clone());
                    }
                }
                SchemaKind::Any(any) => {
                    for key in any.properties.keys() {
                        out.insert(key.clone());
                    }
                }
                _ => {}
            }
        }
    }
    out
}

/// Validates a single `allOf` branch against `value`, suppressing the local
/// `additionalProperties: false` check when the property would be declared by
/// a sibling branch in the union supplied. Other branch validation proceeds
/// as for a standalone schema.
fn validate_allof_branch(
    spec: &OpenAPI,
    schema: &Schema,
    value: &Value,
    location: ValidationLocation,
    field: &str,
    union: &std::collections::HashSet<String>,
    errors: &mut Vec<ValidationError>,
) {
    let suppresses_additional = matches!(
        schema.schema_kind,
        SchemaKind::Type(Type::Object(_)) | SchemaKind::Any(_)
    );
    if !suppresses_additional {
        validate_value(spec, schema, value, location, field, errors);
        return;
    }
    let mut tmp: Vec<ValidationError> = Vec::new();
    validate_value(spec, schema, value, location, field, &mut tmp);
    for err in tmp {
        if err.code == "additionalProperties" {
            if let Some(prop) = err.field.rsplit('.').next() {
                if union.contains(prop) {
                    continue;
                }
            }
        }
        errors.push(err);
    }
}

/// Returns the resolved `oneOf` branch named by `value`'s discriminator
/// property, consulting `discriminator.mapping` when supplied and falling
/// back to a name-based match against `#/components/schemas/X` references
/// when no mapping entry covers the value. Returns `None` when the
/// discriminator property is absent on the value, the value is not a
/// string, or no branch can be located.
fn resolve_discriminated_branch<'a>(
    spec: &'a OpenAPI,
    branches: &'a [ReferenceOr<Schema>],
    disc: &openapiv3::Discriminator,
    value: &Value,
) -> Option<&'a Schema> {
    let map = value.as_object()?;
    let key = map.get(&disc.property_name).and_then(|v| v.as_str())?;
    let target_ref = disc.mapping.get(key).cloned();
    for branch in branches {
        if let ReferenceOr::Reference { reference } = branch {
            if let Some(target) = target_ref.as_deref() {
                if reference == target {
                    return resolve_schema(spec, branch);
                }
            }
            if let Some(name) = reference.strip_prefix("#/components/schemas/") {
                if name.eq_ignore_ascii_case(key) {
                    return resolve_schema(spec, branch);
                }
            }
        }
    }
    None
}

/// Returns the errors a recursive validation would produce, without mutating the
/// caller's error list. Used to test `oneOf` / `anyOf` branch satisfaction.
fn validate_value_silently(spec: &OpenAPI, schema: &Schema, value: &Value) -> Vec<ValidationError> {
    let mut tmp = Vec::new();
    validate_value(spec, schema, value, ValidationLocation::Body, "_", &mut tmp);
    tmp
}

/// Validates a value against an [`AnySchema`] — the catch-all schema variant
/// that doesn't map cleanly onto a single typed variant.
fn validate_any_schema(
    spec: &OpenAPI,
    any: &AnySchema,
    value: &Value,
    location: ValidationLocation,
    field: &str,
    errors: &mut Vec<ValidationError>,
) {
    if let Some(typ) = any.typ.as_deref() {
        let ok = match typ {
            "string" => value.is_string(),
            "integer" => value.is_i64(),
            "number" => value.is_number(),
            "boolean" => value.is_boolean(),
            "object" => value.is_object(),
            "array" => value.is_array(),
            _ => true,
        };
        if !ok {
            errors.push(type_error(location.clone(), field, typ));
            return;
        }
    }
    if !any.enumeration.is_empty() {
        let allowed = any.enumeration.iter().any(|v| v == value);
        if !allowed {
            errors.push(ValidationError::new(
                location.clone(),
                field,
                "enum",
                "value not in enum",
            ));
        }
    }
    if let (Some(s), Some(pat)) = (value.as_str(), any.pattern.as_deref()) {
        check_pattern(s, pat, location.clone(), field, errors);
    }
    if let (Some(s), Some(fmt)) = (value.as_str(), any.format.as_deref()) {
        validate_format(s, fmt, location.clone(), field, errors);
    }
    if let Some(not_ref) = any.not.as_ref() {
        if let Some(sub) = resolve_schema(spec, not_ref.as_ref()) {
            if validate_value_silently(spec, sub, value).is_empty() {
                errors.push(ValidationError::new(
                    location.clone(),
                    field,
                    "not",
                    "value matched a schema it was required not to match",
                ));
            }
        }
    }
    if let Some(map) = value.as_object() {
        for required in &any.required {
            if !map.contains_key(required) {
                errors.push(ValidationError::new(
                    location.clone(),
                    format!("{}.{}", field, required),
                    "required",
                    "missing required property",
                ));
            }
        }
        for (key, prop_ref) in &any.properties {
            if let Some(child) = map.get(key) {
                if let Some(prop_schema) = resolve_boxed_schema(spec, prop_ref) {
                    validate_value(
                        spec,
                        prop_schema,
                        child,
                        location.clone(),
                        &format!("{}.{}", field, key),
                        errors,
                    );
                }
            }
        }
    }
}

/// Compiles `pattern` and pushes an error when `s` does not match. Compilation
/// failures are silently ignored so that broken specs don't yield false errors.
fn check_pattern(
    s: &str,
    pattern: &str,
    location: ValidationLocation,
    field: &str,
    errors: &mut Vec<ValidationError>,
) {
    let Some(re) = cached_regex(pattern) else {
        return;
    };
    if !re.is_match(s) {
        errors.push(ValidationError::new(
            location,
            field,
            "pattern",
            format!("value did not match pattern {}", pattern),
        ));
    }
}

/// Builds a uniform "expected type X" error for the location/field pair.
fn type_error(location: ValidationLocation, field: &str, expected: &str) -> ValidationError {
    ValidationError::new(location, field, "type", format!("expected {}", expected))
}

/// Validates the request body against the resolved `requestBody` definition.
///
/// Required bodies must be present; the request `Content-Type` must match one
/// of the body's media keys; and a JSON body is type-checked against the
/// matching media's schema when both are present.
fn validate_body(
    spec: &OpenAPI,
    body_def: &RequestBody,
    req: &MockRequest,
    errors: &mut Vec<ValidationError>,
) {
    let content_type = req
        .headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("content-type"))
        .map(|(_, v)| v.split(';').next().unwrap_or("").trim().to_string());

    if body_def.required && req.body.is_none() {
        errors.push(ValidationError::new(
            ValidationLocation::Body,
            "body",
            "required",
            "body is required",
        ));
        return;
    }

    if let Some(ct) = content_type.as_deref() {
        if !body_def.content.is_empty() {
            let matches_ct = body_def.content.keys().any(|k| {
                let stripped = k.split(';').next().unwrap_or(k).trim();
                stripped.eq_ignore_ascii_case(ct) || media_range_matches(stripped, ct)
            });
            if !matches_ct {
                errors.push(ValidationError::new(
                    ValidationLocation::Body,
                    "body",
                    "contentType",
                    format!("content-type '{}' is not accepted", ct),
                ));
            }
        }
    }

    let Some(body_value) = req.body.as_ref() else {
        return;
    };

    if is_xml_content_type(content_type.as_deref()) {
        return;
    }

    let media = body_def
        .content
        .iter()
        .find(|(k, _)| {
            let stripped = k.split(';').next().unwrap_or(k).trim();
            stripped.eq_ignore_ascii_case("application/json")
                || stripped
                    .eq_ignore_ascii_case(content_type.as_deref().unwrap_or("application/json"))
        })
        .map(|(_, v)| v);

    let Some(media) = media else { return };
    let Some(schema_ref) = media.schema.as_ref() else {
        return;
    };
    let Some(schema) = resolve_schema(spec, schema_ref) else {
        return;
    };
    let is_multipart = content_type
        .as_deref()
        .map(|ct| ct.eq_ignore_ascii_case("multipart/form-data"))
        .unwrap_or(false);
    if is_multipart {
        validate_required_only(spec, schema, body_value, "body", errors);
    } else {
        validate_value(
            spec,
            schema,
            body_value,
            ValidationLocation::Body,
            "body",
            errors,
        );
    }
}

/// Returns true when the supplied bare content type is an XML media type
/// (`application/xml`, `text/xml`, or any `application/*+xml` variant such as
/// `application/atom+xml`).
///
/// chasm has no XML schema validator, so body schema enforcement is skipped
/// for these media types. The request is still accepted; only schema-level
/// rules are bypassed because the body bytes are not a JSON document that
/// the JSON Schema engine can interrogate.
fn is_xml_content_type(content_type: Option<&str>) -> bool {
    let Some(ct) = content_type else { return false };
    let lower = ct.to_ascii_lowercase();
    lower == "application/xml"
        || lower == "text/xml"
        || (lower.starts_with("application/") && lower.ends_with("+xml"))
}

/// Checks only the `required` property list and `allOf` composition for a
/// body value, skipping per-property type/format checks. Used for
/// `multipart/form-data` bodies where the engine extracts only field names
/// (mapped to empty placeholder values) so strict per-field constraints
/// would always 422 even on well-formed requests.
fn validate_required_only(
    spec: &OpenAPI,
    schema: &Schema,
    value: &Value,
    field: &str,
    errors: &mut Vec<ValidationError>,
) {
    let map = match value.as_object() {
        Some(m) => m,
        None => return,
    };
    match &schema.schema_kind {
        SchemaKind::Type(Type::Object(ot)) => {
            for required in &ot.required {
                if !map.contains_key(required) {
                    errors.push(ValidationError::new(
                        ValidationLocation::Body,
                        format!("{}.{}", field, required),
                        "required",
                        "missing required property",
                    ));
                }
            }
        }
        SchemaKind::AllOf { all_of } => {
            for inner in all_of {
                if let Some(sub) = resolve_schema(spec, inner) {
                    validate_required_only(spec, sub, value, field, errors);
                }
            }
        }
        SchemaKind::Any(any) => {
            for required in &any.required {
                if !map.contains_key(required) {
                    errors.push(ValidationError::new(
                        ValidationLocation::Body,
                        format!("{}.{}", field, required),
                        "required",
                        "missing required property",
                    ));
                }
            }
        }
        _ => {}
    }
}

/// Returns true when two `f64` values are equal under a relative-epsilon
/// tolerance suitable for enum membership tests on arbitrarily-scaled numbers.
///
/// The earlier implementation compared with `f64::EPSILON` directly, which is
/// the representational gap at `1.0` (~2.22e-16) and is meaningless for values
/// orders of magnitude above or below `1.0`. This helper scales the tolerance
/// by `max(|a|, |b|, 1.0)` so the comparison stays sensible across the full
/// dynamic range, and multiplies by `64` to absorb the rounding noise that
/// accumulates over chained spec-to-JSON round-trips.
fn float_eq(a: f64, b: f64) -> bool {
    let diff = (a - b).abs();
    let scale = a.abs().max(b.abs()).max(1.0);
    diff <= f64::EPSILON * scale * 64.0
}

/// Returns true when `value` is within IEEE 754 epsilon tolerance of an exact
/// multiple of `multiple_of`.
///
/// Mirrors the strict float-precision check used in `chasm-faker`'s number
/// generator so generated values agree with validation. Reimplemented locally
/// to avoid a circular dependency between the validator and the faker.
fn passes_strict_multiple_of_check(value: f64, multiple_of: f64) -> bool {
    let remainder = (value / multiple_of) % 1.0;
    remainder.abs() < f64::EPSILON || (1.0 - remainder.abs()).abs() < f64::EPSILON
}

/// Returns true when `arr` contains at least one duplicate JSON value under
/// the standard `serde_json::Value` equality relation. Pairwise O(n^2) walk
/// is fine for the array sizes seen in mock validation.
fn has_duplicate_json_values(arr: &[Value]) -> bool {
    for i in 0..arr.len() {
        for j in (i + 1)..arr.len() {
            if arr[i] == arr[j] {
                return true;
            }
        }
    }
    false
}

/// Parses a request body byte slice into a [`serde_json::Value`] suitable for
/// schema validation, dispatching on the supplied `content_type`.
///
/// JSON bodies (`application/json`, `application/*+json`) are deserialised
/// directly. URL-encoded form bodies
/// (`application/x-www-form-urlencoded`) are parsed into a flat JSON object
/// where keys appearing once map to a string value and keys appearing more
/// than once map to a JSON array of strings, in the order observed on the
/// wire. Multipart bodies (`multipart/form-data`) are best-effort: the
/// helper scans for `Content-Disposition: form-data; name="…"` field markers
/// and produces an object whose keys are the field names and whose values are
/// empty strings, so downstream schema validation can enforce property
/// presence/`required` rules without trying to interpret arbitrary binary
/// content. Any other content type returns `None` so the caller can skip body
/// schema enforcement rather than emit a misleading type error.
///
/// This helper is intentionally pure (no I/O, no allocations beyond the
/// produced JSON tree) so server adapters can compose it with their own body
/// reader and `MockRequest` builder without taking a dependency on chasm's
/// HTTP layer.
pub fn parse_request_body_for_validation(content_type: &str, raw_body: &[u8]) -> Option<Value> {
    parse_request_body_for_validation_strict(content_type, raw_body).ok()
}

/// Strict variant of [`parse_request_body_for_validation`] that distinguishes
/// "unparseable JSON body" from "unsupported content type". An empty body is
/// treated as "no body" and surfaces as `Ok(Value::Null)` so the caller can
/// apply the operation's `required` flag without misclassifying the empty
/// bytes as an invalid_json payload. JSON content types whose body is
/// non-empty but invalid surface as
/// `Err(ValidationError { code = "type", field = "body", .. })`. All other
/// content types behave identically to
/// [`parse_request_body_for_validation`].
pub fn parse_request_body_for_validation_strict(
    content_type: &str,
    raw_body: &[u8],
) -> Result<Value, ValidationError> {
    let bare = content_type
        .split(';')
        .next()
        .unwrap_or(content_type)
        .trim();
    let lower = bare.to_ascii_lowercase();
    if lower == "application/json"
        || (lower.starts_with("application/") && lower.ends_with("+json"))
    {
        if raw_body.iter().all(|b| b.is_ascii_whitespace()) {
            return Ok(Value::Null);
        }
        return serde_json::from_slice(raw_body).map_err(|e| {
            ValidationError::new(
                ValidationLocation::Body,
                "body",
                "type",
                format!("request body is not valid JSON: {}", e),
            )
        });
    }
    if lower == "application/x-www-form-urlencoded" {
        return Ok(parse_form_urlencoded_to_json_value(raw_body));
    }
    if lower == "multipart/form-data" {
        return Ok(parse_multipart_field_names_to_json(content_type, raw_body));
    }
    if is_xml_content_type(Some(&lower)) {
        return Ok(Value::Null);
    }
    Err(ValidationError::new(
        ValidationLocation::Body,
        "body",
        "contentType",
        format!("content type '{}' is not parseable for validation", bare),
    ))
}

/// Parses an `application/x-www-form-urlencoded` body into a flat JSON object.
///
/// Repeated keys are collapsed into a JSON array preserving the order of
/// occurrence on the wire; single keys map to a JSON string. All values stay
/// as strings — downstream schema validation handles any further type
/// coercion. Decoding follows the URL-encoded forms standard: `+` decodes to
/// a space and `%HH` decodes to its byte value, with invalid UTF-8 byte runs
/// being silently dropped.
fn parse_form_urlencoded_to_json_value(raw_body: &[u8]) -> Value {
    let mut map: serde_json::Map<String, Value> = serde_json::Map::new();
    for pair in raw_body.split(|b| *b == b'&') {
        if pair.is_empty() {
            continue;
        }
        let (k_bytes, v_bytes) = match pair.iter().position(|b| *b == b'=') {
            Some(idx) => (&pair[..idx], &pair[idx + 1..]),
            None => (pair, &[][..]),
        };
        let key = match decode_form_urlencoded_bytes(k_bytes) {
            Some(s) => s,
            None => continue,
        };
        let value = decode_form_urlencoded_bytes(v_bytes).unwrap_or_default();
        let new_value = Value::String(value);
        match map.remove(&key) {
            None => {
                map.insert(key, new_value);
            }
            Some(Value::Array(mut arr)) => {
                arr.push(new_value);
                map.insert(key, Value::Array(arr));
            }
            Some(existing) => {
                map.insert(key, Value::Array(vec![existing, new_value]));
            }
        }
    }
    Value::Object(map)
}

/// Decodes a single URL-encoded form component, replacing `+` with a space
/// and percent-escapes with their byte value, returning `None` when the
/// resulting bytes are not valid UTF-8.
fn decode_form_urlencoded_bytes(bytes: &[u8]) -> Option<String> {
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < bytes.len() => {
                let hi = (bytes[i + 1] as char).to_digit(16);
                let lo = (bytes[i + 2] as char).to_digit(16);
                match (hi, lo) {
                    (Some(h), Some(l)) => {
                        out.push((h * 16 + l) as u8);
                        i += 3;
                    }
                    _ => {
                        out.push(bytes[i]);
                        i += 1;
                    }
                }
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8(out).ok()
}

/// Best-effort extraction of multipart form-data field names into a JSON
/// object so request-body validation can at least enforce property presence
/// and `required` rules.
///
/// The function locates the boundary in `content_type`, splits `raw_body` on
/// that boundary, and for each part scans the headers for a
/// `Content-Disposition: form-data; name="…"` directive. Each discovered
/// field name becomes a key in the returned object mapped to an empty string
/// value — the helper does not attempt to interpret the part body (which may
/// be binary), and a string-typed entry is the safest neutral default for
/// downstream JSON Schema checks. An object with no keys is returned when no
/// boundary or field names could be parsed.
fn parse_multipart_field_names_to_json(content_type: &str, raw_body: &[u8]) -> Value {
    let boundary = match extract_multipart_boundary(content_type) {
        Some(b) => b,
        None => return Value::Object(serde_json::Map::new()),
    };
    let delimiter = format!("--{}", boundary);
    let delimiter_bytes = delimiter.as_bytes();
    let mut map: serde_json::Map<String, Value> = serde_json::Map::new();
    let mut cursor = 0usize;
    while cursor < raw_body.len() {
        let Some(start) = find_subsequence(&raw_body[cursor..], delimiter_bytes) else {
            break;
        };
        let part_start = cursor + start + delimiter_bytes.len();
        let next_search_from = part_start;
        let end_offset = find_subsequence(&raw_body[next_search_from..], delimiter_bytes);
        let part_end = match end_offset {
            Some(e) => next_search_from + e,
            None => raw_body.len(),
        };
        if part_start < part_end {
            let part = &raw_body[part_start..part_end];
            if let Some(name) = extract_form_data_name(part) {
                map.entry(name).or_insert(Value::String(String::new()));
            }
        }
        cursor = part_end;
    }
    Value::Object(map)
}

/// Returns the `boundary` parameter from a multipart `Content-Type` header,
/// stripping any surrounding double quotes. Returns `None` when the parameter
/// is missing.
fn extract_multipart_boundary(content_type: &str) -> Option<String> {
    for part in content_type.split(';') {
        let part = part.trim();
        if let Some(rest) = part.strip_prefix("boundary=") {
            let trimmed = rest.trim();
            let unquoted = trimmed
                .strip_prefix('"')
                .and_then(|s| s.strip_suffix('"'))
                .unwrap_or(trimmed);
            if !unquoted.is_empty() {
                return Some(unquoted.to_string());
            }
        }
    }
    None
}

/// Returns the first index in `haystack` at which `needle` appears, or `None`
/// when `needle` is not a sub-slice of `haystack`. Used to scan multipart body
/// bytes for the boundary delimiter without pulling in a regex dependency for
/// binary data.
fn find_subsequence(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || needle.len() > haystack.len() {
        return None;
    }
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

/// Parses a multipart part for its `Content-Disposition: form-data; name="…"`
/// directive and returns the field name when present.
///
/// Scans the headers ahead of the blank line separator and extracts the
/// `name` parameter, stripping surrounding double quotes. Returns `None` when
/// the part has no `Content-Disposition` header, no `name` directive, or
/// non-UTF-8 header bytes.
fn extract_form_data_name(part: &[u8]) -> Option<String> {
    let header_end = find_subsequence(part, b"\r\n\r\n")
        .or_else(|| find_subsequence(part, b"\n\n"))
        .unwrap_or(part.len());
    let header_bytes = &part[..header_end];
    let header_text = std::str::from_utf8(header_bytes).ok()?;
    for line in header_text.split(['\n', '\r']) {
        let line = line.trim();
        if !line
            .to_ascii_lowercase()
            .starts_with("content-disposition:")
        {
            continue;
        }
        for token in line.split(';') {
            let token = token.trim();
            if let Some(rest) = token.strip_prefix("name=") {
                let unquoted = rest
                    .strip_prefix('"')
                    .and_then(|s| s.strip_suffix('"'))
                    .unwrap_or(rest);
                if !unquoted.is_empty() {
                    return Some(unquoted.to_string());
                }
            }
        }
    }
    None
}

/// Returns true when a `type/*` range matches a specific `type/subtype`.
fn media_range_matches(range: &str, specific: &str) -> bool {
    let Some(prefix) = range.strip_suffix("/*") else {
        return false;
    };
    specific
        .split_once('/')
        .map(|(t, _)| t.eq_ignore_ascii_case(prefix))
        .unwrap_or(false)
}
