# Bootstrap a follower from a public snapshot

**Status:** v0 — covers the public-snapshot import flow for a fresh `ligate-devnet-1` follower. Snapshot artefacts published daily by the canonical sequencer; partner follower downloads + imports + boots, skipping DA replay from height 0.

Companion to [`public-devnet-deploy.md`](public-devnet-deploy.md) (which covers the VM bring-up) and [`runbooks/backup-restore.md`](runbooks/backup-restore.md) (which covers operator-private RocksDB backups, NOT public snapshots).

---

## Why this exists

After a month of devnet, a fresh follower replaying DA from height 0 takes hours: multi-GB of blobs to refetch + reprocess + re-derive state. Past the early weeks, replay-from-zero is operationally untenable for partners who just want to mirror the chain.

Public snapshots cut bootstrap from hours to ~10 minutes:

- A daily snapshot of the canonical sequencer's state lands at `gs://ligate-snapshots-public/snapshots/<chain_id>/`
- Partners download via HTTPS (no GCP credentials needed)
- Manifest with `sha256` + `state_root` + `chain_id` proves what the partner got
- Import script handles extract + boot + verify

---

## Trust model

Pre-mainnet, the trust model is **"Ligate Labs publishes; partners trust the URL."** Concretely:

- The snapshot URL is HTTPS-served from a Ligate-owned GCS bucket. TLS proves origin.
- The manifest's `state_root` is checked by `ligate-node` itself when it boots from the imported state; if the imported RocksDB doesn't produce the expected root after replaying a few slots, the node halts.
- An adversary who compromises the snapshot URL (DNS hijack, GCS account takeover) could ship a malicious snapshot. We rely on operational hygiene for that, same as the binary distribution channel.

**Trustless snapshot verification** (light-client / proof of state) is post-mainnet work. Devnet partners accept the trust assumption explicitly when they choose snapshot over full DA replay.

---

## Quick start (partner-side)

```bash
# 1. Bring up the VM per public-devnet-deploy.md (or have any
#    ligate-node binary + config ready locally). Don't start the node yet.

# 2. Place the import script at /opt/ligate/scripts/import-snapshot.sh
#    (cloud-init can sudo install it if you're using the template;
#    otherwise copy from the chain repo's scripts/ directory).
sudo install -m 0755 scripts/import-snapshot.sh /opt/ligate/scripts/

# 3. Run the import:
sudo /opt/ligate/scripts/import-snapshot.sh \
    --chain-id ligate-devnet-1 \
    --tier daily

# The script will:
# - Fetch latest-daily.json for the chain
# - Download the tarball + manifest
# - Verify sha256 against the manifest
# - Stop ligate-node (if running) + quarantine existing state
# - Extract the snapshot to /var/lib/ligate/rollup
# - Start ligate-node
# - Confirm head_height + chain_id match expectations

# 4. Watch the follower catch up from snapshot height to current head
journalctl -fu ligate-node | grep block_committed
curl -s http://127.0.0.1:12346/v1/ledger/slots/latest | jq .number
```

Expected total time: **~5-15 minutes** depending on snapshot size and network. Most of that is the tarball download.

---

## Snapshot catalog

Three tiers, each with its own `latest-*.json` pointer:

| Tier | Cadence | Retention | Use case |
|---|---|---|---|
| `daily` | Nightly 04:30 UTC | 30 days | Default. Most followers want recent state. |
| `weekly` | Sunday 04:30 UTC | 12 weeks | Older partners catching up after extended downtime |
| `monthly` | First of month 04:30 UTC | 12 months | Forensic / audit use cases |

Pointer URL pattern:

```
https://storage.googleapis.com/ligate-snapshots-public/snapshots/<chain_id>/latest-<tier>.json
```

The pointer's body:

```jsonc
{
  "snapshot_url": "https://.../ligate-snapshot-ligate-devnet-1-12345-daily.tar.gz",
  "manifest_url": "https://.../ligate-snapshot-ligate-devnet-1-12345-daily.manifest.json",
  "chain_id": "ligate-devnet-1",
  "block_height": 12345,
  "tier": "daily",
  "updated_at_utc": "2026-05-13T04:35:12Z"
}
```

---

## What you get in a snapshot

A `.tar.gz` containing the `ligate-node` storage directory:

```
rollup/
  rocksdb/         # NOMT state trie + module state
  ledger_db/       # slot / batch / tx history
  ...
```

When extracted to `<storage>.path`, ligate-node treats it as resume state. The chain backfills any slots between the snapshot height and Celestia head over the next few minutes (depends on DA bandwidth).

Snapshots include the chain's full state at the captured height. They do NOT include:

- **Operator-side secrets** (sequencer key, DA signer key, faucet key). Partners must supply their own per [`public-devnet-deploy.md`](public-devnet-deploy.md).
- **rollup.toml / celestia.toml.** Partners use the canonical configs from the chain repo's `devnet-1/` directory.
- **Indexer state.** The api's Postgres is a separate concern; rebuilds from chain when the partner runs their own indexer.

---

## Verification after import

Once the node is back up:

```bash
# 1. Chain identity matches
curl -s http://127.0.0.1:12346/v1/rollup/info | jq '.chain_id, .chain_hash'
# Expected: matches the manifest's chain_id + the canonical chain_hash
# for that chain (you can cross-check via the public RPC at rpc.ligate.io).

# 2. Block height is at or beyond the snapshot's height
curl -s http://127.0.0.1:12346/v1/ledger/slots/latest | jq .number

# 3. State root for the snapshot's height matches manifest
SNAPSHOT_HEIGHT=$(jq -r .block_height /tmp/manifest.json)
SNAPSHOT_ROOT=$(jq -r .state_root /tmp/manifest.json)
curl -s "http://127.0.0.1:12346/v1/ledger/slots/${SNAPSHOT_HEIGHT}" | jq -r .state_root
# Should match SNAPSHOT_ROOT
```

If the state root verification fails after a few slots of DA catch-up, the snapshot is corrupt or tampered. Quarantine the imported state, re-import from a different tier (or fall back to DA replay from genesis).

---

## Operator-side: publishing snapshots

The canonical sequencer publishes snapshots via `scripts/publish-snapshot.sh` driven by a systemd timer. See [`ops/backup/systemd/ligate-snapshot-publish.{service,timer}`](../../ops/backup/systemd/) for the deploy.

One-time setup:

```sh
# 1. Provision the PUBLIC snapshot bucket (note: distinct from the
#    operator-private backup bucket per backup-restore.md). Public-read
#    so partners can fetch without GCP creds.
gcloud storage buckets create gs://ligate-snapshots-public \
    --location=us-central1 \
    --default-storage-class=STANDARD \
    --uniform-bucket-level-access
gcloud storage buckets add-iam-policy-binding gs://ligate-snapshots-public \
    --member=allUsers --role=roles/storage.objectViewer

# 2. Configure lifecycle (per-tier retention)
gcloud storage buckets update gs://ligate-snapshots-public \
    --lifecycle-file=ops/backup/gcs-snapshot-lifecycle.json

# 3. Service account scoped to the bucket
gcloud iam service-accounts create ligate-snapshot \
    --display-name="Ligate snapshot publisher"
gcloud storage buckets add-iam-policy-binding gs://ligate-snapshots-public \
    --member="serviceAccount:ligate-snapshot@${GCP_PROJECT}.iam.gserviceaccount.com" \
    --role="roles/storage.objectAdmin"
gcloud iam service-accounts keys create /etc/ligate/snapshot-sa.json \
    --iam-account=ligate-snapshot@${GCP_PROJECT}.iam.gserviceaccount.com

# 4. Install scripts + units on the canonical sequencer
sudo install -m 0755 scripts/publish-snapshot.sh /opt/ligate/scripts/
sudo install -m 0644 ops/backup/systemd/ligate-snapshot-publish.* \
    /etc/systemd/system/
sudo tee /etc/ligate/snapshot.env <<EOF
PUBLIC_BUCKET=ligate-snapshots-public
GOOGLE_APPLICATION_CREDENTIALS=/etc/ligate/snapshot-sa.json
EOF
sudo chmod 0600 /etc/ligate/snapshot.env
sudo chown ligate:ligate /etc/ligate/snapshot.env
sudo systemctl daemon-reload
sudo systemctl enable --now ligate-snapshot-publish.timer
```

---

## End-to-end drill

Before relying on snapshots in production, run the drill:

```sh
# 1. Publish a snapshot (manual trigger)
sudo systemctl start ligate-snapshot-publish.service

# 2. Verify it landed in the public bucket
gcloud storage ls gs://ligate-snapshots-public/snapshots/ligate-devnet-1/

# 3. On a fresh non-prod VM (or a wiped local docker container)
#    that has ligate-node + configs but no state:
sudo /opt/ligate/scripts/import-snapshot.sh --chain-id ligate-devnet-1

# 4. Confirm the fresh follower catches up to the canonical sequencer's
#    head within a few minutes:
SEQUENCER_HEIGHT=$(curl -s https://rpc.ligate.io/v1/ledger/slots/latest | jq .number)
FOLLOWER_HEIGHT=$(curl -s http://localhost:12346/v1/ledger/slots/latest | jq .number)
echo "sequencer: $SEQUENCER_HEIGHT  follower: $FOLLOWER_HEIGHT"
# Within 5 minutes, FOLLOWER_HEIGHT should equal SEQUENCER_HEIGHT.
```

Schedule the drill quarterly (the chain repo's calendar reminder system pings the operator).

---

## Out of scope

- **Trustless verification.** Would need a light-client construction. Mainnet work.
- **Cross-chain import.** Partner can't import a `ligate-devnet-1` snapshot into a `ligate-devnet-2` node. Chain id is enforced at import time.
- **Differential snapshots.** Each snapshot is a full RocksDB. Incremental snapshots would shrink download size; out of scope until devnet shows operational pain.
- **Operator-controlled (selective) state.** Partners who only care about specific schemas / addresses don't get a custom slice. Mainnet may revisit.

---

## Cross-references

- [Issue #273](https://github.com/ligate-io/ligate-chain/issues/273) — this runbook closes it
- [`public-devnet-deploy.md`](public-devnet-deploy.md) — VM bring-up the import plugs into
- [`runbooks/backup-restore.md`](runbooks/backup-restore.md) — operator-private backups; distinct goal from public snapshots
- [`scripts/publish-snapshot.sh`](../../scripts/publish-snapshot.sh) + [`scripts/import-snapshot.sh`](../../scripts/import-snapshot.sh) — the operational artifacts
