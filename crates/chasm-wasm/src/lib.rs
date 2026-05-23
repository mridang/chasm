use chasm_engine::{load_spec, mock, MockConfig, MockError, MockRequest, OpenAPI, ValidationError};
use serde::Serialize;
use serde_json::json;
use std::collections::HashMap;
use uuid::Uuid;
use wasm_bindgen::prelude::*;

/// Problem document `type` URI for the `NO_PATH_MATCHED_ERROR` class — matches
/// the server-side wire contract in `chasm-server/src/main.rs`.
const TYPE_NO_PATH_MATCHED: &str = "https://chasm.dev/errors#NO_PATH_MATCHED_ERROR";

/// Problem document `type` URI for the `NO_METHOD_MATCHED_ERROR` class — matches
/// the server-side wire contract in `chasm-server/src/main.rs`.
const TYPE_NO_METHOD_MATCHED: &str = "https://chasm.dev/errors#NO_METHOD_MATCHED_ERROR";

/// Problem document `type` URI for the `NO_RESPONSE_RESPONSE_DEFINED` class —
/// matches the server-side wire contract in `chasm-server/src/main.rs`. The
/// double-`RESPONSE` is preserved verbatim so existing ecosystem tooling that
/// keys off the literal token keeps working.
const TYPE_NO_RESPONSE_RESPONSE_DEFINED: &str =
    "https://chasm.dev/errors#NO_RESPONSE_RESPONSE_DEFINED";

/// Problem document `type` URI for the `NO_RESPONSE_DEFINED` class — matches
/// the server-side wire contract in `chasm-server/src/main.rs`.
const TYPE_NO_RESPONSE_DEFINED: &str = "https://chasm.dev/errors#NO_RESPONSE_DEFINED";

/// Problem document `type` URI for the `UNPROCESSABLE_ENTITY` class — matches
/// the server-side wire contract in `chasm-server/src/main.rs`.
const TYPE_UNPROCESSABLE_ENTITY: &str = "https://chasm.dev/errors#UNPROCESSABLE_ENTITY";

/// Problem document `type` URI for the `UNAUTHORIZED` class — matches the
/// server-side wire contract in `chasm-server/src/main.rs`.
const TYPE_UNAUTHORIZED: &str = "https://chasm.dev/errors#UNAUTHORIZED";

/// Problem document `type` URI for the `NOT_FOUND` class — matches the
/// server-side wire contract in `chasm-server/src/main.rs`.
const TYPE_NOT_FOUND: &str = "https://chasm.dev/errors#NOT_FOUND";

/// Problem document `type` URI for the `NOT_ACCEPTABLE` class — matches the
/// server-side wire contract in `chasm-server/src/main.rs`.
const TYPE_NOT_ACCEPTABLE: &str = "https://chasm.dev/errors#NOT_ACCEPTABLE";

/// Problem document `type` URI for the `INTERNAL_SERVER_ERROR` class — matches
/// the server-side wire contract in `chasm-server/src/main.rs`.
const TYPE_INTERNAL_SERVER_ERROR: &str = "https://chasm.dev/errors#INTERNAL_SERVER_ERROR";

/// Builds the `instance` URN attached to every WASM error envelope.
///
/// The WASM API has no request-id concept (each `handle` call is independent),
/// so a fresh UUIDv4 is minted per error and formatted under the
/// `urn:chasm:wasm:` namespace to mirror the server's
/// `urn:chasm:request:<id>` shape.
fn new_wasm_instance_urn() -> String {
    format!("urn:chasm:wasm:{}", Uuid::new_v4())
}

/// Injects typed TypeScript interface declarations into the wasm-bindgen
/// generated `.d.ts` so JS/TS consumers see real shapes for the `handle` and
/// `handle_with_options` return values instead of `any`.
#[wasm_bindgen(typescript_custom_section)]
const TS_INTERFACES: &'static str = r#"
export interface MockResponse {
    status: number;
    contentType: string;
    body: unknown;
}
export interface MockErrorEnvelope {
    type: string;  // RFC 7807 type URI (e.g. "https://chasm.dev/errors#NO_PATH_MATCHED_ERROR")
    title?: string;
    status: number;
    detail?: string;
    instance?: string;
    allow?: string;
    scheme?: string;
    wwwAuthenticate?: string;
    contentType?: string;
    exampleKey?: string;
    code?: number;
    method?: string;
    path?: string;
    acceptable?: string[];
    validation?: Array<{
        location: [string, string];
        severity: string;
        code: string;
        message: string;
    }>;
}
"#;

/// WASM-exposed mock server backed by a pre-loaded OAS3 specification.
#[wasm_bindgen]
#[derive(Debug)]
pub struct Chasm {
    spec: OpenAPI,
    dynamic: bool,
}

#[wasm_bindgen]
impl Chasm {
    /// Parses `spec` as a JSON or YAML OAS 3.x document and creates a new
    /// `Chasm` instance.
    ///
    /// Accepts the spec inline as either a JSON object literal or a YAML
    /// document. The input is pre-classified by inspecting its leading
    /// non-whitespace characters: anything that does not begin with `{`,
    /// `openapi`, `#`, or `---` is rejected up-front with a clear message
    /// rather than being forwarded to `chasm_engine::load_spec`, whose
    /// fallback filesystem branch is meaningless in the WASM sandbox.
    ///
    /// Returns a `JsValue` error string if pre-classification or parsing
    /// fails.
    #[wasm_bindgen(constructor)]
    pub fn new(spec: &str, dynamic: bool) -> Result<Chasm, JsValue> {
        let trimmed = spec.trim_start();
        if !trimmed.starts_with('{')
            && !trimmed.starts_with("openapi")
            && !trimmed.starts_with('#')
            && !trimmed.starts_with("---")
        {
            return Err(JsValue::from_str(
                "spec must be a JSON or YAML OpenAPI 3.x document; got input that does not begin with '{', 'openapi', '#', or '---'",
            ));
        }
        let spec = load_spec(spec).map_err(|e| JsValue::from_str(&e.to_string()))?;
        Ok(Chasm { spec, dynamic })
    }

    /// Generates a mock response for the given request parameters using default
    /// engine configuration apart from the constructor-supplied `dynamic` flag.
    ///
    /// Resolves to a plain JS object matching the `MockResponse` TypeScript
    /// interface — `{ status, contentType, body }` — that survives
    /// `JSON.stringify` and supports normal property access (`result.status`).
    /// Errors are thrown as structured JS objects matching `MockErrorEnvelope`,
    /// shaped like `{ status, type, detail?, scheme?, validation? }`, so JS
    /// callers can use `try { chasm.handle(...) } catch (e) { ... }` to branch
    /// on the failure.
    #[wasm_bindgen(unchecked_return_type = "MockResponse")]
    pub fn handle(
        &self,
        method: &str,
        path: &str,
        prefer: &str,
        accept: &str,
    ) -> Result<JsValue, JsValue> {
        let cfg = {
            let mut __c = MockConfig::default();
            __c.dynamic = self.dynamic;
            __c
        };
        self.run(method, path, prefer, accept, cfg)
    }

    /// Generates a mock response with caller-supplied [`MockConfig`] overrides
    /// parsed from `options_json`.
    ///
    /// `options_json` must be a JSON object literal. Recognised keys map to
    /// [`MockConfig`] fields and are all optional:
    ///
    /// - `seed` (`u64`) — deterministic seed for dynamic generation.
    /// - `forceCode` (`u16`) — force a specific response status code.
    /// - `exampleKey` (`string`) — pick a named example from the response.
    /// - `ignoreExamples` (`bool`) — skip the example pipeline.
    /// - `errors` (`bool`) — enforce request validation and return `422` on
    ///   failure rather than letting the response generate.
    /// - `fillProperties` (`bool`) — `json-schema-faker` fill-properties knob.
    ///
    /// Unknown keys are ignored. The success result is a plain JS object
    /// matching the `MockResponse` TypeScript interface and the error envelope
    /// matches `MockErrorEnvelope`; both shapes are identical to
    /// [`Chasm::handle`].
    #[wasm_bindgen(unchecked_return_type = "MockResponse")]
    pub fn handle_with_options(
        &self,
        method: &str,
        path: &str,
        prefer: &str,
        accept: &str,
        options_json: &str,
    ) -> Result<JsValue, JsValue> {
        let opts: serde_json::Value =
            serde_json::from_str(options_json).map_err(|e| JsValue::from_str(&e.to_string()))?;
        let mut cfg = {
            let mut __c = MockConfig::default();
            __c.dynamic = self.dynamic;
            __c
        };
        if let Some(v) = opts.get("seed").and_then(|v| v.as_u64()) {
            cfg.seed = Some(v);
        }
        if let Some(v) = opts.get("forceCode").and_then(|v| v.as_u64()) {
            cfg.force_code = Some(v as u16);
        }
        if let Some(v) = opts.get("exampleKey").and_then(|v| v.as_str()) {
            cfg.example_key = Some(v.to_string());
        }
        if let Some(v) = opts.get("ignoreExamples").and_then(|v| v.as_bool()) {
            cfg.ignore_examples = v;
        }
        if let Some(v) = opts.get("errors").and_then(|v| v.as_bool()) {
            cfg.errors = v;
        }
        if let Some(v) = opts.get("fillProperties").and_then(|v| v.as_bool()) {
            cfg.fill_properties = Some(v);
        }
        self.run(method, path, prefer, accept, cfg)
    }

    /// Shared core that builds the [`MockRequest`], invokes the engine, and
    /// converts the result into the JS-facing plain-object success value or
    /// structured error envelope.
    ///
    /// Uses a `serde_wasm_bindgen::Serializer` configured with
    /// `serialize_maps_as_objects(true)` so the returned `JsValue` is a real
    /// JS object — `JSON.stringify` round-trips it and `result.status` works
    /// — rather than a `Map`, which is what the default serializer emits.
    fn run(
        &self,
        method: &str,
        path: &str,
        prefer: &str,
        accept: &str,
        cfg: MockConfig,
    ) -> Result<JsValue, JsValue> {
        let mut headers: HashMap<String, String> = HashMap::new();
        if !prefer.is_empty() {
            headers.insert("Prefer".to_string(), prefer.to_string());
        }
        if !accept.is_empty() {
            headers.insert("Accept".to_string(), accept.to_string());
        }

        let req = {
            let mut __r = MockRequest::default();
            __r.method = method.to_string();
            __r.path = path.to_string();
            __r.headers = headers;
            __r.query = HashMap::new();
            __r.body = None;
            __r
        };

        match mock(&self.spec, &req, &cfg) {
            Ok(resp) => {
                let obj = json!({
                    "status": resp.status,
                    "contentType": resp.content_type,
                    "body": resp.body,
                });
                let serializer =
                    serde_wasm_bindgen::Serializer::new().serialize_maps_as_objects(true);
                obj.serialize(&serializer)
                    .map_err(|e| JsValue::from_str(&e.to_string()))
            }
            Err(e) => Err(mock_error_to_js(&e)),
        }
    }
}

/// Maps a [`MockError`] into the structured `JsValue` thrown to JS callers.
///
/// The wire shape uses the same RFC 7807 error classes the server exposes,
/// surfaced as a plain JS object (via a `serialize_maps_as_objects(true)`
/// serializer) rather than a `Map` or problem document so callers can branch
/// on `type` without parsing JSON.
fn mock_error_to_js(err: &MockError) -> JsValue {
    let payload = mock_error_to_payload(err);
    let serializer = serde_wasm_bindgen::Serializer::new().serialize_maps_as_objects(true);
    payload
        .serialize(&serializer)
        .unwrap_or_else(|e| JsValue::from_str(&e.to_string()))
}

/// Builds the JSON payload (`serde_json::Value`) describing a [`MockError`]
/// for surfacing to JS callers.
///
/// Split out from [`mock_error_to_js`] so the mapping table can be exercised
/// from native `cargo test` runs (which cannot call into `JsValue` /
/// `wasm-bindgen` describe machinery) without going through the
/// `wasm-bindgen` serializer.
fn mock_error_to_payload(err: &MockError) -> serde_json::Value {
    let instance = new_wasm_instance_urn();
    match err {
        MockError::NoRoute { .. } => json!({
            "status": 404,
            "type": TYPE_NO_PATH_MATCHED,
            "detail": err.to_string(),
            "instance": instance,
        }),
        MockError::MethodNotAllowed { allow, .. } => json!({
            "status": 405,
            "type": TYPE_NO_METHOD_MATCHED,
            "detail": err.to_string(),
            "allow": allow,
            "instance": instance,
        }),
        MockError::Unauthorized {
            scheme,
            www_authenticate,
        } => json!({
            "status": 401,
            "type": TYPE_UNAUTHORIZED,
            "scheme": scheme,
            "wwwAuthenticate": www_authenticate,
            "instance": instance,
        }),
        MockError::ValidationFailed(errs) => json!({
            "status": 422,
            "type": TYPE_UNPROCESSABLE_ENTITY,
            "validation": errs.iter().map(validation_error_to_value).collect::<Vec<_>>(),
            "instance": instance,
        }),
        MockError::ExampleNotFound {
            content_type,
            example_key,
        } => json!({
            "status": 404,
            "type": TYPE_NOT_FOUND,
            "contentType": content_type,
            "exampleKey": example_key,
            "instance": instance,
        }),
        MockError::Generation { source, .. } => json!({
            "status": 500,
            "type": TYPE_NO_RESPONSE_RESPONSE_DEFINED,
            "title": "Response body generation failed",
            "detail": format!("Response body generation failed: {}", source),
            "instance": instance,
        }),
        MockError::NoResponseDefined => json!({
            "status": 500,
            "type": TYPE_NO_RESPONSE_DEFINED,
            "detail": err.to_string(),
            "instance": instance,
        }),
        MockError::SpecSerialization(msg) => json!({
            "status": 500,
            "type": TYPE_INTERNAL_SERVER_ERROR,
            "detail": msg,
            "instance": instance,
        }),
        MockError::NoResponseForCode { code, method, path } => json!({
            "status": 404,
            "type": TYPE_NO_RESPONSE_DEFINED,
            "detail": err.to_string(),
            "code": code,
            "method": method,
            "path": path,
            "instance": instance,
        }),
        MockError::NotAcceptable { acceptable } => json!({
            "status": 406,
            "type": TYPE_NOT_ACCEPTABLE,
            "detail": err.to_string(),
            "acceptable": acceptable,
            "instance": instance,
        }),
        _ => json!({
            "status": 500,
            "type": TYPE_INTERNAL_SERVER_ERROR,
            "detail": err.to_string(),
            "instance": instance,
        }),
    }
}

/// Serialises a single [`ValidationError`] into the per-entry shape used
/// inside the `validation` array of the `UNPROCESSABLE_ENTITY` envelope.
fn validation_error_to_value(err: &ValidationError) -> serde_json::Value {
    json!({
        "location": [err.location.as_str(), err.field],
        "severity": err.severity.as_str(),
        "code": err.code,
        "message": err.message,
    })
}

/// Tests for this module exercise the `wasm-bindgen`-decorated `Chasm::new`
/// constructor, which depends on `JsValue::from_str` and `wasm-bindgen`
/// describe machinery. Those entry points panic when run on a native target,
/// so this module is gated to `target_arch = "wasm32"` and must be executed
/// via `wasm-pack test --node` (or another wasm32 test runner) rather than
/// `cargo test`.
#[cfg(all(test, target_arch = "wasm32"))]
mod tests {
    use super::*;

    /// Asserts that the constructor rejects inputs that look like file paths
    /// — anything not starting with `{`, `openapi`, `#`, or `---` — with the
    /// dedicated guidance message rather than letting `load_spec` fall through
    /// to its filesystem branch (which surfaces a meaningless I/O error in the
    /// WASM sandbox).
    #[test]
    fn test_rejects_non_spec_input_with_clear_message() {
        let err = Chasm::new("not yaml", false).expect_err("non-spec input must be rejected");
        let msg = err.as_string().expect("error is a string");
        assert!(
            msg.contains("spec must be a JSON or YAML OpenAPI 3.x document"),
            "expected pre-classification message, got: {msg}"
        );
    }

    /// Asserts that the pre-classification preflight rejects an empty string
    /// rather than passing it to `load_spec`.
    #[test]
    fn test_rejects_empty_input() {
        let err = Chasm::new("", false).expect_err("empty input must be rejected");
        let msg = err.as_string().expect("error is a string");
        assert!(msg.contains("spec must be a JSON or YAML"));
    }

    /// Asserts that the pre-classification preflight rejects whitespace-only
    /// input the same way it rejects an empty string.
    #[test]
    fn test_rejects_whitespace_only_input() {
        let err = Chasm::new("   \n\t ", false).expect_err("whitespace must be rejected");
        let msg = err.as_string().expect("error is a string");
        assert!(msg.contains("spec must be a JSON or YAML"));
    }

    /// Asserts that the pre-classification preflight allows YAML inputs that
    /// begin with a comment line (leading `#`) — they should reach `load_spec`
    /// even if `load_spec` itself goes on to reject the body as malformed.
    #[test]
    fn test_allows_yaml_comment_prefix_through_preflight() {
        let err = Chasm::new("# a comment only", false)
            .expect_err("malformed body still fails downstream");
        let msg = err.as_string().expect("error is a string");
        assert!(
            !msg.contains("spec must be a JSON or YAML OpenAPI 3.x document"),
            "comment-prefixed input must pass the preflight; got preflight message: {msg}"
        );
    }

    /// Asserts that a minimal valid YAML OAS 3.0 spec loads via the
    /// constructor without tripping the preflight.
    #[test]
    fn test_accepts_minimal_yaml_spec() {
        let spec = "openapi: 3.0.0\ninfo:\n  title: t\n  version: '1'\npaths: {}\n";
        Chasm::new(spec, false).expect("minimal yaml spec must load");
    }
}

/// Native-target regression tests that exercise the [`MockError`] → JSON
/// envelope mapping without going through `JsValue` / `wasm-bindgen`
/// describe machinery, so they can run under `cargo test` on the host
/// toolchain rather than requiring `wasm-pack test --node`.
#[cfg(all(test, not(target_arch = "wasm32")))]
mod native_tests {
    use super::*;

    /// Asserts that `MockError::NoResponseForCode` maps to a `404` envelope
    /// with `type == "NO_RESPONSE_DEFINED"` and carries the `code`, `method`,
    /// and `path` fields — the regression covering the previously-silent
    /// fall-through to the generic `500 INTERNAL_SERVER_ERROR` arm.
    #[test]
    fn test_no_response_for_code_maps_to_404_envelope() {
        let err = MockError::NoResponseForCode {
            method: "GET".to_string(),
            path: "/pets".to_string(),
            code: 418,
        };
        let payload = mock_error_to_payload(&err);
        assert_eq!(payload.get("status").and_then(|v| v.as_u64()), Some(404));
        assert_eq!(
            payload.get("type").and_then(|v| v.as_str()),
            Some(TYPE_NO_RESPONSE_DEFINED),
        );
        assert_eq!(payload.get("code").and_then(|v| v.as_u64()), Some(418));
        assert_eq!(payload.get("method").and_then(|v| v.as_str()), Some("GET"),);
        assert_eq!(payload.get("path").and_then(|v| v.as_str()), Some("/pets"),);
    }

    /// Asserts that `MockError::NotAcceptable` maps to a `406` envelope with
    /// `type` equal to [`TYPE_NOT_ACCEPTABLE`]
    /// (`"https://chasm.dev/errors#NOT_ACCEPTABLE"`) and the `acceptable`
    /// array, covering the other previously-silent fall-through.
    #[test]
    fn test_not_acceptable_maps_to_406_envelope() {
        let err = MockError::NotAcceptable {
            acceptable: vec!["application/json".to_string(), "text/plain".to_string()],
        };
        let payload = mock_error_to_payload(&err);
        assert_eq!(payload.get("status").and_then(|v| v.as_u64()), Some(406));
        assert_eq!(
            payload.get("type").and_then(|v| v.as_str()),
            Some(TYPE_NOT_ACCEPTABLE),
        );
        let acceptable = payload
            .get("acceptable")
            .and_then(|v| v.as_array())
            .expect("acceptable is an array");
        let values: Vec<&str> = acceptable.iter().filter_map(|v| v.as_str()).collect();
        assert_eq!(values, vec!["application/json", "text/plain"]);
    }

    /// Every WASM error envelope must carry an `instance` URN under the
    /// `urn:chasm:` namespace so callers can correlate envelopes with logs
    /// even though the WASM API has no request-id concept of its own.
    #[test]
    fn test_envelope_carries_instance_urn() {
        let err = MockError::NoRoute {
            method: "GET".to_string(),
            path: "/missing".to_string(),
        };
        let payload = mock_error_to_payload(&err);
        let instance = payload
            .get("instance")
            .and_then(|v| v.as_str())
            .expect("instance field is present");
        assert!(
            instance.starts_with("urn:chasm:"),
            "expected urn:chasm: prefix, got {instance}"
        );
    }

    /// WASM error envelopes use full URI form for `type`, matching the server
    /// wire contract and conforming to RFC 7807.
    #[test]
    fn test_envelope_type_uses_full_uri_form() {
        let err = MockError::NoRoute {
            method: "GET".into(),
            path: "/foo".into(),
        };
        let payload = mock_error_to_payload(&err);

        let type_value = payload["type"].as_str().expect("type field");

        assert!(
            type_value.starts_with("https://"),
            "type must be a URI, got {:?}",
            type_value
        );
        assert!(type_value.contains("NO_PATH_MATCHED_ERROR"));
    }

    /// WASM `MockError::NoResponseDefined` and `MockError::SpecSerialization`
    /// emit distinct `type` URIs, not collapsed under `NO_RESPONSE_RESPONSE_DEFINED`.
    #[test]
    fn test_no_response_defined_and_spec_serialization_have_distinct_envelopes() {
        let no_response = mock_error_to_payload(&MockError::NoResponseDefined);
        let serde_fail = mock_error_to_payload(&MockError::SpecSerialization("test".into()));

        assert_ne!(no_response["type"], serde_fail["type"]);
    }
}
