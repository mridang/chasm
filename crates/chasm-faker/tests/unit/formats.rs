//! Tests for `formats/*.rs` format generators.

use chasm_faker::generate;
use serde_json::json;

use crate::common::seeded_opts;

#[path = "formats/company.rs"]
mod company;
#[path = "formats/faker_namespaces.rs"]
mod faker_namespaces;
#[path = "formats/filesystem.rs"]
mod filesystem;
#[path = "formats/finance.rs"]
mod finance;
#[path = "formats/numeric.rs"]
mod numeric;
#[path = "formats/pydantic.rs"]
mod pydantic;
#[path = "formats/string_formats.rs"]
mod string_formats;

/// Verifies that a schema using the given faker key generates a string without erroring
/// in strict-format mode. Used to assert that a faker namespace is recognised by the
/// walker's allow-list.
pub(crate) fn assert_faker_namespace_recognized(faker_key: &str) {
    let schema = json!({"type": "string", "faker": faker_key});
    let mut opts = seeded_opts(42);
    opts.fail_on_invalid_format = true;

    let result = generate(&schema, &opts);

    assert!(result.is_ok(), "faker key `{}` not recognized", faker_key);
}
