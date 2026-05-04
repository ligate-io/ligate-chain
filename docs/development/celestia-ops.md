# Celestia operations runbook

How to run `ligate-node` against a real Celestia DA layer (Mocha testnet by default). Covers the parts of operator setup that aren't obvious from `devnet/celestia.toml` alone: light-node provisioning, auth-token handling, namespace configuration, and the verification commands that confirm a healthy chain.

Tracking issue: [#166](https://github.com/ligate-io/ligate-chain/issues/166).

## What this covers

- Single-sequencer, single-node deployment against Celestia Mocha (or a local Celestia devnet, if you have one).
- Auth, secrets, and env-var hygiene for the rollup binary.
- Health-check commands matching the metric surface shipped under [#110](https://github.com/ligate-io/ligate-chain/issues/110) (Phase 1 + Phase 2).
- Common failure modes and the triage steps that map to them.

## What this does NOT cover (yet)

- **Multi-node operation (follower mode).** v0 chain doesn't have a passive / read-only mode. Every running instance attempts to act as the registered sequencer. Two nodes against the same Celestia namespace would either fork (if both have valid signer keys) or waste blob fees (if the second has a non-registered DA address). Multi-node lands when the SDK exposes follower-mode APIs; tracked under [#166](https://github.com/ligate-io/ligate-chain/issues/166)'s scope-extension thread.
- **Production hardening** (systemd units, Docker compose, k8s manifests, alerting rules). Pre-mainnet runbook scope only.
- **Mock DA setup.** That's the default `cargo run --bin ligate-node` flow documented in [`devnet/README.md`](../../devnet/README.md). This runbook assumes you've already validated the chain against Mock DA and want to graduate to real DA.

## Prerequisites

- A working `cargo run --bin ligate-node --da-layer mock` against the genesis files in `devnet/genesis/`. If that doesn't boot cleanly, fix it first; Celestia adds operational complexity, not capability.
- A Celestia light node binary (`celestia` from celestiaorg/celestia-node) installed locally OR a remote light-node endpoint you control.
- ~5 GB of disk for the rollup state dir. RocksDB + NOMT preallocation reports much higher in `meta.len()` terms; the chain reports actual block usage via `ligate_state_db_size_bytes`.

## Step 1: Provision a Celestia light node

Mocha is Celestia's public testnet. Free, takes ~10 min to sync, fine for devnet use.

```bash
# One-time install
brew install celestiaorg/celestia/celestia-node
celestia light init --p2p.network=mocha

# Start the light node (foreground or systemd-managed)
celestia light start --core.ip=rpc-mocha.pops.one --p2p.network=mocha
```

Wait until headers catch up (`logs show "header sync finished"`). On a fast connection this is ~10 minutes.

Generate an auth token for the light node's RPC. The chain binary reads the token over the WebSocket endpoint:

```bash
celestia light auth admin --p2p.network=mocha
# emits a JWT; copy it
```

Default light-node endpoints:
- WS: `ws://127.0.0.1:26658` (read path: blobs + headers).
- gRPC: `http://127.0.0.1:9090` (write path: blob submission, sequencer only).

For Mocha public RPCs (skip the local light-node setup), known providers include `wss://celestia-mocha.public-rpc.com` and the QuickNode / Lava endpoints. They issue tokens differently; consult the provider's docs.

## Step 2: Configure secrets via env vars

`devnet/celestia.toml` ships with placeholder values for the secret fields. Override via env vars at runtime; do NOT commit real values:

```bash
export SOV_CELESTIA_RPC_URL='ws://127.0.0.1:26658'
export SOV_CELESTIA_RPC_AUTH_TOKEN="$(celestia light auth admin --p2p.network=mocha)"
export SOV_CELESTIA_SIGNER_KEY="$(pass celestia/sequencer-signer)"  # or your own secret store
```

The signer key controls a Celestia wallet that pays for blob inclusion. Fund it via the Mocha faucet ([https://faucet.celestia-mocha.com](https://faucet.celestia-mocha.com)) before submitting any blobs; ~0.1 TIA covers a few hours of devnet activity.

Every other field in `celestia.toml` is fine at its default for a Mocha-targeted devnet boot.

## Step 3: Boot the rollup

```bash
SOV_CELESTIA_RPC_URL='ws://127.0.0.1:26658' \
SOV_CELESTIA_RPC_AUTH_TOKEN="$AUTH_TOKEN" \
SOV_CELESTIA_SIGNER_KEY="$SIGNER_KEY" \
cargo run --release --bin ligate-node -- \
    --da-layer celestia \
    --rollup-config-path devnet/celestia.toml \
    --genesis-config-dir devnet/genesis \
    --metrics-bind 127.0.0.1:9100
```

First boot: ~30 seconds of state initialisation, then continuous slot production. Watch the logs for `da: submitted blob` lines (sequencer is posting to Celestia) and `attestation: validated` lines (chain accepted the blob's contents).

## Step 4: Verify the chain is alive

Three independent surfaces agree on the chain's state:

```bash
# Metrics: confirm chain activity, sync, and disk health.
curl -s http://127.0.0.1:9100/metrics | grep -E '^ligate_'

# REST: query a chain-state field that genesis seeded.
curl -s http://127.0.0.1:12346/ledger/slots/head | jq .

# Logs: tail the running process for steady-state output.
# Healthy: roughly one block per second on the mock side, but Celestia
# can take 10-12s for inclusion. Sequencer logs `da: submitted blob`
# every batch, follower-style state replay logs `slot processed`.
```

Key metrics to watch on a healthy boot:

| Metric | Healthy reading |
|---|---|
| `ligate_block_height` | Climbs at roughly Celestia's block rate (~1 every 10-12s when active). |
| `ligate_state_db_size_bytes` | Grows monotonically, plateaus during idle periods, jumps on attestation submissions. |
| `ligate_rpc_requests_total{status="200"}` | Increments on every successful query; should match dashboard scrapes plus any client-rs traffic. |
| `ligate_rpc_request_duration_seconds_count{endpoint=...}` | Total query count per route template. |
| `process_resident_memory_bytes` | Stable in the low hundreds of MB during devnet idle. Sustained growth indicates a leak. |

## Common failure modes

| Symptom | Likely cause | Triage |
|---|---|---|
| Boot panics on `light node not synced` | Light node hasn't caught up with Mocha headers yet. | Wait for `header sync finished`. Re-boot. |
| Boot panics on `failed to authenticate to celestia` | Auth token is wrong, expired, or for a different network. | Regenerate via `celestia light auth admin --p2p.network=mocha`. Check `SOV_CELESTIA_RPC_AUTH_TOKEN` matches the running light node. |
| `da: submission failed` repeating | Wallet has no TIA. | Top up via [Mocha faucet](https://faucet.celestia-mocha.com). Each blob is ~0.001 TIA. |
| `da: submission failed (blob too large)` | Batch exceeded Mocha's max-blob limit. | Lower `[sequencer].max_batch_size_bytes` in `celestia.toml`. Mocha caps around 1 MB per blob; default is conservative. |
| Block height stuck | Sequencer is healthy but DA is slow OR another sequencer (fork case) is producing on the same namespace. | Check Mocha block production at [explorer](https://mocha.celenium.io). If Celestia is fine, look for namespace collision. |
| `state_db_size` grows fast | RocksDB compaction lag during heavy writes. Watch for sustained > 1MB/sec for hours. Normal during initial backfill. | If it doesn't plateau, file an issue with the `ligate_state_db_size_bytes` graph over the affected window. |
| `rpc_requests_total{status="5xx"}` non-zero | Handler bug. Status `5xx` should be zero on a stable chain. | Pull the offending request's tracing logs (search for the path template). File an issue with the trace. |

## Stopping cleanly

```bash
# Send SIGTERM; the chain flushes RocksDB + ledger DB before exit.
kill -TERM "$(pgrep -f 'ligate-node.*celestia')"
```

Hard `kill -KILL` works but may leave RocksDB in a state that needs `--repair` on next boot. Avoid in production.

## Restoring after a Mocha reset

Celestia testnets restart periodically (mocha → mocha-2 → ...). When that happens:

1. Operator regenerates the Celestia light-node state (`rm -rf ~/.celestia-light-mocha && celestia light init ...`).
2. Operator regenerates the rollup state dir (the chain's blob namespace from the old testnet is gone).
3. Coordinated chain-id bump per [`docs/protocol/upgrades.md`](../protocol/upgrades.md): `ligate-devnet-1` → `ligate-devnet-2`.
4. Re-run the [genesis ceremony](devnet.md#4-genesis-ceremony).

This is operationally a hard fork, exactly as documented in the upgrades policy. The operator network coordinates via the `#ligate-devnet` channel (or wherever the next public release moves it).

## Cross-references

- [`devnet/celestia.toml`](../../devnet/celestia.toml): the rollup config this runbook drives.
- [`devnet/README.md`](../../devnet/README.md): local-boot recipe (mock-DA), what to validate before attempting Celestia.
- [`devnet.md`](devnet.md): full devnet operator runbook covering topology, attestor onboarding, and monitoring.
- [`docs/protocol/upgrades.md`](../protocol/upgrades.md): chain-id ladder + reset policy.
