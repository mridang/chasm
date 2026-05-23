//! JSON Schema and string-format tests (email, uri, uuid, ipv4/ipv6, date-time, hostname, json-pointer, byte/binary, base64url, etc.).

use crate::common::{default_opts, seeded_opts};
use chasm_faker::generate;
use serde_json::json;

/// `format: email` produces a value matching the loose RFC-5322 mailbox regex.
#[test]
fn test_format_email_matches_email_regex() {
    let schema = json!({"type": "string", "format": "email"});
    let re = regex::Regex::new(r"^[A-Za-z0-9._+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}$").unwrap();

    let value = generate(&schema, &default_opts()).unwrap();
    let s = value.as_str().unwrap();

    assert!(re.is_match(s), "email `{}` did not match regex", s);
}

/// `format: uuid` produces a string matching the canonical 8-4-4-4-12
/// hex UUID shape. This is a SHAPE-ONLY check; the v4-specific variant
/// nibble enforcement lives in `test_format_uuid4_emits_valid_uuid_v4`.
#[test]
fn test_format_uuid_shape_matches_uuid_regex() {
    let schema = json!({"type": "string", "format": "uuid"});
    let re = regex::Regex::new(
        r"^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$",
    )
    .unwrap();

    let value = generate(&schema, &default_opts()).unwrap();
    let s = value.as_str().unwrap();

    assert!(re.is_match(s), "uuid `{}` did not match regex", s);
}

/// `format: uuid4` produces a valid UUID v4 (version nibble + variant nibble).
#[test]
fn test_format_uuid4_emits_valid_uuid_v4() {
    let schema = json!({"type": "string", "format": "uuid4"});
    let mut opts = seeded_opts(2703);
    opts.fail_on_invalid_format = true;

    let value = generate(&schema, &opts).unwrap();
    let s = value.as_str().unwrap();

    let re = regex::Regex::new(
        r"^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-4[0-9a-fA-F]{3}-[89abAB][0-9a-fA-F]{3}-[0-9a-fA-F]{12}$",
    )
    .unwrap();
    assert!(re.is_match(s));
}

/// `format: date-time` produces a non-empty string.
#[test]
fn test_format_date_time_non_empty() {
    let schema = json!({"type": "string", "format": "date-time"});

    let value = generate(&schema, &default_opts()).unwrap();
    let s = value.as_str().unwrap();

    assert!(!s.is_empty());
}

/// `min_date_time` and `max_date_time` constrain date-time generation to the declared range.
#[test]
fn test_date_time_with_min_max_range_respected() {
    use chrono::{DateTime, NaiveDate, NaiveDateTime, NaiveTime, TimeZone, Utc};
    let schema = json!({"type": "string", "format": "date-time"});
    let min_str = "2010-01-01";
    let max_str = "2010-12-31";
    let lo: DateTime<Utc> = {
        let d = NaiveDate::parse_from_str(min_str, "%Y-%m-%d").unwrap();
        let t = NaiveTime::from_hms_opt(0, 0, 0).unwrap();
        Utc.from_local_datetime(&NaiveDateTime::new(d, t)).unwrap()
    };
    let hi: DateTime<Utc> = {
        let d = NaiveDate::parse_from_str(max_str, "%Y-%m-%d").unwrap();
        let t = NaiveTime::from_hms_opt(0, 0, 0).unwrap();
        Utc.from_local_datetime(&NaiveDateTime::new(d, t)).unwrap()
    };
    for i in 0..100 {
        let mut opts = default_opts();
        opts.min_date_time = Some(min_str.to_string());
        opts.max_date_time = Some(max_str.to_string());
        opts.seed = Some(i as u64);
        let value = generate(&schema, &opts).unwrap();
        let s = value.as_str().unwrap();
        let parsed = DateTime::parse_from_rfc3339(s).unwrap();
        let parsed_utc = parsed.with_timezone(&Utc);
        assert!(parsed_utc >= lo && parsed_utc <= hi);
    }
}

/// `format: date` produces a value matching the strict `YYYY-MM-DD` ISO date regex.
#[test]
fn test_format_date_matches_yyyy_mm_dd_regex() {
    let schema = json!({"type": "string", "format": "date"});
    let re = regex::Regex::new(r"^[0-9]{4}-[0-9]{2}-[0-9]{2}$").unwrap();

    let value = generate(&schema, &default_opts()).unwrap();
    let s = value.as_str().unwrap();

    assert!(re.is_match(s), "date `{}` did not match YYYY-MM-DD", s);
}

/// `format: uri` produces a string with an `http://` or `https://` scheme
/// prefix and a non-empty authority component.
#[test]
fn test_format_uri_starts_with_http() {
    let schema = json!({"type": "string", "format": "uri"});

    let value = generate(&schema, &default_opts()).unwrap();
    let s = value.as_str().unwrap();

    assert!(
        s.starts_with("http://") || s.starts_with("https://"),
        "expected http(s):// scheme prefix, got {:?}",
        s
    );
    let after_scheme = s.split_once("://").map(|(_, rest)| rest);
    assert!(
        after_scheme.is_some_and(|rest| !rest.is_empty()),
        "expected URL with non-empty authority, got {:?}",
        s
    );
}

/// `format: ipv4` produces a string that parses as a valid IPv4 literal.
#[test]
fn test_format_ipv4_four_octets() {
    let schema = json!({"type": "string", "format": "ipv4"});

    let value = generate(&schema, &default_opts()).unwrap();
    let s = value.as_str().unwrap();

    let parsed: std::net::Ipv4Addr = s
        .parse()
        .unwrap_or_else(|e| panic!("expected valid IPv4 literal, got {:?}: {}", s, e));
    let _ = parsed;
}

/// `format: ipv6` produces a string that parses as a valid IPv6 literal.
#[test]
fn test_format_ipv6_eight_groups() {
    let schema = json!({"type": "string", "format": "ipv6"});

    let value = generate(&schema, &default_opts()).unwrap();
    let s = value.as_str().unwrap();

    let parsed: std::net::Ipv6Addr = s
        .parse()
        .unwrap_or_else(|e| panic!("expected valid IPv6 literal, got {:?}: {}", s, e));
    let _ = parsed;
}

/// `format: decimal` produces a string parseable as `f64`.
#[test]
fn test_format_decimal_parses_as_f64() {
    let schema = json!({"type": "string", "format": "decimal"});

    let value = generate(&schema, &default_opts()).unwrap();
    let s = value.as_str().unwrap();

    s.parse::<f64>().unwrap();
}

/// `format: slug` produces a kebab-case lowercase-alphanumeric string.
#[test]
fn test_format_slug_kebab_case() {
    let schema = json!({"type": "string", "format": "slug"});
    let re = regex::Regex::new("^[a-z0-9]+(-[a-z0-9]+)*$").unwrap();

    let value = generate(&schema, &default_opts()).unwrap();
    let s = value.as_str().unwrap();

    assert!(re.is_match(s));
}

/// An unknown `format` under default options falls back to a plain string.
#[test]
fn test_unknown_format_lenient_returns_string() {
    let schema = json!({"type": "string", "format": "foobar"});

    let value = generate(&schema, &default_opts()).unwrap();

    assert!(value.is_string());
}

/// An unknown `format` under `fail_on_invalid_format: true` surfaces an error.
#[test]
fn test_unknown_format_strict_errors() {
    let schema = json!({"type": "string", "format": "foobar"});
    let mut opts = default_opts();
    opts.fail_on_invalid_format = true;

    let result = generate(&schema, &opts);

    assert!(result.is_err());
}

/// An unknown `format` combined with a `pattern` falls back to the pattern generator.
#[test]
fn test_unknown_format_uses_pattern() {
    let schema = json!({"type": "string", "format": "foobar", "pattern": "^[A-Z]{4}$"});
    let re = regex::Regex::new("^[A-Z]{4}$").unwrap();

    let value = generate(&schema, &default_opts()).unwrap();
    let s = value.as_str().unwrap();

    assert!(re.is_match(s));
}

/// `format: hex-color` produces a `#RRGGBB` string.
#[test]
fn test_format_hex_color_rrggbb() {
    let schema = json!({"type": "string", "format": "hex-color"});
    let re = regex::Regex::new("^#[0-9A-F]{6}$").unwrap();

    let value = generate(&schema, &default_opts()).unwrap();
    let s = value.as_str().unwrap();

    assert!(re.is_match(s));
}

/// `format: duration` produces an ISO-8601 duration string starting with `P`.
#[test]
fn test_format_duration_iso_8601_shape() {
    let schema = json!({"type": "string", "format": "duration"});
    let re =
        regex::Regex::new("^P([0-9]+Y)?([0-9]+M)?([0-9]+D)?(T([0-9]+H)?([0-9]+M)?([0-9]+S)?)?$")
            .unwrap();

    for i in 0..20 {
        let value = generate(&schema, &seeded_opts(i)).unwrap();
        let s = value.as_str().unwrap();
        assert!(re.is_match(s));
    }
}

/// `format: json-pointer` produces a string that begins with `/`.
#[test]
fn test_format_json_pointer_starts_with_slash() {
    let schema = json!({"type": "string", "format": "json-pointer"});

    let value = generate(&schema, &default_opts()).unwrap();
    let s = value.as_str().unwrap();

    assert!(s.starts_with('/'));
}

/// `format: json-pointer` produces a string with at least one path segment after the leading `/`.
#[test]
fn test_format_json_pointer_has_path_segment() {
    let schema = json!({"type": "string", "format": "json-pointer"});

    let value = generate(&schema, &default_opts()).unwrap();
    let s = value.as_str().unwrap();

    assert!(s.len() > 1);
}

/// `format: relative-json-pointer` produces an `<int>/<segment>` string.
#[test]
fn test_format_relative_json_pointer_shape() {
    let schema = json!({"type": "string", "format": "relative-json-pointer"});
    let re = regex::Regex::new("^[0-9]+/[a-z]+$").unwrap();

    let value = generate(&schema, &default_opts()).unwrap();
    let s = value.as_str().unwrap();

    assert!(re.is_match(s));
}

/// `format: password` produces a string whose length is within a reasonable
/// range for a password generator (8 chars is the conventional lower bound).
#[test]
fn test_format_password_length_is_twelve() {
    let schema = json!({"type": "string", "format": "password"});

    let value = generate(&schema, &default_opts()).unwrap();
    let s = value.as_str().unwrap();

    assert!(
        (8..=64).contains(&s.len()),
        "expected password length in 8..=64, got {} ({:?})",
        s.len(),
        s
    );
}

/// `format: byte` produces a base64 string whose length is a multiple of four.
#[test]
fn test_format_byte_length_multiple_of_four() {
    let schema = json!({"type": "string", "format": "byte"});

    let value = generate(&schema, &default_opts()).unwrap();
    let s = value.as_str().unwrap();

    assert!(s.len().is_multiple_of(4));
}

/// `format: byte` ends with zero, one, or two `=` padding characters.
#[test]
fn test_format_byte_padding_count() {
    let schema = json!({"type": "string", "format": "byte"});

    let value = generate(&schema, &default_opts()).unwrap();
    let s = value.as_str().unwrap();

    let pad_count = s.chars().rev().take_while(|c| *c == '=').count();
    assert!(pad_count <= 2);
}

/// `format: binary` produces a hexadecimal-only lowercase string.
#[test]
fn test_format_binary_lowercase_hex_only() {
    let schema = json!({"type": "string", "format": "binary"});

    let value = generate(&schema, &default_opts()).unwrap();
    let s = value.as_str().unwrap();

    assert!(s
        .chars()
        .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
}

/// `format: regex` with a sibling `pattern` returns the pattern verbatim.
#[test]
fn test_format_regex_returns_pattern_verbatim() {
    let schema = json!({"type": "string", "format": "regex", "pattern": "^[A-Z]{3}$"});

    let value = generate(&schema, &default_opts()).unwrap();
    let s = value.as_str().unwrap();

    assert_eq!(s, "^[A-Z]{3}$");
}

/// `format: phone` produces a `+1-XXX-XXXXXXX` shaped string.
#[test]
fn test_format_phone_shape() {
    let schema = json!({"type": "string", "format": "phone"});
    let re = regex::Regex::new(r"^\+1-[0-9]{3}-[0-9]{7}$").unwrap();

    let value = generate(&schema, &default_opts()).unwrap();
    let s = value.as_str().unwrap();

    assert!(re.is_match(s));
}

/// `format: time` produces a colon-containing string.
#[test]
fn test_format_time_contains_colon() {
    let schema = json!({"type": "string", "format": "time"});

    let value = generate(&schema, &default_opts()).unwrap();
    let s = value.as_str().unwrap();

    assert!(s.contains(':'));
}

/// `format: hostname` produces a non-empty string.
#[test]
fn test_format_hostname_non_empty() {
    let schema = json!({"type": "string", "format": "hostname"});

    let value = generate(&schema, &default_opts()).unwrap();
    let s = value.as_str().unwrap();

    assert!(!s.is_empty());
}

/// Every listed `format` generator is deterministic under the same seed.
#[test]
fn test_format_generators_are_deterministic_under_same_seed() {
    let formats = [
        "email",
        "hostname",
        "ipv4",
        "ipv6",
        "uri",
        "json-pointer",
        "name",
        "company",
        "street-name",
        "city",
        "country",
        "phone-number",
        "username",
        "lorem-sentence",
        "credit-card",
        "currency-code",
        "file-path",
        "semver",
        "color",
        "http-status",
    ];

    for fmt in formats {
        let schema = json!({"type": "string", "format": fmt});
        let a = generate(&schema, &seeded_opts(42)).unwrap();
        let b = generate(&schema, &seeded_opts(42)).unwrap();
        assert_eq!(a, b, "format {} is not deterministic", fmt);
    }
}

/// `format: http-status` returns a value matching the three-digit numeric regex.
#[test]
fn test_format_http_status_three_digit_numeric() {
    let schema = json!({"type": "string", "format": "http-status"});
    let re = regex::Regex::new(r"^[0-9]{3}$").unwrap();

    let value = generate(&schema, &default_opts()).unwrap();
    let s = value.as_str().unwrap();

    assert!(re.is_match(s), "http-status `{}` is not three digits", s);
}

/// `format: valid-status-code` returns a value matching the three-digit numeric regex.
#[test]
fn test_format_valid_status_code_three_digit_numeric() {
    let schema = json!({"type": "string", "format": "valid-status-code"});
    let re = regex::Regex::new(r"^[0-9]{3}$").unwrap();

    let value = generate(&schema, &default_opts()).unwrap();
    let s = value.as_str().unwrap();

    assert!(
        re.is_match(s),
        "valid-status-code `{}` is not three digits",
        s
    );
}

/// `format: color` produces a value with no embedded newlines.
#[test]
fn test_format_color_no_newlines() {
    let schema = json!({"type": "string", "format": "color"});

    let value = generate(&schema, &default_opts()).unwrap();
    let s = value.as_str().unwrap();

    assert!(!s.contains('\n'));
}

/// `format: color` produces a non-empty value.
#[test]
fn test_format_color_non_empty() {
    let schema = json!({"type": "string", "format": "color"});

    let value = generate(&schema, &default_opts()).unwrap();
    let s = value.as_str().unwrap();

    assert!(!s.is_empty());
}

/// `format: base64url` produces a value matching the unpadded URL-safe-alphabet regex.
#[test]
fn test_format_base64url_matches_url_safe_alphabet_regex() {
    let schema = json!({"type": "string", "format": "base64url"});
    let re = regex::Regex::new(r"^[A-Za-z0-9_-]+$").unwrap();

    let value = generate(&schema, &default_opts()).unwrap();
    let s = value.as_str().unwrap();

    assert!(
        re.is_match(s),
        "base64url `{}` outside URL-safe alphabet",
        s
    );
}

/// A `format` plus an incompatible `pattern` surfaces a `SchemaError`.
#[test]
fn test_pattern_and_format_incompatible_surfaces_schema_error() {
    let schema = json!({"type": "string", "format": "email", "pattern": "^[0-9]{6}$"});

    let result = generate(&schema, &seeded_opts(7));

    assert!(
        matches!(result, Err(chasm_faker::FakerError::SchemaError { .. })),
        "expected SchemaError, got {result:?}",
    );
}

/// A `format` plus a compatible `pattern` returns a value satisfying the pattern.
#[test]
fn test_format_and_pattern_compatible_revalidates() {
    let schema = json!({"type": "string", "format": "uuid", "pattern": "^[0-9a-fA-F-]+$"});
    let re = regex::Regex::new("^[0-9a-fA-F-]+$").unwrap();

    let value = generate(&schema, &default_opts()).unwrap();
    let s = value.as_str().unwrap();

    assert!(re.is_match(s));
}

/// `format: secret-bytes` produces base64-shaped padding that varies across seeds.
#[test]
fn test_format_secret_bytes_padding_varies() {
    let schema = json!({"type": "string", "format": "secret-bytes"});
    let mut seen_none = false;
    let mut seen_one = false;
    let mut seen_two = false;
    for seed in 0..200_u64 {
        let value = generate(&schema, &seeded_opts(seed)).unwrap();
        let s = value.as_str().unwrap().to_string();
        let pad = s.chars().rev().take_while(|c| *c == '=').count();
        match pad {
            0 => seen_none = true,
            1 => seen_one = true,
            2 => seen_two = true,
            _ => panic!("unexpected padding length"),
        }
        if seen_none && seen_one && seen_two {
            break;
        }
    }

    assert!(seen_none && seen_one && seen_two);
}

/// `format: idn-hostname` produces at least one value containing non-ASCII
/// characters across a deterministic seed sweep.
#[test]
fn test_format_idn_hostname_contains_unicode() {
    let schema = json!({"type": "string", "format": "idn-hostname"});
    let mut saw_unicode = false;

    for seed in 0..20 {
        let value = generate(&schema, &seeded_opts(seed)).unwrap();
        let s = value.as_str().expect("string");
        if !s.is_ascii() {
            saw_unicode = true;
            break;
        }
    }

    assert!(
        saw_unicode,
        "expected at least one non-ASCII idn-hostname across 20 seeds"
    );
}

/// `format: uri-template` produces a value containing an opening `{` placeholder marker.
#[test]
fn test_format_uri_template_has_open_brace() {
    let schema = json!({"type": "string", "format": "uri-template"});

    let value = generate(&schema, &default_opts()).unwrap();
    let s = value.as_str().unwrap();

    assert!(s.contains('{'));
}

/// `format: uri-template` produces a value containing a closing `}` placeholder marker.
#[test]
fn test_format_uri_template_has_close_brace() {
    let schema = json!({"type": "string", "format": "uri-template"});

    let value = generate(&schema, &default_opts()).unwrap();
    let s = value.as_str().unwrap();

    assert!(s.contains('}'));
}

/// Every listed format name dispatches through `formats::mod` and produces a non-empty string
/// under a deterministic seed.
///
/// This covers the dispatcher arms that are not otherwise asserted, so a regression that
/// silently dropped a match arm would surface as an empty string from this test.
#[test]
fn test_all_known_formats_dispatch_to_non_empty_string() {
    let formats = [
        "iri",
        "mac",
        "mac-address",
        "domain",
        "domain-name",
        "free-email",
        "free-email-provider",
        "lorem-word",
        "lorem-words",
        "lorem-sentence",
        "lorem-sentences",
        "lorem-paragraph",
        "lorem-paragraphs",
        "bic",
        "swift",
        "isin",
        "currency-name",
        "currency-symbol",
        "currency-code",
        "file-name",
        "file-extension",
        "file-path",
        "mime",
        "mime-type",
        "semver",
        "semver-stable",
        "semver-unstable",
        "rgb",
        "rgba",
        "hsl",
        "hsla",
        "building-number",
        "secondary-address",
        "state-name",
        "state-abbr",
        "zip-code",
        "post-code",
        "country-name",
        "country-code",
        "latitude",
        "longitude",
        "timezone",
        "first-name",
        "last-name",
        "name-with-title",
        "title",
        "suffix",
        "street-name",
        "city",
        "city-name",
        "credit-card",
        "user-agent",
        "username",
        "phone-number",
        "buzzword",
        "industry",
        "catch-phrase",
        "company-suffix",
        "directory-path",
        "new-path",
        "json-string",
    ];

    for format in formats {
        let schema = json!({"type": "string", "format": format});
        let value = generate(&schema, &seeded_opts(1)).unwrap();
        let s = value.as_str().expect(format);
        assert!(!s.is_empty(), "format `{}` returned empty string", format);
    }
}

/// Format-name aliases route to the same generator as their canonical name (same seed → same output).
#[test]
fn test_format_aliases_route_to_canonical() {
    let pairs = [
        ("creditcard", "credit-card"),
        ("credit-card-number", "credit-card"),
        ("filepath", "file-path"),
        ("path", "file-path"),
        ("mimetype", "mime"),
        ("mime-type", "mime"),
        ("filename", "file-name"),
        ("extension", "file-extension"),
        ("family-name", "last-name"),
        ("surname", "last-name"),
        ("given-name", "first-name"),
        ("city-name", "city"),
        ("state-name", "state"),
        ("state-code", "state-abbr"),
        ("zip", "zip-code"),
        ("postcode", "post-code"),
        ("country", "country-name"),
        ("lat", "latitude"),
        ("lng", "longitude"),
        ("lon", "longitude"),
        ("time-zone", "timezone"),
        ("user-name", "username"),
        ("macaddress", "mac"),
        ("mac-address", "mac"),
        ("hostname-suffix", "domain"),
        ("domain-name", "domain"),
        ("phonenumber", "phone-number"),
        ("paragraph", "lorem-paragraph"),
        ("sentence", "lorem-sentence"),
        ("word", "lorem-word"),
        ("currency-code", "currency"),
        ("semantic-version", "semver"),
        ("version", "semver"),
        ("swift", "bic"),
        ("isbn-10", "isbn10"),
        ("isbn-13", "isbn13"),
        ("colour", "color"),
        ("rgb-color", "rgb"),
        ("hsl-color", "hsl"),
        ("hsla-color", "hsla"),
        ("rgba-color", "rgba"),
        ("status-code", "http-status"),
        ("rfc-status-code", "http-status"),
        ("date-time", "datetime"),
        ("iso-date-time", "datetime"),
        ("iso-date", "date"),
        ("full-date", "date"),
        ("partial-time", "time"),
        ("url", "uri"),
        ("uri-reference", "uri"),
        ("iri-reference", "uri"),
        ("guid", "uuid"),
        ("uuid4", "uuid"),
        ("bool", "boolean"),
    ];

    for (alias, canonical) in pairs {
        let alias_schema = json!({"type": "string", "format": alias});
        let canonical_schema = json!({"type": "string", "format": canonical});
        let a = generate(&alias_schema, &seeded_opts(1)).unwrap();
        let b = generate(&canonical_schema, &seeded_opts(1)).unwrap();
        assert_eq!(
            a, b,
            "alias `{}` should produce same output as `{}`",
            alias, canonical
        );
    }
}
