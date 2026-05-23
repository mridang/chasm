//! Path, query, and header parameter validation tests.

use super::*;
use std::collections::HashMap;

/// A missing required query parameter yields a `ValidationFailed` carrying a Query/limit entry.
#[test]
fn test_missing_required_query_param_emits_validation_failed() {
    let spec = load_spec(validation_spec()).unwrap();
    let mut r = req("GET", "/pets/dog");
    r.headers.insert("X-Trace-Id".into(), "abc".into());

    let err = mock(&spec, &r, &errors_cfg()).unwrap_err();

    match err {
        MockError::ValidationFailed(errors) => assert!(errors
            .iter()
            .any(|e| e.field == "limit" && e.location == ValidationLocation::Query)),
        other => panic!("expected ValidationFailed, got {other:?}"),
    }
}

/// A non-integer query value where the schema declares `type: integer` emits Query/limit/type.
#[test]
fn test_query_param_type_mismatch_emits_validation_failed() {
    let spec = load_spec(validation_spec()).unwrap();
    let mut r = req("GET", "/pets/dog");
    r.headers.insert("X-Trace-Id".into(), "abc".into());
    r.query.insert("limit".into(), "abc".into());

    let err = mock(&spec, &r, &errors_cfg()).unwrap_err();

    match err {
        MockError::ValidationFailed(errors) => assert!(has_error(
            &errors,
            ValidationLocation::Query,
            "limit",
            "type"
        )),
        other => panic!("expected ValidationFailed, got {other:?}"),
    }
}

/// A path-parameter enum violation surfaces as a Path/kind error.
#[test]
fn test_path_param_enum_violation_emits_path_error() {
    let spec = load_spec(validation_spec()).unwrap();
    let mut r = req("GET", "/pets/fish");
    r.headers.insert("X-Trace-Id".into(), "abc".into());
    r.query.insert("limit".into(), "10".into());

    let err = mock(&spec, &r, &errors_cfg()).unwrap_err();

    match err {
        MockError::ValidationFailed(errors) => assert!(errors
            .iter()
            .any(|e| e.field == "kind" && e.location == ValidationLocation::Path)),
        other => panic!("expected ValidationFailed, got {other:?}"),
    }
}

/// A missing required header surfaces as a Header/X-Trace-Id error.
#[test]
fn test_missing_required_header_emits_header_error() {
    let spec = load_spec(validation_spec()).unwrap();
    let mut r = req("GET", "/pets/dog");
    r.query.insert("limit".into(), "10".into());

    let err = mock(&spec, &r, &errors_cfg()).unwrap_err();

    match err {
        MockError::ValidationFailed(errors) => assert!(errors
            .iter()
            .any(|e| e.field == "X-Trace-Id" && e.location == ValidationLocation::Header)),
        other => panic!("expected ValidationFailed, got {other:?}"),
    }
}

/// A non-integer path-parameter where the schema declares `type: integer` emits a Path/type error.
#[test]
fn test_path_integer_type_mismatch_emits_path_error() {
    let yaml = r#"
openapi: 3.0.0
info: { title: t, version: 1.0.0 }
paths:
  /items/{id}:
    get:
      parameters:
        - in: path
          name: id
          required: true
          schema:
            type: integer
            minimum: 1
      responses:
        '200':
          description: ok
          content:
            application/json:
              example: { ok: true }
"#;
    let spec = load_spec(yaml).unwrap();
    let r = req("GET", "/items/notanumber");

    let err = mock(&spec, &r, &errors_cfg()).unwrap_err();

    match err {
        MockError::ValidationFailed(errors) => assert!(errors
            .iter()
            .any(|e| e.field == "id" && e.location == ValidationLocation::Path)),
        other => panic!("expected ValidationFailed, got {other:?}"),
    }
}

/// A query value below the schema's `minimum` emits a `below minimum` error.
#[test]
fn test_query_minimum_violation() {
    let spec = load_spec(validation_spec()).unwrap();
    let mut r = req("GET", "/pets/dog");
    r.headers.insert("X-Trace-Id".into(), "abc".into());
    r.query.insert("limit".into(), "0".into());

    let err = mock(&spec, &r, &errors_cfg()).unwrap_err();

    match err {
        MockError::ValidationFailed(errors) => assert!(errors
            .iter()
            .any(|e| e.field == "limit" && e.message.contains("below minimum"))),
        other => panic!("expected ValidationFailed, got {other:?}"),
    }
}

/// A query value above the schema's `maximum` emits an `above maximum` error.
#[test]
fn test_query_maximum_violation() {
    let spec = load_spec(validation_spec()).unwrap();
    let mut r = req("GET", "/pets/dog");
    r.headers.insert("X-Trace-Id".into(), "abc".into());
    r.query.insert("limit".into(), "9999".into());

    let err = mock(&spec, &r, &errors_cfg()).unwrap_err();

    match err {
        MockError::ValidationFailed(errors) => assert!(errors
            .iter()
            .any(|e| e.field == "limit" && e.message.contains("above maximum"))),
        other => panic!("expected ValidationFailed, got {other:?}"),
    }
}

/// Header-name lookup is case-insensitive.
#[test]
fn test_header_case_insensitive_lookup() {
    let spec = load_spec(validation_spec()).unwrap();
    let mut r = req("GET", "/pets/dog");
    r.headers.insert("x-trace-id".into(), "abc".into());
    r.query.insert("limit".into(), "10".into());

    let resp = mock(&spec, &r, &errors_cfg()).unwrap();

    assert_eq!(resp.status, 200);
}

/// Header parameters whose names are reserved by RFC 7230 (`Accept`, `Content-Type`, `Authorization`) are ignored.
#[test]
fn test_header_validator_ignores_reserved_names() {
    let yaml = r#"
openapi: 3.0.0
info: { title: t, version: 1.0.0 }
paths:
  /x:
    post:
      parameters:
        - name: Accept
          in: header
          required: true
          schema: { type: string, enum: [application/xml] }
        - name: Content-Type
          in: header
          required: true
          schema: { type: string, enum: [application/xml] }
        - name: Authorization
          in: header
          required: true
          schema: { type: string, pattern: "^Bearer .+$" }
      responses:
        '200': { description: ok }
"#;
    let spec = load_spec(yaml).expect("spec");
    let op = op_for(&spec, "/x", "POST");
    let r = post_request("application/json", None);

    let errors = validate_request_full(&spec, Some("/x"), op, &HashMap::new(), &r);

    assert!(
        errors.is_empty(),
        "reserved header names must be ignored, got {errors:?}"
    );
}

/// A comma-separated array path-parameter in the default `simple` style validates as an array.
#[test]
fn test_array_path_param_simple_style_validates() {
    let yaml = r#"
openapi: 3.0.0
info: { title: t, version: 1.0.0 }
paths:
  /pets/{ids}:
    get:
      parameters:
        - name: ids
          in: path
          required: true
          schema: { type: array, items: { type: integer } }
      responses:
        '200': { description: ok }
"#;
    let spec = load_spec(yaml).expect("spec");
    let op = op_for(&spec, "/pets/{ids}", "GET");
    let mut path_params = HashMap::new();
    path_params.insert("ids".to_string(), "1,2,3".to_string());
    let request = {
        let mut __r = MockRequest::default();
        __r.method = "GET".to_string();
        __r.path = "/pets/1,2,3".to_string();
        __r.headers = HashMap::new();
        __r.query = HashMap::new();
        __r.body = None;
        __r
    };

    let errors = validate_request_full(&spec, Some("/pets/{ids}"), op, &path_params, &request);

    assert!(errors.is_empty(), "got {errors:?}");
}

/// A non-integer element in a `simple`-style array path-parameter fails validation.
#[test]
fn test_array_path_param_rejects_bad_element() {
    let yaml = r#"
openapi: 3.0.0
info: { title: t, version: 1.0.0 }
paths:
  /pets/{ids}:
    get:
      parameters:
        - name: ids
          in: path
          required: true
          schema: { type: array, items: { type: integer } }
      responses:
        '200': { description: ok }
"#;
    let spec = load_spec(yaml).expect("spec");
    let op = op_for(&spec, "/pets/{ids}", "GET");
    let mut path_params = HashMap::new();
    path_params.insert("ids".to_string(), "1,foo,3".to_string());
    let request = {
        let mut __r = MockRequest::default();
        __r.method = "GET".to_string();
        __r.path = "/pets/1,foo,3".to_string();
        __r.headers = HashMap::new();
        __r.query = HashMap::new();
        __r.body = None;
        __r
    };

    let errors = validate_request_full(&spec, Some("/pets/{ids}"), op, &path_params, &request);

    assert!(!errors.is_empty());
}

/// An object header parameter serialised in `simple` style decodes cleanly before schema checks.
#[test]
fn test_object_header_param_simple_style_validates() {
    let yaml = r#"
openapi: 3.0.0
info: { title: t, version: 1.0.0 }
paths:
  /x:
    post:
      parameters:
        - name: X-Filter
          in: header
          required: true
          schema:
            type: object
            properties:
              a: { type: string }
              b: { type: string }
            required: [a, b]
      responses:
        '200': { description: ok }
"#;
    let spec = load_spec(yaml).expect("spec");
    let op = op_for(&spec, "/x", "POST");
    let mut headers = HashMap::new();
    headers.insert("x-filter".to_string(), "a,1,b,2".to_string());
    headers.insert("content-type".to_string(), "application/json".to_string());
    let request = {
        let mut __r = MockRequest::default();
        __r.method = "POST".to_string();
        __r.path = "/x".to_string();
        __r.headers = headers;
        __r.query = HashMap::new();
        __r.body = None;
        __r
    };

    let errors = validate_request_full(&spec, Some("/x"), op, &HashMap::new(), &request);

    assert!(errors.is_empty(), "got {errors:?}");
}

/// A single-value `simple`-style array path-parameter is treated as a one-element array.
#[test]
fn test_simple_style_single_value_array_validates() {
    let yaml = r#"
openapi: 3.0.0
info: { title: t, version: 1.0.0 }
paths:
  /pets/{ids}:
    get:
      parameters:
        - name: ids
          in: path
          required: true
          schema: { type: array, items: { type: integer } }
      responses:
        '200': { description: ok }
"#;
    let spec = load_spec(yaml).expect("spec");
    let op = op_for(&spec, "/pets/{ids}", "GET");
    let mut path_params = HashMap::new();
    path_params.insert("ids".to_string(), "42".to_string());
    let request = {
        let mut __r = MockRequest::default();
        __r.method = "GET".to_string();
        __r.path = "/pets/42".to_string();
        __r.headers = HashMap::new();
        __r.query = HashMap::new();
        __r.body = None;
        __r
    };

    let errors = validate_request_full(&spec, Some("/pets/{ids}"), op, &path_params, &request);

    assert!(errors.is_empty(), "got {errors:?}");
}

/// An `anyOf` query schema with one scalar branch accepts a single scalar value.
#[test]
fn test_anyof_array_query_accepts_scalar() {
    let yaml = r#"
openapi: 3.0.0
info: { title: t, version: 1.0.0 }
paths:
  /things:
    get:
      parameters:
        - name: tag
          in: query
          schema:
            anyOf:
              - type: string
              - type: array
                items: { type: string }
      responses:
        '200': { description: ok }
"#;
    let spec = load_spec(yaml).expect("spec");
    let op = op_for(&spec, "/things", "GET");
    let mut query = HashMap::new();
    query.insert("tag".to_string(), "alpha".to_string());
    let request = {
        let mut __r = MockRequest::default();
        __r.method = "GET".to_string();
        __r.path = "/things".to_string();
        __r.headers = HashMap::new();
        __r.query = query;
        __r.body = None;
        __r
    };

    let errors = validate_request_full(&spec, Some("/things"), op, &HashMap::new(), &request);

    assert!(errors.is_empty(), "got {errors:?}");
}

/// A duplicate header joined with `, ` still satisfies an enum constraint written in the joined form.
#[test]
fn test_duplicate_header_values_joined_with_comma() {
    let yaml = r#"
openapi: 3.0.0
info: { title: t, version: 1.0.0 }
paths:
  /v:
    get:
      parameters:
        - name: X-Forwarded-For
          in: header
          required: true
          schema:
            type: string
            enum: ["1.1.1.1, 2.2.2.2"]
      responses:
        '200':
          description: ok
          content:
            application/json:
              example: { ok: true }
"#;
    let spec = load_spec(yaml).unwrap();
    let mut r = req("GET", "/v");
    r.headers
        .insert("X-Forwarded-For".into(), "1.1.1.1, 2.2.2.2".into());

    let resp = mock(&spec, &r, &errors_cfg()).unwrap();

    assert_eq!(resp.status, 200);
}

/// A duplicate query key joined with `,` satisfies a string pattern constraint.
#[test]
fn test_duplicate_query_keys_joined_with_comma() {
    let yaml = r#"
openapi: 3.0.0
info: { title: t, version: 1.0.0 }
paths:
  /search:
    get:
      parameters:
        - name: tag
          in: query
          required: true
          schema:
            type: string
            pattern: "^[a-z]+(,[a-z]+)+$"
      responses:
        '200':
          description: ok
          content:
            application/json:
              example: { ok: true }
"#;
    let spec = load_spec(yaml).unwrap();
    let mut r = req("GET", "/search");
    r.query.insert("tag".into(), "a,b".into());

    let resp = mock(&spec, &r, &errors_cfg()).unwrap();

    assert_eq!(resp.status, 200);
}

/// A percent-decoded query value is treated as a literal under a `pattern` constraint.
#[test]
fn test_query_value_percent_decoded() {
    let yaml = r#"
openapi: 3.0.0
info: { title: t, version: 1.0.0 }
paths:
  /search:
    get:
      parameters:
        - name: q
          in: query
          required: true
          schema:
            type: string
            pattern: "^[A-Za-z ]+$"
      responses:
        '200':
          description: ok
          content:
            application/json:
              example: { ok: true }
"#;
    let spec = load_spec(yaml).unwrap();
    let mut r = req("GET", "/search");
    r.query.insert("q".into(), "john doe".into());

    let resp = mock(&spec, &r, &errors_cfg()).unwrap();

    assert_eq!(resp.status, 200);
}

/// A non-integer path-parameter where the schema declares `type: integer` emits Path/petId/type.
#[test]
fn test_path_param_non_integer_yields_type_error() {
    let yaml = r#"
openapi: 3.0.0
info: { title: t, version: 1.0.0 }
paths:
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
"#;
    let spec = load_spec(yaml).expect("spec");
    let op = op_for(&spec, "/pets/{petId}", "GET");
    let mut path_params = HashMap::new();
    path_params.insert("petId".to_string(), "café".to_string());
    let request = {
        let mut __r = MockRequest::default();
        __r.method = "GET".to_string();
        __r.path = "/pets/café".to_string();
        __r.headers = HashMap::new();
        __r.query = HashMap::new();
        __r.body = None;
        __r
    };

    let errors = validate_request_full(&spec, Some("/pets/{petId}"), op, &path_params, &request);

    assert!(has_error(
        &errors,
        ValidationLocation::Path,
        "petId",
        "type"
    ));
}

/// A valid integer path-parameter value passes validation cleanly.
#[test]
fn test_path_param_integer_value_validates_cleanly() {
    let yaml = r#"
openapi: 3.0.0
info: { title: t, version: 1.0.0 }
paths:
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
"#;
    let spec = load_spec(yaml).expect("spec");
    let op = op_for(&spec, "/pets/{petId}", "GET");
    let mut path_params = HashMap::new();
    path_params.insert("petId".to_string(), "42".to_string());
    let request = {
        let mut __r = MockRequest::default();
        __r.method = "GET".to_string();
        __r.path = "/pets/42".to_string();
        __r.headers = HashMap::new();
        __r.query = HashMap::new();
        __r.body = None;
        __r
    };

    let errors = validate_request_full(&spec, Some("/pets/{petId}"), op, &path_params, &request);

    assert!(errors.is_empty(), "got {errors:?}");
}

/// A non-numeric query value where the schema declares `type: number` emits Query/weight/type.
#[test]
fn test_query_param_non_number_yields_type_error() {
    let yaml = r#"
openapi: 3.0.0
info: { title: t, version: 1.0.0 }
paths:
  /pets:
    get:
      parameters:
        - name: weight
          in: query
          schema: { type: number }
      responses:
        '200':
          description: ok
"#;
    let spec = load_spec(yaml).expect("spec");
    let op = op_for(&spec, "/pets", "GET");
    let mut query = HashMap::new();
    query.insert("weight".to_string(), "heavy".to_string());
    let request = {
        let mut __r = MockRequest::default();
        __r.method = "GET".to_string();
        __r.path = "/pets".to_string();
        __r.headers = HashMap::new();
        __r.query = query;
        __r.body = None;
        __r
    };

    let errors = validate_request_full(&spec, Some("/pets"), op, &HashMap::new(), &request);

    assert!(has_error(
        &errors,
        ValidationLocation::Query,
        "weight",
        "type"
    ));
}

/// A spec exercising the OAS3 `style: deepObject, explode: true` query
/// serialisation. The `filter` parameter is object-typed with a required
/// `name` field and an optional integer `age` field.
fn deep_object_spec() -> &'static str {
    r#"
openapi: 3.0.0
info: { title: t, version: 1.0.0 }
paths:
  /search:
    get:
      parameters:
        - in: query
          name: filter
          style: deepObject
          explode: true
          schema:
            type: object
            properties:
              name: { type: string }
              age: { type: integer }
            required: [name]
      responses:
        '200':
          description: ok
          content:
            application/json:
              example: { ok: true }
"#
}

/// A `style: deepObject` query parameter accepts the bracketed wire form
/// `?filter[name]=foo&filter[age]=42`, decoding each `<name>[<key>]=<value>`
/// entry into a synthetic JSON object and validating it against the
/// parameter's schema.
#[test]
fn test_deep_object_query_param_accepts_bracketed_form_with_all_fields() {
    let spec = load_spec(deep_object_spec()).unwrap();
    let mut r = req("GET", "/search");
    r.query.insert("filter[name]".into(), "foo".into());
    r.query.insert("filter[age]".into(), "42".into());

    let resp = mock(&spec, &r, &errors_cfg()).unwrap();

    assert_eq!(resp.status, 200);
}

/// Optional fields on a deepObject schema may be omitted; only the required
/// `name` must be present for validation to pass.
#[test]
fn test_deep_object_query_param_accepts_bracketed_form_with_only_required() {
    let spec = load_spec(deep_object_spec()).unwrap();
    let mut r = req("GET", "/search");
    r.query.insert("filter[name]".into(), "foo".into());

    let resp = mock(&spec, &r, &errors_cfg()).unwrap();

    assert_eq!(resp.status, 200);
}

/// Omitting a required field from a deepObject query parameter surfaces a
/// `required`-coded Query error scoped to the missing field. The field
/// identifier carries the `filter.` prefix so clients can pinpoint which
/// property of the object was missing.
#[test]
fn test_deep_object_query_param_missing_required_field_fails() {
    let spec = load_spec(deep_object_spec()).unwrap();
    let mut r = req("GET", "/search");
    r.query.insert("filter[age]".into(), "42".into());

    let err = mock(&spec, &r, &errors_cfg()).unwrap_err();

    match err {
        MockError::ValidationFailed(errors) => assert!(
            errors
                .iter()
                .any(|e| e.location == ValidationLocation::Query
                    && e.code == "required"
                    && (e.field == "filter" || e.field.starts_with("filter."))),
            "expected a Query/required error scoped to filter.*, got {errors:?}"
        ),
        other => panic!("expected ValidationFailed, got {other:?}"),
    }
}

/// A type-incompatible value supplied for a typed deepObject field surfaces a
/// `type`-coded Query error scoped to the offending field; the field
/// identifier carries the `filter.` prefix to pinpoint the property.
#[test]
fn test_deep_object_query_param_type_mismatch_fails() {
    let spec = load_spec(deep_object_spec()).unwrap();
    let mut r = req("GET", "/search");
    r.query.insert("filter[name]".into(), "foo".into());
    r.query.insert("filter[age]".into(), "not-an-int".into());

    let err = mock(&spec, &r, &errors_cfg()).unwrap_err();

    match err {
        MockError::ValidationFailed(errors) => assert!(
            errors
                .iter()
                .any(|e| e.location == ValidationLocation::Query
                    && e.code == "type"
                    && (e.field == "filter" || e.field.starts_with("filter."))),
            "expected a Query/type error scoped to filter.*, got {errors:?}"
        ),
        other => panic!("expected ValidationFailed, got {other:?}"),
    }
}

/// A `style: matrix` path parameter is accepted as if `style: simple` were
/// declared (chasm does not implement RFC 6570 matrix decoding). The
/// validation result is unchanged; the new signal is a one-time
/// `tracing::warn!` emitted by [`validate_request_full`].
#[test]
fn test_matrix_path_param_validates_as_simple_for_backcompat() {
    let yaml = r#"
openapi: 3.0.0
info: { title: t, version: 1.0.0 }
paths:
  /pets/{color}:
    get:
      operationId: getByColor
      parameters:
        - name: color
          in: path
          required: true
          style: matrix
          schema: { type: string }
      responses:
        '200':
          description: ok
"#;
    let spec = load_spec(yaml).expect("spec");
    let op = op_for(&spec, "/pets/{color}", "GET");
    let mut path_params = HashMap::new();
    path_params.insert("color".to_string(), "blue".to_string());
    let request = {
        let mut __r = MockRequest::default();
        __r.method = "GET".to_string();
        __r.path = "/pets/blue".to_string();
        __r.headers = HashMap::new();
        __r.query = HashMap::new();
        __r.body = None;
        __r
    };

    let errors = validate_request_full(&spec, Some("/pets/{color}"), op, &path_params, &request);

    assert!(
        errors.is_empty(),
        "matrix-style path parameter must validate as simple for backcompat; got {errors:?}"
    );
}
