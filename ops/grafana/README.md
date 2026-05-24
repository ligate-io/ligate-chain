# Grafana dashboards

Source of truth for the dashboards currently live on
[`ligate.grafana.net`](https://ligate.grafana.net). Re-import from
these JSON files to restore or replicate.

> **2026-05-24 reorg:** four dashboards, one purpose each, no
> duplicated panels across them. Before this reorg the `ligate-node`
> dashboard alone was carrying engineering internals + business
> totals + cost panels + operator alerts in one wall of 38 panels;
> the same metrics also appeared in `ligate-operator` and
> `ligate-investor` with overlapping queries. Now each panel lives in
> exactly one dashboard and the framing matches the audience.

## Inventory

| File | UID | Audience | Public URL |
|---|---|---|---|
| [`ligate-node.json`](ligate-node.json) | `ligate-node` | Chain engineers (DA pipeline + sequencer state machine) | [/d/ligate-node](https://ligate.grafana.net/d/ligate-node) |
| [`ligate-operator.json`](ligate-operator.json) | `ligate-operator` | 24/7 on-call (host health + service SLA) | [/d/ligate-operator](https://ligate.grafana.net/d/ligate-operator) |
| [`ligate-investor.json`](ligate-investor.json) | `ligate-investor` | Partners + investors (chain growth + token economy) | [/d/ligate-investor](https://ligate.grafana.net/d/ligate-investor) |
| [`ligate-devnet-1-cost.json`](ligate-devnet-1-cost.json) | `ligate-devnet-1-cost` | Money-flow watching (TIA + AVOW + GCP burn) | [/d/ligate-devnet-1-cost](https://ligate.grafana.net/d/ligate-devnet-1-cost) |

## Ownership boundaries

**`ligate-node` (Engineering).** What does the chain process look
like internally? DA submission failures by reason, DA finalization
latency distribution, blob accounting counters (bytes / gas total /
published count), sequencer role + transitions (paper-leader visibility),
mempool depth time series, attestation rejections by reason, attestation
submission rate, plus the three activity-leader tables for "who's
hitting the chain hardest" debugging. Roughly 13 panels.

**`ligate-operator` (On-call).** Is the production deployment healthy
right now? Liveness probes (block height, mempool, state DB, process
uptime), block production cadence, DA finality SLA percentiles,
ligate-node process metrics (CPU/RSS/FDs), RocksDB hot-path latency,
RPC requests/sec + p95 latency by endpoint, telemetry health, full
host VM surface (CPU/RAM/disk/network/load), and the **VM capacity
headroom** stat row sized against the current e2-standard-2 shape
(green/amber/red thresholds for "when to upsize").

**`ligate-investor` (Public-facing).** How is the chain growing?
Cumulative totals (txs, wallets, schemas, attestor sets, attestations),
AVOW supply and treasury balance, top-10 AVOW holders, growth-over-time
charts (new wallets/day, tx rate stacked by kind), DA finality (last
1h), block cadence summary. No process metrics, no host noise.

**`ligate-devnet-1-cost` (Money flow).** What's the chain costing? TIA
balance + draw-down rate against the sequencer's Celestia wallet,
faucet AVOW balance + runway estimate, daily drips counter, plus the
DA + infra spend section (TIA burned cumulative + per-hour, GCP daily
burn time series, GCP by-service + by-VM bar gauges, GCP month-to-date
stat). Single dashboard for everyone watching the burn.

## Data sources

Live dashboards use the Grafana Cloud-managed data sources:

- **Prometheus** (`grafanacloud-ligate-prom`): scraped by Alloy on
  the sequencer VM from `ligate-node:9100/metrics` + host metrics
  via `prometheus.exporter.unix`, pushed every 15 s.
- **Infinity HTTP-JSON** (`yesoreyeram-infinity-datasource`): for
  panels that read from `api.ligate.io` (activity leaders, investor
  totals) or `rpc.ligate.io/cost/*.json` (the GCP cost panels backed
  by the VM-1 BigQuery sidecar at
  `/opt/ligate/bin/gcp-cost-fetch.sh`, see
  [`docs/development/runbooks/gcp-billing-export.md`](../../docs/development/runbooks/gcp-billing-export.md)).

## Import procedure

In Grafana → **Dashboards** → **New** → **Import** → paste contents
of the relevant JSON. When prompted, select the matching data source
type. The dashboards use `${DS_PROMETHEUS}` / `${DS_INFINITY}` as
templated references, so any data source of the right type will work.

Alternatively, push a file via the HTTP API (see the workflow used in
[`docs/development/runbooks/grafana-deploy.md`](../../docs/development/runbooks/grafana-deploy.md)
if/when written; today the canonical path is manual import).

## Versioning

All four files are checked in with `schemaVersion: 38` (Grafana 10.0+).
Future schema bumps update the JSON in place; re-import to pick up.

## Phase 1 + 2 metric tracking

Phase 1 + 2 chain metrics (#110, #164, chain#446 Track 4, chain#452,
chain#392) all surface in `ligate-node` or `ligate-devnet-1-cost`
depending on whether they're internal-state or cost-of-running. Newer
metrics: append to the most-appropriate dashboard and avoid copying
to the other three.

## Reference Prometheus scrape config

The chain ships `/metrics` on port 9100 by default, bound to
`127.0.0.1` for security (per the public devnet runbook). Scrape from
inside the VM / VPC:

```yaml
# /etc/prometheus/prometheus.yml (operator-facing)
global:
  scrape_interval: 15s
  external_labels:
    chain: ligate-devnet-1

scrape_configs:
  - job_name: ligate-node
    static_configs:
      - targets: ['localhost:9100']
        labels:
          instance: sequencer-0
          role: sequencer
```

For multi-node setups (post-v0), add one `static_configs` entry per
node with distinct labels.

If you front Prometheus with a public Grafana per
[#234](https://github.com/ligate-io/ligate-chain/issues/234), keep
the scrape internal and only expose Grafana publicly. The chain's
Caddy config in
[`docs/development/public-devnet-deploy.md`](../../docs/development/public-devnet-deploy.md)
already blocks `/metrics` at the public edge.

## Related

- [`docs/development/public-devnet-deploy.md`](../../docs/development/public-devnet-deploy.md) — operator runbook with VM provisioning, Caddy config, monitoring section
- [`docs/development/celestia-ops.md`](../../docs/development/celestia-ops.md) — Celestia light-node operations
- [`docs/development/runbooks/gcp-billing-export.md`](../../docs/development/runbooks/gcp-billing-export.md) — VM-1 BigQuery sidecar that backs the GCP cost panels
- [#110](https://github.com/ligate-io/ligate-chain/issues/110) — Phase 1 metrics tracking issue
- [#164](https://github.com/ligate-io/ligate-chain/issues/164) — Phase 2 metrics
- [#234](https://github.com/ligate-io/ligate-chain/issues/234) — public Grafana hosting
