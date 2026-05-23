use crate::random::Random;
use fake::faker::internet::en::DomainSuffix;
use fake::faker::lorem::en::Word;
use fake::Fake;

/// Generates a plausible hostname-shaped string by combining a lorem word with a
/// realistic domain suffix, driven by the seeded RNG so output is deterministic
/// for a given seed.
pub fn generate(rng: &mut Random) -> String {
    let word: String = Word().fake_with_rng(rng.inner());
    let suffix: String = DomainSuffix().fake_with_rng(rng.inner());
    format!("{}.{}", word, suffix)
}
