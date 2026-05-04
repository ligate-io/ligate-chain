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

use anyhow::Context as _;
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use prometheus::{Encoder, TextEncoder};
use tokio::net::TcpListener;
use tracing::{info, warn};

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
