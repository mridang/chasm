//! Tests for filesystem-style formats (directory-path, new-path).

use crate::common::seeded_opts;
use chasm_faker::generate;
use serde_json::json;

/// `format: directory-path` produces a POSIX-style absolute path.
#[test]
fn test_format_directory_path_absolute() {
    let schema = json!({"type": "string", "format": "directory-path"});

    let value = generate(&schema, &seeded_opts(42)).unwrap();
    let s = value.as_str().unwrap();

    assert!(s.starts_with('/') && s.len() > 1);
}

/// `format: new-path` produces an absolute path ending in `.new`.
#[test]
fn test_format_new_path_ends_with_new_suffix() {
    let schema = json!({"type": "string", "format": "new-path"});

    let value = generate(&schema, &seeded_opts(42)).unwrap();
    let s = value.as_str().unwrap();

    assert!(s.starts_with('/') && s.ends_with(".new"));
}
