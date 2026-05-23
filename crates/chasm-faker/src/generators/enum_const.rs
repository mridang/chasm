use crate::random::Random;
use serde_json::Value;

/// Picks a random value from the schema's `enum` array.
///
/// Returns `Value::Null` if the enum array is absent or empty.
pub fn generate_enum(schema: &Value, rng: &mut Random) -> Value {
    if let Some(Value::Array(variants)) = schema.get("enum") {
        if variants.is_empty() {
            return Value::Null;
        }
        let idx = rng.pick_index(variants.len());
        return variants[idx].clone();
    }
    Value::Null
}

/// Returns the `const` value from the schema directly.
///
/// When the `const` keyword is present this function returns the constant
/// value verbatim — including `Value::Null` when the schema explicitly sets
/// `const: null`. Returns `Value::Null` only when the `const` keyword itself
/// is absent.
pub fn generate_const(schema: &Value) -> Value {
    match schema.get("const") {
        Some(c) => c.clone(),
        None => Value::Null,
    }
}
