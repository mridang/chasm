//! Tests for `security.rs`.

use chasm_engine::{
    evaluate_security, load_spec, mock, MockConfig, MockError, MockRequest, SecurityResult,
};
use openapiv3::{OpenAPI, Operation};
use std::collections::HashMap;

/// Builds a GET request to `path` with the supplied headers and query.
fn request_with(
    path: &str,
    headers: HashMap<String, String>,
    query: HashMap<String, String>,
) -> MockRequest {
    let mut req = MockRequest::default();
    req.method = "GET".to_string();
    req.path = path.to_string();
    req.headers = headers;
    req.query = query;
    req.body = None;
    req
}

/// Builds a single-entry `HashMap<String, String>` for header/query construction.
fn one(key: &str, value: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    map.insert(key.to_string(), value.to_string());
    map
}

/// Locates the operation registered as `GET /` on the supplied spec.
fn root_get(spec: &OpenAPI) -> &Operation {
    spec.paths
        .paths
        .get("/")
        .and_then(|p| p.as_item())
        .and_then(|p| p.get.as_ref())
        .expect("root GET operation present")
}

/// A spec exposing a `bearerAuth` scheme on `GET /` via a top-level `security` block.
fn bearer_spec() -> &'static str {
    r#"
openapi: 3.0.0
info: { title: t, version: 1.0.0 }
security:
  - bearerAuth: []
paths:
  /:
    get:
      operationId: authenticate
      responses:
        '200':
          description: ok
          content:
            application/json:
              example: { ok: true }
components:
  securitySchemes:
    bearerAuth:
      type: http
      scheme: bearer
"#
}

/// A spec requiring `bearerAuth` globally and on `GET /secure`.
fn secure_bearer_spec() -> &'static str {
    r#"
openapi: 3.0.0
info: { title: t, version: 1.0.0 }
security:
  - bearerAuth: []
paths:
  /secure:
    get:
      responses:
        '200':
          description: ok
          content:
            application/json:
              example: { ok: true }
components:
  securitySchemes:
    bearerAuth:
      type: http
      scheme: bearer
"#
}

/// A spec requiring `bearerAuth` globally with `GET /open` overriding via `security: []`.
fn open_override_spec() -> &'static str {
    r#"
openapi: 3.0.0
info: { title: t, version: 1.0.0 }
security:
  - bearerAuth: []
paths:
  /open:
    get:
      security: []
      responses:
        '200':
          description: ok
          content:
            application/json:
              example: { ok: true }
components:
  securitySchemes:
    bearerAuth:
      type: http
      scheme: bearer
"#
}

/// A spec exposing an api-key-in-header scheme (`X-API-Key`) on `GET /h`.
fn api_key_header_spec() -> &'static str {
    r#"
openapi: 3.0.0
info: { title: t, version: 1.0.0 }
paths:
  /h:
    get:
      security:
        - apiKeyHeader: []
      responses:
        '200':
          description: ok
          content:
            application/json:
              example: { ok: true }
components:
  securitySchemes:
    apiKeyHeader:
      type: apiKey
      in: header
      name: X-API-Key
"#
}

/// A spec exposing an api-key-in-query scheme (`api_key`) on `GET /q`.
fn api_key_query_spec() -> &'static str {
    r#"
openapi: 3.0.0
info: { title: t, version: 1.0.0 }
paths:
  /q:
    get:
      security:
        - apiKeyQuery: []
      responses:
        '200':
          description: ok
          content:
            application/json:
              example: { ok: true }
components:
  securitySchemes:
    apiKeyQuery:
      type: apiKey
      in: query
      name: api_key
"#
}

/// A spec exposing an api-key-in-cookie scheme (`api_key`) on `GET /`.
fn cookie_api_key_spec() -> &'static str {
    r#"
openapi: 3.0.0
info: { title: t, version: 1.0.0 }
paths:
  /:
    get:
      security:
        - apiKeyCookie: []
      responses:
        '200':
          description: ok
          content:
            application/json:
              example: { ok: true }
components:
  securitySchemes:
    apiKeyCookie:
      type: apiKey
      in: cookie
      name: api_key
"#
}

/// A spec exposing an HTTP basic scheme on `GET /b`.
fn basic_auth_spec() -> &'static str {
    r#"
openapi: 3.0.0
info: { title: t, version: 1.0.0 }
paths:
  /b:
    get:
      security:
        - basicAuth: []
      responses:
        '200':
          description: ok
          content:
            application/json:
              example: { ok: true }
components:
  securitySchemes:
    basicAuth:
      type: http
      scheme: basic
"#
}

/// A spec exposing an oauth2 password-flow scheme on `GET /o`.
fn oauth2_spec() -> &'static str {
    r#"
openapi: 3.0.0
info: { title: t, version: 1.0.0 }
paths:
  /o:
    get:
      security:
        - oauth2Scheme: []
      responses:
        '200':
          description: ok
          content:
            application/json:
              example: { ok: true }
components:
  securitySchemes:
    oauth2Scheme:
      type: oauth2
      flows:
        password:
          tokenUrl: https://example.com/token
          scopes: {}
"#
}

/// A spec exposing both `bearerAuth` and `apiKeyHeader` as an OR list on `GET /x`.
fn either_of_two_schemes_spec() -> &'static str {
    r#"
openapi: 3.0.0
info: { title: t, version: 1.0.0 }
paths:
  /x:
    get:
      security:
        - bearerAuth: []
        - apiKeyHeader: []
      responses:
        '200':
          description: ok
          content:
            application/json:
              example: { ok: true }
components:
  securitySchemes:
    bearerAuth:
      type: http
      scheme: bearer
    apiKeyHeader:
      type: apiKey
      in: header
      name: X-API-Key
"#
}

/// A spec requiring `bearerAuth` AND `apiKeyHeader` within a single requirement on `GET /both`.
fn and_combination_spec() -> &'static str {
    r#"
openapi: 3.0.0
info: { title: t, version: 1.0.0 }
paths:
  /both:
    get:
      security:
        - bearerAuth: []
          apiKeyHeader: []
      responses:
        '200':
          description: ok
          content:
            application/json:
              example: { ok: true }
components:
  securitySchemes:
    bearerAuth:
      type: http
      scheme: bearer
    apiKeyHeader:
      type: apiKey
      in: header
      name: X-API-Key
"#
}

/// A spec naming a security scheme (`mysteryScheme`) that is not declared in `components` on `GET /secret`.
fn unknown_scheme_spec() -> &'static str {
    r#"
openapi: 3.0.0
info: { title: t, version: 1.0.0 }
paths:
  /secret:
    get:
      security:
        - mysteryScheme: []
      responses:
        '200':
          description: ok
          content:
            application/json:
              example: { ok: true }
"#
}

/// An `Authorization` header looked up case-insensitively still satisfies `bearerAuth`.
#[test]
fn test_auth_header_lookup_is_case_insensitive() {
    let spec = load_spec(bearer_spec()).unwrap();
    let op = root_get(&spec);
    let req = request_with("/", one("authorization", "Bearer x"), HashMap::new());

    let result = evaluate_security(&spec, op, &req);

    assert_eq!(result, SecurityResult::Authorized);
}

/// A missing bearer token produces `Unauthorized` reporting the scheme name.
#[test]
fn test_missing_bearer_token_reports_scheme_name() {
    let spec = load_spec(secure_bearer_spec()).unwrap();
    let req = request_with("/secure", HashMap::new(), HashMap::new());

    let err = mock(&spec, &req, &MockConfig::default()).unwrap_err();

    match err {
        MockError::Unauthorized { scheme, .. } => assert_eq!(scheme, "bearerAuth"),
        other => panic!("expected Unauthorized, got {other:?}"),
    }
}

/// A missing bearer token produces `Unauthorized` with a `Bearer realm=` WWW-Authenticate challenge.
#[test]
fn test_missing_bearer_token_includes_bearer_www_authenticate() {
    let spec = load_spec(secure_bearer_spec()).unwrap();
    let req = request_with("/secure", HashMap::new(), HashMap::new());

    let err = mock(&spec, &req, &MockConfig::default()).unwrap_err();

    match err {
        MockError::Unauthorized {
            www_authenticate, ..
        } => assert!(www_authenticate.unwrap().starts_with("Bearer realm=")),
        other => panic!("expected Unauthorized, got {other:?}"),
    }
}

/// A correctly-formatted `Authorization: Bearer ...` header satisfies the scheme.
#[test]
fn test_present_bearer_token_authorises_request() {
    let spec = load_spec(secure_bearer_spec()).unwrap();
    let req = request_with(
        "/secure",
        one("Authorization", "Bearer xyz"),
        HashMap::new(),
    );

    let resp = mock(&spec, &req, &MockConfig::default()).unwrap();

    assert_eq!(resp.status, 200);
}

/// An operation with `security: []` overrides the global requirement and grants access.
#[test]
fn test_operation_with_empty_security_overrides_global() {
    let spec = load_spec(open_override_spec()).unwrap();
    let req = request_with("/open", HashMap::new(), HashMap::new());

    let resp = mock(&spec, &req, &MockConfig::default()).unwrap();

    assert_eq!(resp.status, 200);
}

/// A request to an api-key-in-header operation without the header is unauthorised with no `WWW-Authenticate`.
#[test]
fn test_missing_api_key_in_header_omits_www_authenticate() {
    let spec = load_spec(api_key_header_spec()).unwrap();
    let req = request_with("/h", HashMap::new(), HashMap::new());

    let err = mock(&spec, &req, &MockConfig::default()).unwrap_err();

    match err {
        MockError::Unauthorized {
            www_authenticate, ..
        } => assert!(www_authenticate.is_none()),
        other => panic!("expected Unauthorized, got {other:?}"),
    }
}

/// An api-key-in-query value satisfies the scheme.
#[test]
fn test_present_api_key_in_query_authorises_request() {
    let spec = load_spec(api_key_query_spec()).unwrap();
    let req = request_with("/q", HashMap::new(), one("api_key", "k"));

    let resp = mock(&spec, &req, &MockConfig::default()).unwrap();

    assert_eq!(resp.status, 200);
}

/// Two schemes joined as an OR list authorise the request when either alternative is satisfied.
#[test]
fn test_either_of_two_schemes_authorises() {
    let spec = load_spec(either_of_two_schemes_spec()).unwrap();
    let req = request_with("/x", one("X-API-Key", "k"), HashMap::new());

    let resp = mock(&spec, &req, &MockConfig::default()).unwrap();

    assert_eq!(resp.status, 200);
}

/// A scheme requirement naming a scheme not declared in `components` still returns `Unauthorized`.
#[test]
fn test_unknown_security_scheme_returns_unauthorized() {
    let spec = load_spec(unknown_scheme_spec()).unwrap();
    let req = request_with("/secret", HashMap::new(), HashMap::new());

    let err = mock(&spec, &req, &MockConfig::default()).unwrap_err();

    match err {
        MockError::Unauthorized { scheme, .. } => assert_eq!(scheme, "mysteryScheme"),
        other => panic!("expected Unauthorized, got {other:?}"),
    }
}

/// A double-quoted cookie value is unquoted before satisfying an api-key-in-cookie scheme.
#[test]
fn test_cookie_quoted_value_satisfies_api_key_scheme() {
    let spec = load_spec(cookie_api_key_spec()).unwrap();
    let op = root_get(&spec);
    let req = request_with("/", one("cookie", "api_key=\"abc\""), HashMap::new());

    let result = evaluate_security(&spec, op, &req);

    assert_eq!(result, SecurityResult::Authorized);
}

/// A percent-encoded cookie value is decoded before satisfying an api-key-in-cookie scheme.
#[test]
fn test_cookie_percent_decoded_satisfies_api_key_scheme() {
    let spec = load_spec(cookie_api_key_spec()).unwrap();
    let op = root_get(&spec);
    let req = request_with("/", one("cookie", "api_key=hello%20world"), HashMap::new());

    let result = evaluate_security(&spec, op, &req);

    assert_eq!(result, SecurityResult::Authorized);
}

/// A request with no cookie at all fails an api-key-in-cookie scheme.
#[test]
fn test_missing_cookie_fails_api_key_scheme() {
    let spec = load_spec(cookie_api_key_spec()).unwrap();
    let op = root_get(&spec);
    let req = request_with("/", HashMap::new(), HashMap::new());

    let result = evaluate_security(&spec, op, &req);

    assert_ne!(result, SecurityResult::Authorized);
}

/// A request whose `Cookie` header omits the configured key fails an api-key-in-cookie scheme.
#[test]
fn test_cookie_without_configured_key_fails_api_key_scheme() {
    let spec = load_spec(cookie_api_key_spec()).unwrap();
    let op = root_get(&spec);
    let req = request_with("/", one("cookie", "other=value"), HashMap::new());

    let result = evaluate_security(&spec, op, &req);

    assert_ne!(result, SecurityResult::Authorized);
}

/// An `Authorization: Basic ...` header satisfies an HTTP basic scheme.
#[test]
fn test_http_basic_scheme_accepts_basic_authorization() {
    let spec = load_spec(basic_auth_spec()).unwrap();
    let req = request_with(
        "/b",
        one("Authorization", "Basic dXNlcjpwYXNz"),
        HashMap::new(),
    );

    let resp = mock(&spec, &req, &MockConfig::default()).unwrap();

    assert_eq!(resp.status, 200);
}

/// An oauth2 scheme is satisfied by an `Authorization: Bearer ...` header.
#[test]
fn test_oauth2_scheme_requires_bearer_token() {
    let spec = load_spec(oauth2_spec()).unwrap();
    let req = request_with("/o", one("Authorization", "Bearer abc"), HashMap::new());

    let resp = mock(&spec, &req, &MockConfig::default()).unwrap();

    assert_eq!(resp.status, 200);
}

/// An api-key-in-header value satisfies the scheme.
#[test]
fn test_apikey_in_header_with_value_authorises() {
    let spec = load_spec(api_key_header_spec()).unwrap();
    let req = request_with("/h", one("X-API-Key", "k"), HashMap::new());

    let resp = mock(&spec, &req, &MockConfig::default()).unwrap();

    assert_eq!(resp.status, 200);
}

/// `[bearerAuth, apiKeyHeader]` inside one requirement is an AND: both must be supplied.
#[test]
fn test_and_combination_within_single_security_requirement() {
    let spec = load_spec(and_combination_spec()).unwrap();
    let req = request_with("/both", one("Authorization", "Bearer x"), HashMap::new());

    let err = mock(&spec, &req, &MockConfig::default()).unwrap_err();

    assert!(matches!(err, MockError::Unauthorized { .. }));
}
