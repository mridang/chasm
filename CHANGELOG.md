# Changelog

All notable changes to chasm are documented in this file. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and this project
adheres to [Semantic Versioning](https://semver.org/). Pre-1.0 releases
treat a minor-version bump (0.1 → 0.2) as the breaking-change signal.

## [0.2.0] - 2026-05-22

### Breaking changes — chasm-engine

- `MockError::Generation(FakerError)` is now a struct variant
  `Generation { method, path, source }`. Pattern-match callers must migrate
  from `MockError::Generation(e)` to
  `MockError::Generation { source: e, .. }`. The `method` and `path` fields
  now carry the request context, which previously had to be reconstructed
  by the caller.
- `SpecError::Io(String)` is now `SpecError::Io { path, source: std::io::Error }`.
  The variant now preserves the live `std::io::Error` rather than its
  stringified form, restoring access to `Error::source()`.
- `MockConfig`, `MockRequest`, and `MockResponse` are now `#[non_exhaustive]`.
  External callers must construct these types via `T::default()` + field
  mutation rather than struct-literal syntax. The change lets us add
  optional fields non-breakingly in future releases.
- `validate_request` has been removed; use `validate_request_full` instead.
  The new signature takes an additional `path_template: Option<&str>`
  argument so path-item-level shared parameters are not silently dropped.
- `match_operation` has been removed from the public router API. Use
  `route_request` or `route_request_with_strict` and pattern-match on
  `RouteMatch::Operation` to obtain the `MatchedOperation`.
- The `Prefer` header is now case-insensitive per RFC 7240 §2.
  `Prefer: Code=404` (uppercase) is now respected; previously silently
  dropped.

### Breaking changes — chasm-faker

- `FakerError::SchemaError(String)` is now a struct variant
  `SchemaError { path, message }`. The new `path` field carries the JSON
  pointer to the offending schema node.
- `FakerError::AllBranchesFailed.last_error` is now wired to
  `Error::source()` via `#[source]`. Downstream `e.source()` chain
  walkers now traverse into the inner `FakerError`.

### Breaking changes — chasm-wasm

- Problem document `type` URIs are now full RFC 7807 URIs
  (`https://chasm.dev/errors#NO_PATH_MATCHED_ERROR`) instead of bare
  slugs (`NO_PATH_MATCHED_ERROR`). JS callers that switch on `type`
  must update their pattern set.
- `MockError::NoResponseDefined` now emits `type: "https://chasm.dev/errors#NO_RESPONSE_DEFINED"`
  rather than collapsing under `NO_RESPONSE_RESPONSE_DEFINED`.
- `MockError::SpecSerialization` now emits `type: "https://chasm.dev/errors#INTERNAL_SERVER_ERROR"`
  rather than collapsing under `NO_RESPONSE_RESPONSE_DEFINED`.

### Breaking changes — chasm-server

- Problem document `type` URIs moved from
  `https://stoplight.io/prism/errors#...` to
  `https://chasm.dev/errors#...`. Any client switching on `type` must
  update its URI set. The slug portion (`NO_PATH_MATCHED_ERROR`,
  `UNPROCESSABLE_ENTITY`, etc.) is preserved verbatim.

### Behaviour changes

- 204 No Content, 304 Not Modified, 205 Reset Content, and 100-199
  Informational responses now strictly suppress body and `Content-Type` /
  `Content-Length` headers per RFC 9110 §15.2 / §15.3.5 / §15.3.6 / §15.4.5.
  Previously chasm-server emitted `"null"` (the JSON string) with
  `Content-Type: application/json` on these statuses.
- WASM error envelopes now include the RFC 7807 `instance` field
  (`urn:chasm:wasm:<uuid>`).
- `--errors` validation diagnostics emitted when validation is OFF
  (server-wide default) have been demoted from `WARN` to a single
  aggregated `DEBUG` event per request. This closes a log-flood DoS
  vector at default log level.
- WASM consumers can now construct `Chasm` from JSON or YAML spec
  strings (previously WASM-side only accepted JSON).

### Added

- `chasm_spec_reload_failures_total` Prometheus counter for `--watch`
  hot-reload health.
- Cosign keyless signing wired into the release pipeline for OCI image
  artefacts. SBOM attestation generation pending.
- CI feature-flag matrix in `test.yml` (placeholder rows currently
  exercise the same code due to no in-tree `[features]` blocks).
- `byte` format validation now enforces RFC 4648 base64 alphabet
  (previously silently accepted any string).
- `int32` format on integer schemas now enforces `[-2^31, 2^31-1]`
  range (previously silently accepted any `i64`).
- Operation-level and path-item-level `servers[]` overrides are now
  honoured by the router.
- `style: deepObject` query parameters with `?filter[name]=value` form
  are now decoded and validated.
- Response headers declared with only a `schema` (no `example`) now
  fall through to dynamic generation instead of being silently dropped.
- `propertyNames` schema now honours `enum`, `const`, and `format`
  keywords in addition to `pattern`/`minLength`/`maxLength`.

### Fixed

- `MockConfig.errors == false` (the default) no longer runs the full
  validation pipeline on every request; an early-exit gate skips when
  the operation declares no parameters and no request body.
- The walker's `prop_aliases` cycle (e.g. `{"a":"b","b":"a"}`) no
  longer stack-overflows; alias rewrites are now applied in a single
  fixed-point pass.
- `if`/`then`/`else` evaluation correctly fails when an `if` schema
  declares `properties` or `required` and the candidate value is not
  an object. Previously the `if` evaluated as vacuously true.
- `unevaluatedProperties: false` no longer drops keys evaluated by
  `allOf` / `oneOf` / `anyOf` / `if-then-else` branches.
- `Cargo.lock` now resolves a single major for `thiserror` (2.x) and
  `tower-http` (0.6.x). Previously both 1.x/2.x and 0.5/0.6 coexisted.

### Removed

- `chasm_faker` dependency on direct `indexmap` (was unused).
- `chasm_faker` dependency on direct `serde` (transitively available
  via `serde_json`).
- The `fake-color` / `fake-http` / `pydantic` feature flags declared
  but never gated. CI matrix entries for these have been removed.

### Migration

Most upgrades require only the struct-pattern rewrites described above.
The most common changes for library callers:

```rust
// before
let req = MockRequest { method: "GET".into(), path: "/pets".into(),
    headers, query: HashMap::new(), body: None };

// after
let mut req = MockRequest::default();
req.method = "GET".into();
req.path = "/pets".into();
req.headers = headers;
```

```rust
// before
match err {
    MockError::Generation(e) => log::warn!("faker: {}", e),
    ...
}

// after
match err {
    MockError::Generation { source, method, path } =>
        log::warn!("faker for {} {}: {}", method, path, source),
    ...
}
```

For JS callers consuming chasm-wasm error envelopes:
```js
// before
if (err.type === "NO_PATH_MATCHED_ERROR") { ... }

// after
if (err.type === "https://chasm.dev/errors#NO_PATH_MATCHED_ERROR") { ... }
```

For wire-protocol clients of chasm-server problem documents: the slug
portion of `type` URIs (`NO_PATH_MATCHED_ERROR`, `UNPROCESSABLE_ENTITY`,
etc.) is preserved, so a substring match on the slug continues to work.
The base URI changed from `stoplight.io/prism` to `chasm.dev`.

## [0.1.0] - initial release
