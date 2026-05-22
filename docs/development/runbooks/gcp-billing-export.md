# GCP infra cost into Grafana

> One-time setup for `ligate-devnet-1` ops. Routes GCP Cloud Billing
> data into a BigQuery dataset, then exposes that dataset to Grafana
> as a second datasource alongside the existing Prometheus one. After
> setup, the chain Grafana dashboard can show a "Monthly burn rate"
> alongside `ligate_da_tia_burned_nano_estimate_total` (TIA spend) and
> `ligate_protocol_treasury_balance_nano` (LGT on treasury) for full
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
  --description "GCP billing export for ligate-devnet-1 cost telemetry" \
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

## Step 4: Wire Grafana

Once the table has data:

1. Grafana → Connections → Data sources → Add data source → **Google BigQuery**
2. Authentication: GCP service account JSON key. Create the key:

   ```bash
   gcloud iam service-accounts create grafana-bigquery \
     --project="${PROJECT_ID}" \
     --display-name="Grafana BigQuery read-only"

   gcloud projects add-iam-policy-binding "${PROJECT_ID}" \
     --member="serviceAccount:grafana-bigquery@${PROJECT_ID}.iam.gserviceaccount.com" \
     --role="roles/bigquery.dataViewer"

   gcloud projects add-iam-policy-binding "${PROJECT_ID}" \
     --member="serviceAccount:grafana-bigquery@${PROJECT_ID}.iam.gserviceaccount.com" \
     --role="roles/bigquery.jobUser"

   gcloud iam service-accounts keys create grafana-bigquery-key.json \
     --iam-account="grafana-bigquery@${PROJECT_ID}.iam.gserviceaccount.com"
   ```

   Paste the key contents into Grafana's BigQuery datasource form. Delete the local key file afterwards (it gives full read access to the billing data).

3. Default project: `utopian-spring-494915-q1`
4. Save & Test — Grafana queries the dataset to verify connectivity.

## Step 5: Add the panel to the chain dashboard

Add a new panel to `ops/grafana/ligate-node.json` (or directly in the Grafana UI). Suggested query:

```sql
SELECT
  TIMESTAMP_TRUNC(usage_start_time, DAY) AS day,
  SUM(cost) AS cost_usd
FROM `utopian-spring-494915-q1.ligate_billing.gcp_billing_export_resource_v1_*`
WHERE usage_start_time >= TIMESTAMP_SUB(CURRENT_TIMESTAMP(), INTERVAL 30 DAY)
GROUP BY day
ORDER BY day
```

Panel type: **Time series**, x = day, y = cost_usd, unit = `currencyUSD`.
Title: "GCP daily burn (USD)". Position: under the Cost & economy row in the chain dashboard, or in a new "GCP infra" row if you want clear separation.

Useful complementary queries for break-down panels:

```sql
-- Cost by service over the last 30d
SELECT service.description AS service, SUM(cost) AS cost
FROM `utopian-spring-494915-q1.ligate_billing.gcp_billing_export_resource_v1_*`
WHERE usage_start_time >= TIMESTAMP_SUB(CURRENT_TIMESTAMP(), INTERVAL 30 DAY)
GROUP BY service ORDER BY cost DESC
```

```sql
-- Cost by VM (resource name) over the last 7d
SELECT resource.name AS vm, SUM(cost) AS cost
FROM `utopian-spring-494915-q1.ligate_billing.gcp_billing_export_resource_v1_*`
WHERE usage_start_time >= TIMESTAMP_SUB(CURRENT_TIMESTAMP(), INTERVAL 7 DAY)
  AND service.description = 'Compute Engine'
GROUP BY vm ORDER BY cost DESC
```

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
