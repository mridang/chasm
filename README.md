# chasm

chasm is an OpenAPI 3 mock server in Rust. It ships as a single static binary, a WASM module, and a pair of reusable Rust crates: give it an OAS3 spec, it serves mock responses driven by the spec's `example:` blocks or by schema-faking when no example is declared. It mirrors the CLI shape and `Prefer`-header semantics of `@stoplight/prism-http`, so existing scripts and clients work unchanged.

##### Why?

One static binary, no Node runtime, no `npm install` in CI. Fast cold start, suitable for ephemeral test containers and per-PR previews. Compiles to WASM for browsers, service workers, and Node bindings. Library-first: the engine and faker are usable as plain crates from any Rust app without dragging in HTTP plumbing.

## Usage

Clone and run against the bundled petstore spec.

```bash
git clone https://github.com/mridang/chasm.git
cd chasm
cargo run --bin chasm-server -- etc/petstore.yaml
```

In another shell:

```bash
curl http://localhost:4010/pets
curl -H 'Prefer: code=404' http://localhost:4010/pets/1
curl -H 'Prefer: dynamic=true, seed=42' http://localhost:4010/pets
```

Requires Rust 1.75+.

### Server (CLI)

Run `chasm-server` against an OAS3 spec.

```bash
chasm-server etc/petstore.yaml --port 8080 --host 0.0.0.0
chasm-server etc/petstore.yaml --dynamic --seed 1234
chasm-server etc/petstore.yaml --watch
chasm-server --dry-run etc/petstore.yaml
chasm-server validate etc/petstore.yaml
cat etc/petstore.yaml | chasm-server -
```

The spec path is positional; pass `-` to read from stdin (JSON or YAML, sniffed from the first non-whitespace byte). `chasm-server validate <SPEC>` runs the same structural checks the server runs on startup and exits non-zero with a diagnostic if the spec cannot be served — suitable as a pre-commit or CI gate.

Every response carries an `X-Request-ID` header (either the incoming client value or a fresh UUIDv4) and the same id is attached to the structured log line for the request. Errors are returned as RFC 7807 problem documents at `https://chasm.dev/errors#<SLUG>`.

#### Options

| Flag | Short | Default | Description |
| --- | --- | --- | --- |
| `<SPEC>` | | required | Path to the OAS3 specification (JSON or YAML), positional. Use `-` to read from stdin. |
| `--spec <SPEC>` | | | Long form of the spec path, retained for backwards compatibility. |
| `--port <PORT>` | `-p` | `4010` | TCP port to listen on. |
| `--host <HOST>` | | `127.0.0.1` | Interface to bind to. |
| `--dynamic` | `-d` | `false` | Generate response bodies from the schema rather than static examples. |
| `--cors [<BOOL>]` | `-c` | `true` | Enable permissive CORS. Pass `--cors=false` to disable. |
| `--cors-origin <ORIGIN>` | | | Allowed origin for CORS responses. Repeat to allowlist multiple origins. |
| `--cors-credentials` | | `false` | Emit `Access-Control-Allow-Credentials: true`. Requires at least one `--cors-origin`. |
| `--cors-max-age <SECONDS>` | | `600` | `Access-Control-Max-Age` returned on CORS preflight responses. |
| `--cors-expose-headers <NAME>` | | `x-request-id` | Response header name to advertise via `Access-Control-Expose-Headers`. Repeat for multiple. |
| `--expose-spec [<BOOL>]` | | `true` | Mount `/openapi.yaml`, `/openapi.json`, and `/openapi` introspection routes. |
| `--max-connections <N>` | | `1024` | Cap on in-flight HTTP requests; excess clients receive `503 Service Unavailable`. |
| `--errors` | `-e` | `false` | Return `422` with RFC 7807 problem JSON when request validation fails. |
| `--seed <N>` | | | Default seed for dynamic generation; per-request `Prefer: seed=` overrides it. |
| `--ignore-examples` | | `false` | Skip the example pipeline entirely and always go to schema-driven generation. |
| `--watch` | | `false` | Watch the spec file and hot-reload routing tables on change without dropping the listener. |
| `--dry-run` | | `false` | Load and validate the spec, print a summary, then exit without binding. |
| `--verbose` | `-v` | `0` | Increase log verbosity. Repeatable: `-v` = debug, `-vv` = trace. |
| `--json-schema-faker-fill-properties` | | `true` | Controls `json-schema-faker` `fillProperties` behaviour. |
| `--strict-method-matching` | | `false` | `HEAD`/`OPTIONS` on operations that do not declare them return `405` instead of being implicitly served. |
| `--tls-cert <PATH>` | | | PEM certificate chain. Pair with `--tls-key` to terminate TLS via `rustls`. Also reads `CHASM_TLS_CERT`. |
| `--tls-key <PATH>` | | | PEM private key paired with `--tls-cert`. Also reads `CHASM_TLS_KEY`. |
| `--tls-port <PORT>` | | `8443` | HTTPS port when TLS is enabled. Plain HTTP (`--port`) and HTTPS are served at the same time; ports must differ. Also reads `CHASM_TLS_PORT`. |
| `--log-format <FORMAT>` | | `text` | On-the-wire log format: `text` or `json`. Also reads `CHASM_LOG_FORMAT`. |
| `--request-timeout <SECONDS>` | | `30` | Per-request handler timeout; on expiry the server emits a `408 Request Timeout` problem+json envelope. Also reads `CHASM_REQUEST_TIMEOUT`. |

Operational endpoints (always reserved, outside the spec's path-space):

| Path | Purpose |
| --- | --- |
| `/healthz` | Aggregate health check. |
| `/livez` | Process liveness. |
| `/readyz` | Returns `200` once the spec is loaded; flips to `503` during a `--watch` reload. |
| `/metrics` | Prometheus text exposition (request counters, latency histograms, faker/validation timings). |

### Prefer header

Per-request overrides may also be supplied as query parameters prefixed with `__`; query values win on conflict.

| Directive | Query | Type | Effect |
| --- | --- | --- | --- |
| `code=` | `__code` | `u16` | Force a specific response status code. |
| `example=` | `__example` | string | Pick a named entry from the response's `examples` map. |
| `dynamic=` | `__dynamic` | bool | Toggle schema-driven generation for this request. |
| `seed=` | `__seed` | `u64` | Seed dynamic generation for this request. |
| `validate=` | `__validate` | bool | Disable request validation for this request (default: on under `--errors`). |
| `security=` | `__security` | bool | Disable security/auth evaluation for this request (default: on). |

```bash
curl -H 'Prefer: code=404' http://localhost:4010/pets/1
curl -H 'Prefer: example=fido' http://localhost:4010/pets/1
curl -H 'Prefer: dynamic=true, seed=42' http://localhost:4010/pets
curl 'http://localhost:4010/pets?__dynamic=true&__seed=42'
```

See [`docs/PREFER_HEADER.md`](docs/PREFER_HEADER.md) for full semantics, precedence, and edge cases.

### Spec extensions (`x-chasm-*`)

These vendor extensions let a spec drive transport-layer behaviours that SDK test harnesses would otherwise need a second mock for (WireMock-style parity).

| Extension | Where | Type | Effect |
| --- | --- | --- | --- |
| `x-chasm-delay-ms` | operation or response | int | Sleep this many milliseconds before responding (async, non-blocking). The operation-level value wins over the response-level one. |
| `x-chasm-content-encoding` | response | `gzip` \| `br` \| `zstd` | Compress the response body with the named codec and set `Content-Encoding`. Any other value is ignored. |
| `x-chasm-echo` | operation | bool | Replace the response body with a JSON envelope reflecting the incoming request (method, path, headers, cookies, body, content length). |

```yaml
paths:
  /pets:
    get:
      x-chasm-delay-ms: 250
      x-chasm-echo: true
      responses:
        '200':
          x-chasm-content-encoding: gzip
          content:
            application/json:
              example: { ok: true }
```

### WASM

```js
import init, { Chasm } from "./pkg/chasm_wasm.js";

await init();
const specJson = await fetch("/petstore.json").then(r => r.text());
const chasm = new Chasm(specJson, /* dynamic */ false);

const resp = chasm.handle("GET", "/pets", "", "application/json");
console.log(resp);
```

In Node:

```js
const { Chasm } = require("chasm-wasm");
const fs = require("fs");
const chasm = new Chasm(fs.readFileSync("petstore.json", "utf8"), false);
console.log(chasm.handle("GET", "/pets", "code=404", "application/json"));
```

### Library (Rust)

`chasm-engine` is a plain crate. No HTTP server is required to use it. `MockRequest`, `MockResponse`, and `MockConfig` are `#[non_exhaustive]`, so external callers must use `default()` + field mutation rather than struct literals.

```rust
use chasm_engine::{load_spec, mock, MockConfig, MockRequest};
use std::collections::HashMap;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let spec = load_spec("etc/petstore.yaml")?;

    let mut headers = HashMap::new();
    headers.insert("Prefer".into(), "dynamic=true, seed=42".into());

    let mut req = MockRequest::default();
    req.method = "GET".into();
    req.path = "/pets".into();
    req.headers = headers;

    let cfg = MockConfig::default();
    let resp = mock(&spec, &req, &cfg)?;

    println!("{} {} {}", resp.status, resp.content_type, resp.body);
    Ok(())
}
```

`chasm-faker` is similarly standalone if you only need schema-driven value generation.

```rust
use chasm_faker::{generate, GenerateOptions};
use serde_json::json;

let schema = json!({
    "type": "object",
    "required": ["id", "name"],
    "properties": {
        "id":   { "type": "integer", "format": "int64" },
        "name": { "type": "string", "minLength": 1, "maxLength": 32 }
    }
});

let mut opts = GenerateOptions::default();
opts.seed = Some(42);
let value = generate(&schema, &opts).unwrap();
println!("{}", value);
```

#### Crates

| Crate | Purpose |
| --- | --- |
| `chasm-faker` | Port of `json-schema-faker`. Generates JSON values from a JSON Schema with seeding, formats, `$ref` resolution, and extension keywords. |
| `chasm-engine` | Mocking core: spec loading, routing, content negotiation, example/schema pipeline, `Prefer` parsing. No HTTP. |
| `chasm-server` | Axum binary. Wraps `chasm-engine` behind a CLI and emits RFC 7807 problem documents on error. |
| `chasm-wasm` | `wasm-bindgen` bindings exposing `chasm-engine` to browsers and Node. |

### Error envelope

Errors are returned as RFC 7807 problem documents. The `type` URI is `https://chasm.dev/errors#<SLUG>`; clients that switch on the slug portion (`NO_PATH_MATCHED_ERROR`, etc.) keep working across the 0.2.0 base-URI change.

- `NO_PATH_MATCHED_ERROR` (HTTP 404)
- `NO_METHOD_MATCHED_ERROR` (HTTP 405)
- `NOT_FOUND` (HTTP 404) — `Prefer: example=<name>` references a missing example.
- `NOT_ACCEPTABLE` (HTTP 406) — no media type intersects with the request `Accept` header.
- `UNAUTHORIZED` (HTTP 401)
- `UNPROCESSABLE_ENTITY` (HTTP 422) — request validation failed under `--errors`.
- `NO_RESPONSE_DEFINED` (HTTP 404) — `Prefer: code=<n>` names a status the operation does not declare.
- `NO_RESPONSE_RESPONSE_DEFINED` (HTTP 500) — operation has no usable response definition.
- `INTERNAL_SERVER_ERROR` (HTTP 500)
- `REQUEST_TIMEOUT` (HTTP 408) — handler exceeded `--request-timeout`.
- `PAYLOAD_TOO_LARGE` (HTTP 413) — request body exceeded the 16 MiB ceiling.

## Configuration

Environment variables are an alternative to CLI flags. Every flag noted as "Also reads `CHASM_*`" above is overridable this way. See [`docs/ENV.md`](docs/ENV.md) for the full list.

The `etc/` directory ships small specs covering specific features: [`etc/petstore.yaml`](etc/petstore.yaml) (canonical OAS 3.0), [`etc/auth.yaml`](etc/auth.yaml) (bearer JWT + `apiKey` + Basic), [`etc/oneof-discriminator.yaml`](etc/oneof-discriminator.yaml) (`discriminator.propertyName`), [`etc/oas31-nullable.yaml`](etc/oas31-nullable.yaml) (OAS 3.1 `type: [..., "null"]`). All four validate under `chasm-server validate <path>`.

Long-form reference docs:

- [`docs/SCOPE.md`](docs/SCOPE.md) — what chasm does and intentionally doesn't.
- [`docs/PREFER_HEADER.md`](docs/PREFER_HEADER.md) — `Prefer` directive semantics.
- [`docs/METRICS.md`](docs/METRICS.md) — Prometheus metric names, labels, types.
- [`docs/ENV.md`](docs/ENV.md) — every `CHASM_*` environment variable.
- [`docs/BENCHMARKS.md`](docs/BENCHMARKS.md) — measured throughput, memory, binary size.

## Caveats

- Proxy mode (forwarding to an upstream when no example matches) is not implemented. chasm is mock-only.
- No Spectral linting pipeline; `chasm-server validate` performs structural checks only.
- No `prism.json` config file. Configuration is CLI flags and `CHASM_*` env vars.
- Operation-level callbacks and links are parsed but ignored.
- `bearerFormat: JWT` is treated as opaque credentials; chasm does not parse or validate the JWT shape.

## Contributing

Contributions are welcome! If you find a bug or have suggestions for improvement, please open an issue or submit a pull request.

## License

Apache License 2.0 © 2026 Mridang Agarwalla
