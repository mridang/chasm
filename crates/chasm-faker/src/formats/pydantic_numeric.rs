use crate::random::Random;

/// Generates a string representation of a strictly-negative integer, e.g. `"-42"`.
pub fn generate_negative_int(rng: &mut Random) -> String {
    let n = rng.int(1, 9999);
    format!("-{}", n)
}

/// Generates a string representation of a strictly-positive integer, e.g. `"42"`.
pub fn generate_positive_int(rng: &mut Random) -> String {
    let n = rng.int(1, 9999);
    n.to_string()
}

/// Generates a string representation of a non-negative integer (`>= 0`).
pub fn generate_nonnegative_int(rng: &mut Random) -> String {
    let n = rng.int(0, 9999);
    n.to_string()
}

/// Generates a string representation of a non-positive integer (`<= 0`).
pub fn generate_nonpositive_int(rng: &mut Random) -> String {
    let n = rng.int(0, 9999);
    if n == 0 {
        "0".to_string()
    } else {
        format!("-{}", n)
    }
}

/// Generates a string of decimal digits with no leading sign.
pub fn generate_strict_int(rng: &mut Random) -> String {
    let n = rng.int(0, 9_999_999);
    n.to_string()
}

/// Generates the literal string `"true"` or `"false"`.
pub fn generate_strict_bool(rng: &mut Random) -> String {
    if rng.bool() {
        "true".to_string()
    } else {
        "false".to_string()
    }
}

/// Generates a decimal-fraction string like `"3.14"`.
pub fn generate_strict_float(rng: &mut Random) -> String {
    let v = rng.float(-1000.0, 1000.0);
    format!("{:.2}", v)
}
