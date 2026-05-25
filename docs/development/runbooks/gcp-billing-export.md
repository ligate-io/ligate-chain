# GCP infra cost into Grafana

> One-time setup for `ligate-devnet-2` ops. Routes GCP Cloud Billing
> data into a BigQuery dataset, then exposes that dataset to Grafana
> as a second datasource alongside the existing Prometheus one. After
> setup, the chain Grafana dashboard can show a "Monthly burn rate"
> alongside `ligate_da_tia_burned_nano_estimate_total` (TIA spend) and
> `ligate_protocol_treasury_balance_nano` (AVOW on treasury) for full
> cost + economy visibility.

## Why this lives outside `ligate-node`

GCP-specific cost telemetry doesn't belong in the chain binary. The
chain has no idea what VM type it's running on, what Cloud SQL tier
we're paying for, or what egress we're racking up at Cloudflare's
edge. That data lives in GCP's Cloud Billing BigQuery export and is
operator-side. The chain binary stays portable — if someone runs
`ligate-node` on bare metal or AWS, they don't carry GCP code along
for the ride.

The relevant chain-side cost metric — estimated TIA burned on Celestia
DA — IS in `ligate-node` (`ligate_da_tia_burned_nano_estimate_total`,
chain v0.2.11). That number is the chain's own ledger; GCP billing
is the substrate underneath.

## What gets set up

```
GCP Cloud Billing
       │
       ▼  (daily export, ~24h lag for first row)
BigQuery dataset: `ligate_billing.gcp_billing_export_v1_*`
       │
       ▼  (Grafana BigQuery datasource queries this)
Grafana panel: "GCP daily burn (USD)" in the Cost & economy row
```

The export is a managed Google feature — no exporter sidecar to
maintain, no Prometheus job to scrape. BigQuery holds the data;
Grafana queries on demand via the BigQuery plugin.

## Prerequisites

- GCP project: `utopian-spring-494915-q1`
- Billing account associated with that project (find via Cloud Console
  → Billing or `gcloud beta billing projects describe utopian-spring-494915-q1`)
- IAM role on the billing account: `roles/billing.admin` for the
  operator running this setup
- Grafana instance with the BigQuery datasource plugin installed
  (Grafana Cloud has it by default; self-hosted needs
  `grafana-bigquery-datasource` plugin installation)

## Step 1: Create a BigQuery dataset for the export

```bash
# Find the project number; needed for the BigQuery dataset's IAM grant.
PROJECT_ID=utopian-spring-494915-q1
PROJECT_NUMBER=$(gcloud projects describe "$PROJECT_ID" --format='value(projectNumber)')

# Create the dataset. Location: keep in the same region as the rest of
# devnet-1 (us-central1) to avoid cross-region BigQuery egress fees on
# the export itself.
bq --location=us-central1 mk \
  --dataset \
  --description "GCP billing export for ligate-devnet-2 cost telemetry" \
  "${PROJECT_ID}:ligate_billing"
```

## Step 2: Enable the export in Cloud Billing

There is **no CLI** for enabling the export — it's a Console step.

1. Open [Cloud Console → Billing → Billing export](https://console.cloud.google.com/billing/_/manage/export)
2. Switch to the billing account associated with `utopian-spring-494915-q1`
3. Under "Detailed usage cost" (the one with per-resource SKU granularity, more useful than "Standard"), click **EDIT SETTINGS**
4. Project: `utopian-spring-494915-q1`
5. Dataset: `ligate_billing` (the one created in Step 1)
6. Save

Standard vs Detailed export choice:

- **Standard** — daily aggregates, lower granularity, smaller dataset. Sufficient for monthly burn-rate panels.
- **Detailed** — per-SKU-per-resource per-day, large dataset (the cost surface itself adds non-zero BigQuery storage cost), needed for "which VM cost the most this month" breakdowns.

For devnet-1, **Detailed** is fine — the dataset will be small at this scale. Revisit if BigQuery storage cost grows past meaningful.

## Step 3: Wait

Google has a documented **24 to 48 hour lag** for the first export row
to appear after enablement. Plan to come back tomorrow before
configuring Grafana. Until then `bq ls ligate_billing` returns empty
or a table with zero rows.

You can verify the export is wired (just not populated yet) by
checking the table exists once it does:

```bash
bq ls ligate_billing
# expected eventually: gcp_billing_export_resource_v1_<billing_account_id>
```

## Step 4: Wire Grafana (via the VM-1 sidecar)

The original draft of this runbook used Grafana's Google BigQuery datasource with a service-account JSON key. **That path is blocked** by the `constraints/iam.disableServiceAccountKeyCreation` org policy on this project (a good security default — long-lived SA keys are a common exfiltration target). Workload Identity Federation would also work, but adds 30 to 60 minutes of GCP + Grafana setup for a single panel.

The actual pattern we ship: **VM-1 runs a 6-hourly bq query, writes the result JSON, Caddy serves it, Grafana reads it via the Infinity HTTP datasource.** No key file, no WIF, no Grafana plugin install beyond the Infinity one Grafana Cloud already provides.

### 4a. Grant VM-1's compute SA bigquery read perms

```bash
PROJECT_ID=utopian-spring-494915-q1
VM1_SA=$(gcloud compute instances describe ligate-devnet-2-sequencer \
  --zone=us-central1-a --format="value(serviceAccounts[0].email)")
gcloud projects add-iam-policy-binding "$PROJECT_ID" \
  --member="serviceAccount:$VM1_SA" --role="roles/bigquery.dataViewer" --condition=None
gcloud projects add-iam-policy-binding "$PROJECT_ID" \
  --member="serviceAccount:$VM1_SA" --role="roles/bigquery.jobUser" --condition=None
```

### 4b. Change VM-1's OAuth scopes to `cloud-platform`

Default compute VMs have a restricted scope set that does **not** include BigQuery. Even with the IAM roles above, `bq query` from the VM will return `Insufficient Permission` until you broaden the scopes. This requires a stop / scopes-change / start cycle (~6 min of public RPC downtime; same as a machine-type resize).

```bash
gcloud compute instances stop ligate-devnet-2-sequencer --zone=us-central1-a
gcloud compute instances set-service-account ligate-devnet-2-sequencer \
  --zone=us-central1-a \
  --service-account="<vm1-sa-from-4a>" \
  --scopes="https://www.googleapis.com/auth/cloud-platform"
gcloud compute instances start ligate-devnet-2-sequencer --zone=us-central1-a
```

Verify after restart:

```bash
gcloud compute ssh ligate-devnet-2-sequencer --zone=us-central1-a \
  --command='bq query --use_legacy_sql=false --format=json "SELECT 1 AS ok"'
# expected: [{"ok":"1"}]
```

### 4c. Install the bq fetcher script + systemd timer on VM-1

`/opt/ligate/bin/gcp-cost-fetch.sh`:

```bash
#!/bin/bash
set -e
PROJECT_ID=utopian-spring-494915-q1
OUTPUT=/var/www/cost/daily.json
TMP=$(mktemp)
bq query --use_legacy_sql=false --format=json --max_rows=200 \
  "SELECT TIMESTAMP_TRUNC(usage_start_time, DAY) AS day, SUM(cost) AS cost_usd \
   FROM \`${PROJECT_ID}.ligate_billing.gcp_billing_export_resource_v1_*\` \
   WHERE usage_start_time >= TIMESTAMP_SUB(CURRENT_TIMESTAMP(), INTERVAL 30 DAY) \
   GROUP BY day ORDER BY day" > "$TMP"
mv "$TMP" "$OUTPUT"
chmod 644 "$OUTPUT"
```

Make it executable, ensure `/var/www/cost/` exists and is `caddy:caddy`-owned, then drop in:

`/etc/systemd/system/gcp-cost.service` (oneshot wrapping the script) + `/etc/systemd/system/gcp-cost.timer` (`OnBootSec=2min`, `OnUnitActiveSec=6h`). `systemctl enable --now gcp-cost.timer`.

### 4d. Expose the JSON via Caddy

Add to `ops/caddy/Caddyfile` inside the `rpc.ligate.io` site block (just after the `/health` + `/ready` handles):

```caddy
# GCP daily billing JSON for Grafana (chain#453). Refreshed every 6h
# by gcp-cost.timer; reads BigQuery via VM-1 compute SA. Public
# because the data is aggregate cost only (no PII, no secrets).
handle_path /cost/* {
    root * /var/www/cost
    file_server
}
```

`systemctl reload caddy`. Verify:

```bash
curl https://rpc.ligate.io/cost/daily.json
# expected: [] initially, then the bq result rows once the 24h export lag clears
```

### 4e. Add the Grafana panel via Infinity datasource

In Grafana Cloud (Infinity is pre-installed):

1. Add data source: **Infinity** if not already added. No auth needed (the URL is public-readable aggregate cost).
2. Add panel:
   - Type: Time series
   - Datasource: Infinity
   - Type: JSON, Source: URL, URL: `https://rpc.ligate.io/cost/daily.json`
   - Root selector: leave blank (response IS the array)
   - Columns: `day` (timestamp), `cost_usd` (number)
   - Unit: `currencyUSD`
   - Title: "GCP daily burn (USD)"
   - Position: under the **Cost & economy** row in the `ligate-node` dashboard

The panel renders empty until BigQuery populates (~24 to 48h after Step 2's Console enable). Don't worry about the empty state until then.

## Step 5: Extra break-down panels (chain#392)

The original v1 fetch script only wrote `daily.json`. As of chain#392 the source-of-truth script at [`ops/billing/gcp-cost-fetch.sh`](../../../ops/billing/gcp-cost-fetch.sh) runs four queries on every timer fire and writes:

| File | Window | Used by Grafana panel |
|---|---|---|
| `daily.json` | last 30 days | "GCP daily burn (USD, last 30d)" (time series) |
| `by-service.json` | last 30 days | "GCP spend by service (last 30d)" (bar gauge) |
| `by-vm.json` | last 7 days, Compute Engine only | "Compute Engine spend by VM (last 7d)" (bar gauge) |
| `mtd.json` | calendar month-to-date | "GCP month-to-date burn (USD)" (stat) |

All four panels live in the dashboard's **Cost & economy** row and consume the same Infinity HTTP-JSON datasource. Caddy serves the whole `/cost/` directory with one `handle_path` block (no per-file routing needed); the four URLs just resolve to the four JSON files.

To deploy the updated script on VM-1:

```bash
gcloud compute scp ops/billing/gcp-cost-fetch.sh \
    ligate-devnet-2-sequencer:/tmp/gcp-cost-fetch.sh \
    --zone=us-central1-a

gcloud compute ssh ligate-devnet-2-sequencer --zone=us-central1-a --command='
    sudo install -m 0755 -o ligate -g ligate \
        /tmp/gcp-cost-fetch.sh /opt/ligate/bin/gcp-cost-fetch.sh

    # Fire once now instead of waiting for the next 6h tick
    sudo systemctl start gcp-cost.service
'
```

Then re-import `ops/grafana/ligate-node.json` in Grafana to pick up the new panels.

### Adding even more breakdowns later

The script is structured so each query is its own `run_query` call. Add new ones the same shape, point them at a new `/var/www/cost/<name>.json`, then add a matching Infinity-datasource panel in `ligate-node.json`.

## Cost of the export itself

BigQuery storage at devnet-1 scale: cents per month. Query cost: each
on-demand query scans the table; at this dataset size that's
fractions of a cent. Detailed export costs more storage than
Standard but still trivial for a single-project devnet.

## Rollback

To disable the export:

1. Cloud Console → Billing → Billing export → EDIT SETTINGS → disable
2. Optionally delete the dataset: `bq rm -r -f utopian-spring-494915-q1:ligate_billing`
3. Delete the Grafana datasource

The chain Prometheus metrics (`ligate_da_*`, `ligate_protocol_*`) keep working independently — they're sourced from the chain, not from BigQuery.

## Related

- [chain v0.2.11 CHANGELOG entry](../../CHANGELOG.md) — the chain-side cost + economy metrics this complements
- [`ops/grafana/ligate-node.json`](../../../ops/grafana/ligate-node.json) — dashboard where the new panel lands
- GCP docs: <https://cloud.google.com/billing/docs/how-to/export-data-bigquery>
