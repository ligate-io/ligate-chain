# `DAFinalizationLatencyHigh`

**Severity: warn**

## What fired

`histogram_quantile(0.95, rate(ligate_da_finalization_latency_seconds_bucket[5m])) > 60` for 10+ minutes.

The 95th-percentile time from blob submission to Celestia finalization is above 60s — slower than the ~12s mocha block time + small finalization margin we expect in steady state.

## Likely causes

1. **Celestia mocha is congested.** Higher-fee blobs outbidding ours; finalization queue backs up.
2. **Our submission rate exceeded the light node's outbound bandwidth.** Submissions queue locally before reaching the bridge.
3. **Bridge node is slow.** The light node's upstream bridge has high latency to the wider Celestia network.
4. **Subnetwork issue** between the chain VM and the bridge.

## First checks

```sh
# 1. Confirm + see distribution shape
curl -s http://127.0.0.1:9100/metrics | grep ligate_da_finalization_latency

# 2. Cross-check Celestia's own health
# https://mocha.celenium.io/ (transactions per block + recent fees)

# 3. Light node peer count + sampling progress
celestia rpc p2p peers | jq '.peers | length'
celestia rpc das samplingstats

# 4. Our blob fee setting — are we underbidding?
grep -E "blob.fee|gas_price" /opt/ligate/devnet-1/celestia.toml
```

## Fix

- **Mocha congestion** → wait it out; tune our blob fee up in `celestia.toml` if congestion is persistent. Reload + restart.
- **Light node bandwidth** → unlikely on a normal VM, but check `iftop` / `bmon` on the chain VM. Increase the light node's peer count.
- **Bridge node slow** → switch to a backup bridge per [`celestia-light-node.md` Recovery B](../celestia-light-node.md#recovery-b--bridge-node-connectivity).
- **Subnetwork issue** → operator-side ops (check VPC config, NAT, firewall counters).

## False positives

- **First slot after a fresh boot** — local cache cold, the first submission may take longer. Alert's `for: 10m` filters this.
- **Celestia-side scheduled maintenance.** Expected; banner.

## Escalation

Warn-severity → Slack. Escalate to page only if `BlockHeightStall` also fires (then we have a real wedge, not just degraded performance). If p95 stays elevated >2 hours and the chain is keeping up otherwise, file a follow-up ticket to tune blob fees rather than incident-style handle.
