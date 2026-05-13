# Runbook — RocksDB backup + restore

**Status:** v0 — covers the GCS-backed rsync flow that ships in `scripts/` + `ops/backup/systemd/`. Companion to the broader operational layer ([`celestia-light-node.md`](./celestia-light-node.md), [`alerts/`](./alerts/)).

`ligate-node` writes persistent state to RocksDB at `<storage>.path` (default `/var/lib/ligate/rollup`). Without backups, a single bad disk loses the chain's local history; recovery requires full DA replay from genesis, which on a multi-month devnet means hours of replay before we even hit any DA fetch quirks.

This runbook covers: routine snapshotting to GCS, point-in-time restore from a snapshot, and the quarterly drill that proves the restore path actually works.

---

## Why backups, not just DA replay

Re-deriving state from the Celestia namespace works in theory — the chain is deterministic given the DA bytes — but in practice:

- **Replay time:** multi-month devnet means many GB of blobs to refetch + reprocess. Hours, not minutes, even on fast hardware.
- **DA fetch quirks:** mocha bridge availability can hiccup mid-replay; restart-from-zero scenarios encounter rough edges (e.g. light node sample cache misses).
- **Restore predictability:** an operator handed "the chain is back at height H from a snapshot" can verify it advanced. "Replay from zero, finishes when finishes" gives no SLA.

Backups give a known-good fast restore path. DA replay remains the ultimate-source-of-truth fallback when all snapshots are corrupt.

---

## Architecture

```
┌─────────────────────────────┐         ┌──────────────────────┐
│ /var/lib/ligate/rollup      │ rsync   │ /var/lib/ligate/     │
│ (RocksDB write-active)      ├────────►│   snapshots/<tier>/  │
│                             │ --link- │   <timestamp>/       │
│                             │ dest    │  (hardlinked)        │
└─────────────────────────────┘         └──────────┬───────────┘
                                                   │
                                                   │ gcloud storage cp -r
                                                   ▼
                                  ┌────────────────────────────────┐
                                  │ gs://${GCS_BUCKET}/<host>/     │
                                  │   <tier>/<timestamp>/          │
                                  │ + manifest.json (height, etc)  │
                                  └──────────────┬─────────────────┘
                                                 │
                                                 │ lifecycle policy
                                                 ▼
                                  hourly: 7 days │ daily: 60 days │ weekly: 1 year
```

Two layers of pruning, each handles a different scope:

| Layer | Where | Keeps | Configured by |
|---|---|---|---|
| Local snapshots | `/var/lib/ligate/snapshots/<tier>/` | last 24h / 7d / 4w per tier | `scripts/backup-rocksdb.sh` rotation |
| GCS objects | `gs://$GCS_BUCKET/...` | 7d / 60d / 1y per tier | `ops/backup/gcs-lifecycle.json` |

---

## One-time setup

### 1. GCS bucket + service account

```sh
# Pick a regional bucket close to the chain VM. Standard storage class.
gcloud storage buckets create gs://ligate-devnet-1-backups \
    --location=us-central1 \
    --default-storage-class=STANDARD \
    --uniform-bucket-level-access

# Apply the lifecycle policy
gcloud storage buckets update gs://ligate-devnet-1-backups \
    --lifecycle-file=ops/backup/gcs-lifecycle.json

# Create a service account scoped to the bucket for the chain VM
gcloud iam service-accounts create ligate-backup \
    --display-name="Ligate backup writer"

# Grant write-only access to the bucket (not project-wide)
gcloud storage buckets add-iam-policy-binding gs://ligate-devnet-1-backups \
    --member="serviceAccount:ligate-backup@${GCP_PROJECT}.iam.gserviceaccount.com" \
    --role="roles/storage.objectAdmin"

# Issue a key for the chain VM
gcloud iam service-accounts keys create /etc/ligate/backup-sa.json \
    --iam-account=ligate-backup@${GCP_PROJECT}.iam.gserviceaccount.com
```

### 2. Configure the chain VM

```sh
# Place scripts on the VM
sudo install -m 0755 scripts/backup-rocksdb.sh /opt/ligate/scripts/
sudo install -m 0755 scripts/restore-rocksdb.sh /opt/ligate/scripts/
sudo install -m 0755 scripts/restore-drill.sh /opt/ligate/scripts/

# Place systemd units
sudo install -m 0644 ops/backup/systemd/*.service /etc/systemd/system/
sudo install -m 0644 ops/backup/systemd/*.timer /etc/systemd/system/

# Configure env
sudo tee /etc/ligate/backup.env <<EOF
GCS_BUCKET=ligate-devnet-1-backups
LIGATE_STORAGE_DIR=/var/lib/ligate/rollup
GOOGLE_APPLICATION_CREDENTIALS=/etc/ligate/backup-sa.json
EOF
sudo chmod 0600 /etc/ligate/backup.env
sudo chown ligate:ligate /etc/ligate/backup.env

# Enable the timers
sudo systemctl daemon-reload
sudo systemctl enable --now ligate-backup-hourly.timer
sudo systemctl enable --now ligate-backup-daily.timer
sudo systemctl enable --now ligate-backup-weekly.timer

# Verify
systemctl list-timers ligate-backup-*.timer
```

### 3. First manual backup (smoke test)

```sh
# Run once by hand to confirm the chain has write access + the rsync
# completes:
sudo -u ligate \
    GCS_BUCKET=ligate-devnet-1-backups \
    /opt/ligate/scripts/backup-rocksdb.sh

# Inspect:
gcloud storage ls gs://ligate-devnet-1-backups/$(hostname -s)/hourly/
```

---

## Routine snapshotting

Handled by the systemd timers; no operator action required day-to-day. The timers fire at:

| Timer | When | Tier | GCS retention |
|---|---|---|---|
| `ligate-backup-hourly.timer` | every 15 min | `hourly` | 7 days |
| `ligate-backup-daily.timer` | 03:30 UTC nightly | `daily` | 60 days |
| `ligate-backup-weekly.timer` | Sun 04:00 UTC | `weekly` | 1 year |

The `daily` and `weekly` runs use a systemd drop-in that overrides `TIER` on the executed unit. Local rotation keeps the snapshots directory bounded.

Observability: `journalctl -u ligate-backup.service` shows each run. Failures cause `systemctl is-failed` to report yes; pair with an Alertmanager rule (post-#268) for proactive notification.

---

## Restore procedure

Three triggers for a restore:

1. **Disk loss** — VM terminated unexpectedly, disk corruption, accidental wipe.
2. **State corruption** — chain detects an invariant violation; restore to the last clean snapshot.
3. **Chain rollback request** — extremely rare; only for incidents where the chain advanced into a bad state and consensus says "rewind."

### Procedure

```sh
# 1. Identify the snapshot to restore from
gcloud storage ls gs://ligate-devnet-1-backups/$(hostname -s)/hourly/

# 2. Run the restore script. With no args, picks the most recent hourly.
sudo /opt/ligate/scripts/restore-rocksdb.sh

# Or pick a specific snapshot:
sudo /opt/ligate/scripts/restore-rocksdb.sh --tier daily --timestamp 20260512

# 3. The script:
#    - stops ligate-node
#    - moves existing storage to a quarantine path
#    - downloads + places the snapshot
#    - restarts ligate-node
#    - prints the expected vs current block_height

# 4. Verify the head is advancing
journalctl -fu ligate-node | grep "block_committed"
curl -s http://127.0.0.1:12346/v1/ledger/slots/latest | jq .number
```

**Expected duration:** ~5-15 min for a recent hourly snapshot (download dominated by snapshot size; ~1-2 GB on a healthy devnet).

**If the snapshot is corrupt:** `restore-rocksdb.sh` aborts on manifest verification failure. Fall back N snapshots:

```sh
sudo /opt/ligate/scripts/restore-rocksdb.sh --tier hourly --timestamp 20260512T100000Z
# (one snapshot older than the corrupt one)
```

---

## Restore drill

Every quarter, run the drill on a non-prod VM to confirm the backup + restore path actually works.

```sh
# On a non-prod VM (NOT the production sequencer):
sudo /opt/ligate/scripts/restore-drill.sh
```

The drill:

1. Triggers a fresh backup
2. Records `block_height` pre-drill
3. Quarantines the current storage (simulates disk loss)
4. Runs the restore from the most recent hourly
5. Confirms the node restarts + `block_height` advances post-restore
6. Cleans up the quarantined state

Exit codes encode the failure stage (1 = backup, 2 = restore, 3 = post-restore verify). Schedule via cron or a quarterly calendar reminder.

---

## Out of scope

- **Multi-region replication.** A second GCS bucket in `us-east1` would survive a regional outage. Deferred to mainnet.
- **Encrypted-at-rest backups.** Devnet writes plaintext anyway; mainnet revisits with operator-side encryption keys.
- **Sequencer / attestor key backup.** Separate operational concern; covered in [`sequencer-key-rotation.md`](./sequencer-key-rotation.md) + [`da-signer-rotation.md`](./da-signer-rotation.md).
- **Cross-chain backup (DA replay)**. The DA layer IS the off-chain backup; this runbook is the fast-path. Replay-from-DA is the slow-path fallback.

---

## Cross-references

- [`public-devnet-deploy.md`](../public-devnet-deploy.md) — the broader deploy runbook the backup story plugs into
- [`alerts/`](./alerts/) — Alertmanager rules including disk-fill alerts that trigger backup investigation
- [Issue #267](https://github.com/ligate-io/ligate-chain/issues/267) — this runbook closes it
- [`scripts/backup-rocksdb.sh`](../../../scripts/backup-rocksdb.sh) + [`scripts/restore-rocksdb.sh`](../../../scripts/restore-rocksdb.sh) — the operational artifacts
- [`ops/backup/systemd/`](../../../ops/backup/systemd/) — the timer + service units
