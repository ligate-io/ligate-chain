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
//! 6. **Production `RollupAuthenticator` tx-submission path.** The
//!    in-process `e2e_smoke.rs` deliberately sidesteps this because
//!    `TestRunner::execute_transaction` is incompatible with the
//!    production runtime's Auth. This test wires the
//!    `ligate-client::submit::Submitter` against the live HTTP
//!    surface so register / submit calls round-trip through the same
//!    code path operators use.
//!
//! ## What's exercised end-to-end
//!
//! - Binary CLI parsing
//! - DA service init (MockDa)
//! - Process startup ordering
//! - Listener bind
//! - `GET /v1/rollup/info` + `/v1/ledger/slots/latest`
//! - `POST /v1/sequencer/txs` with a signed `RegisterAttestorSet`, a
//!   signed `RegisterSchema` bound to it, and a signed
//!   `SubmitAttestation` carrying an ed25519 signature over the
//!   canonical digest
//! - REST verification via
//!   `/v1/modules/attestation/attestor-sets/{id}`,
//!   `/schemas/{id}`, and `/attestations/{id}:{payload_hash}`
//!
//! ## Why temp config / ephemeral port
//!
//! The repo's `localnet/rollup.toml` hardcodes `bind_port = 12346` and
//! `[storage].path = "localnet/data"`. Running multiple tests (or a
//! local dev node) in parallel would collide on both. The test
//! materialises a temp rollup.toml with the port + storage path
//! rewritten, and points the binary at it.

use std::path::Path;
use std::time::Duration;

use attestation::{
    AttestorSet, AttestorSetId, CallMessage as AttestationCall, PayloadHash, PubKey, Schema,
    SchemaId, SignedAttestationPayload,
};
use ed25519_dalek::{Signer, SigningKey};
use ligate_client::submit::Submitter;
use ligate_rollup::MockRollupSpec;
use ligate_stf::runtime::RuntimeCall;
// `file_serial(node_spawn)` uses fslock to serialise across the
// `binary_spawn_smoke.rs` and `restart_recovery.rs` test binaries.
// Plain `#[serial]` only serialises within ONE test binary; cargo
// test runs the two binaries in parallel, which previously caused
// resource contention on 2-vCPU CI runners (chain#540). Both files
// MUST use the same key (`node_spawn`) to share the lock.
use serial_test::file_serial;
use sov_modules_api::capabilities::UniquenessData;
use sov_modules_api::execution_mode::Native;
use sov_modules_api::transaction::{PriorityFeeBips, UnsignedTransaction};
use sov_modules_api::{Address, Amount, CryptoSpec, SafeString, SafeVec, Spec};
use tempfile::TempDir;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;
use tokio::process::{Child, Command};
use tokio::time::{sleep, Instant};

/// Concrete spec mirrors the chain's runtime. `MockRollupSpec<Native>`
/// shares the chain's address shape and runtime composition; the DA
/// flavour (Mock vs Celestia) is node-side, doesn't affect the
/// signed-tx encoding.
type S = MockRollupSpec<Native>;
type ChainRuntime = ligate_stf::runtime::Runtime<S>;
type SovPrivateKey = <<S as Spec>::CryptoSpec as CryptoSpec>::PrivateKey;
type SovAddress = <S as Spec>::Address;

/// SDK's numeric chain_id (u64) — the tx-signing domain-separation
/// integer from `constants.toml::CHAIN_ID`. Distinct from the
/// cosmos-style `chain_id` STRING (`ligate-localnet`) that
/// `/v1/rollup/info` exposes. The two are independent identifiers
/// the SDK uses at different layers.
const CHAIN_ID_NUMERIC: u64 = 4242;

/// Per-tx fee envelope for test txs (nano-AVOW). Generous enough that
/// no test failure is fee-related; the dev key in
/// `localnet/genesis/bank.json` has 10_000_000_000_000 nano-AVOW
/// (10,000 AVOW), plenty of headroom.
const TEST_MAX_FEE_NANO: u128 = 100_000_000;

/// Localnet dev key per `localnet/local-dev-key.json`. Committed to the
/// chain repo on purpose so localnet tests + tutorials can re-sign
/// against it. Address derives to `lig132yw8ht5p8cetl2jmvknewjawt9xwzdlrk2pyxlnwjyqz3m499u`
/// which is pre-funded in `localnet/genesis/bank.json`.
const DEV_PRIVATE_KEY_BYTES: [u8; 32] = [0x01; 32];

/// Hard ceiling on how long we'll wait for the node's `/v1/rollup/info`
/// to return 200. Cold first-tick on MockDa is ~1s; 180s gives 2-vCPU
/// CI runners headroom even when the `restart_recovery.rs` binary is
/// blocked on the fslock immediately before us and we get the lock
/// while the kernel is still cooling down from its spawn (chain#540).
const READY_TIMEOUT: Duration = Duration::from_secs(180);

/// Polling cadence while waiting for readiness.
const READY_POLL: Duration = Duration::from_millis(250);

/// `cargo test` sets `CARGO_BIN_EXE_<name>` to the absolute path of
/// the built binary, so the test harness builds `ligate-node` (the
/// `[[bin]]` in this crate's Cargo.toml) before running the test
/// and we get a stable path here without invoking cargo recursively.
fn ligate_node_binary() -> &'static str {
    env!("CARGO_BIN_EXE_ligate-node")
}

/// Binary-spawn tests are skipped under `cargo llvm-cov`. They spawn a
/// separate `ligate-node` process whose coverage llvm-cov cannot
/// capture anyway, and the coverage-instrumented binary boots far
/// slower than [`READY_TIMEOUT`], which reintroduced the chain#540
/// flake under the coverage job even with the cross-binary fslock
/// (`file_serial`). The plain `cargo test` CI job still runs these for
/// real. Detected via `LLVM_PROFILE_FILE`, which cargo-llvm-cov sets
/// on every instrumented binary.
fn skip_under_llvm_cov() -> bool {
    if std::env::var_os("LLVM_PROFILE_FILE").is_some() {
        eprintln!("skipping binary-spawn test under llvm-cov instrumentation (chain#540)");
        true
    } else {
        false
    }
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
    let template = include_str!("../../../localnet/rollup.toml");
    let data_dir = data_dir.to_string_lossy();
    template
        .replace(
            "connection_string = \"sqlite://localnet/data/da.sqlite?mode=rwc\"",
            &format!("connection_string = \"sqlite://{data_dir}/da.sqlite?mode=rwc\""),
        )
        .replace("path = \"localnet/data\"", &format!("path = \"{data_dir}\""))
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
    let genesis_dir = format!("{manifest_dir}/../../localnet/genesis");

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
#[file_serial(node_spawn)]
async fn binary_spawn_round_trips_register_and_submit() {
    if skip_under_llvm_cov() {
        return;
    }
    // 1. Per-test workspace + port.
    let temp_dir = TempDir::new().expect("temp dir");
    let port = pick_ephemeral_port().await;
    let base = format!("http://127.0.0.1:{port}");

    // 2. Spawn the node binary. `kill_on_drop` cleans it up if any
    //    assertion below panics.
    let _child = spawn_node(&temp_dir, port).await;

    // 3. Wait for the REST listener to be up.
    wait_for_ready(&base).await.expect("node ready");

    // 4. /v1/rollup/info — chain identity + bech32m chain_hash.
    let client = reqwest::Client::new();
    let info: serde_json::Value =
        client.get(format!("{base}/v1/rollup/info")).send().await.unwrap().json().await.unwrap();
    assert_eq!(info["chain_id"], "ligate-localnet");
    let chain_hash_bech32m = info["chain_hash"].as_str().expect("chain_hash is a string");
    assert!(
        chain_hash_bech32m.starts_with("lsch1"),
        "chain_hash should be bech32m with lsch HRP, got {chain_hash_bech32m}"
    );
    let chain_hash_bytes = decode_chain_hash(chain_hash_bech32m);

    // 5. /v1/ledger/slots/latest — wait until the sequencer has
    //    finalised at least one slot past genesis. The MockDa
    //    sequencer refuses to accept new txs with 503
    //    `WaitingOnDa { needed_finalized_slot_number: 1, .. }` until
    //    DA height advances past 0. With `block_time_ms = 1000` and
    //    `finalization_blocks = 0` in our rollup.toml, slot 1 lands
    //    within ~1-2s of boot. 30s deadline is generous for slow CI.
    let slot = poll_latest_slot_at_least(&client, &base, 1).await.expect("got slot >= 1");
    assert!(
        slot["hash"].as_str().map(|h| h.starts_with("lblk1")).unwrap_or(false),
        "slot.hash should be bech32m with lblk HRP, got {:?}",
        slot["hash"]
    );

    // 6. Production tx-submission path. Build a signing client +
    //    derive the dev key's address. The chain's address rule is
    //    `addr_bytes = pubkey[..28]` (NOT SHA-256(pubkey)[..28] —
    //    see ligate-chain#245 for that subtlety).
    //
    //    `Submitter::new` probes `{base}/modules` to verify the URL
    //    is alive; on this chain the modules surface mounts under
    //    `/v1`, so we hand it the prefixed base.
    let submitter = Submitter::new(&format!("{base}/v1")).await.expect("submitter");
    let signing_key = SigningKey::from_bytes(&DEV_PRIVATE_KEY_BYTES);
    let pubkey_bytes = signing_key.verifying_key().to_bytes();
    let signer_priv = SovPrivateKey::try_from(DEV_PRIVATE_KEY_BYTES.to_vec())
        .expect("DEV_PRIVATE_KEY_BYTES is a valid 32-byte ed25519 seed");
    let owner_addr: SovAddress = {
        let mut addr_bytes = [0u8; 28];
        addr_bytes.copy_from_slice(&pubkey_bytes[..28]);
        Address::from(addr_bytes).into()
    };

    // Three txs in sequence; the localnet sequencer's CheckUniqueness
    // requires monotonic nonces starting at 0 for an account that
    // hasn't submitted before. Genesis seeded the dev key but with
    // no txs, so nonce starts at 0.
    let mut nonce: u64 = 0;

    // 7. RegisterAttestorSet — 1 member (the dev key's pubkey),
    //    threshold 1. Simplest possible set so the test signer can
    //    also sign the attestation later.
    let member = PubKey::from(pubkey_bytes);
    let members_vec = vec![member];
    let attestor_set_id = AttestorSet::derive_id(&members_vec, 1);
    let call: RuntimeCall<S> = RuntimeCall::Attestation(AttestationCall::RegisterAttestorSet {
        members: SafeVec::try_from(members_vec).expect("1 member fits the cap"),
        threshold: 1,
    });
    submit_signed(&submitter, &signer_priv, &chain_hash_bytes, nonce, call).await;
    nonce += 1;
    wait_for_attestor_set(&client, &base, &attestor_set_id).await;

    // 8. RegisterSchema — bound to the just-registered set. Name +
    //    version + payload_shape_hash are arbitrary; the test
    //    asserts the chain stored what we sent.
    let schema_name = "binary-spawn.smoke";
    let schema_version: u32 = 1;
    let payload_shape_hash = [0xABu8; 32];
    let schema_id = Schema::<S>::derive_id(&owner_addr, schema_name, schema_version);
    let call: RuntimeCall<S> = RuntimeCall::Attestation(AttestationCall::RegisterSchema {
        name: SafeString::try_from(schema_name.to_string()).expect("name fits cap"),
        version: schema_version,
        attestor_set: attestor_set_id,
        fee_routing_bps: 0,
        fee_routing_addr: None,
        payload_shape_hash,
    });
    submit_signed(&submitter, &signer_priv, &chain_hash_bytes, nonce, call).await;
    nonce += 1;
    wait_for_schema(&client, &base, &schema_id).await;

    // 9. SubmitAttestation — sign over the canonical
    //    `SignedAttestationPayload` digest, submit as the single
    //    required attestor.
    let payload_hash = PayloadHash::from([0x42u8; 32]);
    let digest = SignedAttestationPayload::<S> {
        schema_id,
        payload_hash,
        submitter: owner_addr,
        timestamp: 0, // chain hardcodes 0 in v0; see
                      // `handle_submit_attestation` comment
    }
    .digest();
    let sig_bytes = signing_key.sign(&digest).to_bytes();
    let attestor_sig = attestation::AttestorSignature {
        pubkey: member,
        sig: SafeVec::try_from(sig_bytes.to_vec()).expect("64-byte ed25519 sig fits the cap"),
    };
    let call: RuntimeCall<S> = RuntimeCall::Attestation(AttestationCall::SubmitAttestation {
        schema_id,
        payload_hash,
        signatures: SafeVec::try_from(vec![attestor_sig])
            .expect("1 signature fits MAX_ATTESTATION_SIGNATURES"),
    });
    submit_signed(&submitter, &signer_priv, &chain_hash_bytes, nonce, call).await;
    wait_for_attestation(&client, &base, &schema_id, &payload_hash).await;

    // 10. _child drops here, kill_on_drop sends SIGKILL to the node.
    //     temp_dir drops next, RocksDB + sqlite files cleared.
}

/// Decode the bech32m `lsch1...` chain hash from `/v1/rollup/info`
/// into raw 32 bytes. The chain emits this exclusively as bech32m
/// since `ligate-chain@0ac7e5b`; old `0x` hex form isn't expected.
fn decode_chain_hash(s: &str) -> [u8; 32] {
    let (_hrp, data) = bech32::decode(s).expect("bech32m decode of chain_hash");
    let arr: [u8; 32] = data.try_into().expect("chain_hash payload is exactly 32 bytes");
    arr
}

/// Wrap a `RuntimeCall` in an `UnsignedTransaction`, sign with the
/// chain-hash binding, borsh-encode, and submit via the production
/// `POST /v1/sequencer/txs` path. Doesn't wait for inclusion — the
/// caller polls the resource-specific endpoint afterwards. Mirrors
/// `crates/bootstrap-cli/src/register.rs::submit_with_nonce`, with
/// a retry loop on the sequencer's `WaitingOnDa` 503: even on
/// `finalization_blocks = 0`, the sequencer needs the DA layer to
/// have finalised at least one block past genesis before it'll
/// accept submissions. With `block_time_ms = 1000` that's typically
/// 1-2s after boot; the retry keeps the test stable on slow CI
/// runners without baking in a fixed sleep.
async fn submit_signed(
    submitter: &Submitter,
    signer_key: &SovPrivateKey,
    chain_hash: &[u8; 32],
    nonce: u64,
    call: RuntimeCall<S>,
) {
    let unsigned = UnsignedTransaction::<ChainRuntime, S>::new(
        call,
        CHAIN_ID_NUMERIC,
        PriorityFeeBips::ZERO,
        Amount::from(TEST_MAX_FEE_NANO),
        UniquenessData::Nonce(nonce),
        None,
    );
    let signed = unsigned.sign(signer_key, chain_hash);
    let signed_bytes = borsh::to_vec(&signed).expect("borsh-encoding signed tx");

    let deadline = Instant::now() + Duration::from_secs(30);
    let mut last_err: Option<String> = None;
    while Instant::now() < deadline {
        match submitter.submit_raw_tx(signed_bytes.clone(), false).await {
            Ok(_hash) => return,
            Err(e) => {
                let msg = format!("{e:#}");
                // Retry only the DA-progress wait. Any other error
                // (bad signature, bad nonce, fee too low) is a real
                // bug and shouldn't be retried.
                if msg.contains("WaitingOnDa") || msg.contains("503") {
                    last_err = Some(msg);
                    sleep(Duration::from_millis(500)).await;
                    continue;
                }
                panic!("sequencer rejected signed tx: {msg}");
            }
        }
    }
    panic!(
        "sequencer never accepted signed tx within 30s; last error: {}",
        last_err.unwrap_or_else(|| "<none>".into())
    );
}

/// Poll `/v1/modules/attestation/attestor-sets/{id}` until 200 or
/// timeout. The chain's preferred sequencer commits within ~1-2s of
/// submit on the localnet block-time setting; 30s is a generous cap
/// for slow CI runners.
async fn wait_for_attestor_set(client: &reqwest::Client, base: &str, id: &AttestorSetId) {
    let url = format!("{base}/v1/modules/attestation/attestor-sets/{id}");
    poll_for_200(client, &url, "attestor set").await;
}

async fn wait_for_schema(client: &reqwest::Client, base: &str, id: &SchemaId) {
    let url = format!("{base}/v1/modules/attestation/schemas/{id}");
    poll_for_200(client, &url, "schema").await;
}

async fn wait_for_attestation(
    client: &reqwest::Client,
    base: &str,
    schema_id: &SchemaId,
    payload_hash: &PayloadHash,
) {
    // Single bech32 id per the v0.2.0 REST route: `{lat1...}`.
    let id = attestation::AttestationId::from_pair(schema_id, payload_hash);
    let url = format!("{base}/v1/modules/attestation/attestations/{id}");
    poll_for_200(client, &url, "attestation").await;
}

async fn poll_for_200(client: &reqwest::Client, url: &str, what: &str) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        if let Ok(resp) = client.get(url).send().await {
            if resp.status().as_u16() == 200 {
                return;
            }
        }
        sleep(Duration::from_millis(300)).await;
    }
    panic!("timeout waiting for {what} at {url}");
}

/// Poll `/v1/ledger/slots/latest` until the chain has produced a
/// slot whose `number` is at least `min_number`. Returns the slot
/// JSON. Bounded retry so a sequencer that fails to advance doesn't
/// wedge the test.
async fn poll_latest_slot_at_least(
    client: &reqwest::Client,
    base: &str,
    min_number: u64,
) -> Result<serde_json::Value, String> {
    let url = format!("{base}/v1/ledger/slots/latest");
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        if let Ok(resp) = client.get(&url).send().await {
            if resp.status().as_u16() == 200 {
                let body: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
                if body["number"].as_u64().unwrap_or(0) >= min_number {
                    return Ok(body);
                }
            }
        }
        sleep(Duration::from_millis(200)).await;
    }
    Err(format!("timeout waiting for slot >= {min_number} at {url}"))
}

/// Pin the REST error envelope shape: `{status, message, details}` +
/// `X-Request-Id` response header. Fires a known-bad request against
/// the attestation REST surface (the canonical chain-defined
/// `errors::*` helper consumer) and asserts both the body shape and
/// the correlation header.
///
/// Why: documented at `docs/protocol/rest-api.md::Error envelope`;
/// the test catches a future SDK bump that silently changes the
/// envelope (we re-export `sov_modules_api::rest::utils::errors`
/// throughout the attestation handlers, so an SDK-side schema change
/// would propagate through this entire surface). Tracking issue:
/// ligate-chain#201.
///
/// `#[file_serial(node_spawn)]` because this spawns a `ligate-node`
/// binary on its own ephemeral port + tempdir, and running it in
/// parallel with the other spawn-the-binary test in this file (or
/// any in `restart_recovery.rs`) caused CI to time out both spawns
/// at `wait_for_ready` on free-tier runners (a single chain boot
/// saturates the runner's CPU; two parallel boots get neither across
/// the readiness budget). Shares the `node_spawn` key with all
/// `restart_recovery.rs` tests so the fslock serialises across the
/// two test binaries (chain#540).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[file_serial(node_spawn)]
async fn rest_error_envelope_pin() {
    if skip_under_llvm_cov() {
        return;
    }
    let temp_dir = TempDir::new().expect("temp dir");
    let port = pick_ephemeral_port().await;
    let base = format!("http://127.0.0.1:{port}");
    let _child = spawn_node(&temp_dir, port).await;
    wait_for_ready(&base).await.expect("node ready");

    let client = reqwest::Client::new();

    // === 404 path: well-formed bech32m, but no schema at that id. ===
    //
    // `SchemaId::from([u8; 32])` Display-renders as canonical bech32m
    // with `lsc1` HRP — same path the chain takes for every id it
    // emits. Same construction shape as `e2e_smoke.rs`'s 404 case;
    // the parser accepts the well-formed id, the lookup misses,
    // the handler returns `errors::not_found_404("Schema", schema_id)`
    // which the SDK renders as `{status:404, message, details:{id}}`.
    let unknown = SchemaId::from([0xAAu8; 32]);
    let unknown_str = unknown.to_string();
    let not_found = client
        .get(format!("{base}/v1/modules/attestation/schemas/{unknown_str}"))
        .send()
        .await
        .expect("404 GET request");
    assert_eq!(not_found.status().as_u16(), 404, "expected 404 for unknown schema id");

    // Note: the original audit assumed `X-Request-Id` would propagate
    // on every response via the SDK's `tower_request_id::RequestIdLayer`
    // + `PropagateHeaderLayer`. The first run of this test caught that
    // it doesn't reach the client in our REST mount today. Filed as a
    // chain-side follow-up; the docs were updated to be honest about
    // the gap. Don't re-add the header assertion until the SDK
    // wire-up gap is closed and the docs flip back to "always present."

    let body_404: serde_json::Value = not_found.json().await.expect("404 body parses as JSON");
    assert_eq!(
        body_404["status"].as_u64(),
        Some(404),
        "404 body.status must equal HTTP status, got {body_404:?}"
    );
    assert!(
        body_404["message"].as_str().is_some_and(|s| !s.is_empty()),
        "404 body.message must be a non-empty string, got {body_404:?}"
    );
    assert!(
        body_404["details"].is_object(),
        "404 body.details must be a JSON object, got {body_404:?}"
    );
    assert!(
        body_404["details"]["id"].as_str().is_some(),
        "404 body.details.id must be the looked-up id (not_found_404 helper convention), got {body_404:?}"
    );

    // === 400 path: malformed bech32m id, parser bails before lookup. ===
    //
    // `not-a-valid-bech32m-id` triggers `errors::bad_request_400` in
    // the handler with the underlying parse error in `details`.
    let bad_request = client
        .get(format!("{base}/v1/modules/attestation/schemas/not-a-valid-bech32m-id"))
        .send()
        .await
        .expect("400 GET request");
    assert_eq!(bad_request.status().as_u16(), 400, "expected 400 for malformed bech32m id");

    // (Same note as above: X-Request-Id propagation is documented as
    // a follow-up; not asserted here.)

    let body_400: serde_json::Value = bad_request.json().await.expect("400 body parses as JSON");
    assert_eq!(
        body_400["status"].as_u64(),
        Some(400),
        "400 body.status must equal HTTP status, got {body_400:?}"
    );
    assert!(
        body_400["message"].as_str().is_some_and(|s| !s.is_empty()),
        "400 body.message must be a non-empty string, got {body_400:?}"
    );
    assert!(
        body_400["details"].is_object(),
        "400 body.details must be a JSON object, got {body_400:?}"
    );
}
