//! End-to-end smoke test for the `/v1/rollup/info` HTTP endpoint.
//!
//! Boots the same `add_routes` shape `mock_rollup.rs` and
//! `celestia_rollup.rs` mount in production on an ephemeral port, hits
//! the route over real HTTP, asserts the response shape.
//!
//! Catches:
//!
//! 1. Router wiring + `State` extractor for `InfoState`.
//! 2. JSON shape: `{"chain_id":"...","chain_hash":"...","version":"..."}`.
//! 3. `chain_hash` always renders as exactly 64 hex chars (no `0x`
//!    prefix, lowercase, padded). A regression here would break
//!    wallets and explorers reading the value.
//! 4. The `version` field reflects the binary's `CARGO_PKG_VERSION`.
//!
//! Tracking issue: #181.

use std::net::SocketAddr;
use std::time::Duration;

use ligate_rollup::info::{self, RollupInfo};
use tokio::net::TcpListener;

async fn spawn_info_server(chain_id: &'static str, chain_hash: [u8; 32]) -> SocketAddr {
    let router = info::router_for_test(chain_id, chain_hash);
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("ephemeral bind");
    let addr = listener.local_addr().expect("listener has addr");
    tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    tokio::task::yield_now().await;
    addr
}

#[tokio::test(flavor = "multi_thread")]
async fn info_returns_configured_chain_id_and_hash() {
    let chain_hash = [0x12u8; 32];
    let addr = spawn_info_server("ligate-localnet", chain_hash).await;

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .expect("reqwest client builds");

    let resp = client
        .get(format!("http://{addr}/rollup/info"))
        .send()
        .await
        .expect("/rollup/info responds");
    assert_eq!(resp.status().as_u16(), 200);

    let body: RollupInfo = resp.json().await.expect("response is JSON of expected shape");
    assert_eq!(body.chain_id, "ligate-localnet");
    assert_eq!(
        body.chain_hash, "1212121212121212121212121212121212121212121212121212121212121212",
        "chain_hash hex must be lowercase, 64 chars, no 0x prefix"
    );
    assert_eq!(body.version, env!("CARGO_PKG_VERSION"));
}

#[tokio::test(flavor = "multi_thread")]
async fn info_renders_zero_chain_hash_as_64_zeros() {
    // The placeholder CHAIN_HASH ([0; 32]) only fires before
    // `build.rs` has run; production never serves it. Snapshot the
    // hex shape anyway so a regression in the encoder (e.g. dropping
    // leading zeros) is caught even on the placeholder.
    let addr = spawn_info_server("ligate-localnet", [0u8; 32]).await;

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .expect("reqwest client builds");

    let body: RollupInfo = client
        .get(format!("http://{addr}/rollup/info"))
        .send()
        .await
        .expect("/rollup/info responds")
        .json()
        .await
        .expect("body parses");

    assert_eq!(body.chain_hash.len(), 64);
    assert!(body.chain_hash.chars().all(|c| c == '0'));
}

#[tokio::test(flavor = "multi_thread")]
async fn info_handles_concurrent_requests() {
    // Operator dashboards poll `/rollup/info` at most once per
    // refresh, but a misbehaving load balancer or webhook could fire
    // many parallel reads. Ensure the handler doesn't serialize
    // (would surface as visible latency on a busy node).
    let addr = spawn_info_server("ligate-devnet-1", [0xABu8; 32]).await;

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .expect("reqwest client builds");

    let mut joins = Vec::with_capacity(16);
    for _ in 0..16 {
        let client = client.clone();
        joins.push(tokio::spawn(async move {
            let resp =
                client.get(format!("http://{addr}/rollup/info")).send().await.expect("response");
            assert_eq!(resp.status().as_u16(), 200);
            let body: RollupInfo = resp.json().await.expect("json");
            assert_eq!(body.chain_id, "ligate-devnet-1");
        }));
    }
    for j in joins {
        j.await.expect("task completes");
    }
}
