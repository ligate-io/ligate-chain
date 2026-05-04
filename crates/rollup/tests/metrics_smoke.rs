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

#[tokio::test(flavor = "multi_thread")]
async fn state_db_size_gauge_reflects_disk_usage() {
    // Phase 2 of #110. Builds a tempdir with a known total file
    // size, drives `sample_state_db_size` once, and asserts the
    // `ligate_state_db_size_bytes` gauge in `/metrics` matches the
    // sampled value. Catches:
    //
    // 1. The directory walker (recursive, follows symlinks one
    //    level, skips unreadable entries).
    // 2. The Prometheus gauge wiring (`IntGauge::set` + Prometheus
    //    text-format encoding).
    // 3. End-to-end /metrics rendering for a numeric metric (the
    //    rest of the smoke test only exercises counters).

    let dir = tempfile::tempdir().expect("tempdir");

    // Write three files at different depths totalling 6 bytes.
    let nested = dir.path().join("nested");
    std::fs::create_dir(&nested).unwrap();
    std::fs::write(dir.path().join("a.bin"), b"AA").unwrap();
    std::fs::write(dir.path().join("b.bin"), b"BB").unwrap();
    std::fs::write(nested.join("c.bin"), b"CC").unwrap();

    metrics::sample_state_db_size(dir.path());

    let addr = spawn_metrics_server().await;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .expect("reqwest client builds");
    let body = client
        .get(format!("http://{addr}/metrics"))
        .send()
        .await
        .expect("metrics endpoint responds")
        .text()
        .await
        .expect("body is utf-8");

    // Substring match: `ligate_state_db_size_bytes 6` is the
    // canonical Prometheus text-format line for our gauge.
    assert!(
        body.contains("ligate_state_db_size_bytes 6"),
        "expected gauge to read 6 bytes after sampling 3 x 2-byte files, got:\n{body}",
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn block_height_gauge_renders_sample_value() {
    // Phase 2 of #110. Drives `sample_block_height` directly with a
    // known SlotNumber and asserts `/metrics` reflects it. Skips the
    // polling task + LedgerDb (testing both end-to-end requires a
    // booted node, which is the Tier 2 manual-verification surface).
    use sov_rollup_interface::common::SlotNumber;

    metrics::sample_block_height(SlotNumber::new(1234));

    let addr = spawn_metrics_server().await;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .expect("reqwest client builds");
    let body = client
        .get(format!("http://{addr}/metrics"))
        .send()
        .await
        .expect("metrics endpoint responds")
        .text()
        .await
        .expect("body is utf-8");

    assert!(
        body.contains("ligate_block_height 1234"),
        "expected `ligate_block_height 1234` after sampling slot 1234, got:\n{body}",
    );
}
