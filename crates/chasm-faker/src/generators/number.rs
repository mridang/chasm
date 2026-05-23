use crate::formats::pydantic_numeric;
use crate::options::GenerateOptions;
use crate::random::Random;
use serde_json::Value;

/// Max attempts to find an integer satisfying `multipleOf` within the bound
/// range before falling back to a deterministic in-range candidate.
const MULTIPLE_OF_RETRY_BUDGET: usize = 100;

/// Generates a JSON numeric value for a Pydantic-style `format` keyword on an
/// integer or number schema.
///
/// Pydantic emits `{type: integer, format: positive-int}` (and friends) to encode
/// constraints like "strictly positive" that don't have a direct JSON Schema
/// keyword. This dispatcher handles those format names directly so the numeric
/// generators don't fall through to the string-side format pipeline.
///
/// Returns `None` for unrecognised format names, leaving the caller's default
/// range-based generation in place. When `as_float` is true the value is
/// emitted as a JSON number with potential fractional part (for `type: number`);
/// otherwise it is emitted as an integer.
fn generate_for_numeric_format(
    format: &str,
    schema: &Value,
    rng: &mut Random,
    as_float: bool,
) -> Option<Value> {
    let has_min = schema.get("minimum").and_then(|v| v.as_f64()).is_some();
    let has_max = schema.get("maximum").and_then(|v| v.as_f64()).is_some();
    let (min_f, max_f, _, _) = extract_bounds(schema);
    match format {
        "positive-int" => {
            let lo = if has_min {
                clamp_to_i64(min_f).max(1)
            } else {
                1
            };
            let hi = if has_max {
                clamp_to_i64(max_f)
            } else {
                1_000_000
            };
            let n = rng.int(lo, hi.max(lo));
            Some(if as_float {
                json_number(n as f64)
            } else {
                Value::Number(serde_json::Number::from(n))
            })
        }
        "negative-int" => {
            let hi = if has_max {
                clamp_to_i64(max_f).min(-1)
            } else {
                -1
            };
            let lo = if has_min {
                clamp_to_i64(min_f)
            } else {
                -1_000_000
            };
            let n = rng.int(lo.min(hi), hi);
            Some(if as_float {
                json_number(n as f64)
            } else {
                Value::Number(serde_json::Number::from(n))
            })
        }
        "nonnegative-int" => {
            let lo = if has_min {
                clamp_to_i64(min_f).max(0)
            } else {
                0
            };
            let hi = if has_max {
                clamp_to_i64(max_f)
            } else {
                1_000_000
            };
            let n = rng.int(lo, hi.max(lo));
            Some(if as_float {
                json_number(n as f64)
            } else {
                Value::Number(serde_json::Number::from(n))
            })
        }
        "nonpositive-int" => {
            let hi = if has_max {
                clamp_to_i64(max_f).min(0)
            } else {
                0
            };
            let lo = if has_min {
                clamp_to_i64(min_f)
            } else {
                -1_000_000
            };
            let n = rng.int(lo.min(hi), hi);
            Some(if as_float {
                json_number(n as f64)
            } else {
                Value::Number(serde_json::Number::from(n))
            })
        }
        "int32" | "strict-int" => {
            let (min_f, max_f, _, _) = extract_bounds(schema);
            let min = if has_min {
                clamp_to_i64(min_f)
                    .max(i32::MIN as i64)
                    .min(i32::MAX as i64)
            } else {
                i32::MIN as i64
            };
            let max = if has_max {
                clamp_to_i64(max_f)
                    .max(i32::MIN as i64)
                    .min(i32::MAX as i64)
            } else {
                i32::MAX as i64
            };
            let n = rng.int(min.min(max), max.max(min));
            Some(if as_float {
                json_number(n as f64)
            } else {
                Value::Number(serde_json::Number::from(n))
            })
        }
        "int64" => {
            let (min_f, max_f, _, _) = extract_bounds(schema);
            let min = if has_min {
                clamp_to_i64(min_f)
            } else {
                -1_000_000_000_i64
            };
            let max = if has_max {
                clamp_to_i64(max_f)
            } else {
                1_000_000_000_i64
            };
            let n = rng.int(min.min(max), max.max(min));
            Some(if as_float {
                json_number(n as f64)
            } else {
                Value::Number(serde_json::Number::from(n))
            })
        }
        "strict-float" | "float" | "double" => {
            let (min_f, max_f, _, _) = extract_bounds(schema);
            let min = if has_min { min_f } else { -1000.0 };
            let max = if has_max { max_f } else { 1000.0 };
            let v = rng.float(min, max);
            if as_float {
                Some(json_number(v))
            } else {
                Some(Value::Number(serde_json::Number::from(v as i64)))
            }
        }
        "strict-bool" => {
            // `strict-bool` should not surface on numeric schemas, but if it does
            // we touch the pydantic helper to keep the format reachable from here
            // and emit a 0/1 integer so the result remains numeric.
            let _ = pydantic_numeric::generate_strict_bool(rng);
            let b = rng.bool();
            let n: i64 = if b { 1 } else { 0 };
            Some(if as_float {
                json_number(n as f64)
            } else {
                Value::Number(serde_json::Number::from(n))
            })
        }
        _ => None,
    }
}

/// Generates a JSON number (floating-point) value respecting the given schema constraints.
///
/// Handles `minimum`, `maximum`, `exclusiveMinimum`, `exclusiveMaximum`, and `multipleOf`.
pub fn generate(schema: &Value, rng: &mut Random, _opts: &GenerateOptions) -> Value {
    if let Some(fmt) = schema.get("format").and_then(|v| v.as_str()) {
        if let Some(v) = generate_for_numeric_format(fmt, schema, rng, true) {
            return v;
        }
    }
    let (min, max, exclusive_min, exclusive_max) = extract_bounds(schema);
    let effective_min = if exclusive_min { next_up(min) } else { min };
    let effective_max = if exclusive_max { next_down(max) } else { max };
    let safe_min = effective_min.min(effective_max);
    let safe_max = effective_max.max(safe_min);
    let multiple_of = schema
        .get("multipleOf")
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0);
    if multiple_of > 0.0 {
        return json_number(generate_multiple_of(safe_min, safe_max, multiple_of, rng));
    }
    if (safe_max - safe_min).abs() < f64::EPSILON {
        return json_number(safe_min);
    }
    let value = rng.float(safe_min, safe_max);
    json_number(value)
}

/// Generates a value in `[safe_min, safe_max]` that is an exact multiple of `multiple_of`.
///
/// Uses integer arithmetic scaled by the decimal precision of `multiple_of` to avoid
/// IEEE 754 round-trip errors (e.g. `56 * 0.01 = 0.5600000000000001`). Retries up to
/// 100 times when the candidate would fail strict validators that test
/// `(v/multiple_of) % 1.0 < f64::EPSILON`, which rejects values like `0.14 / 0.01`
/// whose IEEE 754 division yields `14.0 + 1.78e-15`.
fn generate_multiple_of(safe_min: f64, safe_max: f64, multiple_of: f64, rng: &mut Random) -> f64 {
    // Guard against overflow when `multiple_of` is too large for the scaled-i64 path.
    // The integer arithmetic below multiplies by `scale = 10^places` (up to 1e18), so a
    // `multiple_of` larger than ~1e15 risks overflowing `i64::MAX` (~9.2e18) silently.
    // Above that threshold we fall back to picking a midpoint candidate and let strict
    // validators decide acceptance; emitting any value here is preferable to wrap-around.
    if !multiple_of.is_finite() || multiple_of.abs() > 1e15 {
        if safe_min <= 0.0 && 0.0 <= safe_max {
            return 0.0;
        }
        return safe_min;
    }
    let places = decimal_places(multiple_of).min(18);
    let scale = 10_i64.pow(places);
    let scale_f = scale as f64;
    let mo_int = clamp_to_i64(multiple_of * scale_f);
    let min_int = clamp_to_i64(safe_min * scale_f);
    let max_int = clamp_to_i64(safe_max * scale_f);
    // Verify the round-trip preserves the value: if scaling lost precision, we cannot
    // safely produce an exact integer multiple. Emit a best-effort value.
    let roundtrip_ok =
        (mo_int as f64 / scale_f - multiple_of).abs() < multiple_of.abs() * 1e-10 + f64::EPSILON;
    if !roundtrip_ok {
        if safe_min <= 0.0 && 0.0 <= safe_max {
            return 0.0;
        }
        return safe_min;
    }
    let first = ceil_multiple_i64(min_int, mo_int);
    let last = floor_multiple_i64(max_int, mo_int);
    if mo_int == 0 || first > last {
        return safe_min;
    }
    let count = (last - first) / mo_int;
    for _ in 0..MULTIPLE_OF_RETRY_BUDGET {
        let chosen = rng.int(0, count);
        let result_int = first + chosen * mo_int;
        let result = result_int as f64 / scale as f64;
        if passes_strict_multiple_of_check(result, multiple_of) {
            return result;
        }
    }
    if first <= 0 && 0 <= last {
        return 0.0;
    }
    first as f64 / scale as f64
}

/// Returns true if `value / multiple_of` is within `f64::EPSILON` of an integer.
///
/// Matches the strict float-precision check used by the `jsonschema` crate's
/// `MultipleOfFloatValidator`, so generated values agree with validation.
fn passes_strict_multiple_of_check(value: f64, multiple_of: f64) -> bool {
    let remainder = (value / multiple_of) % 1.0;
    remainder.abs() < f64::EPSILON || (1.0 - remainder.abs()).abs() < f64::EPSILON
}

/// Counts the number of significant decimal places in an f64 value.
///
/// Handles scientific notation produced by Rust's default float formatting
/// (e.g. `1e-7`) by parsing the exponent and combining it with the mantissa's
/// fractional digit count. Returns `0` for non-finite values and zero.
fn decimal_places(v: f64) -> u32 {
    if v == 0.0 || !v.is_finite() {
        return 0;
    }
    let s = format!("{:?}", v);
    let lower = s.to_lowercase();
    if let Some(e_pos) = lower.find('e') {
        let mantissa = &s[..e_pos];
        let exp: i32 = s[e_pos + 1..].parse().unwrap_or(0);
        let mantissa_dp = mantissa
            .find('.')
            .map(|p| (mantissa.len() - p - 1) as i32)
            .unwrap_or(0);
        let total = mantissa_dp - exp;
        return total.max(0) as u32;
    }
    s.find('.').map(|p| (s.len() - p - 1) as u32).unwrap_or(0)
}

/// Returns the next representable `f64` strictly greater than `v`.
///
/// Used to nudge inclusive bounds into exclusive ones without depending on a
/// fixed epsilon, which fails for very small numbers near zero.
fn next_up(v: f64) -> f64 {
    if v.is_nan() || v == f64::INFINITY {
        return v;
    }
    if v == 0.0 {
        return f64::from_bits(1);
    }
    let bits = v.to_bits();
    let next_bits = if v > 0.0 { bits + 1 } else { bits - 1 };
    f64::from_bits(next_bits)
}

/// Returns the next representable `f64` strictly less than `v`.
///
/// Used to nudge inclusive bounds into exclusive ones without depending on a
/// fixed epsilon, which fails for very small numbers near zero.
fn next_down(v: f64) -> f64 {
    if v.is_nan() || v == f64::NEG_INFINITY {
        return v;
    }
    if v == 0.0 {
        return f64::from_bits((-0.0_f64).to_bits() + 1);
    }
    let bits = v.to_bits();
    let next_bits = if v > 0.0 { bits - 1 } else { bits + 1 };
    f64::from_bits(next_bits)
}

/// Saturates an `f64` to the representable `i64` range before truncating.
///
/// Plain `value as i64` is undefined behaviour for non-finite inputs and saturates
/// to `i64::MIN`/`i64::MAX` for out-of-range finite inputs in modern Rust; this helper
/// makes the intent explicit and round-trips NaN to `0`.
fn clamp_to_i64(value: f64) -> i64 {
    if value.is_nan() {
        return 0;
    }
    let clamped = value.clamp(i64::MIN as f64, i64::MAX as f64);
    clamped.round() as i64
}

/// Returns the smallest integer >= `n` that is a multiple of `m`.
fn ceil_multiple_i64(n: i64, m: i64) -> i64 {
    if m == 0 {
        return n;
    }
    let r = n % m;
    if r == 0 {
        n
    } else if n >= 0 {
        n + (m - r)
    } else {
        n - r
    }
}

/// Returns the largest integer <= `n` that is a multiple of `m`.
fn floor_multiple_i64(n: i64, m: i64) -> i64 {
    if m == 0 {
        return n;
    }
    let r = n % m;
    if r == 0 {
        n
    } else if n >= 0 {
        n - r
    } else {
        n - (m + r)
    }
}

/// Generates a JSON integer value respecting the given schema constraints.
///
/// Handles `minimum`, `maximum`, `exclusiveMinimum`, `exclusiveMaximum`, and `multipleOf`.
/// When `multipleOf` is fractional (e.g. `0.5`), candidates are produced via the
/// floating-point multiple-of path and accepted only if they have no fractional
/// component, falling back to scanning integers in range for a valid multiple.
pub fn generate_integer(schema: &Value, rng: &mut Random, _opts: &GenerateOptions) -> Value {
    if let Some(fmt) = schema.get("format").and_then(|v| v.as_str()) {
        if let Some(v) = generate_for_numeric_format(fmt, schema, rng, false) {
            return v;
        }
    }
    let (min_f, max_f, exclusive_min, exclusive_max) = extract_bounds(schema);
    let mut min = clamp_to_i64(min_f.ceil());
    let mut max = clamp_to_i64(max_f.floor());
    if exclusive_min {
        min = min.saturating_add(1);
    }
    if exclusive_max {
        max = max.saturating_sub(1);
    }
    if min > max {
        max = min;
    }
    let multiple_of_f = schema.get("multipleOf").and_then(|v| v.as_f64());
    if let Some(mo) = multiple_of_f {
        if mo > 0.0 {
            return generate_integer_with_multiple_of(min, max, mo, rng);
        }
    }
    Value::Number(serde_json::Number::from(rng.int(min, max)))
}

/// Picks an integer in `[min, max]` whose value is also an exact multiple of `multiple_of`.
///
/// When `multiple_of` is an integer this uses direct integer arithmetic. When it is
/// fractional, candidates are drawn from the floating-point multiple-of generator and
/// accepted only when they have no fractional part; if no such candidate is found, the
/// range is scanned for the first integer that satisfies the strict multiple-of check.
fn generate_integer_with_multiple_of(
    min: i64,
    max: i64,
    multiple_of: f64,
    rng: &mut Random,
) -> Value {
    if multiple_of.fract() == 0.0 {
        let mo_int = multiple_of as i64;
        if mo_int <= 0 {
            return Value::Number(serde_json::Number::from(rng.int(min, max)));
        }
        let first = ceil_multiple_i64(min, mo_int);
        let last = floor_multiple_i64(max, mo_int);
        if first > last {
            return Value::Number(serde_json::Number::from(first));
        }
        let count = (last - first) / mo_int;
        let chosen = rng.int(0, count);
        let result = first + chosen * mo_int;
        return Value::Number(serde_json::Number::from(result));
    }
    for _ in 0..MULTIPLE_OF_RETRY_BUDGET {
        let candidate = generate_multiple_of(min as f64, max as f64, multiple_of, rng);
        if candidate.fract() == 0.0
            && candidate >= min as f64
            && candidate <= max as f64
            && passes_strict_multiple_of_check(candidate, multiple_of)
        {
            return Value::Number(serde_json::Number::from(candidate as i64));
        }
    }
    let mut n = min;
    while n <= max {
        if passes_strict_multiple_of_check(n as f64, multiple_of) {
            return Value::Number(serde_json::Number::from(n));
        }
        if n == i64::MAX {
            break;
        }
        n += 1;
    }
    Value::Number(serde_json::Number::from(min))
}

/// Extracts numeric bounds from a schema, returning `(min, max, exclusive_min, exclusive_max)`.
fn extract_bounds(schema: &Value) -> (f64, f64, bool, bool) {
    let mut min = -1000.0_f64;
    let mut max = 1000.0_f64;
    let mut exclusive_min = false;
    let mut exclusive_max = false;

    if let Some(v) = schema.get("minimum").and_then(|v| v.as_f64()) {
        min = v;
    }
    if let Some(v) = schema.get("maximum").and_then(|v| v.as_f64()) {
        max = v;
    }
    if let Some(v) = schema.get("exclusiveMinimum") {
        if let Some(b) = v.as_bool() {
            exclusive_min = b;
        } else if let Some(n) = v.as_f64() {
            min = n;
            exclusive_min = true;
        }
    }
    if let Some(v) = schema.get("exclusiveMaximum") {
        if let Some(b) = v.as_bool() {
            exclusive_max = b;
        } else if let Some(n) = v.as_f64() {
            max = n;
            exclusive_max = true;
        }
    }
    (min, max, exclusive_min, exclusive_max)
}

/// Converts an `f64` to the nearest representable `serde_json::Value::Number`.
///
/// When `f` has no fractional part and fits in `i64`, returns an integer-valued number
/// so JSON serialisation produces e.g. `1` instead of `1.0`. This matches the upstream
/// json-schema-faker behaviour where whole-number `type: number` values are emitted
/// without a decimal point.
fn json_number(f: f64) -> Value {
    if f.fract() == 0.0 && f.is_finite() && f.abs() < i64::MAX as f64 {
        return Value::Number(serde_json::Number::from(f as i64));
    }
    match serde_json::Number::from_f64(f) {
        Some(n) => Value::Number(n),
        None => Value::Number(serde_json::Number::from(0)),
    }
}
