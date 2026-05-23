//! Custom keyword and string-format registries for the faker.
//!
//! Owns the [`ExtensionRegistry`] and [`FormatRegistry`] types plus their
//! shared `Arc`-based closure aliases. Cloning a registry is cheap because
//! each stored closure is reference-counted, which the snapshot path in
//! [`crate::generate`] relies on for deterministic per-call dispatch.

use crate::random::Random;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;

/// Type alias for a function that generates a `Value` given the local schema node, the
/// root schema, the property name (when invoked from inside an object generator, `None`
/// at the schema root), and the JSON Pointer schema path of the current node.
///
/// This mirrors the upstream JSF callback signature `(value, schema, property,
/// schemaPath)`.
///
/// The public constructor accepts a `Box<dyn Fn>` so user code can call
/// `Box::new(|...| ...)` without importing `Arc`. The registry stores the closure
/// internally as an [`SharedExtensionFn`] (`Arc<dyn Fn ...>`) so registries can be cheaply
/// snapshotted by [`crate::generate`] for deterministic same-seed runs.
///
/// # Forward-compatibility warning
///
/// A future major version of `chasm-faker` is expected to switch this signature
/// from the current four positional parameters to a single `ExtensionCtx` struct
/// (carrying `value`, `schema`, `property`, and `schema_path` as named fields) so
/// new contextual fields can be added without further breaking changes. Callers
/// that want to insulate themselves from that transition should wrap their
/// closures in a small adapter that takes the current four-argument form and
/// delegates to user code, so only the adapter needs editing when the signature
/// changes.
pub type ExtensionFn = Box<dyn Fn(&Value, &Value, Option<&str>, &str) -> Value + Send + Sync>;

/// Shared, reference-counted form of [`ExtensionFn`] used internally by
/// [`ExtensionRegistry`] so cloning the registry only bumps the closures' Arc
/// reference counts rather than duplicating them.
pub type SharedExtensionFn = Arc<dyn Fn(&Value, &Value, Option<&str>, &str) -> Value + Send + Sync>;

/// Type alias for a function that generates a `Value` for a custom string format using
/// the shared `Random` source.
///
/// Format generators are registered via [`crate::register_format`] and invoked via
/// [`crate::invoke_format`]. The dispatcher in `formats/mod.rs` consults the registry
/// before falling through to built-in format generators.
pub type FormatFn = Box<dyn Fn(&mut Random) -> Value + Send + Sync>;

/// Shared, reference-counted form of [`FormatFn`] used internally by
/// [`FormatRegistry`] so cloning the registry only bumps the closures' Arc
/// reference counts rather than duplicating them.
pub type SharedFormatFn = Arc<dyn Fn(&mut Random) -> Value + Send + Sync>;

/// Registry mapping custom schema keywords to their generator functions.
pub struct ExtensionRegistry {
    extensions: HashMap<String, SharedExtensionFn>,
}

impl std::fmt::Debug for ExtensionRegistry {
    /// Surfaces the registered keyword names plus a placeholder for each
    /// closure handler — the closures themselves cannot be inspected, so a
    /// `<extension fn>` sentinel is used in their place to keep the output
    /// readable while still listing the keys that are live.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut dbg = f.debug_struct("ExtensionRegistry");
        let mut keys: Vec<&String> = self.extensions.keys().collect();
        keys.sort();
        let entries: Vec<(&String, &'static str)> =
            keys.into_iter().map(|k| (k, "<extension fn>")).collect();
        dbg.field("extensions", &entries).finish()
    }
}

impl ExtensionRegistry {
    /// Creates a new, empty extension registry.
    pub fn new() -> Self {
        Self {
            extensions: HashMap::new(),
        }
    }

    /// Registers a new keyword handler, replacing any existing registration for that keyword.
    pub fn define(&mut self, keyword: &str, f: ExtensionFn) {
        let stored: SharedExtensionFn = Arc::from(f);
        self.extensions.insert(keyword.to_string(), stored);
    }

    /// Removes all registered keyword handlers.
    pub fn reset(&mut self) {
        self.extensions.clear();
    }

    /// Returns the handler for the given keyword, or `None` if not registered.
    pub fn get(&self, keyword: &str) -> Option<&SharedExtensionFn> {
        self.extensions.get(keyword)
    }
}

impl Clone for ExtensionRegistry {
    /// Produces a snapshot of the registry suitable for use as a per-`generate()` view.
    ///
    /// Cloning is cheap: closures are stored as [`Arc`] so duplicating the registry only
    /// bumps reference counts on each entry rather than copying the underlying closures.
    fn clone(&self) -> Self {
        Self {
            extensions: self
                .extensions
                .iter()
                .map(|(k, v)| (k.clone(), Arc::clone(v)))
                .collect(),
        }
    }
}

impl Default for ExtensionRegistry {
    /// Creates a default (empty) extension registry.
    fn default() -> Self {
        Self::new()
    }
}

/// Registry mapping custom string format names to their generator functions.
pub struct FormatRegistry {
    formats: HashMap<String, SharedFormatFn>,
}

impl std::fmt::Debug for FormatRegistry {
    /// Surfaces the registered format names plus a placeholder for each
    /// closure handler — the closures themselves cannot be inspected, so a
    /// `<format fn>` sentinel is used in their place to keep the output
    /// readable while still listing the keys that are live.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut dbg = f.debug_struct("FormatRegistry");
        let mut keys: Vec<&String> = self.formats.keys().collect();
        keys.sort();
        let entries: Vec<(&String, &'static str)> =
            keys.into_iter().map(|k| (k, "<format fn>")).collect();
        dbg.field("formats", &entries).finish()
    }
}

impl FormatRegistry {
    /// Creates a new, empty format registry.
    pub fn new() -> Self {
        Self {
            formats: HashMap::new(),
        }
    }

    /// Registers a new format generator, replacing any existing registration for that
    /// format name.
    pub fn define(&mut self, name: &str, f: FormatFn) {
        let stored: SharedFormatFn = Arc::from(f);
        self.formats.insert(name.to_string(), stored);
    }

    /// Removes all registered format generators.
    pub fn reset(&mut self) {
        self.formats.clear();
    }

    /// Returns the handler for the given format, or `None` if not registered.
    pub fn get(&self, name: &str) -> Option<&SharedFormatFn> {
        self.formats.get(name)
    }
}

impl Clone for FormatRegistry {
    /// Produces a snapshot of the registry suitable for use as a per-`generate()` view.
    ///
    /// Cloning is cheap: closures are stored as [`Arc`] so duplicating the registry only
    /// bumps reference counts on each entry rather than copying the underlying closures.
    fn clone(&self) -> Self {
        Self {
            formats: self
                .formats
                .iter()
                .map(|(k, v)| (k.clone(), Arc::clone(v)))
                .collect(),
        }
    }
}

impl Default for FormatRegistry {
    /// Creates a default (empty) format registry.
    fn default() -> Self {
        Self::new()
    }
}
