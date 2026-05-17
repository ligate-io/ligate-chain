# `BlockHeightStall`

**Severity: page** (page-severity alerts post to `#chain-alerts` with an `@here` mention; wiring details in [`discord-setup.md`](./discord-setup.md))

## What fired

`ligate_block_height` has not advanced for at least 60s on `ligate-node`. The sequencer is either halted, wedged on DA, or has lost its Celestia light node.

Expression:
```
changes(ligate_block_height[2m]) == 0
  and on(instance) (ligate_block_height > 0)
```

(The `> 0` clause prevents the alert from firing pre-genesis or right after a clean restart where the gauge hasn't initialised yet.)

## Likely causes (ranked by frequency)

1. **Celestia light node is unhealthy.** Blob submissions backed up; sequencer stops sealing batches once `sov-blob-sender`'s queue exceeds capacity.
2. **DA submission failures.** TIA balance hit zero, or the DA signer key is wrong, or the bridge node is unreachable.
3. **Sequencer process is hung but not crashed.** Stuck in a tight loop or deadlock; would also trip `MempoolDeep` shortly after.
4. **Network partition between sequencer and light node.** Both processes alive but can't talk.

## First checks

```sh
# 1. Is the chain process alive + serving?
systemctl status ligate-node
curl -s http://127.0.0.1:12346/v1/info | jq

# 2. What does the chain think its head is?
curl -s http://127.0.0.1:12346/v1/ledger/slots/latest | jq .number

# 3. What's the light node up to?
celestia rpc das samplingstats
celestia rpc header sync-state | jq .height

# 4. Recent ligate-node + light-node logs
journalctl -u ligate-node --since "-10 minutes" | tail -50
journalctl -u celestia-light --since "-10 minutes" | tail -50
```

If the light node looks wedged, jump to [`celestia-light-node.md`](../celestia-light-node.md).

If TIA balance is low, jump to [`da-signer-rotation.md`](../da-signer-rotation.md).

## Fix

- **Light node wedged** → restart per [`celestia-light-node.md` Recovery A](../celestia-light-node.md#recovery-a--restart-the-light-node)
- **TIA balance zero** → top up per [`da-signer-rotation.md` "TIA balance monitoring"](../da-signer-rotation.md#tia-balance-monitoring)
- **Sequencer hung but alive** → `systemctl restart ligate-node`; the cursor is persisted, restart is safe
- **Sequencer crashed** → `journalctl -u ligate-node --since "-30 minutes"` to find the panic; file a bug; `systemctl restart`

## False positives

- **Pre-genesis** (the `> 0` clause prevents this; sanity-check the alert if it ever fires with height 0).
- **Deliberate sequencer halt** (chain-id bump cutover per [`chain-id-bump.md`](../chain-id-bump.md)) — silence the alert in Alertmanager for the cutover window.
- **Slot timer mid-tick** (alert window is 60s; chain produces every ~1s in steady-state so this should never fire on a 1-block lag).

## Escalation

Page-severity → operator phone notification fires automatically. If unresolved 15 minutes after page, post in `#devnet-ops` Slack channel and announce the partial outage via `docs.ligate.io` status banner. Mainnet incident protocol lives in a separate runbook (not yet filed).
