//! Tests for `prefer.rs`.

use chasm_engine::PreferDirectives;
use std::collections::HashMap;

/// Builds an empty header map for tests that exercise the query path.
fn empty_headers() -> HashMap<String, String> {
    HashMap::new()
}

/// Builds an empty query map for tests that exercise the header path.
fn empty_query() -> HashMap<String, String> {
    HashMap::new()
}

/// `from_prefer_header` parses a `code=<n>` token into the numeric override.
#[test]
fn test_parses_code_directive_from_header() {
    let directives = PreferDirectives::from_prefer_header("code=201");

    assert_eq!(directives.code, Some(201));
}

/// `from_prefer_header` parses an `example=<name>` token into the example key.
#[test]
fn test_parses_example_directive_from_header() {
    let directives = PreferDirectives::from_prefer_header("example=foo");

    assert_eq!(directives.example.as_deref(), Some("foo"));
}

/// `from_prefer_header` parses `dynamic=true` into the dynamic override.
#[test]
fn test_parses_dynamic_true_from_header() {
    let directives = PreferDirectives::from_prefer_header("dynamic=true");

    assert_eq!(directives.dynamic, Some(true));
}

/// `from_prefer_header` parses `seed=<n>` into the seed override.
#[test]
fn test_parses_seed_directive_from_header() {
    let directives = PreferDirectives::from_prefer_header("seed=42");

    assert_eq!(directives.seed, Some(42));
}

/// An unknown token in a Prefer header does not clobber the recognised `code`
/// token that comes alongside it.
#[test]
fn test_unknown_token_does_not_clobber_code() {
    let directives = PreferDirectives::from_prefer_header("code=200, unknown=value");

    assert_eq!(directives.code, Some(200));
}

/// An unknown token in a Prefer header does not spuriously set `example` when
/// no `example` token appears.
#[test]
fn test_unknown_token_does_not_clobber_example() {
    let directives = PreferDirectives::from_prefer_header("code=200, unknown=value");

    assert!(directives.example.is_none());
}

/// `from_prefer_header` parses a `code` token alongside other directives.
#[test]
fn test_parses_code_alongside_other_directives() {
    let directives = PreferDirectives::from_prefer_header("code=200, example=bar");

    assert_eq!(directives.code, Some(200));
}

/// `from_prefer_header` parses an `example` token alongside other directives.
#[test]
fn test_parses_example_alongside_other_directives() {
    let directives = PreferDirectives::from_prefer_header("code=200, example=bar");

    assert_eq!(directives.example.as_deref(), Some("bar"));
}

/// `from_query` reads `__code` from the query map.
#[test]
fn test_parses_code_from_query() {
    let mut query = empty_query();
    query.insert("__code".to_string(), "404".to_string());

    let directives = PreferDirectives::from_query(&query);

    assert_eq!(directives.code, Some(404));
}

/// `from_query` reads `__example` from the query map.
#[test]
fn test_parses_example_from_query() {
    let mut query = empty_query();
    query.insert("__example".to_string(), "primary".to_string());

    let directives = PreferDirectives::from_query(&query);

    assert_eq!(directives.example.as_deref(), Some("primary"));
}

/// `from_query` reads `__dynamic` from the query map.
#[test]
fn test_parses_dynamic_from_query() {
    let mut query = empty_query();
    query.insert("__dynamic".to_string(), "true".to_string());

    let directives = PreferDirectives::from_query(&query);

    assert_eq!(directives.dynamic, Some(true));
}

/// `from_query` reads `__seed` from the query map.
#[test]
fn test_parses_seed_from_query() {
    let mut query = empty_query();
    query.insert("__seed".to_string(), "7".to_string());

    let directives = PreferDirectives::from_query(&query);

    assert_eq!(directives.seed, Some(7));
}

/// `merge_query_over_header` lets the query override the header on `code`.
#[test]
fn test_query_overrides_header_on_code() {
    let header = PreferDirectives::from_prefer_header("code=201");
    let mut query_map = empty_query();
    query_map.insert("__code".to_string(), "200".to_string());
    let query = PreferDirectives::from_query(&query_map);

    let merged = PreferDirectives::merge_query_over_header(header, query);

    assert_eq!(merged.code, Some(200));
}

/// `merge_query_over_header` preserves header values when the query is silent.
#[test]
fn test_header_value_kept_when_query_silent() {
    let header = PreferDirectives::from_prefer_header("example=foo");
    let query = PreferDirectives::from_query(&empty_query());

    let merged = PreferDirectives::merge_query_over_header(header, query);

    assert_eq!(merged.example.as_deref(), Some("foo"));
}

/// `from_request` performs a case-insensitive lookup of the `Prefer` header.
#[test]
fn test_request_lookup_is_case_insensitive_on_header_name() {
    let mut headers = empty_headers();
    headers.insert("prefer".to_string(), "code=418".to_string());

    let directives = PreferDirectives::from_request(&headers, &empty_query());

    assert_eq!(directives.code, Some(418));
}

/// `from_prefer_header` parses `validate=false` into the validation bypass override.
#[test]
fn test_parses_validate_false_from_header() {
    let directives = PreferDirectives::from_prefer_header("validate=false");

    assert_eq!(directives.validate, Some(false));
}

/// `from_prefer_header` parses `security=false` into the security bypass override.
#[test]
fn test_parses_security_false_from_header() {
    let directives = PreferDirectives::from_prefer_header("security=false");

    assert_eq!(directives.security, Some(false));
}

/// RFC 7240 §2 token parameters (`; name=value` suffix) are silently dropped.
/// `dynamic=true; foo=bar` parses as `dynamic=true`.
#[test]
fn test_token_parameters_after_semicolon_are_ignored() {
    let directives = PreferDirectives::from_prefer_header("dynamic=true; foo=bar");

    assert_eq!(directives.dynamic, Some(true));
}

/// Token parameters following a numeric directive value are tolerated:
/// `code=201; charset=utf-8` parses the leading `201`.
#[test]
fn test_token_parameters_tolerated_on_numeric_directive() {
    let directives = PreferDirectives::from_prefer_header("code=201; foo=bar");

    assert_eq!(directives.code, Some(201));
}

/// `Prefer` token names are matched case-insensitively per RFC 7240 §2.
#[test]
fn test_prefer_token_case_insensitive() {
    let headers = HashMap::from([("Prefer".to_string(), "CODE=200, Example=foo".to_string())]);
    let query = HashMap::new();

    let directives = PreferDirectives::from_request(&headers, &query);

    assert_eq!(directives.code, Some(200));
    assert_eq!(directives.example, Some("foo".to_string()));
}
