//! Tests for `loader.rs`.

use chasm_engine::loader::{
    is_remote_ref, load_spec_with_remote_refs, normalize_oas31_nullable, parse_remote_ref_body,
    read_capped, validate_remote_url, ReadCappedError,
};
use chasm_engine::{load_spec, SpecError};

/// Builds a minimal OAS3 YAML spec with only local `#/...` refs.
fn local_only_spec() -> &'static str {
    r#"
openapi: 3.0.0
info:
  title: local-only
  version: 1.0.0
paths:
  /things:
    get:
      responses:
        '200':
          description: ok
          content:
            application/json:
              schema:
                $ref: '#/components/schemas/Thing'
components:
  schemas:
    Thing:
      type: object
      properties:
        id:
          type: string
"#
}

/// `load_spec_with_remote_refs` accepts a spec containing only local refs.
#[test]
fn test_loads_spec_without_remote_refs() {
    let spec = load_spec_with_remote_refs(local_only_spec())
        .expect("local-only spec should load via remote-refs entry point");

    assert!(spec.paths.paths.iter().any(|(p, _)| *p == "/things"));
}

/// A `$ref` with a non-http(s) URL is left in place verbatim instead of being fetched.
#[test]
fn test_ignores_non_http_refs() {
    let yaml = r#"
openapi: 3.0.0
info:
  title: file-ref
  version: 1.0.0
paths: {}
components:
  schemas:
    Holder:
      type: object
      properties:
        target:
          $ref: 'file:///nonexistent.yaml'
"#;

    let normalised = normalize_oas31_nullable(yaml).expect("normalisation");
    let tree: serde_json::Value =
        serde_json::from_str(&normalised).expect("normalised output is JSON");

    assert_eq!(
        tree["components"]["schemas"]["Holder"]["properties"]["target"]["$ref"]
            .as_str()
            .unwrap(),
        "file:///nonexistent.yaml"
    );
}

/// `load_spec_with_remote_refs` accepts inline JSON input via the JSON branch.
#[test]
fn test_loads_inline_json_spec() {
    let src = r#"{
        "openapi": "3.0.0",
        "info": {"title": "json", "version": "1.0.0"},
        "paths": {}
    }"#;

    let spec = load_spec_with_remote_refs(src).expect("inline JSON spec should parse");

    assert_eq!(spec.openapi, "3.0.0");
}

/// A JSON `type: ["string", "null"]` is rewritten into the scalar form with `nullable: true`.
#[test]
fn test_rewrites_nullable_type_array_in_json_tree() {
    let src = r#"{
        "openapi": "3.0.0",
        "info": {"title": "nullable", "version": "1.0.0"},
        "paths": {},
        "components": {
            "schemas": {
                "Nick": {
                    "type": ["string", "null"]
                }
            }
        }
    }"#;

    let normalised = normalize_oas31_nullable(src).expect("normalisation");
    let tree: serde_json::Value =
        serde_json::from_str(&normalised).expect("normalised output is JSON");

    assert_eq!(tree["components"]["schemas"]["Nick"]["nullable"], true);
}

/// `parse_remote_ref_body` falls back to a YAML parse when the body is not JSON.
#[test]
fn test_parse_remote_ref_body_accepts_yaml() {
    let yaml_body = "\
type: object
properties:
  id:
    type: string
required:
  - id
";

    let parsed = parse_remote_ref_body(yaml_body)
        .expect("YAML body from a remote $ref must parse via the YAML fallback");

    assert_eq!(parsed.get("type").and_then(|v| v.as_str()), Some("object"));
}

/// `parse_remote_ref_body` still accepts strict JSON content.
#[test]
fn test_parse_remote_ref_body_accepts_json() {
    let json_body = r#"{"type":"object","properties":{"id":{"type":"string"}}}"#;

    let parsed = parse_remote_ref_body(json_body)
        .expect("JSON body from a remote $ref must still parse via the JSON path");

    assert_eq!(parsed.get("type").and_then(|v| v.as_str()), Some("object"));
}

/// `parse_remote_ref_body` returns `None` for content that is neither JSON nor YAML.
#[test]
fn test_parse_remote_ref_body_rejects_garbage() {
    let garbage = "{ this is not json: [and not valid, yaml: either";

    let parsed = parse_remote_ref_body(garbage);

    assert!(parsed.is_none());
}

/// YAML containing a dangling local `$ref`, shared by the rejection tests below.
fn dangling_ref_spec_yaml() -> &'static str {
    r#"
openapi: 3.0.0
info:
  title: t
  version: 1.0.0
paths:
  /x:
    get:
      responses:
        '200':
          description: ok
          content:
            application/json:
              schema:
                $ref: '#/components/schemas/DoesNotExist'
components:
  schemas:
    Present:
      type: string
"#
}

/// `load_spec` rejects a dangling local `$ref` with `SpecError::Parse`.
#[test]
fn test_dangling_ref_rejected_with_parse_variant() {
    let err = load_spec(dangling_ref_spec_yaml()).expect_err("dangling $ref must be rejected");

    assert!(matches!(err, SpecError::Parse(_)));
}

/// The parse error message naming a dangling `$ref` mentions the missing component.
#[test]
fn test_dangling_ref_error_message_names_target() {
    let err = load_spec(dangling_ref_spec_yaml()).expect_err("dangling $ref must be rejected");

    match err {
        SpecError::Parse(msg) => {
            assert!(
                msg.contains("dangling $ref") && msg.contains("DoesNotExist"),
                "expected dangling $ref message naming the target, got: {msg}"
            );
        }
        other => panic!("expected Parse error, got {other:?}"),
    }
}

/// `validate_remote_url` rejects an IPv4 loopback literal.
#[test]
fn test_validate_remote_url_ipv4_loopback() {
    assert!(validate_remote_url("http://127.0.0.1/").is_err());
}

/// `validate_remote_url` rejects RFC 1918 `10/8`.
#[test]
fn test_validate_remote_url_ipv4_rfc1918_10() {
    assert!(validate_remote_url("http://10.0.0.1/").is_err());
}

/// `validate_remote_url` rejects RFC 1918 `172.16/12`.
#[test]
fn test_validate_remote_url_ipv4_rfc1918_172() {
    assert!(validate_remote_url("http://172.16.0.1/").is_err());
}

/// `validate_remote_url` rejects RFC 1918 `192.168/16`.
#[test]
fn test_validate_remote_url_ipv4_rfc1918_192() {
    assert!(validate_remote_url("http://192.168.1.1/").is_err());
}

/// `validate_remote_url` rejects the AWS/GCP/Azure IMDS link-local address.
#[test]
fn test_validate_remote_url_ipv4_link_local_imds() {
    assert!(validate_remote_url("http://169.254.169.254/").is_err());
}

/// `validate_remote_url` rejects the IPv6 loopback literal `::1`.
#[test]
fn test_validate_remote_url_ipv6_loopback() {
    assert!(validate_remote_url("http://[::1]/").is_err());
}

/// `validate_remote_url` rejects IPv6 link-local addresses (`fe80::/10`).
#[test]
fn test_validate_remote_url_ipv6_link_local() {
    assert!(validate_remote_url("http://[fe80::1]/").is_err());
}

/// `validate_remote_url` rejects IPv6 unique-local addresses (`fc00::/7`).
#[test]
fn test_validate_remote_url_ipv6_unique_local() {
    assert!(validate_remote_url("http://[fc00::1]/").is_err());
}

/// `validate_remote_url` rejects non-http(s) schemes such as `file://`.
#[test]
fn test_validate_remote_url_file_scheme() {
    assert!(validate_remote_url("file:///etc/passwd").is_err());
}

/// `validate_remote_url` rejects non-http(s) schemes such as `ftp://`.
#[test]
fn test_validate_remote_url_ftp_scheme() {
    assert!(validate_remote_url("ftp://example.com/").is_err());
}

/// `validate_remote_url` does not reject a syntactically valid public host on
/// grounds other than DNS resolution. We accept either `Ok(())` or a DNS-resolution
/// error message so the test stays green on hermetic CI without network access.
#[test]
fn test_validate_remote_url_public_https_host_or_skips_on_dns_failure() {
    match validate_remote_url("https://example.com/") {
        Ok(()) => {}
        Err(reason) => assert_eq!(reason, "dns resolution failed"),
    }
}

/// `is_remote_ref` only flags absolute http(s) URLs.
#[test]
fn test_is_remote_ref_only_http() {
    assert!(is_remote_ref("http://example.com/x"));
    assert!(is_remote_ref("https://example.com/x"));
    assert!(!is_remote_ref("#/components/schemas/X"));
    assert!(!is_remote_ref("./local.yaml#/X"));
    assert!(!is_remote_ref("file:///etc/passwd"));
}

/// A Swagger 2.0 document is rejected with the dedicated guidance message rather
/// than falling through into a misleading deser failure. The spec is written to a
/// temp file because `load_spec` only treats inline strings as content when they
/// start with `{` or `openapi`.
#[test]
fn test_swagger_two_rejected_with_clean_error() {
    let spec = "swagger: \"2.0\"\ninfo:\n  title: legacy\n  version: '1'\npaths: {}\n";
    let dir = std::env::temp_dir();
    let path = dir.join(format!("chasm-swagger2-{}.yaml", std::process::id()));
    std::fs::write(&path, spec).expect("write temp spec");
    let err = load_spec(path.to_str().unwrap()).expect_err("swagger 2.0 must be rejected");
    let _ = std::fs::remove_file(&path);
    match err {
        SpecError::Parse(msg) => assert!(
            msg.contains("Swagger 2.0"),
            "expected Swagger 2.0 message, got: {msg}"
        ),
        other => panic!("expected Parse error, got {other:?}"),
    }
}

/// A response header declaring only `description` gets a `schema: {type: string}`
/// fallback injected so `openapiv3` can deserialise it.
#[test]
fn test_description_only_header_gets_string_schema_fallback() {
    let spec = "openapi: 3.0.0\ninfo:\n  title: t\n  version: '1'\npaths:\n  /x:\n    get:\n      responses:\n        '200':\n          description: ok\n          headers:\n            X-Trace:\n              description: trace id\n";
    let normalised = normalize_oas31_nullable(spec).expect("normalisation must succeed");
    let tree: serde_json::Value =
        serde_json::from_str(&normalised).expect("normalised output is JSON");
    let header = &tree["paths"]["/x"]["get"]["responses"]["200"]["headers"]["X-Trace"];
    assert_eq!(header["schema"]["type"], "string");
    let spec_obj = load_spec(spec).expect("spec must load after fallback injection");
    assert!(spec_obj.paths.paths.contains_key("/x"));
}

/// An integer bound below `i64::MIN` is clamped to `i64::MIN` and the resulting
/// spec parses cleanly.
#[test]
fn test_minimum_below_i64_min_clamped() {
    let spec = "openapi: 3.0.0\ninfo:\n  title: t\n  version: '1'\npaths: {}\ncomponents:\n  schemas:\n    Seed:\n      type: integer\n      minimum: -9223372036854776000\n";
    let normalised = normalize_oas31_nullable(spec).expect("normalisation must succeed");
    let tree: serde_json::Value =
        serde_json::from_str(&normalised).expect("normalised output is JSON");
    let minimum = &tree["components"]["schemas"]["Seed"]["minimum"];
    assert_eq!(minimum.as_i64(), Some(i64::MIN));
    load_spec(spec).expect("spec must load after clamping");
}

/// Valid `minimum` values inside the `i64` range remain untouched.
#[test]
fn test_in_range_bounds_unchanged() {
    let spec = "openapi: 3.0.0\ninfo:\n  title: t\n  version: '1'\npaths: {}\ncomponents:\n  schemas:\n    A:\n      type: integer\n      minimum: -1\n    B:\n      type: integer\n      minimum: 100\n";
    let normalised = normalize_oas31_nullable(spec).expect("normalisation must succeed");
    let tree: serde_json::Value =
        serde_json::from_str(&normalised).expect("normalised output is JSON");
    assert_eq!(
        tree["components"]["schemas"]["A"]["minimum"].as_i64(),
        Some(-1)
    );
    assert_eq!(
        tree["components"]["schemas"]["B"]["minimum"].as_i64(),
        Some(100)
    );
}

/// Regression: the OAS 3.1 nullable-type-array rewrite still produces the
/// scalar-type + `nullable: true` pair after the new pre-parse passes run.
#[test]
fn test_nullable_rewrite_still_applies() {
    let spec = "openapi: 3.1.0\ninfo:\n  title: t\n  version: '1'\npaths: {}\ncomponents:\n  schemas:\n    Maybe:\n      type: [\"string\", \"null\"]\n";
    let normalised = normalize_oas31_nullable(spec).expect("normalisation must succeed");
    let tree: serde_json::Value =
        serde_json::from_str(&normalised).expect("normalised output is JSON");
    let schema = &tree["components"]["schemas"]["Maybe"];
    assert_eq!(schema["type"], "string");
    assert_eq!(schema["nullable"], true);
}

/// A numeric `exclusiveMinimum` keyword is rewritten into the OAS 3.0 boolean form
/// (`minimum` + `exclusiveMinimum: true`).
#[test]
fn test_numeric_exclusive_minimum_rewritten() {
    let spec = "openapi: 3.1.0\ninfo:\n  title: t\n  version: '1'\npaths: {}\ncomponents:\n  schemas:\n    A:\n      type: integer\n      exclusiveMinimum: 5\n";
    let normalised = normalize_oas31_nullable(spec).expect("normalisation must succeed");
    let tree: serde_json::Value =
        serde_json::from_str(&normalised).expect("normalised output is JSON");
    let schema = &tree["components"]["schemas"]["A"];
    assert_eq!(schema["minimum"].as_i64(), Some(5));
    assert_eq!(schema["exclusiveMinimum"], true);
    load_spec(spec).expect("spec must load after exclusive bound rewrite");
}

/// A numeric `exclusiveMaximum` keyword is rewritten into the OAS 3.0 boolean form
/// (`maximum` + `exclusiveMaximum: true`).
#[test]
fn test_numeric_exclusive_maximum_rewritten() {
    let spec = "openapi: 3.1.0\ninfo:\n  title: t\n  version: '1'\npaths: {}\ncomponents:\n  schemas:\n    A:\n      type: integer\n      exclusiveMaximum: 10\n";
    let normalised = normalize_oas31_nullable(spec).expect("normalisation must succeed");
    let tree: serde_json::Value =
        serde_json::from_str(&normalised).expect("normalised output is JSON");
    let schema = &tree["components"]["schemas"]["A"];
    assert_eq!(schema["maximum"].as_i64(), Some(10));
    assert_eq!(schema["exclusiveMaximum"], true);
}

/// A boolean `exclusiveMinimum` (already in 3.0 form) is left alone — the existing
/// `minimum` companion is preserved verbatim.
#[test]
fn test_boolean_exclusive_minimum_preserved() {
    let spec = "openapi: 3.0.0\ninfo:\n  title: t\n  version: '1'\npaths: {}\ncomponents:\n  schemas:\n    A:\n      type: integer\n      minimum: 5\n      exclusiveMinimum: true\n";
    let normalised = normalize_oas31_nullable(spec).expect("normalisation must succeed");
    let tree: serde_json::Value =
        serde_json::from_str(&normalised).expect("normalised output is JSON");
    let schema = &tree["components"]["schemas"]["A"];
    assert_eq!(schema["minimum"].as_i64(), Some(5));
    assert_eq!(schema["exclusiveMinimum"], true);
}

/// `items: false` alongside `prefixItems` is dropped so the downstream `openapiv3`
/// deserialiser does not reject the schema.
#[test]
fn test_items_false_dropped_after_prefix_items() {
    let spec = "openapi: 3.1.0\ninfo:\n  title: t\n  version: '1'\npaths: {}\ncomponents:\n  schemas:\n    Tuple:\n      type: array\n      prefixItems:\n        - type: string\n        - type: integer\n      items: false\n";
    let normalised = normalize_oas31_nullable(spec).expect("normalisation must succeed");
    let tree: serde_json::Value =
        serde_json::from_str(&normalised).expect("normalised output is JSON");
    let schema = &tree["components"]["schemas"]["Tuple"];
    assert!(schema.get("items").is_none(), "items must be dropped");
    assert!(schema.get("prefixItems").is_some(), "prefixItems preserved");
}

/// `items: true` alongside `prefixItems` is also dropped.
#[test]
fn test_items_true_dropped_after_prefix_items() {
    let spec = "openapi: 3.1.0\ninfo:\n  title: t\n  version: '1'\npaths: {}\ncomponents:\n  schemas:\n    Tuple:\n      type: array\n      prefixItems:\n        - type: string\n      items: true\n";
    let normalised = normalize_oas31_nullable(spec).expect("normalisation must succeed");
    let tree: serde_json::Value =
        serde_json::from_str(&normalised).expect("normalised output is JSON");
    let schema = &tree["components"]["schemas"]["Tuple"];
    assert!(schema.get("items").is_none(), "items must be dropped");
}

/// A path-item `$ref` resolving against `components.pathItems` is inlined so the
/// operation is mounted under the original path.
#[test]
fn test_path_item_ref_inlined() {
    let spec = "openapi: 3.1.0\ninfo:\n  title: t\n  version: '1'\npaths:\n  /foo:\n    $ref: '#/components/pathItems/Shared'\ncomponents:\n  pathItems:\n    Shared:\n      get:\n        responses:\n          '200':\n            description: ok\n";
    let normalised = normalize_oas31_nullable(spec).expect("normalisation must succeed");
    let tree: serde_json::Value =
        serde_json::from_str(&normalised).expect("normalised output is JSON");
    let path = &tree["paths"]["/foo"];
    assert!(
        path.get("get").is_some(),
        "expected inlined GET operation, got {path:?}"
    );
    let spec_obj = load_spec(spec).expect("spec must load with inlined path-item ref");
    assert!(spec_obj.paths.paths.contains_key("/foo"));
}

/// A callback `$ref` resolving against `components.callbacks` is inlined so the
/// parent operation parses cleanly.
#[test]
fn test_callback_ref_inlined() {
    let spec = "openapi: 3.1.0\ninfo:\n  title: t\n  version: '1'\npaths:\n  /trigger:\n    post:\n      responses:\n        '200':\n          description: ok\n      callbacks:\n        onEvent:\n          $ref: '#/components/callbacks/EventHook'\ncomponents:\n  callbacks:\n    EventHook:\n      '{$request.body#/callbackUrl}':\n        post:\n          responses:\n            '200':\n              description: ack\n";
    let normalised = normalize_oas31_nullable(spec).expect("normalisation must succeed");
    let tree: serde_json::Value =
        serde_json::from_str(&normalised).expect("normalised output is JSON");
    let callback = &tree["paths"]["/trigger"]["post"]["callbacks"]["onEvent"];
    assert!(
        callback.get("$ref").is_none(),
        "expected inlined callback, got {callback:?}"
    );
    load_spec(spec).expect("spec must load with inlined callback ref");
}

/// Regression: a nullable rewrite combined with the new header-fallback and
/// bound-clamp passes still yields a parseable spec.
#[test]
fn test_combined_normalisation_loads_cleanly() {
    let spec = "openapi: 3.1.0\ninfo:\n  title: t\n  version: '1'\npaths:\n  /x:\n    get:\n      responses:\n        '200':\n          description: ok\n          headers:\n            X-Trace:\n              description: trace id\ncomponents:\n  schemas:\n    Seed:\n      type: [\"integer\", \"null\"]\n      minimum: -9223372036854776000\n";
    let spec_obj = load_spec(spec).expect("combined pass must load cleanly");
    assert!(spec_obj.paths.paths.contains_key("/x"));
}

/// A `$ref` pointing to a missing entry under `components.schemas` is rejected
/// with a clear error.
#[test]
fn test_dangling_ref_to_components_schemas_rejected() {
    let spec = "openapi: 3.0.0\ninfo:\n  title: t\n  version: '1'\npaths:\n  /x:\n    get:\n      responses:\n        '200':\n          description: ok\n          content:\n            application/json:\n              schema:\n                $ref: '#/components/schemas/MissingSchema'\ncomponents:\n  schemas:\n    Present:\n      type: string\n";
    let err = load_spec(spec).expect_err("dangling ref must be rejected");
    match err {
        SpecError::Parse(msg) => {
            assert!(msg.contains("dangling $ref"), "got: {msg}");
            assert!(msg.contains("MissingSchema"), "got: {msg}");
        }
        other => panic!("expected Parse error, got {other:?}"),
    }
}

/// Two operations sharing the same `operationId` are rejected with a message
/// naming both call sites.
#[test]
fn test_duplicate_operation_id_rejected() {
    let spec = "openapi: 3.0.0\ninfo:\n  title: t\n  version: '1'\npaths:\n  /a:\n    get:\n      operationId: doThing\n      responses:\n        '200':\n          description: ok\n  /b:\n    post:\n      operationId: doThing\n      responses:\n        '200':\n          description: ok\n";
    let err = load_spec(spec).expect_err("duplicate operationId must be rejected");
    match err {
        SpecError::Parse(msg) => {
            assert!(msg.contains("duplicate operationId"), "got: {msg}");
            assert!(msg.contains("doThing"), "got: {msg}");
            assert!(msg.contains("/a"), "got: {msg}");
            assert!(msg.contains("/b"), "got: {msg}");
        }
        other => panic!("expected Parse error, got {other:?}"),
    }
}

/// An unsupported `openapi` version string (here `2.5.6`) is rejected before
/// downstream parsing runs.
#[test]
fn test_unsupported_openapi_version_rejected() {
    let spec = "openapi: 2.5.6\ninfo:\n  title: t\n  version: '1'\npaths: {}\n";
    let err = load_spec(spec).expect_err("unsupported version must be rejected");
    match err {
        SpecError::Parse(msg) => {
            assert!(msg.contains("unsupported openapi version"), "got: {msg}");
            assert!(msg.contains("2.5.6"), "got: {msg}");
        }
        other => panic!("expected Parse error, got {other:?}"),
    }
}

/// A schema whose `type` keyword is outside the closed JSON Schema set is rejected
/// with a pointer to the offending node.
#[test]
fn test_invalid_schema_type_rejected() {
    let spec = "openapi: 3.0.0\ninfo:\n  title: t\n  version: '1'\npaths: {}\ncomponents:\n  schemas:\n    Weird:\n      type: not-a-real-type\n";
    let err = load_spec(spec).expect_err("invalid schema type must be rejected");
    match err {
        SpecError::Parse(msg) => {
            assert!(msg.contains("invalid schema type"), "got: {msg}");
            assert!(msg.contains("not-a-real-type"), "got: {msg}");
        }
        other => panic!("expected Parse error, got {other:?}"),
    }
}

/// Sanity check: a small valid petstore-style spec with multiple distinct
/// `operationId`s, resolvable refs, a supported version, and well-formed schema
/// types must continue to load cleanly.
#[test]
fn test_valid_petstore_spec_loads() {
    let spec = "openapi: 3.0.0\ninfo:\n  title: petstore\n  version: '1'\npaths:\n  /pets:\n    get:\n      operationId: listPets\n      responses:\n        '200':\n          description: ok\n          content:\n            application/json:\n              schema:\n                $ref: '#/components/schemas/Pet'\n    post:\n      operationId: createPet\n      responses:\n        '201':\n          description: created\ncomponents:\n  schemas:\n    Pet:\n      type: object\n      properties:\n        name:\n          type: string\n";
    let spec_obj = load_spec(spec).expect("valid petstore must load");
    assert!(spec_obj.paths.paths.contains_key("/pets"));
}

/// An inline spec string whose length exceeds `MAX_SPEC_BYTES` (64 MiB) must
/// be rejected at load time with a `SpecError::Parse` rather than being
/// parsed. Guards against memory blowup on hostile or malformed input.
#[test]
fn test_oversized_inline_spec_rejected() {
    let mut spec = String::from("openapi: 3.0.0\ninfo:\n  title: t\n  version: '1'\npaths: {}\n");
    spec.push_str("# ");
    spec.push_str(&"a".repeat(64 * 1024 * 1024 + 1024));
    spec.push('\n');
    let err = load_spec(&spec).expect_err("oversized spec must be rejected");
    match err {
        SpecError::Parse(msg) => assert!(
            msg.contains("spec size") || msg.contains("size") || msg.contains("too large"),
            "expected size-related message, got: {msg}"
        ),
        other => panic!("expected Parse error, got {other:?}"),
    }
}

/// `read_capped` returns the full body when the stream stays within the cap.
#[test]
fn test_read_capped_under_limit_returns_full_body() {
    let body = vec![0xABu8; 1024];
    let got = read_capped(std::io::Cursor::new(body.clone()), 4096)
        .expect("body under cap must be returned in full");
    assert_eq!(got.len(), 1024);
    assert!(got.iter().all(|b| *b == 0xAB));
}

/// `read_capped` returns the full body when its length exactly matches the cap.
#[test]
fn test_read_capped_at_limit_returns_full_body() {
    let body = vec![0x01u8; 2048];
    let got = read_capped(std::io::Cursor::new(body.clone()), 2048)
        .expect("body at exact cap must be returned in full");
    assert_eq!(got.len(), 2048);
}

/// `read_capped` rejects an in-memory stream the moment cumulative byte count
/// exceeds the cap, even when no advisory length is available — this is the
/// streaming-DoS guard. A 10 MiB body with a 1 MiB cap must fail.
#[test]
fn test_read_capped_over_limit_aborts() {
    let body = vec![0xFFu8; 10 * 1024 * 1024];
    let cap = 1024 * 1024;
    let err =
        read_capped(std::io::Cursor::new(body), cap).expect_err("body over cap must be rejected");
    match err {
        ReadCappedError::ExceedsCap => {}
        other => panic!("expected ExceedsCap, got {other:?}"),
    }
}

/// `read_capped` rejects a body that crosses the cap by even one byte.
#[test]
fn test_read_capped_rejects_one_byte_over() {
    let body = vec![0x42u8; 1025];
    let err = read_capped(std::io::Cursor::new(body), 1024)
        .expect_err("body one byte over cap must be rejected");
    assert!(matches!(err, ReadCappedError::ExceedsCap));
}

/// `read_capped` propagates I/O errors verbatim instead of swallowing them.
#[test]
fn test_read_capped_propagates_io_error() {
    struct AlwaysFailReader;
    impl std::io::Read for AlwaysFailReader {
        fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
            Err(std::io::Error::other("boom"))
        }
    }
    let err = read_capped(AlwaysFailReader, 1024).expect_err("io error must propagate");
    match err {
        ReadCappedError::Io(_) => {}
        other => panic!("expected Io error, got {other:?}"),
    }
}

/// `SpecError::Io` carries the offending path and the original
/// `std::io::Error` as a `source`, so callers can read the underlying error
/// chain rather than a flattened string.
#[test]
fn test_spec_error_io_carries_path_and_source() {
    let missing = "/tmp/this-path-must-not-exist/chasm-spec-xyz-zzz.yaml";

    let err = load_spec(missing).expect_err("loading a missing file must fail");

    match err {
        SpecError::Io { path, source } => {
            assert_eq!(path, missing, "path field must echo the offending path");
            assert_eq!(
                source.kind(),
                std::io::ErrorKind::NotFound,
                "underlying io::Error must be preserved as source"
            );
        }
        other => panic!("expected SpecError::Io {{ path, source }}, got {other:?}"),
    }
}
