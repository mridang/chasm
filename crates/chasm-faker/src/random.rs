//! Seeded random-number wrapper used throughout the faker.
//!
//! Wraps `rand_chacha::ChaCha8Rng` to give deterministic output for a given
//! seed, owns the deferred error slot the walker uses to surface generator
//! failures, and exposes the helper sampling functions the generators call
//! into.

use crate::FakerError;
use rand::Rng;
use rand::SeedableRng;
use rand_chacha::ChaCha8Rng;
use std::collections::HashMap;

/// Seeded pseudorandom number generator wrapping ChaCha8Rng for reproducibility.
///
/// In addition to producing random values, this type carries an optional `error`
/// slot used by the schema walker to surface errors out of the recursive `walk`
/// function without changing every call's return type.
pub struct Random {
    rng: ChaCha8Rng,
    error: Option<FakerError>,
    fixed_probabilities: bool,
    fixed_counter: u64,
    path: Vec<String>,
    auto_increment: HashMap<String, i64>,
}

impl Random {
    /// Creates a new `Random` instance, seeding with the provided value or a fresh entropy seed.
    pub fn new(seed: Option<u64>) -> Self {
        let rng = match seed {
            Some(s) => ChaCha8Rng::seed_from_u64(s),
            None => ChaCha8Rng::from_entropy(),
        };
        Self {
            rng,
            error: None,
            fixed_probabilities: false,
            fixed_counter: 0,
            path: Vec::new(),
            auto_increment: HashMap::new(),
        }
    }

    /// Pushes a segment onto the current JSON-pointer path used in error reporting.
    pub fn push_path(&mut self, segment: &str) {
        self.path.push(segment.to_string());
    }

    /// Pops the most recently pushed JSON-pointer path segment.
    pub fn pop_path(&mut self) {
        self.path.pop();
    }

    /// Returns the current JSON-pointer path as a leading-slash string.
    ///
    /// Returns `/` when no segments have been pushed.
    pub fn current_path(&self) -> String {
        if self.path.is_empty() {
            return "/".to_string();
        }
        let mut s = String::new();
        for seg in &self.path {
            s.push('/');
            s.push_str(seg);
        }
        s
    }

    /// Enables deterministic-decision mode for inclusion probabilities.
    ///
    /// When enabled, `should_include` cycles deterministically through include/skip
    /// using a counter rather than the PRNG, so two runs with the same seed produce
    /// identical structural outputs regardless of order-dependent RNG consumption.
    pub fn set_fixed_probabilities(&mut self, on: bool) {
        self.fixed_probabilities = on;
    }

    /// Records a generator error so the caller can surface it from `generate()`.
    ///
    /// Only the first error is retained; subsequent calls are ignored so the original
    /// cause is preserved.
    pub fn set_error(&mut self, err: FakerError) {
        if self.error.is_none() {
            self.error = Some(err);
        }
    }

    /// Returns true when an error has been recorded during walk.
    pub fn has_error(&self) -> bool {
        self.error.is_some()
    }

    /// Takes the recorded error, leaving `None` in its place.
    pub fn take_error(&mut self) -> Option<FakerError> {
        self.error.take()
    }

    /// Returns a random `i64` in the inclusive range `[min, max]`.
    pub fn int(&mut self, min: i64, max: i64) -> i64 {
        if min >= max {
            return min;
        }
        self.rng.gen_range(min..=max)
    }

    /// Returns a random `f64` in the half-open range `[min, max)`.
    pub fn float(&mut self, min: f64, max: f64) -> f64 {
        if min >= max {
            return min;
        }
        self.rng.gen_range(min..max)
    }

    /// Returns a random boolean value.
    pub fn bool(&mut self) -> bool {
        self.rng.gen_bool(0.5)
    }

    /// Returns a random index in `[0, len)`.
    pub fn pick_index(&mut self, len: usize) -> usize {
        if len == 0 {
            return 0;
        }
        self.rng.gen_range(0..len)
    }

    /// Returns `true` with the given `probability` (0.0 = never, 1.0 = always).
    ///
    /// When fixed-probability mode is enabled, decisions are made deterministically by
    /// comparing a counter against the probability threshold instead of consuming the PRNG.
    pub fn should_include(&mut self, probability: f64) -> bool {
        if self.fixed_probabilities {
            let p = probability.clamp(0.0, 1.0);
            let before = (self.fixed_counter as f64) * p;
            self.fixed_counter = self.fixed_counter.wrapping_add(1);
            let after = (self.fixed_counter as f64) * p;
            return after.floor() > before.floor();
        }
        self.rng.gen_bool(probability.clamp(0.0, 1.0))
    }

    /// Returns a random `u8` value in `[0, 255]`.
    pub fn byte(&mut self) -> u8 {
        self.rng.gen()
    }

    /// Returns a random character from the provided slice.
    pub fn pick_char<'a>(&mut self, chars: &'a [char]) -> &'a char {
        let idx = self.pick_index(chars.len());
        &chars[idx]
    }

    /// Returns a mutable reference to the underlying ChaCha8 PRNG.
    ///
    /// Exposed so that external crates implementing `rand::distributions::Distribution`
    /// (such as `rand_regex`) can sample directly from the same deterministic stream.
    pub fn inner(&mut self) -> &mut ChaCha8Rng {
        &mut self.rng
    }

    /// Returns the next value for an auto-incrementing counter keyed by `key`.
    ///
    /// Returns `initial` on the first call for a given `key`, then `initial + 1`,
    /// `initial + 2`, ... on subsequent calls. Each `key` maintains an independent
    /// counter so multiple `autoIncrement` keywords across a schema do not collide.
    pub fn next_auto_increment(&mut self, key: &str, initial: i64) -> i64 {
        match self.auto_increment.get_mut(key) {
            Some(v) => {
                *v = v.saturating_add(1);
                *v
            }
            None => {
                self.auto_increment.insert(key.to_string(), initial);
                initial
            }
        }
    }
}

/// Hashes a string to a `u64` seed using the djb2-variant used by Java's `String.hashCode`.
///
/// Computes `h = ((h << 5) - h) + c` over each character, matching the upstream
/// json-schema-faker runner's string-to-seed mapping. Returned as `u64` after taking the
/// 32-bit two's-complement representation, so the value is suitable as a `ChaCha8Rng` seed.
pub fn random_string_seed(s: &str) -> u64 {
    let mut h: i32 = 0;
    for c in s.chars() {
        h = h.wrapping_shl(5).wrapping_sub(h).wrapping_add(c as i32);
    }
    h as u32 as u64
}
