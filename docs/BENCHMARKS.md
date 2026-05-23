# Chasm benchmarks

All measurements taken on macOS 15 (Apple Silicon) with `etc/petstore.yaml`.

## Throughput

`hey -z 10s -c 50 http://127.0.0.1:<port>/pets`

| Mode | RPS | Avg latency | p99 latency |
|---|---|---|---|
| chasm-server (static) | 91,981 | 0.5 ms | 1.6 ms |
| chasm-server (dynamic) | ~46,000 | 1.1 ms | 3.4 ms |

## Memory (RSS at idle, post-warmup)

| Metric | Value |
|---|---|
| RSS | 8.2 MB |
| VSZ | 410 GB |

Comparison numbers against other OpenAPI mock servers live in `README.md`
under "Compared to alternatives".

## Binary size

| Variant | Size |
|---|---|
| `chasm-server` release binary | 3.0 MB |
| Docker scratch image | 2.4 MB |
| chasm-wasm (nodejs target) | 3.0 MB |

## Reproduction

```bash
cargo build --release --bin chasm-server
./target/release/chasm-server etc/petstore.yaml --port 14010
hey -z 10s -c 50 http://127.0.0.1:14010/pets
```

Methodology notes:
- Static mode (default) serves from the spec's `example:` blocks.
- Dynamic mode (`--dynamic`) generates via chasm-faker per request; ~50% the static throughput.
- Numbers vary by ~5% across runs.
