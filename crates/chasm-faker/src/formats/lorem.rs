use crate::random::Random;
use fake::faker::lorem::en::{Paragraph, Paragraphs, Sentence, Sentences, Word, Words};
use fake::Fake;

/// Generates a single lorem-ipsum word.
pub fn generate_word(rng: &mut Random) -> String {
    Word().fake_with_rng(rng.inner())
}

/// Generates a space-separated run of 3..7 lorem-ipsum words.
pub fn generate_words(rng: &mut Random) -> String {
    Words(3..7)
        .fake_with_rng::<Vec<String>, _>(rng.inner())
        .join(" ")
}

/// Generates a single lorem-ipsum sentence containing 3..8 words.
pub fn generate_sentence(rng: &mut Random) -> String {
    Sentence(3..8).fake_with_rng(rng.inner())
}

/// Generates a space-separated run of 3..7 lorem-ipsum sentences.
pub fn generate_sentences(rng: &mut Random) -> String {
    Sentences(3..7)
        .fake_with_rng::<Vec<String>, _>(rng.inner())
        .join(" ")
}

/// Generates a single lorem-ipsum paragraph composed of 2..5 sentences.
pub fn generate_paragraph(rng: &mut Random) -> String {
    Paragraph(2..5).fake_with_rng(rng.inner())
}

/// Generates a double-newline-separated run of 2..4 lorem-ipsum paragraphs.
pub fn generate_paragraphs(rng: &mut Random) -> String {
    Paragraphs(2..4)
        .fake_with_rng::<Vec<String>, _>(rng.inner())
        .join("\n\n")
}
