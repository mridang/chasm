//! JSON body schema validation tests.

use super::*;
use chasm_engine::validation::parse_request_body_for_validation_strict;

/// A missing required body surfaces as a Body/body error.
#[test]
fn test_missing_required_body_emits_body_error() {
    let spec = load_spec(validation_spec()).unwrap();
    let r = req("POST", "/pets/dog");

    let err = mock(&spec, &r, &errors_cfg()).unwrap_err();

    match err {
        MockError::ValidationFailed(errors) => assert!(errors
            .iter()
            .any(|e| e.field == "body" && e.location == ValidationLocation::Body)),
        other => panic!("expected ValidationFailed, got {other:?}"),
    }
}

/// A POST body whose `age` is below the schema's `minimum: 0` emits a single Body/body.age/minimum diagnostic.
#[test]
fn test_body_schema_violation_emits_validation_failed() {
    let spec = load_spec(validation_spec()).unwrap();
    let mut r = req("POST", "/pets/dog");
    r.headers
        .insert("Content-Type".into(), "application/json".into());
    r.body = Some(serde_json::json!({ "name": "rex", "age": -1 }));

    let err = mock(&spec, &r, &errors_cfg()).unwrap_err();

    match err {
        MockError::ValidationFailed(errors) => assert!(has_error(
            &errors,
            ValidationLocation::Body,
            "body.age",
            "minimum"
        )),
        other => panic!("expected ValidationFailed, got {other:?}"),
    }
}

/// An integer body value that is not a multiple of the schema's `multipleOf` emits a `multipleOf` error.
#[test]
fn test_multiple_of_integer_violation() {
    let yaml = r#"
openapi: 3.0.0
info: { title: t, version: 1.0.0 }
paths:
  /n:
    post:
      requestBody:
        required: true
        content:
          application/json:
            schema:
              type: object
              properties:
                n:
                  type: integer
                  multipleOf: 3
      responses:
        '200':
          description: ok
          content:
            application/json:
              example: { ok: true }
"#;
    let spec = load_spec(yaml).unwrap();
    let mut r = req("POST", "/n");
    r.headers
        .insert("Content-Type".into(), "application/json".into());
    r.body = Some(serde_json::json!({ "n": 7 }));

    let err = mock(&spec, &r, &errors_cfg()).unwrap_err();

    match err {
        MockError::ValidationFailed(errors) => {
            assert!(errors.iter().any(|e| e.message.contains("multiple of")))
        }
        other => panic!("expected ValidationFailed, got {other:?}"),
    }
}

/// A floating-point value that is not a multiple of the schema's `multipleOf: 0.3` emits a `multipleOf` error.
#[test]
fn test_multiple_of_number_violation() {
    let yaml = r#"
openapi: 3.0.0
info: { title: t, version: 1.0.0 }
paths:
  /n:
    post:
      requestBody:
        required: true
        content:
          application/json:
            schema:
              type: object
              properties:
                n:
                  type: number
                  multipleOf: 0.3
      responses:
        '200':
          description: ok
          content:
            application/json:
              example: { ok: true }
"#;
    let spec = load_spec(yaml).unwrap();
    let mut r = req("POST", "/n");
    r.headers
        .insert("Content-Type".into(), "application/json".into());
    r.body = Some(serde_json::json!({ "n": 0.7 }));

    let err = mock(&spec, &r, &errors_cfg()).unwrap_err();

    match err {
        MockError::ValidationFailed(errors) => {
            assert!(errors.iter().any(|e| e.message.contains("multiple of")))
        }
        other => panic!("expected ValidationFailed, got {other:?}"),
    }
}

/// A `uniqueItems: true` array with a duplicate emits a `not unique` error.
#[test]
fn test_unique_items_violation() {
    let yaml = r#"
openapi: 3.0.0
info: { title: t, version: 1.0.0 }
paths:
  /t:
    post:
      requestBody:
        required: true
        content:
          application/json:
            schema:
              type: object
              properties:
                tags:
                  type: array
                  uniqueItems: true
                  items: { type: string }
      responses:
        '200':
          description: ok
          content:
            application/json:
              example: { ok: true }
"#;
    let spec = load_spec(yaml).unwrap();
    let mut r = req("POST", "/t");
    r.headers
        .insert("Content-Type".into(), "application/json".into());
    r.body = Some(serde_json::json!({ "tags": ["a", "b", "a"] }));

    let err = mock(&spec, &r, &errors_cfg()).unwrap_err();

    match err {
        MockError::ValidationFailed(errors) => {
            assert!(errors.iter().any(|e| e.message.contains("not unique")))
        }
        other => panic!("expected ValidationFailed, got {other:?}"),
    }
}

/// `additionalProperties: false` rejects any key not declared in `properties`.
#[test]
fn test_additional_properties_false_rejects_unknown_keys() {
    let yaml = r#"
openapi: 3.0.0
info: { title: t, version: 1.0.0 }
paths:
  /o:
    post:
      requestBody:
        required: true
        content:
          application/json:
            schema:
              type: object
              additionalProperties: false
              properties:
                name: { type: string }
      responses:
        '200':
          description: ok
          content:
            application/json:
              example: { ok: true }
"#;
    let spec = load_spec(yaml).unwrap();
    let mut r = req("POST", "/o");
    r.headers
        .insert("Content-Type".into(), "application/json".into());
    r.body = Some(serde_json::json!({ "name": "x", "unknown": 1 }));

    let err = mock(&spec, &r, &errors_cfg()).unwrap_err();

    match err {
        MockError::ValidationFailed(errors) => assert!(errors
            .iter()
            .any(|e| e.field.ends_with(".unknown") && e.message.contains("additional property"))),
        other => panic!("expected ValidationFailed, got {other:?}"),
    }
}

/// A string `pattern` mismatch emits a `pattern` error.
#[test]
fn test_string_pattern_violation() {
    let yaml = r#"
openapi: 3.0.0
info: { title: t, version: 1.0.0 }
paths:
  /s:
    post:
      requestBody:
        required: true
        content:
          application/json:
            schema:
              type: object
              properties:
                code:
                  type: string
                  pattern: "^[A-Z]{3}$"
      responses:
        '200':
          description: ok
          content:
            application/json:
              example: { ok: true }
"#;
    let spec = load_spec(yaml).unwrap();
    let mut r = req("POST", "/s");
    r.headers
        .insert("Content-Type".into(), "application/json".into());
    r.body = Some(serde_json::json!({ "code": "ab1" }));

    let err = mock(&spec, &r, &errors_cfg()).unwrap_err();

    match err {
        MockError::ValidationFailed(errors) => {
            assert!(errors.iter().any(|e| e.message.contains("pattern")))
        }
        other => panic!("expected ValidationFailed, got {other:?}"),
    }
}

/// A pathologically long `pattern` source (70 KiB) is skipped by the
/// per-use compile-site cap rather than compiled into a regex. A spec
/// declaring such a pattern must load successfully and, on a request whose
/// value would obviously fail it, must produce NO `pattern` error — the
/// pattern is treated as unsupported, mirroring the cached_regex `None`
/// fallback behaviour. This is the DoS guard against giant literal patterns.
#[test]
fn test_oversized_pattern_skipped_not_compiled() {
    let big_pattern = "a".repeat(70 * 1024);
    let yaml = format!(
        r#"
openapi: 3.0.0
info: {{ title: t, version: 1.0.0 }}
paths:
  /s:
    post:
      requestBody:
        required: true
        content:
          application/json:
            schema:
              type: object
              properties:
                code:
                  type: string
                  pattern: "{}"
      responses:
        '200':
          description: ok
          content:
            application/json:
              example: {{ ok: true }}
"#,
        big_pattern
    );
    let spec = load_spec(&yaml).expect("spec with oversized pattern must still load");
    let mut r = req("POST", "/s");
    r.headers
        .insert("Content-Type".into(), "application/json".into());
    r.body = Some(serde_json::json!({ "code": "xx" }));

    match mock(&spec, &r, &errors_cfg()) {
        Ok(_) => {}
        Err(MockError::ValidationFailed(errors)) => {
            assert!(
                !errors.iter().any(|e| e.code == "pattern"),
                "oversized pattern must be skipped, not compiled+enforced"
            );
        }
        Err(other) => panic!("unexpected error variant: {other:?}"),
    }
}

/// A string under-running `minLength` emits a `minLength` error.
#[test]
fn test_string_min_length_violation() {
    let yaml = r#"
openapi: 3.0.0
info: { title: t, version: 1.0.0 }
paths:
  /s:
    post:
      requestBody:
        required: true
        content:
          application/json:
            schema:
              type: object
              properties:
                name:
                  type: string
                  minLength: 5
                  maxLength: 10
      responses:
        '200':
          description: ok
          content:
            application/json:
              example: { ok: true }
"#;
    let spec = load_spec(yaml).unwrap();
    let mut r = req("POST", "/s");
    r.headers
        .insert("Content-Type".into(), "application/json".into());
    r.body = Some(serde_json::json!({ "name": "ab" }));

    let err = mock(&spec, &r, &errors_cfg()).unwrap_err();

    match err {
        MockError::ValidationFailed(errors) => {
            assert!(errors.iter().any(|e| e.message.contains("minLength")))
        }
        other => panic!("expected ValidationFailed, got {other:?}"),
    }
}

/// A string exceeding `maxLength` emits a `maxLength` error.
#[test]
fn test_string_max_length_violation() {
    let yaml = r#"
openapi: 3.0.0
info: { title: t, version: 1.0.0 }
paths:
  /s:
    post:
      requestBody:
        required: true
        content:
          application/json:
            schema:
              type: object
              properties:
                name:
                  type: string
                  maxLength: 3
      responses:
        '200':
          description: ok
          content:
            application/json:
              example: { ok: true }
"#;
    let spec = load_spec(yaml).unwrap();
    let mut r = req("POST", "/s");
    r.headers
        .insert("Content-Type".into(), "application/json".into());
    r.body = Some(serde_json::json!({ "name": "abcdef" }));

    let err = mock(&spec, &r, &errors_cfg()).unwrap_err();

    match err {
        MockError::ValidationFailed(errors) => {
            assert!(errors.iter().any(|e| e.message.contains("maxLength")))
        }
        other => panic!("expected ValidationFailed, got {other:?}"),
    }
}

/// An array under-running `minItems` emits a `minItems` error.
#[test]
fn test_array_min_items_violation() {
    let yaml = r#"
openapi: 3.0.0
info: { title: t, version: 1.0.0 }
paths:
  /a:
    post:
      requestBody:
        required: true
        content:
          application/json:
            schema:
              type: object
              properties:
                tags:
                  type: array
                  minItems: 3
                  maxItems: 5
                  items: { type: string }
      responses:
        '200':
          description: ok
          content:
            application/json:
              example: { ok: true }
"#;
    let spec = load_spec(yaml).unwrap();
    let mut r = req("POST", "/a");
    r.headers
        .insert("Content-Type".into(), "application/json".into());
    r.body = Some(serde_json::json!({ "tags": ["a"] }));

    let err = mock(&spec, &r, &errors_cfg()).unwrap_err();

    match err {
        MockError::ValidationFailed(errors) => {
            assert!(errors.iter().any(|e| e.message.contains("minItems")))
        }
        other => panic!("expected ValidationFailed, got {other:?}"),
    }
}

/// An array exceeding `maxItems` emits a `maxItems` error.
#[test]
fn test_array_max_items_violation() {
    let yaml = r#"
openapi: 3.0.0
info: { title: t, version: 1.0.0 }
paths:
  /a:
    post:
      requestBody:
        required: true
        content:
          application/json:
            schema:
              type: object
              properties:
                tags:
                  type: array
                  maxItems: 2
                  items: { type: string }
      responses:
        '200':
          description: ok
          content:
            application/json:
              example: { ok: true }
"#;
    let spec = load_spec(yaml).unwrap();
    let mut r = req("POST", "/a");
    r.headers
        .insert("Content-Type".into(), "application/json".into());
    r.body = Some(serde_json::json!({ "tags": ["a", "b", "c", "d"] }));

    let err = mock(&spec, &r, &errors_cfg()).unwrap_err();

    match err {
        MockError::ValidationFailed(errors) => {
            assert!(errors.iter().any(|e| e.message.contains("maxItems")))
        }
        other => panic!("expected ValidationFailed, got {other:?}"),
    }
}

/// A `$ref`-component schema on the body is resolved and enforced.
#[test]
fn test_body_ref_component_schema_enforced() {
    let yaml = r#"
openapi: 3.0.0
info: { title: t, version: 1.0.0 }
paths:
  /p:
    post:
      requestBody:
        required: true
        content:
          application/json:
            schema:
              $ref: '#/components/schemas/Pet'
      responses:
        '200':
          description: ok
          content:
            application/json:
              example: { ok: true }
components:
  schemas:
    Pet:
      type: object
      required: [name]
      properties:
        name: { type: string }
"#;
    let spec = load_spec(yaml).unwrap();
    let mut r = req("POST", "/p");
    r.headers
        .insert("Content-Type".into(), "application/json".into());
    r.body = Some(serde_json::json!({}));

    let err = mock(&spec, &r, &errors_cfg()).unwrap_err();

    match err {
        MockError::ValidationFailed(errors) => assert!(errors
            .iter()
            .any(|e| e.field.ends_with(".name") && e.message.contains("missing"))),
        other => panic!("expected ValidationFailed, got {other:?}"),
    }
}

/// A string body field declared with `format: email` emits a `format` error on bad input.
#[test]
fn test_format_email_rejects_invalid() {
    let yaml = r#"
openapi: 3.0.0
info: { title: t, version: 1.0.0 }
paths:
  /u:
    post:
      requestBody:
        required: true
        content:
          application/json:
            schema:
              type: object
              properties:
                email:
                  type: string
                  format: email
      responses:
        '200':
          description: ok
          content:
            application/json:
              example: { ok: true }
"#;
    let spec = load_spec(yaml).unwrap();
    let mut r = req("POST", "/u");
    r.headers
        .insert("Content-Type".into(), "application/json".into());
    r.body = Some(serde_json::json!({ "email": "not-an-email" }));

    let err = mock(&spec, &r, &errors_cfg()).unwrap_err();

    match err {
        MockError::ValidationFailed(errors) => assert!(errors
            .iter()
            .any(|e| e.code == "format" && e.message.contains("email"))),
        other => panic!("expected ValidationFailed, got {other:?}"),
    }
}

/// A string body field declared with `format: uuid` emits a `format` error on bad input.
#[test]
fn test_format_uuid_rejects_invalid() {
    let yaml = r#"
openapi: 3.0.0
info: { title: t, version: 1.0.0 }
paths:
  /u:
    post:
      requestBody:
        required: true
        content:
          application/json:
            schema:
              type: object
              properties:
                id:
                  type: string
                  format: uuid
      responses:
        '200':
          description: ok
          content:
            application/json:
              example: { ok: true }
"#;
    let spec = load_spec(yaml).unwrap();
    let mut r = req("POST", "/u");
    r.headers
        .insert("Content-Type".into(), "application/json".into());
    r.body = Some(serde_json::json!({ "id": "not-a-uuid" }));

    let err = mock(&spec, &r, &errors_cfg()).unwrap_err();

    match err {
        MockError::ValidationFailed(errors) => assert!(errors
            .iter()
            .any(|e| e.code == "format" && e.message.contains("uuid"))),
        other => panic!("expected ValidationFailed, got {other:?}"),
    }
}

/// A string body field declared with `format: date-time` emits a `format` error on bad input.
#[test]
fn test_format_date_time_rejects_invalid() {
    let yaml = r#"
openapi: 3.0.0
info: { title: t, version: 1.0.0 }
paths:
  /e:
    post:
      requestBody:
        required: true
        content:
          application/json:
            schema:
              type: object
              properties:
                at:
                  type: string
                  format: date-time
      responses:
        '200':
          description: ok
          content:
            application/json:
              example: { ok: true }
"#;
    let spec = load_spec(yaml).unwrap();
    let mut r = req("POST", "/e");
    r.headers
        .insert("Content-Type".into(), "application/json".into());
    r.body = Some(serde_json::json!({ "at": "yesterday" }));

    let err = mock(&spec, &r, &errors_cfg()).unwrap_err();

    match err {
        MockError::ValidationFailed(errors) => assert!(errors
            .iter()
            .any(|e| e.code == "format" && e.message.contains("date-time"))),
        other => panic!("expected ValidationFailed, got {other:?}"),
    }
}

/// `format: date-time` accepts numeric offsets such as `+02:00`, not just `Z`.
#[test]
fn test_format_date_time_accepts_numeric_offset() {
    let yaml = r#"
openapi: 3.0.0
info: { title: t, version: 1.0.0 }
paths:
  /x:
    post:
      requestBody:
        required: true
        content:
          application/json:
            schema:
              type: object
              properties:
                expires_at:
                  type: string
                  format: date-time
      responses:
        '200': { description: ok }
"#;
    let spec = load_spec(yaml).expect("spec");
    let op = op_for(&spec, "/x", "POST");

    let r = post_request(
        "application/json",
        Some(serde_json::json!({ "expires_at": "2021-02-18T12:02:16.49+02:00" })),
    );
    let errors = validate_request_full(&spec, Some("/x"), op, &HashMap::new(), &r);

    assert!(
        errors.is_empty(),
        "expected clean validation, got {errors:?}"
    );
}

/// A `not` clause rejects values that match the inverted sub-schema.
#[test]
fn test_not_rejects_match() {
    let yaml = r#"
openapi: 3.0.0
info: { title: t, version: 1.0.0 }
paths:
  /n:
    post:
      requestBody:
        required: true
        content:
          application/json:
            schema:
              type: object
              properties:
                v:
                  not:
                    type: string
      responses:
        '200':
          description: ok
          content:
            application/json:
              example: { ok: true }
"#;
    let spec = load_spec(yaml).unwrap();
    let mut r = req("POST", "/n");
    r.headers
        .insert("Content-Type".into(), "application/json".into());
    r.body = Some(serde_json::json!({ "v": "hello" }));

    let err = mock(&spec, &r, &errors_cfg()).unwrap_err();

    match err {
        MockError::ValidationFailed(errors) => {
            assert!(errors.iter().any(|e| e.code == "not"))
        }
        other => panic!("expected ValidationFailed, got {other:?}"),
    }
}

/// An invalid JSON body surfaces as a single `(Body, body, type)` validation error.
#[test]
fn test_invalid_json_body_yields_type_error() {
    let outcome = parse_request_body_for_validation_strict("application/json", b"{not json}");

    let err = outcome.expect_err("invalid JSON must surface a ValidationError");

    assert_eq!(
        (err.location, err.field.as_str(), err.code.as_str()),
        (ValidationLocation::Body, "body", "type")
    );
}

/// An optional empty JSON body validates cleanly.
#[test]
fn test_optional_empty_json_body_passes() {
    let yaml = r#"
openapi: 3.0.0
info: { title: t, version: 1.0.0 }
paths:
  /x:
    post:
      requestBody:
        required: false
        content:
          application/json:
            schema: { type: object }
      responses:
        '200': { description: ok }
"#;
    let spec = load_spec(yaml).expect("spec");
    let op = op_for(&spec, "/x", "POST");
    let r = post_request("application/json", None);

    let errors = validate_request_full(&spec, Some("/x"), op, &HashMap::new(), &r);

    assert!(errors.is_empty(), "got {errors:?}");
}

/// A required missing JSON body emits a `code = required` entry on the body.
#[test]
fn test_required_empty_json_body_emits_required_code() {
    let yaml = r#"
openapi: 3.0.0
info: { title: t, version: 1.0.0 }
paths:
  /x:
    post:
      requestBody:
        required: true
        content:
          application/json:
            schema: { type: object }
      responses:
        '200': { description: ok }
"#;
    let spec = load_spec(yaml).expect("spec");
    let op = op_for(&spec, "/x", "POST");
    let r = post_request("application/json", None);

    let errors = validate_request_full(&spec, Some("/x"), op, &HashMap::new(), &r);

    assert!(has_error(
        &errors,
        ValidationLocation::Body,
        "body",
        "required"
    ));
}

/// A required missing JSON body surfaces exactly one `required` diagnostic
/// and no `type` diagnostic — pairing the negation with a positive presence
/// check proves that validation actually ran.
#[test]
fn test_required_empty_json_body_does_not_emit_type_code() {
    let yaml = r#"
openapi: 3.0.0
info: { title: t, version: 1.0.0 }
paths:
  /x:
    post:
      requestBody:
        required: true
        content:
          application/json:
            schema: { type: object }
      responses:
        '200': { description: ok }
"#;
    let spec = load_spec(yaml).expect("spec");
    let op = op_for(&spec, "/x", "POST");
    let r = post_request("application/json", None);

    let errors = validate_request_full(&spec, Some("/x"), op, &HashMap::new(), &r);

    assert_eq!(
        errors.iter().map(|e| e.code.as_str()).collect::<Vec<_>>(),
        vec!["required"],
        "expected exactly one 'required' diagnostic, got {:?}",
        errors
    );
}

/// A missing required property in a JSON body emits a `required` error on the property path.
#[test]
fn test_required_property_emits_required_error() {
    let yaml = r#"
openapi: 3.0.0
info: { title: t, version: 1.0.0 }
paths:
  /x:
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
      responses:
        '200': { description: ok }
"#;
    let spec = load_spec(yaml).expect("spec");
    let op = op_for(&spec, "/x", "POST");
    let r = post_request("application/json", Some(serde_json::json!({})));

    let errors = validate_request_full(&spec, Some("/x"), op, &HashMap::new(), &r);

    assert!(has_error(
        &errors,
        ValidationLocation::Body,
        "body.name",
        "required"
    ));
}

/// `allOf` with a `$ref` enforces `required` declared inside the referenced schema.
#[test]
fn test_allof_ref_required_is_enforced() {
    let yaml = r#"
openapi: 3.0.0
info: { title: t, version: 1.0.0 }
paths:
  /x:
    post:
      requestBody:
        required: true
        content:
          application/json:
            schema:
              allOf:
                - $ref: '#/components/schemas/Base'
      responses:
        '200': { description: ok }
components:
  schemas:
    Base:
      type: object
      required: [name]
      properties:
        name: { type: string }
"#;
    let spec = load_spec(yaml).expect("spec");
    let op = op_for(&spec, "/x", "POST");
    let r = post_request("application/json", Some(serde_json::json!({})));

    let errors = validate_request_full(&spec, Some("/x"), op, &HashMap::new(), &r);

    assert!(has_error(
        &errors,
        ValidationLocation::Body,
        "body.name",
        "required"
    ));
}

/// `allOf` combining a `$ref` branch with `additionalProperties: false` treats `$ref` properties as declared.
#[test]
fn test_allof_ref_with_additional_properties_false_no_false_positive() {
    let yaml = r#"
openapi: 3.0.0
info: { title: t, version: 1.0.0 }
paths:
  /x:
    post:
      requestBody:
        required: true
        content:
          application/json:
            schema:
              allOf:
                - $ref: '#/components/schemas/Base'
                - additionalProperties: false
      responses:
        '200': { description: ok }
components:
  schemas:
    Base:
      type: object
      required: [name]
      properties:
        name: { type: string }
        age: { type: integer }
"#;
    let spec = load_spec(yaml).expect("spec");
    let op = op_for(&spec, "/x", "POST");
    let r = post_request(
        "application/json",
        Some(serde_json::json!({ "name": "ada", "age": 36 })),
    );

    let errors = validate_request_full(&spec, Some("/x"), op, &HashMap::new(), &r);

    assert!(errors.is_empty(), "got {errors:?}");
}

/// A `oneOf` schema with a discriminator short-circuits to the named branch.
#[test]
fn test_oneof_discriminator_short_circuits() {
    let yaml = r#"
openapi: 3.0.0
info: { title: t, version: 1.0.0 }
paths:
  /pets:
    post:
      requestBody:
        required: true
        content:
          application/json:
            schema:
              oneOf:
                - $ref: '#/components/schemas/Cat'
                - $ref: '#/components/schemas/Dog'
              discriminator:
                propertyName: kind
                mapping:
                  cat: '#/components/schemas/Cat'
                  dog: '#/components/schemas/Dog'
      responses:
        '200': { description: ok }
components:
  schemas:
    Cat:
      type: object
      required: [kind, name]
      properties:
        kind: { type: string, enum: [cat] }
        name: { type: string }
    Dog:
      type: object
      required: [kind, name]
      properties:
        kind: { type: string, enum: [dog] }
        name: { type: string }
"#;
    let spec = load_spec(yaml).expect("spec");
    let op = op_for(&spec, "/pets", "POST");
    let r = post_request(
        "application/json",
        Some(serde_json::json!({ "kind": "cat", "name": "Mittens" })),
    );

    let errors = validate_request_full(&spec, Some("/pets"), op, &HashMap::new(), &r);

    assert!(errors.is_empty(), "got {errors:?}");
}

/// A top-level array body validates against the body schema directly.
#[test]
fn test_top_level_array_body_validates() {
    let yaml = r#"
openapi: 3.0.0
info: { title: t, version: 1.0.0 }
paths:
  /tags:
    post:
      requestBody:
        required: true
        content:
          application/json:
            schema:
              type: array
              items: { type: string }
      responses:
        '200': { description: ok }
"#;
    let spec = load_spec(yaml).expect("spec");
    let op = op_for(&spec, "/tags", "POST");
    let mut headers = HashMap::new();
    headers.insert("content-type".to_string(), "application/json".to_string());
    let request = {
        let mut __r = MockRequest::default();
        __r.method = "POST".to_string();
        __r.path = "/tags".to_string();
        __r.headers = headers;
        __r.query = HashMap::new();
        __r.body = Some(serde_json::json!(["alpha", "beta", "gamma"]));
        __r
    };

    let errors = validate_request_full(&spec, Some("/tags"), op, &HashMap::new(), &request);

    assert!(errors.is_empty(), "got {errors:?}");
}

/// An `application/xml` body bypasses JSON-schema validation entirely.
#[test]
fn test_application_xml_body_skips_schema_validation() {
    let yaml = r#"
openapi: 3.0.0
info: { title: t, version: 1.0.0 }
paths:
  /x:
    post:
      requestBody:
        required: true
        content:
          application/xml:
            schema:
              type: object
              properties:
                name: { type: string }
              required: [name]
      responses:
        '200': { description: ok }
"#;
    let spec = load_spec(yaml).expect("spec");
    let op = op_for(&spec, "/x", "POST");
    let mut headers = HashMap::new();
    headers.insert("content-type".to_string(), "application/xml".to_string());
    let request = {
        let mut __r = MockRequest::default();
        __r.method = "POST".to_string();
        __r.path = "/x".to_string();
        __r.headers = headers;
        __r.query = HashMap::new();
        __r.body = Some(serde_json::Value::String(
            "<doc><name>foo</name></doc>".to_string(),
        ));
        __r
    };

    let errors = validate_request_full(&spec, Some("/x"), op, &HashMap::new(), &request);

    assert!(errors.is_empty(), "got {errors:?}");
}

/// A loader rewrite converts the OAS 3.1 numeric `exclusiveMinimum` form so validation enforces the bound.
#[test]
fn test_exclusive_minimum_numeric_form_enforced() {
    let yaml = r#"
openapi: 3.0.0
info: { title: t, version: 1.0.0 }
paths:
  /count:
    post:
      requestBody:
        required: true
        content:
          application/json:
            schema:
              type: integer
              exclusiveMinimum: 5
      responses:
        '200':
          description: ok
"#;
    let spec = load_spec(yaml).expect("spec");
    let mut r = req("POST", "/count");
    r.headers
        .insert("content-type".into(), "application/json".into());
    r.body = Some(serde_json::json!(5));

    let err = mock(&spec, &r, &errors_cfg()).expect_err("body=5 must violate exclusiveMinimum:5");

    match err {
        MockError::ValidationFailed(errors) => {
            assert!(errors
                .iter()
                .any(|e| e.location == ValidationLocation::Body
                    && e.code.contains("exclusiveMinimum")))
        }
        other => panic!("expected ValidationFailed, got {other:?}"),
    }
}

/// A value above the exclusive lower bound passes validation under the rewritten `exclusiveMinimum`.
#[test]
fn test_exclusive_minimum_accepts_value_above_bound() {
    let yaml = r#"
openapi: 3.0.0
info: { title: t, version: 1.0.0 }
paths:
  /count:
    post:
      requestBody:
        required: true
        content:
          application/json:
            schema:
              type: integer
              exclusiveMinimum: 5
      responses:
        '200':
          description: ok
"#;
    let spec = load_spec(yaml).expect("spec");
    let mut r = req("POST", "/count");
    r.headers
        .insert("content-type".into(), "application/json".into());
    r.body = Some(serde_json::json!(6));

    let resp = mock(&spec, &r, &errors_cfg()).unwrap();

    assert_eq!(resp.status, 200);
}

/// A shared `$ref` body schema is validated independently per request.
#[test]
fn test_body_validated_when_schema_shared_across_operations() {
    let yaml = r#"
openapi: 3.0.0
info: { title: t, version: 1.0.0 }
paths:
  /a:
    post:
      requestBody:
        required: true
        content:
          application/json:
            schema:
              $ref: '#/components/schemas/Widget'
      responses:
        '200':
          description: ok
  /b:
    post:
      requestBody:
        required: true
        content:
          application/json:
            schema:
              $ref: '#/components/schemas/Widget'
      responses:
        '200':
          description: ok
components:
  schemas:
    Widget:
      type: object
      required: [id, name]
      properties:
        id:
          type: integer
        name:
          type: string
"#;
    let spec = load_spec(yaml).expect("spec");
    let mut bad = req("POST", "/b");
    bad.headers
        .insert("content-type".into(), "application/json".into());
    bad.body = Some(serde_json::json!({"id": "not-an-int"}));

    let err = mock(&spec, &bad, &errors_cfg()).unwrap_err();

    match err {
        MockError::ValidationFailed(errors) => assert!(errors
            .iter()
            .any(|e| e.location == ValidationLocation::Body)),
        other => panic!("expected ValidationFailed, got {other:?}"),
    }
}

/// A request whose body is empty but the spec requires it surfaces a "body is required" entry.
#[test]
fn test_request_body_required_with_empty_body_fails() {
    let yaml = r#"
openapi: 3.0.0
info: { title: t, version: 1.0.0 }
paths:
  /submit:
    post:
      requestBody:
        required: true
        content:
          application/json:
            schema:
              type: object
              properties:
                name: { type: string }
      responses:
        '200':
          description: ok
"#;
    let spec = load_spec(yaml).unwrap();
    let mut r = req("POST", "/submit");
    r.headers
        .insert("Content-Type".into(), "application/json".into());

    let err = mock(&spec, &r, &errors_cfg()).unwrap_err();

    match err {
        MockError::ValidationFailed(errors) => assert!(errors
            .iter()
            .any(|e| e.field == "body" && e.message == "body is required")),
        other => panic!("expected ValidationFailed, got {other:?}"),
    }
}

/// A non-JSON binary body delivered as a placeholder string passes a binary upload spec.
#[test]
fn test_binary_body_placeholder_does_not_trigger_required_error() {
    let yaml = r#"
openapi: 3.0.0
info: { title: t, version: 1.0.0 }
paths:
  /upload:
    post:
      requestBody:
        required: true
        content:
          application/octet-stream:
            schema:
              type: string
              format: binary
      responses:
        '200':
          description: ok
          content:
            application/json:
              example: { ok: true }
"#;
    let spec = load_spec(yaml).unwrap();
    let mut r = req("POST", "/upload");
    r.headers
        .insert("Content-Type".into(), "application/octet-stream".into());
    r.body = Some(serde_json::Value::String(
        "<binary application/octet-stream>".to_string(),
    ));

    let resp = mock(&spec, &r, &errors_cfg()).unwrap();

    assert_eq!(resp.status, 200);
}

/// A request body matching zero `oneOf` branches emits a `oneOf` error reporting `0 of N`.
#[test]
fn test_one_of_zero_match_yields_error() {
    let yaml = r#"
openapi: 3.0.0
info: { title: t, version: 1.0.0 }
paths:
  /x:
    post:
      requestBody:
        required: true
        content:
          application/json:
            schema:
              oneOf:
                - type: object
                  required: [admin]
                  properties: { admin: { type: boolean } }
                - type: object
                  required: [user]
                  properties: { user: { type: string } }
      responses:
        '200': { description: ok }
"#;
    let spec = load_spec(yaml).expect("spec");
    let op = op_for(&spec, "/x", "POST");
    let request = post_request(
        "application/json",
        Some(serde_json::json!({ "neither": true })),
    );

    let errors = validate_request_full(&spec, Some("/x"), op, &HashMap::new(), &request);

    assert!(errors
        .iter()
        .any(|e| e.code == "oneOf" && e.message.contains("0 of 2")));
}

/// A request body matching every `oneOf` branch emits a `oneOf` error reporting the actual count.
#[test]
fn test_one_of_multi_match_yields_error() {
    let yaml = r#"
openapi: 3.0.0
info: { title: t, version: 1.0.0 }
paths:
  /x:
    post:
      requestBody:
        required: true
        content:
          application/json:
            schema:
              oneOf:
                - type: object
                  properties: { name: { type: string } }
                - type: object
                  properties: { name: { type: string } }
      responses:
        '200': { description: ok }
"#;
    let spec = load_spec(yaml).expect("spec");
    let op = op_for(&spec, "/x", "POST");
    let request = post_request("application/json", Some(serde_json::json!({ "name": "x" })));

    let errors = validate_request_full(&spec, Some("/x"), op, &HashMap::new(), &request);

    assert!(errors
        .iter()
        .any(|e| e.code == "oneOf" && e.message.contains("2 of 2")));
}

/// An integer path-parameter value that overflows `i64` emits `format/int64`, not `type`.
#[test]
fn test_integer_overflow_yields_format_error() {
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
        '200': { description: ok }
"#;
    let spec = load_spec(yaml).expect("spec");
    let op = op_for(&spec, "/pets/{petId}", "GET");
    let mut path_params = HashMap::new();
    path_params.insert("petId".to_string(), "9999999999999999999".to_string());
    let request = {
        let mut __r = MockRequest::default();
        __r.method = "GET".to_string();
        __r.path = "/pets/9999999999999999999".to_string();
        __r.headers = HashMap::new();
        __r.query = HashMap::new();
        __r.body = None;
        __r
    };

    let errors = validate_request_full(&spec, Some("/pets/{petId}"), op, &path_params, &request);

    assert!(errors
        .iter()
        .any(|e| e.code == "format" && e.message.contains("int64")));
}

/// A float-enum value within a relative tolerance of an allowed entry is accepted.
#[test]
fn test_float_enum_within_relative_tolerance() {
    let yaml = r#"
openapi: 3.0.0
info: { title: t, version: 1.0.0 }
paths:
  /x:
    post:
      requestBody:
        required: true
        content:
          application/json:
            schema:
              type: object
              properties:
                ratio:
                  type: number
                  enum: [1000000.0]
      responses:
        '200': { description: ok }
"#;
    let spec = load_spec(yaml).expect("spec");
    let op = op_for(&spec, "/x", "POST");
    let request = post_request(
        "application/json",
        Some(serde_json::json!({ "ratio": 1_000_000.0 + 1e-10 })),
    );

    let errors = validate_request_full(&spec, Some("/x"), op, &HashMap::new(), &request);

    assert!(errors.is_empty(), "got {errors:?}");
}

/// A float-enum value outside the relative tolerance of every allowed entry is rejected.
#[test]
fn test_float_enum_outside_relative_tolerance_emits_enum_error() {
    let yaml = r#"
openapi: 3.0.0
info: { title: t, version: 1.0.0 }
paths:
  /x:
    post:
      requestBody:
        required: true
        content:
          application/json:
            schema:
              type: object
              properties:
                ratio:
                  type: number
                  enum: [1000000.0]
      responses:
        '200': { description: ok }
"#;
    let spec = load_spec(yaml).expect("spec");
    let op = op_for(&spec, "/x", "POST");
    let request = post_request(
        "application/json",
        Some(serde_json::json!({ "ratio": 2_000_000.0 })),
    );

    let errors = validate_request_full(&spec, Some("/x"), op, &HashMap::new(), &request);

    assert!(errors.iter().any(|e| e.code == "enum"));
}

/// A string body field declared with `format: ipv4` accepts a well-formed dotted-quad literal.
#[test]
fn test_format_ipv4_accepts_valid() {
    let yaml = r#"
openapi: 3.0.0
info: { title: t, version: 1.0.0 }
paths:
  /x:
    post:
      requestBody:
        required: true
        content:
          application/json:
            schema:
              type: object
              properties:
                addr: { type: string, format: ipv4 }
      responses:
        '200': { description: ok }
"#;
    let spec = load_spec(yaml).expect("spec");
    let op = op_for(&spec, "/x", "POST");
    let r = post_request(
        "application/json",
        Some(serde_json::json!({ "addr": "192.168.1.1" })),
    );

    let errors = validate_request_full(&spec, Some("/x"), op, &HashMap::new(), &r);

    assert!(errors.is_empty(), "got {errors:?}");
}

/// A string body field declared with `format: ipv4` rejects an out-of-range dotted-quad literal.
#[test]
fn test_format_ipv4_rejects_invalid() {
    let yaml = r#"
openapi: 3.0.0
info: { title: t, version: 1.0.0 }
paths:
  /x:
    post:
      requestBody:
        required: true
        content:
          application/json:
            schema:
              type: object
              properties:
                addr: { type: string, format: ipv4 }
      responses:
        '200': { description: ok }
"#;
    let spec = load_spec(yaml).expect("spec");
    let op = op_for(&spec, "/x", "POST");
    let r = post_request(
        "application/json",
        Some(serde_json::json!({ "addr": "999.999.999.999" })),
    );

    let errors = validate_request_full(&spec, Some("/x"), op, &HashMap::new(), &r);

    assert!(errors
        .iter()
        .any(|e| e.code == "format" && e.message.contains("ipv4")));
}

/// A string body field declared with `format: ipv6` accepts a well-formed IPv6 literal.
#[test]
fn test_format_ipv6_accepts_valid() {
    let yaml = r#"
openapi: 3.0.0
info: { title: t, version: 1.0.0 }
paths:
  /x:
    post:
      requestBody:
        required: true
        content:
          application/json:
            schema:
              type: object
              properties:
                addr: { type: string, format: ipv6 }
      responses:
        '200': { description: ok }
"#;
    let spec = load_spec(yaml).expect("spec");
    let op = op_for(&spec, "/x", "POST");
    let r = post_request(
        "application/json",
        Some(serde_json::json!({ "addr": "2001:db8::1" })),
    );

    let errors = validate_request_full(&spec, Some("/x"), op, &HashMap::new(), &r);

    assert!(errors.is_empty(), "got {errors:?}");
}

/// A string body field declared with `format: ipv6` rejects a non-IPv6 literal.
#[test]
fn test_format_ipv6_rejects_invalid() {
    let yaml = r#"
openapi: 3.0.0
info: { title: t, version: 1.0.0 }
paths:
  /x:
    post:
      requestBody:
        required: true
        content:
          application/json:
            schema:
              type: object
              properties:
                addr: { type: string, format: ipv6 }
      responses:
        '200': { description: ok }
"#;
    let spec = load_spec(yaml).expect("spec");
    let op = op_for(&spec, "/x", "POST");
    let r = post_request(
        "application/json",
        Some(serde_json::json!({ "addr": "::g" })),
    );

    let errors = validate_request_full(&spec, Some("/x"), op, &HashMap::new(), &r);

    assert!(errors
        .iter()
        .any(|e| e.code == "format" && e.message.contains("ipv6")));
}

/// A string body field declared with `format: hostname` accepts an RFC 1123 hostname.
#[test]
fn test_format_hostname_accepts_valid() {
    let yaml = r#"
openapi: 3.0.0
info: { title: t, version: 1.0.0 }
paths:
  /x:
    post:
      requestBody:
        required: true
        content:
          application/json:
            schema:
              type: object
              properties:
                host: { type: string, format: hostname }
      responses:
        '200': { description: ok }
"#;
    let spec = load_spec(yaml).expect("spec");
    let op = op_for(&spec, "/x", "POST");
    let r = post_request(
        "application/json",
        Some(serde_json::json!({ "host": "example.com" })),
    );

    let errors = validate_request_full(&spec, Some("/x"), op, &HashMap::new(), &r);

    assert!(errors.is_empty(), "got {errors:?}");
}

/// A string body field declared with `format: hostname` rejects an empty leading label.
#[test]
fn test_format_hostname_rejects_invalid() {
    let yaml = r#"
openapi: 3.0.0
info: { title: t, version: 1.0.0 }
paths:
  /x:
    post:
      requestBody:
        required: true
        content:
          application/json:
            schema:
              type: object
              properties:
                host: { type: string, format: hostname }
      responses:
        '200': { description: ok }
"#;
    let spec = load_spec(yaml).expect("spec");
    let op = op_for(&spec, "/x", "POST");
    let r = post_request(
        "application/json",
        Some(serde_json::json!({ "host": ".example" })),
    );

    let errors = validate_request_full(&spec, Some("/x"), op, &HashMap::new(), &r);

    assert!(errors
        .iter()
        .any(|e| e.code == "format" && e.message.contains("hostname")));
}

/// A string body field declared with `format: uri` accepts an absolute http URL.
#[test]
fn test_format_uri_accepts_valid() {
    let yaml = r#"
openapi: 3.0.0
info: { title: t, version: 1.0.0 }
paths:
  /x:
    post:
      requestBody:
        required: true
        content:
          application/json:
            schema:
              type: object
              properties:
                u: { type: string, format: uri }
      responses:
        '200': { description: ok }
"#;
    let spec = load_spec(yaml).expect("spec");
    let op = op_for(&spec, "/x", "POST");
    let r = post_request(
        "application/json",
        Some(serde_json::json!({ "u": "https://example.com/path" })),
    );

    let errors = validate_request_full(&spec, Some("/x"), op, &HashMap::new(), &r);

    assert!(errors.is_empty(), "got {errors:?}");
}

/// A string body field declared with `format: uri` rejects a value missing the scheme prefix.
#[test]
fn test_format_uri_rejects_invalid() {
    let yaml = r#"
openapi: 3.0.0
info: { title: t, version: 1.0.0 }
paths:
  /x:
    post:
      requestBody:
        required: true
        content:
          application/json:
            schema:
              type: object
              properties:
                u: { type: string, format: uri }
      responses:
        '200': { description: ok }
"#;
    let spec = load_spec(yaml).expect("spec");
    let op = op_for(&spec, "/x", "POST");
    let r = post_request(
        "application/json",
        Some(serde_json::json!({ "u": "://no-scheme" })),
    );

    let errors = validate_request_full(&spec, Some("/x"), op, &HashMap::new(), &r);

    assert!(errors
        .iter()
        .any(|e| e.code == "format" && e.message.contains("uri")));
}

/// A string body field declared with `format: date` accepts a calendar date in the canonical form.
#[test]
fn test_format_date_accepts_valid() {
    let yaml = r#"
openapi: 3.0.0
info: { title: t, version: 1.0.0 }
paths:
  /x:
    post:
      requestBody:
        required: true
        content:
          application/json:
            schema:
              type: object
              properties:
                d: { type: string, format: date }
      responses:
        '200': { description: ok }
"#;
    let spec = load_spec(yaml).expect("spec");
    let op = op_for(&spec, "/x", "POST");
    let r = post_request(
        "application/json",
        Some(serde_json::json!({ "d": "2024-01-15" })),
    );

    let errors = validate_request_full(&spec, Some("/x"), op, &HashMap::new(), &r);

    assert!(errors.is_empty(), "got {errors:?}");
}

/// A string body field declared with `format: date` rejects an out-of-range month value.
#[test]
fn test_format_date_rejects_invalid() {
    let yaml = r#"
openapi: 3.0.0
info: { title: t, version: 1.0.0 }
paths:
  /x:
    post:
      requestBody:
        required: true
        content:
          application/json:
            schema:
              type: object
              properties:
                d: { type: string, format: date }
      responses:
        '200': { description: ok }
"#;
    let spec = load_spec(yaml).expect("spec");
    let op = op_for(&spec, "/x", "POST");
    let r = post_request(
        "application/json",
        Some(serde_json::json!({ "d": "2024-13-01" })),
    );

    let errors = validate_request_full(&spec, Some("/x"), op, &HashMap::new(), &r);

    assert!(errors
        .iter()
        .any(|e| e.code == "format" && e.message.contains("date")));
}

/// A string body field declared with `format: time` accepts a canonical `HH:MM:SS` wall-clock value.
#[test]
fn test_format_time_accepts_valid() {
    let yaml = r#"
openapi: 3.0.0
info: { title: t, version: 1.0.0 }
paths:
  /x:
    post:
      requestBody:
        required: true
        content:
          application/json:
            schema:
              type: object
              properties:
                t: { type: string, format: time }
      responses:
        '200': { description: ok }
"#;
    let spec = load_spec(yaml).expect("spec");
    let op = op_for(&spec, "/x", "POST");
    let r = post_request(
        "application/json",
        Some(serde_json::json!({ "t": "12:34:56" })),
    );

    let errors = validate_request_full(&spec, Some("/x"), op, &HashMap::new(), &r);

    assert!(errors.is_empty(), "got {errors:?}");
}

/// A string body field declared with `format: time` rejects an out-of-range hour value.
#[test]
fn test_format_time_rejects_invalid() {
    let yaml = r#"
openapi: 3.0.0
info: { title: t, version: 1.0.0 }
paths:
  /x:
    post:
      requestBody:
        required: true
        content:
          application/json:
            schema:
              type: object
              properties:
                t: { type: string, format: time }
      responses:
        '200': { description: ok }
"#;
    let spec = load_spec(yaml).expect("spec");
    let op = op_for(&spec, "/x", "POST");
    let r = post_request(
        "application/json",
        Some(serde_json::json!({ "t": "25:00:00" })),
    );

    let errors = validate_request_full(&spec, Some("/x"), op, &HashMap::new(), &r);

    assert!(errors
        .iter()
        .any(|e| e.code == "format" && e.message.contains("time")));
}

/// A string body field declared with `format: email` accepts a well-formed local@domain literal.
#[test]
fn test_format_email_accepts_valid() {
    let yaml = r#"
openapi: 3.0.0
info: { title: t, version: 1.0.0 }
paths:
  /x:
    post:
      requestBody:
        required: true
        content:
          application/json:
            schema:
              type: object
              properties:
                email: { type: string, format: email }
      responses:
        '200': { description: ok }
"#;
    let spec = load_spec(yaml).expect("spec");
    let op = op_for(&spec, "/x", "POST");
    let r = post_request(
        "application/json",
        Some(serde_json::json!({ "email": "user@example.com" })),
    );

    let errors = validate_request_full(&spec, Some("/x"), op, &HashMap::new(), &r);

    assert!(errors.is_empty(), "got {errors:?}");
}

/// A string body field declared with `format: uuid` accepts a canonical 8-4-4-4-12 hex literal.
#[test]
fn test_format_uuid_accepts_valid() {
    let yaml = r#"
openapi: 3.0.0
info: { title: t, version: 1.0.0 }
paths:
  /x:
    post:
      requestBody:
        required: true
        content:
          application/json:
            schema:
              type: object
              properties:
                id: { type: string, format: uuid }
      responses:
        '200': { description: ok }
"#;
    let spec = load_spec(yaml).expect("spec");
    let op = op_for(&spec, "/x", "POST");
    let r = post_request(
        "application/json",
        Some(serde_json::json!({ "id": "550e8400-e29b-41d4-a716-446655440000" })),
    );

    let errors = validate_request_full(&spec, Some("/x"), op, &HashMap::new(), &r);

    assert!(errors.is_empty(), "got {errors:?}");
}

/// A string body field declared with `format: binary` accepts an arbitrary string passthrough.
///
/// `binary` is a documentation-only spelling per OAS 3.0 and the validator treats it as a
/// no-op format, so any string-shaped value satisfies the schema.
#[test]
fn test_format_binary_accepts_string_passthrough() {
    let yaml = r#"
openapi: 3.0.0
info: { title: t, version: 1.0.0 }
paths:
  /x:
    post:
      requestBody:
        required: true
        content:
          application/json:
            schema:
              type: object
              properties:
                blob: { type: string, format: binary }
      responses:
        '200': { description: ok }
"#;
    let spec = load_spec(yaml).expect("spec");
    let op = op_for(&spec, "/x", "POST");
    let r = post_request(
        "application/json",
        Some(serde_json::json!({ "blob": "anything-goes" })),
    );

    let errors = validate_request_full(&spec, Some("/x"), op, &HashMap::new(), &r);

    assert!(errors.is_empty(), "got {errors:?}");
}

/// An integer body value above the schema's `exclusiveMaximum` bound emits an `exclusiveMaximum` error.
#[test]
fn test_exclusive_maximum_numeric_form_enforced() {
    let yaml = r#"
openapi: 3.0.0
info: { title: t, version: 1.0.0 }
paths:
  /count:
    post:
      requestBody:
        required: true
        content:
          application/json:
            schema:
              type: integer
              exclusiveMaximum: 10
      responses:
        '200': { description: ok }
"#;
    let spec = load_spec(yaml).expect("spec");
    let mut r = req("POST", "/count");
    r.headers
        .insert("content-type".into(), "application/json".into());
    r.body = Some(serde_json::json!(10));

    let err = mock(&spec, &r, &errors_cfg()).expect_err("body=10 must violate exclusiveMaximum:10");

    match err {
        MockError::ValidationFailed(errors) => {
            assert!(errors
                .iter()
                .any(|e| e.location == ValidationLocation::Body
                    && e.code.contains("exclusiveMaximum")))
        }
        other => panic!("expected ValidationFailed, got {other:?}"),
    }
}

/// A value below the exclusive upper bound passes validation under the rewritten `exclusiveMaximum`.
#[test]
fn test_exclusive_maximum_accepts_value_below_bound() {
    let yaml = r#"
openapi: 3.0.0
info: { title: t, version: 1.0.0 }
paths:
  /count:
    post:
      requestBody:
        required: true
        content:
          application/json:
            schema:
              type: integer
              exclusiveMaximum: 10
      responses:
        '200': { description: ok }
"#;
    let spec = load_spec(yaml).expect("spec");
    let mut r = req("POST", "/count");
    r.headers
        .insert("content-type".into(), "application/json".into());
    r.body = Some(serde_json::json!(9));

    let resp = mock(&spec, &r, &errors_cfg()).unwrap();

    assert_eq!(resp.status, 200);
}

/// A `byte` format value containing only the RFC 4648 base64 alphabet passes validation.
#[test]
fn test_format_byte_accepts_valid_base64() {
    let yaml = r#"
openapi: 3.0.0
info: { title: t, version: 1.0.0 }
paths:
  /x:
    post:
      requestBody:
        required: true
        content:
          application/json:
            schema:
              type: object
              properties:
                blob: { type: string, format: byte }
      responses:
        '200': { description: ok }
"#;
    let spec = load_spec(yaml).expect("spec");
    let op = op_for(&spec, "/x", "POST");
    let r = post_request(
        "application/json",
        Some(serde_json::json!({ "blob": "aGVsbG8=" })),
    );

    let errors = validate_request_full(&spec, Some("/x"), op, &HashMap::new(), &r);

    assert!(errors.is_empty(), "got {errors:?}");
}

/// A `byte` format value containing non-base64 characters emits a `format` diagnostic.
#[test]
fn test_format_byte_rejects_invalid_base64_alphabet() {
    let yaml = r#"
openapi: 3.0.0
info: { title: t, version: 1.0.0 }
paths:
  /x:
    post:
      requestBody:
        required: true
        content:
          application/json:
            schema:
              type: object
              properties:
                blob: { type: string, format: byte }
      responses:
        '200': { description: ok }
"#;
    let spec = load_spec(yaml).expect("spec");
    let op = op_for(&spec, "/x", "POST");
    let r = post_request(
        "application/json",
        Some(serde_json::json!({ "blob": "not base64!" })),
    );

    let errors = validate_request_full(&spec, Some("/x"), op, &HashMap::new(), &r);

    assert!(
        errors
            .iter()
            .any(|e| e.field.ends_with("blob") && e.code == "format"),
        "got {errors:?}"
    );
}

/// A `byte` format value with a length that is not a multiple of four is rejected.
#[test]
fn test_format_byte_rejects_bad_padding_length() {
    let yaml = r#"
openapi: 3.0.0
info: { title: t, version: 1.0.0 }
paths:
  /x:
    post:
      requestBody:
        required: true
        content:
          application/json:
            schema:
              type: object
              properties:
                blob: { type: string, format: byte }
      responses:
        '200': { description: ok }
"#;
    let spec = load_spec(yaml).expect("spec");
    let op = op_for(&spec, "/x", "POST");
    let r = post_request(
        "application/json",
        Some(serde_json::json!({ "blob": "abc" })),
    );

    let errors = validate_request_full(&spec, Some("/x"), op, &HashMap::new(), &r);

    assert!(
        errors
            .iter()
            .any(|e| e.field.ends_with("blob") && e.code == "format"),
        "got {errors:?}"
    );
}

/// An `int32` body value above `i32::MAX` emits a `format` diagnostic.
#[test]
fn test_format_int32_rejects_overflow() {
    let yaml = r#"
openapi: 3.0.0
info: { title: t, version: 1.0.0 }
paths:
  /count:
    post:
      requestBody:
        required: true
        content:
          application/json:
            schema:
              type: integer
              format: int32
      responses:
        '200': { description: ok }
"#;
    let spec = load_spec(yaml).expect("spec");
    let mut r = req("POST", "/count");
    r.headers
        .insert("content-type".into(), "application/json".into());
    r.body = Some(serde_json::json!(2_147_483_648_i64));

    let err =
        mock(&spec, &r, &errors_cfg()).expect_err("value above i32::MAX must violate int32 format");

    match err {
        MockError::ValidationFailed(errors) => {
            assert!(errors
                .iter()
                .any(|e| e.location == ValidationLocation::Body && e.code == "format"));
        }
        other => panic!("expected ValidationFailed, got {other:?}"),
    }
}

/// An `int32` body value at `i32::MAX` passes validation.
#[test]
fn test_format_int32_accepts_max_value() {
    let yaml = r#"
openapi: 3.0.0
info: { title: t, version: 1.0.0 }
paths:
  /count:
    post:
      requestBody:
        required: true
        content:
          application/json:
            schema:
              type: integer
              format: int32
      responses:
        '200': { description: ok }
"#;
    let spec = load_spec(yaml).expect("spec");
    let mut r = req("POST", "/count");
    r.headers
        .insert("content-type".into(), "application/json".into());
    r.body = Some(serde_json::json!(i32::MAX as i64));

    let resp = mock(&spec, &r, &errors_cfg()).unwrap();

    assert_eq!(resp.status, 200);
}

/// An `int32` body value at `i32::MIN` passes validation.
#[test]
fn test_format_int32_accepts_min_value() {
    let yaml = r#"
openapi: 3.0.0
info: { title: t, version: 1.0.0 }
paths:
  /count:
    post:
      requestBody:
        required: true
        content:
          application/json:
            schema:
              type: integer
              format: int32
      responses:
        '200': { description: ok }
"#;
    let spec = load_spec(yaml).expect("spec");
    let mut r = req("POST", "/count");
    r.headers
        .insert("content-type".into(), "application/json".into());
    r.body = Some(serde_json::json!(i32::MIN as i64));

    let resp = mock(&spec, &r, &errors_cfg()).unwrap();

    assert_eq!(resp.status, 200);
}
