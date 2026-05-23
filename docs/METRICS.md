# Metrics

`chasm-server` exposes a Prometheus text-exposition document at
`GET /metrics`. The endpoint is hard-wired into the router before the
spec fallback, so it always wins over a spec operation declared at the
same path.

The implementation is hand-rolled (no `metrics-exporter-prometheus`
dependency); counters and histograms use `AtomicU64` for the hot path.

## Series

| Name | Type | Labels | Meaning |
| --- | --- | --- | --- |
| `chasm_requests_total` | counter | `method`, `route`, `status` | Total HTTP requests served by the mock fallback, keyed by the upper-cased request method (`GET`, `POST`, ...), the spec path template (`route`, e.g. `/pets/{id}`) of the matched operation, and the numeric response status code (`200`, `404`, `422`, ...). Health and `/metrics` requests are not counted. |
| `chasm_validation_errors_total` | counter | `location`, `code` | Per-field request-validation failures emitted by the engine under `--errors`. `location` is one of `path`, `query`, `header`, `body`; `code` is the engine's diagnostic code (e.g. `format`, `required`, `type`, `enum`). Only incremented when a validation error is actually returned to the client. |
| `chasm_request_duration_seconds` | histogram | `method` | Wall-clock duration of the request handler, in seconds, bucketed per upper-cased method. Emitted as the standard Prometheus histogram triple: `_bucket{le="..."}`, `_sum`, `_count`. The `+Inf` bucket equals `_count`. |
| `chasm_spec_reload_failures_total` | counter | _(none)_ | Total number of times the `--watch` debouncer attempted to re-parse the spec from disk and failed (I/O error, parse error, or any other reload-path failure). Monotonic — increments only. Pair with `chasm_last_reload_error` (exposed via `GET /readyz`) to alert on spec-edit regressions without restarting the server. |

## Histogram buckets

`chasm_request_duration_seconds` uses fixed bucket upper-bounds
(seconds):

```
0.005, 0.01, 0.05, 0.1, 0.5, 1.0, 5.0, +Inf
```

These are tuned for a local-loopback mock server where steady-state
requests should land in the first two buckets and anything beyond
`0.5s` is almost certainly cold-start or schema-driven generation
against a large response.

The buckets are compiled in; there is no CLI flag to override them.
If you need different bucketing for an SLO, scrape the
`_sum` / `_count` series and aggregate externally rather than
re-bucketing in Prometheus (which is lossy).

## Cardinality

The `status` label is bounded by the spec's declared response codes
plus the small set of operational codes (`401`, `404`, `405`, `413`,
`422`, `500`). The `method` label is bounded by the HTTP method set.
The `route` label is bounded by the number of distinct path templates
the spec declares — itself a static, finite set — so it does not vary
with user input. The `code` label on `chasm_validation_errors_total`
is bounded by the engine's diagnostic vocabulary. None of the labels
accept user input, so cardinality is finite and small.

## Example scrape

```
# HELP chasm_requests_total Total HTTP requests
# TYPE chasm_requests_total counter
chasm_requests_total{method="GET",route="/pets",status="200"} 142
chasm_requests_total{method="GET",route="/pets/{id}",status="404"} 3
chasm_requests_total{method="POST",route="/pets",status="201"} 8
# HELP chasm_validation_errors_total Total request validation errors
# TYPE chasm_validation_errors_total counter
chasm_validation_errors_total{location="body",code="required"} 2
# HELP chasm_request_duration_seconds Request duration histogram
# TYPE chasm_request_duration_seconds histogram
chasm_request_duration_seconds_bucket{method="GET",le="0.005"} 139
chasm_request_duration_seconds_bucket{method="GET",le="0.01"} 145
chasm_request_duration_seconds_bucket{method="GET",le="+Inf"} 145
chasm_request_duration_seconds_sum{method="GET"} 0.412
chasm_request_duration_seconds_count{method="GET"} 145
```
