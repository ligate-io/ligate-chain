# Runbook — Celestia light node health + recovery

**Status:** v0 — operational coverage for the co-located Celestia light node that `ligate-node` reads from for DA submissions. Companion to [`da-signer-rotation.md`](./da-signer-rotation.md), which covers the Celestia-side signing key separately.

`ligate-node` talks to a local light node at `ws://127.0.0.1:26658`. The light node's job is to track Mocha (or eventual mainnet) headers, accept blob-submission RPC calls from `ligate-node`, and forward those to a Celestia bridge node. If the light node falls behind, crashes, or loses network to the bridge, blob submissions back up and the chain stalls.

This runbook covers: what to monitor, how to verify sync state by hand, and what to do when it goes wrong.

---

## What the light node does in our stack

| Path | Direction | What flows |
|---|---|---|
| `ligate-node` → light node | RPC | `BlobSubmit` calls when sealing a batch; reads of header data when verifying inclusion |
| Light node → bridge node | P2P | Header sync; blob queries; the upstream the light node trusts |
| Bridge node → Celestia network | P2P | Real consensus; the light node is a downstream consumer |

Three single points of failure stack here: the bridge node ([`celestia-ops.md`](../celestia-ops.md) for which bridges we trust), the light node, and the network between them. This runbook addresses #2.

---

## Health checks

### By-hand verification

The Celestia light node has its own RPC. Three queries answer "is it healthy":

```sh
# 1. Sampling stats — is data-availability sampling making progress?
celestia rpc das samplingstats
# Expected: head_of_sampled_chain advancing; concurrency > 0
#
# If head_of_sampled_chain is stuck for >2 minutes, the light node is
# wedged on sampling and won't accept new submissions cleanly.

# 2. Network head — what header height does our light node have?
celestia rpc header network-head | jq .height
# Expected: within ~10 blocks of Mocha's current tip. Cross-check against
# https://mocha.celenium.io/blocks for ground truth.

# 3. Local sync state — has the light node caught up to its network head?
celestia rpc header sync-state | jq '.height,.from_height,.to_height'
# Expected: height == network-head (within a few blocks); to_height
# matches the network head.
```

A light node where all three are healthy is producing valid `BlobSubmit` responses. A light node where any one is wedged blocks `ligate-node`.

### Indicators of trouble

| Symptom | What's happening | Action |
|---|---|---|
| `samplingstats.head_of_sampled_chain` stuck for >2min | Light node sampling stalled (often a bridge-side disconnect) | Restart light node ([Recovery A](#recovery-a-restart-the-light-node)) |
| `header.network-head` more than 20 blocks behind Mocha tip (via Celenium) | Light node lost contact with the bridge | Verify bridge connectivity ([Recovery B](#recovery-b-bridge-node-connectivity)) |
| `header.sync-state.height` < `network-head` by >50 blocks | Light node has the header tip but isn't sampling fast enough | Usually transient; if persistent, [Recovery A](#recovery-a-restart-the-light-node) |
| `ligate-node` logs "blob submission timeout" for ≥3 consecutive submissions | Light node is unhealthy regardless of which RPC says what | [Recovery A](#recovery-a-restart-the-light-node) |
| Light node process is dead (no PID) | Crashed or OOM-killed | Check `journalctl -u celestia-light` for the cause, then start fresh ([Recovery A](#recovery-a-restart-the-light-node)) |

---

## Prometheus monitoring

The Celestia light node exposes its own metrics at `127.0.0.1:9090/metrics` (configurable via `--metrics.endpoint`). Probe these:

```yaml
# prometheus.yml — scrape job for the light node
- job_name: 'celestia-light'
  static_configs:
    - targets: ['127.0.0.1:9090']
  scrape_interval: 15s
```

Relevant metrics:

| Metric | Type | What it means |
|---|---|---|
| `header_synced` | gauge | 1 if local sync state equals network head; 0 otherwise |
| `header_height` | gauge | Last header the light node has accepted from the network |
| `das_head_of_sampled_chain` | gauge | The DAS-side counterpart to `header_height`. Should track within ~10 blocks |
| `das_network_head` | gauge | Network-wide head as seen by sampling |
| `header_subjective_head` | gauge | What the light node's subjective consensus accepted; lags `network_head` by a small amount in healthy state |

Alertmanager rules (track in [#268](https://github.com/ligate-io/ligate-chain/issues/268), draft below):

```yaml
- alert: CelestiaLightNodeUnsynced
  expr: header_synced == 0
  for: 3m
  annotations:
    runbook: docs/development/runbooks/celestia-light-node.md
    summary: Celestia light node sync gauge dropped to 0

- alert: CelestiaLightNodeStaleHeader
  expr: (time() - header_height_timestamp_seconds) > 60
  for: 2m
  annotations:
    runbook: docs/development/runbooks/celestia-light-node.md
    summary: Light node header hasn't advanced in 60s+ (network blip or wedge)

- alert: CelestiaSamplingFarBehindHeaders
  expr: (header_height - das_head_of_sampled_chain) > 50
  for: 5m
  annotations:
    runbook: docs/development/runbooks/celestia-light-node.md
    summary: Sampling lagging headers by >50 blocks
```

These rules don't ship today (`header_height_timestamp_seconds` is illustrative; the real metric naming may differ — verify with `curl http://127.0.0.1:9090/metrics | grep header_`). The acceptance test is "trigger the alert by killing the light node, confirm Alertmanager fires within 3 min".

Also wire a basic TCP probe from the `ligate-node` host to the light node:

```yaml
- alert: CelestiaLightNodeUnreachable
  expr: probe_success{job="celestia-light-tcp"} == 0
  for: 1m
```

Uses `blackbox_exporter` doing a TCP connect to `127.0.0.1:26658` (light node's RPC port). Catches "process exited, port is closed" before the RPC-level alerts.

---

## Recovery procedures

### Recovery A — Restart the light node

The common case. The light node has crashed, OOMed, or wedged on a network blip.

```sh
# 1. Capture state before restart (forensics)
journalctl -u celestia-light --since "-15 minutes" > /tmp/celestia-light-$(date +%s).log
celestia rpc das samplingstats > /tmp/celestia-samplingstats-$(date +%s).json 2>&1 || true

# 2. Restart
systemctl restart celestia-light

# 3. Wait for sync (typically 30-90s on a hot disk)
until celestia rpc header sync-state >/dev/null 2>&1; do
  sleep 2
done
echo "light node accepting RPC"

# 4. Verify sync state caught up
celestia rpc header sync-state | jq .height
# Compare against https://mocha.celenium.io/blocks
```

`ligate-node` retries blob submissions on its own backoff (per `sov-blob-sender`); no need to touch the chain process unless the light node is down for >10 minutes, in which case the chain's blob queue may have backed up enough to need a sequencer restart too.

### Recovery B — Bridge node connectivity

If the light node is healthy but has no peers, `samplingstats` shows zero concurrency and `network_head` stalls.

```sh
# 1. Check current peers
celestia rpc p2p peers | jq '.peers | length'
# Healthy: 4+ peers. Zero or one: bridge connectivity broken.

# 2. Check the trusted-bridge config in the light node's startup args
systemctl cat celestia-light | grep -E "trusted-hash|core.ip|p2p.network"

# 3. List current trusted hash + bridge endpoint
# Default Mocha bridge: rpc-mocha.pops.one. If that's unreachable:
# - From the VM, `dig rpc-mocha.pops.one` should resolve.
# - `curl https://rpc-mocha.pops.one` should return *something* (even a 404 is fine; tests that TCP+TLS works).

# 4. If the configured bridge is down, swap to a backup. Edit the
# systemd unit's ExecStart to use a different bridge:
#   --core.ip rpc-mocha-2.pops.one
# (verify the bridge from celestia docs first)
systemctl daemon-reload
systemctl restart celestia-light
```

The light node config should list multiple bridges so a single bridge outage doesn't take us down. Today only one is configured — adding a backup is an [open follow-up](#open-follow-ups).

### Recovery C — Full re-sync (worst case)

If the light node's local store is corrupt (rare; happens after a disk-full event or an unclean shutdown mid-write):

```sh
# 1. Stop the light node
systemctl stop celestia-light

# 2. Wipe local state
rm -rf /var/lib/celestia-light/data
# Config + keys live elsewhere; this is just the header / sampling store.

# 3. Re-init from a known checkpoint (the older the checkpoint, the
# longer the re-sync). Default Mocha checkpoint:
celestia light init --p2p.network mocha

# 4. Start, wait, verify
systemctl start celestia-light
# Cold re-sync from Mocha genesis takes ~10-30 minutes on a normal
# VM; light nodes only download header chains, not full blocks.
```

During Recovery C, `ligate-node` blob submissions fail. The chain stalls. Plan: communicate to operators via the status banner ("DA layer maintenance in progress, devnet paused").

---

## How `ligate-node` behaves during light-node trouble

`sov-blob-sender` (in the SDK fork's `crates/full-node/sov-blob-sender/`) is the queue between `ligate-node` and the Celestia adapter:

- **Blob submission timeout** → `BlobSubmissionError::Timeout`. Retries with exponential backoff up to ~10 attempts. Sequencer continues building batches; they queue locally.
- **Light node returns "not synced" error** → `BlobSubmissionError::DaNotReady`. Same retry behaviour.
- **Light node unreachable (TCP)** → `BlobSubmissionError::ConnectError`. Same retry behaviour.
- **Light node down for >10 minutes** → queue may exceed `sov-blob-sender`'s capacity (default 1000 pending blobs). Past that, `ligate-node` halts batch sealing and surfaces `WaitingOnDa` to RPC clients (which clients of `/v1/sequencer/txs` retry on). No tx data is lost.

Key invariant: `ligate-node` never drops batches under DA pressure. It buffers, retries, and surfaces backpressure to clients. The recovery is "fix the light node, watch the queue drain."

---

## Backup peers / redundancy (out of scope)

This runbook covers single-light-node operation. Running redundant light nodes (bridge-pair, follower fallback) is post-devnet hardening. Two paths once we're ready:

1. **Bridge-pair**: two light nodes pointing at different Mocha bridges. `ligate-node` round-robins, falls over on TCP failure. Trade-off: 2x cost, 2x state to manage.
2. **DA-service**: contract with a third party who runs the light node + bridge stack for us. Trade-off: external dependency, ongoing cost. Common for ecosystem tooling that doesn't want to operate their own DA infra.

Both deferred until we have devnet operational data on how often the single-light-node setup fails.

---

## Open follow-ups

1. **Alertmanager wiring** — the rules above are draft; verify metric names against `curl localhost:9090/metrics`, refine, land alongside [#268](https://github.com/ligate-io/ligate-chain/issues/268).
2. **Backup bridge config** — add a second bridge to the light node's startup args so we have automatic failover. Default Mocha bridges to consider: `rpc-mocha.pops.one`, `rpc-mocha-4.consensus.celestia-mocha.com`.
3. **Live drill** — kill the light node in production-like conditions, time the recovery, feed the result back into this runbook. Ideally before public devnet announce.

---

## Cross-references

- [`celestia-ops.md`](../celestia-ops.md) — broader Celestia ops (bridge selection, TIA management, namespace).
- [`da-signer-rotation.md`](./da-signer-rotation.md) — Celestia-side signer key rotation; separate concern from light node health.
- [`public-devnet-deploy.md`](../public-devnet-deploy.md) — bring-up runbook the alert rules above are intended to fold into.
- [Issue #268](https://github.com/ligate-io/ligate-chain/issues/268) — Alertmanager wiring (where these rules eventually land).
- [Issue #267](https://github.com/ligate-io/ligate-chain/issues/267) — backup/restore for `ligate-node` RocksDB (sibling concern: light node also has its own state, but it's regenerable from the network, so backup pressure is lower).
