use crate::random::Random;
use fake::faker::name::en::{FirstName, LastName, Name, NameWithTitle, Suffix, Title};
use fake::Fake;

/// Generates a full personal name like "Bob Smith".
pub fn generate_full_name(rng: &mut Random) -> String {
    Name().fake_with_rng(rng.inner())
}

/// Generates a first name.
pub fn generate_first_name(rng: &mut Random) -> String {
    FirstName().fake_with_rng(rng.inner())
}

/// Generates a last name.
pub fn generate_last_name(rng: &mut Random) -> String {
    LastName().fake_with_rng(rng.inner())
}

/// Generates a name with title (e.g. "Dr. Bob Smith Jr.").
pub fn generate_name_with_title(rng: &mut Random) -> String {
    NameWithTitle().fake_with_rng(rng.inner())
}

/// Generates an honorific title like "Dr.", "Mr.".
pub fn generate_title(rng: &mut Random) -> String {
    Title().fake_with_rng(rng.inner())
}

/// Generates a name suffix like "Jr.", "PhD".
pub fn generate_suffix(rng: &mut Random) -> String {
    Suffix().fake_with_rng(rng.inner())
}
