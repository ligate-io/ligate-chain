# Upgrading `ligate-node` on a devnet VM

> One-command in-place binary swap for tagged releases. Auto-rollback
> on start failure or RPC mismatch so a broken release can't leave the
> VM in a stopped state. Tracking: `ligate-io/ligate-chain#349`.

## When to use this

- Patch + minor releases where the CHANGELOG marks `chain_hash unchanged` (safe binary swap; no state quarantine, no replay).
- Any operator-only VM running the canonical `/opt/ligate/...` layout from [`public-devnet-deploy.md`](../public-devnet-deploy.md).

## When NOT to use this

- Releases that bump `chain_hash` (genesis or STF change). Those need the rebuild-from-DA flow in [`public-devnet-deploy.md`](../public-devnet-deploy.md); the upgrade script intentionally does not touch RocksDB.
- Multi-VM clusters under DbElected. The script upgrades one VM at a time; for a cluster, rolling-upgrade the replicas first then the leader (procedure in [`multi-sequencer-failover.md`](multi-sequencer-failover.md)).

## Install the helper

Pull the latest from `main` and drop the script onto the VM. One-time per VM:

```bash
sudo install -m 0755 scripts/upgrade-chain.sh /opt/ligate/scripts/upgrade-chain.sh
```

Then upgrade in one line:

```bash
sudo /opt/ligate/scripts/upgrade-chain.sh 0.2.13
# or `v0.2.13` — the leading `v` is tolerated
```

## What it does

1. Resolves the release tarball URL for `linux-amd64` / `linux-arm64` based on `uname -m`.
2. `HEAD`-checks the tarball + `.sha256` sidecar before touching anything (so a typo'd version aborts cleanly).
3. Snapshots `/opt/ligate/bin/ligate-node` to `/opt/ligate/bin/ligate-node.prev` as the rollback target.
4. `systemctl stop ligate-node`.
5. `install -m 0755 -o ligate -g ligate` the new binary into place.
6. `systemctl start ligate-node` + polls `is-active` (timeout 90 s default).
7. Curl-checks `127.0.0.1:12346/v1/rollup/info` reports the target version (timeout 30 s after active).
8. If 6 or 7 fail: automatically restores `.prev`, restarts, exits non-zero.

Net effect on a healthy upgrade: ~10 s of RPC unavailability (`stop` → `start` + warmup).

## Manual rollback

If the upgrade went green but you want to revert later (e.g. a deeper issue surfaced post-upgrade):

```bash
sudo /opt/ligate/scripts/upgrade-chain.sh rollback
```

This swaps `.prev` back in and restarts. `.prev` is overwritten on every successful upgrade, so `rollback` always lands on the *immediately previous* binary, not arbitrary versions.

## Dry-run

```bash
sudo /opt/ligate/scripts/upgrade-chain.sh --dry-run 0.2.13
```

Prints the resolved version / arch / URLs / paths and exits. Doesn't touch the binary or the service. Use to sanity-check env-var overrides.

## Environment overrides

The script reads sensible defaults but every path is overridable so a non-standard install works:

| Var | Default | Notes |
|---|---|---|
| `LIGATE_BIN` | `/opt/ligate/bin/ligate-node` | Path to the running binary |
| `LIGATE_USER` / `LIGATE_GROUP` | `ligate` / `ligate` | Ownership for the swapped-in binary |
| `LIGATE_SERVICE` | `ligate-node` | Systemd unit name |
| `LIGATE_RPC_URL` | `http://127.0.0.1:12346` | Used for the post-start version check |
| `GH_OWNER_REPO` | `ligate-io/ligate-chain` | Tarball source |
| `START_TIMEOUT_SECS` | `90` | Wait for `systemctl is-active` |
| `WORKDIR` | `/tmp/ligate-upgrade` | Scratch dir for download + extract |

## Verification checklist (post-upgrade)

The script does the version + active checks for you, but for any release worth a Grafana glance:

1. `curl -s http://127.0.0.1:12346/v1/rollup/info | jq .version` — matches expected
2. `curl -s http://127.0.0.1:9100/metrics | head -5` — Prometheus surface up
3. Grafana **Cluster topology** panel shows the VM as a single `leader` (no flapping)
4. `journalctl -u ligate-node -n 50` — no panics, no repeated 5xx

## Known limitations

- **No remote orchestration.** This is a per-VM script. Multi-VM upgrades are an SSH-loop today; an Ansible / fleet runbook is a follow-up.
- **Doesn't touch genesis or RocksDB.** If a release bumps `chain_hash` or migrates storage, the script aborts at the post-start version check (the new binary may refuse to boot against the old state); the auto-rollback recovers, but the operator needs to follow the genesis-change procedure.
- **`.prev` is one-deep.** Two consecutive upgrades clobber the original `.prev`. If you may want to roll back multiple versions, archive `/opt/ligate/bin/ligate-node` to `~/ligate-archive/` manually before re-running the script.

## Related

- [`public-devnet-deploy.md`](../public-devnet-deploy.md) — fresh-install procedure
- [`runbooks/multi-sequencer-failover.md`](multi-sequencer-failover.md) — rolling upgrades across a DbElected cluster
- [`snapshot-bootstrap.md`](../snapshot-bootstrap.md) — when you'd reach for a fresh state import instead
