//! Mock-level security scheme evaluation (apiKey, http bearer/basic, oauth2).
//!
//! **Presence-only — DOES NOT authenticate.** Checks that the request
//! supplies a credential of the declared shape (header, query, cookie,
//! bearer token, basic auth pair) but does not validate signatures,
//! decode JWTs, or call any introspection endpoint.

use crate::engine::MockRequest;
use openapiv3::{APIKeyLocation, OpenAPI, Operation, ReferenceOr, SecurityScheme};
use percent_encoding::percent_decode_str;

/// Outcome of evaluating an operation's effective security requirements.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum SecurityResult {
    /// At least one security requirement was satisfied (or none were declared).
    Authorized,
    /// No security requirement was satisfied; the server should emit a 401.
    Unauthorized {
        /// The scheme name reported in the problem JSON `detail` field.
        scheme: String,
        /// The `WWW-Authenticate` header value to send back, when applicable.
        www_authenticate: Option<String>,
    },
}

/// Evaluates the effective security requirement set for `op` against `req`.
///
/// Operation-level security overrides spec-level security; an empty list means
/// no auth is required. Each requirement is a logical AND across its keys, and
/// the list is a logical OR.
///
/// **Banner: chasm is a MOCK server and does NOT authenticate.** API key and
/// bearer schemes are satisfied by header presence only — any non-empty value
/// passes. Do not deploy chasm in a context where authorisation is the
/// security boundary. See `is_scheme_satisfied` (private helper in this
/// module) for the per-scheme rules.
pub fn evaluate(spec: &OpenAPI, op: &Operation, req: &MockRequest) -> SecurityResult {
    let result = evaluate_inner(spec, op, req);
    match &result {
        SecurityResult::Authorized => {
            tracing::trace!(scheme = "n/a", result = "authorized", "security evaluated");
        }
        SecurityResult::Unauthorized { scheme, .. } => {
            tracing::debug!(scheme = %scheme, result = "unauthorized", "security evaluated");
        }
    }
    result
}

fn evaluate_inner(spec: &OpenAPI, op: &Operation, req: &MockRequest) -> SecurityResult {
    let effective: Option<&Vec<openapiv3::SecurityRequirement>> =
        op.security.as_ref().or(spec.security.as_ref());

    let Some(requirements) = effective else {
        return SecurityResult::Authorized;
    };

    if requirements.is_empty() {
        return SecurityResult::Authorized;
    }

    let mut first_scheme: Option<String> = None;
    let mut first_www_authenticate: Option<String> = None;
    let mut first_declared_scheme: Option<String> = None;

    for requirement in requirements {
        if requirement.is_empty() {
            return SecurityResult::Authorized;
        }
        let mut all_satisfied = true;
        for scheme_name in requirement.keys() {
            if first_declared_scheme.is_none() {
                first_declared_scheme = Some(scheme_name.clone());
            }
            let Some(scheme) = lookup_scheme(spec, scheme_name) else {
                all_satisfied = false;
                continue;
            };
            if first_scheme.is_none() {
                first_scheme = Some(scheme_name.clone());
                first_www_authenticate = www_authenticate_for(scheme, scheme_name);
            }
            if !is_scheme_satisfied(scheme, req) {
                all_satisfied = false;
            }
        }
        if all_satisfied {
            return SecurityResult::Authorized;
        }
    }

    SecurityResult::Unauthorized {
        scheme: first_scheme
            .or(first_declared_scheme)
            .unwrap_or_else(|| "unknown".to_string()),
        www_authenticate: first_www_authenticate,
    }
}

/// Looks a scheme up in `components.securitySchemes`, returning the resolved
/// concrete scheme when present.
///
/// Inline schemes resolve directly; entries shaped as
/// `{ "$ref": "#/components/securitySchemes/X" }` are followed to their target
/// entry (one hop). Cycles and external references resolve to `None` so the
/// caller treats them as unknown.
fn lookup_scheme<'a>(spec: &'a OpenAPI, name: &str) -> Option<&'a SecurityScheme> {
    let components = spec.components.as_ref()?;
    let entry = components.security_schemes.get(name)?;
    resolve_scheme_entry(components, entry)
}

/// Resolves a `ReferenceOr<SecurityScheme>` entry to its concrete
/// [`SecurityScheme`], following a single `#/components/securitySchemes/X`
/// hop when the entry is a reference.
fn resolve_scheme_entry<'a>(
    components: &'a openapiv3::Components,
    entry: &'a ReferenceOr<SecurityScheme>,
) -> Option<&'a SecurityScheme> {
    match entry {
        ReferenceOr::Item(s) => Some(s),
        ReferenceOr::Reference { reference } => {
            let target = reference.strip_prefix("#/components/securitySchemes/")?;
            let next = components.security_schemes.get(target)?;
            match next {
                ReferenceOr::Item(s) => Some(s),
                ReferenceOr::Reference { .. } => None,
            }
        }
    }
}

/// Returns the `WWW-Authenticate` header value to emit when `scheme` fails, or
/// `None` when the scheme doesn't define a registered RFC value (apiKey).
fn www_authenticate_for(scheme: &SecurityScheme, name: &str) -> Option<String> {
    match scheme {
        SecurityScheme::HTTP { scheme: kind, .. } => match kind.to_ascii_lowercase().as_str() {
            "bearer" => Some(format!("Bearer realm=\"{}\"", name)),
            "basic" => Some(format!("Basic realm=\"{}\"", name)),
            other => Some(format!("{} realm=\"{}\"", other, name)),
        },
        SecurityScheme::OAuth2 { .. } | SecurityScheme::OpenIDConnect { .. } => {
            Some(format!("Bearer realm=\"{}\"", name))
        }
        SecurityScheme::APIKey { .. } => None,
    }
}

/// Tests whether a single scheme is satisfied by the incoming request.
///
/// **Banner: chasm is a MOCK server and does NOT authenticate.** Every check
/// in this function is presence-only: any non-empty header, query parameter,
/// or cookie value satisfies the scheme. Bearer / Basic checks confirm the
/// scheme prefix on the `Authorization` header but never inspect the token
/// payload, signature, or expiry. API key checks (`header`, `query`, `cookie`)
/// accept any non-empty string under the configured name. This is sufficient
/// for testing client wiring and surfacing 401s when a credential is missing
/// entirely; it is NOT sufficient as a real authorisation boundary.
fn is_scheme_satisfied(scheme: &SecurityScheme, req: &MockRequest) -> bool {
    match scheme {
        SecurityScheme::HTTP { scheme: kind, .. } => match kind.to_ascii_lowercase().as_str() {
            "bearer" => has_authorization_prefix(req, "bearer"),
            "basic" => has_authorization_prefix(req, "basic"),
            _ => find_header(req, "authorization").is_some(),
        },
        SecurityScheme::OAuth2 { .. } | SecurityScheme::OpenIDConnect { .. } => {
            has_authorization_prefix(req, "bearer")
        }
        SecurityScheme::APIKey { location, name, .. } => match location {
            APIKeyLocation::Header => find_header(req, name)
                .map(|v| !v.is_empty())
                .unwrap_or(false),
            APIKeyLocation::Query => req.query.get(name).map(|v| !v.is_empty()).unwrap_or(false),
            APIKeyLocation::Cookie => find_cookie(req, name)
                .map(|v| !v.is_empty())
                .unwrap_or(false),
        },
    }
}

/// Returns true when the request has an `Authorization` header beginning with
/// `scheme` (case-insensitive) followed by whitespace.
fn has_authorization_prefix(req: &MockRequest, scheme: &str) -> bool {
    let Some(value) = find_header(req, "authorization") else {
        return false;
    };
    let lower = value.trim().to_ascii_lowercase();
    let needle = format!("{} ", scheme);
    lower.starts_with(&needle) && value.trim().len() > needle.len()
}

/// Finds a header by case-insensitive name and returns its raw value.
fn find_header<'a>(req: &'a MockRequest, name: &str) -> Option<&'a str> {
    req.headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.as_str())
}

/// Finds a cookie by name in the `Cookie` header, performing a minimal parse of
/// the standard `name=value; name2=value2` syntax.
///
/// Surrounding double quotes are stripped and the value is percent-decoded so
/// that `api_key="hello%20world"` resolves to `hello world`. Decoding falls
/// back to the raw value when the bytes are not valid UTF-8.
fn find_cookie(req: &MockRequest, name: &str) -> Option<String> {
    let raw = find_header(req, "cookie")?;
    for pair in raw.split(';') {
        let pair = pair.trim();
        if let Some((k, v)) = pair.split_once('=') {
            if k.trim() == name {
                return Some(normalize_cookie_value(v.trim()));
            }
        }
    }
    None
}

/// Strips surrounding double quotes (if present) and percent-decodes the cookie
/// value, returning the raw input when decoding produces invalid UTF-8.
fn normalize_cookie_value(value: &str) -> String {
    let unquoted = if value.len() >= 2 && value.starts_with('"') && value.ends_with('"') {
        &value[1..value.len() - 1]
    } else {
        value
    };
    match percent_decode_str(unquoted).decode_utf8() {
        Ok(decoded) => decoded.into_owned(),
        Err(_) => unquoted.to_string(),
    }
}
