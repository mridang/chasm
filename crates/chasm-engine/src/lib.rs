//! chasm-engine: OpenAPI 3.x mock engine.
//!
//! Give it an OpenAPI 3 spec plus a description of an incoming request,
//! get back a mock response (status, content-type, body). No HTTP layer —
//! that lives in `chasm-server`. This crate is what you'd embed in a custom
//! test harness, a Lambda handler, or a WASM bundle.
//!
//! # Quick start
//!
//! ```no_run
//! use chasm_engine::{load_spec, mock, MockConfig, MockRequest};
//!
//! let spec = load_spec(include_str!("../../../etc/petstore.yaml")).unwrap();
//!
//! let mut req = MockRequest::default();
//! req.method = "GET".into();
//! req.path   = "/pets".into();
//!
//! let cfg = MockConfig::default();
//! let resp = mock(&spec, &req, &cfg).unwrap();
//!
//! assert_eq!(resp.status, 200);
//! ```
//!
//! # Where to look next
//!
//! - [`mock`] — main entry point. Spec + request + config in, response out.
//! - [`MockRequest`] / [`MockResponse`] / [`MockConfig`] — request/response/config shapes.
//! - [`load_spec`] — parse an OAS3 spec from JSON or YAML.
//! - [`PreferDirectives`] — RFC 7240 `Prefer` header parser (`code=`, `example=`,
//!   `dynamic=`, `seed=`, `validate=`, `security=`).
//! - [`route_request`] — path-template matching, exposed for callers that
//!   want routing without the full mock pipeline.
//! - [`MockError`] / [`SpecError`] / [`ValidationError`] — error types.

pub mod engine;
pub mod loader;
pub mod prefer;
pub mod router;
pub mod security;
pub mod validation;

/// Long-form reference documentation for chasm.
///
/// These are markdown files in the repository's `docs/` directory, embedded
/// into rustdoc via `include_str!` so they render with the same styling and
/// search as the API reference. The source markdown also renders on GitHub.
pub mod guides {
    /// Scope of the chasm project: what's in, what's out, why.
    #[doc = include_str!("../../../docs/SCOPE.md")]
    pub mod scope {}

    /// `Prefer` request-header semantics: which directives chasm honours and
    /// how they interact with the spec.
    #[doc = include_str!("../../../docs/PREFER_HEADER.md")]
    pub mod prefer_header {}
}

use chasm_faker::FakerError;
use thiserror::Error;

pub use engine::{mock, MockConfig, MockRequest, MockResponse};
pub use loader::load_spec;
pub use openapiv3::OpenAPI;
pub use prefer::PreferDirectives;
pub use router::{route_request, route_request_with_strict, RouteMatch};
pub use security::{evaluate as evaluate_security, SecurityResult};
pub use validation::{
    validate as validate_request_full, ValidationError, ValidationLocation, ValidationSeverity,
};

/// Errors that occur while loading or parsing an OAS3 specification.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum SpecError {
    /// The file could not be read from disk. Carries the offending path and
    /// the original `std::io::Error` as `source` so callers can inspect the
    /// underlying error chain rather than a flattened message string.
    #[error("I/O error reading {path}: {source}")]
    Io {
        /// Path that was being read when the error occurred.
        path: String,
        /// Underlying I/O error.
        #[source]
        source: std::io::Error,
    },
    /// The file content could not be parsed as JSON or YAML.
    #[error("Parse error: {0}")]
    Parse(String),
}

/// Renders the Display body for [`MockError::ValidationFailed`], inlining the
/// first error's field and message plus the count of any remaining errors.
fn format_validation_failed(errors: &[ValidationError]) -> String {
    match errors.first() {
        None => "Request validation failed (0 errors)".to_string(),
        Some(first) => {
            let extra = errors.len().saturating_sub(1);
            if extra == 0 {
                format!(
                    "Request validation failed: {}: {}",
                    first.field, first.message
                )
            } else {
                format!(
                    "Request validation failed: {}: {} (+{} more)",
                    first.field, first.message, extra
                )
            }
        }
    }
}

/// Renders the Display body for [`MockError::Unauthorized`], including the
/// `WWW-Authenticate` challenge value when present.
fn format_unauthorized(scheme: &str, www_authenticate: &Option<String>) -> String {
    match www_authenticate {
        Some(challenge) => {
            format!(
                "Unauthorized: scheme '{}' (challenge: {})",
                scheme, challenge
            )
        }
        None => format!("Unauthorized: scheme '{}'", scheme),
    }
}

/// Errors that occur while generating a mock response.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum MockError {
    /// No operation in the spec matched the given method and path.
    #[error("No route for {method} {path}")]
    NoRoute {
        /// HTTP method that was attempted.
        method: String,
        /// Request path that did not match any spec route.
        path: String,
    },
    /// A path matched but the HTTP method is not defined on that path.
    ///
    /// Carries the comma-separated list of methods that **are** declared on the
    /// matched path (already computed by the router) so the server adapter can
    /// emit a conformant `Allow` header on the 405 response.
    #[error("Method {method} not allowed for {path}")]
    MethodNotAllowed {
        /// HTTP method that was attempted.
        method: String,
        /// Request path whose route does not declare the attempted method.
        path: String,
        /// `Allow` header value to emit on the 405 response.
        allow: String,
    },
    /// The faker failed to produce a value from the resolved schema. The wrapped
    /// [`chasm_faker::FakerError`] preserves the original cause chain and any
    /// JSON pointer the faker recorded.
    #[error("Generation error for {method} {path}: {source}")]
    Generation {
        /// HTTP method of the request whose response body could not be generated.
        method: String,
        /// Request path of the request whose response body could not be generated.
        path: String,
        /// Underlying faker error that caused generation to fail.
        #[source]
        source: FakerError,
    },
    /// The matched operation has no usable response definition — its
    /// `responses` map is empty or every entry failed to resolve.
    #[error("operation has no responses defined")]
    NoResponseDefined,
    /// A `Prefer: code=<n>` directive (or `__code=<n>` query) named a status
    /// code that does not exist on the matched operation's `responses` map.
    /// Surfaced as a `NO_RESPONSE_DEFINED` 404 envelope so clients can
    /// distinguish "no such code in spec" from "no route at all".
    #[error("No response defined for code {code} on {method} {path}")]
    NoResponseForCode {
        /// HTTP method of the matched operation.
        method: String,
        /// Request path of the matched operation.
        path: String,
        /// The status code the client requested via `Prefer: code=<n>` or
        /// `__code=<n>` that the operation does not declare.
        code: u16,
    },
    /// The client's `Accept` header was set but no media type the spec declares
    /// for the chosen response status code intersects with any of the offered
    /// ranges. Surfaced as a `NOT_ACCEPTABLE` 406 envelope; the wrapped list
    /// carries the spec-declared media types in document order so the server
    /// adapter can include them in the problem detail.
    #[error("None of the offered media types are acceptable: [{}]", acceptable.join(", "))]
    NotAcceptable {
        /// Spec-declared media types for the chosen response, in document order.
        acceptable: Vec<String>,
    },
    /// Serialising the spec or a sub-schema to JSON failed.
    ///
    /// Carries the rendered `serde_json` error so callers can include it in
    /// problem-document detail strings.
    #[error("Spec serialization error: {0}")]
    SpecSerialization(String),
    /// Request validation failed and the server is configured to enforce it.
    #[error("{}", format_validation_failed(.0))]
    ValidationFailed(Vec<ValidationError>),
    /// Authentication is required but no security requirement was satisfied.
    #[error("{}", format_unauthorized(scheme, www_authenticate))]
    Unauthorized {
        /// Name of the scheme reported in the response.
        scheme: String,
        /// `WWW-Authenticate` header value to emit, when applicable.
        www_authenticate: Option<String>,
    },
    /// A `Prefer: example=<name>` directive (or `__example=<name>` query) named
    /// an example that does not exist in the resolved media type's `examples`
    /// map. Surfaced as a `NOT_FOUND` 404 error envelope, carrying the
    /// resolved content type and the requested example key for inclusion in
    /// the problem document detail.
    #[error(
        "Response for contentType: {content_type} and exampleKey: {example_key} does not exist."
    )]
    ExampleNotFound {
        /// Negotiated response content type when the example was requested.
        content_type: String,
        /// Example key requested via `Prefer: example=<name>` or `__example`.
        example_key: String,
    },
}
