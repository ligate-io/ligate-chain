# Grafana dashboards (deployed)

Source of truth for dashboards currently live on `ligate.grafana.net`.
Re-import from the JSON files here to restore or replicate the views.

## Inventory

| File | UID | Public URL | Data source | Status |
|---|---|---|---|---|
| [`ligate-chain.json`](ligate-chain.json) | `ligate-chain` | [/d/ligate-chain](https://ligate.grafana.net/d/ligate-chain) | Infinity REST → `api.ligate.io` | **Live** |
| `ligate-vm.json` (planned) | `ligate-vm` | `/d/ligate-vm` | Prometheus (`grafanacloud-ligate-prom`) ← Grafana Cloud Agent → `node_exporter` on chain VM | Pending agent install |
| [`ligate-devnet-1-cost.json`](ligate-devnet-1-cost.json) | (varies) | — | GCP Cloud Billing (not yet wired) | Pending |

Separately, a Prometheus-based **chain-runtime** dashboard lives at
[`../../ops/grafana/ligate-node.json`](../../ops/grafana/ligate-node.json).
That waits on a Prometheus scraper targeting the chain's
`127.0.0.1:9100/metrics` endpoint — same pre-req as `ligate-vm.json`.
Both re-import together once the agent lands. See
[ligate-chain#316](https://github.com/ligate-io/ligate-chain/issues/316).

## Application & chain dashboard (`ligate-chain.json`)

The "everything we can show today" dashboard. Refresh 15s. Default
window 6h. Eight rows:

1. **Chain identity & live status** — chain_id, chain + indexer head, lag in slots + seconds
2. **Block cadence** — mean / p95 interval, seconds since last, seconds until expected
3. **DA finality (observed, last 1h)** — p50 / p95 / p99 / sample count from `/v1/stats/finality`
4. **Cumulative totals** — txs, wallets, schemas, attestor sets, attestations, active wallets 24h
5. **Token supply & treasury** — total LGT supply, treasury balance
6. **Activity over time** — new wallets/day (30d), tx rate/day by kind (30d, stacked)
7. **Holder distribution** — top 10 LGT holders table
8. **Coverage notes** — explicit text panel listing what's NOT here + where to find it

Designed for both partners/investors (top half: identity, finality,
totals) and ops (cadence + lag panels at the top). Splitting these
back into two dashboards is reasonable later once each grows; for
now one consolidated view beats two redundant ones.

## VM dashboard (`ligate-vm.json`, pending)

Will cover the underlying machine, not the chain process:

- CPU usage (% by mode: user, system, iowait, idle)
- Memory usage (used / free / buffers / cache)
- Disk usage (% per mount, IOPS, throughput)
- Network throughput (bytes/sec in + out per interface)
- Systemd unit status (`caddy`, `ligate-node`, `grafana-agent`)
- Process metrics for `ligate-node` specifically (CPU, RSS, open FDs)

Sourced from `node_exporter` integrations baked into Grafana Cloud
Agent. Agent pushes to `grafanacloud-ligate-prom`. Once installed on
`ligate-devnet-1-sequencer`, this dashboard becomes the second
panel of glass for "is the VM healthy."

Installation tracked in
[ligate-chain#316](https://github.com/ligate-io/ligate-chain/issues/316).

## Re-import procedure

If a dashboard is deleted or corrupted:

```bash
source ~/.config/ligate/secrets.env  # GRAFANA_URL + GRAFANA_TOKEN

JSON_PATH=monitoring/grafana/ligate-chain.json
python3 -c "import json; d=json.load(open('$JSON_PATH')); print(json.dumps({'dashboard': d, 'overwrite': True, 'message': 'Re-import from repo'}))" > /tmp/import.json

curl -X POST -H "Authorization: Bearer $GRAFANA_TOKEN" \
  -H "Content-Type: application/json" \
  "$GRAFANA_URL/api/dashboards/db" \
  -d @/tmp/import.json
```

## Updating a dashboard

Edit in the Grafana UI, then export the canonical JSON back to repo:

```bash
source ~/.config/ligate/secrets.env
UID=ligate-chain
curl -s -H "Authorization: Bearer $GRAFANA_TOKEN" \
  "$GRAFANA_URL/api/dashboards/uid/$UID" | \
  python3 -c "import sys,json; d=json.load(sys.stdin); print(json.dumps(d['dashboard'], indent=2, sort_keys=True))" \
  > monitoring/grafana/$UID.json
git diff monitoring/grafana/$UID.json   # review the change
```

Commit the resulting JSON so the repo stays the source of truth.
