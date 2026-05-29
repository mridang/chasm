//! Tests for `engine.rs`.

use chasm_engine::{load_spec, mock, MockConfig, MockError};

/// `MockResponse::default()` lets external callers construct a starter value
/// without depending on the `#[non_exhaustive]` field set.
#[test]
fn test_mock_response_default_is_constructible() {
    let mut resp = chasm_engine::MockResponse::default();
    resp.status = 204;

    assert_eq!(resp.status, 204);
}

mod common;

/// A baseline YAML spec exercising multiple responses, examples, and a default fallback.
fn baseline_spec() -> &'static str {
    r#"
openapi: 3.0.0
info:
  title: parity
  version: 1.0.0
paths:
  /pets:
    get:
      responses:
        '201':
          description: created
          content:
            application/json:
              example: { code: 201 }
        '200':
          description: ok
          content:
            application/json:
              example: { code: 200 }
        default:
          description: fallback
          content:
            application/json:
              example: { code: 0 }
    post:
      responses:
        '200':
          description: ok
          content:
            application/json:
              examples:
                foo:
                  value: { picked: "foo" }
                bar:
                  value: { picked: "bar" }
  /onlydefault:
    get:
      responses:
        default:
          description: fallback only
          content:
            application/json:
              example: { only: "default" }
  /pets/{id}:
    get:
      responses:
        '200':
          description: ok
          content:
            application/json:
              example: { id: 1 }
"#
}

/// The lowest declared 2xx code is preferred, even when 201 appears before 200.
#[test]
fn test_lowest_2xx_wins_over_document_order() {
    let spec = load_spec(baseline_spec()).unwrap();

    let resp = mock(&spec, &common::req("GET", "/pets"), &MockConfig::default()).unwrap();

    assert_eq!(resp.status, 200);
}

/// `Prefer: code=` overrides the lowest-2xx default and propagates the chosen response body.
#[test]
fn test_prefer_code_overrides_status() {
    let spec = load_spec(baseline_spec()).unwrap();
    let mut r = common::req("GET", "/pets");
    r.headers.insert("Prefer".into(), "code=201".into());

    let resp = mock(&spec, &r, &MockConfig::default()).unwrap();

    assert_eq!(resp.body, serde_json::json!({"code": 201}));
}

/// When only a `default` response is defined the resolved status falls back to 200.
#[test]
fn test_default_response_resolves_to_200() {
    let spec = load_spec(baseline_spec()).unwrap();

    let resp = mock(
        &spec,
        &common::req("GET", "/onlydefault"),
        &MockConfig::default(),
    )
    .unwrap();

    assert_eq!(resp.body, serde_json::json!({"only": "default"}));
}

/// `Prefer: example=foo` selects the named example out of the `examples` map.
#[test]
fn test_prefer_example_selects_named_example() {
    let spec = load_spec(baseline_spec()).unwrap();
    let mut r = common::req("POST", "/pets");
    r.headers.insert("Prefer".into(), "example=bar".into());

    let resp = mock(&spec, &r, &MockConfig::default()).unwrap();

    assert_eq!(resp.body, serde_json::json!({"picked": "bar"}));
}

/// `Prefer: dynamic=true` forces the faker path even when an inline example exists.
#[test]
fn test_prefer_dynamic_skips_examples() {
    let yaml = r#"
openapi: 3.0.0
info: { title: t, version: 1.0.0 }
paths:
  /thing:
    get:
      responses:
        '200':
          description: ok
          content:
            application/json:
              schema:
                type: object
                required: [n]
                properties:
                  n: { type: integer, minimum: 7, maximum: 7 }
              example: { n: 99 }
"#;
    let spec = load_spec(yaml).unwrap();
    let mut r = common::req("GET", "/thing");
    r.headers.insert("Prefer".into(), "dynamic=true".into());

    let resp = mock(&spec, &r, &MockConfig::default()).unwrap();

    assert_eq!(resp.body.get("n").and_then(|v| v.as_i64()), Some(7));
}

/// Query-string overrides win over header `Prefer` directives.
#[test]
fn test_query_overrides_prefer_header() {
    let spec = load_spec(baseline_spec()).unwrap();
    let mut r = common::req("GET", "/pets");
    r.headers.insert("Prefer".into(), "code=201".into());
    r.query.insert("__code".into(), "200".into());

    let resp = mock(&spec, &r, &MockConfig::default()).unwrap();

    assert_eq!(resp.status, 200);
}

/// An `Accept: */*` header returns the first declared content type.
#[test]
fn test_accept_star_star_returns_first_content_type() {
    let yaml = r#"
openapi: 3.0.0
info: { title: t, version: 1.0.0 }
paths:
  /x:
    get:
      responses:
        '200':
          description: ok
          content:
            text/plain:
              example: "hello"
            application/json:
              example: { ok: true }
"#;
    let spec = load_spec(yaml).unwrap();
    let mut r = common::req("GET", "/x");
    r.headers.insert("Accept".into(), "*/*".into());

    let resp = mock(&spec, &r, &MockConfig::default()).unwrap();

    assert_eq!(resp.content_type, "text/plain");
}

/// `Accept` q-factor weights order candidates by descending quality.
#[test]
fn test_accept_q_factor_picks_highest_quality() {
    let yaml = r#"
openapi: 3.0.0
info: { title: t, version: 1.0.0 }
paths:
  /thing:
    get:
      responses:
        '200':
          description: ok
          content:
            application/json:
              example: { ok: true }
            text/plain:
              example: "ok"
"#;
    let spec = load_spec(yaml).unwrap();
    let mut r = common::req("GET", "/thing");
    r.headers.insert(
        "Accept".into(),
        "application/json; q=0.5, text/plain; q=0.9".into(),
    );

    let resp = mock(&spec, &r, &MockConfig::default()).unwrap();

    assert_eq!(resp.content_type, "text/plain");
}

/// An `Accept` entry carrying `q=0` excludes the type from selection.
#[test]
fn test_accept_q_zero_excludes_type() {
    let yaml = r#"
openapi: 3.0.0
info: { title: t, version: 1.0.0 }
paths:
  /pets:
    get:
      responses:
        '200':
          description: ok
          content:
            application/json:
              example: { picked: json }
            text/plain:
              example: picked-plain
"#;
    let spec = load_spec(yaml).unwrap();
    let mut r = common::req("GET", "/pets");
    r.headers.insert(
        "Accept".into(),
        "application/json;q=0, text/plain;q=0.5".into(),
    );

    let resp = mock(&spec, &r, &MockConfig::default()).unwrap();

    assert_eq!(resp.content_type, "text/plain");
}

/// Media types with charset parameters in the spec key still match a bare `Accept` entry.
#[test]
fn test_content_type_param_stripping() {
    let yaml = r#"
openapi: 3.0.0
info: { title: t, version: 1.0.0 }
paths:
  /x:
    get:
      responses:
        '200':
          description: ok
          content:
            "application/json; charset=utf-8":
              example: { ok: true }
"#;
    let spec = load_spec(yaml).unwrap();
    let mut r = common::req("GET", "/x");
    r.headers.insert("Accept".into(), "application/json".into());

    let resp = mock(&spec, &r, &MockConfig::default()).unwrap();

    assert_eq!(resp.body, serde_json::json!({"ok": true}));
}

/// A response header declared as bare `schema: { type: string }` (no
/// example, no format) generates a NON-empty value via the faker fallback,
/// so the header reaches the wire with a usable string rather than being
/// emitted as `""`.
#[test]
fn test_header_schema_only_string_fallback_is_non_empty() {
    let yaml = r#"
openapi: 3.0.0
info: { title: t, version: 1.0.0 }
paths:
  /x:
    get:
      responses:
        '200':
          description: ok
          headers:
            X-Fallback:
              schema:
                type: string
          content:
            application/json:
              example: { ok: true }
"#;
    let spec = load_spec(yaml).unwrap();
    let resp = mock(&spec, &common::req("GET", "/x"), &MockConfig::default()).unwrap();

    let value = resp
        .headers
        .iter()
        .find(|(k, _)| k == "X-Fallback")
        .map(|(_, v)| v.as_str())
        .expect("X-Fallback header must be emitted");
    assert!(
        !value.is_empty(),
        "schema-only `type: string` header must fall through to a non-empty faker value, got {:?}",
        value
    );
}

/// `Accept` headers carrying non-q media-type parameters (e.g. charset)
/// still match the spec's parameter-less content key per RFC 9110 §12.5.1.
#[test]
fn test_accept_with_charset_parameter_matches_plain_content_key() {
    let yaml = r#"
openapi: 3.0.0
info: { title: t, version: 1.0.0 }
paths:
  /pets:
    get:
      responses:
        '200':
          description: ok
          content:
            application/json:
              example: { ok: true }
"#;
    let spec = load_spec(yaml).unwrap();
    let mut r = common::req("GET", "/pets");
    r.headers
        .insert("Accept".into(), "application/json; charset=utf-8".into());

    let resp = mock(&spec, &r, &MockConfig::default()).unwrap();

    assert_eq!(resp.status, 200);
    assert_eq!(resp.content_type, "application/json");
}

/// A spec exposing a mix of custom headers and framing headers that should be filtered.
fn headers_filtering_spec() -> &'static str {
    r#"
openapi: 3.0.0
info: { title: t, version: 1.0.0 }
paths:
  /x:
    get:
      responses:
        '200':
          description: ok
          headers:
            X-RateLimit-Limit:
              schema: { type: integer }
              example: 60
            Content-Encoding:
              schema: { type: string }
              example: gzip
            Content-Length:
              schema: { type: integer }
              example: 99
            X-Custom:
              schema: { type: string, example: from-schema }
          content:
            application/json:
              example: { ok: true }
"#
}

/// A spec-declared `X-RateLimit-Limit` response header is emitted verbatim.
#[test]
fn test_response_x_ratelimit_limit_emitted() {
    let spec = load_spec(headers_filtering_spec()).unwrap();

    let resp = mock(&spec, &common::req("GET", "/x"), &MockConfig::default()).unwrap();

    let names: Vec<String> = resp.headers.iter().map(|(k, _)| k.clone()).collect();
    assert!(names.iter().any(|n| n == "X-RateLimit-Limit"));
}

/// A spec-declared `X-Custom` response header is emitted verbatim.
#[test]
fn test_response_x_custom_emitted() {
    let spec = load_spec(headers_filtering_spec()).unwrap();

    let resp = mock(&spec, &common::req("GET", "/x"), &MockConfig::default()).unwrap();

    let names: Vec<String> = resp.headers.iter().map(|(k, _)| k.clone()).collect();
    assert!(names.iter().any(|n| n == "X-Custom"));
}

/// A spec-declared `Content-Encoding` response header is stripped before emission.
#[test]
fn test_response_content_encoding_stripped() {
    let spec = load_spec(headers_filtering_spec()).unwrap();

    let resp = mock(&spec, &common::req("GET", "/x"), &MockConfig::default()).unwrap();

    let names: Vec<String> = resp.headers.iter().map(|(k, _)| k.clone()).collect();
    assert!(!names
        .iter()
        .any(|n| n.eq_ignore_ascii_case("content-encoding")));
}

/// A spec-declared `Content-Length` response header is stripped before emission.
#[test]
fn test_response_content_length_stripped() {
    let spec = load_spec(headers_filtering_spec()).unwrap();

    let resp = mock(&spec, &common::req("GET", "/x"), &MockConfig::default()).unwrap();

    let names: Vec<String> = resp.headers.iter().map(|(k, _)| k.clone()).collect();
    assert!(!names
        .iter()
        .any(|n| n.eq_ignore_ascii_case("content-length")));
}

/// A spec-declared `Content-Length` response header is stripped before emission.
#[test]
fn test_content_length_from_spec_is_stripped() {
    let yaml = r#"
openapi: 3.0.0
info: { title: bug-1621, version: 1.0.0 }
paths:
  /thing:
    get:
      responses:
        '200':
          description: ok
          headers:
            Content-Length:
              schema: { type: integer }
              example: 9999
          content:
            application/json:
              example: { ok: true }
"#;
    let spec = load_spec(yaml).unwrap();

    let resp = mock(&spec, &common::req("GET", "/thing"), &MockConfig::default()).unwrap();

    let names: Vec<String> = resp.headers.iter().map(|(k, _)| k.clone()).collect();
    assert!(!names
        .iter()
        .any(|n| n.eq_ignore_ascii_case("content-length")));
}

/// A malformed content key such as `"undefined"` is skipped during negotiation.
#[test]
fn test_malformed_content_key_is_skipped() {
    let yaml = r#"
openapi: 3.0.0
info: { title: t, version: 1.0.0 }
paths:
  /x:
    get:
      responses:
        '200':
          description: ok
          content:
            "undefined":
              example: { broken: true }
            "application/json":
              example: { ok: true }
"#;
    let spec = load_spec(yaml).unwrap();

    let resp = mock(&spec, &common::req("GET", "/x"), &MockConfig::default()).unwrap();

    assert_eq!(resp.body, serde_json::json!({"ok": true}));
}

/// An empty-content (`content: {}`) response with a restrictive `Accept` synthesises an empty body.
#[test]
fn test_no_content_with_accept_header_does_not_406() {
    let yaml = r#"
openapi: 3.0.0
info: { title: t, version: 1.0.0 }
paths:
  /responder_endpoints:
    delete:
      responses:
        '200':
          description: Endpoint disenrolled successfully
          content: {}
"#;
    let spec = load_spec(yaml).unwrap();
    let mut r = common::req("DELETE", "/responder_endpoints");
    r.headers.insert("Accept".into(), "application/json".into());

    let resp = mock(&spec, &r, &MockConfig::default()).unwrap();

    assert_eq!(resp.body, serde_json::Value::Null);
}

/// A response declaring no `content` key at all returns an empty body even with `Accept`.
#[test]
fn test_no_content_field_at_all_with_accept_header() {
    let yaml = r#"
openapi: 3.0.0
info: { title: t, version: 1.0.0 }
paths:
  /thing:
    delete:
      responses:
        '204':
          description: no content
"#;
    let spec = load_spec(yaml).unwrap();
    let mut r = common::req("DELETE", "/thing");
    r.headers.insert("Accept".into(), "application/json".into());

    let resp = mock(&spec, &r, &MockConfig::default()).unwrap();

    assert_eq!(resp.status, 204);
}

/// A binary content type (`application/pdf`) negotiates `application/pdf` on the wire.
#[test]
fn test_pdf_content_type_negotiates_application_pdf() {
    let yaml = r#"
openapi: 3.0.0
info: { title: t, version: 1.0.0 }
paths:
  /report:
    get:
      responses:
        '200':
          description: OK
          content:
            application/pdf:
              schema:
                type: string
                format: binary
"#;
    let spec = load_spec(yaml).unwrap();
    let mut r = common::req("GET", "/report");
    r.headers.insert("Accept".into(), "application/pdf".into());

    let resp = mock(&spec, &r, &MockConfig::default()).unwrap();

    assert_eq!(resp.content_type, "application/pdf");
}

/// An `externalValue`-only example does not cause a generation failure.
#[test]
fn test_pdf_with_external_value_example() {
    let yaml = r#"
openapi: 3.0.0
info: { title: t, version: 1.0.0 }
paths:
  /report:
    get:
      responses:
        '200':
          description: OK
          content:
            application/pdf:
              schema:
                type: string
                format: binary
              examples:
                default:
                  summary: A sample report
                  externalValue: "https://example.com/dummy.pdf"
"#;
    let spec = load_spec(yaml).unwrap();
    let mut r = common::req("GET", "/report");
    r.headers.insert("Accept".into(), "application/pdf".into());

    let resp = mock(&spec, &r, &MockConfig::default()).unwrap();

    assert_eq!(resp.content_type, "application/pdf");
}

/// A response with an `example:` patently mismatching its `schema:` is returned verbatim.
#[test]
fn test_example_schema_mismatch_returned_verbatim() {
    let yaml = r#"
openapi: 3.0.0
info: { title: t, version: 1.0.0 }
paths:
  /mismatch:
    get:
      responses:
        '200':
          description: ok
          content:
            application/json:
              schema:
                type: object
                properties:
                  count: { type: integer }
                required: [count]
              example:
                count: "not-a-number"
"#;
    let spec = load_spec(yaml).unwrap();

    let resp = mock(
        &spec,
        &common::req("GET", "/mismatch"),
        &MockConfig::default(),
    )
    .unwrap();

    assert_eq!(resp.body, serde_json::json!({ "count": "not-a-number" }));
}

/// An example referenced via `#/components/examples/X` is resolved.
#[test]
fn test_component_example_ref_resolution() {
    let yaml = r#"
openapi: 3.0.0
info: { title: t, version: 1.0.0 }
paths:
  /x:
    get:
      responses:
        '200':
          description: ok
          content:
            application/json:
              examples:
                primary:
                  $ref: '#/components/examples/Primary'
components:
  examples:
    Primary:
      value: { from: "components" }
"#;
    let spec = load_spec(yaml).unwrap();

    let resp = mock(&spec, &common::req("GET", "/x"), &MockConfig::default()).unwrap();

    assert_eq!(resp.body, serde_json::json!({"from": "components"}));
}

/// `Prefer: example=<unknown>` surfaces `ExampleNotFound` with the content type and example key.
#[test]
fn test_prefer_unknown_example_returns_example_not_found() {
    let yaml = r#"
openapi: 3.0.0
info: { title: t, version: 1.0.0 }
paths:
  /pets:
    get:
      responses:
        '200':
          description: ok
          content:
            application/json:
              examples:
                foo:
                  value: { picked: foo }
"#;
    let spec = load_spec(yaml).unwrap();
    let mut r = common::req("GET", "/pets");
    r.headers.insert("Prefer".into(), "example=missing".into());

    let err = mock(&spec, &r, &MockConfig::default()).unwrap_err();

    match err {
        MockError::ExampleNotFound {
            content_type,
            example_key,
        } => {
            assert_eq!(
                (content_type.as_str(), example_key.as_str()),
                ("application/json", "missing")
            );
        }
        other => panic!("expected ExampleNotFound, got {other:?}"),
    }
}

/// `Prefer: code=<n>` for an undeclared code surfaces `NoResponseForCode` carrying the request shape.
#[test]
fn test_prefer_unknown_code_returns_no_response_for_code() {
    let yaml = r#"
openapi: 3.0.0
info: { title: t, version: 1.0.0 }
paths:
  /x:
    get:
      responses:
        '200':
          description: ok
          content:
            application/json:
              example: { ok: true }
"#;
    let spec = load_spec(yaml).unwrap();
    let mut r = common::req("GET", "/x");
    r.headers.insert("Prefer".into(), "code=500".into());

    let err = mock(&spec, &r, &MockConfig::default()).unwrap_err();

    match err {
        MockError::NoResponseForCode { method, path, code } => {
            assert_eq!((method.as_str(), path.as_str(), code), ("GET", "/x", 500))
        }
        other => panic!("expected NoResponseForCode, got {other:?}"),
    }
}

/// An `Accept` header that does not intersect any spec media type returns `NotAcceptable`.
#[test]
fn test_unacceptable_accept_header_returns_not_acceptable() {
    let yaml = r#"
openapi: 3.0.0
info: { title: t, version: 1.0.0 }
paths:
  /y:
    get:
      responses:
        '200':
          description: ok
          content:
            application/json:
              example: { ok: true }
"#;
    let spec = load_spec(yaml).unwrap();
    let mut r = common::req("GET", "/y");
    r.headers.insert("Accept".into(), "text/csv".into());

    let err = mock(&spec, &r, &MockConfig::default()).unwrap_err();

    match err {
        MockError::NotAcceptable { acceptable } => {
            assert!(acceptable.iter().any(|m| m == "application/json"))
        }
        other => panic!("expected NotAcceptable, got {other:?}"),
    }
}

/// An operation with no declared responses surfaces `NoResponseDefined`.
#[test]
fn test_no_response_defined_returns_distinct_error() {
    let yaml = r#"
openapi: 3.0.0
info: { title: t, version: 1.0.0 }
paths:
  /foo:
    get:
      responses: {}
"#;
    let spec = load_spec(yaml).unwrap();

    let err = mock(&spec, &common::req("GET", "/foo"), &MockConfig::default()).unwrap_err();

    assert!(matches!(err, MockError::NoResponseDefined));
}

/// A spec-level `x-json-schema-faker.minItems` config is honoured during generation.
#[test]
fn test_jsf_config_min_items_applies() {
    let yaml = r#"
openapi: 3.0.0
info: { title: t, version: 1.0.0 }
x-json-schema-faker:
  minItems: 5
  alwaysFakeOptionals: true
paths:
  /xs:
    get:
      responses:
        '200':
          description: ok
          content:
            application/json:
              schema:
                type: array
                items:
                  type: string
"#;
    let spec = load_spec(yaml).unwrap();
    let cfg = {
        let mut __c = MockConfig::default();
        __c.dynamic = true;
        __c
    };

    let resp = mock(&spec, &common::req("GET", "/xs"), &cfg).unwrap();

    assert!(resp.body.as_array().map(|a| a.len() >= 5).unwrap_or(false));
}

/// A spec exercising `optionalsProbability=0` with one required and one optional property.
fn jsf_optionals_probability_zero_spec() -> &'static str {
    r#"
openapi: 3.0.0
info: { title: t, version: 1.0.0 }
x-json-schema-faker:
  optionalsProbability: 0
  alwaysFakeOptionals: false
paths:
  /thing:
    get:
      responses:
        '200':
          description: ok
          content:
            application/json:
              schema:
                type: object
                properties:
                  must: { type: string }
                  maybe: { type: string }
                required: [must]
"#
}

/// With `optionalsProbability=0`, every required property is always present.
#[test]
fn test_jsf_required_property_always_present() {
    let spec = load_spec(jsf_optionals_probability_zero_spec()).unwrap();
    let cfg = {
        let mut __c = MockConfig::default();
        __c.dynamic = true;
        __c
    };

    let resp = mock(&spec, &common::req("GET", "/thing"), &cfg).unwrap();

    assert!(resp.body.as_object().unwrap().contains_key("must"));
}

/// With `optionalsProbability=0`, every optional property is omitted from the result.
#[test]
fn test_jsf_optional_property_omitted_when_probability_zero() {
    let spec = load_spec(jsf_optionals_probability_zero_spec()).unwrap();
    let cfg = {
        let mut __c = MockConfig::default();
        __c.dynamic = true;
        __c
    };

    let resp = mock(&spec, &common::req("GET", "/thing"), &cfg).unwrap();

    assert!(!resp.body.as_object().unwrap().contains_key("maybe"));
}

/// In `--dynamic` mode an `array of $ref` schema produces real referenced objects, not nulls.
#[test]
fn test_dynamic_mode_array_of_ref_resolves_to_objects() {
    let yaml = r#"
openapi: 3.0.3
info: { title: t, version: 1.0.0 }
paths:
  /pets:
    get:
      responses:
        '200':
          description: ok
          content:
            application/json:
              schema:
                type: array
                minItems: 2
                maxItems: 2
                items:
                  $ref: '#/components/schemas/Pet'
  /pets/{petId}:
    get:
      parameters:
        - name: petId
          in: path
          required: true
          schema: { type: integer }
      responses:
        '200':
          description: ok
          content:
            application/json:
              schema:
                $ref: '#/components/schemas/Pet'
components:
  schemas:
    Pet:
      type: object
      required: [id, name]
      properties:
        id: { type: integer }
        name: { type: string, minLength: 1, maxLength: 8 }
        tag: { type: string }
"#;
    let spec = load_spec(yaml).unwrap();
    let cfg = {
        let mut __c = MockConfig::default();
        __c.dynamic = true;
        __c.seed = Some(42);
        __c
    };

    let resp = mock(&spec, &common::req("GET", "/pets"), &cfg).unwrap();

    let arr = resp
        .body
        .as_array()
        .expect("GET /pets dynamic body must be an array");
    assert!(
        arr.iter().all(|item| item.is_object()),
        "every array entry must resolve to a JSON object",
    );
    assert!(
        arr.iter()
            .all(|item| item.as_object().unwrap().contains_key("id")),
        "every resolved object must carry the required `id` key",
    );
    assert!(
        arr.iter()
            .all(|item| item.as_object().unwrap().contains_key("name")),
        "every resolved object must carry the required `name` key",
    );
}

/// Asserts that a nullable-via-`type`-array schema YAML produces both `null` and string draws.
fn assert_nullable_normalised(yaml: &str) {
    let spec = load_spec(yaml).unwrap();

    let mut saw_null = false;
    let mut saw_string = false;
    for seed in 0u64..100 {
        let cfg = {
            let mut __c = MockConfig::default();
            __c.dynamic = true;
            __c.seed = Some(seed);
            __c
        };
        let resp = mock(&spec, &common::req("GET", "/thing"), &cfg).unwrap();
        let name = resp.body.get("name").expect("`name` should be present");
        if name.is_null() {
            saw_null = true;
        } else if name.is_string() {
            saw_string = true;
        }
        if saw_null && saw_string {
            break;
        }
    }

    assert!(saw_null && saw_string);
}

/// The OAS 3.1 nullable-via-type-array idiom is normalised so the generator can produce both `null` and string.
#[test]
fn test_oas_31_nullable_array_normalised() {
    assert_nullable_normalised(
        r#"
openapi: 3.0.0
info: { title: t, version: 1.0.0 }
paths:
  /thing:
    get:
      responses:
        '200':
          description: ok
          content:
            application/json:
              schema:
                type: object
                required: [name]
                properties:
                  name:
                    type: ["string", "null"]
"#,
    );
}

/// The OAS 3.1 `type: ["null", "string"]` ordering is normalised symmetrically with `["string", "null"]`.
#[test]
fn test_oas_31_multi_type_with_null_first_normalised() {
    assert_nullable_normalised(
        r#"
openapi: 3.0.0
info: { title: t, version: 1.0.0 }
paths:
  /thing:
    get:
      responses:
        '200':
          description: ok
          content:
            application/json:
              schema:
                type: object
                required: [name]
                properties:
                  name:
                    type: ["null", "string"]
"#,
    );
}

/// `MockError::SpecSerialization` is constructible and its `Display` carries the inner reason.
///
/// The variant is unreachable through normal API paths (the engine only fails to serialise
/// a spec when `serde_json::to_value(spec)` errors, which requires a non-roundtrippable
/// spec value), so this test pins the public `Display` contract directly.
#[test]
fn test_mock_error_spec_serialization_is_constructible() {
    let err = MockError::SpecSerialization("synthetic failure".into());

    let rendered = err.to_string();

    assert!(rendered.contains("synthetic failure"));
}

/// A response header declared with a schema but no inline example must still be
/// emitted, falling through to the faker for a value derived from the schema's
/// declared type. Real-world specs (AWS, GitHub) routinely declare response
/// headers as `schema: { type: integer }` and expect chasm to produce a value.
#[test]
fn test_response_header_schema_without_example_emits_value() {
    let yaml = r#"
openapi: 3.0.0
info: { title: t, version: 1.0.0 }
paths:
  /x:
    get:
      responses:
        '200':
          description: ok
          headers:
            X-RateLimit-Limit:
              schema:
                type: integer
          content:
            application/json:
              example: { ok: true }
"#;
    let spec = load_spec(yaml).unwrap();

    let resp = mock(&spec, &common::req("GET", "/x"), &MockConfig::default()).unwrap();

    let value = resp
        .headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("X-RateLimit-Limit"))
        .map(|(_, v)| v.clone())
        .expect("X-RateLimit-Limit must be emitted");
    assert!(
        value.parse::<i64>().is_ok(),
        "expected integer-shaped value for X-RateLimit-Limit, got {value:?}"
    );
}

/// A static-mode response body must adopt property-level `example` values when
/// the schema declares them only on its leaf properties (no media-type
/// `example`, no `examples` map, no schema-level `example`). Without this the
/// body collapses to type-zero scalars (`{"id":0,"name":""}`), which diverges
/// from the example-populated object real clients expect.
#[test]
fn test_static_body_uses_property_level_examples() {
    let yaml = r#"
openapi: 3.0.0
info: { title: t, version: 1.0.0 }
paths:
  /pets:
    get:
      responses:
        '200':
          description: ok
          content:
            application/json:
              schema:
                type: object
                required: [id, name]
                properties:
                  id:
                    type: integer
                    example: 10
                  name:
                    type: string
                    example: doggie
"#;
    let spec = load_spec(yaml).unwrap();

    let resp = mock(&spec, &common::req("GET", "/pets"), &MockConfig::default()).unwrap();

    assert_eq!(resp.body, serde_json::json!({ "id": 10, "name": "doggie" }));
}

/// A static-mode response body must adopt property-level `default` values when
/// the schema declares them only on its leaf properties, mirroring the
/// example-honouring behaviour for specs that lean on `default` instead.
#[test]
fn test_static_body_uses_property_level_defaults() {
    let yaml = r#"
openapi: 3.0.0
info: { title: t, version: 1.0.0 }
paths:
  /pets:
    get:
      responses:
        '200':
          description: ok
          content:
            application/json:
              schema:
                type: object
                required: [status, count]
                properties:
                  status:
                    type: string
                    default: available
                  count:
                    type: integer
                    default: 7
"#;
    let spec = load_spec(yaml).unwrap();

    let resp = mock(&spec, &common::req("GET", "/pets"), &MockConfig::default()).unwrap();

    assert_eq!(
        resp.body,
        serde_json::json!({ "status": "available", "count": 7 })
    );
}
