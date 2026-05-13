# `MempoolDeep`

**Severity: warn**

## What fired

`ligate_mempool_depth > 1000` for 5+ minutes. The sequencer is accepting transactions faster than it can pack them into batches.

Threshold (`> 1000`) is a placeholder until we have Phase 6.1 deployment data to tune; expect to revisit once we see real load.

## Likely causes

1. **DA submission backed up.** Sequencer pre-builds batches but can't seal them until Celestia accepts the blob. Mempool fills behind the bottleneck.
2. **Load exceeds capacity.** Abnormally high tx submission rate (spam, viral usage, attack).
3. **Sequencer running but mempool drain task hung.** Specific to the mempool worker; rest of the chain alive.
4. **TIA balance starvation.** Submissions fail soft-style; sequencer keeps queuing; mempool grows.

## First checks

```sh
# 1. Confirm depth + look at trend
curl -s http://127.0.0.1:9100/metrics | grep ligate_mempool_depth

# 2. DA side — is finalization happening?
curl -s http://127.0.0.1:9100/metrics | grep -E "ligate_da_(submission_failures|finalization_latency)"

# 3. Recent submission rate
curl -s http://127.0.0.1:9100/metrics | grep ligate_rpc_requests_total | grep submit

# 4. TIA balance on the DA signer wallet
celestia-appd query bank balances <da-signer-addr> --denom utia
```

## Fix

- **DA submission backed up** → resolve the DA-side issue first ([`celestia-light-node.md`](../celestia-light-node.md) or [`da-signer-rotation.md`](../da-signer-rotation.md)). Mempool drains naturally once submission flows again.
- **Genuine load spike** → no immediate action; chain will catch up. Monitor for `DiskFillingFast` co-firing (would indicate state bloat from real load).
- **Worker hung** → `systemctl restart ligate-node`. Cursor preserved.
- **TIA starvation** → top up per [`da-signer-rotation.md`](../da-signer-rotation.md).

## False positives

- **Steady-state load above threshold.** When real partner usage settles, this threshold will be too low; tune up to reflect actual baseline + 50%.
- **Brief burst during a partner-onboarding wave.** Should resolve within a few minutes; alert's `for: 5m` filters most cases.

## Escalation

Warn-severity → Slack only. Page if it co-fires with `BlockHeightStall` or `DASubmissionFailures` (then it's a wedge, not load).
