# `DASubmissionFailures`

**Severity: page**

## What fired

`rate(ligate_da_submission_failures_total[5m]) > 0` sustained for 5+ minutes. ligate-node's blob submissions to Celestia are failing.

Reason label values come from `crates/rollup/src/metrics.rs::DaSubmissionFailureReason`:
- `light_node_unreachable` — TCP / RPC to the local Celestia light node failed
- `light_node_not_synced` — light node responded but reports unsynced
- `insufficient_funds` — TIA balance too low to pay the blob fee
- `invalid_signer` — DA signer key was rejected by Celestia
- `submission_timeout` — submission accepted but didn't finalize within the configured window
- `unknown` — anything else; logs carry detail

The `{{ $labels.reason }}` template in the alert summary surfaces which.

## Likely causes (by reason)

1. **`light_node_unreachable`** → [`celestia-light-node.md` Recovery A](../celestia-light-node.md#recovery-a--restart-the-light-node)
2. **`light_node_not_synced`** → Light node is alive but lost peers / bridge. [`celestia-light-node.md` Recovery B](../celestia-light-node.md#recovery-b--bridge-node-connectivity)
3. **`insufficient_funds`** → TIA balance hit zero. [`da-signer-rotation.md` "TIA balance monitoring"](../da-signer-rotation.md#tia-balance-monitoring) for top-up procedure.
4. **`invalid_signer`** → Key rotated wrong, key file corrupted, or the DA-signer config mismatch. [`da-signer-rotation.md` Procedure A](../da-signer-rotation.md#procedure-a--rotation-under-operator-control-planned).
5. **`submission_timeout`** → Network or Celestia-side issue. Usually transient; persistent timeouts suggest investigate the bridge.

## First checks

```sh
# 1. Which reason is firing?
curl -s http://127.0.0.1:9100/metrics | grep ligate_da_submission_failures

# 2. Recent submission attempts in the chain log
journalctl -u ligate-node --since "-10 minutes" | grep -iE "da|blob|submit"

# 3. Light node health
celestia rpc das samplingstats
celestia rpc header sync-state

# 4. TIA balance (if `insufficient_funds`)
celestia-appd query bank balances <da-signer-addr> --denom utia
```

## Fix

Branch by the `reason` label per the table above. Each reason has a dedicated runbook section to follow.

If multiple reasons fire simultaneously, start with the light node (`light_node_unreachable` / `not_synced`) — if the light node is wedged, the other reasons may be downstream symptoms.

## False positives

- **Brief network blip** during scheduled bridge maintenance. Alert's `for: 5m` filters single-tx failures.
- **Mocha testnet halt** (Celestia-side incident). Outside our control; pause submissions until Celestia is back, banner up.

## Escalation

Page-severity → operator phone notification fires. Slack post immediately. If light-node restart doesn't resolve in 10 minutes AND `BlockHeightStall` is also firing, post status banner to indicate partial outage. Coordinate with Celestia-side incident channels if Mocha itself is down.
