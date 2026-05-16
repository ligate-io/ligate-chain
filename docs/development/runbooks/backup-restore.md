# Runbook — RocksDB backup + restore

**Status:** v0 — covers the GCS-backed rsync flow that ships in `scripts/` + `ops/backup/systemd/`. Companion to the broader operational layer ([`celestia-light-node.md`](./celestia-light-node.md), [`alerts/`](./alerts/)).

`ligate-node` writes persistent state to RocksDB at `<storage>.path`. On the live devnet-1 sequencer that path is `/opt/ligate/devnet-1/data-celestia`, which is a symlink to `/var/lib/ligate/rocksdb` on the persistent data disk (`/dev/sdb`, 50 GB). The symlink approach keeps the rollup config (`devnet-1/celestia.toml`) host-portable while still landing the actual bytes on the persistent disk so a VM image rebuild doesn't wipe state. Without backups, a single bad disk still loses the chain's local history; recovery would require full DA replay from genesis, which on a multi-month devnet means hours of replay before we even hit any DA fetch quirks.

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
│ /var/lib/ligate/rocksdb     │ rsync   │ /var/lib/ligate/     │
│ (RocksDB write-active,      ├────────►│   snapshots/<tier>/  │
│  symlinked from             │ --link- │   <timestamp>/       │
│  /opt/ligate/devnet-1/      │ dest    │  (hardlinked)        │
│  data-celestia)             │         │                      │
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

### 1. GCS bucket + grant the chain VM write access

```sh
# Regional bucket close to the chain VM, standard storage class,
# uniform IAM, public-access-prevention on.
gcloud storage buckets create gs://ligate-devnet-1-backups \
    --project=$YOUR_GCP_PROJECT \
    --location=us-central1 \
    --default-storage-class=STANDARD \
    --uniform-bucket-level-access \
    --public-access-prevention

# Apply the rotation policy (hourly: 7d, daily: 60d, weekly: 365d)
gcloud storage buckets update gs://ligate-devnet-1-backups \
    --lifecycle-file=ops/backup/gcs-lifecycle.json

# Grant the VM's default Compute Engine service account write access
# on this bucket. (We can't issue a long-lived SA JSON key on this
# project — org policy `constraints/iam.disableServiceAccountKeyCreation`
# is enforced. Using the existing VM SA + a bucket-scoped IAM grant
# avoids needing a key.)
VM_SA=$(gcloud compute instances describe ligate-devnet-1-sequencer \
        --zone us-central1-a --format='value(serviceAccounts[0].email)')
gcloud storage buckets add-iam-policy-binding gs://ligate-devnet-1-backups \
    --member="serviceAccount:$VM_SA" \
    --role="roles/storage.objectAdmin"
```

The VM's default service account ships with `devstorage.read_only` scope only. Add `devstorage.read_write` (cycle of stop → set-service-account → start; ~2 min of chain downtime):

```sh
gcloud compute instances stop ligate-devnet-1-sequencer --zone us-central1-a

gcloud compute instances set-service-account ligate-devnet-1-sequencer \
    --zone us-central1-a \
    --service-account "$VM_SA" \
    --scopes "https://www.googleapis.com/auth/devstorage.read_write,\
https://www.googleapis.com/auth/logging.write,\
https://www.googleapis.com/auth/monitoring.write,\
https://www.googleapis.com/auth/pubsub,\
https://www.googleapis.com/auth/service.management.readonly,\
https://www.googleapis.com/auth/servicecontrol,\
https://www.googleapis.com/auth/trace.append"

gcloud compute instances start ligate-devnet-1-sequencer --zone us-central1-a

# After start: re-enable + start ligate-node (it's not enabled by default
# because the cloud-init only installs the unit; first-boot left it
# disabled. Subsequent boots auto-start once enabled.)
gcloud compute ssh ligate-devnet-1-sequencer --zone us-central1-a --command '
    sudo systemctl enable --now ligate-node.service
'
```

### 2. Configure the chain VM

```sh
# Place scripts on the VM
sudo install -o ligate -g ligate -m 0755 \
    scripts/backup-rocksdb.sh /opt/ligate/scripts/
sudo install -o ligate -g ligate -m 0755 \
    scripts/restore-rocksdb.sh /opt/ligate/scripts/
sudo install -o ligate -g ligate -m 0755 \
    scripts/restore-drill.sh /opt/ligate/scripts/

# Place systemd units. ligate-backup@.service is a template unit;
# the three timers each bind to an instance (hourly / daily / weekly)
# whose name becomes %i, which the template sets as TIER.
sudo install -o root -g root -m 0644 \
    ops/backup/systemd/ligate-backup@.service /etc/systemd/system/
sudo install -o root -g root -m 0644 \
    ops/backup/systemd/ligate-backup-hourly.timer /etc/systemd/system/
sudo install -o root -g root -m 0644 \
    ops/backup/systemd/ligate-backup-daily.timer /etc/systemd/system/
sudo install -o root -g root -m 0644 \
    ops/backup/systemd/ligate-backup-weekly.timer /etc/systemd/system/

# Drop the env file. Copy ops/backup/backup.env.example and fill in
# the per-host fields. Keep chmod 600, owned root:root. Note: TIER
# is NOT in the envfile; it comes from the template instance name.
sudo install -o root -g root -m 0600 \
    ops/backup/backup.env.example /etc/ligate/backup.env

# Stage dir for rsync --link-dest chains (must be on the persistent
# disk so dedupe survives VM rebuilds)
sudo mkdir -p /var/lib/ligate/snapshots
sudo chown ligate:ligate /var/lib/ligate/snapshots

# Enable all three timers
sudo systemctl daemon-reload
sudo systemctl enable --now ligate-backup-hourly.timer
sudo systemctl enable --now ligate-backup-daily.timer
sudo systemctl enable --now ligate-backup-weekly.timer

# Verify
systemctl list-timers 'ligate-backup-*.timer'
```

### 3. First manual backup (smoke test)

```sh
# Trigger one run by hand against each tier. The oneshot returns
# when the upload completes (~30-60s on a healthy devnet at ~1.2GB
# state).
sudo systemctl start ligate-backup@hourly.service
sudo systemctl start ligate-backup@daily.service
sudo journalctl -u 'ligate-backup@*.service' --since "5 minutes ago" --no-pager

# Inspect what landed in GCS:
gcloud storage ls gs://ligate-devnet-1-backups/ligate-devnet-1-sequencer/

# Check the per-snapshot manifest carries height + storage path:
TS=$(gcloud storage ls gs://ligate-devnet-1-backups/ligate-devnet-1-sequencer/hourly/ \
       | tail -1 | awk -F/ '{print $(NF-1)}')
gcloud storage cat \
    "gs://ligate-devnet-1-backups/ligate-devnet-1-sequencer/hourly/$TS/$TS/.snapshot-manifest.json"
```

---

## Routine snapshotting

Handled by the systemd timers; no operator action required day-to-day. The timers fire at:

| Timer | Binds to | When | GCS retention |
|---|---|---|---|
| `ligate-backup-hourly.timer` | `ligate-backup@hourly.service` | every 15 min | 7 days |
| `ligate-backup-daily.timer` | `ligate-backup@daily.service` | 03:30 UTC nightly | 60 days |
| `ligate-backup-weekly.timer` | `ligate-backup@weekly.service` | Sun 04:00 UTC | 1 year |

The template unit `ligate-backup@.service` derives `TIER` from its instance name (`%i`), so the same service file backs all three tiers. Local rotation in `backup-rocksdb.sh` keeps the on-disk snapshots dir bounded per tier; GCS lifecycle in `ops/backup/gcs-lifecycle.json` keeps the bucket bounded by tier prefix.

Observability: `journalctl -u 'ligate-backup@*.service'` shows each run. Failures cause `systemctl is-failed ligate-backup@<tier>.service` to report yes; pair with an Alertmanager rule (post-#268) for proactive notification.

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

## Migrating an existing host to the persistent disk

If you're operating a host where chain state landed on `/dev/root` instead of the persistent disk (cloud-init didn't pre-create the symlink), here's the in-place migration. Done on `ligate-devnet-1-sequencer` on 2026-05-16 with ~3 min of chain downtime.

```sh
# 1. Stop the chain cleanly. TimeoutStopSec=300 gives RocksDB room
#    to flush WAL + finish compaction.
sudo systemctl stop ligate-node.service

# 2. Stage the new home + rsync existing data over. Use --sparse to
#    preserve sparseness on NOMT files (or accept ~4x bloat).
sudo mkdir -p /var/lib/ligate/rocksdb
sudo chown ligate:ligate /var/lib/ligate/rocksdb
sudo -u ligate rsync -a --sparse --info=progress2 \
    /opt/ligate/devnet-1/data-celestia/ \
    /var/lib/ligate/rocksdb/

# 3. Swap the original dir for a symlink pointing at the new home.
#    Keep the original as .MIGRATION-BACKUP-<date> for at least 24h
#    in case the new path has a config / permissions issue surface
#    only after startup.
sudo mv /opt/ligate/devnet-1/data-celestia \
        /opt/ligate/devnet-1/data-celestia.MIGRATION-BACKUP-$(date -u +%Y%m%d)
sudo ln -s /var/lib/ligate/rocksdb /opt/ligate/devnet-1/data-celestia
sudo chown -h ligate:ligate /opt/ligate/devnet-1/data-celestia

# 4. Start the chain. Verify head advances past the pre-stop height
#    AND chain_hash is unchanged (symlink swap shouldn't touch state).
sudo systemctl start ligate-node.service
curl -s https://api.ligate.io/v1/info | jq

# 5. Update /etc/ligate/backup.env: set LIGATE_STORAGE_DIR=/var/lib/ligate/rocksdb
#    so the snapshot manifest records the physical path.

# 6. Trigger one manual backup against the new path:
sudo systemctl start ligate-backup@hourly.service

# 7. After 24h of uneventful operation, reclaim the /dev/root space:
sudo rm -rf /opt/ligate/devnet-1/data-celestia.MIGRATION-BACKUP-*
```

---

## Resizing the persistent disk (online, no downtime)

GCE persistent disks grow live, and ext4 resizes online — no chain stop needed.

```sh
# 1. Grow the GCE disk.
gcloud compute disks resize ligate-data-v2 --zone us-central1-a --size 150GB

# 2. Inside the VM, grow the ext4 filesystem to fill the new size.
gcloud compute ssh ligate-devnet-1-sequencer --zone us-central1-a --command '
  sudo resize2fs /dev/sdb
  df -h /var/lib/ligate
'
```

Done on `ligate-devnet-1-sequencer` 2026-05-16 (50GB → 150GB) when projected local snapshot accumulation (24 hourly × ~5GB) was on track to fill the original disk within ~24h. With the `--sparse` rsync fix (see followups) each snapshot drops from ~5GB to ~1.2GB and the 50GB disk would have been fine; 150GB is comfortable headroom either way.

You can't shrink a GCE persistent disk, so size up cautiously. ext4 supports shrinking but only offline; not a path we'd take on this host.

---

## Out of scope

- **Multi-region replication.** A second GCS bucket in `us-east1` would survive a regional outage. Deferred to mainnet.
- **Encrypted-at-rest backups.** Devnet writes plaintext anyway; mainnet revisits with operator-side encryption keys.
- **Sequencer / attestor key backup.** Separate operational concern; covered in [`sequencer-key-rotation.md`](./sequencer-key-rotation.md) + [`da-signer-rotation.md`](./da-signer-rotation.md).
- **Cross-chain backup (DA replay)**. The DA layer IS the off-chain backup; this runbook is the fast-path. Replay-from-DA is the slow-path fallback.

---

## Known limitations (post-launch followups)

What's live on devnet-1 as of 2026-05-16 vs what's still pending:

| Limitation | Impact | Tracked |
|---|---|---|
| `backup-rocksdb.sh`'s `block_height` capture in the manifest can produce a multi-line value (height + `unknown`) because the script tries two height-extraction paths and writes both. Manifest is still parseable; the value is just ugly. | Cosmetic; restore doesn't depend on this field. | chain#359 followups |
| `backup-rocksdb.sh` uses `rsync -a` without `--sparse`, so sparse files (RocksDB NOMT files contain many) inflate ~4x on copy. Doesn't affect chain operation but bloats local snapshot stage + GCS storage. | Disk-cost only; ~$4/mo extra storage at devnet scale. | chain#359 followups |
| `ligate-node.service` is not enabled by default after first cloud-init boot — the original runbook left it as a manual `systemctl start` step. After every `gcloud compute instances stop|start`, the service must be re-enabled with `sudo systemctl enable --now ligate-node.service`. | One-time post-deploy step; now codified in §1 above. | n/a, fixed in the runbook |
| Cloud-init in `public-devnet-deploy.md` doesn't pre-create the `/var/lib/ligate/rocksdb` directory + symlink at `/opt/ligate/devnet-1/data-celestia`. Until that lands, fresh deploys need a manual symlink step before first `ligate-node` start (else the chain writes to `/dev/root` again). | One-time step on fresh deploys. | chain#359 followups |

---

## Cross-references

- [`public-devnet-deploy.md`](../public-devnet-deploy.md) — the broader deploy runbook the backup story plugs into
- [`alerts/`](./alerts/) — Alertmanager rules including disk-fill alerts that trigger backup investigation
- [Issue #267](https://github.com/ligate-io/ligate-chain/issues/267) — this runbook closes it
- [`scripts/backup-rocksdb.sh`](../../../scripts/backup-rocksdb.sh) + [`scripts/restore-rocksdb.sh`](../../../scripts/restore-rocksdb.sh) — the operational artifacts
- [`ops/backup/systemd/`](../../../ops/backup/systemd/) — the timer + service units
