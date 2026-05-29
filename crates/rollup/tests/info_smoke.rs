//! End-to-end smoke test for the `/v1/rollup/info` HTTP endpoint.
//!
//! Boots the same `add_routes` shape `mock_rollup.rs` and
//! `celestia_rollup.rs` mount in production on an ephemeral port, hits
//! the route over real HTTP, asserts the response shape.
//!
//! Catches:
//!
//! 1. Router wiring + `State` extractor for `InfoState`.
//! 2. JSON shape: `{"chain_id":"...","chain_hash":"lsch1...","version":"..."}`.
//! 3. `chain_hash` always renders as bech32m with HRP `lsch`, so a
//!    regression in the encoder breaks wallets / explorers reading
//!    the value. Decode round-trip pins the bytes.
//! 4. The `version` field reflects the binary's `CARGO_PKG_VERSION`.
//!
//! Tracking issue: #181.

use std::net::SocketAddr;
use std::time::Duration;

use ligate_rollup::info::{self, RollupInfo};
use sov_rollup_interface::ChainHash;
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
    assert!(
        body.chain_hash.starts_with("lsch1"),
        "chain_hash must be bech32m with HRP `lsch`, got {}",
        body.chain_hash
    );
    let parsed: ChainHash = body.chain_hash.parse().expect("bech32m parses");
    assert_eq!(parsed.0, chain_hash, "chain_hash decodes back to the configured bytes");
    assert_eq!(body.version, env!("CARGO_PKG_VERSION"));
}

#[tokio::test(flavor = "multi_thread")]
async fn info_round_trips_zero_chain_hash() {
    // The placeholder CHAIN_HASH ([0; 32]) only fires before
    // `build.rs` has run; production never serves it. Snapshot the
    // bech32m shape anyway so a regression in the encoder (e.g.
    // changing HRP, dropping leading bytes) is caught even on the
    // placeholder.
    let zeros = [0u8; 32];
    let addr = spawn_info_server("ligate-localnet", zeros).await;

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

    assert!(body.chain_hash.starts_with("lsch1"), "got {}", body.chain_hash);
    let parsed: ChainHash = body.chain_hash.parse().expect("bech32m parses");
    assert_eq!(parsed.0, zeros);
}

#[tokio::test(flavor = "multi_thread")]
async fn info_handles_concurrent_requests() {
    // Operator dashboards poll `/rollup/info` at most once per
    // refresh, but a misbehaving load balancer or webhook could fire
    // many parallel reads. Ensure the handler doesn't serialize
    // (would surface as visible latency on a busy node).
    let addr = spawn_info_server("ligate-devnet-3", [0xABu8; 32]).await;

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
            assert_eq!(body.chain_id, "ligate-devnet-3");
        }));
    }
    for j in joins {
        j.await.expect("task completes");
    }
}
