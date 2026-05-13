#!/usr/bin/env bash
# Restore drill: backup → simulate disk loss → restore → verify.
#
# Run quarterly on a non-prod VM. Catches regressions in the backup
# and restore scripts without needing a live incident. See
# `docs/development/runbooks/backup-restore.md#restore-drill`.
#
# Exit codes:
#   0 — drill passed
#   1 — backup step failed
#   2 — restore step failed
#   3 — post-restore verification failed
#
# Tracking issue: ligate-io/ligate-chain#267.

set -euo pipefail

: "${LIGATE_STORAGE_DIR:=/var/lib/ligate/rollup}"

echo "== Backup drill: $(date -u +%Y-%m-%dT%H:%M:%SZ) =="
DRILL_START=$SECONDS

# ----- Phase 1: take a hot snapshot -------------------------------
echo "==> Phase 1: snapshot"
sudo systemctl start ligate-backup.service
PRE_HEIGHT="$(curl -sf http://127.0.0.1:9100/metrics \
                | awk '/^ligate_block_height /{print $2; exit}')"
echo "    pre-drill block_height: ${PRE_HEIGHT}"

# ----- Phase 2: simulate disk loss --------------------------------
echo "==> Phase 2: quarantine the current storage"
sudo systemctl stop ligate-node
QUARANTINE="${LIGATE_STORAGE_DIR}.drill-$(date -u +%s)"
sudo mv "${LIGATE_STORAGE_DIR}" "${QUARANTINE}"
echo "    quarantined at ${QUARANTINE}"

# ----- Phase 3: restore --------------------------------------------
echo "==> Phase 3: restore from most recent hourly"
# Pipe `yes` to auto-confirm the restore script's prompt
if ! yes | /opt/ligate/scripts/restore-rocksdb.sh; then
    echo "RESTORE FAILED — re-instating quarantined state" >&2
    sudo systemctl stop ligate-node || true
    sudo rm -rf "${LIGATE_STORAGE_DIR}"
    sudo mv "${QUARANTINE}" "${LIGATE_STORAGE_DIR}"
    sudo systemctl start ligate-node
    exit 2
fi

# ----- Phase 4: verify ---------------------------------------------
echo "==> Phase 4: verify head advance"
sleep 5  # give the chain a moment to start producing
POST_HEIGHT="$(curl -sf http://127.0.0.1:9100/metrics \
                 | awk '/^ligate_block_height /{print $2; exit}')"
echo "    post-restore block_height: ${POST_HEIGHT}"

if [ "${POST_HEIGHT}" = "${PRE_HEIGHT}" ] || [ -z "${POST_HEIGHT}" ]; then
    echo "VERIFICATION FAILED — head height not advancing after restore" >&2
    exit 3
fi

# ----- Cleanup -----------------------------------------------------
echo "==> Phase 5: cleanup quarantine"
sudo rm -rf "${QUARANTINE}"

DRILL_DURATION=$((SECONDS - DRILL_START))
echo "== Drill PASSED in ${DRILL_DURATION}s =="
echo "   Pre:  ${PRE_HEIGHT}"
echo "   Post: ${POST_HEIGHT}"
