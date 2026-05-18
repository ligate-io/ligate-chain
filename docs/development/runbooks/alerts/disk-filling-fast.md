# `DiskFillingFast`

**Severity: warn** (warn-severity alerts post to `#chain-alerts` without a mention; wiring details in [`discord-setup.md`](./discord-setup.md))

## What fired

`ligate_state_db_size_bytes` is growing at >10%/hour for 30+ minutes on a single instance.

NOMT prunes aggressively, so steady-state growth should be slow. A sudden growth spike usually means abuse (partner spamming attestations) or a backfill blob storm.

## Likely causes

1. **Attestation spam from a partner.** Single address submitting 1000s of attestations per minute against a low-fee schema.
2. **Backfill replay.** Fresh follower catching up from genesis; growth rate matches DA replay speed.
3. **Misconfigured pruning.** NOMT pruning interval set too long; should be ~50 slots default.
4. **Failed pruning.** Pruning task crashed silently; state accumulates.

## First checks

```sh
# 1. Confirm growth rate + absolute size
curl -s http://127.0.0.1:9100/metrics | grep ligate_state_db_size_bytes
df -h /var/lib/ligate

# 2. Recent submit-attestation traffic
curl -s http://127.0.0.1:9100/metrics | grep 'ligate_rpc_requests_total{endpoint=~".*submit.*"}'

# 3. Per-schema attestation counts via the api
curl -s "https://api.ligate.io/v1/schemas?limit=10" \
  | jq '.data[] | {id, name, attestation_count}'

# 4. Pruning task health (logs)
journalctl -u ligate-node | grep -i prune | tail -20
```

## Fix

- **Attestation spam** → identify the partner address from the api's `/v1/addresses/{addr}/txs`. If clearly abusive, add to the chain-side blocklist (when it ships; pre-blocklist, escalate to the partner). For mainnet, fee markets handle this naturally; pre-devnet, social pressure is the lever.
- **Backfill replay** → expected on a fresh follower. No action; growth will plateau once caught up to head.
- **Misconfigured pruning** → check `rollup.toml` for the pruning interval. Match the committed `devnet-1/rollup.toml`.
- **Failed pruning** → restart `ligate-node`. If failure repeats, file an urgent bug with the pruning logs.

## False positives

- **First 24h after a backfill** — growth rate appears high relative to total because the denominator is still small. Tolerate.
- **After a chain-id bump** — fresh state, growth from zero. Same.

## Escalation

Warn-severity → Slack. Escalate to page if disk usage crosses 80% of the partition (separate alert that we'd add once disk-fill metrics are wired); manual judgment otherwise.
