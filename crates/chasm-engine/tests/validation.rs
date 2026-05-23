//! Tests for `validation.rs`.

use chasm_engine::{
    load_spec, mock, validate_request_full, MockConfig, MockError, MockRequest, ValidationError,
    ValidationLocation, ValidationSeverity,
};
use openapiv3::{OpenAPI, Operation};
use std::collections::HashMap;

mod common;
pub(crate) use common::req;

/// Builds a POST request with the given content type and optional parsed body.
pub(crate) fn post_request(content_type: &str, body: Option<serde_json::Value>) -> MockRequest {
    let mut headers = HashMap::new();
    headers.insert("content-type".to_string(), content_type.to_string());
    {
        let mut __r = MockRequest::default();
        __r.method = "POST".to_string();
        __r.path = "/x".to_string();
        __r.headers = headers;
        __r.query = HashMap::new();
        __r.body = body;
        __r
    }
}

/// Returns the `errors=true` mock configuration used by enforcement tests.
pub(crate) fn errors_cfg() -> MockConfig {
    {
        let mut __r = MockConfig::default();
        __r.errors = true;
        __r
    }
}

/// Locates the operation registered under `path_template` + `method`.
pub(crate) fn op_for<'a>(spec: &'a OpenAPI, path_template: &str, method: &str) -> &'a Operation {
    let path_item = spec
        .paths
        .paths
        .get(path_template)
        .and_then(|p| p.as_item())
        .expect("path item present");
    match method {
        "GET" => path_item.get.as_ref(),
        "POST" => path_item.post.as_ref(),
        "PUT" => path_item.put.as_ref(),
        "DELETE" => path_item.delete.as_ref(),
        _ => unreachable!("method {} not handled", method),
    }
    .expect("operation present")
}

/// Returns true when `errors` contains a matching `(location, field, code)` triple.
pub(crate) fn has_error(
    errors: &[ValidationError],
    location: ValidationLocation,
    field: &str,
    code: &str,
) -> bool {
    errors
        .iter()
        .any(|e| e.location == location && e.field == field && e.code == code)
}

/// A spec exercising every parameter location and a JSON request body.
pub(crate) fn validation_spec() -> &'static str {
    r#"
openapi: 3.0.0
info: { title: t, version: 1.0.0 }
paths:
  /pets/{kind}:
    get:
      parameters:
        - in: path
          name: kind
          required: true
          schema:
            type: string
            enum: [dog, cat]
        - in: query
          name: limit
          required: true
          schema:
            type: integer
            minimum: 1
            maximum: 100
        - in: header
          name: X-Trace-Id
          required: true
          schema: { type: string }
      responses:
        '200':
          description: ok
          content:
            application/json:
              example: { ok: true }
    post:
      requestBody:
        required: true
        content:
          application/json:
            schema:
              type: object
              required: [name]
              properties:
                name: { type: string }
                age: { type: integer, minimum: 0 }
      responses:
        '201':
          description: created
          content:
            application/json:
              example: { ok: true }
"#
}

#[path = "validation/body.rs"]
mod body;
#[path = "validation/multipart.rs"]
mod multipart;
#[path = "validation/params.rs"]
mod params;

/// Validation failures do not block response generation when `errors=false`.
#[test]
fn test_validation_failures_do_not_block_when_errors_disabled() {
    let spec = load_spec(validation_spec()).unwrap();
    let r = req("GET", "/pets/fish");

    let resp = mock(&spec, &r, &MockConfig::default()).unwrap();

    assert_eq!(resp.status, 200);
}

/// Validation entries carry a stable JSON Schema rule `code` and an explicit `severity`.
///
/// A `ValidationFailed` envelope entry carries the failing JSON Schema keyword
/// in its `code` field (here, `minimum` for an out-of-range `limit` query).
#[test]
fn test_validation_envelope_includes_code() {
    let spec = load_spec(validation_spec()).unwrap();
    let mut r = req("GET", "/pets/dog");
    r.headers.insert("X-Trace-Id".into(), "abc".into());
    r.query.insert("limit".into(), "0".into());

    let err = mock(&spec, &r, &errors_cfg()).unwrap_err();

    match err {
        MockError::ValidationFailed(errors) => {
            let entry = errors
                .iter()
                .find(|e| e.field == "limit")
                .expect("expected an entry for limit");
            assert_eq!(entry.code, "minimum");
        }
        other => panic!("expected ValidationFailed, got {other:?}"),
    }
}

/// A `ValidationFailed` envelope entry carries `ValidationSeverity::Error` —
/// every currently-emitted validator marks failures as errors (the `Warning`
/// and `Info` variants are reserved for future use). If a `Warning`-emitting
/// validator is added later, a dedicated test should be added alongside this one.
#[test]
fn test_validation_envelope_includes_severity() {
    let spec = load_spec(validation_spec()).unwrap();
    let mut r = req("GET", "/pets/dog");
    r.headers.insert("X-Trace-Id".into(), "abc".into());
    r.query.insert("limit".into(), "0".into());

    let err = mock(&spec, &r, &errors_cfg()).unwrap_err();

    match err {
        MockError::ValidationFailed(errors) => {
            let entry = errors
                .iter()
                .find(|e| e.field == "limit")
                .expect("expected an entry for limit");
            assert_eq!(entry.severity, ValidationSeverity::Error);
        }
        other => panic!("expected ValidationFailed, got {other:?}"),
    }
}

/// `ValidationLocation::as_str` returns the canonical lower-case wire string for `Path`.
#[test]
fn test_validation_location_path_as_str() {
    assert_eq!(ValidationLocation::Path.as_str(), "path");
}

/// `ValidationLocation::as_str` returns the canonical lower-case wire string for `Query`.
#[test]
fn test_validation_location_query_as_str() {
    assert_eq!(ValidationLocation::Query.as_str(), "query");
}

/// `ValidationLocation::as_str` returns the canonical lower-case wire string for `Header`.
#[test]
fn test_validation_location_header_as_str() {
    assert_eq!(ValidationLocation::Header.as_str(), "header");
}

/// `ValidationLocation::as_str` returns the canonical lower-case wire string for `Body`.
#[test]
fn test_validation_location_body_as_str() {
    assert_eq!(ValidationLocation::Body.as_str(), "body");
}

/// `ValidationSeverity::Error::as_str` returns the title-cased wire string `"Error"`.
#[test]
fn test_validation_severity_error_as_str() {
    assert_eq!(ValidationSeverity::Error.as_str(), "Error");
}

/// `ValidationSeverity::Warning::as_str` returns the title-cased wire string `"Warning"`.
#[test]
fn test_validation_severity_warning_as_str() {
    assert_eq!(ValidationSeverity::Warning.as_str(), "Warning");
}

/// `ValidationSeverity::Info::as_str` returns the title-cased wire string `"Info"`.
#[test]
fn test_validation_severity_info_as_str() {
    assert_eq!(ValidationSeverity::Info.as_str(), "Info");
}
