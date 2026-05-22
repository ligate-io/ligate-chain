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
#[cfg(target_os = "linux")]
use prometheus::process_collector::ProcessCollector;
use prometheus::{
    register_histogram, register_histogram_vec, register_int_counter, register_int_counter_vec,
    register_int_gauge, Encoder, Histogram, HistogramVec, IntCounter, IntCounterVec, IntGauge,
    TextEncoder,
};
use sov_blob_sender::{BlobSubmissionError, BlobSubmissionStatus};
use sov_db::ledger_db::LedgerDb;
use sov_rollup_interface::common::SlotNumber;
use sov_rollup_interface::da::DaSpec;
use sov_rollup_interface::node::ledger_api::LedgerStateProvider;
use sov_rollup_interface::BlobHash;
use sov_sequencer::{BlobExecutionStatus, MempoolMetrics};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::sync::broadcast;
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

/// Walk `path` recursively and sum the **on-disk** size of every
/// file (allocated 512-byte blocks, not nominal length).
///
/// `meta.len()` would return the file's logical size, which is
/// wrong for the sparse-allocated files that NOMT and RocksDB
/// produce: a 1M-bucket NOMT hash table reports its nominal
/// preallocation (tens of GB) even when only a few MB of blocks
/// are actually committed to disk. `du -sh` matches
/// `meta.blocks() * 512`, and that's the value operators care
/// about for capacity planning and what dashboards graph as
/// "storage growth".
///
/// On non-Unix targets we fall back to `meta.len()` because the
/// `MetadataExt::blocks` API is Unix-only. Production deployment is
/// Linux; macOS dev gets the correct value too via the same code
/// path. Windows would over-report on sparse files but isn't a
/// supported target.
///
/// Symlinks are followed only one level deep (the default for
/// `read_dir`) to avoid loops; broken symlinks and unreadable
/// entries are silently skipped so a transient mid-compaction view
/// doesn't crash the sampler.
///
/// Returns `0` for paths that don't exist or can't be read. The
/// alternative (returning `Result`) would force the polling task to
/// choose between propagating errors (kills the gauge) or
/// swallowing them (loses signal). Returning a number that's "low
/// or zero" lets dashboards show the regression as a graph dip and
/// alerts catch it.
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
            total = total.saturating_add(file_disk_bytes(&meta));
        }
    }
    total
}

/// On-disk byte count for a single file. Unix: blocks * 512.
/// Non-Unix: `len()` (over-reports on sparse files).
#[cfg(unix)]
fn file_disk_bytes(meta: &std::fs::Metadata) -> u64 {
    use std::os::unix::fs::MetadataExt;
    meta.blocks().saturating_mul(512)
}

#[cfg(not(unix))]
fn file_disk_bytes(meta: &std::fs::Metadata) -> u64 {
    meta.len()
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

// ============================================================================
// Mempool depth gauge (#110 Phase 2, was blocked on SDK upstream)
// ============================================================================

/// Default polling interval for the mempool-depth sampler.
///
/// Slot times are typically 1s on devnet. Polling every 1s keeps the
/// gauge fresh enough that operators see in-flight tx queueing during
/// load spikes without spamming the (cheap) `pending_tx_count` call.
pub const DEFAULT_MEMPOOL_DEPTH_POLL_INTERVAL: Duration = Duration::from_secs(1);

/// `ligate_mempool_depth` gauge. Number of transactions currently
/// sitting in the sequencer mempool, awaiting inclusion in a batch.
fn mempool_depth() -> &'static IntGauge {
    static M: OnceLock<IntGauge> = OnceLock::new();
    M.get_or_init(|| {
        register_int_gauge!(
            "ligate_mempool_depth",
            "Number of transactions currently sitting in the sequencer mempool, \
             awaiting inclusion in a batch."
        )
        .expect("gauge registers once")
    })
}

/// Touch the gauge so `HELP` and `TYPE` show up in `/metrics` from
/// the very first scrape, before the first poll completes.
pub fn init_mempool_depth() {
    let _ = mempool_depth();
}

/// Set the mempool-depth gauge to a concrete value. Pulled out of
/// the polling loop so tests can drive a single sample without a
/// real `MempoolMetrics` provider.
pub fn sample_mempool_depth(count: usize) {
    init_mempool_depth();
    mempool_depth().set(count as i64);
}

/// Spawn a tokio task that polls `mempool_metrics.pending_tx_count()`
/// every `interval` and updates the `ligate_mempool_depth` gauge.
/// Runs until the runtime drops.
///
/// `mempool_metrics` is the `Arc<dyn MempoolMetrics>` exposed by
/// `SequencerCreationReceipt::mempool_metrics` (added to the SDK fork
/// in `ligate-mainline`'s `Expose MempoolMetrics +
/// blob_status_channel` patch). Cloneable cheaply (Arc bump).
pub fn spawn_mempool_depth_task(mempool_metrics: Arc<dyn MempoolMetrics>, interval: Duration) {
    init_mempool_depth();
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(interval);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            ticker.tick().await;
            let count = mempool_metrics.pending_tx_count().await;
            sample_mempool_depth(count);
        }
    });
}

// ============================================================================
// Process metrics (CPU, memory, FDs) via prometheus's `process` feature
// ============================================================================

/// Register the `process_collector` so `process_cpu_seconds_total`,
/// `process_resident_memory_bytes`, `process_open_fds`,
/// `process_max_fds`, and `process_start_time_seconds` show up on
/// every `/metrics` scrape. Idempotent: subsequent calls are no-ops.
///
/// Uses the global default registry, same surface as every other
/// gauge / counter in this module. Operators get OS-level health for
/// free without writing module-side bookkeeping.
///
/// Linux-only: the upstream `prometheus` crate gates
/// `process_collector` behind `target_os = "linux"` because it reads
/// `/proc/self/*`. macOS / Windows builds compile cleanly but skip
/// registration; production deploys are Linux per
/// `docs/development/public-devnet-deploy.md`.
pub fn register_process_collector() {
    #[cfg(target_os = "linux")]
    {
        static REGISTERED: OnceLock<()> = OnceLock::new();
        REGISTERED.get_or_init(|| {
            let collector = ProcessCollector::for_self();
            if let Err(e) = prometheus::default_registry().register(Box::new(collector)) {
                warn!(error = %e, "failed to register process_collector (already registered?)");
            }
        });
    }
    #[cfg(not(target_os = "linux"))]
    debug!("process_collector skipped (non-Linux build)");
}

// ============================================================================
// DA submission metrics (#110 Phase 2, was blocked on SDK upstream)
// ============================================================================

/// Stable snake_case label per `BlobSubmissionError` discriminant.
/// Used as the `reason` label on `ligate_da_submission_failures_total`.
/// Free-form `String` payloads are intentionally NOT included so the
/// Prometheus cardinality stays bounded by the enum's variant count.
fn submission_error_reason(error: &BlobSubmissionError) -> &'static str {
    match error {
        BlobSubmissionError::Submission(_) => "submission",
        BlobSubmissionError::PublishTimeout { .. } => "publish_timeout",
        BlobSubmissionError::Reorg => "reorg",
        BlobSubmissionError::FinalityCheck(_) => "finality_check",
        BlobSubmissionError::MaxRetriesExhausted { .. } => "max_retries_exhausted",
    }
}

/// `ligate_da_submission_failures_total{reason}` counter. One bump per
/// `BlobSubmissionStatus::Failed` event observed on the BlobSender's
/// broadcast channel. `reason` is a stable snake_case discriminant of
/// the typed `BlobSubmissionError` (see `submission_error_reason`).
fn da_submission_failures() -> &'static IntCounterVec {
    static M: OnceLock<IntCounterVec> = OnceLock::new();
    M.get_or_init(|| {
        register_int_counter_vec!(
            "ligate_da_submission_failures_total",
            "Number of DA blob submission failures, labelled by typed-error discriminant.",
            &["reason"]
        )
        .expect("counter vec registers once")
    })
}

/// `ligate_da_finalization_latency_seconds` histogram. Time from
/// `BlobSubmissionStatus::Published` (DA accepted the blob) to
/// `BlobSubmissionStatus::Finalized` (DA finality reached) for a given
/// blob hash. Measures the DA layer's contribution to end-to-end
/// inclusion latency. Mocha block time is ~12s, so default Prometheus
/// buckets (5ms-10s) under-cover the realistic range; we override with
/// a coarser ladder.
///
/// MustSubmit -> Published latency (sequencer-side submission time)
/// requires correlation by blob_hash that the early
/// `BlobSubmissionStatus` variants don't carry. Tracking that needs
/// an upstream change; #164's submission-latency ask is satisfied
/// closely enough by Published -> Finalized for now.
fn da_finalization_latency() -> &'static Histogram {
    static M: OnceLock<Histogram> = OnceLock::new();
    M.get_or_init(|| {
        register_histogram!(
            "ligate_da_finalization_latency_seconds",
            "DA finalization latency in seconds: time from Published to Finalized for a blob.",
            // Mocha is ~12s/block; a few-blocks finality typical.
            // Buckets cover sub-second (caching artefacts), one-block,
            // a few-blocks normal, deep tails (network stalls).
            vec![0.5, 1.0, 5.0, 10.0, 15.0, 30.0, 60.0, 120.0, 300.0, 600.0, 1800.0]
        )
        .expect("histogram registers once")
    })
}

/// `ligate_metrics_dropped_total` counter. Bumped when a metric task
/// observer (currently the DA-status subscriber) gets `Lagged` on its
/// broadcast receiver, signalling we missed `n` events because the
/// subscriber fell behind the channel. Lets us alert on observer
/// starvation rather than silently under-counting.
fn metrics_dropped() -> &'static IntCounter {
    static M: OnceLock<IntCounter> = OnceLock::new();
    M.get_or_init(|| {
        register_int_counter!(
            "ligate_metrics_dropped_total",
            "Number of metrics events dropped because a metric observer's broadcast \
             receiver lagged behind. A non-zero rate indicates the metric task is \
             starved (broadcast channel size 1024 by default)."
        )
        .expect("counter registers once")
    })
}

/// Touch all DA metrics so their `HELP` and `TYPE` lines appear from
/// the very first scrape, before the first event.
pub fn init_da_metrics() {
    let _ = da_submission_failures();
    let _ = da_finalization_latency();
    let _ = metrics_dropped();
    // DA cost estimate metrics are bundled with the rest of the DA
    // surface — same lifecycle, same scrape cadence.
    init_da_cost_metrics();
}

/// Spawn a tokio task that subscribes to the BlobSender's
/// `blob_status_channel` and updates DA-side metrics in response to
/// every status transition:
///
/// - `Published` → record `Instant::now()` keyed by `blob_hash`
/// - `Finalized` → compute `now - start`, observe finalization latency,
///   drop the entry from the in-flight map
/// - `Failed { error, .. }` → bump
///   `da_submission_failures_total{reason=<discriminant>}`, drop any
///   in-flight entry (a Failed blob never finalizes the same submission)
/// - `RecvError::Lagged(n)` → bump `metrics_dropped_total` by `n`,
///   warn-log, continue. Lagged subscribers see this when the channel
///   capacity (1024) overruns the consumer; it tells us to widen the
///   channel or speed up the consumer rather than silently miscount.
/// - `RecvError::Closed` → channel sender dropped (sequencer shutting
///   down). Exit the task cleanly.
///
/// The in-flight HashMap is bounded by a periodic sweep that drops
/// entries older than `INFLIGHT_TTL`. Without it, blobs that get
/// stuck in `Published` without progressing to Finalized or Failed
/// (would be an SDK bug, but defensive) leak memory.
pub fn spawn_da_metrics_task<Da: DaSpec>(
    blob_status_channel: broadcast::Sender<BlobExecutionStatus<Da>>,
) {
    init_da_metrics();
    let mut rx = blob_status_channel.subscribe();

    tokio::spawn(async move {
        // Cap the in-flight map at this many entries. Mocha at ~12s
        // blocks with our typical concurrency stays well under 1k;
        // 4k is a defensive cap that's still bounded.
        const INFLIGHT_CAP: usize = 4096;
        // Sweep entries older than this on each cleanup pass. 30 min
        // is well above any sensible DA finality (Mocha: ~24s for
        // single-block finality, deep finality < 1 min).
        const INFLIGHT_TTL: Duration = Duration::from_secs(30 * 60);
        // Periodic sweep cadence. Cheap; runs on the same task.
        const SWEEP_INTERVAL: Duration = Duration::from_secs(5 * 60);

        let mut in_flight: HashMap<BlobHash, std::time::Instant> = HashMap::new();
        let mut sweep = tokio::time::interval(SWEEP_INTERVAL);
        sweep.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        loop {
            tokio::select! {
                _ = sweep.tick() => {
                    let before = in_flight.len();
                    in_flight.retain(|_, started| started.elapsed() < INFLIGHT_TTL);
                    let dropped = before.saturating_sub(in_flight.len());
                    if dropped > 0 {
                        warn!(dropped, "Pruned stale in-flight DA entries past TTL");
                    }
                }
                event = rx.recv() => match event {
                    Ok(status) => match status.blob_submission_status {
                        BlobSubmissionStatus::Published { receipt } => {
                            // Defensive: cap the map size. Drop the
                            // oldest if we somehow blow past the cap.
                            if in_flight.len() >= INFLIGHT_CAP {
                                if let Some(k) = in_flight.keys().next().cloned() {
                                    in_flight.remove(&k);
                                }
                            }
                            in_flight.insert(receipt.blob_hash, std::time::Instant::now());
                            // Bump the DA cost counters (chain#446
                            // Track 4 + chain#452). The SDK receipt
                            // now carries the real `size_in_bytes`
                            // (always populated) and `gas_used`
                            // (Celestia). `fee_paid` is still `None`
                            // pending Celestia tx-body decode; we
                            // fall back to a per-blob constant in
                            // that case so the existing
                            // `_estimate_total` counter keeps moving.
                            record_da_blob_published(
                                receipt.fee_paid,
                                receipt.gas_used,
                                receipt.size_in_bytes,
                            );
                        }
                        BlobSubmissionStatus::Finalized { receipt } => {
                            if let Some(started) = in_flight.remove(&receipt.blob_hash) {
                                let elapsed = started.elapsed().as_secs_f64();
                                da_finalization_latency().observe(elapsed);
                                debug!(
                                    blob_hash = %receipt.blob_hash,
                                    elapsed_secs = elapsed,
                                    "DA finalization latency observed"
                                );
                            }
                        }
                        BlobSubmissionStatus::Failed { error, will_retry } => {
                            let reason = submission_error_reason(&error);
                            da_submission_failures()
                                .with_label_values(&[reason])
                                .inc();
                            debug!(
                                reason,
                                will_retry,
                                "DA submission failure observed"
                            );
                        }
                        // Processed / MustSubmit don't drive metrics
                        // directly; processed is between Published and
                        // Finalized, MustSubmit is pre-publish.
                        _ => {}
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        metrics_dropped().inc_by(n);
                        warn!(skipped = n, "DA metrics task lagged; bump channel size or speed up consumer");
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        info!("DA metrics task: blob_status_channel closed, exiting");
                        return;
                    }
                }
            }
        }
    });
}

// ============================================================================
// Sequencer role gauge + transition counter (chain#446 follow-up)
//
// Surfaces who's currently the DbElected leader to Grafana so the
// paper-leader scenario (which bit us on 2026-05-21) is visible as a
// metric, not just a log line. Two metrics:
//
// - `ligate_sequencer_role`: 0=unknown, 1=PgSyncReplica, 2=BatchProducer.
//   Single integer because Grafana panels color-code by value cleanly.
// - `ligate_sequencer_role_transitions_total{from,to}`: bumps on each
//   observed transition. The `from`/`to` labels carry the readable
//   names so a Stat panel can show "last leader change" without a join.
// ============================================================================

/// Default polling interval for the sequencer role sampler. 2s
/// matches the SDK's Postgres heartbeat cadence; tighter polling
/// wouldn't surface anything new and would spam the loopback server.
pub const DEFAULT_SEQUENCER_ROLE_POLL_INTERVAL: Duration = Duration::from_secs(2);

/// `ligate_sequencer_role` gauge. 0 = unknown (no observation yet,
/// or local chain HTTP server unreachable), 1 = PgSyncReplica
/// (following leader via Postgres state-sync), 2 = BatchProducer
/// (this node holds the leader lock and posts blobs to DA).
fn sequencer_role() -> &'static IntGauge {
    static M: OnceLock<IntGauge> = OnceLock::new();
    M.get_or_init(|| {
        register_int_gauge!(
            "ligate_sequencer_role",
            "Current sequencer role for this node. 0 = unknown (no observation yet or chain \
             HTTP server unreachable), 1 = PgSyncReplica (following leader via Postgres \
             state-sync), 2 = BatchProducer (this node holds the leader lock and posts blobs \
             to DA)."
        )
        .expect("gauge registers once")
    })
}

/// `ligate_sequencer_role_transitions_total{from,to}` counter. Bumps
/// every time the sampled role differs from the previous sample. The
/// `from` and `to` labels are the readable role names (`unknown`,
/// `replica`, `leader`) so a Grafana panel can show "VM-1: leader →
/// replica at 22:51 UTC" without a join.
fn sequencer_role_transitions() -> &'static IntCounterVec {
    static M: OnceLock<IntCounterVec> = OnceLock::new();
    M.get_or_init(|| {
        register_int_counter_vec!(
            "ligate_sequencer_role_transitions_total",
            "Number of observed sequencer-role transitions on this node, labelled by `from` \
             and `to` role. Each label is `unknown`, `replica`, or `leader`.",
            &["from", "to"]
        )
        .expect("counter registers once")
    })
}

/// Pre-touch both sequencer-role metrics so their HELP/TYPE lines
/// show up in the very first `/metrics` scrape, before the poller's
/// first tick lands.
pub fn init_sequencer_role() {
    let _ = sequencer_role();
    let _ = sequencer_role_transitions();
}

/// Decode the SDK's `/v1/sequencer/role` response into the gauge
/// encoding. Response body is a quoted JSON string like
/// `"BatchProducer"` or `"PgSyncReplica"`. Anything we don't
/// recognise (connection failure, unbound port, future role variant)
/// degrades to `unknown` rather than crashing the poller.
fn encode_role(raw: &str) -> (i64, &'static str) {
    match raw.trim().trim_matches('"') {
        "BatchProducer" => (2, "leader"),
        "PgSyncReplica" => (1, "replica"),
        _ => (0, "unknown"),
    }
}

/// Sample the local `/v1/sequencer/role` endpoint once and update
/// the gauge + transition counter. `prev` carries the last observed
/// label so a transition only bumps the counter on actual change.
/// Pulled out of the spawn loop for unit-testability.
pub async fn sample_sequencer_role(role_url: &str, prev: &mut Option<&'static str>) {
    init_sequencer_role();
    // Loopback fetch. Failures (chain still booting, port not yet
    // bound, body not JSON) all fold into `encode_role` returning
    // `(0, "unknown")` — the right "we don't know" signal for
    // dashboards until the next poll succeeds.
    let body =
        match reqwest::Client::new().get(role_url).timeout(Duration::from_secs(1)).send().await {
            Ok(resp) if resp.status().is_success() => resp.text().await.unwrap_or_default(),
            _ => String::new(),
        };
    let (value, label) = encode_role(&body);
    sequencer_role().set(value);
    if let Some(prev_label) = *prev {
        if prev_label != label {
            sequencer_role_transitions().with_label_values(&[prev_label, label]).inc();
            info!(from = prev_label, to = label, "sequencer role transition observed");
        }
    }
    *prev = Some(label);
}

/// Spawn the sequencer-role poller. Polls `role_url` every `interval`
/// (default 2s) and keeps `ligate_sequencer_role` plus the transition
/// counter up to date. Fire-and-forget; lives until the runtime
/// drops. Safe to call before the SDK's HTTP server is bound — early
/// polls just record `unknown` until the loopback connection succeeds.
pub fn spawn_sequencer_role_task(role_url: String, interval: Duration) {
    init_sequencer_role();
    tokio::spawn(async move {
        let mut prev: Option<&'static str> = None;
        let mut ticker = tokio::time::interval(interval);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            ticker.tick().await;
            sample_sequencer_role(&role_url, &mut prev).await;
        }
    });
}

// ============================================================================
// DA cost estimate (Track 4 follow-up to chain#446)
//
// The Sovereign SDK's `SubmitBlobReceipt` doesn't currently carry the
// actual `fee_paid` value from the Celestia PFB tx receipt — only
// `blob_hash` and `da_transaction_id`. So we surface an *estimate*
// based on a fixed per-blob TIA cost calibrated to Mocha testnet rates
// observed on 2026-05-22. Filed as a follow-up to extend the SDK
// receipt with the real fee.
//
// Two metrics:
// - `ligate_da_blobs_published_total`: bumps every time a blob hits
//   `Published` on the BlobSender's broadcast channel. Useful as a
//   denominator and as a sanity check vs `ligate_da_finalization_*`.
// - `ligate_da_tia_burned_nano_estimate_total`: bumps by the fixed
//   per-blob constant on every Published event. Sum × time = estimated
//   TIA burn over the window. Mark "estimate" in Grafana so nobody
//   reads it as authoritative.
// ============================================================================

/// Estimated cost of a single Celestia blob submission, in nanoTIA
/// (1 TIA = 1_000_000_000 nanoTIA). Calibrated from Mocha gas rates
/// observed on 2026-05-22 for our typical attestation blobs (~13 to
/// ~50 bytes of payload after framing, plus the PFB tx envelope).
/// Conservative — will under-count once attestation payloads include
/// larger evidence blobs. Revisit when chain#XXX (SDK fee_paid in
/// receipt) lands.
pub const DA_BLOB_TIA_ESTIMATE_NANO: u64 = 300_000;

fn da_blobs_published() -> &'static IntCounter {
    static M: OnceLock<IntCounter> = OnceLock::new();
    M.get_or_init(|| {
        register_int_counter!(
            "ligate_da_blobs_published_total",
            "Number of blobs this node has observed reaching `Published` state on Celestia DA. \
             Bumps once per blob, regardless of whether the same blob later finalizes or \
             retries."
        )
        .expect("counter registers once")
    })
}

fn da_tia_burned_nano_estimate() -> &'static IntCounter {
    static M: OnceLock<IntCounter> = OnceLock::new();
    M.get_or_init(|| {
        register_int_counter!(
            "ligate_da_tia_burned_nano_estimate_total",
            "ESTIMATED total TIA (in nanoTIA, 1e-9 TIA) this node has burned posting blobs to \
             Celestia DA. Bumps by a fixed per-blob constant on every observed `Published` \
             event (the SDK's `SubmitBlobReceipt` doesn't currently carry the real `fee_paid`). \
             Under-counts once blob sizes grow; treat as a floor for ops dashboards, not an \
             authoritative ledger."
        )
        .expect("counter registers once")
    })
}

fn da_tia_estimate_per_blob_nano() -> &'static IntGauge {
    static M: OnceLock<IntGauge> = OnceLock::new();
    M.get_or_init(|| {
        register_int_gauge!(
            "ligate_da_tia_estimate_per_blob_nano",
            "The fixed per-blob TIA estimate (in nanoTIA) the chain uses to compute \
             `ligate_da_tia_burned_nano_estimate_total` when the SDK receipt's `fee_paid` is \
             `None`. Exposed as a gauge so dashboards can show the assumed-rate fallback next \
             to the burn counter."
        )
        .expect("gauge registers once")
    })
}

/// `ligate_da_blob_bytes_total`: real on-wire blob size posted to the
/// DA layer, summed across every observed `Published` event. Lets
/// dashboards show GB-per-month posted, real cost-per-byte, and
/// trend size growth as attestation payloads evolve. Bumps by
/// `SubmitBlobReceipt::size_in_bytes`, which every adapter (Celestia,
/// mock-DA) always populates as of chain#452.
fn da_blob_bytes_total() -> &'static IntCounter {
    static M: OnceLock<IntCounter> = OnceLock::new();
    M.get_or_init(|| {
        register_int_counter!(
            "ligate_da_blob_bytes_total",
            "Total bytes this node has posted to the DA layer, summed from the SDK \
             `SubmitBlobReceipt::size_in_bytes` on every observed `Published` event. \
             Authoritative (not an estimate)."
        )
        .expect("counter registers once")
    })
}

/// `ligate_da_blob_gas_total`: real DA-layer gas consumed, summed
/// across every observed `Published` event. Only bumps when the
/// adapter surfaces a value. Celestia leaves it `None` for now
/// (the celestia-grpc client returns a `TxInfo { hash, height }`
/// that drops the gas/fee fields; surfacing them needs a follow-up
/// `get_tx` lookup); mock-DA always leaves it `None`. Once the
/// Celestia follow-up lands this counter starts tracking real gas
/// without code changes here.
fn da_blob_gas_total() -> &'static IntCounter {
    static M: OnceLock<IntCounter> = OnceLock::new();
    M.get_or_init(|| {
        register_int_counter!(
            "ligate_da_blob_gas_total",
            "Total DA-layer gas this node has burned posting blobs, summed from the SDK \
             `SubmitBlobReceipt::gas_used` on every observed `Published` event. Skipped \
             when the adapter doesn't model gas (e.g. mock DA in tests). Authoritative."
        )
        .expect("counter registers once")
    })
}

/// Pre-touch the DA-cost metrics so their HELP/TYPE land in the first
/// `/metrics` scrape. Also sets the per-blob estimate gauge to the
/// compiled-in constant so it's never `0` (a 0 there is misleading).
pub fn init_da_cost_metrics() {
    let _ = da_blobs_published();
    let _ = da_tia_burned_nano_estimate();
    let _ = da_blob_bytes_total();
    let _ = da_blob_gas_total();
    da_tia_estimate_per_blob_nano().set(DA_BLOB_TIA_ESTIMATE_NANO as i64);
}

/// Record one observed `Published` blob event. Called from inside the
/// existing `spawn_da_metrics_task` loop, alongside the in-flight
/// tracking the finalization-latency metric uses.
///
/// The receipt's `fee_paid` is preferred when present (authoritative
/// per-blob nanoTIA cost from the DA layer); we fall back to the
/// compiled-in `DA_BLOB_TIA_ESTIMATE_NANO` constant when the adapter
/// doesn't surface a fee (mock DA in tests, or — temporarily — Celestia
/// while chain#452's tx-body decode is still TODO). `size_in_bytes` is
/// always authoritative; `gas_used` is recorded when present.
pub fn record_da_blob_published(fee_paid: Option<u64>, gas_used: Option<u64>, size_in_bytes: u64) {
    init_da_cost_metrics();
    da_blobs_published().inc();
    let tia_to_record = fee_paid.unwrap_or(DA_BLOB_TIA_ESTIMATE_NANO);
    da_tia_burned_nano_estimate().inc_by(tia_to_record);
    da_blob_bytes_total().inc_by(size_in_bytes);
    if let Some(gas) = gas_used {
        da_blob_gas_total().inc_by(gas);
    }
}

// ============================================================================
// Protocol economy mirror (Track 4 follow-up to chain#446)
//
// Mirrors the api's `/v1/stats/totals` into chain Prometheus so ops
// dashboards have a single source of truth. Each chain node polls the
// api independently; the values are chain-wide, so all instances
// expose the same number (Grafana aggregates with `max` or `last`).
//
// Soft-fails on api unreachable: gauges hold their last observed
// value rather than zeroing out (a 0 would falsely show "treasury
// empty"). Staleness is visible via `ligate_protocol_economy_last_scrape_unix`.
// ============================================================================

/// Default polling interval for the economy sampler. 60s is plenty —
/// treasury and supply move on a per-transaction cadence, and devnet
/// tx volume is low enough that finer sampling adds noise without
/// signal.
pub const DEFAULT_ECONOMY_POLL_INTERVAL: Duration = Duration::from_secs(60);

fn protocol_treasury_balance_nano() -> &'static IntGauge {
    static M: OnceLock<IntGauge> = OnceLock::new();
    M.get_or_init(|| {
        register_int_gauge!(
            "ligate_protocol_treasury_balance_nano",
            "Current LGT balance of the protocol treasury address, in nanoLGT \
             (1 LGT = 1_000_000_000 nanoLGT). Sourced from the api's \
             `/v1/stats/totals.treasury_balance_nano`."
        )
        .expect("gauge registers once")
    })
}

fn protocol_total_supply_nano() -> &'static IntGauge {
    static M: OnceLock<IntGauge> = OnceLock::new();
    M.get_or_init(|| {
        register_int_gauge!(
            "ligate_protocol_total_supply_nano",
            "Total LGT supply, in nanoLGT. Genesis-fixed at 1B LGT for devnet-1; this gauge \
             tracks the chain's reported value so a future emission policy is auto-picked-up."
        )
        .expect("gauge registers once")
    })
}

fn protocol_txs_total() -> &'static IntGauge {
    static M: OnceLock<IntGauge> = OnceLock::new();
    M.get_or_init(|| {
        register_int_gauge!(
            "ligate_protocol_txs_total",
            "Total transactions ever included on chain (per the api's indexer). Includes both \
             SUCCESS and REVERTED."
        )
        .expect("gauge registers once")
    })
}

fn protocol_attestations_total() -> &'static IntGauge {
    static M: OnceLock<IntGauge> = OnceLock::new();
    M.get_or_init(|| {
        register_int_gauge!(
            "ligate_protocol_attestations_total",
            "Total attestations ever submitted, per api indexer."
        )
        .expect("gauge registers once")
    })
}

fn protocol_schemas_registered() -> &'static IntGauge {
    static M: OnceLock<IntGauge> = OnceLock::new();
    M.get_or_init(|| {
        register_int_gauge!(
            "ligate_protocol_schemas_registered",
            "Total schemas registered, per api indexer."
        )
        .expect("gauge registers once")
    })
}

fn protocol_attestor_sets_registered() -> &'static IntGauge {
    static M: OnceLock<IntGauge> = OnceLock::new();
    M.get_or_init(|| {
        register_int_gauge!(
            "ligate_protocol_attestor_sets_registered",
            "Total attestor sets registered, per api indexer."
        )
        .expect("gauge registers once")
    })
}

fn protocol_economy_last_scrape_unix() -> &'static IntGauge {
    static M: OnceLock<IntGauge> = OnceLock::new();
    M.get_or_init(|| {
        register_int_gauge!(
            "ligate_protocol_economy_last_scrape_unix",
            "Unix timestamp (seconds) of the most recent successful poll of the api's \
             `/v1/stats/totals`. If this stops advancing, the api is unreachable and the \
             economy gauges above are stale — Grafana should show a 'stale data' badge once \
             this drifts more than 5 minutes from `now()`."
        )
        .expect("gauge registers once")
    })
}

/// Pre-touch all economy gauges so their `HELP` / `TYPE` lines show
/// up in the very first `/metrics` scrape, before the first poll
/// completes. Without this, alerting rules that match against these
/// metric names would fire a "no data" condition for the first ~60
/// seconds after boot.
pub fn init_economy_metrics() {
    let _ = protocol_treasury_balance_nano();
    let _ = protocol_total_supply_nano();
    let _ = protocol_txs_total();
    let _ = protocol_attestations_total();
    let _ = protocol_schemas_registered();
    let _ = protocol_attestor_sets_registered();
    let _ = protocol_economy_last_scrape_unix();
}

/// Parse a numeric string field out of the api's totals JSON and set
/// the matching gauge. Strings are used by the api because supply +
/// treasury values can exceed JS Number's safe-integer range (2^53);
/// they fit in i64 for any realistic chain state we'll see.
fn set_from_str(v: &serde_json::Value, key: &str, gauge: &IntGauge) {
    if let Some(s) = v.get(key).and_then(|x| x.as_str()) {
        if let Ok(n) = s.parse::<i64>() {
            gauge.set(n);
        }
    }
}

fn set_from_int(v: &serde_json::Value, key: &str, gauge: &IntGauge) {
    if let Some(n) = v.get(key).and_then(|x| x.as_i64()) {
        gauge.set(n);
    }
}

/// Sample the api's `/v1/stats/totals` once and update the protocol-
/// economy gauges. Soft-fails: a network error or a parse error
/// leaves the previous gauge values in place and the
/// `last_scrape_unix` gauge does not advance. Public so a test
/// harness can drive a single sample without spinning the task.
pub async fn sample_economy(api_url: &str) {
    init_economy_metrics();
    let body =
        match reqwest::Client::new().get(api_url).timeout(Duration::from_secs(5)).send().await {
            Ok(resp) if resp.status().is_success() => resp.text().await.unwrap_or_default(),
            _ => return,
        };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&body) else {
        return;
    };
    set_from_str(&v, "treasury_balance_nano", protocol_treasury_balance_nano());
    set_from_str(&v, "total_supply_nano", protocol_total_supply_nano());
    set_from_int(&v, "txs_total", protocol_txs_total());
    set_from_int(&v, "attestations", protocol_attestations_total());
    set_from_int(&v, "schemas", protocol_schemas_registered());
    set_from_int(&v, "attestor_sets", protocol_attestor_sets_registered());
    if let Ok(now) = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
        protocol_economy_last_scrape_unix().set(now.as_secs() as i64);
    }
}

/// Spawn the protocol-economy poller. Polls `api_url` every
/// `interval` (default 60s) and updates the LGT-economy gauges.
/// Fire-and-forget; lives until the runtime drops. Safe before the
/// api is reachable — early polls just skip-update until the network
/// path comes up.
pub fn spawn_economy_task(api_url: String, interval: Duration) {
    init_economy_metrics();
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(interval);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            ticker.tick().await;
            sample_economy(&api_url).await;
        }
    });
}

// ============================================================================
// Unit tests
//
// Placed at the very end so clippy's `items_after_test_module` lint is
// satisfied. When you add new test cases, add them HERE — don't insert
// a new `#[cfg(test)] mod` higher up in the file or downstream items
// stop compiling.
// ============================================================================

#[cfg(test)]
mod sequencer_role_tests {
    use super::encode_role;

    #[test]
    fn encode_leader() {
        let (v, l) = encode_role("\"BatchProducer\"");
        assert_eq!(v, 2);
        assert_eq!(l, "leader");
    }

    #[test]
    fn encode_replica() {
        let (v, l) = encode_role("\"PgSyncReplica\"");
        assert_eq!(v, 1);
        assert_eq!(l, "replica");
    }

    #[test]
    fn encode_unknown_on_empty() {
        let (v, l) = encode_role("");
        assert_eq!(v, 0);
        assert_eq!(l, "unknown");
    }

    #[test]
    fn encode_unknown_on_garbage() {
        // Future role variant or chain on a different version: degrade
        // to unknown rather than crashing the gauge.
        let (v, l) = encode_role("\"SomeFutureRole\"");
        assert_eq!(v, 0);
        assert_eq!(l, "unknown");
    }

    #[test]
    fn encode_handles_whitespace() {
        let (v, l) = encode_role("  \"BatchProducer\"  \n");
        assert_eq!(v, 2);
        assert_eq!(l, "leader");
    }
}
