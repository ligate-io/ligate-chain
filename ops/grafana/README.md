# Grafana dashboard for `ligate-node`

Drop-in Grafana dashboard for the Phase 1 metrics shipped on
`ligate-node:9100/metrics`. Three rows, eight panels:

- **Chain activity** — schemas registered, attestor sets registered,
  attestation submission rate.
- **Node health** — block height, state-DB size, RPC request rate by
  endpoint.
- **Errors / rejections** — top-10 rejection reasons over the last
  hour, RPC p95 latency by endpoint.

Tracking issue: [#165](https://github.com/ligate-io/ligate-chain/issues/165).

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
| State DB size | `ligate_state_db_size_bytes` |
| RPC requests / sec | `sum by (endpoint) (rate(ligate_rpc_requests_total[5m]))` |
| Rejections by reason (top 10) | `topk(10, sum by (reason) (rate(ligate_attestations_rejected_total[1h])))` |
| RPC p95 latency | `histogram_quantile(0.95, sum by (le, endpoint) (rate(ligate_rpc_request_duration_seconds_bucket[5m])))` |

## Versioning

The dashboard is checked into the repo at `ops/grafana/ligate-node.json`
with `schemaVersion: 38` (Grafana 10.0+). Future schema bumps update
the JSON in-place; operators re-import to pick up changes.

When Phase 2 metrics ([#164](https://github.com/ligate-io/ligate-chain/issues/164))
land — DA submission latency, sequencer-specific signals, RPC error
histograms — this dashboard grows a fourth row. See [#165](https://github.com/ligate-io/ligate-chain/issues/165)
for the layout discussion.

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
