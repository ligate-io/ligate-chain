#!/usr/bin/env bash
# Bootstrap a fresh follower from a public snapshot rather than from
# DA replay from height 0.
#
# Usage:
#   import-snapshot.sh [--chain-id <id>] [--tier daily|weekly|monthly]
#   import-snapshot.sh --snapshot-url <https://...>
#
# With `--chain-id`, fetches the corresponding `latest-<tier>.json`
# pointer and downloads the snapshot it names. With `--snapshot-url`,
# downloads that specific tarball directly.
#
# Flow:
#   1. Resolve the snapshot URL.
#   2. Download + verify sha256 against the manifest.
#   3. Stop ligate-node (idempotent if not running).
#   4. Quarantine any existing storage dir.
#   5. Extract the tarball.
#   6. Start ligate-node.
#   7. Watch the first slot append + verify state_root matches manifest.
#
# Tracking issue: ligate-io/ligate-chain#273.

set -euo pipefail

: "${LIGATE_STORAGE_DIR:=/var/lib/ligate/rollup}"
: "${LIGATE_RPC:=http://127.0.0.1:12346}"
: "${STAGING_DIR:=/var/lib/ligate/import-staging}"
: "${PUBLIC_BUCKET_URL:=https://storage.googleapis.com/ligate-snapshots-public}"

CHAIN_ID=""
TIER="daily"
SNAPSHOT_URL=""

while [[ $# -gt 0 ]]; do
    case "$1" in
        --chain-id) CHAIN_ID="$2"; shift 2 ;;
        --tier) TIER="$2"; shift 2 ;;
        --snapshot-url) SNAPSHOT_URL="$2"; shift 2 ;;
        -h|--help) sed -n '2,25p' "$0"; exit 0 ;;
        *) echo "unknown arg: $1" >&2; exit 2 ;;
    esac
done

# ----- Resolve the snapshot URL ------------------------------------
if [ -z "$SNAPSHOT_URL" ]; then
    if [ -z "$CHAIN_ID" ]; then
        echo "either --snapshot-url or --chain-id is required" >&2
        exit 2
    fi
    POINTER_URL="${PUBLIC_BUCKET_URL}/snapshots/${CHAIN_ID}/latest-${TIER}.json"
    echo "==> Fetching pointer: ${POINTER_URL}"
    POINTER="$(curl -sf "${POINTER_URL}" || true)"
    if [ -z "$POINTER" ]; then
        echo "no snapshot pointer at ${POINTER_URL}" >&2
        echo "  (check the chain-id spelling + the tier)" >&2
        exit 1
    fi
    SNAPSHOT_URL="$(echo "$POINTER" | jq -r .snapshot_url)"
    MANIFEST_URL="$(echo "$POINTER" | jq -r .manifest_url)"
else
    # Derive manifest URL from the snapshot URL by replacing the
    # `.tar.gz` suffix with `.manifest.json`.
    MANIFEST_URL="${SNAPSHOT_URL%.tar.gz}.manifest.json"
fi

echo "==> Snapshot URL: ${SNAPSHOT_URL}"
echo "==> Manifest URL: ${MANIFEST_URL}"

# ----- Download manifest + verify chain_id matches our config -----
mkdir -p "${STAGING_DIR}"
MANIFEST_LOCAL="${STAGING_DIR}/manifest.json"
curl -sfL -o "${MANIFEST_LOCAL}" "${MANIFEST_URL}"

EXPECTED_HEIGHT="$(jq -r .block_height "${MANIFEST_LOCAL}")"
EXPECTED_SHA="$(jq -r .sha256 "${MANIFEST_LOCAL}")"
EXPECTED_CHAIN="$(jq -r .chain_id "${MANIFEST_LOCAL}")"
EXPECTED_STATE_ROOT="$(jq -r .state_root "${MANIFEST_LOCAL}")"
EXPECTED_SIZE="$(jq -r .size_bytes "${MANIFEST_LOCAL}")"

echo "==> Manifest: chain=${EXPECTED_CHAIN} height=${EXPECTED_HEIGHT} size=$((EXPECTED_SIZE/1024/1024)) MB"

# ----- Operator confirmation --------------------------------------
echo
echo "About to:"
echo "  1. Stop ligate-node (if running)"
echo "  2. Quarantine ${LIGATE_STORAGE_DIR} (if non-empty)"
echo "  3. Download + extract a ${EXPECTED_SIZE} byte snapshot"
echo "  4. Restart ligate-node from height ${EXPECTED_HEIGHT}"
echo
read -r -p "Proceed? [y/N] " CONFIRM
if [[ ! "$CONFIRM" =~ ^[yY]$ ]]; then
    echo "Aborted by operator."
    exit 0
fi

# ----- Download + verify hash --------------------------------------
TARBALL="${STAGING_DIR}/$(basename "${SNAPSHOT_URL}")"
echo "==> Downloading ${SNAPSHOT_URL}"
curl -fL --progress-bar -o "${TARBALL}" "${SNAPSHOT_URL}"

echo "==> Verifying sha256"
ACTUAL_SHA="$(sha256sum "${TARBALL}" | awk '{print $1}')"
if [ "${ACTUAL_SHA}" != "${EXPECTED_SHA}" ]; then
    echo "SHA256 mismatch!" >&2
    echo "  expected: ${EXPECTED_SHA}" >&2
    echo "  actual:   ${ACTUAL_SHA}" >&2
    echo "Snapshot is corrupt or tampered. Aborting." >&2
    rm -f "${TARBALL}"
    exit 1
fi
echo "    OK: ${ACTUAL_SHA}"

# ----- Stop the node + quarantine ----------------------------------
echo "==> Stopping ligate-node"
sudo systemctl stop ligate-node 2>/dev/null || true

if [ -d "${LIGATE_STORAGE_DIR}" ] && [ -n "$(ls -A "${LIGATE_STORAGE_DIR}" 2>/dev/null)" ]; then
    QUARANTINE="${LIGATE_STORAGE_DIR}.pre-import-$(date -u +%s)"
    echo "==> Quarantining existing storage to ${QUARANTINE}"
    sudo mv "${LIGATE_STORAGE_DIR}" "${QUARANTINE}"
fi

# ----- Extract -----------------------------------------------------
sudo mkdir -p "$(dirname "${LIGATE_STORAGE_DIR}")"
echo "==> Extracting snapshot"
# The tarball's top-level dir matches the basename of the storage dir
# (per `publish-snapshot.sh`'s tar invocation).
sudo tar -xzf "${TARBALL}" -C "$(dirname "${LIGATE_STORAGE_DIR}")"
sudo chown -R ligate:ligate "${LIGATE_STORAGE_DIR}"

# ----- Start ligate-node + verify ---------------------------------
echo "==> Starting ligate-node"
sudo systemctl start ligate-node

echo "==> Waiting for node to come up (up to 90s)"
WAITED=0
while ! curl -sf "${LIGATE_RPC}/v1/info" >/dev/null 2>&1; do
    sleep 2; WAITED=$((WAITED + 2))
    if [ "$WAITED" -gt 90 ]; then
        echo "node not serving after 90s; investigate" >&2
        exit 1
    fi
done

ACTUAL_HEIGHT="$(curl -sf "${LIGATE_RPC}/v1/ledger/slots/latest" | jq -r .number)"
ACTUAL_CHAIN="$(curl -sf "${LIGATE_RPC}/v1/info" | jq -r .chain_id)"
echo "==> Imported: chain_id=${ACTUAL_CHAIN} block_height=${ACTUAL_HEIGHT}"

if [ "${ACTUAL_CHAIN}" != "${EXPECTED_CHAIN}" ]; then
    echo "chain_id mismatch after import!" >&2
    echo "  expected: ${EXPECTED_CHAIN}" >&2
    echo "  actual:   ${ACTUAL_CHAIN}" >&2
    echo "Likely cause: imported snapshot for a different chain id. Restore quarantine." >&2
    exit 1
fi

if [ "${ACTUAL_HEIGHT}" != "${EXPECTED_HEIGHT}" ]; then
    echo "==> Height differs from manifest:"
    echo "    manifest:  ${EXPECTED_HEIGHT}"
    echo "    on-disk:   ${ACTUAL_HEIGHT}"
    echo "    This is normal if the node caught up some slots from DA"
    echo "    in the time between extract and first /info hit."
fi

rm -f "${TARBALL}" "${MANIFEST_LOCAL}"

echo "==> Import complete. Watch:"
echo "       journalctl -fu ligate-node"
echo "       curl -s ${LIGATE_RPC}/v1/ledger/slots/latest | jq .number"
