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
#      for our purposes: ligate-node uses RocksDB's WAL so partial
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

# Snapshot tag: hour for routine runs, day for the daily-keep tier,
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

# Capture the current block height BEFORE any cold-snapshot stop so
# the manifest records the height the snapshot corresponds to.
#
# Source preference: chain RPC (`/v1/ledger/slots/latest .number`)
# over the Prometheus metrics gauge (`ligate_block_height`). The RPC
# returns the real head height as soon as the HTTP server is up,
# while the metrics gauge isn't populated until the chain replays
# enough state to set it. Observed empty for ~2 min after a
# cold-backup-induced restart on 2026-05-17 04:00 UTC, which made
# the manifest record "unknown" even though the chain was healthy.
# RPC fallback to metrics, fallback to "unknown".
HEIGHT_PRESTOP="$(curl -sf http://127.0.0.1:12346/v1/ledger/slots/latest 2>/dev/null \
                    | jq -r '.number // empty' 2>/dev/null)"
if [ -z "${HEIGHT_PRESTOP}" ]; then
    METRICS_PRESTOP="$(curl -sf http://127.0.0.1:9100/metrics 2>/dev/null || true)"
    HEIGHT_PRESTOP="$(awk '/^ligate_block_height /{print $2; exit}' <<< "${METRICS_PRESTOP}")"
fi

# Hot vs cold snapshot semantics:
#
# rsync of a live RocksDB+NOMT directory is best-effort: NOMT files
# (bbn, ht, ln, meta, wal in user_nomt_db / kernel_nomt_db) can be
# caught mid-write, producing a snapshot that triggers
# `assertion failed: pn.0 < max_pn` in nomt-1.0.3's free-list
# allocator when the chain tries to load it. RocksDB's own WAL
# recovers from torn writes; NOMT's beatree allocator doesn't (as of
# nomt-1.0.3; observed during the restore drill on 2026-05-16).
#
# Daily + weekly tiers (the "gold" snapshots used for real restore)
# go COLD: stop ligate-node briefly, take a consistent point-in-time
# rsync, restart. ~30-60s pause on a healthy state DB; runs at
# off-peak hours per the timers (03:30 / 04:00 UTC).
#
# Hourly tier stays HOT (no pause): best-effort, frequent, useful
# for "last 6 hours" rollback if the chain itself didn't survive a
# rough event. Restore is best-effort; the last clean cold daily
# snapshot is always available as fallback.
COLD=false
case "$TIER" in
    daily|weekly) COLD=true ;;
esac

if [ "$COLD" = "true" ]; then
    echo "==> Cold snapshot: pausing ligate-node for a consistent rsync"
    # Two-part shutdown to dodge two bugs:
    #
    # 1. Upstream Sovereign SDK hang: ligate-node logs "shutdown
    #    complete" within ~1s of SIGTERM but the process doesn't
    #    actually exit (surfaced 2026-05-17 03:30 UTC when the first
    #    scheduled daily backup hit `TimeoutStopSec=300` and took
    #    ~5.5 min of chain downtime). SIGKILL is safe in practice:
    #    RocksDB + NOMT WALs recover cleanly on next start,
    #    chain_hash unchanged across the forced-kill events we've
    #    observed.
    #
    # 2. systemd auto-restart from `Restart=on-failure`: a bare
    #    `systemctl kill` makes systemd see the process exit as a
    #    failure and restart ligate-node automatically: the rsync
    #    then runs against a LIVE chain again, defeating the cold
    #    snapshot. Using `systemctl stop --no-block` tells systemd
    #    "we are intentionally stopping, don't restart", then we
    #    escalate to SIGKILL once the SIGTERM grace window expires.
    sudo systemctl stop --no-block ligate-node.service
    # Give graceful-shutdown logs ~3s to flush. The chain finishes
    # its shutdown sequence well within 1s; 3s is a defensive margin.
    sleep 3
    # Escalate: SIGKILL the process if it hasn't exited yet (it
    # almost certainly hasn't, per bug #1). systemd is in "stopping"
    # state from the --no-block stop above, so this doesn't trigger
    # the Restart=on-failure path.
    sudo systemctl kill --signal=SIGKILL ligate-node.service 2>/dev/null || true
    # Wait for the unit to actually transition to inactive/failed so
    # the rsync runs against a quiescent storage directory.
    for _ in $(seq 1 30); do
        STATE=$(systemctl show ligate-node.service -p ActiveState --value)
        [ "$STATE" = "inactive" ] || [ "$STATE" = "failed" ] && break
        sleep 0.5
    done
    echo "    ligate-node stopped (state=${STATE:-unknown})"
fi

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

if [ "$COLD" = "true" ]; then
    echo "==> Restarting ligate-node"
    sudo systemctl start ligate-node.service
fi

# Write a manifest alongside so restore can verify checksum + height.
HEIGHT_FILE="${LOCAL_SNAPSHOT}/.snapshot-manifest.json"
# Prefer the pre-stop height captured above (always the right
# point-in-time number, especially for cold snapshots where the
# metrics endpoint is gone after the systemctl stop). Fall back to
# a fresh post-rsync read for the hot path, then "unknown" if
# both failed.
if [ -n "${HEIGHT_PRESTOP}" ]; then
    HEIGHT="${HEIGHT_PRESTOP}"
else
    # Same source preference as the pre-stop capture: RPC over
    # metrics gauge. Chain is back up by now (post-rsync, post-cold
    # restart in the cold path; never stopped in the hot path).
    HEIGHT="$(curl -sf http://127.0.0.1:12346/v1/ledger/slots/latest 2>/dev/null \
                | jq -r '.number // empty' 2>/dev/null)"
    if [ -z "${HEIGHT}" ]; then
        METRICS="$(curl -sf http://127.0.0.1:9100/metrics 2>/dev/null || true)"
        HEIGHT="$(awk '/^ligate_block_height /{print $2; exit}' <<< "${METRICS}")"
    fi
    [ -z "${HEIGHT}" ] && HEIGHT="unknown"
fi
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
