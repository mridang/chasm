use crate::random::Random;
use fake::faker::internet::en::DomainSuffix;
use fake::faker::lorem::en::Word;
use fake::Fake;

/// Generates a plausible HTTPS URI by combining lorem words with a realistic domain
/// suffix, driven by the seeded RNG so output is deterministic for a given seed.
pub fn generate(rng: &mut Random) -> String {
    let host: String = Word().fake_with_rng(rng.inner());
    let suffix: String = DomainSuffix().fake_with_rng(rng.inner());
    let path: String = Word().fake_with_rng(rng.inner());
    format!("https://{}.{}/{}", host, suffix, path)
}
