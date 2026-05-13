# `ReadyFalse`

**Severity: warn**

## What fired

`ligate-node` is alive (metrics scrape succeeds) but `/ready` has returned non-200 for 60+ seconds.

Expression:
```
(probe_http_status_code{job="ligate-node-ready"} != 200)
  or on(instance) (ligate_ready == 0)
```

`/ready` distinguishes "alive but catching up" from "ready to serve traffic." Common during fresh boots while backfilling DA slots; less common in steady-state and worth investigating then.

## Likely causes

1. **Fresh boot, still backfilling DA slots.** Normal during the first ~30s-2min after startup; alert's `for: 60s` will catch protracted cases.
2. **Sequencer waiting on DA finalization.** Blob submitted but Celestia hasn't finalized it; chain doesn't advance "ready" past unfinalized data.
3. **Attestor quorum lag.** Attestor set hasn't reached threshold for the latest batch; ready state holds.
4. **Misconfigured `/ready` definition.** Chain code's notion of "ready" differs from operator's; should be a rare config bug.

## First checks

```sh
# 1. What does /ready actually say?
curl -i http://127.0.0.1:12346/v1/ready

# 2. Catch-up state
curl -s http://127.0.0.1:12346/v1/info | jq '{indexer_height, head_height, head_lag_slots}'

# 3. Recent DA-side activity
journalctl -u ligate-node --since "-5 minutes" | grep -iE "da|celestia|blob"

# 4. Mempool depth + DA submission failures (rule out a wedge)
curl -s http://127.0.0.1:9100/metrics | grep -E "ligate_mempool_depth|ligate_da_submission_failures"
```

## Fix

- **Fresh boot backfilling** → wait. If still firing 5 minutes after a clean restart, investigate as a wedge.
- **DA finalization lag** → check the Celestia bridge. If finalization is steady but slow, no action; if stalled, jump to [`celestia-light-node.md`](../celestia-light-node.md).
- **Attestor quorum lag** → out of scope for v0 (no attestor quorum yet). When attestors are live, check the attestor-side dashboard.
- **Config bug** → check `rollup.toml` against the committed `devnet-1/celestia.toml`. Reload + restart if drift.

## False positives

- **First 60-90s of every fresh boot.** Expected. Silence during planned restarts.
- **Chain-id bump cutover window.** New chain starts from height 0; ready holds until first slot finalizes.
- **DA-layer maintenance window** (Celestia upgrades). Expected; coordinate via the status banner.

## Escalation

Warn-severity → Slack only. If still firing 30+ minutes with no progress on head_height, manually escalate to page by re-checking against `BlockHeightStall` (which should also be firing if this is a real wedge, not a fresh boot).
