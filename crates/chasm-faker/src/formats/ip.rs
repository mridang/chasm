use crate::random::Random;
use fake::faker::internet::en::{IPv4, IPv6};
use fake::Fake;

/// Generates a random IPv4 address string using `fake::faker::internet::en::IPv4`,
/// driven by the seeded RNG so output is deterministic for a given seed.
pub fn generate_ipv4(rng: &mut Random) -> String {
    IPv4().fake_with_rng(rng.inner())
}

/// Generates a random IPv6 address string using `fake::faker::internet::en::IPv6`,
/// driven by the seeded RNG so output is deterministic for a given seed.
pub fn generate_ipv6(rng: &mut Random) -> String {
    IPv6().fake_with_rng(rng.inner())
}
