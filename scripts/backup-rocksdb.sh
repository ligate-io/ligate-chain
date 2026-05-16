#!/usr/bin/env bash
# rsync ligate-node's RocksDB storage to a GCS bucket.
#
# Designed to run from a systemd timer on the chain VM. Writes to a
# date-keyed prefix so retention (managed by GCS lifecycle policy +
# the rotation logic below) can keep recent snapshots dense and
# older snapshots sparse.
#
# Flow:
#   1. Take a consistent point-in-time snapshot of the storage
#      directory by `rsync --link-dest` against the previous local
#      snapshot. Cheap (hardlinks unchanged files) and atomic enough
#      for our purposes — ligate-node uses RocksDB's WAL so partial
#      writes don't corrupt the on-disk state.
#   2. Upload the snapshot dir to `gs://$GCS_BUCKET/<host>/<timestamp>/`
#      via `gcloud storage cp -r`. Idempotent on re-runs.
#   3. Apply rotation: keep last N=24 hourly, last 7 daily, last 4
#      weekly. Older snapshots are removed via GCS lifecycle policy
#      configured separately; this script writes the tagged prefixes
#      the policy keys off.
#
# Tracking issue: ligate-io/ligate-chain#267.

set -euo pipefail

# ----- Config (env-overridable) ------------------------------------
: "${LIGATE_STORAGE_DIR:=/var/lib/ligate/rollup}"
: "${LIGATE_SNAPSHOT_DIR:=/var/lib/ligate/snapshots}"
: "${GCS_BUCKET:?GCS_BUCKET must be set (e.g. ligate-devnet-1-backups)}"
: "${HOSTNAME:=$(hostname -s)}"

# Snapshot tag — hour for routine runs, day for the daily-keep tier,
# week for the weekly-keep tier. Daily and weekly tiers are written
# by the same script on its respective trigger (the systemd timer
# config lives in `ops/backup/systemd/`).
: "${TIER:=hourly}"

case "$TIER" in
    hourly)  TIMESTAMP="$(date -u +%Y%m%dT%H0000Z)" ;;
    daily)   TIMESTAMP="$(date -u +%Y%m%d)" ;;
    weekly)  TIMESTAMP="$(date -u +%Y-W%V)" ;;
    *) echo "TIER must be hourly|daily|weekly (got: $TIER)" >&2; exit 2 ;;
esac

LOCAL_SNAPSHOT="${LIGATE_SNAPSHOT_DIR}/${TIER}/${TIMESTAMP}"
GCS_PREFIX="gs://${GCS_BUCKET}/${HOSTNAME}/${TIER}/${TIMESTAMP}"

mkdir -p "${LIGATE_SNAPSHOT_DIR}/${TIER}"

# ----- Local snapshot via rsync ------------------------------------
#
# `--link-dest` against the previous snapshot makes unchanged files
# hardlinks (cheap, no disk space). `-a` preserves perms / symlinks.
# `--delete` keeps the snapshot tidy if a file disappeared upstream.
PREVIOUS_SNAPSHOT="$(ls -dt "${LIGATE_SNAPSHOT_DIR}/${TIER}"/* 2>/dev/null | head -1 || true)"

# `--sparse` preserves sparseness on copy. RocksDB NOMT files contain
# many sparse holes; without --sparse, rsync fills them with zeros on
# the destination, inflating each snapshot ~4x on disk for nothing.
# Tested: 1.2 GB physical → 5 GB after copy without --sparse,
# vs 1.2 GB → 1.2 GB with --sparse.
echo "==> Local snapshot: ${LOCAL_SNAPSHOT}"
if [ -n "${PREVIOUS_SNAPSHOT}" ] && [ "${PREVIOUS_SNAPSHOT}" != "${LOCAL_SNAPSHOT}" ]; then
    echo "    using --link-dest=${PREVIOUS_SNAPSHOT}"
    rsync -a --sparse --delete --link-dest="${PREVIOUS_SNAPSHOT}" \
          "${LIGATE_STORAGE_DIR}/" "${LOCAL_SNAPSHOT}/"
else
    rsync -a --sparse --delete "${LIGATE_STORAGE_DIR}/" "${LOCAL_SNAPSHOT}/"
fi

# Write a manifest alongside so restore can verify checksum + height.
HEIGHT_FILE="${LOCAL_SNAPSHOT}/.snapshot-manifest.json"
# Buffer-then-parse to avoid `set -o pipefail` + `awk … exit` SIGPIPE-ing
# the writer of the pipe, which previously made HEIGHT capture BOTH
# awk's match AND the `|| echo "unknown"` fallback (the manifest's
# block_height field would contain a literal newline: `"15187\nunknown"`).
# `<<<` here-string feeds awk via a temp file, not a pipe — no SIGPIPE
# possible even when awk exits before the writer finishes.
METRICS="$(curl -sf http://127.0.0.1:9100/metrics 2>/dev/null || true)"
HEIGHT="$(awk '/^ligate_block_height /{print $2; exit}' <<< "${METRICS}")"
[ -z "${HEIGHT}" ] && HEIGHT="unknown"
{
    echo "{"
    echo "  \"tier\": \"${TIER}\","
    echo "  \"timestamp\": \"${TIMESTAMP}\","
    echo "  \"hostname\": \"${HOSTNAME}\","
    echo "  \"storage_path\": \"${LIGATE_STORAGE_DIR}\","
    echo "  \"block_height\": \"${HEIGHT}\","
    echo "  \"created_at_utc\": \"$(date -u +%Y-%m-%dT%H:%M:%SZ)\""
    echo "}"
} > "${HEIGHT_FILE}"

# ----- Upload to GCS -----------------------------------------------
#
# `gcloud storage cp -r` is parallel + resumable. The destination
# prefix includes the tier + timestamp so restore can find a specific
# snapshot deterministically.
echo "==> Uploading to ${GCS_PREFIX}"
gcloud storage cp -r "${LOCAL_SNAPSHOT}/" "${GCS_PREFIX}/" --quiet

echo "==> Done. Snapshot ${TIER}/${TIMESTAMP} at height ${HEIGHT} uploaded."

# ----- Local rotation (keep last N per tier) -----------------------
#
# GCS-side rotation is via lifecycle policy (see
# `ops/backup/gcs-lifecycle.json`). Local rotation keeps disk usage
# bounded; tune the counts per tier as devnet usage stabilises.
case "$TIER" in
    hourly)  KEEP=24 ;;
    daily)   KEEP=7 ;;
    weekly)  KEEP=4 ;;
esac

echo "==> Pruning local ${TIER} snapshots; keeping last ${KEEP}"
ls -dt "${LIGATE_SNAPSHOT_DIR}/${TIER}"/* 2>/dev/null \
    | tail -n +"$((KEEP + 1))" \
    | xargs -r rm -rf

echo "==> Backup complete."
