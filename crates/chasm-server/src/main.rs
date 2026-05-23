//! Chasm mock server entry point.
//!
//! ## Reserved observability routes
//!
//! The router declares four explicit paths that take priority over the
//! spec-driven mock fallback:
//!
//! - `GET /healthz` — liveness/health summary, returns `200 ok`.
//! - `GET /livez` — Kubernetes-style liveness probe, returns `200 alive`.
//! - `GET /readyz` — Kubernetes-style readiness probe; returns `200 ok` when
//!   the spec is fresh, or `503` with a `spec reload failed: <reason>` body
//!   when `--watch` has observed a deletion or parse failure.
//! - `GET /metrics` — Prometheus text exposition (counter + histogram).
//!
//! These are wired with explicit `.route(...)` calls **before** the
//! `.fallback(handle_request)` registration, so they CANNOT be overridden by a
//! spec that defines an operation under the same path. If a user spec declares
//! e.g. `paths./healthz`, the operator endpoint still wins and the spec
//! operation is unreachable.
//!
//! ## Request ID propagation
//!
//! Every incoming request carries an `X-Request-ID` value: either the one the
//! client sent (when non-empty) or a freshly minted UUIDv4. The id is echoed
//! back as a response header and wired into a `tracing::debug_span!("request",
//! id, method, path)` so every child log line in the engine inherits the id.
//! The span sits at `debug` (not `info`) so the default `RUST_LOG=info` does
//! not emit a per-request entry; the per-request access log line in
//! [`log_request`] is likewise at `debug`. Operators who want per-request
//! telemetry can opt back in with `RUST_LOG=debug` or `-v`. Startup events
//! and explicit errors continue to log at `info`/`error`.

/// Long-form reference documentation for operators.
///
/// These are markdown files in the repository's `docs/` directory, embedded
/// into rustdoc via `include_str!` so they render with the same styling and
/// search as the API reference. The source markdown also renders on GitHub.
pub mod guides {
    /// Prometheus metrics exposed at `GET /metrics`: names, labels, types.
    #[doc = include_str!("../../../docs/METRICS.md")]
    pub mod metrics {}

    /// Environment variables consumed by `chasm-server` at startup.
    #[doc = include_str!("../../../docs/ENV.md")]
    pub mod env {}

    /// Benchmark numbers for `chasm-server` (throughput, memory, binary size).
    #[doc = include_str!("../../../docs/BENCHMARKS.md")]
    pub mod benchmarks {}
}

use axum::{
    body::Body,
    extract::State,
    http::{HeaderMap, HeaderName, HeaderValue, Method, Request, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Router,
};
use chasm_engine::{
    load_spec, mock, MockConfig, MockError, MockRequest, MockResponse, OpenAPI, SpecError,
    ValidationError,
};
use clap::{ArgAction, Args as ClapArgs, Parser, Subcommand, ValueEnum};
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::{
    collections::HashMap,
    io::Read as IoRead,
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex, RwLock,
    },
    time::{Duration, Instant},
};
use tower_http::cors::{Any, CorsLayer};
use tracing::{info, Instrument};
use uuid::Uuid;

/// Hard ceiling, in bytes, applied to every incoming request body.
///
/// Anything larger is rejected with `413 Payload Too Large` carrying a
/// problem document so clients can react uniformly. 16 MiB matches the
/// upstream limit used by comparable mock servers.
const MAX_BODY_BYTES: usize = 16 * 1024 * 1024;

/// Debounce window applied to filesystem change events when `--watch` is set.
///
/// Editors typically rewrite files atomically by renaming, which surfaces as
/// several events back-to-back; coalescing them inside this window keeps
/// reloads down to one per quiescent burst.
const WATCH_DEBOUNCE: Duration = Duration::from_millis(250);

/// Sentinel value for the spec argument that selects stdin as the source.
const STDIN_SPEC_SENTINEL: &str = "-";

/// Problem document `type` URI for the `NO_PATH_MATCHED_ERROR` class.
const TYPE_NO_PATH_MATCHED: &str = "https://chasm.dev/errors#NO_PATH_MATCHED_ERROR";

/// Problem document `type` URI for the `NO_METHOD_MATCHED_ERROR` class.
const TYPE_NO_METHOD_MATCHED: &str = "https://chasm.dev/errors#NO_METHOD_MATCHED_ERROR";

/// Problem document `type` URI for the `NO_RESPONSE_RESPONSE_DEFINED` class.
/// The double-`RESPONSE` is preserved verbatim so existing ecosystem tooling
/// that keys off the literal token keeps working.
const TYPE_NO_RESPONSE_RESPONSE_DEFINED: &str =
    "https://chasm.dev/errors#NO_RESPONSE_RESPONSE_DEFINED";

/// Problem document `type` URI for the `NO_RESPONSE_DEFINED` class.
const TYPE_NO_RESPONSE_DEFINED: &str = "https://chasm.dev/errors#NO_RESPONSE_DEFINED";

/// Problem document `type` URI for the `UNPROCESSABLE_ENTITY` class.
const TYPE_UNPROCESSABLE_ENTITY: &str = "https://chasm.dev/errors#UNPROCESSABLE_ENTITY";

/// Problem document `type` URI for the `UNAUTHORIZED` class.
const TYPE_UNAUTHORIZED: &str = "https://chasm.dev/errors#UNAUTHORIZED";

/// Problem document `type` URI for the `NOT_FOUND` class.
const TYPE_NOT_FOUND: &str = "https://chasm.dev/errors#NOT_FOUND";

/// Problem document `type` URI for the `NOT_ACCEPTABLE` class.
const TYPE_NOT_ACCEPTABLE: &str = "https://chasm.dev/errors#NOT_ACCEPTABLE";

/// Problem document `type` URI for the `INTERNAL_SERVER_ERROR` class.
const TYPE_INTERNAL_SERVER_ERROR: &str = "https://chasm.dev/errors#INTERNAL_SERVER_ERROR";

/// Problem document `type` URI for the `PAYLOAD_TOO_LARGE` class.
const TYPE_PAYLOAD_TOO_LARGE: &str = "https://chasm.dev/errors#PAYLOAD_TOO_LARGE";

/// Top-level CLI entry point.
///
/// Restructured around subcommands so `chasm-server mock spec.yml ...` and
/// `chasm-server validate spec.yml` both work, while the historical flagless
/// invocation (`chasm-server spec.yml ...`) still defaults into `mock` so
/// existing scripts and the README quickstart continue working unchanged.
#[derive(Parser, Debug)]
#[command(
    name = "chasm-server",
    about = "OAS3 mock server",
    args_conflicts_with_subcommands = true
)]
struct Cli {
    /// Subcommand selecting the mode of operation. When omitted, the
    /// positional/flag arguments below are routed into `mock`.
    #[command(subcommand)]
    command: Option<Command>,

    /// Flags forwarded into the default `mock` subcommand when no explicit
    /// subcommand is provided.
    #[command(flatten)]
    mock_args: MockArgs,
}

/// Selects the on-the-wire format for emitted log lines.
///
/// `Text` keeps the historical human-readable ANSI default emitted by
/// `tracing_subscriber::fmt()`; `Json` switches to the structured JSON formatter
/// so log shippers (Vector, Fluent Bit, Loki) can ingest the stream without an
/// intermediate regex parser. When `Json` is selected the per-request tracing
/// span is also promoted from `debug` to `info` so the structured per-request
/// fields surface at the default `RUST_LOG=info` level — operators almost
/// always pair JSON logs with a downstream collector and expect the
/// per-request envelope to be present by default in that mode.
#[derive(Copy, Clone, Debug, ValueEnum, PartialEq, Eq)]
enum LogFormat {
    /// Human-readable single-line format with ANSI colour codes (the default).
    Text,
    /// One JSON object per line, suitable for structured log ingestion.
    Json,
}

/// Enumerates the subcommands chasm-server exposes.
#[derive(Subcommand, Debug)]
enum Command {
    /// Serves mocked responses for the supplied spec (default behaviour).
    Mock(MockArgs),
    /// Loads the spec, reports parse success or failure, and exits.
    Validate(ValidateArgs),
}

/// Arguments accepted by the `mock` subcommand.
///
/// The spec is a positional argument; `--spec` is still accepted as a long
/// form for backwards compatibility, and the literal value `-` reads the
/// spec from stdin.
#[derive(ClapArgs, Debug)]
struct MockArgs {
    /// Path to the OAS3 specification file (positional, JSON or YAML). Use `-`
    /// to read the spec from stdin.
    #[arg(value_name = "SPEC", required_unless_present = "spec_flag")]
    spec_positional: Option<PathBuf>,

    /// Path to the OAS3 specification file (long form, retained for compatibility).
    #[arg(long = "spec", value_name = "SPEC")]
    spec_flag: Option<PathBuf>,

    /// TCP port to listen on.
    #[arg(long, short = 'p', default_value = "4010", env = "CHASM_PORT")]
    port: u16,

    /// Host/interface to bind to (defaults to loopback for safety).
    #[arg(long, default_value = "127.0.0.1", env = "CHASM_HOST")]
    host: String,

    /// Generate responses from the schema instead of static examples.
    #[arg(long, short = 'd', env = "CHASM_DYNAMIC")]
    dynamic: bool,

    /// Enable permissive CORS (default on).
    #[arg(long, short = 'c', default_value = "true", num_args = 0..=1, default_missing_value = "true", env = "CHASM_CORS")]
    cors: bool,

    /// Mount `/openapi.yaml`, `/openapi.json`, and `/openapi` introspection
    /// routes that serve the normalised in-memory spec. Default on, matching
    /// the dominant ecosystem behaviour (Microcks, swagger-mock); pass
    /// `--expose-spec=false` to hide the spec from clients.
    #[arg(
        long = "expose-spec",
        default_value = "true",
        num_args = 0..=1,
        default_missing_value = "true",
        env = "CHASM_EXPOSE_SPEC",
    )]
    expose_spec: bool,

    /// Selects the on-the-wire log format. `text` keeps the historical
    /// human-readable single-line layout; `json` switches to one structured
    /// JSON object per line and promotes the per-request tracing span to
    /// `info` so structured fields surface at the default level.
    #[arg(
        long = "log-format",
        value_enum,
        default_value = "text",
        env = "CHASM_LOG_FORMAT"
    )]
    log_format: LogFormat,

    /// Maximum wall-clock duration (in seconds) the server waits for a
    /// downstream handler before tearing the request down with a
    /// `408 Request Timeout` problem document. Defaults to 30 seconds, which
    /// covers slow regex-driven generators while keeping a hung worker from
    /// pinning a connection forever.
    #[arg(
        long = "request-timeout",
        value_name = "SECONDS",
        default_value = "30",
        env = "CHASM_REQUEST_TIMEOUT"
    )]
    request_timeout: u64,

    /// Allowed origin for CORS responses. Repeat to allowlist multiple origins.
    /// When at least one value is supplied, `Access-Control-Allow-Origin` echoes
    /// the matching request `Origin` instead of the wildcard, and unmatched
    /// origins receive no allow-origin header.
    #[arg(long = "cors-origin", value_name = "ORIGIN")]
    cors_origin: Vec<String>,

    /// Emit `Access-Control-Allow-Credentials: true`. Requires at least one
    /// `--cors-origin`; ignored with a warning when combined with the default
    /// `Any` origin policy because the spec forbids that combination.
    #[arg(long = "cors-credentials")]
    cors_credentials: bool,

    /// `Access-Control-Max-Age` (seconds) returned on CORS preflight responses.
    #[arg(long = "cors-max-age", value_name = "SECONDS", default_value = "600")]
    cors_max_age: u64,

    /// Response header name to advertise via `Access-Control-Expose-Headers`.
    /// Repeat to expose multiple headers. Defaults to `x-request-id`.
    #[arg(
        long = "cors-expose-headers",
        value_name = "NAME",
        default_value = "x-request-id"
    )]
    cors_expose_headers: Vec<String>,

    /// Return 422 when request validation fails (logs warnings otherwise).
    #[arg(long, short = 'e')]
    errors: bool,

    /// Default seed for dynamic generation.
    #[arg(long, env = "CHASM_SEED")]
    seed: Option<u64>,

    /// Skip example pipeline by default.
    #[arg(long)]
    ignore_examples: bool,

    /// Increases log verbosity (`-v` = debug, `-vv` or more = trace).
    #[arg(short = 'v', long = "verbose", action = ArgAction::Count)]
    verbose: u8,

    /// Controls the json-schema-faker `fillProperties` behaviour at the server
    /// level; defaults to `true`.
    #[arg(
        long = "json-schema-faker-fill-properties",
        default_value = "true",
        num_args = 0..=1,
        default_missing_value = "true",
    )]
    json_schema_faker_fill_properties: bool,

    /// Reload the spec from disk whenever the underlying file changes.
    #[arg(long, short = 'w')]
    watch: bool,

    /// Validate the spec, print a summary, and exit without binding a socket.
    #[arg(long)]
    dry_run: bool,

    /// Caps in-flight HTTP requests; excess clients receive `503 Service
    /// Unavailable` rather than degrading p99 latency unboundedly. The default
    /// of 1024 matches a conservative starting point observed to sit well
    /// inside Tokio's worker budget on a typical 4-core developer host.
    #[arg(
        long = "max-connections",
        default_value = "1024",
        env = "CHASM_MAX_CONNECTIONS"
    )]
    max_connections: usize,

    /// Disable chasm's permissive HEAD-mirrors-GET and OPTIONS-preflight
    /// synthesis. With this flag set, requests for `HEAD` or `OPTIONS` against
    /// paths that do not declare those operations return `405 Method Not
    /// Allowed` carrying a `NO_METHOD_MATCHED` envelope. Also suppresses the
    /// CORS middleware (which would otherwise short-circuit every `OPTIONS`
    /// request as a preflight before the engine sees it) so strict 405
    /// responses can actually surface to the client. The default of `false`
    /// preserves the permissive synthesis chasm shipped with originally.
    #[arg(long = "strict-method-matching")]
    strict_method_matching: bool,

    /// PEM-encoded certificate chain enabling TLS termination. Must be paired
    /// with `--tls-key`; supplying one without the other is a startup error.
    /// When both are set the listener serves HTTPS and negotiates HTTP/2 via
    /// ALPN.
    #[arg(long = "tls-cert", value_name = "PATH", env = "CHASM_TLS_CERT")]
    tls_cert: Option<PathBuf>,

    /// PEM-encoded private key paired with `--tls-cert`. Must be supplied
    /// together with the certificate chain; supplying one without the other
    /// is a startup error.
    #[arg(long = "tls-key", value_name = "PATH", env = "CHASM_TLS_KEY")]
    tls_key: Option<PathBuf>,
}

/// Arguments accepted by the `validate` subcommand.
#[derive(ClapArgs, Debug)]
struct ValidateArgs {
    /// Path to the OAS3 specification file (JSON or YAML). Use `-` to read the
    /// spec from stdin.
    #[arg(value_name = "SPEC")]
    spec: PathBuf,
}

/// Shared HTTP application state.
///
/// `spec` is wrapped in `RwLock<Arc<OpenAPI>>` so the filesystem watcher can
/// atomically swap a freshly-loaded spec under a brief write lock while
/// request handlers continue to snapshot the current `Arc` under a read lock.
#[derive(Clone)]
struct AppState {
    /// Parsed OpenAPI spec shared across all requests, hot-swappable when
    /// `--watch` is set.
    spec: Arc<RwLock<Arc<OpenAPI>>>,
    /// Default `MockConfig` cloned per request before per-request overrides apply.
    default_cfg: Arc<MockConfig>,
    /// In-process metrics registry feeding the `/metrics` endpoint.
    metrics: Arc<Metrics>,
    /// Stores the last reload failure (if any) so `/readyz` can surface a
    /// stale-spec condition to orchestrators. `None` means the most recent
    /// reload (or the initial load) succeeded; `Some(reason)` indicates the
    /// spec on disk could not be re-read and the server is serving the
    /// last-known-good copy.
    last_reload_error: Arc<Mutex<Option<String>>>,
    /// On-the-wire log format selected by the operator. Promotes the
    /// per-request tracing span from `debug` to `info` when JSON is selected
    /// so structured per-request fields surface at the default level.
    log_format: LogFormat,
}

/// Histogram bucket upper-bounds (seconds) for `chasm_request_duration_seconds`.
const HISTOGRAM_BUCKETS: [f64; 7] = [0.005, 0.01, 0.05, 0.1, 0.5, 1.0, 5.0];

/// In-process Prometheus-style counters and histograms.
///
/// Hand-rolled rather than pulling in `metrics-exporter-prometheus` to keep the
/// dependency surface minimal. Counters use `AtomicU64` for lock-free updates;
/// the per-label maps sit behind a `RwLock` so the steady-state increment
/// (label set already registered) acquires the read lock only — concurrent
/// recorders share the read guard and contend exclusively on the registration
/// slow path.
#[derive(Default)]
struct Metrics {
    /// `chasm_requests_total{method, route, status}` — total served HTTP
    /// requests keyed by the request method (uppercased), the OAS3 path
    /// template that matched (e.g. `/pets/{petId}`), and the numeric status
    /// code. `route` is the literal `_unmatched` sentinel when no spec
    /// template matched (404 fall-through), bounding the label cardinality at
    /// `|methods| * (|paths|+1) * |statuses|` rather than letting the
    /// concrete URL leak in.
    requests_total: RwLock<HashMap<(String, String, u16), Arc<AtomicU64>>>,
    /// `chasm_validation_errors_total{location, code}` — per-field validation
    /// errors observed from the engine, keyed by location (`path`, `query`,
    /// `header`, `body`) and the engine's diagnostic code.
    validation_errors_total: RwLock<HashMap<(String, String), Arc<AtomicU64>>>,
    /// `chasm_request_duration_seconds` — bucketed counts (cumulative) plus
    /// the running count and sum, ready for Prometheus histogram exposition.
    duration_buckets: RwLock<HashMap<String, Arc<HistogramSeries>>>,
    /// `chasm_inflight_requests` — gauge counting requests currently in
    /// flight, incremented on entry to the handler and decremented after the
    /// response is finalised. Lets operators correlate latency spikes with
    /// concurrency, and (combined with `--max-connections`) makes it visible
    /// when the server is shedding load via the concurrency limiter.
    inflight: AtomicU64,
    /// `chasm_spec_reload_failures_total` — total number of times the
    /// hot-reload watcher attempted to re-parse the spec from disk but failed.
    /// Increments cover I/O errors, parse errors, and any other reload-path
    /// failure observed by the debouncer loop. Monotonic; never decrements.
    spec_reload_failures_total: AtomicU64,
}

/// Per-method histogram state for `chasm_request_duration_seconds`.
struct HistogramSeries {
    /// Cumulative bucket counts, indexed by [`HISTOGRAM_BUCKETS`] position.
    /// Each bucket counts observations with duration `<= bucket_bound`.
    buckets: Vec<AtomicU64>,
    /// Total observations (also exposed as the `+Inf` bucket).
    count: AtomicU64,
    /// Sum of observed durations in seconds, stored as the bit pattern of an
    /// `f64` so we can read/write it atomically.
    sum_bits: AtomicU64,
}

impl HistogramSeries {
    /// Allocates a zeroed histogram with one bucket per [`HISTOGRAM_BUCKETS`] entry.
    fn new() -> Self {
        let mut buckets = Vec::with_capacity(HISTOGRAM_BUCKETS.len());
        for _ in 0..HISTOGRAM_BUCKETS.len() {
            buckets.push(AtomicU64::new(0));
        }
        Self {
            buckets,
            count: AtomicU64::new(0),
            sum_bits: AtomicU64::new(0),
        }
    }
}

/// Acquires `mutex` and recovers the inner guard if the mutex has been
/// poisoned by a panic in another thread.
///
/// Retained for the non-metrics mutexes (e.g. `last_reload_error`); the
/// per-label metrics maps were migrated to `RwLock` and use [`read_metrics`]
/// / [`write_metrics`] instead.
fn lock_metrics<'a, T>(mutex: &'a Mutex<T>, label: &'static str) -> std::sync::MutexGuard<'a, T> {
    match mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => {
            tracing::warn!(mutex = label, "recovering poisoned mutex");
            poisoned.into_inner()
        }
    }
}

/// Acquires the read side of `lock` and recovers the inner guard if the lock
/// has been poisoned by a panic in another thread.
///
/// The metrics `RwLock`s only protect short critical sections; a poisoned
/// lock does not imply the contained data is corrupted, only that some other
/// thread panicked while holding the lock. Crashing the entire server on
/// subsequent metric reads would be far worse than emitting a single `warn!`
/// and continuing, so this helper unwraps the poison into the inner guard.
fn read_metrics<'a, T>(
    lock: &'a RwLock<T>,
    label: &'static str,
) -> std::sync::RwLockReadGuard<'a, T> {
    match lock.read() {
        Ok(guard) => guard,
        Err(poisoned) => {
            tracing::warn!(lock = label, "recovering poisoned metrics rwlock (read)");
            poisoned.into_inner()
        }
    }
}

/// Acquires the write side of `lock`, recovering the inner guard on poison.
///
/// Counterpart to [`read_metrics`] for the registration slow path that
/// inserts a previously-unseen label set into the inner `HashMap`.
fn write_metrics<'a, T>(
    lock: &'a RwLock<T>,
    label: &'static str,
) -> std::sync::RwLockWriteGuard<'a, T> {
    match lock.write() {
        Ok(guard) => guard,
        Err(poisoned) => {
            tracing::warn!(lock = label, "recovering poisoned metrics rwlock (write)");
            poisoned.into_inner()
        }
    }
}

impl Metrics {
    /// Increments `chasm_requests_total{method, route, status}` by one.
    ///
    /// `route` should be the matched OAS3 path template (e.g.
    /// `/pets/{petId}`), the literal `_unmatched` sentinel for fall-through
    /// 404s, or one of the reserved operator-route labels (e.g. `/metrics`).
    /// Templating keeps the metric cardinality bounded by spec size rather
    /// than by the volume of distinct concrete URLs the server has seen.
    fn record_request(&self, method: &str, route: &str, status: u16) {
        let key = (method.to_ascii_uppercase(), route.to_string(), status);
        {
            let guard = read_metrics(&self.requests_total, "requests_total");
            if let Some(counter) = guard.get(&key) {
                counter.fetch_add(1, Ordering::Relaxed);
                return;
            }
        }
        let counter = {
            let mut guard = write_metrics(&self.requests_total, "requests_total");
            guard
                .entry(key)
                .or_insert_with(|| Arc::new(AtomicU64::new(0)))
                .clone()
        };
        counter.fetch_add(1, Ordering::Relaxed);
    }

    /// Increments the in-flight gauge by one and returns a guard that
    /// decrements it on drop.
    ///
    /// Returning a guard rather than exposing the inc/dec calls directly
    /// makes the bookkeeping panic-safe: even if a handler future panics
    /// mid-flight, the guard's `Drop` impl restores the gauge to the correct
    /// value as the stack unwinds.
    fn track_inflight(self: &Arc<Self>) -> InflightGuard {
        self.inflight.fetch_add(1, Ordering::Relaxed);
        InflightGuard {
            metrics: self.clone(),
        }
    }

    /// Records a request duration into the per-method histogram.
    fn record_duration(&self, method: &str, seconds: f64) {
        let upper = method.to_ascii_uppercase();
        let series = {
            let guard = read_metrics(&self.duration_buckets, "duration_buckets");
            if let Some(series) = guard.get(&upper) {
                series.clone()
            } else {
                drop(guard);
                let mut guard = write_metrics(&self.duration_buckets, "duration_buckets");
                guard
                    .entry(upper)
                    .or_insert_with(|| Arc::new(HistogramSeries::new()))
                    .clone()
            }
        };
        for (i, bound) in HISTOGRAM_BUCKETS.iter().enumerate() {
            if seconds <= *bound {
                series.buckets[i].fetch_add(1, Ordering::Relaxed);
            }
        }
        series.count.fetch_add(1, Ordering::Relaxed);
        // Atomically add `seconds` into `sum_bits` interpreted as f64.
        let mut current = series.sum_bits.load(Ordering::Relaxed);
        loop {
            let next = f64::to_bits(f64::from_bits(current) + seconds);
            match series.sum_bits.compare_exchange_weak(
                current,
                next,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(observed) => current = observed,
            }
        }
    }

    /// Renders the registry as a Prometheus text exposition document.
    fn render(&self) -> String {
        let mut out = String::new();
        out.push_str("# HELP chasm_requests_total Total HTTP requests\n");
        out.push_str("# TYPE chasm_requests_total counter\n");
        let requests = read_metrics(&self.requests_total, "requests_total");
        for ((method, route, status), counter) in requests.iter() {
            out.push_str(&format!(
                "chasm_requests_total{{method=\"{}\",route=\"{}\",status=\"{}\"}} {}\n",
                method,
                escape_label_value(route),
                status,
                counter.load(Ordering::Relaxed),
            ));
        }
        drop(requests);

        out.push_str("# HELP chasm_inflight_requests In-flight HTTP requests\n");
        out.push_str("# TYPE chasm_inflight_requests gauge\n");
        out.push_str(&format!(
            "chasm_inflight_requests {}\n",
            self.inflight.load(Ordering::Relaxed),
        ));

        out.push_str(
            "# HELP chasm_spec_reload_failures_total Total spec reload failures from the --watch debouncer\n",
        );
        out.push_str("# TYPE chasm_spec_reload_failures_total counter\n");
        out.push_str(&format!(
            "chasm_spec_reload_failures_total {}\n",
            self.spec_reload_failures_total.load(Ordering::Relaxed),
        ));

        out.push_str("# HELP chasm_validation_errors_total Total request validation errors\n");
        out.push_str("# TYPE chasm_validation_errors_total counter\n");
        let val_errs = read_metrics(&self.validation_errors_total, "validation_errors_total");
        for ((location, code), counter) in val_errs.iter() {
            out.push_str(&format!(
                "chasm_validation_errors_total{{location=\"{}\",code=\"{}\"}} {}\n",
                location,
                code,
                counter.load(Ordering::Relaxed),
            ));
        }
        drop(val_errs);

        out.push_str("# HELP chasm_request_duration_seconds Request duration histogram\n");
        out.push_str("# TYPE chasm_request_duration_seconds histogram\n");
        let durations = read_metrics(&self.duration_buckets, "duration_buckets");
        for (method, series) in durations.iter() {
            let count = series.count.load(Ordering::Relaxed);
            for (i, bound) in HISTOGRAM_BUCKETS.iter().enumerate() {
                let value = series.buckets[i].load(Ordering::Relaxed);
                out.push_str(&format!(
                    "chasm_request_duration_seconds_bucket{{method=\"{}\",le=\"{}\"}} {}\n",
                    method, bound, value,
                ));
            }
            out.push_str(&format!(
                "chasm_request_duration_seconds_bucket{{method=\"{}\",le=\"+Inf\"}} {}\n",
                method, count,
            ));
            let sum = f64::from_bits(series.sum_bits.load(Ordering::Relaxed));
            out.push_str(&format!(
                "chasm_request_duration_seconds_sum{{method=\"{}\"}} {}\n",
                method, sum,
            ));
            out.push_str(&format!(
                "chasm_request_duration_seconds_count{{method=\"{}\"}} {}\n",
                method, count,
            ));
        }

        out
    }
}

/// RAII helper that decrements `Metrics::inflight` on drop.
///
/// Held by [`handle_request_inner`] for the lifetime of the request future so
/// the gauge tracks "requests currently being processed" rather than "requests
/// observed since start". The `Drop` impl runs even on panic-unwind, keeping
/// the gauge from drifting upward if a downstream handler panics.
struct InflightGuard {
    /// Reference to the metrics registry whose gauge this guard manages.
    metrics: Arc<Metrics>,
}

impl Drop for InflightGuard {
    /// Decrements the in-flight gauge by one when the guard goes out of scope.
    fn drop(&mut self) {
        self.metrics.inflight.fetch_sub(1, Ordering::Relaxed);
    }
}

/// Escapes a Prometheus label value per the exposition format rules.
///
/// Backslashes, double-quotes, and newlines are the only characters that need
/// escaping inside a `name="<value>"` label-pair. Path templates derived from
/// OAS3 specs are unlikely to contain any of them, but escaping defensively
/// keeps a hostile spec from producing an unparseable metrics document.
fn escape_label_value(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            other => out.push(other),
        }
    }
    out
}

/// Entry point: dispatches to the chosen subcommand.
#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    match cli.command {
        Some(Command::Mock(args)) => run_mock(args).await,
        Some(Command::Validate(args)) => run_validate(args),
        None => run_mock(cli.mock_args).await,
    }
}

/// Resolves the spec path/sentinel from the `mock` arguments, preferring the
/// positional value over the legacy `--spec` long-form flag.
fn resolve_mock_spec_arg(args: &MockArgs) -> PathBuf {
    args.spec_positional
        .clone()
        .or_else(|| args.spec_flag.clone())
        .expect("clap should have required the spec argument")
}

/// Loads a spec from a `PathBuf`, treating the literal value `-` as stdin.
///
/// Returns the parsed `OpenAPI`, the canonical path string used in log lines,
/// and (when not stdin) the original `PathBuf` suitable for filesystem
/// watching. For stdin sources the path string is the sentinel `-` and the
/// returned `PathBuf` is `None`, since `notify` cannot watch a pipe.
fn load_spec_from_arg(arg: &Path) -> Result<(OpenAPI, String, Option<PathBuf>), SpecError> {
    let arg_str = arg.to_string_lossy().to_string();
    if arg_str == STDIN_SPEC_SENTINEL {
        let mut buf = String::new();
        std::io::stdin()
            .read_to_string(&mut buf)
            .map_err(|source| SpecError::Io {
                path: STDIN_SPEC_SENTINEL.to_string(),
                source,
            })?;
        let spec = load_spec(&buf)?;
        Ok((spec, STDIN_SPEC_SENTINEL.to_string(), None))
    } else {
        let spec = load_spec(&arg_str)?;
        Ok((spec, arg_str, Some(arg.to_path_buf())))
    }
}

/// Implements the `validate` subcommand.
///
/// On success prints `spec is valid` to stdout and exits 0; on failure prints
/// the error to stderr and exits 1. No server is started and no socket is
/// bound, so this is safe to use as a CI pre-flight step.
fn run_validate(args: ValidateArgs) {
    match load_spec_from_arg(&args.spec) {
        Ok(_) => {
            println!("spec is valid");
            std::process::exit(0);
        }
        Err(e) => {
            eprintln!("error: {}", e);
            std::process::exit(1);
        }
    }
}

/// Implements the `mock` subcommand.
///
/// Loads the spec (from disk or stdin), optionally short-circuits via
/// `--dry-run` after printing a summary, optionally installs a filesystem
/// watcher when `--watch` is set, and otherwise runs the Axum server until
/// cancelled.
async fn run_mock(args: MockArgs) {
    init_tracing(args.verbose, args.log_format);

    let spec_arg = resolve_mock_spec_arg(&args);
    let (spec, spec_path_str, watch_path) = match load_spec_from_arg(&spec_arg) {
        Ok(triple) => triple,
        Err(e) => {
            tracing::error!(error = %e, "failed to load spec");
            std::process::exit(1);
        }
    };

    if args.dry_run {
        print_spec_summary(&spec, args.log_format);
        std::process::exit(0);
    }

    let default_cfg = {
        let mut __c = MockConfig::default();
        __c.dynamic = args.dynamic;
        __c.ignore_examples = args.ignore_examples;
        __c.seed = args.seed;
        __c.example_key = None;
        __c.force_code = None;
        __c.errors = args.errors;
        __c.fill_properties = Some(args.json_schema_faker_fill_properties);
        __c.strict_method_matching = args.strict_method_matching;
        __c.check_security = true;
        __c
    };

    let spec_cell = Arc::new(RwLock::new(Arc::new(spec)));
    let last_reload_error: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let metrics = Arc::new(Metrics::default());
    let state = AppState {
        spec: spec_cell.clone(),
        default_cfg: Arc::new(default_cfg),
        metrics: metrics.clone(),
        last_reload_error: last_reload_error.clone(),
        log_format: args.log_format,
    };

    let _watcher_guard = if args.watch {
        match watch_path {
            Some(path) => Some(install_spec_watcher(
                path,
                spec_cell,
                last_reload_error.clone(),
                metrics.clone(),
            )),
            None => {
                tracing::error!("--watch requires a filesystem path, not stdin");
                std::process::exit(1);
            }
        }
    } else {
        None
    };

    let addr: SocketAddr = format!("{}:{}", args.host, args.port)
        .parse()
        .unwrap_or_else(|e| {
            tracing::error!(error = %e, "invalid host/port");
            std::process::exit(1);
        });

    let tls_paths = match (args.tls_cert.as_ref(), args.tls_key.as_ref()) {
        (Some(cert), Some(key)) => Some((cert.clone(), key.clone())),
        (None, None) => None,
        (Some(_), None) => {
            tracing::error!("--tls-cert requires --tls-key (both must be supplied together)");
            std::process::exit(2);
        }
        (None, Some(_)) => {
            tracing::error!("--tls-key requires --tls-cert (both must be supplied together)");
            std::process::exit(2);
        }
    };

    let mut app = build_router(state, args.expose_spec);
    if args.cors && !args.strict_method_matching {
        app = app.layer(build_cors_layer(&CorsOptions {
            origins: args.cors_origin.clone(),
            credentials: args.cors_credentials,
            max_age: args.cors_max_age,
            expose_headers: args.cors_expose_headers.clone(),
        }));
    }
    app = app.layer(tower_http::compression::CompressionLayer::new());
    app = app.layer(tower::limit::ConcurrencyLimitLayer::new(
        args.max_connections,
    ));
    app = apply_timeout_layer(app, Duration::from_secs(args.request_timeout));

    if let Some((cert_path, key_path)) = tls_paths {
        install_default_crypto_provider();
        let tls_config =
            match axum_server::tls_rustls::RustlsConfig::from_pem_file(&cert_path, &key_path).await
            {
                Ok(cfg) => cfg,
                Err(e) => {
                    tracing::error!(
                        cert = %cert_path.display(),
                        key = %key_path.display(),
                        error = %e,
                        "failed to load TLS material"
                    );
                    std::process::exit(1);
                }
            };
        info!(
            "Listening on https://{} (spec: {}, max_connections: {}, http2: alpn)",
            addr, spec_path_str, args.max_connections
        );
        let handle = axum_server::Handle::new();
        let shutdown_handle = handle.clone();
        let graceful_timeout = Duration::from_secs(args.request_timeout.max(1));
        tokio::spawn(async move {
            shutdown_signal().await;
            tracing::info!(
                timeout_secs = graceful_timeout.as_secs(),
                "TLS server received shutdown signal; draining in-flight connections"
            );
            shutdown_handle.graceful_shutdown(Some(graceful_timeout));
        });
        if let Err(e) = axum_server::bind_rustls(addr, tls_config)
            .handle(handle)
            .serve(app.into_make_service())
            .await
        {
            tracing::error!(error = %e, "TLS server terminated");
            std::process::exit(1);
        }
        tracing::info!("shutdown complete");
        return;
    }

    let listener = match tokio::net::TcpListener::bind(&addr).await {
        Ok(l) => l,
        Err(e) => {
            tracing::error!(addr = %addr, error = %e, "failed to bind");
            std::process::exit(1);
        }
    };
    info!(
        "Listening on http://{} (spec: {}, max_connections: {})",
        addr, spec_path_str, args.max_connections
    );
    if let Err(e) = axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
    {
        tracing::error!(error = %e, "server terminated");
        std::process::exit(1);
    }
    tracing::info!("shutdown complete");
}

/// Installs the process-wide `rustls` `CryptoProvider` (see
/// `rustls::crypto::CryptoProvider`) needed by `axum-server`'s TLS backend.
///
/// `rustls` 0.23 made provider selection explicit: even though we enable the
/// `aws-lc-rs` cargo feature, the runtime still requires an explicit
/// `install_default()` call before any TLS handshake. Doing it lazily on the
/// first `--tls-cert`/`--tls-key` startup keeps non-TLS invocations free of
/// the initialisation cost, and ignoring the `Err` is correct: a non-`Ok`
/// return value means another caller (e.g. a test harness or a previous
/// in-process restart) already installed a provider, which is fine.
fn install_default_crypto_provider() {
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
}

/// Prints a one-line summary of the parsed spec to stdout.
///
/// Counts paths, operations (every HTTP verb across every path), and named
/// components (schemas, responses, parameters, examples, request bodies,
/// headers, security schemes, links, callbacks).
fn print_spec_summary(spec: &OpenAPI, log_format: LogFormat) {
    let path_count = spec.paths.paths.len();
    let mut op_count: usize = 0;
    for (_, path_ref) in spec.paths.paths.iter() {
        if let Some(item) = path_ref.as_item() {
            op_count += [
                item.get.is_some(),
                item.put.is_some(),
                item.post.is_some(),
                item.delete.is_some(),
                item.options.is_some(),
                item.head.is_some(),
                item.patch.is_some(),
                item.trace.is_some(),
            ]
            .into_iter()
            .filter(|b| *b)
            .count();
        }
    }
    let comp_count = spec
        .components
        .as_ref()
        .map(|c| {
            c.schemas.len()
                + c.responses.len()
                + c.parameters.len()
                + c.examples.len()
                + c.request_bodies.len()
                + c.headers.len()
                + c.security_schemes.len()
                + c.links.len()
                + c.callbacks.len()
        })
        .unwrap_or(0);
    match log_format {
        LogFormat::Json => {
            let line = serde_json::json!({
                "event": "spec_summary",
                "paths": path_count,
                "operations": op_count,
                "components": comp_count,
            });
            println!("{}", line);
        }
        LogFormat::Text => {
            println!(
                "Loaded {} paths, {} operations, {} components",
                path_count, op_count, comp_count
            );
        }
    }
}

/// Installs a debounced filesystem watcher that hot-reloads `spec_cell` when
/// the file at `path` changes on disk.
///
/// Returns a guard that owns the watcher and the debouncer thread; dropping
/// the guard stops both. The watcher reports any parse failure during reload
/// to stderr and leaves the previously-loaded spec installed, so a transient
/// editor save mid-write never takes the running server offline.
fn install_spec_watcher(
    path: PathBuf,
    spec_cell: Arc<RwLock<Arc<OpenAPI>>>,
    last_reload_error: Arc<Mutex<Option<String>>>,
    metrics: Arc<Metrics>,
) -> SpecWatcherGuard {
    let (tx, rx) = std::sync::mpsc::channel::<Event>();
    let mut watcher: RecommendedWatcher = match notify::recommended_watcher(
        move |res: notify::Result<Event>| {
            if let Ok(event) = res {
                if matches!(
                    event.kind,
                    EventKind::Modify(_) | EventKind::Create(_) | EventKind::Remove(_)
                ) {
                    let _ = tx.send(event);
                }
            }
        },
    ) {
        Ok(w) => w,
        Err(err) => {
            tracing::error!(
                error = %err,
                path = %path.display(),
                "failed to create filesystem watcher; --watch disabled and server will continue without live reload"
            );
            return SpecWatcherGuard {
                _watcher: None,
                _join: None,
            };
        }
    };
    let watch_target = path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf();
    if let Err(err) = watcher.watch(&watch_target, RecursiveMode::NonRecursive) {
        tracing::error!(
            error = %err,
            target = %watch_target.display(),
            "failed to attach filesystem watcher to parent directory; --watch disabled and server will continue without live reload"
        );
        return SpecWatcherGuard {
            _watcher: None,
            _join: None,
        };
    }

    let debounce_path = path.clone();
    let debounce_cell = spec_cell.clone();
    let join = std::thread::spawn(move || {
        debouncer_loop(rx, debounce_path, debounce_cell, last_reload_error, metrics);
    });

    SpecWatcherGuard {
        _watcher: Some(watcher),
        _join: Some(join),
    }
}

/// Holds the running watcher and its debouncer thread.
///
/// Dropping the guard drops the `RecommendedWatcher`, which in turn closes
/// the underlying mpsc channel; the debouncer thread observes the closed
/// channel on its next `recv()` and exits, after which the join handle is
/// dropped without further action.
struct SpecWatcherGuard {
    _watcher: Option<RecommendedWatcher>,
    _join: Option<std::thread::JoinHandle<()>>,
}

/// Coalesces filesystem events within [`WATCH_DEBOUNCE`] and reloads the spec
/// once per quiescent burst.
///
/// Events touching files other than `path` are ignored. On reload failure the
/// previous spec stays installed and the parse error is written to stderr so
/// the operator can fix the spec without restarting the server.
fn debouncer_loop(
    rx: std::sync::mpsc::Receiver<Event>,
    path: PathBuf,
    spec_cell: Arc<RwLock<Arc<OpenAPI>>>,
    last_reload_error: Arc<Mutex<Option<String>>>,
    metrics: Arc<Metrics>,
) {
    let canonical_target = std::fs::canonicalize(&path).ok();
    loop {
        let first = match rx.recv() {
            Ok(ev) => ev,
            Err(_) => return,
        };
        let mut pending = vec![first];
        while let Ok(ev) = rx.recv_timeout(WATCH_DEBOUNCE) {
            pending.push(ev);
        }
        let touches_target = pending.iter().any(|ev| {
            ev.paths
                .iter()
                .any(|p| event_path_matches(p, &path, canonical_target.as_deref()))
        });
        if !touches_target {
            continue;
        }
        let any_remove = pending
            .iter()
            .any(|ev| matches!(ev.kind, EventKind::Remove(_)));
        let any_reload_trigger = pending
            .iter()
            .any(|ev| matches!(ev.kind, EventKind::Modify(_) | EventKind::Create(_)));
        if any_remove && !any_reload_trigger {
            tracing::warn!(
                target: "chasm::watch",
                path = %path.display(),
                "watched spec file was removed; continuing to serve last-known-good"
            );
            metrics
                .spec_reload_failures_total
                .fetch_add(1, Ordering::Relaxed);
            let mut guard = lock_metrics(&last_reload_error, "last_reload_error");
            *guard = Some(format!("watched spec file was removed: {}", path.display()));
            continue;
        }
        if !any_reload_trigger {
            continue;
        }
        match load_spec(&path.to_string_lossy()) {
            Ok(new_spec) => {
                {
                    let mut err_guard = lock_metrics(&last_reload_error, "last_reload_error");
                    *err_guard = None;
                }
                if let Ok(mut guard) = spec_cell.write() {
                    *guard = Arc::new(new_spec);
                }
                tracing::info!(
                    target: "chasm::watch",
                    path = %path.display(),
                    "spec reloaded"
                );
            }
            Err(e) => {
                tracing::warn!(
                    target: "chasm::watch",
                    path = %path.display(),
                    error = %e,
                    "spec reload failed"
                );
                metrics
                    .spec_reload_failures_total
                    .fetch_add(1, Ordering::Relaxed);
                let mut guard = lock_metrics(&last_reload_error, "last_reload_error");
                *guard = Some(e.to_string());
            }
        }
    }
}

/// Returns true when `event_path` refers to `target` (either directly or via
/// canonicalised equality), so the debouncer ignores events touching siblings
/// of the watched file inside the same directory.
fn event_path_matches(event_path: &Path, target: &Path, canonical_target: Option<&Path>) -> bool {
    if event_path == target {
        return true;
    }
    match (std::fs::canonicalize(event_path).ok(), canonical_target) {
        (Some(a), Some(b)) => a.as_path() == b,
        _ => false,
    }
}

/// Initialises `tracing_subscriber` with a filter derived from `verbose` and a
/// formatter selected by `format`.
///
/// `0` selects `info` (the default), `1` selects `debug`, and `2` or more
/// selects `trace`. The `RUST_LOG` environment variable, when set, takes
/// precedence so operators can still narrow individual modules at runtime.
/// `format` chooses between the human-readable single-line formatter
/// ([`LogFormat::Text`], the historical default) and the JSON formatter
/// ([`LogFormat::Json`]), which emits one structured object per line so log
/// shippers can ingest the stream without an intermediate regex parser.
fn init_tracing(verbose: u8, format: LogFormat) {
    let level = match verbose {
        0 => "info",
        1 => "debug",
        _ => "trace",
    };
    let env_filter =
        tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| level.into());
    match format {
        LogFormat::Text => {
            tracing_subscriber::fmt().with_env_filter(env_filter).init();
        }
        LogFormat::Json => {
            tracing_subscriber::fmt()
                .json()
                .with_env_filter(env_filter)
                .init();
        }
    }
}

/// Axum middleware that converts the empty-bodied `408 Request Timeout`
/// response emitted by [`tower_http::timeout::TimeoutLayer`] into an
/// `application/problem+json` envelope consistent with the other error
/// responses chasm produces.
///
/// The timeout layer surfaces a request-timeout by short-circuiting the inner
/// service with `StatusCode::REQUEST_TIMEOUT` and an empty body. Clients that
/// switch on the `type` URI in the problem document would otherwise see a
/// completely different response shape from the timeout path; this middleware
/// patches the body and content-type so the envelope matches `404`/`405`/`413`
/// errors, including the per-request id when one was supplied by the client.
async fn rewrite_timeout_response(
    headers: HeaderMap,
    request: Request<Body>,
    next: axum::middleware::Next,
) -> Response {
    let request_id = resolve_request_id(&headers);
    let method = request.method().clone();
    let path = request.uri().path().to_string();
    let response = next.run(request).await;
    if response.status() != StatusCode::REQUEST_TIMEOUT {
        return response;
    }
    let problem = ProblemJson {
        type_uri: "https://chasm.dev/errors#REQUEST_TIMEOUT".to_string(),
        title: "Request handler exceeded the configured timeout".to_string(),
        status: 408,
        detail: format!(
            "The handler for {} {} did not complete within the configured --request-timeout window.",
            method, path
        ),
        validation: None,
        instance: Some(ProblemJson::instance_uri_for(&request_id)),
    };
    let body_str = serde_json::to_string(&problem.to_json()).unwrap_or_default();
    let mut rewritten = (
        StatusCode::REQUEST_TIMEOUT,
        [(
            axum::http::header::CONTENT_TYPE,
            with_utf8_charset("application/problem+json"),
        )],
        body_str,
    )
        .into_response();
    if let Ok(value) = HeaderValue::try_from(request_id.as_str()) {
        rewritten
            .headers_mut()
            .insert(HeaderName::from_static("x-request-id"), value);
    }
    rewritten
}

/// Layers timeout enforcement onto `app` so that handlers exceeding `timeout`
/// return a 408 problem-document envelope instead of an empty body.
///
/// Order matters: [`tower_http::timeout::TimeoutLayer`] is the inner layer
/// (it short-circuits with [`StatusCode::REQUEST_TIMEOUT`] and an empty body)
/// and [`rewrite_timeout_response`] is the outer layer (it observes that
/// status and converts the empty body into the documented envelope). When
/// `timeout` is zero the router is returned unchanged so a zero-valued
/// `--request-timeout` flag effectively disables the cutoff.
fn apply_timeout_layer(app: Router, timeout: Duration) -> Router {
    if timeout.is_zero() {
        return app;
    }
    app.layer(tower_http::timeout::TimeoutLayer::with_status_code(
        StatusCode::REQUEST_TIMEOUT,
        timeout,
    ))
    .layer(axum::middleware::from_fn(rewrite_timeout_response))
}

/// Resolves once either `SIGINT` (Ctrl-C) or `SIGTERM` (Unix only) is observed.
///
/// Hooked into `axum::serve` via `with_graceful_shutdown` so in-flight requests
/// have a chance to drain rather than being killed mid-response when an
/// orchestrator sends a termination signal.
async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c().await.ok();
    };
    #[cfg(unix)]
    let terminate = async {
        if let Ok(mut s) = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        {
            s.recv().await;
        } else {
            std::future::pending::<()>().await;
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        _ = ctrl_c => {
            tracing::info!(
                signal = "SIGINT",
                "shutdown signal received; draining in-flight requests"
            );
        },
        _ = terminate => {
            tracing::info!(
                signal = "SIGTERM",
                "shutdown signal received; draining in-flight requests"
            );
        },
    }
}

/// Operator-controlled CORS configuration derived from the `--cors-*` flags.
///
/// `origins` empty means the historical permissive `Any` policy is used.
/// `credentials` is silently downgraded when combined with `Any` because the
/// CORS spec forbids that combination; a warning is logged so operators can
/// notice the misconfiguration.
struct CorsOptions {
    /// Explicit origin allowlist; empty means wildcard.
    origins: Vec<String>,
    /// Whether to emit `Access-Control-Allow-Credentials: true`.
    credentials: bool,
    /// Value (in seconds) for `Access-Control-Max-Age` on preflights.
    max_age: u64,
    /// Header names advertised via `Access-Control-Expose-Headers`.
    expose_headers: Vec<String>,
}

/// Constructs the CORS layer honouring the operator-supplied options.
///
/// When `opts.origins` is empty the layer keeps the permissive default
/// (`Any` for origin/methods/headers). When at least one origin is supplied,
/// the layer echoes the matching `Origin` request header and rejects others.
/// Credentials are only enabled when at least one origin is allowlisted; the
/// combination of `Any` + credentials is forbidden by the CORS spec and is
/// downgraded with a warning rather than crashing the server.
fn build_cors_layer(opts: &CorsOptions) -> CorsLayer {
    let mut layer = CorsLayer::new()
        .allow_methods(Any)
        .allow_headers(Any)
        .max_age(Duration::from_secs(opts.max_age));

    let exposed = parse_expose_headers(&opts.expose_headers);
    if !exposed.is_empty() {
        layer = layer.expose_headers(exposed);
    }

    if opts.origins.is_empty() {
        if opts.credentials {
            tracing::warn!(
                "--cors-credentials ignored because no --cors-origin was supplied; the CORS \
                 spec forbids credentials with a wildcard origin"
            );
        }
        return layer.allow_origin(Any);
    }

    let origins = parse_allowed_origins(&opts.origins);
    layer = layer.allow_origin(origins);
    if opts.credentials {
        layer = layer.allow_credentials(true);
    }
    layer
}

/// Parses operator-supplied origin strings into validated `HeaderValue`s for
/// the CORS layer's allowlist.
///
/// Invalid values are dropped with a warning so a typo cannot silently widen
/// the policy; the remaining valid entries are echoed back when the incoming
/// `Origin` header matches.
fn parse_allowed_origins(raw: &[String]) -> Vec<HeaderValue> {
    let mut out = Vec::with_capacity(raw.len());
    for origin in raw {
        match HeaderValue::try_from(origin.as_str()) {
            Ok(v) => out.push(v),
            Err(e) => {
                tracing::warn!(origin = %origin, error = %e, "ignoring invalid --cors-origin")
            }
        }
    }
    out
}

/// Parses operator-supplied expose-header names into validated `HeaderName`s.
///
/// Mirrors [`parse_allowed_origins`] in that invalid names are dropped with a
/// warning rather than aborting startup.
fn parse_expose_headers(raw: &[String]) -> Vec<HeaderName> {
    let mut out = Vec::with_capacity(raw.len());
    for name in raw {
        match HeaderName::try_from(name.as_str()) {
            Ok(v) => out.push(v),
            Err(e) => {
                tracing::warn!(name = %name, error = %e, "ignoring invalid --cors-expose-headers");
            }
        }
    }
    out
}

/// Builds the Axum router with health/metrics endpoints declared *before* the
/// spec-driven fallback.
///
/// `/healthz`, `/livez`, `/readyz`, `/metrics`, and (when `expose_spec` is
/// true) `/openapi`, `/openapi.json`, `/openapi.yaml` are bound with explicit
/// `.route(...)` calls and therefore CANNOT be overridden by a spec that
/// defines an operation under the same path: the operator endpoint always
/// wins. Every other path falls through to [`handle_request`], which dispatches
/// into the chasm engine. When `expose_spec` is false the introspection routes
/// are not bound and fall through to the spec/404 fallback.
fn build_router(state: AppState, expose_spec: bool) -> Router {
    let mut router = Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .route("/livez", get(|| async { "alive" }))
        .route("/readyz", get(readyz_handler))
        .route("/metrics", get(metrics_handler));
    if expose_spec {
        router = router
            .route("/openapi.json", get(openapi_json_handler))
            .route("/openapi.yaml", get(openapi_yaml_handler))
            .route("/openapi", get(openapi_negotiated_handler));
    }
    router.fallback(handle_request).with_state(state)
}

/// Readiness probe that surfaces the spec hot-reload status.
///
/// Returns `200 OK` with body `ok` when the most recent reload (or the
/// initial load) succeeded. Returns `503 Service Unavailable` with a body of
/// `spec reload failed: <reason>` when `--watch` has observed a deletion or a
/// parse failure since the last successful load, signalling to orchestrators
/// that the server is serving a stale spec. Liveness (`/healthz`, `/livez`)
/// remains unaffected and continues to return `200` for as long as the
/// process is alive.
async fn readyz_handler(State(state): State<AppState>) -> Response {
    let maybe_err = match state.last_reload_error.lock() {
        Ok(guard) => guard.clone(),
        Err(poisoned) => poisoned.into_inner().clone(),
    };
    match maybe_err {
        None => (StatusCode::OK, "ok").into_response(),
        Some(reason) => (
            StatusCode::SERVICE_UNAVAILABLE,
            format!("spec reload failed: {}", reason),
        )
            .into_response(),
    }
}

/// Renders the in-process metrics registry as a Prometheus text exposition
/// document, served with the standard `text/plain; version=0.0.4` content type.
async fn metrics_handler(State(state): State<AppState>) -> Response {
    let body = state.metrics.render();
    (
        StatusCode::OK,
        [(
            axum::http::header::CONTENT_TYPE,
            "text/plain; version=0.0.4".to_string(),
        )],
        body,
    )
        .into_response()
}

/// Serves the in-memory `OpenAPI` value as a pretty-printed JSON document at
/// `/openapi.json`.
///
/// The body reflects every loader normalisation (R7+R10): `type: [string,
/// "null"]` rewrites to `type: "string", nullable: true`,
/// `exclusiveMinimum`/`exclusiveMaximum` carry numeric form,
/// `items: false` is dropped after `prefixItems`, and path-item/callback
/// `$ref` entries are inlined. Always emits 200 with `application/json;
/// charset=utf-8`, `Cache-Control: no-cache`, and `X-Request-ID`.
async fn openapi_json_handler(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let spec = snapshot_spec(&state.spec);
    let request_id = resolve_request_id(&headers);
    match serde_json::to_string_pretty(spec.as_ref()) {
        Ok(body) => build_spec_response(body, "application/json; charset=utf-8", &request_id),
        Err(e) => spec_render_error(&request_id, &e.to_string()),
    }
}

/// Serves the in-memory `OpenAPI` value as a YAML document at `/openapi.yaml`.
///
/// Same normalisation guarantees as [`openapi_json_handler`]. Always emits 200
/// with `application/yaml; charset=utf-8`, `Cache-Control: no-cache`, and
/// `X-Request-ID`.
async fn openapi_yaml_handler(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let spec = snapshot_spec(&state.spec);
    let request_id = resolve_request_id(&headers);
    match serde_yaml::to_string(spec.as_ref()) {
        Ok(body) => build_spec_response(body, "application/yaml; charset=utf-8", &request_id),
        Err(e) => spec_render_error(&request_id, &e.to_string()),
    }
}

/// Serves the in-memory `OpenAPI` value at the bare `/openapi` path with
/// content negotiation driven by the `Accept` header.
///
/// `application/json` selects the JSON renderer; `application/yaml`,
/// `application/x-yaml`, and `text/yaml` select the YAML renderer; every other
/// value (including the wildcard `*/*` and an absent header) falls back to
/// JSON because it is the most portable default.
async fn openapi_negotiated_handler(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let accept = headers
        .get(axum::http::header::ACCEPT)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let want_yaml = accept_prefers_yaml(accept);
    let spec = snapshot_spec(&state.spec);
    let request_id = resolve_request_id(&headers);
    if want_yaml {
        match serde_yaml::to_string(spec.as_ref()) {
            Ok(body) => build_spec_response(body, "application/yaml; charset=utf-8", &request_id),
            Err(e) => spec_render_error(&request_id, &e.to_string()),
        }
    } else {
        match serde_json::to_string_pretty(spec.as_ref()) {
            Ok(body) => build_spec_response(body, "application/json; charset=utf-8", &request_id),
            Err(e) => spec_render_error(&request_id, &e.to_string()),
        }
    }
}

/// Reports whether the supplied `Accept` header prefers a YAML media type over
/// JSON.
///
/// Walks the comma-separated list once, returns true on the first
/// `application/yaml`, `application/x-yaml`, or `text/yaml` token (q-values are
/// ignored — the first explicit YAML mention wins, matching how `curl
/// -H 'Accept: application/yaml'` is typically used in the wild).
fn accept_prefers_yaml(accept: &str) -> bool {
    for raw in accept.split(',') {
        let token = raw
            .split(';')
            .next()
            .unwrap_or("")
            .trim()
            .to_ascii_lowercase();
        match token.as_str() {
            "application/yaml" | "application/x-yaml" | "text/yaml" => return true,
            _ => {}
        }
    }
    false
}

/// Assembles a 200 response carrying `body` with the supplied content type,
/// `Cache-Control: no-cache`, and the resolved `X-Request-ID`.
///
/// Cache-Control is set to `no-cache` so `--watch` reloads are never masked by
/// intermediary caches; `X-Request-ID` mirrors the existing per-request
/// propagation pattern.
fn build_spec_response(body: String, content_type: &'static str, request_id: &str) -> Response {
    let mut response = (
        StatusCode::OK,
        [
            (
                axum::http::header::CONTENT_TYPE,
                HeaderValue::from_static(content_type),
            ),
            (
                axum::http::header::CACHE_CONTROL,
                HeaderValue::from_static("no-cache"),
            ),
        ],
        body,
    )
        .into_response();
    if let Ok(value) = HeaderValue::try_from(request_id) {
        response
            .headers_mut()
            .insert(HeaderName::from_static("x-request-id"), value);
    }
    response
}

/// Builds the 500 response emitted when the in-memory spec cannot be
/// serialised to JSON or YAML.
///
/// Serialisation failure here would mean the spec contains data the serializer
/// rejects — exceptionally rare, but surfacing a plain-text 500 with the
/// request id is preferable to a panic.
fn spec_render_error(request_id: &str, reason: &str) -> Response {
    let mut response = (
        StatusCode::INTERNAL_SERVER_ERROR,
        format!("failed to render spec: {}", reason),
    )
        .into_response();
    if let Ok(value) = HeaderValue::try_from(request_id) {
        response
            .headers_mut()
            .insert(HeaderName::from_static("x-request-id"), value);
    }
    response
}

/// Handles all incoming requests by delegating to the mock engine.
///
/// Builds a per-request `MockConfig` by cloning the server default; the engine
/// then merges per-request overrides from the `Prefer` header and `__` query
/// parameters internally. Emits a one-line summary log after the response.
async fn handle_request(
    State(state): State<AppState>,
    method: Method,
    req: Request<Body>,
) -> Response {
    let request_id = resolve_request_id(req.headers());
    let path_for_span = req.uri().path().to_string();
    let query_for_span = req.uri().query().unwrap_or("").to_string();
    let method_for_span = method.to_string();
    let route_template = {
        let spec = snapshot_spec(&state.spec);
        match_route_template(spec.as_ref(), &path_for_span)
    };
    let span = match state.log_format {
        LogFormat::Json => tracing::info_span!(
            "request",
            request_id = %request_id,
            method = %method_for_span,
            path = %path_for_span,
            query = %query_for_span,
            route = %route_template,
        ),
        LogFormat::Text => tracing::debug_span!(
            "request",
            request_id = %request_id,
            method = %method_for_span,
            path = %path_for_span,
            query = %query_for_span,
            route = %route_template,
        ),
    };
    handle_request_inner(state, method, req, request_id, route_template)
        .instrument(span)
        .await
}

/// Inner request handler invoked from within the per-request tracing span.
///
/// Adds the resolved `X-Request-ID`, observability metrics, and the `Vary`
/// header on top of the underlying engine response. Split out from
/// [`handle_request`] only so the surrounding `info_span!` can `instrument`
/// the whole future without nesting the span body manually.
async fn handle_request_inner(
    state: AppState,
    method: Method,
    req: Request<Body>,
    request_id: String,
    route_template: String,
) -> Response {
    let _inflight = state.metrics.track_inflight();
    let start = Instant::now();
    let path = req.uri().path().to_string();
    let headers = extract_headers(req.headers());
    let query = extract_query(req.uri().query().unwrap_or(""));
    let had_prefer = headers
        .iter()
        .any(|(k, v)| k.eq_ignore_ascii_case("prefer") && !v.is_empty());
    let had_accept = headers
        .iter()
        .any(|(k, v)| k.eq_ignore_ascii_case("accept") && !v.is_empty());
    let if_none_match = headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("if-none-match"))
        .map(|(_, v)| v.clone());
    let content_type_main = headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("content-type"))
        .and_then(|(_, v)| v.split(';').next().map(|s| s.trim().to_ascii_lowercase()));
    let (parts, body_stream) = req.into_parts();
    let buffered = match axum::body::to_bytes(body_stream, MAX_BODY_BYTES).await {
        Ok(b) => b,
        Err(_) => {
            let mut resp = payload_too_large_response(method.as_str(), &path, start, &request_id);
            finalize_response(
                &mut resp,
                &state,
                &FinalizeContext {
                    request_id: &request_id,
                    method: method.as_str(),
                    route: &route_template,
                    start,
                    had_prefer,
                    had_accept,
                },
            );
            return resp;
        }
    };
    let req = Request::from_parts(parts, Body::from(buffered));
    let body = read_optional_json_body(req, content_type_main.as_deref()).await;

    let mock_req = {
        let mut __r = MockRequest::default();
        __r.method = method.to_string();
        __r.path = path.clone();
        __r.headers = headers;
        __r.query = query;
        __r.body = body;
        __r
    };

    let cfg = (*state.default_cfg).clone();
    let spec_snapshot = snapshot_spec(&state.spec);

    let mut response = match mock(&spec_snapshot, &mock_req, &cfg) {
        Ok(MockResponse {
            status,
            content_type,
            body,
            headers,
            ..
        }) => {
            let status_code =
                StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
            let bodyless_status = is_bodyless_status(status);
            if bodyless_status && !matches!(body, serde_json::Value::Null) {
                tracing::warn!(
                    status = status,
                    "spec declares a response body for {} but RFC 9110 forbids one; suppressing",
                    status
                );
            }
            let suppress_body =
                bodyless_status || method == Method::HEAD || method == Method::OPTIONS;
            let body_str = if suppress_body {
                String::new()
            } else {
                serde_json::to_string(&body).unwrap_or_default()
            };
            let effective_content_type = if bodyless_status {
                String::new()
            } else {
                content_type.clone()
            };
            let etag = if response_body_is_stable(&cfg) && status_code.is_success() {
                Some(compute_etag(body_str.as_bytes()))
            } else {
                None
            };
            if let Some(tag) = etag.as_deref() {
                if let Some(client_tag) = if_none_match.as_deref() {
                    if client_tag == tag {
                        log_request(
                            method.as_str(),
                            &path,
                            StatusCode::NOT_MODIFIED.as_u16(),
                            &content_type,
                            start,
                        );
                        let mut resp = build_bodyless_response(StatusCode::NOT_MODIFIED);
                        if let Ok(v) = HeaderValue::try_from(tag) {
                            resp.headers_mut().insert(axum::http::header::ETAG, v);
                        }
                        append_spec_headers(resp.headers_mut(), &headers);
                        resp.headers_mut().remove(axum::http::header::CONTENT_TYPE);
                        resp.headers_mut()
                            .remove(axum::http::header::CONTENT_LENGTH);
                        finalize_response(
                            &mut resp,
                            &state,
                            &FinalizeContext {
                                request_id: &request_id,
                                method: method.as_str(),
                                route: &route_template,
                                start,
                                had_prefer,
                                had_accept,
                            },
                        );
                        return resp;
                    }
                }
            }
            log_request(
                method.as_str(),
                &path,
                status_code.as_u16(),
                &content_type,
                start,
            );
            let mut resp = if effective_content_type.is_empty() {
                (status_code, body_str).into_response()
            } else {
                (
                    status_code,
                    [(
                        axum::http::header::CONTENT_TYPE,
                        with_utf8_charset(&effective_content_type),
                    )],
                    body_str,
                )
                    .into_response()
            };
            append_spec_headers(resp.headers_mut(), &headers);
            if bodyless_status {
                resp.headers_mut().remove(axum::http::header::CONTENT_TYPE);
                resp.headers_mut()
                    .remove(axum::http::header::CONTENT_LENGTH);
            }
            if let Some(tag) = etag {
                if let Ok(v) = HeaderValue::try_from(tag.as_str()) {
                    resp.headers_mut().insert(axum::http::header::ETAG, v);
                }
            }
            resp
        }
        Err(e) => {
            if matches!(
                e,
                MockError::Generation { .. }
                    | MockError::NoResponseDefined
                    | MockError::SpecSerialization(_)
            ) {
                tracing::error!(
                    error = ?e,
                    method = %method.as_str(),
                    path = %path,
                    "request handling failed"
                );
            }
            let problem = problem_for_error(&e, method.as_str(), &path, &request_id);
            let status_code =
                StatusCode::from_u16(problem.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
            let body_str = serde_json::to_string(&problem.to_json()).unwrap_or_default();
            log_request(
                method.as_str(),
                &path,
                status_code.as_u16(),
                "application/problem+json",
                start,
            );
            if let MockError::ValidationFailed(errors) = &e {
                for ve in errors {
                    let key = (ve.location.as_str().to_string(), ve.code.clone());
                    {
                        let guard = read_metrics(
                            &state.metrics.validation_errors_total,
                            "validation_errors_total",
                        );
                        if let Some(counter) = guard.get(&key) {
                            counter.fetch_add(1, Ordering::Relaxed);
                            continue;
                        }
                    }
                    let counter = {
                        let mut guard = write_metrics(
                            &state.metrics.validation_errors_total,
                            "validation_errors_total",
                        );
                        guard
                            .entry(key)
                            .or_insert_with(|| Arc::new(AtomicU64::new(0)))
                            .clone()
                    };
                    counter.fetch_add(1, Ordering::Relaxed);
                }
            }
            let www_authenticate = match &e {
                MockError::Unauthorized {
                    www_authenticate, ..
                } => www_authenticate.clone(),
                _ => None,
            };
            let mut resp = (
                status_code,
                [(
                    axum::http::header::CONTENT_TYPE,
                    with_utf8_charset("application/problem+json"),
                )],
                body_str,
            )
                .into_response();
            if let Some(www) = www_authenticate {
                if let Ok(v) = HeaderValue::try_from(www.as_str()) {
                    resp.headers_mut()
                        .insert(axum::http::header::WWW_AUTHENTICATE, v);
                }
            }
            if let MockError::MethodNotAllowed { allow, .. } = &e {
                if let Ok(v) = HeaderValue::try_from(allow.as_str()) {
                    resp.headers_mut().insert(axum::http::header::ALLOW, v);
                }
            }
            if let MockError::ValidationFailed(errors) = &e {
                let violations = validation_errors_to_json(errors);
                if let Ok(serialised) = serde_json::to_string(&violations) {
                    if let Ok(v) = HeaderValue::try_from(serialised.as_str()) {
                        resp.headers_mut()
                            .insert(HeaderName::from_static("sl-violations"), v);
                    }
                }
            }
            resp
        }
    };

    finalize_response(
        &mut response,
        &state,
        &FinalizeContext {
            request_id: &request_id,
            method: method.as_str(),
            route: &route_template,
            start,
            had_prefer,
            had_accept,
        },
    );
    response
}

/// Matches an incoming concrete request path against the spec's path
/// templates and returns the winning template (e.g. `/pets/{petId}`).
///
/// Used purely for metric labelling so the `route` label on
/// `chasm_requests_total` carries the OAS3 path template rather than the
/// concrete URL — keeping cardinality bounded by spec size rather than by
/// the number of distinct user IDs seen at runtime. Returns the literal
/// `_unmatched` sentinel when no template matches, again to bound
/// cardinality (otherwise a high-volume client probing arbitrary URLs would
/// blow up the metrics map). When several templates match the concrete
/// path, the one with the most literal segments wins, mirroring the
/// router's specificity preference. A trailing slash on the request is
/// normalised away (except for the root `/`) before matching.
fn match_route_template(spec: &OpenAPI, path: &str) -> String {
    let normalised = if path.len() > 1 && path.ends_with('/') {
        &path[..path.len() - 1]
    } else {
        path
    };
    let mut best: Option<(usize, &str)> = None;
    for template in spec.paths.paths.keys() {
        if !template_matches_path(template, normalised) {
            continue;
        }
        let literal_count = template.split('/').filter(|s| !s.starts_with('{')).count();
        let is_better = best.map(|(c, _)| literal_count > c).unwrap_or(true);
        if is_better {
            best = Some((literal_count, template.as_str()));
        }
    }
    match best {
        Some((_, t)) => t.to_string(),
        None => "_unmatched".to_string(),
    }
}

/// Reports whether `template` matches the concrete request `path`, where
/// `{name}` segments in the template bind to a single non-empty segment.
///
/// Mixed segments (e.g. `users-{id}-v{version}`) are matched conservatively:
/// the template literal must appear in the concrete segment with `{…}`
/// placeholders standing in for non-empty runs. Used exclusively by
/// [`match_route_template`] for metric labelling and intentionally narrower
/// than the engine's matcher (which also handles server base-path stripping
/// and percent-decoding); a no-match here just yields the `_unmatched`
/// sentinel rather than a routing failure.
fn template_matches_path(template: &str, path: &str) -> bool {
    let t_segs: Vec<&str> = template.split('/').collect();
    let p_segs: Vec<&str> = path.split('/').collect();
    if t_segs.len() != p_segs.len() {
        return false;
    }
    for (t, p) in t_segs.iter().zip(p_segs.iter()) {
        if !segment_matches(t, p) {
            return false;
        }
    }
    true
}

/// Reports whether a template segment matches a concrete request segment.
///
/// Pure `{name}` segments bind to any non-empty concrete segment; literal
/// segments must match byte-for-byte. Mixed segments fall back to a literal
/// equality check (callers that need the engine-grade mixed-segment matcher
/// should use the engine's router directly).
fn segment_matches(template: &str, actual: &str) -> bool {
    if template.starts_with('{')
        && template.ends_with('}')
        && !template[1..template.len() - 1].contains('{')
    {
        return !actual.is_empty();
    }
    template == actual
}

/// Returns the request id to use for this request: the verbatim incoming
/// `X-Request-ID` header when present and non-empty, otherwise a freshly
/// generated UUIDv4.
fn resolve_request_id(headers: &HeaderMap) -> String {
    if let Some(value) = headers.get("x-request-id") {
        if let Ok(s) = value.to_str() {
            let trimmed = s.trim();
            if !trimmed.is_empty() {
                return trimmed.to_string();
            }
        }
    }
    Uuid::new_v4().to_string()
}

/// Stamps the response with the resolved `X-Request-ID`, computes the `Vary`
/// header from the set of request headers that influenced the response, and
/// records the per-request metrics counters and duration histogram.
///
/// `Vary` only lists headers we actually read on the request side:
/// - `Prefer` when the request carried a non-empty `Prefer` header,
/// - `Accept` when the request carried a non-empty `Accept` header.
///
/// `X-Forwarded-*` is intentionally omitted (currently unused).
/// Per-request context carried through [`finalize_response`].
///
/// Grouping the cross-cutting metadata (identifier, method, matched route
/// template, start instant, and which `Vary`-relevant headers the caller
/// observed) into a single struct keeps the finaliser's argument list inside
/// clippy's `too_many_arguments` budget without resorting to `#[allow(...)]`.
struct FinalizeContext<'a> {
    /// Resolved `X-Request-ID` for this request.
    request_id: &'a str,
    /// HTTP method (e.g. `GET`) as a borrowed string slice.
    method: &'a str,
    /// OAS3 path template (`/pets/{petId}`) or `_unmatched` for the 404 path.
    route: &'a str,
    /// Wall-clock instant the request entered the handler; used to record the
    /// duration histogram bucket on the way out.
    start: Instant,
    /// Whether the request carried a non-empty `Prefer` header (drives `Vary`).
    had_prefer: bool,
    /// Whether the request carried a non-empty `Accept` header (drives `Vary`).
    had_accept: bool,
}

fn finalize_response(response: &mut Response, state: &AppState, ctx: &FinalizeContext<'_>) {
    if let Ok(value) = HeaderValue::try_from(ctx.request_id) {
        response
            .headers_mut()
            .insert(HeaderName::from_static("x-request-id"), value);
    }

    let mut vary_parts: Vec<&str> = Vec::new();
    if ctx.had_prefer {
        vary_parts.push("Prefer");
    }
    if ctx.had_accept {
        vary_parts.push("Accept");
    }
    if !vary_parts.is_empty() {
        if let Ok(value) = HeaderValue::try_from(vary_parts.join(", ").as_str()) {
            response
                .headers_mut()
                .insert(axum::http::header::VARY, value);
        }
    }

    let status = response.status().as_u16();
    state.metrics.record_request(ctx.method, ctx.route, status);
    let elapsed_secs = ctx.start.elapsed().as_secs_f64();
    state.metrics.record_duration(ctx.method, elapsed_secs);
}

/// Reports whether the response body for the supplied [`MockConfig`] is
/// expected to be bit-stable across repeated calls.
///
/// Static mode (`!cfg.dynamic`) returns the same example bytes every time,
/// and dynamic mode is reproducible when an explicit `cfg.seed` is supplied.
/// Both cases are safe to attach a strong ETag to; any other combination is
/// non-deterministic and must remain ETag-less.
fn response_body_is_stable(cfg: &MockConfig) -> bool {
    !cfg.dynamic || cfg.seed.is_some()
}

/// Computes a strong ETag for the supplied response body bytes.
///
/// Uses blake3 (fast non-crypto hash) and returns the conventional double-quoted
/// form `"<hex>"` truncated to a 16-character hex prefix (8 bytes of entropy),
/// which is plenty for cache validation while keeping the header compact.
fn compute_etag(body: &[u8]) -> String {
    let hash = blake3::hash(body);
    let hex = hash.to_hex();
    format!("\"{}\"", &hex.as_str()[..16])
}

/// Captures the current spec under a read lock and returns a cheap `Arc` clone.
///
/// The lock is released before this function returns, so subsequent reloads
/// in the watcher thread can swap the cell without waiting for in-flight
/// requests to finish.
fn snapshot_spec(spec: &Arc<RwLock<Arc<OpenAPI>>>) -> Arc<OpenAPI> {
    spec.read()
        .map(|guard| guard.clone())
        .unwrap_or_else(|poisoned| poisoned.into_inner().clone())
}

/// Builds the `413 Payload Too Large` response emitted when the body exceeds
/// [`MAX_BODY_BYTES`].
///
/// Returns a chasm-specific problem document: clients can still switch on
/// the `type` URI alongside the other error classes.
fn payload_too_large_response(
    method: &str,
    path: &str,
    start: Instant,
    request_id: &str,
) -> Response {
    let problem = ProblemJson {
        type_uri: TYPE_PAYLOAD_TOO_LARGE.to_string(),
        title: "Request body too large".to_string(),
        status: 413,
        detail: format!("Request body exceeds the {}-byte limit.", MAX_BODY_BYTES),
        validation: None,
        instance: Some(ProblemJson::instance_uri_for(request_id)),
    };
    let body_str = serde_json::to_string(&problem.to_json()).unwrap_or_default();
    log_request(
        method,
        path,
        problem.status,
        "application/problem+json",
        start,
    );
    (
        StatusCode::PAYLOAD_TOO_LARGE,
        [(
            axum::http::header::CONTENT_TYPE,
            with_utf8_charset("application/problem+json"),
        )],
        body_str,
    )
        .into_response()
}

/// Returns `content_type` with a `; charset=utf-8` parameter appended when the
/// media type is JSON or text and the caller did not already supply a charset.
///
/// Per RFC 8259 §8.1 JSON is UTF-8 by spec, but some defensive HTTP clients
/// still sniff the response body as Latin-1 unless the server states the
/// charset explicitly. By convention we annotate every
/// `application/...json` (including `application/problem+json`),
/// `application/json`, and any `text/*` media type with `charset=utf-8` when
/// the upstream value does not already carry a `charset` parameter. Other
/// media types (e.g. `application/octet-stream`, `image/png`) are returned
/// verbatim because appending a charset to a binary type would be misleading.
fn with_utf8_charset(content_type: &str) -> String {
    if content_type.is_empty() {
        return content_type.to_string();
    }
    let lower = content_type.to_ascii_lowercase();
    if lower.contains("charset=") {
        return content_type.to_string();
    }
    let (main_part, _) = content_type.split_once(';').unwrap_or((content_type, ""));
    let main_lower = main_part.trim().to_ascii_lowercase();
    let is_json = main_lower == "application/json"
        || main_lower.starts_with("application/")
            && (main_lower.ends_with("+json") || main_lower.contains("/json"));
    let is_text = main_lower.starts_with("text/");
    if !is_json && !is_text {
        return content_type.to_string();
    }
    let trimmed = content_type.trim_end();
    if trimmed.ends_with(';') {
        format!("{} charset=utf-8", trimmed)
    } else {
        format!("{}; charset=utf-8", trimmed)
    }
}

/// RFC 7807 problem document used for every error response chasm emits.
struct ProblemJson {
    /// Stable URI identifier for the error class.
    type_uri: String,
    /// Short human-readable summary of the error class.
    title: String,
    /// HTTP status code associated with this occurrence.
    status: u16,
    /// Human-readable explanation specific to this occurrence.
    detail: String,
    /// Optional per-field validation array attached to 422 responses.
    validation: Option<Vec<serde_json::Value>>,
    /// RFC 7807 `instance` URI identifying this specific occurrence; populated
    /// from the per-request id so operators can correlate the body with the
    /// `X-Request-ID` header and the access log.
    instance: Option<String>,
}

impl ProblemJson {
    /// Renders the problem document into its JSON wire shape.
    fn to_json(&self) -> serde_json::Value {
        let mut map = serde_json::Map::new();
        map.insert(
            "type".into(),
            serde_json::Value::String(self.type_uri.clone()),
        );
        map.insert(
            "title".into(),
            serde_json::Value::String(self.title.clone()),
        );
        map.insert(
            "status".into(),
            serde_json::Value::Number(self.status.into()),
        );
        map.insert(
            "detail".into(),
            serde_json::Value::String(self.detail.clone()),
        );
        if let Some(v) = self.validation.as_ref() {
            map.insert("validation".into(), serde_json::Value::Array(v.clone()));
        }
        if let Some(uri) = self.instance.as_ref() {
            map.insert("instance".into(), serde_json::Value::String(uri.clone()));
        }
        serde_json::Value::Object(map)
    }

    /// Returns the conventional `urn:chasm:request:<id>` URI used to populate
    /// the RFC 7807 `instance` field from the per-request id.
    fn instance_uri_for(request_id: &str) -> String {
        format!("urn:chasm:request:{}", request_id)
    }
}

/// Maps a [`MockError`] to its `ProblemJson` envelope.
///
/// Type URIs match well-known constants in the OAS3 mock-server ecosystem so
/// existing clients keep switching on them without code changes.
fn problem_for_error(err: &MockError, method: &str, path: &str, request_id: &str) -> ProblemJson {
    let instance = Some(ProblemJson::instance_uri_for(request_id));
    match err {
        MockError::NoRoute { .. } => ProblemJson {
            type_uri: TYPE_NO_PATH_MATCHED.to_string(),
            title: "Route not resolved, no path matched".to_string(),
            status: 404,
            detail: format!("The route {} {} wasn't found in the spec.", method, path),
            validation: None,
            instance,
        },
        MockError::MethodNotAllowed { .. } => ProblemJson {
            type_uri: TYPE_NO_METHOD_MATCHED.to_string(),
            title: "Route resolved, but no method matched".to_string(),
            status: 405,
            detail: format!("The method {} is not allowed for {}.", method, path),
            validation: None,
            instance,
        },
        MockError::Generation {
            method: gen_method,
            path: gen_path,
            source,
        } => ProblemJson {
            type_uri: TYPE_NO_RESPONSE_RESPONSE_DEFINED.to_string(),
            title: "Response body generation failed".to_string(),
            status: 500,
            detail: format!(
                "Failed to generate a response body for {} {}: {}",
                gen_method, gen_path, source
            ),
            validation: None,
            instance,
        },
        MockError::NoResponseDefined => ProblemJson {
            type_uri: TYPE_NO_RESPONSE_RESPONSE_DEFINED.to_string(),
            title: "No response defined for the selected operation".to_string(),
            status: 500,
            detail: "chasm cannot find a response definition matching the request".to_string(),
            validation: None,
            instance,
        },
        MockError::SpecSerialization(msg) => ProblemJson {
            type_uri: TYPE_NO_RESPONSE_RESPONSE_DEFINED.to_string(),
            title: "No response defined for the selected operation".to_string(),
            status: 500,
            detail: format!("Spec serialization error: {}", msg),
            validation: None,
            instance,
        },
        MockError::ValidationFailed(errors) => ProblemJson {
            type_uri: TYPE_UNPROCESSABLE_ENTITY.to_string(),
            title: "Invalid request".to_string(),
            status: 422,
            detail: "Your request is not valid and no HTTP validation response was found in the spec, so chasm is generating this error for you.".to_string(),
            validation: Some(validation_errors_to_json(errors)),
            instance,
        },
        MockError::Unauthorized { .. } => ProblemJson {
            type_uri: TYPE_UNAUTHORIZED.to_string(),
            title: "Invalid security scheme used".to_string(),
            status: 401,
            detail: "Your request does not fullfil the security requirements and no HTTP unauthorized response was found in the spec, so chasm is generating this error for you.".to_string(),
            validation: None,
            instance,
        },
        MockError::ExampleNotFound { content_type, example_key } => ProblemJson {
            type_uri: TYPE_NOT_FOUND.to_string(),
            title: "The server cannot find the requested content".to_string(),
            status: 404,
            detail: format!(
                "Response for contentType: {} and exampleKey: {} does not exist.",
                content_type, example_key
            ),
            validation: None,
            instance,
        },
        MockError::NoResponseForCode { method: m, path: p, code } => ProblemJson {
            type_uri: TYPE_NO_RESPONSE_DEFINED.to_string(),
            title: "The server cannot find the requested content".to_string(),
            status: 404,
            detail: format!(
                "The response code {} is not defined for {} {} in the spec.",
                code, m, p
            ),
            validation: None,
            instance,
        },
        MockError::NotAcceptable { acceptable } => ProblemJson {
            type_uri: TYPE_NOT_ACCEPTABLE.to_string(),
            title: "The server cannot produce a representation for your accept header".to_string(),
            status: 406,
            detail: format!(
                "Available content types: {}.",
                acceptable.join(", ")
            ),
            validation: None,
            instance,
        },
        _ => ProblemJson {
            type_uri: TYPE_INTERNAL_SERVER_ERROR.to_string(),
            title: "Internal server error".to_string(),
            status: 500,
            detail: err.to_string(),
            validation: None,
            instance,
        },
    }
}

/// Renders the validation errors list into the JSON shape the client receives.
///
/// Each entry uses the structured diagnostic envelope:
/// `{ location: [<area>, <field>], severity, code, message }`. `location` is an
/// array of path segments combining the input area (`"path"`, `"query"`,
/// `"header"`, `"body"`) with the failing field name (or `"body"` for the body
/// itself).
fn validation_errors_to_json(errors: &[ValidationError]) -> Vec<serde_json::Value> {
    errors
        .iter()
        .map(|e| {
            serde_json::json!({
                "location": [e.location.as_str(), e.field.as_str()],
                "severity": e.severity.as_str(),
                "code": e.code,
                "message": e.message,
            })
        })
        .collect()
}

/// Reads the request body into an optional JSON value based on `content_type`.
///
/// Recognises `application/json` (parsed directly) and
/// `application/x-www-form-urlencoded` (parsed into a flat JSON object where
/// keys appearing multiple times become arrays and all values stay strings,
/// matching the conventional form-body parsing).
///
/// For non-JSON, non-form bodies whose content-type is `multipart/form-data`,
/// `application/octet-stream`, or any `text/*` variant, this returns a
/// placeholder `Value::String("<binary {content-type}>")` rather than `None`.
/// The placeholder signals to body validation that a body was supplied — so a
/// `required: true` body annotation cannot falsely fail with a "body is
/// required" 422 — while still skipping schema validation against the JSON
/// schema declared for those content types (the placeholder is a string,
/// which the validator type-checks against the actual schema and will skip
/// gracefully for non-string schemas).
///
/// Returns `None` for empty bodies, fully unrecognised content types, or parse
/// failures.
async fn read_optional_json_body(
    req: Request<Body>,
    content_type: Option<&str>,
) -> Option<serde_json::Value> {
    let ct = content_type?;
    let body = req.into_body();
    let bytes = axum::body::to_bytes(body, usize::MAX).await.ok()?;
    if bytes.is_empty() {
        return None;
    }
    match ct {
        "application/json" => serde_json::from_slice(&bytes).ok(),
        "application/x-www-form-urlencoded" => Some(parse_form_urlencoded_to_json(bytes.as_ref())),
        "multipart/form-data" | "application/octet-stream" => {
            Some(serde_json::Value::String(format!("<binary {}>", ct)))
        }
        other if other.starts_with("text/") => {
            Some(serde_json::Value::String(format!("<binary {}>", other)))
        }
        _ => None,
    }
}

/// Parses a URL-encoded form body into a flat JSON object.
///
/// Keys that appear once map to a string; keys that appear multiple times map
/// to a JSON array of strings (in the order they were encountered). No type
/// coercion is performed — downstream schema validation handles that.
fn parse_form_urlencoded_to_json(body_bytes: &[u8]) -> serde_json::Value {
    let mut map: serde_json::Map<String, serde_json::Value> = serde_json::Map::new();
    for (k, v) in form_urlencoded::parse(body_bytes).into_owned() {
        let new_value = serde_json::Value::String(v);
        match map.remove(&k) {
            None => {
                map.insert(k, new_value);
            }
            Some(serde_json::Value::Array(mut arr)) => {
                arr.push(new_value);
                map.insert(k, serde_json::Value::Array(arr));
            }
            Some(existing) => {
                map.insert(k, serde_json::Value::Array(vec![existing, new_value]));
            }
        }
    }
    serde_json::Value::Object(map)
}

/// Returns `true` when `status` is one of the HTTP status codes that RFC 9110
/// forbids a response body for: 1xx Informational (§15.2), 204 No Content
/// (§15.3.5), 205 Reset Content (§15.3.6), and 304 Not Modified (§15.4.5).
/// The server uses this to suppress the engine's body output and drop the
/// `Content-Type` / `Content-Length` headers so the wire response is
/// RFC-compliant even when the spec author declared a `content` map under
/// those statuses.
fn is_bodyless_status(status: u16) -> bool {
    matches!(status, 100..=199 | 204 | 205 | 304)
}

/// Builds a bodyless `Response` for a status that RFC 9110 forbids from
/// carrying a body (1xx, 204, 205, 304). Uses an explicit `Body::empty()` and routes
/// through `Response::builder` rather than `IntoResponse` so the framework
/// does not silently attach a `Content-Length: 0` derived from the body's
/// known size. Callers still strip `Content-Type` / `Content-Length` to
/// defend against subsequent `append_spec_headers` adds.
fn build_bodyless_response(status: StatusCode) -> Response {
    axum::http::Response::builder()
        .status(status)
        .body(Body::empty())
        .expect("static bodyless response builds")
}

/// Appends spec-derived response headers onto an existing Axum `HeaderMap`,
/// silently skipping entries whose name or value cannot be encoded.
fn append_spec_headers(map: &mut HeaderMap, headers: &[(String, String)]) {
    for (name, value) in headers {
        let Ok(header_name) = HeaderName::try_from(name.as_str()) else {
            continue;
        };
        let Ok(header_value) = HeaderValue::try_from(value.as_str()) else {
            continue;
        };
        map.append(header_name, header_value);
    }
}

/// Emits a single structured per-request access log line at `debug` level.
///
/// Demoted from `info` so the default `RUST_LOG=info` does not emit one line
/// per served request: at 44k RPS the access log alone was producing ~150 MB
/// of stdout per minute and contributed to a near-disk-full crash observed
/// during a load test. Startup events (port bind, watcher state, spec reload)
/// and explicit errors stay at `info`/`error`. Operators who want per-request
/// telemetry can opt back in via `RUST_LOG=debug` or `-v`.
fn log_request(method: &str, path: &str, status: u16, content_type: &str, start: Instant) {
    let latency_ms = start.elapsed().as_millis();
    tracing::debug!(
        method = method,
        path = path,
        status = status,
        content_type = content_type,
        latency_ms = latency_ms as u64,
        "{} {} -> {} {} ({}ms)",
        method,
        path,
        status,
        content_type,
        latency_ms
    );
}

/// Converts Axum's `HeaderMap` into a plain `HashMap<String, String>`.
///
/// Headers that arrive more than once on the wire are joined into a single
/// `HashMap` entry per RFC 7230 §3.2.2: the values are concatenated with the
/// `", "` separator (note the trailing space, which matches the RFC's
/// recommended form). The result is semantically equivalent to a single
/// header containing the joined value for every header whose value does not
/// itself contain a comma.
///
/// Headers whose values may legitimately contain a comma — `Cookie`,
/// `Set-Cookie`, `Authorization`, `WWW-Authenticate`, and
/// `Proxy-Authenticate` — are special-cased: the first occurrence is kept and
/// subsequent occurrences are dropped, because comma-joining them would
/// produce a syntactically different value that downstream parsers might
/// misinterpret.
fn extract_headers(headers: &HeaderMap) -> HashMap<String, String> {
    let mut out: HashMap<String, String> = HashMap::new();
    for (k, v) in headers.iter() {
        let Ok(value) = v.to_str() else { continue };
        let name = k.to_string();
        let value = value.to_string();
        let comma_unsafe = is_comma_unsafe_header(&name);
        match out.get_mut(&name) {
            None => {
                out.insert(name, value);
            }
            Some(existing) => {
                if comma_unsafe {
                    // Drop subsequent occurrences — keep the first.
                    continue;
                }
                existing.push_str(", ");
                existing.push_str(&value);
            }
        }
    }
    out
}

/// Returns true for header names whose values may themselves contain commas,
/// so the duplicate-join path in [`extract_headers`] keeps only the first
/// occurrence rather than RFC 7230 §3.2.2 comma-joining.
///
/// Compares case-insensitively against the five literal names directly so we
/// avoid the per-header `to_ascii_lowercase` `String` allocation that would
/// otherwise be paid on every header on every request.
fn is_comma_unsafe_header(name: &str) -> bool {
    const UNSAFE: [&str; 5] = [
        "cookie",
        "set-cookie",
        "authorization",
        "www-authenticate",
        "proxy-authenticate",
    ];
    UNSAFE.iter().any(|s| name.eq_ignore_ascii_case(s))
}

/// Parses a query string into a `HashMap<String, String>`.
///
/// Both keys and values are percent-decoded so `name=john%20doe` produces
/// `("name", "john doe")` — matching the path-side decoding done by the
/// router. Bytes that do not form valid UTF-8 after decoding fall back to the
/// raw, still-encoded slice rather than failing the request.
///
/// Keys that appear multiple times in the query string are joined with a
/// single `,` (the standard `style=form,explode=true` collection convention
/// from RFC 6570 / OAS3), so downstream validation still sees every supplied
/// value rather than silently dropping all but the last duplicate.
fn extract_query(query: &str) -> HashMap<String, String> {
    use percent_encoding::percent_decode_str;
    let decode = |s: &str| -> String {
        if !s.contains('%') {
            return s.to_string();
        }
        percent_decode_str(s)
            .decode_utf8()
            .map(|cow| cow.into_owned())
            .unwrap_or_else(|_| s.to_string())
    };
    let mut out: HashMap<String, String> = HashMap::new();
    for pair in query.split('&').filter(|s| !s.is_empty()) {
        let mut parts = pair.splitn(2, '=');
        let key_raw = match parts.next() {
            Some(k) => k,
            None => continue,
        };
        let val_raw = parts.next().unwrap_or("");
        let key = decode(key_raw);
        let val = decode(val_raw);
        out.entry(key)
            .and_modify(|existing| {
                existing.push(',');
                existing.push_str(&val);
            })
            .or_insert(val);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `NoRoute` errors render as a complete `NO_PATH_MATCHED_ERROR` envelope.
    #[test]
    fn test_no_route_renders_no_path_matched_envelope() {
        let err = MockError::NoRoute {
            method: "GET".into(),
            path: "/foo".into(),
        };

        let json = problem_for_error(&err, "GET", "/foo", "test-req-id").to_json();

        assert_eq!(
            json,
            serde_json::json!({
                "type": TYPE_NO_PATH_MATCHED,
                "title": "Route not resolved, no path matched",
                "status": 404,
                "detail": "The route GET /foo wasn't found in the spec.",
                "instance": "urn:chasm:request:test-req-id",
            })
        );
    }

    /// `MethodNotAllowed` errors render as a complete `NO_METHOD_MATCHED_ERROR` envelope.
    #[test]
    fn test_method_not_allowed_renders_no_method_matched_envelope() {
        let err = MockError::MethodNotAllowed {
            method: "DELETE".into(),
            path: "/pets".into(),
            allow: "GET, POST".into(),
        };

        let json = problem_for_error(&err, "DELETE", "/pets", "test-req-id").to_json();

        assert_eq!(
            json,
            serde_json::json!({
                "type": TYPE_NO_METHOD_MATCHED,
                "title": "Route resolved, but no method matched",
                "status": 405,
                "detail": "The method DELETE is not allowed for /pets.",
                "instance": "urn:chasm:request:test-req-id",
            })
        );
    }

    /// `ValidationFailed` errors render as a complete `UNPROCESSABLE_ENTITY` envelope.
    #[test]
    fn test_validation_failed_renders_unprocessable_entity_envelope() {
        let errs = vec![ValidationError::new(
            chasm_engine::ValidationLocation::Query,
            "limit",
            "type",
            "expected integer",
        )];
        let err = MockError::ValidationFailed(errs);

        let json = problem_for_error(&err, "GET", "/pets", "test-req-id").to_json();

        assert_eq!(
            json,
            serde_json::json!({
                "type": TYPE_UNPROCESSABLE_ENTITY,
                "title": "Invalid request",
                "status": 422,
                "detail": "Your request is not valid and no HTTP validation response was found in the spec, so chasm is generating this error for you.",
                "validation": [{
                    "location": ["query", "limit"],
                    "severity": "Error",
                    "code": "type",
                    "message": "expected integer",
                }],
                "instance": "urn:chasm:request:test-req-id",
            })
        );
    }

    /// `ExampleNotFound` errors render as a complete `NOT_FOUND` envelope.
    #[test]
    fn test_example_not_found_renders_not_found_envelope() {
        let err = MockError::ExampleNotFound {
            content_type: "application/json".to_string(),
            example_key: "missing".to_string(),
        };

        let json = problem_for_error(&err, "GET", "/pets", "test-req-id").to_json();

        assert_eq!(
            json,
            serde_json::json!({
                "type": TYPE_NOT_FOUND,
                "title": "The server cannot find the requested content",
                "status": 404,
                "detail": "Response for contentType: application/json and exampleKey: missing does not exist.",
                "instance": "urn:chasm:request:test-req-id",
            })
        );
    }

    /// `Unauthorized` errors render as a complete `UNAUTHORIZED` envelope (note the verbatim "fullfil" spelling).
    #[test]
    fn test_unauthorized_renders_unauthorized_envelope() {
        let err = MockError::Unauthorized {
            scheme: "bearerAuth".to_string(),
            www_authenticate: Some("Bearer realm=\"bearerAuth\"".to_string()),
        };

        let json = problem_for_error(&err, "GET", "/secure", "test-req-id").to_json();

        assert_eq!(
            json,
            serde_json::json!({
                "type": TYPE_UNAUTHORIZED,
                "title": "Invalid security scheme used",
                "status": 401,
                "detail": "Your request does not fullfil the security requirements and no HTTP unauthorized response was found in the spec, so chasm is generating this error for you.",
                "instance": "urn:chasm:request:test-req-id",
            })
        );
    }

    /// `NoResponseDefined` errors render as a complete `NO_RESPONSE_RESPONSE_DEFINED` envelope.
    #[test]
    fn test_no_response_defined_renders_no_response_response_defined_envelope() {
        let err = MockError::NoResponseDefined;

        let json = problem_for_error(&err, "GET", "/x", "test-req-id").to_json();

        assert_eq!(
            json,
            serde_json::json!({
                "type": TYPE_NO_RESPONSE_RESPONSE_DEFINED,
                "title": "No response defined for the selected operation",
                "status": 500,
                "detail": "chasm cannot find a response definition matching the request",
                "instance": "urn:chasm:request:test-req-id",
            })
        );
    }

    /// The URL-encoded form parser shapes scalar and repeated keys into one expected JSON object.
    #[test]
    fn test_form_urlencoded_parser_shapes_object() {
        let value = parse_form_urlencoded_to_json(b"name=alice&age=30&tag=a&tag=b");

        assert_eq!(
            value,
            serde_json::json!({
                "name": "alice",
                "age": "30",
                "tag": ["a", "b"],
            })
        );
    }

    /// Spec used by both form-urlencoded body validation tests below.
    const FORM_URLENCODED_VALIDATION_SPEC: &str = r#"
openapi: 3.0.0
info: { title: t, version: 1.0.0 }
paths:
  /submit:
    post:
      requestBody:
        required: true
        content:
          application/x-www-form-urlencoded:
            schema:
              type: object
              required: [name, age]
              properties:
                name: { type: string }
                age: { type: string }
      responses:
        '200':
          description: ok
          content:
            application/json:
              example: { ok: true }
"#;

    /// A URL-encoded form body that satisfies every required field returns 200.
    #[test]
    fn test_form_urlencoded_body_passes_when_required_fields_present() {
        use chasm_engine::{load_spec, mock, MockConfig, MockRequest};
        let spec = load_spec(FORM_URLENCODED_VALIDATION_SPEC).unwrap();

        let parsed_ok = parse_form_urlencoded_to_json(b"name=alice&age=30");
        let mut headers = HashMap::new();
        headers.insert(
            "Content-Type".to_string(),
            "application/x-www-form-urlencoded".to_string(),
        );
        let r_ok = {
            let mut __r = MockRequest::default();
            __r.method = "POST".to_string();
            __r.path = "/submit".to_string();
            __r.headers = headers;
            __r.query = HashMap::new();
            __r.body = Some(parsed_ok);
            __r
        };

        let resp = mock(&spec, &r_ok, &{
            let mut __r = MockConfig::default();
            __r.errors = true;
            __r
        })
        .unwrap();

        assert_eq!(resp.status, 200);
    }

    /// A URL-encoded form body missing a required field surfaces a validation entry on that field.
    #[test]
    fn test_form_urlencoded_body_rejects_when_required_field_missing() {
        use chasm_engine::{load_spec, mock, MockConfig, MockError, MockRequest};
        let spec = load_spec(FORM_URLENCODED_VALIDATION_SPEC).unwrap();

        let parsed_missing = parse_form_urlencoded_to_json(b"name=alice");
        let mut headers = HashMap::new();
        headers.insert(
            "Content-Type".to_string(),
            "application/x-www-form-urlencoded".to_string(),
        );
        let r_bad = {
            let mut __r = MockRequest::default();
            __r.method = "POST".to_string();
            __r.path = "/submit".to_string();
            __r.headers = headers;
            __r.query = HashMap::new();
            __r.body = Some(parsed_missing);
            __r
        };

        let err = mock(&spec, &r_bad, &{
            let mut __r = MockConfig::default();
            __r.errors = true;
            __r
        })
        .unwrap_err();

        match err {
            MockError::ValidationFailed(errors) => {
                assert!(errors.iter().any(|e| e.field.ends_with(".age")));
            }
            other => panic!("expected ValidationFailed, got {:?}", other),
        }
    }

    /// Verifies that `-vvv` increments the count argument to `3`, which the
    /// runtime filter selector then maps to the `trace` level.
    #[test]
    fn test_verbose_flag_parses_count() {
        let cli = Cli::try_parse_from(["chasm-server", "spec.yml", "-vvv"]).expect("parse");
        assert_eq!(cli.mock_args.verbose, 3);
    }

    /// `--json-schema-faker-fill-properties` as a bare flag enables the fill-properties behaviour.
    #[test]
    fn test_json_schema_faker_fill_properties_bare_enables() {
        let bare = Cli::try_parse_from([
            "chasm-server",
            "spec.yml",
            "--json-schema-faker-fill-properties",
        ])
        .expect("bare parse");

        assert!(bare.mock_args.json_schema_faker_fill_properties);
    }

    /// `--json-schema-faker-fill-properties=false` explicitly disables the fill-properties behaviour.
    #[test]
    fn test_json_schema_faker_fill_properties_explicit_false_disables() {
        let explicit_false = Cli::try_parse_from([
            "chasm-server",
            "spec.yml",
            "--json-schema-faker-fill-properties=false",
        ])
        .expect("explicit parse");

        assert!(!explicit_false.mock_args.json_schema_faker_fill_properties);
    }

    /// Omitting `--json-schema-faker-fill-properties` retains the default `true` value.
    #[test]
    fn test_json_schema_faker_fill_properties_default_is_true() {
        let default = Cli::try_parse_from(["chasm-server", "spec.yml"]).expect("default");

        assert!(default.mock_args.json_schema_faker_fill_properties);
    }

    /// Verifies that a request body larger than [`MAX_BODY_BYTES`] is rejected
    /// with a `413 Payload Too Large` problem document.
    #[tokio::test]
    async fn test_oversized_body_returns_413() {
        use axum::http::Request as HttpRequest;
        use tower::ServiceExt;

        let yaml = r#"
openapi: 3.0.0
info: { title: t, version: 1.0.0 }
paths:
  /echo:
    post:
      responses:
        '200':
          description: ok
"#;
        let spec = load_spec(yaml).expect("spec");
        let state = AppState {
            spec: Arc::new(RwLock::new(Arc::new(spec))),
            default_cfg: Arc::new(MockConfig::default()),
            metrics: Arc::new(Metrics::default()),
            last_reload_error: Arc::new(Mutex::new(None)),
            log_format: LogFormat::Text,
        };
        let app = build_router(state, true);

        let big = vec![b'x'; 20 * 1024 * 1024];
        let request = HttpRequest::builder()
            .method("POST")
            .uri("/echo")
            .header("content-type", "application/octet-stream")
            .body(Body::from(big))
            .expect("request");

        let response = app.oneshot(request).await.expect("response");
        assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("collect");
        let json: serde_json::Value = serde_json::from_slice(&bytes).expect("json");
        assert_eq!(json["type"], TYPE_PAYLOAD_TOO_LARGE,);
        assert_eq!(json["status"], 413);
    }

    /// A request body larger than [`MAX_BODY_BYTES`] surfaces a 413 envelope whose
    /// `title` and `detail` strings document the limit overflow.
    #[tokio::test]
    async fn test_payload_too_large_includes_title_and_detail() {
        use axum::http::Request as HttpRequest;
        use tower::ServiceExt;

        let yaml = r#"
openapi: 3.0.0
info: { title: t, version: 1.0.0 }
paths:
  /echo:
    post:
      responses:
        '200':
          description: ok
"#;
        let spec = load_spec(yaml).expect("spec");
        let state = AppState {
            spec: Arc::new(RwLock::new(Arc::new(spec))),
            default_cfg: Arc::new(MockConfig::default()),
            metrics: Arc::new(Metrics::default()),
            last_reload_error: Arc::new(Mutex::new(None)),
            log_format: LogFormat::Text,
        };
        let app = build_router(state, true);
        let big = vec![b'x'; 20 * 1024 * 1024];
        let request = HttpRequest::builder()
            .method("POST")
            .uri("/echo")
            .header("content-type", "application/octet-stream")
            .body(Body::from(big))
            .expect("request");

        let response = app.oneshot(request).await.expect("response");
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("collect");
        let json: serde_json::Value = serde_json::from_slice(&bytes).expect("json");

        let title = json["title"].as_str().unwrap_or("").trim().to_string();
        let detail = json["detail"].as_str().unwrap_or("").trim().to_string();

        assert!(
            !title.is_empty() && !detail.is_empty(),
            "expected non-empty title and detail, got title={title:?}, detail={detail:?}"
        );
    }

    /// Builds a minimal `AppState` backed by the supplied YAML spec, for use in
    /// the observability tests below.
    fn build_test_state(yaml: &str) -> AppState {
        let spec = load_spec(yaml).expect("spec");
        AppState {
            spec: Arc::new(RwLock::new(Arc::new(spec))),
            default_cfg: Arc::new(MockConfig::default()),
            metrics: Arc::new(Metrics::default()),
            last_reload_error: Arc::new(Mutex::new(None)),
            log_format: LogFormat::Text,
        }
    }

    /// The simplest spec that resolves a `GET /pets` request to a 200 JSON
    /// response — shared by several observability tests.
    fn minimal_get_pets_spec() -> &'static str {
        r#"
openapi: 3.0.0
info: { title: t, version: 1.0.0 }
paths:
  /pets:
    get:
      responses:
        '200':
          description: ok
          content:
            application/json:
              example: { ok: true }
"#
    }

    /// Verifies that `GET /healthz` returns `200 ok` even when the spec does
    /// not declare the path, confirming the operator route wins over the
    /// spec-driven fallback.
    #[tokio::test]
    async fn test_health_endpoint_returns_200() {
        use axum::http::Request as HttpRequest;
        use tower::ServiceExt;

        let app = build_router(build_test_state(minimal_get_pets_spec()), true);
        let response = app
            .oneshot(
                HttpRequest::builder()
                    .method("GET")
                    .uri("/healthz")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("collect");
        assert_eq!(&bytes[..], b"ok");
    }

    /// Verifies that `GET /livez` returns `200 alive`, the Kubernetes-style
    /// liveness probe response.
    #[tokio::test]
    async fn test_livez_endpoint_returns_200() {
        use axum::http::Request as HttpRequest;
        use tower::ServiceExt;

        let app = build_router(build_test_state(minimal_get_pets_spec()), true);
        let response = app
            .oneshot(
                HttpRequest::builder()
                    .method("GET")
                    .uri("/livez")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("collect");
        assert_eq!(&bytes[..], b"alive");
    }

    /// Drives one `GET /pets` mock request through the supplied router so
    /// at least one Prometheus counter is registered before `/metrics` is queried.
    async fn seed_one_request(app: &Router) {
        use axum::http::Request as HttpRequest;
        use tower::ServiceExt;
        let _ = app
            .clone()
            .oneshot(
                HttpRequest::builder()
                    .method("GET")
                    .uri("/pets")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
    }

    /// `GET /metrics` returns a `text/plain; version=0.0.4` content type.
    #[tokio::test]
    async fn test_metrics_endpoint_returns_prometheus_content_type() {
        use axum::http::Request as HttpRequest;
        use tower::ServiceExt;

        let app = build_router(build_test_state(minimal_get_pets_spec()), true);
        seed_one_request(&app).await;
        let response = app
            .oneshot(
                HttpRequest::builder()
                    .method("GET")
                    .uri("/metrics")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");

        let ct = response
            .headers()
            .get(axum::http::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();

        assert_eq!(ct, "text/plain; version=0.0.4");
    }

    /// `GET /metrics` emits a `# TYPE chasm_requests_total counter` declaration line.
    #[tokio::test]
    async fn test_metrics_endpoint_declares_requests_total_counter() {
        use axum::http::Request as HttpRequest;
        use tower::ServiceExt;

        let app = build_router(build_test_state(minimal_get_pets_spec()), true);
        seed_one_request(&app).await;
        let response = app
            .oneshot(
                HttpRequest::builder()
                    .method("GET")
                    .uri("/metrics")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("collect");
        let body = std::str::from_utf8(&bytes).expect("utf8").to_string();

        assert!(body.contains("# TYPE chasm_requests_total counter"));
    }

    /// `GET /metrics` emits a `# TYPE chasm_request_duration_seconds histogram` declaration line.
    #[tokio::test]
    async fn test_metrics_endpoint_declares_request_duration_histogram() {
        use axum::http::Request as HttpRequest;
        use tower::ServiceExt;

        let app = build_router(build_test_state(minimal_get_pets_spec()), true);
        seed_one_request(&app).await;
        let response = app
            .oneshot(
                HttpRequest::builder()
                    .method("GET")
                    .uri("/metrics")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("collect");
        let body = std::str::from_utf8(&bytes).expect("utf8").to_string();

        assert!(body.contains("# TYPE chasm_request_duration_seconds histogram"));
    }

    /// `GET /metrics` emits a labelled `chasm_requests_total` sample line for the seeded request.
    #[tokio::test]
    async fn test_metrics_endpoint_emits_labelled_requests_total_sample() {
        use axum::http::Request as HttpRequest;
        use tower::ServiceExt;

        let app = build_router(build_test_state(minimal_get_pets_spec()), true);
        seed_one_request(&app).await;
        let response = app
            .oneshot(
                HttpRequest::builder()
                    .method("GET")
                    .uri("/metrics")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("collect");
        let body = std::str::from_utf8(&bytes).expect("utf8").to_string();

        assert!(
            body.contains("chasm_requests_total{method=\"GET\",route=\"/pets\",status=\"200\"}")
        );
    }

    /// Verifies that a client-supplied `X-Request-ID` is echoed back verbatim
    /// rather than overwritten by the generator.
    #[tokio::test]
    async fn test_request_id_propagated_from_header() {
        use axum::http::Request as HttpRequest;
        use tower::ServiceExt;

        let app = build_router(build_test_state(minimal_get_pets_spec()), true);
        let response = app
            .oneshot(
                HttpRequest::builder()
                    .method("GET")
                    .uri("/pets")
                    .header("x-request-id", "abc-123")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        let id = response
            .headers()
            .get("x-request-id")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        assert_eq!(id, "abc-123");
    }

    /// Verifies that an absent `X-Request-ID` triggers a freshly generated
    /// UUIDv4 (36 chars, four hyphens), so downstream logs can still group
    /// events by request.
    #[tokio::test]
    async fn test_request_id_generated_when_absent() {
        use axum::http::Request as HttpRequest;
        use tower::ServiceExt;

        let app = build_router(build_test_state(minimal_get_pets_spec()), true);
        let response = app
            .oneshot(
                HttpRequest::builder()
                    .method("GET")
                    .uri("/pets")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        let id = response
            .headers()
            .get("x-request-id")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        assert_eq!(id.len(), 36, "expected uuid v4 length, got {:?}", id);
        assert_eq!(id.matches('-').count(), 4);
    }

    /// Sends a `GET /pets` request carrying both `Prefer` and `Accept` and returns the response Vary header.
    async fn vary_header_for_prefer_request() -> String {
        use axum::http::Request as HttpRequest;
        use tower::ServiceExt;

        let app = build_router(build_test_state(minimal_get_pets_spec()), true);
        let response = app
            .oneshot(
                HttpRequest::builder()
                    .method("GET")
                    .uri("/pets")
                    .header("prefer", "code=200")
                    .header("accept", "application/json")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        response
            .headers()
            .get(axum::http::header::VARY)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string()
    }

    /// The response Vary header lists `Prefer` after a request that used `Prefer`.
    #[tokio::test]
    async fn test_vary_header_lists_prefer_when_prefer_used() {
        let vary = vary_header_for_prefer_request().await;

        assert!(vary.contains("Prefer"), "vary header was {vary:?}");
    }

    /// The response Vary header lists `Accept` after a request that used `Prefer`.
    #[tokio::test]
    async fn test_vary_header_lists_accept_when_prefer_used() {
        let vary = vary_header_for_prefer_request().await;

        assert!(vary.contains("Accept"), "vary header was {vary:?}");
    }

    /// Returns the response from `GET /openapi.yaml` for shared assertions below.
    async fn openapi_yaml_response() -> axum::response::Response {
        use axum::http::Request as HttpRequest;
        use tower::ServiceExt;

        let app = build_router(build_test_state(minimal_get_pets_spec()), true);
        app.oneshot(
            HttpRequest::builder()
                .method("GET")
                .uri("/openapi.yaml")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response")
    }

    /// `GET /openapi.yaml` returns `200 OK` when expose-spec is enabled.
    #[tokio::test]
    async fn test_openapi_yaml_returns_200() {
        let response = openapi_yaml_response().await;

        assert_eq!(response.status(), StatusCode::OK);
    }

    /// `GET /openapi.yaml` advertises an `application/yaml` content type when expose-spec is enabled.
    #[tokio::test]
    async fn test_openapi_yaml_returns_yaml_content_type() {
        let response = openapi_yaml_response().await;

        let ct = response
            .headers()
            .get(axum::http::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();

        assert!(ct.starts_with("application/yaml"), "ct: {}", ct);
    }

    /// `GET /openapi.yaml` sets `Cache-Control: no-cache`.
    #[tokio::test]
    async fn test_openapi_yaml_sets_no_cache() {
        let response = openapi_yaml_response().await;

        let cache = response
            .headers()
            .get(axum::http::header::CACHE_CONTROL)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();

        assert_eq!(cache, "no-cache");
    }

    /// `GET /openapi.yaml` includes an `X-Request-ID` header.
    #[tokio::test]
    async fn test_openapi_yaml_sets_x_request_id() {
        let response = openapi_yaml_response().await;

        assert!(response.headers().get("x-request-id").is_some());
    }

    /// `GET /openapi.yaml` body contains the spec keys `openapi:` and `/pets`.
    #[tokio::test]
    async fn test_openapi_yaml_body_contains_spec_keys() {
        let response = openapi_yaml_response().await;
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("collect");
        let body = std::str::from_utf8(&bytes).expect("utf8").to_string();

        assert!(body.contains("openapi:") && body.contains("/pets"));
    }

    /// Returns the response from `GET /openapi.json` for shared assertions below.
    async fn openapi_json_response() -> axum::response::Response {
        use axum::http::Request as HttpRequest;
        use tower::ServiceExt;

        let app = build_router(build_test_state(minimal_get_pets_spec()), true);
        app.oneshot(
            HttpRequest::builder()
                .method("GET")
                .uri("/openapi.json")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response")
    }

    /// `GET /openapi.json` returns `200 OK` when expose-spec is enabled.
    #[tokio::test]
    async fn test_openapi_json_returns_200() {
        let response = openapi_json_response().await;

        assert_eq!(response.status(), StatusCode::OK);
    }

    /// `GET /openapi.json` advertises an `application/json` content type when expose-spec is enabled.
    #[tokio::test]
    async fn test_openapi_json_returns_json_content_type() {
        let response = openapi_json_response().await;

        let ct = response
            .headers()
            .get(axum::http::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();

        assert!(ct.starts_with("application/json"), "ct: {}", ct);
    }

    /// `GET /openapi.json` sets `Cache-Control: no-cache`.
    #[tokio::test]
    async fn test_openapi_json_sets_no_cache() {
        let response = openapi_json_response().await;

        let cache = response
            .headers()
            .get(axum::http::header::CACHE_CONTROL)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();

        assert_eq!(cache, "no-cache");
    }

    /// `GET /openapi.json` includes an `X-Request-ID` header.
    #[tokio::test]
    async fn test_openapi_json_sets_x_request_id() {
        let response = openapi_json_response().await;

        assert!(response.headers().get("x-request-id").is_some());
    }

    /// `GET /openapi.json` body declares the spec version and exposes the `/pets` path.
    #[tokio::test]
    async fn test_openapi_json_body_declares_spec_shape() {
        let response = openapi_json_response().await;
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("collect");
        let value: serde_json::Value = serde_json::from_slice(&bytes).expect("json");

        assert!(value["openapi"] == "3.0.0" && value["paths"]["/pets"].is_object());
    }

    /// Verifies that `--expose-spec=false` unmounts the introspection routes;
    /// requests to `/openapi.json` and `/openapi.yaml` fall through to the
    /// spec-driven fallback and surface a `404` problem document.
    #[tokio::test]
    async fn test_openapi_endpoints_return_404_when_expose_spec_disabled() {
        use axum::http::Request as HttpRequest;
        use tower::ServiceExt;

        let app = build_router(build_test_state(minimal_get_pets_spec()), false);
        for path in ["/openapi.json", "/openapi.yaml", "/openapi"] {
            let response = app
                .clone()
                .oneshot(
                    HttpRequest::builder()
                        .method("GET")
                        .uri(path)
                        .body(Body::empty())
                        .expect("request"),
                )
                .await
                .expect("response");
            assert_eq!(
                response.status(),
                StatusCode::NOT_FOUND,
                "expected 404 for {path}"
            );
        }
    }

    /// `GET /openapi.json` serves the *normalised* in-memory spec, so a
    /// `type: [string, "null"]` source declaration surfaces as the OAS 3.0
    /// `{type: "string", nullable: true}` shape.
    #[tokio::test]
    async fn test_openapi_endpoint_reflects_normalised_spec() {
        use axum::http::Request as HttpRequest;
        use tower::ServiceExt;

        let yaml = r#"
openapi: 3.0.0
info: { title: t, version: 1.0.0 }
paths:
  /pets:
    get:
      responses:
        '200':
          description: ok
          content:
            application/json:
              schema:
                type: object
                properties:
                  name:
                    type: [string, "null"]
"#;
        let app = build_router(build_test_state(yaml), true);
        let response = app
            .oneshot(
                HttpRequest::builder()
                    .method("GET")
                    .uri("/openapi.json")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("collect");
        let value: serde_json::Value = serde_json::from_slice(&bytes).expect("json");
        let name_schema = &value["paths"]["/pets"]["get"]["responses"]["200"]["content"]
            ["application/json"]["schema"]["properties"]["name"];

        assert_eq!(
            name_schema,
            &serde_json::json!({ "type": "string", "nullable": true })
        );
    }

    /// Minimal in-memory spec used by the validate / dry-run / stdin tests.
    const MINIMAL_SPEC_YAML: &str = r#"openapi: 3.0.0
info: { title: t, version: 1.0.0 }
paths:
  /pets:
    get:
      responses:
        '200':
          description: ok
          content:
            application/json:
              example: { ok: true }
components:
  schemas:
    Pet:
      type: object
      properties:
        id: { type: integer }
"#;

    /// Verifies that the `validate` subcommand succeeds on a well-formed spec
    /// by exercising the same loader path the subcommand uses.
    #[test]
    fn test_validate_subcommand_succeeds_on_valid_spec() {
        let dir = std::env::temp_dir();
        let path = dir.join("chasm_validate_ok.yaml");
        std::fs::write(&path, MINIMAL_SPEC_YAML).expect("write");
        let cli = Cli::try_parse_from(["chasm-server", "validate", path.to_str().unwrap()])
            .expect("parse");
        match cli.command {
            Some(Command::Validate(args)) => {
                let result = load_spec_from_arg(&args.spec);
                assert!(result.is_ok(), "expected Ok, got {:?}", result.err());
            }
            other => panic!("expected Validate subcommand, got {:?}", other),
        }
        let _ = std::fs::remove_file(&path);
    }

    /// Verifies that the `validate` subcommand surfaces a parse error when the
    /// spec is malformed YAML/JSON, leaving exit handling to the caller.
    #[test]
    fn test_validate_subcommand_fails_on_malformed() {
        let dir = std::env::temp_dir();
        let path = dir.join("chasm_validate_bad.yaml");
        std::fs::write(&path, "this is: not: a: valid: spec: at all").expect("write");
        let cli = Cli::try_parse_from(["chasm-server", "validate", path.to_str().unwrap()])
            .expect("parse");
        match cli.command {
            Some(Command::Validate(args)) => {
                let result = load_spec_from_arg(&args.spec);
                assert!(result.is_err(), "expected parse failure");
            }
            other => panic!("expected Validate subcommand, got {:?}", other),
        }
        let _ = std::fs::remove_file(&path);
    }

    /// Verifies that `--dry-run` parses on the implicit `mock` subcommand and
    /// sets `mock_args.dry_run` to `true`.
    #[test]
    fn test_dry_run_flag_parses_as_true() {
        let dir = std::env::temp_dir();
        let path = dir.join("chasm_dry_run_flag.yaml");
        std::fs::write(&path, MINIMAL_SPEC_YAML).expect("write");
        let cli = Cli::try_parse_from(["chasm-server", path.to_str().unwrap(), "--dry-run"])
            .expect("parse");

        assert!(cli.mock_args.dry_run);

        let _ = std::fs::remove_file(&path);
    }

    /// Verifies that the minimal spec used by the dry-run flow loads with the
    /// expected path and component-schema counts (no socket bound).
    #[test]
    fn test_dry_run_loads_spec_paths_and_components() {
        let dir = std::env::temp_dir();
        let path = dir.join("chasm_dry_run_counts.yaml");
        std::fs::write(&path, MINIMAL_SPEC_YAML).expect("write");

        let spec = load_spec(path.to_str().unwrap()).expect("load");
        let comp_count = spec
            .components
            .as_ref()
            .map(|c| c.schemas.len())
            .unwrap_or(0);

        assert_eq!(spec.paths.paths.len(), 1);
        assert_eq!(comp_count, 1);

        let _ = std::fs::remove_file(&path);
    }

    /// The literal value `-` on the implicit `mock` subcommand is routed into the stdin sentinel.
    #[test]
    fn test_stdin_loads_spec_with_hyphen_arg_for_mock() {
        let mock_cli = Cli::try_parse_from(["chasm-server", "-"]).expect("mock parse");

        assert_eq!(
            mock_cli
                .mock_args
                .spec_positional
                .as_ref()
                .map(|p| p.to_string_lossy().into_owned()),
            Some("-".to_string())
        );
    }

    /// The literal value `-` on the `validate` subcommand is routed into the stdin sentinel.
    #[test]
    fn test_stdin_loads_spec_with_hyphen_arg_for_validate() {
        let validate_cli =
            Cli::try_parse_from(["chasm-server", "validate", "-"]).expect("validate parse");

        match validate_cli.command {
            Some(Command::Validate(args)) => {
                assert_eq!(args.spec.to_string_lossy(), "-");
            }
            other => panic!("expected Validate subcommand, got {:?}", other),
        }
    }

    /// When a spec declares a `Content-Length` response header the wire response
    /// must still carry exactly one `Content-Length`, and its numeric value must
    /// match the real body byte count (rather than the literal integer the spec
    /// author wrote).
    #[tokio::test]
    async fn test_spec_content_length_header_is_not_duplicated() {
        use axum::http::Request as HttpRequest;
        use tower::ServiceExt;

        let yaml = r#"
openapi: 3.0.0
info: { title: bug-1621-server, version: 1.0.0 }
paths:
  /thing:
    get:
      responses:
        '200':
          description: ok
          headers:
            Content-Length:
              schema: { type: integer }
              example: 9999
          content:
            application/json:
              example: { ok: true }
"#;
        let app = build_router(build_test_state(yaml), true);
        let response = app
            .oneshot(
                HttpRequest::builder()
                    .method("GET")
                    .uri("/thing")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        let cls: Vec<String> = response
            .headers()
            .get_all(axum::http::header::CONTENT_LENGTH)
            .iter()
            .filter_map(|v| v.to_str().ok().map(str::to_string))
            .collect();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("collect");

        assert_eq!(cls, vec![bytes.len().to_string()]);
    }

    /// `GET /readyz` returns `200 OK` when no reload error has been recorded.
    #[tokio::test]
    async fn test_readyz_returns_200_when_spec_is_healthy() {
        use axum::http::Request as HttpRequest;
        use tower::ServiceExt;

        let app = build_router(build_test_state(minimal_get_pets_spec()), true);
        let response = app
            .oneshot(
                HttpRequest::builder()
                    .method("GET")
                    .uri("/readyz")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");

        assert_eq!(response.status(), StatusCode::OK);
    }

    /// `GET /readyz` returns `503 SERVICE_UNAVAILABLE` when the spec watcher has
    /// stored a reload failure in `last_reload_error`.
    #[tokio::test]
    async fn test_readyz_returns_503_when_spec_failed_to_reload() {
        use axum::http::Request as HttpRequest;
        use tower::ServiceExt;

        let state = build_test_state(minimal_get_pets_spec());
        if let Ok(mut guard) = state.last_reload_error.lock() {
            *guard = Some("io: spec deleted".to_string());
        }
        let app = build_router(state, true);
        let response = app
            .oneshot(
                HttpRequest::builder()
                    .method("GET")
                    .uri("/readyz")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");

        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    /// Without explicit `--cors-origin`, the CORS layer emits the wildcard `Access-Control-Allow-Origin: *` on a preflight.
    #[tokio::test]
    async fn test_cors_default_permissive_emits_wildcard_origin() {
        use axum::http::Request as HttpRequest;
        use tower::ServiceExt;

        let layer = build_cors_layer(&CorsOptions {
            origins: vec![],
            credentials: false,
            max_age: 600,
            expose_headers: vec![],
        });
        let app = build_router(build_test_state(minimal_get_pets_spec()), true).layer(layer);
        let response = app
            .oneshot(
                HttpRequest::builder()
                    .method("OPTIONS")
                    .uri("/pets")
                    .header("Origin", "https://example.com")
                    .header("Access-Control-Request-Method", "GET")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");

        let allow = response
            .headers()
            .get("access-control-allow-origin")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();

        assert_eq!(allow, "*");
    }

    /// An explicit `--cors-origin` allowlist echoes the matching request origin verbatim.
    #[tokio::test]
    async fn test_cors_explicit_origin_echoes_matching_origin() {
        use axum::http::Request as HttpRequest;
        use tower::ServiceExt;

        let layer = build_cors_layer(&CorsOptions {
            origins: vec!["https://example.com".to_string()],
            credentials: false,
            max_age: 600,
            expose_headers: vec![],
        });
        let app = build_router(build_test_state(minimal_get_pets_spec()), true).layer(layer);
        let response = app
            .oneshot(
                HttpRequest::builder()
                    .method("OPTIONS")
                    .uri("/pets")
                    .header("Origin", "https://example.com")
                    .header("Access-Control-Request-Method", "GET")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");

        let allow = response
            .headers()
            .get("access-control-allow-origin")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();

        assert_eq!(allow, "https://example.com");
    }

    /// An origin that does not appear in the `--cors-origin` allowlist receives no `Access-Control-Allow-Origin` header.
    #[tokio::test]
    async fn test_cors_unmatched_origin_receives_no_allow_header() {
        use axum::http::Request as HttpRequest;
        use tower::ServiceExt;

        let layer = build_cors_layer(&CorsOptions {
            origins: vec!["https://example.com".to_string()],
            credentials: false,
            max_age: 600,
            expose_headers: vec![],
        });
        let app = build_router(build_test_state(minimal_get_pets_spec()), true).layer(layer);
        let response = app
            .oneshot(
                HttpRequest::builder()
                    .method("OPTIONS")
                    .uri("/pets")
                    .header("Origin", "https://other.com")
                    .header("Access-Control-Request-Method", "GET")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");

        assert!(response
            .headers()
            .get("access-control-allow-origin")
            .is_none());
    }

    /// Combining `--cors-credentials` with no `--cors-origin` downgrades the
    /// `Access-Control-Allow-Credentials` header to absent (the CORS spec forbids
    /// the combination of wildcard origin and credentials).
    #[tokio::test]
    async fn test_cors_credentials_warns_when_combined_with_any_origin() {
        use axum::http::Request as HttpRequest;
        use tower::ServiceExt;

        let layer = build_cors_layer(&CorsOptions {
            origins: vec![],
            credentials: true,
            max_age: 600,
            expose_headers: vec![],
        });
        let app = build_router(build_test_state(minimal_get_pets_spec()), true).layer(layer);
        let response = app
            .oneshot(
                HttpRequest::builder()
                    .method("OPTIONS")
                    .uri("/pets")
                    .header("Origin", "https://example.com")
                    .header("Access-Control-Request-Method", "GET")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");

        assert!(response
            .headers()
            .get("access-control-allow-credentials")
            .is_none());
    }

    /// `--cors-max-age 600` emits `Access-Control-Max-Age: 600` on a preflight response.
    #[tokio::test]
    async fn test_cors_max_age_emitted_on_preflight() {
        use axum::http::Request as HttpRequest;
        use tower::ServiceExt;

        let layer = build_cors_layer(&CorsOptions {
            origins: vec![],
            credentials: false,
            max_age: 600,
            expose_headers: vec![],
        });
        let app = build_router(build_test_state(minimal_get_pets_spec()), true).layer(layer);
        let response = app
            .oneshot(
                HttpRequest::builder()
                    .method("OPTIONS")
                    .uri("/pets")
                    .header("Origin", "https://example.com")
                    .header("Access-Control-Request-Method", "GET")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");

        let max_age = response
            .headers()
            .get("access-control-max-age")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();

        assert_eq!(max_age, "600");
    }

    /// A handler exceeding the configured `--request-timeout` returns a 408 envelope
    /// whose `type` URI is the chasm.dev `REQUEST_TIMEOUT` constant.
    ///
    /// The middleware order here matches the one that produces a converted body:
    /// `TimeoutLayer` is applied first so it is the inner layer; `rewrite_timeout_response`
    /// is applied last so it is the outer layer that observes the 408 and rewrites
    /// the body into a problem-JSON envelope.
    #[tokio::test]
    async fn test_request_timeout_emits_408_problem_document() {
        use axum::http::Request as HttpRequest;
        use axum::routing::get;
        use tower::ServiceExt;

        let app = apply_timeout_layer(
            Router::new().route(
                "/slow",
                get(|| async {
                    tokio::time::sleep(Duration::from_millis(500)).await;
                    "never"
                }),
            ),
            Duration::from_millis(50),
        );

        let response = app
            .oneshot(
                HttpRequest::builder()
                    .method("GET")
                    .uri("/slow")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::REQUEST_TIMEOUT);
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("collect");
        let json: serde_json::Value = serde_json::from_slice(&bytes).expect("json");

        assert_eq!(json["type"], "https://chasm.dev/errors#REQUEST_TIMEOUT");
    }

    /// With `--strict-method-matching`, an OPTIONS request on a path that does not
    /// declare `options` returns 405 instead of being synthesised.
    #[tokio::test]
    async fn test_strict_method_matching_returns_405_for_undeclared_options() {
        use axum::http::Request as HttpRequest;
        use tower::ServiceExt;

        let yaml = r#"
openapi: 3.0.0
info: { title: t, version: 1.0.0 }
paths:
  /pets:
    get:
      responses:
        '200':
          description: ok
          content:
            application/json:
              example: { ok: true }
"#;
        let spec = load_spec(yaml).expect("spec");
        let state = AppState {
            spec: Arc::new(RwLock::new(Arc::new(spec))),
            default_cfg: Arc::new({
                let mut __r = MockConfig::default();
                __r.strict_method_matching = true;
                __r
            }),
            metrics: Arc::new(Metrics::default()),
            last_reload_error: Arc::new(Mutex::new(None)),
            log_format: LogFormat::Text,
        };
        let app = build_router(state, true);
        let response = app
            .oneshot(
                HttpRequest::builder()
                    .method("OPTIONS")
                    .uri("/pets")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");

        assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
    }

    /// `MockError::Generation` renders as a complete `NO_RESPONSE_RESPONSE_DEFINED` envelope
    /// titled "Response body generation failed", including the request's method and path
    /// in the detail string so operators can correlate the failure with the offending
    /// operation.
    #[test]
    fn test_generation_error_renders_no_response_response_defined_envelope() {
        let err = MockError::Generation {
            method: "GET".to_string(),
            path: "/x".to_string(),
            source: chasm_faker::FakerError::SchemaError {
                path: "/".to_string(),
                message: "boom".to_string(),
            },
        };

        let json = problem_for_error(&err, "GET", "/x", "test-req-id").to_json();

        assert_eq!(
            json,
            serde_json::json!({
                "type": TYPE_NO_RESPONSE_RESPONSE_DEFINED,
                "title": "Response body generation failed",
                "status": 500,
                "detail": "Failed to generate a response body for GET /x: schema error at /: boom",
                "instance": "urn:chasm:request:test-req-id",
            })
        );
    }

    /// `MockError::SpecSerialization` renders as a complete `NO_RESPONSE_RESPONSE_DEFINED` envelope.
    #[test]
    fn test_spec_serialization_renders_no_response_response_defined_envelope() {
        let err = MockError::SpecSerialization("bad json".to_string());

        let json = problem_for_error(&err, "GET", "/x", "test-req-id").to_json();

        assert_eq!(
            json,
            serde_json::json!({
                "type": TYPE_NO_RESPONSE_RESPONSE_DEFINED,
                "title": "No response defined for the selected operation",
                "status": 500,
                "detail": "Spec serialization error: bad json",
                "instance": "urn:chasm:request:test-req-id",
            })
        );
    }

    /// `MockError::NoResponseForCode` renders as a complete `NO_RESPONSE_DEFINED` envelope.
    #[test]
    fn test_no_response_for_code_renders_no_response_defined_envelope() {
        let err = MockError::NoResponseForCode {
            method: "GET".to_string(),
            path: "/x".to_string(),
            code: 500,
        };

        let json = problem_for_error(&err, "GET", "/x", "test-req-id").to_json();

        assert_eq!(
            json,
            serde_json::json!({
                "type": TYPE_NO_RESPONSE_DEFINED,
                "title": "The server cannot find the requested content",
                "status": 404,
                "detail": "The response code 500 is not defined for GET /x in the spec.",
                "instance": "urn:chasm:request:test-req-id",
            })
        );
    }

    /// `MockError::NotAcceptable` renders as a complete `NOT_ACCEPTABLE` envelope.
    #[test]
    fn test_not_acceptable_renders_not_acceptable_envelope() {
        let err = MockError::NotAcceptable {
            acceptable: vec!["application/json".to_string()],
        };

        let json = problem_for_error(&err, "GET", "/x", "test-req-id").to_json();

        assert_eq!(
            json,
            serde_json::json!({
                "type": TYPE_NOT_ACCEPTABLE,
                "title": "The server cannot produce a representation for your accept header",
                "status": 406,
                "detail": "Available content types: application/json.",
                "instance": "urn:chasm:request:test-req-id",
            })
        );
    }

    /// RFC 9110 §15.3.5 forbids `content` on a 204 No Content response, so the
    /// server must suppress the body and `Content-Type`/`Content-Length` even
    /// when the engine would normally emit `null`. A DELETE operation whose
    /// only declared response is `204` should produce a zero-byte response
    /// with no `Content-Type` and no `Content-Length: 4` literal `"null"`.
    #[tokio::test]
    async fn test_204_no_content_suppresses_body_and_content_type() {
        use axum::http::Request as HttpRequest;
        use tower::ServiceExt;

        let yaml = r#"
openapi: 3.0.0
info: { title: t, version: 1.0.0 }
paths:
  /pets/{id}:
    delete:
      parameters:
        - in: path
          name: id
          required: true
          schema: { type: string }
      responses:
        '204':
          description: no content
"#;
        let app = build_router(build_test_state(yaml), true);
        let response = app
            .oneshot(
                HttpRequest::builder()
                    .method("DELETE")
                    .uri("/pets/42")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");

        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        assert!(
            !response
                .headers()
                .contains_key(axum::http::header::CONTENT_TYPE),
            "204 must not carry a Content-Type header, got {:?}",
            response.headers().get(axum::http::header::CONTENT_TYPE)
        );
        assert!(
            response
                .headers()
                .get(axum::http::header::CONTENT_LENGTH)
                .and_then(|v| v.to_str().ok())
                .map(|v| v != "4")
                .unwrap_or(true),
            "204 must not advertise Content-Length: 4 (the byte length of \"null\")"
        );
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("collect");
        assert!(
            bytes.is_empty(),
            "204 body must be empty, got {} bytes: {:?}",
            bytes.len(),
            bytes
        );
    }

    /// RFC 9110 §15.4.5 forbids a body on a 304 Not Modified response. When
    /// the engine forces a 304 via `Prefer: code=304`, the server must
    /// suppress the body and drop `Content-Type`.
    #[tokio::test]
    async fn test_304_not_modified_suppresses_body_and_content_type() {
        use axum::http::Request as HttpRequest;
        use tower::ServiceExt;

        let yaml = r#"
openapi: 3.0.0
info: { title: t, version: 1.0.0 }
paths:
  /pets:
    get:
      responses:
        '304':
          description: not modified
        '200':
          description: ok
          content:
            application/json:
              example: { ok: true }
"#;
        let app = build_router(build_test_state(yaml), true);
        let response = app
            .oneshot(
                HttpRequest::builder()
                    .method("GET")
                    .uri("/pets?__code=304")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");

        assert_eq!(response.status(), StatusCode::NOT_MODIFIED);
        assert!(
            !response
                .headers()
                .contains_key(axum::http::header::CONTENT_TYPE),
            "304 must not carry a Content-Type header"
        );
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("collect");
        assert!(bytes.is_empty(), "304 body must be empty");
    }

    /// RFC 9110 §15.3.6 requires 205 Reset Content responses to suppress the
    /// body. When the engine forces a 205 via `Prefer: code=205`, the server
    /// must drop the body and `Content-Type` header.
    #[tokio::test]
    async fn test_205_reset_content_suppresses_body() {
        use axum::http::Request as HttpRequest;
        use tower::ServiceExt;

        let yaml = r#"
openapi: 3.0.0
info: { title: t, version: 1.0.0 }
paths:
  /pets:
    get:
      responses:
        '205':
          description: reset content
        '200':
          description: ok
          content:
            application/json:
              example: { ok: true }
"#;
        let app = build_router(build_test_state(yaml), true);
        let response = app
            .oneshot(
                HttpRequest::builder()
                    .method("GET")
                    .uri("/pets?__code=205")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");

        assert_eq!(response.status(), StatusCode::RESET_CONTENT);
        assert!(
            !response
                .headers()
                .contains_key(axum::http::header::CONTENT_TYPE),
            "205 must not carry a Content-Type header"
        );
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("collect");
        assert!(bytes.is_empty(), "205 body must be empty");
    }

    /// RFC 9110 §15.2 forbids a body on 1xx Informational responses. The
    /// `is_bodyless_status` helper must report `true` for every status in the
    /// 100-199 range so the server suppresses content if such a status is
    /// produced.
    #[test]
    fn test_is_bodyless_status_covers_1xx_and_205() {
        for status in 100..=199u16 {
            assert!(
                is_bodyless_status(status),
                "1xx status {status} must be bodyless per RFC 9110 §15.2"
            );
        }
        assert!(
            is_bodyless_status(205),
            "205 Reset Content must be bodyless per RFC 9110 §15.3.6"
        );
        assert!(!is_bodyless_status(200), "200 must allow a body");
        assert!(!is_bodyless_status(206), "206 must allow a body");
    }

    /// A 304 emitted via ETag-match strips Content-Type and Content-Length to
    /// conform to RFC 9110 §15.4.5.
    #[tokio::test]
    async fn test_etag_match_304_omits_content_type_and_length() {
        use axum::http::Request as HttpRequest;
        use tower::ServiceExt;

        let yaml = r#"
openapi: 3.0.0
info: { title: t, version: 1.0.0 }
paths:
  /pets:
    get:
      responses:
        '200':
          description: ok
          headers:
            X-Custom:
              schema: { type: string, example: keep-me }
            Content-Type:
              schema: { type: string, example: application/json }
          content:
            application/json:
              example: { ok: true }
"#;
        let app = build_router(build_test_state(yaml), true);
        let first = app
            .clone()
            .oneshot(
                HttpRequest::builder()
                    .method("GET")
                    .uri("/pets")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(first.status(), StatusCode::OK);
        let etag = first
            .headers()
            .get(axum::http::header::ETAG)
            .expect("first response must carry an ETag")
            .to_str()
            .expect("etag is ascii")
            .to_string();

        let response = app
            .oneshot(
                HttpRequest::builder()
                    .method("GET")
                    .uri("/pets")
                    .header("If-None-Match", &etag)
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");

        assert_eq!(response.status(), StatusCode::NOT_MODIFIED);
        assert!(
            !response
                .headers()
                .contains_key(axum::http::header::CONTENT_TYPE),
            "ETag-match 304 must not carry a Content-Type header"
        );
        let content_length = response
            .headers()
            .get(axum::http::header::CONTENT_LENGTH)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        assert!(
            matches!(content_length.as_deref(), None | Some("0")),
            "ETag-match 304 must not advertise a non-zero Content-Length; got {:?}",
            content_length,
        );
        assert_eq!(
            response
                .headers()
                .get(axum::http::header::ETAG)
                .and_then(|v| v.to_str().ok()),
            Some(etag.as_str()),
            "ETag-match 304 must echo the matched ETag",
        );
        assert!(
            response.headers().contains_key("x-custom"),
            "non-framing spec headers should still be emitted on a 304",
        );
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("collect");
        assert!(bytes.is_empty(), "ETag-match 304 body must be empty");
    }
}
