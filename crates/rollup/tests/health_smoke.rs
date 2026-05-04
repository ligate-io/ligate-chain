//! End-to-end smoke test for the `/health` and `/ready` HTTP
//! endpoints.
//!
//! Boots the same `add_routes` shape `mock_rollup.rs` and
//! `celestia_rollup.rs` mount in production on an ephemeral port,
//! drives `SyncStatus` through both the synced and syncing
//! variants via the test-side `watch::Sender`, scrapes both routes
//! over real HTTP, and asserts status codes + body content.
//!
//! Catches:
//!  1. Router wiring + State extractor (`HealthState` clones cleanly
//!     into the request handler).
//!  2. JSON shape: `{"status":"synced","synced_da_height":N}` and
//!     `{"status":"syncing","synced_da_height":N,"target_da_height":M}`.
//!  3. 200 vs 503 status codes per sync state.
//!  4. The watch-channel update propagating to subsequent requests
//!     (operators expect /ready to flip to 200 the moment the
//!     sequencer transitions to Synced).
//!
//! Tracking issue: #176.

use std::net::SocketAddr;
use std::time::Duration;

use ligate_rollup::health;
use sov_rollup_interface::node::SyncStatus;
use tokio::net::TcpListener;

async fn spawn_health_server(
    initial: SyncStatus,
) -> (SocketAddr, tokio::sync::watch::Sender<SyncStatus>) {
    let (router, tx) = health::router_for_test(initial);
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("ephemeral bind");
    let addr = listener.local_addr().expect("listener has addr");
    tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    tokio::task::yield_now().await;
    (addr, tx)
}

#[tokio::test(flavor = "multi_thread")]
async fn health_returns_ok_unconditionally() {
    let (addr, _tx) =
        spawn_health_server(SyncStatus::Syncing { synced_da_height: 0, target_da_height: 100 })
            .await;

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .expect("reqwest client builds");

    let resp = client.get(format!("http://{addr}/health")).send().await.expect("/health responds");
    assert_eq!(resp.status().as_u16(), 200);
    let body = resp.text().await.expect("body utf-8");
    assert_eq!(body, r#"{"status":"ok"}"#);
}

#[tokio::test(flavor = "multi_thread")]
async fn ready_returns_200_when_synced() {
    let (addr, _tx) = spawn_health_server(SyncStatus::Synced { synced_da_height: 42 }).await;

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .expect("reqwest client builds");

    let resp = client.get(format!("http://{addr}/ready")).send().await.expect("/ready responds");
    assert_eq!(resp.status().as_u16(), 200);
    let body = resp.text().await.expect("body utf-8");
    assert!(body.contains(r#""status":"synced""#), "body: {body}");
    assert!(body.contains(r#""synced_da_height":42"#), "body: {body}");
}

#[tokio::test(flavor = "multi_thread")]
async fn ready_returns_503_when_syncing() {
    let (addr, _tx) =
        spawn_health_server(SyncStatus::Syncing { synced_da_height: 100, target_da_height: 1000 })
            .await;

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .expect("reqwest client builds");

    let resp = client.get(format!("http://{addr}/ready")).send().await.expect("/ready responds");
    assert_eq!(resp.status().as_u16(), 503);
    let body = resp.text().await.expect("body utf-8");
    assert!(body.contains(r#""status":"syncing""#), "body: {body}");
    assert!(body.contains(r#""synced_da_height":100"#), "body: {body}");
    assert!(body.contains(r#""target_da_height":1000"#), "body: {body}");
}

#[tokio::test(flavor = "multi_thread")]
async fn ready_flips_to_200_after_sync_completes() {
    // Operator scenario: a node boots syncing, replays N blocks, hits
    // synced state. /ready should flip from 503 to 200 the moment
    // the watch channel sees the new value.
    let (addr, tx) =
        spawn_health_server(SyncStatus::Syncing { synced_da_height: 5, target_da_height: 100 })
            .await;

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .expect("reqwest client builds");

    // First call: still syncing.
    let resp = client.get(format!("http://{addr}/ready")).send().await.expect("/ready responds");
    assert_eq!(resp.status().as_u16(), 503);

    // Sequencer caught up; flip the watch channel.
    tx.send(SyncStatus::Synced { synced_da_height: 100 }).expect("send");

    // Second call: now 200.
    let resp = client.get(format!("http://{addr}/ready")).send().await.expect("/ready responds");
    assert_eq!(resp.status().as_u16(), 200);
    let body = resp.text().await.expect("body utf-8");
    assert!(body.contains(r#""synced_da_height":100"#), "body: {body}");
}
