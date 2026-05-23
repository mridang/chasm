use crate::random::Random;
use fake::faker::barcode::en::{Isbn, Isbn10, Isbn13};
use fake::Fake;

/// Generates an ISBN string (10- or 13-digit form, randomly chosen by the faker).
pub fn generate_isbn(rng: &mut Random) -> String {
    Isbn().fake_with_rng(rng.inner())
}

/// Generates a 10-digit ISBN string.
pub fn generate_isbn10(rng: &mut Random) -> String {
    Isbn10().fake_with_rng(rng.inner())
}

/// Generates a 13-digit ISBN string.
pub fn generate_isbn13(rng: &mut Random) -> String {
    Isbn13().fake_with_rng(rng.inner())
}
