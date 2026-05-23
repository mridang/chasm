use crate::random::Random;
use fake::faker::boolean::en::Boolean;
use fake::Fake;

/// Generates a random boolean with 50% probability of being true.
pub fn generate_boolean(rng: &mut Random) -> bool {
    Boolean(50).fake_with_rng(rng.inner())
}
