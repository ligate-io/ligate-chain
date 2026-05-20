# Runbook: Multi-Sequencer Failover (DbElected)

**Status:** v0. Covers DbElected mode on devnet-1 (Postgres-backed
leader election + heartbeat). Documents what we have **live-tested** vs
what is **SDK-documented but not yet exercised in production**. Part of
the multi-sequencer rollout tracked at
[chain#82](https://github.com/ligate-io/ligate-chain/issues/82) (umbrella)
and [chain#422](https://github.com/ligate-io/ligate-chain/issues/422)
(devnet rollout).

This page is about Ligate's `DbElected` sequencer mode: the Sovereign
SDK feature where multiple `ligate-node` processes register with a shared
Postgres database, one acquires a leader lock to produce batches, and the
rest stand by as Replicas ready to take over. It is **not** about
Celestia DA failover or generic sequencer key rotation; for those see
[`celestia-light-node.md`](./celestia-light-node.md) and
[`sequencer-key-rotation.md`](./sequencer-key-rotation.md).

---

## When does failover apply?

| Cause | Severity | What it blocks |
|---|---|---|
| **Leader process crash** (panic, OOM) | High | Batch production halts until a Replica wins the next lock cycle. RPC `submit_tx` returns 503 on the dead node; clients should retry against the load-balanced public endpoint. |
| **Leader VM unhealthy** (kernel hung, disk full, network partition) | High | Heartbeat lapses → lock expires → election unblocks. Recovery clock starts at the heartbeat-timeout boundary, not the moment of failure. |
| **Postgres unreachable** (Cloud SQL maintenance, network outage) | Critical | All nodes stop progressing election. Existing Leader may continue briefly under its current lock TTL, then loses authority. No batches produced until Postgres is reachable again. |
| **Planned Leader replacement** (binary upgrade on the active node) | Low | Stop the Leader cleanly; a Replica takes over within one election cycle. Then upgrade and rejoin as Replica. |

The first three are operational incidents. The fourth is the happy-path
workflow we use for zero-downtime ops releases.

---

## What "Leader" means in DbElected mode

Three sequencer roles exist at the **slot level** in
`rollup.toml → [sequencer.preferred.postgres_config]`:

| `node_role` | Behaviour |
|---|---|
| `Leader` | Competes for and holds the lock unconditionally. Produces batches when it wins. |
| `Replica` | Heartbeats but **never** tries to acquire the lock. Cold standby; useful for "I want this node visible in the cluster but never producing batches". |
| `DbElected` | Heartbeats and competes for the lock. The standard production role. |

Separately, the **binary-level role** (set via `--mode` / `LIGATE_MODE`
in `/etc/ligate/env`) is `Sequencer` or `Follower`:

- `Sequencer` mode runs the sequencer slot, including the Postgres
  heartbeat task and (if `node_role != Replica`) leader-election machinery.
- `Follower` mode syncs the STF from DA but does **not** produce batches.
  It does still run the heartbeat task if `postgres_config` is present,
  which is how a `Follower + Replica` slot ends up registered in the
  `nodes` table.

In practice on devnet-1 today:

- VM-1 (production sequencer): `LIGATE_MODE=sequencer`, `node_role=DbElected` (after cutover). The Leader candidate.
- VM-2 + VM-3 (cold replicas): `LIGATE_MODE=follower`, `node_role=Replica`. Never compete for leadership, just heartbeat + sync.

To turn a Replica into a hot Leader candidate, both flip simultaneously:
`LIGATE_MODE` → `sequencer` and `node_role` → `DbElected`.

---

## Reading cluster state

All state lives in Postgres (`ligate_sequencer` database on Cloud SQL,
private IP `10.123.0.2`). The two tables that matter:

```sql
-- Who's heartbeating, when did we last hear from them
SELECT node_id, address, last_updated FROM nodes ORDER BY node_id;

-- Who currently holds the leader lock
SELECT node_id, last_updated, leader_acquired_at FROM sequencer_leader;
```

Connect from any sequencer VM:

```sh
PG_PASS=$(gcloud secrets versions access latest --secret=cloudsql-ligate-sequencer-db-app)
PGPASSWORD="$PG_PASS" psql -h 10.123.0.2 -U ligate_sequencer -d ligate_sequencer
```

Healthy steady state:
- `nodes` shows every active node with `last_updated` within the last
  heartbeat-interval (~15s by default).
- `sequencer_leader` shows exactly one row whose `node_id` matches the
  expected Leader and whose `last_updated` is fresh.

Unhealthy signals:
- A node missing from `nodes` → not heartbeating (process down or
  Postgres unreachable from that VM).
- `sequencer_leader.last_updated` older than the lock TTL → Leader has
  silently stopped renewing; expect election to fire shortly.
- `sequencer_leader` empty for more than a few seconds → no Leader,
  cluster is not producing batches.

---

## Prerequisite: `bind_host` flip for fan-out routing

The canonical bundle ships with `bind_host = "127.0.0.1"` in
`[runner.http_config]` of `celestia.toml`. That's safe for solo
operators but **incompatible** with the gateway-VM Caddyfile in
`ops/caddy/Caddyfile`, which fans writes out to all 3 sequencer VMs
over the VPC. Flip the bind on every multi-sequencer VM before
deploying the leader-aware Caddyfile:

```sh
# On each VM (VM-1, VM-2, VM-3)
sudo sed -i 's/^bind_host = "127.0.0.1"/bind_host = "0.0.0.0"/' /opt/ligate/devnet-1/celestia.toml
sudo systemctl restart ligate-node

# Verify
sudo ss -tlnp | grep 12346
# Expect: LISTEN ... 0.0.0.0:12346 ... ligate-node
```

The GCP firewall already restricts port 12346 to VPC peers (default
deny + per-VPC allow rule). No public exposure occurs.

For a fresh VM coming up via cloud-init, the bind flip is currently a
manual post-boot step: cloud-init runs the render script and writes
the canonical `127.0.0.1` value, then an operator follows up with the
`sed` above. Wiring `LIGATE_BIND_HOST` into `render-rollup-toml.sh` is
a small follow-up (open as a sub-issue of chain#422 if it bites again).

---

## Procedure A: Planned Leader replacement (zero-downtime upgrade)

Use this when rolling a binary upgrade on the active node. The Replica
becomes Leader for the duration of the upgrade, then we either let it
stay (cluster heals to N-1 + N+1) or hand authority back.

**Pre-flight.** Confirm a healthy Replica exists and is fully synced:

```sh
# Heartbeats fresh on all nodes
PGPASSWORD="$PG_PASS" psql -h 10.123.0.2 -U ligate_sequencer -d ligate_sequencer \
  -c "SELECT node_id, EXTRACT(EPOCH FROM (NOW() - last_updated)) AS age_s FROM nodes;"

# Replicas' head heights match Leader's (within a few blocks)
for vm in ligate-devnet-1-sequencer ligate-devnet-1-sequencer-2 ligate-devnet-1-sequencer-3; do
  echo "=== $vm ==="
  gcloud compute ssh "$vm" --zone=us-central1-a --command='curl -s http://127.0.0.1:9100/metrics | grep ^ligate_block_height'
done
```

1. **Promote a Replica.** On the chosen takeover node, edit
   `/opt/ligate/devnet-1/celestia.toml` and change `node_role = "Replica"`
   → `node_role = "DbElected"`. Restart:
   ```sh
   sudo systemctl restart ligate-node
   ```
   Watch `sequencer_leader`; it will not change yet because the current
   Leader still holds the lock. The promoted node is now in the election
   pool.

2. **Stop the current Leader gracefully.**
   ```sh
   sudo systemctl stop ligate-node
   ```
   Within one election cycle (lock TTL, typically 30s) the promoted
   Replica should win:
   ```sh
   PGPASSWORD="$PG_PASS" psql -h 10.123.0.2 -U ligate_sequencer -d ligate_sequencer \
     -c "SELECT * FROM sequencer_leader;"
   ```

3. **Upgrade + restart the old Leader.** While the new Leader produces,
   the old Leader's VM is free to upgrade. Set `LIGATE_TAG=vX.Y.Z` in
   `/etc/ligate/env`, re-run the install path, and start the service.
   It rejoins as a competing candidate (since its `node_role` is still
   `DbElected`).

4. **(Optional) Hand authority back.** If the historical Leader should
   stay primary, demote the takeover node back to `Replica` and restart
   it. The old Leader wins the next election cycle.

**Time budget.** Lock TTL + restart of takeover node + propagation ≈
30–60s of no-batch-production. Public RPC (load-balanced) keeps serving
reads throughout; `submit_tx` will return 503 from any node not currently
holding the lock.

---

## Procedure B: Unplanned Leader failure (crash, partition)

The auto-recovery story. Triggered by either:
- Leader process crash (systemd will restart it; if restart loops, see
  Procedure C).
- Leader VM unreachable for longer than the lock TTL.

The cluster handles this **without operator intervention** when at least
one healthy `DbElected` node remains:

1. Leader stops renewing its lock entry in `sequencer_leader`.
2. After the lock TTL elapses, a remaining `DbElected` node wins the
   next acquire-attempt and starts producing.
3. The dead Leader's `nodes` row goes stale; an operator should
   investigate but the cluster is already producing.

**What the operator does:**

1. **Confirm a new Leader took over.**
   ```sh
   PGPASSWORD="$PG_PASS" psql -h 10.123.0.2 -U ligate_sequencer -d ligate_sequencer \
     -c "SELECT node_id, EXTRACT(EPOCH FROM (NOW() - last_updated)) AS leader_age_s FROM sequencer_leader;"
   ```
   `leader_age_s` should be small (< heartbeat-interval). If
   `sequencer_leader` is **empty**, the cluster is not producing → escalate
   to Procedure C (forced takeover).

2. **Triage the dead node.**
   ```sh
   gcloud compute ssh <dead-node> --zone=us-central1-a --command='sudo journalctl -u ligate-node --since "10 minutes ago" --no-pager | tail -100'
   ```
   Common causes: OOM (check `dmesg | tail`), Celestia light-node lost
   sync (`sudo systemctl status celestia-light`), disk full (`df -h`).

3. **Recover the dead node.** Once root cause is addressed, bring the
   binary back up. It will rejoin as a competing `DbElected` node, but
   the new Leader will continue producing until the next election cycle
   chooses otherwise.

4. **Decide on Leader stickiness.** If the original Leader was a
   designated "primary" (e.g. better hardware, lower DA-RTT), demote the
   takeover node to `Replica` after the original is healthy. The
   original will reclaim the lock.

---

## Procedure C: Forced takeover (no automatic failover happening)

Used when:
- All `DbElected` nodes are unhealthy and Replicas need to be promoted
  by hand.
- `sequencer_leader` is empty for longer than ~2 minutes and the cluster
  is wedged.

1. **Identify the most-synced Replica.** Pick the one whose
   `ligate_block_height` is highest:
   ```sh
   for vm in ligate-devnet-1-sequencer ligate-devnet-1-sequencer-2 ligate-devnet-1-sequencer-3; do
     echo -n "$vm: "
     gcloud compute ssh "$vm" --zone=us-central1-a --command='curl -s http://127.0.0.1:9100/metrics | grep ^ligate_block_height' 2>/dev/null
   done
   ```

2. **Promote it.** On that VM:
   ```sh
   sudo sed -i 's/^node_role = "Replica"/node_role = "DbElected"/' /opt/ligate/devnet-1/celestia.toml
   # Also flip binary mode if it was a Follower
   sudo sed -i 's/^LIGATE_MODE=follower/LIGATE_MODE=sequencer/' /etc/ligate/env
   sudo systemctl restart ligate-node
   ```

3. **Verify.** Within one lock-TTL cycle:
   ```sh
   PGPASSWORD="$PG_PASS" psql -h 10.123.0.2 -U ligate_sequencer -d ligate_sequencer \
     -c "SELECT * FROM sequencer_leader;"
   ```
   `sequencer_leader.node_id` should match the promoted node. Confirm
   block height starts advancing via Grafana `ligate-node` dashboard
   (filter `$instance` to the promoted node) or
   `curl -s http://127.0.0.1:9100/metrics | grep ligate_block_height`.

4. **DA-key prerequisite.** The promoted node MUST have
   `da.signer_private_key` set in `celestia.toml`; without it batch
   submission to Celestia will fail. On devnet-1 this is currently
   fetched from Secret Manager by cloud-init for every VM. If you spun
   up a Follower without provisioning the DA key, fix that first (see
   [`celestia-light-node.md`](./celestia-light-node.md)).

---

## Procedure D: Postgres outage

Cloud SQL HA failover is automatic and typically takes 60–120s. During
that window:

1. The current Leader cannot renew its lock → it should stop producing
   batches once its in-memory TTL view expires. (Tested in unit tests;
   **not** yet validated end-to-end on devnet-1.)
2. All nodes will surface Postgres errors in logs:
   ```
   ERROR sov_sequencer::preferred::db: failed to renew leader lock ...
   ```
3. No election can happen until Postgres comes back.

**Operator action:** none required if HA failover succeeds. If Cloud SQL
is down for longer than ~5 minutes, escalate per the standard GCP
outage playbook. But note that **this is the single point of failure
in the current devnet-1 multi-sequencer story**. It is acceptable on
devnet (we control the SLA + failure surface); pre-mainnet we need a
plan B (etcd / dedicated quorum). Tracked at
[chain#82](https://github.com/ligate-io/ligate-chain/issues/82).

---

## Validation status

What we have **live-tested** on devnet-1 as of this runbook's first
write:

- Replica registration in `nodes` (VM-2, FOLLOWER binary mode + `node_role=Replica`).
- Heartbeat task runs in both `Sequencer` and `Follower` binary modes.
- Postgres connection via SSL (`sslmode=require`) against Cloud SQL
  private IP works from VMs in the same VPC.

What is **SDK-documented but not yet exercised live**:

- Automatic Leader → Replica failover under crash (Procedure B). Smoke-
  harness test in [`tests/smoke/multi-sequencer/`](../../../tests/smoke/multi-sequencer/) validates the
  election + heartbeat path but on MockDa, not Celestia (tracked at
  [chain#420](https://github.com/ligate-io/ligate-chain/issues/420)).
- Lock TTL behaviour under Postgres maintenance failover (Procedure D).
- DA contention if two nodes briefly both believe they are Leader (we
  have not deliberately split-brained the cluster).
- Caddy leader-aware fan-out in [`ops/caddy/Caddyfile`](../../../ops/caddy/Caddyfile)
  (write traffic routes to whichever upstream returns `BatchProducer`
  on `/v1/sequencer/role`). Probed manually but not exercised under
  load or during a real failover.

**Next planned drill.** Once VM-3 is fully synced and promoted to
`DbElected`, kill VM-1's ligate-node process and time the failover. Open
a follow-up issue with the measured numbers.

---

## Open follow-ups

- [chain#82](https://github.com/ligate-io/ligate-chain/issues/82): multi-sequencer umbrella.
- [chain#420](https://github.com/ligate-io/ligate-chain/issues/420): upgrade Docker Compose smoke harness to use local-celestia-devnet (currently MockDa only).
- [chain#422](https://github.com/ligate-io/ligate-chain/issues/422): devnet-1 rollout, this runbook is deliverable F.
- [chain#426](https://github.com/ligate-io/ligate-chain/issues/426): drop signer-key requirement for Follower mode (would unblock running cold Replicas without DA-signing capability).
