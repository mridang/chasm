# Environment variables

`chasm-server` is primarily configured through CLI flags (see the
[CLI flags table](../README.md#cli-flags) in the README). Every major flag
also has a `CHASM_*` environment-variable alias so the binary can be driven
from container orchestrators, CI runners, and process supervisors without
splicing values into the command line. When both forms are supplied the
explicit CLI flag wins, matching clap's standard precedence rules.

| Variable | CLI flag | Default | Meaning |
| --- | --- | --- | --- |
| `RUST_LOG` | derived from `-v` / `--verbose` | `info` | Standard `env_logger`-style directive. When set, takes precedence over the verbosity flag, so operators can narrow individual modules at runtime without redeploying. |
| `CHASM_PORT` | `--port` | `4010` | TCP port to listen on. |
| `CHASM_HOST` | `--host` | `127.0.0.1` | Interface to bind to (loopback by default for safety). |
| `CHASM_DYNAMIC` | `--dynamic` | `false` | Generate responses from schemas rather than static examples. |
| `CHASM_CORS` | `--cors` | `true` | Enable the permissive CORS layer. |
| `CHASM_EXPOSE_SPEC` | `--expose-spec` | `true` | Mount `/openapi.{json,yaml}` introspection routes. **Disclosure risk**: examples and descriptions in your spec are world-readable when exposed. Spec authors frequently embed sample credentials, hostnames, and PII in `example` blocks — review your spec before exposing it on a public endpoint. Set `--expose-spec=false` for production-facing deployments. |
| `CHASM_MAX_CONNECTIONS` | `--max-connections` | `1024` | Concurrency cap before `503 Service Unavailable` shedding kicks in. |
| `CHASM_SEED` | `--seed` | unset | Default seed for dynamic response generation. |
| `CHASM_LOG_FORMAT` | `--log-format` | `text` | Selects the on-the-wire log format (`text` or `json`); `json` also promotes the per-request tracing span to `info`. |
| `CHASM_REQUEST_TIMEOUT` | `--request-timeout` | `30` | Per-request handler timeout in seconds; on expiry the server emits a `408 Request Timeout` problem+json envelope. |
| `CHASM_TLS_CERT` | `--tls-cert` | unset | PEM-encoded certificate chain. Pair with `CHASM_TLS_KEY` (or `--tls-key`) to terminate TLS in-process via `rustls`; both must be supplied together or both omitted. |
| `CHASM_TLS_KEY` | `--tls-key` | unset | PEM-encoded private key paired with `CHASM_TLS_CERT` (or `--tls-cert`); both must be supplied together or both omitted. |

If `RUST_LOG` is set, its directives win over the level derived from
`--verbose`. Unset it (or unset both) to fall back to `info`.

Examples:

```sh
RUST_LOG=info chasm-server etc/petstore.yaml
RUST_LOG=chasm_engine=debug,chasm_server=info chasm-server etc/petstore.yaml
CHASM_PORT=8080 CHASM_LOG_FORMAT=json chasm-server etc/petstore.yaml
CHASM_REQUEST_TIMEOUT=5 chasm-server etc/petstore.yaml
```

Flags that aren't in the table above (CORS allowlist, watch mode, fill-properties,
strict method matching, dry-run) remain CLI-only on purpose: they are part of
a mock server's test contract and should be visible in the command line, not
buried in the process environment.
