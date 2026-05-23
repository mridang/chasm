use crate::random::Random;
use fake::faker::lorem::en::Word;
use fake::Fake;

/// Generates an RFC 6570 URI Template literal, e.g. `https://example.com/{user_id}/profile`.
///
/// The emitted value always contains at least one `{name}` template variable expression
/// so callers asserting on the `uri-template` shape see a real template, not just a URI.
pub fn generate_uri_template(rng: &mut Random) -> String {
    let var_name: String = Word().fake_with_rng(rng.inner());
    let trailing: String = Word().fake_with_rng(rng.inner());
    format!("https://example.com/{{{}}}/{}", var_name, trailing)
}
