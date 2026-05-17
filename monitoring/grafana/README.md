# Grafana dashboards (deployed)

Source of truth for dashboards currently live on `ligate.grafana.net`.
Re-import from the JSON files here to restore or replicate the views.

## Inventory

| File | UID | Public URL | Data source | Status |
|---|---|---|---|---|
| [`ligate-operator.json`](ligate-operator.json) | `ligate-operator` | [/d/ligate-operator](https://ligate.grafana.net/d/ligate-operator) | Prometheus (`grafanacloud-ligate-prom`) ← Alloy → `ligate-node:9100/metrics` | **Live** |
| [`ligate-investor.json`](ligate-investor.json) | `ligate-investor` | [/d/ligate-investor](https://ligate.grafana.net/d/ligate-investor) | Infinity REST → `api.ligate.io` | **Live** |
| [`ligate-devnet-1-cost.json`](ligate-devnet-1-cost.json) | (varies) | — | GCP Cloud Billing (not yet wired) | Pending |

## Operator dashboard (Prometheus)

What's currently scraped and queryable in `grafanacloud-ligate-prom`:

- `ligate_*`: 15 chain-specific series (block_height, mempool_depth, state_db_size_bytes, attestations_submitted/rejected_total, schemas_registered_total, attestor_sets_registered_total, da_finalization_latency_seconds histogram, rpc_request_duration_seconds histogram, rpc_requests_total, metrics_dropped_total)
- `process_*`: 7 process metrics from the `ligate-node` binary (cpu_seconds_total, resident_memory_bytes, open_fds, max_fds, threads, virtual_memory_bytes, start_time_seconds)
- `rockbound_*` + `schemadb_*`: RocksDB hot-path latency / bytes histograms
- `storage_*`: storage layer counters
- `node_*`: 265 host-level series from Alloy's in-process `prometheus.exporter.unix` (CPU, memory, filesystem, network, disk I/O, load average, systemd unit state, boot time). Added 2026-05-16 per chain#363.

9 rows: live status, throughput, block production over time, DA finality (single-pane + percentile time series), RPC latency + load, ligate-node process, RocksDB hot path, telemetry health, host VM (CPU / RAM / state disk fill / load + network and disk-I/O time series).

## Investor dashboard (Infinity)

Public-facing business metrics. Sourced from `api.ligate.io` via the Infinity REST data source.

7 rows: live status (chain+indexer height, lag, active wallets 24h), cumulative totals (txs, wallets, schemas, attestor sets, attestations), token supply + treasury balance, growth over time (new wallets/day, tx rate/day by kind), top 10 LGT holders, DA finality observed (last 1h), block cadence.

Chain identity (`ligate-devnet-1`) renders in a markdown header at the top. Stat panels in Grafana don't render string values from Infinity well (defaults to "No data" because `lastNotNull` reducer expects numeric). Markdown header is the cleaner fix.

## Re-import procedure

If a dashboard is deleted or corrupted:

```bash
source ~/.config/ligate/secrets.env  # GRAFANA_URL + GRAFANA_TOKEN

JSON_PATH=monitoring/grafana/ligate-operator.json
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
UID=ligate-operator
curl -s -H "Authorization: Bearer $GRAFANA_TOKEN" \
  "$GRAFANA_URL/api/dashboards/uid/$UID" | \
  python3 -c "import sys,json; d=json.load(sys.stdin); print(json.dumps(d['dashboard'], indent=2, sort_keys=True))" \
  > monitoring/grafana/$UID.json
git diff monitoring/grafana/$UID.json   # review the change
```

Commit so the repo stays the source of truth.

## Alloy config reference (the scraper)

Lives on `ligate-devnet-1-sequencer` at `/etc/alloy/config.alloy`. Five
blocks today:

- `local.file "gc_token"`: reads the Grafana Cloud API token from `/etc/alloy/grafana_cloud_token` (chmod 600) at agent start so it never appears in process listings
- `prometheus.scrape "ligate_node"`: scrapes `127.0.0.1:9100` every 15s, forwards to `grafana_cloud`
- `prometheus.exporter.unix "node"`: in-process node_exporter, default collector set minus the GCE-irrelevant ones (`ipvs`, `btrfs`, `infiniband`, `tapestats`, `zfs`). Added 2026-05-16 per chain#363
- `prometheus.scrape "node"`: scrapes the in-process exporter targets above every 15s, same forward path
- `prometheus.remote_write "grafana_cloud"`: pushes to `https://prometheus-prod-66-prod-us-east-3.grafana.net/api/prom/push` with the token, 12h local WAL buffer

`sudo systemctl reload alloy` picks up config changes; no agent reinstall needed.
