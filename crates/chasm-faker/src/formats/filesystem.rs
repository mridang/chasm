use crate::random::Random;
use fake::faker::filesystem::en::{
    FileExtension, FileName, FilePath, MimeType, Semver, SemverStable, SemverUnstable,
};
use fake::Fake;

/// Generates a filename like "report.pdf".
pub fn generate_file_name(rng: &mut Random) -> String {
    FileName().fake_with_rng(rng.inner())
}

/// Generates a file extension string like "txt".
pub fn generate_file_extension(rng: &mut Random) -> String {
    FileExtension().fake_with_rng(rng.inner())
}

/// Generates an absolute file path string.
pub fn generate_file_path(rng: &mut Random) -> String {
    FilePath().fake_with_rng(rng.inner())
}

/// Generates a MIME type identifier like "text/plain".
pub fn generate_mime_type(rng: &mut Random) -> String {
    MimeType().fake_with_rng(rng.inner())
}

/// Generates a semantic version string like "1.2.3".
pub fn generate_semver(rng: &mut Random) -> String {
    Semver().fake_with_rng(rng.inner())
}

/// Generates a stable semantic version string (major >= 1).
pub fn generate_semver_stable(rng: &mut Random) -> String {
    SemverStable().fake_with_rng(rng.inner())
}

/// Generates an unstable semantic version string (major == 0).
pub fn generate_semver_unstable(rng: &mut Random) -> String {
    SemverUnstable().fake_with_rng(rng.inner())
}
