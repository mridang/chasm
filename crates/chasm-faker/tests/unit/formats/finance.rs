//! Tests for ISBN and other finance-related string formats.

use crate::common::seeded_opts;
use chasm_faker::generate;
use serde_json::json;

/// Returns the count of decimal digits plus optional final `X` in `s`, ignoring all other characters.
///
/// ISBN dashed forms include `-` separators; the canonical "digit count" used to distinguish
/// ISBN-10 from ISBN-13 ignores those separators.
fn isbn_digit_count(s: &str) -> usize {
    s.chars()
        .filter(|c| c.is_ascii_digit() || *c == 'X')
        .count()
}

/// `format: isbn` produces a value whose digit count is either 10 or 13 (the only two valid ISBN lengths).
#[test]
fn test_format_isbn_digit_count_is_10_or_13() {
    let schema = json!({"type": "string", "format": "isbn"});

    let value = generate(&schema, &seeded_opts(42)).unwrap();
    let s = value.as_str().unwrap();

    let count = isbn_digit_count(s);
    assert!(
        count == 10 || count == 13,
        "isbn `{}` digit count {}",
        s,
        count
    );
}

/// `format: isbn` produces a value containing only digits, the literal `X` check character, and ASCII dashes.
#[test]
fn test_format_isbn_alphabet() {
    let schema = json!({"type": "string", "format": "isbn"});
    let re = regex::Regex::new(r"^[0-9X-]+$").unwrap();

    let value = generate(&schema, &seeded_opts(42)).unwrap();
    let s = value.as_str().unwrap();

    assert!(re.is_match(s), "isbn `{}` contains invalid characters", s);
}

/// `format: isbn10` produces a value with exactly ten ISBN digits (the final digit may be `X`).
#[test]
fn test_format_isbn10_digit_count_is_10() {
    let schema = json!({"type": "string", "format": "isbn10"});

    let value = generate(&schema, &seeded_opts(42)).unwrap();
    let s = value.as_str().unwrap();

    assert_eq!(isbn_digit_count(s), 10, "isbn10 `{}` digit count wrong", s);
}

/// `format: isbn10` produces a value containing only digits, the check character `X`, and ASCII dashes.
#[test]
fn test_format_isbn10_alphabet() {
    let schema = json!({"type": "string", "format": "isbn10"});
    let re = regex::Regex::new(r"^[0-9X-]+$").unwrap();

    let value = generate(&schema, &seeded_opts(42)).unwrap();
    let s = value.as_str().unwrap();

    assert!(re.is_match(s), "isbn10 `{}` contains invalid characters", s);
}

/// The `isbn-10` alias routes to the ISBN-10 generator (ten ISBN digits).
#[test]
fn test_format_isbn_dash_10_alias_digit_count() {
    let schema = json!({"type": "string", "format": "isbn-10"});

    let value = generate(&schema, &seeded_opts(42)).unwrap();
    let s = value.as_str().unwrap();

    assert_eq!(isbn_digit_count(s), 10, "isbn-10 `{}` digit count wrong", s);
}

/// `format: isbn13` produces a value with exactly thirteen digits.
#[test]
fn test_format_isbn13_digit_count_is_13() {
    let schema = json!({"type": "string", "format": "isbn13"});

    let value = generate(&schema, &seeded_opts(42)).unwrap();
    let s = value.as_str().unwrap();

    assert_eq!(isbn_digit_count(s), 13, "isbn13 `{}` digit count wrong", s);
}

/// `format: isbn13` produces a value beginning with the `978` or `979` ISBN-13 prefix.
#[test]
fn test_format_isbn13_starts_with_978_or_979() {
    let schema = json!({"type": "string", "format": "isbn13"});

    let value = generate(&schema, &seeded_opts(42)).unwrap();
    let s = value.as_str().unwrap();

    assert!(
        s.starts_with("978") || s.starts_with("979"),
        "isbn13 `{}` does not start with 978/979",
        s
    );
}

/// The `isbn-13` alias routes to the ISBN-13 generator (thirteen digits).
#[test]
fn test_format_isbn_dash_13_alias_digit_count() {
    let schema = json!({"type": "string", "format": "isbn-13"});

    let value = generate(&schema, &seeded_opts(42)).unwrap();
    let s = value.as_str().unwrap();

    assert_eq!(isbn_digit_count(s), 13, "isbn-13 `{}` digit count wrong", s);
}
