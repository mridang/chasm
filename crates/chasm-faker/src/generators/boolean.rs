use crate::random::Random;
use serde_json::Value;

/// Generates a random JSON boolean value.
pub fn generate(rng: &mut Random) -> Value {
    Value::Bool(rng.bool())
}
