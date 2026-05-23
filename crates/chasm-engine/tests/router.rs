//! Tests for `router.rs`.

use chasm_engine::{
    load_spec, mock, route_request, route_request_with_strict, MockConfig, MockError, RouteMatch,
};

mod common;

/// Builds a spec exposing GET and POST on `/pets`.
fn multi_method_spec() -> &'static str {
    r#"
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
    post:
      responses:
        '200':
          description: ok
          content:
            application/json:
              example: { ok: true }
"#
}

/// A trailing slash on the request still matches a no-trailing-slash template.
#[test]
fn test_trailing_slash_matches() {
    let spec = load_spec(multi_method_spec()).unwrap();

    let resp = mock(&spec, &common::req("GET", "/pets/"), &MockConfig::default()).unwrap();

    assert_eq!(resp.status, 200);
}

/// A missing path yields `NoRoute`.
#[test]
fn test_no_route_for_unknown_path() {
    let spec = load_spec(multi_method_spec()).unwrap();

    let err = mock(
        &spec,
        &common::req("GET", "/no/such/path"),
        &MockConfig::default(),
    )
    .unwrap_err();

    assert!(matches!(err, MockError::NoRoute { .. }));
}

/// A wrong method on a known path yields `MethodNotAllowed`.
#[test]
fn test_method_not_allowed_distinct_from_no_route() {
    let spec = load_spec(multi_method_spec()).unwrap();

    let err = mock(
        &spec,
        &common::req("DELETE", "/pets"),
        &MockConfig::default(),
    )
    .unwrap_err();

    assert!(matches!(err, MockError::MethodNotAllowed { .. }));
}

/// The `Allow` value on a 405 lists every declared method on the path.
#[test]
fn test_method_not_allowed_carries_allow_header() {
    let yaml = r#"
openapi: 3.0.0
info: { title: t, version: 1.0.0 }
paths:
  /multi:
    get:
      responses:
        '200':
          description: ok
    post:
      responses:
        '200':
          description: ok
    delete:
      responses:
        '200':
          description: ok
"#;
    let spec = load_spec(yaml).unwrap();

    let err = mock(
        &spec,
        &common::req("PATCH", "/multi"),
        &MockConfig::default(),
    )
    .unwrap_err();

    match err {
        MockError::MethodNotAllowed { allow, .. } => {
            let mut got: Vec<&str> = allow.split(',').map(str::trim).collect();
            got.sort();
            assert_eq!(got, vec!["DELETE", "GET", "HEAD", "POST"]);
        }
        other => panic!("expected MethodNotAllowed, got {other:?}"),
    }
}

/// A literal segment beats a parameterised one at the same depth.
#[test]
fn test_static_segment_wins_over_param() {
    let yaml = r#"
openapi: 3.0.0
info: { title: t, version: 1.0.0 }
paths:
  /items/{id}/{date}:
    get:
      responses:
        '200':
          description: dated
          content:
            application/json:
              example: { which: "dated" }
  /items/{id}/notes:
    get:
      responses:
        '200':
          description: notes
          content:
            application/json:
              example: { which: "notes" }
"#;
    let spec = load_spec(yaml).unwrap();

    let resp = mock(
        &spec,
        &common::req("GET", "/items/42/notes"),
        &MockConfig::default(),
    )
    .unwrap();

    assert_eq!(resp.body, serde_json::json!({"which": "notes"}));
}

/// The literal root path matches a `/` template.
#[test]
fn test_root_path_matches() {
    let yaml = r#"
openapi: 3.0.0
info: { title: t, version: 1.0.0 }
paths:
  /:
    get:
      responses:
        '200':
          description: ok
          content:
            application/json:
              example: { root: true }
"#;
    let spec = load_spec(yaml).unwrap();

    let resp = mock(&spec, &common::req("GET", "/"), &MockConfig::default()).unwrap();

    assert_eq!(resp.body, serde_json::json!({"root": true}));
}

/// A path containing a sub-segment template like `users-{id}` captures `id` verbatim.
#[test]
fn test_sub_segment_template_matches() {
    let yaml = r#"
openapi: 3.0.0
info: { title: t, version: 1.0.0 }
paths:
  /users-{id}:
    get:
      parameters:
        - in: path
          name: id
          required: true
          schema: { type: string }
      responses:
        '200':
          description: ok
"#;
    let spec = load_spec(yaml).unwrap();

    let matched = route_request(&spec, "GET", "/users-42");

    match matched {
        RouteMatch::Operation(m) => {
            assert_eq!(m.path_params.get("id").map(String::as_str), Some("42"))
        }
        _ => panic!("expected operation match"),
    }
}

/// The first `servers[].url` base path is stripped before matching.
#[test]
fn test_strips_first_server_base_path() {
    let yaml = r#"
openapi: 3.0.0
info: { title: t, version: 1.0.0 }
servers:
  - url: https://api.example.com/v1
paths:
  /widgets:
    get:
      responses:
        '200':
          description: ok
          content:
            application/json:
              example: { matched: true }
"#;
    let spec = load_spec(yaml).unwrap();

    let resp = mock(
        &spec,
        &common::req("GET", "/v1/widgets"),
        &MockConfig::default(),
    )
    .unwrap();

    assert_eq!(resp.status, 200);
}

/// Multiple `servers[]` entries each contribute a base path that the router accepts.
#[test]
fn test_multiple_servers_base_paths_resolve() {
    let yaml = r#"
openapi: 3.0.0
info: { title: t, version: 1.0.0 }
servers:
  - url: https://api.example.com/v1
  - url: https://api.example.com/v2
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

    let resp_v1 = mock(
        &spec,
        &common::req("GET", "/v1/pets"),
        &MockConfig::default(),
    )
    .expect("v1");
    let resp_v2 = mock(
        &spec,
        &common::req("GET", "/v2/pets"),
        &MockConfig::default(),
    )
    .expect("v2");

    assert_eq!((resp_v1.status, resp_v2.status), (200, 200));
}

/// Server-URL `{var}` placeholders inside the path component are expanded before stripping.
#[test]
fn test_server_template_basepath_expanded() {
    let yaml = r#"
openapi: 3.0.0
info: { title: t, version: 1.0.0 }
servers:
  - url: https://api.example.com/{basePath}/v1
    variables:
      basePath:
        default: v2
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

    let resp = mock(
        &spec,
        &common::req("GET", "/v2/v1/pets"),
        &MockConfig::default(),
    )
    .unwrap();

    assert_eq!(resp.status, 200);
}

/// Server-URL `{var}` placeholders inside the authority portion are expanded before stripping.
#[test]
fn test_server_authority_variable_expanded() {
    let yaml = r#"
openapi: 3.0.0
info: { title: t, version: 1.0.0 }
servers:
  - url: "https://{host}/v1"
    variables:
      host:
        default: api.example.com
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

    let resp = mock(
        &spec,
        &common::req("GET", "/v1/pets"),
        &MockConfig::default(),
    )
    .unwrap();

    assert_eq!(resp.status, 200);
}

/// Both authority-side and path-side server variables are expanded together.
#[test]
fn test_server_variable_mixed_authority_and_path() {
    let yaml = r#"
openapi: 3.0.0
info: { title: t, version: 1.0.0 }
servers:
  - url: "https://api.{region}.example.com/v1/{stage}"
    variables:
      region:
        default: us-east-1
      stage:
        default: v2
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

    let resp = mock(
        &spec,
        &common::req("GET", "/v1/v2/pets"),
        &MockConfig::default(),
    )
    .unwrap();

    assert_eq!(resp.status, 200);
}

/// Percent-encoded path segments are decoded before template matching, and decoded values flow into `path_params`.
#[test]
fn test_path_percent_decoded_matches_template() {
    let yaml = r#"
openapi: 3.0.0
info: { title: t, version: 1.0.0 }
paths:
  /users/{name}:
    get:
      responses:
        '200':
          description: ok
          content:
            application/json:
              example: { ok: true }
"#;
    let spec = load_spec(yaml).unwrap();

    let matched = route_request(&spec, "GET", "/users/john%20doe");

    match matched {
        RouteMatch::Operation(m) => assert_eq!(
            m.path_params.get("name").map(String::as_str),
            Some("john doe")
        ),
        _ => panic!("expected operation match"),
    }
}

/// A `HEAD` request on a path that only declares `get` falls through to GET.
#[test]
fn test_head_falls_through_to_get() {
    let yaml = r#"
openapi: 3.0.0
info: { title: t, version: 1.0.0 }
paths:
  /resource:
    get:
      responses:
        '200':
          description: ok
          content:
            application/json:
              example: { hello: "world" }
"#;
    let spec = load_spec(yaml).unwrap();

    let resp = mock(
        &spec,
        &common::req("HEAD", "/resource"),
        &MockConfig::default(),
    )
    .expect("HEAD should fall through to GET");

    assert_eq!(resp.status, 200);
}

/// `OPTIONS` on a path with no explicit `options` operation synthesises a 200 with an `Allow` header.
#[test]
fn test_options_synthesised_with_allow_header() {
    let yaml = r#"
openapi: 3.0.0
info: { title: t, version: 1.0.0 }
paths:
  /resource:
    get:
      responses:
        '200': { description: ok }
    post:
      responses:
        '200': { description: ok }
    delete:
      responses:
        '200': { description: ok }
"#;
    let spec = load_spec(yaml).unwrap();

    let resp = mock(
        &spec,
        &common::req("OPTIONS", "/resource"),
        &MockConfig::default(),
    )
    .expect("OPTIONS should synthesise");

    let allow = resp
        .headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("Allow"))
        .map(|(_, v)| v.as_str())
        .expect("Allow header present");
    assert_eq!(allow, "DELETE, GET, HEAD, POST");
}

/// `route_request_with_strict(spec, "OPTIONS", path, true)` returns `MethodNotAllowed` on a path without explicit `options`.
#[test]
fn test_route_request_with_strict_returns_method_not_allowed_for_synthesised_options() {
    let yaml = r#"
openapi: 3.0.0
info: { title: t, version: 1.0.0 }
paths:
  /pets:
    get:
      responses:
        '200':
          description: ok
"#;
    let spec = load_spec(yaml).unwrap();

    let matched = route_request_with_strict(&spec, "OPTIONS", "/pets", true);

    assert!(matches!(matched, RouteMatch::MethodNotAllowed(_)));
}

/// `route_request_with_strict(spec, "OPTIONS", path, false)` synthesises an `OPTIONS` response with an `Allow` list.
#[test]
fn test_route_request_with_strict_permissive_synthesises_options() {
    let yaml = r#"
openapi: 3.0.0
info: { title: t, version: 1.0.0 }
paths:
  /pets:
    get:
      responses:
        '200':
          description: ok
"#;
    let spec = load_spec(yaml).unwrap();

    let matched = route_request_with_strict(&spec, "OPTIONS", "/pets", false);

    assert!(matches!(matched, RouteMatch::SynthesisedOptions(_)));
}

/// `route_request` returns `Operation` for a declared method on a declared path.
#[test]
fn test_route_request_returns_operation_for_declared_method() {
    let yaml = r#"
openapi: 3.0.0
info: { title: t, version: 1.0.0 }
paths:
  /pets:
    get:
      responses:
        '200':
          description: ok
"#;
    let spec = load_spec(yaml).unwrap();

    let matched = chasm_engine::router::route_request(&spec, "GET", "/pets");

    assert!(matches!(matched, RouteMatch::Operation(_)));
}

/// `route_request` returns `MethodNotAllowed` for an undeclared method on a declared path.
#[test]
fn test_route_request_returns_method_not_allowed_for_undeclared_method() {
    let yaml = r#"
openapi: 3.0.0
info: { title: t, version: 1.0.0 }
paths:
  /pets:
    get:
      responses:
        '200':
          description: ok
"#;
    let spec = load_spec(yaml).unwrap();

    let matched = chasm_engine::router::route_request(&spec, "DELETE", "/pets");

    assert!(matches!(matched, RouteMatch::MethodNotAllowed(_)));
}

/// `get_operation` returns the declared `Operation` on a `PathItem` for the matching method.
#[test]
fn test_get_operation_returns_declared_operation() {
    let yaml = r#"
openapi: 3.0.0
info: { title: t, version: 1.0.0 }
paths:
  /pets:
    get:
      operationId: listPets
      responses:
        '200':
          description: ok
"#;
    let spec = load_spec(yaml).unwrap();
    let path_item = spec
        .paths
        .paths
        .get("/pets")
        .and_then(|p| p.as_item())
        .expect("path item present");

    let op = chasm_engine::router::get_operation(path_item, "GET");

    assert_eq!(op.and_then(|o| o.operation_id.as_deref()), Some("listPets"));
}

/// An operation-level `servers[]` override is honoured: a request whose path
/// strips the operation-level base resolves to that operation, even though the
/// spec-level `servers[]` declares a different base. OAS3 specifies the
/// override precedence operation > path-item > spec; this test pins the
/// operation tier.
#[test]
fn test_operation_level_servers_override_routes_request() {
    let yaml = r#"
openapi: 3.0.0
info: { title: t, version: 1.0.0 }
servers:
  - url: /v1
paths:
  /pets:
    get:
      operationId: listPetsV2
      servers:
        - url: /v2
      responses:
        '200':
          description: ok
          content:
            application/json:
              example: { v: 2 }
"#;
    let spec = load_spec(yaml).unwrap();

    let matched = chasm_engine::router::route_request(&spec, "GET", "/v2/pets");

    match matched {
        RouteMatch::Operation(m) => {
            assert_eq!(m.operation.operation_id.as_deref(), Some("listPetsV2"));
        }
        _ => panic!("expected /v2/pets to route to the operation-level server override"),
    }
}

/// Operations without an override still resolve via the spec-level `servers[]`
/// entry when the operation-level override exists on a different operation.
#[test]
fn test_spec_level_servers_still_apply_to_unoverridden_operation() {
    let yaml = r#"
openapi: 3.0.0
info: { title: t, version: 1.0.0 }
servers:
  - url: /v1
paths:
  /pets:
    get:
      operationId: listPetsV2
      servers:
        - url: /v2
      responses:
        '200':
          description: ok
          content:
            application/json:
              example: { v: 2 }
  /users:
    get:
      operationId: listUsersV1
      responses:
        '200':
          description: ok
          content:
            application/json:
              example: { v: 1 }
"#;
    let spec = load_spec(yaml).unwrap();

    let matched = chasm_engine::router::route_request(&spec, "GET", "/v1/users");

    match matched {
        RouteMatch::Operation(m) => {
            assert_eq!(m.operation.operation_id.as_deref(), Some("listUsersV1"));
        }
        _ => panic!("expected /v1/users to route via spec-level server base"),
    }
}

/// A path-item-level `servers[]` override applies to every operation under that
/// path item, falling between operation-level and spec-level in precedence.
#[test]
fn test_path_item_level_servers_override_routes_request() {
    let yaml = r#"
openapi: 3.0.0
info: { title: t, version: 1.0.0 }
servers:
  - url: /v1
paths:
  /pets:
    servers:
      - url: /beta
    get:
      operationId: listPetsBeta
      responses:
        '200':
          description: ok
          content:
            application/json:
              example: { v: "beta" }
"#;
    let spec = load_spec(yaml).unwrap();

    let matched = chasm_engine::router::route_request(&spec, "GET", "/beta/pets");

    match matched {
        RouteMatch::Operation(m) => {
            assert_eq!(m.operation.operation_id.as_deref(), Some("listPetsBeta"));
        }
        _ => panic!("expected /beta/pets to route via path-item-level server base"),
    }
}
