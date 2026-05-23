//! Tests for company-name, industry, job-title, catch-phrase, BS family, and related company formats.

use crate::common::seeded_opts;
use chasm_faker::generate;
use serde_json::json;

/// `format: company-name` produces a non-empty string under a deterministic seed.
#[test]
fn test_format_company_name_non_empty() {
    let schema = json!({"type": "string", "format": "company-name"});

    let value = generate(&schema, &seeded_opts(42)).unwrap();
    let s = value.as_str().unwrap();

    assert!(!s.is_empty());
}

/// `format: company-suffix` produces a non-empty string under a deterministic seed.
#[test]
fn test_format_company_suffix_non_empty() {
    let schema = json!({"type": "string", "format": "company-suffix"});

    let value = generate(&schema, &seeded_opts(42)).unwrap();
    let s = value.as_str().unwrap();

    assert!(!s.is_empty());
}

/// `format: industry` produces a non-empty string under a deterministic seed.
#[test]
fn test_format_industry_non_empty() {
    let schema = json!({"type": "string", "format": "industry"});

    let value = generate(&schema, &seeded_opts(42)).unwrap();
    let s = value.as_str().unwrap();

    assert!(!s.is_empty());
}

/// `format: job-title` produces a non-empty string under a deterministic seed.
#[test]
fn test_format_job_title_non_empty() {
    let schema = json!({"type": "string", "format": "job-title"});

    let value = generate(&schema, &seeded_opts(42)).unwrap();
    let s = value.as_str().unwrap();

    assert!(!s.is_empty());
}

/// `format: catch-phrase` produces a non-empty string under a deterministic seed.
#[test]
fn test_format_catch_phrase_non_empty() {
    let schema = json!({"type": "string", "format": "catch-phrase"});

    let value = generate(&schema, &seeded_opts(42)).unwrap();
    let s = value.as_str().unwrap();

    assert!(!s.is_empty());
}

/// `format: buzzword` produces a non-empty string under a deterministic seed.
#[test]
fn test_format_buzzword_non_empty() {
    let schema = json!({"type": "string", "format": "buzzword"});

    let value = generate(&schema, &seeded_opts(42)).unwrap();
    let s = value.as_str().unwrap();

    assert!(!s.is_empty());
}

/// Company-domain format aliases dispatch to the same generator as their canonical name.
///
/// Each `(alias, canonical)` pair must produce the same value under the same seed because the
/// dispatcher in `formats/mod.rs` routes both through the same generator function.
#[test]
fn test_company_format_aliases_route_to_canonical() {
    let pairs = [
        ("company", "company-name"),
        ("profession", "job-title"),
        ("job", "job-title"),
        ("catchphrase", "catch-phrase"),
    ];

    for (alias, canonical) in pairs {
        let alias_schema = json!({"type": "string", "format": alias});
        let canonical_schema = json!({"type": "string", "format": canonical});
        let a = generate(&alias_schema, &seeded_opts(42)).unwrap();
        let b = generate(&canonical_schema, &seeded_opts(42)).unwrap();
        assert_eq!(
            a, b,
            "alias `{}` should produce same output as `{}`",
            alias, canonical
        );
    }
}

/// `format: bs-adjective` produces a single-word non-empty value.
#[test]
fn test_format_bs_adjective_single_word() {
    let schema = json!({"type": "string", "format": "bs-adjective"});

    let value = generate(&schema, &seeded_opts(42)).unwrap();
    let s = value.as_str().unwrap();

    let word_count = s.split_whitespace().count();
    assert!(
        word_count == 1 && !s.is_empty(),
        "bs-adjective `{}` should be a single non-empty word (got {} words)",
        s,
        word_count
    );
}

/// `format: bs-noun` produces a single-word non-empty value.
#[test]
fn test_format_bs_noun_single_word() {
    let schema = json!({"type": "string", "format": "bs-noun"});

    let value = generate(&schema, &seeded_opts(42)).unwrap();
    let s = value.as_str().unwrap();

    let word_count = s.split_whitespace().count();
    assert!(
        word_count == 1 && !s.is_empty(),
        "bs-noun `{}` should be a single non-empty word (got {} words)",
        s,
        word_count
    );
}

/// `format: bs-verb` produces a single-word non-empty value.
#[test]
fn test_format_bs_verb_single_word() {
    let schema = json!({"type": "string", "format": "bs-verb"});

    let value = generate(&schema, &seeded_opts(42)).unwrap();
    let s = value.as_str().unwrap();

    let word_count = s.split_whitespace().count();
    assert!(
        word_count == 1 && !s.is_empty(),
        "bs-verb `{}` should be a single non-empty word (got {} words)",
        s,
        word_count
    );
}
