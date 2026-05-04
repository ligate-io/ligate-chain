//! Prometheus `/metrics` endpoint for `ligate-node`.
//!
//! Phase 1 of #110: exposes a single GET route on a configurable
//! TCP socket (default `127.0.0.1:9100`). The route gathers the
//! global Prometheus default registry and renders it as
//! Prometheus text format. The counters that show up here are
//! defined in [`attestation::metrics`]; future modules add their
//! own and they appear automatically because everything lives on
//! the same default registry.
//!
//! ## Why a separate axum task
//!
//! The SDK's blueprint mounts its own REST server (the runtime
//! per-module routes plus the sequencer / ledger APIs). Adding a
//! `/metrics` route to that router requires either patching the
//! blueprint or wrapping it. Spawning a second axum server on a
//! different port keeps the metrics surface independent of the
//! SDK's internal routing and matches the conventional "9100 for
//! Prometheus, the rest is your app" pattern.
//!
//! ## Why bind to localhost by default
//!
//! Operators run a reverse proxy (nginx, Caddy) in front of the
//! node when they want metrics scraped externally. Binding to
//! `0.0.0.0` by default would expose internal counters to anyone
//! on the network. The `--metrics-bind` flag overrides for ops
//! who know what they're doing.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

use anyhow::Context as _;
use axum::extract::{MatchedPath, Request};
use axum::http::{header, HeaderValue, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use prometheus::{
    register_histogram_vec, register_int_counter_vec, register_int_gauge, Encoder, HistogramVec,
    IntCounterVec, IntGauge, TextEncoder,
};
use sov_db::ledger_db::LedgerDb;
use sov_rollup_interface::common::SlotNumber;
use sov_rollup_interface::node::ledger_api::LedgerStateProvider;
use tokio::net::TcpListener;
use tracing::{debug, info, warn};

/// Build the axum router with a single `/metrics` GET route.
///
/// Pulled out into its own function so the integration test can
/// instantiate the same router and exercise it without binding a
/// real TCP socket.
pub fn router() -> Router {
    Router::new().route("/metrics", get(handle_metrics))
}

/// Render the global Prometheus default registry as text. Returns
/// 200 with `Content-Type: text/plain; version=0.0.4` on success.
async fn handle_metrics() -> Response {
    let mut buf = Vec::with_capacity(4096);
    let encoder = TextEncoder::new();
    let metric_families = prometheus::gather();
    if let Err(e) = encoder.encode(&metric_families, &mut buf) {
        warn!(error = %e, "failed to encode metrics");
        return (StatusCode::INTERNAL_SERVER_ERROR, format!("encode error: {e}")).into_response();
    }
    let mut resp = (StatusCode::OK, buf).into_response();
    resp.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(encoder.format_type())
            .unwrap_or(HeaderValue::from_static("text/plain")),
    );
    resp
}

/// Bind a TCP listener for the metrics endpoint. Pulled out from
/// `serve` so callers (including tests) can request an ephemeral
/// port via `127.0.0.1:0` and read back the actual bound address.
pub async fn bind(addr: SocketAddr) -> anyhow::Result<TcpListener> {
    TcpListener::bind(addr).await.with_context(|| format!("metrics server failed to bind {addr}"))
}

/// Drive the metrics server on `listener` until the task is
/// cancelled (e.g. via tokio runtime shutdown). Once `bind`
/// returns successfully, this loop only fails if axum's accept
/// loop itself errors out (rare; usually a fatal OS-level fault).
pub async fn serve(listener: TcpListener) -> anyhow::Result<()> {
    let actual = listener.local_addr()?;
    info!(bind = %actual, "metrics endpoint listening at /metrics");
    let app = router();
    axum::serve(listener, app)
        .await
        .with_context(|| format!("metrics server crashed (was bound to {actual})"))
}

// ============================================================================
// State DB size gauge (#110 Phase 2)
// ============================================================================

/// Default polling interval for the state-DB-size sampler.
///
/// Operators trade off between staleness and disk I/O: walking a
/// RocksDB directory hits stat(2) on every file, which on a hot
/// node with thousands of SST files is non-trivial. 30 seconds is
/// the SDK demo's default and is fine for capacity planning; alert
/// rules with 5-minute windows comfortably absorb the resolution.
pub const DEFAULT_STATE_DB_SIZE_POLL_INTERVAL: Duration = Duration::from_secs(30);

/// `ligate_state_db_size_bytes` gauge. Total on-disk size of the
/// rollup's storage directory in bytes, updated periodically by
/// [`spawn_state_db_size_task`]. Filesystem-level walk: includes
/// RocksDB SST + WAL + manifest, ledger DB, and any sibling files
/// the SDK plants under the same path.
fn state_db_size_bytes() -> &'static IntGauge {
    static M: OnceLock<IntGauge> = OnceLock::new();
    M.get_or_init(|| {
        register_int_gauge!(
            "ligate_state_db_size_bytes",
            "Total on-disk size of the rollup's storage directory, in bytes. Sampled periodically."
        )
        .expect("gauge registers once")
    })
}

/// Touch the gauge so its `HELP` and `TYPE` lines show up in
/// `/metrics` from the very first scrape. Without this, a node
/// scraped before the first poll completes returns no series for
/// this metric and trips alerting rules that expect it.
pub fn init_state_db_size() {
    let _ = state_db_size_bytes();
}

/// Walk `path` recursively and sum every file's `len()`. Symlinks
/// are followed only one level deep (the default for `read_dir`)
/// to avoid loops; broken symlinks and unreadable entries are
/// silently skipped so a transient mid-compaction view doesn't
/// crash the sampler.
///
/// Returns `0` for paths that don't exist, can't be read, or are
/// empty.  That's intentional: the alternative (returning `Result`)
/// would force the polling task to choose between propagating
/// errors (kills the gauge) or swallowing them (loses signal).
/// Returning a number that's "low or zero" lets dashboards show
/// the regression as a graph dip and alerts catch it.
fn directory_size_bytes(path: &Path) -> u64 {
    let mut total = 0u64;
    let Ok(entries) = std::fs::read_dir(path) else {
        return 0;
    };
    for entry in entries.flatten() {
        let Ok(meta) = entry.metadata() else { continue };
        if meta.is_dir() {
            total = total.saturating_add(directory_size_bytes(&entry.path()));
        } else {
            total = total.saturating_add(meta.len());
        }
    }
    total
}

/// Sample the storage directory once and update the gauge. Pulled
/// out of the polling loop so tests can drive a single sample
/// against a temp dir without spinning a tokio interval.
pub fn sample_state_db_size(path: &Path) {
    init_state_db_size();
    let bytes = directory_size_bytes(path);
    state_db_size_bytes().set(bytes as i64);
    debug!(path = %path.display(), bytes, "state db size sampled");
}

/// Spawn a tokio task that polls the storage directory's total size
/// every `interval` (default 30s) and updates
/// `ligate_state_db_size_bytes`. Runs until the runtime drops.
///
/// The task is fire-and-forget; if the storage path becomes
/// unreadable mid-flight (e.g. operator wiped the directory) the
/// gauge falls to 0, which is the right signal for dashboards.
pub fn spawn_state_db_size_task(storage_path: PathBuf, interval: Duration) {
    init_state_db_size();
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(interval);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            ticker.tick().await;
            sample_state_db_size(&storage_path);
        }
    });
}

// ============================================================================
// Block height gauge (#110 Phase 2)
// ============================================================================

/// Default polling interval for the block-height sampler.
///
/// Slot times are typically 1s on devnet; polling every 2s catches
/// the head with one cycle of headroom and keeps the LedgerDb read
/// rate well below RocksDB's read budget. Tighter cadence is fine
/// (the read is cheap, just a single key lookup) but doesn't help
/// any dashboard scraping at 5-15s intervals.
pub const DEFAULT_BLOCK_HEIGHT_POLL_INTERVAL: Duration = Duration::from_secs(2);

/// `ligate_block_height` gauge. Current head slot number observed
/// by this node. For a sequencer, this is the head it's producing;
/// for a follower, it's the head it has replayed from DA.
fn block_height() -> &'static IntGauge {
    static M: OnceLock<IntGauge> = OnceLock::new();
    M.get_or_init(|| {
        register_int_gauge!(
            "ligate_block_height",
            "Current rollup head slot number observed by this node. Sequencers report what \
             they're producing; followers report what they've replayed from DA."
        )
        .expect("gauge registers once")
    })
}

/// Touch the gauge so its `HELP` and `TYPE` show up in `/metrics`
/// from the very first scrape, before the first poll completes.
pub fn init_block_height() {
    let _ = block_height();
}

/// Set the block-height gauge to a specific slot number. Pulled out
/// of the polling loop so tests can drive a single sample without
/// constructing a `LedgerDb`.
pub fn sample_block_height(slot: SlotNumber) {
    init_block_height();
    block_height().set(slot.get() as i64);
}

// ============================================================================
// RPC histograms (#110 Phase 2)
// ============================================================================

/// `ligate_rpc_requests_total{endpoint,status}` counter. One bump
/// per request that hits a matched route. Unmatched routes (404s on
/// arbitrary paths) are skipped to keep the `endpoint` label
/// cardinality bounded by the actual route surface.
fn rpc_requests() -> &'static IntCounterVec {
    static M: OnceLock<IntCounterVec> = OnceLock::new();
    M.get_or_init(|| {
        register_int_counter_vec!(
            "ligate_rpc_requests_total",
            "Number of REST requests received, labelled by route template and HTTP status.",
            &["endpoint", "status"]
        )
        .expect("counter vec registers once")
    })
}

/// `ligate_rpc_request_duration_seconds{endpoint}` histogram. One
/// observation per request, in seconds. Default Prometheus buckets
/// (0.005s through 10s) cover the realistic latency range for our
/// REST surface (state lookups in milliseconds, slow queries up to
/// a few seconds under load).
fn rpc_request_duration() -> &'static HistogramVec {
    static M: OnceLock<HistogramVec> = OnceLock::new();
    M.get_or_init(|| {
        register_histogram_vec!(
            "ligate_rpc_request_duration_seconds",
            "Duration of REST request handling in seconds, labelled by route template.",
            &["endpoint"]
        )
        .expect("histogram vec registers once")
    })
}

/// Touch both vectors so their `HELP` and `TYPE` lines appear in
/// `/metrics` from the very first scrape, before the first request
/// lands.
pub fn init_rpc_metrics() {
    let _ = rpc_requests();
    let _ = rpc_request_duration();
}

/// Axum middleware: record one `rpc_requests_total` increment + one
/// `rpc_request_duration_seconds` observation per matched route.
///
/// Mirrors the pattern in `sov-stf-runner/src/http/mod.rs:189-222`
/// (`measure_time`): use `MatchedPath::as_str()` for the label so
/// concrete `:id` values don't blow up the cardinality, skip
/// unmatched routes entirely.
///
/// Wire via `endpoints.axum_router.layer(axum::middleware::from_fn(record_rpc_request))`
/// inside `create_endpoints` so all SDK-mounted routes (sequencer,
/// ledger, runtime modules) get the same instrumentation.
pub async fn record_rpc_request(
    matched_path: Option<MatchedPath>,
    req: Request,
    next: Next,
) -> Response {
    let start = std::time::Instant::now();
    let response = next.run(req).await;
    let duration = start.elapsed();

    // Unmatched routes (404s on arbitrary paths) are skipped.
    // Otherwise an attacker could pump the cardinality of the
    // `endpoint` label by hitting random URLs.
    if let Some(path) = matched_path {
        let endpoint = path.as_str();
        let status = response.status().as_str().to_owned();
        rpc_requests().with_label_values(&[endpoint, &status]).inc();
        rpc_request_duration().with_label_values(&[endpoint]).observe(duration.as_secs_f64());
    }

    response
}

// ============================================================================
// Block height gauge (#110 Phase 2)
// ============================================================================

/// Spawn a tokio task that polls `ledger_db.get_head_slot_number()`
/// every `interval` and updates `ligate_block_height`. Runs until
/// the runtime drops.
///
/// The polling pattern (vs `subscribe_slots()`) avoids pulling
/// `futures_util::StreamExt` into the deps and matches the polling
/// shape we already use for `state_db_size`. Slot rate (1Hz) is
/// well under poll cadence (2s) on devnet, so resolution is fine.
pub fn spawn_block_height_task(ledger_db: LedgerDb, interval: Duration) {
    init_block_height();
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(interval);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            ticker.tick().await;
            match ledger_db.get_head_slot_number().await {
                Ok(slot) => sample_block_height(slot),
                Err(e) => warn!(error = ?e, "failed to read head slot number"),
            }
        }
    });
}
