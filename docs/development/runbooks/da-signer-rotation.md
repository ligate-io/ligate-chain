# Runbook — DA Signer (Celestia) Key Rotation

**Status:** v0 — covers the procedure for Celestia DA. Mock DA needs no DA-side key. Companion to [`sequencer-key-rotation.md`](./sequencer-key-rotation.md), which covers the chain-side signing key.

This page is about `da.signer_private_key` in `rollup.toml` — the Celestia wallet that signs blob-submission transactions on the DA layer. The blast radius of a DA-key incident is **different** from a chain-key incident: a DA-key compromise can drain TIA and halt blob submission, but it cannot rewrite chain state or forge attestations.

---

## When does the DA signer need rotation?

| Cause | Severity | What it blocks |
|---|---|---|
| **DA key compromised** (private key leaked) | High | Adversary submits blobs as us, drains TIA, can DoS blob submission via gas-bidding wars. Chain state is safe. |
| **DA key lost** (operator loses the file) | High | We can't submit blobs at all. Chain stalls until rotation. |
| **TIA balance hits zero** | Medium | Submissions fail at the Celestia node. Chain stalls until refunded — easier fix than full rotation. |
| **Routine policy** | Low | Periodic rotation per operator policy. |

The first three are operational outages. The fourth is a planning exercise.

---

## Procedure A — Rotation under operator control (planned)

1. **Generate the replacement Celestia keypair.**
   ```sh
   celestia-appd keys add ligate-da-2026q3 --keyring-backend file
   # Records mnemonic — back it up immediately, separately from the
   # node's filesystem. Mnemonic is the recovery primitive; the file
   # on disk is the operational primitive.
   ```

2. **Fund the new address.**
   - Mocha testnet (devnet target): hit the Mocha faucet (`https://faucet.celestia-mocha.com` or community channels).
   - Internal transfer from the old address: `celestia-appd tx bank send <old> <new> <amount>utia`.
   - **Target balance:** at least 30 days of expected blob-fee burn. For `ligate-devnet-1` with one blob per slot at ~1000ms block time, that's roughly `2.6M slots/month × avg_blob_fee_utia`. Pull the current avg from `[monitoring].telegraf_address` metrics or `celestia-appd query txs` on a sample.

3. **Stop the sequencer.** Required to swap keys cleanly — partial-state rotation isn't supported by the DA adapter.
   ```sh
   systemctl stop ligate-node    # or pkill -f ligate-node
   ```

4. **Update `rollup.toml`.**
   ```toml
   [da]
   signer_private_key = "<new-key-hex>"
   sender_address = "<new-celestia-address>"  # matches the new key
   ```

5. **Update `genesis/sequencer_registry.json`.** `seq_da_address` must match `[da].sender_address` from step 4. Mismatch → sequencer refuses to start with a clear error.

6. **Restart.** The node opens a fresh DA-adapter session with the new key and starts submitting blobs as the new wallet.

7. **Verify.** Watch the next slot's blob land on Celestia from the new address:
   ```sh
   celestia-appd query txs \
     --query "transfer.sender='<new-celestia-address>'" \
     --limit 5
   ```

   And confirm our chain-side advance via `/v1/ledger/slots/latest` reporting a height beyond the rotation point.

**Estimated downtime:** ~30 seconds (clean stop + key swap + restart). Coordinated with operators ahead of time; usually a maintenance window.

---

## Procedure B — Emergency rotation under compromise

Adversary has the private key. Every additional minute is risk. Cut downtime corners:

1. **Generate + fund the replacement key** ahead of time if you have one pre-staged (recommended; keep a warm spare in cold storage). If not, run step 1 of Procedure A as fast as the Celestia faucet allows.
2. **Drain the compromised wallet** before the adversary does. Tx the entire balance minus a tiny rotation-fee buffer to the new address:
   ```sh
   celestia-appd tx bank send <old> <new> <balance-minus-fee>utia
   ```
3. **Stop + rotate + restart** per Procedure A steps 3-6, with no maintenance-window niceties.
4. **Post-mortem.** How did the key leak? File a write-up; tighten key storage (HSM, sealed-secrets, whatever the gap was).

The compromise window — from key leak to wallet drain — is what determines actual loss. Pre-staging a replacement key keeps this window seconds, not hours.

---

## TIA balance monitoring (avoids Procedure B entirely)

Hitting zero is the most common "rotation" event. Avoid it with alerting:

```yaml
# Alertmanager rule (tracked in chain repo #268; sketch here)
- alert: LigateDaSignerLowBalance
  expr: ligate_da_signer_balance_utia < 50_000_000   # 50 TIA in utia
  for: 5m
  annotations:
    runbook: docs/development/runbooks/da-signer-rotation.md#tia-balance-monitoring
```

The chain doesn't emit this metric today; needs a Prometheus exporter that wraps `celestia-appd query bank balances <addr>`. Filed as a follow-up to [#268](https://github.com/ligate-io/ligate-chain/issues/268).

**Manual check** until that ships:

```sh
celestia-appd query bank balances <da-address> --denom utia
```

Recommended threshold: keep at least 60 days of expected burn in the wallet, alert at 30 days remaining, urgent-page at 7 days remaining.

---

## What chain-side observers see during rotation

- **`/v1/rollup/info`** unchanged. Chain identity doesn't depend on the DA signer.
- **`/v1/ledger/slots/latest`** advances slowly (≤ ~30s gap) during the stop-restart window, then resumes normal cadence.
- **Sequencer-registry state** unchanged. The DA address gets updated in genesis but the sequencer's chain-side identity stays the same.

If observers see slot production halt for >2 minutes during a rotation, the operator should investigate (DA adapter failed to find the new key file, TIA wallet not yet funded, etc.) rather than assume it's normal.

---

## Cross-references

- [`sequencer-key-rotation.md`](./sequencer-key-rotation.md) — companion runbook for the chain-side key.
- [`docs/development/celestia-ops.md`](../celestia-ops.md) — broader Celestia-side operations notes.
- [`devnet-1/celestia.toml`](../../../devnet-1/celestia.toml) — the production DA config the procedure above edits.
- [Issue #268](https://github.com/ligate-io/ligate-chain/issues/268) — Alertmanager wiring; the TIA-balance alert lands there.
