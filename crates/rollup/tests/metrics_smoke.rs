//! End-to-end smoke test for the Prometheus `/metrics` endpoint.
//!
//! Boots the same axum router that `ligate-node` runs in production
//! on an ephemeral port, scrapes `/metrics` over real HTTP, and
//! asserts the attestation counters render. Catches:
//!
//! 1. The `bind` -> `serve` plumbing in `ligate_rollup::metrics`.
//! 2. The Prometheus default-registry encoding (text/plain v0.0.4
//!    with the right metric names + HELP / TYPE preambles).
//! 3. The `attestation::metrics::init()` cold-start touch:
//!    counters are emitted at zero before any handler fires.
//!
//! Tracking issue: #110 (Phase 1 scope).

use std::net::SocketAddr;
use std::time::Duration;

use ligate_rollup::metrics;

/// Bind on `127.0.0.1:0`, spawn the server, return the actual
/// listening address. The server task is leaked into the runtime;
/// it gets dropped when the test's tokio runtime tears down.
async fn spawn_metrics_server() -> SocketAddr {
    let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
    let listener = metrics::bind(addr).await.expect("ephemeral bind succeeds");
    let actual = listener.local_addr().expect("listener has local addr");
    tokio::spawn(async move {
        let _ = metrics::serve(listener).await;
    });
    // axum's serve sets up the loop on the listener inside the
    // spawned task; on most platforms `bind` already accepts
    // connections by the time we return here, but yield once so
    // the spawned task gets to wire its handler before we connect.
    tokio::task::yield_now().await;
    actual
}

#[tokio::test(flavor = "multi_thread")]
async fn metrics_endpoint_serves_attestation_counters_at_zero() {
    // Pre-touch the counters so they emit a zero line before any
    // handler fires. This is what `main.rs` does at startup; the
    // test mirrors the production cold-start path.
    attestation::metrics::init();

    let addr = spawn_metrics_server().await;

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .expect("reqwest client builds");

    let url = format!("http://{addr}/metrics");
    let resp = client.get(&url).send().await.expect("metrics endpoint responds");
    assert_eq!(resp.status().as_u16(), 200, "expected 200, got {}", resp.status());

    let content_type = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|h| h.to_str().ok())
        .unwrap_or("")
        .to_string();
    assert!(
        content_type.starts_with("text/plain"),
        "expected text/plain content-type, got {content_type}",
    );

    let body = resp.text().await.expect("body is utf-8");

    // Each counter has a HELP line, a TYPE line, and a value line.
    // We assert the metric name shows up at least once; the
    // Prometheus parser will validate the rest at scrape time.
    for expected in [
        "ligate_attestor_sets_registered_total",
        "ligate_schemas_registered_total",
        "ligate_attestations_submitted_total",
    ] {
        assert!(
            body.contains(expected),
            "expected metric '{expected}' in /metrics body, got:\n{body}",
        );
    }

    // Cold-start values should all be zero. Substring matches the
    // line `<metric_name> 0` (Prometheus text format puts the
    // value after the name, no labels for these counters).
    for expected in [
        "ligate_attestor_sets_registered_total 0",
        "ligate_schemas_registered_total 0",
        "ligate_attestations_submitted_total 0",
    ] {
        assert!(body.contains(expected), "expected '{expected}' (cold-start zero), got:\n{body}",);
    }
}
