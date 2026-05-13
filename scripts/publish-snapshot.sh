#!/usr/bin/env bash
# Publish a state snapshot to the PUBLIC snapshot bucket so a new
# follower can bootstrap from height N rather than replaying DA from
# height 0.
#
# Distinct from `backup-rocksdb.sh`:
# - Different bucket (public-read, vs operator-private for backups)
# - Different cadence (daily, vs every 15 min for ops backups)
# - Different retention (1 daily + 1 weekly + 1 monthly, vs the
#   dense-recent backup curve)
# - Different consumer (new follower joining, vs operator restoring
#   from disk loss)
#
# Flow:
#   1. Pause `ligate-node` momentarily (or use a hot snapshot — see
#      `--hot` flag) to capture a consistent state.
#   2. tar.gz the storage directory.
#   3. Compute sha256.
#   4. Write manifest with height + state_root + sha256 + chain_id.
#   5. Upload tarball + manifest to `gs://$PUBLIC_BUCKET/snapshots/<chain_id>/<height>/`.
#   6. Update the `latest.json` pointer.
#   7. (Optional) Restart `ligate-node`.
#
# Tracking issue: ligate-io/ligate-chain#273.

set -euo pipefail

: "${LIGATE_STORAGE_DIR:=/var/lib/ligate/rollup}"
: "${LIGATE_RPC:=http://127.0.0.1:12346}"
: "${PUBLIC_BUCKET:?PUBLIC_BUCKET must be set (e.g. ligate-snapshots-public)}"
: "${STAGING_DIR:=/var/lib/ligate/snapshot-staging}"

HOT_MODE="false"
TIER="daily"

while [[ $# -gt 0 ]]; do
    case "$1" in
        --hot)
            # Hot snapshot: don't stop ligate-node. Uses rsync's
            # eventual-consistency window; RocksDB's WAL keeps the
            # snapshot recoverable but means the captured height
            # might be slightly behind chain head at capture time.
            HOT_MODE="true"
            shift
            ;;
        --tier)
            TIER="$2"
            shift 2
            ;;
        -h|--help)
            sed -n '2,30p' "$0"
            exit 0
            ;;
        *) echo "unknown arg: $1" >&2; exit 2 ;;
    esac
done

case "$TIER" in
    daily|weekly|monthly) ;;
    *) echo "TIER must be daily|weekly|monthly (got: $TIER)" >&2; exit 2 ;;
esac

# ----- Read chain identity -----------------------------------------
CHAIN_ID="$(curl -sf "${LIGATE_RPC}/v1/info" | jq -r .chain_id)"
HEIGHT_BEFORE="$(curl -sf "${LIGATE_RPC}/v1/ledger/slots/latest" | jq -r .number)"
if [ -z "$CHAIN_ID" ] || [ -z "$HEIGHT_BEFORE" ]; then
    echo "could not read chain identity from $LIGATE_RPC" >&2
    exit 1
fi
echo "==> Chain: ${CHAIN_ID}, head: ${HEIGHT_BEFORE}, tier: ${TIER}"

# ----- Pause (or not) for the snapshot -----------------------------
if [ "${HOT_MODE}" = "false" ]; then
    echo "==> Stopping ligate-node for a consistent cold snapshot"
    sudo systemctl stop ligate-node
fi

mkdir -p "${STAGING_DIR}"
SNAPSHOT_NAME="ligate-snapshot-${CHAIN_ID}-${HEIGHT_BEFORE}-${TIER}.tar.gz"
TARBALL="${STAGING_DIR}/${SNAPSHOT_NAME}"

# ----- Tarball the storage dir -------------------------------------
echo "==> Archiving ${LIGATE_STORAGE_DIR} -> ${TARBALL}"
# `-C` lets us tar the dir without absolute paths inside the archive
# (so restore can `tar -xzf` into any storage path).
sudo tar -czf "${TARBALL}" -C "$(dirname "${LIGATE_STORAGE_DIR}")" \
    "$(basename "${LIGATE_STORAGE_DIR}")"
sudo chown ligate:ligate "${TARBALL}"

# ----- Hash + manifest ---------------------------------------------
echo "==> Computing sha256"
SHA256="$(sha256sum "${TARBALL}" | awk '{print $1}')"
SIZE_BYTES="$(stat -c %s "${TARBALL}")"

# Get the state root for the captured height.
STATE_ROOT="$(curl -sf "${LIGATE_RPC}/v1/ledger/slots/${HEIGHT_BEFORE}" 2>/dev/null \
              | jq -r .state_root 2>/dev/null || echo "")"

MANIFEST="${STAGING_DIR}/${SNAPSHOT_NAME%.tar.gz}.manifest.json"
cat > "${MANIFEST}" <<EOF
{
  "snapshot_name": "${SNAPSHOT_NAME}",
  "chain_id": "${CHAIN_ID}",
  "block_height": ${HEIGHT_BEFORE},
  "state_root": "${STATE_ROOT}",
  "tier": "${TIER}",
  "mode": "$([ "$HOT_MODE" = "true" ] && echo hot || echo cold)",
  "sha256": "${SHA256}",
  "size_bytes": ${SIZE_BYTES},
  "created_at_utc": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
  "publisher_hostname": "$(hostname -s)"
}
EOF

echo "==> Manifest: ${MANIFEST}"
cat "${MANIFEST}"

# ----- Restart ligate-node (if we stopped it) ----------------------
if [ "${HOT_MODE}" = "false" ]; then
    echo "==> Restarting ligate-node"
    sudo systemctl start ligate-node
fi

# ----- Upload to GCS ----------------------------------------------
PREFIX="snapshots/${CHAIN_ID}/${HEIGHT_BEFORE}"
echo "==> Uploading to gs://${PUBLIC_BUCKET}/${PREFIX}/"
gcloud storage cp "${TARBALL}" "gs://${PUBLIC_BUCKET}/${PREFIX}/${SNAPSHOT_NAME}" --quiet
gcloud storage cp "${MANIFEST}" "gs://${PUBLIC_BUCKET}/${PREFIX}/$(basename "${MANIFEST}")" --quiet

# ----- Update the `latest.json` pointer for this tier --------------
#
# Followers fetch `gs://$PUBLIC_BUCKET/snapshots/<chain_id>/latest-<tier>.json`
# to discover the newest snapshot for their chain + tier choice.
LATEST_POINTER="${STAGING_DIR}/latest-${TIER}.json"
cat > "${LATEST_POINTER}" <<EOF
{
  "snapshot_url": "https://storage.googleapis.com/${PUBLIC_BUCKET}/${PREFIX}/${SNAPSHOT_NAME}",
  "manifest_url": "https://storage.googleapis.com/${PUBLIC_BUCKET}/${PREFIX}/$(basename "${MANIFEST}")",
  "chain_id": "${CHAIN_ID}",
  "block_height": ${HEIGHT_BEFORE},
  "tier": "${TIER}",
  "updated_at_utc": "$(date -u +%Y-%m-%dT%H:%M:%SZ)"
}
EOF
gcloud storage cp "${LATEST_POINTER}" \
    "gs://${PUBLIC_BUCKET}/snapshots/${CHAIN_ID}/latest-${TIER}.json" --quiet

# ----- Cleanup staging --------------------------------------------
echo "==> Cleaning local staging"
rm -f "${TARBALL}" "${MANIFEST}" "${LATEST_POINTER}"

echo "==> Snapshot ${SNAPSHOT_NAME} published."
echo "    URL:    https://storage.googleapis.com/${PUBLIC_BUCKET}/${PREFIX}/${SNAPSHOT_NAME}"
echo "    Height: ${HEIGHT_BEFORE}"
echo "    SHA256: ${SHA256}"
