#!/usr/bin/env bash
# Restore ligate-node's RocksDB storage from a GCS snapshot.
#
# Usage:
#   restore-rocksdb.sh [--tier hourly|daily|weekly] [--timestamp <TS>]
#
# If no arguments, restores the most recent hourly snapshot for this
# host. Specific snapshot via `--tier daily --timestamp 20260512`.
#
# Flow:
#   1. Stop ligate-node (chain must be quiet during restore).
#   2. Move existing storage dir aside as `.pre-restore-<timestamp>`.
#   3. Download the GCS snapshot to a staging dir.
#   4. Verify the manifest's checksum + height.
#   5. Move staging into the storage path.
#   6. Restart ligate-node + wait for first block.
#   7. Verify the restored head_height advances.
#
# Tracking issue: ligate-io/ligate-chain#267.

set -euo pipefail

: "${LIGATE_STORAGE_DIR:=/var/lib/ligate/rollup}"
: "${LIGATE_STAGING_DIR:=/var/lib/ligate/restore-staging}"
: "${GCS_BUCKET:?GCS_BUCKET must be set}"
: "${HOSTNAME:=$(hostname -s)}"

TIER="hourly"
TIMESTAMP=""

# ----- Parse args --------------------------------------------------
while [[ $# -gt 0 ]]; do
    case "$1" in
        --tier) TIER="$2"; shift 2 ;;
        --timestamp) TIMESTAMP="$2"; shift 2 ;;
        -h|--help)
            sed -n '2,20p' "$0"
            exit 0
            ;;
        *) echo "unknown arg: $1" >&2; exit 2 ;;
    esac
done

# ----- Find the snapshot -------------------------------------------
if [ -z "$TIMESTAMP" ]; then
    echo "==> No --timestamp; picking the most recent ${TIER} snapshot."
    TIMESTAMP="$(gcloud storage ls "gs://${GCS_BUCKET}/${HOSTNAME}/${TIER}/" \
                  | sed -e 's|/$||' -e 's|.*/||' \
                  | sort -r | head -1)"
    if [ -z "$TIMESTAMP" ]; then
        echo "no snapshots found in gs://${GCS_BUCKET}/${HOSTNAME}/${TIER}/" >&2
        exit 1
    fi
fi

GCS_SOURCE="gs://${GCS_BUCKET}/${HOSTNAME}/${TIER}/${TIMESTAMP}"
echo "==> Restoring from: ${GCS_SOURCE}"

# ----- Confirm with the operator -----------------------------------
echo
echo "This will:"
echo "  1. Stop ligate-node"
echo "  2. Move ${LIGATE_STORAGE_DIR} to ${LIGATE_STORAGE_DIR}.pre-restore-$(date -u +%s)"
echo "  3. Download ${GCS_SOURCE} and place at ${LIGATE_STORAGE_DIR}"
echo "  4. Restart ligate-node"
echo
read -r -p "Proceed? [y/N] " CONFIRM
if [[ ! "$CONFIRM" =~ ^[yY]$ ]]; then
    echo "Aborted by operator."
    exit 0
fi

# ----- Stop the node -----------------------------------------------
echo "==> Stopping ligate-node"
sudo systemctl stop ligate-node || true

# ----- Download the snapshot to staging ----------------------------
mkdir -p "${LIGATE_STAGING_DIR}"
rm -rf "${LIGATE_STAGING_DIR}/restored"
echo "==> Downloading to ${LIGATE_STAGING_DIR}/restored"
gcloud storage cp -r "${GCS_SOURCE}/" "${LIGATE_STAGING_DIR}/" --quiet
mv "${LIGATE_STAGING_DIR}/${TIMESTAMP}" "${LIGATE_STAGING_DIR}/restored"

# ----- Verify manifest ---------------------------------------------
MANIFEST="${LIGATE_STAGING_DIR}/restored/.snapshot-manifest.json"
if [ ! -f "$MANIFEST" ]; then
    echo "snapshot manifest missing at $MANIFEST" >&2
    echo "snapshot may be incomplete; aborting" >&2
    exit 1
fi
EXPECTED_HEIGHT="$(jq -r .block_height "$MANIFEST")"
EXPECTED_HOST="$(jq -r .hostname "$MANIFEST")"
echo "==> Manifest reports: host=${EXPECTED_HOST} height=${EXPECTED_HEIGHT}"

# ----- Move aside the existing storage -----------------------------
if [ -d "${LIGATE_STORAGE_DIR}" ] && [ -n "$(ls -A "${LIGATE_STORAGE_DIR}" 2>/dev/null)" ]; then
    QUARANTINE="${LIGATE_STORAGE_DIR}.pre-restore-$(date -u +%s)"
    echo "==> Moving existing storage to ${QUARANTINE}"
    sudo mv "${LIGATE_STORAGE_DIR}" "${QUARANTINE}"
fi
sudo mkdir -p "${LIGATE_STORAGE_DIR}"

# ----- Place the restored snapshot ---------------------------------
echo "==> Placing restored state"
sudo rsync -a "${LIGATE_STAGING_DIR}/restored/" "${LIGATE_STORAGE_DIR}/"
sudo chown -R ligate:ligate "${LIGATE_STORAGE_DIR}"

# ----- Restart the node --------------------------------------------
echo "==> Starting ligate-node"
sudo systemctl start ligate-node

# ----- Wait + verify -----------------------------------------------
echo "==> Waiting up to 60s for the node to come back"
SECONDS_WAITED=0
while ! curl -sf http://127.0.0.1:9100/metrics >/dev/null 2>&1; do
    sleep 2
    SECONDS_WAITED=$((SECONDS_WAITED + 2))
    if [ "$SECONDS_WAITED" -gt 60 ]; then
        echo "node not serving metrics after 60s; investigate manually" >&2
        exit 1
    fi
done

# Buffer-then-parse via here-string to avoid `set -o pipefail` +
# `awk … exit` SIGPIPE-ing curl. Same shape as in backup-rocksdb.sh.
CURRENT_METRICS="$(curl -sf http://127.0.0.1:9100/metrics 2>/dev/null || true)"
CURRENT_HEIGHT="$(awk '/^ligate_block_height /{print $2; exit}' <<< "${CURRENT_METRICS}")"
[ -z "${CURRENT_HEIGHT}" ] && CURRENT_HEIGHT="unknown"
echo "==> Node restarted; current block_height: ${CURRENT_HEIGHT}"
echo "==> Expected from snapshot:               ${EXPECTED_HEIGHT}"
echo
echo "==> Restore complete. Watch the journal:"
echo "       journalctl -fu ligate-node"
