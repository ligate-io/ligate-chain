//! Binary-spawn E2E smoke test.
//!
//! Spawns the actual `ligate-node` binary as a child process,
//! lets it bind a real HTTP listener, and round-trips a couple of
//! REST queries. Covers the surface area the in-process
//! TestRunner-based `e2e_smoke.rs` deliberately doesn't:
//!
//! 1. **Binary CLI argument parsing.** `ligate-node
//!    --rollup-config-path ... --genesis-config-dir ...` actually
//!    parses the way the README documents.
//! 2. **DA service init.** MockDa (sqlite-backed) opens its
//!    connection successfully under the binary's startup ordering.
//! 3. **Process startup ordering.** Genesis loader → state init →
//!    REST router → sequencer all wire in the right order without
//!    deadlock or race.
//! 4. **Network listener binding.** The configured `[runner.http_config]`
//!    port actually binds and accepts connections.
//! 5. **Real HTTP request/response.** `GET /v1/rollup/info` returns the
//!    documented JSON shape over the wire (not just in-process).
//!
//! ## Scope of this v1
//!
//! What this test DOES exercise (per the surfaces above):
//! - Binary CLI parsing
//! - DA service init (MockDa)
//! - Process startup ordering
//! - Listener bind
//! - REST request/response cycle for `/v1/rollup/info` +
//!   `/v1/ledger/slots/latest`
//!
//! What this v1 does NOT exercise (deliberately deferred):
//! - The production `RollupAuthenticator` tx-submission path. That's
//!   acceptance-criteria item 3 of the source issue (#146). It needs
//!   the cli's signing client (`ligate-client::submit::Submitter`)
//!   linked into the test, plus signed `RegisterSchema` and
//!   `SubmitAttestation` payload construction. A second pass on this
//!   file extends the test to submit + assert per the original spec.
//!
//! Splitting v1 (no submit) vs v2 (with submit) keeps the first
//! ship under the 5-minute CI budget the issue calls out.
//!
//! ## Why temp config / ephemeral port
//!
//! The repo's `devnet/rollup.toml` hardcodes `bind_port = 12346` and
//! `[storage].path = "devnet/data"`. Running multiple tests (or a
//! local dev node) in parallel would collide on both. The test
//! materialises a temp rollup.toml with the port + storage path
//! rewritten, and points the binary at it.

use std::path::Path;
use std::time::Duration;

use tempfile::TempDir;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;
use tokio::process::{Child, Command};
use tokio::time::{sleep, Instant};

/// Hard ceiling on how long we'll wait for the node's `/v1/rollup/info`
/// to return 200. Cold first-tick on MockDa is ~1s; 60s is generous
/// for slow CI runners.
const READY_TIMEOUT: Duration = Duration::from_secs(60);

/// Polling cadence while waiting for readiness.
const READY_POLL: Duration = Duration::from_millis(250);

/// `cargo test` sets `CARGO_BIN_EXE_<name>` to the absolute path of
/// the built binary, so the test harness builds `ligate-node` (the
/// `[[bin]]` in this crate's Cargo.toml) before running the test
/// and we get a stable path here without invoking cargo recursively.
fn ligate_node_binary() -> &'static str {
    env!("CARGO_BIN_EXE_ligate-node")
}

/// Allocate an ephemeral port by binding `127.0.0.1:0`, reading the
/// assigned port, then closing the socket so the spawned node can
/// reuse it. There's a TOCTOU window between drop and bind by the
/// child, but the kernel won't recycle the port that fast under
/// normal load — flake risk is acceptable for a smoke test.
async fn pick_ephemeral_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    port
}

/// Build a rollup.toml string with the bind_port and storage paths
/// rewritten to the per-test values. The rest of the config (chain
/// id, sequencer, DA finality, etc.) stays identical to the repo's
/// devnet template so the test exercises the same boot path
/// operators run.
///
/// We include the canonical devnet config at build time and do a
/// surgical string replacement rather than a TOML parse/serialise
/// roundtrip — toml-rs's serializer reorders sections and drops
/// blank lines, which would make the diff between this and the
/// committed template noisy and hard to audit.
fn render_rollup_toml(bind_port: u16, data_dir: &Path) -> String {
    let template = include_str!("../../../devnet/rollup.toml");
    let data_dir = data_dir.to_string_lossy();
    template
        .replace(
            "connection_string = \"sqlite://devnet/data/da.sqlite?mode=rwc\"",
            &format!("connection_string = \"sqlite://{data_dir}/da.sqlite?mode=rwc\""),
        )
        .replace("path = \"devnet/data\"", &format!("path = \"{data_dir}\""))
        .replace("bind_port = 12346", &format!("bind_port = {bind_port}"))
        .replace(
            "telegraf_address = \"udp://127.0.0.1:8094\"",
            // Bind telegraf to a different port too so concurrent
            // tests don't all collide on 8094. UDP drops packets when
            // nothing's listening, so any port works.
            &format!("telegraf_address = \"udp://127.0.0.1:{}\"", bind_port.wrapping_add(1000)),
        )
}

/// Spawn `ligate-node` as a child process, return the Child + URL.
///
/// Inherits the test's env, including `SKIP_GUEST_BUILD` /
/// `CONSTANTS_MANIFEST_PATH` which CI sets at the workflow level. On
/// the operator host these are the same envs the README documents
/// for `cargo install` — the test exercises that env-var contract.
async fn spawn_node(temp_dir: &TempDir, port: u16) -> Child {
    // Write the temp rollup.toml.
    let rollup_path = temp_dir.path().join("rollup.toml");
    let mut rollup_file = tokio::fs::File::create(&rollup_path).await.unwrap();
    let rendered = render_rollup_toml(port, temp_dir.path());
    rollup_file.write_all(rendered.as_bytes()).await.unwrap();
    rollup_file.flush().await.unwrap();
    drop(rollup_file);

    // Use the repo's canonical genesis bundle. The genesis is
    // append-only for tests — none of the configs reference paths
    // that need rewriting (they're all in-process module configs).
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let genesis_dir = format!("{manifest_dir}/../../devnet/genesis");

    Command::new(ligate_node_binary())
        .arg("--rollup-config-path")
        .arg(&rollup_path)
        .arg("--genesis-config-dir")
        .arg(&genesis_dir)
        // Detach from the test process group so the kill in
        // `drop_child_on_panic` below cleanly tears it down.
        .kill_on_drop(true)
        .spawn()
        .expect("spawn ligate-node binary")
}

/// Poll `GET {base}/v1/rollup/info` until 200 or [`READY_TIMEOUT`].
async fn wait_for_ready(base: &str) -> Result<(), String> {
    let url = format!("{base}/v1/rollup/info");
    let client = reqwest::Client::builder().timeout(Duration::from_secs(2)).build().unwrap();
    let started = Instant::now();
    loop {
        if started.elapsed() > READY_TIMEOUT {
            return Err(format!("timeout waiting for {url} to return 200"));
        }
        match client.get(&url).send().await {
            Ok(resp) if resp.status().as_u16() == 200 => return Ok(()),
            _ => sleep(READY_POLL).await,
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn binary_spawn_round_trips_info_and_latest_slot() {
    // 1. Per-test workspace + port.
    let temp_dir = TempDir::new().expect("temp dir");
    let port = pick_ephemeral_port().await;
    let base = format!("http://127.0.0.1:{port}");

    // 2. Spawn the node binary. `kill_on_drop` cleans it up if any
    //    assertion below panics.
    let _child = spawn_node(&temp_dir, port).await;

    // 3. Wait for the REST listener to be up.
    wait_for_ready(&base).await.expect("node ready");

    // 4. Hit /v1/rollup/info and verify the documented JSON shape.
    let client = reqwest::Client::new();
    let info: serde_json::Value =
        client.get(format!("{base}/v1/rollup/info")).send().await.unwrap().json().await.unwrap();
    assert_eq!(info["chain_id"], "ligate-localnet");
    let chain_hash = info["chain_hash"].as_str().expect("chain_hash is a string");
    assert!(
        chain_hash.starts_with("lsch1"),
        "chain_hash should be bech32m with lsch HRP, got {chain_hash}"
    );

    // 5. Hit /v1/ledger/slots/latest. Even on a fresh chain the
    //    sequencer produces a slot within ~1s of boot (block_time_ms
    //    = 1000 in the config we materialised); poll for up to 10s
    //    in case CI is slow.
    let slot = poll_latest_slot(&client, &base).await.expect("got slot");
    assert!(
        slot["hash"].as_str().map(|h| h.starts_with("lblk1")).unwrap_or(false),
        "slot.hash should be bech32m with lblk HRP, got {:?}",
        slot["hash"]
    );

    // 6. _child drops here, kill_on_drop sends SIGKILL to the node.
    //    temp_dir drops next, RocksDB + sqlite files cleared.
}

/// Poll the latest-slot endpoint until it returns 200 (chain has
/// produced at least one slot). Bounded retry so a sequencer that
/// fails to produce slots doesn't wedge the test.
async fn poll_latest_slot(
    client: &reqwest::Client,
    base: &str,
) -> Result<serde_json::Value, String> {
    let url = format!("{base}/v1/ledger/slots/latest");
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if let Ok(resp) = client.get(&url).send().await {
            if resp.status().as_u16() == 200 {
                return resp.json().await.map_err(|e| e.to_string());
            }
        }
        sleep(Duration::from_millis(200)).await;
    }
    Err(format!("timeout waiting for {url}"))
}
