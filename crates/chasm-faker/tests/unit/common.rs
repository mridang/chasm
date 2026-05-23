//! Shared helpers used by every chasm-faker unit test file.

use chasm_faker::GenerateOptions;

/// Builds the default options used by most unit tests.
pub fn default_opts() -> GenerateOptions {
    GenerateOptions::default()
}

/// Builds a seeded options object for deterministic tests.
pub fn seeded_opts(seed: u64) -> GenerateOptions {
    let mut opts = GenerateOptions::default();
    opts.seed = Some(seed);
    opts
}
