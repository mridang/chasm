//! Tests for `random.rs`.

use crate::common::{default_opts, seeded_opts};
use chasm_faker::__test_internals::{random_string_seed, Random};
use chasm_faker::generate;
use serde_json::json;

/// `next_auto_increment` returns the initial value on the first call for a key.
#[test]
fn test_auto_increment_returns_initial_on_first_call() {
    let mut rng = Random::new(Some(0));

    let first = rng.next_auto_increment("foo", 10);

    assert_eq!(first, 10);
}

/// `next_auto_increment` increments by one on subsequent calls for the same key.
#[test]
fn test_auto_increment_increments_on_subsequent_calls() {
    let mut rng = Random::new(Some(0));
    rng.next_auto_increment("foo", 10);

    let second = rng.next_auto_increment("foo", 10);

    assert_eq!(second, 11);
}

/// `next_auto_increment` keeps independent counters per key.
#[test]
fn test_auto_increment_independent_counters_per_key() {
    let mut rng = Random::new(Some(0));
    rng.next_auto_increment("a", 1);
    rng.next_auto_increment("b", 100);
    rng.next_auto_increment("a", 1);

    let b_second = rng.next_auto_increment("b", 100);

    assert_eq!(b_second, 101);
}

/// The same seed produces identical output for the same schema.
#[test]
fn test_same_seed_produces_same_output() {
    let schema = json!({
        "type": "object",
        "properties": {
            "a": {"type": "integer", "minimum": 0, "maximum": 100},
            "b": {"type": "string"}
        },
        "required": ["a", "b"]
    });
    let mut opts = seeded_opts(42);
    opts.always_fake_optionals = true;

    let result1 = generate(&schema, &opts).unwrap();
    let result2 = generate(&schema, &opts).unwrap();

    assert_eq!(result1, result2);
}

/// Different seeds produce different outputs for complex schemas.
#[test]
fn test_different_seeds_produce_different_output() {
    let schema = json!({"type": "integer", "minimum": 0, "maximum": 1000000});

    let result1 = generate(&schema, &seeded_opts(1)).unwrap();
    let result2 = generate(&schema, &seeded_opts(999999)).unwrap();

    assert_ne!(result1, result2);
}

/// Omitting the seed still produces a value within the schema-declared range.
#[test]
fn test_no_seed_generates_in_range_value() {
    let schema = json!({"type": "integer", "minimum": 0, "maximum": 100});
    let mut opts = default_opts();
    opts.seed = None;

    let value = generate(&schema, &opts).unwrap();
    let n = value.as_i64().unwrap();

    assert!((0..=100).contains(&n));
}

/// `random_string_seed` hashes the same input to the same `u64` on every call.
#[test]
fn test_random_string_seed_is_pure() {
    let first = random_string_seed("some seed");

    let second = random_string_seed("some seed");

    assert_eq!(first, second);
}

/// Different input strings hash to different `u64` seeds.
#[test]
fn test_random_string_seed_differs_per_input() {
    let one = random_string_seed("alpha");

    let other = random_string_seed("beta");

    assert_ne!(one, other);
}

/// A seed derived via `random_string_seed` produces deterministic generator output.
#[test]
fn test_string_derived_seed_is_deterministic() {
    let schema = json!({"type": "integer", "minimum": 0, "maximum": 1000});
    let seed = random_string_seed("some seed");

    let result1 = generate(&schema, &seeded_opts(seed)).unwrap();
    let result2 = generate(&schema, &seeded_opts(seed)).unwrap();

    assert_eq!(result1, result2);
}

/// `push_path` appends a segment so `current_path` reports the joined leading-slash form.
#[test]
fn test_push_path_appends_segment_to_current_path() {
    let mut rng = Random::new(Some(0));

    rng.push_path("a");

    assert_eq!(rng.current_path(), "/a");
}

/// `pop_path` removes the most recently pushed segment so `current_path` falls back to the previous join.
#[test]
fn test_pop_path_removes_last_segment() {
    let mut rng = Random::new(Some(0));
    rng.push_path("a");
    rng.push_path("b");

    rng.pop_path();

    assert_eq!(rng.current_path(), "/a");
}

/// `current_path` returns the single-slash sentinel when no segments have been pushed.
#[test]
fn test_current_path_returns_root_when_empty() {
    let rng = Random::new(Some(0));

    assert_eq!(rng.current_path(), "/");
}

/// `int(min, max)` returns a value within the inclusive `[min, max]` range.
#[test]
fn test_int_returns_value_in_range() {
    let mut rng = Random::new(Some(7));

    let value = rng.int(10, 20);

    assert!((10..=20).contains(&value));
}

/// `int(min, max)` clamps to `min` when `min >= max` (degenerate range).
#[test]
fn test_int_clamps_when_min_ge_max() {
    let mut rng = Random::new(Some(0));

    let value = rng.int(5, 5);

    assert_eq!(value, 5);
}

/// `float(min, max)` returns a value within the half-open `[min, max)` range.
#[test]
fn test_float_returns_value_in_range() {
    let mut rng = Random::new(Some(3));

    let value = rng.float(0.0, 1.0);

    assert!((0.0..1.0).contains(&value));
}

/// `float(min, max)` clamps to `min` when `min >= max` (degenerate range).
#[test]
fn test_float_clamps_when_min_ge_max() {
    let mut rng = Random::new(Some(0));

    let value = rng.float(2.5, 2.5);

    assert!((value - 2.5).abs() < f64::EPSILON);
}

/// `bool()` samples both `true` and `false` over many seeds, demonstrating a non-degenerate distribution.
#[test]
fn test_bool_samples_both_values_across_seeds() {
    let mut saw_true = false;
    let mut saw_false = false;
    for seed in 0u64..50 {
        let mut rng = Random::new(Some(seed));
        if rng.bool() {
            saw_true = true;
        } else {
            saw_false = true;
        }
        if saw_true && saw_false {
            break;
        }
    }

    assert!(saw_true && saw_false);
}

/// `pick_index(n)` returns an index within `[0, n)` for a non-empty range.
#[test]
fn test_pick_index_returns_value_in_range() {
    let mut rng = Random::new(Some(5));

    let idx = rng.pick_index(10);

    assert!(idx < 10);
}

/// `pick_index(0)` returns `0` as the degenerate guard for an empty range.
#[test]
fn test_pick_index_zero_returns_zero() {
    let mut rng = Random::new(Some(0));

    let idx = rng.pick_index(0);

    assert_eq!(idx, 0);
}

/// `set_fixed_probabilities(true)` switches `should_include` to a deterministic counter-based decision,
/// so repeated calls at `p=0.5` produce a stable include/skip cycle independent of the PRNG.
#[test]
fn test_set_fixed_probabilities_yields_deterministic_should_include() {
    let mut rng = Random::new(Some(123));
    rng.set_fixed_probabilities(true);

    let decisions: Vec<bool> = (0..6).map(|_| rng.should_include(0.5)).collect();

    assert_eq!(decisions, vec![false, true, false, true, false, true]);
}

/// `set_error` records an error and `has_error` reports it; `take_error` returns the recorded value
/// and clears the slot so a subsequent `has_error` returns `false`.
#[test]
fn test_error_state_set_has_take_round_trip() {
    use chasm_faker::FakerError;
    let mut rng = Random::new(Some(0));
    rng.set_error(FakerError::SchemaError {
        path: "/".to_string(),
        message: "boom".to_string(),
    });
    let had_error_before = rng.has_error();

    let taken = rng.take_error();
    let has_error_after = rng.has_error();

    assert_eq!(
        (had_error_before, taken.is_some(), has_error_after),
        (true, true, false),
        "expected (set, take returns Some, slot cleared); got ({had_error_before}, {:?}, {has_error_after})",
        taken.is_some(),
    );
}
