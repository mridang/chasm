use crate::random::Random;
use fake::faker::internet::en::SafeEmail;
use fake::Fake;

/// Generates a plausible email address using `fake::faker::internet::en::SafeEmail`,
/// driven by the seeded RNG so output is deterministic for a given seed.
pub fn generate(rng: &mut Random) -> String {
    SafeEmail().fake_with_rng(rng.inner())
}
