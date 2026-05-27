# Runbook, Operator Wallet Key Rotation

**Status:** v0, covers the procedure for the operator wallet key under two backing stores: the current file + GCP Secret Manager setup (devnet), and the GCP Cloud KMS setup specced in [`key-management.md`](../../protocol/key-management.md) (pre-mainnet target).

This page covers rotating the **operator wallet key**, the bech32m address that holds the treasury balance assigned in genesis and signs operator-level transactions (faucet drips, sequencer registration, fee-routing config). It is distinct from:

- The chain-side ed25519 signing key (batch authorisation), see [`sequencer-key-rotation.md`](./sequencer-key-rotation.md)
- The Celestia DA signer (blob submission), see [`da-signer-rotation.md`](./da-signer-rotation.md)

In our devnet-2 deployment all three may resolve to the same physical operator, but they are separate keys with separate rotation procedures. Read all three before any rotation event.

**Tracks:** [chain#514](https://github.com/ligate-io/ligate-chain/issues/514)

---

## When does the operator wallet key need rotation?

| Cause | Severity | What it blocks |
|---|---|---|
| **Compromise** (private key leaked / exfiltrated) | Critical | Adversary drains the treasury, submits drip txs from the faucet pool, can register/de-register sequencer slots if the operator role overlaps that authority. Rotate immediately. |
| **Loss** (operator can't recover the key file, KMS access revoked) | High | We can't sign operator-level txs. Treasury is frozen until rotation. |
| **Pre-mainnet hardening migration** | Planned | Move from file-backed to KMS-backed during the dry run before mainnet. |
| **Routine policy** | Low | Periodic rotation per operator policy (e.g. annual). |

The first three are forcing functions; the fourth is a planning exercise.

---

## Why this rotation is heavier than the DA rotation

The operator key is treasury-balance-holding in genesis. The chain's `genesis/accounts.json` (or equivalent in the active devnet path) assigns the initial balance to a specific bech32m address. Changing that address means:

1. **Genesis changes.** The new bech32m goes into `accounts.json` in place of the old one. The chain hash recomputes.
2. **Chain id bump.** Because the chain hash is pinned via `chain_hash_pin.rs`, the deterministic chain identity changes. We bump the chain id ladder (e.g. `ligate-devnet-2` → `ligate-devnet-3`).
3. **State is invalidated.** Existing nodes can't keep their RocksDB; every full node re-syncs from genesis.
4. **Every downstream service re-points.** `ligate-cli`, `ligate-api`, `ligate-mneme`, the explorer all have env vars or config files that reference operator addresses or the chain id; each needs updating.

This is closer in shape to [`chain-id-bump.md`](./chain-id-bump.md) than to [`da-signer-rotation.md`](./da-signer-rotation.md), and the procedure below leans on the chain-id-bump runbook for the genesis steps.

---

## Pre-rotation checklist (do this every time)

- [ ] Read this page in full.
- [ ] Confirm which key is being rotated (operator wallet, NOT sequencer signing, NOT DA signer).
- [ ] Identify which downstream services reference the current operator address. Inventory: chain genesis, `rollup.toml`, `ligate-api` env vars, `ligate-cli` config, faucet bot config, monitoring dashboards.
- [ ] Decide which Procedure (A, B, or C) applies.
- [ ] Snapshot current state for rollback: RocksDB tarball, genesis files, config files, KMS key versions. Run the snapshot procedure from [`backup-restore.md`](./backup-restore.md).
- [ ] Schedule a maintenance window (Procedure A only).
- [ ] Pre-stage the replacement key (under operator control, not yet in genesis).

---

## Procedure A, planned rotation, file → file (devnet)

Use case: routine rotation on devnet while we still use the file-backed setup. Acceptable when state is disposable (devnet is allowed to bump chain id).

1. **Generate the replacement key.**
   ```sh
   ligate-cli keys generate --output ~/.ligate-keys/devnet-3/operator.key
   # Records mnemonic, store separately from the node filesystem.
   ```

2. **Record the new bech32m address.**
   ```sh
   ligate-cli keys show ~/.ligate-keys/devnet-3/operator.key
   # → lid1...
   ```

3. **Bump the chain id ladder.** Follow [`chain-id-bump.md`](./chain-id-bump.md) for the full procedure. Key steps:
   - Update `devnet/genesis/accounts.json`: replace the old operator bech32m with the new one in the treasury-balance line.
   - Update `devnet/genesis/sequencer_registry.json` if the sequencer's `bond_address` is the operator key.
   - Update `devnet/rollup.toml` `[sequencer].rollup_address` if applicable.
   - Bump `chain_id` from `ligate-devnet-N` to `ligate-devnet-(N+1)`.
   - Recompute `CHAIN_HASH` (auto on next build).

4. **Update downstream services in parallel** (before restart):
   - `ligate-api` (Railway env vars): `LIGATE_OPERATOR_ADDRESS`, `CHAIN_ID`. See `~/.config/ligate/secrets.env` for Railway tokens.
   - `ligate-cli` (config): operator address in any `~/.ligate-cli/config.toml`.
   - `faucet-bot` (Railway env vars): operator address, if it submits drips from this key.
   - Monitoring dashboards (Grafana): any panels filtering by operator address need the new bech32m.
   - Explorer (`ligate-explorer`): hardcoded operator address constants in `ENV` or `wrangler.toml`.

5. **TRUNCATE indexer Postgres** on `ligate-devnet-N-api` Railway service. Stale rows from the old chain id will cause merge conflicts when the new chain replays. Procedure: see [`backup-restore.md`](./backup-restore.md) §"Indexer state reset across chain id bump."

6. **Re-genesis the sequencer.**
   ```sh
   systemctl stop ligate-node
   rm -rf /var/lib/ligate/devnet-N/data/*
   # Deploy new genesis + new key file (mode 0600)
   systemctl start ligate-node
   ```

7. **Verify.**
   ```sh
   ligate-cli info             # should report new chain id
   ligate-cli balance lid1...  # new operator address holds treasury balance
   curl -s https://rpc.ligate.io/v1/rollup/info | jq .chain_id
   ```

8. **Revoke the old key.** Cryptographically you cannot un-sign historical state; the old key remains valid in the old genesis. What you do:
   - Delete the file from laptop + Secret Manager + Keychain.
   - Document the rotation event with timestamp + reason in `docs/development/runbooks/key-rotation-log.md` (create on first event).
   - If the rotation was a compromise-driven event, publish a public attestation event so external observers know the old key is retired.

**Estimated downtime:** ~30 minutes (genesis edit + downstream config updates + restart + verification). Schedule a maintenance window.

---

## Procedure B, emergency rotation under compromise

Adversary has the operator private key. Time is the loss surface; every additional minute the adversary can sign treasury draws.

1. **Drain the treasury to a cold address** (NOT the new operator key, a separate offline-only address). The adversary will try the same move; race them.
   ```sh
   ligate-cli send --from operator --to lid1...cold --amount <treasury - fees>
   ```
   If the adversary already drained, skip to step 2 and accept the loss; this is now a post-mortem exercise.

2. **Pre-staged key on hand?** If yes, swap directly. If no, generate per Procedure A step 1 as fast as possible.

3. **Run Procedure A steps 3-7** with no maintenance-window niceties. Bump the chain id, update genesis, restart. Every minute the old chain id is live, the adversary's key still works on it.

4. **Public attestation event.** Post the rotation to the chain status page, the @ligate_io X account, and the official channels. Coordinate with partners (Mneme, anchor-buyer LOI holders) before they discover it themselves.

5. **Post-mortem within 24 hours.** How did the key leak? Filesystem? Backup tarball? Logged crash dump? Whatever the gap was, tighten it before mainnet. This is exactly why we want KMS-backed keys before mainnet, so a single-laptop compromise is not a single-key compromise.

The compromise window, from leak to chain-id-bump, is what determines actual loss. Pre-staging a replacement key and practicing this procedure on devnet keeps the window minutes, not hours.

---

## Procedure C, planned rotation, file → KMS (the pre-mainnet dry run)

Use case: the migration described in [`key-management.md`](../../protocol/key-management.md) §6 step 3. This is the test of the KMS-backed path on devnet before mainnet ships.

1. **Stand up `ligate-signer-proxy`.** Follow the deployment notes that will land in the new `crates/signer-proxy/README.md`. Service runs as a systemd unit on the sequencer VM.

2. **Create the KMS keyring and operator key.**
   ```sh
   # One-time, done via gcloud (operator running with appropriate IAM):
   gcloud kms keyrings create ligate-devnet-keys \
     --location=us-central1
   gcloud kms keys create operator-v1 \
     --location=us-central1 \
     --keyring=ligate-devnet-keys \
     --purpose=asymmetric-signing \
     --default-algorithm=ec-sign-ed25519 \
     --protection-level=hsm
   ```

3. **Export the new public key + derive the bech32m address.**
   ```sh
   gcloud kms keys versions get-public-key 1 \
     --key=operator-v1 \
     --keyring=ligate-devnet-keys \
     --location=us-central1 \
     --output-file=operator-v1.pub
   ligate-cli keys import-public --pub-file operator-v1.pub
   # → derives lid1...
   ```

4. **Bind the signer-proxy service account to the key.**
   ```sh
   gcloud kms keys add-iam-policy-binding operator-v1 \
     --location=us-central1 \
     --keyring=ligate-devnet-keys \
     --member="serviceAccount:ligate-signer@<project>.iam.gserviceaccount.com" \
     --role=roles/cloudkms.signer
   ```

5. **Run Procedure A steps 3-5** to bump the chain id ladder with the new bech32m. The genesis edits are identical; only the backing store of the private key differs.

6. **Switch `rollup.toml` to proxy mode.**
   ```toml
   [signer]
   mode = "proxy"
   socket_path = "/run/ligate-signer-proxy.sock"
   ```

7. **Start the proxy + the sequencer.** Confirm the chain signs its first batch successfully via the proxy. Check audit logs in Cloud Logging:
   ```
   resource.type="cloudkms_cryptokey" AND protoPayload.methodName="AsymmetricSign"
   ```

8. **Verify the steady state for at least 72 hours** before declaring the migration done. Watch:
   - Signing latency p50/p99 (target: p50 < 50ms, p99 < 150ms per `key-management.md` R2)
   - Error rate at the proxy
   - Chain block production cadence (no slot-budget overrun)

9. **Decommission the old file-backed key.** Delete from laptop + Secret Manager + Keychain. The KMS-backed key is now canonical.

---

## What chain-side observers see during rotation

- **`/v1/rollup/info`**: `chain_id` changes (e.g. `ligate-devnet-2` → `ligate-devnet-3`).
- **`/v1/ledger/slots/latest`**: resets to slot 0 of the new chain. Operators of full nodes re-sync from new genesis.
- **All historical txs from the old chain are abandoned** at the consumer-API layer. The indexer Postgres is truncated and re-fills from the new genesis. Partner-facing notice required: see Procedure A step 4.

If a partner queries the old chain id post-rotation, they get an empty result, not a redirect. The chain id is the namespace. Communication ahead of time prevents support load post-event.

---

## Verification checklist (post-rotation)

```sh
# 1. Chain identity reflects the bump
curl -s https://rpc.ligate.io/v1/rollup/info | jq .chain_id

# 2. Operator balance loaded from new genesis
ligate-cli balance lid1...  # the new operator address

# 3. Old operator address shows zero (or doesn't exist on the new chain)
ligate-cli balance lid1...old  # not found

# 4. Indexer ingestion caught up to current slot
curl -s https://api.ligate.io/v1/stats/health | jq

# 5. Faucet drip works end-to-end
curl -X POST https://api.ligate.io/v1/drip/lid1...somenewaddr

# 6. Sequencer is producing blocks at expected cadence
curl -s https://rpc.ligate.io/v1/ledger/slots/latest | jq .height
# Wait 30s, query again, height should advance.

# 7. (KMS mode only) Cloud Audit Logs show signing events
gcloud logging read 'resource.type="cloudkms_cryptokey"' --limit=5
```

If any check fails, halt and investigate before declaring the rotation complete. Rollback procedure: restore the snapshot from the pre-rotation checklist step.

---

## Open follow-ups

1. **`docs/development/runbooks/key-rotation-log.md`.** Append-only log of every rotation event (date, reason, procedure used). Create on first rotation event.
2. **Automate Procedure A steps 4-5 (downstream config updates + indexer truncate).** Currently a hand-edit across multiple repos. Filed as a follow-up to #514 once we've done one rotation by hand and know which env vars hurt most.
3. **Test Procedure C end-to-end on localnet** before running it on devnet-2. Filed as a follow-up to #514.
4. **HSM-tier signer-proxy crate** at `crates/signer-proxy`. Tracked separately per `key-management.md` §7 item 1.

---

## Cross-references

- [`key-management.md`](../../protocol/key-management.md): the RFC that picks GCP Cloud KMS and specs the signer-proxy
- [`sequencer-key-rotation.md`](./sequencer-key-rotation.md): chain-side ed25519 signing key rotation (different key, similar genesis impact)
- [`da-signer-rotation.md`](./da-signer-rotation.md): Celestia DA wallet rotation (different key, no genesis impact)
- [`chain-id-bump.md`](./chain-id-bump.md): the genesis + chain id bump procedure this runbook leans on for Procedure A steps 3-5
- [`backup-restore.md`](./backup-restore.md): snapshot + restore procedures for the rollback path
- [chain#514](https://github.com/ligate-io/ligate-chain/issues/514): tracking issue for this runbook + key-management RFC
- [chain#360](https://github.com/ligate-io/ligate-chain/issues/360): parent pre-mainnet security gap tracker
