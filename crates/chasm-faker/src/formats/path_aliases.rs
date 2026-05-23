use crate::random::Random;
use fake::faker::lorem::en::Word;
use fake::Fake;

/// Generates a POSIX-style directory path with two or three lowercase segments.
///
/// Unlike `file-path`, the produced value names a directory (no trailing filename),
/// e.g. `/usr/local/share`. Words are drawn from the lorem corpus and the segment
/// count and content are driven by the seeded RNG.
pub fn generate_directory_path(rng: &mut Random) -> String {
    let segment_count = rng.int(2, 3) as usize;
    let mut parts: Vec<String> = Vec::with_capacity(segment_count);
    for _ in 0..segment_count {
        let w: String = Word().fake_with_rng(rng.inner());
        parts.push(w);
    }
    format!("/{}", parts.join("/"))
}

/// Generates a file path ending with a `.new` suffix.
///
/// Aliased to the `new-path` format: produces a value shaped like a file path
/// whose final segment ends with a `.new` extension, useful for callers wanting
/// a fresh path marker. The directory portion is generated via the same word
/// corpus used by `directory-path`.
pub fn generate_new_path(rng: &mut Random) -> String {
    let dir = generate_directory_path(rng);
    let name: String = Word().fake_with_rng(rng.inner());
    format!("{}/{}.new", dir, name)
}
