# Runbook — Sequencer Key Rotation

**Status:** v0 — documents the procedure but warns where on-chain rotation primitives are missing today. Pre-devnet rotation is a re-genesis event; post-devnet rotation needs the upgrade module ([#42](https://github.com/ligate-io/ligate-chain/issues/42)) plus a `RotateSequencerKey` runtime call ([this runbook's open follow-up](#open-follow-ups)).

This page covers rotating the **sequencer's chain-side signing key** — the ed25519 keypair that authorises sequencer-built batches. NOT the DA-side key that signs Celestia blob submissions; that's [`da-signer-rotation.md`](./da-signer-rotation.md).

---

## When does the chain-side sequencer key need rotation?

1. **Compromise.** The private key file leaks, gets exfiltrated, ends up in a logged crash dump. **Severity: critical.** Adversary can forge batches; rotate immediately and re-genesis if any pre-rotation batch is suspect.
2. **Lost key, recoverable state.** Operator machine dies but ledger / RocksDB lives. Key file is gone but on-chain state is intact. Need a way to swap the registered sequencer pubkey while keeping every existing slot.
3. **Routine rotation policy.** Some operator policies mandate periodic key rotation (e.g. every 12 months). Pre-mainnet, this is a planning decision more than a forcing function.

Cases 1 + 2 are operationally urgent. Case 3 is a planning exercise.

---

## What rotation means at the chain level

The sequencer pubkey lives in `crates/sequencer-registry` / `genesis/sequencer_registry.json` as the on-chain authorised signer. Three different things could change:

| Field | Where stored | Rotation primitive needed |
|---|---|---|
| `sequencer.rollup_address` (lig1...) | sequencer-registry state | `RotateSequencerAddress` runtime call (NOT shipped — [open follow-up](#open-follow-ups)) |
| `sequencer.signer_private_key` | Operator's filesystem (NOT on-chain) | Filesystem operation, but only valid if `rollup_address` matches the on-chain entry |
| `seq_da_address` | sequencer-registry + DA layer | Concert with DA-side rotation ([`da-signer-rotation.md`](./da-signer-rotation.md)) |

The Sovereign SDK does **not** ship a hot-rotation primitive in v0. Until [#42 lands](https://github.com/ligate-io/ligate-chain/issues/42) + a sequencer-side rotation call lands on top, rotation is either:

- **Re-genesis** (chain id ladder bumps, e.g. `ligate-devnet-1` → `ligate-devnet-2`), OR
- **Coordinated restart with operator-side state surgery**, which we don't endorse for any state we want to keep.

---

## Procedure A — pre-devnet / localnet (re-genesis)

Acceptable when state is disposable (devnet still bootstrapping, no partner data on the chain).

1. **Stop the node.**
   ```sh
   pkill -f ligate-node
   ```

2. **Wipe state.**
   ```sh
   rm -rf devnet/data/*
   ```

3. **Generate the new key.**
   ```sh
   ligate-genesis-tool keys generate --roles sequencer --output devnet/genesis/keys/
   # writes sequencer.key (mode 0600) + sequencer.address
   ```

4. **Update genesis.**
   - `devnet/genesis/sequencer_registry.json`: replace `address` with the new bech32m.
   - `devnet/rollup.toml`: update `[sequencer].rollup_address` to match.
   - Bump the chain id if state surgery would have leaked old data: `chain_id = "ligate-devnet-2"`.

5. **Recompute `CHAIN_HASH`** (auto on next build — the chain pins it via `chain_hash_pin.rs`).

6. **Reboot.** Every operator and partner re-onboards against the new genesis.

This procedure loses every existing tx, attestor set, schema, and attestation. Don't run it after partners have started using the chain.

---

## Procedure B — post-devnet (designed; not yet shipped)

Once [#42 (upgrade module)](https://github.com/ligate-io/ligate-chain/issues/42) and a sequencer-rotation call ship, the flow becomes:

1. **Generate the replacement key** offline.
2. **Sign a `RotateSequencerKey { new_pubkey, effective_at_slot }` tx** with the OLD key. Submit during normal sequencer operation.
3. **Monitor inclusion** via `/v1/ledger/txs/{hash}`. The chain accepts the rotation tx, records the new pubkey, and schedules the cutover at `effective_at_slot`.
4. **Race-condition guard.** Between submission and `effective_at_slot`, the sequencer keeps signing with the OLD key. Any in-flight batches confirm cleanly. After `effective_at_slot`, batches signed with the old key are rejected.
5. **Swap the key on the operator filesystem** before `effective_at_slot` lands. If you don't, batch production halts at the cutover slot.
6. **Verify** via `/v1/rollup/info` and `/v1/modules/sequencer-registry/sequencers/{addr}` that the new pubkey is live.

This procedure is the desired Day-1 mainnet behaviour. **Not implemented yet.**

---

## Procedure C — recovery from lost key, state intact (operator-side surgery)

Pre-devnet only. Use case: operator machine died, RocksDB exists on backup, key file is gone.

1. **Stop the node.** (It can't sign anything anyway.)
2. **Generate a fresh key.** Same as Procedure A step 3.
3. **Edit `devnet/genesis/sequencer_registry.json`** to swap in the new address.
4. **Recompute `CHAIN_HASH`** — this WILL change because the registered sequencer pubkey is part of the hash.
5. **Re-genesis.** Even though the state directory exists, the chain hash mismatch makes it invalid. Wipe + rebuild from snapshot, see [#273](https://github.com/ligate-io/ligate-chain/issues/273) (state snapshot publishing) once that ships.

In short: pre-#273, recovery from lost key without re-genesis is impossible. After #273, the operator can rebuild state from a published snapshot while swapping in a fresh key via the same upgrade-module path as Procedure B.

---

## Attester-side view

Attestors don't see the sequencer's chain-side key directly. They see batches arrive with valid signatures (or not). When rotation happens:

- **Procedure A** — attestors re-onboard against the new genesis. Their existing attestor-set memberships need re-registration if the chain id bumped (the attestor set's deterministic id depends on member pubkeys, which don't change — so the same `las1...` id should re-derive on the new chain, but the registration TX has to be re-submitted).
- **Procedure B** — transparent. The chain emits an `Attestation/SequencerRotated` event ([open follow-up — needs filing](#open-follow-ups)) at `effective_at_slot`; attestors that depend on knowing the sequencer's identity (paranoid operators, monitoring dashboards) listen for that.

---

## Verification after rotation

```sh
# Chain identity should reflect the new chain_hash if you re-genesised
ligate info

# Sequencer registration entry on-chain
curl -s https://rpc.ligate.io/v1/modules/sequencer-registry/sequencers/$ADDR | jq

# Recent batches should be signed by the new pubkey — verify via the
# batch detail endpoint
curl -s https://rpc.ligate.io/v1/ledger/batches/latest | jq .signer_pubkey
```

---

## Open follow-ups

This runbook surfaces several gaps the chain still has to ship:

1. **`RotateSequencerKey` runtime call.** Needs to live alongside `RegisterSequencer` in `sequencer-registry`. Bundles `(old_pubkey, new_pubkey, effective_at_slot)`, signed by the old key. Not yet filed; file as a follow-up to [#42](https://github.com/ligate-io/ligate-chain/issues/42).
2. **`Attestation/SequencerRotated` event.** So attestors and dashboards know when rotation took effect. Same shape as the typed events from [#295](https://github.com/ligate-io/ligate-chain/issues/295); follow the same pattern.
3. **State-snapshot publishing** ([#273](https://github.com/ligate-io/ligate-chain/issues/273)). Unblocks Procedure C — operators can rebuild state from a snapshot when their local DB is gone.
4. **Pre-devnet drill.** Run Procedure A end-to-end on `ligate-localnet` and time it; record the operator-facing steps as a script so the actual devnet-1 rotation is mechanical.

This page lands the procedure for state-disposable rotation today and clearly marks the gaps for state-preserving rotation. When the gaps close, this page tightens in lockstep.
