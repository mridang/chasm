use crate::random::Random;
use fake::faker::internet::en::DomainSuffix;
use fake::faker::lorem::en::Word;
use fake::Fake;

/// Pre-canned non-ASCII labels used to make IDN-shaped values contain Unicode.
const UNICODE_LABELS: &[&str] = &["例え", "пример", "мир", "テスト"];

/// Returns a deterministically chosen non-ASCII label from the canned list.
fn pick_unicode_label(rng: &mut Random) -> &'static str {
    let idx = rng.pick_index(UNICODE_LABELS.len());
    UNICODE_LABELS[idx]
}

/// Generates an Internationalised Resource Identifier (IRI) with a Unicode path segment.
///
/// Produces a value shaped like `https://<label>.<suffix>/<path>` where `<label>` is one
/// of a small set of pre-canned non-ASCII strings, ensuring the result actually contains
/// Unicode characters and not just ASCII punycode.
pub fn generate_iri(rng: &mut Random) -> String {
    let label = pick_unicode_label(rng);
    let suffix: String = DomainSuffix().fake_with_rng(rng.inner());
    let path: String = Word().fake_with_rng(rng.inner());
    format!("https://{}.{}/{}", label, suffix, path)
}

/// Generates an internationalised email address with a Unicode-domain right-hand side.
///
/// Produces a value shaped like `<user>@<label>.<suffix>` where `<label>` is drawn from
/// the canned non-ASCII list. The local part is a lowercase lorem word.
pub fn generate_idn_email(rng: &mut Random) -> String {
    let user: String = Word().fake_with_rng(rng.inner());
    let label = pick_unicode_label(rng);
    let suffix: String = DomainSuffix().fake_with_rng(rng.inner());
    format!("{}@{}.{}", user, label, suffix)
}

/// Generates an internationalised domain name (IDN) such as `例え.jp`.
///
/// Combines one of the canned non-ASCII labels with a realistic ASCII top-level domain
/// suffix, yielding a hostname that contains Unicode characters in its left-most label.
pub fn generate_idn_hostname(rng: &mut Random) -> String {
    let label = pick_unicode_label(rng);
    let suffix: String = DomainSuffix().fake_with_rng(rng.inner());
    format!("{}.{}", label, suffix)
}
