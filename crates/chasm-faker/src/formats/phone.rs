use crate::random::Random;
use fake::faker::phone_number::en::PhoneNumber;
use fake::Fake;

/// Generates a plausible phone number string.
pub fn generate_phone_number(rng: &mut Random) -> String {
    PhoneNumber().fake_with_rng(rng.inner())
}
