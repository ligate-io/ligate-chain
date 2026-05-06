# Public devnet deploy runbook

How to bring up a `ligate-devnet-1` node on GCP, in either operator role:

- **Sequencer** (the canonical Ligate-Labs node, single-instance in v0).
- **Follower** (any third-party operator running their own copy of the chain state).

Reads alongside [`celestia-ops.md`](celestia-ops.md), which covers the Celestia light-node specifics, and [`devnet-1/README.md`](../../devnet-1/README.md), which is the chain-config template this runbook deploys.

Tracking issue: [#189](https://github.com/ligate-io/ligate-chain/issues/189). Sibling issues this depends on or is depended on by:

- [#79](https://github.com/ligate-io/ligate-chain/issues/79): hosted RPC + sequencer infrastructure (this runbook IS that issue's chain-side deliverable; the cloud-side IaC is its own scope).
- [#125](https://github.com/ligate-io/ligate-chain/issues/125): GCP foundation (project / billing setup; this runbook assumes done).
- [#188](https://github.com/ligate-io/ligate-chain/issues/188): `devnet-1/` config artifacts.
- [#181](https://github.com/ligate-io/ligate-chain/issues/181): chain-id ladder validator.
- [#176](https://github.com/ligate-io/ligate-chain/issues/176): `/health` and `/ready` operator probes.
- [#110](https://github.com/ligate-io/ligate-chain/issues/110): Prometheus metrics endpoint.

## What this covers

- VM provisioning recommendations per role.
- `cloud-init` script that installs systemd units for `celestia-light`, `ligate-node`, and (on the sequencer host only) the faucet service.
- Co-located Celestia light node setup (the full Celestia commands live in `celestia-ops.md`; this runbook calls them).
- Caddy reverse proxy with automatic Let's Encrypt TLS for the public REST endpoint.
- Backup strategy for RocksDB.
- Prometheus scrape config and operator probes.
- Failure-mode triage table.
- Mocha-reset re-genesis playbook (when Celestia testnet resets, which it does roughly once a year).

## What this does NOT cover

- **Multi-sequencer operation** ([#82](https://github.com/ligate-io/ligate-chain/issues/82)). v0 is single-sequencer.
- **Force-include / sequencer-bypass paths** ([#81](https://github.com/ligate-io/ligate-chain/issues/81)). v1+.
- **Mainnet hardening** (HSM-backed keys, redundant DA bridges, alerting beyond best-effort). Pre-mainnet runbook scope only.
- **The actual cloud account provisioning** (project creation, billing setup, IAM). [#125](https://github.com/ligate-io/ligate-chain/issues/125)'s scope; this runbook starts from "GCP project exists, you can `gcloud compute instances create`".

## VM sizing

| Role | GCP shape | vCPU | RAM | Disk | $/mo (sustained-use) |
|---|---|---|---|---|---|
| Sequencer | `e2-standard-4` | 4 | 16 GB | 150 GB SSD | ~100 USD |
| Follower | `e2-medium` | 2 | 4 GB | 50 GB SSD | ~25 USD |

Sequencer sizing covers `ligate-node` + co-located Celestia light node + faucet service + headroom for log/metrics export. Follower sizing covers `ligate-node` + co-located Celestia light node only (no faucet, no sequencing).

Both roles can drop to half their disk if you don't need history retention longer than a month; the chain's NOMT prunes aggressively. Conservative numbers above so you don't fight low-disk alerts in the first 90 days.

## Step 1: Provision the VM

```bash
gcloud compute instances create ligate-devnet-1-sequencer \
    --project=$YOUR_GCP_PROJECT \
    --zone=us-central1-a \
    --machine-type=e2-standard-4 \
    --image-family=debian-12 \
    --image-project=debian-cloud \
    --boot-disk-size=20GB \
    --create-disk=name=ligate-data,size=150GB,type=pd-ssd \
    --tags=http-server,https-server \
    --metadata-from-file=user-data=cloud-init.yaml
```

For a follower, swap `--machine-type=e2-medium`, `--create-disk=...,size=50GB`, and rename the instance.

Reserve a static external IP and bind it:

```bash
gcloud compute addresses create ligate-devnet-1-rpc \
    --project=$YOUR_GCP_PROJECT --region=us-central1
gcloud compute instances add-access-config ligate-devnet-1-sequencer \
    --address=$(gcloud compute addresses describe ligate-devnet-1-rpc \
        --project=$YOUR_GCP_PROJECT --region=us-central1 --format='value(address)')
```

Point `rpc.ligate.io` (or your follower-side equivalent) at that IP via your DNS provider. Cloudflare in front gives free DDoS protection; either proxy or DNS-only mode works.

## Step 2: cloud-init

The `cloud-init.yaml` referenced above:

```yaml
#cloud-config
package_update: true
package_upgrade: true

packages:
  - build-essential
  - pkg-config
  - libssl-dev
  - cmake
  - clang
  - curl
  - git
  - jq
  - rsync

write_files:
  - path: /etc/systemd/system/celestia-light.service
    content: |
      [Unit]
      Description=Celestia light node (Mocha)
      After=network-online.target
      Wants=network-online.target

      [Service]
      User=ligate
      ExecStart=/usr/local/bin/celestia light start \
          --core.ip rpc-mocha.pops.one \
          --p2p.network mocha \
          --rpc.addr 127.0.0.1 \
          --rpc.port 26658
      Restart=on-failure
      RestartSec=5
      LimitNOFILE=65536

      [Install]
      WantedBy=multi-user.target

  - path: /etc/systemd/system/ligate-node.service
    content: |
      [Unit]
      Description=Ligate Chain node (devnet-1)
      After=celestia-light.service network-online.target
      Wants=celestia-light.service

      [Service]
      User=ligate
      WorkingDirectory=/opt/ligate
      EnvironmentFile=/etc/ligate/env
      ExecStart=/opt/ligate/bin/ligate-node \
          --da-layer celestia \
          --rollup-config-path /opt/ligate/devnet-1/celestia.toml \
          --genesis-config-dir /opt/ligate/devnet-1/genesis \
          --metrics-bind 127.0.0.1:9100
      Restart=on-failure
      RestartSec=10
      LimitNOFILE=65536

      [Install]
      WantedBy=multi-user.target

runcmd:
  - useradd -m -s /bin/bash ligate
  - mkdir -p /opt/ligate /etc/ligate /var/lib/ligate
  - chown ligate:ligate /opt/ligate /etc/ligate /var/lib/ligate
  # Mount the persistent data disk to /var/lib/ligate
  - |
    DEVICE=$(readlink -f /dev/disk/by-id/google-ligate-data)
    mkfs.ext4 -F $DEVICE
    echo "$DEVICE /var/lib/ligate ext4 defaults,nofail 0 2" >> /etc/fstab
    mount /var/lib/ligate
  # Install Rust + risc0 toolchain (sequencer only; follower can skip risc0)
  - sudo -u ligate bash -lc 'curl --proto "=https" --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y'
  # Install Celestia light node binary
  - |
    CELESTIA_VERSION=v0.20.0
    curl -L "https://github.com/celestiaorg/celestia-node/releases/download/${CELESTIA_VERSION}/celestia-linux-amd64" \
      -o /usr/local/bin/celestia && chmod +x /usr/local/bin/celestia
  - sudo -u ligate /usr/local/bin/celestia light init --p2p.network=mocha
  - systemctl daemon-reload
  - systemctl enable --now celestia-light.service
  # ligate-node service starts manually after the operator has placed the
  # binary, devnet-1 configs, and /etc/ligate/env (with secrets).
```

The cloud-init does the chassis. The operator still has to:

- Place the `ligate-node` binary at `/opt/ligate/bin/ligate-node`. From a tag onward, the canonical artifact is a GitHub Release tarball (`ligate-node-${VERSION}-linux-amd64.tar.gz` or `linux-arm64.tar.gz`); the release workflow at `.github/workflows/release.yml` produces them on every `v*` tag. Pre-tag operators build from source via `cargo build --release --bin ligate-node`.
- Clone the chain repo's `devnet-1/` directory to `/opt/ligate/devnet-1/`.
- Write `/etc/ligate/env` with the secrets (sequencer only):
  ```
  SOV_CELESTIA_RPC_AUTH_TOKEN=<from "celestia light auth admin">
  SOV_CELESTIA_GRPC_URL=http://127.0.0.1:9090
  SOV_CELESTIA_SIGNER_KEY=<hex private key holding Mocha TIA>
  RUST_LOG=info,sov=info
  ```
- `systemctl enable --now ligate-node.service`.

## Step 3: Caddy reverse proxy + TLS

Caddy on the same VM is the simplest TLS story. It autorenewed Let's Encrypt cert, no certbot fiddling.

```caddyfile
# /etc/caddy/Caddyfile
rpc.ligate.io {
    encode gzip
    reverse_proxy 127.0.0.1:12346

    # Health probes are unversioned; Caddy strips the /v1 nest.
    handle /health {
        reverse_proxy 127.0.0.1:12346
    }
    handle /ready {
        reverse_proxy 127.0.0.1:12346
    }

    # Block the Prometheus surface from being public.
    # Operators scrape it from inside the VPC over 9100.
    handle /metrics {
        respond "Not exposed publicly. See ops docs." 404
    }
}
```

Install + enable:

```bash
sudo apt install -y caddy
sudo cp Caddyfile /etc/caddy/Caddyfile
sudo systemctl reload caddy
```

For a follower exposing their own REST surface, change the host to their domain. For a follower not exposing anything publicly (just running their own chain mirror locally), skip Caddy entirely and only `ligate-node` needs to run.

## Step 4: Verify the bring-up

From your laptop:

```bash
# Health probes (operator concern; always 200 if the binary is alive).
curl https://rpc.ligate.io/health
# {"status":"ok"}

curl https://rpc.ligate.io/ready
# 200 with {"status":"synced","synced_da_height":N} once caught up
# 503 with {"status":"syncing","synced_da_height":N,"target_da_height":M} during sync

# Chain identity (the network operators are talking to).
curl https://rpc.ligate.io/v1/rollup/info
# {"chain_id":"ligate-devnet-1","chain_hash":"<64 hex>","version":"X.Y.Z"}

# Followers cross-check chain_hash against the canonical Ligate-Labs node:
diff <(curl -s https://rpc.ligate.io/v1/rollup/info | jq -r .chain_hash) \
     <(curl -s https://your-follower.example.com/v1/rollup/info | jq -r .chain_hash)
# Empty diff => same runtime composition => safe to interop.
```

## Step 5: Monitoring

The chain ships Prometheus metrics on `127.0.0.1:9100/metrics` (separate port from the chain REST). Scrape from inside the VPC; do NOT expose externally.

GCP-friendly path: install Cloud Monitoring's OpenTelemetry collector on the VM and point it at `127.0.0.1:9100`. Alternatively, run a Prometheus + Grafana on a separate VM and scrape over the internal network.

The metric set + labels lives in `crates/rollup/src/metrics.rs` and the umbrella issue [#110](https://github.com/ligate-io/ligate-chain/issues/110). Phase 2 metrics shipped in [#167-#171](https://github.com/ligate-io/ligate-chain/issues/167); three metrics (mempool depth, DA submission latency, DA failure-by-reason) remain blocked on SDK upstream and are tracked in [#164](https://github.com/ligate-io/ligate-chain/issues/164).

A starter Grafana dashboard is tracked in [#165](https://github.com/ligate-io/ligate-chain/issues/165). Until that lands, the per-metric `HELP` strings in `/metrics` output document each one inline.

## Step 6: Backups

Two persistent surfaces need backup:

- `/var/lib/ligate` (RocksDB + NOMT state). Recoverable from genesis + DA replay if lost, but a fresh sync takes hours and operators want fast restart.
- `/etc/ligate/env` (sequencer signer key, faucet hot key). Cannot be regenerated; protect.

Cron-driven rsync of state to a Cloud Storage bucket every 6 hours:

```bash
# /etc/cron.d/ligate-backup
0 */6 * * * ligate /usr/bin/gsutil -q -m rsync -r -d /var/lib/ligate gs://ligate-backups/devnet-1/$(hostname)/state
```

Secrets go to Secret Manager, not GCS:

```bash
gcloud secrets create ligate-devnet-1-sequencer-signer \
    --replication-policy=user-managed --locations=us-central1
gcloud secrets versions add ligate-devnet-1-sequencer-signer --data-file=-
# (paste the private key, then EOF)
```

The `ligate-node` systemd unit reads from `/etc/ligate/env`, which on first boot is hydrated from Secret Manager (custom systemd dependency or operator-run script). Avoids the secret ever sitting on disk in plaintext outside the OS keyring.

## Failure-mode triage

| Symptom | Likely cause | First check | Fix |
|---|---|---|---|
| `ligate-node` won't start, "missing [chain] section" | Wrong config file passed | `journalctl -u ligate-node` | Check `--rollup-config-path` points at `/opt/ligate/devnet-1/celestia.toml`, not the localnet one |
| `ligate-node` won't start, "chain_id 'X' does not match the locked ladder" | Operator typo in `[chain]` | `cat /opt/ligate/devnet-1/celestia.toml \| grep chain_id` | Fix to `ligate-devnet-1`, restart |
| Sequencer cannot reach Celestia | Light node down or wrong endpoint | `systemctl status celestia-light`, `curl ws://127.0.0.1:26658` | Restart `celestia-light`; if persistent, check Mocha bridge availability |
| `/ready` returns 503 with high target_da_height | Node is catching up | Look at `synced_da_height` over 5 min | Wait; sync rate ~1 block/s on healthy network |
| `/ready` returns 503 indefinitely | Sequencer can't keep up with DA | Check `block_height` metric and DA polling rate | Increase `da_polling_interval_ms` upward, or check for state-corruption indicators |
| Disk fills | RocksDB compaction lag, log spam, or backup not pruning | `du -sh /var/lib/ligate/*` | Identify largest subdir, prune (logs ok to delete; state requires re-sync) |
| `/v1/rollup/info` `chain_hash` doesn't match the canonical node's | Genesis JSON drift or different binary version | `curl /v1/rollup/info` on both, diff | Operator pulled wrong genesis bundle or different `version`; reinstall from canonical source |
| Sequencer signer key compromised | Audit + assume compromise | Move TIA out of the wallet, generate new key, re-genesis | One-shot: rotate via re-genesis to `devnet-2`. v1+: upgrade module ([#42](https://github.com/ligate-io/ligate-chain/issues/42)) handles in-place |
| Mocha resets | Celestia testnet wiped | Watch [Celestia announcements](https://discord.gg/celestiacommunity) | Re-genesis (next section) |

## Mocha reset re-genesis playbook

Mocha resets roughly once a year. When it does, our chain's DA-layer references go stale; the cleanest recovery is to spin up a fresh chain id (`ligate-devnet-2`) on the new Mocha epoch.

Steps:

1. Wait for the Celestia team's announcement of the new Mocha genesis hash.
2. In `ligate-chain`, copy `devnet-1/` to `devnet-2/`. Edit:
   - `celestia.toml`: bump `chain_id = "ligate-devnet-2"`, update `storage.path = "devnet-2/data-celestia"`.
   - `genesis/`: regenerate via `ligate-genesis-tool` ([#191](https://github.com/ligate-io/ligate-chain/issues/191)), keeping the same operator-controlled keys.
3. Add CI test rows in `crates/rollup/tests/devnet_config.rs` for `devnet-2/` (mirror the `devnet_1_*` tests).
4. Land the PR; tag a new release (`v0.2.0-devnet-2` or similar).
5. Operators redeploy: pull the new binary + new `devnet-2/` configs, point at the new Mocha endpoint, restart.
6. Update DNS: `rpc.ligate.io` continues pointing at the same VM; the chain just runs the new chain-id from now on.

The four-row ladder (`ligate-localnet`, `ligate-devnet-N`, `ligate-testnet-N`, `ligate-N`) is designed for exactly this: trailing number bumps on state-breaking restarts, no other reason.

## Follower-specific notes

Follower mode is a strict subset of the sequencer setup:

- No faucet service.
- No `SOV_CELESTIA_SIGNER_KEY` (followers don't submit blobs, no TIA needed).
- No external TLS endpoint required (followers can run private; if they want to expose their own REST, the Caddy step is identical with their own domain).
- Smaller VM (`e2-medium`).

The verification command from Step 4 is the canonical "am I caught up to the right chain" check:

```bash
# On the follower:
curl http://127.0.0.1:12346/v1/rollup/info | jq .chain_hash
# Compare against:
curl https://rpc.ligate.io/v1/rollup/info | jq .chain_hash
# Equal => you're tracking the canonical chain.
```

If the hashes diverge, the follower is on the wrong genesis bundle (the `devnet-1/genesis/` they pulled was from a different commit), the wrong binary version (different runtime composition produces a different `chain_hash`), or the wrong chain_id entirely.

## What the runbook deliberately omits

- **Helm charts and Kubernetes manifests.** No k8s in v0.
- **Multi-region failover.** Single-sequencer chain; failover requires #82 multi-sequencer first.
- **HSM-backed signer keys.** Pre-mainnet polish.
- **Alertmanager rules.** Operator-specific; sample rules can ship with [#165](https://github.com/ligate-io/ligate-chain/issues/165) Grafana dashboard.

For each, file an issue or extend this runbook when the need arises.
