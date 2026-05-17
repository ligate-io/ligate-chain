# Runbook — Mocha smoke gate

**Status:** v0 — covers the one-time pre-devnet-announce smoke against real Celestia Mocha. Companion to [`celestia-light-node.md`](./celestia-light-node.md), [`da-signer-rotation.md`](./da-signer-rotation.md), and the broader [`public-devnet-deploy.md`](../public-devnet-deploy.md).

Bech32m + DA-adapter changes that landed in Phase 5/6 (TmHash display, `BlobWithSender.hash` → `BlobHash`, the wallet's `TmHashSchema` attribute) have only been verified against MockDA. **First boot against Mocha is the smoke.** Find the bug here, not after public devnet announce.

This runbook is the procedure for executing that smoke. The artifact is the documented run pasted back into [#285](https://github.com/ligate-io/ligate-chain/issues/285).

---

## What this smoke verifies

| Surface | What we're checking |
|---|---|
| Chain identity | `chain_hash` round-trips as `lsch1...` against both `/v1/rollup/info` and `/v1/rollup/schema` |
| Slot encoding | Slot `hash` is `lblk1...`, `state_root` is `lsr1...`, batch hashes are `lba1...` |
| Tx round-trip | A transfer submitted via `ligate-cli` returns `ltx1...`, lands on Celestia, finalizes |
| DA blob encoding | DA blob hashes show as `lbz1...` in `journalctl -u ligate-node` |
| DA pipeline state machine | Tx transitions Published → Processed → Finalized correctly |
| DA metrics | `ligate_da_finalization_latency_seconds_bucket` increments; p99 in 12s-60s band |
| Failure handling | Killing the light node mid-submission triggers `ligate_da_submission_failures_total` with the correct `reason` label; node recovers when light node restarts |

Anything not on this list is out of scope for this smoke — covered elsewhere in the test pyramid.

---

## Prerequisites

- A non-prod VM (or local machine) per [`public-devnet-deploy.md`](../public-devnet-deploy.md). Don't run this against the production sequencer.
- A funded Mocha TIA wallet (≥10 TIA recommended for safety margin; the smoke uses fractions of a TIA but you want headroom for retries).
- The Mocha-flavoured chain config at `devnet-1/celestia.toml` with `[da].signer_private_key` pointing at the funded wallet.
- Celestia light node v0.30.2 co-located, syncing Mocha (per [`celestia-light-node.md`](./celestia-light-node.md)).
- `ligate-cli` installed locally (or available on the smoke VM).

---

## Procedure

Execute step-by-step. Paste the output of each command back into [#285](https://github.com/ligate-io/ligate-chain/issues/285) as you go (or capture to a single transcript file via `script(1)`).

### Step 1 — Boot the chain against Mocha

```sh
# On the smoke VM, with the Mocha-funded TIA wallet wired:
systemctl start celestia-light
# Wait for sync, per celestia-light-node.md
celestia rpc das samplingstats | jq '.head_of_sampled_chain'
celestia rpc header sync-state | jq '{height, network_head}'

# Then start the chain
systemctl start ligate-node
journalctl -fu ligate-node | head -50
```

**Expected:** `ligate-node` starts, opens DA-adapter session with the light node, begins producing slot 1 within ~30s. Log lines like `BlobSubmitter connected` and `slot_committed slot=1`.

### Step 2 — Chain identity round-trip

```sh
curl -s http://127.0.0.1:12346/v1/rollup/info | jq '{chain_id, chain_hash, version}'
curl -s http://127.0.0.1:12346/v1/rollup/schema | jq .chain_hash
```

**Expected:** both `chain_hash` values are byte-identical and start with `lsch1`. Cross-check against the build-time-pinned hash via:

```sh
grep CHAIN_HASH crates/stf/src/chain_hash_pin.rs
```

The pinned value should match the runtime-emitted one. If they diverge, the smoke fails — file a bug, do NOT continue.

### Step 3 — Slot encoding round-trip

```sh
# Wait for at least slot 3 so we have non-genesis batches
curl -s http://127.0.0.1:12346/v1/ledger/slots/latest | jq .number
# Once >= 3:
SLOT=3
curl -s http://127.0.0.1:12346/v1/ledger/slots/${SLOT} | jq '{hash, state_root, batch_range}'
```

**Expected:**
- `hash` starts with `lblk1` and decodes to 32 bytes
- `state_root` starts with `lsr1` and decodes to 64 bytes (NOMT root)
- `batch_range.start <= batch_range.end`

Decode-check via the bech32 helper or `ligate-cli`:

```sh
ligate decode-hash <hash-value>
# (or use the chain repo's `crates/rollup/tests/binary_spawn_smoke.rs` decode patterns)
```

### Step 4 — Transfer round-trip

```sh
# Use the localnet dev key (pre-funded with 10000 LGT at genesis, per
# devnet/local-dev-key.json) as the sender. Pick any address as recipient.
RECIPIENT="lig1u8z2rxh6ymjwkqsasme64f5kfphtfm2kf4kkn0clusfpr34amezsp5j7yp"

ligate --rpc http://127.0.0.1:12346 transfer \
    --to "$RECIPIENT" \
    --amount 1 \
    --token-id "token_1nyl0e0yweragfsatygt24zmd8jrr2vqtvdfptzjhxkguz2xxx3vs0y07u7" \
    --from <path-to-dev-key>
```

**Expected:** CLI returns a tx hash like `ltx1...`, NOT a hex string.

Pull the tx back:

```sh
TX_HASH="<the ltx1... value from above>"
curl -s "http://127.0.0.1:12346/v1/ledger/txs/${TX_HASH}" | jq '.hash, .batch_number'
```

**Expected:** `hash` field matches the submitted `ltx1...` exactly. The byte-form decode matches.

### Step 5 — Tx state-machine progression

Watch the tx transition through DA states. The chain's `sov-sequencer` emits state-change events visible in the journal:

```sh
journalctl -u ligate-node --since "-2 minutes" | grep -E "${TX_HASH}|tx_status_changed"
```

**Expected:** see `tx_status_changed` events with the sequence `Published` → `Processed` → `Finalized` for the smoke tx. Finalization may take 1-2 minutes depending on Mocha's block cadence.

### Step 6 — DA blob hash encoding

```sh
journalctl -u ligate-node --since "-5 minutes" | grep -E "lbz1|blob_hash"
```

**Expected:** DA blob hashes appear in logs as `lbz1...`, NOT as `0x...`. Captures any leaked hex display surfaces.

### Step 7 — DA metrics

```sh
curl -s http://127.0.0.1:9100/metrics \
    | grep -E "ligate_da_(finalization_latency|submission_failures)"
```

**Expected:**
- `ligate_da_finalization_latency_seconds_count > 0`
- `histogram_quantile(0.99, ligate_da_finalization_latency_seconds_bucket)` between 12s and 60s on healthy Mocha
- `ligate_da_submission_failures_total{reason=...}` either absent (no failures) or only `reason="submission_timeout"` for the very first blob (cold-start retry can hit timeout once)

### Step 8 — Forced failure injection

The chain should handle light-node death gracefully — backoff + retry, no panic.

```sh
# Submit a tx, then kill the light node WHILE it's in flight
ligate --rpc http://127.0.0.1:12346 transfer \
    --to "$RECIPIENT" --amount 1 --token-id "$TOKEN_ID" --from <key> &
TX_PID=$!
sleep 1  # let the submit start
sudo systemctl stop celestia-light
wait $TX_PID

# Check what failure mode the chain saw
curl -s http://127.0.0.1:9100/metrics \
    | grep ligate_da_submission_failures_total

# Bring the light node back
sudo systemctl start celestia-light

# Watch the chain recover
journalctl -fu ligate-node | grep -E "blob_submission|da_recover" | head -20
```

**Expected:**
- `ligate_da_submission_failures_total{reason="light_node_unreachable"}` or `{reason="submission_timeout"}` increments by ≥1
- After restarting the light node, blob submission resumes without operator intervention
- Block height eventually advances past the pre-kill height

If the chain crashes or wedges instead of retrying, the smoke fails — file a bug.

### Step 9 — Capture + paste

```sh
# Capture the final-state snapshot of all the relevant surfaces
{
    echo "=== chain_hash round-trip ==="
    curl -s http://127.0.0.1:12346/v1/rollup/info | jq
    echo "=== latest slot ==="
    curl -s http://127.0.0.1:12346/v1/ledger/slots/latest | jq
    echo "=== smoke tx ==="
    curl -s "http://127.0.0.1:12346/v1/ledger/txs/${TX_HASH}" | jq
    echo "=== metrics ==="
    curl -s http://127.0.0.1:9100/metrics | grep -E "ligate_(block_height|da_)"
} > /tmp/mocha-smoke-$(date -u +%Y%m%dT%H%M%SZ).log

# Paste the contents into issue #285's results comment.
```

---

## Pass/fail criteria

The smoke passes if every step above produced the expected output without bugs or unexplained surfaces. Specifically:

- ✅ All hash types display in bech32m (no leaked `0x...` hex anywhere)
- ✅ Tx round-trips byte-identical across REST endpoints
- ✅ DA pipeline transitions through all three states for the smoke tx
- ✅ Forced light-node failure surfaces a typed reason, not a crash
- ✅ Recovery from light-node failure is automatic

Any bug surfaces a follow-up issue. File before announce.

---

## Recurring smoke (CI-driven, live)

The same procedure runs **nightly at 06:00 UTC** via [`.github/workflows/mocha-smoke.yml`](../../.github/workflows/mocha-smoke.yml), plus on-demand via `workflow_dispatch`. Status badge at the top of the chain README.

Wiring:

- `MOCHA_TIA_SIGNER_KEY` (repo secret): operator's funded TIA wallet hex private key.
- `MOCHA_TIA_DA_ADDRESS` (repo variable): bech32 `celestia1...` form of the same wallet, used to substitute the placeholder DA address in the ephemeral genesis the workflow generates.
- `MOCHA_GRPC_URL` (repo variable, optional): Celestia consensus gRPC endpoint for blob submission. Defaults to a public Mocha endpoint; override to switch providers.
- `MOCHA_SMOKE_ENABLED` (repo variable): `true` to enable. Set to anything else (or unset) to kill all fires of the workflow without a PR. This is the kill-switch if Mocha or the pops.one bridge has a known bad day.

Workflow flow:

1. Build `ligate-node` + `ligate-genesis-tool` (cold cache ~50 min; warm ~10-15).
2. Install celestia-node v0.30.2, start it against `${MOCHA_BRIDGE}` (default `rpc-mocha.pops.one`).
3. Generate ephemeral keys (operator + demo1 + demo2), substitute all four placeholder addresses (3 `lig1` + 1 `celestia1`), run `ligate-genesis-tool generate --da celestia` to produce + verify a valid genesis, then anchor `genesis_da_height` to a live Mocha block.
4. Boot `ligate-node --da-layer celestia` against the ephemeral genesis.
5. Run steps 1-7 of the manual procedure (chain identity, slot encoding, DA metrics) against the running node.

The CI smoke is a subset of the manual procedure (steps 1-7; step 8 forced-failure injection is harder to do reliably in CI without flake risk).

On failure, the workflow uploads `/tmp/ligate-node.log` and `/tmp/celestia.log` as artifacts (`mocha-smoke-logs`, 14-day retention) for triage.

---

## Out of scope

- **Multi-sequencer topology.** v0 is single-sequencer; multi covered post-#82.
- **Hash-encoding micro-tests.** Already covered by `crates/rollup-interface/src/common/bech32m_encoded.rs` unit tests (and the SDK fork's). This smoke is the *integrated* check, not the type-level one.
- **Performance benchmarking.** Mocha latency varies; we record p99 but don't gate on a SLO.

---

## Cross-references

- [Issue #285](https://github.com/ligate-io/ligate-chain/issues/285) — paste the smoke results here
- [`celestia-light-node.md`](./celestia-light-node.md) — light node failure modes referenced in step 8
- [`da-signer-rotation.md`](./da-signer-rotation.md) — TIA wallet management
- [`public-devnet-deploy.md`](../public-devnet-deploy.md) — VM bring-up the smoke runs against
- [`.github/workflows/mocha-smoke.yml`](../../../.github/workflows/mocha-smoke.yml) — recurring CI version
