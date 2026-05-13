# Cost monitoring — Mocha TIA spend + GCP infra spend

**Status:** v0 — Grafana dashboard + Alertmanager rules for cost visibility on `ligate-devnet-1`. Companion to [`da-signer-rotation.md`](runbooks/da-signer-rotation.md) (TIA wallet management), [`alerts/`](runbooks/alerts/) (alert runbooks), and [`public-devnet-deploy.md`](public-devnet-deploy.md) (the deploy this monitors).

Two cost surfaces:

| Surface | What's spending | Source of truth |
|---|---|---|
| Mocha TIA | DA signer wallet pays per-blob to Celestia | Celestia light node's `/metrics` endpoint (`celestia_node_da_signer_balance_utia`) |
| GCP infra | VM-hours, GCS storage, network egress | GCP billing export → BigQuery |

The dashboard at [`monitoring/grafana/ligate-devnet-1-cost.json`](../../monitoring/grafana/ligate-devnet-1-cost.json) puts both on one page so the operator can correlate cost spikes against chain activity.

---

## Mocha TIA cost

### Steady-state expectation

1 blob per slot at ~1000ms slot time × 86,400 slots/day = ~86,400 blobs/day. Average blob fee on Mocha ≈ 0.001 TIA. Daily burn ≈ **0.86 TIA/day** at full utilisation; usually less because not every slot has chain activity.

| Tier | Approximate runway |
|---|---|
| 50 TIA (warn threshold) | ~30 days |
| 12 TIA (page threshold) | ~7 days |
| 5 TIA/day draw-down (spike alarm) | something's wrong: abuse, backfill, or bug |

### What surfaces in Grafana

Three panels on the dashboard:

- **Current TIA balance** (stat). Red < 10, yellow < 50, green ≥ 50.
- **TIA balance trend** (timeseries). Plotted over the dashboard's window so the operator can eyeball the slope.
- **TIA draw-down rate** per `$window` (stat). Negative slope is normal (balance decreasing); positive sign in the panel after the `* -1` makes "TIA burned per day" the readable value.

### Alerting

Three rules in [`ops/prometheus/alerts.yaml`](../../ops/prometheus/alerts.yaml) under the `ligate-node.cost` group:

- `TIABalanceLow` — warn, 30m persistence. Below 50 TIA. Operator schedules a top-up within the next week.
- `TIABalanceCritical` — page, 5m persistence. Below 12 TIA. Operator tops up immediately or blob submission halts.
- `TIADrawdownSpike` — warn, 1h persistence. Daily burn rate exceeds 5 TIA/day. Surfaces abnormal activity.

Top-up procedure: [`da-signer-rotation.md` "TIA balance monitoring"](runbooks/da-signer-rotation.md#tia-balance-monitoring).

---

## GCP infra cost

### Steady-state expectation

`ligate-devnet-1`'s baseline:

| Resource | Quantity | $/mo |
|---|---|---|
| Sequencer VM (`e2-medium`) | 1 | ~25 USD |
| Persistent disk (150 GB SSD) | 1 | ~25 USD |
| Network egress (modest) | 10-50 GB/mo | < 5 USD |
| GCS storage (backups + snapshots) | 5-20 GB-mo | < 1 USD |

Target envelope: **$30-$50/month**. Anything above $200/mo is the "something's misconfigured" line — RED on the dashboard.

### Setup

GCP billing export to BigQuery is a one-time UI step per project:

1. **GCP Console** → **Billing** → **Billing export** → **BigQuery export**
2. Select the chain's BigQuery dataset (create one named `ligate_billing` if needed)
3. Enable "Detailed usage cost data" (the schema the dashboard queries)
4. Wait ~24h for the first export to land

Once data flows, Grafana's BigQuery datasource queries the export tables for the GCP panels.

### Budget alert

Set a GCP **Budget Alert** at the project level: $100/mo warning, $200/mo critical. Configure via:

```
GCP Console → Billing → Budgets & alerts → Create budget
```

Notifications fire on the project owner's email + a Pub/Sub topic (which we can wire into Slack via a Cloud Function if desired; deferred until the alert actually fires).

### Lookerstudio dashboard

Pin a Lookerstudio dashboard to the project for executive-level cost visibility (the Grafana dashboard is operator-facing):

1. Lookerstudio → New report → Connect data → BigQuery
2. Pick the `ligate_billing.gcp_billing_export_v1_*` table
3. Panels: monthly cost trend, cost by service, cost by SKU, projected month-end total
4. Share read-only with the team

This is the "where's my money going?" view; the Grafana dashboard is the "is this normal right now?" view.

---

## Why two cost layers?

TIA and GCP are entirely separate billing systems with entirely separate budgets:

- **TIA** is consumed per-blob from a finite wallet. Running out = chain stalls.
- **GCP** is consumed per resource-hour from a credit card. Running out = bill grows but service continues.

The risk profile differs:
- TIA risk is **operational** (chain stops); detect early via the warn alert + top up.
- GCP risk is **financial** (surprise bill); detect at month-end via the dashboard + budget alert.

Same dashboard, different priorities. The Grafana panels make this explicit by grouping into rows.

---

## What's deliberately NOT here

- **GCP IAM cost-control quotas.** Setting per-resource quotas to hard-cap spend is a separate engineering effort; pre-devnet, the budget alert + visibility is sufficient.
- **Per-tenant cost attribution.** Once partners deploy follower nodes, we may want to attribute their consumption back to them. Not in scope until that's a real concern.
- **Predictive cost models.** Forecasting the next month's spend from current trend. Useful but post-mainnet.

---

## Cross-references

- [Issue #277](https://github.com/ligate-io/ligate-chain/issues/277) — this work closes it
- [`monitoring/grafana/ligate-devnet-1-cost.json`](../../monitoring/grafana/ligate-devnet-1-cost.json) — the dashboard
- [`ops/prometheus/alerts.yaml`](../../ops/prometheus/alerts.yaml) `ligate-node.cost` group — the alert rules
- [`runbooks/da-signer-rotation.md`](runbooks/da-signer-rotation.md) — TIA wallet management procedure
- [`runbooks/alerts/`](runbooks/alerts/) — alert runbooks (Tia* alerts inherit those patterns)
- [Issue #234](https://github.com/ligate-io/ligate-chain/issues/234) — public Grafana host (where the dashboard ultimately renders)
