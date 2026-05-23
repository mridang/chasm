use crate::formats;
use crate::options::GenerateOptions;
use crate::random::Random;
use rand::distributions::Distribution;
use serde_json::Value;
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;

/// Maximum number of compiled regexes kept in each per-thread cache before
/// older entries are evicted (by clearing the map). Bounded so that a
/// pathological spec with thousands of distinct patterns cannot grow these
/// caches without limit.
const PATTERN_CACHE_CAPACITY: usize = 256;

/// Maximum raw byte length of a `pattern` source we will hand to the regex
/// compiler. Patterns longer than this are treated as unsupported (skipped
/// silently with a `tracing::warn!`) rather than compiled, so a spec
/// declaring a multi-megabyte literal cannot turn the generator into an
/// unbounded allocator on first use.
const MAX_PATTERN_BYTES: usize = 65_536;

thread_local! {
    /// Per-thread cache of compiled `regex::Regex` instances, keyed by the
    /// original (possibly anchored) pattern. Failed compilations are cached
    /// as `None` so a broken pattern is not retried on every call.
    static REGEX_CACHE: RefCell<HashMap<String, Option<Arc<regex::Regex>>>> =
        RefCell::new(HashMap::new());

    /// Per-thread cache of compiled `rand_regex::Regex` samplers, keyed by
    /// the anchored pattern. Failed compilations are cached as `None`.
    static RAND_REGEX_CACHE: RefCell<HashMap<String, Option<Arc<rand_regex::Regex>>>> =
        RefCell::new(HashMap::new());
}

/// Returns a compiled `regex::Regex` for `pattern`, consulting the per-thread
/// cache first and inserting the result on miss. Returns `None` when the
/// pattern fails to compile.
fn cached_regex(pattern: &str) -> Option<Arc<regex::Regex>> {
    if pattern.len() > MAX_PATTERN_BYTES {
        tracing::warn!(
            pattern_bytes = pattern.len(),
            limit = MAX_PATTERN_BYTES,
            "regex pattern source exceeds size cap: skipping (treated as unsupported)"
        );
        return None;
    }
    REGEX_CACHE.with(|cell| {
        let mut map = cell.borrow_mut();
        if let Some(entry) = map.get(pattern) {
            return entry.clone();
        }
        if map.len() >= PATTERN_CACHE_CAPACITY {
            map.clear();
        }
        let compiled = regex::Regex::new(pattern).ok().map(Arc::new);
        map.insert(pattern.to_string(), compiled.clone());
        compiled
    })
}

/// Returns a compiled `rand_regex::Regex` sampler for the anchored pattern,
/// consulting the per-thread cache first. Returns `None` when the pattern
/// cannot be compiled by `rand_regex`.
fn cached_rand_regex(anchored: &str) -> Option<Arc<rand_regex::Regex>> {
    if anchored.len() > MAX_PATTERN_BYTES {
        tracing::warn!(
            pattern_bytes = anchored.len(),
            limit = MAX_PATTERN_BYTES,
            "rand_regex pattern source exceeds size cap: skipping (treated as unsupported)"
        );
        return None;
    }
    RAND_REGEX_CACHE.with(|cell| {
        let mut map = cell.borrow_mut();
        if let Some(entry) = map.get(anchored) {
            return entry.clone();
        }
        if map.len() >= PATTERN_CACHE_CAPACITY {
            map.clear();
        }
        let compiled = rand_regex::Regex::compile(anchored, 32).ok().map(Arc::new);
        map.insert(anchored.to_string(), compiled.clone());
        compiled
    })
}

/// Maximum string length chasm will allocate from a single schema, to prevent DoS via crafted minLength/maxLength values.
pub(crate) const MAX_GENERATED_STRING_LENGTH: usize = 1 << 20;

/// Maximum array length chasm will allocate from a single schema.
pub(crate) const MAX_GENERATED_ITEMS: usize = 1 << 16;

/// Maximum object key count.
pub(crate) const MAX_GENERATED_PROPERTIES: usize = 1 << 16;

/// Maximum regex repeat quantifier `{n}` chasm will honor; larger values are clamped.
pub(crate) const MAX_REGEX_REPEAT: usize = 1024;

/// Generates a JSON string value respecting all applicable string schema constraints.
///
/// Handles `format`, `pattern`, `minLength`, and `maxLength` in combination.
/// When `format` names a recognised generator it takes precedence; when `format` is
/// unknown but a sibling `pattern` is present, the pattern generator is used so the
/// emitted value still satisfies the schema's regular expression.
pub fn generate(schema: &Value, _root: &Value, opts: &GenerateOptions, rng: &mut Random) -> Value {
    let raw_min_length = schema
        .get("minLength")
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as usize;
    let raw_max_length_opt: Option<usize> = schema
        .get("maxLength")
        .and_then(|v| v.as_u64())
        .map(|v| v as usize);
    if raw_min_length > MAX_GENERATED_STRING_LENGTH {
        let path = rng.current_path();
        rng.set_error(crate::FakerError::SchemaError {
            path,
            message: format!(
                "string keyword minLength={} capped to {} for safety",
                raw_min_length, MAX_GENERATED_STRING_LENGTH
            ),
        });
    }
    if let Some(raw_max) = raw_max_length_opt {
        if raw_max > MAX_GENERATED_STRING_LENGTH {
            let path = rng.current_path();
            rng.set_error(crate::FakerError::SchemaError {
                path,
                message: format!(
                    "string keyword maxLength={} capped to {} for safety",
                    raw_max, MAX_GENERATED_STRING_LENGTH
                ),
            });
        }
    }
    let min_length = raw_min_length.min(MAX_GENERATED_STRING_LENGTH);
    let max_length_opt: Option<usize> =
        raw_max_length_opt.map(|v| v.min(MAX_GENERATED_STRING_LENGTH));
    let max_length = max_length_opt.unwrap_or(20);
    let safe_max = max_length.max(min_length);

    let format_opt = schema.get("format").and_then(|v| v.as_str());
    let pattern_opt = schema.get("pattern").and_then(|v| v.as_str());

    if let (Some(format), Some(pattern)) = (format_opt, pattern_opt) {
        if format == "regex" && formats::is_known_format(format) {
            let s =
                formats::generate_for_format_with_options(format, rng, Some(schema), Some(opts));
            return Value::String(s);
        }
        if formats::is_known_format(format) {
            let format_raw =
                formats::generate_for_format_with_options(format, rng, Some(schema), Some(opts));
            let format_value = if max_length_opt.is_none() && min_length == 0 {
                format_raw
            } else {
                pad_or_trim(format_raw, min_length, safe_max, rng)
            };
            let pattern_value = generate_from_pattern(pattern, rng, min_length, safe_max);

            let format_matches_pattern = matches_pattern_strict(pattern, &format_value);
            let pattern_matches_format = format_output_looks_like(format, &pattern_value);

            if format_matches_pattern {
                return Value::String(format_value);
            }
            if pattern_matches_format {
                return Value::String(pattern_value);
            }
            let path = rng.current_path();
            rng.set_error(crate::FakerError::SchemaError {
                path,
                message: format!(
                    "format '{}' output doesn't match pattern '{}'",
                    format, pattern
                ),
            });
            return Value::String(pattern_value);
        }
        if opts.fail_on_invalid_format {
            rng.set_error(crate::FakerError::UnknownRegistryKey {
                name: format.to_string(),
                path: rng.current_path(),
            });
        }
    }

    if let Some(format) = format_opt {
        if formats::is_known_format(format) {
            let s =
                formats::generate_for_format_with_options(format, rng, Some(schema), Some(opts));
            if max_length_opt.is_none() && min_length == 0 {
                return Value::String(s);
            }
            return Value::String(pad_or_trim(s, min_length, safe_max, rng));
        }
        if opts.fail_on_invalid_format {
            rng.set_error(crate::FakerError::UnknownRegistryKey {
                name: format.to_string(),
                path: rng.current_path(),
            });
        }
    }

    if let Some(pattern) = pattern_opt {
        let s = generate_from_pattern(pattern, rng, min_length, safe_max);
        let len = s.chars().count();
        let in_range = len >= min_length && len <= safe_max;
        let matches = matches_pattern_strict(pattern, &s);
        if !in_range && matches && len < min_length {
            if let Some(padded) = pad_pattern_output(&s, pattern, min_length, safe_max, rng) {
                return Value::String(padded);
            }
        }
        if !in_range || !matches {
            if let Some(example) = schema_example_string(schema) {
                return Value::String(example);
            }
            let path = rng.current_path();
            rng.set_error(crate::FakerError::SchemaError {
                path,
                message: "Given sample does not match schema".to_string(),
            });
        }
        return Value::String(s);
    }

    let len = if min_length == safe_max {
        min_length
    } else {
        rng.int(min_length as i64, safe_max as i64) as usize
    };
    if safe_max == 0 && min_length == 0 {
        return Value::String(String::new());
    }
    Value::String(random_alphanumeric(rng, len.max(1)))
}

/// Returns the schema's `example` value as a string, or the first string entry
/// from its `examples` array if `example` is absent.
///
/// Used as a fallback when dynamic generation cannot produce a value that
/// satisfies the schema's `pattern`: rather than emit a value that violates
/// the regex, prefer any author-supplied example so the response remains
/// schema-conformant. Returns `None` when neither key is present or neither
/// carries a string value.
fn schema_example_string(schema: &Value) -> Option<String> {
    if let Some(Value::String(s)) = schema.get("example") {
        return Some(s.clone());
    }
    if let Some(Value::Array(arr)) = schema.get("examples") {
        for entry in arr {
            if let Value::String(s) = entry {
                return Some(s.clone());
            }
        }
    }
    None
}

/// Returns true when `value` plausibly satisfies the loose shape constraint implied by `format`.
///
/// Used when a schema declares both `format` and `pattern`: this lets the pattern path's
/// output be accepted when it happens to also look like the requested format, so we avoid
/// failing the more specific pattern constraint when the two are compatible. The checks are
/// intentionally permissive — they only reject values that obviously violate the named
/// format. Unknown formats return `true` so the pattern path wins.
fn format_output_looks_like(format: &str, value: &str) -> bool {
    match format {
        "email" | "idn-email" | "free-email" => value.contains('@'),
        "uri" | "url" | "iri" | "uri-reference" | "iri-reference" => {
            value.contains(':') || value.starts_with('/')
        }
        "uuid" | "uuid1" | "uuid3" | "uuid4" | "uuid5" | "guid" => {
            value.matches('-').count() >= 4 && value.len() >= 32
        }
        "ipv4" => value.split('.').count() == 4,
        "ipv6" => value.contains(':'),
        "hostname" | "idn-hostname" | "domain" | "domain-name" => value.contains('.'),
        "date" | "iso-date" | "full-date" => value.matches('-').count() >= 2,
        "time" | "partial-time" => value.contains(':'),
        "date-time" | "datetime" | "iso-date-time" => value.contains('T') || value.contains(' '),
        "json-pointer" => value.starts_with('/') || value.is_empty(),
        "hex-color" => value.starts_with('#'),
        _ => true,
    }
}

/// Pads a pattern-derived string up to `min_length` while preserving the pattern match.
///
/// When a pattern such as `^` or `^foo` legitimately admits longer strings but the
/// generator emits a short sample that falls below `minLength`, this helper appends
/// alphanumeric characters one at a time and re-checks the anchored pattern after each
/// append. Returns `Some(padded)` once the result lies within `[min_length, max_length]`
/// and still matches the pattern, or `None` when no amount of single-character padding
/// can satisfy both constraints (for example a `^$` pattern that forbids any non-empty
/// suffix). The bounded retry loop guards against pathological patterns.
fn pad_pattern_output(
    base: &str,
    pattern: &str,
    min_length: usize,
    max_length: usize,
    rng: &mut Random,
) -> Option<String> {
    if max_length < min_length {
        return None;
    }
    let chars: Vec<char> = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789"
        .chars()
        .collect();
    let mut candidate = base.to_string();
    let mut safety = 0usize;
    let cap = min_length.saturating_add(64);
    while candidate.chars().count() < min_length && safety < cap {
        safety += 1;
        let pick = *rng.pick_char(&chars);
        let mut next = candidate.clone();
        next.push(pick);
        if next.chars().count() > max_length {
            return None;
        }
        if matches_pattern_strict(pattern, &next) {
            candidate = next;
        } else {
            return None;
        }
    }
    let len = candidate.chars().count();
    if len >= min_length && len <= max_length && matches_pattern_strict(pattern, &candidate) {
        Some(candidate)
    } else {
        None
    }
}

/// Adjusts a generated string to satisfy `minLength` and `maxLength` constraints.
fn pad_or_trim(s: String, min: usize, max: usize, rng: &mut Random) -> String {
    let chars: Vec<char> = s.chars().collect();
    let len = chars.len();
    if len >= min && len <= max {
        return s;
    }
    if len > max {
        return chars[..max].iter().collect();
    }
    let extra: String = random_alphanumeric(rng, min - len);
    format!("{}{}", s, extra)
}

/// Generates a random alphanumeric string of the given length.
///
/// Intentionally restricted to the ASCII alphabet `[a-zA-Z0-9]` so that
/// generated values are predictable across runs, safely embeddable into URLs
/// and headers without percent-encoding surprises, and trivially comparable in
/// snapshot tests. Spec-mandated `pattern` constraints, when present, still
/// override this generator via `generate_from_pattern`, so a spec author who
/// needs Unicode coverage can demand it explicitly through `pattern`.
///
/// R8C noted that the default `type: string` faker output therefore never
/// exercises Latin Extended, CJK, or emoji codepoints, which is a real gap
/// for clients claiming Unicode-safety. The fix shape is to gate non-ASCII
/// coverage behind an opt-in flag rather than flipping the default and
/// breaking every existing snapshot test downstream.
///
/// Future work: opt-in Unicode coverage via a `GenerateOptions.unicode_strings` flag
/// that swaps this ASCII alphabet for a Unicode-aware sampler. Held back because
/// flipping the default would break every downstream snapshot test that currently
/// asserts ASCII-only output; the flag must be additive rather than substitutive.
fn random_alphanumeric(rng: &mut Random, len: usize) -> String {
    let chars: Vec<char> = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789"
        .chars()
        .collect();
    (0..len).map(|_| *rng.pick_char(&chars)).collect()
}

/// Generates a string that satisfies the given regex pattern with length constraints.
///
/// First attempts to use `rand_regex` to parse the pattern's AST and sample a matching
/// string directly. Up to ten samples are taken, accepting the first that satisfies the
/// length window and the anchored pattern. When `rand_regex` cannot compile the pattern,
/// or none of its samples satisfy the constraints, falls back to a hand-rolled regex
/// generator that handles a useful subset of syntax. As a last resort, returns a
/// pattern-best-effort string padded or trimmed to fit the length window.
pub fn generate_from_pattern(
    pattern: &str,
    rng: &mut Random,
    min_length: usize,
    max_length: usize,
) -> String {
    let effective_max = if max_length == 0 {
        usize::MAX
    } else {
        max_length
    };
    let (safe_pattern, clamped_high) = clamp_pattern_quantifiers(pattern);
    if clamped_high {
        let path = rng.current_path();
        rng.set_error(crate::FakerError::SchemaError {
            path,
            message: format!(
                "regex quantifier in '{}' capped to {} for safety",
                pattern, MAX_REGEX_REPEAT
            ),
        });
    }
    if let Some(s) = try_rand_regex(&safe_pattern, rng, min_length, effective_max) {
        return s;
    }
    for _ in 0..10 {
        let stripped = safe_pattern.trim_start_matches('^').trim_end_matches('$');
        let result = generate_from_pattern_inner(stripped, rng);
        let len = result.chars().count();
        if len >= min_length
            && len <= effective_max
            && matches_pattern_strict(&safe_pattern, &result)
        {
            return result;
        }
    }
    let stripped = safe_pattern.trim_start_matches('^').trim_end_matches('$');
    let result = generate_from_pattern_inner(stripped, rng);
    let chars: Vec<char> = result.chars().collect();
    let len = chars.len();
    if len > effective_max && effective_max > 0 && effective_max < usize::MAX {
        let trimmed: String = chars[..effective_max].iter().collect();
        if validate_pattern(&trimmed, &safe_pattern) {
            return trimmed;
        }
        return chars[..effective_max.min(len)].iter().collect();
    }
    if len < min_length {
        let extra = random_alphanumeric(rng, min_length - len);
        return format!("{}{}", chars.iter().collect::<String>(), extra);
    }
    chars.iter().collect()
}

/// Rewrites every `{n}` and `{n,m}` quantifier in a regex pattern so neither bound exceeds `MAX_REGEX_REPEAT`.
///
/// Returns the rewritten pattern and a boolean indicating whether any quantifier was actually clamped.
/// Escapes inside character classes are left untouched; only top-level quantifiers are rewritten.
fn clamp_pattern_quantifiers(pattern: &str) -> (String, bool) {
    let chars: Vec<char> = pattern.chars().collect();
    let mut out = String::with_capacity(pattern.len());
    let mut clamped = false;
    let mut i = 0usize;
    while i < chars.len() {
        let ch = chars[i];
        if ch == '\\' && i + 1 < chars.len() {
            out.push(ch);
            out.push(chars[i + 1]);
            i += 2;
            continue;
        }
        if ch == '{' {
            if let Some(rel_close) = chars[i..].iter().position(|&c| c == '}') {
                let inner: String = chars[i + 1..i + rel_close].iter().collect();
                let trimmed = inner.trim();
                let parts: Vec<&str> = trimmed.split(',').collect();
                let parsed: Vec<Option<usize>> = parts
                    .iter()
                    .map(|p| {
                        let t = p.trim();
                        if t.is_empty() {
                            None
                        } else {
                            t.parse::<usize>().ok()
                        }
                    })
                    .collect();
                let is_numeric_quantifier = !parts.is_empty()
                    && parts.iter().all(|p| {
                        let t = p.trim();
                        t.is_empty() || t.parse::<usize>().is_ok()
                    })
                    && parsed.iter().any(|v| v.is_some());
                if is_numeric_quantifier {
                    let rewritten = match parsed.as_slice() {
                        [Some(n)] => {
                            if *n > MAX_REGEX_REPEAT {
                                clamped = true;
                                format!("{{{}}}", MAX_REGEX_REPEAT)
                            } else {
                                format!("{{{}}}", n)
                            }
                        }
                        [Some(lo), None] => {
                            if *lo > MAX_REGEX_REPEAT {
                                clamped = true;
                                format!("{{{},}}", MAX_REGEX_REPEAT)
                            } else {
                                format!("{{{},}}", lo)
                            }
                        }
                        [Some(lo), Some(hi)] => {
                            let lo_c = (*lo).min(MAX_REGEX_REPEAT);
                            let hi_c = (*hi).min(MAX_REGEX_REPEAT).max(lo_c);
                            if *lo > MAX_REGEX_REPEAT || *hi > MAX_REGEX_REPEAT {
                                clamped = true;
                            }
                            format!("{{{},{}}}", lo_c, hi_c)
                        }
                        _ => format!("{{{}}}", inner),
                    };
                    out.push_str(&rewritten);
                    i += rel_close + 1;
                    continue;
                }
            }
        }
        out.push(ch);
        i += 1;
    }
    (out, clamped)
}

/// Attempts to compile and sample a regex via `rand_regex`, returning the first sample
/// that satisfies the anchored pattern and the `[min_length, max_length]` window.
///
/// Returns `None` when the pattern fails to compile, or when none of the attempted
/// samples satisfy both the regex and the length bounds within ten tries.
fn try_rand_regex(
    pattern: &str,
    rng: &mut Random,
    min_length: usize,
    max_length: usize,
) -> Option<String> {
    let anchored = anchor_pattern(pattern);
    let parsed = cached_rand_regex(&anchored)?;
    for _ in 0..10 {
        let sample: String = parsed.sample(rng.inner());
        let len = sample.chars().count();
        if len >= min_length && len <= max_length && matches_pattern_strict(pattern, &sample) {
            return Some(sample);
        }
    }
    None
}

/// Returns the pattern anchored at both ends with `^` and `$` for compilation.
fn anchor_pattern(pattern: &str) -> String {
    let mut s = String::with_capacity(pattern.len() + 2);
    if !pattern.starts_with('^') {
        s.push('^');
    }
    s.push_str(pattern);
    if !pattern.ends_with('$') {
        s.push('$');
    }
    s
}

/// Checks if a string approximately matches a pattern using the regex crate.
fn validate_pattern(s: &str, pattern: &str) -> bool {
    match cached_regex(pattern) {
        Some(re) => re.is_match(s),
        None => true,
    }
}

/// Returns true when the given pattern compiles and the value matches it under anchored semantics.
fn matches_pattern_strict(pattern: &str, value: &str) -> bool {
    let anchored = if pattern.starts_with('^') {
        pattern.to_string()
    } else {
        format!("^{}", pattern)
    };
    let anchored = if anchored.ends_with('$') {
        anchored
    } else {
        format!("{}$", anchored)
    };
    match cached_regex(&anchored) {
        Some(re) => re.is_match(value),
        None => false,
    }
}

/// Core recursive pattern generator that handles a subset of regex syntax.
fn generate_from_pattern_inner(pattern: &str, rng: &mut Random) -> String {
    let mut result = String::new();
    let chars: Vec<char> = pattern.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let ch = chars[i];
        if ch == '(' {
            let end = find_group_end(&chars, i);
            let group_content: String = chars[i + 1..end].iter().collect();
            let alternatives: Vec<&str> = split_alternation(&group_content);
            let picked_idx = rng.pick_index(alternatives.len());
            let picked = alternatives[picked_idx];
            let sub = generate_from_pattern_inner(picked, rng);
            i = end + 1;
            let quantifier_result = handle_quantifier(&chars, i, &sub, rng);
            result.push_str(&quantifier_result.0);
            i += quantifier_result.1;
            continue;
        }
        if ch == '[' {
            let end = find_class_end(&chars, i);
            let class_str: String = chars[i + 1..end].iter().collect();
            let char_set = expand_char_class(&class_str);
            i = end + 1;
            let unit = pick_from_set(&char_set, rng);
            let quantifier_result = handle_quantifier(&chars, i, &unit.to_string(), rng);
            result.push_str(&quantifier_result.0);
            i += quantifier_result.1;
            continue;
        }
        if ch == '\\' && i + 1 < chars.len() {
            let escaped = chars[i + 1];
            let unit = match escaped {
                'd' => {
                    let digits: Vec<char> = "0123456789".chars().collect();
                    rng.pick_char(&digits).to_string()
                }
                'D' => {
                    let non_digits: Vec<char> = "abcdefghijklmnopqrstuvwxyz".chars().collect();
                    rng.pick_char(&non_digits).to_string()
                }
                'w' => {
                    let word_chars: Vec<char> =
                        "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789_"
                            .chars()
                            .collect();
                    rng.pick_char(&word_chars).to_string()
                }
                'W' => {
                    let non_word: Vec<char> = "!@#$%^&*()-=+[]{}|;':\",./<>?".chars().collect();
                    rng.pick_char(&non_word).to_string()
                }
                's' => " ".to_string(),
                'S' => {
                    let non_space: Vec<char> = "abcdefghijklmnopqrstuvwxyz".chars().collect();
                    rng.pick_char(&non_space).to_string()
                }
                'n' => "\n".to_string(),
                't' => "\t".to_string(),
                _ => escaped.to_string(),
            };
            i += 2;
            let quantifier_result = handle_quantifier(&chars, i, &unit, rng);
            result.push_str(&quantifier_result.0);
            i += quantifier_result.1;
            continue;
        }
        if ch == '.' {
            let all_chars: Vec<char> =
                "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789"
                    .chars()
                    .collect();
            let unit = rng.pick_char(&all_chars).to_string();
            i += 1;
            let quantifier_result = handle_quantifier(&chars, i, &unit, rng);
            result.push_str(&quantifier_result.0);
            i += quantifier_result.1;
            continue;
        }
        if ch == '^' || ch == '$' {
            i += 1;
            continue;
        }
        if ch == '|' {
            break;
        }
        let unit = ch.to_string();
        i += 1;
        let quantifier_result = handle_quantifier(&chars, i, &unit, rng);
        result.push_str(&quantifier_result.0);
        i += quantifier_result.1;
    }
    result
}

/// Finds the matching closing parenthesis for a group starting at `start`.
fn find_group_end(chars: &[char], start: usize) -> usize {
    let mut depth = 0;
    for (i, &ch) in chars.iter().enumerate().skip(start) {
        if ch == '(' {
            depth += 1;
        } else if ch == ')' {
            depth -= 1;
            if depth == 0 {
                return i;
            }
        }
    }
    chars.len() - 1
}

/// Finds the matching closing bracket `]` for a character class starting at `start`.
fn find_class_end(chars: &[char], start: usize) -> usize {
    for (i, &ch) in chars.iter().enumerate().skip(start + 1) {
        if ch == ']' {
            return i;
        }
    }
    chars.len() - 1
}

/// Splits a pattern string on top-level `|` characters, respecting nested groups.
fn split_alternation(pattern: &str) -> Vec<&str> {
    let mut result = Vec::new();
    let mut depth = 0;
    let mut start = 0;
    for (i, ch) in pattern.char_indices() {
        match ch {
            '(' => depth += 1,
            ')' => depth -= 1,
            '|' if depth == 0 => {
                result.push(&pattern[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    result.push(&pattern[start..]);
    result
}

/// Expands a character class string like `a-z0-9` into its constituent characters.
fn expand_char_class(class_str: &str) -> Vec<char> {
    let mut chars = Vec::new();
    let negated = class_str.starts_with('^');
    let content = if negated { &class_str[1..] } else { class_str };
    let class_chars: Vec<char> = content.chars().collect();
    let mut i = 0;
    while i < class_chars.len() {
        if i + 2 < class_chars.len() && class_chars[i + 1] == '-' {
            let start_char = class_chars[i];
            let end_char = class_chars[i + 2];
            for c in start_char..=end_char {
                chars.push(c);
            }
            i += 3;
        } else if class_chars[i] == '\\' && i + 1 < class_chars.len() {
            match class_chars[i + 1] {
                'd' => chars.extend('0'..='9'),
                'w' => {
                    chars.extend('a'..='z');
                    chars.extend('A'..='Z');
                    chars.extend('0'..='9');
                    chars.push('_');
                }
                's' => chars.push(' '),
                _ => chars.push(class_chars[i + 1]),
            }
            i += 2;
        } else {
            chars.push(class_chars[i]);
            i += 1;
        }
    }
    if negated || chars.is_empty() {
        if negated {
            let all: Vec<char> = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789"
                .chars()
                .collect();
            return all.into_iter().filter(|c| !chars.contains(c)).collect();
        }
        "abcdefghijklmnopqrstuvwxyz".chars().collect()
    } else {
        chars
    }
}

/// Picks a random character from a non-empty character set.
fn pick_from_set(set: &[char], rng: &mut Random) -> char {
    if set.is_empty() {
        return 'a';
    }
    *rng.pick_char(set)
}

/// Processes the quantifier at `chars[pos]` and returns `(expanded_string, chars_consumed)`.
fn handle_quantifier(chars: &[char], pos: usize, unit: &str, rng: &mut Random) -> (String, usize) {
    if pos >= chars.len() {
        return (unit.to_string(), 0);
    }
    match chars[pos] {
        '*' => {
            let count = rng.int(0, 5) as usize;
            (unit.repeat(count), 1)
        }
        '+' => {
            let count = rng.int(1, 5) as usize;
            (unit.repeat(count), 1)
        }
        '?' => {
            let count = rng.int(0, 1) as usize;
            (unit.repeat(count), 1)
        }
        '{' => {
            let close = chars[pos..].iter().position(|&c| c == '}');
            if let Some(rel_close) = close {
                let inner: String = chars[pos + 1..pos + rel_close].iter().collect();
                let consumed = rel_close + 1;
                if let Some(comma_pos) = inner.find(',') {
                    let min_str = &inner[..comma_pos];
                    let max_str = &inner[comma_pos + 1..];
                    let raw_min_n: usize = min_str.trim().parse().unwrap_or(1);
                    let raw_max_n: usize = if max_str.trim().is_empty() {
                        raw_min_n.saturating_add(5)
                    } else {
                        max_str
                            .trim()
                            .parse()
                            .unwrap_or(raw_min_n.saturating_add(5))
                    };
                    if raw_min_n > MAX_REGEX_REPEAT || raw_max_n > MAX_REGEX_REPEAT {
                        let path = rng.current_path();
                        rng.set_error(crate::FakerError::SchemaError {
                            path,
                            message: format!(
                                "regex quantifier {{{},{}}} capped to {} for safety",
                                raw_min_n, raw_max_n, MAX_REGEX_REPEAT
                            ),
                        });
                    }
                    let min_n = raw_min_n.min(MAX_REGEX_REPEAT);
                    let max_n = raw_max_n.min(MAX_REGEX_REPEAT).max(min_n);
                    let count = rng.int(min_n as i64, max_n as i64) as usize;
                    return (unit.repeat(count), consumed);
                } else {
                    let raw_exact: usize = inner.trim().parse().unwrap_or(1);
                    if raw_exact > MAX_REGEX_REPEAT {
                        let path = rng.current_path();
                        rng.set_error(crate::FakerError::SchemaError {
                            path,
                            message: format!(
                                "regex quantifier {{{}}} capped to {} for safety",
                                raw_exact, MAX_REGEX_REPEAT
                            ),
                        });
                    }
                    let exact = raw_exact.min(MAX_REGEX_REPEAT);
                    return (unit.repeat(exact), consumed);
                }
            }
            (unit.to_string(), 0)
        }
        _ => (unit.to_string(), 0),
    }
}
