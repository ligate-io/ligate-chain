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
    // Phase 2 of #110. Builds a tempdir with a 1 MB file, drives
    // `sample_state_db_size` once, and asserts the
    // `ligate_state_db_size_bytes` gauge in `/metrics` reflects
    // the real on-disk allocation. Catches:
    //
    // 1. The directory walker (recursive, skips unreadable entries).
    // 2. The Prometheus gauge wiring (`IntGauge::set` + Prometheus
    //    text-format encoding).
    // 3. End-to-end /metrics rendering for a numeric metric.
    // 4. The on-disk-vs-nominal semantics: we use `blocks() * 512`
    //    (matches `du`), not `len()` (which over-reports on the
    //    sparse-allocated files NOMT and RocksDB produce).
    //
    // 1 MB rather than a few bytes so filesystem block-size noise
    // (4 KB on most setups) doesn't dominate the assertion.
    use std::ops::RangeInclusive;

    // A non-sparse 1 MiB file uses ~1 MiB of blocks. Allow a 5%
    // over-allocation slack to absorb filesystem block sizing
    // differences (ext4 / APFS / ZFS / tmpfs all behave a bit
    // differently). Sparse files would trip a much smaller value;
    // a `len()`-based regression would trip a much larger one.
    const FILE_SIZE: usize = 1_048_576;
    let acceptable: RangeInclusive<u64> = (FILE_SIZE as u64)..=(FILE_SIZE as u64 * 105 / 100);

    let dir = tempfile::tempdir().expect("tempdir");
    let nested = dir.path().join("nested");
    std::fs::create_dir(&nested).unwrap();
    std::fs::write(nested.join("blob.bin"), vec![0xABu8; FILE_SIZE]).unwrap();

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

    // Extract the numeric value from the gauge line.
    let line = body
        .lines()
        .find(|l| l.starts_with("ligate_state_db_size_bytes "))
        .unwrap_or_else(|| panic!("ligate_state_db_size_bytes missing from /metrics:\n{body}"));
    let value: u64 = line
        .split_whitespace()
        .nth(1)
        .and_then(|v| v.parse().ok())
        .unwrap_or_else(|| panic!("could not parse value from line: {line}"));

    assert!(
        acceptable.contains(&value),
        "expected gauge in {:?} after writing a 1 MiB file, got {} (bug regression: \
         over-reports indicate `len()` semantics; under-reports indicate sparse-file \
         under-counting)",
        acceptable,
        value,
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn rpc_middleware_records_matched_requests() {
    // Phase 2 of #110. Spins up a tiny test router with one
    // parameterised route, applies the metrics middleware, sends
    // two requests against different concrete `:id` values, then
    // asserts the counter and histogram both record under the
    // route TEMPLATE label (not the concrete value).
    //
    // Catches:
    //  1. Middleware wires correctly via `Router::layer`.
    //  2. `MatchedPath::as_str()` collapses concrete IDs into the
    //     template form, keeping label cardinality bounded.
    //  3. Histogram counts (`_count`) and counter both increment.

    use axum::routing::get;
    use axum::Router;
    use ligate_rollup::metrics::record_rpc_request;
    use tokio::net::TcpListener;

    metrics::init_rpc_metrics();

    // Test router: `/test/:id` returns "ok" with a known body.
    let app = Router::new()
        .route("/test/{id}", get(|| async { "ok" }))
        .layer(axum::middleware::from_fn(record_rpc_request));

    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind ephemeral");
    let test_addr = listener.local_addr().expect("test listener has addr");
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    tokio::task::yield_now().await;

    let metrics_addr = spawn_metrics_server().await;

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .expect("reqwest client builds");

    // Two requests with different concrete path params; both should
    // collapse to the same `/test/{id}` template label.
    for id in ["123", "456"] {
        let resp = client
            .get(format!("http://{test_addr}/test/{id}"))
            .send()
            .await
            .expect("request succeeds");
        assert_eq!(resp.status().as_u16(), 200);
    }

    let body = client
        .get(format!("http://{metrics_addr}/metrics"))
        .send()
        .await
        .expect("metrics endpoint responds")
        .text()
        .await
        .expect("body is utf-8");

    // Counter: two hits on the same template label.
    assert!(
        body.contains(r#"ligate_rpc_requests_total{endpoint="/test/{id}",status="200"} 2"#),
        "expected counter to bump twice on the templated route, got:\n{body}",
    );

    // Histogram: `_count` line records two observations on the
    // template label. Histogram bucket lines are also emitted but
    // their values depend on timing; `_count` is deterministic.
    assert!(
        body.contains(r#"ligate_rpc_request_duration_seconds_count{endpoint="/test/{id}"} 2"#),
        "expected histogram count to be 2, got:\n{body}",
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
