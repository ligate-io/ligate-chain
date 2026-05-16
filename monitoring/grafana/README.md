# Grafana dashboards (deployed)

Dashboards currently live on `ligate.grafana.net`. Source of truth for
the live dashboards is this directory — re-import from the JSON files
here to restore or replicate the views.

## Inventory

| File | UID | Public URL | Data source | Status |
|---|---|---|---|---|
| [`ligate-investor.json`](ligate-investor.json) | `ligate-investor` | [/d/ligate-investor](https://ligate.grafana.net/d/ligate-investor) | Infinity REST → `api.ligate.io` | **Live** |
| [`ligate-operator.json`](ligate-operator.json) | `ligate-operator` | [/d/ligate-operator](https://ligate.grafana.net/d/ligate-operator) | Infinity REST → `api.ligate.io` | **Live** |
| [`ligate-devnet-1-cost.json`](ligate-devnet-1-cost.json) | (varies) | — | GCP Cloud Billing (not yet wired) | Pending |

A separate Prometheus-based operator dashboard lives at
[`../../ops/grafana/ligate-node.json`](../../ops/grafana/ligate-node.json).
That one waits on the Prometheus scraper landing per
[ligate-chain#316](https://github.com/ligate-io/ligate-chain/issues/316).
Once the chain VM has a Grafana Agent pushing chain metrics to
`grafanacloud-ligate-prom`, that dashboard re-imports alongside the
two above.

## Investor dashboard

Public-facing key numbers. Refreshes every 30s, default time range
6h. Covers chain identity, cumulative totals (txs, wallets, schemas,
attestor sets, attestations), token supply + treasury balance,
activity trends (new wallets per day, tx rate per day by kind), top
10 LGT holders, DA finality percentiles (observed via the indexer's
re-poll loop per ligate-api `/v1/stats/finality`), and block cadence.

Designed to be the link partners + investors + the public see —
keep it factual + minimal, no Prometheus-only operator depth.

## Operator dashboard

Internal ops view. Refreshes every 10s, default time range 1h.
Covers live indexer-vs-chain-head state, block cadence (mean / p95
interval, seconds-since-last as block-stall signal), DA finality
observations, and recent activity. Calls out explicitly what's
missing (Prometheus-only metrics) with a link to the tracking issue.

Aimed at "is the chain healthy right now" rather than "deep system
forensics" — for the latter, the Prometheus operator dashboard
once it lands.

## Re-import procedure

If a dashboard is deleted or corrupted:

```bash
source ~/.config/ligate/secrets.env  # GRAFANA_URL + GRAFANA_TOKEN

DASHBOARD=ligate-investor
JSON_PATH=monitoring/grafana/$DASHBOARD.json

# Wrap the dashboard JSON in the import envelope
python3 -c "import json; d=json.load(open('$JSON_PATH')); print(json.dumps({'dashboard': d, 'overwrite': True, 'message': 'Re-import from repo'}))" > /tmp/import.json

curl -X POST -H "Authorization: Bearer $GRAFANA_TOKEN" \
  -H "Content-Type: application/json" \
  "$GRAFANA_URL/api/dashboards/db" \
  -d @/tmp/import.json
```

## Updating a dashboard

Edit the dashboard in the Grafana UI, then export the canonical JSON
back to this repo:

```bash
source ~/.config/ligate/secrets.env
UID=ligate-investor
curl -s -H "Authorization: Bearer $GRAFANA_TOKEN" \
  "$GRAFANA_URL/api/dashboards/uid/$UID" | \
  python3 -c "import sys,json; d=json.load(sys.stdin); print(json.dumps(d['dashboard'], indent=2, sort_keys=True))" \
  > monitoring/grafana/$UID.json
git diff monitoring/grafana/$UID.json   # review the change
```

Commit the resulting JSON so the repo stays the source of truth.
