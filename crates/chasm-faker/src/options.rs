//! Caller-facing knobs for tuning a single faker invocation.
//!
//! Defines the [`GenerateOptions`] struct plus the [`OutputTransform`]
//! closure alias used by callers to post-process the generated value. The
//! per-call registry snapshots that propagate through the walker also live
//! here.

use crate::extensions::{ExtensionRegistry, FormatRegistry};
use std::collections::HashMap;
use std::sync::Arc;

/// Closure type for post-generation output transforms.
///
/// Receives `(generated_value, root_schema)` and returns a possibly-modified value.
/// Boxed `Fn` so closures with captured state are supported; `Send + Sync` so the
/// option struct remains shareable across threads.
pub type OutputTransform =
    Arc<dyn Fn(&serde_json::Value, &serde_json::Value) -> serde_json::Value + Send + Sync>;

/// Configuration options controlling how fake data is generated from JSON schemas.
#[derive(Clone)]
#[non_exhaustive]
pub struct GenerateOptions {
    /// Optional seed for deterministic pseudorandom generation.
    pub seed: Option<u64>,
    /// Maximum nesting depth before returning safe terminal values.
    pub max_depth: usize,
    /// Whether to always generate optional (non-required) properties.
    pub always_fake_optionals: bool,
    /// Probability between 0.0 and 1.0 that each optional property is included.
    pub optionals_probability: Option<f64>,
    /// Whether to return the schema's `default` value when present.
    pub use_default_value: bool,
    /// Whether to return a value from the schema's `examples` array when present.
    pub use_examples_value: bool,
    /// Minimum number of times a `$ref` may recurse before stopping.
    pub ref_depth_min: usize,
    /// Maximum number of times a `$ref` may recurse before stopping.
    pub ref_depth_max: usize,
    /// Property names to skip entirely during object generation.
    pub ignore_properties: Vec<String>,
    /// Property names to remove from the generated object after generation.
    pub prune_properties: Vec<String>,
    /// When true, only required properties are generated for objects.
    pub required_only: bool,
    /// When true, an unknown `type` string returns an error instead of null.
    pub fail_on_invalid_type: bool,
    /// When true, an unknown `format` string returns an error instead of falling back.
    ///
    /// Defaults to `false` to match lenient behaviour for unknown formats; strict
    /// mode is opt-in by setting this to `true`.
    pub fail_on_invalid_format: bool,
    /// When true, probabilities are applied deterministically in order (not randomly).
    pub fixed_probabilities: bool,
    /// Global minimum items override for all arrays in this generation pass.
    pub min_items: Option<usize>,
    /// Global maximum items override for all arrays in this generation pass.
    pub max_items: Option<usize>,
    /// Default value returned for unrecognised types when `fail_on_invalid_type` is false.
    pub default_invalid_type_product: Option<serde_json::Value>,
    /// Map of external `$ref` identifiers (URL or `$id`) to inline replacement schemas.
    ///
    /// When a schema references one of these identifiers, the resolver substitutes the
    /// supplied schema instead of attempting a network fetch.
    pub external_refs: HashMap<String, serde_json::Value>,
    /// When true, missing optional properties are added to satisfy `minProperties`.
    ///
    /// Mirrors the upstream `fillProperties` json-schema-faker setting. Defaults to `true`.
    pub fill_properties: bool,
    /// When true, generated object properties whose value is JSON `null` are removed.
    ///
    /// Mirrors the upstream `omitNulls` json-schema-faker setting. Applied after each
    /// object is fully generated so that `type: ["string", "null"]` properties that
    /// happen to resolve to `null` are pruned from the output rather than emitted as
    /// `"key": null`. Defaults to `false`.
    pub omit_nulls: bool,
    /// Earliest ISO-8601 timestamp that the date-time format generator may produce.
    pub min_date_time: Option<String>,
    /// Latest ISO-8601 timestamp that the date-time format generator may produce.
    pub max_date_time: Option<String>,
    /// Map of schema-key aliases applied before schema processing: `{ alias_key: canonical_key }`.
    ///
    /// When a schema object contains a key matching an alias, the key is renamed to the
    /// canonical name before generation, unless the canonical key is already present.
    pub prop_aliases: HashMap<String, String>,
    /// Optional hard cap on array length when the array's `items` schema declares a `default`.
    ///
    /// When `Some(n)` and `use_default_value` is `true`, an array whose item schema carries a
    /// `default` value is truncated to at most `n` elements, overriding `minItems`/`maxItems`
    /// and global `min_items`/`max_items`. Defaults to `None` (no override).
    pub max_default_items: Option<usize>,
    /// When true, the input schema's `$schema` keyword is validated against supported drafts.
    ///
    /// Defaults to `false` for lenient processing; opt-in strict mode is enabled by
    /// setting this to `true`.
    pub validate_schema_version: bool,
    /// Optional post-generation transform applied to the final value.
    ///
    /// The closure receives the generated value and the root schema; its return value
    /// replaces the generated output. Used by callers that need a final fix-up pass
    /// (e.g. injecting computed fields) without writing a custom walker.
    pub output_transform: Option<OutputTransform>,
    /// Per-call snapshot of the global extension registry.
    ///
    /// [`crate::generate`] populates this with an `Arc` clone of
    /// [`crate::extensions::ExtensionRegistry`] taken from the process-global registry
    /// at entry. The walker then consults this snapshot instead of re-reading the
    /// global registry on every extension lookup. This guarantees that concurrent calls
    /// to [`crate::define`] from another thread cannot mutate the set of registered
    /// extensions mid-generation, preserving same-seed determinism.
    ///
    /// Callers should leave this field as `None`: it is filled in automatically by
    /// `generate()` and intended to be opaque to user code.
    pub extension_snapshot: Option<Arc<ExtensionRegistry>>,
    /// Per-call snapshot of the global format registry.
    ///
    /// [`crate::generate`] populates this with an `Arc` clone of
    /// [`crate::extensions::FormatRegistry`] taken from the process-global registry
    /// at entry, for the same nondeterminism-avoidance reason as
    /// [`Self::extension_snapshot`].
    ///
    /// Note: the format dispatcher in `formats/mod.rs` currently still consults the
    /// global registry directly. The snapshot is plumbed here so the walker can use
    /// it for extension-driven lookups; widening it to the format dispatcher is a
    /// follow-up.
    ///
    /// Callers should leave this field as `None`: it is filled in automatically by
    /// `generate()` and intended to be opaque to user code.
    pub format_snapshot: Option<Arc<FormatRegistry>>,
}

impl std::fmt::Debug for GenerateOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GenerateOptions")
            .field("seed", &self.seed)
            .field("max_depth", &self.max_depth)
            .field("always_fake_optionals", &self.always_fake_optionals)
            .field("optionals_probability", &self.optionals_probability)
            .field("use_default_value", &self.use_default_value)
            .field("use_examples_value", &self.use_examples_value)
            .field("ref_depth_min", &self.ref_depth_min)
            .field("ref_depth_max", &self.ref_depth_max)
            .field("ignore_properties", &self.ignore_properties)
            .field("prune_properties", &self.prune_properties)
            .field("required_only", &self.required_only)
            .field("fail_on_invalid_type", &self.fail_on_invalid_type)
            .field("fail_on_invalid_format", &self.fail_on_invalid_format)
            .field("fixed_probabilities", &self.fixed_probabilities)
            .field("min_items", &self.min_items)
            .field("max_items", &self.max_items)
            .field(
                "default_invalid_type_product",
                &self.default_invalid_type_product,
            )
            .field("external_refs", &self.external_refs)
            .field("fill_properties", &self.fill_properties)
            .field("omit_nulls", &self.omit_nulls)
            .field("min_date_time", &self.min_date_time)
            .field("max_date_time", &self.max_date_time)
            .field("prop_aliases", &self.prop_aliases)
            .field("max_default_items", &self.max_default_items)
            .field("validate_schema_version", &self.validate_schema_version)
            .field(
                "output_transform",
                &self.output_transform.as_ref().map(|_| "<fn>"),
            )
            .field(
                "extension_snapshot",
                &self.extension_snapshot.as_ref().map(|_| "<snapshot>"),
            )
            .field(
                "format_snapshot",
                &self.format_snapshot.as_ref().map(|_| "<snapshot>"),
            )
            .finish()
    }
}

impl Default for GenerateOptions {
    /// Returns sensible defaults matching the json-schema-faker JavaScript library defaults.
    fn default() -> Self {
        Self {
            seed: None,
            max_depth: 5,
            always_fake_optionals: false,
            optionals_probability: None,
            use_default_value: false,
            use_examples_value: false,
            ref_depth_min: 0,
            ref_depth_max: 3,
            ignore_properties: Vec::new(),
            prune_properties: Vec::new(),
            required_only: false,
            fail_on_invalid_type: true,
            fail_on_invalid_format: false,
            fixed_probabilities: false,
            min_items: None,
            max_items: None,
            default_invalid_type_product: None,
            external_refs: HashMap::new(),
            fill_properties: true,
            omit_nulls: false,
            min_date_time: None,
            max_date_time: None,
            prop_aliases: HashMap::new(),
            max_default_items: None,
            validate_schema_version: false,
            output_transform: None,
            extension_snapshot: None,
            format_snapshot: None,
        }
    }
}
