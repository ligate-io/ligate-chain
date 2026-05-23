#!/usr/bin/env bash
# Refresh the GCP cost JSON sidecar files consumed by Grafana.
#
# Runs as a systemd oneshot fired every 6h by `gcp-cost.timer` on
# VM-1. Each query in this script writes one JSON file under
# `/var/www/cost/`, which Caddy serves at `https://rpc.ligate.io/cost/`
# for the Grafana Infinity HTTP-JSON datasource to consume.
#
# Why this lives in ops/billing/ and not /opt/ligate/bin/:
#   The VM-1 copy at /opt/ligate/bin/gcp-cost-fetch.sh is the live
#   one; this is the source-of-truth tracked in git. Cloud-init can
#   copy this file into place on a fresh VM; manual VM-1 updates use
#   `gcloud compute scp` + `sudo install`.
#
# Files written (consumed by panels in ops/grafana/ligate-node.json):
#   /var/www/cost/daily.json      -- rolling 30-day daily total
#   /var/www/cost/by-service.json -- last 30 days, grouped by service
#   /var/www/cost/by-vm.json      -- last 7 days, grouped by Compute Engine resource name
#   /var/www/cost/mtd.json        -- month-to-date single-cell stat
#
# Tracking: chain#392.

set -euo pipefail

PROJECT_ID="${PROJECT_ID:-utopian-spring-494915-q1}"
DATASET="${DATASET:-ligate_billing}"
TABLE_PREFIX="${TABLE_PREFIX:-gcp_billing_export_resource_v1_*}"
OUTPUT_DIR="${OUTPUT_DIR:-/var/www/cost}"

TABLE="\`${PROJECT_ID}.${DATASET}.${TABLE_PREFIX}\`"

mkdir -p "$OUTPUT_DIR"

# Atomic write: render to a temp file, then `mv` into place. Caddy
# never sees a half-written response.
run_query() {
    local label="$1" sql="$2" output_path="$3"
    echo "==> ${label}: ${output_path}"
    local tmp
    tmp=$(mktemp)
    bq query --use_legacy_sql=false --format=json --max_rows=200 "$sql" > "$tmp"
    mv "$tmp" "$output_path"
    chmod 644 "$output_path"
}

# 1. Daily aggregate, rolling 30-day window.
#    Time series panel: x=day, y=cost_usd.
run_query "daily (last 30d)" "
    SELECT TIMESTAMP_TRUNC(usage_start_time, DAY) AS day,
           SUM(cost) AS cost_usd
    FROM ${TABLE}
    WHERE usage_start_time >= TIMESTAMP_SUB(CURRENT_TIMESTAMP(), INTERVAL 30 DAY)
    GROUP BY day
    ORDER BY day
" "${OUTPUT_DIR}/daily.json"

# 2. Cost by service, rolling 30-day window.
#    Bar panel: x=service, y=cost_usd. Sorted high-to-low so the
#    top spend is leftmost.
run_query "by-service (last 30d)" "
    SELECT service.description AS service,
           SUM(cost) AS cost_usd
    FROM ${TABLE}
    WHERE usage_start_time >= TIMESTAMP_SUB(CURRENT_TIMESTAMP(), INTERVAL 30 DAY)
    GROUP BY service
    ORDER BY cost_usd DESC
" "${OUTPUT_DIR}/by-service.json"

# 3. Cost by VM (resource.name), last 7d, Compute Engine only.
#    Bar panel: x=vm, y=cost_usd. Filtered to Compute Engine so we
#    aren't mixing in BigQuery/Cloud Storage/etc. resources which
#    aren't VMs.
run_query "by-vm (last 7d)" "
    SELECT resource.name AS vm,
           SUM(cost) AS cost_usd
    FROM ${TABLE}
    WHERE usage_start_time >= TIMESTAMP_SUB(CURRENT_TIMESTAMP(), INTERVAL 7 DAY)
      AND service.description = 'Compute Engine'
      AND resource.name IS NOT NULL
    GROUP BY vm
    ORDER BY cost_usd DESC
" "${OUTPUT_DIR}/by-vm.json"

# 4. Month-to-date total, single number.
#    Stat panel showing one big USD value. Uses calendar-month
#    boundary (DATE_TRUNC) so on the 1st of the month this snaps
#    back to zero, then climbs over the month.
run_query "mtd (calendar-month)" "
    SELECT SUM(cost) AS cost_usd
    FROM ${TABLE}
    WHERE usage_start_time >= TIMESTAMP(DATE_TRUNC(CURRENT_DATE(), MONTH))
" "${OUTPUT_DIR}/mtd.json"

echo "==> done"
