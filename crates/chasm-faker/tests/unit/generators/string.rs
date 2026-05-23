//! Tests for `generators/string.rs`.

use crate::common::{default_opts, seeded_opts};
use chasm_faker::{generate, FakerError};
use serde_json::json;

/// `generate()` returns a string when the schema declares `type: "string"`.
#[test]
fn test_returns_string_for_string_type() {
    let schema = json!({"type": "string"});

    let value = generate(&schema, &default_opts()).unwrap();

    assert!(value.is_string());
}

/// A `minLength` constraint produces a string of at least that length.
#[test]
fn test_min_length_constraint_respected() {
    let schema = json!({"type": "string", "minLength": 10});

    let value = generate(&schema, &default_opts()).unwrap();
    let s = value.as_str().unwrap();

    assert!(s.len() >= 10);
}

/// A `maxLength` constraint produces a string no longer than that length.
#[test]
fn test_max_length_constraint_respected() {
    let schema = json!({"type": "string", "maxLength": 5});

    let value = generate(&schema, &default_opts()).unwrap();
    let s = value.as_str().unwrap();

    assert!(s.len() <= 5);
}

/// `minLength: 0` and `maxLength: 0` together produce an empty string.
#[test]
fn test_min_and_max_length_zero_produce_empty_string() {
    let schema = json!({"type": "string", "minLength": 0, "maxLength": 0});

    let value = generate(&schema, &default_opts()).unwrap();
    let s = value.as_str().unwrap();

    assert_eq!(s, "");
}

/// A digit-only `pattern` produces a string of digits.
#[test]
fn test_pattern_digits_yields_digit_string() {
    let schema = json!({"type": "string", "pattern": "^[0-9]+$"});

    let value = generate(&schema, &default_opts()).unwrap();
    let s = value.as_str().unwrap();

    assert!(!s.is_empty() && s.chars().all(|c| c.is_ascii_digit()));
}

/// A lowercase `pattern` produces only lowercase letters.
#[test]
fn test_pattern_lowercase_yields_lowercase_only() {
    let schema = json!({"type": "string", "pattern": "^[a-z]+$"});

    let value = generate(&schema, &default_opts()).unwrap();
    let s = value.as_str().unwrap();

    assert!(!s.is_empty() && s.chars().all(|c| c.is_ascii_lowercase()));
}

/// A complex `pattern` produces a string of the declared fixed length.
#[test]
fn test_pattern_fixed_length_two_plus_three() {
    let schema = json!({"type": "string", "pattern": "^[a-z]{2}[0-9]{3}$"});

    let value = generate(&schema, &default_opts()).unwrap();
    let s = value.as_str().unwrap();

    assert_eq!(s.len(), 5);
}

/// A `pattern` combined with `minLength` produces a string of at least that length.
#[test]
fn test_pattern_with_min_length_constraint_respected() {
    let schema = json!({"type": "string", "pattern": ".+", "minLength": 21});

    let value = generate(&schema, &default_opts()).unwrap();
    let s = value.as_str().unwrap();

    assert!(s.len() >= 21);
}

/// A `pattern` combined with `maxLength` produces a string no longer than that length.
#[test]
fn test_pattern_with_max_length_constraint_respected() {
    let schema = json!({"type": "string", "pattern": ".+", "maxLength": 2});

    let value = generate(&schema, &default_opts()).unwrap();
    let s = value.as_str().unwrap();

    assert!(s.len() <= 2);
}

/// An uppercase-digit-hyphen pattern produces only conforming characters.
#[test]
fn test_pattern_upper_digit_hyphen_class() {
    let schema = json!({"type": "string", "pattern": "^[A-Z0-9-]+$"});
    let re = regex::Regex::new("^[A-Z0-9-]+$").unwrap();

    for _ in 0..10 {
        let value = generate(&schema, &default_opts()).unwrap();
        let s = value.as_str().unwrap();

        assert!(re.is_match(s));
    }
}

/// An SSN-shaped pattern produces strings matching `\d{3}-\d{2}-\d{4}`.
#[test]
fn test_pattern_ssn_shape() {
    let schema = json!({"type": "string", "pattern": "^\\d{3}-\\d{2}-\\d{4}$"});
    let re = regex::Regex::new("^\\d{3}-\\d{2}-\\d{4}$").unwrap();

    for _ in 0..10 {
        let value = generate(&schema, &default_opts()).unwrap();
        let s = value.as_str().unwrap();

        assert!(re.is_match(s));
    }
}

/// A fixed-width hex pattern produces 8 lowercase hex characters.
#[test]
fn test_pattern_hex_eight_chars() {
    let schema = json!({"type": "string", "pattern": "^[a-f0-9]{8}$"});
    let re = regex::Regex::new("^[a-f0-9]{8}$").unwrap();

    for _ in 0..10 {
        let value = generate(&schema, &default_opts()).unwrap();
        let s = value.as_str().unwrap();

        assert!(re.is_match(s));
    }
}

/// A `pattern` containing non-ASCII (Cyrillic) characters produces a non-empty value.
#[test]
fn test_pattern_non_ascii_produces_non_empty_value() {
    let schema = json!({
        "type": "string",
        "pattern": "^[\u{0410}-\u{042F}\u{0401}\u{0430}-\u{044F}\u{0451}]+([- ][\u{0410}-\u{042F}\u{0401}\u{0430}-\u{044F}\u{0451}]+)*$"
    });

    let value = generate(&schema, &seeded_opts(13)).unwrap();
    let s = value.as_str().expect("result must be a string");

    assert!(!s.is_empty());
}

/// A purely-Cyrillic `pattern` produces only Cyrillic codepoints within the declared ranges.
#[test]
fn test_pattern_cyrillic_emits_cyrillic_only() {
    let schema = json!({
        "type": "string",
        "pattern": "^[\u{0410}-\u{042F}\u{0430}-\u{044F}]{3,8}$"
    });

    let value = generate(&schema, &seeded_opts(13)).unwrap();
    let s = value.as_str().expect("result must be a string");

    for c in s.chars() {
        let cp = c as u32;
        let in_class = (0x0410..=0x042F).contains(&cp) || (0x0430..=0x044F).contains(&cp);
        assert!(in_class);
    }
}

/// A pattern anchored with non-ASCII characters produces a value matching the regex.
#[test]
fn test_pattern_non_ascii_anchored_matches_regex() {
    let pattern = "^[\u{0410}-\u{042F}\u{0401}\u{0430}-\u{044F}\u{0451}]+([- ][\u{0410}-\u{042F}\u{0401}\u{0430}-\u{044F}\u{0451}]+)*$";
    let schema = json!({"type": "string", "pattern": pattern});

    let value = generate(&schema, &seeded_opts(7)).unwrap();
    let s = value.as_str().unwrap();

    let re = regex::Regex::new(pattern).unwrap();
    assert!(re.is_match(s));
}

/// A broken (unparseable) `pattern` combined with an `example` returns the example.
#[test]
fn test_broken_pattern_falls_back_to_example() {
    let schema = json!({
        "type": "string",
        "pattern": "[unclosed",
        "example": "fallback-value"
    });
    let mut opts = seeded_opts(11);
    opts.use_examples_value = false;

    let value = generate(&schema, &opts).unwrap();

    assert_eq!(value.as_str().unwrap(), "fallback-value");
}

/// An impossible-to-match `pattern` combined with an `example` returns the example.
#[test]
fn test_impossible_pattern_falls_back_to_example() {
    let schema = json!({
        "type": "string",
        "pattern": "$impossible^",
        "example": "literal-example"
    });
    let mut opts = seeded_opts(42);
    opts.use_examples_value = false;

    let value = generate(&schema, &opts).unwrap();

    assert_eq!(value.as_str().unwrap(), "literal-example");
}

/// A satisfiable `pattern` with a valid `example` returns a value matching the pattern.
#[test]
fn test_satisfiable_pattern_with_example_matches_pattern() {
    let schema = json!({
        "type": "string",
        "pattern": "^[aA-zZ]{2}(-[aA-zZ]{4})?(-[aA-zZ]{2})?$",
        "example": "en-Latn-US"
    });
    let mut opts = seeded_opts(11);
    opts.use_examples_value = false;

    let value = generate(&schema, &opts).unwrap();
    let s = value.as_str().unwrap();

    let re = regex::Regex::new("^[aA-zZ]{2}(-[aA-zZ]{4})?(-[aA-zZ]{2})?$").unwrap();
    assert!(re.is_match(s));
}

/// A `pattern` plus `minLength`/`maxLength` over an uppercase-digit class produces a matching value.
#[test]
fn test_pattern_with_length_bounds_matches_regex() {
    let schema = json!({
        "type": "string",
        "minLength": 4,
        "maxLength": 6,
        "pattern": "^[A-Z0-9]+$"
    });

    let value = generate(&schema, &seeded_opts(2698)).unwrap();
    let s = value.as_str().unwrap();

    let re = regex::Regex::new("^[A-Z0-9]+$").unwrap();
    assert!(re.is_match(s));
}

/// A `pattern` containing a literal hyphen keeps every emitted character inside the class.
#[test]
fn test_pattern_with_literal_hyphen_stays_in_class() {
    let pattern = "^[A-Z0-9-]+$";
    let schema = json!({
        "type": "string",
        "pattern": pattern,
        "minLength": 3,
        "maxLength": 8
    });

    let value = generate(&schema, &seeded_opts(26981)).unwrap();
    let s = value.as_str().unwrap();

    for c in s.chars() {
        let in_class = c.is_ascii_uppercase() || c.is_ascii_digit() || c == '-';
        assert!(in_class);
    }
}

/// A `minLength` value above the safety cap is clamped without allocating huge strings.
#[test]
fn test_string_min_length_capped_at_safety_limit() {
    let schema = json!({"type": "string", "minLength": 1u64 << 22});

    let result = generate(&schema, &default_opts());

    match result {
        Ok(value) => {
            let s = value.as_str().expect("expected string value");
            assert!(s.chars().count() <= 1 << 20);
        }
        Err(FakerError::SchemaError { message, .. }) => assert!(message.contains("capped")),
        Err(other) => panic!("unexpected error: {:?}", other),
    }
}

/// A regex quantifier of one million repeats is clamped rather than allocating a huge string.
#[test]
fn test_regex_repeat_capped_at_safety_limit() {
    let schema = json!({"type": "string", "pattern": "^a{1000000}$"});

    let result = generate(&schema, &default_opts());

    match result {
        Ok(value) => {
            let s = value.as_str().expect("expected string value");
            assert!(s.chars().count() <= 1 << 20);
        }
        Err(FakerError::SchemaError { message, .. }) => assert!(!message.is_empty()),
        Err(other) => panic!("unexpected error: {:?}", other),
    }
}
