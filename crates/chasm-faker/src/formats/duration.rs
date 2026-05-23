use crate::random::Random;

/// Generates a valid ISO 8601 duration string.
pub fn generate(rng: &mut Random) -> String {
    let years = rng.int(0, 5);
    let months = rng.int(0, 11);
    let days = rng.int(0, 30);
    if years > 0 {
        format!("P{}Y{}M{}D", years, months, days)
    } else if months > 0 {
        format!("P{}M{}D", months, days)
    } else {
        format!("P{}D", days.max(1))
    }
}
