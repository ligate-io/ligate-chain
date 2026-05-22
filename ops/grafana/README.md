# Grafana dashboard for `ligate-node`

Drop-in Grafana dashboard for the Phase 1 + Phase 2 metrics shipped
on `ligate-node:9100/metrics`. Nine rows, thirty-eight panels:

- **Chain activity** — schemas registered, attestor sets registered,
  attestation submission rate.
- **Node health** — block height, mempool depth, state-DB size, RPC
  request rate by endpoint.
- **DA layer** — DA submission failures by reason, DA finalization
  latency p50 / p95 / p99 (Mocha-aware buckets).
- **Errors / rejections** — top-10 attestation rejection reasons over
  the last hour, RPC p95 latency by endpoint.
- **Process / observability** — process CPU (cores), RSS, open FDs,
  metrics-dropped rate (broadcast lag).
- **Cluster topology** (chain#446 follow-up) — sequencer role per VM
  (`unknown` / `replica` / `leader` value-mapped from
  `ligate_sequencer_role`) and rate of role transitions
  (`ligate_sequencer_role_transitions_total`). The role panel is the
  primary visual for the paper-leader scenario from 2026-05-21:
  exactly one `leader` should be lit at any time. Flat-zero
  transitions = steady state; spikes during a planned failover drill
  are expected; rapid flapping or two simultaneous leaders = page
  on-call.
- **Cost & economy** (chain#446 Track 4) — cumulative TIA burned
  (estimate, from `ligate_da_tia_burned_nano_estimate_total`), TIA
  burn rate per hour, protocol treasury balance in LGT
  (`ligate_protocol_treasury_balance_nano`), a 4-up stat row
  mirroring api `/v1/stats/totals` (txs / attestations / schemas /
  attestor sets), and **GCP daily burn (USD)** sourced from the
  VM-1 BigQuery sidecar at `https://rpc.ligate.io/cost/daily.json`
  (see `docs/development/runbooks/gcp-billing-export.md`). The TIA
  burn is an estimate based on a fixed per-blob constant;
  follow-up chain#452 covers the SDK receipt extension to make it
  authoritative.
- **VM capacity headroom** (chain#449 follow-up) — four threshold
  stat panels colored green / amber / red so you know when to
  upsize *before* a saturation incident:
  - **CPU usage (cores)** sized for e2-standard-2 (2 vCPU). Red
    >1.5 cores sustained = bump to e2-standard-4.
  - **Memory (RSS)** sized for 8 GB. Red >4 GB = bump VM tier.
  - **State DB size** sized for the default 100 GB SSD. Red >70 GB
    = resize SSD online (non-disruptive grow).
  - **Open FDs** as a canary for descriptor leaks (ulimit 65536).
- **Activity leaders** (api#67) — three table panels backed by the
  api's leaderboard endpoints via the Infinity HTTP-JSON
  datasource: top attesters (`/v1/stats/top-attesters`), top schema
  owners (`/v1/stats/top-schema-owners`), top attestor sets
  (`/v1/stats/top-attestor-sets`). Same shape across all three
  (`rank` / `address` / `count`), 10 rows each, 30s cache on the
  api side. The "who's using the chain most" view.

Tracking issue: [#165](https://github.com/ligate-io/ligate-chain/issues/165).

The Infinity datasource (used by GCP daily burn + the three
leaderboards) is preinstalled in Grafana Cloud. Self-hosted Grafana
needs `grafana-infinity-datasource` from the plugins catalog. No
auth — both target hosts (`api.ligate.io` + `rpc.ligate.io`) are
public.

## Import

In Grafana → **Dashboards** → **New** → **Import** → paste the
contents of [`ligate-node.json`](ligate-node.json) (or upload the
file directly).

When prompted, select your Prometheus data source. The dashboard
uses `${DS_PROMETHEUS}` as a templated reference, so any data source
of `type: prometheus` will work.

The dashboard refreshes every 30 seconds and defaults to a 6-hour
window; both are configurable from the dashboard's time-picker /
refresh controls.

## Reference Prometheus scrape config

The chain ships `/metrics` on port 9100 by default, bound to
`127.0.0.1` for security (per the public devnet runbook). Scrape
from inside the VM / VPC:

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

For a follower, set `role: follower` and adjust `instance:`
accordingly. For multi-node setups (post-v0), add one `static_configs`
entry per node with distinct labels.

If you front Prometheus with a public Grafana per
[#234](https://github.com/ligate-io/ligate-chain/issues/234), keep
the scrape internal and only expose Grafana publicly. The chain's
Caddy config in [`docs/development/public-devnet-deploy.md`](../../docs/development/public-devnet-deploy.md)
already blocks `/metrics` at the public edge.

## Panel queries (PromQL reference)

The dashboard JSON inlines these; documented here so an operator can
recreate or fork them without re-importing.

| Panel | Query |
|---|---|
| Schemas registered | `ligate_schemas_registered_total` |
| Attestor sets registered | `ligate_attestor_sets_registered_total` |
| Attestations submitted (rate) | `rate(ligate_attestations_submitted_total[5m])` |
| Block height | `ligate_block_height` |
| Mempool depth | `ligate_mempool_depth` |
| State DB size | `ligate_state_db_size_bytes` |
| RPC requests / sec | `sum by (endpoint) (rate(ligate_rpc_requests_total[5m]))` |
| DA submission failures (rate, by reason) | `sum by (reason) (rate(ligate_da_submission_failures_total[5m]))` |
| DA finalization latency p50 / p95 / p99 | `histogram_quantile(0.99, sum by (le) (rate(ligate_da_finalization_latency_seconds_bucket[5m])))` |
| Rejections by reason (top 10) | `topk(10, sum by (reason) (rate(ligate_attestations_rejected_total[1h])))` |
| RPC p95 latency | `histogram_quantile(0.95, sum by (le, endpoint) (rate(ligate_rpc_request_duration_seconds_bucket[5m])))` |
| Process CPU (cores) | `rate(process_cpu_seconds_total[5m])` |
| Process RSS | `process_resident_memory_bytes` |
| Process open FDs | `process_open_fds` |
| Metrics dropped (broadcast lagged) | `rate(ligate_metrics_dropped_total[5m])` |

## Versioning

The dashboard is checked into the repo at `ops/grafana/ligate-node.json`
with `schemaVersion: 38` (Grafana 10.0+). Future schema bumps update
the JSON in-place; operators re-import to pick up changes.

Phase 2 metrics ([#164](https://github.com/ligate-io/ligate-chain/issues/164))
landed in #281 (mempool, process collector, CHAIN_HASH pin) and #282
(DA failures, DA finalization latency, broadcast-lag counter). The
dashboard's "DA layer" and "Process / observability" rows wrap them.
Future panels for sequencer signals not yet exposed (validator stalls,
state-trie depth) will land via follow-up issues.

## Deploying a public Grafana

Hosting a public read-only Grafana at `grafana.ligate.io` is tracked
in [#234](https://github.com/ligate-io/ligate-chain/issues/234).
Until that lands, this dashboard is operator-internal.

## Related

- [`docs/development/public-devnet-deploy.md`](../../docs/development/public-devnet-deploy.md) — operator runbook with VM provisioning, Caddy config, monitoring section
- [`docs/development/celestia-ops.md`](../../docs/development/celestia-ops.md) — Celestia light-node operations (the DA layer's own metrics live there)
- [#110](https://github.com/ligate-io/ligate-chain/issues/110) — Phase 1 metrics tracking issue
- [#164](https://github.com/ligate-io/ligate-chain/issues/164) — Phase 2 metrics (extends this dashboard)
- [#234](https://github.com/ligate-io/ligate-chain/issues/234) — public Grafana hosting
