use crate::random::Random;
use chrono::{Datelike, Duration, NaiveDate};
use serde_json::{json, Value};

use super::{email, lorem};

/// Generates a `YYYY-MM-DD` date string within ±10 years of a fixed anchor.
///
/// Mirrors Pydantic v2's `condate` constrained-date type. The anchor is the
/// fixed date `2024-01-15` (not `Utc::now`) so that a given `--seed` produces
/// byte-identical output run-to-run; deterministic output is the documented
/// contract of `--seed`, and that trumps the "realistic dates around the
/// current era" goal a wall-clock anchor would have provided.
pub fn generate_condate(rng: &mut Random) -> String {
    let anchor = NaiveDate::from_ymd_opt(2024, 1, 15)
        .expect("hard-coded anchor date 2024-01-15 is always valid");
    let offset_days = rng.int(-3650, 3650);
    let picked = anchor + Duration::days(offset_days);
    format!(
        "{:04}-{:02}-{:02}",
        picked.year(),
        picked.month(),
        picked.day()
    )
}

/// Generates a constrained decimal string like `"12345.67"` with 2..=4 fractional digits.
///
/// Mirrors Pydantic v2's `condecimal` constrained-decimal type. The fractional
/// part is left zero-padded to the chosen width so the emitted string preserves
/// the requested precision rather than dropping trailing zeros.
pub fn generate_condecimal(rng: &mut Random) -> String {
    let whole = rng.int(0, 99_999);
    let frac_digits = rng.int(2, 4) as usize;
    let max_frac = 10_i64.pow(frac_digits as u32);
    let frac = rng.int(0, max_frac - 1);
    format!("{}.{:0width$}", whole, frac, width = frac_digits)
}

/// Generates an RFC3339 timestamp carrying an explicit `+00:00` UTC offset.
///
/// Pydantic v2's `aware-datetime` requires a timezone-bearing value. The legacy
/// `date-time` format emits the `Z` suffix; this variant uses the explicit numeric
/// offset Pydantic exercises in its docs (e.g. `2024-01-15T10:30:00+00:00`).
pub fn generate_aware_datetime(rng: &mut Random) -> String {
    let year = rng.int(2000, 2030);
    let month = rng.int(1, 12);
    let day = rng.int(1, 28);
    let hour = rng.int(0, 23);
    let minute = rng.int(0, 59);
    let second = rng.int(0, 59);
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}+00:00",
        year, month, day, hour, minute, second
    )
}

/// Generates an ISO 8601 timestamp without any timezone designator.
///
/// Pydantic v2's `naive-datetime` requires the absence of a timezone, which the
/// stock `date-time` generator can't produce (it always emits `Z`). Emits the
/// canonical `YYYY-MM-DDTHH:MM:SS` shape with no trailing offset.
pub fn generate_naive_datetime(rng: &mut Random) -> String {
    let year = rng.int(2000, 2030);
    let month = rng.int(1, 12);
    let day = rng.int(1, 28);
    let hour = rng.int(0, 23);
    let minute = rng.int(0, 59);
    let second = rng.int(0, 59);
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}",
        year, month, day, hour, minute, second
    )
}

/// Generates an RFC 5322 display-name email like `"John Doe <john@example.com>"`.
///
/// Pydantic v2's `name-email` accepts the full RFC 5322 mailbox syntax. The
/// display name is sourced from two lorem words (lowercased by the upstream
/// generator, capitalised here) and the address is delegated to the existing
/// `email` generator so the same registry-override hook applies.
pub fn generate_name_email(rng: &mut Random) -> String {
    let first = capitalise(&lorem::generate_word(rng));
    let last = capitalise(&lorem::generate_word(rng));
    let address = email::generate(rng);
    format!("{} {} <{}>", first, last, address)
}

/// Capitalises the first character of `word`, leaving the rest unchanged.
fn capitalise(word: &str) -> String {
    let mut chars = word.chars();
    match chars.next() {
        Some(c) => c.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

/// Generates a JSON-encoded string containing a tiny object literal.
///
/// Pydantic v2's `json-string` expects a string that itself parses as JSON. This
/// emits the canonical `{"k":"v"}` shape via `serde_json::to_string` so callers
/// that double-decode the value always succeed.
pub fn generate_json_string(_rng: &mut Random) -> String {
    let v = json!({ "k": "v" });
    serde_json::to_string(&Value::Object(v.as_object().cloned().unwrap_or_default()))
        .unwrap_or_else(|_| "{}".to_string())
}
