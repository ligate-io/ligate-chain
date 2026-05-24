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

| Role | GCP shape | vCPU | RAM | Boot disk | Data disk | $/mo (sustained-use) |
|---|---|---|---|---|---|---|
| Sequencer | `e2-standard-4` | 4 | 16 GB | 20 GB SSD | 150 GB SSD | ~135 USD |
| Follower | `e2-medium` | 2 | 4 GB | 20 GB SSD | 100 GB SSD | ~30 USD |

Sequencer sizing covers `ligate-node` + co-located Celestia light node + the local backup snapshot stage (`/var/lib/ligate/snapshots/`, which keeps last 24 hourly + 4 weekly per `backup-rocksdb.sh` rotation; the daily tier was dropped in chain#468). RocksDB NOMT files are sparse but rsync inflates them on copy, so each local snapshot is currently ~5GB; steady-state worst case for local snapshots is ~120GB (tracked under #359 followups; `--sparse` flag in the backup script would cut this ~4x).

Faucet and indexer **do not run on this VM**; they live in [`ligate-io/ligate-api`](https://github.com/ligate-io/ligate-api), deployed to Railway alongside a Railway-managed Postgres. The Next.js explorer at `explorer.ligate.io` is yet another deploy on Vercel, talking to `api.ligate.io`. Follower sizing is smaller because followers don't keep the local backup stage (they restore from GCS or replay from DA if state is lost).

Persistent data disk can be **live-resized without downtime** via `gcloud compute disks resize` + `resize2fs /dev/sdb`; ext4 supports online growth. Done on `ligate-devnet-1-sequencer` on 2026-05-16 (50GB → 150GB) when local snapshot accumulation projected to fill the original 50GB within ~24h.

**OS choice.** As of chain#345 the released `ligate-node` Linux binary is built on Ubuntu 22.04 (GLIBC 2.35), so it runs cleanly on every common server distro: Ubuntu 22.04+, Debian 12+, RHEL 9, Amazon Linux 2023. Older distros (GLIBC <2.35) still need a from-source build. Ubuntu 24.04 stays the canonical operator choice for the rest of the runbook because cloud-init + GCP image families are stable on it, but Debian 12 works fine too if you prefer.

## Step 1: Provision the VM

```bash
gcloud compute instances create ligate-devnet-1-sequencer \
    --project=$YOUR_GCP_PROJECT \
    --zone=us-central1-a \
    --machine-type=e2-standard-4 \
    --image-family=ubuntu-2404-lts-amd64 \
    --image-project=ubuntu-os-cloud \
    --boot-disk-size=20GB \
    --create-disk=name=ligate-data,size=150GB,type=pd-ssd \
    --tags=http-server,https-server \
    --metadata-from-file=user-data=ops/cloud-init/sequencer.yaml
```

For a follower, swap `--machine-type=e2-medium`, `--create-disk=...,size=100GB`, and rename the instance.

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

The `cloud-init.yaml` referenced above is checked in at [`ops/cloud-init/sequencer.yaml`](../../ops/cloud-init/sequencer.yaml). Pass it to `gcloud compute instances create` via `--metadata-from-file=user-data=ops/cloud-init/sequencer.yaml` (as in the Step 1 invocation above).

What it provisions:

- Build tooling (rustc + cargo via rustup, plus `build-essential` / `clang` / `pkg-config` / `libssl-dev` so a from-source `cargo build --release --bin ligate-node` works if a release tarball isn't usable).
- The `ligate` system user, with `/opt/ligate`, `/etc/ligate`, and `/var/lib/ligate` ownership.
- The persistent data disk formatted as ext4 and mounted at `/var/lib/ligate` (idempotent — re-running cloud-init on a VM with an existing FS doesn't reformat). `/var/lib/ligate/rocksdb` is pre-created so the Step 3 storage symlink lands somewhere valid.
- `celestia-light` v0.30.2 installed to `/usr/local/bin/celestia`, initialised against Mocha, and a `celestia-light.service` systemd unit that auto-starts on boot.
- A `ligate-node.service` systemd unit (with `TimeoutStopSec=300` so RocksDB compaction has time to finish — chain#362 retro). **Not enabled** — would crashloop on missing binary; operator starts it manually after Step 3.
- `chrony` enabled as belt-and-braces NTP.

The cloud-init does the chassis. The operator still has to:

- Place the `ligate-node` binary at `/opt/ligate/bin/ligate-node`. From a tag onward, the canonical artifact is a GitHub Release tarball (`ligate-node-${VERSION}-linux-amd64.tar.gz` or `linux-arm64.tar.gz` for the Linux server case; `darwin-arm64` and `darwin-amd64` are also published for developer tooling). The release workflow at `.github/workflows/release.yml` produces all four targets on every `v*` tag. Pre-tag operators build from source via `cargo build --release --bin ligate-node`.
- Clone the chain repo's `devnet-1/` directory to `/opt/ligate/devnet-1/`.
- **Symlink the chain's storage path to the persistent disk** before first boot. The rollup config (`devnet-1/celestia.toml`) sets `[storage] path = "devnet-1/data-celestia"` relative to ligate-node's working directory (`/opt/ligate`). The cloud-init above pre-created `/var/lib/ligate/rocksdb` on the persistent disk; pointing the chain path at it is one command:
  ```bash
  sudo -u ligate ln -s /var/lib/ligate/rocksdb /opt/ligate/devnet-1/data-celestia
  ```
  Without this, the chain writes state to the 20 GB boot disk and a VM image rebuild loses chain history. Backups (per `docs/development/runbooks/backup-restore.md`) reduce the blast radius to ~15 minutes' RPO, but the persistent disk is the right home regardless. (Done after the fact on `ligate-devnet-1-sequencer` on 2026-05-16, runbook documents the in-place migration.)
- **Pin `genesis_da_height` before first boot.** `devnet-1/genesis/chain_state.json` ships `genesis_da_height: 0` — a placeholder. Celestia rejects a request for DA block 0, so the chain cannot initialize against Mocha while it's `0` (this is exactly what the Mocha smoke gate surfaced; see [#331](https://github.com/ligate-io/ligate-chain/issues/331)). Set it to a recent **finalized** Mocha height:
  ```bash
  # Latest Mocha consensus height, minus a finality margin.
  MOCHA_HEIGHT=$(curl -s https://rpc-mocha.pops.one/status | jq -r .result.sync_info.latest_block_height)
  PIN=$((MOCHA_HEIGHT - 20))
  jq ".genesis_da_height = $PIN" /opt/ligate/devnet-1/genesis/chain_state.json \
      > /tmp/chain_state.json && mv /tmp/chain_state.json /opt/ligate/devnet-1/genesis/chain_state.json
  ```
  The chain starts reading DA from this height, so it must be recent — a stale value forces the node to sync months of Mocha history before producing its first slot. This is a launch-time value: re-pin it on every re-genesis, including `devnet-N` rotation after a Mocha reset (see the re-genesis playbook below).
- Write `/etc/ligate/env` with the secrets (sequencer only):
  ```
  SOV_CELESTIA_RPC_AUTH_TOKEN=<from "celestia light auth admin">
  SOV_CELESTIA_GRPC_URL=http://127.0.0.1:9090
  SOV_CELESTIA_SIGNER_KEY=<hex private key holding Mocha TIA>
  RUST_LOG=info,sov=info
  ```
  Every node needs `SOV_CELESTIA_SIGNER_KEY` set to the preferred sequencer's private key (DbElected mode: all nodes share the same Celestia signer; only the current lock-holder posts blobs). The `--mode follower` flag and its associated read-only mode were removed in chain#446 after a paper-leader incident; if you want a true read-only RPC node that doesn't run the sequencer at all, that's tracked as chain#447.
- `systemctl enable --now ligate-node.service`. The unit file already has the right `ExecStart` for DbElected operation:
  ```
  ExecStart=/opt/ligate/bin/ligate-node \
      --da-layer celestia \
      --rollup-config-path /opt/ligate/devnet-1/celestia.toml \
      --genesis-config-dir /opt/ligate/devnet-1/genesis
  ```
  At runtime the SDK's DbElected machinery elects one leader via the Postgres lock; the others heartbeat and serve reads. Non-leader nodes return 503 on `POST /v1/sequencer/txs` automatically (no chain-side middleware needed).
- If you'll run cold backups (weekly tier stops ligate-node briefly for a consistent rsync — see `docs/development/runbooks/backup-restore.md`), install the sudoers drop-in:
  ```bash
  sudo install -o root -g root -m 0440 \
      ops/backup/sudoers.d/ligate-backup /etc/sudoers.d/ligate-backup
  ```
  That allows the `ligate` user to `systemctl stop/start ligate-node.service` without a password prompt. Narrow scope; only those two commands, only on that one service.

## Step 2.5: NTP / clock sync (required)

`ligate-node` stamps each slot with `Time::now()` at production. Operators whose host clocks disagree by more than a second produce skewed `slot.timestamp` values, which corrupts dashboards / SLA computations and — once attesters are in the loop — can drive quorum disputes when attesters disagree on whether a batch arrived before its deadline.

This step is not optional on bare-metal. GCP, AWS, and Azure all run managed NTP by default and the cloud-init above installs `chrony` as belt-and-braces. Verify before starting `ligate-node`:

```bash
chronyc tracking | grep -E "Reference|Stratum|System time|Last offset"
# Expected: System time drift < 1.0 seconds, Last offset within a few ms
```

If `chronyc tracking` reports drift > 1s or shows the system as unsynchronised, fix it before starting the node. Common causes:

- **Air-gapped or firewall-blocked machine.** `chrony` defaults to public NTP pools; if outbound UDP/123 is blocked you'll never sync. Either open the port to `pool.ntp.org`, or point `chrony` at an internal NTP server via `/etc/chrony/chrony.conf` (`server <internal-ntp> iburst` line).
- **VM clock drift after host migration.** GCP live-migration occasionally jumps the clock. `chronyc makestep` once to correct, then verify with `chronyc tracking`.
- **macOS workstation hosting `ligate-node` for local dev.** The system uses `timed` rather than chrony; verify with `sudo sntp -sS time.apple.com`. Same < 1s requirement.

### Failure mode if drift exceeds threshold

The chain doesn't refuse to start with a skewed clock — it just produces bad data. What you observe:

- `slot.timestamp` values from this sequencer drift relative to wall clock and to other operators' followers.
- Dashboards that compute "blocks per second" from timestamps misreport.
- Once attesters check timestamp claims against their own clocks (post-#268 attester-side checks), attesters disagree on whether a batch is fresh enough; quorum disputes appear in attester logs.

The simplest mitigation is to never let drift get to that point. The `chronyc tracking` check above takes 2 seconds to run. Wire it into your monitoring (the [#268](https://github.com/ligate-io/ligate-chain/issues/268) Alertmanager work has a candidate `clock_skew_seconds` metric for exactly this).

### Optional: clock-skew Prometheus metric

A follow-up to this section: expose `ligate_node_clock_skew_seconds` from `ligate-node` itself by comparing `Time::now()` against an external NTP source on a 30s tick. Pair with an Alertmanager rule:

```yaml
- alert: LigateNodeClockSkew
  expr: abs(ligate_node_clock_skew_seconds) > 1.0
  for: 2m
  annotations:
    runbook: docs/development/public-devnet-deploy.md#step-25-ntp--clock-sync-required
```

Tracked as a follow-up to [#268](https://github.com/ligate-io/ligate-chain/issues/268).

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

    # Swagger UI hot-fix #1: the bundled swagger-initializer.js
    # hardcodes `url: "/openapi-v3.json"` (absolute, no /v1/ prefix).
    # The spec actually lives at /v1/openapi-v3.json (the chain mounts
    # everything under /v1). Rewrite the request internally so the
    # browser request succeeds without touching the chain binary.
    handle /openapi-v3.json {
        rewrite * /v1/openapi-v3.json
        reverse_proxy 127.0.0.1:12346
    }

    # Swagger UI hot-fix #2: utoipa (the SDK's openapi-spec generator)
    # hardcodes `openapi: "3.1.0"` as a single-variant enum that the
    # chain can't downgrade from code. The bundled swagger-ui assets
    # the chain ships are an older build that refuses to render any
    # spec declaring 3.1.x, even when the content has no 3.1-only
    # features. Override the bundled assets with swagger-ui-dist@5+
    # served from disk; the modern bundle renders 3.1 natively.
    #
    # Install the assets (one-time):
    #   sudo mkdir -p /var/www/swagger-ui
    #   curl -sL https://github.com/swagger-api/swagger-ui/archive/refs/tags/v5.17.14.tar.gz \
    #     | sudo tar -xz --strip-components=2 -C /var/www/swagger-ui \
    #       swagger-ui-5.17.14/dist
    #   sudo tee /var/www/swagger-ui/swagger-initializer.js >/dev/null <<'EOF'
    #   window.onload = function() {
    #     window.ui = SwaggerUIBundle({
    #       url: "/openapi-v3.json",
    #       dom_id: "#swagger-ui",
    #       deepLinking: true,
    #       layout: "StandaloneLayout",
    #       presets: [SwaggerUIBundle.presets.apis, SwaggerUIStandalonePreset],
    #       plugins: [SwaggerUIBundle.plugins.DownloadUrl],
    #     });
    #   };
    #   EOF
    #   sudo chown -R caddy:caddy /var/www/swagger-ui
    #
    # Drop this handle + the /var/www/swagger-ui assets once Sovereign
    # SDK bumps its bundled swagger-ui to 5+ (tracking: ligate-io/ligate-chain#394).
    handle_path /v1/swagger-ui/* {
        root * /var/www/swagger-ui
        try_files {path} /index.html
        file_server
    }
    handle /v1/swagger-ui {
        redir /v1/swagger-ui/ 308
    }

    # Block the Prometheus surface from being public.
    # Operators scrape it from inside the VPC over 9100.
    handle /metrics {
        respond "Not exposed publicly. See ops docs." 404
    }
}
```

Production also wraps the above with a `(cloudflare_only)` macro that 403s any request whose source IP isn't in Cloudflare's published edge ranges. See `/etc/caddy/Caddyfile` on `ligate-devnet-1-sequencer` (or the CF-IP list at <https://www.cloudflare.com/ips-v4/>) for the macro. Skip for non-Cloudflare deploys.

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

## Step 4.5: Register canonical schemas (run from a workstation)

`devnet-1/genesis/attestation.json` ships with `initial_attestor_sets: []` and `initial_schemas: []`. Once the chain has produced a few blocks and `/v1/rollup/info` returns a non-zero `chain_hash`, register the canonical Themisra schema (`themisra.proof-of-prompt/v1`) via the [`ligate-bootstrap`](../../crates/bootstrap-cli/) ceremony from chain repo PR #249.

This step runs from your **workstation** (not the GCP VM) — the operator key only needs to be loaded into the bootstrap-cli's process briefly to sign two registration txs. The VM never sees the operator key in this flow.

```bash
# 1. From your workstation, in the chain repo:
cp devnet-1/canonical-schemas.toml.example devnet-1/canonical-schemas.toml
$EDITOR devnet-1/canonical-schemas.toml  # fill in real attestor pubkeys

# 2. Run the ceremony binary. Auto-fetches chain_hash from /v1/rollup/info.
#    --chain-id is the numeric u64 CHAIN_ID from constants.toml (NOT
#    the "ligate-devnet-1" human-readable string).
cargo run --release -p ligate-bootstrap-cli -- register-canonical-schemas \
    --config devnet-1/canonical-schemas.toml \
    --signer-key ~/.ligate-keys/devnet-1/operator.key \
    --rpc https://rpc.ligate.io \
    --chain-id 4242

# 3. Verify the schema landed.
curl https://rpc.ligate.io/v1/modules/attestation/schemas/lsc1...
```

Idempotent — re-running skips already-landed registrations. Full operator runbook with troubleshooting at [`canonical-schema-registration.md`](canonical-schema-registration.md).

This step happens **before** announcing devnet to partners; the schema_id needs to be published in `v0.1.0-devnet` release notes alongside the rpc URL and chain_hash.

## Step 4.7: Faucet — does NOT run on this VM

Pre-launch architecture moved the faucet off the chain VM. It now runs as part of [`ligate-io/ligate-api`](https://github.com/ligate-io/ligate-api) (unified Rust HTTP service hosting drip + indexer queries) on Railway, against a Railway-managed Postgres. The chain VM only runs `ligate-node` + the Celestia light node.

Concretely:

- Chain (`rpc.ligate.io`) lives on this GCP VM.
- API (`api.ligate.io/v1/drip` for faucet, `/v1/blocks*` etc. for indexer queries) lives on Railway.
- Explorer (`explorer.ligate.io`) lives on Vercel as a Next.js app talking to `api.ligate.io`.

Faucet deploy is documented in [`ligate-api` README](https://github.com/ligate-io/ligate-api). The operator transfers an initial 1M AVOW from the operator key (one-shot from a workstation via `ligate transfer`) to the faucet hot-key address; the api service holds the faucet hot key as a Railway secret and uses it to sign drip txs.

Operational layer (key rotation, balance monitoring, abuse mitigation, treasury top-up): [`runbooks/faucet-ops.md`](runbooks/faucet-ops.md).

This split keeps the GCP VM minimal (`e2-medium`, ~$25/mo, only chain + Celestia DA processes) and lets the api scale independently from the chain.

## Step 5: Monitoring

The chain ships Prometheus metrics on `127.0.0.1:9100/metrics` (separate port from the chain REST). Scrape from inside the VPC; do NOT expose externally.

GCP-friendly path: install Cloud Monitoring's OpenTelemetry collector on the VM and point it at `127.0.0.1:9100`. Alternatively, run a Prometheus + Grafana on a separate VM and scrape over the internal network.

The metric set + labels lives in `crates/rollup/src/metrics.rs` and the umbrella issue [#110](https://github.com/ligate-io/ligate-chain/issues/110). Phase 2 metrics shipped in [#167-#171](https://github.com/ligate-io/ligate-chain/issues/167); three metrics (mempool depth, DA submission latency, DA failure-by-reason) remain blocked on SDK upstream and are tracked in [#164](https://github.com/ligate-io/ligate-chain/issues/164).

A starter Grafana dashboard is tracked in [#165](https://github.com/ligate-io/ligate-chain/issues/165). Until that lands, the per-metric `HELP` strings in `/metrics` output document each one inline.

Alerting rules ship in [`ops/prometheus/alerts.yaml`](../../ops/prometheus/alerts.yaml) with Alertmanager routing in [`ops/prometheus/alertmanager.yaml`](../../ops/prometheus/alertmanager.yaml). Each alert has a runbook under [`docs/development/runbooks/alerts/`](runbooks/alerts/README.md) keyed by alert name; the `runbook_url` annotation on each rule routes the on-call to the right file via the firing notification. Load `alerts.yaml` via `rule_files:` on the Prometheus server; configure Alertmanager with the rendered `alertmanager.yaml` (Slack webhook + SMTP password injected at deploy time, never in repo).

The Celestia light node has its own monitoring story (its own `/metrics` endpoint, its own failure modes); the health-check + recovery procedure lives at [`runbooks/celestia-light-node.md`](runbooks/celestia-light-node.md).

## Step 6: Backups

Full backup/restore procedure: [`runbooks/backup-restore.md`](runbooks/backup-restore.md). Scripts ship at [`scripts/backup-rocksdb.sh`](../../scripts/backup-rocksdb.sh) + [`scripts/restore-rocksdb.sh`](../../scripts/restore-rocksdb.sh); systemd units at [`ops/backup/systemd/`](../../ops/backup/systemd/); GCS lifecycle at [`ops/backup/gcs-lifecycle.json`](../../ops/backup/gcs-lifecycle.json).

Three timers (hourly/daily/weekly) covering the dense-recent / sparse-old retention curve described in the runbook. Quarterly restore drill via [`scripts/restore-drill.sh`](../../scripts/restore-drill.sh) on a non-prod VM proves the restore path works without waiting for an incident.

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
3. Add CI test rows in `crates/rollup/tests/localnet_config.rs` for `devnet-2/` (mirror the `devnet_1_*` tests).
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

## Container variant (`ghcr.io/ligate-io/ligate-chain`)

The default systemd-on-VM flow above ships `ligate-node` as a native binary. Operators who prefer containers can swap step 2's systemd unit for a `docker run` invocation against the published GHCR image (#195). The image is built by `.github/workflows/docker.yml` on every `v*` tag for both `linux/amd64` and `linux/arm64`.

```bash
docker run -d \
    --name ligate-node \
    --restart unless-stopped \
    -v /var/lib/ligate:/var/lib/ligate \
    -v /opt/ligate/devnet-1:/opt/ligate/devnet-1:ro \
    -p 12346:12346 -p 9100:9100 \
    -e SOV_CELESTIA_RPC_URL=ws://host.docker.internal:26658 \
    -e SOV_CELESTIA_GRPC_URL=http://host.docker.internal:9090 \
    -e SOV_CELESTIA_SIGNER_KEY="$(gcloud secrets versions access latest --secret=ligate-devnet-1-sequencer-signer)" \
    -e RUST_LOG=info,sov=info \
    ghcr.io/ligate-io/ligate-chain:v0.3.0 \
    --da-layer celestia \
    --rollup-config-path /opt/ligate/devnet-1/celestia.toml \
    --genesis-config-dir /opt/ligate/devnet-1/genesis \
    --metrics-bind 0.0.0.0:9100
```

Caddy still runs natively on the host (or in its own container; either is fine). The Celestia light node also runs separately, since the chain image only carries `ligate-node`.

The image is unprivileged by default (UID 1000 `ligate` user) and runs `--help` if invoked without arguments. Tag `:latest` pins the most recent full release; use a specific `:vX.Y.Z` tag in production.

### Docker Compose variant (recommended for follower operators)

For the simplest "pull-and-go" experience, the chain repo ships a checked-in compose file at [`examples/docker-compose.yaml`](../../examples/docker-compose.yaml) (#221) that orchestrates `ligate-node` + a Celestia light node together. Three steps:

```bash
git clone https://github.com/ligate-io/ligate-chain
cd ligate-chain/examples
cp .env.example .env  # edit if running as sequencer
docker-compose up -d
```

After ~10 minutes for Celestia to header-sync, the node is reachable on `127.0.0.1:12346`. See [`examples/README.md`](../../examples/README.md) for full instructions including sequencer-mode setup and genesis-substitution flow.

The systemd-on-VM path above is preferred for the canonical Ligate Labs sequencer deploy (better journald integration, per-service restart policies, secret-store integration). The compose path is preferred for follower operators and casual one-host setups.

## What the runbook deliberately omits

- **Helm charts and Kubernetes manifests.** No k8s in v0.
- **Multi-region failover.** Single-sequencer chain; failover requires #82 multi-sequencer first.
- **HSM-backed signer keys.** Pre-mainnet polish.
- **Alertmanager rules.** Operator-specific; sample rules can ship with [#165](https://github.com/ligate-io/ligate-chain/issues/165) Grafana dashboard.

For each, file an issue or extend this runbook when the need arises.
