//! Tests for `generators/null.rs`.

use crate::common::default_opts;
use chasm_faker::generate;
use serde_json::json;

/// `generate()` returns a JSON null value when the schema declares `type: "null"`.
#[test]
fn test_returns_null_for_null_type() {
    let schema = json!({"type": "null"});

    let value = generate(&schema, &default_opts()).unwrap();

    assert!(value.is_null());
}
