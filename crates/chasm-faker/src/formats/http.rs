use crate::random::Random;
use fake::faker::http::en::{RfcStatusCode, ValidStatusCode};
use fake::Fake;

/// Generates an RFC-registered HTTP status code as a numeric string.
///
/// The upstream `fake` crate returns a code-plus-reason string such as
/// `"205 Reset Content"`. JSON Schema `format: http-status` callers expect just
/// the 3-digit numeric portion, so the leading token is split off.
pub fn generate_rfc_status_code(rng: &mut Random) -> String {
    let raw: String = RfcStatusCode().fake_with_rng(rng.inner());
    raw.split_whitespace().next().unwrap_or("200").to_string()
}

/// Generates any syntactically valid 100..599 HTTP status code as a numeric string.
///
/// Mirrors `generate_rfc_status_code` by trimming any trailing reason phrase so
/// the returned value matches `^[1-5][0-9]{2}$`.
pub fn generate_valid_status_code(rng: &mut Random) -> String {
    let raw: String = ValidStatusCode().fake_with_rng(rng.inner());
    raw.split_whitespace().next().unwrap_or("200").to_string()
}
