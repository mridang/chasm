use crate::random::Random;
use fake::faker::lorem::en::Word;
use fake::Fake;

/// Generates a valid JSON Pointer string as defined in RFC 6901, composed of a
/// single lorem-word segment driven by the seeded RNG so output is deterministic
/// for a given seed.
pub fn generate(rng: &mut Random) -> String {
    let word: String = Word().fake_with_rng(rng.inner());
    format!("/{}", word)
}
