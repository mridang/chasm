# Scope

chasm is an OAS3 mock server built around a `json-schema-faker`-style
generator. It is intentionally narrower than a fully-featured API gateway.

## In scope

These are implemented today and considered stable.

- **OAS3 mock server.** `chasm-server` exposes a CLI with `--port`, `--host`,
  `--dynamic`, `--cors`, `--cors-origin`, `--cors-credentials`,
  `--cors-max-age`, `--cors-expose-headers`, `--errors`, `--seed`,
  `--ignore-examples`, `--json-schema-faker-fill-properties`, `--verbose`,
  `--watch`, `--dry-run`, `--expose-spec`, `--max-connections`,
  `--strict-method-matching`, `--tls-cert`, `--tls-key`, `--tls-port`,
  `--log-format`, and `--request-timeout`.

  **`--expose-spec` disclosure risk.** `--expose-spec` defaults to `true`,
  which mounts `/openapi.{json,yaml}` and world-publishes the loaded spec
  including any inline `example` and `description` blocks. Spec authors
  frequently embed sample credentials, internal hostnames, and PII in
  `example` blocks — review your spec before exposing it on a public
  endpoint, or set `--expose-spec=false` for production-facing deployments.
- **HTTPS termination.** Supply `--tls-cert` and `--tls-key` (PEM-encoded
  chain + key) to terminate TLS in-process via `rustls`; both flags must
  be set together or both omitted. When TLS is enabled, chasm serves plain
  HTTP on `--port` and HTTPS on `--tls-port` (default `8443`)
  simultaneously, so a single process answers both schemes; the two ports
  must differ.
- **HTTP/2.** Negotiated automatically via ALPN when TLS is enabled. No
  flag required.
- **`validate` subcommand.** `chasm-server validate <SPEC>` parses the
  spec, runs the same structural checks the server runs on startup, and
  exits non-zero with a human-readable diagnostic if the spec cannot be
  served. Useful as a pre-commit / CI gate.
- **`--watch` for spec hot-reload.** The server watches the spec file (and
  any sibling files in the same directory for filesystems that move-on-save)
  and rebuilds its in-memory routing/validation tables on change without
  dropping the listener.
- **`--dry-run`.** Loads and validates the spec, prints a summary of
  discovered operations and the chosen bind address, then exits without
  binding. Pairs naturally with `--watch` in editor integrations.
- **Stdin spec loading via `-`.** Passing `-` as the spec path reads the
  spec from stdin. Works with both JSON and YAML; the parser sniffs the
  first non-whitespace byte.
- **Spec loading.** JSON and YAML, parsed via `openapiv3`. Basic HTTPS
  `$ref` resolution for single-hop remote references; see *Deferred* for
  the full remote resolver caveat.

  **Remote `$ref` non-determinism.** When a spec uses remote (`https://`)
  `$ref`s, chasm fetches and inlines them at load time. The wall-clock
  budget (30s default), per-fetch timeout (5s), and the fetched content
  itself can vary across runs, so seeded generation against specs with
  remote `$ref`s is NOT byte-deterministic across separate `load_spec`
  invocations. For seed-stable output, pre-resolve remote `$ref`s with
  a bundler (e.g. `redocly bundle`) before feeding the spec to chasm.
- **Routing.** Path-template matching with parameters, method selection,
  and `405 Method Not Allowed` / `404` differentiation.
- **Authority-side server variable expansion.** `servers[].variables` are
  expanded into the effective base path so operations declared under
  e.g. `https://{env}.example.com/{basePath}` route correctly regardless of
  the variable values supplied.
- **Content negotiation.** Parses the `Accept` header with `q=` factors,
  handles `*/*` and `type/*` wildcards, tolerates spec content keys with
  `; charset=...` parameters, and skips malformed media keys. Responses
  include a `Vary: Accept` header (plus any other negotiation axes used)
  so caches do not collapse representations.
- **Response status selection.** Lowest numeric `2xx`, then `default`,
  then first response, then synthetic `200`. Honours `2XX`/`4XX`/`5XX`
  range keys.
- **Example pipeline.** Named example via `Prefer: example=`, first entry
  in `examples`, inline `example`, then `schema.example`. Resolves
  `#/components/examples/X` references.
- **Schema-driven generation.** Via `chasm-faker`. Honours `--dynamic`,
  `--seed`, and per-request `Prefer: dynamic=`/`seed=`.
- **`x-faker` per-property hint.** Schemas can attach an
  `x-faker: "<namespace.method>"` string to a property to dispatch
  through the faker namespace (mirroring `json-schema-faker`'s hint).
  When the key names a recognised faker namespace (`name.fullName`,
  `internet.email`, ...), the walker invokes the registered `faker`
  extension; otherwise it logs a warning and falls through to
  type-based generation. Pairs with the top-level
  `x-json-schema-faker` config and the `faker:` keyword.
- **Discriminator-aware `oneOf`/`anyOf` payloads.** When a composition
  schema declares a `discriminator`, the faker picks a branch consistent
  with the discriminator mapping and seeds the discriminator property
  value to match, so generated payloads round-trip through strict client
  decoders.
- **`nullable: true` probabilistic null generation.** Properties marked
  `nullable: true` (OAS 3.0) are emitted as `null` a configurable fraction
  of the time during dynamic generation, instead of always being filled.
- **OAS 3.1 `type: ["X", "null"]` normalisation.** Union-with-null type
  arrays are normalised to the same internal representation as
  `nullable: true`, so 3.0 and 3.1 specs behave identically.
- **Strict `format:` validation.** The request validator enforces `email`,
  `uri`, `uuid`, `date-time`, `date`, `time`, `ipv4`, `ipv6`, and
  `hostname` formats on string-typed parameters and body fields.
- **`dependentRequired` / `dependentSchemas` validation.** Both keywords
  are evaluated during request body validation, producing structured
  diagnostics on failure.
- **`not` constraint validation.** A subschema under `not` that matches
  the candidate value causes the candidate to be rejected with a
  diagnostic pointing at the offending field.
- **Pydantic v2 string formats.** The faker recognises `condate`,
  `condecimal`, `aware-datetime`, `naive-datetime`, `aware-date`,
  `naive-date`, `name-email`, and `json-string` formats and emits values
  Pydantic will accept without coercion warnings.
- **`Prefer` header.** `code=`, `example=`, `dynamic=`, `seed=`,
  `validate=`, and `security=`. The query-parameter form (`__code`,
  `__example`, `__dynamic`, `__seed`, `__validate`, `__security`) wins
  on conflict.
- **Response headers.** Spec-defined response headers are emitted with
  values from inline `example` or `schema.example`. Hop-by-hop and framing
  headers (`Content-Encoding`, `Content-Length`, `Transfer-Encoding`,
  `Date`, `Connection`) are stripped because chasm does not compute them.
- **`X-Request-ID` propagation.** Incoming `X-Request-ID` headers are
  echoed back on the response; when absent, a fresh UUIDv4 is generated
  and returned. The same id is attached to the structured log fields for
  every line emitted while handling the request.
- **WireMock-parity transport behaviours.** Three `x-chasm-*` extensions
  let a spec drive transport-layer behaviour that SDK test harnesses
  otherwise need a second mock for:
  - **`x-chasm-delay-ms: <int>`** — sleeps for the given milliseconds
    before responding (an async, non-blocking delay). Declared on the
    operation or the response object; the operation-level value wins.
  - **`x-chasm-content-encoding: gzip|br|zstd`** — compresses the
    response body with the named codec and sets `Content-Encoding`
    accordingly. Declared on the response object. Any other value is
    ignored. This is the one case where chasm computes `Content-Encoding`
    itself rather than stripping it (see *Response headers* above), and it
    skips the negotiated compression layer to avoid double-encoding.
  - **`x-chasm-echo: true`** — replaces the response body with a JSON
    envelope reflecting the incoming request (method, path, headers,
    cookies, body, content length). Declared on the operation.
- **Multi-value response headers.** A response header whose `example` (or
  `schema.example`) is a JSON array is emitted as one header line per
  array element, so `Set-Cookie` / `Link` style multi-valued headers
  round-trip faithfully.
- **Faithful status & body emission.** A single declared non-`2xx`
  response status is emitted verbatim (e.g. a `302` with a `Location`
  header), `text/*` bodies are emitted as raw text rather than
  JSON-quoted, and bodyless statuses (`1xx`, `204`, `205`, `304`) send no
  body.
- **Request validation (`--errors` / `-e`).** Validates path, query, and
  header parameters as well as JSON request bodies against the operation's
  schemas, including the strict `format:`, `dependentRequired`,
  `dependentSchemas`, and `not` keywords above. Failures are returned as
  RFC 7807 `422 UNPROCESSABLE_ENTITY` envelopes whose `validation` entries
  use a structured diagnostic shape. Implemented in
  `crates/chasm-engine/src/validation.rs` and invoked from
  `engine.rs::mock()`.
- **Security / auth.** Operation-level `security` requirements are
  enforced: missing or malformed credentials yield `401 UNAUTHORIZED` with
  a `WWW-Authenticate` header derived from the declared scheme. Lives in
  `crates/chasm-engine/src/security.rs`.
- **Request size ceiling.** Request bodies that exceed the 16 MiB
  server-side limit are rejected with `413 PAYLOAD_TOO_LARGE`.
- **Named example lookup errors.** A `Prefer: example=<name>` (or
  `__example=<name>`) that does not match any example in the spec
  produces `404 NOT_FOUND`.
- **Error envelope.** RFC 7807 problem documents with stable `type` URIs:
  `NO_PATH_MATCHED_ERROR` (404), `NO_METHOD_MATCHED_ERROR` (405),
  `NOT_FOUND` (404), `NOT_ACCEPTABLE` (406), `UNAUTHORIZED` (401),
  `UNPROCESSABLE_ENTITY` (422), `PAYLOAD_TOO_LARGE` (413),
  `NO_RESPONSE_DEFINED` (404), `NO_RESPONSE_RESPONSE_DEFINED` (500, wire
  string preserved verbatim), `INTERNAL_SERVER_ERROR` (500), and
  `REQUEST_TIMEOUT` (408, fallback type URI for the request-timeout
  envelope).
- **Health endpoints.** `/healthz`, `/livez`, and `/readyz` are exposed
  outside the spec's path space. `/livez` reports process liveness;
  `/readyz` flips to `200` once the spec is loaded and routing tables
  are built (and back to `503` during a `--watch` reload).
- **Prometheus metrics.** `/metrics` exposes request counters, latency
  histograms, and faker/validation timings in the standard Prometheus
  text exposition format.
- **Tracing.** Deeper `tracing::debug!` instrumentation across the engine
  (routing, content negotiation, example pipeline, schema generation,
  validation) so `-vv` output is useful for triage rather than just noise.
- **CORS.** Permissive by default. Toggle with `--cors`.
- **WASM bindings.** `chasm-wasm` exposes the engine to browsers and Node
  via `wasm-bindgen`.

## Deferred

Implemented partially or as a stub; tracked work.

- **Dedicated static generation path.** chasm currently approximates a
  "static" mode by calling `chasm-faker` with `required_only: true` and
  `always_fake_optionals: false`. A real static generator returning
  type-default-shaped values (rather than randomised ones) is pending
  in `chasm-faker`.

## Out of scope

Explicit non-goals. No work is planned in this repository for any of
these; they are listed so users know not to file issues asking for them.

- **Proxy mode / `--upstream` / record-replay.** chasm is mock-only and
  will stay that way; use a dedicated reverse proxy in front if you need
  upstream forwarding or response capture.
- **Spectral linter integration.** chasm does not lint specs or surface
  ruleset violations. Run Spectral (or `redocly lint`) separately.
- **Per-project config file format.** All configuration is CLI flags and
  environment variables.
- **Callbacks (operation-level).** OpenAPI `callbacks` definitions are
  parsed but ignored; chasm will not initiate outbound HTTP.
- **Links (response chaining).** OpenAPI `links` between operations are
  parsed but not surfaced; clients that want to follow them must do so
  themselves.
- **JWT shape validation for `bearerFormat: JWT`.** A bearer token is
  accepted if present and well-formed as opaque credentials; chasm does
  not verify the JWT structure, signature, or claims.
- **NPM SDK publish.** `chasm-wasm` exists and is built in CI, but is not
  yet published to npm. Consume it from the built artefact or from a
  git submodule until that lands.
- **Spec-editor UI integration testing.** chasm is not exercised against
  any specific OpenAPI design tool's mocking UI; standards-conformant
  specs should work but this is not part of CI.
- **Matrix / label parameter styles.** OpenAPI's `matrix` and `label`
  path parameter styles are not implemented. chasm logs a `tracing::warn!`
  on first encounter of an unsupported style per `(operation, parameter)`
  pair; the parameter is then accepted as if `style: simple` were declared.
  The same accept-but-warn applies to query `pipeDelimited` and
  `spaceDelimited` styles, which fall through to the default form decoding.
- **Full remote `$ref` resolution.** Basic single-hop HTTPS `$ref`
  resolution is supported, but a complete remote resolver (with caching,
  file-scheme support, cycle detection across documents, and
  authentication) is out of scope. Use a bundling tool like
  `redocly bundle` to flatten specs that depend on it.
- **Contract testing harness.** Diffing real responses against the spec
  is the job of a separate tool; not part of chasm.
- **OAS2 / Swagger 2.0.** Only OpenAPI 3.0/3.1 (whatever `openapiv3`
  parses) is supported.
- **gRPC, GraphQL, AsyncAPI.** HTTP/REST only.
- **Persistent state.** Mock responses are stateless. POST/PUT/DELETE do
  not mutate anything; subsequent GETs do not see "created" resources.
- **Webhooks.** OAS 3.1's top-level `webhooks` keyword is not supported.
  `openapiv3` v2 cannot model it; chasm silently drops the section. Any
  operation declared under `webhooks` will not be mounted as a route.
- **`$dynamicRef` / `$dynamicAnchor`.** JSON Schema 2020-12 dynamic
  reference resolution is not implemented. Use plain `$ref` for cross-spec
  composition.
- **`format` assertion vocabulary.** chasm treats `format` as an annotation
  by default for generation purposes; opt-in assertion semantics from the
  JSON Schema 2020-12 format-assertion vocabulary are not implemented. The
  request validator still enforces the format set listed under "In scope".

## Known limitations

These features are partially supported via loader rewrites or other
workarounds. They are listed here so callers know which idioms get
normalised on load.

- **`exclusiveMinimum` / `exclusiveMaximum` numeric form.** OAS 3.1 stores
  these as standalone numeric keywords. chasm rewrites them to the OAS 3.0
  boolean form (`minimum: N` + `exclusiveMinimum: true`) at load time so
  the downstream parser accepts them.
- **Path-item `$ref`.** A `paths./foo: {$ref: '#/components/pathItems/X'}`
  envelope is resolved against the root document and inlined before
  deserialisation, so the route gets mounted.
- **Callback `$ref`.** A `callbacks.<name>: {$ref: '...'}` envelope is
  resolved and inlined at load time. Callbacks themselves are still not
  executed (see "Out of scope" → Callbacks), but referenced callback
  definitions no longer break the spec parse.
- **`unevaluatedProperties` request validation.** Partial: schema-driven
  generation honours the keyword, but request validation does not yet
  evaluate the unevaluated-property set after applier keywords resolve,
  so violations slip through `--errors`. `dependentRequired` and
  `dependentSchemas` are fully validated — see "In scope".
- **`refDepthMin` / `refDepthMax` recursion depth hints.** The
  `x-ref-depth-min` / `x-ref-depth-max` extensions are parsed but
  currently not honoured by the faker; cyclic `$ref` graphs fall back
  to the global depth limit.
- **`prefixItems` + `items` combination semantics.** The validator does
  not yet enforce the "items applies to entries beyond the prefix" rule
  across all `prefixItems` + `items` arrangements; some specs that mix
  the two see lax validation.
