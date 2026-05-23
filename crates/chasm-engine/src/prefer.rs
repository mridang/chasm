//! RFC 7240 `Prefer`-header parser for chasm-specific directives.
//!
//! Recognises `code=`, `dynamic=`, `example=`, `seed=`, `validate=`, and
//! `security=`. Also parses the equivalent `__code` / `__dynamic` /
//! `__example` / `__seed` / `__validate` / `__security` query-parameter
//! form, with query values winning on conflict.

use std::collections::HashMap;

/// Parsed directives extracted from a request's `Prefer` header or query string.
///
/// Provides per-request control of mock behaviour. All fields are optional;
/// absence means "no override".
#[derive(Debug, Default, Clone)]
#[non_exhaustive]
pub struct PreferDirectives {
    /// Forced HTTP status code from `Prefer: code=` or query `__code`.
    pub code: Option<u16>,
    /// Named example selector from `Prefer: example=` or query `__example`.
    pub example: Option<String>,
    /// Forces dynamic generation when `Some(true)` and disables when `Some(false)`.
    pub dynamic: Option<bool>,
    /// Per-request seed for deterministic dynamic generation.
    pub seed: Option<u64>,
    /// Per-request override that skips request validation when `Some(false)`
    /// even when the server was started with `--errors`. `Some(true)` forces
    /// validation on (no-op when already on). Provides a request-header
    /// bypass without polluting the HTTP namespace.
    pub validate: Option<bool>,
    /// Per-request override that skips the security scheme check when
    /// `Some(false)`. `Some(true)` forces the check on (default).
    pub security: Option<bool>,
}

impl PreferDirectives {
    /// Parses a raw `Prefer` header value into a [`PreferDirectives`].
    ///
    /// The header is a comma-separated list of `name=value` tokens. Unknown tokens
    /// are silently ignored. Token names are matched case-INsensitively per
    /// RFC 7240 §2. Per RFC 7240 §2, tokens may carry parameters separated by
    /// `;` (e.g. `respond-async; wait=10`); chasm's documented directives do
    /// not define any parameters, so anything after a `;` in a value is
    /// silently dropped.
    pub fn from_prefer_header(value: &str) -> Self {
        let mut out = Self::default();
        for part in value.split(',') {
            let part = part.trim();
            if let Some(rest) = strip_prefix_ignore_ascii_case(part, "code=") {
                if let Ok(n) = strip_token_parameters(rest).parse::<u16>() {
                    out.code = Some(n);
                }
            } else if let Some(rest) = strip_prefix_ignore_ascii_case(part, "example=") {
                out.example = Some(strip_token_parameters(rest).to_string());
            } else if let Some(rest) = strip_prefix_ignore_ascii_case(part, "dynamic=") {
                out.dynamic = parse_bool(strip_token_parameters(rest));
            } else if let Some(rest) = strip_prefix_ignore_ascii_case(part, "seed=") {
                if let Ok(n) = strip_token_parameters(rest).parse::<u64>() {
                    out.seed = Some(n);
                }
            } else if let Some(rest) = strip_prefix_ignore_ascii_case(part, "validate=") {
                out.validate = parse_bool(strip_token_parameters(rest));
            } else if let Some(rest) = strip_prefix_ignore_ascii_case(part, "security=") {
                out.security = parse_bool(strip_token_parameters(rest));
            }
        }
        out
    }

    /// Parses the `__code`, `__example`, `__dynamic`, `__seed`, `__validate`,
    /// and `__security` query parameters.
    ///
    /// Returns a fresh [`PreferDirectives`] populated only from the query string.
    pub fn from_query(query: &HashMap<String, String>) -> Self {
        let mut out = Self::default();
        if let Some(v) = query.get("__code") {
            if let Ok(n) = v.parse::<u16>() {
                out.code = Some(n);
            }
        }
        if let Some(v) = query.get("__example") {
            out.example = Some(v.clone());
        }
        if let Some(v) = query.get("__dynamic") {
            out.dynamic = parse_bool(v);
        }
        if let Some(v) = query.get("__seed") {
            if let Ok(n) = v.parse::<u64>() {
                out.seed = Some(n);
            }
        }
        if let Some(v) = query.get("__validate") {
            out.validate = parse_bool(v);
        }
        if let Some(v) = query.get("__security") {
            out.security = parse_bool(v);
        }
        out
    }

    /// Combines a header-derived set with a query-derived set, with the query winning on conflict.
    ///
    /// Query takes precedence because query overrides are the most explicit
    /// signal a caller can pass.
    pub fn merge_query_over_header(header: Self, query: Self) -> Self {
        Self {
            code: query.code.or(header.code),
            example: query.example.or(header.example),
            dynamic: query.dynamic.or(header.dynamic),
            seed: query.seed.or(header.seed),
            validate: query.validate.or(header.validate),
            security: query.security.or(header.security),
        }
    }

    /// Convenience constructor: reads the `Prefer` header from a header map (any case) and the query.
    pub fn from_request(
        headers: &HashMap<String, String>,
        query: &HashMap<String, String>,
    ) -> Self {
        let header_directives = headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("prefer"))
            .map(|(_, v)| Self::from_prefer_header(v))
            .unwrap_or_default();
        let query_directives = Self::from_query(query);
        Self::merge_query_over_header(header_directives, query_directives)
    }
}

/// Strips `prefix` from the front of `s` ignoring ASCII case, returning the
/// remainder when matched. Used to satisfy RFC 7240 §2 which requires
/// `Prefer` token names to be matched case-insensitively.
fn strip_prefix_ignore_ascii_case<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    if s.len() >= prefix.len() && s[..prefix.len()].eq_ignore_ascii_case(prefix) {
        Some(&s[prefix.len()..])
    } else {
        None
    }
}

/// Strips RFC 7240 §2 token parameters from a directive value. A `Prefer`
/// token value may be followed by `; name=value` parameter pairs; chasm
/// silently ignores all such parameters, so a value like `dynamic=true; foo=bar`
/// parses `true` rather than failing.
fn strip_token_parameters(value: &str) -> &str {
    value.split(';').next().unwrap_or(value).trim()
}

/// Parses a textual boolean (`true`/`false`/`1`/`0`, case-insensitive) into `Option<bool>`.
fn parse_bool(s: &str) -> Option<bool> {
    match s.to_ascii_lowercase().as_str() {
        "true" | "1" | "yes" => Some(true),
        "false" | "0" | "no" => Some(false),
        _ => None,
    }
}
